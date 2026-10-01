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
            .with_tool_grants(Arc::clone(&self.tool_grants)),
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

        // Readiness was set before `start` returned, so the surface is ready the
        // moment it accepts a connection. Graceful shutdown stops new admission
        // and drains in-flight connections before it resolves.
        let serving = axum::serve(self.listener, app).with_graceful_shutdown(shutdown);

        if serving.await.is_err() {
            // A serve error is not a successful drain.
            let _ = drain.begin();
            return false;
        }

        // Stop admitting and unpublish after connections have drained, so no
        // client discovers a daemon that is already stopping.
        if drain.begin().is_err() {
            return false;
        }

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
    // The tool fabric is composed **once** and handed to both consumers; see `tool_fabric_over` for why
    // composing it twice would let a grant written through one surface be invisible to the pipeline the
    // other resolves against.
    let (tools, tool_grants) = tool_fabric_over(database.pool().clone())?;
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
    // `tool_fabric_over`, and both are attached: a pipeline without the surface could not be configured
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
        guard,
    })
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
fn tool_fabric_over(
    pool: sqlx::SqlitePool,
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

    let clock: Arc<dyn jarvis_domain::clock::Clock> = Arc::new(crate::time::SystemClock::new());
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
    let defaults = NativeReadOnlyGrants::new(tools.clone());
    // **One store handle, shared by the source and the surface.** The source reads it on every dispatch
    // and the surface writes it from a client, so a second handle would let the two disagree about what
    // is configured — the same argument the catalog's single list records.
    let store: Arc<dyn jarvis_application::repository::tool_grant::ToolGrantRepository> = Arc::new(
        crate::storage::tool_grant_repository::SqliteToolGrantRepository::new(pool.clone()),
    );
    let grants = StoredGrants::new(Arc::clone(&store), defaults, &tools);
    let executor = crate::native_tools::NativeExecutor::new(Arc::clone(&clock));
    let ledger = crate::storage::tool_call_repository::SqliteToolCallRepository::new(pool.clone());
    let approvals = crate::storage::approval_repository::SqliteApprovalRepository::new(pool);
    let pipeline = Arc::new(ToolCallService::new(
        Arc::clone(&catalog) as Arc<dyn jarvis_application::tool_call::ToolCatalog>,
        Arc::new(grants),
        Arc::new(SchemaValidator::new()),
        Arc::new(FingerprintHasher::new()),
        Arc::new(executor),
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
}
