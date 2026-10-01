//! The tool-authorization use cases: list, read, create, replace, and revoke a grant.
//!
//! This is the layer between `ToolGrantRepository` and a client, and it exists so the HTTP and CLI
//! surfaces hold no authorization policy of their own — the same shape `approval_service` has, and for the
//! same reason: a rule that lives in a handler is a rule the next handler does not have.
//!
//! ## What this layer decides, and what it deliberately does not
//!
//! **It resolves the definition a grant is for, so the narrowing rule has a definition to narrow.** The
//! request carries a *capability* string — the only thing a client can name — and this layer resolves it
//! against the catalog. A capability the catalog does not offer is refused rather than stored, because a
//! grant for a tool nobody can invoke is a row that appears in a listing as authority and confers nothing.
//!
//! **It does not decide whether a grant is *wise*.** Whether `email.send` should be granted to read-only is
//! an operator's judgment, and the ceiling checks are a bound rather than a policy: this layer refuses a
//! grant that would *widen* the tool, and accepts any narrowing an operator wrote. That distinction is why
//! [`NewToolGrant::narrowing`] lives in the repository port rather than here — it is a property of what a
//! grant *is*, not a use case.
//!
//! **It never writes a grant on behalf of a tool call.** A grant is written by an authenticated
//! control-plane caller and by nothing else; the tool pipeline reads grants and cannot create one. That is
//! the boundary the whole store depends on: a model that could mint its own authorization would make the
//! approval path decorative, which is `TLS-012`'s concern expressed one layer out.
//!
//! ## Two rules inherited from the approval service
//!
//! **The server derives the operator.** The request body cannot assert who configured a grant: the
//! principal recorded as `granted_by` comes from the [`RequestContext`], which only trusted code can build.
//! **And the scope is a parameter on every read**, so a grant belonging to another workspace is reported as
//! absent rather than as forbidden — an authorization row is the most security-relevant in the profile, and
//! a distinguishable "forbidden" would confirm that another workspace has a grant for a tool.

use std::sync::Arc;

use jarvis_domain::ids::{PrincipalId, ToolDenyRuleId, ToolGrantId, WorkspaceId};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::classification::{Effect, Risk, Scope};

use crate::repository::RepositoryError;
use crate::repository::tool_grant::{
    GrantListFilter, GrantPage, MAX_GRANT_NOTE_BYTES, NewDenyRule, NewToolGrant, StoredDenyRule,
    StoredToolGrant, ToolGrantRepository,
};
use crate::request_context::RequestContext;
use crate::tool_call::ToolCatalog;

/// What a caller asks a grant to be.
///
/// **A request rather than the stored type**, so the wire shape and the stored shape can differ where they
/// must: this carries a *capability* (the only thing a client can name) and a set of *scope strings* (the
/// only spelling a client can send), while [`NewToolGrant`] carries the resolved identity and typed scopes.
/// The translation is this layer's, and doing it here is what keeps a handler from resolving a definition
/// — which would need the catalog, which a handler must not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRequest {
    /// The capability to grant, e.g. `clock.now@1`.
    pub capability: String,
    /// The scopes to confer, as contract strings. Empty confers none.
    pub scopes: Vec<String>,
    /// The effects to permit, as contract strings. Must be a subset of the tool's declared effects.
    pub effects: Vec<String>,
    /// The greatest risk to permit.
    pub risk_ceiling: Risk,
    /// The most sensitive argument to permit.
    pub sensitivity_ceiling: Sensitivity,
    /// When the grant stops applying, or `None` for a standing grant.
    pub expires_at: Option<UtcTimestamp>,
    /// The workspace, resolved server-side from the authenticated caller.
    pub workspace: WorkspaceId,
    /// The principal the grant is *for*.
    pub principal: PrincipalId,
    /// `None` to create; `Some(version)` to replace the version the caller read.
    pub expected_version: Option<u32>,
}

/// What went wrong, in terms that reach a client.
///
/// Each variant is a **distinct operator action**, which is why they are not collapsed: an unknown tool
/// means "re-read the tool list", an unknown scope means "check the definition's declared scopes", a
/// widening means "you asked for more than the tool does", and a conflict means "someone else edited this".
/// A single "invalid grant" would send an operator to re-read the whole form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantServiceError {
    /// The named capability is not in the catalog.
    UnknownTool,
    /// The requested effect is not in the closed set the contract defines.
    UnknownEffect,
    /// The requested scope is not a usable scope string.
    InvalidScope,
    /// The grant would widen the tool, and the field that widened is named by the code.
    Widens {
        /// The stable code naming the field that widened.
        code: &'static str,
    },
    /// A version was named and no grant exists for that capability.
    NotFound,
    /// A create was asked for and a grant already exists.
    AlreadyExists,
    /// Someone else advanced the grant since the caller read it.
    VersionConflict {
        /// The version the caller expected.
        expected: u64,
        /// The version stored.
        actual: u64,
    },
    /// The store could not be read or written.
    Storage,
    /// The daemon has no tool pipeline, so there is no catalog to resolve against.
    ///
    /// A distinct variant rather than `UnknownTool`, because the two are different facts: one means the
    /// operator named a tool that does not exist, the other means this deployment cannot answer the
    /// question at all — and an operator who saw the first for the second would go looking for a typo.
    PipelineUnavailable,
    /// A deny rule named nothing, or its reason was unusable.
    InvalidDenyRule,
}

impl GrantServiceError {
    /// Returns the stable, namespaced code a client branches on.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownTool => "tool.grant_unknown_tool",
            Self::UnknownEffect => "tool.grant_unknown_effect",
            Self::InvalidScope => "tool.grant_invalid_scope",
            // A widening carries the repository's own field name, so the two spellings cannot disagree.
            Self::Widens { code } => code,
            Self::NotFound => "tool.grant_not_found",
            Self::AlreadyExists => "tool.grant_exists",
            Self::VersionConflict { .. } => "tool.grant_version_conflict",
            Self::Storage => "storage.query_failed",
            Self::PipelineUnavailable => "tool.pipeline_unavailable",
            Self::InvalidDenyRule => "tool.deny_rule_invalid",
        }
    }

    /// Returns whether retrying the same request unchanged could succeed.
    ///
    /// A version conflict **is** retryable in the only sense that matters to a client — after re-reading,
    /// the same intent can be applied — while a widening never is: the same request widens by the same
    /// amount. Distinguishing them is what keeps a UI from offering "try again" for a request that must be
    /// edited.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::Storage | Self::VersionConflict { .. })
    }
}

impl std::fmt::Display for GrantServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::UnknownTool => "the named tool is not registered",
            Self::UnknownEffect => "the named effect is not one the contract defines",
            Self::InvalidScope => "a named scope is not a usable scope",
            Self::Widens { code } => code,
            Self::NotFound => "no grant exists for that tool",
            Self::AlreadyExists => "a grant already exists for that tool",
            Self::VersionConflict { .. } => "the grant was modified concurrently",
            Self::Storage => "the grant store could not be reached",
            Self::PipelineUnavailable => "this deployment has no tool pipeline to resolve against",
            Self::InvalidDenyRule => "the deny rule is unusable",
        };
        formatter.write_str(text)
    }
}

impl From<RepositoryError> for GrantServiceError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::NotFound => Self::NotFound,
            RepositoryError::VersionConflict { expected, actual } => {
                Self::VersionConflict { expected, actual }
            }
            // A narrowing refusal arrives as a named conflict, and the name is **carried rather than
            // flattened** so the operator is told which field to change — the whole reason the store names
            // fields instead of using one code. `grant_exists` gets its own variant because it is a
            // different fact: the operator asked to create and one is already there.
            RepositoryError::Conflict {
                what: "grant_exists",
            } => Self::AlreadyExists,
            RepositoryError::Conflict {
                what: "deny_reason" | "deny_rule",
            } => Self::InvalidDenyRule,
            RepositoryError::Conflict { what } if what.starts_with("grant_") => {
                Self::Widens { code: what }
            }
            _ => Self::Storage,
        }
    }
}

/// The tool-authorization use cases.
pub struct ToolGrantService {
    store: Arc<dyn ToolGrantRepository>,
    catalog: Arc<dyn ToolCatalog>,
}

impl std::fmt::Debug for ToolGrantService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The catalog and the store are not printed: one holds reviewed definitions and the other a
        // connection pool, and neither belongs in a diagnostic line.
        formatter
            .debug_struct("ToolGrantService")
            .finish_non_exhaustive()
    }
}

impl ToolGrantService {
    /// Builds the service over the store and the catalog.
    ///
    /// **The catalog is a parameter rather than something read off a `ToolCallService`**, and the reason is
    /// that `ToolCallService` exposes `capabilities()` and not `resolve` — so there is no way to recover the
    /// catalog from it. A convenience constructor over the service would therefore have to write a grant
    /// without resolving the definition, which is exactly what the narrowing rule needs; the daemon passes
    /// the same catalog it gave the pipeline instead, so the two cannot disagree about which tools exist.
    #[must_use]
    pub fn new(store: Arc<dyn ToolGrantRepository>, catalog: Arc<dyn ToolCatalog>) -> Self {
        Self { store, catalog }
    }

    /// Lists the grants in force in `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`GrantServiceError::Storage`] when the store cannot be read.
    pub async fn list(
        &self,
        workspace: WorkspaceId,
        filter: GrantListFilter,
        limit: u32,
    ) -> Result<GrantPage, GrantServiceError> {
        Ok(self.store.list(workspace, filter, limit).await?)
    }

    /// Loads one grant within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`GrantServiceError::NotFound`] for an absent or foreign grant.
    pub async fn read(
        &self,
        workspace: WorkspaceId,
        id: ToolGrantId,
    ) -> Result<StoredToolGrant, GrantServiceError> {
        Ok(self.store.load(workspace, id).await?)
    }

    /// Creates or replaces a grant.
    ///
    /// The definition is resolved from the capability the caller named, so the narrowing rule has something
    /// to narrow — and a capability the catalog does not offer is refused rather than stored, because a grant
    /// for a tool nobody can invoke would appear in a listing as authority while conferring nothing.
    ///
    /// `context` supplies the operator identity and the workspace, and **the request's own workspace and
    /// principal are overwritten from it**: a body cannot assert the scope a grant is written in, which is
    /// the same rule the approval decision follows.
    ///
    /// # Errors
    ///
    /// Returns [`GrantServiceError::UnknownTool`] or [`GrantServiceError::PipelineUnavailable`] when the
    /// capability cannot be resolved, [`GrantServiceError::UnknownEffect`] or
    /// [`GrantServiceError::InvalidScope`] for a spelling the contract does not define,
    /// [`GrantServiceError::Widens`] naming the field when the grant would exceed the tool, and
    /// [`GrantServiceError::AlreadyExists`] / [`GrantServiceError::VersionConflict`] from the store.
    pub async fn write(
        &self,
        context: &RequestContext,
        request: &GrantRequest,
        at: UtcTimestamp,
    ) -> Result<StoredToolGrant, GrantServiceError> {
        let Some(tool) = self
            .catalog
            .resolve(context.workspace_id, &request.capability)
        else {
            return Err(GrantServiceError::UnknownTool);
        };
        let effects = parse_effects(&request.effects)?;
        let scopes = parse_scopes(&request.scopes)?;
        let grant = NewToolGrant::narrowing(
            &tool.definition,
            context.workspace_id,
            request.principal,
            scopes,
            effects,
            request.risk_ceiling,
            request.sensitivity_ceiling,
            request.expires_at,
            // **The operator comes from the context, never from the request.** A body that named who
            // configured a grant could attribute a widening to somebody who never wrote it, and the audit
            // trail is the one thing a grant record exists to carry.
            context.principal_id,
            at,
            request.expected_version,
        )
        .map_err(GrantServiceError::from)?;
        Ok(self.store.put(&grant, at).await?)
    }

    /// Withdraws a grant, keeping the row for audit.
    ///
    /// # Errors
    ///
    /// Returns [`GrantServiceError::NotFound`] for an absent or foreign grant, and
    /// [`GrantServiceError::VersionConflict`] when the caller's view is stale.
    pub async fn revoke(
        &self,
        workspace: WorkspaceId,
        id: ToolGrantId,
        expected: u32,
        at: UtcTimestamp,
    ) -> Result<StoredToolGrant, GrantServiceError> {
        Ok(self.store.revoke(workspace, id, expected, at).await?)
    }

    /// Stores a deny rule.
    ///
    /// The rule's workspace comes from `context` unless the request leaves it unset, in which case it is a
    /// refusal that applies in every workspace — which is the broadest form and must stay expressible.
    ///
    /// # Errors
    ///
    /// Returns [`GrantServiceError::UnknownEffect`] for an effect spelling the contract does not define,
    /// [`GrantServiceError::InvalidDenyRule`] for a rule that names nothing, and
    /// [`GrantServiceError::Storage`] when the store cannot be written.
    pub async fn add_deny_rule(
        &self,
        context: &RequestContext,
        request: &DenyRuleRequest<'_>,
        at: UtcTimestamp,
    ) -> Result<ToolDenyRuleId, GrantServiceError> {
        let effects = parse_effects(request.effects)?;
        let rule = NewDenyRule {
            workspace_id: request.workspace_wide.then_some(context.workspace_id),
            capability: request.capability.map(str::to_owned),
            principal: request.principal,
            effects,
            reason: request.reason.to_owned(),
            created_at: at,
        }
        .validated()
        .map_err(|_| GrantServiceError::InvalidDenyRule)?;
        Ok(self.store.add_deny_rule(&rule).await?)
    }

    /// Lists the deny rules visible in `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`GrantServiceError::Storage`] when the store cannot be read.
    pub async fn list_deny_rules(
        &self,
        workspace: WorkspaceId,
        limit: u32,
    ) -> Result<Vec<StoredDenyRule>, GrantServiceError> {
        Ok(self.store.list_deny_rules(workspace, limit).await?)
    }

    /// Removes a deny rule.
    ///
    /// # Errors
    ///
    /// Returns [`GrantServiceError::NotFound`] for a rule that is absent or invisible from this workspace.
    pub async fn remove_deny_rule(
        &self,
        workspace: WorkspaceId,
        id: ToolDenyRuleId,
    ) -> Result<(), GrantServiceError> {
        Ok(self.store.remove_deny_rule(workspace, id).await?)
    }

    /// Returns how many bytes a deny reason may hold.
    ///
    /// A reader so a surface can validate before sending, rather than a second literal that would let the
    /// advertised bound drift from the enforced one — the same reason `approval_service` re-exports the
    /// domain's note bound instead of declaring one.
    #[must_use]
    pub const fn max_reason_bytes() -> usize {
        MAX_GRANT_NOTE_BYTES
    }
}

/// A deny rule a caller asks to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyRuleRequest<'a> {
    /// The capability to refuse, or `None` for a rule that names only effects.
    pub capability: Option<&'a str>,
    /// The principal to refuse, or `None` for every principal.
    pub principal: Option<PrincipalId>,
    /// The effects to refuse, as contract strings. Empty means "not constrained by effects".
    pub effects: &'a [String],
    /// The reason shown to a principal whose call was refused.
    pub reason: &'a str,
    /// Whether the refusal applies in `context`'s workspace **only**, rather than in every workspace.
    ///
    /// `true` for a workspace-scoped refusal; `false` for a profile-wide one. Named as a positive
    /// (`workspace_wide` rather than `global`) because the narrow case is the ordinary one and a defaulted
    /// struct field would otherwise widen a refusal to every workspace.
    pub workspace_wide: bool,
}

/// Parses effect spellings into the closed set.
///
/// An unknown spelling is refused rather than ignored, because a grant that silently dropped an effect the
/// caller asked to permit would confer less than the operator wrote — and a refusal that silently dropped one
/// would refuse less, which is the direction that matters more.
fn parse_effects(
    values: &[String],
) -> Result<std::collections::BTreeSet<Effect>, GrantServiceError> {
    values
        .iter()
        .map(|value| Effect::parse(value).map_err(|_| GrantServiceError::UnknownEffect))
        .collect()
}

/// Parses scope spellings into the domain type.
fn parse_scopes(values: &[String]) -> Result<std::collections::BTreeSet<Scope>, GrantServiceError> {
    values
        .iter()
        .map(|value| Scope::new(value).map_err(|_| GrantServiceError::InvalidScope))
        .collect()
}
