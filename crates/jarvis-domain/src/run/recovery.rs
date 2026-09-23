//! Restart recovery: what to do with a run that was interrupted.
//!
//! A daemon can stop while a run is mid-flight — a crash, an unclean kill, a failed
//! upgrade. The local control API is explicit about what must happen next:
//!
//! > On restart, terminal runs remain terminal. Nonterminal runs are recovered to an
//! > explicit resumable or failed state; they are never inferred complete from partial
//! > text.
//!
//! This module is that rule as a value. It is deliberately a **pure classification**:
//! given a run's durable state, it says what that run's fate must be and why. It reads
//! no storage and signals nothing, so the decision can be tested exhaustively without a
//! database, and the adapter that applies it has no policy of its own to get wrong.
//!
//! Three decisions are deliberate:
//!
//! - **A run is never resumed automatically.** Resuming means re-running a model call,
//!   which is a side effect on a provider. This slice has no budget, no retry
//!   ownership rule, and no record of what the interrupted call had already produced,
//!   so resuming from here would spend money and duplicate an effect on a guess.
//!   `Resumable` is therefore a *classification a caller may act on*, not an action
//!   this code takes.
//! - **A run that produced output still fails, and the partial text is not discarded.**
//!   The contract requires that a partial answer never become a completed one, and the
//!   output already exists as durable `run.output_text.delta` events, so the honest
//!   outcome is `Failed` with those events intact. Deleting them would destroy the only
//!   record of what the caller already saw.
//! - **Waiting is classified separately from working.** A run parked on a timer or an
//!   approval has a *named* dependency, so its interruption is a different fact from a
//!   run that was mid-call. Collapsing the two would tell an operator that a parked run
//!   "failed", which it did not.

use crate::run::state::RunState;

/// What startup must do with a run found in a non-terminal state.
///
/// The variants are exhaustive over the non-terminal states: every one of them either
/// was parked on a named dependency or was working, so there is no third case and no
/// variant for "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// The run had a named dependency and could be resumed once it is satisfied.
    ///
    /// No work is lost, because a parked run was not doing anything: its durable state
    /// says what it waits for. This slice records the classification but does not
    /// resume it — see `docs/architecture/agent-runtime.md` on waits being
    /// first-class — because nothing here can satisfy a dependency yet.
    Resumable {
        /// The state it was parked in, which is what a resuming caller must know.
        parked_in: RunState,
    },
    /// The run was working and its work did not survive the restart.
    Abandoned {
        /// The state it was working in.
        was_in: RunState,
    },
}

impl RecoveryAction {
    /// The state the run must be moved to.
    ///
    /// Both classifications lead to `Failed`, and that is the honest answer for both:
    /// a parked run cannot resume because nothing can satisfy its dependency, and an
    /// abandoned run cannot continue because the model call that was in flight has no
    /// outcome. Reporting `Waiting` for a parked run would leave it non-terminal
    /// forever, which is the defect this whole module exists to fix.
    #[must_use]
    pub const fn target_state(self) -> RunState {
        RunState::Failed
    }

    /// The bounded reason recorded with the recovery transition.
    ///
    /// A stable code rather than prose: it reaches the run's activity event and the
    /// operator's diagnostics, and a caller switching on it needs a value that does not
    /// change when someone improves a sentence.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Resumable { .. } => "interrupted_while_waiting",
            Self::Abandoned { .. } => "interrupted_while_working",
        }
    }

    /// The normalized error code recorded on the run.
    #[must_use]
    pub const fn error_code(self) -> &'static str {
        "run.interrupted_by_restart"
    }

    /// Whether the run had a resumable dependency rather than lost work.
    #[must_use]
    pub const fn was_resumable(self) -> bool {
        matches!(self, Self::Resumable { .. })
    }
}

/// Classifies a run found in `state` at startup.
///
/// Returns `None` for a terminal state, because a terminal run is left exactly as it
/// is: recovering it would rewrite a finished run's outcome, and the contract's first
/// words on this subject are that "terminal runs remain terminal".
///
/// This is deliberately exhaustive over the non-terminal states rather than defaulting
/// to one classification. A new state added to [`RunState`] will fail to compile here,
/// which forces the person adding it to decide what an interrupted run in it means —
/// the alternative is a silent default that reports the wrong thing for a state nobody
/// thought about.
#[must_use]
pub const fn classify(state: RunState) -> Option<RecoveryAction> {
    match state {
        // A terminal run is not recovered.
        RunState::Completed | RunState::Failed | RunState::Cancelled => None,
        // Parked on a named dependency: nothing was lost, and the run's own durable
        // state says what it waits for.
        RunState::AwaitingApproval | RunState::Waiting => {
            Some(RecoveryAction::Resumable { parked_in: state })
        }
        // Working: a model call, a tool call, an observation, or the final answer was
        // in flight, and none of them has an outcome that survived the restart.
        RunState::Received
        | RunState::ContextBuilding
        | RunState::Planning
        | RunState::AwaitingModel
        | RunState::ExecutingTool
        | RunState::Observing
        | RunState::Responding => Some(RecoveryAction::Abandoned { was_in: state }),
    }
}

/// Whether a run in `state` needs a recovery transition at startup.
///
/// A named predicate rather than a `classify`-and-match at each call site, because
/// "which runs need reconciling" is the first question a recovery pass asks and it
/// should read as one.
#[must_use]
pub const fn needs_recovery(state: RunState) -> bool {
    classify(state).is_some()
}

#[cfg(test)]
mod tests {
    use super::{RecoveryAction, classify, needs_recovery};
    use crate::run::state::RunState;

    /// Every state in the machine, so a test cannot accidentally omit one.
    const ALL: [RunState; 12] = [
        RunState::Received,
        RunState::ContextBuilding,
        RunState::Planning,
        RunState::AwaitingModel,
        RunState::AwaitingApproval,
        RunState::ExecutingTool,
        RunState::Observing,
        RunState::Waiting,
        RunState::Responding,
        RunState::Completed,
        RunState::Failed,
        RunState::Cancelled,
    ];

    #[test]
    fn a_terminal_run_is_never_recovered() {
        // "Terminal runs remain terminal" is the contract's first rule here, and
        // recovering one would rewrite a finished run's outcome.
        for state in [RunState::Completed, RunState::Failed, RunState::Cancelled] {
            assert_eq!(classify(state), None, "{state} must not be recovered");
            assert!(!needs_recovery(state), "{state} must not need recovery");
        }
    }

    #[test]
    fn every_non_terminal_state_is_classified_and_none_is_left_alone() {
        // The exhaustive check: a non-terminal run left unclassified would stay
        // non-terminal forever, which is the defect this module exists to fix.
        for state in ALL.iter().copied().filter(|state| !state.is_terminal()) {
            assert!(
                classify(state).is_some(),
                "{state} is non-terminal and must have a recovery action",
            );
            assert!(needs_recovery(state), "{state} must need recovery");
        }
    }

    #[test]
    fn a_parked_run_is_resumable_and_a_working_run_is_abandoned() {
        // The distinction that matters: a parked run lost nothing, so an operator must
        // not be told it "failed" as if work had been destroyed.
        for state in [RunState::AwaitingApproval, RunState::Waiting] {
            let action = classify(state).expect("parked states are classified");
            assert_eq!(
                action,
                RecoveryAction::Resumable { parked_in: state },
                "{state} was parked, not working",
            );
            assert!(action.was_resumable(), "{state}");
        }
        for state in [
            RunState::Received,
            RunState::ContextBuilding,
            RunState::Planning,
            RunState::AwaitingModel,
            RunState::ExecutingTool,
            RunState::Observing,
            RunState::Responding,
        ] {
            let action = classify(state).expect("working states are classified");
            assert_eq!(
                action,
                RecoveryAction::Abandoned { was_in: state },
                "{state}"
            );
            assert!(!action.was_resumable(), "{state}");
        }
    }

    #[test]
    fn every_recovery_action_targets_a_state_the_run_can_actually_reach() {
        // The property the earlier diagram lacked: an action whose target state is not a
        // legal transition from where the run is would be unimplementable, which is
        // exactly what `AwaitingApproval` and `Observing` were before this change.
        for state in ALL.iter().copied() {
            let Some(action) = classify(state) else {
                continue;
            };
            let target = action.target_state();
            assert!(
                state.can_transition_to(target),
                "{state} cannot reach {target}, so its recovery is unimplementable",
            );
            assert!(target.is_terminal(), "{target} must be terminal");
        }
    }

    #[test]
    fn every_recovery_action_names_a_reason_and_an_error_code() {
        for state in ALL.iter().copied() {
            let Some(action) = classify(state) else {
                continue;
            };
            // The reason is bounded by the transition type's own limit, so it cannot be
            // refused when the transition is built.
            assert!(
                action.reason().len() <= crate::run::state::MAX_REASON_BYTES,
                "{} is over the reason bound",
                action.reason(),
            );
            assert!(action.error_code().starts_with("run."), "{state}");
            // The two reasons are distinct, so a diagnostic can tell the cases apart.
            assert!(!action.reason().is_empty());
        }
        assert_ne!(
            RecoveryAction::Resumable {
                parked_in: RunState::Waiting
            }
            .reason(),
            RecoveryAction::Abandoned {
                was_in: RunState::Planning
            }
            .reason(),
        );
    }

    #[test]
    fn both_classifications_land_on_the_same_terminal_state() {
        // One target for both cases, so recovery has one outcome to reason about, and
        // the difference between the cases survives only in the recorded reason.
        assert_eq!(
            RecoveryAction::Resumable {
                parked_in: RunState::Waiting
            }
            .target_state(),
            RunState::Failed,
        );
        assert_eq!(
            RecoveryAction::Abandoned {
                was_in: RunState::Planning
            }
            .target_state(),
            RunState::Failed,
        );
    }
}
