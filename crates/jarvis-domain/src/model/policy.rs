//! The model data policy: typed rules, their layered precedence, and the
//! requested/effective decision a model call records.
//!
//! This module is the domain half of `docs/contracts/model-data-policy.md`. The
//! central rule it enforces is **precedence with no weakening**: a hard deny from
//! a higher layer cannot be relaxed by a lower one, and explicit user selection is
//! itself a layer rather than an override.
//!
//! The merge is implemented as a fold over layers ordered from strongest to
//! weakest, where each layer may only *narrow* the accumulated result. That is why
//! [`PolicyRules::merge_stricter`] returns an error when it
//! cannot express a
//! narrowing: refusing is what keeps an unimplemented interaction between two
//! rules from silently permitting more than either rule allowed.
//!
//! A second rule it enforces is that a **provider claim is not a JARVIS
//! guarantee**. The effective decision records what was selected and the evidence
//! that supported it, and [`EffectiveDataPolicy`] carries no field that could be
//! read as "JARVIS promises the provider deleted this".

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::ModelDataPolicyId;
use crate::model::identity::{EndpointClass, ModelRef};
use crate::time::UtcTimestamp;

/// Where model inputs and outputs may go.
///
/// Ordered from most restrictive to least, so a stricter value is a smaller one
/// and `min` is the merge operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locality {
    /// Stays on the local device; no network call.
    LocalOnly,
    /// A network the operator controls is permitted.
    PrivateNetworkAllowed,
    /// An approved cloud provider is permitted.
    ApprovedCloudAllowed,
}

/// The required documented provider retention behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRetention {
    /// Evidence states no retention for the selected account, endpoint, mode.
    NoneDocumented,
    /// Evidence states a bounded retention window.
    BoundedDocumented,
    /// The provider default is acceptable.
    ProviderDefaultAllowed,
}

/// The required documented provider training-use behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingUse {
    /// Evidence states inputs are not used for training.
    DisallowedDocumented,
    /// The account's own policy is acceptable.
    AccountPolicyAllowed,
    /// The provider default is acceptable.
    ProviderDefaultAllowed,
}

/// Whether JARVIS telemetry may be emitted for a model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Telemetry {
    /// No telemetry beyond local records.
    Disabled,
    /// Local-only telemetry.
    LocalOnly,
}

/// A data-classification ceiling for the call's content.
///
/// The ladder is the one the policy contract's example uses: a call's maximum
/// sensitivity bounds what may be sent, so a higher value is less restrictive and
/// the merge takes the minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Publicly shareable content.
    Public,
    /// Ordinary internal content.
    Internal,
    /// Confidential content, the contract's stated example ceiling.
    Confidential,
    /// The most restricted content.
    Restricted,
}

impl Sensitivity {
    /// Returns the label JARVIS stores and puts on the wire.
    ///
    /// The label exists in the domain because two layers need the same spelling and neither can
    /// reach the other: `jarvis-application` writes a message's stored label and the route decision
    /// that judges it, and `jarvis-protocol` renders the wire form. A `serde(rename_all)` derive is
    /// not enough, because the stored message label is built by hand and would have to repeat the
    /// spelling — which is how a message ends up labelled with something the decision did not
    /// classify. `context_assembly::parse_sensitivity` is the inverse and has a test asserting the
    /// two agree over every variant.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Confidential => "confidential",
            Self::Restricted => "restricted",
        }
    }
}

/// Whether a route may fall back to another compliant candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPermission {
    /// No fallback; the selected candidate or nothing.
    Denied,
    /// A fallback that satisfies every hard rule is permitted.
    CompliantOnly,
}

/// The typed rules of one model data policy version.
///
/// Every field is a typed enum or a set of typed identifiers, never a free-form
/// provider string, so an unsupported value cannot be expressed and therefore
/// cannot be silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRules {
    /// Where inputs may go.
    pub locality: Locality,
    /// Providers explicitly permitted. Empty means "no provider restriction at
    /// this layer", not "no provider permitted".
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub allowed_providers: BTreeSet<String>,
    /// Models explicitly permitted.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub allowed_models: BTreeSet<String>,
    /// The required documented retention behavior.
    pub maximum_provider_retention: ProviderRetention,
    /// The required documented training-use behavior.
    pub provider_training_use: TrainingUse,
    /// Whether telemetry may be emitted.
    pub telemetry: Telemetry,
    /// Residency regions the content may be processed in. Empty means "no region
    /// restriction at this layer".
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub allowed_residency_regions: BTreeSet<String>,
    /// The most sensitive content this policy permits.
    pub maximum_sensitivity: Sensitivity,
    /// Whether a compliant fallback is permitted.
    pub allow_fallback: FallbackPermission,
}

impl PolicyRules {
    /// The most permissive rules, used as the fold's identity.
    ///
    /// Every subsequent layer may only narrow this, which is what makes the merge
    /// a sequence of restrictions rather than a sequence of resolutions: a layer
    /// that is *absent* cannot loosen a layer that denies.
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            locality: Locality::ApprovedCloudAllowed,
            allowed_providers: BTreeSet::new(),
            allowed_models: BTreeSet::new(),
            maximum_provider_retention: ProviderRetention::ProviderDefaultAllowed,
            provider_training_use: TrainingUse::ProviderDefaultAllowed,
            telemetry: Telemetry::LocalOnly,
            allowed_residency_regions: BTreeSet::new(),
            maximum_sensitivity: Sensitivity::Restricted,
            allow_fallback: FallbackPermission::CompliantOnly,
        }
    }

    /// Returns a copy narrowed by `stricter`.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidPolicyLayer`] when a set-valued rule cannot
    /// be narrowed by intersection. The empty set means "no restriction at this
    /// layer", so intersecting an unrestricted layer with a restricted one must
    /// produce the restricted set rather than the empty set — and a *disjoint*
    /// intersection (two layers naming different allowed providers) has no
    /// compliant value at all, which is reported rather than resolved to empty.
    pub fn merge_stricter(&self, stricter: &Self) -> Result<Self, DomainError> {
        Ok(Self {
            locality: self.locality.min(stricter.locality),
            allowed_providers: intersect(&self.allowed_providers, &stricter.allowed_providers)?,
            allowed_models: intersect(&self.allowed_models, &stricter.allowed_models)?,
            maximum_provider_retention: self
                .maximum_provider_retention
                .min(stricter.maximum_provider_retention),
            provider_training_use: self
                .provider_training_use
                .min(stricter.provider_training_use),
            telemetry: self.telemetry.min(stricter.telemetry),
            allowed_residency_regions: intersect(
                &self.allowed_residency_regions,
                &stricter.allowed_residency_regions,
            )?,
            maximum_sensitivity: self.maximum_sensitivity.min(stricter.maximum_sensitivity),
            allow_fallback: self.allow_fallback.min(stricter.allow_fallback),
        })
    }
}

/// Narrows two allow-lists where an empty list means "unrestricted".
fn intersect(
    accumulated: &BTreeSet<String>,
    stricter: &BTreeSet<String>,
) -> Result<BTreeSet<String>, DomainError> {
    match (accumulated.is_empty(), stricter.is_empty()) {
        (true, _) => Ok(stricter.clone()),
        (_, true) => Ok(accumulated.clone()),
        (false, false) => {
            let narrowed: BTreeSet<String> = accumulated.intersection(stricter).cloned().collect();
            if narrowed.is_empty() {
                // Two disjoint allow-lists permit nothing; reporting it is what
                // keeps an impossible policy from looking like an empty permit set
                // that a later layer might treat as "unrestricted".
                return Err(DomainError::InvalidPolicyLayer);
            }
            Ok(narrowed)
        }
    }
}

/// One layer in the precedence order.
///
/// The order is the contract's order, and the derived `Ord` follows the
/// declaration order, so the fold can sort layers rather than rely on a hand-kept
/// list that could drift from the documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyLayer {
    /// Legal, deployment, and administrator denies.
    LegalOrAdministrator,
    /// Workspace policy and data-classification ceiling.
    Workspace,
    /// Resource/document sensitivity policy.
    ResourceSensitivity,
    /// Task or run explicit restrictions.
    TaskRestriction,
    /// User preferences.
    UserPreference,
    /// Provider and model defaults.
    ProviderDefault,
}

impl PolicyLayer {
    /// Every layer in precedence order, strongest first.
    pub const ORDERED: [Self; 6] = [
        Self::LegalOrAdministrator,
        Self::Workspace,
        Self::ResourceSensitivity,
        Self::TaskRestriction,
        Self::UserPreference,
        Self::ProviderDefault,
    ];
}

/// A named layer's contribution to the resolved policy, retained for explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyLayerContribution {
    /// Which layer this is.
    pub layer: PolicyLayer,
    /// The rules this layer supplied.
    pub rules: PolicyRules,
    /// The immutable policy version that supplied them, when it is a stored one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<PolicyVersionRef>,
}

/// A stored policy version's lifecycle state.
///
/// Only an `Active` version is resolved for a new call. Archiving is a state rather than a
/// delete because a past route decision names the version it ran under, and removing the row
/// would make that decision unexplainable — the contract requires historical records to keep
/// "the policy/evidence version needed to explain a past decision".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelDataPolicyStatus {
    /// In force: this is the version a new call resolves.
    Active,
    /// Retained for explanation, but not applied to a new call.
    Archived,
}

impl ModelDataPolicyStatus {
    /// Returns the contract-shaped name, which is also the stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    /// Parses the stored value.
    ///
    /// Returns `None` for anything else, so a reader decides what an unknown status means
    /// rather than this function inventing a third state.
    #[must_use]
    pub const fn from_stored(value: &str) -> Option<Self> {
        match value.as_bytes() {
            b"active" => Some(Self::Active),
            b"archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

/// A reference to an immutable stored policy version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyVersionRef {
    /// The policy record identifier.
    pub policy_id: ModelDataPolicyId,
    /// The immutable version number.
    pub version: u32,
}

/// The result of merging every layer.
///
/// Retains the layers that produced it so a route decision can explain itself
/// without re-deriving the merge from current state, which may have changed since
/// the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPolicy {
    /// The merged, most restrictive rules.
    pub rules: PolicyRules,
    /// The layers that contributed, in precedence order.
    pub layers: Vec<PolicyLayerContribution>,
}

impl ResolvedPolicy {
    /// Merges layers into one policy, strongest first.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidPolicyLayer`] when two layers cannot be
    /// combined. Every layer is merged rather than the first deny being taken, so
    /// a stricter allow-list in a *later* layer still narrows an earlier one, which
    /// is the case a "first deny wins" shortcut gets wrong.
    pub fn merge(layers: Vec<PolicyLayerContribution>) -> Result<Self, DomainError> {
        let mut ordered = layers;
        ordered.sort_by_key(|contribution| contribution.layer);

        let mut rules = PolicyRules::permissive();
        for contribution in &ordered {
            rules = rules.merge_stricter(&contribution.rules)?;
        }
        Ok(Self {
            rules,
            layers: ordered,
        })
    }

    /// Returns the layers that actually narrowed the result.
    ///
    /// Used by operator output so the reader sees which policy is responsible for
    /// a refusal, rather than being told only that a rule applies.
    #[must_use]
    pub fn narrowing_layers(&self) -> Vec<PolicyLayer> {
        let mut accumulated = PolicyRules::permissive();
        let mut narrowing = Vec::new();
        for contribution in &self.layers {
            if let Ok(next) = accumulated.merge_stricter(&contribution.rules) {
                if next != accumulated {
                    narrowing.push(contribution.layer);
                }
                accumulated = next;
            }
        }
        narrowing
    }
}

/// What was asked for, before any selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedDataPolicy {
    /// The requested locality.
    pub locality: Locality,
    /// The requested documented retention behavior.
    pub maximum_provider_retention: ProviderRetention,
    /// The requested documented training-use behavior.
    pub provider_training_use: TrainingUse,
    /// The requested telemetry setting.
    pub telemetry: Telemetry,
    /// The requested residency regions.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub allowed_residency_regions: BTreeSet<String>,
    /// The sensitivity of the content being sent.
    pub sensitivity: Sensitivity,
}

impl From<&PolicyRules> for RequestedDataPolicy {
    fn from(rules: &PolicyRules) -> Self {
        Self {
            locality: rules.locality,
            maximum_provider_retention: rules.maximum_provider_retention,
            provider_training_use: rules.provider_training_use,
            telemetry: rules.telemetry,
            allowed_residency_regions: rules.allowed_residency_regions.clone(),
            sensitivity: rules.maximum_sensitivity,
        }
    }
}

/// Where the selected provider actually runs, as resolved for this call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveResidency {
    /// On the local device.
    LocalDevice,
    /// In a region the operator named.
    Region,
}

/// The resolved data-handling posture of the selected route.
///
/// Deliberately carries **no** field stating that a provider deleted data or did
/// not train on it. The contract requires provider guarantees to be exposed as
/// `DOCUMENTED`/`OBSERVED` evidence rather than as JARVIS guarantees, and the way
/// to make that structural is to have no such field to populate: what is recorded
/// is which evidence supported the selection.
///
/// The identity fields are the same [`ModelRef`] the rest of the gateway uses
/// rather than free-form strings, because the whole point of the provider/model
/// split is that the pair is one value everywhere — a decision that recorded its
/// selection as text could name a provider/model combination no `ModelRef` can
/// express.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveDataPolicy {
    /// The selected provider and model.
    pub model: ModelRef,
    /// The endpoint class of the selection.
    pub endpoint_class: EndpointClass,
    /// How retention was classified for this call.
    pub retention: EffectiveRetention,
    /// How training use was classified for this call.
    pub training_use: EffectiveTrainingUse,
    /// The effective telemetry setting.
    pub telemetry: Telemetry,
    /// The effective residency.
    pub residency: EffectiveResidency,
}

impl EffectiveDataPolicy {
    /// Returns whether the selection is consistent with its own classification.
    ///
    /// A local endpoint cannot be recorded as having provider retention or
    /// training use, because those are cloud concepts: `not_applicable_local` is
    /// the only truthful value. This is a check rather than an impossibility
    /// because the two fields are set independently by the routing layer, and the
    /// inconsistency would be invisible — a local route reporting
    /// `bounded_documented` reads as *more* careful than it is.
    #[must_use]
    pub fn is_self_consistent(&self) -> bool {
        match self.endpoint_class {
            EndpointClass::Local => {
                matches!(self.retention, EffectiveRetention::NotApplicableLocal)
                    && matches!(self.training_use, EffectiveTrainingUse::NotApplicableLocal)
                    && matches!(self.residency, EffectiveResidency::LocalDevice)
            }
            EndpointClass::PrivateNetwork | EndpointClass::ApprovedCloud => {
                !matches!(self.retention, EffectiveRetention::NotApplicableLocal)
                    && !matches!(self.training_use, EffectiveTrainingUse::NotApplicableLocal)
            }
        }
    }
}

/// The retention classification of the effective route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveRetention {
    /// The endpoint is local, so provider retention does not apply.
    NotApplicableLocal,
    /// Retention is bounded by documented, current evidence.
    BoundedDocumented,
    /// No retention is documented by current evidence.
    NoneDocumented,
    /// The provider's own default terms were accepted, which **documents nothing**.
    ///
    /// A distinct value rather than `none_documented`, and the distinction is the contract's:
    /// `none_documented` means "current official evidence states no retention", while this
    /// means "the caller accepted whatever the provider does by default". Recording the second
    /// as the first would claim evidence that does not exist — the promotion the contract
    /// forbids — and a route accepted on default terms would read as the most careful kind.
    ProviderDefault,
}

/// The training-use classification of the effective route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveTrainingUse {
    /// The endpoint is local, so provider training use does not apply.
    NotApplicableLocal,
    /// Training use is disallowed by documented, current evidence.
    DisallowedDocumented,
    /// The account's own policy governs.
    AccountPolicy,
}

/// Why one candidate was rejected.
///
/// A closed enum rather than a message, because a rejection reason is recorded in
/// the route decision and read by operators; a free-form string would let two
/// different causes render identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    /// The candidate is not in the layer's provider allow-list.
    ProviderNotAllowed,
    /// The candidate is not in the layer's model allow-list.
    ModelNotAllowed,
    /// The candidate's endpoint class is outside the required locality.
    LocalityViolated,
    /// The candidate cannot document the required retention behavior.
    RetentionUnsatisfied,
    /// The candidate cannot document the required training-use behavior.
    TrainingUseUnsatisfied,
    /// The candidate's residency is outside the allowed regions.
    ResidencyUnsatisfied,
    /// A hard capability cannot be attested for the candidate.
    CapabilityUnattested,
    /// The candidate's evidence has expired.
    EvidenceStale,
    /// The candidate has no evidence note at all.
    EvidenceMissing,
    /// The content's classification exceeds the policy's sensitivity ceiling.
    ///
    /// This is not a fact about a candidate — the same reason is reported for every
    /// candidate — and that is why it is checked *first*. The ceiling bounds what may be
    /// sent at all, so a call above it is refused before any candidate's placement or
    /// evidence is examined; a per-candidate reason would imply that some other candidate
    /// could have carried it, which is exactly the promotion the ceiling forbids.
    SensitivityExceedsPolicy,
}

impl RejectionReason {
    /// Returns the policy rule this reason is about, when it is about a rule at all.
    ///
    /// This is the bridge between a refusal and the exception that may relax it, and it is
    /// deliberately a **mapping** rather than a field carried beside the reason: deriving the
    /// rule from the reason means there is exactly one place where "which rule failed" is
    /// decided, while a second field would be a second answer to the same question.
    ///
    /// Three reasons return `None` and cannot be relaxed by any exception:
    /// `CapabilityUnattested`, `EvidenceStale`, and `EvidenceMissing` are facts about
    /// **evidence**, not about a rule an operator wrote. Nobody can grant "this provider's
    /// capability is verified" — only a measurement or a current official source can — so
    /// there is no rule for an exception to name and none is offered.
    ///
    /// The three reasons whose keys are **non**-waivable still return their key, because the
    /// refusal has to be nameable for the relaxation path to answer "that rule cannot be
    /// relaxed" rather than silently finding no exception for it. Whether the key may in fact
    /// be relaxed is [`crate::model::exception::PolicyRuleKey::is_waivable`]'s answer, applied
    /// in one place rather than implied by this mapping.
    #[must_use]
    pub const fn policy_rule(self) -> Option<crate::model::exception::PolicyRuleKey> {
        use crate::model::exception::PolicyRuleKey;
        match self {
            Self::ProviderNotAllowed => Some(PolicyRuleKey::AllowedProviders),
            Self::ModelNotAllowed => Some(PolicyRuleKey::AllowedModels),
            Self::LocalityViolated => Some(PolicyRuleKey::Locality),
            Self::RetentionUnsatisfied => Some(PolicyRuleKey::MaximumProviderRetention),
            Self::TrainingUseUnsatisfied => Some(PolicyRuleKey::ProviderTrainingUse),
            Self::ResidencyUnsatisfied => Some(PolicyRuleKey::ResidencyRegions),
            Self::SensitivityExceedsPolicy => Some(PolicyRuleKey::MaximumSensitivity),
            Self::CapabilityUnattested | Self::EvidenceStale | Self::EvidenceMissing => None,
        }
    }
}

/// A rejected candidate and the reason it was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedCandidate {
    /// The candidate that was rejected.
    pub model: ModelRef,
    /// Why it was rejected.
    pub reason: RejectionReason,
}

/// The persisted route decision for one model call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRouteDecision {
    /// The policy version this decision was made under.
    pub policy: PolicyVersionRef,
    /// What was requested.
    pub requested: RequestedDataPolicy,
    /// What was selected.
    pub effective: EffectiveDataPolicy,
    /// The evidence notes that supported the selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
    /// The candidates that were rejected, with reasons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_candidates: Vec<RejectedCandidate>,
    /// The policy exception that permitted this call, when one was needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_ref: Option<String>,
    /// When the decision was made.
    pub decided_at: UtcTimestamp,
}

impl ModelRouteDecision {
    /// Returns whether the selected route required a policy exception.
    ///
    /// An exception is a durable, separately revocable record, so a decision that
    /// needed one must name it. A decision that relaxed a hard rule without an
    /// exception reference is a defect, and this is the predicate that exposes it.
    #[must_use]
    pub const fn relied_on_exception(&self) -> bool {
        self.exception_ref.is_some()
    }
}

impl fmt::Display for RejectionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::ProviderNotAllowed => "provider not allowed",
            Self::ModelNotAllowed => "model not allowed",
            Self::LocalityViolated => "locality violated",
            Self::RetentionUnsatisfied => "retention requirement unsatisfied",
            Self::TrainingUseUnsatisfied => "training-use requirement unsatisfied",
            Self::ResidencyUnsatisfied => "residency not allowed",
            Self::CapabilityUnattested => "capability not attested",
            Self::EvidenceStale => "evidence is stale",
            Self::EvidenceMissing => "evidence is missing",
            Self::SensitivityExceedsPolicy => {
                "the content exceeds the policy's sensitivity ceiling"
            }
        };
        formatter.write_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EffectiveDataPolicy, EffectiveResidency, EffectiveRetention, EffectiveTrainingUse,
        FallbackPermission, Locality, ModelRouteDecision, PolicyLayer, PolicyLayerContribution,
        PolicyRules, PolicyVersionRef, ProviderRetention, RejectionReason, RequestedDataPolicy,
        ResolvedPolicy, Sensitivity, Telemetry, TrainingUse,
    };
    use crate::ids::ModelDataPolicyId;
    use crate::model::identity::{EndpointClass, ModelId, ModelRef, ProviderId};
    use crate::time::UtcTimestamp;

    fn policy_id() -> ModelDataPolicyId {
        ModelDataPolicyId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d").expect("valid")
    }

    fn model_ref(provider: &str, model: &str) -> ModelRef {
        ModelRef::new(
            ProviderId::parse(provider).expect("valid provider"),
            ModelId::parse(model).expect("valid model"),
        )
    }

    fn contribution(layer: PolicyLayer, rules: PolicyRules) -> PolicyLayerContribution {
        PolicyLayerContribution {
            layer,
            rules,
            policy_version: None,
        }
    }

    #[test]
    fn a_stricter_later_layer_still_narrows_an_earlier_one() {
        // The case a "first deny wins" shortcut gets wrong: the workspace allows
        // cloud, and a *later* task restriction narrows it to local-only.
        let workspace = PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            ..PolicyRules::permissive()
        };
        let task = PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        };
        let resolved = ResolvedPolicy::merge(vec![
            contribution(PolicyLayer::Workspace, workspace),
            contribution(PolicyLayer::TaskRestriction, task),
        ])
        .expect("merging compatible layers");

        assert_eq!(resolved.rules.locality, Locality::LocalOnly);
    }

    #[test]
    fn an_absent_layer_cannot_loosen_a_deny() {
        // Only the administrator layer is present and it denies cloud. Nothing
        // else is supplied, so the fold's permissive identity must not leak.
        let administrator = PolicyRules {
            locality: Locality::LocalOnly,
            allow_fallback: FallbackPermission::Denied,
            ..PolicyRules::permissive()
        };
        let resolved = ResolvedPolicy::merge(vec![contribution(
            PolicyLayer::LegalOrAdministrator,
            administrator,
        )])
        .expect("a single layer merges with the identity");

        assert_eq!(resolved.rules.locality, Locality::LocalOnly);
        assert_eq!(resolved.rules.allow_fallback, FallbackPermission::Denied);
    }

    #[test]
    fn layers_are_merged_strongest_first_whatever_order_they_arrive_in() {
        let strict = PolicyRules {
            maximum_sensitivity: Sensitivity::Internal,
            ..PolicyRules::permissive()
        };
        let weakest = PolicyRules {
            maximum_sensitivity: Sensitivity::Restricted,
            ..PolicyRules::permissive()
        };
        // Supplied weakest-first; the merge must sort by precedence anyway.
        let resolved = ResolvedPolicy::merge(vec![
            contribution(PolicyLayer::ProviderDefault, weakest),
            contribution(PolicyLayer::LegalOrAdministrator, strict),
        ])
        .expect("merging");

        assert_eq!(resolved.rules.maximum_sensitivity, Sensitivity::Internal);
        assert_eq!(
            resolved.layers.first().map(|entry| entry.layer),
            Some(PolicyLayer::LegalOrAdministrator),
        );
    }

    #[test]
    fn an_allow_list_narrows_to_the_intersection() {
        let first = PolicyRules {
            allowed_providers: ["local.ollama".to_owned(), "openai".to_owned()]
                .into_iter()
                .collect(),
            ..PolicyRules::permissive()
        };
        let second = PolicyRules {
            allowed_providers: ["local.ollama".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        };
        let resolved = ResolvedPolicy::merge(vec![
            contribution(PolicyLayer::Workspace, first),
            contribution(PolicyLayer::UserPreference, second),
        ])
        .expect("compatible allow-lists");

        assert_eq!(
            resolved.rules.allowed_providers,
            ["local.ollama".to_owned()].into_iter().collect(),
        );
    }

    #[test]
    fn two_disjoint_allow_lists_are_refused_rather_than_resolved_empty() {
        let first = PolicyRules {
            allowed_providers: ["openai".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        };
        let second = PolicyRules {
            allowed_providers: ["local.ollama".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        };
        let error = ResolvedPolicy::merge(vec![
            contribution(PolicyLayer::Workspace, first),
            contribution(PolicyLayer::TaskRestriction, second),
        ])
        .expect_err("a contradictory policy must be reported");
        assert_eq!(error.code(), "jarvis.invalid_policy_layer");
    }

    #[test]
    fn an_unrestricted_layer_does_not_erase_a_restricted_one() {
        // The empty set means "no restriction at this layer". A `union` or an
        // unchecked `intersection` would turn this into "no providers allowed".
        let restricted = PolicyRules {
            allowed_providers: ["local.ollama".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        };
        let resolved = ResolvedPolicy::merge(vec![
            contribution(PolicyLayer::Workspace, restricted.clone()),
            contribution(PolicyLayer::UserPreference, PolicyRules::permissive()),
        ])
        .expect("an unrestricted layer is not a contradiction");

        assert_eq!(
            resolved.rules.allowed_providers,
            restricted.allowed_providers
        );
    }

    #[test]
    fn narrowing_layers_names_only_the_layers_that_changed_something() {
        let denying = PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        };
        let resolved = ResolvedPolicy::merge(vec![
            contribution(PolicyLayer::LegalOrAdministrator, denying),
            contribution(PolicyLayer::UserPreference, PolicyRules::permissive()),
        ])
        .expect("merging");

        assert_eq!(
            resolved.narrowing_layers(),
            vec![PolicyLayer::LegalOrAdministrator],
            "a permissive layer must not be reported as the reason",
        );
    }

    #[test]
    fn the_effective_policy_has_no_provider_guarantee_field() {
        // Encoding the contract's "provider guarantees are DOCUMENTED or OBSERVED,
        // not JARVIS guarantees" as a serialized-shape assertion: the effective
        // object records classifications, never a promise about the provider.
        let effective = EffectiveDataPolicy {
            model: model_ref("local.ollama", "llama3.1"),
            endpoint_class: EndpointClass::Local,
            retention: EffectiveRetention::NotApplicableLocal,
            training_use: EffectiveTrainingUse::NotApplicableLocal,
            telemetry: Telemetry::Disabled,
            residency: EffectiveResidency::LocalDevice,
        };
        let json = serde_json::to_string(&effective).expect("serializes");
        for forbidden in ["deleted", "guaranteed", "promise", "certified"] {
            assert!(
                !json.contains(forbidden),
                "the effective policy must not claim {forbidden}: {json}",
            );
        }
        assert!(effective.is_self_consistent());
    }

    #[test]
    fn a_cloud_endpoint_cannot_claim_local_classifications() {
        // The inconsistency is invisible otherwise: a cloud route recorded as
        // `not_applicable_local` reads as *more* careful than it is.
        let inconsistent = EffectiveDataPolicy {
            model: model_ref("openai", "gpt-x1"),
            endpoint_class: EndpointClass::ApprovedCloud,
            retention: EffectiveRetention::NotApplicableLocal,
            training_use: EffectiveTrainingUse::DisallowedDocumented,
            telemetry: Telemetry::Disabled,
            residency: EffectiveResidency::Region,
        };
        assert!(
            !inconsistent.is_self_consistent(),
            "a cloud endpoint with a local retention classification must be rejected",
        );
    }

    #[test]
    fn a_local_endpoint_cannot_claim_a_provider_retention_classification() {
        let inconsistent = EffectiveDataPolicy {
            model: model_ref("local.ollama", "llama3.1"),
            endpoint_class: EndpointClass::Local,
            retention: EffectiveRetention::BoundedDocumented,
            training_use: EffectiveTrainingUse::NotApplicableLocal,
            telemetry: Telemetry::LocalOnly,
            residency: EffectiveResidency::LocalDevice,
        };
        assert!(!inconsistent.is_self_consistent());
    }

    #[test]
    fn a_route_decision_that_needed_an_exception_says_so() {
        let decision = ModelRouteDecision {
            policy: PolicyVersionRef {
                policy_id: policy_id(),
                version: 4,
            },
            requested: RequestedDataPolicy {
                locality: Locality::LocalOnly,
                maximum_provider_retention: ProviderRetention::NoneDocumented,
                provider_training_use: TrainingUse::DisallowedDocumented,
                telemetry: Telemetry::Disabled,
                allowed_residency_regions: ["eu".to_owned()].into_iter().collect(),
                sensitivity: Sensitivity::Confidential,
            },
            effective: EffectiveDataPolicy {
                model: model_ref("openai", "gpt-x1"),
                endpoint_class: EndpointClass::ApprovedCloud,
                retention: EffectiveRetention::NoneDocumented,
                training_use: EffectiveTrainingUse::DisallowedDocumented,
                telemetry: Telemetry::Disabled,
                residency: EffectiveResidency::Region,
            },
            evidence_refs: vec!["openai-compatible-model".to_owned()],
            rejected_candidates: vec![super::RejectedCandidate {
                model: model_ref("local.ollama", "llama3.1"),
                reason: RejectionReason::CapabilityUnattested,
            }],
            exception_ref: Some("exception-018f".to_owned()),
            decided_at: UtcTimestamp::parse("2026-09-22T00:00:00Z").expect("valid"),
        };

        assert!(decision.relied_on_exception());
        assert!(decision.effective.is_self_consistent());
        let json = serde_json::to_string(&decision).expect("serializes");
        assert!(json.contains("\"capability_unattested\""), "{json}");
        // The selection is a structured `ModelRef`, so the pair is on the record
        // rather than being re-parsed out of a display string.
        assert!(json.contains("\"provider_id\":\"openai\""), "{json}");
        assert!(json.contains("\"model_id\":\"gpt-x1\""), "{json}");
        assert_eq!(
            decision.effective.model.to_string(),
            "openai/gpt-x1",
            "the display form is for humans, the record is structured",
        );
    }

    #[test]
    fn a_rejected_candidate_keeps_the_same_identity_shape_as_a_selection() {
        // Rejection and selection share `ModelRef`, so a rejected candidate cannot
        // name a provider/model pair that no selection could ever express.
        let rejection = super::RejectedCandidate {
            model: model_ref("local.ollama", "llama3.1"),
            reason: RejectionReason::EvidenceStale,
        };
        let json = serde_json::to_string(&rejection).expect("serializes");
        assert!(json.contains("\"evidence_stale\""), "{json}");
        assert!(json.contains("\"provider_id\":\"local.ollama\""), "{json}");
    }

    #[test]
    fn rejection_reasons_render_distinctly() {
        let reasons = [
            RejectionReason::ProviderNotAllowed,
            RejectionReason::ModelNotAllowed,
            RejectionReason::LocalityViolated,
            RejectionReason::RetentionUnsatisfied,
            RejectionReason::TrainingUseUnsatisfied,
            RejectionReason::ResidencyUnsatisfied,
            RejectionReason::CapabilityUnattested,
            RejectionReason::EvidenceStale,
            RejectionReason::EvidenceMissing,
            RejectionReason::SensitivityExceedsPolicy,
        ];
        let mut rendered: Vec<String> = reasons.iter().map(ToString::to_string).collect();
        rendered.sort();
        rendered.dedup();
        assert_eq!(
            rendered.len(),
            reasons.len(),
            "each reason must render distinctly",
        );
    }

    #[test]
    fn the_layer_order_matches_the_contracts_precedence_list() {
        assert_eq!(
            PolicyLayer::ORDERED,
            [
                PolicyLayer::LegalOrAdministrator,
                PolicyLayer::Workspace,
                PolicyLayer::ResourceSensitivity,
                PolicyLayer::TaskRestriction,
                PolicyLayer::UserPreference,
                PolicyLayer::ProviderDefault,
            ],
        );
    }
}
