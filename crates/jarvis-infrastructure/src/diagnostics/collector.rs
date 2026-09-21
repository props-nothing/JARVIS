//! The deterministic diagnostics collector.
//!
//! This used to be a function inside the command-line binary, which meant it
//! could not be tested without spawning a process and could not be reused by
//! the support bundle. It is a library item now: a pure function from a
//! [`ProfilePaths`] and a [`DiagnosticsEnvironment`] to a [`CheckReport`].
//!
//! Every check is a small function that returns one [`Finding`], so a check
//! that is added later cannot accidentally make an earlier one unreachable. The
//! order is fixed (directories, database, daemon, credential, configuration,
//! service) because the report is read top to bottom and the earlier checks
//! explain the later ones.

use jarvis_protocol::DiscoveryFile;

use crate::auth::ClientCredentialPath;
use crate::client::read_credential;
use crate::config::{Config, config_file_path, read_bounded};
use crate::paths::ProfilePaths;
use crate::service::{ServiceController, ServiceSpec};
use crate::storage::{Database, schema};

use super::{CheckReport, DaemonSummary, Finding};

/// The fixed advice texts, kept together so the check functions read as logic
/// and the operator-facing wording is reviewable in one place.
const ADVICE_DIRECTORIES: &str =
    "Check ownership and free space for the profile root, then rerun doctor.";
const ADVICE_DATABASE_OPEN: &str =
    "The database cannot be opened. Restore the most recent verified backup.";
const ADVICE_DATABASE_INTEGRITY: &str =
    "Run a verified backup and restore; do not delete the database file.";
const ADVICE_STORAGE_MIGRATION: &str =
    "Back up the database, then let the matching daemon version migrate it.";
const ADVICE_CONFIG_INVALID: &str =
    "Fix or remove the configuration file; defaults apply when absent.";
const ADVICE_CONFIG_UNSUPPORTED: &str =
    "Do not downgrade. Run the version that wrote this configuration or migrate it explicitly.";
const ADVICE_CREDENTIAL_MISSING: &str =
    "Start jarvisd once to enroll a local client credential, then rerun doctor.";
const ADVICE_DAEMON_NOT_RUNNING: &str =
    "Start jarvisd, or use portable foreground mode. This is not a blocking finding.";

/// The daemon facts a check needs, described without depending on the client
/// transport type.
///
/// The CLI's `Discovered` does not carry the API major, and the checks must not
/// force it to. Taking a small descriptor keeps the collector usable from both
/// the client (`Discovered`) and the daemon-side protocol type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonDescriptor<'a> {
    /// The daemon instance identifier. Safe to record; it grants nothing.
    pub instance_id: &'a str,
    /// The daemon process id. Diagnostic only, never authority.
    pub pid: u32,
    /// The API major version the daemon reported.
    pub api_major: u32,
}

impl<'a> From<&'a DiscoveryFile> for DaemonDescriptor<'a> {
    fn from(file: &'a DiscoveryFile) -> Self {
        Self {
            instance_id: &file.instance_id,
            pid: file.pid,
            api_major: file.api_major,
        }
    }
}

impl<'a> From<&'a crate::client::Discovered> for DaemonDescriptor<'a> {
    fn from(discovered: &'a crate::client::Discovered) -> Self {
        // The client's discovery type resolves the same file, so the API major is
        // the one this client speaks. It is not read from the daemon here because
        // `Discovered` is a reachability value, not a version record.
        Self {
            instance_id: &discovered.instance_id,
            pid: discovered.pid,
            api_major: crate::http::API_MAJOR,
        }
    }
}

/// The environment facts a check needs that the library must not read directly.
///
/// Injecting them is what makes the collector a pure function: a test can assert
/// every branch without a running daemon, a real service manager, or a clock,
/// and the production caller supplies the real values.
pub struct DiagnosticsEnvironment<'a> {
    /// The daemon descriptor, when a discovery file exists and parses.
    pub daemon: Option<DaemonDescriptor<'a>>,
    /// The stable code from the discovery failure, when it failed.
    pub discovery_error: Option<&'static str>,
    /// The service controller for this platform, when one exists.
    pub controller: Option<&'a dyn ServiceController>,
    /// The service spec, when the daemon executable could be resolved.
    pub service_spec: Option<&'a ServiceSpec>,
    /// The stable code from the service-spec failure, when it failed.
    pub service_spec_error: Option<&'static str>,
    /// The stable code from the controller lookup failure, when it failed.
    pub controller_error: Option<&'static str>,
}

/// Runs every diagnostic check against `paths`.
///
/// The database checks are asynchronous because opening SQLite is; everything
/// else is synchronous.
pub async fn collect(
    paths: &ProfilePaths,
    environment: &DiagnosticsEnvironment<'_>,
) -> CheckReport {
    let mut report = CheckReport::new();
    report.push(check_directories(paths));
    report.push(check_database(paths).await);
    report.push(check_schema(paths).await);
    report.push(check_daemon(environment));
    report.push(check_credential(paths));
    report.push(check_configuration(paths));
    report.push(check_service(environment));
    report
}

/// Reduces an environment to the small daemon record a bundle records.
#[must_use]
pub fn daemon_summary(environment: &DiagnosticsEnvironment<'_>) -> DaemonSummary {
    match environment.daemon {
        Some(descriptor) => DaemonSummary {
            instance_id: Some(descriptor.instance_id.to_owned()),
            pid: Some(descriptor.pid),
            server_version: Some(crate::http::SERVER_VERSION.to_owned()),
            unavailable_code: None,
        },
        None => DaemonSummary {
            instance_id: None,
            pid: None,
            server_version: None,
            unavailable_code: environment
                .discovery_error
                .or(Some("jarvis.daemon_unreachable")),
        },
    }
}

/// Checks that the profile directories exist and are owner-only where the
/// platform can report it.
///
/// Creating them is deliberate: a fresh profile is a normal state, and doctor is
/// the supported way to prepare one.
fn check_directories(paths: &ProfilePaths) -> Finding {
    match paths.ensure_directories() {
        Ok(()) => Finding::ok("profile directories", "present"),
        Err(error) => Finding::error("profile directories", error.code(), ADVICE_DIRECTORIES),
    }
}

/// Checks that the database exists, opens, and passes integrity plus
/// foreign-key checks.
async fn check_database(paths: &ProfilePaths) -> Finding {
    let database_path = database_file(paths);
    if !database_path.exists() {
        return Finding::ok(
            "database",
            "not created yet (the first daemon start creates it)",
        );
    }

    match Database::open(&database_path).await {
        Ok(database) => {
            let result = match database.check_integrity().await {
                Ok(()) => match database.sqlite_version().await {
                    Ok(version) => {
                        Finding::ok("database", format!("integrity ok, sqlite {version}"))
                    }
                    Err(error) => {
                        Finding::error("database", error.code(), ADVICE_DATABASE_INTEGRITY)
                    }
                },
                Err(error) => Finding::error("database", error.code(), ADVICE_DATABASE_INTEGRITY),
            };
            database.close().await;
            result
        }
        Err(error) => Finding::error("database", error.code(), ADVICE_DATABASE_OPEN),
    }
}

/// Checks that the on-disk schema is readable by this binary.
async fn check_schema(paths: &ProfilePaths) -> Finding {
    let database_path = database_file(paths);
    if !database_path.exists() {
        return Finding::ok("schema", "no database yet");
    }
    match Database::open(&database_path).await {
        Ok(database) => {
            let result = match schema::read_compatibility(database.pool()).await {
                Ok(compatibility) => {
                    if compatibility.is_readable() {
                        Finding::ok(
                            "schema",
                            format!(
                                "version {} (writer {})",
                                compatibility.schema_version, compatibility.writer_version
                            ),
                        )
                    } else {
                        Finding::error(
                            "schema",
                            format!("unsupported version {}", compatibility.schema_version),
                            ADVICE_STORAGE_MIGRATION,
                        )
                    }
                }
                Err(error) => Finding::error("schema", error.code(), ADVICE_STORAGE_MIGRATION),
            };
            database.close().await;
            result
        }
        Err(error) => Finding::error("schema", error.code(), ADVICE_DATABASE_OPEN),
    }
}

/// Checks daemon reachability.
///
/// A daemon that is not running is a **warning**, not an error: foreground and
/// portable use are supported, so this must not force a non-zero exit on a
/// healthy profile.
fn check_daemon(environment: &DiagnosticsEnvironment<'_>) -> Finding {
    match environment.daemon {
        Some(descriptor) => Finding::ok(
            "daemon",
            format!(
                "running (instance {}, api major {})",
                descriptor.instance_id, descriptor.api_major
            ),
        ),
        None => Finding::warning(
            "daemon",
            environment
                .discovery_error
                .unwrap_or("jarvis.daemon_not_running"),
            ADVICE_DAEMON_NOT_RUNNING,
        ),
    }
}

/// Checks that a client credential is enrolled, without reading its value into
/// the report.
fn check_credential(paths: &ProfilePaths) -> Finding {
    let credential_path = ClientCredentialPath::in_config_dir(paths.config_dir());
    match read_credential(credential_path.path()) {
        Ok(_) => Finding::ok("credential", "enrolled (owner-only file)"),
        Err(error) => Finding::warning("credential", error.code(), ADVICE_CREDENTIAL_MISSING),
    }
}

/// Checks the configuration file, distinguishing "absent" from "invalid".
fn check_configuration(paths: &ProfilePaths) -> Finding {
    let path = config_file_path(paths.config_dir());
    if !path.exists() {
        return Finding::ok("configuration", "no file (defaults apply)");
    }
    let Ok(bytes) = read_bounded(&path) else {
        return Finding::error("configuration", "jarvis.config_read", ADVICE_CONFIG_INVALID);
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Finding::error(
            "configuration",
            "jarvis.config_encoding",
            ADVICE_CONFIG_INVALID,
        );
    };
    match Config::from_toml(text) {
        Ok(config) => Finding::ok(
            "configuration",
            format!(
                "schema {} ok (storage {:?}, telemetry {})",
                config.schema_version, config.storage.kind, config.privacy.telemetry
            ),
        ),
        Err(error) => {
            // An unsupported version is a different operator action from a
            // malformed file, so the code picks the advice.
            let advice = if error.code() == "jarvis.config_schema_unsupported" {
                ADVICE_CONFIG_UNSUPPORTED
            } else {
                ADVICE_CONFIG_INVALID
            };
            Finding::error("configuration", error.code(), advice)
        }
    }
}

/// Checks service registration.
///
/// An unregistered service is a warning: the daemon is equally valid in the
/// foreground. This check never changes registration.
fn check_service(environment: &DiagnosticsEnvironment<'_>) -> Finding {
    let Some(controller) = environment.controller else {
        return Finding::warning(
            "service",
            environment
                .controller_error
                .unwrap_or("jarvis.service_facility_unavailable"),
            "Use foreground or portable mode on this platform.",
        );
    };
    let Some(spec) = environment.service_spec else {
        return Finding::warning(
            "service",
            environment
                .service_spec_error
                .unwrap_or("jarvis.service_spec_unavailable"),
            "Run jarvis service from a directory containing jarvisd.",
        );
    };
    match controller.status(spec) {
        Ok(state) => Finding::ok(
            "service",
            format!("{} ({})", state.name(), controller.backend().name()),
        ),
        Err(error) => Finding::warning(
            "service",
            error.code(),
            "Run jarvis service install to register the per-user service.",
        ),
    }
}

/// Returns the canonical database file for a profile.
///
/// The database lives on the data root, not the runtime root, because it is
/// durable state.
#[must_use]
pub fn database_file(paths: &ProfilePaths) -> std::path::PathBuf {
    paths.database_dir().join("jarvis.sqlite")
}

#[cfg(test)]
mod tests {
    use super::database_file;

    use crate::paths::ProfilePaths;

    #[test]
    fn the_database_file_is_durable_state_not_runtime_state() {
        let root = std::env::temp_dir().join("jarvis-fnd013-collector-db");
        let profile = ProfilePaths::portable(root);
        let database = database_file(&profile);
        assert!(database.starts_with(profile.data_dir()));
        assert!(!database.starts_with(profile.runtime_dir()));
    }
}
