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
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

use crate::auth::ClientRegistry;
use crate::http::{ApiState, Readiness};
use crate::lifecycle::{DiscoveryError, InstanceError, InstanceGuard};
use crate::storage::StorageError;
use crate::storage::repositories::SqliteRepositories;
use jarvis_application::repository::run::RecoverySummary;
use jarvis_application::run_service::{RunCancellationRegistry, RunService};

/// The default bounded drain grace period.
pub const DEFAULT_DRAIN_GRACE: Duration = Duration::from_secs(10);

/// The daemon's resolved runtime configuration.
#[derive(Debug, Clone)]
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
        }
    }

    /// Returns the discovery file path.
    #[must_use]
    pub fn discovery_path(&self) -> PathBuf {
        self.discovery_dir.join("discovery.json")
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
            Self::Discovery(_) => false,
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
    recovery: RecoverySummary,
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
            .with_policies(Arc::clone(&self.policies), Arc::clone(&self.inventory)),
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
    let requested = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let listener = TcpListener::bind(requested)
        .await
        .map_err(|_| StartupError::Bind)?;
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
    let ports = run_ports(Arc::clone(&repositories));
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
    let inventory = Arc::new(crate::http::ProviderInventory::new(
        ports.provider.as_ref(),
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

    Ok(RunningDaemon {
        listener,
        discovery_path: config.discovery_path(),
        instance_id,
        readiness,
        clients: Arc::new(clients),
        runs,
        policies,
        inventory,
        recovery,
        guard,
    })
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
/// see which source served a run.
///
/// The repository is passed in rather than built here, and it is the same value the
/// daemon's policy surface uses, so the policy a run's route is selected from is the
/// policy the API reads back.
fn run_ports(repositories: Arc<SqliteRepositories>) -> jarvis_application::run_service::RunPorts {
    use jarvis_application::model::{ModelProvider, ScriptedProvider};
    use jarvis_application::run_service::RunPorts;
    use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
    use jarvis_domain::model::stream::{FinishReason, ModelStreamEventKind};

    let provider: Arc<dyn ModelProvider> = match (
        // The identifiers are literals this build controls, so a failure here would be a
        // programming error rather than a runtime condition. They are validated once and
        // fall back rather than being unwrapped, so a future edit that mistypes one degrades
        // to a provider that serves no model — which makes every run fail with
        // `run.no_model_served`, a state an operator can see — instead of panicking on the
        // startup path.
        ProviderId::from_literal("scripted.local"),
        ModelId::from_literal("scripted-echo"),
    ) {
        (Some(provider_id), Some(model_id)) => {
            // The script echoes a bounded acknowledgement rather than the caller's text,
            // so the deterministic path cannot be mistaken for a real model's answer and
            // its output cannot reflect prompt content into a public event payload.
            Arc::new(
                ScriptedProvider::new(ModelRef::new(provider_id, model_id))
                    .emit(ModelStreamEventKind::OutputItemAdded {
                        item_id: "scripted-answer".to_owned(),
                    })
                    .emit_text(
                        "scripted-answer",
                        "The scripted provider received this run. Configure a model provider to receive real answers.",
                    )
                    .emit(ModelStreamEventKind::CallCompleted {
                        finish_reason: FinishReason::Stop,
                        usage: None,
                        refused: false,
                    }),
            )
        }
        _ => Arc::new(ScriptedProvider::serving_no_model()),
    };

    RunPorts {
        runs: Arc::clone(&repositories)
            as Arc<dyn jarvis_application::repository::run::RunRepository>,
        conversations: Arc::clone(&repositories)
            as Arc<dyn jarvis_application::repository::conversation::ConversationRepository>,
        model_calls: Arc::clone(&repositories)
            as Arc<dyn jarvis_application::repository::model_call::ModelCallRepository>,
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
    }
}

/// Formats the loopback base URL for a bound address.
fn format_base_url(address: SocketAddr) -> String {
    match address.ip() {
        IpAddr::V4(ip) => format!("http://{ip}:{}", address.port()),
        IpAddr::V6(ip) => format!("http://[{ip}]:{}", address.port()),
    }
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
