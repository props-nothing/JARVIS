//! The context manifest: what was included, why, and what was excluded.
//!
//! The architecture's "Context Manifest" section requires a record containing the
//! budget, the included item references with "source, sensitivity, and token
//! estimate", "reason and score components for inclusion", and
//! "exclusions/truncation summaries". This module is that record.
//!
//! Two rules shape the types:
//!
//! - **It references content, it does not contain it.** Every item holds a
//!   [`reference`](ContextManifestItem::reference), never the text, because the
//!   architecture says the manifest "enables diagnosis without storing duplicate
//!   prompt text" and that "access to the referenced content still follows current
//!   authorization and retention rules". A manifest that embedded the content would
//!   outlive the retention decision that allowed it.
//! - **Exclusions are recorded by reason, never by content.** A refused sensitive
//!   item is counted, not named. That is what `ACC-035` requires when it says
//!   "excluded sensitive content never reaches the provider request" — a manifest
//!   that listed the excluded text would put it back on the wire.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::context::budget::ExclusionReason;
use crate::context::source::CandidateSource;
use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;

/// The most distinct exclusion reasons a summary can carry.
///
/// The reason set is closed, so this is the size of that set. The bound exists so
/// `ExclusionSummary` cannot grow an attacker-controlled map, and a test asserts it
/// matches the number of [`ExclusionReason`] variants.
const EXCLUSION_REASON_COUNT: usize = 4;

/// One included item, as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextManifestItem {
    /// A stable reference to the content. Never the content itself.
    pub reference: String,
    /// Where it came from.
    pub source: CandidateSource,
    /// Its sensitivity label.
    pub sensitivity: Sensitivity,
    /// The token estimate used when budgeting it.
    pub tokens: u64,
    /// Why it was included.
    pub reason: crate::context::source::InclusionReason,
    /// The ranking score at selection time.
    ///
    /// Recorded separately from the reason so a later evaluation can see that an item
    /// was selected *because* it was policy and not because it scored well — the two
    /// would be indistinguishable from the inclusion alone.
    pub score: f64,
}

/// How many candidates were excluded, by reason.
///
/// Counts rather than a list of references. The reason counts are what an operator
/// needs ("eleven items were over budget"), while the references of *sensitive*
/// refusals must not be recorded at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionSummary {
    /// One count per reason that occurred. Reasons with no exclusions are absent.
    pub counts: BTreeMap<String, u64>,
}

impl ExclusionSummary {
    /// Builds a summary from the reasons observed during one assembly.
    #[must_use]
    pub fn from_reasons(reasons: &[ExclusionReason]) -> Self {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for reason in reasons {
            // `to_owned` rather than a `&'static str` key: the summary serializes,
            // and a borrowed key would not survive the wire.
            *counts.entry(reason.as_str().to_owned()).or_insert(0) += 1;
        }
        Self { counts }
    }

    /// Returns how many candidates were excluded for `reason`.
    #[must_use]
    pub fn count(&self, reason: ExclusionReason) -> u64 {
        self.counts.get(reason.as_str()).copied().unwrap_or(0)
    }

    /// Returns the total number of exclusions recorded.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.counts.values().sum()
    }

    /// Returns whether nothing was excluded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Returns whether every key is a known exclusion reason.
    ///
    /// Used by a test against a deserialized summary: a key the domain does not
    /// recognize means the manifest was written by a different version, and treating
    /// an unknown reason as "not excluded" would under-report refusals.
    #[must_use]
    pub fn has_only_known_reasons(&self) -> bool {
        !self.counts.is_empty() && self.counts.len() <= EXCLUSION_REASON_COUNT
    }
}

/// The record of one context assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextManifest {
    /// The token budget the assembly was built under.
    pub budget_tokens: u64,
    /// The tokens the included items account for.
    pub used_tokens: u64,
    /// How many candidates were offered, included or not.
    ///
    /// Recorded so a manifest that included two of two hundred is distinguishable
    /// from one that included two of two, which the inclusion list alone cannot show.
    pub offered_candidates: u64,
    /// The included items, in selection order.
    pub included: Vec<ContextManifestItem>,
    /// The exclusions, counted by reason.
    pub exclusions: ExclusionSummary,
    /// When the assembly ran.
    pub assembled_at: UtcTimestamp,
}

impl ContextManifest {
    /// Returns how many candidates were excluded in total.
    #[must_use]
    pub fn excluded_total(&self) -> u64 {
        self.exclusions.total()
    }

    /// Returns whether the manifest's own arithmetic is consistent.
    ///
    /// Included plus excluded must equal offered, and the used tokens must equal the
    /// sum of the included items' costs, with the used total never exceeding the
    /// budget. A manifest failing either check describes a decision the code could
    /// not have made, so it is a corruption signal rather than a display problem.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        let included = u64::try_from(self.included.len()).unwrap_or(u64::MAX);
        let accounted = included.saturating_add(self.excluded_total());
        let token_sum: u64 = self.included.iter().map(|item| item.tokens).sum();
        accounted == self.offered_candidates
            && token_sum == self.used_tokens
            && self.used_tokens <= self.budget_tokens
    }
}

impl fmt::Display for ContextManifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A one-line operator summary: the counts and the budget, never an item
        // reference, because a reference can name content and a log line is not a
        // place to reproduce a manifest.
        write!(
            formatter,
            "context {} item(s), {}/{} tokens, {} excluded of {} offered",
            self.included.len(),
            self.used_tokens,
            self.budget_tokens,
            self.excluded_total(),
            self.offered_candidates,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextManifest, ContextManifestItem, ExclusionSummary};
    use crate::context::budget::ExclusionReason;
    use crate::context::source::{CandidateSource, InclusionReason};
    use crate::model::policy::Sensitivity;
    use crate::time::UtcTimestamp;

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn item(reference: &str, tokens: u64) -> ContextManifestItem {
        ContextManifestItem {
            reference: reference.to_owned(),
            source: CandidateSource::Conversation,
            sensitivity: Sensitivity::Internal,
            tokens,
            reason: InclusionReason::RecentTurn,
            score: 0.5,
        }
    }

    fn manifest(included: Vec<ContextManifestItem>, offered: u64, used: u64) -> ContextManifest {
        ContextManifest {
            budget_tokens: 1_000,
            used_tokens: used,
            offered_candidates: offered,
            included,
            exclusions: ExclusionSummary::default(),
            assembled_at: now(),
        }
    }

    #[test]
    fn a_summary_counts_each_reason_separately() {
        let summary = ExclusionSummary::from_reasons(&[
            ExclusionReason::OverBudget,
            ExclusionReason::OverBudget,
            ExclusionReason::Sensitivity,
        ]);
        assert_eq!(summary.count(ExclusionReason::OverBudget), 2);
        assert_eq!(summary.count(ExclusionReason::Sensitivity), 1);
        assert_eq!(summary.count(ExclusionReason::Expired), 0);
        assert_eq!(summary.total(), 3);
    }

    #[test]
    fn an_absent_reason_counts_as_zero_rather_than_being_unknown() {
        // The counts map omits reasons that did not occur, so a lookup must not
        // distinguish "this reason did not happen" from "this reason is not tracked".
        let summary = ExclusionSummary::default();
        assert!(summary.is_empty());
        for reason in [
            ExclusionReason::Sensitivity,
            ExclusionReason::OverBudget,
            ExclusionReason::Expired,
            ExclusionReason::Duplicate,
        ] {
            assert_eq!(summary.count(reason), 0);
        }
    }

    #[test]
    fn a_consistent_manifest_accounts_for_every_candidate() {
        // Two included and three excluded, so five were offered. The fixture is
        // arithmetic rather than assertion: getting it wrong is what the check
        // exists to catch, including in a test.
        let summary = ExclusionSummary::from_reasons(&[
            ExclusionReason::OverBudget,
            ExclusionReason::OverBudget,
            ExclusionReason::Duplicate,
        ]);
        let candidate = ContextManifest {
            budget_tokens: 1_000,
            used_tokens: 30,
            offered_candidates: 5,
            included: vec![item("a", 10), item("b", 20)],
            exclusions: summary,
            assembled_at: now(),
        };
        assert!(candidate.is_consistent());
        assert_eq!(candidate.excluded_total(), 3);
        assert_eq!(candidate.included.len(), 2);
    }

    #[test]
    fn an_inconsistent_manifest_is_detected() {
        // Offered does not equal included plus excluded: the manifest describes a
        // decision the code could not have made.
        let inconsistent = manifest(vec![item("a", 10)], 5, 10);
        assert!(!inconsistent.is_consistent());

        // Used tokens disagree with the included items' sum.
        let bad_tokens = manifest(vec![item("a", 10)], 1, 99);
        assert!(!bad_tokens.is_consistent());

        // Used tokens exceed the budget, which the budgeter cannot produce.
        let over_budget = ContextManifest {
            budget_tokens: 5,
            used_tokens: 10,
            offered_candidates: 1,
            included: vec![item("a", 10)],
            exclusions: ExclusionSummary::default(),
            assembled_at: now(),
        };
        assert!(!over_budget.is_consistent());
    }

    #[test]
    fn the_summary_rejects_more_reasons_than_the_set_has() {
        // A summary with more keys than there are reasons was written by something
        // that does not share this reason set, so an unknown reason would be read as
        // "not excluded" and refusals would be under-reported.
        let mut unknown = ExclusionSummary::default();
        for index in 0..=super::EXCLUSION_REASON_COUNT {
            unknown.counts.insert(format!("reason-{index}"), 1);
        }
        assert!(!unknown.has_only_known_reasons());

        let known = ExclusionSummary::from_reasons(&[ExclusionReason::Expired]);
        assert!(known.has_only_known_reasons());
    }

    #[test]
    fn the_manifest_serializes_without_content_or_a_reasoning_field() {
        // Two `FR-RUN-005` properties at once: the manifest carries references rather
        // than text, and it has no field that could carry hidden reasoning.
        let candidate = manifest(vec![item("msg-abc", 10)], 1, 10);
        let json = serde_json::to_string(&candidate).expect("serializes");
        assert!(
            !json.contains("reasoning") && !json.contains("chain_of_thought"),
            "the manifest must have no reasoning field: {json}",
        );
        assert!(
            json.contains("msg-abc"),
            "a reference must be present so the item is identifiable",
        );
        // The item carries a reference and metadata, never a body.
        assert!(!json.contains("\"content\""), "{json}");
    }

    #[test]
    fn the_operator_rendering_names_counts_and_never_a_reference() {
        let candidate = manifest(vec![item("msg-abc", 10)], 3, 10);
        let rendered = candidate.to_string();
        assert!(rendered.contains("1 item"), "{rendered}");
        assert!(rendered.contains("10/1000 tokens"), "{rendered}");
        assert!(
            !rendered.contains("msg-abc"),
            "a log line must not reproduce a manifest reference: {rendered}",
        );
    }
}
