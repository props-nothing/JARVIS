//! Tests for the tool-call recovery pass.
//!
//! Five properties are worth stating up front, because all five are silent failures:
//!
//! - **A dispatched call must end up `RECONCILING`, never retried.** It is the conclusion `ACC-025`
//!   depends on, and the wrong answer is a duplicated effect rather than a confusing message.
//! - **A call that never dispatched must be settled `CANCELLED`, not left holding its reservation.** Its
//!   holder is gone, so leaving it non-terminal makes every later retry of that key answer `InFlight`.
//!   This is asserted across every pre-dispatch state, because the pass's first version could reach none
//!   of them and a test over one state would not have shown that.
//! - **A row already `RECONCILING` must not be written.** The conversion scan excludes it — the pass's
//!   own output — so this is asserted as the scan's reach rather than as a branch, since a branch nothing
//!   can reach is not coverage.
//! - **A failed write must not stop the pass**, or one bad row strands every later one.
//! - **A stalled page must not loop**, or startup hangs. The paging decision is asserted as a pure
//!   function so the hang is unrepresentable in a test rather than an observed timeout.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::repository::tool_call::{EffectingScan, MAX_EFFECTING_SCAN, ToolCallRepository};
use crate::repository::{RepositoryError, RepositoryFuture};
use crate::tool_recovery::{ToolRecoveryReport, reconcile_tool_calls};

use super::{PageOutcome, outcome_of};

use jarvis_domain::ids::{PrincipalId, RunId, ToolCallId, ToolCallRecordId, WorkspaceId};
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use jarvis_domain::tool::ledger::{
    LedgerEntry, LedgerOperation, ReservationKey, ToolCallState, ToolCallTransition,
    ToolCallVersion,
};

/// One row in the double, with the version guard the adapter applies.
#[derive(Debug, Clone)]
struct Row {
    entry: LedgerEntry,
}

/// An in-memory ledger for these tests.
///
/// **It holds no transition table of its own.** Legality comes from `LedgerEntry::apply`, exactly as
/// the SQLite adapter obtains it, so a test against this double cannot pass while the domain's table
/// says otherwise — which is the same discipline the run recovery double records for `RunLifecycle`.
/// It adds only the storage semantics: scope as a predicate, the version guard, and the scan's ordering
/// and bound.
#[derive(Debug, Clone, Default)]
struct LedgerDouble {
    rows: Arc<Mutex<BTreeMap<ToolCallRecordId, Row>>>,
    /// When set, every transition is refused — the state a page must not loop on.
    refuse_writes: Arc<AtomicBool>,
    /// How many writes were attempted, so a test can prove a row was skipped rather than written.
    writes: Arc<AtomicU64>,
}

impl LedgerDouble {
    fn new() -> Self {
        Self::default()
    }

    fn refusing() -> Self {
        let double = Self::new();
        double.refuse_writes.store(true, Ordering::SeqCst);
        double
    }

    fn writes(&self) -> u64 {
        self.writes.load(Ordering::SeqCst)
    }

    fn insert(&self, entry: LedgerEntry) {
        let mut rows = self.rows.lock().expect("the lock is not poisoned");
        rows.insert(entry.id, Row { entry });
    }

    fn row(&self, id: ToolCallRecordId) -> LedgerEntry {
        let rows = self.rows.lock().expect("the lock is not poisoned");
        rows.get(&id).expect("the row exists").entry.clone()
    }

    fn count(&self) -> usize {
        self.rows.lock().expect("the lock is not poisoned").len()
    }

    /// The two scans, which differ only in which states they select.
    ///
    /// `awaiting_conversion` offers every non-terminal state **except** `reconciling` — the pass's own
    /// output — so the double mirrors the adapter's `state IN (...)` list exactly. Getting this narrower
    /// is the defect the real adapter had, so the double must not reproduce it: a double that selected
    /// `executing` alone would let a pass that reached only dispatched calls pass every test.
    fn scan(&self, limit: u32, include_reconciling: bool) -> RepositoryFuture<'_, EffectingScan> {
        Box::pin(async move {
            let rows = self.rows.lock().map_err(|_| RepositoryError::Query)?;
            let mut found: Vec<LedgerEntry> = rows
                .values()
                .filter(|row| {
                    let state = row.entry.state();
                    let wanted = !state.is_terminal()
                        && (include_reconciling || state != ToolCallState::Reconciling);
                    wanted && row.entry.outcome().is_none()
                })
                .map(|row| row.entry.clone())
                .collect();
            // Oldest first, like the adapter's `ORDER BY updated_at ASC`.
            found.sort_by_key(LedgerEntry::updated_at);
            let bounded = found.len() > limit as usize;
            found.truncate(limit as usize);
            Ok(EffectingScan {
                records: found,
                bounded,
            })
        })
    }
}

impl ToolCallRepository for LedgerDouble {
    fn reserve(&self, _entry: &LedgerEntry) -> RepositoryFuture<'_, ReservationOutcomeAlias> {
        // Not exercised by this pass: recovery reads and transitions, it never reserves.
        Box::pin(async { Err(RepositoryError::Query) })
    }

    fn load(
        &self,
        workspace: WorkspaceId,
        record: ToolCallRecordId,
    ) -> RepositoryFuture<'_, LedgerEntry> {
        Box::pin(async move {
            let rows = self.rows.lock().map_err(|_| RepositoryError::Query)?;
            rows.get(&record)
                .filter(|row| row.entry.key.workspace == workspace)
                .map(|row| row.entry.clone())
                .ok_or(RepositoryError::NotFound)
        })
    }

    fn apply_transition(
        &self,
        workspace: WorkspaceId,
        transition: &ToolCallTransition,
        expected: ToolCallVersion,
        entry: &LedgerEntry,
    ) -> RepositoryFuture<'_, ()> {
        let entry = entry.clone();
        let transition = transition.clone();
        Box::pin(async move {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if self.refuse_writes.load(Ordering::SeqCst) {
                return Err(RepositoryError::Query);
            }
            let mut rows = self.rows.lock().map_err(|_| RepositoryError::Query)?;
            let row = rows
                .get_mut(&entry.id)
                .filter(|row| row.entry.key.workspace == workspace)
                .ok_or(RepositoryError::NotFound)?;
            if row.entry.version() != expected {
                return Err(RepositoryError::VersionConflict {
                    expected: expected.get(),
                    actual: row.entry.version().get(),
                });
            }
            // The domain decides the edge; this double only records what it permitted, so a pass that
            // wrote an illegal transition would fail here rather than store it.
            if !row.entry.state().can_transition_to(transition.to) {
                return Err(RepositoryError::TransitionRefused {
                    code: "tool.state_conflict",
                });
            }
            row.entry = entry;
            Ok(())
        })
    }

    fn possibly_effecting(&self, limit: u32) -> RepositoryFuture<'_, EffectingScan> {
        self.scan(limit, true)
    }

    fn awaiting_conversion(&self, limit: u32) -> RepositoryFuture<'_, EffectingScan> {
        // `executing` alone, so the set strictly shrinks as rows are converted — which is what makes the
        // pass's paging terminate. A double that returned reconciling rows here would reproduce the
        // unreachable-page defect the real adapter's narrower predicate exists to avoid.
        self.scan(limit, false)
    }
}

/// The reservation verdict, aliased so this double can decline to implement `reserve` meaningfully
/// without restating the domain's enum.
use jarvis_domain::tool::ledger::ReservationOutcome as ReservationOutcomeAlias;

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:00:00Z").expect("the fixture instant parses")
}

fn later() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:05:00Z").expect("the fixture instant parses")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(1))
}

fn key(suffix: &str) -> ReservationKey {
    ReservationKey::new(
        ToolIdentity {
            capability: ToolCapability::parse("mail.send@1").expect("canonical"),
            source: ToolSource::new(
                SourceKind::Connector,
                "acme.mail",
                ToolVersion::parse("1.0.0").expect("valid"),
            )
            .expect("the fixture source is valid"),
            schema_fingerprint: SchemaFingerprint::from_bytes([7; 32]),
        },
        workspace(),
        PrincipalId::from_uuid(uuid::Uuid::from_u128(1)),
        suffix,
    )
    .expect("a valid key")
}

fn row(record: u128, suffix: &str, at: UtcTimestamp) -> LedgerEntry {
    LedgerEntry::reserve(
        ToolCallId::from_uuid(uuid::Uuid::from_u128(record)),
        1,
        RunId::from_uuid(uuid::Uuid::from_u128(5)),
        key(suffix),
        LedgerOperation::Execute,
        at,
    )
}

/// Drives a row to `to` in memory, so the fixture reaches the state under test.
///
/// **The path is derived by breadth-first search over the domain's own table, not written out here.**
/// The first version hardcoded the sequence and went straight from `REQUESTED` to `EXECUTING` — which
/// is not an edge — so the fixture failed for a reason about the helper rather than about the pass. A
/// second version added `WAITING_APPROVAL` and `APPROVED` by hand and immediately got `REQUESTED ->
/// APPROVED` wrong, which is the same defect a second time. Deriving the path means a helper cannot
/// disagree with the machine it exercises, and the two extra states are reached for free.
fn drive_to(entry: &mut LedgerEntry, to: ToolCallState) {
    let mut frontier: Vec<(ToolCallState, Vec<ToolCallState>)> = vec![(entry.state(), Vec::new())];
    let mut seen: Vec<ToolCallState> = vec![entry.state()];
    let path = loop {
        let (from, path) = frontier.remove(0);
        if from == to {
            break path;
        }
        for next in ToolCallState::ALL.iter().copied() {
            if from.can_transition_to(next) && !seen.contains(&next) {
                seen.push(next);
                let mut extended = path.clone();
                extended.push(next);
                frontier.push((next, extended));
            }
        }
    };
    for step in path {
        let version = entry.version();
        let outcome = if step.is_terminal() {
            Some(ToolErrorClass::ProviderError)
        } else {
            None
        };
        entry
            .apply(step, version, outcome, later())
            .expect("the fixture walk is legal");
    }
}

// ---------------------------------------------------------------------------------------
// The classification the pass is built on.
// ---------------------------------------------------------------------------------------

#[test]
fn the_classifications_target_is_a_legal_edge_for_every_state_it_answers_for() {
    // **The property whose absence was latent, and it is only latent because nothing called the
    // classification.** `RECONCILING -> RECONCILING` is refused by the table, so the target for a row
    // already reconciling is not a transition a caller may make — and the classification's own test
    // drove only the `EXECUTING` case, where the edge is legal, so this was never asserted. Now that a
    // pass applies the classification, an illegal target would be a real failure rather than a
    // hypothetical one.
    use jarvis_domain::tool::ledger::{ToolCallState, classify_interrupted};
    for state in ToolCallState::ALL {
        let Some(action) = classify_interrupted(*state) else {
            continue;
        };
        if action.needs_write() {
            assert!(
                state.can_transition_to(action.target_state()),
                "{state} -> {} must be a legal edge if the pass is to write it",
                action.target_state(),
            );
        } else {
            // The one case with nothing to write: the target equals the state, which the table refuses
            // precisely so a no-op cannot record progress.
            assert_eq!(
                action.target_state(),
                *state,
                "a classification needing no write must target the state it is already in",
            );
            assert_eq!(
                *state,
                ToolCallState::Reconciling,
                "only a reconciling row is already where recovery wants it",
            );
        }
    }
}

#[test]
fn only_a_row_already_reconciling_needs_no_write() {
    // The complement, stated positively so the `matches!` in `needs_write` cannot be widened without
    // failing here. A mutation that made `needs_write` always `true` would re-introduce the illegal
    // self-transition; one that made it always `false` would strand every call.
    use jarvis_domain::tool::ledger::{InterruptedCallAction, ToolCallState};
    let needs = [
        (
            InterruptedCallAction::SafeToRetry {
                was_in: ToolCallState::Requested,
            },
            true,
        ),
        (
            InterruptedCallAction::SafeToRetry {
                was_in: ToolCallState::Reserved,
            },
            true,
        ),
        (
            InterruptedCallAction::Reconcile {
                was_in: ToolCallState::Executing,
            },
            true,
        ),
        (
            InterruptedCallAction::Reconcile {
                was_in: ToolCallState::Reconciling,
            },
            false,
        ),
    ];
    for (action, expected) in needs {
        assert_eq!(action.needs_write(), expected, "{action:?}");
    }
}

// ---------------------------------------------------------------------------------------
// The pass.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_dispatched_call_is_moved_to_reconciling_and_never_cancelled() {
    // **The conclusion `ACC-025` depends on.** A call that reached `EXECUTING` may have effected, so it
    // must be marked as needing a provider read — and the wrong answer here is a duplicated effect, not
    // a confusing message.
    let ledger = LedgerDouble::new();
    let mut stranded = row(10, "stranded", now());
    drive_to(&mut stranded, ToolCallState::Executing);
    ledger.insert(stranded.clone());
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    assert_eq!(report.reconciling, 1);
    assert_eq!(
        report.cancelled, 0,
        "a dispatched call must never be cancelled as retryable"
    );
    assert!(report.is_complete(), "{report:?}");
    let stored = ledger.row(stranded.id);
    assert_eq!(
        stored.state(),
        ToolCallState::Reconciling,
        "**a dispatched call must be marked as unsettled**, so the next reservation is told to \
         reconcile rather than to wait",
    );
    assert!(
        stored.was_dispatched(),
        "the dispatch fact must survive the move, or the retry decision loses its input",
    );
    assert_eq!(
        stored.version().get(),
        5,
        "four fixture steps plus the recovery transition"
    );
}

#[tokio::test]
async fn a_call_that_already_reconciling_is_not_offered_to_the_pass_at_all() {
    // **The case the illegal self-transition hid, and the fix changed *how* it is avoided.** The
    // classification still answers `Reconcile` for a row found in `RECONCILING` — correctly, because such
    // a row may have effected — and `RECONCILING -> RECONCILING` is still not an edge. But the pass no
    // longer meets that case, because the conversion scan excludes `RECONCILING` (the pass's own output)
    // and selects every other non-terminal state: a reconciling row is *outstanding work*, not
    // *unconverted work*, and conflating the two is what made the first version page over its own output
    // and hang.
    //
    // So the assertion is now on the **scan** rather than on `needs_write` alone: the row must not be
    // offered, no write may happen, and it must still be visible through the outstanding-work list that a
    // caller reporting recovery progress uses.
    let ledger = LedgerDouble::new();
    let mut stranded = row(11, "already-reconciling", now());
    drive_to(&mut stranded, ToolCallState::Reconciling);
    ledger.insert(stranded.clone());
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    assert_eq!(
        ledger
            .awaiting_conversion(10)
            .await
            .expect("the conversion input is readable")
            .records
            .len(),
        0,
        "**a reconciling row is outstanding work, not unconverted work**: offering it here is what made \
         the pass page over its own output",
    );
    assert_eq!(
        ledger
            .possibly_effecting(10)
            .await
            .expect("the work list is readable")
            .records
            .len(),
        1,
        "it must still be visible as work awaiting a provider read",
    );

    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    assert_eq!(report.changed(), 0, "this pass had nothing to convert");
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(
        ledger.writes(),
        0,
        "**no write may be attempted for a row already in the target state**: the table refuses a \
         no-op transition, so a write here would fail and be reported as a fault",
    );
    assert_eq!(ledger.row(stranded.id).state(), ToolCallState::Reconciling);
}

#[tokio::test]
async fn a_call_that_never_dispatched_is_cancelled_rather_than_reconciled() {
    // The other half: nothing reached the provider, so nothing about the row is worth preserving as
    // in-flight — and leaving it non-terminal would make a later reservation wait on a process that is
    // gone. Cancelled rather than failed, because no work was lost.
    //
    // **This test used to assert the opposite, and its own name is what gave it away.** It drove a row to
    // `RESERVED` and then asserted `cancelled == 0`, explaining that the effecting scan omits
    // undispatched rows. That was true of the *reporting* scan and false of the *conversion* scan the pass
    // actually pages — which selected `executing` alone, so a stranded pre-dispatch reservation was never
    // settled at all. The assertion matched the bug. A row in `REQUESTED`..`RESERVED` is exactly what the
    // pass must resolve, because the dead process still holds the reservation.
    let ledger = LedgerDouble::new();
    let mut reserved = row(12, "undispatched", now());
    drive_to(&mut reserved, ToolCallState::Reserved);
    ledger.insert(reserved.clone());
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    assert_eq!(
        report.cancelled, 1,
        "**a stranded pre-dispatch reservation must be settled**: its holder is gone, so leaving it \
         non-terminal makes every later retry of this key answer `InFlight` — waiting on a dead process",
    );
    assert_eq!(report.reconciling, 0);
    let stored = ledger.row(reserved.id);
    assert_eq!(stored.state(), ToolCallState::Cancelled);
    assert!(
        !stored.was_dispatched(),
        "nothing reached the provider, so no dispatch may be recorded",
    );
    // The terminal row carries an outcome. `apply` writes `None` for a terminal transition handed `None`,
    // and a terminal row with `NULL` outcome is refused as corruption by the reader — so a pass that wrote
    // one would produce a row the next start cannot read.
    assert_eq!(
        stored.outcome(),
        Some(ToolErrorClass::Cancelled),
        "a terminal row must record why it ended, or the next start refuses it as corrupt",
    );
}

#[tokio::test]
async fn every_state_the_classification_settles_is_offered_to_the_pass() {
    // **The reachability property, asserted over the domain's own state set rather than a list here.**
    // `classify_interrupted` answers for six non-terminal states; the conversion scan must offer exactly
    // those whose settling would shrink the set it pages. The one exclusion is `reconciling` — the pass's
    // own output — and it must be the *only* one, because every other exclusion is a state the
    // classification can name and the pass can never reach.
    use jarvis_domain::tool::ledger::classify_interrupted;

    let ledger = LedgerDouble::new();
    let mut offered = 0_u32;
    for state in ToolCallState::ALL.iter().copied() {
        let Some(action) = classify_interrupted(state) else {
            continue;
        };
        // A row already in its target state is the pass's own output and is deliberately not offered;
        // every other classified state must be, or the classification has nowhere to be applied.
        if action.needs_write() {
            offered += 1;
        }
        let mut row = row(7000 + u128::from(state as u8), "reach", now());
        if state != ToolCallState::Requested {
            drive_to(&mut row, state);
        }
        ledger.insert(row);
    }
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());
    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    // Five pre-dispatch states, each cancelled; `executing` and `reconciling` are the dispatched ones,
    // but only `executing` needs a write (a `reconciling` row is already in its target state and is not
    // offered). Four state values are terminal and classify to `None`.
    assert_eq!(
        report.cancelled, 5,
        "every pre-dispatch state must be reachable and settled: {report:?}",
    );
    assert_eq!(
        report.reconciling, 1,
        "the `executing` row must be the only one converted: {report:?}",
    );
    assert_eq!(
        offered, 6,
        "six non-terminal states need a write; a seventh would mean a state was added without a scan \
         entry, and a fifth would mean one was lost",
    );
    assert!(report.is_complete(), "{report:?}");
    assert!(report.failures.is_empty(), "{:?}", report.failures);
}

#[tokio::test]
async fn a_terminal_call_is_never_touched() {
    // A recorded outcome is not re-derived, and rewriting it would mean re-reporting an effect that
    // landed or erasing the record of one that did. The scan excludes terminal rows, so this asserts
    // the exclusion holds through the whole pass rather than only in the query.
    let ledger = LedgerDouble::new();
    let mut finished = row(13, "finished", now());
    drive_to(&mut finished, ToolCallState::Succeeded);
    ledger.insert(finished.clone());
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    assert_eq!(report.changed(), 0);
    assert_eq!(ledger.writes(), 0);
    assert_eq!(
        ledger.row(finished.id).outcome(),
        Some(ToolErrorClass::ProviderError),
        "the recorded outcome must be exactly as it was",
    );
}

#[tokio::test]
async fn a_failed_write_is_counted_and_does_not_stop_the_pass() {
    // Stopping at the first failure would leave every **later** row unsettled, turning one problem into
    // many. The count is what lets a caller report it.
    //
    // **The page is filled to the adapter's own bound, and the first version was not.** Two rows against
    // a limit of `MAX_EFFECTING_SCAN` leaves a page that is *not* bounded, so the pass correctly reports
    // a complete run and the `incomplete_store` assertion failed for a reason that was about the fixture
    // rather than about the code. A stall is only observable when the store held *more* than one page,
    // so the store is filled past the bound and every write is refused.
    let ledger = LedgerDouble::refusing();
    for index in 0..=MAX_EFFECTING_SCAN {
        let mut stranded = row(500 + u128::from(index), &format!("stranded-{index}"), now());
        drive_to(&mut stranded, ToolCallState::Executing);
        ledger.insert(stranded);
    }
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    assert_eq!(report.changed(), 0);
    assert_eq!(
        report.failures.len(),
        MAX_EFFECTING_SCAN as usize,
        "**every row on the page must be attempted**: one refusal must not stop the pass",
    );
    assert!(
        report.incomplete_store,
        "a full page that changed nothing must be reported as incomplete, or a caller concludes the \
         profile is settled when it is not",
    );
    assert!(!report.is_complete());
}

#[tokio::test]
async fn a_partial_page_whose_writes_all_fail_is_reported_as_complete() {
    // The complement of the stall test, and the distinction the `bounded` flag exists for: a page the
    // store had nothing beyond is *complete* even when every write on it failed. The rows are still
    // reported as failures, so "nothing is left" and "nothing failed" stay separate facts.
    let ledger = LedgerDouble::refusing();
    let mut stranded = row(30, "stranded", now());
    drive_to(&mut stranded, ToolCallState::Executing);
    ledger.insert(stranded.clone());
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    assert_eq!(report.failures.len(), 1);
    assert!(
        !report.incomplete_store,
        "the store held nothing beyond the page, so the run was complete even though it failed",
    );
    assert!(
        !report.is_complete(),
        "a failure still means the pass is not complete"
    );
    assert_eq!(ledger.count(), 1, "a refused write stored nothing");
}

#[tokio::test]
async fn a_pass_is_idempotent() {
    // The second run must change nothing, because the first left no row the scan offers. This is what
    // makes recovery safe to run on every restart, including one after a restart.
    let ledger = LedgerDouble::new();
    let mut stranded = row(16, "stranded", now());
    drive_to(&mut stranded, ToolCallState::Executing);
    ledger.insert(stranded.clone());
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    let first = reconcile_tool_calls(&port, later()).await.expect("runs");
    assert_eq!(first.reconciling, 1);
    let writes_after_first = ledger.writes();

    let second = reconcile_tool_calls(&port, later()).await.expect("runs");
    assert_eq!(
        second.changed(),
        0,
        "a settled store offers nothing: {second:?}"
    );
    assert!(second.is_complete());
    assert_eq!(
        ledger.writes(),
        writes_after_first,
        "the second pass must write nothing at all",
    );
}

#[tokio::test]
async fn a_pass_reports_nothing_when_the_store_holds_nothing() {
    // The empty-profile case, which startup takes on a fresh install. It must be a clean, complete pass
    // rather than an error — and `examined_anything` must stay false so a caller does not report
    // recovery work that did not happen.
    let ledger = LedgerDouble::new();
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger);

    let report = reconcile_tool_calls(&port, later()).await.expect("runs");

    assert_eq!(report, ToolRecoveryReport::default());
    assert!(report.is_complete());
    assert!(!report.examined_anything());
}

#[test]
fn a_full_page_that_converted_nothing_and_failed_nothing_is_drained_rather_than_stalled() {
    // **The regression test for a hung startup path, and a mutation is what found it.** The scan
    // **contains its own output**: converting an `EXECUTING` row produces a `RECONCILING` row that the
    // same scan returns on a later page, because a reconciling row is durable work awaiting a provider
    // read. So a full page of rows already in the target state is this pass's work *finished*, not a
    // stall — and returning `Stalled` for it made a store with more than one page of stranded calls
    // report `incomplete_store` for ever, which the daemon turns into a refusal to start.
    assert_eq!(
        outcome_of(true, 0, 0),
        PageOutcome::Drained,
        "a full page with nothing to convert and nothing refused is work finished, not a stall",
    );
    // The genuine stall: a full page whose every write was refused, so the rows are exactly as the scan
    // finds them and another read would repeat the refusals.
    assert_eq!(
        outcome_of(true, 0, 3),
        PageOutcome::Stalled,
        "a full page that converted nothing because every write failed would loop for ever",
    );
    // Progress on a full page means read on, whether or not some writes also failed.
    assert_eq!(outcome_of(true, 1, 0), PageOutcome::More);
    assert_eq!(
        outcome_of(true, 2, 1),
        PageOutcome::More,
        "a page that converted rows advanced, even if it also refused some",
    );
    // And a page the store had nothing beyond is done regardless.
    assert_eq!(outcome_of(false, 0, 0), PageOutcome::Drained);
    assert_eq!(outcome_of(false, 7, 4), PageOutcome::Drained);
}

#[tokio::test]
async fn a_store_holding_more_than_one_page_of_stranded_calls_drains_rather_than_stalling() {
    // **The end-to-end form of the same defect, because the pure function alone cannot show that the loop
    // uses it correctly.** Every one of these rows needs a conversion, and there are more than
    // `MAX_EFFECTING_SCAN` of them — so the pass must page, convert the whole set, and terminate. The
    // first version converted them all, saw converted rows on the second page, and then terminated on
    // "did this page change anything" — which the reconciling rows made look like a stall, so it reported
    // `incomplete_store` and a daemon would have refused to start on every restart.
    let ledger = LedgerDouble::new();
    let total = MAX_EFFECTING_SCAN + 5;
    for index in 0..total {
        let mut stranded = row(1000 + u128::from(index), &format!("page-{index}"), now());
        drive_to(&mut stranded, ToolCallState::Executing);
        ledger.insert(stranded);
    }
    let port: Arc<dyn ToolCallRepository> = Arc::new(ledger.clone());

    let report = reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs");

    assert_eq!(
        report.reconciling,
        u64::from(total),
        "**every stranded call must be converted, not just the first page**: {report:?}",
    );
    assert!(
        !report.incomplete_store,
        "**the pass must drain rather than report the store unfinished**: a reconciling row comes back \
         from the same scan, so terminating on \"did this page change anything\" would make every restart \
         refuse",
    );
    assert!(report.is_complete(), "{report:?}");
    // Every row was settled, so nothing waits on a provider read: the converted rows are the ones whose
    // outcomes are unknown, and they are counted as reconciled rather than as outstanding.
    assert_eq!(report.reconciling, u64::from(total), "{report:?}");
}
