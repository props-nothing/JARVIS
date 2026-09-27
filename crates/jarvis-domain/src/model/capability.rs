//! Model capability descriptors and their evidence.
//!
//! A descriptor records what a model has been *verified* to support, with the
//! evidence that says so. Two rules from the architecture are enforced here
//! rather than documented:
//!
//! 1. **Provider marketing names are not capability evidence.** Every capability
//!    value carries an [`EvidenceLabel`]; a value labelled `UNVERIFIED` cannot
//!    satisfy a hard requirement, so a claim without evidence cannot be routed to.
//! 2. **Incremental delivery is a measurement, not a flag.** A model can advertise
//!    streaming and still deliver its whole reply in one burst, which turns a
//!    streaming pipeline into a silent no-op. [`IncrementalDelivery`] therefore
//!    stores a measured time-to-first-token **and** a token spread, and
//!    [`CapabilityDescriptor::incremental_delivery`] refuses to answer from a
//!    boolean.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::model::identity::EndpointClass;
use crate::time::{IsoDate, UtcTimestamp};

/// How strongly a capability value is supported by evidence.
///
/// The order is the trust order, so `>=` is a usable comparison. `InfErred` and
/// `Unverified` are deliberately distinct: an inference is a considered guess
/// from a related fact, while `UNVERIFIED` means nothing supports the claim. A
/// hard routing requirement must not be satisfied by either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLabel {
    /// No evidence supports the value.
    Unverified,
    /// Derived from a related verified fact, not itself observed.
    Inferred,
    /// Observed against a test account or captured fixture.
    Observed,
    /// Stated by official provider documentation.
    Documented,
    /// Documented **and** confirmed by observation for the pinned version.
    Verified,
}

impl EvidenceLabel {
    /// Returns whether the label can satisfy a hard routing requirement.
    ///
    /// Only `VERIFIED` may. A hard requirement is a promise the gateway makes to
    /// the policy layer, so `DOCUMENTED` is not enough: documentation describes a
    /// provider's current version, and this repository may pin an older one.
    #[must_use]
    pub const fn satisfies_hard_requirement(self) -> bool {
        matches!(self, Self::Verified)
    }

    /// Returns whether the label can satisfy a **model data policy** rule.
    ///
    /// Deliberately weaker than [`satisfies_hard_requirement`](Self::satisfies_hard_requirement),
    /// because `model-data-policy.md` sets a different bar for a different kind of claim. It
    /// names the labels that **cannot** satisfy a "hard retention/training/residency rule" —
    /// "expired, `STALE`, `INFERRED`, or `UNVERIFIED`" — which admits `DOCUMENTED` and
    /// `OBSERVED` by omission. That is coherent rather than lax: whether a provider documents
    /// bounded retention is a statement about its *current published terms*, and the official
    /// documentation is exactly the source that establishes it, while a capability like
    /// "supports tool calling" can differ between the documented version and the pinned one.
    ///
    /// Both predicates exist because using one for both would fail in one direction whichever
    /// was chosen: the capability rule applied to retention would refuse a provider whose terms
    /// are documented but not independently reproduced, and this rule applied to capabilities
    /// would route to a version that does not have the feature.
    #[must_use]
    pub const fn satisfies_data_policy_rule(self) -> bool {
        matches!(self, Self::Verified | Self::Documented | Self::Observed)
    }

    /// Returns the uppercase spelling used in contracts and operator output.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Unverified => "UNVERIFIED",
            Self::Inferred => "INFERRED",
            Self::Observed => "OBSERVED",
            Self::Documented => "DOCUMENTED",
            Self::Verified => "VERIFIED",
        }
    }
}

impl fmt::Display for EvidenceLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// Where a capability value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// The evidence note identifier in `docs/research/evidence-manifest.json`.
    pub integration_id: String,
    /// The capability key this evidence supports.
    pub capability_key: String,
    /// The evidence strength.
    pub label: EvidenceLabel,
    /// The official source this was read from.
    pub source_url: String,
    /// The day the value was verified.
    pub last_verified: IsoDate,
    /// The last day the value may be used without revalidation.
    pub revalidate_by: IsoDate,
}

impl Evidence {
    /// Returns whether this evidence is still valid on `today`.
    ///
    /// A `revalidate_by` day is inclusive, because it names the last valid day.
    #[must_use]
    pub fn is_fresh_on(&self, today: IsoDate) -> bool {
        self.revalidate_by.is_no_earlier_than(today)
    }

    /// Returns whether this evidence can satisfy a hard requirement on `today`.
    ///
    /// Expired, `STALE`, `INFERRED`, and `UNVERIFIED` evidence cannot. This is
    /// one predicate rather than two checks at each call site, because a caller
    /// that remembered the label but forgot the date would route to a value whose
    /// evidence has expired — which reads as success.
    #[must_use]
    pub fn satisfies_hard_requirement_on(&self, today: IsoDate) -> bool {
        self.label.satisfies_hard_requirement() && self.is_fresh_on(today)
    }

    /// Returns whether this evidence can satisfy a **model data policy** rule on `today`.
    ///
    /// The same shape as
    /// [`satisfies_hard_requirement_on`](Self::satisfies_hard_requirement_on) over the weaker
    /// label set `model-data-policy.md` fixes for retention, training, and residency claims.
    #[must_use]
    pub fn satisfies_data_policy_rule_on(&self, today: IsoDate) -> bool {
        self.label.satisfies_data_policy_rule() && self.is_fresh_on(today)
    }
}

/// A measured incremental-delivery profile.
///
/// Both numbers are required. A record of `streaming: true` is `UNVERIFIED` for
/// latency routing, so a profile that stores only a first-token figure cannot be
/// constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncrementalDelivery {
    /// Measured milliseconds to the first non-empty output delta.
    pub time_to_first_token_ms: u32,
    /// Measured milliseconds between the first and last output delta.
    ///
    /// A burst delivery has a spread near zero, which is exactly the failure a
    /// streaming boolean hides: the first-token number looks good and the caller
    /// still waits for the whole generation.
    pub chunk_spread_ms: u32,
    /// The number of output deltas the measurement observed after the first.
    ///
    /// Zero means the reply arrived in one piece regardless of the declared
    /// streaming support, so it is a burst by observation rather than by timing.
    pub observed_deltas: u32,
}

impl IncrementalDelivery {
    /// The smallest chunk spread that counts as genuinely incremental.
    ///
    /// A model that delivers its entire reply within this window is treated as a
    /// single burst even if it emitted several deltas, because a caller waiting
    /// for the first token gains nothing from deltas that arrive together.
    pub const MIN_INCREMENTAL_SPREAD_MS: u32 = 50;

    /// Returns whether delivery is incremental rather than a single burst.
    ///
    /// This is the predicate the routing layer uses, so "streams" and "delivers
    /// incrementally" cannot be confused: a model with a fast first token and no
    /// spread is a burst, and a route that required incremental delivery must
    /// reject it.
    #[must_use]
    pub const fn is_incremental(self) -> bool {
        self.observed_deltas > 0 && self.chunk_spread_ms >= Self::MIN_INCREMENTAL_SPREAD_MS
    }
}

/// One call's raw delivery timing, before it becomes a [`IncrementalDelivery`] profile.
///
/// **This is the producer `IncrementalDelivery` never had.** The profile type, the routing
/// predicate that consumes it, and the burst tests all existed and were correct, while every
/// `CapabilityDescriptor` in the workspace was constructed with `incremental_delivery: None`:
/// nothing measured a call, so the "verified time to first token *and* token spread" the model
/// gateway architecture requires could not be recorded for any model. The measurement lives
/// here rather than in the controller because turning three raw instants into a profile is a
/// rule about what the profile *means* — the same reason `is_incremental` is not re-derived at
/// each call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryMeasurement {
    /// The instant the call was handed to the provider.
    ///
    /// The interval this anchors is the one an operator feels: from "JARVIS asked" to "the
    /// first token appeared". Anchoring it at the first delta instead would measure nothing —
    /// a call's first delta *is* its first delta — which is the mistake this field exists to
    /// make unrepresentable.
    pub started_at: UtcTimestamp,
    /// The instant the first output delta arrived.
    pub first_output_at: UtcTimestamp,
    /// The instant the last output delta arrived.
    ///
    /// Equal to `first_output_at` for a call that produced exactly one delta, which is why
    /// the delta count is a separate input: one delta is a burst whatever its timing, and
    /// the two instants cannot express that on their own.
    pub last_output_at: UtcTimestamp,
    /// How many output deltas the call produced.
    pub delta_count: u32,
}

impl DeliveryMeasurement {
    /// Derives the profile this measurement describes.
    ///
    /// Three decisions are deliberate, and each exists because the alternative records a
    /// plausible wrong number:
    ///
    /// - **`observed_deltas` counts the deltas *after the first*.** The profile's own
    ///   documentation says so, and its consumer reads `> 0` as "more than one piece
    ///   arrived". A `delta_count` of one is therefore zero observed deltas — a burst by
    ///   observation — which is the value that makes a one-delta call fail an
    ///   incremental-delivery requirement rather than pass it on a fast first token.
    /// - **A negative elapsed interval is clamped to zero rather than refused.** All three
    ///   instants come from the same clock in arrival order, but a clock that steps backwards
    ///   would make a later instant precede an earlier one and `u32::try_from` would fail on
    ///   the subtraction — turning a clock skew into a storage error on the completion path.
    ///   Zero says "no measurable interval", which is the fail-closed reading, and it keeps
    ///   the `is_incremental` verdict a function of the measurement alone.
    /// - **A duration beyond `u32::MAX` milliseconds saturates.** A call cannot run for 49
    ///   days under any run budget, so the branch is unreachable in practice, and a
    ///   saturating value keeps a nonsensical input from panicking on the completion path
    ///   where there is no caller to answer an error.
    #[must_use]
    pub fn profile(&self) -> IncrementalDelivery {
        IncrementalDelivery {
            time_to_first_token_ms: millis_between(self.started_at, self.first_output_at),
            chunk_spread_ms: millis_between(self.first_output_at, self.last_output_at),
            observed_deltas: self.delta_count.saturating_sub(1),
        }
    }
}

/// The whole milliseconds from `from` to `to`, clamped to zero when `to` precedes `from`.
///
/// A free function so both intervals in [`DeliveryMeasurement::profile`] are computed by the
/// same rule: one clamped inline and the other not is how a clock skew becomes an error on
/// one field and a wrong number on the other.
fn millis_between(from: UtcTimestamp, to: UtcTimestamp) -> u32 {
    let elapsed = to.as_timestamp().as_millisecond() - from.as_timestamp().as_millisecond();
    if elapsed < 0 {
        return 0;
    }
    u32::try_from(elapsed).unwrap_or(u32::MAX)
}

/// The fewest distinct calls that may back an attested per-model delivery profile.
///
/// **One call is an anecdote, and this constant is the whole difference between "measured" and
/// "measured once".** A single attempt's figures are a property of that attempt — the prompt
/// that happened to be sent, the moment the provider happened to be under load, the one network
/// path it happened to take — so a descriptor built from one row would tell the routing layer a
/// model's delivery *profile* while having observed one sample of it. Three is chosen as the
/// smallest count that can disagree with itself: two samples that agree prove nothing about the
/// next, while three are the fewest from which a spread across calls is even visible. It is a
/// floor on sample size, not a confidence claim; a real campaign should record far more.
pub const MIN_PROFILE_SAMPLES: usize = 3;

/// The `source_url` recorded for a value JARVIS measured itself.
///
/// An `Evidence` value normally cites an official source, and a measurement taken from this
/// deployment's own recorded calls has no external one. A scheme with no host is used rather
/// than an empty string or a plausible-looking `https://` address, because both of those would
/// be worse: an empty string reads as a defect, and a fabricated URL is a citation to a page
/// that does not say what the value says — which is the promotion evidence exists to prevent.
/// `jarvis://` cannot be fetched and cannot be mistaken for a provider's documentation.
pub const MEASURED_SOURCE_URL: &str = "jarvis://model-calls";

/// The capability key a measured delivery profile is recorded under.
const DELIVERY_CAPABILITY_KEY: &str = "incremental_delivery";

/// One model's measurement campaign: every call that produced output, as raw figures.
///
/// A collection rather than a pre-computed average, because the aggregation rule is domain
/// knowledge and it must be applied where it can be tested against the samples rather than
/// entrusted to whoever happened to write the `SELECT`. The reader supplies rows; this type
/// decides what they mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverySamples {
    /// The provider-qualified model every sample belongs to.
    pub model: crate::model::identity::ModelRef,
    /// The endpoint class the samples were taken against.
    pub endpoint_class: EndpointClass,
    /// Each call's profile, in the order it was loaded.
    pub samples: Vec<IncrementalDelivery>,
}

impl DeliverySamples {
    /// Returns whether there are enough samples to attest a profile.
    ///
    /// The count is checked against [`MIN_PROFILE_SAMPLES`]. A campaign below the floor returns
    /// `false` here rather than a profile, and the caller must then record no evidence at all —
    /// which is the fail-closed direction, since an absent descriptor makes a route that requires
    /// incremental delivery *refuse* rather than pass on one observation.
    #[must_use]
    pub fn is_sufficient(&self) -> bool {
        self.samples.len() >= MIN_PROFILE_SAMPLES
    }

    /// Aggregates the samples into one profile, or `None` when the campaign is too small.
    ///
    /// Three decisions, and the first is the one that keeps the verdict honest:
    ///
    /// - **The aggregate is incremental only if every sample was.** A profile describes what a
    ///   model does, so a campaign in which any call burst is a model that *can* burst, and
    ///   reporting the best sample — or an average spread that a few good samples lift over the
    ///   threshold — would answer "this model delivers incrementally" for a model that sometimes
    ///   does not. The routing layer's question is whether a caller may rely on incremental
    ///   delivery, so the aggregate is the *worst* sample, not the mean. This is deliberately the
    ///   conservative direction, and it is why the field is not named `average_spread_ms`.
    /// - **`time_to_first_token_ms` is the maximum, and `chunk_spread_ms` is the minimum.** Both
    ///   are the pessimistic reading of the same rule: a caller planning for latency uses the
    ///   slowest first token observed, and a caller asking "do deltas keep arriving" uses the
    ///   narrowest spread observed. The optimistic figure for either would let a model be routed
    ///   to on the strength of its best call.
    /// - **`observed_deltas` is the minimum.** It is the sample size of the thinnest call, so a
    ///   campaign containing a one-delta reply reports zero observed deltas and the profile is a
    ///   burst by observation even when the other calls streamed — the same conservative rule as
    ///   the spread, expressed on the count a timing-only measurement cannot see.
    ///
    /// A campaign with too few samples returns `None` even though the folds below would be
    /// vacuously satisfiable, because an aggregate over an insufficient sample is not a profile.
    #[must_use]
    pub fn aggregate(&self) -> Option<IncrementalDelivery> {
        if !self.is_sufficient() {
            return None;
        }
        let mut worst_time_to_first_token_ms = 0u32;
        let mut narrowest_chunk_spread_ms = u32::MAX;
        let mut thinnest_observed_deltas = u32::MAX;
        for sample in &self.samples {
            worst_time_to_first_token_ms =
                worst_time_to_first_token_ms.max(sample.time_to_first_token_ms);
            narrowest_chunk_spread_ms = narrowest_chunk_spread_ms.min(sample.chunk_spread_ms);
            thinnest_observed_deltas = thinnest_observed_deltas.min(sample.observed_deltas);
        }
        Some(IncrementalDelivery {
            time_to_first_token_ms: worst_time_to_first_token_ms,
            chunk_spread_ms: narrowest_chunk_spread_ms,
            observed_deltas: thinnest_observed_deltas,
        })
    }

    /// Returns the profile with the evidence that attests it, or `None` when the campaign is
    /// too small to attest anything.
    ///
    /// The label is always [`EvidenceLabel::Verified`], and that is a deliberate reading of the
    /// contract rather than an upgrade. `VERIFIED` is "confirmed by a current official
    /// specification, schema, or **live test against the pinned version**"; these figures are
    /// exactly that live test — observed against the endpoint this deployment is pinned to, by
    /// this deployment. `DOCUMENTED` would be the wrong label in the other direction, because no
    /// document states them. A value asserted with no samples at all is unreachable: the
    /// sufficiency floor is checked here, so a caller cannot attest a campaign it does not have.
    ///
    /// `integration_id` names the adapter that produced the numbers, because "which provider did
    /// JARVIS measure" is not answerable from the figures themselves, and `last_verified` /
    /// `revalidate_by` come from the caller's clock rather than from inside, so a campaign cannot
    /// silently certify itself as fresh forever.
    #[must_use]
    pub fn attested_profile(
        &self,
        integration_id: &str,
        last_verified: IsoDate,
        revalidate_by: IsoDate,
    ) -> Option<Attested<IncrementalDelivery>> {
        Some(Attested {
            value: self.aggregate()?,
            evidence: Evidence {
                integration_id: integration_id.to_owned(),
                capability_key: DELIVERY_CAPABILITY_KEY.to_owned(),
                label: EvidenceLabel::Verified,
                source_url: MEASURED_SOURCE_URL.to_owned(),
                last_verified,
                revalidate_by,
            },
        })
    }
}

/// A capability value with the evidence that supports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attested<T> {
    /// The attested value.
    pub value: T,
    /// The evidence supporting it.
    pub evidence: Evidence,
}

impl<T> Attested<T> {
    /// Returns whether this value may satisfy a hard requirement on `today`.
    #[must_use]
    pub fn satisfies_hard_requirement_on(&self, today: IsoDate) -> bool {
        self.evidence.satisfies_hard_requirement_on(today)
    }
}

/// A capability the routing layer may require.
///
/// The set is closed and named rather than a `String`, because a free-form key
/// would let a requirement be written that nothing can ever match, which fails
/// closed only by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Incremental (measured) delivery of output deltas.
    IncrementalDelivery,
    /// Native tool/function calling.
    ToolCalling,
    /// Parallel tool calls within one turn.
    ParallelToolCalls,
    /// Structured output constrained to a supported JSON Schema subset.
    StructuredOutput,
    /// A provider continuation or response identifier for resume.
    ConversationContinuation,
    /// A provider-reported usage block.
    UsageReporting,
    /// A safe, user-visible reasoning summary.
    ReasoningSummary,
}

/// A model capability descriptor.
///
/// Only the values that have been measured or documented appear; every field is
/// optional so "not verified" is representable and is never confused with
/// "unsupported".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    /// The provider-qualified model this describes.
    pub model: crate::model::identity::ModelRef,
    /// The endpoint class the measurement or documentation applies to.
    pub endpoint_class: EndpointClass,
    /// The measured incremental-delivery profile, when it has been measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental_delivery: Option<Attested<IncrementalDelivery>>,
    /// The maximum advertised context length in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<Attested<u64>>,
    /// The maximum advertised output length in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<Attested<u64>>,
    /// Additional capability keys with their evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Attested<Capability>>,
}

impl CapabilityDescriptor {
    /// Creates a descriptor with no attested capabilities yet.
    #[must_use]
    pub fn new(model: crate::model::identity::ModelRef, endpoint_class: EndpointClass) -> Self {
        Self {
            model,
            endpoint_class,
            incremental_delivery: None,
            max_context_tokens: None,
            max_output_tokens: None,
            capabilities: Vec::new(),
        }
    }

    /// Returns the attested value of `capability`, when present.
    #[must_use]
    pub fn capability(&self, capability: Capability) -> Option<&Attested<Capability>> {
        self.capabilities
            .iter()
            .find(|entry| entry.value == capability)
    }

    /// Returns whether `capability` is supported by fresh, hard-requirement
    /// evidence on `today`.
    ///
    /// A value with no descriptor entry, or one whose evidence is expired,
    /// inferred, or unverified, returns `false`. That is the fail-closed answer:
    /// the routing layer rejects a candidate it cannot attest rather than
    /// assuming support.
    #[must_use]
    pub fn supports_on(&self, capability: Capability, today: IsoDate) -> bool {
        match capability {
            Capability::IncrementalDelivery => self
                .incremental_delivery
                .as_ref()
                .is_some_and(|attested| attested.satisfies_hard_requirement_on(today)),
            other => self
                .capability(other)
                .is_some_and(|attested| attested.satisfies_hard_requirement_on(today)),
        }
    }

    /// Returns the measured incremental-delivery profile when supported on `today`.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::DeliveryNotIncremental`] when the measurement exists
    /// and is fresh, but the value itself shows a single burst. That is a different
    /// answer from `Ok(None)`, which means "no usable measurement at all", and the
    /// two need different operator responses: one is a model that advertised
    /// streaming and does not deliver it, the other is a model nobody measured. A
    /// boolean API would collapse them, which is the failure a `streaming: true`
    /// flag produces.
    pub fn incremental_delivery_on(
        &self,
        today: IsoDate,
    ) -> Result<Option<IncrementalDelivery>, DomainError> {
        let Some(attested) = self.incremental_delivery.as_ref() else {
            return Ok(None);
        };
        if !attested.satisfies_hard_requirement_on(today) {
            return Ok(None);
        }
        if !attested.value.is_incremental() {
            return Err(DomainError::DeliveryNotIncremental);
        }
        Ok(Some(attested.value))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Attested, Capability, CapabilityDescriptor, DeliveryMeasurement, DeliverySamples, Evidence,
        EvidenceLabel, IncrementalDelivery, MEASURED_SOURCE_URL, MIN_PROFILE_SAMPLES,
    };
    use crate::model::identity::{EndpointClass, ModelId, ModelRef, ProviderId};
    use crate::time::{IsoDate, UtcTimestamp};

    fn evidence(label: EvidenceLabel, revalidate_by: &str) -> Evidence {
        Evidence {
            integration_id: "openai-compatible-model".to_owned(),
            capability_key: "incremental_delivery".to_owned(),
            label,
            source_url: "https://example.invalid/llms.txt".to_owned(),
            last_verified: IsoDate::parse("2026-09-21").expect("valid"),
            revalidate_by: IsoDate::parse(revalidate_by).expect("valid"),
        }
    }

    fn today() -> IsoDate {
        IsoDate::parse("2026-09-22").expect("valid")
    }

    fn model_ref() -> ModelRef {
        ModelRef::new(
            ProviderId::parse("local.ollama").expect("valid"),
            ModelId::parse("llama3.1").expect("valid"),
        )
    }

    #[test]
    fn only_verified_evidence_satisfies_a_hard_requirement() {
        assert!(EvidenceLabel::Verified.satisfies_hard_requirement());
        for label in [
            EvidenceLabel::Documented,
            EvidenceLabel::Observed,
            EvidenceLabel::Inferred,
            EvidenceLabel::Unverified,
        ] {
            assert!(
                !label.satisfies_hard_requirement(),
                "{label} must not satisfy a hard requirement",
            );
        }
    }

    #[test]
    fn evidence_is_valid_through_its_revalidation_day_and_stale_after_it() {
        // `revalidate_by` names the LAST day the value may be used, so a day past
        // it is expired. Asserting the boundary from both sides is what pins the
        // comparison down: an off-by-one here would keep stale evidence routable.
        let on_the_last_valid_day = evidence(EvidenceLabel::Verified, "2026-09-22");
        assert!(on_the_last_valid_day.is_fresh_on(today()));
        assert!(on_the_last_valid_day.satisfies_hard_requirement_on(today()));

        let one_day_past = evidence(EvidenceLabel::Verified, "2026-09-21");
        assert!(
            !one_day_past.is_fresh_on(today()),
            "the day after revalidate_by is already expired",
        );
        assert!(!one_day_past.satisfies_hard_requirement_on(today()));

        let long_expired = evidence(EvidenceLabel::Verified, "2026-09-01");
        assert!(!long_expired.is_fresh_on(today()));
    }

    #[test]
    fn a_fresh_label_that_is_not_verified_still_fails_a_hard_requirement() {
        let documented = evidence(EvidenceLabel::Documented, "2027-01-01");
        assert!(documented.is_fresh_on(today()));
        assert!(
            !documented.satisfies_hard_requirement_on(today()),
            "fresh documentation is not a verified value",
        );
    }

    #[test]
    fn a_burst_delivery_is_not_incremental_even_with_a_fast_first_token() {
        let burst = IncrementalDelivery {
            time_to_first_token_ms: 120,
            chunk_spread_ms: 0,
            observed_deltas: 12,
        };
        assert!(
            !burst.is_incremental(),
            "deltas arriving together are a burst, not a stream",
        );

        let single = IncrementalDelivery {
            time_to_first_token_ms: 120,
            chunk_spread_ms: 4_000,
            observed_deltas: 0,
        };
        assert!(
            !single.is_incremental(),
            "no observed delta after the first is a burst regardless of timing",
        );

        let incremental = IncrementalDelivery {
            time_to_first_token_ms: 120,
            chunk_spread_ms: IncrementalDelivery::MIN_INCREMENTAL_SPREAD_MS,
            observed_deltas: 12,
        };
        assert!(incremental.is_incremental());
    }

    /// Builds a measurement from millisecond offsets, so a test states intervals rather than
    /// instants: the assertions below are about a spread and a delay, and spelling them as
    /// timestamps would make a reader subtract three numbers to see the interval being asserted.
    fn measured(
        start_ms: i64,
        first_output_ms: i64,
        last_output_ms: i64,
        delta_count: u32,
    ) -> DeliveryMeasurement {
        fn at(offset_ms: i64) -> UtcTimestamp {
            let base = UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid");
            UtcTimestamp::from_timestamp(
                base.as_timestamp() + jiff::SignedDuration::from_millis(offset_ms),
            )
        }
        DeliveryMeasurement {
            started_at: at(start_ms),
            first_output_at: at(first_output_ms),
            last_output_at: at(last_output_ms),
            delta_count,
        }
    }

    #[test]
    fn a_measurement_produces_the_profile_the_router_reads() {
        // The producer `IncrementalDelivery` never had. Every part of this pipeline existed and
        // worked — the profile type, the routing predicate, the burst tests — and **nothing
        // measured a call**, so every descriptor in the workspace was built with
        // `incremental_delivery: None`. The interval asserted here is the one an operator feels:
        // from the call being handed to the provider to the first token appearing.
        let profile = measured(0, 250, 1_450, 40).profile();
        assert_eq!(profile.time_to_first_token_ms, 250);
        assert_eq!(profile.chunk_spread_ms, 1_200);
        // 40 deltas means 39 after the first, because `observed_deltas` is documented as the
        // number *after* the first and the routing predicate reads `> 0` as "more than one piece".
        assert_eq!(profile.observed_deltas, 39);
        assert!(profile.is_incremental());
    }

    #[test]
    fn a_single_delta_is_a_burst_by_observation_whatever_its_timing() {
        // The case a timing-only measurement cannot see. One delta arriving after a long wait
        // has a plausible first-token number and a spread of zero — and a *nonzero* first-token
        // interval, so a profile derived from instants alone would look like a very slow but
        // legitimate stream. Only the count says otherwise.
        let profile = measured(0, 5_000, 5_000, 1).profile();
        assert_eq!(profile.time_to_first_token_ms, 5_000);
        assert_eq!(profile.chunk_spread_ms, 0);
        assert_eq!(
            profile.observed_deltas, 0,
            "one delta is zero deltas after the first",
        );
        assert!(
            !profile.is_incremental(),
            "a reply delivered in one piece is a burst however long it took",
        );
    }

    #[test]
    fn deltas_arriving_together_are_a_burst_despite_the_count() {
        // The other half, and the reason both inputs are needed: many deltas arriving within the
        // minimum window are a burst, because a caller waiting for the first token gains nothing
        // from a flush that arrives all at once.
        let profile = measured(0, 100, 120, 30).profile();
        assert_eq!(profile.observed_deltas, 29);
        assert!(
            profile.chunk_spread_ms < IncrementalDelivery::MIN_INCREMENTAL_SPREAD_MS,
            "the fixture must sit below the minimum spread or it proves nothing",
        );
        assert!(!profile.is_incremental());
    }

    #[test]
    fn a_clock_that_steps_backwards_yields_no_spread_rather_than_a_refusal() {
        // All three instants come from the same clock in arrival order, so a later one preceding
        // an earlier one is a clock fault rather than a caller error. Clamping to zero keeps the
        // verdict fail-closed and, more importantly, keeps the completion path from failing on a
        // subtraction — there is no caller at that point to answer an error.
        let profile = measured(1_000, 500, 400, 10).profile();
        assert_eq!(profile.time_to_first_token_ms, 0);
        assert_eq!(profile.chunk_spread_ms, 0);
        assert!(!profile.is_incremental());
    }

    #[test]
    fn a_measurement_with_no_deltas_still_produces_a_profile_a_router_must_reject() {
        // Zero deltas is `observed_deltas: 0`, which is a burst — not "unknown". A caller that
        // wanted "no measurement" must leave the descriptor's field absent; arriving here with a
        // count of zero means the call produced nothing, and routing to it as incremental would
        // be the optimistic reading of an empty measurement.
        let profile = measured(0, 0, 0, 0).profile();
        assert_eq!(profile.observed_deltas, 0);
        assert!(!profile.is_incremental());
    }

    #[test]
    fn a_descriptor_without_a_measurement_does_not_support_incremental_delivery() {
        let descriptor = CapabilityDescriptor::new(model_ref(), EndpointClass::Local);
        assert!(!descriptor.supports_on(Capability::IncrementalDelivery, today()));
        assert_eq!(
            descriptor
                .incremental_delivery_on(today())
                .expect("absent is not an error"),
            None,
        );
    }

    #[test]
    fn a_burst_measurement_is_reported_as_a_typed_refusal_not_as_supported() {
        let mut descriptor = CapabilityDescriptor::new(model_ref(), EndpointClass::Local);
        descriptor.incremental_delivery = Some(Attested {
            value: IncrementalDelivery {
                time_to_first_token_ms: 90,
                chunk_spread_ms: 0,
                observed_deltas: 8,
            },
            evidence: evidence(EvidenceLabel::Verified, "2027-01-01"),
        });

        // The measurement exists and is verified, so `supports_on` is true in the
        // sense that the value is attested. The failure is in the *value*: the
        // model reports streaming and delivers a burst, which is the case a
        // streaming boolean cannot express.
        assert!(descriptor.supports_on(Capability::IncrementalDelivery, today()));
        assert!(
            descriptor.incremental_delivery_on(today()).is_err(),
            "an attested burst must be a typed refusal, not a supported route",
        );
    }

    #[test]
    fn an_expired_measurement_is_treated_as_unmeasured() {
        let mut descriptor = CapabilityDescriptor::new(model_ref(), EndpointClass::Local);
        descriptor.incremental_delivery = Some(Attested {
            value: IncrementalDelivery {
                time_to_first_token_ms: 90,
                chunk_spread_ms: 900,
                observed_deltas: 40,
            },
            evidence: evidence(EvidenceLabel::Verified, "2026-09-01"),
        });
        assert!(!descriptor.supports_on(Capability::IncrementalDelivery, today()));
        assert_eq!(
            descriptor
                .incremental_delivery_on(today())
                .expect("expired is not an error"),
            None,
        );
    }

    #[test]
    fn a_plain_capability_requires_fresh_verified_evidence() {
        let mut descriptor = CapabilityDescriptor::new(model_ref(), EndpointClass::Local);
        descriptor.capabilities.push(Attested {
            value: Capability::ToolCalling,
            evidence: evidence(EvidenceLabel::Verified, "2027-01-01"),
        });
        assert!(descriptor.supports_on(Capability::ToolCalling, today()));
        assert!(
            !descriptor.supports_on(Capability::ParallelToolCalls, today()),
            "an unlisted capability must not be assumed",
        );
    }

    #[test]
    fn the_evidence_label_strings_match_the_contract() {
        assert_eq!(EvidenceLabel::Unverified.to_string(), "UNVERIFIED");
        assert_eq!(EvidenceLabel::Inferred.to_string(), "INFERRED");
        assert_eq!(EvidenceLabel::Observed.to_string(), "OBSERVED");
        assert_eq!(EvidenceLabel::Documented.to_string(), "DOCUMENTED");
        assert_eq!(EvidenceLabel::Verified.to_string(), "VERIFIED");
    }

    /// Builds a campaign from `(spread_ms, observed_deltas)` pairs.
    fn campaign(samples: &[(u32, u32)]) -> DeliverySamples {
        DeliverySamples {
            model: model_ref(),
            endpoint_class: EndpointClass::Local,
            samples: samples
                .iter()
                .map(|(spread, deltas)| IncrementalDelivery {
                    time_to_first_token_ms: 100,
                    chunk_spread_ms: *spread,
                    observed_deltas: *deltas,
                })
                .collect(),
        }
    }

    #[test]
    fn one_call_is_not_a_profile() {
        // The whole difference between "measured" and "measured once". A single attempt's figures
        // are a property of that attempt, so a descriptor built from one row would tell the router
        // a model's *profile* while having observed one sample of it — and `VERIFIED` would then be
        // asserted for something that was never a campaign.
        let single = campaign(&[(900, 40)]);
        assert!(!single.is_sufficient());
        assert_eq!(single.aggregate(), None);
        assert_eq!(
            single.attested_profile("openai-compatible-model", today(), today()),
            None
        );

        // Two agreeing samples still prove nothing about the next, so the floor is three and the
        // boundary is asserted from both sides rather than only below it.
        assert!(!campaign(&[(900, 40), (900, 40)]).is_sufficient());
        assert!(campaign(&[(900, 40), (900, 40), (900, 40)]).is_sufficient());
    }

    #[test]
    fn one_burst_sample_makes_the_whole_campaign_a_burst() {
        // **The falsification this rule exists for.** Three calls spread over ~400 ms and one that
        // arrived in a single flush: a mean of the spreads clears `MIN_INCREMENTAL_SPREAD_MS`
        // comfortably, and an "any sample was incremental" rule also passes — both would report
        // this model as delivering incrementally, and a caller relying on that would wait for the
        // whole generation on the one call that burst.
        let samples = campaign(&[(900, 40), (900, 40), (900, 40), (0, 8)]);
        let aggregate = samples.aggregate().expect("four samples is sufficient");

        assert_eq!(
            aggregate.chunk_spread_ms, 0,
            "the narrowest spread is the one that decides, so the burst sample wins",
        );
        assert!(
            !aggregate.is_incremental(),
            "a model that burst once is a model that can burst",
        );

        // The mean is computed here only to show the two rules disagree on this exact input: a
        // rule that averaged would reach the opposite verdict, which is what makes this test a
        // check rather than a restatement.
        let mean_spread = samples
            .samples
            .iter()
            .map(|s| s.chunk_spread_ms)
            .sum::<u32>()
            / u32::try_from(samples.samples.len()).expect("small");
        assert!(
            mean_spread >= IncrementalDelivery::MIN_INCREMENTAL_SPREAD_MS,
            "the fixture must be one the averaging rule would wrongly accept",
        );
    }

    #[test]
    fn one_single_delta_sample_makes_the_whole_campaign_a_burst_by_observation() {
        // The count half of the same rule, and the half a spread-only aggregate cannot express:
        // every sample spread over a healthy window while one reply arrived as a single delta, so
        // the thinnest sample reports zero observed deltas. Note the fixture uses `0`, which is
        // what `DeliveryMeasurement::profile` produces for a one-delta call — the delta count is
        // a raw input and `observed_deltas` is the derived "after the first" figure, so passing
        // `1` here would be constructing a profile that a measurement cannot produce.
        let aggregate = campaign(&[(900, 40), (900, 40), (900, 0)])
            .aggregate()
            .expect("three samples is sufficient");
        assert_eq!(aggregate.observed_deltas, 0);
        assert!(!aggregate.is_incremental());
    }

    #[test]
    fn the_aggregate_takes_the_pessimistic_figure_for_every_field() {
        // A campaign where the calls disagree in the *other* direction — every one incremental —
        // still reports the slowest first token, the narrowest spread, and the thinnest count.
        // Taking the best of anything would let a model be routed to on its strongest call.
        let samples = DeliverySamples {
            model: model_ref(),
            endpoint_class: EndpointClass::Local,
            samples: vec![
                IncrementalDelivery {
                    time_to_first_token_ms: 80,
                    chunk_spread_ms: 2_000,
                    observed_deltas: 60,
                },
                IncrementalDelivery {
                    time_to_first_token_ms: 640,
                    chunk_spread_ms: 900,
                    observed_deltas: 12,
                },
                IncrementalDelivery {
                    time_to_first_token_ms: 300,
                    chunk_spread_ms: 1_400,
                    observed_deltas: 31,
                },
            ],
        };
        let aggregate = samples.aggregate().expect("three samples is sufficient");
        assert_eq!(
            aggregate.time_to_first_token_ms, 640,
            "the slowest first token"
        );
        assert_eq!(aggregate.chunk_spread_ms, 900, "the narrowest spread");
        assert_eq!(aggregate.observed_deltas, 12, "the thinnest sample size");
        assert!(
            aggregate.is_incremental(),
            "every sample streamed, so this one may route"
        );
    }

    #[test]
    fn a_measured_profile_carries_verified_evidence_and_no_fabricated_source() {
        let samples = campaign(&[(900, 40), (1_100, 30), (700, 50)]);
        let attested = samples
            .attested_profile(
                "openai-compatible-model",
                today(),
                IsoDate::parse("2027-01-01").expect("valid"),
            )
            .expect("three samples attest a profile");

        // `VERIFIED` is "confirmed by a live test against the pinned version", which is exactly
        // what these figures are and what no document states. `DOCUMENTED` would be wrong in the
        // other direction.
        assert_eq!(attested.evidence.label, EvidenceLabel::Verified);
        assert_eq!(attested.evidence.integration_id, "openai-compatible-model");
        assert_eq!(attested.evidence.source_url, MEASURED_SOURCE_URL);
        // A measurement cites no external page. The sentinel must not be mistakable for one, and
        // an empty string would read as a defect rather than as "JARVIS measured this itself".
        assert!(MEASURED_SOURCE_URL.starts_with("jarvis://"));
        assert!(!MEASURED_SOURCE_URL.starts_with("http"));

        // The evidence makes the descriptor routable, which is the point of attesting at all.
        let mut descriptor = CapabilityDescriptor::new(model_ref(), EndpointClass::Local);
        descriptor.incremental_delivery = Some(attested);
        assert!(descriptor.supports_on(Capability::IncrementalDelivery, today()));
        assert!(descriptor.incremental_delivery_on(today()).is_ok());
    }

    #[test]
    fn a_campaign_too_small_to_attest_cannot_claim_verified_evidence() {
        // The floor is checked *inside* the attesting call, not only at its call site, so a caller
        // cannot reach `Some(Attested { label: Verified })` with one sample however it constructs
        // the campaign. A label asserted over an insufficient sample is the promotion the evidence
        // rule exists to forbid.
        for size in 0..MIN_PROFILE_SAMPLES {
            let samples = campaign(&vec![(900, 40); size]);
            assert_eq!(
                samples.attested_profile("openai-compatible-model", today(), today()),
                None,
                "a campaign of {size} samples must attest nothing",
            );
        }
    }
}
