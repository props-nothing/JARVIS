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
use std::sync::Arc;

use clap::{Parser, Subcommand};
use jarvis_infrastructure::auth::ClientCredentialPath;
use jarvis_infrastructure::client::{
    ClientError, Discovered, discover, get_authenticated, get_with_status, post_authenticated,
    read_credential,
};
use jarvis_infrastructure::config::{Config, config_file_path, read_bounded};
use jarvis_infrastructure::diagnostics::{
    ClientReachability, DaemonDescriptor, DiagnosticsEnvironment, EnvironmentSummary, Redactor,
    add_log_tails, apply, collect, daemon_summary, export_bundle, plan_bundle, plans_for,
    unrepairable,
};
use jarvis_infrastructure::install::{
    ExistingInstall, InstallError, InstallLayout, InstallMode, VerifiedRelease,
    apply as apply_install, plan_install, plan_rollback, plan_uninstall, plan_update,
};
use jarvis_infrastructure::paths::ProfilePaths;
use jarvis_infrastructure::release::{
    MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES, ReleaseError, TEST_KEY_ID, TrustStore,
    default_signature_path, read_bounded as read_release_file, verify_artifacts,
};
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
    /// Ask a question and print the run's answer.
    Ask {
        /// The text to send.
        text: String,
    },
    /// Inspect durable runs.
    Runs {
        #[command(subcommand)]
        action: RunsAction,
    },
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
    /// Verify a signed release manifest and the artifacts it lists.
    ///
    /// This is a consumer-side check: it uses the trust store compiled into this
    /// binary and never accepts a signing key as an argument, because a key the
    /// caller supplies proves nothing.
    VerifyRelease {
        /// The release manifest to verify.
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
        /// The detached signature. Defaults to `<manifest>.sig`.
        #[arg(long, value_name = "FILE")]
        signature: Option<PathBuf>,
        /// The directory holding the listed artifacts. Defaults to the
        /// manifest's own directory.
        #[arg(long, value_name = "DIR")]
        artifacts: Option<PathBuf>,
    },
    /// Report or change the installed program version.
    ///
    /// Every mutating action is a named subcommand and previews unless
    /// `--confirm` is passed, so a bare `jarvis install` can never change an
    /// installed product. User data is never touched except by `--purge`, which
    /// additionally requires its own acknowledgement.
    Install {
        #[command(subcommand)]
        action: Option<InstallAction>,
        /// Use an explicit install root instead of the standard per-user location.
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
}

/// Installed-version changes a caller can request explicitly.
#[derive(Debug, Subcommand)]
enum InstallAction {
    /// Report the installed and active versions.
    Status,
    /// Plan an install or update from a verified release.
    Update {
        /// The release manifest to install from.
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
        /// The detached signature. Defaults to `<manifest>.sig`.
        #[arg(long, value_name = "FILE")]
        signature: Option<PathBuf>,
        /// The directory holding the listed artifacts. Defaults to the
        /// manifest's own directory.
        #[arg(long, value_name = "DIR")]
        artifacts: Option<PathBuf>,
        /// The platform target to install. Defaults to the release's own target.
        #[arg(long, value_name = "TARGET")]
        target: Option<String>,
        /// Apply the plan. Without this the command only previews it.
        #[arg(long)]
        confirm: bool,
    },
    /// Plan a return to the previous version.
    Rollback {
        /// Apply the plan. Without this the command only previews it.
        #[arg(long)]
        confirm: bool,
    },
    /// Plan removal of installed program files.
    Uninstall {
        /// Also remove the user profile, including the database. Requires
        /// `--acknowledge-purge` as well, because this destroys user data.
        #[arg(long)]
        purge: bool,
        /// Acknowledge that `--purge` destroys user data. Ignored without it.
        #[arg(long = "acknowledge-purge")]
        acknowledge_purge: bool,
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
        Command::Ask { text } => ask(paths, &text).await,
        Command::Runs { action } => runs(paths, action).await,
        Command::Config => config(paths),
        Command::Logs => logs(paths),
        Command::Service { action } => service(action).await,
        Command::Doctor => doctor(paths).await,
        Command::SupportBundle { output, exclude } => support_bundle(paths, output, exclude).await,
        Command::Repair { confirm } => repair(paths, confirm).await,
        Command::VerifyRelease {
            manifest,
            signature,
            artifacts,
        } => verify_release(&manifest, signature, artifacts),
        Command::Install { action, root } => install(paths, action, root),
    }
}

/// Run inspection a caller can request.
///
/// `show` and `events` are read-only; `cancel` changes durable state and is therefore its
/// own explicit action, so a bare `jarvis runs` can never stop anything.
#[derive(Debug, Subcommand)]
enum RunsAction {
    /// Print one run's state.
    Show {
        /// The run identifier.
        run_id: String,
    },
    /// Print a run's events as a stream.
    Events {
        /// The run identifier.
        run_id: String,
        /// Resume after this event identifier.
        #[arg(long = "last-event-id", value_name = "ID")]
        last_event_id: Option<String>,
    },
    /// Request cancellation of a run.
    Cancel {
        /// The run identifier.
        run_id: String,
    },
}

/// The daemon connection a command needs.
struct ClientState {
    discovered: Discovered,
    credential: String,
}

/// Resolves the daemon and credential a command needs.
///
/// Shared by every command that talks to the daemon, so the credential read, the
/// discovery read, and the failure reporting cannot differ between them.
fn daemon_client(paths: &ProfilePaths) -> Result<ClientState, ClientError> {
    let discovered = discover(&discovery_path_for(paths))?;
    let credential_path = paths.config_dir().join("client-credential");
    let credential = read_credential(&credential_path)?;
    Ok(ClientState {
        discovered,
        credential,
    })
}

/// Sends one question and prints the answer.
///
/// The command returns as soon as the run is created and then follows it, because a run
/// can outlive the request that created it: printing the create response and exiting
/// would leave the operator without the answer they asked for.
async fn ask(paths: &ProfilePaths, text: &str) -> ExitCode {
    if text.trim().is_empty() {
        eprintln!("error: the question is empty");
        return ExitCode::from(EXIT_ATTENTION);
    }
    let state = match daemon_client(paths) {
        Ok(state) => Arc::new(state),
        Err(error) => return report_client_error(&error),
    };
    // `model_policy` is omitted rather than sent, so the daemon resolves the workspace's active
    // policy for the run. The CLI cannot name it: the policy identifier is derived from the
    // workspace, and the workspace is resolved server-side from this client's credential, so the
    // only value the CLI could send is one it invented. It previously sent
    // `{"policy_id":"default","version":1}` — a policy that has never existed in any workspace —
    // and because the daemon ignored the field the request succeeded anyway, which meant the
    // run's record described a policy nobody had configured. Omission states the truth: this run
    // is governed by whatever the workspace has in force.
    let body = format!(
        concat!(
            r#"{{"conversation_id":null,"input":{{"type":"text","text":{}}},"#,
            r#""runtime":"jarvis-native"}}"#
        ),
        json_string(text),
    );
    let headers = format!("Idempotency-Key: {}\r\n", idempotency_key());
    let (status, response) = match post_authenticated(
        &state.discovered,
        &state.credential,
        "/api/v1/runs",
        API_MAJOR,
        &headers,
        &body,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return report_client_error(&error),
    };
    if !(200..300).contains(&status) {
        eprintln!("error: {response}");
        return ExitCode::from(EXIT_ATTENTION);
    }
    let Some(run_id) = serde_json::from_str::<serde_json::Value>(&response)
        .ok()
        .and_then(|value| value["run_id"].as_str().map(str::to_owned))
    else {
        eprintln!("error: the daemon response has no run id");
        return ExitCode::from(EXIT_ATTENTION);
    };

    follow_run(&state, &run_id).await
}

/// Follows a run's events until it reaches a terminal state, printing its answer.
async fn follow_run(state: &ClientState, run_id: &str) -> ExitCode {
    // The stream is polled rather than held open, because this build serves the events
    // endpoint as a bounded replay rather than a live follow. The loop is what a client
    // does against that shape: reconnect with `Last-Event-ID` until a terminal event
    // arrives. It is bounded so a run that never finishes cannot hang the command.
    let mut last_event_id: Option<String> = None;
    let mut streamed = false;
    for _ in 0..600 {
        let extra = last_event_id
            .as_ref()
            .map_or_else(String::new, |id| format!("Last-Event-ID: {id}\r\n"));
        let (status, body) = match get_with_status(
            &state.discovered,
            &state.credential,
            &format!("/api/v1/runs/{run_id}/events"),
            API_MAJOR,
            &extra,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => return report_client_error(&error),
        };
        if !(200..300).contains(&status) {
            eprintln!("error: {body}");
            return ExitCode::from(EXIT_ATTENTION);
        }

        let mut terminal: Option<String> = None;
        for frame in parse_sse(&body) {
            if let Some(id) = frame.id.clone() {
                last_event_id = Some(id);
            }
            if frame.event == "run.output_text.delta"
                && let Some(delta) = frame.payload("delta")
            {
                print!("{delta}");
                streamed = true;
            }
            if frame.event == "run.completed"
                || frame.event == "run.failed"
                || frame.event == "run.cancelled"
            {
                terminal = Some(frame.event);
            }
        }
        if let Some(event) = terminal {
            if streamed {
                println!();
            }
            if event == "run.completed" {
                return ExitCode::SUCCESS;
            }
            // A failed or cancelled run is reported by its own event, so an operator
            // sees what happened rather than only that the command did not succeed.
            eprintln!("error: {}", event.replace("run.", "run "));
            return ExitCode::from(EXIT_ATTENTION);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    eprintln!("error: the run did not reach a terminal state in time");
    ExitCode::from(EXIT_ATTENTION)
}

/// Inspects a run.
async fn runs(paths: &ProfilePaths, action: RunsAction) -> ExitCode {
    let state = match daemon_client(paths) {
        Ok(state) => state,
        Err(error) => return report_client_error(&error),
    };
    match action {
        RunsAction::Show { run_id } => {
            let (status, body) = match get_with_status(
                &state.discovered,
                &state.credential,
                &format!("/api/v1/runs/{run_id}"),
                API_MAJOR,
                "",
            )
            .await
            {
                Ok(result) => result,
                Err(error) => return report_client_error(&error),
            };
            if !(200..300).contains(&status) {
                eprintln!("error: {body}");
                return ExitCode::from(EXIT_ATTENTION);
            }
            // The daemon's own response is printed rather than a re-derived summary, so
            // the client cannot disagree with the daemon about a run's state.
            println!("{body}");
            ExitCode::SUCCESS
        }
        RunsAction::Events {
            run_id,
            last_event_id,
        } => {
            let extra =
                last_event_id.map_or_else(String::new, |id| format!("Last-Event-ID: {id}\r\n"));
            let (status, body) = match get_with_status(
                &state.discovered,
                &state.credential,
                &format!("/api/v1/runs/{run_id}/events"),
                API_MAJOR,
                &extra,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => return report_client_error(&error),
            };
            if !(200..300).contains(&status) {
                eprintln!("error: {body}");
                return ExitCode::from(EXIT_ATTENTION);
            }
            print!("{body}");
            ExitCode::SUCCESS
        }
        RunsAction::Cancel { run_id } => {
            let headers = format!("Idempotency-Key: {}\r\n", idempotency_key());
            let (status, body) = match post_authenticated(
                &state.discovered,
                &state.credential,
                &format!("/api/v1/runs/{run_id}/cancel"),
                API_MAJOR,
                &headers,
                r#"{"reason":"user_requested"}"#,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => return report_client_error(&error),
            };
            if !(200..300).contains(&status) {
                eprintln!("error: {body}");
                return ExitCode::from(EXIT_ATTENTION);
            }
            println!("{body}");
            ExitCode::SUCCESS
        }
    }
}

/// One parsed server-sent event.
struct SseFrame {
    id: Option<String>,
    event: String,
    data: String,
}

impl SseFrame {
    /// Reads one field out of the event's JSON payload.
    fn payload(&self, field: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(&self.data)
            .ok()?
            .get("payload")?
            .get(field)?
            .as_str()
            .map(str::to_owned)
    }
}

/// Parses SSE frames out of a response body.
///
/// A comment line is skipped, which is what the contract requires of a keepalive: it
/// carries no `id` and must not be mistaken for an event or consume a sequence.
fn parse_sse(body: &str) -> Vec<SseFrame> {
    let mut frames = Vec::new();
    let mut id = None;
    let mut event = None;
    let mut data = String::new();
    for line in body.lines() {
        if line.is_empty() {
            if let Some(event_type) = event.take() {
                frames.push(SseFrame {
                    id: id.take(),
                    event: event_type,
                    data: std::mem::take(&mut data),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("id: ") {
            id = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("event: ") {
            event = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("data: ") {
            value.clone_into(&mut data);
        }
        // A line beginning with `:` is a comment and is deliberately ignored, so a
        // keepalive cannot be mistaken for an event.
    }
    frames
}

/// Encodes a string as a JSON string literal.
///
/// Hand-written rather than taking a JSON dependency in the CLI for one value, and it
/// escapes everything a JSON string may not contain, so a question with a quote or a
/// newline cannot corrupt the request body.
fn json_string(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control < '\u{20}' => {
                // `write!` rather than `push_str(&format!(..))`, which would allocate a
                // temporary string per control character.
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Generates an idempotency key for one command.
///
/// A fresh key per invocation is deliberate: the contract makes a *repeat with the same
/// key* idempotent, and an operator typing `jarvis ask` twice is issuing two commands
/// rather than retrying one.
///
/// The value is derived from the clock and the process rather than from a random source,
/// because this is the only place the CLI needs one and a key is only ever compared for
/// equality — it is never a secret and never a security token. A cryptographic source
/// would be a dependency the research gate requires evidence for, and using one here
/// would imply the value is a credential, which it is not.
fn idempotency_key() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("cli-{}-{nanos}", std::process::id())
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
    let reachability = discovered.as_ref().ok().map(ClientReachability::new);
    let environment = DiagnosticsEnvironment {
        daemon,
        discovery_error,
        reachability: reachability
            .as_ref()
            .map(|probe| probe as &dyn jarvis_infrastructure::diagnostics::DaemonReachability),
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
    let reachability = discovered.as_ref().ok().map(ClientReachability::new);
    let environment = DiagnosticsEnvironment {
        daemon,
        discovery_error,
        reachability: reachability
            .as_ref()
            .map(|probe| probe as &dyn jarvis_infrastructure::diagnostics::DaemonReachability),
        controller: controller.as_deref().ok(),
        service_spec: spec.as_ref().ok(),
        service_spec_error: spec.as_ref().err().map(ServiceError::code),
        controller_error: controller.as_ref().err().map(ServiceError::code),
    };

    let report = collect(paths, &environment).await;
    let summary = daemon_summary(&environment).await;
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

/// Reports or changes the installed program version.
///
/// The command is plan-first and read-only by default. `status` reports the active
/// version, and every mutating action previews its exact effect unless `--confirm`
/// is passed. The install root and the profile are separate: user data is never
/// touched except by `--purge`, which additionally requires its own
/// acknowledgement, so there is no single flag that silently destroys the database.
fn install(paths: &ProfilePaths, action: Option<InstallAction>, root: Option<PathBuf>) -> ExitCode {
    let layout = match root {
        Some(root) => InstallLayout::new(InstallMode::Portable, root),
        None => match InstallLayout::standard() {
            Ok(layout) => layout,
            Err(error) => return report_install_error(&error),
        },
    };

    println!("install root: {}", layout.root().display());
    println!("install mode: {}", layout.mode().token());
    println!("profile:      {}", paths.data_dir().display());

    let state = ExistingInstall::observe(&layout);

    match action.unwrap_or(InstallAction::Status) {
        InstallAction::Status => {
            match &state.state.active {
                Some(active) => println!("active:       {active}"),
                None => println!("active:       (none)"),
            }
            match &state.state.previous {
                Some(previous) => println!("rollback to:  {previous}"),
                None => println!("rollback to:  (none recorded)"),
            }
            println!("installed:    {}", state.state.installed.len());
            for version in &state.state.installed {
                let marker = if Some(version) == state.state.active.as_ref() {
                    " (active)"
                } else if Some(version) == state.state.previous.as_ref() {
                    " (rollback target)"
                } else {
                    ""
                };
                println!("  {version}{marker}");
            }
            // The data path is printed last so an operator always sees where their
            // data is, including after an uninstall that retained it.
            println!("user data:    {}", paths.data_dir().display());
            ExitCode::from(EXIT_OK)
        }
        InstallAction::Update {
            manifest,
            signature,
            artifacts,
            target,
            confirm,
        } => install_update(
            &layout, paths, &state, &manifest, signature, artifacts, target, confirm,
        ),
        InstallAction::Rollback { confirm } => install_rollback(&layout, paths, &state, confirm),
        InstallAction::Uninstall {
            purge,
            acknowledge_purge,
            confirm,
        } => install_uninstall(&layout, paths, &state, purge, acknowledge_purge, confirm),
    }
}

/// Builds and optionally applies an install or update from a verified release.
#[allow(clippy::too_many_arguments)]
fn install_update(
    layout: &InstallLayout,
    paths: &ProfilePaths,
    state: &ExistingInstall<'_>,
    manifest: &Path,
    signature: Option<PathBuf>,
    artifacts: Option<PathBuf>,
    target: Option<String>,
    confirm: bool,
) -> ExitCode {
    let signature = signature.unwrap_or_else(|| default_signature_path(manifest));
    let directory = artifacts.unwrap_or_else(|| {
        manifest
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    });

    let manifest_bytes =
        match read_release_file(manifest, MAX_MANIFEST_BYTES, ReleaseError::ManifestTooLarge) {
            Ok(bytes) => bytes,
            Err(error) => return report_release_error(&error),
        };
    let signature_bytes = match read_release_file(
        &signature,
        MAX_SIGNATURE_BYTES,
        ReleaseError::SignatureTooLarge,
    ) {
        Ok(bytes) => bytes,
        Err(error) => return report_release_error(&error),
    };

    // The release must verify before a plan can exist. There is deliberately no
    // path that installs unverified bytes.
    let release = match VerifiedRelease::verify(&manifest_bytes, &signature_bytes, &directory) {
        Ok(release) => release,
        Err(error) => return report_install_error(&error),
    };
    let target = target.unwrap_or_else(|| release.manifest().target.clone());

    // An update when something is active, a first install otherwise. The library
    // decides which, so the CLI cannot mislabel one as the other.
    let planned = if state.state.is_installed() {
        plan_update(
            state,
            paths,
            release.manifest(),
            release.artifacts(),
            &target,
        )
    } else {
        plan_install(
            state,
            paths,
            release.manifest(),
            release.artifacts(),
            &target,
        )
    };
    let plan = match planned {
        Ok(plan) => plan,
        Err(error) => return report_install_error(&error),
    };

    print!("{}", plan.render());

    if !confirm {
        println!("\npreview only; pass --confirm to apply");
        return ExitCode::from(EXIT_OK);
    }

    match apply_install(layout, paths, &plan, Some(&release), true, false) {
        Ok(outcome) => {
            println!(
                "\nresult:  ok ({} staged, {} removed; {})",
                outcome.staged, outcome.removed, outcome.detail
            );
            ExitCode::from(EXIT_OK)
        }
        Err(error) => report_install_error(&error),
    }
}

/// Builds and optionally applies a rollback to the recorded previous version.
fn install_rollback(
    layout: &InstallLayout,
    paths: &ProfilePaths,
    state: &ExistingInstall<'_>,
    confirm: bool,
) -> ExitCode {
    let plan = match plan_rollback(state, paths) {
        Ok(plan) => plan,
        Err(error) => return report_install_error(&error),
    };
    print!("{}", plan.render());

    if !confirm {
        println!("\npreview only; pass --confirm to apply");
        return ExitCode::from(EXIT_OK);
    }

    // A rollback stages nothing: it only moves the pointer, and the target version
    // was already verified when it was installed.
    match apply_install(layout, paths, &plan, None, true, false) {
        Ok(outcome) => {
            println!("\nresult:  ok ({})", outcome.detail);
            ExitCode::from(EXIT_OK)
        }
        Err(error) => report_install_error(&error),
    }
}

/// Builds and optionally applies an uninstall, with or without a purge.
fn install_uninstall(
    layout: &InstallLayout,
    paths: &ProfilePaths,
    state: &ExistingInstall<'_>,
    purge: bool,
    acknowledge_purge: bool,
    confirm: bool,
) -> ExitCode {
    let plan = match plan_uninstall(state, paths, purge) {
        Ok(plan) => plan,
        Err(error) => return report_install_error(&error),
    };
    print!("{}", plan.render());

    if !confirm {
        println!("\npreview only; pass --confirm to apply");
        if purge && !acknowledge_purge {
            println!("note: a purge also requires --acknowledge-purge");
        }
        return ExitCode::from(EXIT_OK);
    }

    match apply_install(layout, paths, &plan, None, true, acknowledge_purge) {
        Ok(outcome) => {
            println!("\nresult:  ok ({} removed)", outcome.removed);
            if !purge {
                // The retained path is the operator's most important fact here.
                println!("user data retained at {}", paths.data_dir().display());
            }
            ExitCode::from(EXIT_OK)
        }
        Err(error) => report_install_error(&error),
    }
}

/// Reports an install failure with its code and the reason.
///
/// The code and the reason both come from the library, so the CLI cannot invent a
/// cause the operation did not actually observe.
fn report_install_error(error: &InstallError) -> ExitCode {
    eprintln!("error:  {}", error.code());
    eprintln!("reason: {error}");
    if !error.retryable() {
        eprintln!("note:   this is deterministic; repeating it will not change the outcome");
    }
    ExitCode::from(EXIT_ATTENTION)
}

/// Builds the environment record printed into a bundle manifest.
fn environment_summary(paths: &ProfilePaths) -> EnvironmentSummary {
    EnvironmentSummary::current(paths.mode().token(), API_MAJOR)
}

/// Verifies a signed release manifest and the artifacts it lists.
///
/// This command is the consumer half of `FND-011`. It answers one question the
/// user actually has: *will the bytes I am about to run be the bytes that were
/// released?* It performs two checks, and both are required.
///
/// 1. The manifest signature is verified against the **trust store compiled into
///    this binary**. No `--key` argument exists, and that is deliberate: a
///    verification against a key the caller supplies proves only that the caller
///    and the signer agreed, not that JARVIS's maintainers signed anything.
/// 2. Every listed artifact is hashed and compared. A valid manifest with an
///    altered payload is exactly the case a signature-only check misses.
///
/// The report is printed even on failure, because the *reason* is the actionable
/// part: an unknown key, a schema from the future, and a tampered byte all need
/// different operator responses and must never collapse into "verification
/// failed".
fn verify_release(
    manifest: &Path,
    signature: Option<PathBuf>,
    artifacts: Option<PathBuf>,
) -> ExitCode {
    let signature = signature.unwrap_or_else(|| default_signature_path(manifest));
    let directory = artifacts.unwrap_or_else(|| {
        manifest
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    });

    println!("manifest:   {}", manifest.display());
    println!("signature:  {}", signature.display());
    println!("artifacts:  {}", directory.display());

    // The trust store is compiled in and printed, so a verification that used the
    // non-production test key is visibly different from a production one. A test
    // key must never be mistaken for a production guarantee.
    let store = TrustStore::builtin();
    let test_only = store.contains(TEST_KEY_ID);
    println!("trust:      {} key(s)", store.len());
    if test_only {
        println!("warning:    this build trusts only the NON-PRODUCTION test key");
        println!("            ({TEST_KEY_ID}); a pass is not a production guarantee");
    }

    let manifest_bytes =
        match read_release_file(manifest, MAX_MANIFEST_BYTES, ReleaseError::ManifestTooLarge) {
            Ok(bytes) => bytes,
            Err(error) => return report_release_error(&error),
        };
    let signature_bytes = match read_release_file(
        &signature,
        MAX_SIGNATURE_BYTES,
        ReleaseError::SignatureTooLarge,
    ) {
        Ok(bytes) => bytes,
        Err(error) => return report_release_error(&error),
    };

    let verified = match store.verify_manifest(&manifest_bytes, &signature_bytes) {
        Ok(verified) => verified,
        Err(error) => return report_release_error(&error),
    };

    println!("verified:   signature and manifest shape");
    println!("version:    {}", verified.version);
    println!("channel:    {}", verified.channel);
    println!("target:     {}", verified.target);
    println!("build:      {}", verified.build);
    println!("published:  {}", verified.published_at);
    println!("artifacts:  {}\n", verified.artifacts.len());

    let verified_artifacts = match verify_artifacts(&verified, &directory) {
        Ok(verified_artifacts) => verified_artifacts,
        Err(error) => return report_release_error(&error),
    };

    for artifact in &verified_artifacts {
        println!(
            "  ok  {} ({}, {} bytes)",
            artifact.file, artifact.target, artifact.size
        );
    }
    println!(
        "\nverified {} artifact(s); every listed byte matches its signed digest",
        verified_artifacts.len()
    );
    ExitCode::from(EXIT_OK)
}

/// Reports a release verification failure with its code and the reason.
///
/// The code and the error both come from the library, so the CLI cannot invent a
/// cause the verification did not actually observe.
fn report_release_error(error: &ReleaseError) -> ExitCode {
    eprintln!("error:  {}", error.code());
    eprintln!("reason: {error}");
    if !error.retryable() {
        eprintln!("note:   this is deterministic; retrying the same bytes will not change it");
    }
    ExitCode::from(EXIT_ATTENTION)
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
    let reachability = discovered.as_ref().ok().map(ClientReachability::new);
    let environment = DiagnosticsEnvironment {
        daemon,
        discovery_error,
        reachability: reachability
            .as_ref()
            .map(|probe| probe as &dyn jarvis_infrastructure::diagnostics::DaemonReachability),
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
    // A daemon rejection is reported with the daemon's **own** code, because
    // `jarvis.daemon_rejected` only says a rejection occurred — an operator reading it
    // cannot tell an unknown run from a reused idempotency key or a rejected credential.
    match error.daemon_code() {
        Some(code) => eprintln!("error: {code}"),
        None => eprintln!("error: {}", error.code()),
    }
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
    use super::{
        Cli, Command, InstallAction, StatusBody, idempotency_key, json_string, parse_sse,
        parse_status,
    };
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
            (
                vec!["jarvis", "verify-release", "--manifest", "m.json"],
                "verify-release",
            ),
            (vec!["jarvis", "install"], "install"),
            (vec!["jarvis", "ask", "hello"], "ask"),
            (vec!["jarvis", "runs", "show", "abc"], "runs"),
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
                Command::VerifyRelease { .. } => "verify-release",
                Command::Install { .. } => "install",
                Command::Ask { .. } => "ask",
                Command::Runs { .. } => "runs",
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn a_bare_install_command_requests_no_change() {
        // The safety property: the default action is read-only, so the shortest
        // invocation cannot change an installed product.
        let cli = Cli::try_parse_from(["jarvis", "install"]).expect("parses");
        assert!(
            matches!(
                cli.command,
                Command::Install { action: None, .. }
                    | Command::Install {
                        action: Some(InstallAction::Status),
                        ..
                    }
            ),
            "a bare `install` must not select a change",
        );

        // Every mutating action must default to preview, so `--confirm` is the only
        // way to apply one.
        for arguments in [
            vec!["jarvis", "install", "rollback"],
            vec!["jarvis", "install", "uninstall"],
            vec!["jarvis", "install", "uninstall", "--purge"],
        ] {
            let cli = Cli::try_parse_from(arguments.clone()).expect("parses");
            let Command::Install {
                action:
                    Some(
                        InstallAction::Rollback { confirm }
                        | InstallAction::Uninstall { confirm, .. },
                    ),
                ..
            } = cli.command
            else {
                unreachable!("expected a mutating install action");
            };
            assert!(!confirm, "{arguments:?} must not confirm by default");
        }
    }

    #[test]
    fn a_purge_requires_its_own_acknowledgement_and_a_confirmation() {
        // Two separate flags, because a purge destroys the database. One flag that
        // did both would let "confirm" be reached without the destructive intent.
        let cli = Cli::try_parse_from(["jarvis", "install", "uninstall", "--purge", "--confirm"])
            .expect("parses");
        match cli.command {
            Command::Install {
                action:
                    Some(InstallAction::Uninstall {
                        purge,
                        acknowledge_purge,
                        confirm,
                    }),
                ..
            } => {
                assert!(purge, "--purge must select a purge");
                assert!(confirm, "--confirm must select application");
                assert!(
                    !acknowledge_purge,
                    "the acknowledgement must be a separate, deliberate flag",
                );
            }
            _ => unreachable!("expected an uninstall action"),
        }

        let acknowledged = Cli::try_parse_from([
            "jarvis",
            "install",
            "uninstall",
            "--purge",
            "--acknowledge-purge",
        ])
        .expect("parses");
        match acknowledged.command {
            Command::Install {
                action:
                    Some(InstallAction::Uninstall {
                        acknowledge_purge, ..
                    }),
                ..
            } => assert!(acknowledge_purge),
            _ => unreachable!("expected an uninstall action"),
        }

        // A bare uninstall must not imply a purge.
        let bare = Cli::try_parse_from(["jarvis", "install", "uninstall"]).expect("parses");
        match bare.command {
            Command::Install {
                action: Some(InstallAction::Uninstall { purge, .. }),
                ..
            } => assert!(!purge, "a bare uninstall must retain user data"),
            _ => unreachable!("expected an uninstall action"),
        }
    }

    #[test]
    fn an_explicit_install_root_is_accepted_before_the_install_subcommand() {
        // The root belongs to the `install` group, so it precedes the action. The
        // install root is also separate from the global `--profile`: they are
        // different directories and must not be conflated.
        let cli = Cli::try_parse_from(["jarvis", "install", "--root", "/tmp/i", "status"])
            .expect("parses");
        match cli.command {
            Command::Install { root, .. } => {
                assert_eq!(root, Some(std::path::PathBuf::from("/tmp/i")));
            }
            _ => unreachable!("expected an install command"),
        }
        assert_eq!(cli.profile, None, "the profile root is a separate option");

        // Without it, the standard per-user location is used, so the common
        // invocation stays one word.
        let standard = Cli::try_parse_from(["jarvis", "install", "status"]).expect("parses");
        match standard.command {
            Command::Install { root, .. } => assert!(root.is_none()),
            _ => unreachable!("expected an install command"),
        }
    }

    #[test]
    fn verify_release_requires_a_manifest_and_accepts_no_signing_key() {
        // A manifest is required: a bare `verify-release` has nothing to check.
        assert!(Cli::try_parse_from(["jarvis", "verify-release"]).is_err());

        // The important property: there is no `--key` option. A verification
        // against a key the caller supplies proves only that the caller and the
        // signer agreed, which is not a trust decision. Offering the flag at all
        // would invite treating it as one.
        for spelling in ["--key", "--public-key", "--trust"] {
            assert!(
                Cli::try_parse_from([
                    "jarvis",
                    "verify-release",
                    "--manifest",
                    "m.json",
                    spelling,
                    "k"
                ])
                .is_err(),
                "{spelling} must not be accepted",
            );
        }

        // The optional paths default to the manifest's own location and its
        // `.sig` sidecar rather than to nothing, so the common invocation is one
        // argument.
        let cli = Cli::try_parse_from(["jarvis", "verify-release", "--manifest", "rel/m.json"])
            .expect("parses");
        match cli.command {
            Command::VerifyRelease {
                manifest,
                signature,
                artifacts,
            } => {
                assert_eq!(manifest, std::path::PathBuf::from("rel/m.json"));
                assert!(signature.is_none(), "the sidecar is the default");
                assert!(artifacts.is_none(), "the manifest directory is the default");
            }
            _ => unreachable!("verify-release must parse to its own variant"),
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
                    | Command::Repair { .. }
                    | Command::VerifyRelease { .. }
                    | Command::Install { .. }
                    | Command::Ask { .. }
                    | Command::Runs { .. } => String::from("unexpected"),
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

    #[test]
    fn a_run_event_stream_is_parsed_into_ordered_frames() {
        // The parser is the client's half of the SSE contract, so it is asserted against
        // the exact framing the daemon renders: an `id`, an `event`, a `data` document,
        // and a blank-line terminator.
        let body = concat!(
            "id: 0195f4f1-0475-7613-a92c-edf01183e909\n",
            "event: run.output_text.delta\n",
            "data: {\"payload\":{\"item_id\":\"out-1\",\"delta\":\"Hello\"}}\n",
            "\n",
            ": keepalive\n",
            "\n",
            "id: 0195f4f1-0475-7613-a92c-edf01183e90a\n",
            "event: run.completed\n",
            "data: {\"payload\":null}\n",
            "\n",
        );
        let frames = parse_sse(body);
        assert_eq!(frames.len(), 2, "a keepalive is not an event");
        assert_eq!(frames[0].event, "run.output_text.delta");
        assert_eq!(frames[0].payload("delta").as_deref(), Some("Hello"));
        assert_eq!(
            frames[0].id.as_deref(),
            Some("0195f4f1-0475-7613-a92c-edf01183e909"),
        );
        assert_eq!(frames[1].event, "run.completed");
        // The terminal event's payload is not an object, so a field read yields nothing
        // rather than a fabricated value.
        assert_eq!(frames[1].payload("delta"), None);
    }

    #[test]
    fn a_question_is_encoded_so_it_cannot_corrupt_the_request_body() {
        // The encoder exists so a quote or a newline in a question cannot produce a body
        // the daemon refuses — which would look like a client bug rather than a quoting
        // one.
        assert_eq!(json_string("hello"), "\"hello\"");
        assert_eq!(json_string("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        assert_eq!(json_string("back\\slash"), "\"back\\\\slash\"");
        assert_eq!(json_string("tab\there"), "\"tab\\there\"");
        // A control character that has no short escape is unicode-escaped, so it cannot
        // appear raw in a JSON string.
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
        // A multi-byte character is passed through unchanged rather than mangled.
        assert_eq!(json_string("héllo — ok"), "\"héllo — ok\"");
    }

    #[test]
    fn an_idempotency_key_is_fresh_per_command_and_is_not_a_secret() {
        // The key only has to differ between two invocations, because the contract's
        // idempotency is about a *repeat with the same key*. It is deliberately not
        // random: it is not a credential and never grants anything.
        let first = idempotency_key();
        let second = idempotency_key();
        assert_ne!(first, second, "two commands must not share a key");
        assert!(first.starts_with("cli-"), "{first}");
        assert!(!first.contains('\0'), "{first}");
    }

    #[test]
    fn a_bare_runs_command_cannot_cancel_a_run() {
        // `cancel` is a named subcommand, so the shortest invocation is read-only. This
        // is the same safety property `service` and `install` have.
        assert!(Cli::try_parse_from(["jarvis", "runs"]).is_err());
        for word in ["show", "events", "cancel"] {
            let cli = Cli::try_parse_from(["jarvis", "runs", word, "abc"]).expect("parses");
            assert!(
                matches!(cli.command, Command::Runs { .. }),
                "runs {word} must parse",
            );
        }
        // A resume position is an option, not a positional, so it cannot be mistaken for
        // a run identifier.
        assert!(
            Cli::try_parse_from(["jarvis", "runs", "events", "abc", "--last-event-id", "id"])
                .is_ok()
        );
    }
}
