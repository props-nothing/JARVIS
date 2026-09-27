//! The tool-call ledger repository port: atomic reservations and durable transitions.
//!
//! `TLS-006` built the ledger's shape — the eleven-state machine, the transition table, the
//! five-dimension reservation key, and the `ACC-025` recovery classification — and recorded the honest
//! gap in the same breath:
//!
//! > the reservation is atomic only within one process's memory: the adapter that makes it atomic
//! > across processes is part of the persistence work, not of this module.
//!
//! That sentence is the whole reason this port exists. An in-memory map makes a duplicate submission
//! find the existing row when both arrive at **one** daemon. It cannot when they arrive at two, and a
//! second process is exactly the case that produces a second side effect — which is the one outcome the
//! entire ledger is built to prevent. So the atomicity of [`ToolCallRepository::reserve`] is not a
//! property of the implementation chosen for convenience; it is the contract, and the only way to hold
//! it is to let the **store** decide, in one statement, which writer won.
//!
//! **The port therefore returns a verdict rather than a `Result`.** A duplicate is not a failure: it is
//! the reservation working. [`ReservationOutcome`] is the domain's own four-variant answer, returned
//! unchanged, so an adapter cannot invent a fifth meaning for "the key is already taken" while looking
//! like it agrees with the domain.
//!
//! ## Why `reserve` takes the whole row rather than a key
//!
//! The caller has already built the row — the domain's `LedgerEntry::reserve` — and the adapter must
//! store *that* value, not a reconstruction. Taking a key and building a row here would put a second
//! definition of what a fresh ledger row is beside the domain's, and the two would disagree first about
//! the initial state and then about the attempt number.
//!
//! ## The scope rule is the same one four other slices recorded
//!
//! Every read takes a [`WorkspaceId`], and a row from another workspace is
//! [`RepositoryError::NotFound`](super::RepositoryError::NotFound) — **indistinguishable from a row that
//! does not exist**, following the rule the local control API states for runs. The reservation key
//! carries the workspace *and* the principal, so a duplicate lookup is scoped correctly by construction;
//! the explicit parameter on the reads is what stops a caller addressing another workspace's row by
//! naming its record identifier.

use jarvis_domain::ids::{ToolCallRecordId, WorkspaceId};
use jarvis_domain::tool::ledger::{
    LedgerEntry, ReservationOutcome, ToolCallTransition, ToolCallVersion,
};

use super::RepositoryFuture;

/// The largest number of possibly-effecting calls one scan may return.
///
/// **A bound here is a page size, not a total, and the distinction is the whole reason
/// [`EffectingScan::bounded`] exists.** The scan is ordered oldest-first, so the rows a bound hides are
/// the **newest** — and since a crash leaves rows unsettled across every restart, a pass that read one
/// page and stopped would leave the most recently stranded effects stranded forever, on this restart
/// and every later one. That is the same argument `MAX_INCOMPLETE_RUNS` records for runs, and the same
/// mistake would be made on the ledger side by treating a full page as "nothing left".
pub const MAX_EFFECTING_SCAN: u32 = 500;

/// One page of dispatched calls whose outcome was never recorded.
///
/// `bounded` travels to the caller rather than being inferred from
/// `records.len() == MAX_EFFECTING_SCAN`, for the reason the run recovery page records: the inference
/// silently becomes wrong if the bound changes, and a store that returned exactly its limit for an
/// exhausted table would satisfy it — making "there may be more" look identical to "that is all".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectingScan {
    /// The stranded calls, oldest first.
    pub records: Vec<LedgerEntry>,
    /// Whether the store stopped at its bound with rows still unread.
    pub bounded: bool,
}

/// A durable store of tool-call attempts.
///
/// **`reserve` is the method the rest is built around.** Every other operation concerns one row whose
/// identity the caller already knows; `reserve` is the only one where two callers race, and it is the
/// only one whose failure mode is a duplicated external effect rather than a confusing message.
pub trait ToolCallRepository: Send + Sync {
    /// Attempts to reserve a call's key, reporting what the store found.
    ///
    /// **The lookup and the insert are one operation, and the store decides.** A caller that looked up,
    /// decided, and then inserted would have a window between the two in which a second caller — in
    /// another process — could do the same, and both would dispatch. The adapter must therefore obtain
    /// its answer from a single statement's outcome rather than from a read followed by a write.
    ///
    /// Returns the domain's [`ReservationOutcome`], which distinguishes the three duplicate shapes that
    /// need three different caller responses: **read the recorded answer**, **wait**, or **reconcile and
    /// never retry**. Collapsing them into an error would make a caller treat "an answer already exists"
    /// and "the effect may have happened" alike, and the safe action for those is not the same.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`](super::RepositoryError::Query) for a driver failure. A
    /// duplicate is **not** an error.
    fn reserve(&self, entry: &LedgerEntry) -> RepositoryFuture<'_, ReservationOutcome>;

    /// Loads one row within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](super::RepositoryError::NotFound) for a row that is absent
    /// **or** owned by another workspace, and
    /// [`RepositoryError::Corrupted`](super::RepositoryError::Corrupted) when a stored value cannot be
    /// interpreted — including a terminal row with no recorded outcome, which is a row that claims to
    /// have finished without saying what happened.
    fn load(
        &self,
        workspace: WorkspaceId,
        record: ToolCallRecordId,
    ) -> RepositoryFuture<'_, LedgerEntry>;

    /// Stores a transition, appending its trail row **in the same transaction**.
    ///
    /// The trail is not decoration here: it is what records that a call was **dispatched**, which is the
    /// one fact a later retry decision turns on. A state change that committed without its trail row
    /// would leave the state and the audit of it disagreeing, which is the architecture's persistence
    /// rule applied to the ledger.
    ///
    /// `entry` is the caller's already-transitioned value, because the domain's `apply` produced it —
    /// recomputing the target state or the version here would be a second implementation of the state
    /// machine. The optimistic version predicate stays, so a writer that slipped in between the caller's
    /// read and this statement is refused rather than overwriting a recorded outcome.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](super::RepositoryError::NotFound) for an absent or foreign
    /// row, [`RepositoryError::VersionConflict`](super::RepositoryError::VersionConflict) when `expected`
    /// is not the stored version, and
    /// [`RepositoryError::TransitionRefused`](super::RepositoryError::TransitionRefused) when the
    /// domain's own table refuses the edge from the **stored** state.
    fn apply_transition(
        &self,
        workspace: WorkspaceId,
        transition: &ToolCallTransition,
        expected: ToolCallVersion,
        entry: &LedgerEntry,
    ) -> RepositoryFuture<'_, ()>;

    /// Returns the dispatched calls whose outcome was never recorded.
    ///
    /// **The outstanding-work list**: a call that may have effected and has no outcome, in either of the
    /// two states that describe it. Deliberately unscoped, for the reason
    /// `RunRepository::incomplete_runs` records — startup reconciliation is a whole-profile concern, and
    /// a scan scoped to one workspace would leave every other workspace's unsettled calls unsettled with
    /// no symptom.
    ///
    /// This is the list a caller reports and a provider read walks. It is **not** what the recovery pass
    /// pages over, and the difference is the whole reason [`Self::awaiting_conversion`] exists: this set
    /// contains the recovery pass's own output, so paging it does not terminate.
    ///
    /// `limit` is the caller's page size, and [`EffectingScan::bounded`] tells the caller whether the page
    /// was full — so a caller can page until drained rather than assuming one read saw everything. The
    /// adapter applies its own bound as well, so a caller cannot ask for an unbounded scan of a table that
    /// grows with real calls.
    fn possibly_effecting(&self, limit: u32) -> RepositoryFuture<'_, EffectingScan>;

    /// Returns the calls that are dispatched and have **not yet been marked as needing reconciliation**.
    ///
    /// **The conversion input, and it is a separate method because the work list above cannot be paged.**
    /// Converting a row in `EXECUTING` produces a row in `RECONCILING`, which the work list still returns
    /// — legitimately, since a reconciling row is durable work. So a pass that paged the work list would
    /// re-read its own output, and rows beyond the page could never be reached: with the ordering tied on
    /// `updated_at`, a second page can hold only already-converted rows, leaving the rest stranded for
    /// ever. This method selects `executing` alone, so **the set strictly shrinks** as the pass converts
    /// rows and the paging terminates for the same reason the run pass's does.
    ///
    /// A caller that wants everything outstanding asks [`Self::possibly_effecting`]; a caller that is
    /// converting asks this one. Two methods, each with one meaning, rather than one whose meaning depends
    /// on which caller named it.
    fn awaiting_conversion(&self, limit: u32) -> RepositoryFuture<'_, EffectingScan>;
}
