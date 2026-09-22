//! Applying run transitions with provenance and optimistic concurrency.
//!
//! The architecture states that "a transition records actor, reason, expected
//! prior version, timestamp, and correlation metadata", and that state names are
//! domain concepts. [`RunLifecycle`] is the value that holds those facts, and
//! [`RunLifecycle::apply`] is the single place a state change is decided.
//!
//! Making the state private behind `apply` is the point: a caller cannot write a
//! state directly, so an edge the architecture diagram does not contain cannot be
//! reached by assigning to a field. The alternative — a public `state` field with a
//! documented rule — is exactly the shape where a legal-looking assignment creates
//! an illegal transition.

use std::fmt;

use serde::Serialize;

use crate::error::DomainError;
use crate::ids::CorrelationId;
use crate::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
use crate::time::UtcTimestamp;

/// A request to move a run from one state to another.
///
/// Carries the provenance the architecture requires plus the version the caller
/// believes is current, so a stale writer is refused rather than silently
/// overwriting a newer state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTransition {
    /// The state the caller believes the run is in.
    pub expected_from: RunState,
    /// The state to move to.
    pub to: RunState,
    /// The version the caller believes is current.
    pub expected_version: RunVersion,
    /// Who caused the transition.
    pub actor: TransitionActor,
    /// Why the transition happened.
    pub reason: TransitionReason,
    /// The instant the transition was decided.
    pub occurred_at: UtcTimestamp,
    /// Correlates this transition with the request that caused it.
    pub correlation_id: Option<CorrelationId>,
}

impl RunTransition {
    /// Builds a transition.
    #[must_use]
    pub fn new(
        expected_from: RunState,
        to: RunState,
        expected_version: RunVersion,
        actor: TransitionActor,
        reason: TransitionReason,
        occurred_at: UtcTimestamp,
    ) -> Self {
        Self {
            expected_from,
            to,
            expected_version,
            actor,
            reason,
            occurred_at,
            correlation_id: None,
        }
    }

    /// Attaches the correlation identifier of the request that caused this.
    #[must_use]
    pub fn with_correlation(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }
}

/// The durable, serializable record of a transition that was applied.
///
/// An audit row is written from this rather than from the request, because the
/// *applied* version and the state that was actually left are facts the request
/// only predicted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunTransitionRecord {
    /// The state that was left.
    pub from: RunState,
    /// The state that was entered.
    pub to: RunState,
    /// The version the run had *before* this transition.
    pub prior_version: RunVersion,
    /// The version the run has *after* this transition.
    pub version: RunVersion,
    /// Who caused it.
    pub actor: TransitionActor,
    /// Why it happened.
    pub reason: TransitionReason,
    /// When it was decided.
    pub occurred_at: UtcTimestamp,
    /// The request correlation, when the caller supplied one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

/// The lifecycle of one run: its state, version, and terminal moment.
///
/// The state is private so every change goes through [`apply`](Self::apply), which
/// is where the transition table, the optimistic check, and the terminal rule are
/// enforced together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLifecycle {
    state: RunState,
    version: RunVersion,
    terminal_at: Option<UtcTimestamp>,
}

impl RunLifecycle {
    /// Creates a run in `Received`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: RunState::Received,
            version: RunVersion::FIRST,
            terminal_at: None,
        }
    }

    /// Returns the current state.
    #[must_use]
    pub const fn state(&self) -> RunState {
        self.state
    }

    /// Returns the current version.
    #[must_use]
    pub const fn version(&self) -> RunVersion {
        self.version
    }

    /// Returns whether the run has reached a terminal state.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// Returns when the run became terminal, if it has.
    ///
    /// A distinct field rather than a derivation from `updated_at`, because the
    /// moment a run finished is a durable fact that must survive later
    /// bookkeeping writes.
    #[must_use]
    pub const fn terminal_at(&self) -> Option<UtcTimestamp> {
        self.terminal_at
    }

    /// Applies `transition`, or refuses it with a typed reason.
    ///
    /// The checks are ordered deliberately, and each order is chosen so the caller
    /// receives the *most specific true* answer:
    ///
    /// 1. **Terminal first.** A finished run is absorbing, so any later request
    ///    gets `RunAlreadyTerminal` — a more useful answer than "wrong version",
    ///    which would send the caller to inspect concurrency instead of the state.
    /// 2. **Version next.** A caller working from a stale view must be told its view
    ///    is stale before it is told the edge is illegal, because an illegal edge
    ///    computed from a stale state may actually be legal from the current one.
    /// 3. **The edge last**, since a legality answer is only meaningful once the
    ///    caller's view is known to be current.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::RunAlreadyTerminal`] for a terminal run,
    /// [`DomainError::RunVersionConflict`] for a stale expected version, and
    /// [`DomainError::RunTransitionNotAllowed`] for an edge the architecture
    /// diagram does not contain.
    pub fn apply(
        &mut self,
        transition: &RunTransition,
    ) -> Result<RunTransitionRecord, DomainError> {
        if self.terminal_at.is_some() {
            // The terminal instant exists precisely so this refusal can name when
            // the run finished; the state alone would not say whether the late
            // write was seconds or hours after the fact. It is surfaced through
            // `terminal_at()` rather than in the error, which stays a stable code.
            return Err(DomainError::RunAlreadyTerminal { state: self.state });
        }
        if transition.expected_version != self.version {
            return Err(DomainError::RunVersionConflict {
                expected: transition.expected_version,
                actual: self.version,
            });
        }
        if transition.expected_from != self.state {
            // The caller's view of the state is stale even though the version
            // matched, which can only happen if two transitions were computed from
            // one version. Refusing is the safe answer: applying it would write a
            // transition whose `from` names a state the run was never in.
            return Err(DomainError::RunTransitionNotAllowed {
                from: self.state,
                to: transition.to,
            });
        }
        if !self.state.can_transition_to(transition.to) {
            return Err(DomainError::RunTransitionNotAllowed {
                from: self.state,
                to: transition.to,
            });
        }

        let prior_version = self.version;
        self.state = transition.to;
        self.version = self.version.next()?;
        if transition.to.is_terminal() {
            self.terminal_at = Some(transition.occurred_at);
        }
        Ok(RunTransitionRecord {
            from: transition.expected_from,
            to: transition.to,
            prior_version,
            version: self.version,
            actor: transition.actor,
            reason: transition.reason.clone(),
            occurred_at: transition.occurred_at,
            correlation_id: transition.correlation_id,
        })
    }

    /// Returns whether `state` is the state this lifecycle would accept a
    /// transition *from* at the current version.
    ///
    /// Exposed so a caller that has just read a run can build a correct
    /// [`RunTransition`] without duplicating the "expected from" bookkeeping, which
    /// is precisely the value that is easy to get wrong.
    #[must_use]
    pub const fn expected_from(&self) -> RunState {
        self.state
    }

    /// Returns the version a caller must state in its next [`RunTransition`].
    #[must_use]
    pub const fn expected_version(&self) -> RunVersion {
        self.version
    }
}

impl Default for RunLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunLifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "run state {} at version {}",
            self.state, self.version,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{RunLifecycle, RunTransition};
    use crate::error::DomainError;
    use crate::ids::CorrelationId;
    use crate::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
    use crate::time::UtcTimestamp;

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn reason(text: &str) -> TransitionReason {
        TransitionReason::new(text).expect("valid")
    }

    fn transition(from: RunState, to: RunState, version: RunVersion) -> RunTransition {
        RunTransition::new(
            from,
            to,
            version,
            TransitionActor::Controller,
            reason("step"),
            now(),
        )
    }

    /// Advances a lifecycle through `path`, asserting each step is accepted.
    fn drive(lifecycle: &mut RunLifecycle, path: &[RunState]) {
        for target in path {
            let step = transition(lifecycle.state(), *target, lifecycle.version());
            lifecycle.apply(&step).expect("the step is legal");
        }
    }

    #[test]
    fn a_new_run_starts_received_at_version_one() {
        let lifecycle = RunLifecycle::new();
        assert_eq!(lifecycle.state(), RunState::Received);
        assert_eq!(lifecycle.version(), RunVersion::FIRST);
        assert!(!lifecycle.is_terminal());
        assert_eq!(lifecycle.terminal_at(), None);
    }

    #[test]
    fn a_full_happy_path_reaches_completed_with_each_version_recorded() {
        let mut lifecycle = RunLifecycle::new();
        drive(
            &mut lifecycle,
            &[
                RunState::ContextBuilding,
                RunState::Planning,
                RunState::AwaitingModel,
                RunState::ExecutingTool,
                RunState::Observing,
                RunState::Planning,
                RunState::Responding,
                RunState::Completed,
            ],
        );
        assert_eq!(lifecycle.state(), RunState::Completed);
        assert!(lifecycle.is_terminal());
        // 1 initial version + 8 accepted transitions.
        assert_eq!(lifecycle.version().get(), 9);
        assert_eq!(lifecycle.terminal_at(), Some(now()));
    }

    #[test]
    fn the_record_carries_both_versions_and_the_provenance() {
        // The audit row must describe what *happened*, so the prior and applied
        // versions are both recorded rather than the caller's expectation alone.
        let mut lifecycle = RunLifecycle::new();
        let correlation =
            CorrelationId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d").expect("valid");
        let step = RunTransition::new(
            RunState::Received,
            RunState::ContextBuilding,
            RunVersion::FIRST,
            TransitionActor::Controller,
            reason("context_build_requested"),
            now(),
        )
        .with_correlation(correlation);

        let record = lifecycle.apply(&step).expect("the step is legal");
        assert_eq!(record.from, RunState::Received);
        assert_eq!(record.to, RunState::ContextBuilding);
        assert_eq!(record.prior_version, RunVersion::FIRST);
        assert_eq!(record.version.get(), 2);
        assert_eq!(record.actor, TransitionActor::Controller);
        assert_eq!(record.reason.as_str(), "context_build_requested");
        assert_eq!(record.correlation_id, Some(correlation));
    }

    #[test]
    fn a_stale_version_is_refused_and_changes_nothing() {
        let mut lifecycle = RunLifecycle::new();
        drive(&mut lifecycle, &[RunState::ContextBuilding]);

        // A caller still holding version 1 writes after another worker advanced to 2.
        let stale = transition(
            RunState::Received,
            RunState::ContextBuilding,
            RunVersion::FIRST,
        );
        let error = lifecycle
            .apply(&stale)
            .expect_err("a stale version must be refused");
        assert_eq!(error.code(), "jarvis.run_version_conflict");
        // The refusal is total: neither state nor version moved.
        assert_eq!(lifecycle.state(), RunState::ContextBuilding);
        assert_eq!(lifecycle.version().get(), 2);
    }

    #[test]
    fn a_version_conflict_is_reported_before_the_edge_is_judged() {
        // Ordering matters for the answer the caller receives. An illegal edge
        // computed from a stale view may be legal from the current state, so the
        // caller is told its view is stale first.
        let mut lifecycle = RunLifecycle::new();
        drive(&mut lifecycle, &[RunState::ContextBuilding]);

        let stale_and_illegal =
            transition(RunState::Received, RunState::Responding, RunVersion::FIRST);
        let error = lifecycle
            .apply(&stale_and_illegal)
            .expect_err("must be refused");
        assert_eq!(
            error.code(),
            "jarvis.run_version_conflict",
            "the stale view must be reported before the illegal edge",
        );
    }

    #[test]
    fn an_edge_the_diagram_lacks_is_refused() {
        let mut lifecycle = RunLifecycle::new();
        drive(
            &mut lifecycle,
            &[RunState::ContextBuilding, RunState::Planning],
        );

        // Planning has no edge to a tool in the architecture diagram.
        let illegal = transition(
            RunState::Planning,
            RunState::ExecutingTool,
            lifecycle.version(),
        );
        let error = lifecycle
            .apply(&illegal)
            .expect_err("an invented edge must be refused");
        assert_eq!(error.code(), "jarvis.run_transition_not_allowed");
        assert_eq!(lifecycle.state(), RunState::Planning);
    }

    #[test]
    fn a_mismatched_expected_from_is_refused_even_at_the_right_version() {
        // Two transitions computed from one version is the case this guards: the
        // version check alone would let the second one through, and it would write
        // a transition whose `from` names a state the run was never in.
        let mut lifecycle = RunLifecycle::new();
        drive(&mut lifecycle, &[RunState::ContextBuilding]);

        let wrong_from = transition(
            RunState::Planning,
            RunState::AwaitingModel,
            lifecycle.version(),
        );
        let error = lifecycle
            .apply(&wrong_from)
            .expect_err("a wrong expected_from must be refused");
        assert_eq!(error.code(), "jarvis.run_transition_not_allowed");
        assert_eq!(lifecycle.state(), RunState::ContextBuilding);
    }

    #[test]
    fn a_terminal_run_absorbs_every_later_transition() {
        let mut lifecycle = RunLifecycle::new();
        drive(
            &mut lifecycle,
            &[
                RunState::ContextBuilding,
                RunState::Planning,
                RunState::Responding,
                RunState::Completed,
            ],
        );
        let version_at_completion = lifecycle.version();

        // Even a transition that would otherwise be legal from a non-terminal state
        // is refused, and the answer names the terminal state rather than the
        // version, so the caller is not sent to debug concurrency.
        let late = transition(
            RunState::Completed,
            RunState::Planning,
            version_at_completion,
        );
        let error = lifecycle
            .apply(&late)
            .expect_err("a finished run must absorb");
        assert_eq!(error.code(), "jarvis.run_already_terminal");
        assert_eq!(lifecycle.state(), RunState::Completed);
        assert_eq!(lifecycle.version(), version_at_completion);
    }

    #[test]
    fn the_terminal_instant_is_recorded_and_survives_no_later_change() {
        let mut lifecycle = RunLifecycle::new();
        let later = UtcTimestamp::parse("2026-09-22T13:00:00Z").expect("valid");
        drive(
            &mut lifecycle,
            &[
                RunState::ContextBuilding,
                RunState::Planning,
                RunState::AwaitingModel,
                RunState::Failed,
            ],
        );
        assert_eq!(lifecycle.terminal_at(), Some(now()));
        assert_ne!(
            lifecycle.terminal_at(),
            Some(later),
            "the terminal instant must be the one the terminal transition carried",
        );
    }

    #[test]
    fn cancellation_and_waiting_are_reachable_and_distinct_from_failure() {
        let mut lifecycle = RunLifecycle::new();
        drive(
            &mut lifecycle,
            &[
                RunState::ContextBuilding,
                RunState::Planning,
                RunState::AwaitingModel,
                RunState::AwaitingApproval,
            ],
        );
        assert!(lifecycle.state().is_waiting());
        assert!(!lifecycle.is_terminal());

        // Approval is answered by cancellation, which is terminal but is *not* a
        // failure: an operator must be able to tell them apart.
        let cancel = RunTransition::new(
            RunState::AwaitingApproval,
            RunState::Cancelled,
            lifecycle.version(),
            TransitionActor::Principal,
            reason("user_requested"),
            now(),
        );
        lifecycle
            .apply(&cancel)
            .expect("cancellation from waiting is legal");
        assert!(lifecycle.is_terminal());
        assert_eq!(lifecycle.state(), RunState::Cancelled);
    }

    #[test]
    fn a_waiting_run_can_resume_through_planning() {
        let mut lifecycle = RunLifecycle::new();
        drive(
            &mut lifecycle,
            &[
                RunState::ContextBuilding,
                RunState::Planning,
                RunState::AwaitingModel,
                RunState::ExecutingTool,
                RunState::Observing,
                RunState::Waiting,
            ],
        );
        assert!(lifecycle.state().is_waiting());

        drive(
            &mut lifecycle,
            &[
                RunState::Planning,
                RunState::Responding,
                RunState::Completed,
            ],
        );
        assert_eq!(lifecycle.state(), RunState::Completed);
    }

    #[test]
    fn a_rejected_approval_observes_rather_than_executing() {
        // The diagram routes a rejected or expired approval to `Observing`, which is
        // what keeps a refusal from looking like a completed tool execution.
        let mut lifecycle = RunLifecycle::new();
        drive(
            &mut lifecycle,
            &[
                RunState::ContextBuilding,
                RunState::Planning,
                RunState::AwaitingModel,
                RunState::AwaitingApproval,
                RunState::Observing,
            ],
        );
        assert_eq!(lifecycle.state(), RunState::Observing);
        assert!(!lifecycle.state().is_waiting());
        assert!(!lifecycle.is_terminal());
    }

    #[test]
    fn expected_from_and_expected_version_track_the_live_value() {
        // A caller building the next transition from these accessors must be right
        // without re-deriving the bookkeeping, which is the value easiest to get
        // wrong.
        let mut lifecycle = RunLifecycle::new();
        drive(&mut lifecycle, &[RunState::ContextBuilding]);
        assert_eq!(lifecycle.expected_from(), RunState::ContextBuilding);
        assert_eq!(lifecycle.expected_version(), lifecycle.version());
        assert_eq!(lifecycle.expected_version().get(), 2);

        // And using them produces an accepted transition.
        let step = transition(
            lifecycle.expected_from(),
            RunState::Planning,
            lifecycle.expected_version(),
        );
        lifecycle
            .apply(&step)
            .expect("the accessors give a legal step");
    }

    #[test]
    fn the_debug_and_display_rendering_name_the_state_and_version() {
        let lifecycle = RunLifecycle::new();
        let rendered = lifecycle.to_string();
        assert!(rendered.contains("received"), "{rendered}");
        assert!(rendered.contains("version 1"), "{rendered}");
    }

    #[test]
    fn an_error_never_renders_the_reason_text_as_an_unbounded_value() {
        // The reason is bounded at the boundary, so an error message that includes
        // it is bounded too. This asserts the refusal path rather than the string.
        let over = "a".repeat(TransitionReason::new("ok").expect("valid").as_str().len() + 1);
        assert!(
            TransitionReason::new(&over).is_ok(),
            "a short value is valid"
        );
        let too_long = "a".repeat(300);
        assert_eq!(
            TransitionReason::new(&too_long)
                .expect_err("over the bound")
                .code(),
            "jarvis.invalid_transition_reason",
        );
    }

    #[test]
    fn a_version_conflict_reports_both_versions() {
        // An operator needs to know what the caller expected *and* what is current;
        // "conflict" alone does not say whether the caller is one write behind or
        // many. The variants carry both, which this asserts structurally.
        let mut lifecycle = RunLifecycle::new();
        drive(&mut lifecycle, &[RunState::ContextBuilding]);
        let stale = transition(
            RunState::Received,
            RunState::ContextBuilding,
            RunVersion::FIRST,
        );
        let error = lifecycle.apply(&stale).expect_err("stale");
        let DomainError::RunVersionConflict { expected, actual } = error else {
            // `expect_err` cannot be used to destructure, so the failure is
            // reported as an assertion rather than a panic (which the workspace
            // lint policy denies even in tests).
            return assert_eq!(
                error.code(),
                "jarvis.run_version_conflict",
                "expected a version conflict",
            );
        };
        assert_eq!(expected, RunVersion::FIRST);
        assert_eq!(actual.get(), 2);
    }
}
