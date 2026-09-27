//! The approval repository port: durable approval records and their transitions.
//!
//! `TLS-005` asked for "durable approval records", and the domain half was built with the record, its
//! state machine, and its bounds. What was missing is this: **no port and no adapter**, so "durable"
//! meant the shape was durable and serialization-tested rather than that a row existed. This port is
//! where that stops being an omission.
//!
//! The persistence rule the architecture states for every state change applies here unchanged:
//!
//! > Persist state before publishing an event that claims the transition happened.
//!
//! So [`ApprovalRepository::apply_transition`] writes the state change **and** the transition row in one
//! transaction, and a caller that needs an event emits it after that returns. The method takes the
//! transition the caller applied rather than recomputing it, because the applied version and the state
//! actually left are facts the caller's own `apply` produced; recomputing them here would be a second
//! implementation of the state machine.
//!
//! **Concurrency is optimistic and explicit**, following `RunRepository`: every write states the version
//! it expects, and a stale writer is refused with
//! [`RepositoryError::VersionConflict`](super::RepositoryError::VersionConflict) rather than overwriting
//! a decision. That matters more for an approval than for a run, because the thing being overwritten is
//! a **user's own answer** to a prompt.

use jarvis_domain::ids::{ApprovalId, PrincipalId, WorkspaceId};
use jarvis_domain::tool::approval::{
    ApprovalActor, ApprovalTransitionRecord, ApprovalVersion, DurableApproval,
};

use super::RepositoryFuture;

/// What `decide` did, so a caller can distinguish a first decision from a repeat.
///
/// Three outcomes rather than a `Result`, for the reason the ledger's reservation needed four: **a
/// second identical decision is not a failure**, it is idempotence — a user double-tapping "approve"
/// sends two requests, and the second must not be an error the caller reports to them. But a second
/// decision that *differs* is a genuine conflict, and a spent or superseded approval is a third case
/// with its own message. Collapsing them into `Err` would make the ordinary double-tap look like a
/// failure and hide the case that needs the user's attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecideOutcome {
    /// The transition was applied and stored.
    Applied,
    /// The stored approval is already in the requested state, at the version the caller stated.
    ///
    /// Idempotent rather than conflicting, because the caller's intent is satisfied and nothing was
    /// overwritten. The version check is what distinguishes this from a stale write: if the versions
    /// disagreed, the caller is working from a view older than the stored decision and is refused.
    ///
    /// **Carries no version, and that is a correction.** The variant already says the stored state
    /// equals the requested target *and* the stored version equals the one the caller stated — those
    /// are the two conditions that produce it — so a payload could only re-state the caller's own
    /// input. An earlier version returned the stored version, and the adapter bound the version the
    /// caller had stated instead: always equal to the input, and therefore never wrong in a way a
    /// round-trip test could see. The two ways to make it informative are both false statements: the
    /// stored version is the caller's version by construction, and the version the approval was in
    /// *before* the repeat is a fact this port does not have — an `INSERT`-free repeat reads the row,
    /// so it could supply it, but a caller that had already been told which state it is in does not
    /// need it re-reported as a number.
    AlreadyInState,
}

/// A durable approval store.
///
/// **Scoped by workspace, not by principal.** An approval is issued by one principal, but a *read* of
/// it can legitimately come from the daemon acting on behalf of a run, and the workspace boundary is
/// the one the architecture makes structural ("Authorize every operation at the resource and effect
/// level"). Every method therefore takes the workspace and resolves the record inside it, so a caller
/// cannot address another workspace's approval by naming its identifier — the defect class this project
/// fixed for runs and for idempotency records.
pub trait ApprovalRepository: Send + Sync {
    /// Inserts a new approval in `PENDING`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`](super::RepositoryError::Conflict) when an approval with
    /// this identifier already exists. A `PENDING` approval is created by a *request*, and a duplicated
    /// request is a caller bug rather than an idempotent retry: the caller has no way to name the same
    /// approval twice, since the identifier is generated at construction.
    fn request(&self, approval: &DurableApproval) -> RepositoryFuture<'_, ()>;

    /// Loads an approval within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](super::RepositoryError::NotFound) for an approval that is
    /// absent **or** owned by another workspace, and
    /// [`RepositoryError::Corrupted`](super::RepositoryError::Corrupted) when a stored state cannot be
    /// interpreted.
    fn load(
        &self,
        workspace: WorkspaceId,
        approval: ApprovalId,
    ) -> RepositoryFuture<'_, DurableApproval>;

    /// Stores a decision or lifecycle transition, returning what happened.
    ///
    /// Writes the new state, the new version, and the decision's provenance (who decided, on which
    /// channel, when) in one transaction, and appends the transition record in the same transaction so
    /// the audit trail cannot disagree with the state it describes.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](super::RepositoryError::NotFound) for an approval that is
    /// absent or owned by another workspace, and
    /// [`RepositoryError::VersionConflict`](super::RepositoryError::VersionConflict) when `expected` is
    /// not the stored version — including when the stored state has moved on, since a stale view must be
    /// reported as stale before it is reported as a wrong transition.
    fn apply_transition(
        &self,
        workspace: WorkspaceId,
        transition: &ApprovalTransitionRecord,
        expected: ApprovalVersion,
        actor: &ApprovalActor,
        approval: &DurableApproval,
    ) -> RepositoryFuture<'_, DecideOutcome>;

    /// Returns the approvals awaiting a decision in `workspace`.
    ///
    /// **Unscoped by principal on purpose.** An operator view has to show every pending prompt in the
    /// workspace, and a listing scoped to one principal would hide the ones waiting on somebody else —
    /// the same reasoning `RunRepository::incomplete_runs` records for startup recovery. Callers that
    /// need one principal's queue filter the result.
    fn pending_in(
        &self,
        workspace: WorkspaceId,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<DurableApproval>>;

    /// Returns the approvals a principal decided, most recent first.
    ///
    /// Used to answer "what did I approve?" without walking every record, so the ordering is part of
    /// the contract rather than an accident of the query plan.
    fn decided_by(
        &self,
        workspace: WorkspaceId,
        principal: PrincipalId,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<DurableApproval>>;
}
