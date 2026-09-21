//! The `jarvis` command-line client.
//!
//! `jarvis` is a thin client and administration surface. It discovers a running
//! `jarvisd` and calls its authenticated local API. It never opens the database
//! for normal product commands, because the daemon is the single authority.
//!
//! Argument parsing uses `try_parse` so a caller receives a typed error rather
//! than an unexpected process exit, and secret values are never accepted as
//! command-line arguments (process listings and shell history would expose them).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jarvis_infrastructure::auth::ClientCredentialPath;
use jarvis_infrastructure::client::{ClientError, discover, get_authenticated, read_credential};
use jarvis_infrastructure::config::{Config, config_file_path, read_bounded};
use jarvis_infrastructure::diagnostics::{
    DaemonDescriptor, DiagnosticsEnvironment, EnvironmentSummary, Redactor, add_log_tails, apply,
    collect, daemon_summary, export_bundle, plan_bundle, plans_for, unrepairable,
};
use jarvis_infrastructure::paths::ProfilePaths;
use jarvis_infrastructure::service::{
    DEFAULT_SERVICE_NAME, ServiceError, ServiceSpec, ServiceState, current_controller,
};

/// The API major version this client speaks.
const API_MAJOR: u32 = 1;

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
    profile: Option<PathBuf>,
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
    /// Preview or export a reviewable, redacted diagnostics bundle.
    SupportBundle {
        /// Write the bundle here. Without this the command only previews it.
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// Exclude an optional item by its path from the preview.
        #[arg(long = "exclude", value_name = "PATH")]
        exclude: Vec<String>,
    },
    /// Preview a repair plan, or apply one after explicit confirmation.
    Repair {
        /// Apply the plan. Without this the command only previews it.
        #[arg(long)]
        confirm: bool,
    },
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
        Command::SupportBundle { output, exclude } => support_bundle(paths, output, exclude).await,
        Command::Repair { confirm } => repair(paths, confirm).await,
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
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return path.to_path_buf();
    };
    match rest.strip_prefix("UNC\\") {
        Some(unc) => PathBuf::from(format!(r"\\{unc}")),
        None => PathBuf::from(rest),
    }
}

/// Deterministic diagnostics with actionable findings.
///
/// The checks themselves live in `jarvis_infrastructure::diagnostics`, so they
/// can be tested without spawning a process and reused by the support bundle.
/// This function only resolves the injectable environment (a running daemon, a
/// service controller, a service spec) and prints the report.
async fn doctor(paths: &ProfilePaths) -> ExitCode {
    let discovered = discover(&discovery_path_for(paths));
    let (daemon, discovery_error) = match &discovered {
        Ok(discovered) => (Some(DaemonDescriptor::from(discovered)), None),
        Err(error) => (None, Some(error.code())),
    };

    let controller = current_controller();
    let spec = service_spec();
    let environment = DiagnosticsEnvironment {
        daemon,
        discovery_error,
        controller: controller.as_deref().ok(),
        service_spec: spec.as_ref().ok(),
        service_spec_error: spec.as_ref().err().map(ServiceError::code),
        controller_error: controller.as_ref().err().map(ServiceError::code),
    };

    let report = collect(paths, &environment).await;
    print!("{}", report.render());
    ExitCode::from(report.exit_code())
}

/// Builds and optionally exports a reviewable, redacted support bundle.
///
/// Without `--output` this is a preview only: nothing is written and the user
/// sees exactly what a bundle would contain, including which items are optional.
/// With `--output` the same plan is materialized and written, so what was
/// previewed and what was exported cannot diverge.
async fn support_bundle(
    paths: &ProfilePaths,
    output: Option<PathBuf>,
    exclude: Vec<String>,
) -> ExitCode {
    let discovered = discover(&discovery_path_for(paths));
    let (daemon, discovery_error) = match &discovered {
        Ok(discovered) => (Some(DaemonDescriptor::from(discovered)), None),
        Err(error) => (None, Some(error.code())),
    };
    let controller = current_controller();
    let spec = service_spec();
    let environment = DiagnosticsEnvironment {
        daemon,
        discovery_error,
        controller: controller.as_deref().ok(),
        service_spec: spec.as_ref().ok(),
        service_spec_error: spec.as_ref().err().map(ServiceError::code),
        controller_error: controller.as_ref().err().map(ServiceError::code),
    };

    let report = collect(paths, &environment).await;
    let summary = daemon_summary(&environment);
    let mut plan = plan_bundle(paths.mode().token(), API_MAJOR, &report, &summary);
    add_log_tails(&mut plan, paths.log_dir());

    // Exclusions are validated against the plan, so an unknown or required name
    // is an error rather than a silently ignored argument.
    let excluded = match plan.resolve_exclusions(&exclude) {
        Ok(excluded) => excluded,
        Err(error) => {
            eprintln!("error: {} -- {}", error.code(), error);
            return ExitCode::from(EXIT_ATTENTION);
        }
    };

    print!("{}", plan.render());

    let Some(destination) = output else {
        println!("\npreview only; pass --output <FILE> to write the bundle");
        return ExitCode::from(EXIT_OK);
    };

    // The redactor is created here and has the enrolled credential registered,
    // because the client legitimately holds it and a bundle must never carry it.
    let redactor = match Redactor::new() {
        Ok(redactor) => redactor,
        Err(error) => {
            eprintln!("error: jarvis.redactor_unavailable -- {error}");
            return ExitCode::from(EXIT_ATTENTION);
        }
    };
    let credential_path = ClientCredentialPath::in_config_dir(paths.config_dir());
    let mut registered = 0_usize;
    if let Ok(credential) = read_credential(credential_path.path())
        && redactor.register(&credential).is_ok()
    {
        registered += 1;
    }

    // The whole export is one library call, so the digest recorded in the
    // manifest is guaranteed to describe the archive that is written.
    match export_bundle(
        &plan,
        &excluded,
        &redactor,
        &environment_summary(paths),
        &summary,
        &destination,
    ) {
        Ok(outcome) => {
            println!(
                "\nwrote {} ({} bytes)",
                destination.display(),
                outcome.bytes_written
            );
            println!("  items:    {} of {}", outcome.included, plan.items().len());
            println!("  excluded: {}", outcome.excluded);
            println!("  secrets registered for redaction: {registered}");
            println!("  content sha256: {}", outcome.digest);
            ExitCode::from(EXIT_OK)
        }
        Err(error) => {
            eprintln!("error: {} -- {}", error.code(), error);
            ExitCode::from(EXIT_ATTENTION)
        }
    }
}

/// Builds the environment record printed into a bundle manifest.
fn environment_summary(paths: &ProfilePaths) -> EnvironmentSummary {
    EnvironmentSummary::current(paths.mode().token(), API_MAJOR)
}

/// Previews a repair plan, or applies it after explicit confirmation.
///
/// The preview is the default and the only thing a bare invocation does, because
/// a repair mutates durable local state. `--confirm` is required to apply, and it
/// is a single explicit flag rather than a `y/n` prompt so the command stays
/// usable from a script while still requiring a deliberate decision.
///
/// Findings with no safe automated repair are reported explicitly. That is the
/// important half of the output: otherwise "repair found nothing to do" would be
/// indistinguishable from "repair cannot help you".
async fn repair(paths: &ProfilePaths, confirm: bool) -> ExitCode {
    // Repair reasons over the same diagnostics a doctor run produces, so a plan
    // can never address a problem the checks did not actually observe.
    let discovered = discover(&discovery_path_for(paths));
    let (daemon, discovery_error) = match &discovered {
        Ok(discovered) => (Some(DaemonDescriptor::from(discovered)), None),
        Err(error) => (None, Some(error.code())),
    };
    let controller = current_controller();
    let spec = service_spec();
    let environment = DiagnosticsEnvironment {
        daemon,
        discovery_error,
        controller: controller.as_deref().ok(),
        service_spec: spec.as_ref().ok(),
        service_spec_error: spec.as_ref().err().map(ServiceError::code),
        controller_error: controller.as_ref().err().map(ServiceError::code),
    };

    let report = collect(paths, &environment).await;
    let plans = plans_for(&report, paths);
    let refused = unrepairable(&report, paths);

    if plans.is_empty() {
        println!("no repairable problems were found");
    }
    let mut applied = 0_usize;
    let mut failed = 0_usize;
    for plan in &plans {
        println!("{}", plan.render());
        if confirm {
            match apply(paths, plan, true) {
                Ok(outcome) => {
                    applied += 1;
                    println!(
                        "result:    ok ({} action(s); verified: {})",
                        outcome.applied, outcome.detail
                    );
                }
                Err(error) => {
                    failed += 1;
                    println!("result:    refused: {}", error.code());
                    println!("reason:    {error}");
                }
            }
        } else {
            println!("result:    preview only; pass --confirm to apply");
        }
    }

    if !refused.is_empty() {
        println!("\nnot repairable by JARVIS (an operator decision or a restore is needed):");
        for (check, error) in &refused {
            println!("  {check}: {}", error.code());
            println!("    {error}");
        }
    }

    if failed > 0 {
        ExitCode::from(EXIT_ATTENTION)
    } else {
        println!(
            "\n{applied} plan(s) applied, {} previewed, {} not repairable",
            if confirm { 0 } else { plans.len() - applied },
            refused.len()
        );
        ExitCode::from(EXIT_OK)
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
fn discovery_path_for(paths: &ProfilePaths) -> PathBuf {
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
            (vec!["jarvis", "support-bundle"], "support-bundle"),
            (vec!["jarvis", "repair"], "repair"),
        ] {
            let cli = Cli::try_parse_from(&arguments).expect("documented command parses");
            let actual = match cli.command {
                Command::Status => "status",
                Command::Config => "config",
                Command::Logs => "logs",
                Command::Service { .. } => "service",
                Command::Doctor => "doctor",
                Command::SupportBundle { .. } => "support-bundle",
                Command::Repair { .. } => "repair",
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn repair_previews_unless_confirmed() {
        // The safety property: a bare `repair` must not mutate anything. A repair
        // that applied itself because the user typed the shortest form would be
        // the most damaging possible default in this program.
        let cli = Cli::try_parse_from(["jarvis", "repair"]).expect("parses");
        match cli.command {
            Command::Repair { confirm } => assert!(!confirm, "a bare repair must not confirm"),
            _ => unreachable!("repair must parse to its own variant"),
        }

        let confirmed = Cli::try_parse_from(["jarvis", "repair", "--confirm"]).expect("parses");
        match confirmed.command {
            Command::Repair { confirm } => assert!(confirm),
            _ => unreachable!("repair must parse to its own variant"),
        }
    }

    #[test]
    fn support_bundle_previews_unless_an_output_path_is_given() {
        // The safety property: the shortest form must not write a file anywhere.
        // A bundle the user has not reviewed is exactly the kind of accidental
        // disclosure this command exists to prevent.
        let cli = Cli::try_parse_from(["jarvis", "support-bundle"]).expect("parses");
        match cli.command {
            Command::SupportBundle { output, exclude } => {
                assert!(output.is_none(), "a bare support-bundle must not write");
                assert!(exclude.is_empty());
            }
            _ => unreachable!("support-bundle must parse to its own variant"),
        }
    }

    #[test]
    fn support_bundle_accepts_repeated_exclusions_and_an_output_path() {
        let cli = Cli::try_parse_from([
            "jarvis",
            "support-bundle",
            "--output",
            "bundle.zip",
            "--exclude",
            "environment.json",
            "--exclude",
            "daemon.json",
        ])
        .expect("parses");
        match cli.command {
            Command::SupportBundle { output, exclude } => {
                assert_eq!(output.as_deref(), Some(std::path::Path::new("bundle.zip")));
                assert_eq!(exclude, vec!["environment.json", "daemon.json"]);
            }
            _ => unreachable!("support-bundle must parse to its own variant"),
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
                    Command::Status
                    | Command::Config
                    | Command::Logs
                    | Command::Doctor
                    | Command::SupportBundle { .. }
                    | Command::Repair { .. } => String::from("unexpected"),
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
