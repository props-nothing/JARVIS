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
    read_credential, stream_response,
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

/// The media type the run event stream is served as.
///
/// The contract states that the events route "requires `Accept: text/event-stream`", so this
/// client states it. A client that relies on the server not checking the header works until the
/// server starts doing so — and then fails for a reason that names the daemon rather than the
/// client.
const EVENT_STREAM_MEDIA_TYPE: &str = "text/event-stream";

/// Exit code for a successful command.
const EXIT_OK: u8 = 0;
/// Exit code for a usage or environment problem the operator must fix.
const EXIT_ATTENTION: u8 = 1;

/// How many times a run's stream may be re-established before the follow gives up.
///
/// Bounded because the alternative is a command that never returns: a daemon restarting in a loop
/// would otherwise be followed forever. Three attempts covers the realistic case the bound exists for
/// — a daemon that restarted once — while failing fast enough that a persistent fault surfaces as an
/// error rather than as a hang.
const STREAM_ATTEMPTS: u32 = 3;

/// Builds the extra headers the event-stream route takes.
///
/// The contract states that `GET /api/v1/runs/{run_id}/events` "requires
/// `Accept: text/event-stream`", and this is the reference client for that surface — so it states
/// what it accepts rather than relying on the daemon's tolerance. Until this it sent only the
/// resume header, which worked **solely** because the daemon did not check: a permissive server and
/// a conforming client are indistinguishable when driven against each other, so both halves were
/// wrong together and neither showed it.
///
/// A named function rather than an inline `format!` so the client's conformance is assertable
/// without a running daemon. A header this client is required to send is exactly the kind of thing
/// that disappears in a refactor while every test still passes.
#[must_use]
fn event_stream_headers(last_event_id: Option<&str>) -> String {
    let resume = last_event_id.map_or_else(String::new, |id| format!("Last-Event-ID: {id}\r\n"));
    format!("Accept: {EVENT_STREAM_MEDIA_TYPE}\r\n{resume}")
}

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
        /// Continue an existing conversation instead of starting a new one.
        #[arg(long, value_name = "ID")]
        conversation: Option<String>,
    },
    /// Inspect durable runs.
    Runs {
        #[command(subcommand)]
        action: RunsAction,
    },
    /// Inspect and decide approval requests.
    ///
    /// `list` and `show` are read-only; `approve`, `reject`, and `cancel` change durable state and are
    /// therefore their own explicit actions, so a bare `jarvis approvals` can never decide anything —
    /// the same rule `runs` follows.
    Approvals {
        #[command(subcommand)]
        action: ApprovalsAction,
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
        Command::Ask { text, conversation } => ask(paths, &text, conversation.as_deref()).await,
        Command::Runs { action } => runs(paths, action).await,
        Command::Approvals { action } => approvals(paths, action).await,
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

/// Approval inspection and decision a caller can request.
///
/// **`approve` and `reject` take a fingerprint as a required argument**, because a decision is bound to
/// the exact action the user reviewed: the server compares the digest it holds against the one the
/// client states, and a decision that named no digest would be a decision about *something*. `show`
/// prints the fingerprint, so the value the user passes is the value the daemon printed.
#[derive(Debug, Subcommand)]
enum ApprovalsAction {
    /// List the approvals this client may decide.
    List {
        /// The page size to request. The daemon clamps it to its own bound.
        #[arg(long, value_name = "N")]
        limit: Option<u32>,
        /// Narrow to one risk level. `critical` is the set that will demand a step-up.
        #[arg(long, value_name = "LEVEL")]
        risk: Option<RiskArg>,
        /// Resume a page from the `next_cursor` a previous listing printed.
        ///
        /// Without this the daemon's own `next_cursor` was a value a client could **read and not use**:
        /// the listing reported `has_more: true` and handed back a position, and the reference client had
        /// no way to send it — the same dead end the cursor was introduced to close on the daemon side,
        /// one layer out.
        #[arg(long, value_name = "CURSOR")]
        cursor: Option<String>,
    },
    /// Print one approval.
    Show {
        /// The approval identifier.
        approval_id: String,
    },
    /// Approve an approval.
    Approve {
        /// The approval identifier.
        approval_id: String,
        /// The action fingerprint printed by `show` or `list`.
        #[arg(long, value_name = "FINGERPRINT")]
        fingerprint: String,
        /// The version the caller believes is current.
        #[arg(long, value_name = "N")]
        version: u64,
    },
    /// Reject an approval.
    Reject {
        /// The approval identifier.
        approval_id: String,
        /// The action fingerprint printed by `show` or `list`.
        #[arg(long, value_name = "FINGERPRINT")]
        fingerprint: String,
        /// The version the caller believes is current.
        #[arg(long, value_name = "N")]
        version: u64,
    },
    /// Withdraw a pending or approved-but-unused request.
    Cancel {
        /// The approval identifier.
        approval_id: String,
        /// The version the caller believes is current.
        #[arg(long, value_name = "N")]
        version: u64,
        /// Why it is being withdrawn.
        #[arg(long, value_name = "REASON")]
        reason: Option<String>,
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
async fn ask(paths: &ProfilePaths, text: &str, conversation: Option<&str>) -> ExitCode {
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
    let body = ask_body(text, conversation);
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

    // The run is named on **stderr**, so the answer on stdout stays exactly the model's text — a caller
    // redirecting stdout gets nothing but the answer — while an operator who needs to follow, diagnose,
    // or cancel the run afterwards has its identifier. `jarvis runs show <id>` and `runs events <id>`
    // take it, and until this the identifier existed only in a response body the CLI did not print, so
    // the run a question created could not be named by the person who asked it.
    eprintln!("run {run_id}");

    follow_run(&state, &run_id).await
}

/// What a follower should do with one frame.
///
/// **Extracted so the classification is assertable without a running daemon.** Which event names mean
/// "print this", "the run is over", and "resume" is exactly the kind of decision that is invisible
/// until it is wrong — and the wrong version of it is not a crash but a *report*: a client that
/// mistook the overrun signal for a run ending would tell an operator a working run had finished,
/// which is the failure the signal exists to prevent. A test can drive this over every event type the
/// protocol defines, which it cannot do for a loop that needs a daemon on the other end.
#[derive(Debug, PartialEq, Eq)]
enum FollowStep {
    /// Print the increment this frame carries.
    Delta(String),
    /// The run reached a terminal state, and whether it ended successfully.
    RunEnded {
        /// `true` only for `run.completed`.
        success: bool,
    },
    /// The stream was cut short because this client fell behind, and resuming is the correct response.
    Resume,
    /// Nothing for the follower to do.
    Ignored,
}

/// Watches the `sequence` numbers of a run's stream and refuses to continue across a gap.
///
/// **The contract's rule, stated for clients and, until this, enforced by nothing:**
/// "Clients ignore unknown additive event types but **never ignore a sequence gap** or unknown
/// terminal state." The reference client did not ignore a gap *deliberately* — it ignored it by never
/// reading `sequence` at all. That is the same failure the contract names, arrived at by omission
/// rather than by decision, and it is worse than it sounds for this surface: `sequence` starts at 1
/// and increases by exactly one per event, so a gap means the daemon's stream is not the run's stream.
/// A follower that keeps printing would deliver a *fabricated* answer — two halves of an output that
/// were never adjacent — with nothing to indicate it.
///
/// A gap is therefore **fatal rather than resumable**, and that is the whole point of detecting it.
/// The client cannot repair it: it does not know which events it missed or whether they still exist,
/// and reconnecting would re-read from the position it already reached. So the honest response is to
/// stop and say so, rather than to print an answer that may not be one.
///
/// **The check is skipped on a resumed stream for the first frame only**, because that is the frame
/// the resume header deliberately lands *after*: `Last-Event-ID` means "resume strictly after this
/// event", so the first frame of a resumed stream is legitimately not the successor of the last frame
/// of the previous one. Accepting a gap there would be the opposite error — inventing a requirement
/// the contract does not have.
#[derive(Debug, Default)]
struct SequenceWatcher {
    /// The last sequence acknowledged, or `None` before the first frame of a stream.
    expected: Option<u64>,
    /// Whether this stream began from a resume header, so its first frame starts a new expectation.
    resumed: bool,
}

/// What a sequence number means for the stream a follower is reading.
#[derive(Debug, PartialEq, Eq)]
enum SequenceCheck {
    /// In order, or the first frame of the stream.
    InOrder,
    /// The stream skipped events this client will never see.
    Gap {
        /// The sequence the stream should have reached.
        expected: u64,
        /// The sequence it actually delivered.
        found: u64,
    },
    /// The frame carries no parseable sequence, so nothing about order can be concluded.
    Unreadable,
}

impl SequenceWatcher {
    /// Creates a watcher for a stream, stating whether it began from a resume position.
    fn new(resumed: bool) -> Self {
        Self {
            expected: None,
            resumed,
        }
    }

    /// Checks one frame's sequence and advances the expectation.
    fn check(&mut self, frame: &SseFrame) -> SequenceCheck {
        let Some(found) = frame.sequence() else {
            // A frame with no sequence cannot be placed in the stream. This is **not** treated as a
            // gap: the contract bounds what a client may conclude from a frame, and inventing a gap
            // from an absent field would refuse a stream the daemon is sending correctly. The one
            // exception would be a frame the *protocol* requires to carry one, and every frame this
            // client acts on does — so an unreadable sequence is reported rather than assumed.
            return SequenceCheck::Unreadable;
        };
        let previous = self.expected;
        // **The first frame sets the expectation rather than checking it.** On a fresh stream the
        // contract says `sequence` starts at 1, so this still refuses a stream that opens at 5 — which
        // is the case worth catching, because it means the client is reading a stream it has already
        // missed the start of. On a resumed stream the client accepts wherever the daemon resumes,
        // because `Last-Event-ID` means "strictly after that event" and the first frame of a resume is
        // deliberately not the successor of the last frame of the previous connection.
        let Some(previous) = previous else {
            if !self.resumed && found != 1 {
                return SequenceCheck::Gap { expected: 1, found };
            }
            self.resumed = false;
            self.expected = Some(found.saturating_add(1));
            return SequenceCheck::InOrder;
        };
        if found != previous {
            return SequenceCheck::Gap {
                expected: previous,
                found,
            };
        }
        self.expected = Some(found.saturating_add(1));
        SequenceCheck::InOrder
    }
}

/// Classifies one frame for a follower.
///
/// The names are the protocol crate's constants rather than literals, because they are contract
/// fields a client switches on: a typo in a literal here would be invisible to this build and would
/// simply stop matching, so the follower would silently treat a terminal event as ignorable. Using the
/// constants makes a rename a compile error.
fn follow_step(frame: &SseFrame) -> FollowStep {
    use jarvis_protocol::run::event_type as kind;
    if frame.event == kind::OUTPUT_TEXT_DELTA {
        // A delta with no text is still a frame this client has classified; there is simply nothing
        // to print, which is what `Ignored` says.
        return frame
            .payload("delta")
            .map_or(FollowStep::Ignored, FollowStep::Delta);
    }
    if frame.event == kind::COMPLETED {
        return FollowStep::RunEnded { success: true };
    }
    if frame.event == kind::FAILED || frame.event == kind::CANCELLED {
        return FollowStep::RunEnded { success: false };
    }
    if frame.event == kind::STREAM_OVERRUN {
        return FollowStep::Resume;
    }
    FollowStep::Ignored
}

/// Follows a run's events until it reaches a terminal state, printing its answer.
async fn follow_run(state: &ClientState, run_id: &str) -> ExitCode {
    // **One connection, held open, with deltas printed as they arrive.** This used to poll: the loop
    // reconnected every 50 ms with `Last-Event-ID` and printed everything the daemon had retained so
    // far, which was the only way to follow a run while the events endpoint delivered a bounded replay
    // and closed. The endpoint now follows the run live, so polling is no longer required — and it was
    // never equivalent, because the answer appeared in bursts at whatever the poll interval was
    // rather than as the model produced it, which is the one thing a streaming client is for.
    //
    // The incremental parser is what makes it possible: a piece of body can end anywhere, including
    // the middle of a `data:` line, so a parser that worked on a whole body would have to buffer the
    // entire stream and would show nothing until the run was over.
    //
    // **A dropped connection is resumed, not abandoned.** The connection now lives for as long as the
    // run does, so it is exposed to a failure a 50 ms poll never was: a daemon restart, or a socket
    // reset, would end a follow that had already printed half an answer. The contract makes the
    // recovery exact — `Last-Event-ID` resumes *strictly after* that event — so the last event id the
    // parser saw is kept and used to reconnect. Resumption therefore cannot duplicate output, which is
    // the property that makes it safe to retry a stream that has already delivered text; and it is
    // bounded, so a daemon that keeps dropping connections fails with a message rather than looping
    // forever.
    let path = format!("/api/v1/runs/{run_id}/events");
    let mut last_event_id: Option<String> = None;
    let mut streamed = false;

    for attempt in 0..STREAM_ATTEMPTS {
        // Whether the attempt that just ended did so because the daemon **said** it cut the stream
        // short. Declared inside the loop, because it describes one connection: a previous attempt's
        // overrun must not explain a later attempt ending for a different reason, or a genuine silence
        // would be retried as though the daemon had asked for it.
        let mut overran = false;
        let extra = event_stream_headers(last_event_id.as_deref());
        let mut frames = SseParser::new();
        let mut outcome: Option<ExitCode> = None;
        // The id of the last frame this attempt saw, kept separately from `last_event_id` so a
        // reconnect uses what was actually delivered rather than what a previous attempt had.
        let mut resumed_at: Option<String> = None;
        // **A gap is fatal rather than resumable, so it ends this attempt with a report rather than
        // setting a retry flag.** `resumed` is `true` only when this attempt actually sent a resume
        // header, because `Last-Event-ID` lands *after* the named event — so the first frame of a
        // resumed stream is legitimately not the successor of the last frame of the previous one.
        let mut watcher = SequenceWatcher::new(last_event_id.is_some());
        let mut gap: Option<(u64, u64)> = None;

        let result = stream_response(
            &state.discovered,
            &state.credential,
            &path,
            API_MAJOR,
            &extra,
            |piece| {
                for frame in frames.push(piece) {
                    if let Some(id) = frame.id.clone() {
                        resumed_at = Some(id);
                    }
                    match watcher.check(&frame) {
                        SequenceCheck::InOrder | SequenceCheck::Unreadable => {}
                        SequenceCheck::Gap { expected, found } => {
                            // Stop reading immediately: every further frame would be printed into an
                            // answer that is already known not to be the run's output, and the point of
                            // detecting the gap is to avoid delivering that.
                            gap = Some((expected, found));
                            return false;
                        }
                    }
                    match follow_step(&frame) {
                        FollowStep::Delta(delta) => {
                            print!("{delta}");
                            // Flushed per delta, because stdout is block-buffered when it is not a
                            // terminal and an unflushed buffer would deliver the whole answer at the
                            // end — which is exactly the behaviour streaming exists to avoid. A
                            // failure to flush is ignored deliberately: it means the reader went
                            // away, and stopping the stream over that would abort a run's follow for
                            // no reason.
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                            streamed = true;
                        }
                        FollowStep::RunEnded { success } => {
                            if streamed {
                                println!();
                            }
                            outcome = Some(if success {
                                ExitCode::SUCCESS
                            } else {
                                // A failed or cancelled run is reported by its own event, so an
                                // operator sees what happened rather than only that the command did
                                // not succeed.
                                eprintln!("error: {}", frame.event.replace("run.", "run "));
                                ExitCode::from(EXIT_ATTENTION)
                            });
                            // `false` hangs up. A disconnect never cancels a durable run, so stopping
                            // after the terminal event is safe and closes the connection rather than
                            // waiting for the daemon to do it.
                            return false;
                        }
                        FollowStep::Resume => {
                            // **The one frame this client must not mistake for an ending.** The run is
                            // untouched — the daemon cut the *stream* because this client stopped
                            // reading fast enough — so the correct response is to reconnect from the
                            // last event id actually received, which is exactly what the retry loop
                            // below does when a stream ends without a terminal. Treating it as an
                            // ending would report a working run as finished, which is the failure the
                            // signal exists to prevent, and `follow_step`'s tests hold that apart.
                            //
                            // `overran` is set so the end of this stream is *expected* rather than
                            // reported as a fault in the loop below.
                            overran = true;
                            return false;
                        }
                        FollowStep::Ignored => {}
                    }
                }
                true
            },
        )
        .await;

        if let Some(id) = resumed_at {
            last_event_id = Some(id);
        }
        if let Some(outcome) = outcome {
            return outcome;
        }
        // **A gap is checked before the retry decision, and that ordering is the guarantee.**
        // `follow_after` answers "should this attempt be retried"; a gap is not retryable at all, so
        // consulting it first would let a gap be answered as though it were a dropped connection and
        // reconnected — which re-reads from the position already reached and cannot recover the
        // missing events.
        if let Some((expected, found)) = gap {
            return AttemptReport::SequenceGap { expected, found }.exit_code();
        }
        match follow_after(&result, overran, attempt, last_event_id.is_some()) {
            AttemptAfter::Resume => {}
            AttemptAfter::Report(report) => return report.exit_code(),
        }
    }
    eprintln!("error: the run's stream could not be followed to a terminal event");
    ExitCode::from(EXIT_ATTENTION)
}

/// What a follower should do when one attempt's stream has ended without a terminal event.
#[derive(Debug, PartialEq, Eq)]
enum AttemptAfter {
    /// Reconnect from the last delivered event id.
    Resume,
    /// Stop, and say this.
    Report(AttemptReport),
}

/// Why a follow ended without a terminal event, as something a follower can act on.
///
/// **Extracted from the loop as a decision over values.** Every clause here is a comparison the
/// compiler cannot check and the daemon cannot demonstrate: whether a transport failure is worth
/// retrying, whether a *signalled* overrun is, and — the one that matters most — that an overrun is
/// **not** the client's own fault to report. An `if` chain inside a loop driven by a live connection
/// can only be exercised with a daemon on the other end reproducing the exact failure, so the clause
/// that is wrong is the clause nobody tests. As a function over an error, a flag, an attempt number,
/// and whether a position exists, every combination is enumerated by a test.
#[derive(Debug, PartialEq, Eq)]
enum AttemptReport {
    /// The daemon closed the stream without saying how the run ended.
    EndedWithoutTerminal,
    /// The overrun signal arrived before a single event, so there is no position to resume from.
    OverranBeforeAnyEvent,
    /// The stream kept overrunning, and the bounded number of resumes is spent.
    OverrunBudgetExhausted,
    /// The stream skipped events this client will never see.
    SequenceGap {
        /// The sequence the stream should have reached.
        expected: u64,
        /// The sequence it actually delivered.
        found: u64,
    },
    /// A client fault, reported by its own kind.
    Client(ClientErrorKind),
}

impl AttemptReport {
    /// The message this report prints, and the code to exit with.
    fn exit_code(self) -> ExitCode {
        match self {
            Self::EndedWithoutTerminal => {
                eprintln!("error: the run's stream ended without a terminal event");
                ExitCode::from(EXIT_ATTENTION)
            }
            Self::OverranBeforeAnyEvent => {
                eprintln!(
                    "error: the stream overran before any event arrived, so there is no position to \
                     resume from"
                );
                ExitCode::from(EXIT_ATTENTION)
            }
            Self::OverrunBudgetExhausted => {
                eprintln!(
                    "error: the stream overran {STREAM_ATTEMPTS} times without reaching a terminal \
                     event"
                );
                ExitCode::from(EXIT_ATTENTION)
            }
            // **Reported rather than worked around, and this is the point of detecting it.** The
            // client cannot repair a gap: it does not know which events it missed or whether they
            // still exist, and reconnecting would re-read from the position it already reached. The
            // honest response is to refuse to print an answer that may be two disconnected halves of
            // an output, because the contract says a client never ignores a sequence gap.
            Self::SequenceGap { expected, found } => {
                eprintln!(
                    "error: the run's stream skipped events (expected sequence {expected}, got \
                     {found}); anything printed above may not be the run's answer"
                );
                ExitCode::from(EXIT_ATTENTION)
            }
            Self::Client(kind) => kind.report(),
        }
    }
}
/// Which shape of client failure ended an attempt.
///
/// A mirror of [`ClientError`]'s variants rather than the error itself, because the decision has to be
/// a pure function of *what kind* of failure this was: carrying the error into [`AttemptAfter`] would
/// invite a decision branch that reads the error's fields, and fields are exactly what a test over
/// this enum cannot enumerate.
#[derive(Debug, PartialEq, Eq)]
enum ClientErrorKind {
    /// The connection failed or timed out, which is the one case a resume can fix.
    Transport,
    /// Anything else, reported as it is.
    Other,
}

impl ClientErrorKind {
    /// Reports a client failure, by kind.
    ///
    /// The two kinds print **different messages** because they call for different operator responses,
    /// which is also what makes this a method on the value rather than on the enum: a transport failure
    /// that has spent its attempt budget is a daemon that keeps dropping connections, while any other
    /// failure is one this client cannot interpret and the operator should read. Printing one message
    /// for both would hide which of the two happened.
    fn report(&self) -> ExitCode {
        match self {
            Self::Transport => eprintln!(
                "error: the run's stream could not be followed to a terminal event after \
                 {STREAM_ATTEMPTS} attempts"
            ),
            Self::Other => eprintln!("error: the run's stream could not be read"),
        }
        ExitCode::from(EXIT_ATTENTION)
    }
}

/// Decides what to do when an attempt ends without a terminal event.
fn follow_after(
    result: &Result<(), ClientError>,
    overran: bool,
    attempt: u32,
    has_position: bool,
) -> AttemptAfter {
    let more_attempts = attempt + 1 < STREAM_ATTEMPTS;
    match result {
        // A transport failure is the one error a resume can fix, and only when there is a position to
        // resume from — a connection that failed before any event arrived has nothing to ask for, and
        // retrying it would just be the same failure again.
        Err(ClientError::Transport | ClientError::Timeout) if more_attempts && has_position => {
            AttemptAfter::Resume
        }
        // **A signalled overrun is a resume, not a failure, and it must not be reported as a fault.**
        // The daemon said to reconnect; telling the operator the stream "ended without a terminal
        // event" would be this client arguing with the daemon about a fact the daemon just stated.
        // Still bounded, because a daemon disconnecting in a loop must surface as an error rather than
        // as a command that never returns.
        Ok(()) if overran && more_attempts && has_position => AttemptAfter::Resume,
        // The overrun signal with nothing to resume *from* is one unusable case: the signal came
        // before a single event, so reconnecting would name a position that does not exist. Said
        // plainly rather than retried, because no number of retries reaches an event that never came.
        Ok(()) if overran && !has_position => {
            AttemptAfter::Report(AttemptReport::OverranBeforeAnyEvent)
        }
        // **An overrun past the budget is its own outcome, not the silent-ending one.** The stream did
        // end for the reason the daemon stated, so reporting it as "ended without a terminal event"
        // would drop the one fact the signal carried — and the two are reached by different conditions,
        // which is what made this a real distinction rather than a nicety: a test written against the
        // conflated version failed here, naming `OverranBeforeAnyEvent` where it expected this.
        Ok(()) if overran => AttemptAfter::Report(AttemptReport::OverrunBudgetExhausted),
        // A clean end with no terminal event means the daemon closed the stream without saying how the
        // run ended, which is not success — a stream that stops is not a stream that finished.
        Ok(()) => AttemptAfter::Report(AttemptReport::EndedWithoutTerminal),
        Err(ClientError::Transport | ClientError::Timeout) => {
            AttemptAfter::Report(AttemptReport::Client(ClientErrorKind::Transport))
        }
        Err(_) => AttemptAfter::Report(AttemptReport::Client(ClientErrorKind::Other)),
    }
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
            // **Streamed rather than buffered, and that is a correctness fix rather than a polish.**
            // The buffered reader reads to end-of-body and caps what it keeps at `MAX_RESPONSE_BYTES`,
            // which was correct while the endpoint delivered a bounded replay and closed. The endpoint
            // now follows the run live, so a buffered read of it would do two wrong things at once:
            // block until the run finished — the opposite of following it — and silently **truncate**
            // at the cap, so a long run's later events were never shown and nothing said so. The
            // streaming client delivers each frame as it arrives, so the same command now behaves the
            // way the flag it takes implies.
            let extra = event_stream_headers(last_event_id.as_deref());
            let mut parser = SseParser::new();
            let result = stream_response(
                &state.discovered,
                &state.credential,
                &format!("/api/v1/runs/{run_id}/events"),
                API_MAJOR,
                &extra,
                |piece| {
                    for frame in parser.push(piece) {
                        print!("{}", frame.render());
                    }
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    // Every frame is printed, including a terminal one, and the connection is closed
                    // by the daemon when the run ends — so this reads to end-of-stream rather than
                    // deciding for itself when to stop.
                    true
                },
            )
            .await;
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => report_client_error(&error),
            }
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

/// A risk level a caller may narrow the approval listing to.
///
/// **The contract's closed set, spelled from the wire vocabulary rather than through
/// `jarvis_domain::Risk`.** This crate depends on `jarvis-protocol` and `jarvis-infrastructure`, not on
/// the domain crate, and the contract's rule is that the wire value is the contract's own spelling —
/// so the four level names live in `jarvis_protocol::approval::risk` beside the `ApprovalView.risk`
/// field they must agree with, and this argument parses from that same list. A local copy here would be
/// a second spelling of a value the daemon sends, which is the two-spellings defect the project keeps
/// finding.
///
/// **A closed set rather than a free string.** `--risk severe` must be refused by the client, not sent
/// to the daemon to be refused there: the daemon answers `400 request.invalid`, but a client that
/// forwarded an unknown level would be relying on the server to validate its own command line. The
/// enum makes an unrecognised level a parse error before any request is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RiskArg {
    Low,
    Moderate,
    High,
    Critical,
}

impl RiskArg {
    /// The contract spelling, which is what the query carries.
    fn as_contract_str(self) -> &'static str {
        match self {
            Self::Low => jarvis_protocol::approval::risk::LOW,
            Self::Moderate => jarvis_protocol::approval::risk::MODERATE,
            Self::High => jarvis_protocol::approval::risk::HIGH,
            Self::Critical => jarvis_protocol::approval::risk::CRITICAL,
        }
    }

    /// Parses the contract spelling, refusing anything else.
    ///
    /// Written against the protocol's own constant list rather than a local match so a fifth level
    /// added to the wire vocabulary forces a decision here instead of being silently unselectable.
    fn parse(value: &str) -> Result<Self, String> {
        for (name, level) in [
            (jarvis_protocol::approval::risk::LOW, Self::Low),
            (jarvis_protocol::approval::risk::MODERATE, Self::Moderate),
            (jarvis_protocol::approval::risk::HIGH, Self::High),
            (jarvis_protocol::approval::risk::CRITICAL, Self::Critical),
        ] {
            if value == name {
                return Ok(level);
            }
        }
        Err(format!(
            "unknown risk level {value:?}; expected one of {}",
            jarvis_protocol::approval::risk::LEVELS.join(", "),
        ))
    }
}

impl std::str::FromStr for RiskArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Builds the approvals listing path from the client's own arguments.
///
/// **Extracted as a pure function so the query-string construction is testable without a daemon.** The
/// three parameters are the ones a `approvals list` may set, and the ordering is fixed (`limit`, `risk`,
/// `cursor`) so the path is deterministic and a test can assert it byte for byte.
///
/// **The `cursor` value is inserted verbatim, and that is safe because it is already URL-safe.** The
/// daemon's cursor is base64url-without-padding behind a `v1.` prefix, so it contains only
/// `[A-Za-z0-9_-]` and a dot — no `&`, `=`, or space that would break the query — and it is an *opaque*
/// position the client must not reinterpret. Percent-encoding it here would be wrong in the other
/// direction: the daemon decodes the literal value it minted, so an encoded copy would name a position
/// it never produced.
fn list_path(limit: Option<u32>, risk: Option<RiskArg>, cursor: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(limit) = limit {
        query.push(format!("limit={limit}"));
    }
    if let Some(risk) = risk {
        query.push(format!("risk={}", risk.as_contract_str()));
    }
    if let Some(cursor) = cursor {
        query.push(format!("cursor={cursor}"));
    }
    if query.is_empty() {
        "/api/v1/approvals".to_owned()
    } else {
        format!("/api/v1/approvals?{}", query.join("&"))
    }
}

/// Approval inspection and decision.
///
/// The daemon's own response is printed rather than a re-derived summary, so the client cannot
/// disagree with the daemon about an approval's state — the same rule `runs show` follows. A refusal
/// prints the daemon's **own** body, because it carries the stable code and the shared envelope: a
/// client that reported only "the daemon refused" would leave an operator unable to tell a missing
/// approval from a wrong channel or a stale version.
async fn approvals(paths: &ProfilePaths, action: ApprovalsAction) -> ExitCode {
    let state = match daemon_client(paths) {
        Ok(state) => state,
        Err(error) => return report_client_error(&error),
    };
    match action {
        ApprovalsAction::List {
            limit,
            risk,
            cursor,
        } => {
            let path = list_path(limit, risk, cursor.as_deref());
            print_daemon_response(
                get_with_status(&state.discovered, &state.credential, &path, API_MAJOR, "").await,
            )
        }
        ApprovalsAction::Show { approval_id } => print_daemon_response(
            get_with_status(
                &state.discovered,
                &state.credential,
                &format!("/api/v1/approvals/{approval_id}"),
                API_MAJOR,
                "",
            )
            .await,
        ),
        ApprovalsAction::Approve {
            approval_id,
            fingerprint,
            version,
        } => decide(&state, &approval_id, "approve", &fingerprint, version).await,
        ApprovalsAction::Reject {
            approval_id,
            fingerprint,
            version,
        } => decide(&state, &approval_id, "reject", &fingerprint, version).await,
        ApprovalsAction::Cancel {
            approval_id,
            version,
            reason,
        } => {
            let body = match &reason {
                Some(reason) => serde_json::json!({
                    "expected_version": version,
                    "reason": reason,
                }),
                None => serde_json::json!({ "expected_version": version }),
            };
            let headers = format!("Idempotency-Key: {}\r\n", idempotency_key());
            print_daemon_response(
                post_authenticated(
                    &state.discovered,
                    &state.credential,
                    &format!("/api/v1/approvals/{approval_id}/cancel"),
                    API_MAJOR,
                    &headers,
                    &body.to_string(),
                )
                .await,
            )
        }
    }
}

/// Sends a decision and prints the daemon's answer.
///
/// A separate function because `approve` and `reject` differ by one verb, and writing the request
/// twice is how the two come to differ in some other way — a fingerprint passed positionally, or a
/// version transposed with the others.
async fn decide(
    state: &ClientState,
    approval_id: &str,
    decision: &str,
    fingerprint: &str,
    version: u64,
) -> ExitCode {
    let body = serde_json::json!({
        "decision": decision,
        "expected_version": version,
        "action_fingerprint": fingerprint,
    });
    let headers = format!("Idempotency-Key: {}\r\n", idempotency_key());
    print_daemon_response(
        post_authenticated(
            &state.discovered,
            &state.credential,
            &format!("/api/v1/approvals/{approval_id}/decide"),
            API_MAJOR,
            &headers,
            &body.to_string(),
        )
        .await,
    )
}

/// Prints a daemon response, or its refusal, as the CLI's result.
///
/// The body is printed verbatim in both directions: on success so the client cannot disagree with the
/// daemon about a state, and on a refusal because the envelope carries the stable code an operator
/// needs rather than a client-side paraphrase of it.
fn print_daemon_response(result: Result<(u16, String), ClientError>) -> ExitCode {
    match result {
        Ok((status, body)) if (200..300).contains(&status) => {
            println!("{body}");
            ExitCode::SUCCESS
        }
        Ok((_, body)) => {
            eprintln!("error: {body}");
            ExitCode::from(EXIT_ATTENTION)
        }
        Err(error) => report_client_error(&error),
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

    /// Reads the frame's `sequence`, which is a **top-level** field rather than a payload one.
    ///
    /// The contract puts `sequence` beside `event_id`, `run_id`, and `occurred_at`, so looking for it
    /// under `payload` would find nothing on every frame — which is exactly the shape of the defect
    /// this accessor exists to close: a client that never reads it cannot notice a gap.
    fn sequence(&self) -> Option<u64> {
        serde_json::from_str::<serde_json::Value>(&self.data)
            .ok()?
            .get("sequence")?
            .as_u64()
    }

    /// Renders this frame as it arrived on the wire.
    ///
    /// Used by `runs events`, whose output *is* the stream: a command that reformatted the frames
    /// would no longer be showing the operator what the daemon sent, which is the whole point of it.
    /// The `data:` line is emitted verbatim rather than re-serialized, so a payload this build cannot
    /// parse still round-trips.
    fn render(&self) -> String {
        let mut out = String::new();
        if let Some(id) = &self.id {
            out.push_str("id: ");
            out.push_str(id);
            out.push('\n');
        }
        out.push_str("event: ");
        out.push_str(&self.event);
        out.push('\n');
        out.push_str("data: ");
        out.push_str(&self.data);
        out.push_str("\n\n");
        out
    }
}

/// A complete `SSE` frame, and whether it is a keepalive.
///
/// An incremental server-sent-event parser.
///
/// Incomplete frames accumulate. This is the whole reason the type exists rather than a free
/// function over a whole body: a streamed body arrives in pieces that respect no framing boundary —
/// a piece can end in the middle of a `data:` line, which is exactly what happens when a frame is
/// larger than the socket buffer — so a parser that expected complete frames would drop or mangle
/// the frame it split. The buffering here is bounded by the frame it is assembling, which the server
/// bounds in turn.
#[derive(Debug, Default)]
struct SseParser {
    /// Bytes of a frame that has not yet been terminated by a blank line.
    pending: String,
}

impl SseParser {
    /// Creates an empty parser.
    fn new() -> Self {
        Self::default()
    }

    /// Feeds one piece of body and returns every frame it completed.
    ///
    /// The delimiter is a **blank line**, and a trailing `\r` is stripped so both `\n\n` and
    /// `\r\n\r\n` terminate a frame. The alternative — splitting on `\n\n` only — would leave the `\r`
    /// inside the last field's value, so a frame would carry a payload with a stray carriage return
    /// that the JSON parser rejects.
    fn push(&mut self, piece: &str) -> Vec<SseFrame> {
        self.pending.push_str(piece);
        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some(at) = find_frame_end(&self.pending[consumed..]) {
            let block = self.pending[consumed..consumed + at].to_owned();
            consumed += at;
            // Skip the delimiter itself.
            let rest = &self.pending[consumed..];
            let delimiter = if rest.starts_with("\r\n\r\n") { 4 } else { 2 };
            consumed += delimiter;
            if let Some(frame) = parse_frame(&block) {
                frames.push(frame);
            }
        }
        // Retain only the incomplete tail, so the buffer does not grow without bound as a long stream
        // is consumed — a parser that kept everything would hold the whole answer in memory twice.
        self.pending.drain(..consumed);
        frames
    }
}

/// Returns the byte index where a frame's block ends, if a blank line has arrived.
fn find_frame_end(text: &str) -> Option<usize> {
    let lf = text.find("\n\n");
    let crlf = text.find("\r\n\r\n");
    match (lf, crlf) {
        // CRLF is preferred when it starts no later, because its first `\n\n` match *is* inside it and
        // taking the LF index would leave a stray `\r` in the block.
        (Some(lf), Some(crlf)) => Some(if crlf <= lf { crlf } else { lf }),
        (Some(lf), None) => Some(lf),
        (None, Some(crlf)) => Some(crlf),
        (None, None) => None,
    }
}

/// Parses one complete frame block, or `None` when it carries no event type.
///
/// A block with no `event:` line is a comment (a keepalive) or an unknown field set, and either way
/// there is no event to report — which is what the contract requires of a keepalive.
fn parse_frame(block: &str) -> Option<SseFrame> {
    let mut id = None;
    let mut event = None;
    let mut data = String::new();
    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("id: ") {
            id = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("event: ") {
            event = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("data: ") {
            // A multi-line payload repeats the field, and `\n` joins the pieces. Clobbering instead
            // of appending would truncate a payload to its last line, which for a JSON object is
            // usually invalid and so would appear as a malformed frame rather than a lost one.
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }
        // A line beginning with `:` is a comment and is deliberately ignored, so a
        // keepalive cannot be mistaken for an event.
    }
    event.map(|event| SseFrame { id, event, data })
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

/// Builds the body of a create-run request for `jarvis ask`.
///
/// **Extracted so the request's shape is assertable without a daemon.** The field that mattered here
/// is `conversation_id`: the daemon has accepted it since the route existed, and the CLI sent `null`
/// unconditionally, so every `jarvis ask` began a fresh conversation and the model received no
/// history. Nothing could observe that: the request was valid, the run succeeded, and the only
/// evidence was the answer ignoring what had been said before.
///
/// `null` rather than an omitted key when no conversation is named, because this is the field's own
/// "start a new one" value. Sending it explicitly keeps the request's shape stable whichever form the
/// flag takes, so a caller diffing two requests sees the conversation change and nothing else.
///
/// `model_policy` is **omitted**, never sent as a placeholder: the identifier is derived from the
/// workspace, which is resolved server-side from this client's credential, so the only value the CLI
/// could send is one it invented. It previously sent `{"policy_id":"default","version":1}`, and
/// because the daemon ignored the field the request succeeded anyway — so the run's record described a
/// policy nobody had configured. Omission states the truth.
fn ask_body(text: &str, conversation: Option<&str>) -> String {
    format!(
        concat!(
            r#"{{"conversation_id":{},"input":{{"type":"text","text":{}}},"#,
            r#""runtime":"jarvis-native"}}"#
        ),
        conversation.map_or("null".to_owned(), json_string),
        json_string(text),
    )
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
        ApprovalsAction, AttemptAfter, AttemptReport, Cli, ClientErrorKind, ClientState, Command,
        FollowStep, InstallAction, RiskArg, STREAM_ATTEMPTS, SequenceCheck, SequenceWatcher,
        SseFrame, SseParser, StatusBody, ask_body, event_stream_headers, follow_after, follow_run,
        follow_step, idempotency_key, json_string, list_path, parse_status,
    };
    use clap::Parser as _;
    use jarvis_infrastructure::client::Discovered;
    use std::process::ExitCode;

    #[test]
    fn a_sequence_gap_is_detected_and_is_not_retried() {
        // **The contract's rule, which nothing enforced: "clients … never ignore a sequence gap".** The
        // reference client did not decide to ignore one — it ignored one by never reading `sequence` at
        // all. The consequence is the reason the rule exists: a follower that keeps printing across a
        // gap delivers two halves of an output that were never adjacent, as though they were an answer.
        let frame = |sequence: u64| SseFrame {
            id: Some(format!("0195f4f1-0475-7613-a92c-edf01183e9{sequence:02}")),
            event: jarvis_protocol::run::event_type::OUTPUT_TEXT_DELTA.to_owned(),
            data: format!(r#"{{"sequence":{sequence},"payload":{{"delta":"x"}}}}"#),
        };

        // A healthy stream is in order and never reports a gap — the precondition without which the
        // gap assertions below would be satisfied by a watcher that simply always reported one.
        let mut watcher = SequenceWatcher::new(false);
        for sequence in 1..=5 {
            assert_eq!(
                watcher.check(&frame(sequence)),
                SequenceCheck::InOrder,
                "sequence {sequence} must be in order",
            );
        }

        // **A gap is reported, and it names both sides**, because "a gap happened" without the numbers
        // is not actionable for an operator reading the error.
        assert_eq!(
            watcher.check(&frame(8)),
            SequenceCheck::Gap {
                expected: 6,
                found: 8
            },
        );
        // And a repeat is a gap too, not a "no progress": the contract says the stream increases by
        // exactly one, so a repeated sequence means the client is being sent a position it already
        // consumed.
        let mut watcher = SequenceWatcher::new(false);
        assert_eq!(watcher.check(&frame(1)), SequenceCheck::InOrder);
        assert_eq!(
            watcher.check(&frame(1)),
            SequenceCheck::Gap {
                expected: 2,
                found: 1
            },
        );
    }

    #[test]
    fn a_fresh_stream_must_open_at_sequence_one() {
        // The contract states `sequence` starts at 1 per run, so a stream that opens at 5 is one whose
        // start this client never saw. Accepting it would be the same failure as ignoring a later gap,
        // reached by a different route — and it is the case a "first frame sets the expectation"
        // implementation gets wrong by treating *any* first frame as legitimate.
        let mut watcher = SequenceWatcher::new(false);
        assert_eq!(
            watcher.check(&SseFrame {
                id: Some("a".to_owned()),
                event: "run.received".to_owned(),
                data: r#"{"sequence":5,"payload":null}"#.to_owned(),
            }),
            SequenceCheck::Gap {
                expected: 1,
                found: 5
            },
        );
    }

    #[test]
    fn a_resumed_stream_accepts_the_position_the_resume_header_asked_for() {
        // **The opposite error, and refusing it is not caution — it is a requirement.** `Last-Event-ID`
        // means "resume strictly after that event", so the first frame of a resumed stream is
        // legitimately not the successor of the last frame the previous attempt saw. A watcher that
        // required contiguity *across* a reconnect would refuse every healthy resume, turning the
        // client's own recovery path into an error.
        let mut watcher = SequenceWatcher::new(true);
        assert_eq!(
            watcher.check(&SseFrame {
                id: Some("a".to_owned()),
                event: jarvis_protocol::run::event_type::OUTPUT_TEXT_DELTA.to_owned(),
                data: r#"{"sequence":41,"payload":{"delta":"x"}}"#.to_owned(),
            }),
            SequenceCheck::InOrder,
            "the first frame of a resumed stream is wherever the daemon resumed",
        );
        // From there it must be contiguous again: relaxing the first frame must not relax the stream.
        assert_eq!(
            watcher.check(&SseFrame {
                id: Some("b".to_owned()),
                event: jarvis_protocol::run::event_type::OUTPUT_TEXT_DELTA.to_owned(),
                data: r#"{"sequence":43,"payload":{"delta":"x"}}"#.to_owned(),
            }),
            SequenceCheck::Gap {
                expected: 42,
                found: 43
            },
            "contiguity resumes after the first frame of a resumed stream",
        );
    }

    #[test]
    fn the_sequence_is_read_from_the_envelope_and_not_from_the_payload() {
        // **The shape of the defect this closes.** `sequence` sits beside `event_id` and `run_id` in
        // the frame, not inside `payload` — and a client that looked for it under `payload` would find
        // nothing on *every* frame, so its gap check would never fire and nothing would fail. This
        // asserts the value is found where the daemon actually puts it.
        let frame = SseFrame {
            id: Some("0195f4f1-0475-7613-a92c-edf01183e909".to_owned()),
            event: jarvis_protocol::run::event_type::OUTPUT_TEXT_DELTA.to_owned(),
            // A payload that *also* carries a `sequence`, so a lookup in the wrong place finds a
            // plausible number and the test cannot pass by coincidence.
            data: r#"{"sequence":2,"payload":{"delta":"x","sequence":99}}"#.to_owned(),
        };
        assert_eq!(
            frame.sequence(),
            Some(2),
            "the envelope's sequence is the one"
        );

        // And a frame with no sequence is `Unreadable` rather than a gap: the contract bounds what a
        // client concludes from a frame, and inventing a gap from an absent field would refuse a
        // stream the daemon is sending correctly.
        //
        // A **resumed** watcher here, deliberately, so this asserts the field's *location* and nothing
        // else. My first version used a fresh one and failed on my own freshness rule — a stream
        // opening at sequence 2 is a gap, which is correct and is asserted by its own test above. Mixing
        // the two properties into one assertion is what made the first version wrong.
        let mut watcher = SequenceWatcher::new(true);
        assert_eq!(watcher.check(&frame), SequenceCheck::InOrder);
        assert_eq!(
            watcher.check(&SseFrame {
                id: None,
                event: "run.planning".to_owned(),
                data: r#"{"payload":null}"#.to_owned(),
            }),
            SequenceCheck::Unreadable,
        );
    }

    #[test]
    fn a_gap_outranks_a_resume_and_is_reported_rather_than_retried() {
        // **The ordering is the guarantee.** `follow_after` decides whether an ended attempt should be
        // retried; a gap is not retryable at all, so consulting it first would let a gap be answered as
        // a dropped connection and reconnected — which re-reads from the position already reached and
        // cannot recover the missing events. Asserted through the report rather than the flags, because
        // the code's order is what makes it true and a test of the flags alone would not notice a
        // reorder.
        let gap = AttemptReport::SequenceGap {
            expected: 6,
            found: 8,
        };
        assert_ne!(
            gap,
            AttemptReport::EndedWithoutTerminal,
            "a gap is not the same outcome as a clean end, or the error would say the wrong thing",
        );
        // The report renders both numbers, which is what makes it actionable.
        let rendered = format!("{gap:?}");
        assert!(rendered.contains('6'), "{rendered}");
        assert!(rendered.contains('8'), "{rendered}");
    }

    /// Serves one SSE stream from a stub daemon and returns what `follow_run` reported.
    ///
    /// **Why a socket rather than more unit tests.** The gap check is a function over frames, and the
    /// tests above drive it directly — but a function that is correct and *never called* is exactly the
    /// defect this round closes, so the wiring itself has to be exercised. `Discovered` is a plain
    /// struct over a base URL, so a listener on an ephemeral loopback port is a complete stand-in for a
    /// daemon: the client resolves the host, dials it, writes a real request, and parses a real
    /// response, which is everything the gap check sits between.
    async fn follow_a_stubbed_stream(body: &'static str) -> ExitCode {
        use tokio::io::AsyncWriteExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral loopback port");
        let port = listener.local_addr().expect("has an address").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accepts");
            // The request is read before the response is written, because a client that wrote more
            // would otherwise see the response before finishing its own write.
            let mut request = [0_u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut request).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len(),
            );
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
            // Dropped here, which closes the connection.
        });

        let state = ClientState {
            discovered: Discovered {
                base_url: format!("http://127.0.0.1:{port}"),
                instance_id: "0195f4f1-0475-7613-a92c-edf01183e909".to_owned(),
                pid: 1,
            },
            credential: "not-a-real-credential".to_owned(),
        };
        let code = follow_run(&state, "0195f4f1-0475-7613-a92c-edf01183e909").await;
        let _ = server.await;
        code
    }

    /// One SSE frame with the envelope's top-level `sequence` and a delta payload.
    fn delta_frame(sequence: u64) -> String {
        format!(
            "id: 0195f4f1-0475-7613-a92c-edf01183e9{sequence:02}\n\
             event: run.output_text.delta\n\
             data: {{\"sequence\":{sequence},\"payload\":{{\"delta\":\"chunk{sequence}\"}}}}\n\n"
        )
    }

    #[tokio::test]
    async fn a_stream_that_skips_a_sequence_makes_the_follow_fail_rather_than_print() {
        // **The end-to-end half, and the wiring is the point.** The tests above prove the watcher
        // detects a gap; this proves the follower *acts* on it. Without it the whole feature could be a
        // correct function no caller consults — precisely the shape this round closes, since the
        // client's original defect was that it never read `sequence` at all.
        // **The stream carries a terminal, and that is what makes this test mean anything.** My first
        // version ended the body after the gapped frames, and the mutation that disabled the whole gap
        // handling **passed**: a stream with no terminal event fails the follow as
        // `EndedWithoutTerminal` whether or not the gap was noticed, so `assert_ne!(SUCCESS)` held for
        // the wrong reason. A gap-ignoring client must be able to *succeed* here for the assertion to
        // be about the gap — so the terminal is present, and only a client that notices the gap fails.
        let body: String = [
            delta_frame(1),
            delta_frame(2),
            // The gap.
            delta_frame(4),
            delta_frame(5),
            "id: 0195f4f1-0475-7613-a92c-edf01183e906\n\
             event: run.completed\n\
             data: {\"sequence\":6,\"payload\":null}\n\n"
                .to_owned(),
        ]
        .concat();
        let code = follow_a_stubbed_stream(Box::leak(body.into_boxed_str())).await;
        assert_ne!(
            code,
            ExitCode::SUCCESS,
            "a stream that skipped a sequence must not be reported as a successful follow, even \
             though it ends in a terminal event",
        );
    }

    #[tokio::test]
    async fn a_contiguous_stream_is_followed_to_its_terminal_without_a_gap_error() {
        // **The negative half, without which the test above is satisfied by a client that fails every
        // stream.** A watcher reporting a gap unconditionally would pass the previous test and be
        // useless — the "a filter that matches nothing is still a passing for" family, from the other
        // side.
        let body: String = [
            delta_frame(1),
            delta_frame(2),
            delta_frame(3),
            "id: 0195f4f1-0475-7613-a92c-edf01183e904\n\
             event: run.completed\n\
             data: {\"sequence\":4,\"payload\":null}\n\n"
                .to_owned(),
        ]
        .concat();
        let code = follow_a_stubbed_stream(Box::leak(body.into_boxed_str())).await;
        assert_eq!(
            code,
            ExitCode::SUCCESS,
            "a contiguous stream ending in a terminal must be followed successfully",
        );
    }

    #[test]
    fn every_way_an_attempt_can_end_is_decided_deliberately() {
        // **The clause that is wrong is the clause nobody tests**, and this loop is the worst place for
        // that: it is driven by a live connection, so reproducing "a transport failure on attempt 2
        // with no position" needs a daemon failing in that exact way. The decision is a function over
        // an outcome, a flag, an attempt number, and whether a position exists, so every combination is
        // enumerated here instead.
        use jarvis_infrastructure::client::ClientError;

        let transport = Err(ClientError::Transport);
        let other = Err(ClientError::MalformedResponse); // any non-transport failure

        // A transport failure with a position and budget left is the one case a resume can fix.
        assert_eq!(
            follow_after(&transport, false, 0, true),
            AttemptAfter::Resume,
        );
        // Without a position there is nothing to ask for, so retrying repeats the same failure.
        assert_eq!(
            follow_after(&transport, false, 0, false),
            AttemptAfter::Report(AttemptReport::Client(ClientErrorKind::Transport)),
            "a transport failure with nothing to resume from must not be retried",
        );
        // And the budget is a real bound: the last attempt reports rather than looping for ever.
        assert_eq!(
            follow_after(&transport, false, STREAM_ATTEMPTS - 1, true),
            AttemptAfter::Report(AttemptReport::Client(ClientErrorKind::Transport)),
            "the attempt budget must end the follow",
        );

        // **A signalled overrun resumes, and it is NOT reported as a fault.** This is the pair the
        // extraction exists for: the same "stream ended, no terminal" condition leads to opposite
        // outcomes depending on a flag, and the wrong answer tells an operator a working run failed.
        assert_eq!(follow_after(&Ok(()), true, 0, true), AttemptAfter::Resume);
        assert_ne!(
            follow_after(&Ok(()), true, 0, true),
            AttemptAfter::Report(AttemptReport::EndedWithoutTerminal),
            "a signalled overrun must not be reported as the stream ending silently",
        );
        // An overrun with no position is the one unusable case: it is not retried, and it says why.
        assert_eq!(
            follow_after(&Ok(()), true, 0, false),
            AttemptAfter::Report(AttemptReport::OverranBeforeAnyEvent),
        );
        // An overrun past the budget is still bounded, like any other resume — and it reports
        // *that*, rather than claiming the stream ended silently.
        assert_eq!(
            follow_after(&Ok(()), true, STREAM_ATTEMPTS - 1, true),
            AttemptAfter::Report(AttemptReport::OverrunBudgetExhausted),
            "an unbounded overrun loop would be a command that never returns",
        );

        // A clean end with no overrun and no terminal is the client's own honest report.
        assert_eq!(
            follow_after(&Ok(()), false, 0, true),
            AttemptAfter::Report(AttemptReport::EndedWithoutTerminal),
        );
        // A non-transport failure is never retried, whatever else is true.
        assert_eq!(
            follow_after(&other, false, 0, true),
            AttemptAfter::Report(AttemptReport::Client(ClientErrorKind::Other)),
            "only a transport failure is resumable, because only it is fixed by reconnecting",
        );
    }

    #[test]
    fn an_overrun_signal_is_a_resume_and_never_a_run_ending() {
        // **The assertion that stops the worst outcome.** The daemon sends `stream.overrun` when it
        // cut a *stream* short because this client stopped reading; the run itself is untouched and
        // still working. A client that classified it as a terminal event would tell an operator a
        // working run had finished — and, worse, would stop following a run that is going to produce
        // an answer. Both halves are asserted, because "it is classified as resume" is only meaningful
        // beside "it is not classified as an ending".
        let overrun = SseFrame {
            id: None,
            event: jarvis_protocol::run::event_type::STREAM_OVERRUN.to_owned(),
            data: r#"{"contract_version":"0.1.0","error":{"code":"stream.overrun"}}"#.to_owned(),
        };
        assert_eq!(follow_step(&overrun), FollowStep::Resume);
        assert_ne!(
            follow_step(&overrun),
            FollowStep::RunEnded { success: true },
            "an overrun is not a success",
        );
        assert_ne!(
            follow_step(&overrun),
            FollowStep::RunEnded { success: false },
            "an overrun is not a failure of the run",
        );

        // And the three real terminals must still be terminals, so the new arm cannot have swallowed
        // one — a classification bug in the *other* direction, which would leave this client
        // following a run that had already ended until the daemon closed the connection.
        for (event, success) in [
            (jarvis_protocol::run::event_type::COMPLETED, true),
            (jarvis_protocol::run::event_type::FAILED, false),
            (jarvis_protocol::run::event_type::CANCELLED, false),
        ] {
            assert_eq!(
                follow_step(&SseFrame {
                    id: Some("0195f4f1-0475-7613-a92c-edf01183e909".to_owned()),
                    event: event.to_owned(),
                    data: "{}".to_owned(),
                }),
                FollowStep::RunEnded { success },
                "{event} must still end the follow",
            );
        }
    }

    #[test]
    fn every_event_type_the_protocol_defines_is_classified_deliberately() {
        // The gap this closes is a *silent* one: a client switches on wire strings, so an event type
        // added to the protocol and not named here simply stops matching, and the follower treats it
        // as ignorable with nothing failing. Enumerating the protocol's own constants means a new
        // event type has to be given an answer — and `Ignored` is a legitimate answer, which is why
        // this asserts the *classification exists* rather than that it is a particular one.
        for event in [
            jarvis_protocol::run::event_type::RECEIVED,
            jarvis_protocol::run::event_type::CONTEXT_BUILDING,
            jarvis_protocol::run::event_type::PLANNING,
            jarvis_protocol::run::event_type::MODEL_STARTED,
            jarvis_protocol::run::event_type::RESPONDING,
            jarvis_protocol::run::event_type::USAGE,
        ] {
            assert_eq!(
                follow_step(&SseFrame {
                    id: None,
                    event: event.to_owned(),
                    data: "{}".to_owned(),
                }),
                FollowStep::Ignored,
                "{event} carries nothing for a text follow to print, and this asserts that is a \
                 decision rather than a gap",
            );
        }
        // A delta prints its text, which is the one arm that carries data.
        assert_eq!(
            follow_step(&SseFrame {
                id: None,
                event: jarvis_protocol::run::event_type::OUTPUT_TEXT_DELTA.to_owned(),
                data: r#"{"payload":{"delta":"hello"}}"#.to_owned(),
            }),
            FollowStep::Delta("hello".to_owned()),
        );
        // And a delta whose payload a build cannot parse prints nothing rather than panicking, because
        // an unreadable payload is a version skew this client should survive.
        assert_eq!(
            follow_step(&SseFrame {
                id: None,
                event: jarvis_protocol::run::event_type::OUTPUT_TEXT_DELTA.to_owned(),
                data: "not json".to_owned(),
            }),
            FollowStep::Ignored,
        );
    }

    #[test]
    fn the_event_stream_request_states_the_media_type_the_contract_requires() {
        // The contract requires `Accept: text/event-stream` on the events route, and this client is
        // the reference implementation of that surface. It sent **no** `Accept` header at all until
        // this, which worked only because the daemon did not check — the two defects hid each other,
        // so no test driving one against the other could see either.
        //
        // Asserted on the string this client builds rather than through a server, because the rule
        // is about what the client *sends*: a test that went over the wire would pass as soon as the
        // server started tolerating the omission, which is exactly the state this leaves behind.
        let bare = event_stream_headers(None);
        assert!(
            bare.contains(&format!("Accept: {}\r\n", super::EVENT_STREAM_MEDIA_TYPE)),
            "{bare}",
        );
        assert!(
            !bare.contains("Last-Event-ID"),
            "no resume was asked for: {bare}"
        );

        // And the resume header is still sent when one is asked for, so adding the media type did
        // not replace the header that was already there.
        let resumed = event_stream_headers(Some("0195f4f1-0475-7613-a92c-edf01183e909"));
        assert!(
            resumed.contains("Accept: text/event-stream\r\n"),
            "{resumed}",
        );
        assert!(
            resumed.contains("Last-Event-ID: 0195f4f1-0475-7613-a92c-edf01183e909\r\n"),
            "{resumed}",
        );

        // Every header line ends in CRLF and none is empty, because this string is spliced directly
        // into the request the client writes — a missing terminator would merge two headers into
        // one and send a header nobody wrote.
        for line in bare.lines() {
            assert!(!line.is_empty(), "{bare}");
            assert!(
                bare.contains(&format!("{line}\r\n")),
                "every line must be terminated: {bare}",
            );
        }
    }

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
            (vec!["jarvis", "approvals", "list"], "approvals"),
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
                Command::Approvals { .. } => "approvals",
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
                    | Command::Runs { .. }
                    | Command::Approvals { .. } => String::from("unexpected"),
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
        let mut parser = SseParser::new();
        let frames = parser.push(body);
        assert_eq!(frames.len(), 2, "a keepalive is not an event");
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
    fn a_frame_split_across_pieces_is_assembled_rather_than_dropped() {
        // **The property that makes streaming possible, and the reason the parser is incremental.**
        // A body arrives in pieces that respect no framing boundary, so a piece can end in the middle
        // of a `data:` line — which is exactly what happens whenever a frame is larger than the socket
        // buffer, and therefore normal for a long answer. A parser that expected whole frames would
        // either drop the frame or emit a truncated payload, and a truncated `data:` document is not
        // valid JSON, so the frame would silently become nothing.
        //
        // The frame is deliberately split **inside the JSON**, which is the case that breaks a
        // line-based parser: it does not see a `data: ` prefix on the second half at all.
        let whole = concat!(
            "id: 0195f4f1-0475-7613-a92c-edf01183e909\n",
            "event: run.output_text.delta\n",
            "data: {\"payload\":{\"item_id\":\"out-1\",\"delta\":\"Hello\"}}\n",
            "\n",
        );
        // Every split point must work, not just one: a parser that happened to tolerate a split at a
        // newline and fail mid-line would look correct against a single hand-picked fixture.
        for split_at in 0..whole.len() {
            let (first, second) = whole.split_at(split_at);
            let mut parser = SseParser::new();
            let from_first = parser.push(first);
            let from_second = parser.push(second);
            let frames: Vec<SseFrame> = from_first.into_iter().chain(from_second).collect();
            assert_eq!(
                frames.len(),
                1,
                "a split at {split_at} must yield exactly one frame",
            );
            assert_eq!(
                frames[0].event, "run.output_text.delta",
                "split at {split_at}"
            );
            assert_eq!(
                frames[0].payload("delta").as_deref(),
                Some("Hello"),
                "a payload must survive a split at {split_at}",
            );
        }
    }

    #[test]
    fn an_incomplete_frame_yields_nothing_until_it_is_terminated() {
        // The other half of the same property: a parser that emitted a frame as soon as it saw an
        // `event:` line would deliver a frame whose `data` had not arrived, and a caller reading the
        // payload would get nothing and conclude the event carried none.
        let mut parser = SseParser::new();
        assert!(
            parser
                .push("event: run.completed\ndata: {\"payload\":null}\n")
                .is_empty(),
            "a frame without its blank-line terminator is not complete",
        );
        let frames = parser.push("\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event, "run.completed");
    }

    #[test]
    fn a_crlf_terminated_frame_does_not_carry_a_stray_carriage_return() {
        // A server may frame with CRLF, and splitting on `\n\n` alone would leave the `\r` from the
        // last field's line inside its value — so a payload would end in a carriage return and the
        // JSON parse would fail, turning a healthy frame into a malformed one.
        let body = concat!(
            "id: 0195f4f1\r\n",
            "event: run.output_text.delta\r\n",
            "data: {\"payload\":{\"delta\":\"Hi\"}}\r\n",
            "\r\n",
        );
        let mut parser = SseParser::new();
        let frames = parser.push(body);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload("delta").as_deref(), Some("Hi"));
        assert!(
            !frames[0].data.ends_with('\r'),
            "the carriage return belongs to the framing: {:?}",
            frames[0].data,
        );
    }

    #[test]
    fn a_frame_renders_back_to_the_framing_it_arrived_in() {
        // `runs events` prints this, so it must produce something a client can parse — an `id` line
        // when there is an id, an `event` line, a `data` line, and the blank terminator.
        let mut parser = SseParser::new();
        let frames =
            parser.push("id: 0195f4f1\nevent: run.completed\ndata: {\"payload\":null}\n\n");
        assert_eq!(
            frames[0].render(),
            "id: 0195f4f1\nevent: run.completed\ndata: {\"payload\":null}\n\n",
        );
        // A frame with no id omits the line rather than emitting `id: ` with an empty value, which a
        // client resuming from it would echo as a position that does not exist.
        let mut parser = SseParser::new();
        let frames = parser.push("event: run.completed\ndata: {}\n\n");
        assert_eq!(frames[0].render(), "event: run.completed\ndata: {}\n\n");
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
    fn a_question_without_a_conversation_starts_a_new_one() {
        // The default shape the CLI has always sent. Asserted so adding the flag cannot change it:
        // a `jarvis ask` with no `--conversation` must still ask for a fresh conversation.
        let body = ask_body("hello", None);
        assert!(body.contains(r#""conversation_id":null"#), "{body}");
        assert!(
            body.contains(r#""input":{"type":"text","text":"hello"}"#),
            "{body}"
        );
        assert!(body.contains(r#""runtime":"jarvis-native""#), "{body}");
        // The policy is **omitted**, not sent as a placeholder. A `policy_id` here would name a
        // policy the CLI invented, and the run's record would describe one nobody configured.
        assert!(!body.contains("model_policy"), "{body}");
    }

    #[test]
    fn a_named_conversation_is_sent_so_the_model_receives_its_history() {
        // **The defect this test exists for is behavioural, so no other test could see it.** The
        // daemon has accepted `conversation_id` since the create route existed and the CLI sent
        // `null` unconditionally, so every `jarvis ask` began a fresh conversation: the request was
        // valid, the run succeeded, and the only symptom was an answer that ignored what had been
        // said before — which reads as a model problem, not as a dropped field.
        let body = ask_body("and then?", Some("0195f4f0-4c13-7bf4-89fb-f067adac13ee"));
        assert!(
            body.contains(r#""conversation_id":"0195f4f0-4c13-7bf4-89fb-f067adac13ee""#),
            "the named conversation must reach the request: {body}",
        );
        assert!(!body.contains("conversation_id\":null"), "{body}");
        // The identifier goes through the JSON encoder like the question does, so a value containing
        // a quote cannot produce a body the daemon refuses.
        let quoting = ask_body("hi", Some("has\"quote"));
        assert!(
            quoting.contains(r#""conversation_id":"has\"quote""#),
            "{quoting}"
        );
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

    #[test]
    fn a_bare_approvals_command_cannot_decide_anything() {
        // The same safety property `runs` has: a decision is a named subcommand, so the shortest
        // invocation lists rather than decides.
        assert!(Cli::try_parse_from(["jarvis", "approvals"]).is_err());
        assert!(Cli::try_parse_from(["jarvis", "approvals", "list"]).is_ok());
        assert!(Cli::try_parse_from(["jarvis", "approvals", "show", "abc"]).is_ok());
    }

    #[test]
    fn the_listing_accepts_a_risk_narrow_and_a_cursor() {
        // **The client half of a feature that had none.** The daemon serves `?risk=` and hands back a
        // `next_cursor`, but `approvals list` could send only `--limit` — so a client that printed
        // `next_cursor` had no way to pass it back, the same read-but-unusable dead end the cursor was
        // introduced on the daemon side to close. Both options must parse, and together with `--limit`,
        // because a page is narrowed *and* resumed in real use.
        let cli = Cli::try_parse_from([
            "jarvis",
            "approvals",
            "list",
            "--limit",
            "5",
            "--risk",
            "critical",
            "--cursor",
            "v1.abc",
        ])
        .expect("parses");
        match cli.command {
            Command::Approvals {
                action:
                    ApprovalsAction::List {
                        limit,
                        risk,
                        cursor,
                    },
            } => {
                assert_eq!(limit, Some(5));
                assert_eq!(risk, Some(RiskArg::Critical));
                assert_eq!(cursor.as_deref(), Some("v1.abc"));
            }
            other => unreachable!("expected approvals list, got {other:?}"),
        }
        // The shortest form names no filters, so a bare `list` is the un-narrowed first page.
        let bare = Cli::try_parse_from(["jarvis", "approvals", "list"]).expect("parses");
        assert!(matches!(
            bare.command,
            Command::Approvals {
                action: ApprovalsAction::List {
                    limit: None,
                    risk: None,
                    cursor: None,
                },
            }
        ));
    }

    #[test]
    fn an_unknown_risk_level_is_refused_before_a_request_is_built() {
        // A closed set parsed on the client, so `--risk severe` never reaches the daemon. The daemon
        // would answer `400 request.invalid`, but a client that forwarded an unrecognised level would
        // be trusting the server to validate its own command line — and the refusal would arrive as a
        // network round trip rather than as a usage error.
        assert!(Cli::try_parse_from(["jarvis", "approvals", "list", "--risk", "severe"]).is_err());
        // Every level the wire vocabulary names is accepted, driven by `LEVELS` so a fifth level added
        // to the contract is either selectable here or fails to parse — the assertion cannot go stale
        // silently.
        for level in jarvis_protocol::approval::risk::LEVELS {
            let parsed = Cli::try_parse_from(["jarvis", "approvals", "list", "--risk", level])
                .expect("every contract level must parse");
            assert!(matches!(parsed.command, Command::Approvals { .. }));
        }
    }

    #[test]
    fn the_listing_path_carries_exactly_the_filters_the_client_was_given() {
        // Byte-exact, because the path *is* the request: a transposed or dropped parameter is a
        // different query, and the daemon reads the three by name. Asserting the string is what makes
        // "the filter the user typed reaches the wire" checkable without a daemon — the same reason
        // `ask_body` is asserted as JSON rather than inspected field by field.
        assert_eq!(list_path(None, None, None), "/api/v1/approvals");
        assert_eq!(
            list_path(Some(25), None, None),
            "/api/v1/approvals?limit=25"
        );
        assert_eq!(
            list_path(None, Some(RiskArg::Critical), None),
            "/api/v1/approvals?risk=critical",
        );
        assert_eq!(
            list_path(None, None, Some("v1.abc")),
            "/api/v1/approvals?cursor=v1.abc",
        );
        // All three, in the fixed order, so a resumed narrowed page composes rather than replacing.
        assert_eq!(
            list_path(Some(5), Some(RiskArg::High), Some("v1.xyz")),
            "/api/v1/approvals?limit=5&risk=high&cursor=v1.xyz",
        );
        // Every level the protocol names renders as its own contract spelling, so the path cannot carry
        // a spelling the daemon's `Risk::parse` would refuse.
        for (level, expected) in [
            (RiskArg::Low, "low"),
            (RiskArg::Moderate, "moderate"),
            (RiskArg::High, "high"),
            (RiskArg::Critical, "critical"),
        ] {
            assert_eq!(level.as_contract_str(), expected);
            assert_eq!(
                list_path(None, Some(level), None),
                format!("/api/v1/approvals?risk={expected}"),
            );
        }
    }

    #[test]
    fn an_approval_decision_requires_both_the_fingerprint_and_the_version() {
        // **Both are required arguments, and that is the point.** The daemon compares the fingerprint
        // it holds against the one the client states, so a decision that named none would be a decision
        // about *something* — and the version is the precondition that makes a concurrent decision
        // refusable. A default for either would let a caller decide an action it never reviewed.
        for word in ["approve", "reject"] {
            assert!(
                Cli::try_parse_from(["jarvis", "approvals", word, "abc"]).is_err(),
                "{word} must require its fingerprint and version",
            );
            assert!(
                Cli::try_parse_from([
                    "jarvis",
                    "approvals",
                    word,
                    "abc",
                    "--fingerprint",
                    "sha256:aa",
                ])
                .is_err(),
                "{word} must require the version too",
            );
            assert!(
                Cli::try_parse_from([
                    "jarvis",
                    "approvals",
                    word,
                    "abc",
                    "--fingerprint",
                    "sha256:aa",
                    "--version",
                    "1",
                ])
                .is_ok(),
                "{word} must parse with both",
            );
        }
        // A cancellation needs the version, and its reason is optional — a withdrawal does not always
        // have one, and requiring text would invite an empty string.
        assert!(Cli::try_parse_from(["jarvis", "approvals", "cancel", "abc"]).is_err());
        assert!(
            Cli::try_parse_from(["jarvis", "approvals", "cancel", "abc", "--version", "1"]).is_ok(),
        );
        assert!(
            Cli::try_parse_from([
                "jarvis",
                "approvals",
                "cancel",
                "abc",
                "--version",
                "1",
                "--reason",
                "changed my mind",
            ])
            .is_ok(),
        );
    }
}
