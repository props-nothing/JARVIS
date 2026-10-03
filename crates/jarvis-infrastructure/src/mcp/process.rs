//! Launching a local MCP server as an isolated child process.
//!
//! The tool fabric requires that an untrusted server is launched with "isolated environment variables
//! and working directories", bounded "stdout/stderr capture and message sizes", and enforced
//! "startup, heartbeat, turn, idle, and shutdown timeouts". This module is the launch half of that:
//! it builds the command, bounds every string that reaches it, and captures the child's diagnostics
//! without letting them deadlock or grow.
//!
//! # What is deliberately *not* here
//!
//! There is no sandbox. `AGENTS.md` says "dangerous code and shell execution never runs unsandboxed
//! merely because a model requested it", and a process launched here is **not** sandboxed either — it
//! inherits the daemon's user, its filesystem view, and its network. What this module does is remove
//! the *ambient* surface it can remove (the environment) and bound what the child can do to the
//! daemon (its output, its lifetime, its startup time). A real platform sandbox is a separate,
//! reviewed capability, and pretending otherwise here would be the kind of claim that makes a real
//! gap invisible.
//!
//! # Three structural decisions, each matching an existing convention
//!
//! - **Argv only, never a composed command string.** `service::exec` records the reason: "spawned
//!   directly, never through a shell, so no argument can be reinterpreted as a shell construct". MCP
//!   server arguments routinely contain spaces and punctuation, and the argv form is exactly what
//!   makes them inert rather than injectable.
//! - **`env_clear()` then explicit pairs.** The default for a child is to inherit everything, which
//!   would hand an untrusted server the daemon's `HOME`, cloud credentials, tokens, and any
//!   `*_PASSWORD` an operator happens to have exported. Clearing first makes inheritance impossible
//!   rather than unlikely: there is no path that passes an unlisted variable.
//! - **The program path is resolved by the caller, not searched by this module.** Since `env_clear`
//!   removes `PATH`, a bare program name cannot be resolved by the child's own environment — so the
//!   caller resolves it against the daemon's environment *once*, at configuration time, and the
//!   resolved path is what is validated and recorded. That keeps resolution next to authorization
//!   rather than inside the spawn.
//!
//! # Bounded diagnostics without deadlock
//!
//! A piped stderr that nobody reads is a **hang**: the child blocks once the OS pipe buffer fills and
//! the MCP session stops making progress. `rmcp`'s own default is worse for our purposes — stderr is
//! inherited, which sprays an untrusted server's output across the daemon's own stderr with no bound
//! and no redaction. So stderr is piped, drained by a task this module spawns, and kept as a
//! **bounded tail**: when the child dies, the last bytes before it died are what explain why, and the
//! first bytes of a chatty server's startup banner explain nothing.
//!
//! # A launched server must be **dropped inside a Tokio runtime**
//!
//! This is a constraint `rmcp` imposes and it is easy to violate by accident. Its child-process
//! transport kills the child from its `Drop`, and `kill()` awaits a reap — so dropping a transport
//! outside a runtime **panics inside the SDK** with "there is no reactor running", at
//! `transport/child_process.rs:50`. A daemon is async and so drops servers in-runtime as a matter of
//! course; the hazard is a *synchronous* shutdown path, or a test that builds a server inside a
//! runtime and drops it after `Runtime::block_on` has returned. [`McpServerProcess::shutdown`] exists
//! so there is a deliberate in-runtime path rather than only an implicit one.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::process::{ChildStderr, Command as TokioCommand};
use tokio::task::JoinHandle;

use rmcp::transport::TokioChildProcess;

use crate::plugin::contains_control;

/// The shortest accepted startup timeout, in milliseconds.
///
/// Mirrors `plugin::MIN_PLUGIN_STARTUP_TIMEOUT_MS`: zero would mean "never started", which is not a
/// limit but a refusal with a delay.
pub const MIN_MCP_STARTUP_TIMEOUT_MS: u64 = 1;

/// The longest accepted startup timeout, in milliseconds (one minute).
///
/// A bound on a *child process*, so it is bounded here rather than trusted: "an unbounded startup
/// timeout is a hang with no clock", the same sentence `plugin` records for the same reason.
pub const MAX_MCP_STARTUP_TIMEOUT_MS: u64 = 60_000;

/// The longest accepted program, argument, environment key, or path string, in bytes.
///
/// One bound for every string that reaches the process table, because they all reach persisted
/// configuration, logs, and operator output. `plugin` uses the same technique for the same reason.
pub const MAX_MCP_TOKEN_BYTES: usize = 512;

/// The greatest accepted number of arguments.
pub const MAX_MCP_ARGUMENTS: usize = 64;

/// The greatest accepted number of environment variables.
///
/// A small bound on purpose: the point of an explicit list is that an operator reviewed it, and a
/// list of a hundred entries is one nobody read. It also bounds what a hostile *document* can make
/// the daemon allocate before validation refuses it.
pub const MAX_MCP_ENV_VARS: usize = 32;

/// The greatest accepted environment value, in bytes.
///
/// Larger than a token because a legitimate value can be a path list or a certificate, and still
/// bounded because it is attacker-influenced text that reaches a child process.
pub const MAX_MCP_ENV_VALUE_BYTES: usize = 4 * 1024;

/// The greatest number of child stderr bytes retained, in bytes.
pub const MAX_MCP_STDERR_BYTES: usize = 16 * 1024;

/// The startup timeout applied when configuration does not name one.
///
/// Ten seconds because it matches the SDK's own auto-discovery probe timeout, so a server that cannot
/// complete `server/discover` inside this window is a server that would have failed the SDK's legacy
/// fallback anyway — the two clocks agreeing avoids a class of "it fell back but JARVIS said it timed
/// out" confusion.
pub const DEFAULT_MCP_STARTUP_TIMEOUT_MS: u64 = 10_000;

/// The size of one read from a child's stderr.
const STDERR_CHUNK_BYTES: usize = 4 * 1024;

/// Why a launch specification was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpProcessError {
    /// The program path is empty, too long, or contains a control character.
    #[error("mcp server program is not usable")]
    ProgramInvalid,
    /// Too many arguments, or one of them is unusable.
    #[error("mcp server argument is not usable")]
    ArgumentInvalid,
    /// The working directory is unusable.
    #[error("mcp server working directory is not usable")]
    WorkingDirectoryInvalid,
    /// The environment list has too many entries, or one key or value is unusable.
    #[error("mcp server environment entry is not usable")]
    EnvironmentInvalid,
    /// The startup timeout is outside its bounds.
    #[error("mcp server startup timeout {timeout_ms} is outside {min}..={max}")]
    StartupTimeoutOutOfRange {
        /// The value that was supplied.
        timeout_ms: u64,
        /// The shortest accepted value.
        min: u64,
        /// The longest accepted value.
        max: u64,
    },
    /// The child could not be spawned.
    #[error("mcp server process could not be spawned")]
    SpawnFailed,
}

impl McpProcessError {
    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ProgramInvalid => "mcp.program_invalid",
            Self::ArgumentInvalid => "mcp.argument_invalid",
            Self::WorkingDirectoryInvalid => "mcp.working_directory_invalid",
            Self::EnvironmentInvalid => "mcp.environment_invalid",
            Self::StartupTimeoutOutOfRange { .. } => "mcp.startup_timeout_out_of_range",
            Self::SpawnFailed => "mcp.spawn_failed",
        }
    }
}

/// Everything the daemon needs to launch one configured MCP server, and nothing ambient.
///
/// Every field is an explicit decision: the program was resolved by the caller, the arguments were
/// reviewed, the environment is the complete list the child may see, and the working directory does
/// not default to the daemon's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpLaunchSpec {
    /// The **resolved** program path. `env_clear` means a bare name cannot be resolved by the child,
    /// so the caller resolves it against the daemon's environment at configuration time.
    pub program: PathBuf,
    /// The argument vector. Passed to the child verbatim, never through a shell.
    pub args: Vec<String>,
    /// The complete environment the child may see.
    pub env: Vec<(String, String)>,
    /// The child's working directory. Absent means "the daemon's", which is why it is worth setting.
    pub working_dir: Option<PathBuf>,
    /// How long the child may take to become usable before it is a failure.
    pub startup_timeout_ms: u64,
}

impl McpLaunchSpec {
    /// Validates every string that reaches the process table.
    ///
    /// # Errors
    ///
    /// Returns the [`McpProcessError`] naming the field at fault. Each refusal names a field because a
    /// specification is assembled from configuration an operator wrote, and "the specification is
    /// invalid" alone does not say which line to fix.
    pub fn validate(&self) -> Result<(), McpProcessError> {
        if self.program.as_os_str().is_empty() {
            return Err(McpProcessError::ProgramInvalid);
        }
        // `to_str` rather than `to_string_lossy`: a **non-UTF-8** path is refused rather than replaced,
        // because lossy conversion would silently substitute U+FFFD and hand a *different* path to the
        // process table than the one that was reviewed.
        let Some(program) = self.program.to_str() else {
            return Err(McpProcessError::ProgramInvalid);
        };
        if !usable_token(program, MAX_MCP_TOKEN_BYTES) {
            return Err(McpProcessError::ProgramInvalid);
        }
        if self.args.len() > MAX_MCP_ARGUMENTS {
            return Err(McpProcessError::ArgumentInvalid);
        }
        for argument in &self.args {
            if !usable_token(argument, MAX_MCP_TOKEN_BYTES) {
                return Err(McpProcessError::ArgumentInvalid);
            }
        }
        if let Some(directory) = &self.working_dir {
            let Some(directory) = directory.to_str() else {
                return Err(McpProcessError::WorkingDirectoryInvalid);
            };
            if !usable_token(directory, MAX_MCP_TOKEN_BYTES) {
                return Err(McpProcessError::WorkingDirectoryInvalid);
            }
        }
        if self.env.len() > MAX_MCP_ENV_VARS {
            return Err(McpProcessError::EnvironmentInvalid);
        }
        for (key, value) in &self.env {
            if !usable_env_key(key) || !usable_env_value(value) {
                return Err(McpProcessError::EnvironmentInvalid);
            }
        }
        if !(MIN_MCP_STARTUP_TIMEOUT_MS..=MAX_MCP_STARTUP_TIMEOUT_MS)
            .contains(&self.startup_timeout_ms)
        {
            return Err(McpProcessError::StartupTimeoutOutOfRange {
                timeout_ms: self.startup_timeout_ms,
                min: MIN_MCP_STARTUP_TIMEOUT_MS,
                max: MAX_MCP_STARTUP_TIMEOUT_MS,
            });
        }
        Ok(())
    }
}

/// Returns whether `value` is a usable token for a program, argument, or path.
///
/// Control characters are refused through [`contains_control`] rather than by a second rule, so the
/// workspace has **one** definition of what corrupts a log line or a terminal. A NUL byte is in that
/// range, which matters here beyond cosmetics: it cannot be passed to an exec call at all.
fn usable_token(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !contains_control(value)
}

/// Returns whether `key` is a usable environment variable name.
///
/// `=` is refused because it is the separator the platform uses, so a key containing one is not a
/// name but two assignments, and an empty key is refused because it is what an unset field produces.
fn usable_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_MCP_TOKEN_BYTES
        && !key.contains('=')
        && !contains_control(key)
}

/// Returns whether `value` is a usable environment value.
///
/// **Empty is allowed and control characters are not.** An empty value is a real configuration — a
/// variable deliberately set to nothing, which is distinguishable from an absent one — while a
/// control character is the thing that corrupts a log line.
fn usable_env_value(value: &str) -> bool {
    value.len() <= MAX_MCP_ENV_VALUE_BYTES && !contains_control(value)
}

/// Builds the command for a launch, with the ambient environment removed.
///
/// **This is the function the isolation property lives in, and it is deliberately pure** so that it
/// can be asserted on directly. A test that spawned a child to check isolation would be slower, flakier,
/// and would prove less: it could only observe the environment a child *happened* to report.
///
/// # Errors
///
/// Returns whatever [`McpLaunchSpec::validate`] returns, so no unvalidated string reaches the process
/// table.
pub fn build_command(spec: &McpLaunchSpec) -> Result<TokioCommand, McpProcessError> {
    spec.validate()?;
    let mut command = TokioCommand::new(&spec.program);
    command.args(&spec.args);
    // The load-bearing line. `env_clear` first and *then* the explicit pairs means there is no path
    // that passes an unlisted variable, rather than a path that happens not to.
    command.env_clear();
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    if let Some(directory) = &spec.working_dir {
        command.current_dir(directory);
    }
    // Belt and braces: `rmcp`'s `TokioChildProcess` already kills its child on drop, and this makes
    // the same guarantee hold for any other use of a command built here.
    command.kill_on_drop(true);
    Ok(command)
}

/// A launched MCP server: its transport, and the bounded tail of what it wrote to stderr.
pub struct McpServerProcess {
    transport: TokioChildProcess,
    diagnostics: Arc<Mutex<StderrTail>>,
    /// The draining task, held so the handle's lifetime is tied to this struct and the task is
    /// abortable rather than leaked.
    drain: JoinHandle<()>,
}

impl std::fmt::Debug for McpServerProcess {
    /// Reports the retained diagnostic **size**, never its content.
    ///
    /// Hand-written rather than derived for two reasons: the transport is not `Debug`, and a derived
    /// `Debug` would put an untrusted server's stderr into any `{:?}` of this value — which is how
    /// untrusted text reaches a log line nobody intended it to reach.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let retained = self.diagnostics.lock().map_or(0, |tail| tail.bytes.len());
        formatter
            .debug_struct("McpServerProcess")
            .field("diagnostics_bytes", &retained)
            .finish_non_exhaustive()
    }
}

impl McpServerProcess {
    /// Returns the transport to serve a client over.
    #[must_use]
    pub fn transport(&self) -> &TokioChildProcess {
        &self.transport
    }

    /// Takes the transport, for handing it to the SDK's client entry point.
    #[must_use]
    pub fn into_transport(self) -> TokioChildProcess {
        // The drain task is **not** aborted here: the transport that remains still owns the child, so
        // the child's stderr is still open and the task is still the thing keeping it drained. Dropping
        // the handle detaches the task rather than cancelling it, which is the behavior wanted — it
        // ends by itself when the child exits and closes the pipe.
        let Self {
            transport,
            diagnostics: _,
            drain,
        } = self;
        drop(drain);
        transport
    }

    /// Returns the bounded tail of the child's stderr, for operator diagnostics.
    ///
    /// A poisoned lock returns an empty string rather than propagating a panic: diagnostics are not
    /// worth failing a launch over, and a panic in the draining task means the child is already gone.
    #[must_use]
    pub fn diagnostics(&self) -> String {
        self.diagnostics
            .lock()
            .map_or_else(|_| String::new(), |tail| tail.text())
    }

    /// Shuts the child down, awaiting its exit.
    ///
    /// The deliberate in-runtime counterpart to the drop-time kill: see the module doc's note on why a
    /// transport must not be dropped outside a runtime. Calling this makes the shutdown an explicit
    /// step with a bounded wait rather than an implicit one inside a `Drop` that cannot report failure.
    ///
    /// The drain task is detached first so it keeps reading while the child exits — aborting it here
    /// could discard the very bytes that explain a failed shutdown.
    ///
    /// # Errors
    ///
    /// Returns the OS error when the child could not be reaped within the SDK's grace period.
    pub async fn shutdown(mut self) -> std::io::Result<()> {
        let shutdown = self.transport.graceful_shutdown().await;
        // The drain task ends by itself once the child's stderr closes; dropping the handle detaches it.
        let Self {
            drain,
            diagnostics: _,
            transport: _,
        } = self;
        drop(drain);
        shutdown
    }
}

/// Spawns a configured MCP server over stdio.
///
/// Must be called from within a Tokio runtime: `rmcp`'s transports cannot be constructed outside one
/// (calling them outside panics inside the SDK with "there is no reactor running"), and this spawns a
/// draining task.
///
/// # Errors
///
/// Returns [`McpProcessError::SpawnFailed`] when the child cannot be started — most often because the
/// resolved program does not exist on this machine — and otherwise whatever
/// [`build_command`] returns.
pub fn spawn_stdio_server(spec: &McpLaunchSpec) -> Result<McpServerProcess, McpProcessError> {
    let command = build_command(spec)?;
    // `stderr(Stdio::piped())` overrides the SDK's default of **inheriting** stderr, which would
    // spray an untrusted server's output across the daemon's own, unbounded and unredacted.
    let (transport, stderr) = TokioChildProcess::builder(command)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| McpProcessError::SpawnFailed)?;
    let diagnostics = Arc::new(Mutex::new(StderrTail::default()));
    let drain = match stderr {
        Some(stderr) => tokio::spawn(drain_stderr(stderr, Arc::clone(&diagnostics))),
        // The SDK returns `None` when the child's stderr was already taken, which cannot happen for a
        // command built here — but a spawn that hands back no pipe must not become an unwrap.
        None => tokio::spawn(async {}),
    };
    Ok(McpServerProcess {
        transport,
        diagnostics,
        drain,
    })
}

/// Reads a child's stderr to EOF, keeping only the last [`MAX_MCP_STDERR_BYTES`].
async fn drain_stderr(mut stderr: ChildStderr, tail: Arc<Mutex<StderrTail>>) {
    use tokio::io::AsyncReadExt as _;

    let mut chunk = [0_u8; STDERR_CHUNK_BYTES];
    loop {
        match stderr.read(&mut chunk).await {
            // EOF, or a read error such as the child having been killed: either way there is nothing
            // more to read, and a failed read is not a reason to panic a background task.
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if let Some(bytes) = chunk.get(..read)
                    && let Ok(mut tail) = tail.lock()
                {
                    tail.push(bytes);
                }
            }
        }
    }
}

/// The bounded tail of a child's stderr.
///
/// Keeps the **last** bytes rather than the first. A server that dies mid-session wrote the reason
/// immediately before it died, and its startup banner — the part a first-bytes buffer would have
/// kept — explains nothing about the failure.
#[derive(Debug, Default)]
struct StderrTail {
    bytes: Vec<u8>,
}
impl StderrTail {
    /// Appends bytes, discarding the oldest so the retained length stays bounded.
    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() > MAX_MCP_STDERR_BYTES {
            // `drain` rather than a slice, so the allocation is reused instead of copying the tail
            // into a new buffer on every chunk once the bound is reached.
            let excess = self.bytes.len() - MAX_MCP_STDERR_BYTES;
            self.bytes.drain(..excess);
        }
    }

    /// Returns the retained bytes as text, with invalid sequences replaced.
    ///
    /// Lossy **here** rather than at the boundary is the opposite of the rule the program path
    /// follows, and for the opposite reason: a path is passed back to the operating system, where a
    /// substituted character means a different file, while diagnostics are only ever displayed, where
    /// a substituted character is strictly better than dropping the message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
