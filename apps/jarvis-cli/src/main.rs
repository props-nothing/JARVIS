//! The `jarvis` command-line client.
//!
//! `jarvis` is a thin client and administration surface. It discovers a running
//! `jarvisd` and calls its authenticated local API. It never opens the database
//! for normal product commands, because the daemon is the single authority.
//!
//! Argument parsing uses `try_parse` so a caller receives a typed error rather
//! than an unexpected process exit, and secret values are never accepted as
//! command-line arguments (process listings and shell history would expose them).

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jarvis_infrastructure::client::{ClientError, discover, get_authenticated, read_credential};
use jarvis_infrastructure::config::{Config, config_file_path, read_bounded};
use jarvis_infrastructure::paths::ProfilePaths;
use jarvis_infrastructure::service::{
    DEFAULT_SERVICE_NAME, ServiceError, ServiceSpec, ServiceState, current_controller,
};
use jarvis_infrastructure::storage::{Database, schema};

/// Exit code for a successful command.
const EXIT_OK: u8 = 0;
/// Exit code for a usage or environment problem the operator must fix.
const EXIT_ATTENTION: u8 = 1;

/// The `jarvis` command-line client.
#[derive(Debug, Parser)]
#[command(
    name = "jarvis",
    about = "Local JARVIS client and administration surface",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Use an explicit portable profile root instead of the standard profile.
    #[arg(long, global = true, value_name = "DIR")]
    profile: Option<std::path::PathBuf>,
}

/// The supported Foundation commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Report daemon and profile status.
    Status,
    /// Inspect the resolved configuration without printing secret values.
    Config,
    /// List local log files.
    Logs,
    /// Report service registration state, or change it explicitly.
    Service {
        #[command(subcommand)]
        action: Option<ServiceAction>,
    },
    /// Run deterministic diagnostics and print actionable findings.
    Doctor,
}

/// Registration changes a caller can request explicitly.
///
/// `show` is the default and is read-only. Every mutating action is spelled out
/// as its own subcommand so a bare `jarvis service` can never change the system.
#[derive(Debug, Subcommand)]
enum ServiceAction {
    /// Print the plan that install would execute, without executing it.
    Show,
    /// Install the per-user service registration.
    Install,
    /// Remove the per-user service registration.
    Uninstall,
    /// Start the registered service.
    Start,
    /// Stop the registered service.
    Stop,
}

fn main() -> ExitCode {
    // A multi-threaded runtime is used because the client performs bounded
    // concurrent work during doctor.
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::from(EXIT_ATTENTION);
    };

    match Cli::try_parse() {
        Ok(cli) => runtime.block_on(run(cli)),
        Err(error) => {
            // Clap's own report is safe to print; it never contains a secret.
            let _ = error.print();
            ExitCode::from(EXIT_ATTENTION)
        }
    }
}

/// Resolves the profile once, then dispatches.
async fn run(cli: Cli) -> ExitCode {
    let Ok(resolved) = jarvis_infrastructure::profile::resolve(cli.profile.clone()) else {
        eprintln!("error: jarvis.home_directory_unavailable");
        return ExitCode::from(EXIT_ATTENTION);
    };
    let paths = &resolved.paths;

    match cli.command {
        Command::Status => status(paths).await,
        Command::Config => config(paths),
        Command::Logs => logs(paths),
        Command::Service { action } => service(action).await,
        Command::Doctor => doctor(paths).await,
    }
}

/// Reports daemon and profile status from the authenticated endpoint.
async fn status(paths: &ProfilePaths) -> ExitCode {
    let discovered = match discover(&discovery_path_for(paths)) {
        Ok(discovered) => discovered,
        Err(error) => return report_client_error(&error),
    };

    let credential_path = paths.config_dir().join("client-credential");
    let Ok(credential) = read_credential(&credential_path) else {
        return report_client_error(&ClientError::NoCredential);
    };

    match get_authenticated(&discovered, &credential, "/api/v1/system/status", 1).await {
        Ok(body) => match parse_status(&body) {
            Ok(status) => {
                println!("profile:    default");
                println!("instance:   {}", status.instance_id);
                println!("version:    {}", status.server_version);
                println!("api major:  {}", status.api_major);
                println!("state:      {}", status.state);
                println!(
                    "storage:    {} ({})",
                    status.storage.kind, status.storage.status
                );
                println!("pid:        {}", discovered.pid);
                ExitCode::from(EXIT_OK)
            }
            Err(error) => report_client_error(&error),
        },
        Err(error) => report_client_error(&error),
    }
}

/// The fields `status` presents, as the contract defines them.
#[derive(Debug, serde::Deserialize)]
struct StatusBody {
    instance_id: String,
    server_version: String,
    api_major: u32,
    state: String,
    storage: StorageBody,
}

/// The bounded storage sub-object.
#[derive(Debug, serde::Deserialize)]
struct StorageBody {
    kind: String,
    status: String,
}

/// Parses the status body, rejecting an unexpected shape.
fn parse_status(body: &str) -> Result<StatusBody, ClientError> {
    serde_json::from_str(body).map_err(|_| ClientError::MalformedResponse)
}

/// Prints the resolved configuration without any secret value.
fn config(paths: &ProfilePaths) -> ExitCode {
    let path = config_file_path(paths.config_dir());
    let Ok(bytes) = read_bounded(&path) else {
        // A missing config file is normal: defaults apply.
        println!("config file:  {} (absent; defaults apply)", path.display());
        return ExitCode::from(EXIT_OK);
    };

    let Ok(text) = std::str::from_utf8(&bytes) else {
        eprintln!("error: jarvis.config_parse");
        return ExitCode::from(EXIT_ATTENTION);
    };

    let Ok(config) = Config::from_toml(text) else {
        eprintln!("error: jarvis.config_parse");
        return ExitCode::from(EXIT_ATTENTION);
    };

    // Secret references are printed; values never are.
    println!("config file:  {}", path.display());
    println!("schema:       {}", config.schema_version);
    println!("log level:    {:?}", config.runtime.log_level);
    println!("storage kind: {:?}", config.storage.kind);
    println!("model policy: {}", config.model.policy_id);
    if let Some(reference) = &config.model.api_key_ref {
        println!("api key ref:  {reference}");
    } else {
        println!("api key ref:  (none)");
    }
    println!("telemetry:    {}", config.privacy.telemetry);
    ExitCode::from(EXIT_OK)
}

/// Lists log files by name and size, never by content.
fn logs(paths: &ProfilePaths) -> ExitCode {
    let directory = paths.log_dir();
    let Ok(entries) = std::fs::read_dir(directory) else {
        println!("log directory: {} (absent)", directory.display());
        return ExitCode::from(EXIT_OK);
    };

    println!("log directory: {}", directory.display());
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .collect();
    files.sort_by_key(std::fs::DirEntry::file_name);

    if files.is_empty() {
        println!("(no log files yet)");
    }
    for entry in files {
        let size = entry.metadata().map_or(0, |metadata| metadata.len());
        println!("  {} ({} bytes)", entry.file_name().to_string_lossy(), size);
    }
    ExitCode::from(EXIT_OK)
}

/// Reports service registration state, or executes an explicit change.
///
/// The default action is read-only: it inspects registration and prints the exact
/// effect that installing would have, so an operator approves a concrete command
/// rather than a description. A change happens only when a mutating subcommand is
/// named, and no path here ever requests elevation.
async fn service(action: Option<ServiceAction>) -> ExitCode {
    let controller = match current_controller() {
        Ok(controller) => controller,
        Err(error) => {
            println!("service:  {}", ServiceState::NotInstalled.name());
            println!("backend:  none");
            println!("error:    {}", error.code());
            println!("advice:   {}", error.advice());
            return ExitCode::from(EXIT_ATTENTION);
        }
    };

    println!("backend:  {}", controller.backend().name());

    let spec = match service_spec() {
        Ok(spec) => spec,
        Err(error) => {
            println!("error:    {}", error.code());
            println!("advice:   {}", error.advice());
            return ExitCode::from(EXIT_ATTENTION);
        }
    };

    let state = match controller.status(&spec) {
        Ok(state) => state,
        Err(error) => {
            println!("service:  unknown");
            println!("error:    {}", error.code());
            println!("advice:   {}", error.advice());
            return ExitCode::from(EXIT_ATTENTION);
        }
    };
    println!("service:  {}", state.name());
    println!("mode:     per-user, no elevation");

    let action = action.unwrap_or(ServiceAction::Show);
    let plan = match action {
        ServiceAction::Show | ServiceAction::Install => controller.plan_install(&spec),
        ServiceAction::Uninstall => controller.plan_uninstall(&spec),
        ServiceAction::Start => controller.plan_start(&spec),
        ServiceAction::Stop => controller.plan_stop(&spec),
    };

    let plan = match plan {
        Ok(plan) => plan,
        Err(error) => {
            println!("error:    {}", error.code());
            println!("advice:   {}", error.advice());
            return ExitCode::from(EXIT_ATTENTION);
        }
    };

    // The plan is always printed, for a change as well as a preview, so the
    // operator's log records exactly what was executed.
    print!("{}", plan.render());

    if matches!(action, ServiceAction::Show) {
        return ExitCode::from(EXIT_OK);
    }

    match jarvis_infrastructure::service::exec::execute(&plan).await {
        Ok(outcome) if outcome.succeeded() => {
            println!("result:   ok");
            ExitCode::from(EXIT_OK)
        }
        Ok(outcome) => {
            // The exit code is a failure class, not a value; the tool's own text
            // is bounded and is surfaced once so the operator can act.
            println!("result:   failed (status {:?})", outcome.status);
            println!("output:   {}", outcome.output.trim());
            ExitCode::from(EXIT_ATTENTION)
        }
        Err(error) => {
            println!("result:   failed");
            println!("error:    {}", error.code());
            println!("advice:   {}", error.advice());
            ExitCode::from(EXIT_ATTENTION)
        }
    }
}

/// Builds the service spec for the installed daemon.
///
/// The daemon is a separate binary from this client, so the daemon is resolved
/// next to the running executable rather than assuming this process is the
/// daemon. A service that pointed at `jarvis` would start the client and never
/// serve, so a missing daemon is reported instead of substituted.
///
/// The path is canonicalized so a symlinked install yields the real absolute
/// path, then any platform verbatim prefix is removed because a definition file
/// must contain a plain path.
fn service_spec() -> Result<ServiceSpec, ServiceError> {
    let current = std::env::current_exe().map_err(|_| ServiceError::RelativeExecutable)?;
    let directory = current
        .parent()
        .ok_or(ServiceError::RelativeExecutable)?
        .to_path_buf();

    let daemon = ["jarvisd", "jarvisd.exe"]
        .iter()
        .map(|name| directory.join(name))
        .find(|candidate| candidate.is_file())
        .ok_or(ServiceError::RelativeExecutable)?;

    let resolved = std::fs::canonicalize(&daemon).unwrap_or(daemon);
    let executable = strip_verbatim_prefix(&resolved);

    ServiceSpec::new(
        DEFAULT_SERVICE_NAME,
        executable,
        "JARVIS local daemon (jarvisd)",
    )
}

/// Removes a Windows verbatim path prefix.
///
/// `std::fs::canonicalize` returns `\\?\C:\…` on Windows. The prefix is meaningful
/// to the Win32 API but is not a path an operator or a service definition should
/// contain, and a task or unit that received it would be hard to read and to
/// migrate. `\\?\UNC\server\share` becomes `\\server\share`.
#[must_use]
fn strip_verbatim_prefix(path: &std::path::Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return path.to_path_buf();
    };
    match rest.strip_prefix("UNC\\") {
        Some(unc) => std::path::PathBuf::from(format!(r"\\{unc}")),
        None => std::path::PathBuf::from(rest),
    }
}

/// Deterministic diagnostics with actionable findings.
async fn doctor(paths: &ProfilePaths) -> ExitCode {
    let mut findings = 0_usize;

    // 1. Profile directories and permissions.
    if paths.ensure_directories().is_ok() {
        println!("ok      profile directories");
    } else {
        println!("error   profile directories: check ownership of the profile root");
        findings += 1;
    }

    // 2. Database presence, integrity, and schema compatibility.
    let database_path = paths.database_dir().join("jarvis.sqlite");
    if database_path.exists() {
        match Database::open(&database_path).await {
            Ok(database) => {
                match database.check_integrity().await {
                    Ok(()) => println!("ok      database integrity"),
                    Err(error) => {
                        println!("error   database integrity: {}", error.code());
                        findings += 1;
                    }
                }
                match database.sqlite_version().await {
                    Ok(version) => println!("ok      sqlite {version}"),
                    Err(error) => {
                        println!("error   sqlite version: {}", error.code());
                        findings += 1;
                    }
                }
                match schema::read_compatibility(database.pool()).await {
                    Ok(compatibility) => println!(
                        "ok      schema version {} (writer {})",
                        compatibility.schema_version, compatibility.writer_version
                    ),
                    Err(error) => {
                        println!("error   schema compatibility: {}", error.code());
                        findings += 1;
                    }
                }
                database.close().await;
            }
            Err(error) => {
                println!("error   database open: {}", error.code());
                findings += 1;
            }
        }
    } else {
        println!("ok      database not created yet (first daemon start creates it)");
    }

    // 3. Daemon reachability.
    match discover(&discovery_path_for(paths)) {
        Ok(discovered) => println!(
            "ok      daemon running (instance {}, pid {})",
            discovered.instance_id, discovered.pid
        ),
        Err(error) => {
            println!("warn    daemon: {} -- {}", error.code(), error.advice());
        }
    }

    // 4. Credential presence, without printing it.
    let credential_path = paths.config_dir().join("client-credential");
    match read_credential(&credential_path) {
        Ok(_) => println!("ok      client credential present (owner-only file)"),
        Err(error) => println!("warn    credential: {} -- {}", error.code(), error.advice()),
    }

    // 5. Service registration. A missing service is not a blocking finding: the
    // daemon is equally valid started in the foreground. An unusable facility is
    // reported as a warning with the portable alternative.
    match current_controller() {
        Ok(controller) => match service_spec() {
            Ok(spec) => match controller.status(&spec) {
                Ok(state) => println!(
                    "ok      service {} ({})",
                    state.name(),
                    controller.backend().name(),
                ),
                Err(error) => println!("warn    service: {} -- {}", error.code(), error.advice()),
            },
            Err(error) => println!("warn    service: {} -- {}", error.code(), error.advice()),
        },
        Err(error) => println!(
            "warn    service facility: {} -- {}",
            error.code(),
            error.advice(),
        ),
    }

    if findings == 0 {
        println!("\nno blocking findings");
        ExitCode::from(EXIT_OK)
    } else {
        println!("\n{findings} blocking finding(s)");
        ExitCode::from(EXIT_ATTENTION)
    }
}

/// Prints a client error with its code and actionable advice.
fn report_client_error(error: &ClientError) -> ExitCode {
    eprintln!("error: {}", error.code());
    eprintln!("advice: {}", error.advice());
    ExitCode::from(EXIT_ATTENTION)
}

/// Returns the runtime discovery path for a profile.
#[must_use]
fn discovery_path_for(paths: &ProfilePaths) -> std::path::PathBuf {
    paths.runtime_dir().join("discovery.json")
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, StatusBody, parse_status};
    use clap::Parser as _;

    #[test]
    fn every_documented_command_parses() {
        for (arguments, expected) in [
            (vec!["jarvis", "status"], "status"),
            (vec!["jarvis", "config"], "config"),
            (vec!["jarvis", "logs"], "logs"),
            (vec!["jarvis", "service"], "service"),
            (vec!["jarvis", "doctor"], "doctor"),
        ] {
            let cli = Cli::try_parse_from(&arguments).expect("documented command parses");
            let actual = match cli.command {
                Command::Status => "status",
                Command::Config => "config",
                Command::Logs => "logs",
                Command::Service { .. } => "service",
                Command::Doctor => "doctor",
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn a_bare_service_command_requests_no_change() {
        // This is the safety property that matters most in this file: the default
        // action must be read-only, so an operator who types the shortest form
        // cannot modify the system.
        let cli = Cli::try_parse_from(["jarvis", "service"]).expect("parses");
        assert!(
            matches!(cli.command, Command::Service { action: None }),
            "a bare `service` must not select a change",
        );
    }

    #[test]
    fn every_service_action_parses_to_its_own_variant() {
        for word in ["show", "install", "uninstall", "start", "stop"] {
            let cli = Cli::try_parse_from(["jarvis", "service", word]).expect("parses");
            assert!(
                matches!(cli.command, Command::Service { action: Some(_) }),
                "{word} must select an action",
            );
        }
        // Each spelling is distinct: the set of accepted words is exactly the set
        // of actions, so a typo cannot silently fall back to the read-only form.
        let selected: Vec<String> = ["show", "install", "uninstall", "start", "stop"]
            .iter()
            .map(|word| {
                let cli = Cli::try_parse_from(["jarvis", "service", word]).expect("parses");
                match cli.command {
                    Command::Service { action } => format!("{action:?}"),
                    Command::Status | Command::Config | Command::Logs | Command::Doctor => {
                        String::from("unexpected")
                    }
                }
            })
            .collect();
        let mut unique = selected.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            selected.len(),
            "actions must be distinct: {selected:?}"
        );
        assert!(
            !selected.iter().any(|name| name == "unexpected"),
            "{selected:?}"
        );
    }

    #[test]
    fn an_unknown_service_action_is_refused() {
        assert!(Cli::try_parse_from(["jarvis", "service", "reinstall"]).is_err());
    }

    #[test]
    fn a_profile_root_is_accepted_globally() {
        // The option is global, so it must work before or after the subcommand.
        for arguments in [
            vec!["jarvis", "--profile", "/tmp/p", "status"],
            vec!["jarvis", "status", "--profile", "/tmp/p"],
        ] {
            let cli = Cli::try_parse_from(arguments.clone()).expect("parses");
            assert_eq!(
                cli.profile,
                Some(std::path::PathBuf::from("/tmp/p")),
                "{arguments:?}"
            );
        }
        let cli = Cli::try_parse_from(["jarvis", "status"]).expect("parses");
        assert_eq!(cli.profile, None, "no profile means the standard profile");
    }

    #[test]
    fn a_typo_produces_a_typed_error_not_a_process_exit() {
        // `try_parse` is what keeps this path from calling `exit` internally.
        assert!(Cli::try_parse_from(["jarvis", "statsu"]).is_err());
        assert!(Cli::try_parse_from(["jarvis"]).is_err());
    }

    #[test]
    fn a_status_body_of_the_contract_shape_parses() {
        let body = r#"{"instance_id":"0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09",
            "server_version":"0.1.0","api_major":1,"state":"ready",
            "profile":"default","storage":{"kind":"sqlite","status":"ready"},
            "capabilities":["system.status"]}"#;
        let parsed: StatusBody = parse_status(body).expect("contract shape parses");
        assert_eq!(parsed.api_major, 1);
        assert_eq!(parsed.storage.kind, "sqlite");
        assert_eq!(parsed.state, "ready");
    }

    #[test]
    fn an_unexpected_status_body_is_rejected_rather_than_printed() {
        for body in ["", "{}", "not json", r#"{"state":"ready"}"#] {
            assert!(parse_status(body).is_err(), "{body:?}");
        }
    }

    #[test]
    fn help_and_version_are_reported_through_clap_errors() {
        assert!(Cli::try_parse_from(["jarvis", "--help"]).is_err());
        assert!(Cli::try_parse_from(["jarvis", "--version"]).is_err());
    }
}
