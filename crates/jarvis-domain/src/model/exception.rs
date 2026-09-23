//! Policy exceptions: durable, separately revocable relaxations of a specific rule.
//!
//! `docs/contracts/model-data-policy.md` states the rule this module exists to make
//! structural: "Policy exceptions are separate durable records; editing a request body
//! cannot create one." An exception therefore has its own identity, its own lifecycle, and
//! its own revocation, and a route that relied on one records the reference — which is what
//! [`crate::model::policy::ModelRouteDecision::relied_on_exception`] reads.
//!
//! Four decisions are load-bearing:
//!
//! - **An exception relaxes exactly one named rule.** `rule` is a
//!   [`PolicyRuleKey`], not a free-form description, so an exception cannot silently widen
//!   from the rule it was granted for to another. A stored exception is checked against the
//!   rule its candidate actually failed, and a mismatch is not a relaxation at all.
//! - **A non-waivable rule cannot be named by a grant at all.** "Exceptions cannot override
//!   non-waivable legal/administrator denies" is enforced by [`PolicyRuleKey::is_waivable`]
//!   and [`PolicyException::grant`] refusing the whole record, rather than by a check at each
//!   use site. The sensitivity ceiling and the two allow-lists are non-waivable *here*, in the
//!   type, because the ceiling already has no second spelling above the route path and a
//!   waivable ceiling would undo that.
//! - **Unusable states are one predicate, and it is asked at the decision instant.**
//!   [`PolicyException::is_usable_at`] answers "may this permit a call *now*", so a revoked,
//!   consumed, or expired exception cannot be used by a caller that forgot one of the three
//!   cases. [`PolicyException::state_at`] reports *which* it is, so an operator is not told
//!   "expired" for a revocation.
//! - **Single use is a property of the record, not a flag on the call.** `consumed_at`
//!   participates in the same predicate, so a consumed exception is unusable without the
//!   caller having to know it was single-use.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{PolicyExceptionId, PrincipalId, WorkspaceId};
use crate::model::identity::{ModelRef, Region};
use crate::model::policy::{Locality, PolicyVersionRef, Sensitivity};
use crate::time::UtcTimestamp;

/// The strongest identity assurance a grant may require a caller to have held.
///
/// Mirrors the application layer's `AuthenticationAssurance` as a **domain** value, because
/// the step-up rule is a policy concept rather than a transport one: the contract states
/// "Sensitive or cross-border exceptions require policy-defined step-up/approval", and which
/// grants need it is a property of the rule, not of the channel the request arrived on.
///
/// Declared here rather than imported from `jarvis-application` because the dependency runs
/// application -> domain and the ordering means the domain cannot name it.
///
/// **No mapping exists yet, and this type currently has no producer above the domain.** A route
/// to grant an exception over HTTP does not exist, `PolicyService::grant_exception` takes this
/// value directly from its caller, and `AuthenticatedClient` hardcodes
/// `AuthenticationAssurance::Standard` — so no HTTP client can ever be `Elevated`, and every
/// step-up grant is un-grantable over the wire. Recording the absence rather than an intended
/// mapping: a doc comment claiming a boundary translation that nothing performs is the same
/// shape as a column with no writer, and it reads as coverage until somebody greps for the
/// function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredAssurance {
    /// An ordinary verified credential is enough.
    Standard,
    /// The caller must have completed a step-up challenge.
    Elevated,
}

impl RequiredAssurance {
    /// Returns whether `held` satisfies this requirement.
    ///
    /// A ladder rather than an equality, so an elevated caller satisfies a standard grant —
    /// the reverse would refuse a more strongly authenticated caller, which is a refusal no
    /// operator would want and one that would push toward weakening the grant.
    ///
    /// Written as a `match` rather than `held >= self`, because `Ord`'s comparison operators
    /// are not `const`-callable on this toolchain (the same reason `min()` cannot be used in a
    /// `const fn` elsewhere in this workspace). The arm table is exhaustive, so a third
    /// assurance level would fail to compile here instead of silently inheriting a comparison.
    #[must_use]
    pub const fn is_satisfied_by(self, held: RequiredAssurance) -> bool {
        match (self, held) {
            // The two satisfied cases are one arm because they are the same answer, not by
            // coincidence: every requirement is met by an elevated holder, and a standard one is met
            // only by a standard holder. The remaining arm is the sole refusal, and it is written out
            // rather than left to a `_` so a third assurance level fails to compile here.
            (Self::Standard, Self::Standard | Self::Elevated)
            | (Self::Elevated, Self::Elevated) => true,
            (Self::Elevated, Self::Standard) => false,
        }
    }
}

/// The longest accepted exception reason.
///
/// Bounded because the reason reaches an operator display and a durable row, and it is
/// explicitly *not* content: the contract requires an exception to be included in the route
/// decision/audit "without exposing content", so the reason is a short operator-authored
/// label rather than anything derived from a request.
pub const MAX_EXCEPTION_REASON_BYTES: usize = 1_024;

/// Which policy rule an exception relaxes.
///
/// A closed enum rather than a string, for the same reason every rule vocabulary in this
/// workspace is closed: a free-form key could name a rule nothing evaluates, so an exception
/// could be granted for a rule that is never checked and appear to authorize something it
/// does not.
///
/// The set is deliberately the **whole** rule vocabulary rather than only the waivable part.
/// Splitting it would make "an exception for the sensitivity ceiling" unrepresentable, and an
/// unrepresentable mistake is not the same as a refused one: the grant path has to *receive*
/// such a request in order to answer "that rule cannot be relaxed by an exception", and
/// [`PolicyException::grant`] is where that answer lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRuleKey {
    /// The locality ceiling (`policy.rules.locality`).
    Locality,
    /// The provider allow-list.
    AllowedProviders,
    /// The model allow-list.
    AllowedModels,
    /// The required documented retention behavior.
    MaximumProviderRetention,
    /// The required documented training-use behavior.
    ProviderTrainingUse,
    /// Whether telemetry may be emitted.
    Telemetry,
    /// The allowed residency regions.
    ResidencyRegions,
    /// The data-classification ceiling.
    MaximumSensitivity,
    /// Whether a compliant fallback is permitted.
    AllowFallback,
}

impl PolicyRuleKey {
    /// Returns the contract-shaped name, which is also the stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Locality => "locality",
            Self::AllowedProviders => "allowed_providers",
            Self::AllowedModels => "allowed_models",
            Self::MaximumProviderRetention => "maximum_provider_retention",
            Self::ProviderTrainingUse => "provider_training_use",
            Self::Telemetry => "telemetry",
            Self::ResidencyRegions => "residency_regions",
            Self::MaximumSensitivity => "maximum_sensitivity",
            Self::AllowFallback => "allow_fallback",
        }
    }

    /// Parses the stored value.
    ///
    /// Returns `None` for anything else, so a reader decides what an unknown key means
    /// rather than this function inventing one — an unrecognized rule key must not be
    /// treated as "relaxes nothing" and silently ignored, nor as "relaxes everything".
    #[must_use]
    pub fn from_stored(value: &str) -> Option<Self> {
        [
            Self::Locality,
            Self::AllowedProviders,
            Self::AllowedModels,
            Self::MaximumProviderRetention,
            Self::ProviderTrainingUse,
            Self::Telemetry,
            Self::ResidencyRegions,
            Self::MaximumSensitivity,
            Self::AllowFallback,
        ]
        .into_iter()
        .find(|key| key.as_str() == value)
    }

    /// Returns whether an exception may relax this rule.
    ///
    /// Three rules are **not** waivable, and each for its own reason:
    ///
    /// - `MaximumSensitivity` is the ceiling the selector enforces before any candidate is
    ///   examined. Making it waivable would give the ceiling the second spelling that check
    ///   exists to prevent, and the contract places the data-classification ceiling in the
    ///   *workspace* layer, which a task-scoped exception cannot lift.
    /// - `AllowedProviders` and `AllowedModels` are administrator deny/short lists. The
    ///   contract's "exceptions cannot override non-waivable legal/administrator denies" names
    ///   exactly this case: an operator's deliberate exclusion is not something a scoped
    ///   exception lifts.
    ///
    /// `Telemetry` is waivable because it is a disclosure preference rather than a deny:
    /// "emit local telemetry for this call" is a choice an operator can grant, while
    /// "nothing may be recorded" is a floor `Disabled` already is.
    #[must_use]
    pub const fn is_waivable(self) -> bool {
        match self {
            Self::Locality
            | Self::MaximumProviderRetention
            | Self::ProviderTrainingUse
            | Self::Telemetry
            | Self::ResidencyRegions
            | Self::AllowFallback => true,
            Self::AllowedProviders | Self::AllowedModels | Self::MaximumSensitivity => false,
        }
    }

    /// Returns whether a grant of this rule always requires a step-up challenge.
    ///
    /// The contract: "Sensitive or cross-border exceptions require policy-defined
    /// step-up/approval." Two rules are cross-border by construction, and both are named here
    /// rather than left to a caller's judgement:
    ///
    /// - `Locality` moves content across a boundary it was not permitted to cross — a
    ///   local-only policy relaxed to a private network or a cloud endpoint is exactly the
    ///   "move private/local-only data to cloud" transition the architecture forbids silently.
    /// - `ResidencyRegions` admits a processing region the policy excluded, which is the
    ///   cross-border case the sentence names.
    ///
    /// The remaining waivable rules are not automatically stepped up: relaxing a retention or
    /// training requirement accepts a provider's published default, which is a disclosure
    /// decision an operator makes explicitly rather than a boundary crossed by accident.
    /// A *sensitive* grant — one whose scope raises the classification — carries its own
    /// requirement through [`ExceptionScope::maximum_sensitivity`] being present, which
    /// [`PolicyException::grant`] turns into [`RequiredAssurance::Elevated`].
    #[must_use]
    pub const fn requires_step_up(self) -> bool {
        matches!(self, Self::Locality | Self::ResidencyRegions)
    }
}

impl fmt::Display for PolicyRuleKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The lifecycle state of an exception.
///
/// A four-way answer rather than a boolean, because "may this be used" and "why not" are
/// different questions and an operator asked to re-grant an exception needs the second. The
/// order matches the predicate in [`PolicyException::state_at`]: revocation and consumption
/// are terminal decisions, while expiry is a deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionState {
    /// Granted, unexpired, not revoked, not consumed: it may permit a call.
    Issued,
    /// Its expiry instant has passed.
    Expired,
    /// It was revoked before it could be used.
    Revoked,
    /// A single-use exception that has already permitted one call.
    Consumed,
}

impl ExceptionState {
    /// Returns the contract-shaped name, which is also the stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Issued => "issued",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::Consumed => "consumed",
        }
    }

    /// Returns whether an exception in this state may permit a call.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Issued)
    }
}

/// What an exception applies to, beyond its rule.
///
/// The contract's "provider/model/task/resource selectors" and "maximum scope". Every
/// selector is optional, and an absent selector means "no restriction on this axis" —
/// which is the *wider* reading, so a grant that names nothing is a workspace-wide
/// relaxation of its rule. That direction is deliberate: the caller must state what it is
/// narrowing *to*, because an exception is the one mechanism that relaxes a rule, and a
/// default that narrowed further than the caller asked would be a silent surprise in the
/// dangerous direction.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExceptionScope {
    /// The provider this exception applies to, when it is not every provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// The model this exception applies to, when it is not every model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// The region this exception applies to, when it is not every region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// The most sensitive content this exception permits, when the exception names a ceiling.
    ///
    /// Present only for a relaxation that *sets* a classification, and absent for one that
    /// does not. An exception that relaxed locality must not also be read as raising the
    /// sensitivity ceiling, so the two are separate fields rather than one "value".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_sensitivity: Option<Sensitivity>,
    /// The locality this exception permits, when the exception names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locality: Option<Locality>,
}

impl ExceptionScope {
    /// Returns whether this scope covers `model`, when the scope names a model.
    ///
    /// A scope that names a model covers only that model, and one that names none covers all
    /// of them — the same reading as the provider axis.
    #[must_use]
    pub fn covers(&self, model: &ModelRef) -> bool {
        let provider_ok = self
            .provider_id
            .as_ref()
            .is_none_or(|id| id == model.provider_id.as_str());
        let model_ok = self
            .model_id
            .as_ref()
            .is_none_or(|id| id == model.model_id.as_str());
        provider_ok && model_ok
    }
}

/// A durable, separately revocable relaxation of one policy rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyException {
    /// Its own identity, carried by every decision that relied on it.
    pub id: PolicyExceptionId,
    /// The workspace that owns it. Scope is a field, never inferred.
    pub workspace_id: WorkspaceId,
    /// The policy version it was granted against.
    ///
    /// Recorded like every other policy reference in this workspace: an exception granted
    /// under one version's rules must not silently apply to a different version's.
    pub policy: PolicyVersionRef,
    /// The principal that granted it, and thus the principal accountable for it.
    pub granting_principal_id: PrincipalId,
    /// The single rule it relaxes.
    pub rule: PolicyRuleKey,
    /// What it applies to.
    pub scope: ExceptionScope,
    /// An operator-authored label for why it was granted.
    ///
    /// Bounded and never content: the contract requires an exception to be auditable
    /// "without exposing content", and a reason copied from a request would put caller text
    /// in a durable audit row.
    pub reason_ref: String,
    /// The assurance the granting principal had to hold for this grant to be valid.
    ///
    /// Recorded on the record rather than recomputed from the rule, because the requirement
    /// is part of what was approved: an operator who stepped up for a locality grant approved
    /// *that* relaxation, and re-deriving the requirement later would let a rule's step-up
    /// policy change retroactively alter what a past grant meant.
    pub required_assurance: RequiredAssurance,
    /// Whether one use consumes it.
    pub single_use: bool,
    /// When it was granted.
    pub issued_at: UtcTimestamp,
    /// When it stops being usable. Always present: "exceptions expire by default".
    pub expires_at: UtcTimestamp,
    /// When it was revoked, if it was.
    pub revoked_at: Option<UtcTimestamp>,
    /// When it was consumed, if it was.
    pub consumed_at: Option<UtcTimestamp>,
}

/// Everything needed to grant one exception.
///
/// A struct rather than an eleven-argument constructor, and the reason is not only the lint: two
/// of the arguments are `UtcTimestamp` and two more are strings, so a positional call is where the
/// issue instant and the expiry — or the reason and a scope identifier — get transposed without the
/// compiler noticing. Naming each field at the call site is what makes that impossible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPolicyException {
    /// The exception's own identity, generated by the caller so a test can be deterministic.
    pub id: PolicyExceptionId,
    /// The workspace that owns it. Scope is a field, never inferred.
    pub workspace_id: WorkspaceId,
    /// The policy version it is granted against.
    pub policy: PolicyVersionRef,
    /// The principal accountable for it.
    pub granting_principal_id: PrincipalId,
    /// The assurance that principal actually held.
    pub granting_assurance: RequiredAssurance,
    /// The single rule it relaxes.
    pub rule: PolicyRuleKey,
    /// What it applies to, and the value it permits.
    pub scope: ExceptionScope,
    /// An operator-authored label for why it was granted.
    pub reason_ref: String,
    /// Whether one use consumes it.
    pub single_use: bool,
    /// When it was granted.
    pub issued_at: UtcTimestamp,
    /// When it stops being usable.
    pub expires_at: UtcTimestamp,
}

impl PolicyException {
    /// Grants an exception from `request`.
    ///
    /// `request.granting_assurance` is the assurance the granting principal actually held, and the
    /// step-up rule is enforced here: a grant whose rule requires step-up, or whose scope raises a
    /// classification, is refused unless the principal held [`RequiredAssurance::Elevated`]. The
    /// refusal is at grant time because the alternative — issuing a grant that records an elevated
    /// requirement while the principal was only standard — would store a record whose own field says
    /// it should not exist.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ExceptionRequired`] when `rule` is not waivable, because the refusal
    /// has to happen where the record would be created: a grant that was stored and then ignored at
    /// evaluation time would leave an operator believing a rule had been relaxed when it had not.
    ///
    /// Returns [`DomainError::ExceptionRequired`] when the grant needs step-up and
    /// `granting_assurance` is not elevated. It shares the code with the non-waivable refusal
    /// deliberately: both are "this exception cannot be granted as asked", and the operator's
    /// remedy — obtain approval, or change the grant — is the same shape.
    ///
    /// Returns [`DomainError::InvalidTransitionReason`] when the reason is empty, over
    /// [`MAX_EXCEPTION_REASON_BYTES`], or contains a NUL byte — the same bound every other
    /// operator-authored label in this workspace carries, so a long reason cannot become an
    /// unbounded durable value.
    pub fn grant(request: NewPolicyException) -> Result<Self, DomainError> {
        let NewPolicyException {
            id,
            workspace_id,
            policy,
            granting_principal_id,
            granting_assurance,
            rule,
            scope,
            reason_ref,
            single_use,
            issued_at,
            expires_at,
        } = request;

        if !rule.is_waivable() {
            return Err(DomainError::ExceptionRequired);
        }
        if reason_ref.is_empty()
            || reason_ref.len() > MAX_EXCEPTION_REASON_BYTES
            || reason_ref.contains('\0')
        {
            return Err(DomainError::InvalidTransitionReason);
        }
        // An expiry at or before the issue instant is unusable the moment it is created.
        // Refused rather than stored, because such a record reads as a real relaxation in an
        // audit while permitting nothing.
        if expires_at <= issued_at {
            return Err(DomainError::ExceptionExpired);
        }

        // The step-up requirement is derived from the rule and the scope, recorded on the
        // record, and enforced against what the principal held. Both inputs matter: a locality
        // grant crosses a boundary by construction, while a grant that names a classification
        // is "sensitive" in the contract's own words even when its rule does not cross one.
        let required_assurance = if rule.requires_step_up() || scope.maximum_sensitivity.is_some() {
            RequiredAssurance::Elevated
        } else {
            RequiredAssurance::Standard
        };
        if !required_assurance.is_satisfied_by(granting_assurance) {
            return Err(DomainError::ExceptionRequired);
        }

        Ok(Self {
            id,
            workspace_id,
            policy,
            granting_principal_id,
            rule,
            scope,
            reason_ref,
            required_assurance,
            single_use,
            issued_at,
            expires_at,
            revoked_at: None,
            consumed_at: None,
        })
    }

    /// Returns this exception's state at `now`.
    ///
    /// Asked with an explicit instant rather than read from a clock, so a decision can be
    /// replayed and explained at the moment it happened. Revocation and consumption are
    /// checked before expiry because they are decisions someone took, while expiry is the
    /// passage of time; an operator asking "why is this unusable" is better served by the
    /// decision than by the deadline.
    #[must_use]
    pub fn state_at(&self, now: UtcTimestamp) -> ExceptionState {
        if self.revoked_at.is_some() {
            return ExceptionState::Revoked;
        }
        if self.consumed_at.is_some() {
            return ExceptionState::Consumed;
        }
        // The expiry instant itself is expired, the same convention the run deadline uses:
        // an exception that stops being usable at `T` cannot permit a call made at `T`,
        // because that call would finish after the relaxation ended.
        if now >= self.expires_at {
            return ExceptionState::Expired;
        }
        ExceptionState::Issued
    }

    /// Returns whether this exception may permit a call at `now`.
    #[must_use]
    pub fn is_usable_at(&self, now: UtcTimestamp) -> bool {
        self.state_at(now).is_usable()
    }

    /// Returns whether this exception relaxes `rule`.
    ///
    /// The one-rule property is what stops an exception granted for retention from being
    /// read as also permitting a locality it never named. Expressed as a discriminant
    /// comparison rather than a `match` with one arm per variant, because a hand-written arm
    /// table is where a new rule key gets forgotten — and a forgotten arm would make the new
    /// rule permanently unrelaxable in a way no test would notice.
    #[must_use]
    pub const fn relaxes(&self, rule: PolicyRuleKey) -> bool {
        self.rule as u8 == rule as u8
    }
}

#[cfg(test)]
#[path = "exception_tests.rs"]
mod tests;
