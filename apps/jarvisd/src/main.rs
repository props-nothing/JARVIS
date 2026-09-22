//! Composition root for the JARVIS daemon.
//!
//! This file wires components together and owns the process lifecycle. It
//! contains no business logic: startup order, drain, and exit live in
//! `jarvis_infrastructure::daemon`, and the HTTP surface lives in
//! `jarvis_infrastructure::http`.

use std::process::ExitCode;
use std::time::Duration;

use jarvis_infrastructure::auth::{ClientCredentialPath, ClientRegistry};
use jarvis_infrastructure::daemon::{DaemonConfig, RunningDaemon, start};
use jarvis_infrastructure::profile::resolve;
use jarvis_observability::logging::{LoggingConfig, init};

/// Exit code used when the daemon cannot start.
const EXIT_STARTUP_FAILED: u8 = 1;

/// Exit code used when drain did not complete within its bound.
const EXIT_DRAIN_TIMEOUT: u8 = 2;

fn main() -> ExitCode {
    // A multi-threaded runtime is used because the daemon serves concurrent
    // requests and supervises child work.
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::from(EXIT_STARTUP_FAILED);
    };

    runtime.block_on(run())
}

/// Resolves the profile, installs logging, starts the daemon, and serves.
async fn run() -> ExitCode {
    // The daemon and the client resolve the profile the same way, so a
    // clean-machine run cannot have the two disagree about which profile they
    // are using.
    let Ok(resolved) = resolve(portable_root_argument()) else {
        // No usable home directory is a fatal, explicit condition. JARVIS never
        // falls back to the current directory for durable state.
        return ExitCode::from(EXIT_STARTUP_FAILED);
    };
    let paths = &resolved.paths;

    if paths.ensure_directories().is_err() {
        return ExitCode::from(EXIT_STARTUP_FAILED);
    }

    let Ok(logging) = init(&LoggingConfig::new(paths.log_dir())) else {
        return ExitCode::from(EXIT_STARTUP_FAILED);
    };

    let config = DaemonConfig::from_profile(paths);

    // The resolved source is recorded because an operator diagnosing a
    // surprising profile needs to know which input won.
    tracing::info!(profile_source = resolved.source.name(), "profile resolved");

    // Enroll the owner client if this profile has none yet. A newly created
    // credential is registered for redaction before anything else can log it;
    // the plaintext exists only here and in the owner-only client file.
    let credential_path = ClientCredentialPath::in_config_dir(paths.config_dir());
    let Ok((clients, created_credential)) = ClientRegistry::new().into_enrolled(&credential_path)
    else {
        return ExitCode::from(EXIT_STARTUP_FAILED);
    };
    if let Some(secret) = created_credential.as_deref() {
        let _ = logging.register_secret(secret);
    }

    let instance_id = jarvis_infrastructure::ids::UuidV7Generator::new().next_instance_id();

    let Ok(started_at) = jarvis_infrastructure::time::SystemClock::new()
        .now()
        .map(|now| now.to_string())
    else {
        return ExitCode::from(EXIT_STARTUP_FAILED);
    };

    let daemon = match start(&config, clients, instance_id, started_at).await {
        Ok(daemon) => daemon,
        Err(error) => {
            // The code is safe to record: it names a failure class, not a value.
            tracing::error!(code = error.code(), "daemon startup failed");
            return ExitCode::from(EXIT_STARTUP_FAILED);
        }
    };

    // A non-empty summary means the previous shutdown left runs mid-flight. Reported at
    // startup because abandoning a run is a fact about the operator's own data, and
    // discovering it only by polling a run that never finishes is worse than being told.
    let recovery = daemon.recovery();
    if !recovery.is_empty() {
        tracing::warn!(
            abandoned = recovery.abandoned,
            parked = recovery.parked,
            "recovered runs left incomplete by the previous shutdown",
        );
    }

    tracing::info!(instance_id = daemon.instance_id(), "daemon ready");

    serve(daemon, config.drain_grace).await
}

/// Returns an explicit portable profile root from the command line, if given.
///
/// A bare `--profile <dir>` form is accepted so the daemon matches the client's
/// option without pulling a full argument-parsing dependency into the daemon.
fn portable_root_argument() -> Option<std::path::PathBuf> {
    let mut arguments = std::env::args_os().skip(1);
    for argument in arguments.by_ref() {
        if argument == "--profile" {
            return arguments.next().map(std::path::PathBuf::from);
        }
    }
    None
}

/// Serves the local surface until a shutdown signal arrives, then drains.
async fn serve(daemon: RunningDaemon, grace: Duration) -> ExitCode {
    tracing::info!("listening on the local loopback surface");

    // Either signal runs the same bounded drain inside `serve_until`.
    if daemon.serve_until(shutdown_signal(), grace).await {
        tracing::info!("drained");
        ExitCode::SUCCESS
    } else {
        // The incomplete drain is already recorded as the readiness reason.
        tracing::error!("drain did not complete within its bound");
        ExitCode::from(EXIT_DRAIN_TIMEOUT)
    }
}

/// Completes on Ctrl-C or a termination signal, whichever arrives first.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use jarvis_infrastructure::profile::ProfileSource;

    #[test]
    fn profile_sources_are_named_stably() {
        assert_eq!(ProfileSource::Portable.name(), "portable");
        assert_eq!(ProfileSource::Standard.name(), "standard");
    }
}
