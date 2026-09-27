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
//! - **Every non-terminal state is offered except `reconciling`.** The classification answers for six
//!   states — `Reconcile` for `executing`/`reconciling`, `SafeToRetry` for the five pre-dispatch ones —
//!   and a scan narrower than that leaves a classification with nowhere to be applied. That is not
//!   hypothetical: this pass's first version selected `executing` alone, so a stranded **pre-dispatch**
//!   reservation was never settled and its dead holder blocked every retry of that key for ever — the
//!   very harm this pass exists to remove, applied to the states it could not see. `reconciling` is the
//!   one exclusion, because the pass's own output for an `executing` row is a `reconciling` row; every
//!   other state strictly shrinks (`cancelled` is terminal, and `reconciling` is excluded), so the set
//!   the loop pages is strictly smaller after each conversion and the paging terminates.
//! - **A terminal target carries an outcome**, because a terminal row with a `NULL` outcome is refused
//!   as corruption by the reader — a pass that wrote one would produce a row the next start cannot read.
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
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::ledger::{
    InterruptedCallAction, ToolCallTransition, classify_interrupted,
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
    ///
    /// A row found **already** `RECONCILING` is not counted here or anywhere: the conversion scan
    /// selects every non-terminal state *except* `reconciling`, so such a row is never offered to the
    /// pass. It is not lost — `possibly_effecting` returns it as outstanding work awaiting a provider
    /// read — but a pass cannot convert work whose conversion has already happened, and counting it as
    /// this pass's would make a second pass look like it had changed something.
    pub reconciling: u64,
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
    /// Returns the number of calls this pass settled.
    ///
    /// Each is a row the pass moved to a terminal or reconciled state — `reconciling` for a dispatched
    /// call, `cancelled` for one that never dispatched. It is the only count of what the pass did:
    /// a row already in its target state is not offered to the pass at all, so there is no third
    /// "already correct" category to report.
    #[must_use]
    pub const fn changed(&self) -> u64 {
        self.reconciling + self.cancelled
    }

    /// Returns the total number of rows this pass settled.
    ///
    /// The same value as [`Self::changed`], and kept as a named method because a caller asking "how many
    /// rows did recovery look at" is a different question from "how many did it write" even where the
    /// answer coincides. The two were once different — a reconciling row was counted here and not in
    /// `changed` — but the conversion scan excludes reconciling rows, so that category is unproducible
    /// and the second counter was removed rather than left as a field nothing could ever set.
    #[must_use]
    pub const fn examined(&self) -> u64 {
        self.changed()
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
        // **The conversion input, not the outstanding-work list.** The work list (`possibly_effecting`)
        // returns `executing` **and** `reconciling` rows — the latter is this pass's own output for an
        // `executing` row — so paging it would re-read rows already converted and could never reach the
        // ones beyond the first page. `awaiting_conversion` returns every non-terminal state **except**
        // `reconciling`, so the set strictly shrinks as rows are settled and the paging terminates.
        let scan = calls
            .awaiting_conversion(MAX_EFFECTING_SCAN)
            .await
            .map_err(ToolRecoveryError::Read)?;
        let bounded = scan.bounded;
        // **The drain signal is rows *settled*, not rows examined, and the first version used the wrong
        // one — which a mutation turned into a hung startup path.** A page's honest facts are how many
        // rows it *settled* and how many writes *failed*. A page that settled nothing and failed nothing
        // held only rows already in their target state — which cannot happen for a scan that excludes
        // `reconciling`, but the branch is kept because `outcome_of` is the decision and a scan widened
        // again must not silently hang. A page whose every write failed **is** the genuine stall: the rows
        // are exactly as the scan finds them, so reading again would repeat the refusals.
        let settled_before = report.changed();
        let failed_before = report.failures.len();

        for entry in scan.records {
            apply(calls, &entry, at, &mut report).await;
        }

        // Both settled outcomes count as progress: a cancelled call and a reconciling one both leave the
        // set the scan returns. Counting only `reconciling` would make a page of pre-dispatch rows look
        // like a stall and report the store unfinished for ever.
        let moved = report.changed() - settled_before;
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
        Ok(()) => match action {
            // A dispatched call whose outcome is unknown: moved to `RECONCILING`, where the next
            // reservation is told to reconcile rather than to wait.
            InterruptedCallAction::Reconcile { .. } => {
                report.reconciling = report.reconciling.saturating_add(1);
            }
            // A call that never reached the provider: ended as cancelled, because nothing about it is
            // worth preserving as in-flight — and leaving it reserved is what makes a later retry wait
            // on a process that is gone.
            InterruptedCallAction::SafeToRetry { .. } => {
                report.cancelled = report.cancelled.saturating_add(1);
            }
        },
        Err(error) => {
            // The row stays where it is, so the next pass — or the next restart — sees it again.
            report.failures.push((entry.id, error));
        }
    }
}

/// Moves one call to the state the classification requires.
///
/// **The move is made through the domain's `apply`, not by writing a state.** That is what makes the
/// optimistically-checked version travel and what refuses an edge the table does not contain.
///
/// **The `needs_write` guard is defence-in-depth here rather than a live branch.** The conversion scan
/// excludes `reconciling`, so the pass can only be handed an action whose target differs from the row's
/// state, and `classify_interrupted` returns `needs_write() == false` for exactly one input — a row
/// found in `reconciling` — which this pass therefore never sees. It is kept because the branch belongs
/// to `settle`'s contract rather than to one caller: a row already in its target state has nothing to
/// transition to, and a widened scan would otherwise write an `RECONCILING -> RECONCILING` no-op the
/// table refuses, recording a failure for a row that is already correct. The doc says so because a
/// reader who greps `needs_write` will find exactly one use and should know it is not on the live path.
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
    let target = action.target_state();
    // **A terminal target must carry an outcome, and `Cancelled` is the outcome for a cancelled call.**
    // `LedgerEntry::apply` records `None` for a terminal transition when handed `None`, and a terminal
    // row with a `NULL` outcome is refused as corruption by both `LedgerEntry::restore` and the adapter's
    // reader — so the very row this pass wrote would be unreadable on the next start. The class comes
    // from the classification's own target rather than from a caller's guess: only `SafeToRetry` settles
    // a call as cancelled, and its reason is that the call was stopped rather than that it failed.
    let outcome = target.is_terminal().then_some(ToolErrorClass::Cancelled);
    let transition: ToolCallTransition = match row.apply(target, expected, outcome, at) {
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
/// Deliberately the same three answers as the run pass's `PageOutcome`, reached from **two** counts: how
/// many rows the page settled and how many writes it failed. A page that settled nothing and failed
/// nothing is not a stall — nothing needed settling — and only a page whose every write was refused is,
/// because the rows then sit exactly as the scan finds them and another read would repeat the refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageOutcome {
    /// This pass's settling work is done, whether or not the store still holds reconciling calls.
    Drained,
    /// The store held more than this page and this page settled something, so read on.
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
