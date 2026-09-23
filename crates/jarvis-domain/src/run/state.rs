//! Run states and the actor and reason a transition carries.
//!
//! The state set and every legal edge are transcribed from the state diagram in
//! `docs/architecture/agent-runtime.md`. The architecture is explicit that "state
//! names are domain concepts, **not UI strings**", so there is deliberately **no**
//! client-visible projection here: the local control API exposes a coarser state
//! set, and that mapping belongs at the API boundary where the wire contract is
//! owned (`BRN-007`). Putting it here would make one enum carry both a domain
//! concept and a presentation decision, which is the coupling the architecture
//! sentence forbids.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// The longest accepted transition reason.
///
/// A reason reaches the audit record and operator output, so it is bounded at the
/// boundary rather than where it is displayed.
pub const MAX_REASON_BYTES: usize = 256;

/// Where a run is in its lifecycle.
///
/// The full controller state set. Clients see the coarser set on `RunView.state` instead —
/// produced by `jarvis_infrastructure::http::runs::wire_state` from the constants in
/// `jarvis_protocol::run::run_state` — because the local control API deliberately exposes
/// fewer states than the controller tracks.
///
/// This line used to name a `WireRunState` type that **does not exist anywhere in the
/// workspace**. The reference was never checked: rustdoc's `broken_intra_doc_links` is a
/// warning by default and this project did not deny it, so a doc comment pointing a reader at
/// a type nobody had written read exactly like one that resolved. It is the same class of
/// defect as a doc comment claiming a caller for a function none calls, and it is fixed by
/// naming where the projection actually lives rather than by inventing the type the sentence
/// assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// The run exists and has not started work.
    Received,
    /// Identity, policy, and context are being resolved.
    ContextBuilding,
    /// The controller is choosing the next action.
    Planning,
    /// A model call is outstanding.
    AwaitingModel,
    /// A tool intent is waiting for a decision.
    AwaitingApproval,
    /// An approved tool is running.
    ExecutingTool,
    /// A tool outcome, refusal, or expiry is being folded back into the run.
    Observing,
    /// The run is parked on a timer, event, or resumed dependency.
    Waiting,
    /// The final answer is being produced.
    Responding,
    /// The run finished with a result.
    Completed,
    /// The run ended in a normalized error.
    Failed,
    /// The run ended because cancellation was requested.
    Cancelled,
}

impl RunState {
    /// Returns whether this state is terminal.
    ///
    /// Terminal states are absorbing: a late worker must not be able to advance a
    /// finished run, and a second terminal event would break the contract's
    /// "exactly one terminal event" rule.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Returns whether the run is parked rather than progressing.
    ///
    /// A waiting run is *not* stalled: it has a named dependency it is waiting on,
    /// which is why the two waiting states are distinct from `Failed` and why
    /// `BRN-005`'s "explicit waiting states" requirement exists. A run that is
    /// merely not progressing and has no dependency is a fault, and the two must
    /// not look alike to an operator.
    #[must_use]
    pub const fn is_waiting(self) -> bool {
        matches!(self, Self::AwaitingApproval | Self::Waiting)
    }

    /// Returns the states this state may transition to.
    ///
    /// Transcribed from the architecture diagram, not derived from a rule, so a
    /// missing edge in the document stays missing here rather than being filled in
    /// by assumption. An edge the document does not contain is a question for the
    /// architecture owner, and inventing it silently would make the code and the
    /// document disagree while both still looked correct.
    #[must_use]
    pub const fn allowed_targets(self) -> &'static [Self] {
        match self {
            Self::Received => &[Self::ContextBuilding, Self::Cancelled, Self::Failed],
            // A cancellation is legal from every working state, because a caller can ask to
            // stop at any moment and the contract requires the request to reach a durable
            // terminal transition. `ContextBuilding`, `Planning`, and `Observing` were the
            // three states missing that edge, and a real-daemon journey proved it: a run
            // cancelled while building context sat in `context_building` **forever**, with
            // the signal accepted and unrecordable. It is the fifth time an edge absent
            // from the architecture diagram was the defect, and the first found by an
            // executable acceptance test rather than by reading — the diagram has been an
            // optimistic one, describing how a run progresses and not how it stops.
            //
            // `ContextBuilding` and `Waiting` share a target list: both can only advance
            // into `Planning`, fail, or be cancelled. They are merged rather than repeated
            // because the equality is real, not incidental.
            Self::ContextBuilding | Self::Waiting => {
                &[Self::Planning, Self::Failed, Self::Cancelled]
            }
            Self::Planning => &[
                Self::AwaitingModel,
                Self::Responding,
                Self::Failed,
                Self::Cancelled,
            ],
            // `Responding` is reachable directly from `AwaitingModel` because the
            // architecture's native runtime says to "ask a model for either a final
            // response or typed tool intent" — the final response *is* the model
            // call's outcome, so refusing this edge would make a plain
            // question-and-answer run unable to reach `Completed` at all.
            Self::AwaitingModel => &[
                Self::Responding,
                Self::ExecutingTool,
                Self::AwaitingApproval,
                Self::Failed,
                Self::Cancelled,
            ],
            // `AwaitingApproval --> Failed` and `Observing --> Failed` exist because
            // they were the only non-terminal states from which `Failed` was
            // unreachable, which made the restart-recovery rule unimplementable: the
            // local control API requires a non-terminal run found at startup to be
            // recovered "to an explicit resumable or failed state", and a run
            // interrupted while awaiting a decision or while folding back a tool
            // outcome had no legal way to become either.
            Self::AwaitingApproval => &[
                Self::ExecutingTool,
                Self::Observing,
                Self::Failed,
                Self::Cancelled,
            ],
            Self::ExecutingTool => &[Self::Observing, Self::Failed, Self::Cancelled],
            Self::Observing => &[Self::Planning, Self::Waiting, Self::Failed, Self::Cancelled],
            // A failure or a cancellation *while producing the final answer* is a
            // real outcome, not an impossible one: the provider can fail mid-stream
            // and the caller can cancel during it. `ACC-012` requires that a
            // disconnect "never becomes false `Completed`" and `ACC-016` requires
            // cancelling "during ... streaming", so both must be expressible.
            Self::Responding => &[Self::Completed, Self::Failed, Self::Cancelled],
            // Terminal states are absorbing, so the list is empty rather than
            // "everything", which would let a late worker restart a finished run.
            Self::Completed | Self::Failed | Self::Cancelled => &[],
        }
    }

    /// Returns whether `to` is a legal transition from this state.
    #[must_use]
    pub fn can_transition_to(self, to: Self) -> bool {
        self.allowed_targets().contains(&to)
    }
}

impl fmt::Display for RunState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Contract-shaped snake_case, so an operator line and a wire value agree.
        let text = match self {
            Self::Received => "received",
            Self::ContextBuilding => "context_building",
            Self::Planning => "planning",
            Self::AwaitingModel => "awaiting_model",
            Self::AwaitingApproval => "awaiting_approval",
            Self::ExecutingTool => "executing_tool",
            Self::Observing => "observing",
            Self::Waiting => "waiting",
            Self::Responding => "responding",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        formatter.write_str(text)
    }
}

/// Who caused a state transition.
///
/// A closed set rather than a free string, because the actor is an audit input: a
/// caller-invented actor name would make "which component moved this run"
/// unanswerable, and the value would look authoritative while meaning nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionActor {
    /// The authenticated principal that owns the run.
    Principal,
    /// The run controller itself, advancing its own strategy.
    Controller,
    /// Deterministic policy evaluated a tool intent.
    Policy,
    /// A scheduler, timer, or event wait woke the run.
    Scheduler,
    /// An external runtime adapter reported a change.
    ExternalRuntime,
    /// An operator acted through an administrative surface.
    Operator,
    /// The daemon's supervision reconciled a run after a restart.
    ///
    /// Distinct from [`Controller`](Self::Controller) because a recovery transition was
    /// not a decision the run made: recording it as one would misattribute it, and an
    /// operator reading the audit trail needs to see that the daemon, not the run, ended
    /// it.
    Supervisor,
}

impl TransitionActor {
    /// Returns the contract-shaped name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Controller => "controller",
            Self::Policy => "policy",
            Self::Scheduler => "scheduler",
            Self::ExternalRuntime => "external_runtime",
            Self::Operator => "operator",
            Self::Supervisor => "supervisor",
        }
    }
}

impl fmt::Display for TransitionActor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A bounded, non-empty explanation of why a transition happened.
///
/// Free text rather than a closed enum, because reasons are diagnostic detail that
/// grows with the product, but bounded and validated so an unbounded
/// caller-supplied string never reaches an audit row or an operator line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransitionReason(String);

impl TransitionReason {
    /// Validates and builds a reason.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidTransitionReason`] when the value is empty,
    /// longer than [`MAX_REASON_BYTES`], or contains a NUL byte — the same rule the
    /// provider-string bound uses, because both reach persisted records.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() || value.len() > MAX_REASON_BYTES || value.contains('\0') {
            return Err(DomainError::InvalidTransitionReason);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the reason as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransitionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An optimistic-concurrency version for a run.
///
/// Every durable aggregate carries one, and a transition states the version it
/// expects, so two workers cannot both advance one run by writing in sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunVersion(u64);

impl RunVersion {
    /// The version of a freshly created run.
    ///
    /// `1` rather than `0` so a stored version is never the "unset" value, which
    /// makes "the row has no version" indistinguishable from "the row's first
    /// version" if zero is ever used as a sentinel.
    pub const FIRST: Self = Self(1);

    /// Wraps a version value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next version.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::RunVersionExhausted`] at `u64::MAX` rather than
    /// wrapping, because a wrapped version would look like a stale write and the
    /// next optimistic transition would be refused for a reason that is not true.
    pub const fn next(self) -> Result<Self, DomainError> {
        match self.0.checked_add(1) {
            Some(next) => Ok(Self(next)),
            None => Err(DomainError::RunVersionExhausted),
        }
    }
}

impl fmt::Display for RunVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_REASON_BYTES, RunState, RunVersion, TransitionActor, TransitionReason};

    /// Every controller state, so a totality claim can be checked rather than stated.
    const ALL_STATES: [RunState; 12] = [
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
    fn terminal_and_waiting_are_disjoint_and_named_explicitly() {
        for state in ALL_STATES {
            assert!(
                !(state.is_terminal() && state.is_waiting()),
                "{state} must not be both terminal and waiting",
            );
        }
        for state in [RunState::Completed, RunState::Failed, RunState::Cancelled] {
            assert!(state.is_terminal(), "{state} must be terminal");
            assert!(state.allowed_targets().is_empty(), "{state} must absorb");
        }
        // `BRN-005` requires explicit waiting states. A run parked on a dependency
        // is not a failure, so the two must be distinguishable.
        for state in [RunState::AwaitingApproval, RunState::Waiting] {
            assert!(state.is_waiting(), "{state} must be a waiting state");
            assert!(!state.is_terminal(), "{state} must not be terminal");
        }
    }

    #[test]
    fn every_edge_in_the_architecture_diagram_is_present() {
        // Transcribed from `docs/architecture/agent-runtime.md`. Asserting the list
        // rather than sampling it is what keeps the code and the diagram from
        // drifting: a removed edge here would silently forbid a legal transition.
        let expected: [(RunState, &[RunState]); 9] = [
            (
                RunState::Received,
                &[
                    RunState::ContextBuilding,
                    RunState::Cancelled,
                    RunState::Failed,
                ],
            ),
            (
                RunState::ContextBuilding,
                &[RunState::Planning, RunState::Failed, RunState::Cancelled],
            ),
            (
                RunState::Planning,
                &[
                    RunState::AwaitingModel,
                    RunState::Responding,
                    RunState::Failed,
                    RunState::Cancelled,
                ],
            ),
            (
                RunState::AwaitingModel,
                &[
                    RunState::Responding,
                    RunState::ExecutingTool,
                    RunState::AwaitingApproval,
                    RunState::Failed,
                    RunState::Cancelled,
                ],
            ),
            (
                RunState::AwaitingApproval,
                &[
                    RunState::ExecutingTool,
                    RunState::Observing,
                    RunState::Failed,
                    RunState::Cancelled,
                ],
            ),
            (
                RunState::ExecutingTool,
                &[RunState::Observing, RunState::Failed, RunState::Cancelled],
            ),
            (
                RunState::Observing,
                &[
                    RunState::Planning,
                    RunState::Waiting,
                    RunState::Failed,
                    RunState::Cancelled,
                ],
            ),
            (
                RunState::Waiting,
                &[RunState::Planning, RunState::Failed, RunState::Cancelled],
            ),
            (
                RunState::Responding,
                &[RunState::Completed, RunState::Failed, RunState::Cancelled],
            ),
        ];
        for (from, targets) in expected {
            assert_eq!(
                from.allowed_targets(),
                targets,
                "the edges from {from} must match the architecture diagram",
            );
        }
    }

    #[test]
    fn a_plain_question_and_answer_path_reaches_completed() {
        // The product's core loop. Before this edge existed the machine could not
        // express it: `AwaitingModel` reached only tool and failure states, so a
        // plain text answer was stuck and `BRN-007`'s CLI chat had no legal path to
        // `Completed`. The earlier happy-path test did not catch it because it drove
        // a *tool* path through `ExecutingTool`, which is legal but is not the
        // question-and-answer case.
        let path = [
            RunState::ContextBuilding,
            RunState::Planning,
            RunState::AwaitingModel,
            RunState::Responding,
            RunState::Completed,
        ];
        let mut reachable = vec![RunState::Received];
        for pair in path.windows(2) {
            assert!(
                pair[0].can_transition_to(pair[1]),
                "{} must transition to {}",
                pair[0],
                pair[1],
            );
            reachable.push(pair[1]);
        }
        assert!(reachable.last().copied().is_some_and(RunState::is_terminal));
    }

    #[test]
    fn a_failure_or_cancellation_during_the_answer_is_expressible() {
        // `ACC-012` requires a disconnect never to become a false `Completed`, and
        // `ACC-016` requires cancelling during streaming. Both need an edge out of
        // `Responding` that is not `Completed`.
        assert!(RunState::Responding.can_transition_to(RunState::Failed));
        assert!(RunState::Responding.can_transition_to(RunState::Cancelled));
        assert!(RunState::Responding.can_transition_to(RunState::Completed));
    }

    #[test]
    fn an_edge_absent_from_the_diagram_is_refused() {
        // Edges the diagram does not contain stay absent rather than being invented
        // at a call site. `AwaitingModel` has no return to `Planning` (it goes
        // straight to `Responding` or to a tool), and no state skips from
        // `ContextBuilding` to a model call.
        // The edges that remain absent, each of which a caller might reasonably
        // assume: `AwaitingModel` does not return to `Planning` (it goes straight to
        // `Responding` or to a tool), and nothing skips from `ContextBuilding` or
        // `Received` straight to a model call.
        assert!(!RunState::AwaitingModel.can_transition_to(RunState::Planning));
        assert!(!RunState::Received.can_transition_to(RunState::AwaitingModel));
        assert!(!RunState::ContextBuilding.can_transition_to(RunState::AwaitingModel));
        // A run cannot skip from planning straight to a tool.
        assert!(!RunState::Planning.can_transition_to(RunState::ExecutingTool));
        // And nothing leaves a terminal state.
        for terminal in [RunState::Completed, RunState::Failed, RunState::Cancelled] {
            for target in ALL_STATES {
                assert!(
                    !terminal.can_transition_to(target),
                    "{terminal} must not transition to {target}",
                );
            }
        }
    }

    #[test]
    fn no_state_transitions_to_itself() {
        // A self-edge would let a retry loop re-record the same state forever while
        // looking productive, and the diagram contains none.
        for state in ALL_STATES {
            assert!(
                !state.can_transition_to(state),
                "{state} must not have a self-transition",
            );
        }
    }

    #[test]
    fn every_non_terminal_state_has_at_least_one_successor() {
        // A non-terminal state with no successors would be a run that can never
        // progress and never terminate, which is a silent stuck state rather than a
        // reported one. The architecture diagram has no such state.
        for state in ALL_STATES {
            if !state.is_terminal() {
                assert!(
                    !state.allowed_targets().is_empty(),
                    "{state} is non-terminal and must have a successor",
                );
            }
        }
    }

    #[test]
    fn every_non_terminal_state_can_reach_a_terminal_state() {
        // Reachability rather than a direct edge: `ExecutingTool` reaches a terminal
        // through `Observing -> Planning -> Responding -> Completed`, so the check is
        // a search. Without it, a state could be entered and never left.
        fn reaches_terminal(state: RunState, seen: &mut Vec<RunState>) -> bool {
            if state.is_terminal() {
                return true;
            }
            if seen.contains(&state) {
                return false;
            }
            seen.push(state);
            state
                .allowed_targets()
                .iter()
                .any(|target| reaches_terminal(*target, seen))
        }
        for state in ALL_STATES {
            assert!(
                reaches_terminal(state, &mut Vec::new()),
                "{state} must be able to reach a terminal state",
            );
        }
    }

    #[test]
    fn a_transition_reason_is_bounded_and_non_empty() {
        assert!(TransitionReason::new("user_requested").is_ok());
        for bad in ["", "a\0b"] {
            let error = TransitionReason::new(bad).expect_err("must be refused");
            assert_eq!(error.code(), "jarvis.invalid_transition_reason");
        }
        let over = "a".repeat(MAX_REASON_BYTES + 1);
        assert!(TransitionReason::new(&over).is_err());
        assert!(
            TransitionReason::new(&"a".repeat(MAX_REASON_BYTES)).is_ok(),
            "exactly the bound is inside it",
        );
    }

    #[test]
    fn the_version_starts_at_one_and_advances_strictly() {
        assert_eq!(RunVersion::FIRST.get(), 1);
        assert_eq!(RunVersion::FIRST.next().expect("advances").get(), 2);

        // A version cannot wrap: a wrapped value would look like a stale write.
        let maximum = RunVersion::new(u64::MAX);
        let error = maximum.next().expect_err("the version must not wrap");
        assert_eq!(error.code(), "jarvis.run_version_exhausted");
    }

    #[test]
    fn actor_names_are_closed_and_contract_shaped() {
        // A free-form actor would make "which component moved this run"
        // unanswerable while still looking authoritative in an audit row.
        for (actor, expected) in [
            (TransitionActor::Principal, "principal"),
            (TransitionActor::Controller, "controller"),
            (TransitionActor::Policy, "policy"),
            (TransitionActor::Scheduler, "scheduler"),
            (TransitionActor::ExternalRuntime, "external_runtime"),
            (TransitionActor::Operator, "operator"),
        ] {
            assert_eq!(actor.as_str(), expected);
            assert_eq!(actor.to_string(), expected);
        }
    }

    #[test]
    fn state_display_matches_the_wire_spelling() {
        for state in ALL_STATES {
            let text = state.to_string();
            assert!(
                text.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{state} must render snake_case, got {text}",
            );
        }
        assert_eq!(RunState::ContextBuilding.to_string(), "context_building");
        assert_eq!(RunState::AwaitingApproval.to_string(), "awaiting_approval");
    }
}
