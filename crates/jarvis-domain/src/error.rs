//! Typed domain errors with stable, namespaced codes.
//!
//! Each error carries a stable machine-readable `code` and a `retryable` flag.
//! Retryability describes a specific operation outcome, not an error class in
//! every context, so it is decided by the variant that produced the error.

use std::fmt;

use thiserror::Error;

/// The stable namespace prefix for every JARVIS-owned error code.
const CODE_PREFIX: &str = "jarvis.";

/// An error produced by a domain primitive.
///
/// Libraries return this typed error; higher layers add human-facing context.
/// The [`Display`](fmt::Display) message is safe to surface to a caller: it
/// never contains secrets, credentials, or untrusted payload bytes.
#[derive(Debug, Error)]
pub enum DomainError {
    /// A value was not a canonical, well-formed identifier.
    #[error("the value is not a canonical JARVIS identifier")]
    InvalidIdentifier {
        /// The identifier kind that failed to parse, for operator diagnostics.
        kind: &'static str,
    },
    /// A timestamp was not a valid absolute instant.
    #[error("the value is not a valid RFC 3339 UTC instant")]
    InvalidTimestamp,
    /// The operating-system clock reported an out-of-range or unusable value.
    #[error("the system clock reported an unusable instant")]
    SystemClockUnavailable,
}

impl DomainError {
    /// Returns the stable, namespaced error code.
    ///
    /// Codes are contract surface: clients may branch on them, so they change
    /// only with a contract-version change.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidIdentifier { .. } => "jarvis.invalid_identifier",
            Self::InvalidTimestamp => "jarvis.invalid_timestamp",
            Self::SystemClockUnavailable => "jarvis.system_clock_unavailable",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    ///
    /// Every current variant fails deterministically for the same input, so a
    /// blind retry cannot succeed and is reported as non-retryable.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::InvalidIdentifier { .. }
            | Self::InvalidTimestamp
            | Self::SystemClockUnavailable => false,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// A borrowed, namespaced error code with a `jarvis.` prefix.
///
/// This is a presentation type for boundaries (the error envelope emits
/// `code`). It borrows a constant, so constructing it never allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorCode(&'static str);

impl ErrorCode {
    /// Creates an error code from a namespaced string.
    ///
    /// A code that does not carry the JARVIS prefix is replaced by
    /// [`ErrorCode::internal`] so an accidental non-namespaced code can never
    /// reach a client.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        // `starts_with` is const-stable for `&str`; fall back to a byte compare.
        if starts_with(code.as_bytes(), CODE_PREFIX.as_bytes()) {
            Self(code)
        } else {
            Self::INTERNAL
        }
    }

    /// The code used when no more specific, namespaced code applies.
    pub const INTERNAL: Self = Self("jarvis.internal");

    /// Returns the code as a string slice.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Returns whether `haystack` begins with `needle`.
const fn starts_with(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    let mut index = 0;
    while index < needle.len() {
        if haystack[index] != needle[index] {
            return false;
        }
        index += 1;
    }
    true
}

impl From<&DomainError> for ErrorCode {
    fn from(error: &DomainError) -> Self {
        Self::new(error.code())
    }
}

#[cfg(test)]
mod tests {
    use super::{DomainError, ErrorCode};

    #[test]
    fn every_code_is_namespaced_and_unique_per_variant() {
        let errors = [
            DomainError::InvalidIdentifier { kind: "workspace" },
            DomainError::InvalidTimestamp,
            DomainError::SystemClockUnavailable,
        ];
        let mut codes: Vec<&str> = errors.iter().map(DomainError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len(), "error codes must be unique");
        for code in codes {
            assert!(
                code.starts_with("jarvis."),
                "code {code} must be namespaced",
            );
        }
    }

    #[test]
    fn error_code_rejects_a_non_namespaced_value() {
        assert_eq!(
            ErrorCode::new("tool.permission_denied"),
            ErrorCode::INTERNAL
        );
        assert_eq!(
            ErrorCode::new("jarvis.invalid_timestamp").as_str(),
            "jarvis.invalid_timestamp",
        );
    }

    #[test]
    fn domain_errors_are_not_retryable() {
        for error in [
            DomainError::InvalidIdentifier { kind: "principal" },
            DomainError::InvalidTimestamp,
            DomainError::SystemClockUnavailable,
        ] {
            assert!(!error.retryable(), "{} must not be retryable", error.code());
        }
    }
}
