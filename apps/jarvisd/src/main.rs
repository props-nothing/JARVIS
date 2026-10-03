//! Composition root for the JARVIS daemon.
//!
//! This file wires components together and owns the process lifecycle. It
//! contains no business logic: startup order, drain, and exit live in
//! `jarvis_infrastructure::daemon`, and the HTTP surface lives in
//! `jarvis_infrastructure::http`.

use std::process::ExitCode;
use std::time::Duration;

use jarvis_infrastructure::auth::{ClientCredentialPath, ClientRegistry};
use jarvis_infrastructure::config::{Config, EnvSecretResolver, config_file_path};
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

    // Compose the model provider **before** the daemon starts, so a configuration it cannot serve
    // is refused while nothing has been published and no client can reach a daemon that would fail
    // every run. The layered load is `jarvisd`'s job rather than `daemon::start`'s because startup
    // owns the process lifecycle and configuration is an input to it, not part of it.
    let config = match compose_provider(paths.config_dir(), config) {
        Ok(config) => config,
        Err(code) => {
            // The code names a failure class, never a value; the credential is never in it.
            tracing::error!(code, "model provider configuration was refused");
            return ExitCode::from(EXIT_STARTUP_FAILED);
        }
    };

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

    // The tool-call half of the same fact, and it had no caller: the report was computed, checked for
    // an incomplete store, and then dropped. A **reconciled** call is an effect whose existence is
    // unknown, and a **cancelled** one is a reservation whose holder had died — the second being
    // invisible otherwise, because a leaked reservation makes a later retry fail with a state that
    // looks like a busy call rather than like a crash.
    let tool_recovery = daemon.tool_recovery();
    if tool_recovery.changed() > 0 || !tool_recovery.failures.is_empty() {
        tracing::warn!(
            reconciling = tool_recovery.reconciling,
            cancelled = tool_recovery.cancelled,
            failures = tool_recovery.failures.len(),
            "recovered tool calls left incomplete by the previous shutdown",
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

/// Loads the layered configuration and composes the model provider it names.
///
/// Returns `Err(code)` with the failing step's stable, namespaced code. A configuration that
/// **cannot be read** is not fatal — a fresh profile has no file, and a daemon must start — but a
/// configuration that **was read and refused** is, because serving runs against a misconfigured
/// endpoint is worse than not starting.
///
/// The environment layer is read through the explicit allowlist, so an unrelated `JARVIS_*`
/// variable is ignored rather than projected onto a setting.
fn compose_provider(
    config_dir: &std::path::Path,
    config: DaemonConfig,
) -> Result<DaemonConfig, &'static str> {
    // The environment layer is read as a snapshot of borrowed pairs. A snapshot rather than a
    // lazy iterator because the layer is applied once, at startup, and reading the environment
    // twice could observe two different values for one key.
    let environment: Vec<(String, String)> = std::env::vars().collect();
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let path = config_file_path(config_dir);
    let layered = Config::layered(
        Some(&path),
        environment,
        &jarvis_infrastructure::config::ConfigOverrides::default(),
    )
    .map_err(|error| error.code())?;
    // The environment resolver is the operator-facing source: a reference such as
    // `env:JARVIS_MODEL_KEY` reads the process environment at resolution time, so the value never
    // lives in the configuration file and never reaches a record.
    let provider =
        jarvis_infrastructure::model_providers::resolve(&layered, &EnvSecretResolver::new())
            .map_err(jarvis_infrastructure::model_providers::ProviderResolutionError::code)?;
    // **The reviewed refusals are validated here, before the daemon starts.** A refusal whose reason is
    // empty, whose effect spelling is unknown, or whose capability is not canonical is a configuration
    // fault rather than a runtime one: a daemon that started with a broken refusal would run believing it
    // had forbidden something it had not, which is the fail-open direction on the one input that has no
    // override. A refusal that names no tool **and** no effect is refused for the same reason — it would
    // appear in the configuration file and match nothing.
    let reviewed = layered
        .tools
        .reviewed_rules()
        .map_err(|error| error.code())?;
    // **The MCP declarations are validated here too, and carried whether or not they are enabled.** A name
    // that is not a usable identity, a program that cannot reach a process table, or an environment key with a
    // `=` in it is a *configuration* fault, so it fails startup rather than surfacing later as a server that
    // "failed to start". Disabled declarations are validated as well, because a disabled server is a reviewed
    // record that must stay correct — the composition skips it, so a broken one would sit in the file doing
    // nothing while its author believed it was merely switched off.
    let mcp_servers = layered.mcp.declarations().map_err(|error| error.code())?;
    // **The file roots are validated here too**: a name that is not a short lowercase slug, a path that is not
    // absolute, or a repeated name is a configuration fault that fails startup rather than a tool that later
    // refuses every call. Whether the directory exists is checked when the daemon opens it.
    let file_roots = layered
        .tools
        .files
        .declarations()
        .map_err(|error| error.code())?;
    Ok(config
        .with_provider(provider)
        .with_reviewed_deny_rules(reviewed)
        .with_autonomy(layered.tools.autonomy)
        .with_file_roots(file_roots)
        .with_agents(layered.tools.agents.enabled)
        .with_mcp_servers(mcp_servers))
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
