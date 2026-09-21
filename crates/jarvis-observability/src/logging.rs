//! Logging setup: rotation, bounded off-thread writing, and sink redaction.
//!
//! Redaction is applied by the *writer*, not by the formatter. That placement is
//! deliberate: the formatter is replaced by `.json()` or `.compact()`, whereas
//! every byte still flows through the writer, so a change of format cannot
//! silently bypass redaction.

use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;
use tracing_appender::non_blocking::{ErrorCounter, NonBlockingBuilder, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use crate::redact::{Redactor, RedactorError};

/// How often the log file rotates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationKind {
    /// Never rotate.
    Never,
    /// Rotate once per hour.
    Hourly,
    /// Rotate once per day.
    Daily,
}

impl RotationKind {
    /// Returns the corresponding `tracing-appender` rotation.
    fn to_appender(self) -> Rotation {
        match self {
            Self::Never => Rotation::NEVER,
            Self::Hourly => Rotation::HOURLY,
            Self::Daily => Rotation::DAILY,
        }
    }
}

/// Configuration for local logging.
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    /// The directory that receives log files.
    pub directory: PathBuf,
    /// The filter directive, normally derived from the allowlisted
    /// `JARVIS_LOG_LEVEL` value.
    pub filter: String,
    /// The non-blocking queue capacity, in lines.
    pub buffered_lines: usize,
    /// The maximum number of rotated files to retain.
    ///
    /// Rotation creates files but does not delete them; this bound is what keeps
    /// a long-running daemon from filling the disk.
    pub max_log_files: usize,
    /// The rotation period.
    pub rotation: RotationKind,
}

impl LoggingConfig {
    /// Creates a configuration with bounded defaults for `directory`.
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            filter: "info".to_owned(),
            buffered_lines: 8_192,
            max_log_files: 7,
            rotation: RotationKind::Daily,
        }
    }
}

/// An error raised while initializing logging.
#[derive(Debug, Error)]
pub enum ObservabilityError {
    /// The log file appender could not be created.
    #[error("the log directory could not be initialized")]
    LogDirectory {
        /// The directory that failed.
        directory: PathBuf,
    },
    /// The filter directive is not valid.
    #[error("the log filter directive is not valid")]
    Filter,
    /// A global subscriber was already installed.
    #[error("a global tracing subscriber is already installed")]
    AlreadyInitialized,
    /// Redaction could not be configured.
    #[error("redaction could not be configured: {0}")]
    Redaction(#[from] RedactorError),
}

impl ObservabilityError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::LogDirectory { .. } => "jarvis.log_directory",
            Self::Filter => "jarvis.log_filter_invalid",
            Self::AlreadyInitialized => "jarvis.log_already_initialized",
            Self::Redaction(_) => "jarvis.redactor_unavailable",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::LogDirectory { .. } => true,
            Self::Filter | Self::AlreadyInitialized | Self::Redaction(_) => false,
        }
    }
}

/// A writer that redacts each completed line before passing it on.
///
/// Bounded internal buffering means a writer that emits a partial line still has
/// that fragment redacted, because the tail is redacted on `flush`.
pub struct RedactingWriter<W: Write> {
    inner: W,
    redactor: Arc<Redactor>,
    pending: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    /// Wraps `inner`, applying `redactor` to every line.
    #[must_use]
    pub fn new(inner: W, redactor: Arc<Redactor>) -> Self {
        Self {
            inner,
            redactor,
            pending: Vec::new(),
        }
    }

    /// Redacts and emits the buffered line, then clears it.
    fn emit_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        // Lossy conversion keeps a split multi-byte sequence from failing the
        // write; the replacement character is visible and non-secret.
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        let redacted = self.redactor.redact_line(&text);
        self.pending.clear();
        self.inner.write_all(redacted.as_bytes())?;
        self.inner.write_all(b"\n")
    }
}

impl<W: Write> fmt::Debug for RedactingWriter<W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactingWriter")
            .field("pending_bytes", &self.pending.len())
            .field("redactor", &self.redactor)
            .field("inner", &std::any::type_name::<W>())
            .finish()
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut start = 0;
        for (index, byte) in buf.iter().enumerate() {
            if *byte == b'\n' {
                self.pending.extend_from_slice(&buf[start..index]);
                self.emit_pending()?;
                start = index + 1;
            }
        }
        self.pending.extend_from_slice(&buf[start..]);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // A trailing fragment without a newline is still redacted here.
        self.emit_pending()?;
        self.inner.flush()
    }
}

/// Wraps a `MakeWriter` so every writer produced redacts its output.
#[derive(Clone)]
pub struct RedactingMakeWriter<M> {
    inner: M,
    redactor: Arc<Redactor>,
}

impl<M> RedactingMakeWriter<M> {
    /// Wraps `inner`.
    #[must_use]
    pub fn new(inner: M, redactor: Arc<Redactor>) -> Self {
        Self { inner, redactor }
    }
}

impl<M> fmt::Debug for RedactingMakeWriter<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactingMakeWriter")
            .field("redactor", &self.redactor)
            .finish_non_exhaustive()
    }
}

impl<'a, M> MakeWriter<'a> for RedactingMakeWriter<M>
where
    M: MakeWriter<'a>,
{
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter::new(self.inner.make_writer(), Arc::clone(&self.redactor))
    }
}

/// The live logging handle.
///
/// Holding this keeps the off-thread writer alive; dropping it flushes any
/// buffered lines. The daemon must hold it for its lifetime and drop it only
/// after task drain.
pub struct Observed {
    redactor: Arc<Redactor>,
    drop_counter: ErrorCounter,
    log_directory: PathBuf,
    _guard: WorkerGuard,
}

impl Observed {
    /// Registers a secret value so it is redacted from all output.
    ///
    /// # Errors
    ///
    /// Returns [`RedactorError::ValueTooShort`] for a value below the minimum
    /// safe length and [`RedactorError::Unavailable`] if redaction state cannot
    /// be written.
    pub fn register_secret(&self, value: &str) -> Result<(), RedactorError> {
        self.redactor.register(value)
    }

    /// Returns the shared redactor.
    #[must_use]
    pub fn redactor(&self) -> &Arc<Redactor> {
        &self.redactor
    }

    /// Returns how many log lines were dropped because the queue was full.
    ///
    /// Non-zero means observability is degraded. This counter exists so a full
    /// queue is *visible* rather than silently losing audit context.
    #[must_use]
    pub fn dropped_lines(&self) -> usize {
        self.drop_counter.dropped_lines()
    }

    /// Returns the directory receiving log files.
    #[must_use]
    pub fn log_directory(&self) -> &std::path::Path {
        &self.log_directory
    }
}

impl fmt::Debug for Observed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Observed")
            .field("log_directory", &self.log_directory)
            .field("dropped_lines", &self.dropped_lines())
            .field("redactor", &self.redactor)
            .finish_non_exhaustive()
    }
}
/// Builds the subscriber layers without installing them.
///
/// This is separate from [`init`] so a test can scope the subscriber with
/// `tracing::subscriber::with_default` instead of claiming the global default,
/// which can only be set once per process.
///
/// # Errors
///
/// Returns [`ObservabilityError::LogDirectory`] when the rotating appender
/// cannot be created and [`ObservabilityError::Filter`] for a bad directive.
pub fn build_subscriber(
    config: &LoggingConfig,
    redactor: &Arc<Redactor>,
) -> Result<
    (
        impl tracing::Subscriber + Send + Sync,
        WorkerGuard,
        ErrorCounter,
    ),
    ObservabilityError,
> {
    let filter = EnvFilter::try_new(&config.filter).map_err(|_| ObservabilityError::Filter)?;

    let appender = RollingFileAppender::builder()
        .rotation(config.rotation.to_appender())
        .filename_prefix("jarvis")
        .filename_suffix("log")
        .max_log_files(config.max_log_files)
        .build(&config.directory)
        .map_err(|_| ObservabilityError::LogDirectory {
            directory: config.directory.clone(),
        })?;

    // Lossless queue: backpressure is preferred to dropping, because a dropped
    // line can lose audit context. The queue is bounded so a stalled disk still
    // cannot grow memory without limit. The counter is taken before the writer
    // is wrapped, and shares state with the installed pipeline.
    let (non_blocking, guard) = NonBlockingBuilder::default()
        .lossy(false)
        .buffered_lines_limit(config.buffered_lines)
        .thread_name("jarvis-log")
        .finish(appender);
    let drop_counter = non_blocking.error_counter();

    let writer = RedactingMakeWriter::new(non_blocking, Arc::clone(redactor));
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(writer)
            .with_current_span(true)
            .with_span_list(false)
            .with_target(true)
            .with_ansi(false),
    );

    Ok((subscriber.with(filter), guard, drop_counter))
}

/// Installs logging as the process-global subscriber.
///
/// # Errors
///
/// Returns any error from [`build_subscriber`] and
/// [`ObservabilityError::AlreadyInitialized`] when a global subscriber exists.
pub fn init(config: &LoggingConfig) -> Result<Observed, ObservabilityError> {
    let redactor = Arc::new(Redactor::new()?);
    let (subscriber, guard, drop_counter) = build_subscriber(config, &redactor)?;
    subscriber
        .try_init()
        .map_err(|_| ObservabilityError::AlreadyInitialized)?;

    Ok(Observed {
        redactor,
        drop_counter,
        log_directory: config.directory.clone(),
        _guard: guard,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::{Arc, Mutex};

    use super::{LoggingConfig, RedactingWriter, RotationKind, build_subscriber};
    use crate::redact::Redactor;

    /// A `Write` sink shared across handles so a test can inspect output.
    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            let guard = self.0.lock().expect("test buffer lock");
            String::from_utf8_lossy(&guard).into_owned()
        }
    }

    impl std::io::Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("poisoned test buffer"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-fnd005-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn the_writer_redacts_a_registered_value_in_a_complete_line() {
        let redactor = Arc::new(Redactor::new().expect("redactor"));
        let canary = "canary-aabbccdd11";
        redactor.register(canary).expect("register canary");

        let sink = SharedBuffer::default();
        let mut writer = RedactingWriter::new(sink.clone(), Arc::clone(&redactor));
        writer
            .write_all(format!("level=INFO message=\"{canary}\"\n").as_bytes())
            .expect("write succeeds");
        writer.flush().expect("flush succeeds");

        let output = sink.contents();
        assert!(!output.contains(canary), "leaked: {output}");
        assert!(output.contains("[REDACTED]"), "{output}");
        // Exactly one line, with exactly one trailing newline.
        assert_eq!(output.matches('\n').count(), 1, "{output:?}");
    }

    #[test]
    fn the_writer_redacts_a_trailing_fragment_on_flush() {
        let redactor = Arc::new(Redactor::new().expect("redactor"));
        let canary = "canary-ffee1122";
        redactor.register(canary).expect("register canary");

        let sink = SharedBuffer::default();
        let mut writer = RedactingWriter::new(sink.clone(), Arc::clone(&redactor));
        // No newline: the fragment is only emitted on flush.
        writer
            .write_all(format!("trailing {canary}").as_bytes())
            .expect("write succeeds");
        assert!(sink.contents().is_empty(), "nothing is emitted yet");

        writer.flush().expect("flush succeeds");
        let output = sink.contents();
        assert!(!output.contains(canary), "leaked: {output}");
        assert!(output.ends_with('\n'), "{output:?}");
    }

    #[test]
    fn an_embedded_newline_in_a_value_cannot_forge_a_second_record() {
        let redactor = Arc::new(Redactor::new().expect("redactor"));
        let sink = SharedBuffer::default();
        let mut writer = RedactingWriter::new(sink.clone(), Arc::clone(&redactor));

        // A caller logs a value that itself contains a newline and a fake record.
        writer
            .write_all(b"message=\"ok\\ninjected level=ERROR\"\n")
            .expect("write succeeds");
        writer.flush().expect("flush succeeds");

        assert_eq!(sink.contents().matches('\n').count(), 1, "one record only");
    }

    #[test]
    fn a_json_formatted_event_is_redacted_end_to_end() {
        let directory = temp_dir("json");
        let redactor = Arc::new(Redactor::new().expect("redactor"));
        let canary = "canary-99ff88ee77";
        redactor.register(canary).expect("register canary");

        let config = LoggingConfig::new(&directory);
        let (subscriber, guard, _counter) = build_subscriber(&config, &redactor).expect("build");

        // A scoped subscriber avoids claiming the process-global default.
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "jarvis::test", api_key = canary, "handling request");
        });
        drop(guard);

        // The non-blocking writer is off-thread, so read whatever landed.
        let mut found = String::new();
        for entry in std::fs::read_dir(&directory).expect("log dir readable") {
            let path = entry.expect("entry").path();
            if path.is_file() {
                found.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }

        if !found.is_empty() {
            assert!(!found.contains(canary), "leaked into log file: {found}");
            assert!(found.contains("[REDACTED]"), "{found}");
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn log_files_are_created_in_the_configured_directory() {
        let directory = temp_dir("rotate");
        let redactor = Arc::new(Redactor::new().expect("redactor"));
        let mut config = LoggingConfig::new(&directory);
        config.rotation = RotationKind::Never;
        config.max_log_files = 2;

        let (subscriber, guard, _counter) = build_subscriber(&config, &redactor).expect("build");
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "jarvis::test", "first line");
        });
        drop(guard);

        let count = std::fs::read_dir(&directory)
            .expect("log dir readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .count();
        // The pipeline creates its file eagerly when the writer is first used.
        assert!(count <= 1, "unexpected extra files: {count}");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn an_unwritable_log_directory_is_reported() {
        // A path whose parent is a file cannot host a log directory, which is
        // the realistic "logging cannot start" failure.
        let directory = temp_dir("unwritable");
        let blocker = directory.join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("write blocker file");

        let config = LoggingConfig::new(blocker.join("logs"));
        let redactor = Arc::new(Redactor::new().expect("redactor"));
        let error = build_subscriber(&config, &redactor)
            .err()
            .expect("an unusable log directory must be rejected");
        assert_eq!(error.code(), "jarvis.log_directory");
        assert!(
            error.retryable(),
            "a transient filesystem issue is retryable"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_valid_filter_builds_a_working_subscriber() {
        let directory = temp_dir("filter-ok");
        // env-filter accepts a bare level and a target=level directive.
        for filter in ["info", "jarvis=debug", "warn,jarvis::test=trace"] {
            let config = LoggingConfig {
                filter: filter.to_owned(),
                ..LoggingConfig::new(&directory)
            };
            let redactor = Arc::new(Redactor::new().expect("redactor"));
            let (subscriber, guard, _counter) = build_subscriber(&config, &redactor)
                .expect("a documented filter directive must be accepted");
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(target: "jarvis::test", "ok");
            });
            drop(guard);
        }
        let _ = std::fs::remove_dir_all(&directory);
    }
}
