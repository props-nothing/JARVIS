//! Local, structured observability support for JARVIS processes.
//!
//! Logging is local and structured; there is no remote telemetry sink in this
//! milestone. Secret redaction is applied at the writer, so changing the log
//! format cannot bypass it.
#![forbid(unsafe_code)]

pub mod logging;
pub mod redact;

pub use logging::{
    LoggingConfig, ObservabilityError, Observed, RedactingMakeWriter, RedactingWriter,
    RotationKind, build_subscriber, init,
};
pub use redact::{MIN_SECRET_LEN, REDACTED, Redactor, RedactorError, sanitize_control_chars};
