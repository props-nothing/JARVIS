//! Tests for the policy exception model.
//!
//! The value here is in the **refusals and the unusable states**, because an exception is the
//! one mechanism in this workspace that relaxes a rule. A grant path that always succeeded, or
//! a usability predicate that accepted a revoked record, would pass any test that only issued
//! an exception and used it once.

use super::{
    ExceptionScope, ExceptionState, MAX_EXCEPTION_REASON_BYTES, NewPolicyException,
    PolicyException, PolicyRuleKey, RequiredAssurance,
};
use crate::error::DomainError;
use crate::ids::{ModelDataPolicyId, PolicyExceptionId, PrincipalId, WorkspaceId};
use crate::model::identity::{ModelId, ModelRef, ProviderId, Region};
use crate::model::policy::{Locality, PolicyVersionRef, Sensitivity};
use crate::time::UtcTimestamp;

fn id() -> PolicyExceptionId {
    PolicyExceptionId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d").expect("valid")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c6d").expect("valid")
}

fn principal() -> PrincipalId {
    PrincipalId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c7d").expect("valid")
}

fn policy() -> PolicyVersionRef {
    PolicyVersionRef {
        policy_id: ModelDataPolicyId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c8d").expect("valid"),
        version: 4,
    }
}

fn at(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse(value).expect("valid")
}

/// A grant request for `rule` with `scope`, at `assurance`.
///
/// One builder so the tests that only differ in the rule, the scope, or the assurance do not repeat
/// eleven fields — and so a field added to the request is one edit rather than a dozen.
fn grant_request(
    rule: PolicyRuleKey,
    scope: ExceptionScope,
    assurance: RequiredAssurance,
) -> NewPolicyException {
    NewPolicyException {
        id: id(),
        workspace_id: workspace(),
        policy: policy(),
        granting_principal_id: principal(),
        granting_assurance: assurance,
        rule,
        scope,
        reason_ref: "operator approved".to_owned(),
        single_use: false,
        issued_at: at("2026-09-23T00:00:00Z"),
        expires_at: at("2026-09-24T00:00:00Z"),
    }
}

/// A grant of `rule` that expires a day after it is issued.
///
/// Granted at `Elevated` assurance, which satisfies every step-up requirement — the tests that
/// need to prove a *standard* principal is refused construct their own grant instead.
fn granted(rule: PolicyRuleKey) -> PolicyException {
    PolicyException::grant(grant_request(
        rule,
        ExceptionScope::default(),
        RequiredAssurance::Elevated,
    ))
    .expect("a waivable rule with a bounded reason is grantable")
}

#[test]
fn a_non_waivable_rule_cannot_be_granted_an_exception_at_all() {
    // The contract: "Exceptions cannot override non-waivable legal/administrator denies." The
    // refusal is at grant time rather than at evaluation, so an operator cannot create a
    // record that reads as a relaxation in an audit while being ignored when a call is routed.
    for rule in [
        PolicyRuleKey::AllowedProviders,
        PolicyRuleKey::AllowedModels,
        PolicyRuleKey::MaximumSensitivity,
    ] {
        let error = PolicyException::grant(grant_request(
            rule,
            ExceptionScope::default(),
            RequiredAssurance::Elevated,
        ))
        .expect_err("a non-waivable rule must be refused");
        assert_eq!(error.code(), "model.exception_required");
        assert!(!rule.is_waivable(), "{rule} must be marked non-waivable");
    }
}

#[test]
fn every_waivable_rule_can_be_granted_and_no_other_rule_can() {
    // The complement of the refusal above, asserted over the whole vocabulary so a rule added
    // without a waivability decision fails here rather than silently defaulting to waivable.
    let waivable = [
        PolicyRuleKey::Locality,
        PolicyRuleKey::MaximumProviderRetention,
        PolicyRuleKey::ProviderTrainingUse,
        PolicyRuleKey::Telemetry,
        PolicyRuleKey::ResidencyRegions,
        PolicyRuleKey::AllowFallback,
    ];
    for rule in waivable {
        assert!(granted(rule).relaxes(rule));
    }
    assert_eq!(
        waivable.iter().filter(|rule| rule.is_waivable()).count(),
        waivable.len(),
        "every rule in this list must report itself waivable",
    );
}

#[test]
fn an_expiry_at_or_before_the_issue_instant_is_refused_rather_than_stored() {
    // Such a record would read as a real relaxation in an audit while permitting nothing,
    // which is the shape most easily mistaken for a working grant.
    for expiry in ["2026-09-23T00:00:00Z", "2026-09-22T23:59:59Z"] {
        let mut request = grant_request(
            PolicyRuleKey::Locality,
            ExceptionScope::default(),
            RequiredAssurance::Elevated,
        );
        request.expires_at = at(expiry);
        let error = PolicyException::grant(request)
            .expect_err("an expiry that has already passed must be refused");
        assert_eq!(error.code(), "model.exception_expired");
    }
}

#[test]
fn an_empty_or_unbounded_reason_is_refused() {
    let long = "x".repeat(MAX_EXCEPTION_REASON_BYTES + 1);
    for reason in [String::new(), long, "has\0nul".to_owned()] {
        let mut request = grant_request(
            PolicyRuleKey::Locality,
            ExceptionScope::default(),
            RequiredAssurance::Elevated,
        );
        request.reason_ref = reason;
        let error =
            PolicyException::grant(request).expect_err("an unusable reason must be refused");
        assert_eq!(error.code(), "jarvis.invalid_transition_reason");
    }
}

#[test]
fn an_issued_exception_is_usable_before_its_expiry_and_expired_at_it() {
    // Both sides of the boundary, so a `>`/`>=` slip is caught. The expiry instant itself is
    // expired — the same convention the run deadline uses, because an exception that stops
    // being usable at `T` cannot permit a call made at `T`.
    let exception = granted(PolicyRuleKey::Locality);
    assert!(exception.is_usable_at(at("2026-09-23T12:00:00Z")));
    assert_eq!(
        exception.state_at(at("2026-09-23T12:00:00Z")),
        ExceptionState::Issued,
    );
    assert!(!exception.is_usable_at(at("2026-09-24T00:00:00Z")));
    assert_eq!(
        exception.state_at(at("2026-09-24T00:00:00Z")),
        ExceptionState::Expired,
    );
}

#[test]
fn a_revoked_exception_is_reported_as_revoked_rather_than_expired() {
    // Both are unusable, and the distinction is the whole reason `state_at` exists: telling an
    // operator an exception expired when someone revoked it names the wrong cause, and the two
    // call for different actions.
    let mut exception = granted(PolicyRuleKey::Locality);
    exception.revoked_at = Some(at("2026-09-23T06:00:00Z"));
    assert!(!exception.is_usable_at(at("2026-09-23T12:00:00Z")));
    assert_eq!(
        exception.state_at(at("2026-09-23T12:00:00Z")),
        ExceptionState::Revoked,
    );
    // And after the deadline it is still reported as revoked, because the revocation is a
    // decision that happened, not a state that expiry can overwrite.
    assert_eq!(
        exception.state_at(at("2026-11-01T00:00:00Z")),
        ExceptionState::Revoked,
    );
}

#[test]
fn a_consumed_single_use_exception_cannot_be_used_a_second_time() {
    // Single use is a property of the record rather than a flag on the call, so a caller that
    // forgot it was single-use still cannot reuse it: the predicate consults `consumed_at`.
    let mut exception = granted(PolicyRuleKey::Locality);
    exception.consumed_at = Some(at("2026-09-23T06:00:00Z"));
    assert!(!exception.is_usable_at(at("2026-09-23T12:00:00Z")));
    assert_eq!(
        exception.state_at(at("2026-09-23T12:00:00Z")),
        ExceptionState::Consumed,
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn a_scope_narrows_on_the_axis_it_names_and_not_on_the_others() {
    // An exception is the one mechanism that relaxes a rule, so its scope must not be read as
    // wider than it is. Each axis is asserted independently, because a `covers` that ignored
    // one of them would permit a provider or model the grant never named.
    let model = ModelRef::new(
        ProviderId::parse("openai").expect("valid"),
        ModelId::parse("gpt-x1").expect("valid"),
    );
    let other_provider = ModelRef::new(
        ProviderId::parse("anthropic").expect("valid"),
        ModelId::parse("gpt-x1").expect("valid"),
    );
    let other_model = ModelRef::new(
        ProviderId::parse("openai").expect("valid"),
        ModelId::parse("gpt-x2").expect("valid"),
    );

    // Naming nothing covers every model, which is the deliberate direction: the caller states
    // what it narrows to, and a default that narrowed further would surprise in the dangerous
    // direction.
    let unscoped = ExceptionScope::default();
    assert!(unscoped.covers(&model));
    assert!(unscoped.covers(&other_provider));

    let by_provider = ExceptionScope {
        provider_id: Some("openai".to_owned()),
        ..ExceptionScope::default()
    };
    assert!(by_provider.covers(&model));
    assert!(!by_provider.covers(&other_provider));
    assert!(by_provider.covers(&other_model));

    let by_model = ExceptionScope {
        model_id: Some("gpt-x1".to_owned()),
        ..ExceptionScope::default()
    };
    assert!(by_model.covers(&model));
    assert!(by_model.covers(&other_provider));
    assert!(!by_model.covers(&other_model));

    // A region is carried on the scope for a residency relaxation but is not part of `covers`,
    // which is a model predicate: a region does not select a model, so reading it here would
    // refuse a candidate for the wrong reason.
    let by_region = ExceptionScope {
        region: Some(Region::parse("eu").expect("valid")),
        ..ExceptionScope::default()
    };
    assert!(by_region.covers(&model));

    // The two classification fields are separate values rather than one "value", so an
    // exception that relaxes locality cannot be read as raising the sensitivity ceiling.
    let classified = ExceptionScope {
        maximum_sensitivity: Some(Sensitivity::Confidential),
        locality: Some(Locality::PrivateNetworkAllowed),
        ..ExceptionScope::default()
    };
    assert_eq!(
        classified.maximum_sensitivity,
        Some(Sensitivity::Confidential),
    );
    assert_eq!(classified.locality, Some(Locality::PrivateNetworkAllowed),);
    assert!(classified.covers(&model));
}

#[test]
fn an_exception_relaxes_only_the_rule_it_names() {
    // The one-rule property, asserted across the whole vocabulary so a `relaxes` that returned
    // true too broadly fails here rather than permitting a rule nobody granted.
    let exception = granted(PolicyRuleKey::MaximumProviderRetention);
    for rule in [
        PolicyRuleKey::Locality,
        PolicyRuleKey::AllowedProviders,
        PolicyRuleKey::AllowedModels,
        PolicyRuleKey::MaximumProviderRetention,
        PolicyRuleKey::ProviderTrainingUse,
        PolicyRuleKey::Telemetry,
        PolicyRuleKey::ResidencyRegions,
        PolicyRuleKey::MaximumSensitivity,
        PolicyRuleKey::AllowFallback,
    ] {
        assert_eq!(
            exception.relaxes(rule),
            rule == PolicyRuleKey::MaximumProviderRetention,
            "{rule} must not be relaxed by a retention exception",
        );
    }
}

#[test]
fn every_rule_key_round_trips_through_its_stored_form() {
    // The stored value is the contract's spelling, and a key that does not round-trip would be
    // read back as `None` and reported as corruption for a row that is perfectly valid.
    for rule in [
        PolicyRuleKey::Locality,
        PolicyRuleKey::AllowedProviders,
        PolicyRuleKey::AllowedModels,
        PolicyRuleKey::MaximumProviderRetention,
        PolicyRuleKey::ProviderTrainingUse,
        PolicyRuleKey::Telemetry,
        PolicyRuleKey::ResidencyRegions,
        PolicyRuleKey::MaximumSensitivity,
        PolicyRuleKey::AllowFallback,
    ] {
        assert_eq!(PolicyRuleKey::from_stored(rule.as_str()), Some(rule));
        assert_eq!(rule.to_string(), rule.as_str());
    }
    assert_eq!(PolicyRuleKey::from_stored("not_a_rule"), None);
}

#[test]
fn every_exception_state_round_trips_and_only_issued_is_usable() {
    for (state, name) in [
        (ExceptionState::Issued, "issued"),
        (ExceptionState::Expired, "expired"),
        (ExceptionState::Revoked, "revoked"),
        (ExceptionState::Consumed, "consumed"),
    ] {
        assert_eq!(state.as_str(), name);
        assert_eq!(
            state.is_usable(),
            state == ExceptionState::Issued,
            "{name} usability must match its state",
        );
    }
}

/// Grants `rule` with `scope` at `assurance`, for the step-up tests.
fn grant_at(
    rule: PolicyRuleKey,
    scope: ExceptionScope,
    assurance: RequiredAssurance,
) -> Result<PolicyException, DomainError> {
    PolicyException::grant(grant_request(rule, scope, assurance))
}

#[test]
fn a_step_up_grant_is_required_for_cross_border_and_sensitive_exceptions() {
    // The contract: "Sensitive or cross-border exceptions require policy-defined
    // step-up/approval." Both inputs to the derivation are asserted, because a grant can be
    // cross-border through its *rule* (locality, residency) or sensitive through its *scope*
    // (a named classification), and dropping either would let one class through at standard
    // assurance while the other is still caught.
    //
    // Cross-border by rule: a standard principal is refused, an elevated one succeeds.
    for rule in [PolicyRuleKey::Locality, PolicyRuleKey::ResidencyRegions] {
        let scope = ExceptionScope {
            locality: Some(Locality::PrivateNetworkAllowed),
            ..ExceptionScope::default()
        };
        let refused = grant_at(rule, scope.clone(), RequiredAssurance::Standard)
            .expect_err("a cross-border grant must require step-up");
        assert_eq!(refused.code(), "model.exception_required");
        let granted = grant_at(rule, scope, RequiredAssurance::Elevated).expect("elevated");
        assert_eq!(granted.required_assurance, RequiredAssurance::Elevated);
    }

    // Cross-border by scope: a rule that does not require step-up on its own does once its
    // scope names a classification, which is the "sensitive" half of the same sentence.
    let sensitive = ExceptionScope {
        maximum_sensitivity: Some(Sensitivity::Confidential),
        ..ExceptionScope::default()
    };
    let refused = grant_at(
        PolicyRuleKey::MaximumProviderRetention,
        sensitive.clone(),
        RequiredAssurance::Standard,
    )
    .expect_err("a sensitive grant must require step-up");
    assert_eq!(refused.code(), "model.exception_required");
    let granted = grant_at(
        PolicyRuleKey::MaximumProviderRetention,
        sensitive,
        RequiredAssurance::Elevated,
    )
    .expect("elevated");
    assert_eq!(granted.required_assurance, RequiredAssurance::Elevated);
}

#[test]
fn a_plain_disclosure_grant_needs_only_standard_assurance() {
    // The other side of the boundary: relaxing a retention requirement accepts a provider's
    // published default, which is a decision an operator makes explicitly rather than a boundary
    // crossed. Requiring step-up for it too would make every grant an escalation, and an operator
    // who must escalate for everything stops distinguishing the two.
    let granted = grant_at(
        PolicyRuleKey::MaximumProviderRetention,
        ExceptionScope::default(),
        RequiredAssurance::Standard,
    )
    .expect("a plain retention grant needs no step-up");
    assert_eq!(granted.required_assurance, RequiredAssurance::Standard);
}

#[test]
fn an_elevated_principal_satisfies_a_standard_requirement_but_not_the_reverse() {
    // The ladder, asserted in both directions: a more strongly authenticated caller must not be
    // refused a standard grant, and a standard caller must not satisfy an elevated one.
    assert!(RequiredAssurance::Standard.is_satisfied_by(RequiredAssurance::Elevated));
    assert!(RequiredAssurance::Standard.is_satisfied_by(RequiredAssurance::Standard));
    assert!(RequiredAssurance::Elevated.is_satisfied_by(RequiredAssurance::Elevated));
    assert!(!RequiredAssurance::Elevated.is_satisfied_by(RequiredAssurance::Standard));
}

#[test]
fn a_rule_that_crosses_a_boundary_requires_step_up_and_no_other_waivable_rule_does() {
    // The predicate over the whole vocabulary, so a waivable rule added with the wrong step-up
    // answer fails here rather than being discovered as a permissive default.
    for rule in [
        PolicyRuleKey::Locality,
        PolicyRuleKey::MaximumProviderRetention,
        PolicyRuleKey::ProviderTrainingUse,
        PolicyRuleKey::Telemetry,
        PolicyRuleKey::ResidencyRegions,
        PolicyRuleKey::AllowFallback,
    ] {
        assert_eq!(
            rule.requires_step_up(),
            matches!(
                rule,
                PolicyRuleKey::Locality | PolicyRuleKey::ResidencyRegions
            ),
            "{rule} has the wrong step-up requirement",
        );
    }
}
