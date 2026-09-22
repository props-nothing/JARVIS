//! Context candidates, where they came from, and why they were offered.
//!
//! A candidate carries its own provenance — source, sensitivity, token cost, and
//! the reason it was offered — because the architecture's "Context Manifest"
//! section requires every included item to record "source, sensitivity, and token
//! estimate" plus "reason and score components for inclusion". Making those fields
//! part of the candidate means a caller cannot offer an item without saying where it
//! came from, which is what stops provenance from being reconstructed after the
//! fact.
//!
//! The [`CandidateSource`] set is **closed and ordered**, and that is deliberate.
//! The architecture states a "Context Source Priority" order, so the source is what
//! decides an item's priority band; a free-form source string would let an item
//! claim a band nothing can verify. The set also has **no variant for
//! model-internal reasoning**: `FR-RUN-005` forbids persisting or exposing hidden
//! chain-of-thought, and the surest way to honour that is to make it inexpressible
//! rather than to filter it later.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;

/// The most context candidates a caller may offer.
///
/// Re-exported from the budget module so the bound has one definition; the source
/// module names it because a candidate list is built here.
pub use crate::context::budget::MAX_CANDIDATES;

/// Where a context item came from.
///
/// Ordered exactly as the architecture's "Context Source Priority" list, so the
/// discriminant **is** the priority band and `Ord` is the ranking. A new variant
/// must be inserted where the architecture places it, not appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePriority {
    /// Immutable system safety and product policy.
    SystemPolicy,
    /// Authenticated workspace/user policy and the active task.
    WorkspacePolicyAndTask,
    /// Current conversation turns and pending state.
    Conversation,
    /// Relevant confirmed memories.
    Memory,
    /// Relevant documents, entities, and events.
    RetrievedContent,
    /// Tool observations and runtime-specific context.
    ToolObservation,
    /// Optional style preferences.
    StylePreference,
}

impl SourcePriority {
    /// Returns the priority band, lowest being filled first.
    ///
    /// Returned explicitly rather than relying on the enum's discriminant, so
    /// inserting a variant in the wrong place is a test failure rather than a silent
    /// ranking change.
    #[must_use]
    pub const fn band(self) -> u8 {
        match self {
            Self::SystemPolicy => 0,
            Self::WorkspacePolicyAndTask => 1,
            Self::Conversation => 2,
            Self::Memory => 3,
            Self::RetrievedContent => 4,
            Self::ToolObservation => 5,
            Self::StylePreference => 6,
        }
    }

    /// Returns whether content from this source is **untrusted**.
    ///
    /// Retrieved content, tool observations, and memories derived from them can
    /// carry an injection attempt. The architecture requires untrusted retrieved
    /// content to be "clearly delimited and never placed where a model could confuse
    /// it with system policy", which is only possible if the untrusted sources are
    /// named as such rather than treated like the others.
    #[must_use]
    pub const fn is_untrusted(self) -> bool {
        matches!(
            self,
            Self::RetrievedContent | Self::ToolObservation | Self::Memory
        )
    }
}

impl fmt::Display for SourcePriority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::SystemPolicy => "system_policy",
            Self::WorkspacePolicyAndTask => "workspace_policy_and_task",
            Self::Conversation => "conversation",
            Self::Memory => "memory",
            Self::RetrievedContent => "retrieved_content",
            Self::ToolObservation => "tool_observation",
            Self::StylePreference => "style_preference",
        };
        formatter.write_str(text)
    }
}

/// The concrete origin of a context item.
///
/// Finer-grained than [`SourcePriority`]: the band decides whether an item is filled
/// before another, while this names *what* it is, so a manifest can be read by an
/// operator without knowing the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    /// The immutable system policy block.
    SystemPolicy,
    /// Workspace and user policy, plus the active task statement.
    ActiveTask,
    /// A turn from the current conversation.
    Conversation,
    /// A confirmed durable memory.
    Memory,
    /// A retrieved document, entity, or event.
    RetrievedContent,
    /// An observation returned by a tool.
    ToolObservation,
    /// A style or formatting preference.
    StylePreference,
}

impl CandidateSource {
    /// Returns the priority band this source belongs to.
    #[must_use]
    pub const fn priority(self) -> SourcePriority {
        match self {
            Self::SystemPolicy => SourcePriority::SystemPolicy,
            Self::ActiveTask => SourcePriority::WorkspacePolicyAndTask,
            Self::Conversation => SourcePriority::Conversation,
            Self::Memory => SourcePriority::Memory,
            Self::RetrievedContent => SourcePriority::RetrievedContent,
            Self::ToolObservation => SourcePriority::ToolObservation,
            Self::StylePreference => SourcePriority::StylePreference,
        }
    }

    /// Returns whether content from this source is untrusted.
    #[must_use]
    pub const fn is_untrusted(self) -> bool {
        self.priority().is_untrusted()
    }
}

impl fmt::Display for CandidateSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::SystemPolicy => "system_policy",
            Self::ActiveTask => "active_task",
            Self::Conversation => "conversation",
            Self::Memory => "memory",
            Self::RetrievedContent => "retrieved_content",
            Self::ToolObservation => "tool_observation",
            Self::StylePreference => "style_preference",
        };
        formatter.write_str(text)
    }
}

/// Why a candidate was offered.
///
/// A closed set rather than free text, because the manifest's purpose is to explain
/// an inclusion to a person later, and a caller-invented reason string would make
/// "why was this sent" unanswerable while still looking like an explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InclusionReason {
    /// It is immutable policy and always present.
    MandatoryPolicy,
    /// It states what the run is currently trying to do.
    ActiveObjective,
    /// It is a recent turn in the current conversation.
    RecentTurn,
    /// It is a confirmed memory the task depends on.
    RelevantMemory,
    /// It was retrieved as relevant to the task.
    RetrievedForTask,
    /// It is an observation the run must reason over.
    ToolObservationRequired,
    /// It is a user-stated preference.
    UserPreference,
}

impl InclusionReason {
    /// Returns the contract-shaped name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MandatoryPolicy => "mandatory_policy",
            Self::ActiveObjective => "active_objective",
            Self::RecentTurn => "recent_turn",
            Self::RelevantMemory => "relevant_memory",
            Self::RetrievedForTask => "retrieved_for_task",
            Self::ToolObservationRequired => "tool_observation_required",
            Self::UserPreference => "user_preference",
        }
    }
}

impl fmt::Display for InclusionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One candidate offered to the budgeter.
///
/// The token cost is carried rather than computed here because counting tokens is a
/// model-specific job owned by the adapter; this layer only needs the number and the
/// guarantee that it is bounded.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextCandidate {
    /// A stable reference to the content.
    ///
    /// A reference, never the content itself: the architecture's manifest "enables
    /// diagnosis without storing duplicate prompt text", and access to the
    /// referenced content still follows current authorization and retention.
    pub reference: String,
    /// Where it came from.
    pub source: CandidateSource,
    /// Its sensitivity label.
    pub sensitivity: Sensitivity,
    /// Its token cost.
    pub tokens: u64,
    /// Why it was offered.
    pub reason: InclusionReason,
    /// The ranking score. Not required to be a probability; only its order matters.
    pub score: f64,
    /// When the content occurred, used as a recency tiebreak.
    pub occurred_at: UtcTimestamp,
    /// The last instant the content is valid, when it has one.
    pub valid_until: Option<UtcTimestamp>,
}

impl ContextCandidate {
    /// Returns the candidate's priority band.
    #[must_use]
    pub const fn priority(&self) -> SourcePriority {
        self.source.priority()
    }

    /// Returns whether this candidate's content is untrusted.
    #[must_use]
    pub const fn is_untrusted(&self) -> bool {
        self.source.is_untrusted()
    }

    /// Validates the candidate.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ContextCandidateInvalid`] when the reference is empty
    /// or over its bound, when `score` is not finite, or when a candidate claims a
    /// token cost of zero. A zero-cost item is refused rather than admitted, because
    /// it would let content enter the context without consuming any budget — the one
    /// way to bypass the bound this module exists to enforce.
    pub fn validated(self) -> Result<Self, DomainError> {
        if self.reference.is_empty()
            || self.reference.len() > MAX_REFERENCE_BYTES
            || self.reference.contains('\0')
        {
            return Err(DomainError::ContextCandidateInvalid);
        }
        if !self.score.is_finite() {
            return Err(DomainError::ContextCandidateInvalid);
        }
        if self.tokens == 0 {
            return Err(DomainError::ContextCandidateInvalid);
        }
        Ok(self)
    }
}

/// The longest accepted candidate reference.
pub const MAX_REFERENCE_BYTES: usize = 512;

#[cfg(test)]
mod tests {
    use super::{
        CandidateSource, ContextCandidate, InclusionReason, MAX_REFERENCE_BYTES, SourcePriority,
    };
    use crate::model::policy::Sensitivity;
    use crate::time::UtcTimestamp;

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn candidate(reference: &str) -> ContextCandidate {
        ContextCandidate {
            reference: reference.to_owned(),
            source: CandidateSource::Conversation,
            sensitivity: Sensitivity::Internal,
            tokens: 10,
            reason: InclusionReason::RecentTurn,
            score: 0.5,
            occurred_at: now(),
            valid_until: None,
        }
    }

    #[test]
    fn the_priority_order_matches_the_architecture_list() {
        // The architecture's "Context Source Priority" order, asserted explicitly so
        // inserting a variant in the wrong place fails here rather than silently
        // changing the ranking.
        let ordered = [
            SourcePriority::SystemPolicy,
            SourcePriority::WorkspacePolicyAndTask,
            SourcePriority::Conversation,
            SourcePriority::Memory,
            SourcePriority::RetrievedContent,
            SourcePriority::ToolObservation,
            SourcePriority::StylePreference,
        ];
        for pair in ordered.windows(2) {
            assert!(
                pair[0].band() < pair[1].band(),
                "{} must be filled before {}",
                pair[0],
                pair[1],
            );
        }
        // And the bands are dense from zero, so no gap can hide a variant.
        for (index, priority) in ordered.iter().enumerate() {
            assert_eq!(priority.band(), u8::try_from(index).expect("small"));
        }
    }

    #[test]
    fn every_source_maps_onto_a_band_and_agrees_about_trust() {
        for source in [
            CandidateSource::SystemPolicy,
            CandidateSource::ActiveTask,
            CandidateSource::Conversation,
            CandidateSource::Memory,
            CandidateSource::RetrievedContent,
            CandidateSource::ToolObservation,
            CandidateSource::StylePreference,
        ] {
            assert_eq!(source.is_untrusted(), source.priority().is_untrusted());
        }
        // Retrieved content, memory, and tool observations can carry an injection
        // attempt; policy, the active task, and a conversation turn cannot be said to
        // be untrusted in the same way.
        for untrusted in [
            CandidateSource::RetrievedContent,
            CandidateSource::ToolObservation,
            CandidateSource::Memory,
        ] {
            assert!(untrusted.is_untrusted(), "{untrusted} must be untrusted");
        }
        for trusted in [
            CandidateSource::SystemPolicy,
            CandidateSource::ActiveTask,
            CandidateSource::Conversation,
        ] {
            assert!(!trusted.is_untrusted(), "{trusted} must not be untrusted");
        }
    }

    #[test]
    fn a_candidate_reference_is_bounded_and_non_empty() {
        assert!(candidate("msg-1").validated().is_ok());
        assert!(candidate("").validated().is_err());
        assert!(candidate("a\0b").validated().is_err());
        let over = "a".repeat(MAX_REFERENCE_BYTES + 1);
        assert!(candidate(&over).validated().is_err());
        assert!(
            candidate(&"a".repeat(MAX_REFERENCE_BYTES))
                .validated()
                .is_ok(),
            "exactly the bound is inside it",
        );
    }

    #[test]
    fn a_zero_cost_candidate_is_refused_because_it_would_bypass_the_budget() {
        let mut free = candidate("free");
        free.tokens = 0;
        let error = free
            .validated()
            .expect_err("a zero-cost item must be refused");
        assert_eq!(error.code(), "jarvis.context_candidate_invalid");
    }

    #[test]
    fn a_non_finite_score_is_refused() {
        // A NaN score would make the ranking comparator's order undefined, so the
        // same candidate set could assemble differently on two runs.
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut scored = candidate("scored");
            scored.score = bad;
            assert!(scored.validated().is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn a_candidate_reports_its_band_and_trust_from_its_source() {
        let mut memory = candidate("mem-1");
        memory.source = CandidateSource::Memory;
        assert_eq!(memory.priority(), SourcePriority::Memory);
        assert!(memory.is_untrusted());

        let mut policy = candidate("policy-1");
        policy.source = CandidateSource::SystemPolicy;
        assert_eq!(policy.priority(), SourcePriority::SystemPolicy);
        assert!(!policy.is_untrusted());
    }
}
