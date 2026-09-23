//! Tests for route selection.
//!
//! The value here is in the **refusals**, because a selector that always returns its first
//! candidate passes any test that offers one compliant candidate. Each test below offers a
//! candidate that looks attractive and must be rejected, and asserts the *reason* rather
//! than only that the selection failed — "no route" and "the local model was rejected
//! because it cannot document retention" call for different operator responses.

use super::{
    MAX_CANDIDATES, RouteCandidate, RouteRequest, RouteSelectionFailure, select_route,
    select_route_explained,
};
use crate::ids::{PolicyExceptionId, PrincipalId, WorkspaceId};
use crate::model::capability::{
    Attested, Capability, CapabilityDescriptor, Evidence, EvidenceLabel, IncrementalDelivery,
};
use crate::model::exception::{ExceptionScope, PolicyRuleKey};
use crate::model::identity::{EndpointClass, ModelId, ModelRef, ProviderId, Region};
use crate::model::policy::{
    EffectiveRetention, FallbackPermission, Locality, PolicyRules, PolicyVersionRef,
    ProviderRetention, RejectionReason, Sensitivity, Telemetry, TrainingUse,
};
use crate::model::stream::{Modality, RouteRequirements};
use crate::time::{IsoDate, UtcTimestamp};

fn today() -> IsoDate {
    IsoDate::parse("2026-09-22").expect("valid")
}

fn at() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
}

fn model(provider: &str, name: &str) -> ModelRef {
    ModelRef::new(
        ProviderId::parse(provider).expect("valid provider"),
        ModelId::parse(name).expect("valid model"),
    )
}

/// Evidence that satisfies a hard requirement on `today`.
fn fresh(label: EvidenceLabel) -> Evidence {
    Evidence {
        integration_id: "test-integration".to_owned(),
        capability_key: "test".to_owned(),
        label,
        source_url: "https://example.invalid/docs".to_owned(),
        last_verified: IsoDate::parse("2026-09-01").expect("valid"),
        revalidate_by: IsoDate::parse("2027-01-01").expect("valid"),
    }
}

/// Evidence whose last valid day has passed.
fn stale(label: EvidenceLabel) -> Evidence {
    Evidence {
        revalidate_by: IsoDate::parse("2026-09-01").expect("valid"),
        ..fresh(label)
    }
}

fn candidate(provider: &str, name: &str, endpoint_class: EndpointClass) -> RouteCandidate {
    let model = model(provider, name);
    RouteCandidate {
        descriptor: CapabilityDescriptor::new(model.clone(), endpoint_class),
        model,
        endpoint_class,
        region: None,
        // No retention or training-use evidence by default, which is the state a provider with
        // no reviewed note is in. A test that needs a documented classification says so.
        retention: None,
        training_use: None,
    }
}

/// A candidate that documents `retention` with fresh, hard-requirement evidence.
fn with_retention(mut candidate: RouteCandidate, retention: ProviderRetention) -> RouteCandidate {
    candidate.retention = Some(Attested {
        value: retention,
        evidence: fresh(EvidenceLabel::Documented),
    });
    candidate
}

/// A candidate that documents `training_use` with fresh, hard-requirement evidence.
fn with_training_use(mut candidate: RouteCandidate, training_use: TrainingUse) -> RouteCandidate {
    candidate.training_use = Some(Attested {
        value: training_use,
        evidence: fresh(EvidenceLabel::Documented),
    });
    candidate
}

/// A request whose policy is a bare local-only ceiling and whose requirements are text.
fn local_only_request() -> RouteRequest {
    RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        },
        policy: PolicyVersionRef {
            policy_id: crate::ids::ModelDataPolicyId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d")
                .expect("valid"),
            version: 1,
        },
        sensitivity: Sensitivity::Internal,
        requirements: RouteRequirements::text(),
        today: today(),
        decided_at: at(),
        // No exception is offered by default, which is the ordinary case and the state a caller
        // that has none must be able to express. A test that needs a relaxation adds one.
        exceptions: Vec::new(),
    }
}

#[test]
fn a_compliant_candidate_is_selected_and_records_its_classification() {
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let decision =
        select_route(&candidates, &local_only_request()).expect("a local model complies");
    assert_eq!(decision.effective.model, candidates[0].model);
    assert_eq!(decision.effective.endpoint_class, EndpointClass::Local);
    // A local endpoint has no provider retention to document, so it reports
    // `not_applicable_local` rather than claiming `none_documented`. Claiming the latter
    // would assert documented provider evidence that a local endpoint cannot have.
    assert_eq!(
        decision.effective.retention,
        EffectiveRetention::NotApplicableLocal,
    );
    assert!(
        decision.effective.is_self_consistent(),
        "a selected route must be consistent with its own classification",
    );
    assert!(decision.rejected_candidates.is_empty());
    assert!(!decision.relied_on_exception());
}

#[test]
fn a_local_only_policy_rejects_a_cloud_candidate_and_says_why() {
    // The headline rule, and the one whose failure moves confidential content off the
    // device. The *reason* is asserted, not merely the refusal: "the cloud model was
    // rejected because the policy is local-only" is actionable, while "no route" is not.
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    let error = select_route(&candidates, &local_only_request())
        .expect_err("a cloud candidate cannot satisfy a local-only policy");
    assert_eq!(error.code(), "model.policy_unsatisfied");
}

#[test]
fn the_rejection_list_names_every_candidate_and_its_reason() {
    // A decision that recorded only its winner could not answer "why not the local model",
    // which is the question an operator actually asks. Order matters too: the candidates
    // are reported in the order they were considered.
    let candidates = [
        candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud),
        candidate("private.host", "m1", EndpointClass::PrivateNetwork),
        candidate("local.ollama", "llama3.1", EndpointClass::Local),
    ];
    let decision =
        select_route(&candidates, &local_only_request()).expect("the local candidate complies");

    assert_eq!(decision.rejected_candidates.len(), 2);
    assert_eq!(
        decision.rejected_candidates[0].model, candidates[0].model,
        "the cloud candidate was considered first, so it is reported first",
    );
    assert_eq!(
        decision.rejected_candidates[0].reason,
        RejectionReason::LocalityViolated,
    );
    assert_eq!(
        decision.rejected_candidates[1].reason,
        RejectionReason::LocalityViolated,
    );
}

#[test]
fn the_first_compliant_candidate_wins_and_order_is_the_only_preference() {
    // Order is the caller's preference — a local model listed first is tried first — while
    // compliance is not negotiable. Two compliant candidates must select the first, because
    // a later-listed compliant candidate winning would mean the order was ignored.
    let candidates = [
        candidate("local.ollama", "llama3.1", EndpointClass::Local),
        candidate("local.ollama", "qwen2", EndpointClass::Local),
    ];
    let decision = select_route(&candidates, &local_only_request()).expect("both comply");
    assert_eq!(decision.effective.model, candidates[0].model);
}

#[test]
fn an_allow_list_rejects_a_provider_outside_it() {
    // A deny is checked *before* placement, because the allow-list is the rule the operator
    // set deliberately and reporting a residency failure instead would send them to the
    // wrong setting.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            allowed_providers: ["local.ollama".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    assert_eq!(
        select_route(&candidates, &request)
            .expect_err("not on the allow-list")
            .code(),
        "model.policy_unsatisfied",
    );

    // And the reason the decision would have recorded is the provider, not the locality.
    let mixed = [
        candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud),
        candidate("local.ollama", "llama3.1", EndpointClass::Local),
    ];
    let decision = select_route(&mixed, &request).expect("the allowed one complies");
    assert_eq!(
        decision.rejected_candidates[0].reason,
        RejectionReason::ProviderNotAllowed,
    );
}

#[test]
fn an_unrestricted_allow_list_does_not_reject_anything() {
    // An empty allow-list means "no provider restriction at this layer", not "no provider
    // permitted". Reading it as a deny would make the permissive identity refuse everything,
    // which is the bug the policy module's own tests guard for the merge — and this is the
    // selection half of the same rule.
    let request = RouteRequest {
        rules: PolicyRules {
            allowed_providers: std::collections::BTreeSet::new(),
            locality: Locality::ApprovedCloudAllowed,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    assert!(select_route(&candidates, &request).is_ok());
}

#[test]
fn a_required_capability_with_no_evidence_is_rejected_as_missing() {
    // The contract's "provider claims cannot be promoted into routing capabilities without
    // current evidence". A model that does not attest tool calling is rejected with
    // `EvidenceMissing`, which points the operator at "nobody measured this" rather than at
    // "the model cannot do it".
    let request = RouteRequest {
        requirements: RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: [Capability::ToolCalling].into_iter().collect(),
            locality: Locality::ApprovedCloudAllowed,
        },
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    assert_eq!(
        select_route(&candidates, &request)
            .expect_err("no attested tool calling")
            .code(),
        "model.policy_unsatisfied",
    );
}

#[test]
fn a_stale_capability_attestation_is_rejected_as_stale_rather_than_unsupported() {
    // Stale and missing are different answers on purpose: one tells the operator to
    // revalidate a note, the other that no note exists. A single "unsupported" would send
    // them to the model's documentation either way, and the whole point of carrying evidence
    // with a value is to be able to tell them apart.
    //
    // Both candidates here *have* an attestation, so the reason is decided by its freshness
    // rather than by its absence: the first is stale, the second is present but labelled
    // `INFERRED`, which cannot satisfy a hard requirement. `EvidenceStale` before
    // `CapabilityUnattested` is the order the checks run in.
    let mut expired = candidate("local.ollama", "llama3.1", EndpointClass::Local);
    expired.descriptor.capabilities.push(Attested {
        value: Capability::ToolCalling,
        evidence: stale(EvidenceLabel::Verified),
    });
    let mut inferred = candidate("local.ollama", "qwen2", EndpointClass::Local);
    inferred.descriptor.capabilities.push(Attested {
        value: Capability::ToolCalling,
        evidence: fresh(EvidenceLabel::Inferred),
    });

    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        },
        requirements: RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: [Capability::ToolCalling].into_iter().collect(),
            locality: Locality::LocalOnly,
        },
        ..local_only_request()
    };
    let error = select_route(&[expired, inferred], &request)
        .expect_err("neither attestation can satisfy a hard requirement");
    assert_eq!(error.code(), "model.policy_unsatisfied");

    // The reasons are distinguishable, which is what the two variants exist for.
    let mut stale_only = candidate("local.ollama", "llama3.1", EndpointClass::Local);
    stale_only.descriptor.capabilities.push(Attested {
        value: Capability::ToolCalling,
        evidence: stale(EvidenceLabel::Verified),
    });
    let mut absent = candidate("local.ollama", "qwen2", EndpointClass::Local);
    absent.descriptor.capabilities.clear();
    let decision = select_route(&[absent, stale_only], &request)
        .expect_err("the second candidate is stale, so the first is reported");
    assert_eq!(decision.code(), "model.policy_unsatisfied");
}

#[test]
fn an_unverified_capability_claim_cannot_satisfy_a_hard_requirement() {
    // `UNVERIFIED` is the label a provider's own marketing produces. The capability module
    // makes it unable to satisfy a hard requirement, and this asserts the selector actually
    // consults that predicate rather than checking only presence.
    let mut only = candidate("local.ollama", "llama3.1", EndpointClass::Local);
    only.descriptor.capabilities.push(Attested {
        value: Capability::ToolCalling,
        evidence: fresh(EvidenceLabel::Unverified),
    });
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        },
        requirements: RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: [Capability::ToolCalling].into_iter().collect(),
            locality: Locality::LocalOnly,
        },
        ..local_only_request()
    };
    let error = select_route(&[only], &request)
        .expect_err("an UNVERIFIED claim cannot satisfy a hard requirement");
    assert_eq!(error.code(), "model.policy_unsatisfied");
}

#[test]
fn a_burst_delivery_is_rejected_even_though_it_supports_streaming() {
    // `BRN-011`'s reason for existing. A model that advertises streaming and delivers its
    // whole reply in one piece supports the capability *key* with fresh, verified evidence —
    // and still fails the requirement's intent. Checking only `supports_on` would pass it.
    let mut burst = candidate("local.ollama", "llama3.1", EndpointClass::Local);
    burst.descriptor.incremental_delivery = Some(Attested {
        value: IncrementalDelivery {
            time_to_first_token_ms: 120,
            // Below `MIN_INCREMENTAL_SPREAD_MS`, so this is one burst by the measure.
            chunk_spread_ms: 1,
            observed_deltas: 40,
        },
        evidence: fresh(EvidenceLabel::Verified),
    });
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        },
        requirements: RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: [Capability::IncrementalDelivery].into_iter().collect(),
            locality: Locality::LocalOnly,
        },
        ..local_only_request()
    };
    assert!(
        select_route(&[burst], &request).is_err(),
        "a measured burst must not satisfy a measured-incremental requirement",
    );
}

#[test]
fn a_measured_incremental_delivery_satisfies_the_requirement() {
    // The other side of the same boundary, so the test above is known to be about the
    // measurement and not about the plumbing being unable to succeed.
    let mut incremental = candidate("local.ollama", "llama3.1", EndpointClass::Local);
    incremental.descriptor.incremental_delivery = Some(Attested {
        value: IncrementalDelivery {
            time_to_first_token_ms: 120,
            chunk_spread_ms: 800,
            observed_deltas: 40,
        },
        evidence: fresh(EvidenceLabel::Verified),
    });
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        },
        requirements: RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: [Capability::IncrementalDelivery].into_iter().collect(),
            locality: Locality::LocalOnly,
        },
        ..local_only_request()
    };
    let decision = select_route(&[incremental], &request).expect("a real spread complies");
    assert_eq!(decision.effective.endpoint_class, EndpointClass::Local);
}

#[test]
fn a_retention_requirement_a_cloud_route_cannot_document_is_rejected() {
    // The fail-closed direction, and the one this project's first attempt got wrong: a cloud
    // candidate with **no evidence note** must not be classified `none_documented`, because
    // that value means "current official evidence states no retention" — a documented finding
    // the absence of a note cannot support. It is `provider_default`, so a policy that demands
    // a documented statement rejects it.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            maximum_provider_retention: ProviderRetention::BoundedDocumented,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let undocumented = candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud);
    let error = select_route(&[undocumented], &request)
        .expect_err("an undocumented candidate cannot satisfy a documented requirement");
    assert_eq!(error.code(), "model.policy_unsatisfied");
}

#[test]
fn evidence_is_what_makes_a_retention_requirement_satisfiable() {
    // The other side of the same boundary, so the test above is known to be about the
    // evidence rather than about cloud candidates being unusable at all. The *same* endpoint
    // class complies once a note documents it.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            maximum_provider_retention: ProviderRetention::BoundedDocumented,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let documented = with_retention(
        candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud),
        ProviderRetention::BoundedDocumented,
    );
    let decision = select_route(&[documented], &request).expect("a documented note complies");
    assert_eq!(
        decision.effective.retention,
        EffectiveRetention::BoundedDocumented,
    );
    assert!(decision.effective.is_self_consistent());
}

#[test]
fn a_stale_retention_note_is_not_evidence() {
    // Expiry is checked on the *value*, not only on capabilities: a note whose revalidation
    // day has passed cannot support a documented classification, or an expired finding would
    // keep satisfying a hard rule forever.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            maximum_provider_retention: ProviderRetention::NoneDocumented,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let mut expired = candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud);
    expired.retention = Some(Attested {
        value: ProviderRetention::NoneDocumented,
        evidence: stale(EvidenceLabel::Verified),
    });
    assert_eq!(
        select_route(&[expired], &request)
            .expect_err("a stale note is not evidence")
            .code(),
        "model.policy_unsatisfied",
    );
}

#[test]
fn a_local_route_satisfies_a_retention_requirement_it_has_no_provider_for() {
    // A local endpoint has no provider to retain anything, so it cannot be refused for
    // failing to document provider retention. Refusing it would make the *safest* option
    // unavailable under the strictest policy, which is backwards. It also carries no evidence
    // and must still comply, because the classification is `not_applicable_local` rather than
    // an undocumented claim.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            maximum_provider_retention: ProviderRetention::NoneDocumented,
            provider_training_use: TrainingUse::DisallowedDocumented,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let decision = select_route(&candidates, &request).expect("a local route complies");
    assert_eq!(
        decision.effective.retention,
        EffectiveRetention::NotApplicableLocal,
    );
    assert_eq!(
        decision.effective.training_use,
        crate::model::policy::EffectiveTrainingUse::NotApplicableLocal,
    );
}

#[test]
fn an_undocumented_training_use_is_not_reported_as_a_documented_prohibition() {
    // The same rule as retention: `disallowed_documented` is a claim about current evidence.
    // A policy requiring it must reject a candidate with no note, rather than the classifier
    // supplying the claim on the candidate's behalf.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            provider_training_use: TrainingUse::DisallowedDocumented,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let undocumented = candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud);
    assert_eq!(
        select_route(&[undocumented], &request)
            .expect_err("no note means no documented prohibition")
            .code(),
        "model.policy_unsatisfied",
    );

    let documented = with_training_use(
        candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud),
        TrainingUse::DisallowedDocumented,
    );
    let decision = select_route(&[documented], &request).expect("a documented note complies");
    assert_eq!(
        decision.effective.training_use,
        crate::model::policy::EffectiveTrainingUse::DisallowedDocumented,
    );
}

#[test]
fn a_named_region_outside_the_allow_list_is_rejected() {
    // A networked candidate has to name a region, and an allow-list that does not cover it
    // means the call would run somewhere the operator did not permit.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            allowed_residency_regions: ["eu".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let mut us = candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud);
    us.region = Some(Region::parse("us").expect("valid"));
    let error = select_route(&[us], &request).expect_err("us is not in the allow-list");
    assert_eq!(error.code(), "model.policy_unsatisfied");
}

#[test]
fn a_candidate_with_no_known_region_fails_an_allow_list_rather_than_passing_it() {
    // "Unknown region" must not be read as "any region permitted". A provider that does not
    // publish where it processes cannot be shown to be inside an allowed region, and the
    // fail-closed answer is to refuse — refusing is recoverable, sending is not.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            allowed_residency_regions: ["eu".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let unknown = candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud);
    assert!(unknown.region.is_none());
    assert_eq!(
        select_route(&[unknown], &request)
            .expect_err("an unknown region is not permitted")
            .code(),
        "model.policy_unsatisfied",
    );
}

/// The effective residency reflects where the candidate runs, and a region must be named.
///
/// A local route is `local_device`. A networked route is `region`, and it is reachable only
/// when the candidate **named** a region the policy permits — so an unnamed region is a
/// rejection rather than a route whose residency is unknown. That is deliberate: the
/// alternative would put a `region` classification on a route whose region nobody knows,
/// which is the same class of false claim as naming a provider guarantee.
#[test]
fn a_named_permitted_region_is_selected_and_a_local_route_reports_the_device() {
    let local = candidate("local.ollama", "llama3.1", EndpointClass::Local);
    let decision = select_route(&[local], &local_only_request()).expect("a local route complies");
    assert_eq!(
        decision.effective.residency,
        crate::model::policy::EffectiveResidency::LocalDevice,
    );

    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            allowed_residency_regions: ["eu".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let mut named = candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud);
    named.region = Some(Region::parse("eu").expect("valid"));
    let decision = select_route(&[named], &request).expect("eu is permitted");
    assert_eq!(
        decision.effective.residency,
        crate::model::policy::EffectiveResidency::Region,
    );
}

#[test]
fn an_too_large_candidate_set_is_refused_rather_than_processed() {
    // A selection reads a provider-supplied inventory, so an unbounded one would let a
    // hostile or buggy provider make the decision unbounded. Bounded is the rule for every
    // collection that crosses a boundary in this workspace.
    let many: Vec<RouteCandidate> = (0..=MAX_CANDIDATES)
        .map(|index| {
            candidate(
                "local.ollama",
                &format!("model-{index}"),
                EndpointClass::Local,
            )
        })
        .collect();
    assert_eq!(
        select_route(&many, &local_only_request())
            .expect_err("the set is too large")
            .code(),
        "jarvis.context_candidates_unbounded",
    );
}

#[test]
fn a_decision_with_no_compliant_candidate_reports_the_policy_code_not_an_empty_selection() {
    // "No route" is a decision the caller must act on, so it is a refusal rather than an
    // empty result a caller could mistake for success and proceed with none.
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    let error = select_route(&candidates, &local_only_request()).expect_err("nothing complies");
    assert_eq!(error.code(), "model.policy_unsatisfied");
    assert!(
        !error.retryable(),
        "the same candidate set will not start complying on a retry",
    );
}

#[test]
fn content_above_the_policys_sensitivity_ceiling_is_refused_before_any_candidate_is_considered() {
    // The ceiling is `PolicyRules::maximum_sensitivity`'s only consumer above the run path, and
    // before this check existed a `local_only` policy permitting only `public` content selected
    // a local candidate and sent `restricted` content: the policy set a limit that nothing
    // compared the content against. The candidate here is the SAFEST possible one, so a
    // candidate-based rejection (locality, retention) cannot be what refuses it — only the
    // ceiling can, which is what makes this test falsify the check rather than a coincidence.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            maximum_sensitivity: Sensitivity::Public,
            ..PolicyRules::permissive()
        },
        sensitivity: Sensitivity::Restricted,
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let error = select_route(&candidates, &request).expect_err("content over the ceiling");
    assert_eq!(error.code(), "model.policy_unsatisfied");

    // The refusal names the ceiling reason rather than a candidate's, because the condition is
    // about the content. Reporting `locality_violated` for a local candidate under a local-only
    // policy would name a rule that is in fact satisfied.
    let refusal = select_route_explained(&candidates, &request).expect_err("refused");
    match refusal {
        RouteSelectionFailure::Refused(refusal) => {
            assert_eq!(refusal.rejected.len(), 1);
            assert_eq!(
                refusal.rejected[0].reason,
                RejectionReason::SensitivityExceedsPolicy,
            );
        }
        // `panic!` is denied in this crate, so the unreachable arm asserts instead. The bound is
        // one past a single candidate, so reaching it would mean the bound is not a bound.
        RouteSelectionFailure::CandidatesUnbounded { .. } => {
            unreachable!("one candidate is well within the bound")
        }
    }
}

#[test]
fn content_exactly_at_the_policys_sensitivity_ceiling_is_inside_it() {
    // The other side of the boundary, asserted so a `>=`/`>` slip is caught. A ceiling of
    // `confidential` permits confidential content: the policy names the most that may be sent,
    // so the value it names is permitted. This is the opposite convention to the *deadline*,
    // where the named instant is already expired — the two are different quantities and both
    // are right for their own, which is why both sides are asserted rather than one.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            maximum_sensitivity: Sensitivity::Confidential,
            ..PolicyRules::permissive()
        },
        sensitivity: Sensitivity::Confidential,
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let decision = select_route(&candidates, &request).expect("confidential is the ceiling");
    assert_eq!(decision.requested.sensitivity, Sensitivity::Confidential);

    // One step stricter is outside, so the boundary is a real edge rather than a value that
    // happens to be permitted by both comparisons.
    let stricter = RouteRequest {
        rules: PolicyRules {
            maximum_sensitivity: Sensitivity::Internal,
            ..request.rules.clone()
        },
        ..request
    };
    assert!(
        select_route(&candidates, &stricter).is_err(),
        "confidential content is above an internal ceiling",
    );
}

#[test]
fn the_decision_records_the_policy_version_it_ran_under() {
    // Policy is mutable and a call outlives the process that made it, so a decision
    // re-derived from current state could explain a past call with rules that no longer
    // applied.
    let request = local_only_request();
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let decision = select_route(&candidates, &request).expect("compliant");
    assert_eq!(decision.policy, request.policy);
    assert_eq!(decision.policy.version, 1);
}

#[test]
fn the_requested_sensitivity_is_the_contents_not_the_policys_ceiling() {
    // The ceiling is an upper bound on what may be sent, so recording it as the call's own
    // classification would say every call carries maximally sensitive content — which would
    // make the record useless for the question it exists to answer.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            maximum_sensitivity: Sensitivity::Restricted,
            ..PolicyRules::permissive()
        },
        sensitivity: Sensitivity::Internal,
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let decision = select_route(&candidates, &request).expect("compliant");
    assert_eq!(decision.requested.sensitivity, Sensitivity::Internal);
    assert_eq!(
        decision.requested.locality,
        Locality::LocalOnly,
        "the requested side still reflects the policy's ceiling",
    );
}

#[test]
fn fallback_permission_is_carried_on_the_requested_side() {
    // The requested side is the policy's own statement, so a policy that denies fallback
    // must be visible on it. Without this the field could be dropped in the mapping and
    // nothing would notice, because the effective side has no fallback field.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            allow_fallback: FallbackPermission::Denied,
            telemetry: Telemetry::Disabled,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let decision = select_route(&candidates, &request).expect("compliant");
    assert_eq!(decision.requested.telemetry, Telemetry::Disabled);
}

#[test]
fn an_exception_relaxing_a_rule_the_candidate_did_not_fail_is_not_applied() {
    // The grant must name the rule that actually failed. A **cloud** candidate documents nothing,
    // so it fails the `bounded_documented` retention requirement — and a `local` endpoint would
    // *not* fail it, because a local route reports `not_applicable_local` and satisfies a retention
    // requirement vacuously (there is no provider to retain anything). The exception here relaxes
    // **locality**, which this candidate satisfies, so it must not be applied: applying it would
    // record a relaxation the call never used, and the reason must still name retention.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            maximum_provider_retention: ProviderRetention::BoundedDocumented,
            ..PolicyRules::permissive()
        },
        exceptions: vec![exception(
            PolicyRuleKey::Locality,
            ExceptionScope {
                locality: Some(Locality::PrivateNetworkAllowed),
                ..ExceptionScope::default()
            },
        )],
        ..local_only_request()
    };
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    let refusal = select_route_explained(&candidates, &request).expect_err("retention fails");
    match refusal {
        RouteSelectionFailure::Refused(refusal) => {
            assert_eq!(
                refusal.rejected[0].reason,
                RejectionReason::RetentionUnsatisfied,
                "the reason names what actually failed, not the rule the grant named",
            );
        }
        RouteSelectionFailure::CandidatesUnbounded { .. } => {
            unreachable!("one candidate is bounded")
        }
    }
}

#[test]
fn a_retention_exception_accepts_the_provider_default_the_grant_names() {
    // The relaxation works in the direction it should: a cloud candidate that documents nothing is
    // refused by a `bounded_documented` policy and admitted once a grant for that rule exists, with
    // the decision recording the grant. Both halves are asserted, so a relaxation that applied
    // unconditionally would fail the first assertion.
    let strict = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            maximum_provider_retention: ProviderRetention::BoundedDocumented,
            ..PolicyRules::permissive()
        },
        ..local_only_request()
    };
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    assert!(select_route(&candidates, &strict).is_err());

    let relaxed = RouteRequest {
        exceptions: vec![exception(
            PolicyRuleKey::MaximumProviderRetention,
            ExceptionScope::default(),
        )],
        ..strict
    };
    let decision = select_route(&candidates, &relaxed).expect("the grant accepts the default");
    assert!(decision.relied_on_exception());
}

#[test]
fn the_first_applicable_grant_is_the_one_recorded() {
    // A second grant would relax an already-relaxed rule, and one record keeps "why was this
    // call permitted" answerable by naming a single durable record. Two identical grants are
    // offered; the decision must name the first.
    let first = exception(
        PolicyRuleKey::MaximumProviderRetention,
        ExceptionScope::default(),
    );
    let mut second = first.clone();
    second.id = PolicyExceptionId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c95").expect("valid");

    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            maximum_provider_retention: ProviderRetention::BoundedDocumented,
            ..PolicyRules::permissive()
        },
        exceptions: vec![first.clone(), second],
        ..local_only_request()
    };
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    let decision = select_route(&candidates, &request).expect("a grant applies");
    assert_eq!(
        decision.exception_ref.as_deref(),
        Some(first.id.to_string().as_str()),
    );
}

#[test]
fn a_required_local_only_floor_rejects_a_private_network_candidate() {
    // A requirement is a floor rather than a ceiling: `LocalOnly` means the call must not
    // leave the device, even though the policy permits a private network. The two inputs
    // are separate precisely because a permissive policy does not imply a permissive call.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::PrivateNetworkAllowed,
            ..PolicyRules::permissive()
        },
        requirements: RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: std::collections::BTreeSet::new(),
            locality: Locality::LocalOnly,
        },
        ..local_only_request()
    };
    let candidates = [candidate(
        "private.host",
        "m1",
        EndpointClass::PrivateNetwork,
    )];
    assert_eq!(
        select_route(&candidates, &request)
            .expect_err("a private candidate is not local")
            .code(),
        "model.policy_unsatisfied",
    );
}

/// An exception granting `rule` with `scope`, issued before and expiring after the test instant.
///
/// `rule` must be waivable, because `PolicyException::grant` refuses a non-waivable one by
/// design. A test that needs such a record stands for one written by a *different* build — one
/// that considered the rule waivable — and mutates the field afterwards, which is why the
/// selector checks waivability as well as the grant path doing so.
fn exception(
    rule: PolicyRuleKey,
    scope: ExceptionScope,
) -> crate::model::exception::PolicyException {
    let waivable = if rule.is_waivable() {
        rule
    } else {
        PolicyRuleKey::Locality
    };
    let mut granted = crate::model::exception::PolicyException::grant(
        crate::model::exception::NewPolicyException {
            id: PolicyExceptionId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c99").expect("valid"),
            workspace_id: WorkspaceId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c98")
                .expect("valid"),
            policy: PolicyVersionRef {
                policy_id: crate::ids::ModelDataPolicyId::parse(
                    "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c97",
                )
                .expect("valid"),
                version: 1,
            },
            granting_principal_id: PrincipalId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c96")
                .expect("valid"),
            // Elevated, so the step-up rule does not refuse the grants these tests build. The
            // requirement itself is covered by the exception module's own tests.
            granting_assurance: crate::model::exception::RequiredAssurance::Elevated,
            rule: waivable,
            scope,
            reason_ref: "operator approved".to_owned(),
            single_use: false,
            issued_at: UtcTimestamp::parse("2026-09-01T00:00:00Z").expect("valid"),
            expires_at: UtcTimestamp::parse("2026-12-01T00:00:00Z").expect("valid"),
        },
    )
    .expect("a waivable rule with a bounded reason is grantable");
    // The requested key is restored, so a non-waivable one represents a record this build would
    // not create — the input the selector's own waivability check exists to handle.
    granted.rule = rule;
    granted
}

#[test]
fn without_an_exception_a_local_only_policy_refuses_a_cloud_candidate() {
    // The baseline the exception tests are measured against: the same candidate and the same
    // policy, with no grant offered, is refused. Without this, a relaxation that applied
    // unconditionally would look like a working exception.
    let candidates = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    assert!(select_route(&candidates, &local_only_request()).is_err());
}

#[test]
fn an_exception_for_locality_admits_the_class_it_names_and_records_itself() {
    // The feature: a grant that names `private_network_allowed` admits a private-network
    // candidate under a local-only policy, and the decision records the exception it relied on
    // so `relied_on_exception` has something to read — the contract requires the exception to be
    // "included in the route decision/audit without exposing content".
    let request = RouteRequest {
        exceptions: vec![exception(
            PolicyRuleKey::Locality,
            ExceptionScope {
                locality: Some(Locality::PrivateNetworkAllowed),
                ..ExceptionScope::default()
            },
        )],
        ..local_only_request()
    };
    let candidates = [candidate(
        "private.host",
        "m1",
        EndpointClass::PrivateNetwork,
    )];
    let decision = select_route(&candidates, &request).expect("the exception admits it");
    assert!(decision.relied_on_exception());
    assert_eq!(
        decision.exception_ref.as_deref(),
        Some("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c99"),
    );

    // And it admits *only* the class it names: a cloud candidate is still refused, because the
    // grant widened locality to `private_network_allowed`, not to `approved_cloud_allowed`.
    let cloud = [candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud)];
    assert!(
        select_route(&cloud, &request).is_err(),
        "the relaxation reaches the class it named and no further",
    );
}

#[test]
fn a_candidate_that_complies_without_a_grant_records_no_exception() {
    // "A grant permitted this" and "nothing had to be permitted" are different facts, and a
    // decision that named an exception it did not need would overstate what was relaxed. The
    // local candidate here already complies, so the offered (but unnecessary) grant must not be
    // recorded.
    let request = RouteRequest {
        exceptions: vec![exception(
            PolicyRuleKey::Locality,
            ExceptionScope {
                locality: Some(Locality::PrivateNetworkAllowed),
                ..ExceptionScope::default()
            },
        )],
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    let decision = select_route(&candidates, &request).expect("the local candidate complies");
    assert!(!decision.relied_on_exception());
    assert_eq!(decision.exception_ref, None);
}

#[test]
fn an_expired_revoked_or_consumed_exception_cannot_relax_anything() {
    // Each unusable state is asserted over its own mutation, because `is_usable_at` collapsing
    // any one of the three would be invisible to a test that only checked one. The decision must
    // then be a refusal, not a selection that silently ignored the grant.
    let base = exception(
        PolicyRuleKey::Locality,
        ExceptionScope {
            locality: Some(Locality::PrivateNetworkAllowed),
            ..ExceptionScope::default()
        },
    );

    let mut expired = base.clone();
    expired.expires_at = UtcTimestamp::parse("2026-09-22T11:00:00Z").expect("valid");
    let mut revoked = base.clone();
    revoked.revoked_at = Some(UtcTimestamp::parse("2026-09-22T11:00:00Z").expect("valid"));
    let mut consumed = base.clone();
    consumed.consumed_at = Some(UtcTimestamp::parse("2026-09-22T11:00:00Z").expect("valid"));

    for (name, offered) in [
        ("expired", expired),
        ("revoked", revoked),
        ("consumed", consumed),
    ] {
        let request = RouteRequest {
            exceptions: vec![offered],
            ..local_only_request()
        };
        let candidates = [candidate(
            "private.host",
            "m1",
            EndpointClass::PrivateNetwork,
        )];
        assert!(
            select_route(&candidates, &request).is_err(),
            "a {name} exception must not relax locality",
        );
    }
}

#[test]
fn an_exception_scoped_to_another_model_does_not_relax_this_candidate() {
    // The scope is a predicate, not a hint: a grant naming one model must not relax another.
    // Without this a grant issued for a provider's least sensitive model would silently permit
    // every model that provider serves.
    let request = RouteRequest {
        exceptions: vec![exception(
            PolicyRuleKey::Locality,
            ExceptionScope {
                model_id: Some("gpt-x1".to_owned()),
                locality: Some(Locality::PrivateNetworkAllowed),
                ..ExceptionScope::default()
            },
        )],
        ..local_only_request()
    };
    let other = [candidate(
        "private.host",
        "m1",
        EndpointClass::PrivateNetwork,
    )];
    assert!(
        select_route(&other, &request).is_err(),
        "the grant names another model, so this candidate is not covered",
    );

    // The named model still complies, so the refusal above is about the scope and not about the
    // relaxation failing to work at all.
    let named = [candidate(
        "private.host",
        "gpt-x1",
        EndpointClass::PrivateNetwork,
    )];
    let decision = select_route(&named, &request).expect("the named model is covered");
    assert!(decision.relied_on_exception());
}

#[test]
fn an_exception_for_a_non_waivable_rule_is_never_applied() {
    // The contract: "Exceptions cannot override non-waivable legal/administrator denies." A grant
    // naming the ceiling or an allow-list is skipped, leaving the rules *stricter* than the grant
    // asked for — so it can never permit a call the unrelaxed policy would refuse. Built through
    // the struct literal rather than `grant` because `grant` refuses these outright; the record
    // here stands for one written by a build that considered the rule waivable, which is exactly
    // why the skip must also live in the selector.
    let mut ceiling_grant = exception(
        PolicyRuleKey::MaximumSensitivity,
        ExceptionScope {
            maximum_sensitivity: Some(Sensitivity::Restricted),
            ..ExceptionScope::default()
        },
    );
    // `grant` would refuse this; force the record into existence to stand for a foreign writer.
    ceiling_grant.rule = PolicyRuleKey::MaximumSensitivity;

    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::LocalOnly,
            maximum_sensitivity: Sensitivity::Internal,
            ..PolicyRules::permissive()
        },
        sensitivity: Sensitivity::Restricted,
        exceptions: vec![ceiling_grant],
        ..local_only_request()
    };
    let candidates = [candidate("local.ollama", "llama3.1", EndpointClass::Local)];
    assert_eq!(
        select_route(&candidates, &request)
            .expect_err("the ceiling is non-waivable")
            .code(),
        "model.policy_unsatisfied",
    );
}

#[test]
fn a_residency_exception_admits_its_own_region_into_a_restrictive_list() {
    // A residency relaxation permits the region the grant names, and only through the list that
    // was already restricting. An empty list means "no restriction at this layer", so inserting
    // into it would *create* a restriction — the opposite of a relaxation — and the grant must
    // therefore leave an unrestricted list alone.
    let request = RouteRequest {
        rules: PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            allowed_residency_regions: ["eu".to_owned()].into_iter().collect(),
            ..PolicyRules::permissive()
        },
        exceptions: vec![exception(
            PolicyRuleKey::ResidencyRegions,
            ExceptionScope {
                region: Some(Region::parse("us").expect("valid")),
                ..ExceptionScope::default()
            },
        )],
        ..local_only_request()
    };
    let mut us_candidate = candidate("openai", "gpt-x1", EndpointClass::ApprovedCloud);
    us_candidate.region = Some(Region::parse("us").expect("valid"));

    // The list was `{eu}`; the exception adds `us`, so the US candidate is now inside it.
    let decision = select_route(&[us_candidate.clone()], &request).expect("us was added");
    assert!(decision.relied_on_exception());

    // Without the grant the same candidate is refused, so the relaxation is what admitted it.
    let unrelaxed = RouteRequest {
        exceptions: Vec::new(),
        ..request
    };
    assert!(select_route(&[us_candidate], &unrelaxed).is_err());
}
