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
    /// A provider-supplied string was empty or exceeded its bound.
    #[error("a provider value is empty or exceeds its bounded length")]
    UnboundedProviderValue,
    /// A stream sequence counter could not advance.
    #[error("the model stream sequence is exhausted")]
    StreamSequenceExhausted,
    /// A model stream frame named a different call than the stream owns.
    #[error("the model stream frame belongs to a different call")]
    StreamCallMismatch,
    /// A model stream frame repeated or reordered its sequence.
    #[error("the model stream sequence must increase strictly")]
    StreamSequenceNotMonotonic,
    /// A second stream-start frame arrived.
    #[error("the model stream has already started")]
    StreamAlreadyStarted,
    /// A frame referenced a tool call that was never added.
    #[error("the frame references an unknown tool call")]
    UnknownToolCall,
    /// The assembled argument deltas did not match the completion payload.
    #[error("the tool argument deltas do not match the completed arguments")]
    ToolArgumentsMismatch,
    /// A stream ended with a tool call that was never completed.
    #[error("the model stream ended with an unfinished tool call")]
    UnfinishedToolCall,
    /// A tool call was dispatched with incomplete or invalid arguments.
    ///
    /// A distinct variant because the two cases need different operator
    /// responses: an incomplete call means frames were lost, while invalid JSON
    /// means the model produced unusable arguments.
    #[error("the tool arguments are not complete, valid JSON")]
    ToolArgumentsNotExecutable,
    /// A tool result was present without the tool call it answers.
    #[error("a tool result has no matching tool call")]
    OrphanedToolResult,
    /// One canonical tool-call ID was declared twice.
    #[error("the same canonical tool call identifier appears twice")]
    DuplicateToolCallId,
    /// A requested policy relaxation could not be expressed as valid policy.
    #[error("the model data policy layer is not a usable value")]
    InvalidPolicyLayer,
    /// A model advertised streaming but delivers its output as a single burst.
    ///
    /// Distinct from an absent measurement: one is a model that does not deliver
    /// incrementally, the other is a model nobody measured, and a boolean
    /// `streaming` flag cannot tell them apart.
    #[error("the model delivers its output as a single burst")]
    DeliveryNotIncremental,
    /// A model data policy was not found.
    #[error("the model data policy was not found")]
    PolicyNotFound,
    /// A model data policy update lost a version race.
    #[error("the model data policy version conflicts with a concurrent update")]
    PolicyVersionConflict,
    /// No compliant route satisfies the resolved model data policy.
    #[error("no compliant model route satisfies the active policy")]
    PolicyUnsatisfied,
    /// A required capability has no evidence at all.
    #[error("the provider capability has no evidence")]
    EvidenceMissing,
    /// A required capability's evidence has expired.
    #[error("the provider capability evidence is stale")]
    EvidenceStale,
    /// A route needs a policy exception and none was granted.
    #[error("a model data policy exception is required")]
    ExceptionRequired,
    /// A route relied on a policy exception that has expired.
    #[error("the model data policy exception has expired")]
    ExceptionExpired,
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
            Self::UnboundedProviderValue => "jarvis.unbounded_provider_value",
            Self::StreamSequenceExhausted => "jarvis.stream_sequence_exhausted",
            Self::StreamCallMismatch => "jarvis.stream_call_mismatch",
            Self::StreamSequenceNotMonotonic => "jarvis.stream_sequence_not_monotonic",
            Self::StreamAlreadyStarted => "jarvis.stream_already_started",
            Self::UnknownToolCall => "jarvis.unknown_tool_call",
            Self::ToolArgumentsMismatch => "jarvis.tool_arguments_mismatch",
            Self::UnfinishedToolCall => "jarvis.unfinished_tool_call",
            Self::ToolArgumentsNotExecutable => "jarvis.tool_arguments_not_executable",
            Self::OrphanedToolResult => "jarvis.orphaned_tool_result",
            Self::DuplicateToolCallId => "jarvis.duplicate_tool_call_id",
            Self::InvalidPolicyLayer => "jarvis.invalid_policy_layer",
            Self::DeliveryNotIncremental => "jarvis.delivery_not_incremental",
            Self::PolicyNotFound => "model.policy_not_found",
            Self::PolicyVersionConflict => "model.policy_version_conflict",
            Self::PolicyUnsatisfied => "model.policy_unsatisfied",
            Self::EvidenceMissing => "model.evidence_missing",
            Self::EvidenceStale => "model.evidence_stale",
            Self::ExceptionRequired => "model.exception_required",
            Self::ExceptionExpired => "model.exception_expired",
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
            | Self::SystemClockUnavailable
            | Self::UnboundedProviderValue
            | Self::StreamSequenceExhausted
            | Self::StreamCallMismatch
            | Self::StreamSequenceNotMonotonic
            | Self::StreamAlreadyStarted
            | Self::UnknownToolCall
            | Self::ToolArgumentsMismatch
            | Self::UnfinishedToolCall
            | Self::ToolArgumentsNotExecutable
            | Self::OrphanedToolResult
            | Self::DuplicateToolCallId
            | Self::InvalidPolicyLayer
            | Self::DeliveryNotIncremental
            | Self::PolicyNotFound
            | Self::PolicyVersionConflict
            | Self::PolicyUnsatisfied
            | Self::EvidenceMissing
            | Self::EvidenceStale
            | Self::ExceptionRequired
            | Self::ExceptionExpired => false,
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
    fn the_model_policy_codes_are_the_contracts_codes_verbatim() {
        // The model data policy contract fixes these strings. A rename here would
        // silently change a documented client-visible code, and the contract's
        // own text would then be wrong without any test noticing.
        for (error, expected) in [
            (DomainError::PolicyNotFound, "model.policy_not_found"),
            (
                DomainError::PolicyVersionConflict,
                "model.policy_version_conflict",
            ),
            (DomainError::PolicyUnsatisfied, "model.policy_unsatisfied"),
            (DomainError::EvidenceMissing, "model.evidence_missing"),
            (DomainError::EvidenceStale, "model.evidence_stale"),
            (DomainError::ExceptionRequired, "model.exception_required"),
            (DomainError::ExceptionExpired, "model.exception_expired"),
        ] {
            assert_eq!(error.code(), expected);
            assert!(!error.retryable(), "{expected} is not retryable unchanged");
        }
    }

    #[test]
    fn every_model_stream_and_policy_code_is_unique_and_namespaced() {
        let errors = [
            DomainError::UnboundedProviderValue,
            DomainError::StreamSequenceExhausted,
            DomainError::StreamCallMismatch,
            DomainError::StreamSequenceNotMonotonic,
            DomainError::StreamAlreadyStarted,
            DomainError::UnknownToolCall,
            DomainError::ToolArgumentsMismatch,
            DomainError::UnfinishedToolCall,
            DomainError::ToolArgumentsNotExecutable,
            DomainError::OrphanedToolResult,
            DomainError::DuplicateToolCallId,
            DomainError::InvalidPolicyLayer,
            DomainError::DeliveryNotIncremental,
            DomainError::PolicyNotFound,
            DomainError::PolicyVersionConflict,
            DomainError::PolicyUnsatisfied,
            DomainError::EvidenceMissing,
            DomainError::EvidenceStale,
            DomainError::ExceptionRequired,
            DomainError::ExceptionExpired,
        ];
        let mut codes: Vec<&str> = errors.iter().map(DomainError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len(), "codes must be unique");
        for code in codes {
            assert!(
                code.starts_with("jarvis.") || code.starts_with("model."),
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
