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
    EventVisibility, NewActivityEvent, RecoverySummary, RunRepository, RunWrite,
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
}

impl ReconciliationReport {
    /// Returns whether every incomplete run was recovered.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
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
    let incomplete = runs.incomplete_runs().await.map_err(RecoveryError::Read)?;

    let mut report = ReconciliationReport {
        summary: RecoverySummary::default(),
        failures: Vec::new(),
    };

    for entry in incomplete {
        // The classification is asked per run rather than derived here, so the fate of an
        // interrupted run has exactly one definition.
        let Some(action) = classify(entry.run.state) else {
            // Unreachable in practice, because the read only returns non-terminal runs.
            // Skipped rather than treated as an error, so a store that returned a
            // terminal run cannot cause a spurious recovery write.
            continue;
        };

        let sequence = match runs
            .next_event_sequence(entry.workspace_id, entry.run.id)
            .await
        {
            Ok(sequence) => sequence,
            Err(error) => {
                report.failures.push((entry.run.id, error.code()));
                continue;
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
            continue;
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

        match runs
            .transition(entry.workspace_id, RunWrite::new(&transition, event))
            .await
        {
            Ok(_) => {
                if action.was_resumable() {
                    report.summary.parked = report.summary.parked.saturating_add(1);
                } else {
                    report.summary.abandoned = report.summary.abandoned.saturating_add(1);
                }
            }
            Err(error) => report.failures.push((entry.run.id, error.code())),
        }
    }

    Ok(report)
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

#[cfg(test)]
mod tests;
