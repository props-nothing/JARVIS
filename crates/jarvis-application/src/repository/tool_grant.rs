//! The durable tool-grant store: the port that makes authorization configurable.
//!
//! `TLS-015` names this store, and `jarvis-infrastructure`'s `NativeReadOnlyGrants` was written to be the
//! thing it replaces. Until this port existed, what a deployment allowed was a **constructor**: the only
//! way to change it was to change the code, so "let `files.read` work without asking but always ask about
//! `email.send`" was not a sentence the product could be told.
//!
//! ## Four rules this port enforces rather than documents
//!
//! 1. **A grant may only narrow.** [`NewToolGrant::narrowing`] takes the definition the grant is for and
//!    refuses a grant that confers a scope the tool does not require, permits an effect the tool does not
//!    declare, or raises a ceiling above the tool's own risk or input sensitivity. A grant is the
//!    operator's answer to *"how much of this tool may be used without asking"*, and an answer larger than
//!    the tool itself is a configuration mistake that would otherwise sit in the store looking like
//!    permission. This is the same invariant the plugin carry-forward rule states for updates.
//! 2. **A capability, not an identity, is the key.** `ToolIdentity` includes the schema fingerprint, so a
//!    grant keyed by it would read as *absent* after a tool was recompiled rather than as *replaced* — and
//!    `resolve_grant` distinguishes those two because they lead an operator to different next steps. The
//!    identity is stored and compared, never trusted as the key.
//! 3. **Revocation is a state, not a delete.** The approval contract requires that revoking "cannot erase
//!    historical audit", so a revoked grant stays readable with its `status` changed. A delete would make
//!    a past decision unexplainable, which is the rule the policy version's archive state already follows.
//! 4. **Every read is workspace-scoped.** A grant belonging to another workspace is
//!    [`RepositoryError::NotFound`] — indistinguishable from one that does not exist, the rule the local
//!    control API states for runs and the three other stores follow. A grant is the most security-relevant
//!    row in the profile, so a distinguishable "forbidden" would leak the existence of another workspace's
//!    configuration.
//!
//! ## Why the write is an upsert-at-a-version rather than an insert
//!
//! The natural key is `(workspace, principal, capability)`, so there is one row per grant *identity* and
//! editing its ceilings is an `UPDATE`. The caller states the version it is replacing, and a mismatch is
//! [`RepositoryError::VersionConflict`] — the same fact the run and policy stores report, and the reason
//! two operators editing one grant concurrently cannot silently overwrite each other. A caller that meant
//! to create is refused when a row already exists, so "I thought I was adding a grant" and "I was
//! replacing one" are distinguishable rather than both succeeding.

use std::collections::BTreeSet;

use jarvis_domain::ids::{PrincipalId, ToolGrantId, WorkspaceId};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::classification::{Effect, Risk, Scope};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::identity::ToolIdentity;
use jarvis_domain::tool::policy::{DenyRule, Grant};

use super::{RepositoryError, RepositoryFuture};

/// The longest accepted operator note or deny reason.
///
/// Bounded because it is caller-supplied text that reaches an operator display and a stored row. Kept
/// generous: a reason a user reads is allowed to be a sentence.
pub const MAX_GRANT_NOTE_BYTES: usize = 512;

/// The largest number of grants one page may return.
///
/// A bound on a page rather than on a total, for the reason the approval and run scans record: a caller
/// that cannot tell a full page from an exhausted one concludes there is nothing more to administer. The
/// adapter applies this itself so a caller cannot request an unbounded scan of a table that grows with
/// real configuration.
pub const MAX_GRANT_PAGE: u32 = 200;

/// What a caller asks a grant to be.
///
/// **Built through [`Self::narrowing`], not constructed field by field at a call site**, and that is the
/// whole point of the type: the narrowing rule is the one thing that must not be skippable, and a struct
/// literal would let a handler write ceilings the tool does not have. A caller that genuinely needs a
/// partial update reads the stored grant, changes the fields, and validates the result against the
/// definition again — which is what makes "the grant I am writing still only narrows" true on every write
/// rather than only on creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToolGrant {
    /// The workspace that owns it.
    pub workspace_id: WorkspaceId,
    /// The principal the grant is *for*.
    pub principal_id: PrincipalId,
    /// The exact tool identity the grant was issued against, read back and compared by the evaluator.
    pub identity: ToolIdentity,
    /// The scopes conferred. May be a subset of what the tool requires; an empty set is legal.
    pub scopes: BTreeSet<Scope>,
    /// The effects permitted. Must be a subset of the tool's declared effects.
    pub effects: BTreeSet<Effect>,
    /// The greatest risk permitted. Must not exceed the tool's own declared risk.
    pub risk_ceiling: Risk,
    /// The most sensitive argument permitted. Must not exceed the tool's declared input sensitivity.
    pub sensitivity_ceiling: Sensitivity,
    /// When it stops applying, or `None` for a standing grant.
    pub expires_at: Option<UtcTimestamp>,
    /// The operator who configured it, for the audit trail.
    pub granted_by: PrincipalId,
    /// When it was written.
    pub created_at: UtcTimestamp,
    /// The version the caller believes it is replacing, or `None` to create.
    ///
    /// `None` means "create" and is refused when a row already exists; `Some(n)` means "replace version
    /// `n`" and is refused when the stored version differs. Making this one field rather than an enum is
    /// deliberate — both cases carry a version for the *stored* row, and the difference is only whether
    /// the caller expects one to be there, which the error already reports.
    pub expected_version: Option<u32>,
}

impl NewToolGrant {
    /// Builds a grant that only narrows `definition`, refusing one that would widen it.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] naming the field that widened:
    /// `grant_scope` for a scope the tool does not require, `grant_effect` for an effect it does not
    /// declare, `grant_risk_ceiling` for a ceiling above its risk, and `grant_sensitivity_ceiling` for a
    /// ceiling above its input sensitivity. Each is a separate name because an operator fixing one needs
    /// to know which, and a single "invalid grant" would send them to re-read the whole form.
    ///
    /// The direction is the load-bearing part: a grant **narrower** than the tool is ordinary and useful
    /// ("let this read `~/notes`, not the whole home directory"), while a grant **wider** than the tool is
    /// a mistake that would sit in the store looking like permission for something the tool cannot do.
    #[allow(clippy::too_many_arguments)]
    pub fn narrowing(
        definition: &ToolDefinition,
        workspace_id: WorkspaceId,
        principal_id: PrincipalId,
        scopes: BTreeSet<Scope>,
        effects: BTreeSet<Effect>,
        risk_ceiling: Risk,
        sensitivity_ceiling: Sensitivity,
        expires_at: Option<UtcTimestamp>,
        granted_by: PrincipalId,
        created_at: UtcTimestamp,
        expected_version: Option<u32>,
    ) -> Result<Self, RepositoryError> {
        // A scope the tool does not require is refused rather than ignored: a definition declares the
        // scopes it needs and the evaluator checks `missing_scope`, so conferring an extra one is either a
        // typo or an attempt to grant authority the tool never asked for. Both should be visible.
        let declared_scopes: BTreeSet<&Scope> = definition.required_scopes.iter().collect();
        if scopes.iter().any(|scope| !declared_scopes.contains(scope)) {
            return Err(RepositoryError::Conflict {
                what: "grant_scope",
            });
        }
        // The effects are the one dimension where a **narrower** grant is the norm, so only widening is
        // refused. A tool that declares `{ReadOnly, Write}` may be granted `{ReadOnly}` alone — that is
        // the useful direction and the reason this check is a subset test rather than an equality.
        if effects
            .iter()
            .any(|effect| !definition.effects.contains(effect))
        {
            return Err(RepositoryError::Conflict {
                what: "grant_effect",
            });
        }
        if risk_ceiling > definition.risk {
            return Err(RepositoryError::Conflict {
                what: "grant_risk_ceiling",
            });
        }
        if sensitivity_ceiling > definition.data_classes.input {
            return Err(RepositoryError::Conflict {
                what: "grant_sensitivity_ceiling",
            });
        }
        Ok(Self {
            workspace_id,
            principal_id,
            identity: definition.identity.clone(),
            scopes,
            effects,
            risk_ceiling,
            sensitivity_ceiling,
            expires_at,
            granted_by,
            created_at,
            expected_version,
        })
    }

    /// Returns the capability this grant is keyed by.
    ///
    /// Derived from the identity rather than stored separately, so the key and the stored identity cannot
    /// disagree — a second field could be written with a capability that did not match the identity's, and
    /// the unique index would then deduplicate the wrong pairs.
    #[must_use]
    pub fn capability(&self) -> String {
        self.identity.capability.to_string()
    }

    /// Returns the domain grant the evaluator consumes.
    ///
    /// **Constructed here rather than in the evaluator**, so a stored row and a reviewed configuration
    /// produce the same type by the same path: `Grant`'s fields are public and a second construction site
    /// could set `expires_at` from a different source, which would make a stored expiry and a served one
    /// disagree about when access lapsed.
    #[must_use]
    pub fn as_grant(&self) -> Grant {
        Grant {
            identity: self.identity.clone(),
            workspace: self.workspace_id,
            principal: self.principal_id,
            scopes: self.scopes.clone(),
            effects: self.effects.clone(),
            risk_ceiling: self.risk_ceiling,
            sensitivity_ceiling: self.sensitivity_ceiling,
            expires_at: self.expires_at,
        }
    }
}

/// A grant as stored, with the lifecycle state a reader needs.
///
/// The lifecycle is not on [`Grant`] because the evaluator must never see it: `Grant::applies_at` answers
/// whether a grant authorizes a call, and a revoked grant is *absent* from that question rather than a
/// grant with a flag. Keeping `status` here means the adapter filters by it in the query, so a revoked row
/// cannot reach policy even by a code path that forgot to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredToolGrant {
    /// The row's own identity.
    pub id: ToolGrantId,
    /// The stored grant, which is what the evaluator consumes.
    pub grant: Grant,
    /// Whether it is currently in force.
    pub active: bool,
    /// The optimistic version a writer must match to replace it.
    pub version: u32,
    /// The operator who configured it.
    pub granted_by: PrincipalId,
    /// When it was written.
    pub created_at: UtcTimestamp,
    /// When it last changed.
    pub updated_at: UtcTimestamp,
}

/// One page of grants, and whether the store stopped at its bound.
///
/// `bounded` travels to the caller rather than being inferred from `grants.len() == limit`, for the reason
/// the approval, run, and ledger scans record: the inference silently becomes wrong if the bound changes,
/// and a store that returned exactly its limit for an exhausted table would satisfy it — making "there may
/// be more" look identical to "that is all".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantPage {
    /// The grants in this page, in the store's own (capability) order.
    pub grants: Vec<StoredToolGrant>,
    /// Whether the store stopped at its bound with rows still unread.
    pub bounded: bool,
}

/// The narrowing a listing applies beyond its workspace.
///
/// One value rather than a positional argument, for the reason `ApprovalListFilter` records: the listing
/// already takes a workspace, a principal, and a limit, and adding a scalar filter positionally invites a
/// transposition the compiler cannot see. Both filters are **query predicates rather than post-filters**,
/// or a page would be spent on rows the caller excluded and come back short.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GrantListFilter {
    /// Restrict to grants for one principal, when set.
    pub principal: Option<PrincipalId>,
    /// Restrict to active or inactive grants, when set.
    pub active: Option<bool>,
}

/// A durable store of tool grants and deny rules.
///
/// **`grants_for` is the method the tool pipeline calls on every dispatch**, and it is deliberately a
/// *read of what applies* rather than a pre-matched answer: the source filters by the two dimensions it
/// owns (principal and workspace) and returns the rest for the evaluator to check. An adapter that
/// pre-matched identity, scopes, effects, and ceilings here would be a second authorization rule beside
/// the domain's — the defect `jarvis-domain::tool::policy` names in its own header.
pub trait ToolGrantRepository: Send + Sync {
    /// Returns every grant currently in force for one principal in one workspace.
    ///
    /// **Revoked and expired rows are excluded by the query, not by the caller.** A source that returned
    /// them and relied on `Grant::applies_at` to filter would still be correct for expiry — but a revoked
    /// grant is not "not yet applicable", it is *withdrawn*, and a reader that had to remember the
    /// difference is one bug away from re-authorizing it.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] when the store cannot be read.
    fn grants_for(
        &self,
        workspace: WorkspaceId,
        principal: PrincipalId,
    ) -> RepositoryFuture<'_, Vec<Grant>>;

    /// Returns every deny rule that could apply in one workspace, in insertion order.
    ///
    /// **Returns [`StoredDenyRule`] rather than [`DenyRule`]**, because a stored rule names a *capability*
    /// and the evaluator compares *identities*: the translation is the adapter's, and it cannot be done
    /// from a `DenyRule` alone — that type's `identity` field would have to be invented. Handing back the
    /// domain shape would therefore force the very thing this module's header records as the defect, a
    /// rule stored by identity that stops refusing after a tool is recompiled.
    ///
    /// Unscoped by principal, because a rule may name no principal at all and would then apply to
    /// everyone — filtering by principal in the query would drop the broadest refusals, which is the
    /// direction that loses a restriction. The evaluator already treats an unnamed dimension as "not a
    /// constraint".
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] when the store cannot be read.
    fn deny_rules(&self, workspace: WorkspaceId) -> RepositoryFuture<'_, Vec<StoredDenyRule>>;

    /// Loads one grant within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for a grant that is absent **or** owned by another workspace,
    /// and [`RepositoryError::Corrupted`] when a stored value cannot be interpreted.
    fn load(
        &self,
        workspace: WorkspaceId,
        id: ToolGrantId,
    ) -> RepositoryFuture<'_, StoredToolGrant>;

    /// Stores a grant, creating it or replacing the version the caller named.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] naming `grant_exists` when `expected_version` is `None` and a
    /// row already exists for the key, [`RepositoryError::VersionConflict`] when a version was named and
    /// the stored one differs, and [`RepositoryError::NotFound`] when a version was named and no row
    /// exists.
    fn put(&self, grant: &NewToolGrant, at: UtcTimestamp) -> RepositoryFuture<'_, StoredToolGrant>;

    /// Withdraws a grant, keeping the row for audit.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign grant, and
    /// [`RepositoryError::VersionConflict`] when `expected` is not the stored version. Revoking an
    /// already-revoked grant is **not** an error: the desired state is the current one, which is the same
    /// idempotency rule the approval decision follows for a repeated decision.
    fn revoke(
        &self,
        workspace: WorkspaceId,
        id: ToolGrantId,
        expected: u32,
        at: UtcTimestamp,
    ) -> RepositoryFuture<'_, StoredToolGrant>;

    /// Returns a page of grants in `workspace`, in capability order.
    ///
    /// Ordered by capability rather than by insertion, so a listing is stable across calls and an operator
    /// comparing two reads sees the same order — the property the tool registry's own enumeration records.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] when the store cannot be read.
    fn list(
        &self,
        workspace: WorkspaceId,
        filter: GrantListFilter,
        limit: u32,
    ) -> RepositoryFuture<'_, GrantPage>;

    /// Stores a deny rule.
    ///
    /// A rule has no version and no lifecycle: it is added or removed, because a refusal carries no
    /// authority to edit. `reason` is validated here rather than at the schema so the caller receives "the
    /// reason is unusable" rather than a decoded `CHECK` failure.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] naming `deny_reason` when the reason is empty, over
    /// [`MAX_GRANT_NOTE_BYTES`], or contains a NUL byte.
    fn add_deny_rule(
        &self,
        rule: &NewDenyRule,
    ) -> RepositoryFuture<'_, jarvis_domain::ids::ToolDenyRuleId>;

    /// Removes a deny rule.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for a rule that is absent or owned by another workspace.
    fn remove_deny_rule(
        &self,
        workspace: WorkspaceId,
        id: jarvis_domain::ids::ToolDenyRuleId,
    ) -> RepositoryFuture<'_, ()>;

    /// Returns every deny rule in `workspace`, in insertion order.
    ///
    /// Distinct from [`Self::deny_rules`], which returns what the **evaluator** consumes and therefore
    /// includes the workspace-wide rules and drops nothing. This one is the administration read, so it
    /// shows a rule's identity and reason — the fields an operator needs to remove one.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] when the store cannot be read.
    fn list_deny_rules(
        &self,
        workspace: WorkspaceId,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<StoredDenyRule>>;
}

/// A deny rule to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDenyRule {
    /// The workspace that owns it, or `None` for a rule that applies everywhere.
    pub workspace_id: Option<WorkspaceId>,
    /// The capability it refuses, when it names one.
    pub capability: Option<String>,
    /// The principal it refuses, when it names one.
    pub principal: Option<PrincipalId>,
    /// The effects it refuses. Empty means "not constrained by effects".
    pub effects: BTreeSet<Effect>,
    /// The reason shown to a principal whose call was refused.
    pub reason: String,
    /// When it was created.
    pub created_at: UtcTimestamp,
}

impl NewDenyRule {
    /// Validates the rule before it reaches storage.
    ///
    /// The **empty rule is refused**, and that is a deliberate departure from the domain's own rule that an
    /// empty `DenyRule` matches nothing. The domain refuses to over-match because a defaulted record would
    /// otherwise refuse everything; here the rule is being *written by an operator*, so an empty one is a
    /// configuration mistake rather than a defaulted value — and accepting it would store a row that
    /// silently does nothing while appearing in the listing.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] naming `deny_reason` when the reason is unusable, and
    /// `deny_rule` when the rule names nothing at all.
    pub fn validated(self) -> Result<Self, RepositoryError> {
        if self.reason.is_empty()
            || self.reason.len() > MAX_GRANT_NOTE_BYTES
            || self.reason.contains('\0')
        {
            return Err(RepositoryError::Conflict {
                what: "deny_reason",
            });
        }
        if self.workspace_id.is_none()
            && self.capability.is_none()
            && self.principal.is_none()
            && self.effects.is_empty()
        {
            return Err(RepositoryError::Conflict { what: "deny_rule" });
        }
        Ok(self)
    }

    /// Returns the domain rule the evaluator consumes.
    ///
    /// The capability is parsed into an identity match by the evaluator, which compares capabilities
    /// rather than identities for the reason [`NewToolGrant`] records — so this returns a rule naming no
    /// identity and the evaluator matches on capability. `DenyRule.identity` is therefore left `None` here,
    /// and the capability is carried beside it so the adapter can store it; see
    /// `jarvis_infrastructure::tool_adapters::StoredGrants` for the comparison that reads it.
    #[must_use]
    pub fn as_rule(&self) -> DenyRule {
        DenyRule {
            // **Always `None`.** A deny rule is stored by capability, so it has no exact identity to name
            // — and naming one would make the refusal stop applying after a tool was recompiled, which is
            // the direction that loses a restriction.
            identity: None,
            principal: self.principal,
            workspace: self.workspace_id,
            effects: self.effects.clone(),
        }
    }
}

/// A deny rule as stored, with the fields an operator needs to remove it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDenyRule {
    /// The rule's own identity.
    pub id: jarvis_domain::ids::ToolDenyRuleId,
    /// The capability it refuses, when it names one.
    pub capability: Option<String>,
    /// The rule as the evaluator consumes it.
    pub rule: DenyRule,
    /// The reason shown to a refused principal.
    pub reason: String,
    /// When it was created.
    pub created_at: UtcTimestamp,
}
