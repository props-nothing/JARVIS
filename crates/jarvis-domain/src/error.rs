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
    /// A portable sampling or reasoning control was outside its accepted range.
    ///
    /// Distinct from an unknown field: an unknown provider key is a parse
    /// rejection, while a known control with an impossible value is a semantic
    /// refusal, and the two need different operator responses.
    #[error("a portable model setting is outside its accepted range")]
    ModelSettingsInvalid,
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
    /// A run transition named a target the architecture diagram does not contain.
    ///
    /// Distinct from a version conflict: this is a caller asking for an edge that
    /// does not exist, not a caller working from a stale view, and the two need
    /// different operator responses.
    #[error("the run state transition is not an allowed edge")]
    RunTransitionNotAllowed {
        /// The state the run is actually in.
        from: crate::run::state::RunState,
        /// The requested target state.
        to: crate::run::state::RunState,
    },
    /// A run transition lost an optimistic-concurrency race.
    #[error("the run version conflicts with a concurrent transition")]
    RunVersionConflict {
        /// The version the caller expected.
        expected: crate::run::state::RunVersion,
        /// The version that is actually current.
        actual: crate::run::state::RunVersion,
    },
    /// A transition was attempted on a run that has already reached a terminal state.
    ///
    /// Carries the terminal state because, when a write is refused for this reason,
    /// the operator's next question is *which* terminal state the run settled in —
    /// "already terminal" alone does not distinguish a completed run from a
    /// cancelled one.
    #[error("the run has already reached a terminal state")]
    RunAlreadyTerminal {
        /// The terminal state the run is in.
        state: crate::run::state::RunState,
    },
    /// The run version counter could not advance.
    #[error("the run version is exhausted")]
    RunVersionExhausted,
    /// A transition reason was empty or exceeded its bound.
    #[error("the transition reason is empty or exceeds its bounded length")]
    InvalidTransitionReason,
    /// A context token budget was zero or exceeded its bound.
    #[error("the context token budget is outside its accepted range")]
    ContextBudgetInvalid,
    /// More context candidates were offered than the bound allows.
    #[error("too many context candidates were offered")]
    ContextCandidatesUnbounded,
    /// A context candidate was malformed.
    ///
    /// Distinct from an unbounded set: one is a single unusable item, the other is a
    /// list too large to process, and the two need different caller responses.
    #[error("the context candidate is empty, unbounded, or not a usable value")]
    ContextCandidateInvalid,
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
            Self::ModelSettingsInvalid => "model.settings_invalid",
            Self::PolicyNotFound => "model.policy_not_found",
            Self::PolicyVersionConflict => "model.policy_version_conflict",
            Self::PolicyUnsatisfied => "model.policy_unsatisfied",
            Self::EvidenceMissing => "model.evidence_missing",
            Self::EvidenceStale => "model.evidence_stale",
            Self::ExceptionRequired => "model.exception_required",
            Self::ExceptionExpired => "model.exception_expired",
            Self::RunTransitionNotAllowed { .. } => "jarvis.run_transition_not_allowed",
            Self::RunVersionConflict { .. } => "jarvis.run_version_conflict",
            Self::RunAlreadyTerminal { .. } => "jarvis.run_already_terminal",
            Self::RunVersionExhausted => "jarvis.run_version_exhausted",
            Self::InvalidTransitionReason => "jarvis.invalid_transition_reason",
            Self::ContextBudgetInvalid => "jarvis.context_budget_invalid",
            Self::ContextCandidatesUnbounded => "jarvis.context_candidates_unbounded",
            Self::ContextCandidateInvalid => "jarvis.context_candidate_invalid",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    ///
    /// This is decided per variant rather than per error class, because the same
    /// "conflict" idea has different answers here. A **version conflict** is
    /// retryable: the caller re-reads the run, recomputes the transition against
    /// the current state, and can succeed — that is the whole point of stating an
    /// expected version. Everything else is not: an absent edge stays absent, a
    /// terminal run stays terminal, and a reason that failed validation fails again
    /// for the same input.
    ///
    /// The context errors are all non-retryable for the same reason: an invalid
    /// budget, an over-large candidate list, and a malformed candidate all fail
    /// again unchanged.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::RunVersionConflict { .. })
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
    /// [`ErrorCode::INTERNAL`] so an accidental non-namespaced code can never
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
            DomainError::ModelSettingsInvalid,
            DomainError::PolicyNotFound,
            DomainError::PolicyVersionConflict,
            DomainError::PolicyUnsatisfied,
            DomainError::EvidenceMissing,
            DomainError::EvidenceStale,
            DomainError::ExceptionRequired,
            DomainError::ExceptionExpired,
            DomainError::RunTransitionNotAllowed {
                from: crate::run::state::RunState::Received,
                to: crate::run::state::RunState::Planning,
            },
            DomainError::RunVersionConflict {
                expected: crate::run::state::RunVersion::FIRST,
                actual: crate::run::state::RunVersion::new(2),
            },
            DomainError::RunAlreadyTerminal {
                state: crate::run::state::RunState::Completed,
            },
            DomainError::RunVersionExhausted,
            DomainError::InvalidTransitionReason,
            DomainError::ContextBudgetInvalid,
            DomainError::ContextCandidatesUnbounded,
            DomainError::ContextCandidateInvalid,
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

    #[test]
    fn a_stale_run_version_is_retryable_but_the_other_run_errors_are_not() {
        // A version conflict is the one domain error a caller can resolve by
        // re-reading and recomputing, which is what stating an expected version is
        // for. An absent edge, a terminal run, and an invalid reason all fail again
        // for the same input, so retrying them blindly cannot succeed.
        assert!(
            DomainError::RunVersionConflict {
                expected: crate::run::state::RunVersion::FIRST,
                actual: crate::run::state::RunVersion::new(2),
            }
            .retryable(),
        );
        for error in [
            DomainError::RunTransitionNotAllowed {
                from: crate::run::state::RunState::Received,
                to: crate::run::state::RunState::Planning,
            },
            DomainError::RunAlreadyTerminal {
                state: crate::run::state::RunState::Completed,
            },
            DomainError::RunVersionExhausted,
            DomainError::InvalidTransitionReason,
            DomainError::ContextBudgetInvalid,
            DomainError::ContextCandidatesUnbounded,
            DomainError::ContextCandidateInvalid,
        ] {
            assert!(!error.retryable(), "{} must not be retryable", error.code());
        }
    }
}
