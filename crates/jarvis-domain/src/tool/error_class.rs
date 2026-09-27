//! The tool contract's error classes and the retry posture of each.
//!
//! The contract lists sixteen codes and says provider errors "map to these while protected
//! diagnostics retain a bounded safe source reference". They are a closed set here, so a code
//! outside it is refused rather than forwarded, and each carries a [`RetryPosture`].
//!
//! **Why a posture rather than a `retryable` boolean.** The project's own rule — stated in
//! `jarvis_domain::error` — is that "retryability describes a specific operation outcome, not an
//! error class in every context". For tool calls that distinction is load-bearing, because the same
//! class answers differently depending on the tool: a `timeout` on a naturally-idempotent read can
//! be retried, while a `timeout` on a tool that declared `Idempotency::None` may have already sent
//! the message. A per-class boolean would have to pick one answer and be wrong for the other, and
//! the wrong direction is the one that sends twice. So the class states *what kind* of retry is
//! permissible and [`ToolErrorClass::retryable_for`] resolves it against the tool's declaration —
//! which is the only input that can settle the question.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::classification::Idempotency;

/// How a retry of a failed call relates to the effect it may already have had.
///
/// Three postures rather than two, because the middle one is where the danger lives: an error that
/// may or may not have taken effect cannot be answered "retryable" or "not retryable" without being
/// wrong in one direction, and the safe answer depends on what the tool declared about repetition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RetryPosture {
    /// Retrying unchanged fails again for the same reason.
    ///
    /// Includes the cases a caller often mistakes for transient: a rejected approval and an expired
    /// one both need a **new** approval rather than a repeat, and `outcome_ambiguous` is the
    /// contract's explicit "enter reconciliation, never automatic retry".
    Never,
    /// Retrying unchanged may succeed and cannot duplicate an effect.
    ///
    /// The request was refused **before** anything happened — the provider was unavailable, or a
    /// rate limit rejected it — so there is nothing to duplicate.
    Safe,
    /// Retrying may succeed but could repeat an effect that already happened.
    ///
    /// The outcome is unknown, which is what makes this the only posture that needs the tool's own
    /// declaration to resolve. A tool that declared itself idempotent may be retried; one that
    /// declared no repetition story must be reconciled instead, because a second send of the same
    /// message is not a retry but a second message.
    OnlyIfIdempotent,
}

/// One of the tool contract's sixteen error classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorClass {
    /// No such tool, or it is not registered.
    NotFound,
    /// The tool exists but cannot be reached or is unhealthy.
    Unavailable,
    /// The arguments did not validate against the tool's input schema.
    SchemaInvalid,
    /// The caller is not authorized to invoke this tool with these effects.
    PermissionDenied,
    /// The call needs an approval that does not exist.
    ApprovalRequired,
    /// An approval was refused.
    ApprovalRejected,
    /// An approval existed and has expired.
    ApprovalExpired,
    /// The call's state conflicts with the request, for example a repeated reservation.
    Conflict,
    /// The tool or provider refused the rate of calls.
    RateLimited,
    /// The call exceeded its deadline.
    Timeout,
    /// The call was cancelled.
    Cancelled,
    /// The provider rejected JARVIS's credentials.
    ProviderAuth,
    /// The provider reported a failure.
    ProviderError,
    /// The result did not validate against the tool's output schema, or exceeded its bounds.
    OutputInvalid,
    /// The provider's outcome cannot be determined.
    ///
    /// **Not a failure to report and move on.** The contract says an unknown outcome "uses
    /// `RECONCILING`, not automatic retry", because the effect may or may not have happened and only
    /// reconciliation can establish which. Its posture is therefore [`RetryPosture::Never`] even
    /// though the call might be repeatable: what is needed is a read, not a second write.
    OutcomeAmbiguous,
    /// A bound was exceeded — arguments, result size, attempts, or a budget.
    LimitExceeded,
}

impl ToolErrorClass {
    /// Every class, in the order the contract lists them.
    pub const ALL: &'static [Self] = &[
        Self::NotFound,
        Self::Unavailable,
        Self::SchemaInvalid,
        Self::PermissionDenied,
        Self::ApprovalRequired,
        Self::ApprovalRejected,
        Self::ApprovalExpired,
        Self::Conflict,
        Self::RateLimited,
        Self::Timeout,
        Self::Cancelled,
        Self::ProviderAuth,
        Self::ProviderError,
        Self::OutputInvalid,
        Self::OutcomeAmbiguous,
        Self::LimitExceeded,
    ];

    /// Parses the contract's code.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::DomainError::ToolDefinitionInvalid`] naming `error_class` for any
    /// value outside the set. A provider's own error text must be **mapped** by its adapter rather
    /// than passed through, and refusing an unknown code is what makes that mapping mandatory
    /// instead of optional: a class that fell through as `Unknown` would reach a client as a code
    /// the contract does not define.
    pub fn parse(value: &str) -> Result<Self, crate::error::DomainError> {
        Self::ALL
            .iter()
            .copied()
            .find(|class| class.as_contract_str() == value)
            .ok_or(crate::error::DomainError::ToolDefinitionInvalid {
                field: "error_class",
            })
    }

    /// Returns the contract's code.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::NotFound => "tool.not_found",
            Self::Unavailable => "tool.unavailable",
            Self::SchemaInvalid => "tool.schema_invalid",
            Self::PermissionDenied => "tool.permission_denied",
            Self::ApprovalRequired => "tool.approval_required",
            Self::ApprovalRejected => "tool.approval_rejected",
            Self::ApprovalExpired => "tool.approval_expired",
            Self::Conflict => "tool.conflict",
            Self::RateLimited => "tool.rate_limited",
            Self::Timeout => "tool.timeout",
            Self::Cancelled => "tool.cancelled",
            Self::ProviderAuth => "tool.provider_auth",
            Self::ProviderError => "tool.provider_error",
            Self::OutputInvalid => "tool.output_invalid",
            Self::OutcomeAmbiguous => "tool.outcome_ambiguous",
            Self::LimitExceeded => "tool.limit_exceeded",
        }
    }

    /// Returns what kind of retry this class permits.
    #[must_use]
    pub const fn retry_posture(self) -> RetryPosture {
        match self {
            // Refused before anything happened: the provider was not reached, or the request was
            // rejected at the door. Retrying cannot duplicate an effect.
            Self::Unavailable | Self::RateLimited => RetryPosture::Safe,
            // May or may not have taken effect. The tool's own declaration decides.
            Self::Timeout | Self::ProviderError => RetryPosture::OnlyIfIdempotent,
            // Everything else fails again unchanged, or needs a decision rather than a repeat.
            Self::NotFound
            | Self::SchemaInvalid
            | Self::PermissionDenied
            | Self::ApprovalRequired
            | Self::ApprovalRejected
            | Self::ApprovalExpired
            | Self::Conflict
            | Self::Cancelled
            | Self::ProviderAuth
            | Self::OutputInvalid
            | Self::OutcomeAmbiguous
            | Self::LimitExceeded => RetryPosture::Never,
        }
    }

    /// Resolves whether a call that failed this way may be retried, given the tool's declaration.
    ///
    /// **This is the only place the question is answered**, so the resolution cannot differ between
    /// two call sites. `idempotency` is required rather than defaulted: a caller that did not look up
    /// the tool's declaration has no basis for a retry, and defaulting to "not idempotent" would be
    /// the safe answer *only* by accident of this particular default.
    #[must_use]
    pub const fn retryable_for(self, idempotency: Idempotency) -> bool {
        match self.retry_posture() {
            RetryPosture::Never => false,
            RetryPosture::Safe => true,
            // `CallerKeyed` counts as idempotent **when the same key is reused**, which is the whole
            // point of the key: a retry that reuses it is the same logical operation, so the tool
            // can recognise the repeat. `NaturallyIdempotent` needs no key. Only `None` — the default
            // for a tool whose author said nothing — leaves the repeat unanswerable, and it is
            // refused.
            RetryPosture::OnlyIfIdempotent => !matches!(idempotency, Idempotency::None),
        }
    }

    /// Returns whether this class means the call's outcome is unsettled.
    ///
    /// Used to decide whether a call enters reconciliation. Separate from the retry question
    /// because the two are not opposites: `Timeout` may be retryable **and** unsettled, depending on
    /// the tool, while `OutcomeAmbiguous` is unsettled and never retryable.
    #[must_use]
    pub const fn is_unsettled(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::ProviderError | Self::OutcomeAmbiguous
        )
    }
}

impl fmt::Display for ToolErrorClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}
