//! Deterministic policy evaluation with explainable decisions.
//!
//! `TLS-004` asks for two properties, and both are enforced by the shape of this module rather than
//! by discipline:
//!
//! - **Deterministic.** [`evaluate`] is a pure function of its three inputs. It reads no clock, no
//!   environment, no random source, and no I/O, and `now` is a parameter rather than something the
//!   evaluator looks up. That is what makes two evaluations of the same request comparable at all:
//!   an evaluator that read the clock could return different answers for one action, and an audit
//!   could not say which of them authorized the effect.
//! - **Explainable.** Every decision carries one or more [`PolicyReason`] values, and an `Allow` is
//!   **never produced by falling through**: it requires a positive reason. The architecture states
//!   the model cannot modify the decision, so the reason is what a user is shown when they ask why;
//!   a decision that could be `Allow` for "no rule matched" would be unexplainable in the one case
//!   where the explanation matters.
//!
//! The precedence order is fixed and is the heart of the module. **A deny rule beats a grant**, and
//! that is not a stylistic choice: the architecture lists "current grants, **deny rules**, budgets,
//! quiet hours, and environment" as inputs, and an implementation that consulted grants first would
//! let a grant override an explicit refusal — which is the same "the thing consulted first is the
//! thing that decides" mistake as putting an authorization answer in a discovery cache.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;

use super::classification::{Effect, Risk, Scope};
use super::identity::ToolIdentity;

/// The three outcomes the architecture names.
///
/// The contract's tool error classes include `tool.permission_denied` and
/// `tool.approval_required`, and the mapping from these is *not* one-to-one in the direction that
/// matters: `Deny` and "no approval exists" are different facts, and a caller that collapsed them
/// would report "you are not allowed" for an action the user simply has not been asked about yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOutcome {
    /// The action may proceed.
    Allow,
    /// The action requires a human decision before it may proceed.
    Ask,
    /// The action must not proceed.
    Deny,
}

impl PolicyOutcome {
    /// Returns the spelling operator output uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Allow => "ALLOW",
            Self::Ask => "ASK",
            Self::Deny => "DENY",
        }
    }
}

impl std::fmt::Display for PolicyOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// Why a decision came out the way it did.
///
/// A closed set, and each variant names **one rule**, because these are what a user is shown. Two
/// refusals that a user must react to differently — "your grant does not cover writing" versus "the
/// grant expired" — need different variants, and the module's own tests assert that a decision
/// carries the *specific* reason rather than a general one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyReason {
    /// An explicit deny rule matched. Consulted before any grant.
    ExplicitDeny,
    /// No grant covers this principal, workspace, and tool.
    NoGrant,
    /// A grant covers the tool but the caller does not hold a required scope.
    MissingScope,
    /// A grant covers the tool but does not permit one of its effects.
    EffectNotPermitted,
    /// The grant names a **different** tool identity than the one being invoked.
    ///
    /// Separate from [`Self::NoGrant`] because it is the `ACC-024` case: a grant exists and still
    /// names the tool — the implementation behind it changed. A user told "no grant" would go looking
    /// for a missing grant rather than at the replacement.
    IdentityReplaced,
    /// The grant's risk ceiling is below the tool's risk.
    RiskAboveCeiling,
    /// The arguments are classified above the grant's sensitivity ceiling.
    SensitivityAboveCeiling,
    /// The grant has expired.
    GrantExpired,
    /// The tool is disabled.
    ToolDisabled,
    /// The run's deadline has passed, so no new tool call may start.
    DeadlinePassed,
    /// A deny-listed effect is present, whatever the grant permits.
    EffectDenied,
    /// An exact approval matched this action.
    ApprovalMatched,
    /// An approval matched the tool but its fingerprint named a different action.
    ApprovalFingerprintMismatch,
    /// An approval matched and has expired.
    ApprovalExpired,
    /// An approval matched and has already been consumed.
    ApprovalConsumed,
    /// The tool's own default is to ask and nothing else decided.
    DefaultAsk,
    /// The tool's own default is to deny and nothing else decided.
    DefaultDeny,
    /// The tool is low risk and read-only, so policy allows it without a prompt.
    LowRiskReadOnly,
    /// The operator's autonomy level allows an action of this effect and risk without a prompt.
    AutonomyAllowed,
}

/// How much JARVIS may do without asking, as the operator declared it.
///
/// **A posture the operator sets, never something a model or a tool can influence.** It is the answer to
/// "how often should this interrupt me", and the answer has to be deterministic and reviewable: the same
/// level and the same action always decide the same way. It widens exactly one thing — which actions are
/// allowed *without a prompt* when a grant already covers them — and it never overrides a deny rule, a
/// grant constraint, a tool that declares itself `Deny`, or an approval requirement for anything
/// consequential.
///
/// | level | allowed without a prompt |
/// |---|---|
/// | `ask` | only what a tool declares `Allow` and is read-only and low risk (the original behaviour) |
/// | `balanced` | read-only, low-risk actions from any source the operator configured |
/// | `autonomous` | the above, plus reversible local writes of at most moderate risk |
///
/// What no level allows unprompted: external communication, destruction, code execution, money, privilege,
/// physical effects, or anything of high or critical risk. Those always ask.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    /// Ask about everything that is not a declared low-risk read.
    Ask,
    /// Read-only, low-risk actions run; everything else asks. The default.
    #[default]
    Balanced,
    /// Reversible local writes of at most moderate risk also run.
    Autonomous,
}

impl AutonomyLevel {
    /// Returns whether an action with these effects and this risk needs no prompt at this level.
    ///
    /// The **single definition** of the rule: policy uses it to skip the prompt and the grant source uses
    /// it to decide which tools are covered implicitly, so the two cannot disagree about what a level
    /// means.
    #[must_use]
    pub fn auto_allows(self, effects: &BTreeSet<Effect>, risk: Risk) -> bool {
        if effects.is_empty() {
            return false;
        }
        match self {
            Self::Ask => false,
            Self::Balanced => {
                effects.iter().all(|effect| *effect == Effect::ReadOnly) && risk <= Risk::Low
            }
            Self::Autonomous => {
                effects
                    .iter()
                    .all(|effect| matches!(effect, Effect::ReadOnly | Effect::Write))
                    && risk <= Risk::Moderate
            }
        }
    }

    /// Returns the spelling configuration and the wire use.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Balanced => "balanced",
            Self::Autonomous => "autonomous",
        }
    }

    /// Parses the spelling [`Self::as_contract_str`] produces.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ask" => Some(Self::Ask),
            "balanced" => Some(Self::Balanced),
            "autonomous" => Some(Self::Autonomous),
            _ => None,
        }
    }
}

/// One grant: what a principal is allowed to do with one tool in one workspace.
///
/// A grant names a **tool identity** rather than a capability, which is the whole point: identity
/// includes the source and the schema fingerprint, so a grant cannot survive a replacement. Storing
/// a capability here would make `ACC-024`'s rule unenforceable at this layer, and the `IdentityReplaced`
/// reason exists so an operator can see that it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The exact tool identity this grant covers.
    pub identity: ToolIdentity,
    /// The workspace it applies in.
    pub workspace: crate::ids::WorkspaceId,
    /// The principal it was issued to.
    pub principal: crate::ids::PrincipalId,
    /// The scopes it confers.
    pub scopes: BTreeSet<Scope>,
    /// The effects it permits. An effect outside this set is refused.
    pub effects: BTreeSet<Effect>,
    /// The greatest risk it permits.
    pub risk_ceiling: Risk,
    /// The most sensitive arguments it permits.
    pub sensitivity_ceiling: Sensitivity,
    /// When it stops applying, if it does.
    ///
    /// Optional rather than required, and an **absent** value means "does not expire" only because a
    /// standing grant for a read-only tool is legitimate. A grant that never expires for a
    /// consequential tool is a separate policy question and not this type's to refuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<UtcTimestamp>,
}

impl Grant {
    /// Returns whether the grant applies to `identity` in `workspace` for `principal` at `now`.
    ///
    /// The three scopes are checked together because a grant failing any one of them is not "close
    /// enough": a grant issued to another principal for the same tool in the same workspace is
    /// exactly the confused-deputy case, and one that applied "by tool" would authorize it.
    ///
    /// An expired grant is **not** applicable rather than applicable-and-refused, so the caller's
    /// reason is `NoGrant` — with one deliberate exception in [`evaluate`], which distinguishes them
    /// when the identity matches, because "your grant expired" and "you have no grant" lead a user to
    /// different next steps.
    #[must_use]
    pub fn applies_at(
        &self,
        identity: &ToolIdentity,
        workspace: crate::ids::WorkspaceId,
        principal: crate::ids::PrincipalId,
        now: UtcTimestamp,
    ) -> bool {
        &self.identity == identity
            && self.workspace == workspace
            && self.principal == principal
            && !self.is_expired_at(now)
    }

    /// Returns whether the grant names this identity but has expired at `now`.
    ///
    /// Used to tell the two cases apart for the operator, since both produce a refusal.
    #[must_use]
    pub fn names_identity_but_expired(
        &self,
        identity: &ToolIdentity,
        workspace: crate::ids::WorkspaceId,
        principal: crate::ids::PrincipalId,
        now: UtcTimestamp,
    ) -> bool {
        &self.identity == identity
            && self.workspace == workspace
            && self.principal == principal
            && self.is_expired_at(now)
    }

    /// Returns whether the grant has expired at `now`.
    #[must_use]
    pub fn is_expired_at(&self, now: UtcTimestamp) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }
}

/// One approval that has been granted, as policy needs to see it.
///
/// A **record** rather than a decision, and the distinction is what makes the check meaningful: an
/// approval states what was approved, and policy re-decides whether it still applies. A type that
/// carried its own `Allow` would be an authorization answer sitting in durable storage, which is what
/// an approval must not become.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// The tool the approval was for.
    pub identity: ToolIdentity,
    /// The principal who gave it.
    pub principal: crate::ids::PrincipalId,
    /// The workspace it applies in.
    pub workspace: crate::ids::WorkspaceId,
    /// A digest over the exact action approved.
    ///
    /// Compared against [`PolicyRequest::action_digest`] rather than interpreted: **the computation is
    /// `jarvis_infrastructure`'s**, because hashing is a concrete implementation this layer must not depend
    /// on, and the canonicalization is
    /// [`crate::tool::canonical`](super::canonical)'s. The type is the domain's, so a malformed or
    /// differently-typed digest is refused where it is accepted rather than compared as text — which is
    /// what makes "a mismatch **fails**" a property of the value instead of a string comparison nobody
    /// validated. The contract's rule is that an approval for one action must not authorize another:
    /// "approving 'send this email' does not approve a rewritten recipient, subject, body, attachment, or
    /// account".
    pub action_digest: super::canonical::ActionDigest,
    /// When the approval stops being valid.
    pub expires_at: UtcTimestamp,
    /// Whether the approval covers **any call to the tool** rather than one exact action.
    ///
    /// A standing approval ("always allow this") is matched on identity, principal and workspace and not
    /// on the action digest, and is never spent by a use. It is still bounded by the grant's own effect
    /// and risk ceilings and by its expiry, and a deny rule still wins over it.
    pub standing: bool,
    /// Whether it has already been spent.
    ///
    /// A one-shot approval is consumed when its call is reserved, so a record that is still present
    /// but spent must not authorize a second call. Checked here rather than left to the reservation
    /// path, because policy is evaluated **before** reservation and an approval that policy allowed
    /// on the strength of a spent record would reach a reservation that then refused it — a refusal
    /// at the wrong layer, with the policy audit claiming the action was permitted.
    pub consumed: bool,
}

/// One deny rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenyRule {
    /// The tool it refuses, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ToolIdentity>,
    /// The principal it refuses, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<crate::ids::PrincipalId>,
    /// The workspace it refuses in, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<crate::ids::WorkspaceId>,
    /// The effects it refuses, if any.
    #[serde(default)]
    pub effects: BTreeSet<Effect>,
}

impl DenyRule {
    /// Returns whether this rule refuses the given request.
    ///
    /// **An empty rule matches nothing, and that is deliberate.** A rule with no fields is what an
    /// unpopulated record produces, and a rule that matched everything would refuse every action in
    /// the profile — a total outage from a construction mistake. The fail-closed direction is the
    /// *default outcome* (nothing is allowed without a grant), not a rule that matches everything.
    #[must_use]
    pub fn matches(&self, request: &PolicyRequest<'_>) -> bool {
        let names_nothing = self.identity.is_none()
            && self.principal.is_none()
            && self.workspace.is_none()
            && self.effects.is_empty();
        if names_nothing {
            return false;
        }
        // Each named dimension must agree; an unnamed one is not a constraint. Effects are checked
        // separately below because they are a set rather than a scalar: naming an effect refuses a
        // tool that *has* it, not one that matches exactly.
        if self
            .identity
            .as_ref()
            .is_some_and(|identity| identity != request.identity)
        {
            return false;
        }
        if self
            .principal
            .is_some_and(|principal| principal != request.principal)
        {
            return false;
        }
        if self
            .workspace
            .is_some_and(|workspace| workspace != request.workspace)
        {
            return false;
        }
        if !self.effects.is_empty()
            && !self
                .effects
                .iter()
                .any(|effect| request.effects.contains(effect))
        {
            return false;
        }
        true
    }
}

/// Everything policy decides on, gathered so the evaluator takes one argument.
#[derive(Debug, Clone)]
pub struct PolicyRequest<'a> {
    /// The exact tool identity being invoked.
    pub identity: &'a ToolIdentity,
    /// The principal asking.
    pub principal: crate::ids::PrincipalId,
    /// The resolved workspace. Resolved server-side, never taken from the call.
    pub workspace: crate::ids::WorkspaceId,
    /// The tool's declared effects.
    pub effects: &'a BTreeSet<Effect>,
    /// The tool's declared risk.
    pub risk: Risk,
    /// The tool's declared scopes.
    pub required_scopes: &'a BTreeSet<Scope>,
    /// The tool's declared default approval posture.
    pub default_approval: super::classification::ApprovalHint,
    /// The classification of the arguments.
    pub sensitivity: Sensitivity,
    /// A digest of the exact action, matching an approval's.
    ///
    /// **By value rather than by reference, and [`ActionDigest`](super::canonical::ActionDigest) is `Copy`**
    /// — a 32-byte value needs no borrow, and owning it means a caller that computed one can pass it
    /// through without a lifetime that ties the request to the computation. The typed value is what stops
    /// this from being a string comparison nobody validated.
    pub action_digest: super::canonical::ActionDigest,
    /// Whether the tool is currently enabled.
    pub tool_enabled: bool,
    /// The run's deadline, if it has one.
    pub deadline: Option<UtcTimestamp>,
}

/// The inputs policy reads, gathered so the evaluator's determinism is visible in its signature.
///
/// Every field is a value rather than a port: no repository, no clock, no environment. `evaluate`
/// therefore cannot do I/O even by accident, which is what makes "deterministic" a property of the
/// types rather than a promise in a comment.
#[derive(Debug, Clone)]
pub struct PolicyInputs<'a> {
    /// The time to evaluate at.
    pub now: UtcTimestamp,
    /// The grants the principal holds. **All** of them, not the matching one: choosing the match is
    /// the evaluator's job, and a caller that pre-filtered could filter wrongly.
    pub grants: &'a [Grant],
    /// Approvals already granted.
    pub approvals: &'a [ApprovalRecord],
    /// Deny rules.
    pub deny_rules: &'a [DenyRule],
}

/// The result of evaluating one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    /// What may happen.
    pub outcome: PolicyOutcome,
    /// Why, in the order the rules were consulted.
    ///
    /// A `Vec` rather than one reason because a refusal can have more than one cause and an operator
    /// fixing only the first would be surprised by the next. The first entry is the deciding rule.
    pub reasons: Vec<PolicyReason>,
    /// The grant that decided it, when a grant was consulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<GrantRef>,
    /// Whether the outcome is final or a later layer may narrow it.
    pub final_outcome: bool,
}

/// Identifies the grant a decision leaned on, without copying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRef {
    /// The principal the grant was issued to.
    pub principal: crate::ids::PrincipalId,
    /// The workspace it applies in.
    pub workspace: crate::ids::WorkspaceId,
}

impl PolicyDecision {
    /// Returns whether the action may proceed without a prompt.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.outcome == PolicyOutcome::Allow
    }

    /// Returns whether the action needs a human decision.
    #[must_use]
    pub fn needs_approval(&self) -> bool {
        self.outcome == PolicyOutcome::Ask
    }

    /// Returns whether the action is refused.
    #[must_use]
    pub fn is_denied(&self) -> bool {
        self.outcome == PolicyOutcome::Deny
    }

    /// Returns the deciding reason, which is the first one recorded.
    #[must_use]
    pub fn reason(&self) -> Option<PolicyReason> {
        self.reasons.first().copied()
    }

    /// Returns the tool error class this decision corresponds to.
    ///
    /// The mapping is here rather than at each call site because getting it wrong produces a message
    /// a user cannot act on: an `Ask` means `tool.approval_required`, which prompts them, while a
    /// `Deny` for a missing grant means `tool.permission_denied`, which does not. An implementation
    /// that returned `permission_denied` for both would tell a user they are not allowed when the
    /// system is waiting to ask them.
    #[must_use]
    pub fn error_class(&self) -> Option<super::error_class::ToolErrorClass> {
        use super::error_class::ToolErrorClass as Class;
        match self.outcome {
            PolicyOutcome::Allow => None,
            PolicyOutcome::Ask => Some(Class::ApprovalRequired),
            PolicyOutcome::Deny => Some(match self.reasons.first() {
                // A refusal caused by an approval that existed and lapsed names the approval, so a
                // user knows to re-approve rather than to seek permission they already have.
                Some(PolicyReason::ApprovalExpired) => Class::ApprovalExpired,
                Some(
                    PolicyReason::ApprovalFingerprintMismatch | PolicyReason::ApprovalConsumed,
                ) => Class::ApprovalRejected,
                _ => Class::PermissionDenied,
            }),
        }
    }
}

/// Returns whether every required scope is present in `held`.
fn missing_scope(held: &BTreeSet<Scope>, required: &BTreeSet<Scope>) -> Option<Scope> {
    required.iter().find(|scope| !held.contains(scope)).cloned()
}

/// Returns a final refusal, which no later step can alter.
fn refusal(reason: PolicyReason) -> PolicyDecision {
    PolicyDecision {
        outcome: PolicyOutcome::Deny,
        reasons: vec![reason],
        grant: None,
        final_outcome: true,
    }
}

/// Resolves which grant applies, distinguishing the three refusals a user reacts to differently.
///
/// Returns either the applicable grant or the reason to refuse. The three refusal cases are separate
/// because their next steps differ: an absent grant means asking for one, an expired grant means
/// re-requesting access that lapsed, and a replacement means the tool they had access to is a
/// different tool now. A single "no grant" would send every one of them to the wrong place.
fn resolve_grant<'a>(
    request: &PolicyRequest<'_>,
    grants: &'a [Grant],
    now: UtcTimestamp,
) -> Result<&'a Grant, PolicyReason> {
    if let Some(grant) = grants
        .iter()
        .find(|grant| grant.applies_at(request.identity, request.workspace, request.principal, now))
    {
        return Ok(grant);
    }
    let expired = grants.iter().any(|candidate| {
        candidate.names_identity_but_expired(
            request.identity,
            request.workspace,
            request.principal,
            now,
        )
    });
    // A replacement is a grant for the **same capability** but a different identity, which is the
    // `ACC-024` case: the grant still names the tool by name while the implementation behind it
    // changed. Only the capability is compared, because the identity difference is what makes it a
    // replacement rather than an unrelated tool.
    let replaced = grants.iter().any(|candidate| {
        candidate.workspace == request.workspace
            && candidate.principal == request.principal
            && candidate.identity.capability == request.identity.capability
            && candidate.identity != *request.identity
    });
    // Order matters: an expired grant is the more specific fact and the more actionable one, so it is
    // reported ahead of a replacement that may also be true.
    Err(if expired {
        PolicyReason::GrantExpired
    } else if replaced {
        PolicyReason::IdentityReplaced
    } else {
        PolicyReason::NoGrant
    })
}

/// Collects every grant constraint the request fails.
///
/// **Collected rather than returned one at a time**, so a refusal lists every problem: a user fixing
/// one at a time discovers the next afterwards, and the list is bounded by the number of checks.
fn failed_constraints(request: &PolicyRequest<'_>, grant: &Grant) -> Vec<PolicyReason> {
    let mut reasons = Vec::new();
    if missing_scope(&grant.scopes, request.required_scopes).is_some() {
        reasons.push(PolicyReason::MissingScope);
    }
    if request
        .effects
        .iter()
        .any(|effect| !grant.effects.contains(effect))
    {
        reasons.push(PolicyReason::EffectNotPermitted);
    }
    if request.risk > grant.risk_ceiling {
        reasons.push(PolicyReason::RiskAboveCeiling);
    }
    if request.sensitivity > grant.sensitivity_ceiling {
        reasons.push(PolicyReason::SensitivityAboveCeiling);
    }
    reasons
}

/// What looking for an approval found.
///
/// **Three cases rather than a `Result`**, and the distinction is load-bearing rather than tidy. A
/// `Result<(), PolicyReason>` cannot express "nothing matched" separately from "something matched and
/// failed", because both are `Err` — and collapsing them is a real defect: the first version of this
/// helper returned `Err(DefaultAsk)` when no approval existed at all, which overrode a tool's
/// declared `Deny` default and turned a refusal into a prompt. A caller had no way to tell, and the
/// mistake was found by a test rather than by reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalOutcome {
    /// An approval permits this exact action.
    Matched,
    /// An approval named this tool and scope but did not permit the action.
    Mismatched(PolicyReason),
    /// No approval named this tool, principal, and workspace at all.
    Absent,
}

/// Finds an approval that permits this exact action.
///
/// The ordering inside is load-bearing and each check's comment says why it is where it is.
fn resolve_approval(
    request: &PolicyRequest<'_>,
    approvals: &[ApprovalRecord],
    now: UtcTimestamp,
) -> ApprovalOutcome {
    let mut most_specific: Option<PolicyReason> = None;
    for approval in approvals {
        if approval.identity != *request.identity
            || approval.principal != request.principal
            || approval.workspace != request.workspace
        {
            continue;
        }
        // The fingerprint is checked **before** expiry and consumption. An approval for a different
        // action is not "an approval that lapsed": reporting expiry for it would tell a user to
        // re-approve, and re-approving the action they already approved would still not match.
        // A standing approval names the tool rather than one action, so it has no digest to compare.
        if !approval.standing && approval.action_digest != request.action_digest {
            most_specific.get_or_insert(PolicyReason::ApprovalFingerprintMismatch);
            continue;
        }
        if approval.consumed {
            most_specific.get_or_insert(PolicyReason::ApprovalConsumed);
            continue;
        }
        if now >= approval.expires_at {
            most_specific.get_or_insert(PolicyReason::ApprovalExpired);
            continue;
        }
        return ApprovalOutcome::Matched;
    }
    match most_specific {
        Some(reason) => ApprovalOutcome::Mismatched(reason),
        None => ApprovalOutcome::Absent,
    }
}

/// Evaluates policy for one request, deterministically.
///
/// The precedence order is fixed and each step's comment says why it is where it is:
///
/// 1. **Deny rules**, because they are the only input with no override. A grant consulted first would
///    make an explicit refusal overridable by a grant, and the architecture lists deny rules as a
///    policy input precisely so that cannot happen.
/// 2. **Tool availability and the run's deadline**, because a call to a disabled tool or on an
///    expired run fails whatever the grants say.
/// 3. **Grant resolution**, which distinguishes "no grant", "grant expired", and "grant names a
///    different implementation" — three refusals a user must react to differently.
/// 4. **Grant constraints**: scopes, effects, risk ceiling, sensitivity ceiling.
/// 5. **Approvals**, which can raise an `Ask` to an `Allow` but can never raise a `Deny`.
/// 6. **The tool's declared default**, with read-only low-risk tools allowed and everything else
///    asking — so nothing is allowed by falling through.
#[must_use]
pub fn evaluate(request: &PolicyRequest<'_>, inputs: &PolicyInputs<'_>) -> PolicyDecision {
    evaluate_at(request, inputs, AutonomyLevel::Ask)
}

/// Evaluates policy under an autonomy level.
///
/// [`evaluate`] is this with [`AutonomyLevel::Ask`], which is the original behaviour, so a caller that has
/// no level to state keeps exactly what it had. The level only widens step 6: an action it
/// [auto-allows](AutonomyLevel::auto_allows) is permitted without a prompt **when the tool does not declare
/// itself `Deny`**, and only after the deny rules, the grant and its constraints have all been satisfied.
#[must_use]
pub fn evaluate_at(
    request: &PolicyRequest<'_>,
    inputs: &PolicyInputs<'_>,
    autonomy: AutonomyLevel,
) -> PolicyDecision {
    let now = inputs.now;

    // Step 1: deny rules first, and their refusal is final. Returning here rather than accumulating
    // reasons is deliberate: nothing later can change a deny, so continuing would only build a list
    // that implies the later steps mattered.
    if inputs.deny_rules.iter().any(|rule| rule.matches(request)) {
        return refusal(PolicyReason::ExplicitDeny);
    }

    // Step 2: availability. A disabled tool is refused before a grant is even consulted, because a
    // grant for a disabled tool is stale configuration rather than permission.
    if !request.tool_enabled {
        return refusal(PolicyReason::ToolDisabled);
    }
    if request.deadline.is_some_and(|deadline| now >= deadline) {
        return refusal(PolicyReason::DeadlinePassed);
    }

    // Step 3: the grant.
    let grant = match resolve_grant(request, inputs.grants, now) {
        Ok(grant) => grant,
        Err(reason) => return refusal(reason),
    };
    let grant_ref = GrantRef {
        principal: grant.principal,
        workspace: grant.workspace,
    };

    // Step 4: constraints.
    let failed = failed_constraints(request, grant);
    if !failed.is_empty() {
        return PolicyDecision {
            outcome: PolicyOutcome::Deny,
            reasons: failed,
            grant: Some(grant_ref),
            final_outcome: true,
        };
    }

    // Step 5: approvals. An approval can only **raise** an `Ask` to an `Allow`; a `Deny` has already
    // returned, so a matching approval can never overturn one.
    let approval_mismatch = match resolve_approval(request, inputs.approvals, now) {
        ApprovalOutcome::Matched => {
            return PolicyDecision {
                outcome: PolicyOutcome::Allow,
                reasons: vec![PolicyReason::ApprovalMatched],
                grant: Some(grant_ref),
                final_outcome: false,
            };
        }
        ApprovalOutcome::Mismatched(reason) => Some(reason),
        // No approval exists for this tool at all. That is **not** a mismatch: there is nothing to
        // explain, so the tool's own default decides and its reason is used instead. Treating it as a
        // mismatch is what turned a tool's `Deny` into an `Ask` in the first version.
        ApprovalOutcome::Absent => None,
    };

    // Step 6: the tool's declared default.
    if request.default_approval != super::classification::ApprovalHint::Deny
        && autonomy.auto_allows(request.effects, request.risk)
    {
        return PolicyDecision {
            outcome: PolicyOutcome::Allow,
            reasons: vec![PolicyReason::AutonomyAllowed],
            grant: Some(grant_ref),
            final_outcome: false,
        };
    }
    finalize_default(request, grant_ref, approval_mismatch)
}

/// Applies the tool's declared default, which is the last word when nothing else decided.
///
/// Extracted from [`evaluate`] rather than inlined, because the default is the step whose subtlety is
/// easiest to lose in a long function: an `Allow` here requires a **positive** property (read-only
/// *and* low risk), so a tool that declares `allow` while doing something consequential is asked
/// about anyway.
fn finalize_default(
    request: &PolicyRequest<'_>,
    grant_ref: GrantRef,
    approval_mismatch: Option<PolicyReason>,
) -> PolicyDecision {
    let read_only_and_low_risk = request.effects.contains(&Effect::ReadOnly)
        && !request
            .effects
            .iter()
            .any(|effect| effect.is_consequential())
        && request.risk <= Risk::Low;
    // `Allow` and `Ask` produce the same *shape* of decision, and the arms are merged rather than
    // repeated so the difference between them is the condition rather than a duplicated body — two
    // identical bodies are how one of them drifts from the other.
    let asks = match request.default_approval {
        super::classification::ApprovalHint::Allow if read_only_and_low_risk => {
            return PolicyDecision {
                outcome: PolicyOutcome::Allow,
                reasons: vec![PolicyReason::LowRiskReadOnly],
                grant: Some(grant_ref),
                final_outcome: false,
            };
        }
        // A tool that asks to be allowed without asking the user, and is not a low-risk read, is
        // **asked about anyway**. The declared default is a hint (the contract says so), and a hint
        // is not enough to send an email unprompted.
        super::classification::ApprovalHint::Allow | super::classification::ApprovalHint::Ask => {
            true
        }
        super::classification::ApprovalHint::Deny => false,
    };
    PolicyDecision {
        outcome: if asks {
            PolicyOutcome::Ask
        } else {
            PolicyOutcome::Deny
        },
        reasons: vec![approval_mismatch.unwrap_or(if asks {
            PolicyReason::DefaultAsk
        } else {
            PolicyReason::DefaultDeny
        })],
        grant: Some(grant_ref),
        // A tool-declared deny is final; an ask is not, since a layer above may still narrow it.
        final_outcome: !asks,
    }
}
