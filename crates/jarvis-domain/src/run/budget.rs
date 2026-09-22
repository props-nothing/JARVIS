//! Per-run budgets, and the pure arithmetic that decides whether one is spent.
//!
//! `model-gateway.md` lists "deadline, cancellation, token, and cost budgets" as part
//! of a normalized model call, and `agent-runtime.md` lists "model/tool usage and
//! budget state" as part of the durable run record. This module is the typed form of
//! both, so the *decision* about whether a budget is exhausted is one function rather
//! than a comparison repeated at each call site.
//!
//! ## Why the arithmetic lives here
//!
//! Whether a deadline has passed is a pure question about two instants, and the answer
//! must not depend on which layer asks. Putting [`Budget::remaining`] in the domain means
//! the controller, a future workflow engine, and a future tool ledger all spend the same
//! budget the same way — and, more importantly, that "this run is out of time" is not
//! derived separately in each of them.
//!
//! ## What this module deliberately does not do
//!
//! It does not *enforce* anything. Enforcing a deadline means bounding a wait, which is
//! an async concern belonging to whoever performs the wait. The domain decides whether a
//! budget permits another step and how much time is left; the caller bounds its own
//! await. That split is why this type holds no clock and no timer.

use serde::{Deserialize, Serialize};

use crate::model::policy::{PolicyVersionRef, Sensitivity};
use crate::model::stream::{CallLimits, Usage};
use crate::run::retry::RetryPolicy;
use crate::time::UtcTimestamp;

/// The largest accepted step timeout, in milliseconds.
///
/// Bounded for the same reason every other bound here is: an unbounded timeout is an
/// unbounded wait, and this repository's rule is that every duration is bounded. One
/// hour is far above any single model call this milestone contemplates and still finite.
pub const MAX_STEP_TIMEOUT_MS: u64 = 3_600_000;

/// The default bound on a run's overall wall-clock time, in milliseconds.
///
/// Fifteen minutes. A run that has not finished in this long is not going to: the
/// controller performs one model turn, and a provider that has not answered within a
/// quarter of an hour is a hang rather than a slow model. This is a *default*, not a
/// ceiling — a caller may supply its own, and the field is optional.
pub const DEFAULT_RUN_DEADLINE_MS: u64 = 900_000;

/// The budget a run is executed under.
///
/// Every field is optional, because "no token cap" and "a cap of zero" are different
/// facts and conflating them would make an unset budget read as an exhausted one. The
/// same reasoning as [`Usage`](crate::model::stream::Usage), which keeps unset counters
/// absent rather than zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBudget {
    /// The wall-clock instant by which the run must finish.
    ///
    /// An absolute instant, not a duration: the run outlives any single process, and a
    /// duration would restart its own countdown on every read. `agent_runs.deadline_at`
    /// stores exactly this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<UtcTimestamp>,
    /// The longest a single step may take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_timeout_ms: Option<u64>,
    /// The maximum output tokens across the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// The maximum tokens the **assembled context** may occupy.
    ///
    /// The mirror of [`max_output_tokens`](Self::max_output_tokens) on the input side,
    /// and it exists because a transcript cannot be sent whole: the controller reads a
    /// bounded window of recent messages, but a window of 200 messages can still exceed
    /// any model's window, so before this the prompt was bounded in *message count* and
    /// unbounded in tokens. It reaches the domain's `ContextBudget`, which is what
    /// enforces the architecture's rule that policy filtering precedes ranking — a
    /// ceiling applied after selection could not refuse a candidate that was chosen.
    ///
    /// Optional for the same reason as every other field: "no context ceiling" and "a
    /// ceiling of zero" are different facts, and zero is not expressible because
    /// `ContextBudget` refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
    /// The maximum estimated cost across the run, in millionths of the billing unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_microunits: Option<u64>,
    /// When the run started, as an absolute instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<UtcTimestamp>,
    /// The policy version this run was created under, when one was in force.
    ///
    /// Recorded with the budget for the same reason the deadline is: the request that created
    /// the run may be long gone when someone asks *why* confidential content was held back or
    /// permitted. A run that recorded only the ceiling would leave an operator unable to explain
    /// the number, and one that recorded only the reference would make a manifest unreadable
    /// after the policy was archived. Both are stored, and the reference is what makes the
    /// decision replayable under the contract's historical-record rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyVersionRef>,
    /// The most sensitive content this run's context may carry.
    ///
    /// The merged policy's `maximum_sensitivity`, resolved from the stored policy at creation and
    /// carried here because the controller needs it while assembling the model input and must not
    /// re-read the policy per call: a policy edited mid-run would otherwise change the ceiling a
    /// running run is judged against, so two steps of one run could be held to different rules.
    ///
    /// Serialized as the contract's spelling in `budget_json`, so an operator reading a run's row
    /// sees the same word the API returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_sensitivity: Option<Sensitivity>,
    /// How a failed model call may be retried.
    ///
    /// Carried with the budget rather than configured on the controller, because a run
    /// should record the policy it ran under for the same reason it records its deadline:
    /// the request that created it may be long gone when someone asks why it made three
    /// attempts. It defaults to [`RetryPolicy::none`], so the absence of a policy is a
    /// recorded fact rather than an unstated default.
    #[serde(default)]
    pub retry: RetryPolicy,
}

impl RunBudget {
    /// Builds a budget that finishes by `deadline`, with no other limit.
    #[must_use]
    pub const fn with_deadline(deadline: UtcTimestamp) -> Self {
        Self {
            deadline: Some(deadline),
            step_timeout_ms: None,
            max_output_tokens: None,
            max_context_tokens: None,
            max_cost_microunits: None,
            started_at: None,
            policy: None,
            // `None`, not a permissive value. A run created with no policy in force is judged
            // against nothing, and the controller treats an absent ceiling as "record the label
            // but hold nothing back" — the honest state, rather than a default that would read
            // exactly like a real policy while constraining nothing.
            max_context_sensitivity: None,
            retry: RetryPolicy::none(),
        }
    }

    /// Returns this budget with the policy this run executes under.
    ///
    /// Both fields are set together rather than separately, because a recorded ceiling without
    /// the version that produced it cannot be explained and a version without its resolved
    /// ceiling forces a later reader to re-derive a decision from data that may since have been
    /// archived. The pair is one fact: "this run was held to *this* policy's ceiling".
    #[must_use]
    pub const fn with_policy(mut self, policy: PolicyVersionRef, ceiling: Sensitivity) -> Self {
        self.policy = Some(policy);
        self.max_context_sensitivity = Some(ceiling);
        self
    }

    /// Returns the sensitivity ceiling an assembly should apply.
    ///
    /// `None` means **no policy was in force**, which is different from a permissive ceiling: a
    /// caller must be able to tell "a policy said everything is permitted" from "nobody set a
    /// policy", because only the first is a decision an operator made. This distinction is why
    /// the field is an `Option` rather than defaulting to the most permissive variant.
    #[must_use]
    pub const fn context_sensitivity_ceiling(&self) -> Option<Sensitivity> {
        self.max_context_sensitivity
    }

    /// Returns this budget with a context-token ceiling applied.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::ContextTokensOutOfRange`] when `tokens` is zero or above
    /// [`crate::context::budget::MAX_CONTEXT_BUDGET_TOKENS`]. Zero is refused rather
    /// than read as "no context": the domain's `ContextBudget` refuses it too, and a
    /// budget that admitted a value the assembler rejects would fail at the far
    /// boundary instead of at the caller that set it.
    pub const fn with_context_tokens(mut self, tokens: u64) -> Result<Self, BudgetError> {
        if tokens == 0 || tokens > crate::context::budget::MAX_CONTEXT_BUDGET_TOKENS {
            return Err(BudgetError::ContextTokensOutOfRange);
        }
        self.max_context_tokens = Some(tokens);
        Ok(self)
    }

    /// Builds a budget that expires `after_ms` milliseconds after `start`.
    ///
    /// The instant is computed here rather than by each caller, because "when does this
    /// run expire" must have one answer: a caller that added the milliseconds itself
    /// could overflow, or round, differently from another.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::StepTimeoutOutOfRange`] when `after_ms` is zero or over
    /// [`MAX_STEP_TIMEOUT_MS`], and [`BudgetError::Malformed`] when adding it to `start`
    /// would leave the representable range.
    pub fn expiring_after(start: UtcTimestamp, after_ms: u64) -> Result<Self, BudgetError> {
        if after_ms == 0 || after_ms > MAX_STEP_TIMEOUT_MS {
            return Err(BudgetError::StepTimeoutOutOfRange);
        }
        let millis = i64::try_from(after_ms).map_err(|_| BudgetError::Malformed)?;
        let deadline = start
            .as_timestamp()
            .checked_add(jiff::Span::new().milliseconds(millis))
            .map_err(|_| BudgetError::Malformed)?;
        Ok(Self::with_deadline(UtcTimestamp::from_timestamp(deadline)))
    }

    /// Returns this budget with a retry policy applied.
    #[must_use]
    pub const fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Returns this budget with a single-step timeout applied.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::StepTimeoutOutOfRange`] when `millis` is zero or above
    /// [`MAX_STEP_TIMEOUT_MS`]. Zero is refused rather than treated as "no bound": a
    /// zero timeout would fail every step immediately, which reads to an operator as a
    /// broken provider rather than as a misconfigured budget.
    pub const fn with_step_timeout(mut self, millis: u64) -> Result<Self, BudgetError> {
        if millis == 0 || millis > MAX_STEP_TIMEOUT_MS {
            return Err(BudgetError::StepTimeoutOutOfRange);
        }
        self.step_timeout_ms = Some(millis);
        Ok(self)
    }

    /// Returns which ceiling `usage` breached, if any.
    ///
    /// This is what makes a token or cost ceiling a bound rather than a number carried in
    /// a request. Until this existed, `max_output_tokens` reached the provider and nothing
    /// compared what came back against it.
    ///
    /// The comparison is strictly **greater than**, so a call that used exactly its
    /// ceiling is inside the budget. That is the opposite boundary convention from
    /// [`status_at`](Self::status_at), where the deadline instant itself is expired, and
    /// the two are consistent for the same reason: the boundary belongs to the side that
    /// cannot do more work. A ceiling of 2048 permits producing token 2048, while a
    /// deadline of `T` does not permit work at `T` because that work would finish after
    /// `T`.
    ///
    /// An unreported counter cannot breach a ceiling. That is not permissiveness for its
    /// own sake: refusing on an absent value would fail every run against a provider that
    /// does not report usage, which is a false failure rather than a safety property.
    /// [`budget_is_verifiable`](Self::budget_is_verifiable) states the gap instead of
    /// hiding it.
    #[must_use]
    pub fn exceeded_by(&self, usage: &Usage) -> Option<BudgetLimit> {
        // Checked in a fixed order so two simultaneous breaches always report the same
        // one, and the output ceiling first because it is the one a run controls
        // directly.
        if let (Some(cap), Some(actual)) = (self.max_output_tokens, usage.output_tokens)
            && actual > cap
        {
            return Some(BudgetLimit::OutputTokens);
        }
        if let (Some(cap), Some(actual)) =
            (self.max_cost_microunits, usage.estimated_cost_microunits)
            && actual > cap
        {
            return Some(BudgetLimit::Cost);
        }
        None
    }

    /// Returns whether a token or cost ceiling is set **and** the usage to judge it by was
    /// reported.
    ///
    /// Exposed so a caller can record that a ceiling went unverified rather than reporting
    /// a silently unchecked budget as an enforced one. A ceiling with no measurement is the
    /// state most likely to be mistaken for enforcement, because nothing observable
    /// distinguishes it from a ceiling that was met.
    #[must_use]
    pub fn budget_is_verifiable(&self, usage: &Usage) -> bool {
        let tokens_verifiable = self.max_output_tokens.is_none() || usage.output_tokens.is_some();
        let cost_verifiable =
            self.max_cost_microunits.is_none() || usage.estimated_cost_microunits.is_some();
        tokens_verifiable && cost_verifiable
    }

    /// Returns whether any token or cost ceiling is set.
    #[must_use]
    pub const fn has_consumption_ceiling(&self) -> bool {
        self.max_output_tokens.is_some() || self.max_cost_microunits.is_some()
    }

    /// Returns the time left before the deadline, or `None` when no deadline is set.
    ///
    /// A deadline that has already passed yields [`BudgetStatus::Expired`], and one that
    /// is exactly *now* also yields `Expired`: a budget that has no time left cannot
    /// fund another step, and treating the boundary instant as still-valid would make
    /// the answer depend on sub-nanosecond timing.
    #[must_use]
    pub fn status_at(&self, now: UtcTimestamp) -> BudgetStatus {
        let Some(deadline) = self.deadline else {
            return BudgetStatus::Unbounded;
        };
        let remaining = deadline.as_timestamp().duration_since(now.as_timestamp());
        // A negative span is spelled `is_negative`; a zero span means the deadline is
        // this instant, which has no time left to spend.
        if remaining.is_negative() || remaining.is_zero() {
            BudgetStatus::Expired { deadline }
        } else {
            // A saturating conversion. It cannot actually saturate: `jiff`'s timestamp
            // range caps the widest possible span at ~2.5e14 ms, which fits `u64` with
            // room to spare, and the test below asserts that bound rather than assuming
            // it. Written as a conversion rather than a cast so no lossy `as` appears on
            // a value derived from stored data, and so the failure direction — if the
            // range ever widened — would be "report a huge budget" rather than "report
            // no time left", which is the safer of the two.
            BudgetStatus::Remaining {
                deadline,
                millis: u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX),
            }
        }
    }

    /// Returns whether the budget permits work to begin at `now`.
    #[must_use]
    pub fn permits_step_at(&self, now: UtcTimestamp) -> bool {
        !matches!(self.status_at(now), BudgetStatus::Expired { .. })
    }

    /// Returns the per-call limits this budget imposes on one model call.
    ///
    /// The run's deadline is carried onto the call, so an adapter that honours the
    /// contract's `limits.deadline` bounds its own request. Sending no deadline let a
    /// provider wait indefinitely, which is the defect this method exists to close.
    #[must_use]
    pub const fn call_limits(&self) -> CallLimits {
        CallLimits {
            deadline: self.deadline,
            max_output_tokens: self.max_output_tokens,
            max_cost_microunits: self.max_cost_microunits,
        }
    }
}

/// What a budget permits at a given instant.
///
/// A three-way answer rather than a boolean, because "there is no deadline" and "the
/// deadline is far away" are different facts, and a caller that reports an operator
/// message needs to say which applies. A boolean would make an unbounded run and a
/// recently-renewed one indistinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetStatus {
    /// No deadline is set, so no time limit applies.
    Unbounded,
    /// Time remains before the deadline.
    Remaining {
        /// The deadline being counted toward.
        deadline: UtcTimestamp,
        /// Whole milliseconds left, saturating rather than wrapping on overflow.
        millis: u64,
    },
    /// The deadline has passed.
    Expired {
        /// The deadline that passed.
        deadline: UtcTimestamp,
    },
}

impl BudgetStatus {
    /// Returns whether time remains, including when unbounded.
    #[must_use]
    pub const fn is_permitted(&self) -> bool {
        !matches!(self, Self::Expired { .. })
    }
}

/// A consumption ceiling a run breached.
///
/// Named rather than reported as a boolean, because the two ceilings have different
/// remedies: an output-token breach means the model was asked for too much, while a cost
/// breach means the route was too expensive for the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetLimit {
    /// Output tokens exceeded [`RunBudget::max_output_tokens`].
    OutputTokens,
    /// Estimated cost exceeded [`RunBudget::max_cost_microunits`].
    Cost,
}

impl BudgetLimit {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::OutputTokens => "run.budget_output_tokens_exceeded",
            Self::Cost => "run.budget_cost_exceeded",
        }
    }
}

impl std::fmt::Display for BudgetLimit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutputTokens => formatter.write_str("the run exceeded its output-token budget"),
            Self::Cost => formatter.write_str("the run exceeded its cost budget"),
        }
    }
}

/// A budget that could not be interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetError {
    /// A step timeout was zero or above [`MAX_STEP_TIMEOUT_MS`].
    StepTimeoutOutOfRange,
    /// A context ceiling was zero or above
    /// [`crate::context::budget::MAX_CONTEXT_BUDGET_TOKENS`].
    ContextTokensOutOfRange,
    /// A stored budget could not be read back.
    Malformed,
}

impl BudgetError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StepTimeoutOutOfRange => "run.budget_step_timeout_out_of_range",
            Self::ContextTokensOutOfRange => "run.budget_context_tokens_out_of_range",
            Self::Malformed => "run.budget_malformed",
        }
    }
}

impl std::fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StepTimeoutOutOfRange => {
                write!(
                    formatter,
                    "a step timeout must be 1..={MAX_STEP_TIMEOUT_MS} ms"
                )
            }
            Self::ContextTokensOutOfRange => write!(
                formatter,
                "a context ceiling must be 1..={} tokens",
                crate::context::budget::MAX_CONTEXT_BUDGET_TOKENS,
            ),
            Self::Malformed => formatter.write_str("the stored run budget could not be read"),
        }
    }
}

impl std::error::Error for BudgetError {}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
