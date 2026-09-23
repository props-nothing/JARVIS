//! The durable model data policy store and the route decisions made under it.
//!
//! `docs/contracts/model-data-policy.md` fixes this port's shape: "`model_data_policies`
//! stores immutable versions; changing rules creates a new version", and a
//! "create-run request references a policy ID/version". The port is the input side of
//! `jarvis_domain::model::routing`, which had a selector and no way to obtain the rules
//! it selects against.
//!
//! Three decisions are worth stating, because each is a rule rather than a convenience:
//!
//! - **A version is immutable, so the write is `create` and not `update`.** `insert_version`
//!   refuses a version that already exists rather than replacing it, because the contract
//!   requires a past decision to remain explainable and an `UPDATE` could silently rewrite
//!   the rules a call was made under. Changing rules is a new `version`, which is why the
//!   write takes a whole [`NewPolicyVersion`] rather than a set of fields.
//! - **The version is checked rather than assigned.** A caller states the version it is
//!   creating, and a collision is [`RepositoryError::VersionConflict`] — the same typed
//!   outcome the run state machine uses for optimistic concurrency, because it is the same
//!   fact: someone else advanced the record since this caller read it. Assigning the next
//!   version inside the store would make a concurrent update indistinguishable from a
//!   sequential one, and the contract requires `resource.version_conflict` for the former.
//! - **Scope is a parameter on every read.** A policy belongs to a workspace, so a policy
//!   from another workspace is reported as absent rather than as forbidden — the same rule
//!   the run repository follows, because the local control API requires another scope's
//!   resource to be indistinguishable from a missing one.

use jarvis_domain::ids::WorkspaceId;
use jarvis_domain::model::policy::{
    ModelDataPolicyStatus, ModelRouteDecision, PolicyRules, PolicyVersionRef,
};
use jarvis_domain::time::UtcTimestamp;

use crate::repository::{RepositoryError, RepositoryFuture};

/// The longest accepted policy name.
///
/// Bounded because a name reaches an operator display and a stored row, and neither
/// should accept an unbounded caller-supplied string. 200 bytes is far above any
/// human-authored label and still finite.
pub const MAX_POLICY_NAME_BYTES: usize = 200;

/// The longest accepted exception reason.
pub const MAX_EXCEPTION_REASON_BYTES: usize = 1_024;

/// One immutable policy version to store.
///
/// The natural key is `(policy_id, version)`, so both are carried rather than the
/// store assigning a version: a caller that names the version it is creating can be told
/// its view was stale, while one that lets the store choose can only be told it succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPolicyVersion {
    /// The policy this version belongs to.
    pub policy_id: jarvis_domain::ids::ModelDataPolicyId,
    /// The version being created, from 1.
    pub version: u32,
    /// The workspace that owns it.
    pub workspace_id: WorkspaceId,
    /// The operator-facing name.
    pub name: String,
    /// The lifecycle state.
    pub status: ModelDataPolicyStatus,
    /// The typed rules, which the adapter serializes.
    pub rules: PolicyRules,
    /// When it was created.
    pub created_at: UtcTimestamp,
}

impl NewPolicyVersion {
    /// Validates the version before it reaches storage.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when the name is empty, over
    /// [`MAX_POLICY_NAME_BYTES`], or contains a NUL byte, and when the version is zero.
    /// Refusing here rather than at the schema means the caller receives "the name is
    /// unusable" rather than a decoded `CHECK` failure, and it is the same reason the run
    /// repository validates its objective reference: an unbounded caller-supplied string
    /// must not reach a persisted row.
    pub fn validated(self) -> Result<Self, RepositoryError> {
        if self.name.is_empty()
            || self.name.len() > MAX_POLICY_NAME_BYTES
            || self.name.contains('\0')
        {
            return Err(RepositoryError::Conflict {
                what: "policy_name",
            });
        }
        if self.version < 1 {
            return Err(RepositoryError::Conflict {
                what: "policy_version",
            });
        }
        Ok(self)
    }
}

/// A loaded policy version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPolicyVersion {
    /// The policy this version belongs to.
    pub policy_id: jarvis_domain::ids::ModelDataPolicyId,
    /// The immutable version number.
    pub version: u32,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
    /// The operator-facing name.
    pub name: String,
    /// The lifecycle state.
    pub status: ModelDataPolicyStatus,
    /// The typed rules, as read back and re-validated.
    pub rules: PolicyRules,
    /// When it was created.
    pub created_at: UtcTimestamp,
}

impl StoredPolicyVersion {
    /// Returns the reference a route decision records.
    #[must_use]
    pub const fn reference(&self) -> PolicyVersionRef {
        PolicyVersionRef {
            policy_id: self.policy_id,
            version: self.version,
        }
    }
}

/// The durable model data policy store.
pub trait ModelDataPolicyRepository: Send + Sync {
    /// Stores a new immutable policy version.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::VersionConflict`] when `(policy_id, version)` already
    /// exists — the contract's `model.policy_version_conflict`, reported from the store
    /// because only the store can see the collision. Returns
    /// [`RepositoryError::Conflict`] for an unusable name or a zero version, and
    /// [`RepositoryError::Query`] for a driver failure.
    fn insert_version(&self, policy: NewPolicyVersion) -> RepositoryFuture<'_, ()>;

    /// Reads one version, scoped to `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when it is absent **or** owned by another
    /// workspace, and [`RepositoryError::Corrupted`] when the stored rules cannot be read
    /// back as the domain type. The second is deliberately not `NotFound`: a row that
    /// exists but cannot be interpreted is a corruption signal, and reporting it as absent
    /// would let a caller create a replacement for a policy that is still there.
    fn load_version(
        &self,
        workspace: WorkspaceId,
        reference: PolicyVersionRef,
    ) -> RepositoryFuture<'_, StoredPolicyVersion>;

    /// Reads the workspace's **active** policy version.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when the workspace has no active policy. That
    /// is a real state rather than an error to swallow: a call made with no policy in force
    /// would apply nothing, so the caller has to decide whether that is permitted, and it
    /// cannot decide if the store reports an empty policy instead.
    fn load_active(&self, workspace: WorkspaceId) -> RepositoryFuture<'_, StoredPolicyVersion>;

    /// Records a route decision.
    ///
    /// Decisions are append-only: a decision explains a call that already happened, so
    /// rewriting one would destroy the evidence rather than update it. The write is
    /// idempotent by identity, so a retry that reuses a decision does not create a second
    /// record for the same reasoning.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when the decision's identity already exists,
    /// and [`RepositoryError::Query`] for a driver failure.
    fn record_decision(
        &self,
        workspace: WorkspaceId,
        decision_id: jarvis_domain::ids::ModelRouteDecisionId,
        decision: ModelRouteDecision,
    ) -> RepositoryFuture<'_, ()>;

    /// Reads a recorded decision, scoped to `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when it is absent or belongs to another
    /// workspace, and [`RepositoryError::Corrupted`] when a stored part cannot be read
    /// back as the domain type.
    fn load_decision(
        &self,
        workspace: WorkspaceId,
        decision_id: jarvis_domain::ids::ModelRouteDecisionId,
    ) -> RepositoryFuture<'_, ModelRouteDecision>;

    /// Grants a durable exception.
    ///
    /// The write is a plain `INSERT` behind the primary key, so a duplicate identity is a
    /// conflict rather than a replacement. That matters because an exception is the one record
    /// that *relaxes* a rule: rewriting one in place would change, after the fact, what a past
    /// decision was permitted by — the same reason a policy version and a route decision are
    /// both insert-only here.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when the identity already exists, and
    /// [`RepositoryError::Query`] for a driver failure. The record's own rules — a waivable rule,
    /// a bounded reason, a step-up requirement that was met — are enforced by
    /// [`PolicyException::grant`], so this method cannot be handed an invalid one; a record read
    /// from a database written by another build is re-validated on the way out instead.
    fn grant_exception(
        &self,
        workspace: WorkspaceId,
        exception: JarvisPolicyException,
    ) -> RepositoryFuture<'_, ()>;

    /// Reads one exception, scoped to `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when it is absent or owned by another workspace —
    /// indistinguishable by design — and [`RepositoryError::Corrupted`] when the stored scope or
    /// rule key cannot be read back as the domain type. The second is deliberately not
    /// `NotFound`: reporting an unreadable row as absent would let a caller grant a second
    /// exception for a rule that already has one.
    fn load_exception(
        &self,
        workspace: WorkspaceId,
        exception_id: jarvis_domain::ids::PolicyExceptionId,
    ) -> RepositoryFuture<'_, JarvisPolicyException>;

    /// Reads every exception a workspace holds, whether or not it is still usable.
    ///
    /// All of them rather than only the usable ones, because usability is a question about an
    /// *instant* and the caller is the one that knows it — this selection happens at the decision
    /// instant, and recovery or a diagnostics read may ask about a different one. The store's job
    /// is to return what was written, and [`JarvisPolicyException::is_usable_at`] is the single
    /// predicate that answers the other question.
    ///
    /// Bounded by [`MAX_EXCEPTIONS_PER_WORKSPACE`], because a caller reads this list on the route
    /// path and an unbounded read would let a workspace accumulate enough grants to make every
    /// selection expensive.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Corrupted`] when a stored scope or rule key cannot be read back,
    /// and [`RepositoryError::Query`] for a driver failure.
    fn list_exceptions(
        &self,
        workspace: WorkspaceId,
    ) -> RepositoryFuture<'_, Vec<JarvisPolicyException>>;

    /// Records that an exception was revoked at `at`.
    ///
    /// A state change rather than a delete, so the record survives to explain why a past call was
    /// permitted. Re-revoking is idempotent: the first instant is kept, because the second would
    /// rewrite when the decision was actually taken.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when it is absent or foreign, and
    /// [`RepositoryError::Conflict`] when it was already **consumed** — a used single-use grant
    /// cannot also be revoked, because the two are different decisions and accepting both would
    /// record a revocation that did not prevent anything.
    fn revoke_exception(
        &self,
        workspace: WorkspaceId,
        exception_id: jarvis_domain::ids::PolicyExceptionId,
        at: UtcTimestamp,
    ) -> RepositoryFuture<'_, ()>;

    /// Records that a single-use exception was consumed at `at`.
    ///
    /// The **conditional** update is the guard: `consumed_at IS NULL AND revoked_at IS NULL` is
    /// part of the statement rather than a pre-read, so two concurrent calls cannot both consume
    /// one grant. A pre-read followed by an unconditional write is the shape that lets both
    /// succeed and reports a single-use exception as used twice.
    ///
    /// **Only a single-use grant is affected.** `single_use = 1` is part of the same predicate, so
    /// consuming a repeatable grant is a no-op reported as success rather than an error or a
    /// stamp: the caller's grant is intact and still usable, and recording a consumption instant
    /// on it would make a later reader report a grant in force as spent.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when it is absent or foreign, and
    /// [`RepositoryError::Conflict`] when it was already consumed or revoked — the caller's
    /// grant is gone, which is a fact it must not be told succeeded.
    fn consume_exception(
        &self,
        workspace: WorkspaceId,
        exception_id: jarvis_domain::ids::PolicyExceptionId,
        at: UtcTimestamp,
    ) -> RepositoryFuture<'_, ()>;
}

/// The most exceptions one workspace read may return.
///
/// Bounded because [`ModelDataPolicyRepository::list_exceptions`] is read on the route path: an
/// unbounded list would let a workspace accumulate enough grants to make every selection
/// expensive, and a real workspace holds a handful. Exceeding it is reported as
/// [`RepositoryError::Conflict`] rather than truncating, because a silently shortened list would
/// drop a grant that an operator believes is in force.
pub const MAX_EXCEPTIONS_PER_WORKSPACE: usize = 256;

/// The domain exception type, under a local alias that keeps the port's signatures readable.
pub use jarvis_domain::model::exception::PolicyException as JarvisPolicyException;
