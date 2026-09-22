//! The token budget and the assembly pipeline.
//!
//! The order of operations here is the architecture's pipeline, and the order is
//! the point: scope and policy filtering happen **before** ranking, because the
//! architecture is explicit that post-filtering a ranked result leaks both recall
//! and side channels — a caller can infer that something was filtered from the
//! shape of what came back.

use std::collections::BTreeSet;

use crate::context::manifest::{ContextManifest, ContextManifestItem, ExclusionSummary};
use crate::context::source::{CandidateSource, ContextCandidate};
use crate::error::DomainError;
use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;

/// The largest accepted context budget.
///
/// A bound exists because the budget multiplies against a per-item cost and reaches
/// a provider request; an unbounded one would let a caller ask for a context larger
/// than any model accepts, which then fails at the far boundary instead of here.
pub const MAX_CONTEXT_BUDGET_TOKENS: u64 = 10_000_000;

/// The largest accepted number of candidates offered in one assembly.
///
/// Bounded so a hostile or buggy caller cannot make the ranker process an unbounded
/// list, which is the same reason the storage architecture bounds every collection
/// that crosses a boundary.
pub const MAX_CANDIDATES: usize = 10_000;

/// The token budget for one context assembly.
///
/// Validated on construction, so an empty or oversized budget is unrepresentable
/// rather than merely invalid at the point it is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextBudget {
    tokens: u64,
}

impl ContextBudget {
    /// Builds a budget of `tokens`.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ContextBudgetInvalid`] when `tokens` is zero or above
    /// [`MAX_CONTEXT_BUDGET_TOKENS`]. Zero is refused rather than treated as "no
    /// context", because an empty budget and a deliberately minimal one are
    /// different intentions and only the caller knows which was meant.
    pub const fn new(tokens: u64) -> Result<Self, DomainError> {
        if tokens == 0 || tokens > MAX_CONTEXT_BUDGET_TOKENS {
            return Err(DomainError::ContextBudgetInvalid);
        }
        Ok(Self { tokens })
    }

    /// Returns the total token budget.
    #[must_use]
    pub const fn tokens(self) -> u64 {
        self.tokens
    }

    /// Returns the budget remaining after `used` tokens.
    ///
    /// Saturating rather than wrapping: a caller that over-counted would otherwise
    /// see a huge remaining budget, which is the direction of error that sends
    /// everything.
    #[must_use]
    pub const fn remaining_after(self, used: u64) -> u64 {
        self.tokens.saturating_sub(used)
    }

    /// Assembles a context from `candidates` under this budget.
    ///
    /// The steps are the architecture's pipeline, and the order is load-bearing:
    ///
    /// 1. **Filter by scope and policy first** — a candidate whose sensitivity
    ///    exceeds `ceiling`, or whose validity has passed, is excluded *before*
    ///    ranking. Post-filtering a ranked result would leak recall through the shape
    ///    of the answer.
    /// 2. **Rank deterministically**, then keep the first occurrence of each
    ///    reference — ranking first means the *most privileged* duplicate is kept
    ///    rather than whichever happened to come first in the caller's list.
    /// 3. **Fill within the budget**, excluding an item that does not fit rather
    ///    than truncating it.
    /// 4. **Record the manifest**, including every exclusion with its reason, so
    ///    "the context was small" and "most candidate content was refused" are
    ///    distinguishable.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ContextCandidatesUnbounded`] when more than
    /// [`MAX_CANDIDATES`] are offered.
    pub fn assemble(
        self,
        candidates: Vec<ContextCandidate>,
        ceiling: Sensitivity,
        now: UtcTimestamp,
    ) -> Result<AssembledContext, DomainError> {
        if candidates.len() > MAX_CANDIDATES {
            return Err(DomainError::ContextCandidatesUnbounded);
        }

        let offered = candidates.len();
        let mut excluded: Vec<ExclusionReason> = Vec::new();

        // Step 1: scope and policy filter, before anything is ranked.
        let mut eligible = Vec::new();
        for candidate in candidates {
            if candidate.sensitivity > ceiling {
                excluded.push(ExclusionReason::Sensitivity);
                continue;
            }
            if candidate.valid_until.is_some_and(|validity| validity < now) {
                excluded.push(ExclusionReason::Expired);
                continue;
            }
            eligible.push(candidate);
        }

        // Step 2: rank by priority band, then score, then recency, then reference.
        // Every tiebreak is present so the order is total and the outcome does not
        // depend on the order the caller happened to supply.
        eligible.sort_by(|left, right| {
            left.priority()
                .cmp(&right.priority())
                .then_with(|| right.score.total_cmp(&left.score))
                .then_with(|| right.occurred_at.cmp(&left.occurred_at))
                .then_with(|| left.reference.cmp(&right.reference))
        });
        // Deduplicate by reference, keeping the first (highest-ranked) occurrence.
        // The set owns its keys rather than borrowing them: the candidates are moved
        // into `deduplicated` below, so a borrowed key could not outlive its owner.
        // The clone is bounded per reference and the candidate list is bounded above,
        // so this cannot grow without limit.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut deduplicated: Vec<ContextCandidate> = Vec::new();
        for candidate in eligible {
            if seen.insert(candidate.reference.clone()) {
                deduplicated.push(candidate);
            } else {
                excluded.push(ExclusionReason::Duplicate);
            }
        }

        // Step 3: fill within the budget, in the ranked order.
        let mut included = Vec::new();
        let mut items = Vec::new();
        let mut used = 0_u64;
        for candidate in deduplicated {
            if candidate.tokens > self.remaining_after(used) {
                // Excluded rather than truncated: a half-truncated message reads as
                // a complete one to the model, which is worse than omitting it.
                excluded.push(ExclusionReason::OverBudget);
                continue;
            }
            used += candidate.tokens;
            items.push(ContextManifestItem {
                reference: candidate.reference.clone(),
                source: candidate.source,
                sensitivity: candidate.sensitivity,
                tokens: candidate.tokens,
                reason: candidate.reason,
                score: candidate.score,
            });
            included.push(IncludedItem {
                reference: candidate.reference,
                source: candidate.source,
                sensitivity: candidate.sensitivity,
                tokens: candidate.tokens,
            });
        }

        // Step 4: the manifest is built from what actually happened, not from what
        // was intended, so it cannot describe a decision the code did not make.
        let manifest = ContextManifest {
            budget_tokens: self.tokens,
            used_tokens: used,
            offered_candidates: u64::try_from(offered).unwrap_or(u64::MAX),
            included: items,
            exclusions: ExclusionSummary::from_reasons(&excluded),
            assembled_at: now,
        };
        Ok(AssembledContext {
            included,
            manifest,
            used_tokens: used,
        })
    }
}

/// One assembled context: the included items and the manifest describing the
/// decision.
///
/// `PartialEq` but not `Eq`: an item carries a floating-point score, and a float
/// makes equality non-reflexive (`NaN != NaN`). Claiming `Eq` would let a caller
/// put these in a `BTreeSet` and get an inconsistent collection, so the weaker
/// bound is the honest one.
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledContext {
    /// The included candidates, in the order they were selected.
    pub included: Vec<IncludedItem>,
    /// The manifest recording what was included, why, and what was excluded.
    pub manifest: ContextManifest,
    /// The tokens the included items account for.
    pub used_tokens: u64,
}

impl AssembledContext {
    /// Returns whether the assembly used its whole budget.
    ///
    /// Reported rather than left to a caller comparing two numbers, because "the
    /// budget is exhausted" is the signal that a later item was dropped for budget
    /// rather than for policy.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.used_tokens >= self.manifest.budget_tokens
    }

    /// Returns how many candidates were refused for exceeding the sensitivity
    /// ceiling.
    ///
    /// A count, not the content: `ACC-035` requires that "excluded sensitive content
    /// never reaches the provider request", so the manifest records that something
    /// was refused without naming it.
    #[must_use]
    pub fn refused_for_sensitivity(&self) -> u64 {
        self.manifest.exclusions.count(ExclusionReason::Sensitivity)
    }
}

/// One item that made it into the context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludedItem {
    /// The candidate's stable reference.
    pub reference: String,
    /// Where it came from, which decides its priority band.
    pub source: CandidateSource,
    /// Its sensitivity label.
    pub sensitivity: Sensitivity,
    /// The tokens it costs.
    pub tokens: u64,
}

/// Why a candidate was not included.
///
/// The reason matters because the operator's next question after "the context was
/// small" is *which* rule emptied it, and "excluded" alone does not distinguish an
/// over-budget item from a policy-refused one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExclusionReason {
    /// Its sensitivity exceeded the call's ceiling.
    Sensitivity,
    /// It did not fit the remaining budget.
    OverBudget,
    /// Its validity had passed.
    Expired,
    /// It duplicated a reference already included.
    Duplicate,
}

impl ExclusionReason {
    /// Returns the contract-shaped name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sensitivity => "sensitivity",
            Self::OverBudget => "over_budget",
            Self::Expired => "expired",
            Self::Duplicate => "duplicate",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextBudget, ExclusionReason, MAX_CANDIDATES, MAX_CONTEXT_BUDGET_TOKENS};
    use crate::context::source::{CandidateSource, ContextCandidate, InclusionReason};
    use crate::model::policy::Sensitivity;
    use crate::time::UtcTimestamp;

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn candidate(reference: &str, tokens: u64, sensitivity: Sensitivity) -> ContextCandidate {
        ContextCandidate {
            reference: reference.to_owned(),
            source: CandidateSource::Conversation,
            sensitivity,
            tokens,
            reason: InclusionReason::RecentTurn,
            score: 0.5,
            occurred_at: now(),
            valid_until: None,
        }
    }

    #[test]
    fn a_budget_is_bounded_and_non_zero() {
        assert!(ContextBudget::new(1).is_ok());
        assert!(ContextBudget::new(MAX_CONTEXT_BUDGET_TOKENS).is_ok());
        for bad in [0, MAX_CONTEXT_BUDGET_TOKENS + 1] {
            let error = ContextBudget::new(bad).expect_err("must be refused");
            assert_eq!(error.code(), "jarvis.context_budget_invalid");
        }
    }

    #[test]
    fn remaining_saturates_rather_than_wrapping() {
        // Wrapping would report a huge remaining budget after an over-count, which
        // is the direction of error that sends everything.
        let budget = ContextBudget::new(100).expect("valid");
        assert_eq!(budget.remaining_after(40), 60);
        assert_eq!(budget.remaining_after(100), 0);
        assert_eq!(budget.remaining_after(1_000), 0);
    }

    #[test]
    fn an_empty_candidate_set_assembles_an_empty_context() {
        let budget = ContextBudget::new(1_000).expect("valid");
        let assembled = budget
            .assemble(Vec::new(), Sensitivity::Internal, now())
            .expect("an empty set is valid");
        assert!(assembled.included.is_empty());
        assert_eq!(assembled.used_tokens, 0);
        assert_eq!(assembled.manifest.offered_candidates, 0);
    }

    #[test]
    fn an_over_budget_candidate_is_excluded_not_truncated() {
        // Truncation is the wrong answer: a half message reads as a complete one.
        let budget = ContextBudget::new(100).expect("valid");
        let assembled = budget
            .assemble(
                vec![
                    candidate("a", 60, Sensitivity::Internal),
                    candidate("b", 60, Sensitivity::Internal),
                ],
                Sensitivity::Internal,
                now(),
            )
            .expect("valid");
        assert_eq!(assembled.included.len(), 1, "only one item fits");
        assert_eq!(assembled.included[0].reference, "a");
        assert_eq!(assembled.used_tokens, 60);
        assert_eq!(
            assembled
                .manifest
                .exclusions
                .count(ExclusionReason::OverBudget),
            1,
        );
    }

    #[test]
    fn a_sensitive_candidate_is_excluded_before_ranking() {
        // The ordering the architecture requires: policy filtering precedes ranking,
        // so a refused candidate is never scored. The count is recorded so absence is
        // attributable to a rule rather than left as a smaller result.
        let budget = ContextBudget::new(1_000).expect("valid");
        let assembled = budget
            .assemble(
                vec![
                    candidate("secret", 10, Sensitivity::Restricted),
                    candidate("ok", 10, Sensitivity::Internal),
                ],
                Sensitivity::Internal,
                now(),
            )
            .expect("valid");
        assert_eq!(assembled.included.len(), 1);
        assert_eq!(assembled.included[0].reference, "ok");
        assert_eq!(assembled.refused_for_sensitivity(), 1);
        assert!(
            !assembled
                .manifest
                .included
                .iter()
                .any(|item| item.reference == "secret"),
            "a restricted item must not appear in the manifest's inclusions",
        );
    }

    #[test]
    fn a_duplicate_reference_is_kept_once_at_its_best_occurrence() {
        // The lower-scoring copy is offered first, so an implementation that kept
        // "the first one seen" would keep the worse one.
        let budget = ContextBudget::new(1_000).expect("valid");
        let mut low = candidate("same", 10, Sensitivity::Internal);
        low.score = 0.1;
        let mut high = candidate("same", 10, Sensitivity::Internal);
        high.score = 0.9;
        let assembled = budget
            .assemble(vec![low, high], Sensitivity::Internal, now())
            .expect("valid");
        assert_eq!(assembled.included.len(), 1);
        // Compared as an ordering rather than for exact equality: what the test means
        // is that the better-scoring copy survived, and an exact float comparison
        // would be asserting the arithmetic of the fixture rather than the rule.
        assert!(
            assembled.manifest.included[0].score > 0.5,
            "the higher-scoring copy must be the one kept: {}",
            assembled.manifest.included[0].score,
        );
        assert_eq!(
            assembled
                .manifest
                .exclusions
                .count(ExclusionReason::Duplicate),
            1,
        );
    }

    #[test]
    fn an_expired_candidate_is_excluded_and_recorded() {
        let budget = ContextBudget::new(1_000).expect("valid");
        let mut expired = candidate("old", 10, Sensitivity::Internal);
        expired.valid_until = Some(UtcTimestamp::parse("2026-09-21T00:00:00Z").expect("valid"));
        let assembled = budget
            .assemble(vec![expired], Sensitivity::Internal, now())
            .expect("valid");
        assert!(assembled.included.is_empty());
        assert_eq!(
            assembled
                .manifest
                .exclusions
                .count(ExclusionReason::Expired),
            1,
        );
    }

    #[test]
    fn the_candidate_bound_is_enforced() {
        // The bound is checked from the length, before any per-candidate work, so an
        // over-large set is refused rather than processed and then rejected.
        assert_eq!(MAX_CANDIDATES, 10_000);
        let budget = ContextBudget::new(1_000).expect("valid");
        let many = (0..=MAX_CANDIDATES)
            .map(|index| candidate(&format!("c{index}"), 1, Sensitivity::Internal))
            .collect();
        let error = budget
            .assemble(many, Sensitivity::Internal, now())
            .expect_err("an over-large set must be refused");
        assert_eq!(error.code(), "jarvis.context_candidates_unbounded");
    }

    #[test]
    fn a_full_budget_is_reported_rather_than_left_to_the_caller_to_compare() {
        let budget = ContextBudget::new(100).expect("valid");
        let assembled = budget
            .assemble(
                vec![candidate("a", 100, Sensitivity::Internal)],
                Sensitivity::Internal,
                now(),
            )
            .expect("valid");
        assert!(assembled.is_full());
        assert_eq!(assembled.used_tokens, 100);
    }
}
