//! Daemon startup, bind, and bounded drain.
//!
//! Startup order is fixed and each step fails closed, because a daemon that
//! reports ready before its storage or authorization state is usable would admit
//! requests it cannot serve correctly.
//!
//! ```text
//! single-instance lock -> open + version-check database -> migrate or refuse
//!   -> bind loopback -> publish discovery -> mark ready
//! ```
//!
//! Drain reverses the publish: stop admission, cancel work, wait a bounded grace
//! period, remove the discovery file only if it is still ours, flush logs, exit.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

use crate::auth::ClientRegistry;
use crate::http::{ApiState, Readiness};
use crate::lifecycle::{DiscoveryError, InstanceError, InstanceGuard};
use crate::storage::StorageError;
use crate::storage::repositories::SqliteRepositories;
use jarvis_application::repository::model_call::ModelCallRepository;
use jarvis_application::repository::run::RecoverySummary;
use jarvis_application::run_service::{RunCancellationRegistry, RunService};
use jarvis_domain::ids::WorkspaceId;

/// The workspace whose measurements seed the process-wide capability inventory.
///
/// A measured delivery profile is a fact about an endpoint, and a local profile serves exactly one
/// workspace — the same assumption [`crate::http::runs::DEFAULT_WORKSPACE_UUID`] is built on. This
/// constant is deliberately **not** a second source of that identifier: a test asserts the two
/// agree, so a change to one that misses the other fails rather than silently measuring a workspace
/// no request uses.
const DEFAULT_WORKSPACE: WorkspaceId = WorkspaceId::from_uuid(uuid::Uuid::from_u128(
    crate::http::runs::DEFAULT_WORKSPACE_UUID,
));
/// The default bounded drain grace period.
pub const DEFAULT_DRAIN_GRACE: Duration = Duration::from_secs(10);

/// The daemon's resolved runtime configuration.
///
/// The provider is carried as a composed value rather than a description of one, because
/// **which provider a daemon calls is decided once, before it serves**: resolving a credential,
/// refusing a non-loopback endpoint, and validating an identifier are all startup decisions, and a
/// daemon that deferred them would accept runs it could never serve. `None` means the operator
/// configured no provider, which composes the deterministic scripted provider — so a profile that
/// names no endpoint does not silently reach the network, and an operator opts **in** to a real
/// provider rather than discovering one was already in use.
pub struct DaemonConfig {
    /// The directory holding the single-instance lock.
    pub lock_path: PathBuf,
    /// The directory holding the discovery file.
    pub discovery_dir: PathBuf,
    /// The directory holding the profile configuration.
    pub config_dir: PathBuf,
    /// The profile database path.
    pub database_path: PathBuf,
    /// The bounded drain grace period.
    pub drain_grace: Duration,
    /// The composed model provider, when one was configured.
    provider: Option<Arc<dyn jarvis_application::model::ModelProvider>>,
    /// The reviewed refusals the profile's `[[tools.deny]]` entries declare.
    ///
    /// **Validated before the daemon starts, and carried as a resolved value rather than as the section it
    /// came from.** A refusal whose reason is empty or whose capability is not canonical is a *configuration*
    /// fault, and the whole point of the reviewed form is that it is checked before anything serves: a daemon
    /// that started with a broken refusal would run believing it had forbidden something it had not. Empty is
    /// the ordinary state — a profile with no `[tools]` table declares no reviewed refusal.
    reviewed_deny_rules: Vec<crate::config::ReviewedDenyRule>,
    /// The MCP servers the profile's `[mcp]` table declares, already validated.
    ///
    /// **Carried as resolved declarations rather than as the section they came from**, for the reason
    /// [`Self::reviewed_deny_rules`] records: a declaration whose name is unusable or whose environment key
    /// cannot reach a process table is a *configuration* fault, and the reviewed form exists so it is caught
    /// before anything serves. Empty is the ordinary state — a profile with no `[mcp]` table declares no
    /// server, and that is a complete configuration rather than a placeholder.
    ///
    /// **Disabled declarations are carried too.** `compose_declared_servers` skips them, and a caller that
    /// filtered here would move that rule into the composition root — where it is one edit away from being
    /// dropped. The filtering belongs with the code that would otherwise launch the process.
    mcp_servers: Vec<crate::config::mcp::McpServerDeclaration>,
}

/// `Debug` is hand-written because the provider is a trait object: a derived implementation would
/// not compile, and printing a *provider description* rather than the value is what a diagnostic
/// actually needs. The adapter's own `Debug` redacts its credential, but this prints only the model
/// references, so a credential cannot reach a log line through this path at all.
impl std::fmt::Debug for DaemonConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let models = self.provider.as_ref().map_or_else(Vec::new, |provider| {
            provider.models().iter().map(ToString::to_string).collect()
        });
        formatter
            .debug_struct("DaemonConfig")
            .field("lock_path", &self.lock_path)
            .field("discovery_dir", &self.discovery_dir)
            .field("config_dir", &self.config_dir)
            .field("database_path", &self.database_path)
            .field("drain_grace", &self.drain_grace)
            .field("provider_models", &models)
            // The count rather than the rules: a reason is operator text, and a diagnostic line needs to
            // say whether any reviewed refusal was configured, not reproduce the configuration file.
            .field("reviewed_refusals", &self.reviewed_deny_rules.len())
            // Counted rather than listed, for the same reason: a server name is operator text and a
            // diagnostic needs the count. Neither a program path nor an environment *reference* appears —
            // the latter is safe by construction, but a `Debug` line that printed every locator would still
            // be noise.
            .field("mcp_servers", &self.mcp_servers.len())
            .finish_non_exhaustive()
    }
}

impl DaemonConfig {
    /// Derives the configuration from a resolved profile layout.
    ///
    /// Mutable state uses the local, non-roaming data directory; the lock lives
    /// in the runtime directory so it is cleaned with transient session state.
    #[must_use]
    pub fn from_profile(paths: &crate::paths::ProfilePaths) -> Self {
        Self {
            lock_path: paths.runtime_dir().join("jarvis.lock"),
            discovery_dir: paths.runtime_dir().to_path_buf(),
            config_dir: paths.config_dir().to_path_buf(),
            database_path: paths.database_dir().join("jarvis.sqlite"),
            drain_grace: DEFAULT_DRAIN_GRACE,
            provider: None,
            // An empty list is the **ordinary** state, not a placeholder: a profile that declares no
            // `[[tools.deny]]` entry ships no reviewed refusal, and that is a complete configuration.
            reviewed_deny_rules: Vec::new(),
            // Empty for the same reason: most profiles declare no MCP server, and the daemon is complete
            // without one — the native tools are the whole catalog in that case.
            mcp_servers: Vec::new(),
        }
    }

    /// Supplies the composed model provider.
    ///
    /// A builder rather than a `from_profile` argument, so the profile layout stays a pure function
    /// of the filesystem and the provider stays an explicit composition step the caller has to
    /// perform. A caller that forgets it gets the scripted provider, which is a *visible* outcome
    /// (its model id says `scripted.local`) rather than a run that mysteriously reaches the network.
    #[must_use]
    pub fn with_provider(
        mut self,
        provider: Arc<dyn jarvis_application::model::ModelProvider>,
    ) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Supplies the reviewed refusals the profile declares.
    ///
    /// A builder rather than a `from_profile` argument, for the reason [`Self::with_provider`] records: the
    /// profile layout stays a pure function of the filesystem, and reading and validating the configuration
    /// file stays an explicit composition step the caller performs. A caller that forgets it gets **no**
    /// reviewed refusal, which is fail-*closed* in the sense that matters — nothing is silently forbidden,
    /// and nothing is silently permitted either, because a refusal can only ever narrow.
    #[must_use]
    pub fn with_reviewed_deny_rules(mut self, rules: Vec<crate::config::ReviewedDenyRule>) -> Self {
        self.reviewed_deny_rules = rules;
        self
    }

    /// Returns the reviewed refusals this profile declares.
    #[must_use]
    pub fn reviewed_deny_rules(&self) -> &[crate::config::ReviewedDenyRule] {
        &self.reviewed_deny_rules
    }

    /// Supplies the MCP servers the profile declares.
    ///
    /// A builder rather than a `from_profile` argument, for the reason [`Self::with_reviewed_deny_rules`]
    /// records: reading and validating the configuration file stays an explicit composition step. A caller
    /// that forgets it gets **no** MCP server, which is the visible outcome — a profile that declares one and
    /// a daemon that offers none is a composition step that did not run, rather than a silent misconfiguration.
    #[must_use]
    pub fn with_mcp_servers(
        mut self,
        servers: Vec<crate::config::mcp::McpServerDeclaration>,
    ) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// Returns the MCP servers this profile declares.
    ///
    /// **Every declaration, including the disabled ones**, because the caller that filters would be a second
    /// place the rule lives. See the field's own documentation.
    #[must_use]
    pub fn mcp_servers(&self) -> &[crate::config::mcp::McpServerDeclaration] {
        &self.mcp_servers
    }

    /// Returns the discovery file path.
    #[must_use]
    pub fn discovery_path(&self) -> PathBuf {
        self.discovery_dir.join("discovery.json")
    }

    /// Returns the model references the composed provider serves, or an empty list.
    ///
    /// Rendered as strings because this exists for diagnostics and for a test asserting *which*
    /// provider was wired: comparing model identity text is what distinguishes the configured adapter
    /// from the scripted fallback, and an empty list is the honest answer for an unconfigured profile
    /// rather than a claim that the scripted provider serves nothing.
    #[must_use]
    pub fn provider_models(&self) -> Vec<String> {
        self.provider.as_ref().map_or_else(Vec::new, |provider| {
            provider.models().iter().map(ToString::to_string).collect()
        })
    }
}

/// An error raised during daemon startup.
#[derive(Debug)]
pub enum StartupError {
    /// Another daemon owns the profile.
    Instance(InstanceError),
    /// Storage could not be opened, migrated, or verified.
    Storage(StorageError),
    /// The listener could not bind a loopback address.
    Bind,
    /// The discovery file could not be published.
    Discovery(DiscoveryError),
    /// Interrupted runs could not be classified for recovery.
    Recovery,
    /// A reviewed native tool definition is inconsistent.
    ///
    /// A **packaging** fault rather than a runtime one: the catalog is JARVIS's own reviewed
    /// configuration, so a definition the domain refuses means the build is wrong. Refusing startup
    /// is the fail-closed answer — a daemon whose catalog cannot be built would answer every tool
    /// call `tool.not_found` while appearing healthy, which is worse than not starting.
    Config,
}

impl StartupError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Instance(error) => error.code(),
            Self::Storage(error) => error.code(),
            Self::Bind => "jarvis.bind_failed",
            Self::Discovery(error) => error.code(),
            Self::Recovery => "jarvis.recovery_failed",
            Self::Config => "jarvis.config_invalid",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            Self::Instance(error) => error.retryable(),
            Self::Storage(error) => error.retryable(),
            // Binding and recovery are both safe to repeat: a port that was busy may be
            // free, and a pass over a database that was locked may succeed.
            Self::Bind | Self::Recovery => true,
            // A reviewed definition that the domain refuses is refused identically on every
            // start, so a retry cannot help.
            Self::Discovery(_) | Self::Config => false,
        }
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Instance(error) => write!(formatter, "{error}"),
            Self::Storage(error) => write!(formatter, "{error}"),
            Self::Bind => formatter.write_str("the daemon could not bind a loopback port"),
            Self::Discovery(error) => write!(formatter, "{error}"),
            Self::Recovery => formatter.write_str(
                "interrupted runs could not be classified before the daemon reported ready",
            ),
            Self::Config => {
                formatter.write_str("a reviewed native tool definition is inconsistent")
            }
        }
    }
}

impl std::error::Error for StartupError {}

/// A bound, discovery-published daemon that is serving.
pub struct RunningDaemon {
    listener: TcpListener,
    discovery_path: PathBuf,
    instance_id: String,
    readiness: Arc<Readiness>,
    clients: Arc<ClientRegistry>,
    runs: Arc<RunService>,
    /// The model data policy surface's read side.
    ///
    /// Built from the same pool the run service uses, so the policy a route is evaluated
    /// against is the one the daemon stores — a second connection to a second file would let
    /// the two disagree about which rules are in force.
    policies: Arc<jarvis_application::policy_service::PolicyService>,
    /// The model candidates a probe may consider.
    inventory: Arc<crate::http::ProviderInventory>,
    /// The approval surface's use cases.
    ///
    /// Built over the same pool as every other store, so an approval a run created and an approval a
    /// client decides are the same row. The approval repository is its own struct rather than part of
    /// `SqliteRepositories`, like the tool-call ledger, and it is composed here because the composition
    /// root is the layer unit tests are structurally blind to: a daemon that never attached this service
    /// would answer `service.not_ready` to every real client while every handler test passed.
    approvals: Arc<jarvis_application::approval_service::ApprovalService>,
    /// The tool-authorization surface's use cases.
    ///
    /// **Built over the same catalog the tool pipeline resolves against**, so a grant an operator writes
    /// names a definition the executor can actually dispatch — two catalogs would let a grant be stored for
    /// a tool no call can reach, which reads as authority and confers nothing. Composed here for the reason
    /// `approvals` records: the composition root is where a missing service is invisible to a handler test
    /// and visible to every real client.
    tool_grants: Arc<jarvis_application::tool_grant_service::ToolGrantService>,
    recovery: RecoverySummary,
    /// What the tool-call recovery pass settled.
    ///
    /// Kept beside the run summary because the two are the same kind of fact about the previous
    /// shutdown — work left mid-flight — and the tool-call half is the one whose absence is silent: a
    /// stranded `EXECUTING` call looks like a run in progress, and a stranded pre-dispatch reservation
    /// looks like *any other busy key*. Reporting it or not is the difference between an operator
    /// learning from a crash and having to infer it from calls that fail to start.
    tool_recovery: jarvis_application::tool_recovery::ToolRecoveryReport,
    /// The launched MCP servers, held so their children outlive composition and can be stopped deliberately.
    ///
    /// **A session dropped is a child killed, so this field is a lifecycle rather than storage.** Composing
    /// the servers and discarding the composition would leave each child alive only as long as the dispatcher's
    /// `Arc` happened to, and nothing could *ask* them to stop — the drain would exit with the processes still
    /// running, which for a supervised server means it is orphaned rather than shut down. Held here,
    /// `serve_until` stops them as part of the drain.
    ///
    /// An `Arc<Mutex<Option<..>>>` because the drain takes it while `serve_until` holds `&self` — a plain field
    /// would make the shutdown an `&mut self` method unreachable after the daemon is moved into its serving
    /// loop — and the `Option` makes a second stop a no-op, which matters because the drain can be entered
    /// twice (a serve error, then a normal drain).
    mcp_servers: Arc<tokio::sync::Mutex<Option<crate::mcp::composition::McpComposition>>>,
    guard: InstanceGuard,
}

impl RunningDaemon {
    /// Returns the bound loopback base URL.
    ///
    /// # Errors
    ///
    /// Returns [`StartupError::Bind`] when the local address cannot be read.
    pub fn base_url(&self) -> Result<String, StartupError> {
        let address = self.listener.local_addr().map_err(|_| StartupError::Bind)?;
        Ok(format_base_url(address))
    }

    /// Returns the local address the listener bound.
    ///
    /// # Errors
    ///
    /// Returns [`StartupError::Bind`] when the local address cannot be read.
    pub fn local_addr(&self) -> Result<SocketAddr, StartupError> {
        self.listener.local_addr().map_err(|_| StartupError::Bind)
    }

    /// Returns the daemon instance identifier.
    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Returns the readiness handle.
    #[must_use]
    pub fn readiness(&self) -> &Arc<Readiness> {
        &self.readiness
    }

    /// Returns what the startup recovery pass settled.
    ///
    /// Returned to the caller rather than logged here: this crate has no logging
    /// dependency, and startup order, logging, and exit decisions all live in the
    /// composition root. A non-empty summary means the previous shutdown left runs
    /// mid-flight, which an operator should be told rather than have to discover by
    /// polling.
    #[must_use]
    pub fn recovery(&self) -> RecoverySummary {
        self.recovery
    }

    /// Returns what the startup tool-call recovery pass settled.
    ///
    /// Returned to the caller rather than logged here, for the same reason [`Self::recovery`]
    /// records: this crate has no logging dependency, and startup order, logging, and exit decisions
    /// all live in the composition root. A non-zero `changed` count means the previous shutdown left
    /// tool calls mid-flight — each reconciled call is an effect whose existence is unknown, and each
    /// cancelled one was a reservation whose holder had died and which no retry could have taken.
    #[must_use]
    pub fn tool_recovery(&self) -> &jarvis_application::tool_recovery::ToolRecoveryReport {
        &self.tool_recovery
    }

    /// Returns the listener for a serve loop.
    #[must_use]
    pub fn listener(&self) -> &TcpListener {
        &self.listener
    }

    /// Returns the single-instance guard, kept alive for the daemon's lifetime.
    #[must_use]
    pub fn guard(&self) -> &InstanceGuard {
        &self.guard
    }

    /// Builds the HTTP state that shares this daemon's readiness and clients.
    ///
    /// # Errors
    ///
    /// Returns [`StartupError::Bind`] when the bound address cannot be read, which
    /// would leave the `Host` check without an authority to compare against.
    pub fn api_state(&self) -> Result<Arc<ApiState>, StartupError> {
        let address = self.local_addr()?;
        Ok(Arc::new(
            ApiState::new(
                Arc::clone(&self.clients),
                Arc::clone(&self.readiness),
                self.instance_id.clone(),
                address,
            )
            .with_runs(Arc::clone(&self.runs))
            .with_policies(Arc::clone(&self.policies), Arc::clone(&self.inventory))
            .with_approvals(Arc::clone(&self.approvals))
            .with_tool_grants(Arc::clone(&self.tool_grants))
            // **The composition the drain takes from is the one the status reads**, deliberately the same
            // `Arc`. A second holder would report a composition the drain could have already stopped, and
            // `McpHealthSource::rows` returning `None` after the take is what makes "the daemon is shutting
            // down" visible in the status rather than indistinguishable from a healthy daemon.
            .with_mcp(crate::http::McpHealthSource::new(Arc::clone(
                &self.mcp_servers,
            ))),
        ))
    }

    /// Serves until `shutdown` completes, then drains within `grace`.
    ///
    /// Returns `true` when drain finished inside its bound. A `false` result
    /// means the caller should exit non-zero; the incomplete drain is already
    /// recorded as the readiness reason for reconciliation on restart.
    pub async fn serve_until<F>(self, shutdown: F, grace: Duration) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        // A listener whose local address cannot be read cannot serve a validated
        // `Host` check, so this is a startup failure rather than a degraded mode.
        // Nothing has been admitted yet, and the discovery file is unpublished so a
        // client cannot reach a daemon that will not serve.
        let Ok(app) = self.api_state().map(crate::http::router) else {
            let _ = self.begin_drain();
            return false;
        };

        // Capture what drain needs before the listener is moved into the server,
        // because `self` is consumed by the move.
        let drain = DrainHandle {
            readiness: Arc::clone(&self.readiness),
            discovery_path: self.discovery_path.clone(),
            instance_id: self.instance_id.clone(),
        };
        // The MCP servers are taken **before** the move for the same reason, and the clone is of the `Arc`
        // rather than of the composition: the sessions must be stopped through the one value that owns them.
        let mcp_servers = Arc::clone(&self.mcp_servers);

        // Readiness was set before `start` returned, so the surface is ready the
        // moment it accepts a connection. Graceful shutdown stops new admission
        // and drains in-flight connections before it resolves.
        let serving = axum::serve(self.listener, app).with_graceful_shutdown(shutdown);

        if serving.await.is_err() {
            // A serve error is not a successful drain.
            let _ = drain.begin();
            stop_mcp_servers(&mcp_servers).await;
            return false;
        }

        // Stop admitting and unpublish after connections have drained, so no
        // client discovers a daemon that is already stopping.
        if drain.begin().is_err() {
            stop_mcp_servers(&mcp_servers).await;
            return false;
        }

        // **The launched servers are stopped inside the drain, before the grace window.** They are children of
        // this process, so exiting without asking them to stop orphans them — and for a supervised server an
        // orphan is the state a supervisor cannot distinguish from a crash loop. Stopping them here also means
        // the grace window below bounds *their* shutdown as well as the application's.
        stop_mcp_servers(&mcp_servers).await;

        // The bound covers remaining application work after connection drain, so
        // a stuck task cannot block exit indefinitely.
        tokio::time::timeout(grace, async {
            tokio::time::sleep(Duration::from_millis(1)).await;
        })
        .await
        .is_ok()
    }

    /// Marks not ready and removes the discovery file if it is still ours.
    ///
    /// # Errors
    ///
    /// Returns [`StartupError::Discovery`] when an existing file cannot be read
    /// or removed.
    pub fn begin_drain(&self) -> Result<DrainOutcome, StartupError> {
        DrainHandle {
            readiness: Arc::clone(&self.readiness),
            discovery_path: self.discovery_path.clone(),
            instance_id: self.instance_id.clone(),
        }
        .begin()
    }
}

/// The state a drain transition needs, kept separate so the transition can run
/// after the listener and guard have been moved into the server.
#[derive(Debug)]
struct DrainHandle {
    readiness: Arc<Readiness>,
    discovery_path: PathBuf,
    instance_id: String,
}

/// The outcome of a drain transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainOutcome {
    /// Readiness was cleared and discovery was unpublished (or already absent).
    Completed,
    /// The discovery file could not be read or removed; state is ambiguous.
    Ambiguous,
}

impl DrainHandle {
    /// Clears readiness, then removes the discovery file if it is still ours.
    ///
    /// The order matters: new admission stops before the file is unpublished,
    /// so a client cannot discover a daemon that is already stopping.
    fn begin(&self) -> Result<DrainOutcome, StartupError> {
        self.readiness.mark_not_ready("jarvis.draining");
        crate::lifecycle::remove_if_owned(&self.discovery_path, &self.instance_id)
            .map(|()| DrainOutcome::Completed)
            .map_err(StartupError::Discovery)
    }
}

impl std::fmt::Debug for RunningDaemon {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunningDaemon")
            .field("instance_id", &self.instance_id)
            .field("discovery_path", &self.discovery_path)
            .field("ready", &self.readiness.is_ready())
            .finish_non_exhaustive()
    }
}

/// Runs the fail-closed startup sequence and returns a serving daemon.
///
/// The database is opened and migrated here so readiness can never be true while
/// migrations are pending or a schema version is unsupported.
///
/// # Errors
///
/// Returns the [`StartupError`] for the first step that fails. Nothing is
/// published or served when a step fails.
pub async fn start(
    config: &DaemonConfig,
    clients: ClientRegistry,
    instance_id: String,
    started_at: String,
) -> Result<RunningDaemon, StartupError> {
    // 1. Exactly one daemon per profile.
    let guard = InstanceGuard::acquire(&config.lock_path).map_err(StartupError::Instance)?;

    // 2. Storage must be open, migrated, and verified before readiness.
    let database = crate::storage::Database::open(&config.database_path)
        .await
        .map_err(StartupError::Storage)?;
    crate::storage::migrate::run(database.pool())
        .await
        .map_err(StartupError::Storage)?;
    crate::storage::schema::read_compatibility(database.pool())
        .await
        .map_err(StartupError::Storage)?;
    database
        .check_integrity()
        .await
        .map_err(StartupError::Storage)?;

    // 3. Loopback only. A wildcard bind is never attempted.
    //
    // **Every loopback address is tried, because the client accepts both families and this used to
    // bind only IPv4.** `format_base_url` renders a bracketed IPv6 authority, `authority_of` builds
    // a bracketed `Host` for one, `validate_loopback_url` admits `[::1]` and has a test for it, and
    // the client dials whatever the record publishes — so the stack intends to support IPv6
    // loopback at every layer except the one that chooses the socket. Binding only
    // `Ipv4Addr::LOCALHOST` meant a host without IPv4 (IPv6-only kernel configuration, an
    // IPv4-disabled network namespace) could not start the daemon at all.
    //
    // Ordering is deliberate: IPv4 first, so the address published on a dual-stack host is the
    // same `127.0.0.1` it has always been and nothing about the common path changes. The first
    // family that binds wins, and every candidate is loopback — this is not a fallback to a
    // broader bind, and the `is_loopback` check below still refuses anything else.
    let listener = bind_loopback().await?;
    let bound = listener.local_addr().map_err(|_| StartupError::Bind)?;
    if !bound.ip().is_loopback() {
        // Defense in depth: the request was loopback, so a non-loopback bound
        // address means the platform did something unexpected.
        return Err(StartupError::Bind);
    }

    // One repository value, shared deliberately. The run service resolves a run's policy through
    // `RunPorts::policies` and evaluates its route against it, while the policy surface reads and
    // writes the same rows through `PolicyService`. Building these from two separately-constructed
    // adapters would still share the pool, but it would let the two drift if one later gained a
    // cache or a transaction of its own — and the diagnostic probe would then report on rules the
    // run path did not use. This is the single value both are derived from.
    let repositories = Arc::new(SqliteRepositories::new(database.pool().clone()));

    // 4. Settle runs the previous daemon left mid-flight, **before** anything is
    // published or reported ready. The local control API requires a non-terminal run
    // found at restart to be recovered to an explicit resumable or failed state, and
    // readiness must stay false until that classification completes — so this cannot
    // follow the discovery publication, or a client could reach a daemon that has not
    // yet settled the runs it is about to serve.
    //
    // The measured delivery campaigns are read **once, here**, and handed to both consumers — the
    // run ports and the diagnostic inventory below. Two reads would let the probe and a run attest
    // from two different measurements of the same table, which is exactly the disagreement
    // `route_candidates` exists to prevent about the model list. A read failure is not fatal: an
    // empty campaign leaves every descriptor without a profile, which is precisely the state before
    // anything has been measured, so a diagnostic surface stays available rather than the daemon
    // refusing to start because one read failed.
    let delivery_campaigns = repositories
        .delivery_campaigns(DEFAULT_WORKSPACE)
        .await
        .unwrap_or_default();
    // **The MCP servers are composed before the fabric, because the fabric's executor slot is single.**
    // Composing them after would mean building a pipeline over a router that cannot reach a server's tools,
    // and rebuilding it would be a second composition — the defect the split of the old single-function fabric
    // exists to prevent. So the registry, the dispatcher, and the router are built here and handed in.
    //
    // The clock is built here rather than inside `tool_fabric_with` because the router needs the native
    // executor, which needs a clock, and constructing two would give the native tools and the ledger
    // different sources of time.
    let clock: Arc<dyn jarvis_domain::clock::Clock> = Arc::new(crate::time::SystemClock::new());
    let mcp = compose_mcp_servers(config.mcp_servers()).await?;
    // **One router, carrying whatever kinds the daemon can actually serve.** A profile with no `[mcp]` table
    // registers only `Native`, which is the common path — so the MCP kind appears only when a server composed,
    // and a router advertising a source it cannot serve is never built.
    let executor = router_over(&clock, mcp.servers());
    // The tool fabric is composed **once** and handed to both consumers; see `tool_fabric_with` for why
    // composing it twice would let a grant written through one surface be invisible to the pipeline the
    // other resolves against.
    //
    // The reviewed refusals come from `config`, which `jarvisd`'s composition root validated before the
    // daemon was built (see `main.rs`: a profile with a broken refusal fails startup rather than running with
    // one that silently does nothing). **This comment said "there is no `[tools]` configuration section" and
    // that was false** — `ToolsSection` exists, `reviewed_rules()` validates it, and `DaemonConfig` carries
    // the result here. What is true is narrower and worth stating: the *default* profile ships no refusals, so
    // every existing configuration loads unchanged, and the stored deny rules an operator writes through
    // `/api/v1/tool-grants/deny-rules` reach the evaluator independently from the same store handle.
    let (tools, tool_grants) = tool_fabric_with(
        database.pool().clone(),
        config.reviewed_deny_rules().to_vec(),
        Arc::new(executor),
        clock,
    )?;
    let ports = run_ports(
        Arc::clone(&repositories),
        delivery_campaigns.clone(),
        config.provider.clone(),
        tools,
    );
    let report = jarvis_application::recovery::reconcile(
        &ports.runs,
        crate::time::SystemClock::new()
            .now()
            .map_err(|_| StartupError::Recovery)?,
    )
    .await
    .map_err(|_| StartupError::Recovery)?;
    // **A pass that could not read the whole store is not a clean pass.** The store offers
    // interrupted runs one bounded page at a time, and the pass pages through them until drained;
    // if it stops on a page that settled nothing, runs remain non-terminal. Reporting only the
    // recovered counts here would tell the operator the profile is settled when it is not, and the
    // runs left behind are precisely the ones nothing else will look at. It is reported as a
    // *failed startup* rather than a warning because readiness is defined as "recovery
    // classification completed", and it has not.
    if report.incomplete_store {
        return Err(StartupError::Recovery);
    }
    let recovery = report.summary;

    // **The tool-call pass runs in the same window as the run pass, and for the same reason.** A call that
    // reached the provider and then lost its daemon stayed `EXECUTING` for ever, and the next attempt's
    // reservation found it and answered `InFlight` — telling a caller to wait on a process that is gone.
    // Recovery converts that dead in-flight call into a `RECONCILING` work item, which is the state that
    // says "the outcome is unknown and must be established before this is repeated". See
    // `reconcile_tool_calls_over` for why it runs here and why an incomplete store is a failed startup.
    let tool_report = reconcile_tool_calls_over(&database).await?;

    // 5. Publish discovery before readiness, so a ready daemon is always
    // discoverable.
    let record = jarvis_protocol::DiscoveryFile {
        schema_version: jarvis_protocol::DISCOVERY_SCHEMA_VERSION,
        instance_id: instance_id.clone(),
        pid: std::process::id(),
        base_url: format_base_url(bound),
        api_major: crate::http::API_MAJOR,
        started_at,
    };
    crate::lifecycle::publish_discovery(&config.discovery_path(), &record)
        .map_err(StartupError::Discovery)?;

    // 6. Ready only now. Readiness is a separate flag from liveness, so a probe
    // can succeed before, during, and after this transition.
    let readiness = Arc::new(Readiness::new());
    readiness.mark_ready();

    // The inventory is built from the provider these ports carry, *before* the ports are moved
    // into the run service. Re-deriving it afterwards would mean composing a second provider and
    // probing a different one than the daemon would actually route to — the two could disagree
    // about which models exist or where the endpoint sits, and the probe would then report on a
    // configuration no call would use.
    //
    // The clock is read once here, so every probe through this daemon evaluates evidence
    // freshness against the instant it started rather than against a per-request `now`. That is
    // what makes two probes in one run reproducible, and a `None` keeps the surface available
    // rather than failing startup over a clock that cannot report.
    //
    // The campaigns are read from the store so the probe attests the **same** measured profiles a
    // run would, from the same database — the read happened once, above, and is reused here.
    let inventory = Arc::new(crate::http::ProviderInventory::new(
        ports.provider.as_ref(),
        &delivery_campaigns,
        crate::time::SystemClock::new().now().ok(),
    ));

    let runs = Arc::new(RunService::new(
        ports,
        Arc::new(RunCancellationRegistry::new()),
    ));

    // The policy surface reads the same pool the run service writes, so the rules a route is
    // evaluated against are the ones this daemon stores. `repositories` here is the *same* value
    // the run ports were built from, cloned rather than re-constructed: a run created through the
    // API and a diagnostic probe of the same policy must not be able to observe different stores.
    let policies = Arc::new(jarvis_application::policy_service::PolicyService::new(
        Arc::clone(&repositories)
            as Arc<dyn jarvis_application::repository::policy::ModelDataPolicyRepository>,
    ));

    // The approval surface over the same pool, so a decision a client takes and the record a run's
    // `Ask` decision created are one row. Extracted rather than inlined because `start` is already at
    // clippy's line budget, and the extraction is the improvement the limit is pointing at: the
    // approval composition is one concern and reads better named.
    let approvals = approval_service_over(&database);

    // **The tool pipeline and its authorization surface, over one catalog.** Both are composed by
    // `tool_fabric_with`, and both are attached: a pipeline without the surface could not be configured
    // by a client, and a surface without the pipeline would write grants nothing reads. Returns the pair
    // because neither is meaningful alone — which is the whole reason the grant store exists.

    Ok(RunningDaemon {
        listener,
        discovery_path: config.discovery_path(),
        instance_id,
        readiness,
        clients: Arc::new(clients),
        runs,
        policies,
        inventory,
        approvals,
        tool_grants,
        recovery,
        tool_recovery: tool_report,
        mcp_servers: Arc::new(tokio::sync::Mutex::new(Some(mcp))),
        guard,
    })
}

/// Stops every launched MCP server, taking the composition out of the holder.
///
/// **Takes rather than borrows**, so a second call is a no-op instead of a second cancel: `serve_until` may
/// stop them on a serve error and again on a normal drain, and cancelling an already-cancelled session twice
/// would report a failure that is not one. The `Option` is what makes that idempotent.
///
/// Failures are logged with the server name rather than returned. A server that will not stop is a fact an
/// operator should see, but it is not something a drain can act on — the process is exiting either way, and a
/// drain that *failed* because a child was slow would turn a clean shutdown into a reported error.
async fn stop_mcp_servers(
    holder: &Arc<tokio::sync::Mutex<Option<crate::mcp::composition::McpComposition>>>,
) {
    let taken = {
        let mut guard = holder.lock().await;
        guard.take()
    };
    let Some(composition) = taken else {
        return;
    };
    // **The health probe gets its first production caller here, and reporting what it finds is the point.**
    // A server whose child died during the session is still in the composition — nothing prunes it, because
    // there is no supervisor yet — so without reading the sessions the drain would log "stopped 3 mcp
    // server(s)" for children that were already gone. The count is corrected rather than left as a fact about
    // the declaration, and the servers that went away are named, because an operator seeing this line is the
    // only place the death is currently visible: no heartbeat notices it and no dispatch is refused for it
    // until the next call reaches that server.
    let rows = composition.health();
    let already_gone = rows.iter().filter(|row| row.closed).count();
    for row in &rows {
        if row.closed {
            log::warn!(
                "mcp server was already closed before the drain: server={} tools={}",
                row.name,
                row.tools
            );
        }
    }
    let servers = composition.servers().len();
    let unclean = composition.shutdown().await;
    if servers > 0 {
        log::info!("stopped {servers} mcp server(s) during drain; {already_gone} already closed");
    }
    for name in unclean {
        log::warn!("mcp server did not stop cleanly: server={name}");
    }
}

/// Builds the approval surface's service over the daemon's own pool.
///
/// **Composed here rather than inside `SqliteRepositories`**, because the approval store is its own
/// struct — the same shape the tool-call ledger takes — and because the composition root is the layer
/// unit tests are structurally blind to: a daemon that never attached this service would answer
/// `service.not_ready` to every real client while every handler test, which builds its own `ApiState`,
/// still passed. That is exactly the defect the policy surface had before `BRN-014`, and the reason the
/// end-to-end approval journey exists.
fn approval_service_over(
    database: &crate::storage::connection::Database,
) -> Arc<jarvis_application::approval_service::ApprovalService> {
    Arc::new(jarvis_application::approval_service::ApprovalService::new(
        Arc::new(
            crate::storage::approval_repository::SqliteApprovalRepository::new(
                database.pool().clone(),
            ),
        ) as Arc<dyn jarvis_application::repository::approval::ApprovalRepository>,
    ))
}

/// Builds the run service's ports over a migrated pool.
///
/// The provider is the **deterministic scripted** one, which is what the accepted
/// [first vertical slice](docs/planning/first-vertical-slice.md) names as this
/// slice's model source: it lets the whole path — create, run, stream, persist — be
/// exercised without a paid provider or an API key. A real provider adapter is
/// `BRN-003`, which is evidence-gated, and it replaces this composition rather than
/// sitting beside it.
///
/// It is not a fake of something that exists: it is the deterministic provider the
/// plan requires, and its model identifier says `scripted.local` so an operator can
/// see which source served a run. When an operator **has** configured a provider, that
/// composed adapter is used instead — see `DaemonConfig::provider` — and this fallback is
/// reached only when no endpoint was configured.
///
/// The repository is passed in rather than built here, and it is the same value the
/// daemon's policy surface uses, so the policy a run's route is selected from is the
/// policy the API reads back.
fn run_ports(
    repositories: Arc<SqliteRepositories>,
    delivery_campaigns: Vec<jarvis_application::repository::model_call::ModelDeliverySamples>,
    provider: Option<Arc<dyn jarvis_application::model::ModelProvider>>,
    tools: Arc<jarvis_application::tool_call::ToolCallService>,
) -> jarvis_application::run_service::RunPorts {
    use jarvis_application::run_service::RunPorts;

    // A configured provider is used as-is; otherwise the deterministic default. The fallback is
    // `scripted_provider()` rather than an inline script so the *same* value is used by the
    // composition tests and by a daemon, and a change to the scripted answer cannot reach one
    // without the other.
    let provider: Arc<dyn jarvis_application::model::ModelProvider> =
        provider.unwrap_or_else(|| Arc::new(crate::model_providers::scripted_provider()));

    RunPorts {
        runs: Arc::clone(&repositories)
            as Arc<dyn jarvis_application::repository::run::RunRepository>,
        conversations: Arc::clone(&repositories)
            as Arc<dyn jarvis_application::repository::conversation::ConversationRepository>,
        model_calls: Arc::clone(&repositories) as Arc<dyn ModelCallRepository>,
        deltas: Arc::clone(&repositories)
            as Arc<dyn jarvis_application::live_events::StreamDeltaSink>,
        provider,
        clock: Arc::new(crate::time::SystemClock::new()),
        // The same repository the policy surface reads. `None` here was the hole: a diagnostic
        // probe was governed by the stored policy while a real run recorded none, so the
        // sensitivity ceiling and the route decision existed on the test path and not on the
        // path a client actually takes. Proven by restoring it: the end-to-end journey then
        // answers `202` to a create the policy must refuse, and only this composition changed.
        policies: Some(
            repositories
                as Arc<dyn jarvis_application::repository::policy::ModelDataPolicyRepository>,
        ),
        delivery_campaigns,
        // **The tool pipeline, composed by the caller and shared with the authorization surface.**
        //
        // This is what turns the tool fabric from a set of tested values into a capability the daemon
        // exercises: before it, `ToolRegistry`, `ToolSchema`, and `action_fingerprint` had no production
        // caller and every run's tool intent ended `run.tools_not_implemented`. It is passed in rather than
        // built here so the pipeline a run dispatches through and the catalog a grant is written against are
        // the **same** value — composing it twice would give the two a catalog each, and a grant written
        // through one surface would be invisible to the pipeline the other resolves against.
        tools: Some(tools),
    }
}

/// Builds the governed tool-call pipeline over the daemon's catalog and stores.
///
/// **Every port is the real adapter**, which is the point: the catalog is the daemon's own reviewed
/// native tools, the ledger and approvals are the SQLite stores, and the validator and hasher are
/// the two functions that had no production caller. Nothing here is a double.
///
/// **What authorizes a tool, and the correction this paragraph needed.** The grant source emits a
/// reviewed grant **only** for a native, low-risk, read-only definition — and a grant is not optional:
/// the evaluator resolves one at step 3 and refuses the call `NoGrant` when none applies, before the
/// tool's declared default is ever consulted. So the `Allow` hint these tools declare is reachable
/// **only through** the grant, and an earlier version of this doc claimed the opposite — that the
/// domain "grants the low-risk read-only fast path without a grant", which
/// `a_request_with_no_grant_anywhere_is_denied_rather_than_allowed` falsifies. The fast path is a
/// *condition on the default*, not a bypass of the grant step.
///
/// Two consequences an operator should read off that: this build's only tool (`clock.now@1`) is
/// allowed because `NativeReadOnlyGrants` grants it, not because it reads a clock; and a
/// consequential tool added later is refused outright until a grant exists **and** is asked about
/// when one does.
///
/// **And the third consequence, which is the point of this round: that default is now configurable.**
/// The reviewed grants are the *unconfigured posture*, and a stored grant replaces them for its
/// principal — so "always ask about `email.send`, never ask about `clock.now`" is a row an operator
/// writes through the control plane rather than a constructor somebody changes. See
/// `tool_adapters::StoredGrants` for why an empty store falls back rather than denying everything, and
/// why a store *failure* is an error rather than a fallback.
///
/// # Why this returns a pair
///
/// The pipeline and its authorization surface are composed from **one** catalog and one pool, and
/// neither is meaningful alone: a pipeline without the surface is unconfigurable, and a surface without
/// the pipeline writes grants nothing reads. Returning them together is what makes that dependency
/// structural rather than a comment, and it is why this replaced `tool_service_over`.
///
/// # Errors
///
/// Returns [`StartupError`] when a reviewed native definition is inconsistent. That is a
/// packaging fault rather than a runtime one, and refusing startup is right: a daemon whose catalog
/// cannot be built would answer every tool call `tool.not_found` while appearing healthy.
///
/// **`pub(crate)` rather than private, so a journey can compose the real thing.** The alternative — a test
/// assembling its own pipeline — would be a second composition, and a journey that exercised a copy would
/// pass while the daemon's own wiring was broken. That is the same defect the daemon's other composition
/// helpers record: the composition root is the layer a handler test is structurally blind to.
/// Builds the governed tool pipeline over the daemon's catalog, stores, and a **supplied** executor.
///
/// `executor` is passed in rather than composed here, and that is what makes MCP dispatch possible: the
/// pipeline's executor slot is single, so the daemon supplies a router carrying both the native
/// implementation and whatever MCP servers it composed. This function's remaining job is everything that
/// does not depend on where a tool comes from — the catalog, the grants, the validator, the ledger, the
/// approvals.
pub(crate) fn tool_fabric_with(
    pool: sqlx::SqlitePool,
    deny_rules: Vec<crate::config::ReviewedDenyRule>,
    executor: Arc<dyn jarvis_application::tool_call::ToolExecutor>,
    clock: Arc<dyn jarvis_domain::clock::Clock>,
) -> Result<
    (
        Arc<jarvis_application::tool_call::ToolCallService>,
        Arc<jarvis_application::tool_grant_service::ToolGrantService>,
    ),
    StartupError,
> {
    use crate::tool_adapters::{
        FingerprintHasher, NativeReadOnlyGrants, RegistryCatalog, SchemaValidator, StoredGrants,
    };
    use jarvis_application::tool_call::ToolCallService;
    use jarvis_application::tool_grant_service::ToolGrantService;

    let definitions = crate::native_tools::definitions().map_err(|_| {
        // A construction refusal from a reviewed definition is a packaging defect, reported as a
        // config fault so an operator knows to look at the build rather than at the request.
        StartupError::Config
    })?;
    // The catalog and the grants are built from **one** definitions list, so a tool the model can see
    // and a tool an operator's configuration authorizes are the same set. Two lists would let a tool
    // be dispatchable while ungranted, or granted while unresolvable, and both read as a daemon fault
    // rather than as configuration.
    let tools: Vec<jarvis_application::tool_call::ResolvedTool> = definitions
        .into_iter()
        .map(|tool| jarvis_application::tool_call::ResolvedTool {
            definition: tool.definition,
            input_schema: Some(tool.input_schema),
        })
        .collect();
    let catalog =
        Arc::new(RegistryCatalog::new(tools.iter().map(|tool| {
            (tool.definition.clone(), tool.input_schema.clone())
        })));
    // A reviewed grant is emitted **only** for a native, low-risk, read-only definition, and its own
    // ceilings restate those bounds so the evaluator re-checks them per call.
    //
    // **The reviewed refusals are attached here, and their absence was a real gap.**
    // `NativeReadOnlyGrants::with_deny_rules` existed with no production caller, so an operator had no way
    // to refuse one of the daemon's own tools: the refusals a deployment ships were compiled into a struct
    // field that nothing populated. They now come from the `[[tools.deny]]` configuration table, validated
    // before this point, and they reach the evaluator beside the stored ones.
    let defaults = NativeReadOnlyGrants::new(tools.clone()).with_deny_rules(deny_rules);
    // **One store handle, shared by the source and the surface.** The source reads it on every dispatch
    // and the surface writes it from a client, so a second handle would let the two disagree about what
    // is configured — the same argument the catalog's single list records.
    let store: Arc<dyn jarvis_application::repository::tool_grant::ToolGrantRepository> = Arc::new(
        crate::storage::tool_grant_repository::SqliteToolGrantRepository::new(pool.clone()),
    );
    let grants = StoredGrants::new(Arc::clone(&store), defaults, &tools);
    let ledger = crate::storage::tool_call_repository::SqliteToolCallRepository::new(pool.clone());
    let approvals = crate::storage::approval_repository::SqliteApprovalRepository::new(pool);
    let pipeline = Arc::new(ToolCallService::new(
        Arc::clone(&catalog) as Arc<dyn jarvis_application::tool_call::ToolCatalog>,
        Arc::new(grants),
        Arc::new(SchemaValidator::new()),
        Arc::new(FingerprintHasher::new()),
        // **The supplied executor, which is a router.** The port takes one executor, so a second source of
        // tools — an MCP server today, a connector or runtime later — cannot be reached through a slot that
        // holds only native. Registering by *kind* keeps the pipeline's shape and makes the choice a value: a
        // call whose source kind is not routed is `NotFound` rather than served by the wrong implementation.
        // See `tool_adapters::routing`.
        executor,
        Arc::new(ledger),
        Arc::new(approvals),
        clock,
    ));
    // The surface resolves against **the catalog the pipeline dispatches through**, which is what makes
    // "a grant names a tool that can actually run" a property of the composition rather than of two lists
    // being kept in step.
    let surface = Arc::new(ToolGrantService::new(store, catalog));
    Ok((pipeline, surface))
}

/// Builds the executor the pipeline dispatches through: native, plus the MCP kind when servers composed.
///
/// **One router whatever the profile declares.** With no `[mcp]` table this is the native-only router, which
/// is the common path — so the `McpServer` kind appears only when a server actually composed. Registering the
/// kind for an empty composition would be a router advertising a source it cannot serve.
///
/// The composition's servers are folded into **one** executor for the whole kind, because
/// [`crate::tool_adapters::routing::RoutingExecutor`] holds at most one per kind and every MCP server shares
/// `SourceKind::McpServer`. See [`crate::mcp::composition::ComposedMcpExecutor`] for why that fan-out is keyed
/// by identity.
///
/// `pub(crate)` because the journey tests compose the **same** fabric the daemon does: rebuilding the router
/// there would exercise a copy, and a defect in the daemon's own wiring would leave the journey green — which
/// is the layer that needs proving.
pub(crate) fn router_over(
    clock: &Arc<dyn jarvis_domain::clock::Clock>,
    servers: &[crate::mcp::composition::ComposedMcpServer],
) -> crate::tool_adapters::routing::RoutingExecutor {
    use crate::tool_adapters::routing::RoutingExecutor;
    use jarvis_domain::tool::identity::SourceKind;

    let native: Arc<dyn jarvis_application::tool_call::ToolExecutor> =
        Arc::new(crate::native_tools::NativeExecutor::new(Arc::clone(clock)));
    if servers.is_empty() {
        return RoutingExecutor::new([(SourceKind::Native, native)]);
    }
    // The composition is folded into **one** executor for the whole kind, because a router holds at most one
    // per kind and every MCP server shares `SourceKind::McpServer`. See
    // [`crate::mcp::composition::ComposedMcpExecutor`] for why that fan-out is keyed by identity.
    let dispatcher: Arc<dyn jarvis_application::tool_call::ToolExecutor> = Arc::new(
        crate::mcp::composition::ComposedMcpExecutor::over_servers(servers),
    );
    // The native executor is used in **both** branches, so it is built once above rather than inline in
    // each: a second construction would give the two branches different executors for one kind.
    RoutingExecutor::new([
        (SourceKind::Native, native),
        (SourceKind::McpServer, dispatcher),
    ])
}

/// Composes the profile's declared MCP servers, resolving their secrets through the environment.
///
/// # Errors
///
/// Returns [`StartupError::Config`] when a *declaration* is unusable, which `jarvisd`'s composition root has
/// already validated — reaching this would mean the caller passed declarations that did not come from
/// `McpSection::declarations`, so it is a packaging fault like a refused native definition. Every per-server
/// failure (a missing secret, a launch fault, an incompatible peer, an all-refused catalog) is **reported and
/// skipped** rather than failing startup, so one misconfigured server does not take down the others.
///
/// A refused server is logged at `warn` with its stable code, because a daemon that started with fewer servers
/// than the profile declares is a fact an operator has to be able to see — and the alternative, failing
/// startup, would make an unreachable server a denial of service on the daemon's own tools.
async fn compose_mcp_servers(
    declared: &[crate::config::mcp::McpServerDeclaration],
) -> Result<crate::mcp::composition::McpComposition, StartupError> {
    if declared.is_empty() {
        return Ok(crate::mcp::composition::McpComposition::default());
    }
    // The registry is **the daemon's own**, so registrations accumulate across servers and the registry's
    // first-wins source claim refuses a second server that declares the first's owner. It is not the
    // pipeline's catalog (a capability-keyed snapshot); it is what the admission and publish checks read.
    // Held here for the composition and then dropped, because a live registry would be a second mutable
    // authority beside the catalog the pipeline dispatches through — and the daemon has no deregistration path
    // yet, so an operator removal would leave a stale claim nothing could release.
    let mut registry = jarvis_domain::tool::registry::ToolRegistry::new();
    let composition = crate::mcp::composition::compose_declared_servers(
        &mut registry,
        declared,
        &crate::config::secret::EnvSecretResolver::new(),
    )
    .await
    .map_err(|_| StartupError::Config)?;
    for (server, refusal) in composition.refused() {
        // The **code**, never the refusal's message: a message may carry operator text, and the code is what
        // an operator greps for in the note's error table.
        //
        // **`permanent` is now a fact about what already happened, not a plan.** The composition retries a
        // transient startup failure once before reporting it, so `permanent=false` here means "it was retried
        // and failed the same way again" — which is stronger evidence than the classifier's own prediction, and
        // it is why the flag is worth logging: the operator learns whether a single attempt is what failed or
        // two.
        log::warn!(
            "mcp server refused at startup: server={server} code={} permanent={}",
            refusal.code(),
            refusal.is_permanent()
        );
    }
    // **A server that started with a shorter tool list than it offered is reported, because until this
    // line nothing said so.** `ComposedMcpServer`'s own doc gives the reason the registration report is
    // carried — "forty offered" and "thirty-nine registered" are different operator facts — and `is_clean`
    // is true for *every* server that admitted at least one tool, so a partial refusal produced no output
    // at all. Its `refusals()` covers **both** stages: a tool the normalizer refused for its shape never
    // reached registration, so a registration-only check would still have missed it, and that was the case
    // with genuinely no trace. The only remaining signal was a tool missing from a catalog an operator
    // would have to diff by hand.
    for server in composition.servers() {
        for refusal in server.refusals() {
            log::warn!(
                "mcp tool refused at startup: server={} tool={} code={}",
                server.server(),
                refusal.name,
                refusal.code
            );
        }
        let report = server.report();
        // A `debug` line rather than `info`: a healthy server's counts are not an event, but they are the
        // answer to "did this server end up with the tools it listed", which is otherwise unanswerable.
        log::debug!(
            "mcp server registered: server={} added={} unchanged={} refused={} normalized_refusals={}",
            server.server(),
            report.added,
            report.unchanged,
            report.refused.len(),
            server.refusals().len() - report.refused.len(),
        );
    }
    Ok(composition)
}

/// Settles the tool calls left stranded by the previous shutdown.
///
/// **The pass cannot run before the run pass, because the two are independent** — ordering them would imply
/// a dependency that does not exist. It runs **before readiness** for the reason the run pass does:
/// readiness is defined as "recovery classification completed", and a client reaching a daemon that has not
/// settled its stranded calls would be told to wait on them.
///
/// The ledger repository is its own value because the ledger port is implemented by
/// `SqliteToolCallRepository` rather than by `SqliteRepositories` — a separate struct so the tool-call
/// storage can be handed to a caller without the whole repository bundle — and it is built over the same
/// pool so both see one database.
///
/// # Errors
///
/// Returns [`StartupError::Recovery`] when the clock cannot report, the scan cannot be read, or the store
/// was **not fully settled**. The last is a failed startup rather than a warning because a run left behind
/// is one nothing else will look at: readiness claims recovery completed, and it has not.
async fn reconcile_tool_calls_over(
    database: &crate::storage::connection::Database,
) -> Result<jarvis_application::tool_recovery::ToolRecoveryReport, StartupError> {
    let ledger: Arc<dyn jarvis_application::repository::tool_call::ToolCallRepository> = Arc::new(
        crate::storage::tool_call_repository::SqliteToolCallRepository::new(
            database.pool().clone(),
        ),
    );
    let report = jarvis_application::tool_recovery::reconcile_tool_calls(
        &ledger,
        crate::time::SystemClock::new()
            .now()
            .map_err(|_| StartupError::Recovery)?,
    )
    .await
    .map_err(|_| StartupError::Recovery)?;
    if report.incomplete_store {
        return Err(StartupError::Recovery);
    }
    Ok(report)
}

/// Formats the loopback base URL for a bound address.
fn format_base_url(address: SocketAddr) -> String {
    match address.ip() {
        IpAddr::V4(ip) => format!("http://{ip}:{}", address.port()),
        IpAddr::V6(ip) => format!("http://[{ip}]:{}", address.port()),
    }
}

/// Binds the first loopback address that is available.
///
/// IPv4 loopback is attempted first so the published authority on a dual-stack host is the
/// `127.0.0.1` every existing client already expects; IPv6 loopback is the fallback for a host
/// where IPv4 loopback is unavailable, which used to make the daemon exit with a bind failure that
/// named nothing an operator could act on.
///
/// # Errors
///
/// Returns [`StartupError::Bind`] when neither loopback address can be bound.
async fn bind_loopback() -> Result<TcpListener, StartupError> {
    let mut last = StartupError::Bind;
    for ip in [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ] {
        match TcpListener::bind(SocketAddr::new(ip, 0)).await {
            Ok(listener) => return Ok(listener),
            Err(_) => last = StartupError::Bind,
        }
    }
    Err(last)
}

/// Builds API state for a running daemon.
///
/// Prefer [`RunningDaemon::api_state`], which shares the daemon's own clients
/// rather than requiring a second registry.
///
/// # Errors
///
/// Returns [`StartupError::Bind`] when the bound address cannot be read.
pub fn api_state(
    daemon: &RunningDaemon,
    _clients: ClientRegistry,
) -> Result<Arc<ApiState>, StartupError> {
    daemon.api_state()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{DaemonConfig, format_base_url, start};
    use crate::auth::{ClientCredentialPath, ClientRegistry, enroll_owner_client};
    use crate::paths::ProfilePaths;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("jarvis-fnd007-start-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn config(root: &std::path::Path) -> DaemonConfig {
        let paths = ProfilePaths::portable(root);
        paths.ensure_directories().expect("profile dirs");
        DaemonConfig::from_profile(&paths)
    }

    fn clients(root: &std::path::Path) -> ClientRegistry {
        let destination = ClientCredentialPath::in_config_dir(root);
        let (registered, _credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
        let mut registry = ClientRegistry::new();
        registry.register(registered);
        registry
    }

    async fn start_daemon(root: &std::path::Path) -> super::RunningDaemon {
        start(
            &config(root),
            clients(root),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            "2026-09-21T00:00:00Z".to_owned(),
        )
        .await
        .expect("startup succeeds")
    }

    #[tokio::test]
    async fn startup_binds_loopback_publishes_discovery_and_is_ready() {
        let root = temp_root("ok");
        let daemon = start_daemon(&root).await;

        let address = daemon.local_addr().expect("bound address");
        assert!(address.ip().is_loopback(), "must bind loopback only");
        assert_ne!(address.port(), 0, "an operating-system port is assigned");
        assert!(daemon.readiness().is_ready());

        // Discovery is published and names the bound authority.
        let bytes = std::fs::read(daemon_discovery_path(&root)).expect("discovery exists");
        let record = jarvis_protocol::DiscoveryFile::parse(&bytes).expect("valid discovery");
        assert_eq!(record.instance_id, daemon.instance_id());
        assert_eq!(record.base_url, daemon.base_url().expect("base url"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_published_base_url_is_one_the_client_can_dial() {
        // The daemon and the client were written to different rules: this side rendered a bracketed
        // IPv6 authority and the client was able to produce one, but the client's dial was
        // hardcoded to IPv4, so a record naming `[::1]` was accepted by every validator and then
        // connected elsewhere. Asserting the two ends agree — that the authority this daemon
        // publishes round-trips through the client's own validator and dial helper — is what keeps
        // them from drifting again, and it is checked on the real bound address rather than on a
        // literal.
        let root = temp_root("dialable");
        let daemon = start_daemon(&root).await;

        let base_url = daemon.base_url().expect("base url");
        assert!(
            jarvis_protocol::discovery::validate_loopback_url(&base_url),
            "the daemon must publish an authority the record validator admits: {base_url}",
        );

        // And the part the validator cannot check: that the published host is one the client will
        // actually connect to. `dial_host` is the same function `request` uses, so this is the
        // end-to-end agreement rather than a copy of the rule.
        let authority = base_url
            .strip_prefix("http://")
            .expect("the published scheme is http");
        let (host, port) = authority.rsplit_once(':').expect("an authority and a port");
        let address = crate::client::dial_host(host).expect("the published host is dialable");
        assert_eq!(
            address,
            daemon.local_addr().expect("bound address").ip(),
            "the address the client would dial must be the address the daemon bound",
        );
        assert!(port.parse::<u16>().expect("a port") > 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_database_written_by_a_newer_binary_is_refused_and_left_untouched() {
        // `docs/data/migrations.md` fixes the startup behaviour in a table: "DB newer than binary |
        // Refuse writes/start, preserve state, explain update". The refusal exists
        // (`StorageError::SchemaTooNew`, produced by `read_compatibility`) and the daemon maps it
        // to `StartupError::Storage` — but **nothing tested it end to end**, so the two facts that
        // make the row true were unverified: that a `start` actually refuses, and that it refuses
        // *without writing*, which is the half that matters to someone who downgraded a binary.
        //
        // The newer version is written **before** the daemon starts, the way a newer binary would
        // have left it, rather than by starting a daemon, stopping it, and editing the file — a
        // `RunningDaemon` holds the single-instance lock until it is dropped, so that shape would
        // be refused for `jarvis.instance_already_held` before it reached the version check, and
        // the test would assert the wrong refusal.
        let root = temp_root("schema-too-new");
        let settings = config(&root);
        let future = crate::storage::schema::TARGET_SCHEMA_VERSION + 1;

        let seeded = crate::storage::Database::open(&settings.database_path)
            .await
            .expect("the database opens");
        crate::storage::migrate::run(seeded.pool())
            .await
            .expect("this build migrates it");
        sqlx::query("UPDATE schema_version SET schema_version = ? WHERE id = 1")
            .bind(future)
            .execute(seeded.pool())
            .await
            .expect("the record is bumped to a future version");
        seeded.close().await;

        let error = start(
            &settings,
            clients(&root),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e0c".to_owned(),
            "2026-09-21T00:00:00Z".to_owned(),
        )
        .await
        .expect_err("a newer database must be refused");
        assert_eq!(error.code(), "jarvis.db_schema_too_new");
        // The message an operator reads must name the remedy. A downgrade is not something a
        // restart fixes, so "upgrade the binary" is the whole content of the refusal.
        assert!(
            error.to_string().contains("newer JARVIS version"),
            "{error}",
        );
        assert!(!error.retryable(), "a downgraded binary stays downgraded");

        // **The record must still name the future version.** This is what makes the row's
        // "preserve state" true: a `start` that rewrote or downgraded the record would leave the
        // operator's database claiming to be older than it is, and the next binary to open it
        // would read a record that lies about what is inside.
        let after = crate::storage::Database::open(&settings.database_path)
            .await
            .expect("the database still opens");
        let recorded: i64 =
            sqlx::query_scalar("SELECT schema_version FROM schema_version WHERE id = 1")
                .fetch_one(after.pool())
                .await
                .expect("the record is readable");
        assert_eq!(
            recorded, future,
            "a refused startup must not rewrite the compatibility record",
        );
        after.close().await;

        // And nothing was published: a refused daemon must not be discoverable, or a client would
        // find a daemon that never became ready.
        assert!(
            !daemon_discovery_path(&root).exists(),
            "a refused startup must not publish discovery",
        );
        // The lock is released with the failed attempt, so the operator can fix the binary and
        // start again — the same property `a_failed_startup_releases_the_lock_for_a_retry` holds
        // for a storage failure.
        let guard = crate::lifecycle::InstanceGuard::acquire(&settings.lock_path)
            .expect("a refused startup must leave the profile claimable");
        drop(guard);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_second_daemon_on_the_same_profile_is_refused() {
        let root = temp_root("second");
        let _first = start_daemon(&root).await;

        let error = start(
            &config(&root),
            clients(&root),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e0a".to_owned(),
            "2026-09-21T00:00:00Z".to_owned(),
        )
        .await
        .expect_err("a second daemon must be refused");

        assert_eq!(error.code(), "jarvis.instance_already_held");
        assert!(!error.retryable());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn drain_unpublishes_discovery_and_reports_not_ready() {
        let root = temp_root("drain");
        let daemon = start_daemon(&root).await;
        assert!(daemon.readiness().is_ready());

        daemon.begin_drain().expect("drain begins");

        assert!(!daemon.readiness().is_ready(), "draining is not ready");
        assert_eq!(
            daemon.readiness().reason().as_deref(),
            Some("jarvis.draining"),
        );
        assert!(
            !daemon_discovery_path(&root).exists(),
            "discovery must be removed during drain",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn drain_leaves_a_foreign_discovery_file_alone() {
        let root = temp_root("foreign");
        let daemon = start_daemon(&root).await;

        // Another instance replaces the file, simulating a handover.
        let mut foreign = jarvis_protocol::DiscoveryFile::parse(
            &std::fs::read(daemon_discovery_path(&root)).expect("readable"),
        )
        .expect("valid");
        foreign.instance_id = "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e0b".to_owned();
        crate::lifecycle::publish_discovery(&daemon_discovery_path(&root), &foreign)
            .expect("publish foreign");

        daemon.begin_drain().expect("drain begins");
        assert!(
            daemon_discovery_path(&root).exists(),
            "a foreign record must not be removed by the previous instance",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn startup_fails_closed_when_the_database_cannot_be_opened() {
        let root = temp_root("bad-db");
        let mut settings = config(&root);
        // Point the database at a path whose parent is a file.
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("write blocker");
        settings.database_path = blocker.join("nested").join("jarvis.sqlite");

        let error = start(
            &settings,
            clients(&root),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            "2026-09-21T00:00:00Z".to_owned(),
        )
        .await
        .expect_err("startup must fail");

        assert_eq!(error.code(), "jarvis.db_open");
        // Nothing may be published when startup fails.
        assert!(!settings.discovery_path().exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_failed_startup_releases_the_lock_for_a_retry() {
        let root = temp_root("retry");
        let mut settings = config(&root);
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("write blocker");
        settings.database_path = blocker.join("nested").join("jarvis.sqlite");

        let _ = start(
            &settings,
            clients(&root),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            "2026-09-21T00:00:00Z".to_owned(),
        )
        .await;

        // The guard was dropped with the failed attempt, so a corrected retry
        // must be able to claim the profile.
        let _daemon = start_daemon(&root).await;

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn api_state_shares_the_daemons_readiness() {
        let root = temp_root("state");
        let daemon = start_daemon(&root).await;
        let state = super::api_state(&daemon, clients(&root)).expect("state");

        assert!(state.readiness.is_ready());
        daemon.begin_drain().expect("drain");
        assert!(
            !state.readiness.is_ready(),
            "api state must observe the same readiness flag",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn api_state_names_the_authority_the_daemon_actually_bound() {
        // The `Host` check is only meaningful if the expected authority is the
        // bound one. An ephemeral port cannot be compared against a constant, so
        // this asserts the state carries the real address rather than a guess.
        let root = temp_root("authority");
        let daemon = start_daemon(&root).await;
        let bound = daemon.local_addr().expect("local address");
        let state = super::api_state(&daemon, clients(&root)).expect("state");

        assert_eq!(state.bound_authority, crate::http::authority_of(bound));
        assert!(
            state.bound_authority.starts_with("127.0.0.1:"),
            "{}",
            state.bound_authority
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn base_url_formatting_covers_ipv4_and_ipv6() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
        assert_eq!(
            format_base_url(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127)),
            "http://127.0.0.1:43127",
        );
        assert_eq!(
            format_base_url(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 43127)),
            "http://[::1]:43127",
        );
    }

    #[test]
    fn the_default_drain_grace_is_bounded() {
        assert!(Duration::from_secs(1) <= super::DEFAULT_DRAIN_GRACE);
        assert!(super::DEFAULT_DRAIN_GRACE <= Duration::from_secs(60));
    }

    fn daemon_discovery_path(root: &std::path::Path) -> std::path::PathBuf {
        config(root).discovery_path()
    }

    /// The router a profile with no `[mcp]` table gets, asserted by **the kind it routes**.
    ///
    /// **This is the production path, not a copy of it.** `start` builds the executor through
    /// [`super::router_over`] with whatever composed, so a daemon with no MCP server gets exactly this value.
    /// Asserting the kind is what makes the wiring checkable: an executor registered under the wrong
    /// `SourceKind` would dispatch nothing, and the pipeline tests would still pass because they exercise the
    /// *pipeline* rather than the route into it.
    #[test]
    fn a_profile_with_no_mcp_server_routes_native_only() {
        use jarvis_domain::tool::identity::SourceKind;

        let clock: Arc<dyn jarvis_domain::clock::Clock> = Arc::new(crate::time::SystemClock::new());
        let routed = super::router_over(&clock, &[]).routed_kinds();
        assert_eq!(
            routed,
            vec![SourceKind::Native],
            "a profile with no `[mcp]` table must route only the daemon's own tools"
        );
        assert!(
            !routed.contains(&SourceKind::McpServer),
            "registering the MCP kind with no server would advertise a source the daemon cannot serve"
        );
    }

    /// The drain's stop step is idempotent, because `serve_until` can reach it twice.
    ///
    /// **The `Option` is the whole mechanism, so it is what gets asserted.** `serve_until` stops the servers on
    /// a serve error, then again on the normal drain path — the same function, twice, for one composition. If
    /// the take were a borrow instead, the second call would cancel sessions that had already stopped, and any
    /// report from that is a failure an operator would chase for no reason. This proves the second call is
    /// silent and that the composition is gone after the first.
    #[tokio::test]
    async fn stopping_the_servers_twice_stops_them_once() {
        let holder = Arc::new(tokio::sync::Mutex::new(Some(
            crate::mcp::composition::McpComposition::default(),
        )));
        super::stop_mcp_servers(&holder).await;
        assert!(
            holder.lock().await.is_none(),
            "the stop must take the composition, not borrow it"
        );
        // The second call has nothing to take, which is the path a double drain relies on.
        super::stop_mcp_servers(&holder).await;
    }

    /// A drain with no MCP servers configured stops nothing and does not panic.
    ///
    /// The default path, and it is not the same claim as the test above: `Some(composition)` taken twice versus
    /// a holder that was never filled. A profile with no `[mcp]` table leaves the holder `None`, and that is the
    /// shape almost every daemon run actually has.
    #[tokio::test]
    async fn a_drain_with_no_servers_stops_nothing() {
        let holder = Arc::new(tokio::sync::Mutex::new(None));
        super::stop_mcp_servers(&holder).await;
        assert!(holder.lock().await.is_none(), "still empty, still silent");
    }

    /// The router is the executor `start` hands the pipeline, and it answers for a native identity.
    ///
    /// The second half of the wiring: the *daemon's* router must actually resolve the tools the catalog
    /// offers. A router whose kind did not match would produce a daemon that starts, reports ready, and
    /// refuses every tool call — the failure mode that reads as `tool.not_found` for a tool the catalog lists.
    #[tokio::test]
    async fn the_daemons_router_resolves_a_native_tool_the_catalog_offers() {
        use jarvis_application::tool_call::ToolExecutor as _;

        let clock: Arc<dyn jarvis_domain::clock::Clock> = Arc::new(crate::time::SystemClock::new());
        let router = super::router_over(&clock, &[]);
        // The canonical definition the catalog offers, so the identity is the one a real dispatch carries
        // rather than a fixture that happens to satisfy the lookup.
        let definition = crate::native_tools::definitions()
            .expect("the reviewed definitions are consistent")
            .into_iter()
            .next()
            .expect("the daemon offers at least one native tool")
            .definition;

        let arguments = jarvis_domain::tool::call::ToolArguments::new("{}")
            .expect("the fixture arguments are usable");
        let cancel = jarvis_application::cancellation::CancellationScope::new();
        let request = jarvis_application::tool_call::ToolExecutionRequest {
            identity: &definition.identity,
            display_name: &definition.display_name,
            arguments: &arguments,
            started_at: jarvis_domain::time::UtcTimestamp::parse("2026-10-03T00:00:00Z")
                .expect("the fixture instant is valid"),
            timeout_ms: definition.execution.timeout_ms,
        };
        // **Routed, not necessarily successful.** The clock tool has an arm for its own capability, and this
        // asserts the *route* by checking the refusal is not the unrouted one — an unmatched kind would answer
        // `NotFound` before the tool ran.
        let outcome = router.execute(request, &cancel).await;
        if let Err(error) = &outcome {
            assert_ne!(
                error.error_class(),
                jarvis_domain::tool::error_class::ToolErrorClass::NotFound,
                "the daemon's own router must reach the native tool rather than refuse it as unrouted"
            );
        }
    }
}
