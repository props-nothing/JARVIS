//! Retry policy for a model call, and the pure decision that applies it.
//!
//! `model-gateway.md` states the rule this module implements: **"exactly one layer owns
//! each retry"**, with a bounded list of retry candidates and an explicit list of
//! failures that must not be retried. The decision is here, in the domain, so it is one
//! function rather than a condition repeated at each failure site.
//!
//! ## The safety boundary is *when* the failure happened, not only why
//!
//! The contract's most important sentence here is that an **ambiguous accepted request**
//! must not be retried unless provider idempotency is documented and used. That is why
//! [`FailureSite`] is a required input rather than a detail the caller may omit: a
//! connection reset *before* the provider accepted the call cannot have consumed
//! anything, while a failure *after* acceptance may have produced output, been billed,
//! and left a provider-side record. Those two situations can carry the *same* error
//! ([`FailureClass::Permanent`] versus [`FailureClass::Transient`] is a separate axis), so the
//! classification alone cannot decide — and a retry policy that looked
//! only at the error would retry the ambiguous case, which is the one the contract names.
//!
//! ## Why the retry must fit inside the deadline
//!
//! A retry whose backoff would outlive the run's deadline is not a retry; it is a delay
//! followed by the same failure, with the run's remaining time spent to learn nothing.
//! [`RetryDecision::decide`] therefore refuses a retry that cannot begin before the
//! deadline, which makes "the run failed on time" true rather than approximate.
//!
//! ## No jitter, deliberately
//!
//! Backoff is deterministic. Adding jitter would need an entropy source, and the
//! application crate has none in its reviewed dependency set; more importantly a
//! deterministic schedule is the one a test can assert. Jitter matters for a fleet
//! retrying in lockstep, which a single local daemon serving one run is not.

use serde::{Deserialize, Serialize};

use crate::run::budget::{BudgetStatus, RunBudget};
use crate::time::UtcTimestamp;

/// How many attempts one logical model call may make, and how long to wait between them.
///
/// Bounded by construction: an unbounded retry count is an unbounded run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// The total number of attempts allowed, including the first.
    ///
    /// `1` means "do not retry", which is a legitimate policy and the default for a run
    /// that did not ask for one.
    pub max_attempts: u32,
    /// The delay before the second attempt, in milliseconds.
    pub base_backoff_ms: u64,
    /// The ceiling the backoff may grow to, in milliseconds.
    pub max_backoff_ms: u64,
}

/// The largest accepted attempt count.
///
/// Bounded for the same reason every duration here is: each attempt spends the run's
/// time and budget, so a very large count is a way to spend both without bound. Five is
/// generous for a local provider and still finite.
pub const MAX_ATTEMPTS: u32 = 5;

/// The largest accepted backoff, in milliseconds.
pub const MAX_BACKOFF_MS: u64 = 60_000;

impl RetryPolicy {
    /// A policy that never retries.
    ///
    /// The default, because retrying is a choice with a cost: a provider that fails for a
    /// reason the repeat cannot change should be reported, not repeated, and a policy that
    /// retried by default would spend a budget the caller never offered.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            max_attempts: 1,
            base_backoff_ms: 0,
            max_backoff_ms: 0,
        }
    }

    /// Builds a policy of at most `max_attempts` attempts with exponential backoff.
    ///
    /// # Errors
    ///
    /// Returns [`RetryError::AttemptsOutOfRange`] when `max_attempts` is zero or over
    /// [`MAX_ATTEMPTS`], and [`RetryError::BackoffOutOfRange`] when the base is greater
    /// than the ceiling or the ceiling is over [`MAX_BACKOFF_MS`]. A base above the
    /// ceiling is refused rather than silently clamped, because a clamped value is one the
    /// caller did not choose and cannot detect.
    pub fn new(
        max_attempts: u32,
        base_backoff_ms: u64,
        max_backoff_ms: u64,
    ) -> Result<Self, RetryError> {
        if max_attempts == 0 || max_attempts > MAX_ATTEMPTS {
            return Err(RetryError::AttemptsOutOfRange);
        }
        if base_backoff_ms > max_backoff_ms || max_backoff_ms > MAX_BACKOFF_MS {
            return Err(RetryError::BackoffOutOfRange);
        }
        Ok(Self {
            max_attempts,
            base_backoff_ms,
            max_backoff_ms,
        })
    }

    /// Returns whether `attempt` may be followed by another.
    ///
    /// The comparison is `<` rather than `<=`, so with `max_attempts` of 2 the first
    /// attempt (`1`) may be followed and the second (`2`) may not. An off-by-one here
    /// would either retry once more than the caller allowed or never retry at all.
    #[must_use]
    pub const fn permits_attempt(&self, next_attempt: u32) -> bool {
        next_attempt <= self.max_attempts
    }

    /// Returns the delay before `attempt`, in milliseconds.
    ///
    /// Doubles per attempt from the base and saturates at the ceiling, so the growth is
    /// bounded even when the attempt count is at its maximum. Attempt 1 is the first
    /// attempt and is not preceded by a delay, so it returns zero.
    #[must_use]
    pub const fn backoff_ms(&self, attempt: u32) -> u64 {
        if attempt <= 1 {
            return 0;
        }
        // Doubling is done with saturation rather than `checked_shl`, because a shift of 32
        // or more is undefined for the count and `attempt` is caller-influenced. The cap on
        // the exponent is spelled as an `if` rather than `min`, because `min` is not
        // callable in a `const fn` on this toolchain.
        let exponent = attempt.saturating_sub(2);
        let doublings = if exponent > 31 { 31 } else { exponent };
        let scaled = self.base_backoff_ms.saturating_mul(1u64 << doublings);
        if scaled > self.max_backoff_ms {
            self.max_backoff_ms
        } else {
            scaled
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::none()
    }
}

/// Whether repeating a failure could change its outcome.
///
/// Supplied by the caller rather than derived here, and that split is deliberate: deciding
/// it needs a provider's error vocabulary, which lives at the adapter boundary — the
/// application layer's `ProviderError` owns it. The domain owns the *policy*; the layer that
/// speaks the provider's error codes owns the *classification*. A single enum here would
/// have to name every provider's failure modes, which is exactly the coupling this
/// workspace forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Repeating the same call unchanged could succeed — a timing or availability fact.
    Transient,
    /// Repeating it cannot succeed: a refusal, a rejected credential, a malformed request.
    Permanent,
}

impl FailureClass {
    /// Returns whether a repeat could change the outcome.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// When a failure happened relative to the provider accepting the call.
///
/// A required input to [`RetryDecision::decide`], not an inference: the same kind of
/// failure can arrive on either side of acceptance, so the failure alone cannot decide
/// whether the work was ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureSite {
    /// The provider never accepted the call, so nothing can have been produced, billed, or
    /// recorded. A retry here cannot repeat an effect.
    BeforeAcceptance,
    /// The provider accepted the call and something happened afterwards. The request is
    /// **ambiguous**: output may exist, may have been billed, and may have been recorded
    /// provider-side, so repeating it is only permitted once provider idempotency is
    /// documented and used.
    AfterAcceptance,
}

/// What the retry policy permits for a failure.
///
/// Three variants rather than a boolean, because "do not retry" and "wait, then retry"
/// and "a retry was allowed but cannot fit" are different facts an operator acts on
/// differently — the last one says the run's budget was the limiting factor, not the
/// policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Perform another attempt after `delay_ms`.
    Retry {
        /// The attempt number the retry will be.
        attempt: u32,
        /// How long to wait first.
        delay_ms: u64,
    },
    /// Do not retry, for the stated reason.
    GiveUp {
        /// Why the retry was refused.
        reason: RetryRefusal,
    },
}

impl RetryDecision {
    /// Returns whether a retry will be performed.
    #[must_use]
    pub const fn is_retry(&self) -> bool {
        matches!(self, Self::Retry { .. })
    }

    /// Decides whether `error` at `site` may be retried.
    ///
    /// The checks are in a fixed order so a failure that violates several rules always
    /// reports the same one, and the **safety** check comes first: a failure after
    /// acceptance is refused whatever the error says, because no other rule can make an
    /// ambiguous request safe.
    #[must_use]
    pub fn decide(
        policy: &RetryPolicy,
        class: FailureClass,
        site: FailureSite,
        completed_attempts: u32,
        budget: &RunBudget,
        now: UtcTimestamp,
    ) -> Self {
        // Safety first, and it is not overridable by retryability: the contract forbids
        // retrying an ambiguous accepted request.
        if site == FailureSite::AfterAcceptance {
            return Self::GiveUp {
                reason: RetryRefusal::AmbiguousAfterAcceptance,
            };
        }
        // Whether a repeat could change the outcome is the caller's answer, not this
        // module's: deciding it needs the provider's error vocabulary, which lives at the
        // adapter boundary. One layer owns each retry, and this module owns the *policy*
        // while the caller owns the classification.
        if class == FailureClass::Permanent {
            return Self::GiveUp {
                reason: RetryRefusal::NotRetryable,
            };
        }
        let next_attempt = completed_attempts.saturating_add(1);
        if !policy.permits_attempt(next_attempt) {
            return Self::GiveUp {
                reason: RetryRefusal::AttemptsExhausted,
            };
        }
        let delay_ms = policy.backoff_ms(next_attempt);
        // A retry that cannot begin before the deadline is a delay followed by the same
        // failure, so the run's remaining time would be spent learning nothing.
        if !budget.permits_step_at(now) {
            return Self::GiveUp {
                reason: RetryRefusal::DeadlineExceeded,
            };
        }
        if let BudgetStatus::Remaining { millis, .. } = budget.status_at(now)
            && millis <= delay_ms
        {
            return Self::GiveUp {
                reason: RetryRefusal::BackoffOutlivesDeadline,
            };
        }
        Self::Retry {
            attempt: next_attempt,
            delay_ms,
        }
    }
}

impl RetryDecision {
    /// Returns whether the refusal was caused by the run's budget rather than the policy.
    ///
    /// Exposed so a caller can say *which* limit stopped the run: "the provider kept
    /// failing" and "the run ran out of time" are different facts, and a retry refused by
    /// the budget means the run should be reported as timed out rather than as a provider
    /// fault.
    #[must_use]
    pub const fn refused_by_budget(&self) -> bool {
        matches!(
            self,
            Self::GiveUp {
                reason: RetryRefusal::DeadlineExceeded | RetryRefusal::BackoffOutlivesDeadline,
            }
        )
    }
}

/// Why a retry was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryRefusal {
    /// The provider had accepted the call, so the request is ambiguous.
    AmbiguousAfterAcceptance,
    /// The error is one a repeat cannot change.
    NotRetryable,
    /// The policy's attempt count is used up.
    AttemptsExhausted,
    /// The run's deadline has passed.
    DeadlineExceeded,
    /// The backoff would outlive the run's remaining time.
    BackoffOutlivesDeadline,
}

impl RetryRefusal {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::AmbiguousAfterAcceptance => "run.retry_ambiguous_request",
            Self::NotRetryable => "run.retry_not_retryable",
            Self::AttemptsExhausted => "run.retry_attempts_exhausted",
            Self::DeadlineExceeded => "run.deadline_exceeded",
            Self::BackoffOutlivesDeadline => "run.retry_backoff_outlives_deadline",
        }
    }
}

/// A retry policy that could not be interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryError {
    /// The attempt count was zero or over [`MAX_ATTEMPTS`].
    AttemptsOutOfRange,
    /// The backoff base exceeded the ceiling, or the ceiling was over [`MAX_BACKOFF_MS`].
    BackoffOutOfRange,
}

impl RetryError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::AttemptsOutOfRange => "run.retry_attempts_out_of_range",
            Self::BackoffOutOfRange => "run.retry_backoff_out_of_range",
        }
    }
}

impl std::fmt::Display for RetryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AttemptsOutOfRange => {
                write!(formatter, "attempts must be 1..={MAX_ATTEMPTS}")
            }
            Self::BackoffOutOfRange => write!(
                formatter,
                "the backoff base must not exceed the ceiling, and the ceiling must be at most {MAX_BACKOFF_MS} ms"
            ),
        }
    }
}

impl std::error::Error for RetryError {}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
