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
use crate::time::IsoDate;

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
        Attested, Capability, CapabilityDescriptor, Evidence, EvidenceLabel, IncrementalDelivery,
    };
    use crate::model::identity::{EndpointClass, ModelId, ModelRef, ProviderId};
    use crate::time::IsoDate;

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
}
