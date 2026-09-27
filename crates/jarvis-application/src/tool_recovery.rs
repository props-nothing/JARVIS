//! Startup reconciliation for tool calls a restart interrupted.
//!
//! `BRN-008` built the pass that recovers interrupted **runs**, and `TLS-006` built the ledger's
//! classification and its work list — `classify_interrupted` and a scan for dispatched calls with no
//! recorded outcome. What was missing is the join: nothing called either, so a call that reached the
//! provider and then lost its daemon stayed `EXECUTING` for ever, and the next attempt's reservation
//! found it and answered `InFlight` — telling a caller to **wait on a process that is gone**. A
//! stranded call is not merely un-audited; it blocks the very retry that would resolve it.
//!
//! This pass is the caller. It mirrors the run pass's shape deliberately, because the two answer the
//! same question about two kinds of durable work and a reader should not have to learn it twice:
//!
//! - **The classification is the domain's**, so this module has no policy to get wrong. It reads, asks
//!   `classify_interrupted`, and writes.
//! - **The scan is paged, because a bound is a page size and not a total.** The rows a bound hides are
//!   the newest, so a single-page pass would leave them stranded on this restart and every later one.
//!   `EffectingScan::bounded` is what the caller is told, and the loop stops either when the store is
//!   drained or when a page changed nothing.
//! - **One row's failed write does not stop the pass**, because stopping would leave every later row
//!   unsettled. The failure is counted, so a caller can report it.
//! - **A row is only moved where a move is legal.** `RECONCILING -> RECONCILING` is not an edge, and a
//!   row found already reconciling needs no write — so `InterruptedCallAction::needs_write` decides,
//!   rather than this module comparing states for itself.
//!
//! ## What this pass does *not* do, and it is the important half
//!
//! **It does not reconcile, and it does not retry.** Reconciliation means reading the provider to
//! establish whether an effect landed, which needs a provider connection and a tool invocation this
//! build does not have — there is no executor, and the controller refuses a tool intent outright. So
//! the pass moves a dispatched call from `EXECUTING` to `RECONCILING`, which is the state that says
//! *"the outcome is unknown and must be established before this is repeated"*. That is a real change:
//! a stranded `EXECUTING` row looks in-flight to a reservation, while a `RECONCILING` row is reported
//! as [`ReservationOutcome::Unsettled`](jarvis_domain::tool::ledger::ReservationOutcome::Unsettled)
//! and a caller is told to reconcile rather than wait. **The pass converts a dead in-flight call into
//! a work item**, and leaves the provider read to the slice that can make one.
//!
//! A call that never dispatched ends `CANCELLED` for a related reason: it cannot have effected, so
//! nothing about it is worth preserving as in-flight, and leaving it non-terminal would make a later
//! reservation wait on it. Terminal rows are never touched — a recorded outcome is not re-derived.

use std::sync::Arc;

use jarvis_domain::ids::ToolCallRecordId;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::ledger::{
    InterruptedCallAction, ToolCallState, ToolCallTransition, classify_interrupted,
};

use crate::repository::RepositoryError;
use crate::repository::tool_call::{MAX_EFFECTING_SCAN, ToolCallRepository};

/// Why a tool-call recovery pass could not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRecoveryError {
    /// The stranded calls could not be listed.
    Read(RepositoryError),
}

impl ToolRecoveryError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Read(error) => error.code(),
        }
    }
}

/// What a tool-call recovery pass did, including what it failed to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRecoveryReport {
    /// Calls that were **dispatched** and moved from `EXECUTING` to reconciliation.
    ///
    /// The count that matters most, because each one is an effect whose existence is unknown. Reported
    /// separately from `cancelled` rather than as one "recovered" total: the two need different
    /// operator attention, and a reader given one number would have to guess which they were looking at.
    pub reconciling: u64,
    /// Calls that were **already reconciling** when the pass read them, so no write was needed.
    ///
    /// **A separate counter, and the first version did not have one.** It folded this into
    /// `reconciling`, which made the report say a row had been *moved* when it had not — and it made a
    /// second pass look like it had changed something, so "is recovery idempotent" was unassertable.
    /// The distinction is real and not bookkeeping: a reconciling row legitimately stays on the scan for
    /// as long as its outcome is unknown, because it *is* the work item awaiting a provider read. So a
    /// settled pass reports `reconciling == 0` with this field non-zero, which is the honest description
    /// of "there is work here and it is already marked as work".
    pub awaiting_reconciliation: u64,
    /// Calls that had never been dispatched and were ended as cancelled.
    pub cancelled: u64,
    /// Calls whose write failed, and the code each failure reported.
    pub failures: Vec<(ToolCallRecordId, &'static str)>,
    /// Whether the store still held stranded calls this pass did not read.
    ///
    /// Distinct from `failures` for the reason the run pass records: a failure was *touched* and could
    /// not be settled, while this is a row that was never *reached*. Collapsing them would make
    /// "nothing failed" look like "nothing is left".
    pub incomplete_store: bool,
}

impl ToolRecoveryReport {
    /// Returns the number of calls this pass changed.
    ///
    /// **Writes only.** A row found already reconciling is not counted, which is what makes
    /// `changed() == 0` on a second pass mean "this pass wrote nothing" rather than "this pass wrote
    /// nothing new" — a difference that decides whether a caller may treat the store as settled.
    #[must_use]
    pub const fn changed(&self) -> u64 {
        self.reconciling + self.cancelled
    }

    /// Returns the total number of rows this pass examined, changed or not.
    ///
    /// `changed` answers "did this pass write", this answers "how many rows are still outstanding" — and
    /// a caller reporting recovery progress needs the second, because a reconciling row is durable work
    /// that will be found again on every restart until a provider read resolves it.
    #[must_use]
    pub const fn examined(&self) -> u64 {
        self.changed() + self.awaiting_reconciliation
    }

    /// Returns whether the pass settled everything it saw **and saw everything**.
    ///
    /// Both halves, for the reason the run pass's `is_complete` records: "no write failed" is not "no
    /// row was left behind", and a caller asking whether recovery finished means both.
    ///
    /// **It does not mean the scan is empty**, and that is deliberate: a call awaiting reconciliation
    /// stays outstanding until the provider is read, which is work this pass cannot do. A caller that
    /// needs "is anything still outstanding" asks [`Self::examined`], not this.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && !self.incomplete_store
    }

    /// Returns whether the pass examined at least one call.
    #[must_use]
    pub fn examined_anything(&self) -> bool {
        self.examined() > 0 || !self.failures.is_empty()
    }
}

/// Recovers every stranded tool call in the profile.
///
/// The instant is a parameter rather than a clock read, so a pass is reproducible and a test can assert
/// the exact transition it produced.
///
/// # Errors
///
/// Returns [`ToolRecoveryError::Read`] when the stranded calls cannot be listed. A failure on an
/// individual row's write is **not** an error: it is recorded in [`ToolRecoveryReport::failures`],
/// because stopping the pass there would leave every later row unsettled too.
pub async fn reconcile_tool_calls(
    calls: &Arc<dyn ToolCallRepository>,
    at: UtcTimestamp,
) -> Result<ToolRecoveryReport, ToolRecoveryError> {
    let mut report = ToolRecoveryReport::default();

    loop {
        // **The conversion input, not the outstanding-work list.** The work list contains this pass's own
        // output — converting an `EXECUTING` row produces a `RECONCILING` row that the work list still
        // returns — so paging it would re-read rows already converted and could never reach the ones
        // beyond the first page. `awaiting_conversion` selects `EXECUTING` alone, so the set strictly
        // shrinks as rows are converted and the paging terminates.
        let scan = calls
            .awaiting_conversion(MAX_EFFECTING_SCAN)
            .await
            .map_err(ToolRecoveryError::Read)?;
        let bounded = scan.bounded;
        // **The drain signal is rows *converted*, not rows examined, and the first version used the
        // wrong one — which a mutation turned into a hung startup path.** This pass pages over a set
        // that **contains its own output**: the scan selects undispatched-outcome rows in `EXECUTING` or
        // `RECONCILING`, and converting an `EXECUTING` row produces a `RECONCILING` row that the very
        // same scan still returns — legitimately, because a reconciling row is durable work awaiting a
        // provider read. So a converted row comes back on a later page, and terminating on "did this
        // page change anything" made a store with more than one page of stranded calls look **stalled
        // for ever**, which the daemon turns into a refusal to start.
        //
        // The two facts a page can honestly report are therefore counted separately: how many rows it
        // *converted*, and how many writes *failed*. A page that converted nothing and failed nothing
        // held only rows already in the target state, so there is no conversion work left and the store
        // is drained **of this pass's work** — which is a different thing from having no unsettled calls,
        // and the report keeps them apart.
        let moved_before = report.reconciling;
        let failed_before = report.failures.len();

        for entry in scan.records {
            apply(calls, &entry, at, &mut report).await;
        }

        let moved = report.reconciling - moved_before;
        let failed = report.failures.len() - failed_before;

        match outcome_of(bounded, moved, failed) {
            PageOutcome::Drained => {
                report.incomplete_store = false;
                break;
            }
            PageOutcome::More => {}
            PageOutcome::Stalled => {
                report.incomplete_store = true;
                break;
            }
        }
    }

    Ok(report)
}

/// Applies one stranded call's recovery, recording the outcome on `report`.
async fn apply(
    calls: &Arc<dyn ToolCallRepository>,
    entry: &jarvis_domain::tool::ledger::LedgerEntry,
    at: UtcTimestamp,
    report: &mut ToolRecoveryReport,
) {
    // The classification is asked rather than derived here, so a call's fate has one definition.
    let Some(action) = classify_interrupted(entry.state()) else {
        // Unreachable, because the scan returns only dispatched non-terminal rows. Skipped rather than
        // treated as an error, so a store that returned a terminal row cannot cause a spurious write.
        return;
    };

    match settle(calls, entry, action, at).await {
        Ok(()) => {
            match action {
                InterruptedCallAction::Reconcile { was_in } => {
                    // **Two counters for one action, because a row moved and a row already there are
                    // different facts.** A reconciling row stays on the scan until its outcome is known,
                    // so counting it as "moved" would make every pass report work it did not do — and
                    // would make a second pass look like a change, which is what an idempotency test has
                    // to be able to see.
                    if was_in == ToolCallState::Reconciling {
                        report.awaiting_reconciliation =
                            report.awaiting_reconciliation.saturating_add(1);
                    } else {
                        report.reconciling = report.reconciling.saturating_add(1);
                    }
                }
                InterruptedCallAction::SafeToRetry { .. } => {
                    report.cancelled = report.cancelled.saturating_add(1);
                }
            }
        }
        Err(error) => {
            // The row stays where it is, so the next pass — or the next restart — sees it again.
            report.failures.push((entry.id, error));
        }
    }
}

/// Moves one call to the state the classification requires.
///
/// **The move is made through the domain's `apply`, not by writing a state.** That is what makes the
/// optimistically-checked version travel and what refuses an edge the table does not contain — and it
/// is why `needs_write` exists: a row already in `RECONCILING` has nothing to transition to, so
/// `apply` would refuse it and this pass would record a failure for a row that is already correct.
async fn settle(
    calls: &Arc<dyn ToolCallRepository>,
    entry: &jarvis_domain::tool::ledger::LedgerEntry,
    action: InterruptedCallAction,
    at: UtcTimestamp,
) -> Result<(), &'static str> {
    if !action.needs_write() {
        // `RECONCILING -> RECONCILING` is not an edge, and the row is already where recovery wants it.
        // Counted as settled by the caller, because a row found correct is not a row left behind.
        return Ok(());
    }
    let mut row = entry.clone();
    let expected = row.version();
    let transition: ToolCallTransition = match row.apply(action.target_state(), expected, None, at)
    {
        Ok(transition) => transition,
        // A refused edge means the classification and the table disagree, which is a defect in one of
        // them rather than a store fault — reported under the refusal's own code so it is diagnosable
        // instead of looking like a database error.
        Err(_) => return Err("tool.state_conflict"),
    };
    calls
        .apply_transition(entry.key.workspace, &transition, expected, &row)
        .await
        // `code` takes `&self`, so it cannot be passed directly as the `map_err` function here.
        .map_err(|error| error.code())
}

/// What a paging pass must do after handling one page.
///
/// Deliberately the same three answers as the run pass's `PageOutcome`, but reached from **two** counts
/// rather than one, because this pass's scan contains its own output: a converted row returns on a later
/// page, so "did this page change anything" is not a termination signal. See the paging loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageOutcome {
    /// This pass's conversion work is done, whether or not the store still holds unsettled calls.
    Drained,
    /// The store held more than this page and this page converted something, so read on.
    More,
    /// The store held more than this page, this page converted **nothing**, and every write it attempted
    /// failed. Reading it again would behave identically, so the store is reported as still holding rows.
    Stalled,
}

/// Decides what to do after one page.
///
/// A free function rather than a method so all three answers are assertable without a store — the same
/// reason the run pass extracted it. The two arguments are the two facts a page can report honestly:
/// `moved` is rows **converted** (not rows examined, and not rows already in the target state), and
/// `failed` is writes refused.
///
/// **A full page that converted nothing and failed nothing is `Drained`, not `Stalled`.** The first
/// version returned `Stalled` for it, which hung: a page of rows that are all already reconciling has no
/// work to do, so reading it again would find the same rows and do nothing again — for ever. The
/// distinction that matters is whether the *store* refused to make progress; a page that simply held
/// nothing this pass could convert is finished.
#[must_use]
fn outcome_of(bounded: bool, moved: u64, failed: usize) -> PageOutcome {
    match (bounded, moved) {
        // The store held nothing beyond this page, so the pass saw everything and is done.
        (false, _) => PageOutcome::Drained,
        // A full page that converted a row means the read advanced past work it had done.
        (true, 1..) => PageOutcome::More,
        // A full page that converted nothing: either it held only rows already in the target state, which
        // is this pass's work finished, or every write it attempted was refused — the stall. The two are
        // told apart by the failure count alone, which is why it is the second half of the judgement.
        (true, 0) if failed == 0 => PageOutcome::Drained,
        // A full page that converted nothing because **every write was refused** is the stall: the rows
        // are still exactly as the scan finds them, so another read would repeat the refusals.
        (true, 0) => PageOutcome::Stalled,
    }
}

#[cfg(test)]
#[path = "tool_recovery/tests.rs"]
mod tests;
