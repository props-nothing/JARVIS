//! Typed domain errors with stable, namespaced codes.
//!
//! Each error carries a stable machine-readable `code` and a `retryable` flag.
//! Retryability describes a specific operation outcome, not an error class in
//! every context, so it is decided by the variant that produced the error.

use std::fmt;

use thiserror::Error;

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
    /// A canonical tool identifier was not in its canonical form.
    ///
    /// Distinct from an unusable definition: this is the *identity* being wrong, and a
    /// caller that normalized it instead of refusing would let two spellings denote one
    /// tool — which is how an approval for one implementation is presented as an approval
    /// for a different one. See [`crate::tool::ToolCapability`].
    #[error("the value is not a canonical tool identifier")]
    ToolIdentifierNotCanonical,
    /// A canonical tool definition was empty, unbounded, or internally inconsistent.
    ///
    /// Carries the offending field rather than a prose message so an operator sees *which*
    /// part of a manifest was refused; a tool definition is assembled from several sources
    /// and "the definition is invalid" alone does not say where to look.
    #[error("the tool definition field `{field}` is not a usable value")]
    ToolDefinitionInvalid {
        /// The definition field that failed validation.
        field: &'static str,
    },
    /// Two different tools claimed the same identity.
    ///
    /// **A failure rather than a replacement.** Registration is where a collision is detectable,
    /// and it is the last place it is cheap to detect: both definitions are known here, while a
    /// caller resolving the identity later sees only one answer and cannot tell that anything was
    /// overwritten. Silently letting the later registration win is how a discovery cache change
    /// retargets an existing approval, which is the outcome the tool contract forbids.
    #[error("a tool with this identity is already registered with different content")]
    ToolIdentityConflict,
    /// One source identity was claimed by two different server configurations.
    ///
    /// This is the tool fabric's "source-identity collision" case. An identity names a **source**
    /// as its owner and version, so two servers declaring `acme.files 1.0.0` are indistinguishable
    /// by identity — and a second server that declares it inherits every approval recorded for the
    /// first. Refusing the claim is what makes impersonation impossible rather than merely
    /// detected later, and it is why the server **configuration** identity has to be part of what
    /// is registered rather than only part of the cache key.
    #[error("the source identity is already claimed by a different server configuration")]
    ToolSourceConflict,
    /// A tool identifier was looked up and is not registered.
    #[error("the tool is not registered")]
    ToolNotRegistered,
    /// An approval transition was attempted from a state that does not permit it.
    ///
    /// Covers both an edge the approval diagram does not contain and a transition on a **terminal**
    /// approval, because the two have the same cause: the approval's state does not allow the move.
    /// A decision is immutable once terminal, so "approve an already-consumed approval" and
    /// "consume a rejected one" are one class of refusal with two names for its states.
    #[error("the approval state transition is not an allowed edge")]
    ApprovalStateConflict {
        /// The state the approval is actually in.
        from: crate::tool::approval::ApprovalState,
        /// The requested target state.
        to: crate::tool::approval::ApprovalState,
    },
    /// An approval transition lost an optimistic-concurrency race.
    #[error("the approval version conflicts with a concurrent transition")]
    ApprovalVersionConflict {
        /// The version the caller expected.
        expected: crate::tool::approval::ApprovalVersion,
        /// The version that is actually current.
        actual: crate::tool::approval::ApprovalVersion,
    },
    /// An approval was presented for an action it did not approve.
    ///
    /// **A distinct error rather than a generic denial**, because a refusal of this kind needs a
    /// different user action from every other: the approval exists, is unexpired, and is unconsumed,
    /// so re-approving *the same action* changes nothing. The contract names the mechanism directly
    /// — "approving 'send this email' does not approve a rewritten recipient, subject, body,
    /// attachment, or account" — and this is the code that says so.
    #[error("the approval does not cover this action")]
    ApprovalFingerprintMismatch,
    /// A tool-call transition was attempted from a state that does not permit it.
    ///
    /// Names both states because the executor's response depends on them: a call that cannot be
    /// reserved is a duplicate submission, while a call that cannot be marked succeeded is a
    /// lifecycle fault, and "the transition is not allowed" alone does not distinguish them.
    #[error("the tool call state transition is not an allowed edge")]
    ToolCallStateConflict {
        /// The state the call is actually in.
        from: crate::tool::ledger::ToolCallState,
        /// The requested target state.
        to: crate::tool::ledger::ToolCallState,
    },
    /// A tool-call transition lost an optimistic-concurrency race.
    #[error("the tool call version conflicts with a concurrent transition")]
    ToolCallVersionConflict {
        /// The version the caller expected.
        expected: crate::tool::ledger::ToolCallVersion,
        /// The version that is actually current.
        actual: crate::tool::ledger::ToolCallVersion,
    },
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
            Self::ToolIdentifierNotCanonical => "tool.identifier_not_canonical",
            Self::ToolDefinitionInvalid { .. } => "tool.definition_invalid",
            Self::ToolIdentityConflict => "tool.conflict",
            Self::ToolSourceConflict => "tool.source_conflict",
            Self::ToolNotRegistered => "tool.not_found",
            Self::ApprovalStateConflict { .. } => "approval.state_conflict",
            Self::ApprovalVersionConflict { .. } => "approval.version_conflict",
            Self::ApprovalFingerprintMismatch => "approval.fingerprint_mismatch",
            Self::ToolCallStateConflict { .. } => "tool.state_conflict",
            Self::ToolCallVersionConflict { .. } => "tool.version_conflict",
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

/// A borrowed, namespaced error code.
///
/// This is a presentation type for boundaries (the error envelope emits
/// `code`). It borrows a constant, so constructing it never allocates.
///
/// **A code is accepted when its first segment is a namespace JARVIS owns**, not only
/// when it begins with `jarvis.`. The earlier rule tested the `jarvis.` prefix alone,
/// and the contracts disagree with it: `tool-contract.md` defines sixteen codes under
/// `tool.`, the approval contract twelve under `approval.`, and the model and storage
/// contracts use `model.` and `storage.`. Two consequences followed, and the second is
/// the one that matters. The tool-fabric codes would have been rewritten to
/// `jarvis.internal`, so the sixteen codes the contract tells a client to branch on
/// would **all have collapsed into one** — a client could no longer tell a rate limit
/// from a rejection. And a namespace the contracts own could not be added without
/// changing this function, so the rule that decided which codes may exist was written
/// in a place the contract could not see.
///
/// A namespace list rather than a prefix test, because *any* dotted string starts with
/// something: accepting everything would let a provider or a runtime name a code JARVIS
/// then forwards to a client as though JARVIS had produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorCode(&'static str);

impl ErrorCode {
    /// The namespaces the contracts define codes under.
    ///
    /// Order is irrelevant; this is a membership set, not a precedence order.
    pub const NAMESPACES: &'static [&'static str] = &[
        "jarvis", "tool", "approval", "model", "run", "stream", "storage", "event", "session",
    ];

    /// Creates an error code from a namespaced string.
    ///
    /// A code whose namespace JARVIS does not own is replaced by
    /// [`ErrorCode::INTERNAL`], so an accidental or foreign code can never reach a
    /// client as though JARVIS had produced it.
    #[must_use]
    pub const fn new(code: &'static str) -> Self {
        let bytes = code.as_bytes();
        let mut index = 0;
        while index < Self::NAMESPACES.len() {
            let namespace = Self::NAMESPACES[index].as_bytes();
            // Two conditions, and both are needed. The namespace must be followed by `.`, so
            // `toolbox.x` is not `tool.x` — a boundary-only check would accept it. And there
            // must be at least one character **after** the dot, because a namespace names a
            // family of errors and not an error: `tool.` is the family with nothing in it.
            if starts_with(bytes, namespace)
                && bytes.len() > namespace.len() + 1
                && bytes[namespace.len()] == b'.'
            {
                return Self(code);
            }
            index += 1;
        }
        Self::INTERNAL
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
            // **Asserted through `ErrorCode::new` rather than against a hand-written prefix
            // list**, because the prefix list is what was wrong: it admitted `jarvis.` and
            // `model.` only, so a `tool.` or `approval.` code passed this loop and was then
            // rewritten to `jarvis.internal` at the boundary. Asking the type that actually
            // gates the boundary is the difference between testing the list and testing the
            // rule. `jarvis.*` and `model.*` are covered as a consequence rather than by a
            // special case.
            assert_eq!(
                ErrorCode::new(code).as_str(),
                code,
                "code {code} must be accepted by the boundary, not rewritten to {}",
                ErrorCode::INTERNAL.as_str(),
            );
        }
    }

    #[test]
    fn error_code_rejects_a_non_namespaced_value() {
        // A code whose namespace JARVIS does not own falls back to the internal code. This
        // is the fail-closed direction: a foreign or accidental code must not reach a
        // client as though JARVIS had produced it.
        for foreign in [
            "acme.thing",
            "Tool.permission_denied",
            "toolbox.x",
            "tool.",
            "tool",
            "",
            "toolx",
        ] {
            assert_eq!(
                ErrorCode::new(foreign),
                ErrorCode::INTERNAL,
                "{foreign} must not be accepted as a JARVIS code",
            );
        }
        assert_eq!(
            ErrorCode::new("jarvis.invalid_timestamp").as_str(),
            "jarvis.invalid_timestamp",
        );
    }

    #[test]
    fn every_namespace_the_contracts_define_is_accepted() {
        // **This is the test whose absence let the defect survive.** The old assertion used
        // `tool.permission_denied` — a code `tool-contract.md` defines — as its example of a
        // value that must be REJECTED, so the single test covering the rule encoded the bug
        // rather than catching it.
        //
        // The samples are a table compared against `NAMESPACES` as a **set in both directions**,
        // rather than a `match` with a catch-all arm. A catch-all arm would accept a namespace
        // added to `NAMESPACES` without a sample — the one change that could otherwise slip
        // through, and the one that matters, since the sample is what proves the namespace is
        // actually reached. Set equality makes either omission a failure.
        let samples: [(&'static str, &'static str); 9] = [
            ("jarvis", "jarvis.invalid_timestamp"),
            ("tool", "tool.permission_denied"),
            ("approval", "approval.required"),
            ("model", "model.policy_not_found"),
            ("run", "run.failed"),
            ("stream", "stream.overrun"),
            ("storage", "storage.transition_refused"),
            ("event", "event.envelope_invalid"),
            ("session", "session.expired"),
        ];
        let mut claimed: Vec<&str> = samples.iter().map(|(namespace, _)| *namespace).collect();
        claimed.sort_unstable();
        let mut declared: Vec<&str> = ErrorCode::NAMESPACES.to_vec();
        declared.sort_unstable();
        assert_eq!(
            claimed, declared,
            "every declared namespace needs a sample here and every sample needs a declared \
             namespace; a mismatch means one was added without the other",
        );

        for (namespace, sample) in samples {
            assert_eq!(
                ErrorCode::new(sample).as_str(),
                sample,
                "the {namespace} namespace must be accepted",
            );
            // The namespace **alone** is not a code: a family name names no error. Written as a
            // literal pair rather than a concatenation because `ErrorCode::new` takes a
            // `&'static str` and this must not allocate.
            let bare = match namespace {
                "jarvis" => "jarvis.",
                "tool" => "tool.",
                "approval" => "approval.",
                "model" => "model.",
                "run" => "run.",
                "stream" => "stream.",
                "storage" => "storage.",
                "event" => "event.",
                _ => "session.",
            };
            assert_eq!(
                ErrorCode::new(bare),
                ErrorCode::INTERNAL,
                "the bare namespace {namespace} names no error",
            );
        }
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
