//! Running an external service-manager program with a bounded result.
//!
//! Every platform controller shells out. This module keeps that in one place so
//! the bounds and the non-interactive guarantee are stated once: a fixed
//! argument vector (never a shell), a bounded wait, and bounded captured output.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::ServiceError;

/// The bounded time a service-manager command may take.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The bounded number of captured output bytes.
pub const MAX_CAPTURED_BYTES: usize = 64 * 1024;

/// The result of a service-manager command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    /// The process exit code, when the process exited normally.
    pub status: Option<i32>,
    /// Whether the process reported success.
    pub success: bool,
    /// Bounded combined output. Never logged wholesale, and never a secret
    /// source; it exists for operator diagnostics only.
    pub output: String,
}

impl CommandOutcome {
    /// Returns whether the command reported success.
    #[must_use]
    pub const fn succeeded(&self) -> bool {
        self.success
    }
}

/// Runs `program` with `args` and a bounded wait.
///
/// The command is spawned directly, never through a shell, so no argument can be
/// reinterpreted as a shell construct.
///
/// # Errors
///
/// Returns [`ServiceError::ProgramUnavailable`] when the program cannot be
/// spawned (typically absent on this system) and
/// [`ServiceError::CommandFailed`] when it does not finish inside the bound.
pub async fn run_program(program: &str, args: &[String]) -> Result<CommandOutcome, ServiceError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The service-manager tool must never prompt: a prompt would hang a
        // non-interactive install.
        .kill_on_drop(true);

    let child = command
        .spawn()
        .map_err(|_| ServiceError::ProgramUnavailable)?;

    let output = tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ServiceError::CommandFailed {
            code: "jarvis.service_command_timeout",
        })?
        .map_err(|_| ServiceError::CommandFailed {
            code: "jarvis.service_command_failed",
        })?;

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    if combined.len() > MAX_CAPTURED_BYTES {
        combined.truncate(MAX_CAPTURED_BYTES);
    }

    Ok(CommandOutcome {
        status: output.status.code(),
        success: output.status.success(),
        output: combined,
    })
}

/// Runs `program` with `args` synchronously, returning whether it succeeded.
///
/// This exists for a read-only status query that the synchronous
/// [`ServiceController::status`][crate::service::ServiceController::status] contract needs. The
/// same bounds apply: a fixed
/// argument vector, no shell, a bounded wait, and bounded captured output.
///
/// # Errors
///
/// Returns [`ServiceError::ProgramUnavailable`] when the program cannot be
/// spawned and [`ServiceError::CommandFailed`] when it does not finish in time.
pub fn run_program_blocking(program: &str, args: &[String]) -> Result<bool, ServiceError> {
    use std::io::Read as _;

    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ServiceError::ProgramUnavailable)?;

    // A synchronous wait cannot observe a timeout without threads, so the child
    // is polled with a deadline and killed if it overruns.
    let deadline = std::time::Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ServiceError::CommandFailed {
                    code: "jarvis.service_command_timeout",
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                return Err(ServiceError::CommandFailed {
                    code: "jarvis.service_command_failed",
                });
            }
        }
    };

    // Drain the pipes so a chatty tool cannot deadlock on a full buffer.
    if let Some(mut stdout) = child.stdout.take() {
        let mut sink = Vec::new();
        let _ = stdout
            .by_ref()
            .take(MAX_CAPTURED_BYTES as u64)
            .read_to_end(&mut sink);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let mut sink = Vec::new();
        let _ = stderr
            .by_ref()
            .take(MAX_CAPTURED_BYTES as u64)
            .read_to_end(&mut sink);
    }

    Ok(status.success())
}

/// Executes a plan after writing its definition, when it has one.
///
/// # Errors
///
/// Returns [`ServiceError::DefinitionWrite`] when the definition file cannot be
/// created owner-only, and the command errors from [`run_program`].
pub async fn execute(plan: &super::ServicePlan) -> Result<CommandOutcome, ServiceError> {
    if let (Some(path), Some(definition)) = (&plan.definition_path, &plan.definition) {
        write_definition(path, definition.as_bytes())?;
    }

    let program = plan
        .program
        .to_str()
        .ok_or(ServiceError::ProgramUnavailable)?;
    run_program(program, &plan.args).await
}

/// Writes a definition file owner-only, creating its directory owner-only.
fn write_definition(path: &Path, bytes: &[u8]) -> Result<(), ServiceError> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // `umask` can only clear bits, so 0o600 is an upper bound.
        options.mode(0o600);
    }

    let mut file = options
        .open(path)
        .map_err(|_| ServiceError::DefinitionWrite)?;
    file.write_all(bytes)
        .map_err(|_| ServiceError::DefinitionWrite)?;
    file.sync_all().map_err(|_| ServiceError::DefinitionWrite)
}

/// Creates a directory owner-only on Unix.
fn create_private_dir(path: &Path) -> Result<(), ServiceError> {
    if path.is_dir() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(path)
            .map_err(|_| ServiceError::DefinitionWrite)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path).map_err(|_| ServiceError::DefinitionWrite)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_CAPTURED_BYTES, run_program};
    use crate::service::ServiceError;

    #[tokio::test]
    async fn a_missing_program_is_reported_not_panicked() {
        let error = run_program("jarvis-definitely-not-a-real-program", &[])
            .await
            .expect_err("must be unavailable");
        assert_eq!(error, ServiceError::ProgramUnavailable);
        assert_eq!(error.code(), "jarvis.service_program_unavailable");
        assert!(error.retryable());
    }

    #[tokio::test]
    async fn a_successful_command_reports_success_and_output() {
        // `cmd /c` and `sh -c` are used only here in a test to obtain a known
        // exit code; production code never goes through a shell.
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/c".to_owned(), "echo hello".to_owned()]);
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c".to_owned(), "echo hello".to_owned()]);

        let outcome = run_program(program, &args).await.expect("runs");
        assert!(outcome.succeeded(), "{outcome:?}");
        assert_eq!(outcome.status, Some(0));
        assert!(outcome.output.contains("hello"), "{outcome:?}");
    }

    #[tokio::test]
    async fn a_failing_command_reports_a_nonzero_status() {
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/c".to_owned(), "exit 3".to_owned()]);
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c".to_owned(), "exit 3".to_owned()]);

        let outcome = run_program(program, &args).await.expect("runs");
        assert!(!outcome.succeeded());
        assert_eq!(outcome.status, Some(3));
    }

    #[tokio::test]
    async fn captured_output_is_bounded() {
        // A chatty service-manager tool must not be able to exhaust memory, so a
        // command that emits far more than the bound is truncated.
        #[cfg(windows)]
        let (program, args) = (
            "cmd",
            vec![
                "/c".to_owned(),
                "for /L %i in (1,1,20000) do @echo 0123456789abcdef".to_owned(),
            ],
        );
        #[cfg(not(windows))]
        let (program, args) = (
            "sh",
            vec![
                "-c".to_owned(),
                "i=0; while [ $i -lt 20000 ]; do echo 0123456789abcdef; i=$((i+1)); done"
                    .to_owned(),
            ],
        );

        let outcome = run_program(program, &args).await.expect("runs");
        assert!(outcome.succeeded(), "{outcome:?}");
        assert!(
            outcome.output.len() <= MAX_CAPTURED_BYTES,
            "captured {} bytes, bound is {MAX_CAPTURED_BYTES}",
            outcome.output.len(),
        );
    }

    /// Exercises the synchronous read-only path used by `status`.
    #[test]
    fn the_blocking_path_reports_success_and_failure() {
        #[cfg(windows)]
        let (ok_program, ok_args, bad_program, bad_args) = (
            "cmd",
            vec!["/c".to_owned(), "echo ok".to_owned()],
            "cmd",
            vec!["/c".to_owned(), "exit 4".to_owned()],
        );
        #[cfg(not(windows))]
        let (ok_program, ok_args, bad_program, bad_args) = (
            "sh",
            vec!["-c".to_owned(), "echo ok".to_owned()],
            "sh",
            vec!["-c".to_owned(), "exit 4".to_owned()],
        );

        assert!(super::run_program_blocking(ok_program, &ok_args).expect("runs"));
        assert!(!super::run_program_blocking(bad_program, &bad_args).expect("runs"));

        let error = super::run_program_blocking("jarvis-definitely-not-a-real-program", &[])
            .expect_err("must be unavailable");
        assert_eq!(error, ServiceError::ProgramUnavailable);
    }
}
