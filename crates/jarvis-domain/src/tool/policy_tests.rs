//! Tests for deterministic policy evaluation.
//!
//! Two properties are the subject of most of what follows, and each test names the **other** answer
//! it is ruling out:
//!
//! 1. **Determinism.** The evaluator is a pure function, and the test that matters asserts two
//!    evaluations of one request are equal — because "deterministic" is exactly the claim that two
//!    runs agree, and nothing else establishes it.
//! - **No allow by omission.** Every `Allow` carries a positive reason, and the tests assert that a
//!   request with nothing going for it is not allowed. A test that only asserted an `Allow` for a
//!   well-formed request would pass against an evaluator that allowed everything by default, which is
//!   the failure this module exists to prevent.

use std::collections::BTreeSet;

use super::classification::{ApprovalHint, Effect, Risk, Scope};
use super::error_class::ToolErrorClass;
use super::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use super::policy::{
    ApprovalRecord, DenyRule, Grant, PolicyInputs, PolicyReason, PolicyRequest, evaluate,
};
use crate::ids::{PrincipalId, WorkspaceId};
use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;

// ---------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------

fn instant(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse(value).expect("the fixture instant parses")
}

fn now() -> UtcTimestamp {
    instant("2026-09-27T12:00:00Z")
}

fn principal(seed: u128) -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(seed))
}

fn workspace(seed: u128) -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(seed))
}

fn scope(value: &str) -> Scope {
    Scope::new(value).expect("the fixture scope is valid")
}

fn scopes(values: &[&str]) -> BTreeSet<Scope> {
    values.iter().map(|value| scope(value)).collect()
}

fn effects(values: &[Effect]) -> BTreeSet<Effect> {
    values.iter().copied().collect()
}

fn tool_identity(capability: &str, owner: &str, seed: u8) -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::parse(capability).expect("the fixture capability parses"),
        source: ToolSource::new(
            SourceKind::McpServer,
            owner,
            ToolVersion::parse("1.0.0").expect("valid"),
        )
        .expect("the fixture source is valid"),
        schema_fingerprint: SchemaFingerprint::from_bytes([seed; 32]),
    }
}

/// An ordinary read tool: low risk, one read-only effect, one scope, asks by default.
fn read_identity() -> ToolIdentity {
    tool_identity("fs.read@1", "acme.files", 1)
}

/// A grant that covers the read tool for principal 1 in workspace 1, with everything permitted.
fn grant_for(identity: &ToolIdentity) -> Grant {
    Grant {
        identity: identity.clone(),
        workspace: workspace(1),
        principal: principal(1),
        scopes: scopes(&["fs.read"]),
        effects: effects(&[
            Effect::ReadOnly,
            Effect::Write,
            Effect::ExternalCommunication,
        ]),
        risk_ceiling: Risk::Critical,
        sensitivity_ceiling: Sensitivity::Restricted,
        expires_at: None,
    }
}

/// Builds a request for the read tool with every field permissive, so a test changes only what it
/// is about. Named `permissive` rather than `default` because it is the *most allowed* shape and a
/// reader must not mistake it for a neutral one.
fn permissive_request<'a>(
    identity: &'a ToolIdentity,
    tool_effects: &'a BTreeSet<Effect>,
    required_scopes: &'a BTreeSet<Scope>,
    action_digest: &'a str,
) -> PolicyRequest<'a> {
    PolicyRequest {
        identity,
        principal: principal(1),
        workspace: workspace(1),
        effects: tool_effects,
        risk: Risk::Low,
        required_scopes,
        default_approval: ApprovalHint::Ask,
        sensitivity: Sensitivity::Internal,
        action_digest,
        tool_enabled: true,
        deadline: None,
    }
}

fn no_approvals() -> Vec<ApprovalRecord> {
    Vec::new()
}

// ---------------------------------------------------------------------------------------
// Determinism.
// ---------------------------------------------------------------------------------------

#[test]
fn two_evaluations_of_one_request_are_identical() {
    // **This is what "deterministic" means**, and it is asserted rather than argued from the
    // signature. An evaluator that read a clock, an environment variable, or a random source would
    // fail here while still compiling, and the failure would be invisible in production until an
    // audit compared two decisions for one action and found them different.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    // The tool declares `Allow`, which for a read-only low-risk tool is honoured — so the outcome is
    // a definite `Allow` rather than an `Ask`. Asserting on a definite outcome is the point: two
    // evaluations agreeing on `Ask` would not show that either consulted the same inputs.
    request.default_approval = ApprovalHint::Allow;
    let inputs = PolicyInputs {
        now: now(),
        grants: &grants,
        approvals: &no_approvals(),
        deny_rules: &[],
    };
    let first = evaluate(&request, &inputs);
    let second = evaluate(&request, &inputs);
    assert_eq!(first, second);
    assert!(first.is_allowed(), "{first:?}");
}

#[test]
fn one_request_evaluates_the_same_regardless_of_the_order_of_the_inputs() {
    // Determinism extends to input order, which is a stronger claim than "the same call twice". A
    // grant list whose first match decided would answer differently for the same set of grants
    // depending on how a repository happened to order them — and a repository order is not a policy.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let permissive = grant_for(&identity);
    // A second grant that does **not** cover this tool, placed before and after in turn.
    let mut unrelated = grant_for(&tool_identity("fs.write@1", "acme.files", 2));
    unrelated.scopes = scopes(&["fs.write"]);
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    // A definite `Allow`, so the comparison is of a positive decision rather than of two `Ask`s.
    request.default_approval = ApprovalHint::Allow;

    let forward = vec![permissive.clone(), unrelated.clone()];
    let backward = vec![unrelated, permissive];
    let forward_decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &forward,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    let backward_decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &backward,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert_eq!(
        forward_decision, backward_decision,
        "the answer must not depend on how the grants were ordered",
    );
    assert!(forward_decision.is_allowed());
}

// ---------------------------------------------------------------------------------------
// Precedence: deny rules beat grants.
// ---------------------------------------------------------------------------------------

#[test]
fn an_explicit_deny_rule_beats_a_grant_that_permits_the_same_action() {
    // **The precedence rule the module exists to enforce.** The architecture lists deny rules and
    // grants as separate inputs; an evaluator that consulted the grant first would return `Allow`
    // here, because every constraint on the grant is satisfied. The order is therefore the whole
    // content of this test, and it is asserted with the grant present and permissive.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    let deny_rules = vec![DenyRule {
        identity: Some(identity.clone()),
        principal: None,
        workspace: None,
        effects: BTreeSet::new(),
    }];

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &deny_rules,
        },
    );
    assert!(decision.is_denied(), "the deny rule must win: {decision:?}");
    assert_eq!(decision.reason(), Some(PolicyReason::ExplicitDeny));
    assert!(
        decision.grant.is_none(),
        "a refusal decided by a deny rule must not name a grant, because no grant was consulted",
    );
    assert!(decision.final_outcome, "a deny rule's refusal is final");
}

#[test]
fn a_deny_rule_may_name_only_an_effect_and_then_refuses_a_tool_that_has_it() {
    // The other shape a deny rule takes. An effect-only rule refuses any tool carrying that effect,
    // which is how "never execute code" is expressed without enumerating every tool.
    let identity = tool_identity("shell.exec@1", "acme.shell", 3);
    let tool_effects = effects(&[Effect::CodeExecution]);
    let required = scopes(&["shell.exec"]);
    let mut grant = grant_for(&identity);
    grant.scopes = scopes(&["shell.exec"]);
    grant.effects = effects(&[Effect::CodeExecution]);
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    let deny_rules = vec![DenyRule {
        identity: None,
        principal: None,
        workspace: None,
        effects: effects(&[Effect::CodeExecution]),
    }];

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&grant),
            approvals: &no_approvals(),
            deny_rules: &deny_rules,
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::ExplicitDeny));
}

#[test]
fn a_deny_rule_that_names_nothing_refuses_nothing() {
    // **A rule with no fields must not match everything.** An empty record is what an unpopulated
    // configuration entry produces, and a rule that matched everything would refuse every action in
    // the profile — a total outage from a construction mistake. The fail-closed direction is the
    // default *outcome* (nothing is allowed without a grant), not a rule that matches everything.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    let deny_rules = vec![DenyRule {
        identity: None,
        principal: None,
        workspace: None,
        effects: BTreeSet::new(),
    }];

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &deny_rules,
        },
    );
    assert!(
        !decision.is_denied(),
        "an empty deny rule must not refuse the action: {decision:?}",
    );
}

// ---------------------------------------------------------------------------------------
// Grant resolution: the three refusals a user reacts to differently.
// ---------------------------------------------------------------------------------------

#[test]
fn a_grant_for_another_principal_does_not_apply() {
    // A grant issued to another principal for the same tool in the same workspace is the
    // confused-deputy case, and a grant that applied "by tool" would authorize it. One workspace is
    // shared by every client in a local profile, so this is the common case rather than the exotic
    // one — the same shape as the idempotency and discovery-cache scoping defects.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let mut other_principals_grant = grant_for(&identity);
    other_principals_grant.principal = principal(2);
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&other_principals_grant),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::NoGrant));
}

#[test]
fn a_grant_for_another_workspace_does_not_apply() {
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let mut other_workspace_grant = grant_for(&identity);
    other_workspace_grant.workspace = workspace(2);
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&other_workspace_grant),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::NoGrant));
}

#[test]
fn an_expired_grant_is_reported_as_expired_rather_than_as_absent() {
    // Both refuse, and the difference is what the **user** does next: "you have no grant" sends them
    // to ask for one, while "your grant expired" tells them the access existed and lapsed. Asserted
    // alongside the absent case so the two reasons are shown to be distinct rather than both being
    // whatever the implementation happened to produce.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let mut expired = grant_for(&identity);
    expired.expires_at = Some(instant("2026-09-27T11:59:59Z"));
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&expired),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(
        decision.reason(),
        Some(PolicyReason::GrantExpired),
        "an expired grant must say so, because the user's next step differs from having none",
    );
    // The boundary: a grant expiring **exactly** at `now` is expired, because an expiry at an instant
    // does not permit work at that instant. The opposite convention from a token ceiling, and
    // consistent with `RunBudget`'s deadline.
    let mut exactly = grant_for(&identity);
    exactly.expires_at = Some(now());
    assert!(
        evaluate(
            &request,
            &PolicyInputs {
                now: now(),
                grants: std::slice::from_ref(&exactly),
                approvals: &no_approvals(),
                deny_rules: &[],
            },
        )
        .is_denied(),
        "a grant expiring exactly at `now` has expired",
    );
}

#[test]
fn a_replacement_behind_the_same_capability_is_reported_as_a_replacement() {
    // **The `ACC-024` case at the policy layer.** A grant exists, and it names `fs.read@1` — the
    // capability the caller is invoking — while the invocation's identity differs because the source
    // or the schema fingerprint changed. A user told "no grant" would look for a missing grant; the
    // truth is that the tool they had a grant for is a different tool now.
    let granted_identity = read_identity();
    let replacement = tool_identity("fs.read@1", "other.files", 9);
    let grants = vec![grant_for(&granted_identity)];
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let request = permissive_request(&replacement, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(
        decision.reason(),
        Some(PolicyReason::IdentityReplaced),
        "a grant naming the same capability but a different implementation must be reported as a \
         replacement, not as an absent grant",
    );
}

#[test]
fn a_grant_for_a_different_capability_is_simply_absent() {
    // The boundary of the replacement rule: a grant for a *different* tool is not a replacement, and
    // reporting it as one would tell a user their tool was replaced when they simply never had a
    // grant for the thing they are calling.
    let identity = read_identity();
    let grants = vec![grant_for(&tool_identity("mail.send@1", "acme.mail", 5))];
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert_eq!(decision.reason(), Some(PolicyReason::NoGrant));
}

// ---------------------------------------------------------------------------------------
// Grant constraints.
// ---------------------------------------------------------------------------------------

#[test]
fn a_missing_scope_refuses_even_though_the_grant_names_the_tool() {
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read", "fs.export"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::MissingScope));
    assert!(
        decision.grant.is_some(),
        "a refusal by a grant's own constraint must name the grant, so an operator can find it",
    );
}

#[test]
fn an_effect_outside_the_grant_refuses() {
    // The grant covers reading; the tool also writes. Asserted with every **other** constraint
    // permissive, so the only thing that can produce the refusal is the effect check.
    let identity = tool_identity("fs.write@1", "acme.files", 2);
    let tool_effects = effects(&[Effect::Write]);
    let required = scopes(&["fs.read"]);
    let mut grant = grant_for(&identity);
    grant.effects = effects(&[Effect::ReadOnly]);
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&grant),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::EffectNotPermitted));
}

#[test]
fn a_risk_above_the_ceiling_and_a_sensitivity_above_the_ceiling_are_both_reported() {
    // **Constraints are collected rather than returned one at a time**, so a user fixing one problem
    // does not discover the next afterwards. Both failures are present here and both must appear —
    // a test with one of them would pass against an implementation that returned on the first.
    let identity = tool_identity("shell.exec@1", "acme.shell", 3);
    let tool_effects = effects(&[Effect::CodeExecution]);
    let required = scopes(&["fs.read"]);
    let mut grant = grant_for(&identity);
    grant.effects = effects(&[Effect::CodeExecution]);
    grant.risk_ceiling = Risk::Low;
    grant.sensitivity_ceiling = Sensitivity::Internal;
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.risk = Risk::Critical;
    request.sensitivity = Sensitivity::Restricted;

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&grant),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(
        decision.reasons,
        vec![
            PolicyReason::RiskAboveCeiling,
            PolicyReason::SensitivityAboveCeiling
        ],
        "both failed constraints must be listed, not only the first encountered",
    );
}

#[test]
fn a_risk_exactly_at_the_ceiling_is_permitted() {
    // The boundary in the other direction, and it is the opposite convention from an expiry: a risk
    // ceiling of `Moderate` permits `Moderate`, because the ceiling names the greatest permitted
    // value — whereas an expiry instant does not permit work at that instant. Asserted so the two
    // conventions are shown to be deliberate rather than accidentally inconsistent.
    let identity = tool_identity("fs.write@1", "acme.files", 2);
    let tool_effects = effects(&[Effect::Write]);
    let required = scopes(&["fs.read"]);
    let mut grant = grant_for(&identity);
    grant.risk_ceiling = Risk::Moderate;
    grant.sensitivity_ceiling = Sensitivity::Internal;
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.risk = Risk::Moderate;
    request.sensitivity = Sensitivity::Internal;

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&grant),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(
        !decision.is_denied(),
        "a risk at exactly the ceiling is permitted: {decision:?}",
    );
}

// ---------------------------------------------------------------------------------------
// Availability and the run's deadline.
// ---------------------------------------------------------------------------------------

#[test]
fn a_disabled_tool_is_refused_before_any_grant_is_consulted() {
    // A grant for a disabled tool is stale configuration rather than permission, so the refusal is
    // decided without a grant and must not name one.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.tool_enabled = false;

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::ToolDisabled));
    assert!(decision.grant.is_none(), "no grant was consulted");
}

#[test]
fn a_passed_deadline_refuses_a_new_call() {
    // A run whose deadline has passed cannot start another tool call: the call would be work the run
    // has no budget for, and starting it would spend the budget of whatever asked.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.deadline = Some(instant("2026-09-27T11:59:59Z"));

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::DeadlinePassed));

    // And a deadline in the future does not refuse, so the check is about the deadline rather than
    // about having one.
    let mut future = permissive_request(&identity, &tool_effects, &required, "digest-a");
    future.deadline = Some(instant("2026-09-27T12:00:01Z"));
    assert!(
        !evaluate(
            &future,
            &PolicyInputs {
                now: now(),
                grants: &grants,
                approvals: &no_approvals(),
                deny_rules: &[],
            },
        )
        .is_denied(),
    );
}

// ---------------------------------------------------------------------------------------
// Approvals.
// ---------------------------------------------------------------------------------------

/// An approval that matches the read tool, principal 1, workspace 1, and one digest.
fn approval_for(identity: &ToolIdentity, digest: &str, expires_at: &str) -> ApprovalRecord {
    ApprovalRecord {
        identity: identity.clone(),
        principal: principal(1),
        workspace: workspace(1),
        action_digest: digest.to_owned(),
        expires_at: instant(expires_at),
        consumed: false,
    }
}

#[test]
fn a_matching_approval_raises_an_ask_to_an_allow() {
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    let approvals = vec![approval_for(&identity, "digest-a", "2026-09-27T12:10:00Z")];

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &approvals,
            deny_rules: &[],
        },
    );
    assert!(decision.is_allowed(), "{decision:?}");
    assert_eq!(decision.reason(), Some(PolicyReason::ApprovalMatched));
    assert!(
        !decision.final_outcome,
        "an approval-based allow is not final: a later layer may still narrow it",
    );
}

#[test]
fn an_approval_for_a_different_action_does_not_authorize_this_one() {
    // The contract: "approving 'send this email' does not approve a rewritten recipient, subject,
    // body, attachment, or account." The fingerprint is the mechanism, and a mismatch must fail.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "the-real-action");
    let approvals = vec![approval_for(
        &identity,
        "some-other-action",
        "2026-09-27T12:10:00Z",
    )];

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &approvals,
            deny_rules: &[],
        },
    );
    assert!(
        decision.needs_approval(),
        "a mismatched approval must not allow, and must not deny either — the action is one the \
         user has not been asked about, so the answer is to ask: {decision:?}",
    );
    assert_eq!(
        decision.reason(),
        Some(PolicyReason::ApprovalFingerprintMismatch),
        "the reason must name the mismatch, or a user would re-approve what they already approved",
    );
}

#[test]
fn the_fingerprint_is_checked_before_expiry_and_consumption() {
    // **The ordering with teeth.** An approval whose digest differs *and* which has expired must
    // report the mismatch, because re-approving cannot fix a digest that still will not match — while
    // reporting "expired" tells the user to do exactly that. Same for a consumed one: a spent
    // approval for another action is not "already used", it is "never for this".
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "the-real-action");

    let mut expired_and_mismatched =
        approval_for(&identity, "other-action", "2026-09-27T11:00:00Z");
    expired_and_mismatched.consumed = true;
    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: std::slice::from_ref(&expired_and_mismatched),
            deny_rules: &[],
        },
    );
    assert_eq!(
        decision.reason(),
        Some(PolicyReason::ApprovalFingerprintMismatch),
        "the digest is checked first, because re-approving cannot fix a mismatch",
    );

    // With the digest matching, the next check is consumption, then expiry — asserted separately so
    // each ordering is pinned rather than only the first.
    let mut consumed = approval_for(&identity, "the-real-action", "2026-09-27T12:10:00Z");
    consumed.consumed = true;
    assert_eq!(
        evaluate(
            &request,
            &PolicyInputs {
                now: now(),
                grants: &grants,
                approvals: std::slice::from_ref(&consumed),
                deny_rules: &[],
            },
        )
        .reason(),
        Some(PolicyReason::ApprovalConsumed),
    );

    let expired = approval_for(&identity, "the-real-action", "2026-09-27T11:00:00Z");
    assert_eq!(
        evaluate(
            &request,
            &PolicyInputs {
                now: now(),
                grants: &grants,
                approvals: std::slice::from_ref(&expired),
                deny_rules: &[],
            },
        )
        .reason(),
        Some(PolicyReason::ApprovalExpired),
    );
}

#[test]
fn an_approval_from_another_principal_or_workspace_does_not_apply() {
    // An approval is a statement by **one** principal about **one** workspace. Applying it across
    // either boundary is the same confused-deputy case as a cross-principal grant, and an approval is
    // the stronger credential of the two.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let mut other_principal = approval_for(&identity, "digest-a", "2026-09-27T12:10:00Z");
    other_principal.principal = principal(2);
    let mut other_workspace = approval_for(&identity, "digest-a", "2026-09-27T12:10:00Z");
    other_workspace.workspace = workspace(2);

    for candidate in [other_principal, other_workspace] {
        let decision = evaluate(
            &request,
            &PolicyInputs {
                now: now(),
                grants: &grants,
                approvals: std::slice::from_ref(&candidate),
                deny_rules: &[],
            },
        );
        assert!(
            !decision.is_allowed(),
            "a grant to another principal or workspace must not allow this action: {decision:?}",
        );
    }
}

#[test]
fn an_approval_can_never_overturn_a_deny() {
    // An approval raises an `Ask` to an `Allow`; it cannot reach a `Deny`, because the deny rules and
    // the grant constraints have already returned by the time approvals are consulted. Asserted with a
    // **matching** approval present, so the test would fail against an implementation that checked
    // approvals first.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    let approvals = vec![approval_for(&identity, "digest-a", "2026-09-27T12:10:00Z")];
    let deny_rules = vec![DenyRule {
        identity: Some(identity.clone()),
        principal: None,
        workspace: None,
        effects: BTreeSet::new(),
    }];

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &approvals,
            deny_rules: &deny_rules,
        },
    );
    assert!(
        decision.is_denied(),
        "a matching approval must not overturn an explicit deny: {decision:?}",
    );
}

// ---------------------------------------------------------------------------------------
// No allow by omission.
// ---------------------------------------------------------------------------------------

#[test]
fn a_destructive_tool_is_asked_about_even_when_it_asks_to_be_allowed() {
    // **The `ApprovalHint::Allow` trap.** The contract calls the declared default "a default policy
    // hint, not authorization", so a tool that says "allow me" and is neither low-risk nor read-only
    // is still asked about. An implementation that honoured the hint literally would send an email
    // unprompted because the email tool's manifest said `allow`.
    let identity = tool_identity("mail.send@1", "acme.mail", 5);
    let tool_effects = effects(&[Effect::ExternalCommunication, Effect::Write]);
    let required = scopes(&["fs.read"]);
    let mut grant = grant_for(&identity);
    grant.effects = effects(&[Effect::ExternalCommunication, Effect::Write]);
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.default_approval = ApprovalHint::Allow;
    request.risk = Risk::High;

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&grant),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(
        decision.needs_approval(),
        "a consequential tool declaring `allow` must still be asked about: {decision:?}",
    );
    assert_eq!(decision.reason(), Some(PolicyReason::DefaultAsk));

    // And the positive control: a **read-only, low-risk** tool declaring `allow` is allowed, so the
    // rule refuses the dangerous combination rather than `ApprovalHint::Allow` entirely.
    let read_tool = read_identity();
    let read_effects = effects(&[Effect::ReadOnly]);
    let read_required = scopes(&["fs.read"]);
    let read_grant = grant_for(&read_tool);
    let mut read_request =
        permissive_request(&read_tool, &read_effects, &read_required, "digest-a");
    read_request.default_approval = ApprovalHint::Allow;
    let read_decision = evaluate(
        &read_request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&read_grant),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(read_decision.is_allowed(), "{read_decision:?}");
    assert_eq!(read_decision.reason(), Some(PolicyReason::LowRiskReadOnly));
    assert!(
        read_decision.reason().is_some(),
        "an ALLOW must carry a positive reason; an unexplained allow is what `no allow by \
         omission` forbids",
    );
}

#[test]
fn a_request_with_no_grant_anywhere_is_denied_rather_than_allowed() {
    // **The single most important assertion in this file**, and the one a "happy path" test suite
    // omits. Every input is empty: no grants, no approvals, no deny rules. An evaluator that allowed
    // by falling through would return `Allow` here, and that is the default that makes every other
    // rule irrelevant — a profile with no configuration would permit everything.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let request = permissive_request(&identity, &tool_effects, &required, "digest-a");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &[],
            approvals: &[],
            deny_rules: &[],
        },
    );
    assert!(
        decision.is_denied(),
        "with no grant, no approval, and no deny rule the answer must be DENY: {decision:?}",
    );
    assert_eq!(decision.reason(), Some(PolicyReason::NoGrant));
    assert!(
        !decision.reasons.is_empty(),
        "every decision carries at least one reason, including a refusal",
    );
}

#[test]
fn a_tool_defaulting_to_deny_is_denied_when_the_grant_permits_everything() {
    // The tool's own posture is the last word when nothing else decides, and `Deny` must survive a
    // fully permissive grant — otherwise a tool could not refuse itself.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.default_approval = ApprovalHint::Deny;

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::DefaultDeny));
    assert!(decision.final_outcome, "a tool-declared deny is final");
}

#[test]
fn an_absent_approval_is_not_a_mismatched_approval() {
    // **The regression test for a defect a refactor introduced.** `resolve_approval` first returned
    // `Result<(), PolicyReason>` with `Err(DefaultAsk)` when no approval existed at all, so "nothing
    // matched" and "something matched and failed" were both `Err` — and the `DefaultAsk` overrode a
    // tool's declared `Deny`, turning a refusal into a prompt. It was found by
    // `a_tool_defaulting_to_deny_is_denied_when_the_grant_permits_everything` failing, not by reading.
    //
    // Asserted as a **pair** over one request shape, because the point is the difference: with no
    // approvals the decision must be the tool's `Deny`, and with an approval that lapsed it must
    // report the lapse. A single assertion could not distinguish the two bugs.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.default_approval = ApprovalHint::Deny;

    let absent = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &[],
            deny_rules: &[],
        },
    );
    assert_eq!(
        absent.reason(),
        Some(PolicyReason::DefaultDeny),
        "with NO approval for this tool the tool's own default decides, and it is DENY",
    );
    assert!(absent.is_denied());

    let lapsed = vec![approval_for(&identity, "digest-a", "2026-09-27T11:00:00Z")];
    let mismatched = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: &lapsed,
            deny_rules: &[],
        },
    );
    assert_eq!(
        mismatched.reason(),
        Some(PolicyReason::ApprovalExpired),
        "an approval that EXISTS and lapsed is a different fact, and the more useful one to report",
    );

    // And an approval that exists but is for another action is a third case, so all three are pinned.
    let other_action = vec![approval_for(
        &identity,
        "some-other-action",
        "2026-09-27T12:10:00Z",
    )];
    assert_eq!(
        evaluate(
            &request,
            &PolicyInputs {
                now: now(),
                grants: &grants,
                approvals: &other_action,
                deny_rules: &[],
            },
        )
        .reason(),
        Some(PolicyReason::ApprovalFingerprintMismatch),
    );
}

#[test]
fn every_decision_carries_at_least_one_reason() {
    // Swept across a matrix of outcomes rather than asserted once, because the rule is "every
    // decision is explainable" and one case's explanation does not establish another's. The matrix
    // walks the precedence order: a deny rule, a disabled tool, a missing grant, a failed constraint,
    // a tool default, and an approval.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];

    let deny_rule = DenyRule {
        identity: Some(identity.clone()),
        principal: None,
        workspace: None,
        effects: BTreeSet::new(),
    };
    // Bound rather than inline, because a `PolicyRequest` borrows its scope set and an inline
    // `scopes(&[...])` temporary would be dropped at the end of the tuple expression.
    let impossible_scope = scopes(&["fs.other"]);
    let cases: Vec<(&str, PolicyRequest<'_>, Vec<Grant>, Vec<DenyRule>)> = vec![
        (
            "explicit deny",
            permissive_request(&identity, &tool_effects, &required, "d"),
            grants.clone(),
            vec![deny_rule],
        ),
        (
            "no grant",
            permissive_request(&identity, &tool_effects, &required, "d"),
            Vec::new(),
            Vec::new(),
        ),
        (
            "grant constraint",
            permissive_request(&identity, &tool_effects, &impossible_scope, "d"),
            grants.clone(),
            Vec::new(),
        ),
        (
            "tool default",
            permissive_request(&identity, &tool_effects, &required, "d"),
            grants.clone(),
            Vec::new(),
        ),
    ];
    for (label, request, case_grants, case_rules) in &cases {
        let decision = evaluate(
            request,
            &PolicyInputs {
                now: now(),
                grants: case_grants,
                approvals: &no_approvals(),
                deny_rules: case_rules,
            },
        );
        assert!(
            !decision.reasons.is_empty(),
            "the {label} case must carry a reason",
        );
    }
}

// ---------------------------------------------------------------------------------------
// The decision's error mapping.
// ---------------------------------------------------------------------------------------

#[test]
fn an_ask_maps_to_approval_required_and_a_permission_refusal_to_permission_denied() {
    // **The mapping that decides what a user is told.** `Ask` means the system is waiting to prompt
    // them, while `Deny` for a missing grant means they are not permitted — and an implementation
    // that returned `permission_denied` for both would tell a user they are not allowed when the
    // system is about to ask them. Both sides are asserted, because one alone would pass against an
    // implementation that mapped everything to the same class.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let ask = evaluate(
        &permissive_request(&identity, &tool_effects, &required, "d"),
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&grant_for(&identity)),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(ask.needs_approval());
    assert_eq!(ask.error_class(), Some(ToolErrorClass::ApprovalRequired));

    let denied = evaluate(
        &permissive_request(&identity, &tool_effects, &required, "d"),
        &PolicyInputs {
            now: now(),
            grants: &[],
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(denied.is_denied());
    assert_eq!(denied.error_class(), Some(ToolErrorClass::PermissionDenied));
}

#[test]
fn an_allow_maps_to_no_error_class_at_all() {
    // An `Allow` has no error, and returning one would make a successful decision look like a
    // refusal to a caller that branched on `Some`.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let mut request = permissive_request(&identity, &tool_effects, &required, "d");
    request.default_approval = ApprovalHint::Allow;
    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: std::slice::from_ref(&grant_for(&identity)),
            approvals: &no_approvals(),
            deny_rules: &[],
        },
    );
    assert!(decision.is_allowed());
    assert_eq!(decision.error_class(), None);
}

#[test]
fn a_denial_caused_by_a_lapsed_approval_names_the_approval_rather_than_permission() {
    // The specific cases the mapping exists for: a user whose approval expired must be told to
    // re-approve, not to seek permission they already hold. A single `permission_denied` for every
    // denial would lose exactly that.
    let identity = read_identity();
    let tool_effects = effects(&[Effect::ReadOnly]);
    let required = scopes(&["fs.read"]);
    let grants = vec![grant_for(&identity)];
    // A tool that denies by default, with an expired approval for its exact action. The expiry is
    // what the decision reports, so the class names the approval.
    let mut request = permissive_request(&identity, &tool_effects, &required, "digest-a");
    request.default_approval = ApprovalHint::Deny;
    let expired = approval_for(&identity, "digest-a", "2026-09-27T11:00:00Z");

    let decision = evaluate(
        &request,
        &PolicyInputs {
            now: now(),
            grants: &grants,
            approvals: std::slice::from_ref(&expired),
            deny_rules: &[],
        },
    );
    assert!(decision.is_denied());
    assert_eq!(decision.reason(), Some(PolicyReason::ApprovalExpired));
    assert_eq!(
        decision.error_class(),
        Some(ToolErrorClass::ApprovalExpired),
        "an expired approval must not be reported as a permission problem",
    );
}
