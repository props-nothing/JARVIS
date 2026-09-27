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
    ApprovalActor, ApprovalChannel, ApprovalTransitionRecord, ApprovalVersion, DurableApproval,
};

use super::RepositoryFuture;

/// The largest number of pending approvals one listing may return.
///
/// **A bound on a page, not on a total.** The store applies it to every `pending_in` call, and the
/// service clamps a caller's requested limit to it rather than refusing — so a client asking for more
/// receives a full page and the daemon's own bound, which is what the contract's "bounded page size"
/// requires. It is declared here rather than in the HTTP layer because it is a property of the query
/// the store performs: a bound the adapter did not apply would make the constant a statement about a
/// document rather than about a read.
///
/// **⚠ The wire's `max_page` value is a second declaration of this same bound, and nothing here can
/// compare them.** `jarvis_protocol::approval::MAX_APPROVAL_PAGE` is what a client is *told*, and
/// this crate may not depend on the wire vocabulary, so raising that one alone would leave the daemon
/// advertising a page size larger than the one its own query applies — and a client paging by
/// `max_page` would receive fewer rows than the response claimed, unable to tell a full page from a
/// truncated one. The comparison lives in `jarvis-infrastructure`, the one crate that depends on
/// both: `the_reported_page_bound_is_the_one_the_store_enforces` in `http`'s tests.
pub const MAX_PENDING_PAGE: u32 = 200;

/// One page of pending approvals, and whether the store stopped at its bound.
///
/// `bounded` travels to the caller rather than being inferred from `approvals.len() == limit`, for the
/// reason the run and ledger scans record: the inference silently becomes wrong if the bound changes,
/// and a store that returned exactly its limit for an exhausted table would satisfy it — making "there
/// may be more" look identical to "that is all". It matters more here than in either of those, because
/// a caller that cannot tell the two apart **concludes there are no more approvals to decide** and
/// performs no work, which is a silent stop rather than a short list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalsPage {
    /// The approvals in this page, in the store's own order.
    pub approvals: Vec<DurableApproval>,
    /// Whether the store stopped at its bound with rows still unread.
    pub bounded: bool,
}

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

    /// Returns the approvals awaiting a decision in `workspace` that `channel` may decide.
    ///
    /// **Unscoped by principal on purpose.** An operator view has to show every pending prompt in the
    /// workspace, and a listing scoped to one principal would hide the ones waiting on somebody else —
    /// the same reasoning `RunRepository::incomplete_runs` records for startup recovery. Callers that
    /// need one principal's queue filter the result.
    ///
    /// **`channel` is a query predicate, not a post-filter, and that is a correctness requirement.**
    /// A caller may decide only what its channel is named in, so applying `limit` to rows it cannot see
    /// would spend the page on them and return a short list — and a client that received a short list
    /// concludes there is nothing more to decide. The bound must apply to the rows the caller can
    /// actually act on, which means the store has to exclude the others *before* it counts.
    ///
    /// **The order is part of the contract**: soonest deadline first, because that is the order an
    /// operator has to act in. It is a **total** order — the identifier breaks a tie on `expires_at` —
    /// because a page whose order is not total can show one row twice and hide another.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`](super::RepositoryError::Query) when the store cannot be read.
    fn pending_in(
        &self,
        workspace: WorkspaceId,
        channel: ApprovalChannel,
        limit: u32,
    ) -> RepositoryFuture<'_, ApprovalsPage>;

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

    /// Returns an approval's transition trail, oldest first.
    ///
    /// **This read exists because the decision's note is stored in the transition and had no reader.**
    /// The contract's decision body carries a `comment` and its cancellation body a `reason`; both are
    /// written into `actor_json`, and until this method nothing could read them back — so the audit trail
    /// said *that* a human decided but never *why*, which is the value the two wire fields exist to carry.
    /// A writer with no reader is the shape this project has now found five times, and the fix is the
    /// reader rather than a second writer.
    ///
    /// The order is **oldest first** because a trail is read forwards: "pending, then approved, then
    /// consumed" describes what happened, while the reverse reads as a puzzle. It is a total order — the
    /// append-only identifier breaks an `occurred_at` tie — so a page boundary cannot show one step twice
    /// and hide another.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](super::RepositoryError::NotFound) for an approval that is
    /// absent or owned by another workspace, and
    /// [`RepositoryError::Corrupted`](super::RepositoryError::Corrupted) for a stored actor whose payload
    /// cannot be interpreted — which is a record whose audit row cannot be read, not a missing one.
    fn transitions(
        &self,
        workspace: WorkspaceId,
        approval: ApprovalId,
    ) -> RepositoryFuture<'_, Vec<ApprovalTransitionRecord>>;
}
