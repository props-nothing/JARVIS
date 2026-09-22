//! Selecting a route: which candidate a call may go to, and why the others may not.
//!
//! `policy.rs` holds the rules and `capability.rs` holds what a candidate can attest;
//! this module is the decision that joins them. It exists because the two halves could
//! not see each other: `RejectionReason` was defined with nothing to produce it and
//! `ModelRouteDecision` with nothing to build it, so the contract's test 7 — "routing
//! rejection rather than silent relaxation" — had vocabulary but no behaviour.
//!
//! Four rules shape the selection, and each is a contract rule rather than a preference:
//!
//! - **A hard rule is checked, never relaxed.** A candidate that cannot satisfy the
//!   resolved policy or attest a required capability is **rejected with a reason**. There
//!   is no path that downgrades a requirement because nothing satisfied it, because
//!   silently relaxing is exactly what the contract forbids and what would move
//!   confidential content to a provider that should not see it.
//! - **A local route cannot claim provider guarantees.** Retention and training use are
//!   cloud concepts, so a `Local` endpoint's effective classification is
//!   `not_applicable_local` — which means the policy's retention and training rules are
//!   **vacuous** for it rather than satisfied by it. That is the honest reading: a local
//!   endpoint has no provider to retain anything, and reporting it as satisfying
//!   `none_documented` would claim documented evidence it does not have.
//! - **Every rejection is recorded, not just the winner.** A decision that named only its
//!   selection could not answer "why not the local model", which is the question an
//!   operator actually asks. The candidate set is bounded so a hostile inventory cannot
//!   make the decision unbounded.
//! - **The decision records the policy version it ran under.** Policy is mutable and a
//!   call outlives the process that made it, so a decision re-derived from current state
//!   could explain a past call with rules that no longer applied.
//!
//! This is deliberately **not** an adapter concern. Selection is deterministic application
//! policy over an inventory, so it lives in the domain where the state machine and the
//! budget live, and an adapter supplies only the inventory and the evidence.

use crate::error::DomainError;
use crate::model::capability::{Attested, Capability, CapabilityDescriptor};
use crate::model::identity::{EndpointClass, ModelRef, Region};
use crate::model::policy::{
    EffectiveDataPolicy, EffectiveResidency, EffectiveRetention, EffectiveTrainingUse, Locality,
    ModelRouteDecision, PolicyRules, PolicyVersionRef, ProviderRetention, RejectedCandidate,
    RejectionReason, RequestedDataPolicy, Sensitivity, TrainingUse,
};
use crate::model::stream::RouteRequirements;
use crate::time::{IsoDate, UtcTimestamp};

/// The most candidates one selection may consider.
///
/// Bounded for the same reason every collection in this workspace is: a selection reads a
/// provider-supplied inventory, and an unbounded one would let a hostile or buggy provider
/// make the decision unbounded. A real gateway considers a handful.
pub const MAX_CANDIDATES: usize = 256;

/// One candidate: what it is, where it runs, and what it can attest.
///
/// The endpoint class is a required field rather than derived from the provider name,
/// because the identification module states the point: `local.ollama` *looks* local and a
/// hosted endpoint reached over a private network may not, so a name-based guess would
/// evaluate a local-only policy incorrectly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteCandidate {
    /// The provider-qualified model.
    pub model: ModelRef,
    /// Where this candidate actually runs.
    pub endpoint_class: EndpointClass,
    /// The residency region this candidate processes in, when it is networked.
    ///
    /// Optional because a local endpoint has no region, and because a networked candidate
    /// whose region the provider does not publish must be able to say "unknown" rather than
    /// claim one. A missing region fails an allow-list check rather than passing it, which is
    /// the only fail-closed direction.
    pub region: Option<Region>,
    /// What this candidate's endpoint can **document** about retention.
    ///
    /// Supplied by the caller from evidence rather than derived from the endpoint class, and
    /// that is the point: whether a provider documents bounded retention is a fact about the
    /// provider's current terms, which no amount of classification can infer. Deriving
    /// `none_documented` from "it is a cloud endpoint" would claim a documented finding from a
    /// category, which is the promotion `model-data-policy.md` forbids. An `Attested` value
    /// carries the evidence that supports it.
    pub retention: Option<Attested<ProviderRetention>>,
    /// What this candidate's endpoint can **document** about training use.
    pub training_use: Option<Attested<TrainingUse>>,
    /// Its attested capabilities and their evidence.
    pub descriptor: CapabilityDescriptor,
}

/// Everything a selection needs besides the candidate list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRequest {
    /// The merged policy rules the call must satisfy.
    pub rules: PolicyRules,
    /// The policy version the rules came from.
    pub policy: PolicyVersionRef,
    /// The sensitivity of the content being sent.
    pub sensitivity: Sensitivity,
    /// The hard capabilities and locality the call requires.
    pub requirements: RouteRequirements,
    /// The day to evaluate evidence freshness against.
    ///
    /// Supplied rather than read from a clock: an evaluation that silently used "now"
    /// could not be reproduced when investigating why a route was refused last week.
    pub today: IsoDate,
    /// The instant the decision was taken.
    ///
    /// Carried on the request rather than passed beside it, because the decision records it
    /// and a caller that had to pass two instants — the evaluation day and the decision
    /// instant — could transpose them without the compiler noticing.
    pub decided_at: UtcTimestamp,
}

/// Why a route could not be selected.
///
/// An enum rather than one struct because the two failures are different facts with
/// different remedies: a refusal means every candidate was evaluated and none complied, while
/// an oversized set means the candidates were never examined at all. Collapsing them would
/// tell a caller "your policy refused every model" when the truth was "you offered too many",
/// and the first would send it to edit a policy that is not the problem. It also keeps the
/// contract's two codes distinct, which a client branching on them depends on.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteSelectionFailure {
    /// Every candidate was evaluated and none satisfied the policy.
    Refused(RouteRefusal),
    /// More candidates were offered than the bound allows, so none was evaluated.
    CandidatesUnbounded {
        /// How many were offered, so an operator can see the scale of the problem.
        offered: usize,
    },
}

/// A refusal: every candidate that was considered, with its reason.
///
/// The reason list is the **actionable half** — "no route" alone leaves a caller unable to
/// act, since the next step differs depending on whether the local model was refused for
/// locality or because nobody offered it. The list is built by the selector and carried back,
/// so there is exactly one implementation of "why was this candidate refused" rather than one
/// that selects and another that explains.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRefusal {
    /// Every candidate that was considered, with the reason it was refused.
    pub rejected: Vec<RejectedCandidate>,
    /// What the policy requested.
    pub requested: RequestedDataPolicy,
    /// The policy version the evaluation ran under.
    pub policy: PolicyVersionRef,
}

/// Selects the candidate a call may go to.
///
/// Candidates are considered in the order supplied, and the **first compliant candidate
/// in that order wins**. Order is the caller's preference — a local model listed first is
/// tried first — while compliance is not negotiable. There is deliberately no scoring: a
/// scoring function could rank an incompliant candidate above a compliant one, and the
/// contract's rule is a filter, not a preference.
///
/// This is the verdict-only view over [`select_route_explained`], which is the one
/// implementation: the two cannot disagree about *whether* a route exists, and a caller that
/// needs to report *why* uses the explained form rather than a second selection.
///
/// # Errors
///
/// Returns [`DomainError::PolicyUnsatisfied`] when no candidate satisfies every hard rule.
/// That is a refusal rather than an empty selection, because "no route" is a decision the
/// caller must act on, and returning an empty list would let it proceed with none. The code
/// is the contract's existing `model.policy_unsatisfied` rather than a new one, because a
/// client branching on it must not learn a second spelling for the same condition.
///
/// Returns [`DomainError::ContextCandidatesUnbounded`] when more than [`MAX_CANDIDATES`]
/// are offered. Reusing the context module's code is deliberate: the condition is "too many
/// candidates", its remedy is identical, and a parallel code would be a second spelling a
/// client would have to learn for no added meaning.
pub fn select_route(
    candidates: &[RouteCandidate],
    request: &RouteRequest,
) -> Result<ModelRouteDecision, DomainError> {
    select_route_explained(candidates, request).map_err(|failure| match failure {
        RouteSelectionFailure::Refused(_) => DomainError::PolicyUnsatisfied,
        RouteSelectionFailure::CandidatesUnbounded { .. } => {
            DomainError::ContextCandidatesUnbounded
        }
    })
}

/// Selects a route, returning the structured failure when nothing complies.
///
/// # Errors
///
/// Returns [`RouteSelectionFailure::Refused`] with every rejected candidate and its reason
/// when all candidates were evaluated and none complied, and
/// [`RouteSelectionFailure::CandidatesUnbounded`] when the set was too large to examine.
pub fn select_route_explained(
    candidates: &[RouteCandidate],
    request: &RouteRequest,
) -> Result<ModelRouteDecision, RouteSelectionFailure> {
    if candidates.len() > MAX_CANDIDATES {
        return Err(RouteSelectionFailure::CandidatesUnbounded {
            offered: candidates.len(),
        });
    }

    let requested = requested_of(request);
    let mut rejected: Vec<RejectedCandidate> = Vec::new();
    for candidate in candidates {
        match evaluate(candidate, request) {
            Ok(effective) => {
                return Ok(ModelRouteDecision {
                    policy: request.policy,
                    requested,
                    effective,
                    // Empty rather than fabricated: which evidence notes supported a
                    // selection is the caller's to supply, and an invented reference would
                    // be worse than an absent one because it would be *believed*.
                    evidence_refs: Vec::new(),
                    rejected_candidates: rejected,
                    exception_ref: None,
                    decided_at: request.decided_at,
                });
            }
            Err(reason) => rejected.push(RejectedCandidate {
                model: candidate.model.clone(),
                reason,
            }),
        }
    }

    Err(RouteSelectionFailure::Refused(RouteRefusal {
        rejected,
        requested,
        policy: request.policy,
    }))
}

/// Builds the requested side of a decision from a request.
fn requested_of(request: &RouteRequest) -> RequestedDataPolicy {
    RequestedDataPolicy {
        locality: request.rules.locality,
        maximum_provider_retention: request.rules.maximum_provider_retention,
        provider_training_use: request.rules.provider_training_use,
        telemetry: request.rules.telemetry,
        allowed_residency_regions: request.rules.allowed_residency_regions.clone(),
        // The **content's** sensitivity, not the policy's ceiling. The ceiling is an upper
        // bound on what may be sent, and recording it here as though it were the call's own
        // classification would say every call carries maximally sensitive content.
        sensitivity: request.sensitivity,
    }
}

/// Returns whether `candidate` satisfies every hard rule, or the first reason it does not.
///
/// The order of the checks is the order an operator reads them in: identity before
/// placement, placement before behaviour, behaviour before evidence. A candidate that is
/// not on the allow-list should be reported as such rather than as a residency failure,
/// even when both are true, because the first is the rule the operator set deliberately.
fn evaluate(
    candidate: &RouteCandidate,
    request: &RouteRequest,
) -> Result<EffectiveDataPolicy, RejectionReason> {
    let rules = &request.rules;

    if !rules.allowed_providers.is_empty()
        && !rules
            .allowed_providers
            .contains(candidate.model.provider_id.as_str())
    {
        return Err(RejectionReason::ProviderNotAllowed);
    }
    if !rules.allowed_models.is_empty()
        && !rules
            .allowed_models
            .contains(candidate.model.model_id.as_str())
    {
        return Err(RejectionReason::ModelNotAllowed);
    }

    let locality = effective_locality(candidate.endpoint_class);
    if !satisfies_locality(rules.locality, locality) {
        return Err(RejectionReason::LocalityViolated);
    }
    if !satisfies_required_locality(request.requirements.locality, locality) {
        return Err(RejectionReason::LocalityViolated);
    }

    let retention = effective_retention(candidate, request.today);
    if !satisfies_retention(rules.maximum_provider_retention, retention) {
        return Err(RejectionReason::RetentionUnsatisfied);
    }
    let training_use = effective_training_use(candidate, request.today);
    if !satisfies_training_use(rules.provider_training_use, training_use) {
        return Err(RejectionReason::TrainingUseUnsatisfied);
    }

    // Residency is checked next, because it is a placement rule like locality rather than a
    // behavioural one. A networked candidate with no named region cannot be shown to be
    // inside an allowed one, so an allow-list that is set and does not cover it is a
    // rejection — refusing is the only fail-closed answer when the region is unknown.
    if !rules.allowed_residency_regions.is_empty() {
        let permitted = candidate
            .region
            .as_ref()
            .is_some_and(|region| rules.allowed_residency_regions.contains(region.as_str()));
        if !permitted {
            return Err(RejectionReason::ResidencyUnsatisfied);
        }
    }

    // Every required capability must be attested by fresh, hard-requirement evidence. A
    // capability that is absent, expired, inferred, or unverified is a rejection rather
    // than a downgrade, which is the contract's "provider claims cannot be promoted into
    // routing capabilities without current evidence".
    for capability in &request.requirements.required_capabilities {
        if !candidate.descriptor.supports_on(*capability, request.today) {
            return Err(rejection_for_absent_capability(
                candidate,
                *capability,
                request.today,
            ));
        }
    }

    // The incremental-delivery requirement is checked *twice*, and the second check is the
    // one this project's own `BRN-011` exists for: a candidate whose measurement is fresh
    // but shows a single burst supports the capability key while failing the requirement's
    // intent. `incremental_delivery_on` is the predicate that keeps "streams" from passing
    // as "delivers incrementally" — `Ok(None)` means no usable measurement (already rejected
    // above), and `Err(DeliveryNotIncremental)` means the measurement exists and says burst.
    if request.requirements.requires_incremental_delivery()
        && matches!(
            candidate.descriptor.incremental_delivery_on(request.today),
            Err(DomainError::DeliveryNotIncremental)
        )
    {
        return Err(RejectionReason::CapabilityUnattested);
    }

    Ok(EffectiveDataPolicy {
        model: candidate.model.clone(),
        endpoint_class: candidate.endpoint_class,
        retention,
        training_use,
        telemetry: rules.telemetry,
        residency: effective_residency(candidate),
    })
}

/// Returns why a required capability is absent.
///
/// Three answers rather than one, because the operator's next action differs: an absent
/// descriptor means nobody measured the model, a stale one means the measurement needs
/// revalidating, and a present-but-unusable one means the model does not support it.
fn rejection_for_absent_capability(
    candidate: &RouteCandidate,
    capability: Capability,
    today: IsoDate,
) -> RejectionReason {
    match candidate.descriptor.capability(capability) {
        None => RejectionReason::EvidenceMissing,
        Some(attested) if !attested.evidence.is_fresh_on(today) => RejectionReason::EvidenceStale,
        Some(_) => RejectionReason::CapabilityUnattested,
    }
}

/// The locality a candidate's endpoint class actually provides.
///
/// Derived from the class rather than the name, because a provider id can look local
/// without being local.
#[must_use]
const fn effective_locality(endpoint_class: EndpointClass) -> Locality {
    match endpoint_class {
        EndpointClass::Local => Locality::LocalOnly,
        EndpointClass::PrivateNetwork => Locality::PrivateNetworkAllowed,
        EndpointClass::ApprovedCloud => Locality::ApprovedCloudAllowed,
    }
}

/// Returns whether a candidate's locality is inside the policy's ceiling.
///
/// The policy's value is the **most** a call may use, so a candidate at or below it
/// complies: a policy of `PrivateNetworkAllowed` permits a local candidate, and refusing
/// it would make the strictest option unavailable under a permissive policy.
#[must_use]
const fn satisfies_locality(ceiling: Locality, actual: Locality) -> bool {
    // Ordered most-restrictive first, so a smaller discriminant is stricter and "at or
    // below the ceiling" is `actual <= ceiling`.
    matches!(
        (ceiling, actual),
        (Locality::LocalOnly, Locality::LocalOnly)
            | (
                Locality::PrivateNetworkAllowed,
                Locality::LocalOnly | Locality::PrivateNetworkAllowed
            )
            | (
                Locality::ApprovedCloudAllowed,
                Locality::LocalOnly
                    | Locality::PrivateNetworkAllowed
                    | Locality::ApprovedCloudAllowed
            )
    )
}

/// Returns whether a candidate's locality satisfies a requirement that a specific class is
/// *required*.
///
/// A requirement is a floor rather than a ceiling: `requirements.locality` of
/// `LocalOnly` means the call must not leave the device.
#[must_use]
const fn satisfies_required_locality(required: Locality, actual: Locality) -> bool {
    matches!(
        (required, actual),
        (Locality::LocalOnly, Locality::LocalOnly)
            | (
                Locality::PrivateNetworkAllowed,
                Locality::LocalOnly | Locality::PrivateNetworkAllowed
            )
            | (
                Locality::ApprovedCloudAllowed,
                Locality::LocalOnly
                    | Locality::PrivateNetworkAllowed
                    | Locality::ApprovedCloudAllowed
            )
    )
}

/// The retention classification of a candidate.
///
/// A local endpoint has no provider to retain anything, so the answer is
/// `not_applicable_local` regardless of what any evidence says — a provider retention claim
/// about a local runtime would be a category error. Otherwise the answer comes from the
/// candidate's own **fresh** evidence, and with no usable evidence it is
/// `provider_default`: what the provider does by default was accepted, which documents
/// nothing. Returning `none_documented` there would claim a documented finding of "no
/// retention" from the absence of a note, which is the promotion the contract forbids and
/// would make an undocumented route look like the most careful kind.
#[must_use]
fn effective_retention(candidate: &RouteCandidate, today: IsoDate) -> EffectiveRetention {
    if matches!(candidate.endpoint_class, EndpointClass::Local) {
        return EffectiveRetention::NotApplicableLocal;
    }
    match candidate.retention.as_ref() {
        Some(attested) if attested.evidence.satisfies_data_policy_rule_on(today) => {
            match attested.value {
                ProviderRetention::NoneDocumented => EffectiveRetention::NoneDocumented,
                ProviderRetention::BoundedDocumented => EffectiveRetention::BoundedDocumented,
                ProviderRetention::ProviderDefaultAllowed => EffectiveRetention::ProviderDefault,
            }
        }
        Some(_) | None => EffectiveRetention::ProviderDefault,
    }
}

/// The training-use classification of a candidate.
///
/// The same shape as [`effective_retention`], and the same reason: `disallowed_documented` is
/// a claim about current evidence, so it cannot be inferred from the endpoint class. Without
/// usable evidence the answer is the account's own policy governing, which is what
/// `account_policy` states — never `disallowed_documented`, which would assert a documented
/// prohibition nobody recorded.
#[must_use]
fn effective_training_use(candidate: &RouteCandidate, today: IsoDate) -> EffectiveTrainingUse {
    if matches!(candidate.endpoint_class, EndpointClass::Local) {
        return EffectiveTrainingUse::NotApplicableLocal;
    }
    match candidate.training_use.as_ref() {
        Some(attested) if attested.evidence.satisfies_data_policy_rule_on(today) => {
            match attested.value {
                TrainingUse::DisallowedDocumented | TrainingUse::AccountPolicyAllowed => {
                    EffectiveTrainingUse::DisallowedDocumented
                }
                TrainingUse::ProviderDefaultAllowed => EffectiveTrainingUse::AccountPolicy,
            }
        }
        Some(_) | None => EffectiveTrainingUse::AccountPolicy,
    }
}

/// Returns whether a retention classification satisfies the required behavior.
///
/// A requirement is a floor over documentation strength, ordered strongest first:
/// `NoneDocumented` demands the strongest statement, so it refuses a route that can only
/// document "bounded". `ProviderDefaultAllowed` accepts anything, because the caller stated
/// the provider's own default is acceptable — which is what makes `provider_default`
/// admissible *here* and only here.
#[must_use]
const fn satisfies_retention(required: ProviderRetention, actual: EffectiveRetention) -> bool {
    match required {
        ProviderRetention::ProviderDefaultAllowed => true,
        // A local endpoint has no provider retention to document, so it cannot be refused for
        // failing to document one.
        ProviderRetention::NoneDocumented => matches!(
            actual,
            EffectiveRetention::NotApplicableLocal | EffectiveRetention::NoneDocumented
        ),
        ProviderRetention::BoundedDocumented => matches!(
            actual,
            EffectiveRetention::NotApplicableLocal
                | EffectiveRetention::NoneDocumented
                | EffectiveRetention::BoundedDocumented
        ),
    }
}

/// Returns whether a training-use classification satisfies the required behavior.
#[must_use]
const fn satisfies_training_use(required: TrainingUse, actual: EffectiveTrainingUse) -> bool {
    match required {
        TrainingUse::ProviderDefaultAllowed => true,
        TrainingUse::DisallowedDocumented | TrainingUse::AccountPolicyAllowed => matches!(
            actual,
            EffectiveTrainingUse::NotApplicableLocal | EffectiveTrainingUse::DisallowedDocumented
        ),
    }
}

/// The residency a candidate provides.
///
/// A local endpoint processes on the device, which is its own residency rather than a
/// region, so it reports `local_device` for the same reason its retention is
/// `not_applicable_local`. A networked candidate reports the region it actually named —
/// read from the **candidate**, not from the policy: the policy's own region list was
/// already used to reject a candidate outside it, and reporting the first allowed region
/// here would claim the call runs somewhere it might not.
#[must_use]
fn effective_residency(candidate: &RouteCandidate) -> EffectiveResidency {
    match candidate.endpoint_class {
        EndpointClass::Local => EffectiveResidency::LocalDevice,
        EndpointClass::PrivateNetwork | EndpointClass::ApprovedCloud => EffectiveResidency::Region,
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
