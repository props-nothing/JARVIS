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
use crate::model::exception::{PolicyException, PolicyRuleKey};
use crate::model::identity::{EndpointClass, ModelRef, Region};
use crate::model::policy::{
    EffectiveDataPolicy, EffectiveResidency, EffectiveRetention, EffectiveTrainingUse,
    FallbackPermission, Locality, ModelRouteDecision, PolicyRules, PolicyVersionRef,
    ProviderRetention, RejectedCandidate, RejectionReason, RequestedDataPolicy, Sensitivity,
    TrainingUse,
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
    /// The policy exceptions that **may** apply to this call.
    ///
    /// A list rather than one value, and the list is a *permission to consider* rather than a
    /// decision. An exception relaxes one named rule for one candidate scope, and whether any
    /// of them actually applies depends on which rule a candidate failed — so the selector
    /// decides, and this field supplies the candidates it decides among. It is deliberately
    /// **not** a set of pre-relaxed rules: a caller that merged them itself would be the
    /// second implementation of "which rules does this exception relax", and an unselected
    /// exception would silently widen the policy.
    ///
    /// Empty means "no exception was offered", which is the ordinary case. An offered
    /// exception that is expired, revoked, consumed, or scoped to another model is ignored by
    /// the selector as a non-match rather than reported, because the refusal that results is
    /// the truthful answer to "did any exception permit this call".
    pub exceptions: Vec<PolicyException>,
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

    // The policy's sensitivity **ceiling** is checked before any candidate is examined, and
    // that ordering is the rule rather than an optimization. The ceiling bounds what may be
    // sent by *any* route, so content above it has no compliant route even when every
    // candidate is local — whereas a per-candidate retention or locality rejection means
    // "this candidate cannot carry it" and implies another one might. Recording the former as
    // the latter would let an operator conclude that adding a stricter candidate fixes a call
    // that no candidate may carry.
    //
    // This is the `PolicyRules::maximum_sensitivity` field's *only* consumer above the run
    // path: before it was enforced here, a `local_only` policy that permits only `public`
    // content selected a local candidate and sent `restricted` content, because no check
    // compared the content's classification against the ceiling the policy set.
    if request.sensitivity > request.rules.maximum_sensitivity {
        return Err(RouteSelectionFailure::Refused(RouteRefusal {
            // Every candidate is reported with the same reason, because the condition is about
            // the content rather than about any candidate. An empty list would read as "nothing
            // was considered", which is the `CandidatesUnbounded` meaning this must stay
            // distinct from.
            rejected: candidates
                .iter()
                .map(|candidate| RejectedCandidate {
                    model: candidate.model.clone(),
                    reason: RejectionReason::SensitivityExceedsPolicy,
                })
                .collect(),
            requested,
            policy: request.policy,
        }));
    }

    let mut rejected: Vec<RejectedCandidate> = Vec::new();
    for candidate in candidates {
        // The candidate is judged against the **unrelaxed** rules first. An exception is
        // consulted only when the policy refuses, and this ordering is the honest semantics
        // rather than an optimization: `relied_on_exception()` means "this call needed a
        // relaxation", so a candidate that already complies must not record a grant it did not
        // use — and, for a single-use exception, must not *consume* one either. Evaluating
        // relaxed-first would also report a grant as relied on whenever it merely existed.
        match evaluate(candidate, request, &request.rules) {
            Ok(effective) => {
                return Ok(selected(request, effective, rejected, None));
            }
            Err(strict_reason) => {
                // The grant may only relax the rule that actually failed. A locality exception
                // offered to a candidate that failed for retention relaxes nothing useful, and
                // applying it would record a relaxation the call never used.
                let (relaxed, applicable) = rules_for(candidate, request, strict_reason);
                match evaluate(candidate, request, &relaxed) {
                    Ok(effective) => {
                        let reference = applicable.map(|exception| exception.id.to_string());
                        return Ok(selected(request, effective, rejected, reference));
                    }
                    Err(reason) => rejected.push(RejectedCandidate {
                        model: candidate.model.clone(),
                        // The reason from the **relaxed** evaluation, because it is the one that
                        // describes why the call failed after every honour-able grant was
                        // applied. Reporting the strict reason would name a rule an exception
                        // had already relaxed, sending an operator to re-grant what is granted.
                        reason,
                    }),
                }
            }
        }
    }

    Err(RouteSelectionFailure::Refused(RouteRefusal {
        rejected,
        requested,
        policy: request.policy,
    }))
}

/// Builds the decision for one selected candidate.
///
/// One constructor, so the strict and relaxed paths cannot disagree about the parts that are
/// the same for both — the policy reference, the requested side, the rejected candidates
/// considered before this one, and the decision instant.
fn selected(
    request: &RouteRequest,
    effective: EffectiveDataPolicy,
    rejected: Vec<RejectedCandidate>,
    exception_ref: Option<String>,
) -> ModelRouteDecision {
    ModelRouteDecision {
        policy: request.policy,
        requested: requested_of(request),
        effective,
        // Empty rather than fabricated: which evidence notes supported a selection is the
        // caller's to supply, and an invented reference would be worse than an absent one
        // because it would be *believed*.
        evidence_refs: Vec::new(),
        rejected_candidates: rejected,
        // The exception this selection **actually needed**, recorded so the contract's
        // "included in the route decision/audit without exposing content" is satisfiable and
        // `relied_on_exception` has something to read. A candidate that complied without any
        // relaxation records `None`, which is the difference between "a grant permitted this"
        // and "nothing had to be permitted".
        exception_ref,
        decided_at: request.decided_at,
    }
}

/// Returns the rules to judge `candidate` against, and the exception that relaxed one.
///
/// Consulted only after the unrelaxed policy has refused, and `failed` is the rule it refused
/// on — so the grant must name **that** rule. Five conditions, and each closes a way a
/// relaxation could widen past what was granted:
///
/// - **The failed rule must be nameable.** `CapabilityUnattested`, `EvidenceStale`, and
///   `EvidenceMissing` are facts about evidence, not policy: no operator can grant that a
///   capability is verified, so nothing is offered and the refusal stands.
/// - **The exception must be usable at the decision instant.** An expired, revoked, or consumed
///   grant is skipped rather than reported, so a caller that offered a stale exception learns
///   the truth — the call was refused — rather than being told a relaxation applied.
/// - **The exception must cover this model.** A grant scoped to one provider or model relaxes
///   the rule for that one and no other: the contract's "provider/model/task/resource selectors"
///   and "maximum scope".
/// - **The exception must name a waivable rule. A non-waivable one is ignored, never applied.**
///   Ignoring leaves the rules *stricter* than the operator asked for, so it can never permit a
///   call the unrelaxed policy would refuse — whereas refusing the whole evaluation would turn
///   one unusable grant into a refusal of every candidate, including ones that never needed it.
///   The check lives here as well as in `PolicyException::grant` because a record written by a
///   different build, one that considered the rule waivable, must still not be honoured here.
/// - **A grant that names no value for its rule is not applied.** An exception for `locality`
///   with no locality names nothing to permit; applying it by widening to the most permissive
///   value would move local-only data to a cloud endpoint on a grant that never said so.
///
/// The **first** applicable grant is used. A second would relax an already-relaxed rule, and one
/// record keeps the audit answerable: "why was this call permitted" names one durable record.
fn rules_for<'a>(
    candidate: &RouteCandidate,
    request: &'a RouteRequest,
    failed: RejectionReason,
) -> (PolicyRules, Option<&'a PolicyException>) {
    let Some(failed_rule) = failed.policy_rule() else {
        return (request.rules.clone(), None);
    };

    for exception in &request.exceptions {
        if !exception.is_usable_at(request.decided_at)
            || !exception.scope.covers(&candidate.model)
            || !exception.rule.is_waivable()
            || !exception.relaxes(failed_rule)
            || !names_a_relaxation(exception)
        {
            continue;
        }
        let mut rules = request.rules.clone();
        relax_one(&mut rules, exception);
        return (rules, Some(exception));
    }

    (request.rules.clone(), None)
}

/// Returns whether `exception` names a value for the rule it relaxes.
///
/// A grant whose scope carries nothing for its own rule has no relaxation to apply, and the
/// alternative to skipping it — using a default — would be this layer inventing the permission
/// the operator did not state.
fn names_a_relaxation(exception: &PolicyException) -> bool {
    match exception.rule {
        PolicyRuleKey::Locality => exception.scope.locality.is_some(),
        PolicyRuleKey::ResidencyRegions => exception.scope.region.is_some(),
        // These rules are relaxed *to* a named value by the kind of grant itself, so there is no
        // scope field to check: "accept the provider default" and "fallback may be compliant
        // only" are the relaxations, and each is stated by the rule key alone.
        PolicyRuleKey::MaximumProviderRetention
        | PolicyRuleKey::ProviderTrainingUse
        | PolicyRuleKey::AllowFallback => true,
        // Three kinds of grant name no relaxation to apply, and they are one arm because the
        // answer is the same fact rather than by coincidence:
        //
        // - Telemetry's relaxation is a no-op (see `relax_one`), so it never becomes the exception
        //   a call relied on.
        // - The other three are non-waivable, so `rules_for` has already skipped them — the
        //   grouping does not lose that distinction, because `is_waivable` is what `rules_for`
        //   consults and it is asked before this function.
        //
        // Written out rather than left to a `_`, so a rule added to the vocabulary fails to compile
        // here instead of silently inheriting one of these answers.
        PolicyRuleKey::Telemetry
        | PolicyRuleKey::AllowedProviders
        | PolicyRuleKey::AllowedModels
        | PolicyRuleKey::MaximumSensitivity => false,
    }
}

/// Applies `exception`'s relaxation to `rules`.
///
/// The single place a relaxation is written, so "which field does this rule touch, and in which
/// direction" has one answer. Each arm moves the rule toward permitting what the exception
/// names — the only direction an exception exists to move — and the two classification fields
/// are deliberately independent: an exception that relaxes `locality` does **not** touch
/// `maximum_sensitivity`, and one that relaxes the ceiling does not touch locality. Each is set
/// from the exception's own scope field, so relaxing one rule cannot raise another as a side
/// effect.
fn relax_one(rules: &mut PolicyRules, exception: &PolicyException) {
    let scope = &exception.scope;
    match exception.rule {
        PolicyRuleKey::Locality => {
            if let Some(permitted) = scope.locality {
                rules.locality = rules.locality.max(permitted);
            }
        }
        PolicyRuleKey::ResidencyRegions => {
            if let Some(region) = scope.region.as_ref() {
                // The exception's own region is added to the allow-list. An allow-list is a
                // restriction, so *adding* a region narrows it — except when it was empty,
                // which means "no restriction at this layer": inserting into an empty set would
                // silently restrict to that one region. So an empty list stays empty and the
                // grant applies through the per-candidate scope check instead.
                if !rules.allowed_residency_regions.is_empty() {
                    rules.allowed_residency_regions.insert(region.to_string());
                }
            }
        }
        // "Accept the provider's own default terms" is what relaxing a retention or training
        // requirement to its least demanding value means, so the value is set rather than
        // widened from the scope, which has no field for it.
        PolicyRuleKey::MaximumProviderRetention => {
            rules.maximum_provider_retention = ProviderRetention::ProviderDefaultAllowed;
        }
        PolicyRuleKey::ProviderTrainingUse => {
            rules.provider_training_use = TrainingUse::ProviderDefaultAllowed;
        }
        PolicyRuleKey::AllowFallback => {
            rules.allow_fallback = FallbackPermission::CompliantOnly;
        }
        // A no-op by design: `Telemetry`'s two values are `Disabled` and `LocalOnly`, and the
        // permissive one is `LocalOnly` — which `PolicyRules::permissive()` already sets. An
        // exception cannot make JARVIS emit telemetry a policy disabled.
        //
        // Grouped with the non-waivable keys because the body is the same fact: this arm relaxes
        // nothing. The comment above says why for telemetry and the comment below says why for the
        // rest, which is what keeps the grouping from reading as a forgotten case.
        PolicyRuleKey::Telemetry
        | PolicyRuleKey::AllowedProviders
        | PolicyRuleKey::AllowedModels
        | PolicyRuleKey::MaximumSensitivity => {}
    }
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
/// `rules` is the request's `PolicyRules` after any applicable exception has relaxed it, which
/// is why it is passed separately from `request`: the relaxation is per candidate (an exception's
/// scope names a provider and model) while everything else on the request is the call's. Reading
/// `request.rules` here instead would silently ignore every exception and make a granted
/// relaxation inert.
///
/// The order of the checks is the order an operator reads them in: identity before
/// placement, placement before behaviour, behaviour before evidence. A candidate that is
/// not on the allow-list should be reported as such rather than as a residency failure,
/// even when both are true, because the first is the rule the operator set deliberately.
///
/// The policy's sensitivity ceiling is deliberately **not** checked here. It is a fact about
/// the content rather than about a candidate, so it is refused once in
/// [`select_route_explained`] before any candidate is examined; checking it per candidate
/// would report every candidate as equally at fault for a limit none of them can change.
fn evaluate(
    candidate: &RouteCandidate,
    request: &RouteRequest,
    rules: &PolicyRules,
) -> Result<EffectiveDataPolicy, RejectionReason> {
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
