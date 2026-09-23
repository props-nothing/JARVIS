//! Startup reconciliation: recovering runs that a restart interrupted.
//!
//! The local control API requires that a non-terminal run found at startup be
//! "recovered to an explicit resumable or failed state". This module is the pass that
//! does it, and it lives in the application layer rather than in `jarvisd` because two
//! callers need it and they must not disagree: the daemon runs it before it reports
//! ready, and `jarvis doctor` runs it to report what a restart would do. Putting it in
//! the binary would either duplicate the logic or force the client to open the database,
//! which the CLI is built on *not* doing — "the daemon is the single authority".
//!
//! Four decisions are deliberate:
//!
//! - **The classification is the domain's, not this module's.** `run::recovery`
//!   decides what an interrupted run's fate must be, so this pass has no policy to get
//!   wrong; it reads, asks, and writes.
//! - **Each run is written independently.** One run whose write fails must not stop the
//!   pass, because stopping would leave every *later* run non-terminal — turning one
//!   problem into many. The failure is counted instead, so a caller can report it.
//! - **A terminal run is never touched.** Recovery reads only non-terminal runs, and the
//!   write it makes is a transition, so the domain's optimistically-checked version
//!   guard refuses a run that reached a terminal state between the read and the write
//!   rather than overwriting a real outcome.
//! - **Nothing is resumed.** The classification distinguishes a parked run from an
//!   abandoned one, but both are moved to `Failed`, because resuming means re-running a
//!   model call and nothing here knows what the interrupted call had already produced.
//!   A resumable run is *reported* as resumable; resuming it is later work.

use std::sync::Arc;

use jarvis_domain::ids::RunId;
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::recovery::{RecoveryAction, classify};
use jarvis_domain::run::state::{RunState, TransitionActor, TransitionReason};
use jarvis_domain::time::UtcTimestamp;

use crate::repository::RepositoryError;
use crate::repository::run::{
    EventVisibility, NewActivityEvent, RecoverySummary, RunRepository, RunWrite, TerminalOutcome,
};

/// Why a recovery pass could not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    /// The incomplete runs could not be read.
    Read(RepositoryError),
}

impl RecoveryError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Read(error) => error.code(),
        }
    }
}

/// What a pass did, including what it failed to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationReport {
    /// The runs this pass recovered.
    pub summary: RecoverySummary,
    /// Runs whose recovery write failed, and the code each failure reported.
    ///
    /// Reported rather than swallowed: a run this pass could not recover is still
    /// non-terminal, so a caller that hides the failure leaves a run nobody will look at
    /// again until the next restart.
    pub failures: Vec<(RunId, &'static str)>,
    /// Whether the store still held interrupted runs this pass did not read.
    ///
    /// Distinct from `failures`, and the distinction is the whole reason it exists: a failure is a
    /// run that was *touched* and could not be settled, while this is a run that was never
    /// *reached*. Collapsing the two would make "nothing failed" look like "nothing is left".
    pub incomplete_store: bool,
}

impl ReconciliationReport {
    /// Returns whether the pass recovered everything it saw **and saw everything**.
    ///
    /// Both halves matter and only one used to be checked. "No write failed" is not "no run was
    /// left behind": the store reads interrupted runs one bounded page at a time, ordered
    /// oldest-first, so a single-page pass against a store holding more than
    /// [`MAX_INCOMPLETE_RUNS`](crate::repository::run::MAX_INCOMPLETE_RUNS) would report a clean
    /// recovery while leaving the **newest** runs non-terminal — and would do so on every restart,
    /// because the same oldest page would be recovered each time. A caller asking "did recovery
    /// finish" has to mean both, so this returns the conjunction rather than the failure check.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && !self.incomplete_store
    }

    /// Returns whether the pass examined at least one run.
    #[must_use]
    pub fn examined_anything(&self) -> bool {
        self.examined() > 0
    }

    /// Returns the number of runs this pass examined.
    #[must_use]
    pub fn examined(&self) -> u64 {
        self.summary.total() + u64::try_from(self.failures.len()).unwrap_or(u64::MAX)
    }
}

/// Recovers every non-terminal run in the profile.
///
/// The instant and the actor are parameters rather than reads of a clock, so a pass is
/// reproducible and a test can assert the exact transition it produced.
///
/// # Errors
///
/// Returns [`RecoveryError::Read`] when the incomplete runs cannot be listed. A failure
/// on an individual run's write is **not** an error: it is recorded in
/// [`ReconciliationReport::failures`], because stopping the pass there would leave every
/// later run non-terminal too.
pub async fn reconcile(
    runs: &Arc<dyn RunRepository>,
    at: UtcTimestamp,
) -> Result<ReconciliationReport, RecoveryError> {
    let mut report = ReconciliationReport {
        summary: RecoverySummary::default(),
        failures: Vec::new(),
        incomplete_store: false,
    };

    // **The read is paged, because a bound is a page size and not a total.** One pass handled one
    // page, and interrupted runs come oldest-first, so a store holding more than
    // `MAX_INCOMPLETE_RUNS` interrupted runs would have its newest ones left non-terminal — on
    // this restart and every later one, because each pass would recover the same oldest page and
    // stop. That is the precise state this pass exists to prevent, hiding behind a bound that
    // looked like it only limited memory.
    //
    // The loop terminates on either of two facts, and both are needed:
    //
    //   - the store reports it stopped at its bound (`bounded == false` means it saw everything);
    //   - a pass recovered nothing. Without this, a page of runs whose writes all failed would be
    //     read, fail, and be read again for ever — because a failed write leaves the run
    //     non-terminal, which is exactly what this read looks for. **A repair loop must be able to
    //     conclude as well as continue**, and the only honest conclusion is "this page did not
    //     change, so another read of it would behave the same".
    loop {
        let page = runs.incomplete_runs().await.map_err(RecoveryError::Read)?;
        let bounded = page.bounded;
        let before = report.summary.total();

        for entry in page.runs {
            apply(runs, entry, at, &mut report).await;
        }

        // The decision is a pure function so both of its dangerous answers are assertable: a
        // `loop` with the wrong condition hangs the startup path, and a `break` with the wrong one
        // strands runs for ever.
        match page_outcome(bounded, report.summary.total() - before) {
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

/// Applies one interrupted run's recovery, recording the outcome on `report`.
///
/// Extracted from the pass so the paging loop above reads as paging rather than as a mix of paging
/// and recovery, and so there is exactly one place a run's fate is decided and recorded.
async fn apply(
    runs: &Arc<dyn RunRepository>,
    entry: crate::repository::run::IncompleteRun,
    at: UtcTimestamp,
    report: &mut ReconciliationReport,
) {
    // The classification is asked per run rather than derived here, so the fate of an
    // interrupted run has exactly one definition.
    let Some(action) = classify(entry.run.state) else {
        // Unreachable in practice, because the read only returns non-terminal runs.
        // Skipped rather than treated as an error, so a store that returned a
        // terminal run cannot cause a spurious recovery write.
        return;
    };

    let sequence = match runs
        .next_event_sequence(entry.workspace_id, entry.run.id)
        .await
    {
        Ok(sequence) => sequence,
        Err(error) => {
            report.failures.push((entry.run.id, error.code()));
            return;
        }
    };

    // The reason comes from the classification, whose own test asserts it is
    // non-empty and inside the bound. A failure here would mean that invariant broke,
    // so it is reported as a failed recovery rather than papered over with a
    // fallback reason — a recovered run with an invented reason would be worse than
    // one left for the next pass.
    let Ok(reason) = TransitionReason::new(action.reason()) else {
        report
            .failures
            .push((entry.run.id, "jarvis.invalid_transition_reason"));
        return;
    };

    let transition = RunTransition::new(
        entry.run.state,
        action.target_state(),
        entry.run.version,
        // The actor is the supervisor, not the controller: this transition was not a
        // decision the run made, and recording it as one would misattribute it.
        TransitionActor::Supervisor,
        reason,
        at,
    );

    let event = NewActivityEvent {
        run_id: entry.run.id,
        sequence,
        // The event type names the *outcome*, so a client following the stream learns
        // the run ended rather than being told a state changed to nothing.
        event_type: terminal_event_for(action).to_owned(),
        // The payload carries the classification and the state the run was in, which
        // is what an operator needs to tell "was parked" from "lost work".
        payload_json: Some(recovery_payload(action, entry.run.state)),
        visibility: EventVisibility::Public,
        occurred_at: at,
    };

    // The outcome travels on the write so a recovered run's own row carries the code a
    // client reads. `RecoveryAction` already computes it for the event payload, so the two
    // cannot disagree about why the run ended.
    let write = match action.target_state() {
        RunState::Failed => RunWrite::new(&transition, event)
            .failed_with(TerminalOutcome::failed(action.error_code())),
        _ => RunWrite::new(&transition, event),
    };
    match runs.transition(entry.workspace_id, write).await {
        Ok(_) => {
            if action.was_resumable() {
                report.summary.parked = report.summary.parked.saturating_add(1);
            } else {
                report.summary.abandoned = report.summary.abandoned.saturating_add(1);
            }
        }
        Err(error) => {
            // The run stays non-terminal, so the next pass — or the next restart — will see it
            // again. Counted rather than returned, because stopping here would leave every later
            // run non-terminal too.
            report.failures.push((entry.run.id, error.code()));
        }
    }
}

/// The terminal event type for a recovery action.
fn terminal_event_for(action: RecoveryAction) -> &'static str {
    // Both classifications end the run, so both publish the failure terminal. The
    // distinction survives in the payload and the reason, which is where a consumer that
    // cares about it looks.
    let _ = action;
    "run.failed"
}

/// Builds the redacted public payload for a recovery event.
///
/// A fixed shape with no caller text: the payload reports the classification, the state
/// the run was in, and the error code, and nothing else can appear in it. It is built by
/// hand rather than through a serializer so this module needs no JSON dependency, and the
/// fields are escaped from a closed set of identifiers so no escaping is required.
fn recovery_payload(action: RecoveryAction, was_in: RunState) -> String {
    let (classification, parked_in) = match action {
        RecoveryAction::Resumable { .. } => ("resumable", format!("\"{was_in}\"")),
        RecoveryAction::Abandoned { .. } => ("abandoned", "null".to_owned()),
    };
    // Both values come from a closed set — a literal and a domain state's own spelling —
    // so neither can contain a character that would break the document.
    format!(
        "{{\"classification\":\"{classification}\",\"was_in\":\"{was_in}\",\"parked_in\":{parked_in},\"code\":\"{}\"}}",
        action.error_code(),
    )
}

/// What a paging pass must do after handling one page.
///
/// A judgement made in one place rather than inline in the loop, because the dangerous case is
/// subtle: a page whose writes all failed leaves its runs in exactly the state the next read
/// looks for, so "read again" is not always the right answer. Getting this wrong in the
/// conservative direction is an infinite loop on the startup path; getting it wrong in the
/// permissive direction is a run left non-terminal for ever. A pure function is the only way to
/// assert both without one of them hanging a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageOutcome {
    /// The store held nothing more, and everything offered was handled.
    Drained,
    /// The store held more than this page, and this page changed something, so read on.
    More,
    /// The store held more than this page and this page changed **nothing**, so reading it again
    /// would behave identically. Stopped, and the store is reported as still holding runs.
    Stalled,
}

/// Decides what to do after one page.
///
/// `settled` counts the runs this page actually moved, which is what makes the difference between
/// continuing and stopping. A page can be full and settle nothing when every write failed, or when
/// every run was already terminal between the read and the write.
#[must_use]
fn page_outcome(bounded: bool, settled: u64) -> PageOutcome {
    match (bounded, settled) {
        (false, _) => PageOutcome::Drained,
        (true, 0) => PageOutcome::Stalled,
        (true, _) => PageOutcome::More,
    }
}

#[cfg(test)]
mod tests;
