//! The run repository port.
//!
//! `docs/architecture/storage-data.md` requires purpose-specific operations, an
//! explicit scope on every read, and a transition method that enforces the expected
//! version and state. This module is that shape:
//!
//! - [`RunRepository::create`] inserts a run in `Received` and reports a duplicate
//!   as a conflict rather than a silent second row.
//! - [`RunRepository::transition`] applies a [`RunTransition`] and appends the run's
//!   durable activity event **in one transaction**. Doing both in one unit is the
//!   first required atomic use case in the storage architecture, and it is what
//!   makes "persist the transition before publishing an event that claims it
//!   occurred" true rather than merely intended: there is no window in which the
//!   event exists and the state does not.
//! - [`RunRepository::load`] and [`RunRepository::load_for_resume`] are scoped reads.
//!   A run in another workspace is [`RepositoryError::NotFound`], not a forbidden
//!   result, because the local control API requires the two to be indistinguishable.

use crate::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{ConversationId, PrincipalId, RunActivityEventId, RunId, WorkspaceId};
use jarvis_domain::run::budget::RunBudget;
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::state::{RunState, RunVersion};
use jarvis_domain::time::UtcTimestamp;

/// The largest accepted objective reference.
///
/// A reference, not the objective text: large content lives in artifacts with
/// relational metadata (see the schema document), so an unbounded string here
/// would be the one place large content crept back into a row.
pub const MAX_REFERENCE_BYTES: usize = 512;

/// The largest accepted runtime identity or version string.
///
/// Bounded because both reach persisted columns and both are validated against what the build
/// supports: an unbounded value would let a caller write a runtime identity no reader can match.
/// The bound matches `MAX_REFERENCE_BYTES`'s order of magnitude rather than being an arbitrary new
/// number — a runtime name and its version are identifiers, not content.
pub const MAX_RUNTIME_ID_BYTES: usize = 128;

/// The runtime a run is executed by, and the version of it.
///
/// Recorded on the run rather than kept in the request, because the request is gone the moment it
/// returns while the run outlives it: `agent-runtime.md` requires a resume to "validate runtime
/// identity/version", and a run whose row could not say which runtime executed it could not be
/// validated against anything.
///
/// Before this, `agent_runs.runtime_id` and `runtime_version` existed in the schema and were
/// referenced by **no code at all**: `CreateRunRequest.runtime` was required and validated by the
/// handler and then discarded, so every row carried `NULL` for both. The handler's check is real
/// — it refuses a runtime this build does not support — but it recorded nothing, which is the
/// "validated and then dropped" shape rather than a missing check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRuntime {
    /// The runtime's stable identifier (`jarvis-native`), which is what a resume matches on.
    pub id: String,
    /// The runtime's version, so a resume can tell a compatible runtime from a changed one.
    pub version: String,
}

impl RunRuntime {
    /// The native runtime this build executes runs with.
    ///
    /// The literal rather than `jarvis_protocol::NATIVE_RUNTIME`, because this crate cannot depend
    /// on `jarvis-protocol`; the two are asserted equal by a cross-check test in the crate that can
    /// name both, which is the technique this workspace uses for every duplicated contract string.
    ///
    /// The version is the **package** version rather than the wire contract version: the contract
    /// version says which protocol an event frame speaks, while this says which build executed the
    /// run, and a resume needs the second — a runtime's behaviour can change without its wire shape
    /// changing.
    #[must_use]
    pub fn native() -> Self {
        Self {
            id: NATIVE_RUNTIME_ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Builds a runtime identity, validating both fields.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when either value is empty, over
    /// [`MAX_RUNTIME_ID_BYTES`], or contains a NUL byte. Both reach a persisted column, and the
    /// schema cannot bound a `TEXT` column — so the rule lives here, at the one place a run is
    /// built, rather than at each adapter.
    pub fn new(id: &str, version: &str) -> Result<Self, RepositoryError> {
        for value in [id, version] {
            if value.is_empty() || value.len() > MAX_RUNTIME_ID_BYTES || value.contains('\0') {
                return Err(RepositoryError::Conflict {
                    what: "run_runtime_identity",
                });
            }
        }
        Ok(Self {
            id: id.to_owned(),
            version: version.to_owned(),
        })
    }
}

/// The native runtime's identifier.
///
/// Public so the cross-check test can name it rather than restating the literal, which would make
/// the test agree with a second copy instead of with the value itself.
pub const NATIVE_RUNTIME_ID: &str = "jarvis-native";

/// The fields needed to create a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    /// The run to create.
    pub id: RunId,
    /// The workspace that owns it. Scope is a parameter, never inferred.
    pub workspace_id: WorkspaceId,
    /// The conversation it belongs to.
    pub conversation_id: ConversationId,
    /// The principal that requested it.
    pub principal_id: PrincipalId,
    /// A bounded reference to the objective, or `None`.
    pub objective_ref: Option<String>,
    /// The instant the run was created.
    pub created_at: UtcTimestamp,
    /// The runtime this run is executed by, and its version.
    ///
    /// Required rather than optional: a run is always executed by *something*, and an absent value
    /// would make "no runtime recorded" and "the native runtime" indistinguishable — which is
    /// exactly the state every row was in while these columns were unreferenced. The constructors
    /// default it to [`RunRuntime::native`], so a run records one without every call site naming
    /// it, and [`with_runtime`](Self::with_runtime) overrides it.
    pub runtime: RunRuntime,
    /// The per-run wall-clock deadline, which `agent_runs.deadline_at` stores.
    ///
    /// Held separately from [`budget`](Self::budget) because the schema keeps it in its
    /// own column: a deadline is queried and ordered by (a startup pass or a scheduler
    /// wants runs ordered by when they expire), while the rest of the budget is opaque.
    pub deadline_at: Option<UtcTimestamp>,
    /// The full run budget, serialized into `agent_runs.budget_json`.
    ///
    /// Stored rather than re-derived on every process start, because recovery reads this
    /// run when the request that created it no longer exists. A budget reconstructed
    /// from current defaults would silently re-fund a run whose deadline had already
    /// passed, which is the opposite of what a budget is for.
    pub budget: RunBudget,
}

impl NewRun {
    /// Builds a run creation request with no deadline and no budget cap.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when `objective_ref` is over
    /// [`MAX_REFERENCE_BYTES`] or contains a NUL byte, because an unbounded
    /// caller-supplied reference would otherwise reach a persisted row.
    pub fn new(
        id: RunId,
        workspace_id: WorkspaceId,
        conversation_id: ConversationId,
        principal_id: PrincipalId,
        objective_ref: Option<String>,
        created_at: UtcTimestamp,
    ) -> Result<Self, RepositoryError> {
        Self::with_budget(
            id,
            workspace_id,
            conversation_id,
            principal_id,
            objective_ref,
            created_at,
            RunBudget::default(),
        )
    }

    /// Returns this request with the runtime that will execute it.
    ///
    /// An override rather than a required constructor argument, so a caller that only has the
    /// native runtime — every caller today — does not have to name it, while a build that serves
    /// more than one can. The default is [`RunRuntime::native`], which is **true** of this build:
    /// the create handler refuses any runtime it does not support, so native is the only one that
    /// can reach here.
    #[must_use]
    pub fn with_runtime(mut self, runtime: RunRuntime) -> Self {
        self.runtime = runtime;
        self
    }

    /// Builds a run creation request under `budget`.
    ///
    /// The deadline is taken from the budget rather than passed separately, so a run
    /// cannot be stored with a `deadline_at` column that disagrees with the budget the
    /// executor will read. The two representations are one input here.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when `objective_ref` is over
    /// [`MAX_REFERENCE_BYTES`] or contains a NUL byte.
    pub fn with_budget(
        id: RunId,
        workspace_id: WorkspaceId,
        conversation_id: ConversationId,
        principal_id: PrincipalId,
        objective_ref: Option<String>,
        created_at: UtcTimestamp,
        budget: RunBudget,
    ) -> Result<Self, RepositoryError> {
        if objective_ref.as_ref().is_some_and(|value| {
            value.is_empty() || value.len() > MAX_REFERENCE_BYTES || value.contains('\0')
        }) {
            return Err(RepositoryError::Conflict {
                what: "run_objective_ref",
            });
        }
        Ok(Self {
            id,
            workspace_id,
            conversation_id,
            principal_id,
            objective_ref,
            created_at,
            // The native runtime is the default because it is the only one this build can serve;
            // a caller that runs another overrides it through `with_runtime`.
            runtime: RunRuntime::native(),
            deadline_at: budget.deadline,
            budget,
        })
    }
}

/// A loaded run, in the terms the controller needs.
///
/// Deliberately not the full row: the controller needs identity, state, version,
/// and the waiting/terminal facts, and exposing more would invite a caller to
/// build policy decisions from storage columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRun {
    /// The run identifier.
    pub id: RunId,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
    /// The conversation it belongs to.
    pub conversation_id: ConversationId,
    /// The requesting principal.
    pub principal_id: PrincipalId,
    /// The current state.
    pub state: RunState,
    /// The optimistic version.
    pub version: RunVersion,
    /// A bounded reference to the objective.
    pub objective_ref: Option<String>,
    /// The instant it was created.
    pub created_at: UtcTimestamp,
    /// The instant it started, once it has.
    pub started_at: Option<UtcTimestamp>,
    /// The instant it last changed.
    pub updated_at: UtcTimestamp,
    /// The instant it reached a terminal state, once it has.
    pub completed_at: Option<UtcTimestamp>,
    /// The normalized error code, when it failed.
    pub error_code: Option<String>,
    /// The per-run wall-clock deadline, when one was set.
    ///
    /// Carried on the read rather than looked up separately, because the recovery pass
    /// needs it to decide whether an interrupted run had already run out of time: a
    /// run whose deadline passed is not "interrupted work", it is expired work.
    pub deadline_at: Option<UtcTimestamp>,
    /// The runtime that executed it, when recorded.
    ///
    /// Optional **on the read** while required on the create, and the asymmetry is the schema's:
    /// the columns are nullable, so a row written before this existed has none. Reading it as
    /// required would make every pre-existing row unpresentable, which is why the migration is
    /// additive and the read is tolerant.
    pub runtime: Option<RunRuntime>,
    /// The run budget this run was created under.
    pub budget: RunBudget,
}

impl StoredRun {
    /// Returns whether the run has reached a terminal state.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }
}

/// What a resumed run needs, which is more than what a display read needs.
///
/// `storage-data.md` names this operation separately from a plain load, and the
/// distinction is real: a resume needs the terminal instant to decide whether the
/// run is finished, and the waiting reference to know what it was parked on, while
/// a status read needs neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResumeState {
    /// The run's identity, state, and version.
    pub run: StoredRun,
    /// The kind of dependency the run is waiting on, when it is waiting.
    pub waiting_kind: Option<String>,
    /// The specific reference being awaited, when it is waiting.
    pub waiting_ref: Option<String>,
}

/// The dependency a run is parked on.
///
/// The schema requires a waiting run to name both a kind and a reference, so this
/// is supplied with a transition into a waiting state rather than left to the
/// schema to reject. Requiring it at the port means "a waiting run has nothing to
/// wait for" is unrepresentable instead of merely invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingOn {
    /// The kind of dependency, such as `approval` or `timer`.
    pub kind: String,
    /// The specific reference being awaited.
    pub reference: String,
}

impl WaitingOn {
    /// Builds a waiting reference.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when either part is empty, over
    /// [`MAX_REFERENCE_BYTES`], or contains a NUL byte.
    pub fn new(kind: &str, reference: &str) -> Result<Self, RepositoryError> {
        let bad = |value: &str| {
            value.is_empty() || value.len() > MAX_REFERENCE_BYTES || value.contains('\0')
        };
        if bad(kind) || bad(reference) {
            return Err(RepositoryError::Conflict { what: "waiting_on" });
        }
        Ok(Self {
            kind: kind.to_owned(),
            reference: reference.to_owned(),
        })
    }
}

/// The typed outcome a failed run settles with.
///
/// The code is a `&'static str` rather than a `String` because every failure this layer produces
/// is a compile-time constant, and a stored code is a client-visible identifier: allowing an
/// owned string would let one be built at runtime from a provider's message, which is the way
/// provider internals reach a stable code.
///
/// `code` is the same namespaced string the controller's own error reports, so the run's durable
/// row and the returned error cannot disagree about why it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOutcome {
    /// The stable, namespaced error code.
    pub code: &'static str,
}

impl TerminalOutcome {
    /// Builds a terminal failure with `code`.
    #[must_use]
    pub const fn failed(code: &'static str) -> Self {
        Self { code }
    }
}

/// One atomic run write: a transition, the dependency it enters or leaves, the outcome
/// it settles with, and the activity event that describes it.
///
/// Bundled because `docs/architecture/storage-data.md` defines "transition run state
/// and append its durable activity event" as **one** use case. Passing the parts
/// separately would let a caller move the state without its event, which is the exact
/// window the atomicity requirement exists to close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWrite<'a> {
    /// The transition to apply. Its edge legality is checked by the repository,
    /// which delegates to the domain's transition table.
    pub transition: &'a RunTransition,
    /// The dependency being entered, required when the target state is a waiting
    /// state and refused when it is not.
    pub waiting: Option<WaitingOn>,
    /// The typed outcome, required when the target state is `Failed`.
    ///
    /// Carried on the write rather than scraped later from the event's reason string,
    /// because the reason is a short human label (`context_unassembled`) while the client's
    /// `error_code` is a namespaced code — and deriving one from the other would make a
    /// cosmetic change to a log label silently change a client-visible identifier.
    pub outcome: Option<TerminalOutcome>,
    /// The activity event describing the transition.
    pub event: NewActivityEvent,
}

impl<'a> RunWrite<'a> {
    /// Builds a write that leaves the run in a non-waiting, non-failed state.
    #[must_use]
    pub fn new(transition: &'a RunTransition, event: NewActivityEvent) -> Self {
        Self {
            transition,
            waiting: None,
            // A non-`Failed` target must not carry an outcome; this is the ordinary case, and
            // `is_consistent` is what makes the rule enforceable rather than remembered.
            outcome: None,
            event,
        }
    }

    /// Attaches the dependency the run is entering.
    #[must_use]
    pub fn waiting_on(mut self, waiting: WaitingOn) -> Self {
        self.waiting = Some(waiting);
        self
    }

    /// Attaches the typed outcome a failed run settles with.
    #[must_use]
    pub fn failed_with(mut self, outcome: TerminalOutcome) -> Self {
        self.outcome = Some(outcome);
        self
    }

    /// Returns whether this write is internally consistent.
    ///
    /// A waiting target must name a dependency and a non-waiting target must not, which is the
    /// same rule the schema's `CHECK` enforces. Checking it here means the mismatch is a typed
    /// refusal rather than a constraint failure the caller has to decode.
    ///
    /// A `Failed` target must name its outcome and every other target must not. The adapter
    /// *relies* on this: it writes `error_code` when — and only when — the target is `Failed`,
    /// because a `completed` run's stale `error_code` would describe a failure that did not
    /// happen, and a retried run that failed once must not keep reporting that failure after it
    /// succeeds. That reliance is why the rule is asserted here rather than left to each caller.
    ///
    /// **The event's run is deliberately not checked here, and cannot be.**
    /// [`RunTransition`](jarvis_domain::run::lifecycle::RunTransition) carries no run identity at
    /// all — only the two states, the expected version, the actor, the reason, and the instant — so
    /// there is nothing on this type to compare "the run the transition moves" against. The rule
    /// belongs to the layer that knows the run, and it lives there: the adapter checks the opening
    /// event in `insert_run` and the transitioning event in `transition`, because the run is a
    /// parameter of both. Asserting it here would require adding a run field to the transition,
    /// which is a domain change rather than a validation one.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.transition.to.is_waiting() == self.waiting.is_some()
            && (self.transition.to == RunState::Failed) == self.outcome.is_some()
    }
}

/// The durable activity event appended with a transition.
///
/// A projection for clients, not the private model trace: `payload_json` carries
/// already-redacted public detail, and `visibility` separates what an ordinary
/// client may see from what only an operator may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewActivityEvent {
    /// The run it belongs to.
    pub run_id: RunId,
    /// The next position in that run's event stream.
    pub sequence: u64,
    /// The event type, such as `run.context_building`.
    pub event_type: String,
    /// Bounded, already-redacted public payload.
    pub payload_json: Option<String>,
    /// Who may see it.
    pub visibility: EventVisibility,
    /// When it occurred.
    pub occurred_at: UtcTimestamp,
}

/// A loaded activity event, as a client stream needs it.
///
/// Carries its own [`id`](Self::id) because a client resumes by echoing that value
/// as `Last-Event-ID`, so it must be stable and globally unique rather than derived
/// from the run and sequence at read time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredActivityEvent {
    /// The globally unique event identifier.
    pub id: RunActivityEventId,
    /// The run it belongs to.
    pub run_id: RunId,
    /// Its position in the run's stream.
    pub sequence: u64,
    /// The event type.
    pub event_type: String,
    /// The redacted public payload, when the event has one.
    pub payload_json: Option<String>,
    /// Who may see it.
    pub visibility: EventVisibility,
    /// When it occurred.
    pub occurred_at: UtcTimestamp,
}

/// An idempotency record for a run-creating command.
///
/// The contract requires a repeated create with the same canonical request to
/// return the original run while the same key with different input is a conflict.
/// Storing the request digest rather than the request body is what makes the second
/// comparison possible without retaining caller text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIdempotencyRecord {
    /// The caller-supplied key.
    pub key: String,
    /// The workspace the command was made in.
    pub workspace_id: WorkspaceId,
    /// The operation the key was used for.
    pub operation: String,
    /// The digest of the canonical request.
    pub request_digest: String,
    /// The run the first use created.
    pub run_id: RunId,
    /// When the record was created.
    pub created_at: UtcTimestamp,
}

/// The outcome of claiming an idempotency key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyClaim {
    /// The key was unused and is now claimed.
    Claimed,
    /// The key was already used for an identical request; return this run.
    Replay(RunId),
    /// The key was already used for a different request.
    Conflict,
}

/// A run that startup found in a non-terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteRun {
    /// The workspace that owns it, which a recovery write must name.
    pub workspace_id: WorkspaceId,
    /// The run's identity, state, and version.
    pub run: StoredRun,
    /// What it was parked on, when it was waiting.
    pub waiting_kind: Option<String>,
    /// The specific reference it was waiting for, when it was waiting.
    pub waiting_ref: Option<String>,
}

/// The largest number of incomplete runs one reconciliation page may return.
///
/// Bounded for the same reason every other read here is: a database left with a very
/// large number of interrupted runs must not be pulled into memory at once by the
/// startup path.
pub const MAX_INCOMPLETE_RUNS: u32 = 500;

/// What a recovery pass did.
///
/// Reported rather than logged so the caller can decide how to surface it: a daemon
/// that started and abandoned runs needs to say so, and the count is what it says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoverySummary {
    /// Runs recovered to a terminal failed state.
    pub abandoned: u64,
    /// Runs whose work was gone but which were parked on a dependency.
    pub parked: u64,
}

impl RecoverySummary {
    /// Returns the number of runs this pass changed.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.abandoned + self.parked
    }

    /// Returns whether the pass changed anything.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// A run and its events, for a client stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEventPage {
    /// The events, in ascending sequence order.
    pub events: Vec<StoredActivityEvent>,
    /// The run's client-visible terminal state, when it has reached one.
    ///
    /// Present so a stream that replays an already-finished run knows to deliver
    /// exactly one terminal event and close, rather than waiting for one that was
    /// published before the client connected.
    pub terminal_state: Option<RunState>,
}

/// The largest number of events one page read may return.
///
/// A bounded read rather than the whole stream, because a long run accumulates
/// events and replaying all of them at once would let one reconnect load an
/// unbounded amount into memory.
pub const MAX_EVENT_PAGE: u32 = 500;

/// The largest accepted idempotency key.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;

/// Checks an idempotency key's shape.
///
/// # Errors
///
/// Returns [`RepositoryError::Conflict`] when the key is empty, over
/// [`MAX_IDEMPOTENCY_KEY_BYTES`], or contains a NUL byte, because the key reaches a
/// persisted column and a `UNIQUE` constraint.
pub fn validate_idempotency_key(key: &str) -> Result<(), RepositoryError> {
    if key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES || key.contains('\0') {
        return Err(RepositoryError::Conflict {
            what: "idempotency_key",
        });
    }
    Ok(())
}

/// Builds a run's opening `run.received` event.
///
/// Provided so the first event is constructed identically everywhere: a run's stream
/// must begin at sequence 1 with the received type, and a caller that assembled it by
/// hand could start it at another position or with a type no client expects.
#[must_use]
pub fn run_received_event(run_id: RunId, occurred_at: UtcTimestamp) -> NewActivityEvent {
    NewActivityEvent {
        run_id,
        sequence: 1,
        event_type: "run.received".to_owned(),
        payload_json: None,
        visibility: EventVisibility::Public,
        occurred_at,
    }
}

/// Who may see an activity event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventVisibility {
    /// Any authenticated client of the run's workspace.
    Public,
    /// Only an operator-facing surface.
    Operator,
}

impl EventVisibility {
    /// Returns the stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Operator => "operator",
        }
    }

    /// Parses the stored spelling.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Corrupted`] for an unrecognized value, because a
    /// row whose visibility cannot be interpreted must not be silently treated as
    /// public or skipped.
    pub fn parse(value: &str) -> Result<Self, RepositoryError> {
        match value {
            "public" => Ok(Self::Public),
            "operator" => Ok(Self::Operator),
            _ => Err(RepositoryError::Corrupted {
                column: "visibility",
            }),
        }
    }
}

/// The durable run store.
pub trait RunRepository: Send + Sync {
    /// Inserts a new run in `Received`, appending its opening event atomically.
    ///
    /// The opening event is a parameter rather than appended by the caller because
    /// the local control API requires exactly this: a client that connects before the
    /// run does any work must still be able to replay `run.received`, and the
    /// "persist the transition before publishing an event that claims it occurred"
    /// rule applies to creation as much as to any later transition. Appending it
    /// afterwards would leave a window in which the run exists with no first event.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when the run already exists, when the
    /// opening event's sequence is not 1, or when the event names a different run;
    /// and [`RepositoryError::NotFound`] when the conversation does not exist in
    /// `workspace`.
    fn create(&self, run: NewRun, opening_event: NewActivityEvent) -> RepositoryFuture<'_, ()>;

    /// Loads a run within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for a run that is absent **or** owned
    /// by another workspace, and [`RepositoryError::Corrupted`] when a stored state
    /// cannot be interpreted.
    fn load(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, StoredRun>;

    /// Loads a run together with the dependency it is parked on.
    ///
    /// # Errors
    ///
    /// As [`load`](Self::load).
    fn load_for_resume(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> RepositoryFuture<'_, RunResumeState>;

    /// Applies `write` and appends its activity event in one transaction.
    ///
    /// The event's `run_id` must be the same run. The state write and the event
    /// append commit together, so a reader never observes an event describing a
    /// transition that is not yet durable, or a transition with no event.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign run,
    /// [`RepositoryError::VersionConflict`] when the stored version differs from
    /// the transition's expected version,
    /// [`RepositoryError::TransitionRefused`] when the domain refuses the edge (the
    /// refusal's own code is preserved, so "no such edge" stays distinguishable from
    /// "stale view"), and [`RepositoryError::Conflict`] when the event's sequence is
    /// not the next one or the write's waiting information is inconsistent.
    ///
    /// The `write` borrow carries its own lifetime rather than using the elided
    /// `&self` one, because the returned future borrows both: tying it to `&self`
    /// alone would make the signature unnameable in an implementation.
    fn transition<'a>(
        &'a self,
        workspace: WorkspaceId,
        write: RunWrite<'a>,
    ) -> RepositoryFuture<'a, StoredRun>;

    /// Returns the next unused activity sequence for `run`.
    ///
    /// Exposed so a caller can build a [`NewActivityEvent`] without reading the
    /// whole event stream, and so the "sequence increases by exactly one" rule has a
    /// single source rather than a `max + 1` written at each call site.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign run.
    fn next_event_sequence(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, u64>;

    /// Appends one activity event **without** changing the run's state.
    ///
    /// A separate operation from [`transition`](Self::transition), because not every public event
    /// is a state change. The contract's minimum first-slice list includes `run.usage`, which
    /// reports what a call consumed and leaves the run exactly where it was — and before this the
    /// only way to write an event was to write a transition with it, so an informational event was
    /// **unwritable** and `run.usage` was never published at all.
    ///
    /// The run's `version` is deliberately **not** advanced. A version is the optimistic-concurrency
    /// token a transition states it expects, so bumping it for a report that changed no state would
    /// make two workers' transitions refuse each other for a write neither of them made — the
    /// concurrency control is about state, and a report is not state.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign run, and
    /// [`RepositoryError::Conflict`] when the event's sequence is not the next one — a gap in the
    /// sequence is what a client is required to refuse rather than skip.
    fn append_event(
        &self,
        workspace: WorkspaceId,
        event: NewActivityEvent,
    ) -> RepositoryFuture<'_, u64>;

    /// Reads a page of a run's public activity events in ascending sequence order.
    ///
    /// The read is scoped by workspace **and by visibility**, so an operator-only
    /// event is never returned to an ordinary client. Filtering after the read would
    /// mean the row was already in memory and only the presentation changed, which is
    /// the same shape as a leak that has not happened yet.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign run.
    fn load_events(
        &self,
        workspace: WorkspaceId,
        run: RunId,
        from_sequence: u64,
        limit: u32,
    ) -> RepositoryFuture<'_, RunEventPage>;

    /// Claims an idempotency key for a run-creating command.
    ///
    /// The claim is atomic: two concurrent requests with one key must not both be
    /// told `Claimed`. A repeated key with an identical digest returns
    /// [`IdempotencyClaim::Replay`] carrying the original run, and a repeated key
    /// with a different digest returns [`IdempotencyClaim::Conflict`], which is what
    /// makes a retry safe and a different request under the same key refusable.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when the key's shape is invalid and
    /// [`RepositoryError::Query`] for a driver failure.
    fn claim_idempotency(
        &self,
        record: NewIdempotencyRecord,
    ) -> RepositoryFuture<'_, IdempotencyClaim>;

    /// Reads an idempotency record without writing anything, if one exists.
    ///
    /// Exposed for a caller that must decide whether to do preparatory work before it
    /// can claim — a run needs a conversation, and creating a conversation for a
    /// request that turns out to be a replay would leave an orphan. The read is
    /// advisory only: [`create_run_idempotent`](Self::create_run_idempotent) is what
    /// actually decides, because only it is atomic.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] for a driver failure.
    fn lookup_idempotency(
        &self,
        workspace: WorkspaceId,
        operation: &str,
        key: &str,
    ) -> RepositoryFuture<'_, Option<(String, RunId)>>;

    /// Creates a run, its opening event, and its idempotency record in one write.
    ///
    /// This is the operation the local control API's atomicity rule requires:
    /// "acknowledged mutation state and idempotency records are committed
    /// atomically". Splitting it into a claim followed by a create leaves the gap the
    /// rule exists to close — a key claimed by a command whose run was never written,
    /// or a run written twice because two requests both passed a separate check.
    ///
    /// If the key already exists, **nothing is created** and the result is
    /// [`IdempotencyClaim::Replay`] or [`IdempotencyClaim::Conflict`].
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when the run or conversation already
    /// exists, the opening event is malformed, or the key's shape is invalid; and
    /// [`RepositoryError::Query`] for a driver failure.
    fn create_run_idempotent(
        &self,
        run: NewRun,
        opening_event: NewActivityEvent,
        record: NewIdempotencyRecord,
    ) -> RepositoryFuture<'_, IdempotencyClaim>;

    /// Lists runs that are not in a terminal state, oldest first.
    ///
    /// Every workspace is included, because this is a **startup reconciliation** read
    /// and not a client read: the pass must find every interrupted run in the profile,
    /// and scoping it to one workspace would silently leave the others non-terminal
    /// forever. Each entry carries its own workspace so the recovery write can name the
    /// scope it applies to.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Corrupted`] for a stored state this build cannot
    /// interpret, because a run whose state is unreadable must not be skipped: skipping
    /// it would leave it non-terminal with nobody aware, which is the exact failure this
    /// operation exists to prevent.
    fn incomplete_runs(&self) -> RepositoryFuture<'_, Vec<IncompleteRun>>;
}

#[cfg(test)]
mod tests {
    use super::{
        EventVisibility, MAX_REFERENCE_BYTES, MAX_RUNTIME_ID_BYTES, NATIVE_RUNTIME_ID, NewRun,
        RunRuntime, StoredRun,
    };
    use crate::repository::RepositoryError;
    use jarvis_domain::ids::{ConversationId, PrincipalId, RunId, WorkspaceId};
    use jarvis_domain::run::budget::RunBudget;
    use jarvis_domain::run::state::{RunState, RunVersion};
    use jarvis_domain::time::UtcTimestamp;

    fn id(value: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(value)
    }

    fn run_id() -> RunId {
        RunId::from_uuid(id(1))
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid(id(2))
    }

    fn conversation() -> ConversationId {
        ConversationId::from_uuid(id(3))
    }

    fn principal() -> PrincipalId {
        PrincipalId::from_uuid(id(4))
    }

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn new_run(objective_ref: Option<&str>) -> Result<NewRun, RepositoryError> {
        NewRun::new(
            run_id(),
            workspace(),
            conversation(),
            principal(),
            objective_ref.map(ToOwned::to_owned),
            now(),
        )
    }

    #[test]
    fn a_new_run_is_scoped_and_carries_its_identity() {
        let run = new_run(Some("objective-1")).expect("valid");
        assert_eq!(run.workspace_id, workspace());
        assert_eq!(run.conversation_id, conversation());
        assert_eq!(run.principal_id, principal());
        assert_eq!(run.objective_ref.as_deref(), Some("objective-1"));
    }

    #[test]
    fn an_objective_reference_is_bounded_and_non_empty() {
        assert!(new_run(None).is_ok(), "an absent reference is valid");
        assert!(new_run(Some("ok")).is_ok());
        let over = "a".repeat(MAX_REFERENCE_BYTES + 1);
        assert!(
            new_run(Some(&over)).is_err(),
            "over the bound must be refused"
        );
        assert!(
            new_run(Some(&"a".repeat(MAX_REFERENCE_BYTES))).is_ok(),
            "exactly the bound is inside it",
        );
        assert!(new_run(Some("")).is_err(), "empty must be refused");
        assert!(new_run(Some("a\0b")).is_err(), "a NUL must be refused");
    }

    #[test]
    fn a_new_run_records_the_native_runtime_without_being_told() {
        // The point is that the caller does **not** pass a runtime: the constructor defaults to
        // one, so a run cannot be created in a state where the schema's runtime columns stay
        // `NULL`. Asserting the value rather than "some runtime" is what makes this falsifiable —
        // `RunRuntime::default()` or an empty pair would satisfy a looser assertion.
        let run = new_run(Some("objective-1")).expect("valid");
        assert_eq!(run.runtime.id, NATIVE_RUNTIME_ID);
        assert_eq!(run.runtime.version, env!("CARGO_PKG_VERSION"));
        assert!(!run.runtime.version.is_empty());
    }

    #[test]
    fn a_run_runtime_can_be_overridden_for_a_non_native_build() {
        // The default must be an *overridable* default rather than a hardcoded literal, or a build
        // that serves more than the native runtime cannot record what it served.
        let override_runtime = RunRuntime::new("external-runtime", "9.9.9").expect("valid");
        let run = new_run(Some("objective-1"))
            .expect("valid")
            .with_runtime(override_runtime.clone());
        assert_eq!(run.runtime, override_runtime);
        assert_ne!(run.runtime.id, NATIVE_RUNTIME_ID);
    }

    #[test]
    fn a_runtime_identity_is_bounded_and_non_empty_on_both_fields() {
        assert!(RunRuntime::new("jarvis-native", "0.1.0").is_ok());
        assert!(
            RunRuntime::new(&"a".repeat(MAX_RUNTIME_ID_BYTES), "0.1.0").is_ok(),
            "exactly the bound is inside it",
        );

        // Both fields reach a persisted column, so both are validated. Checking only `id` would
        // let an empty version through, and an empty version is the value that made "no runtime
        // recorded" indistinguishable from "a runtime was recorded" in the first place.
        for (id, version) in [
            ("", "0.1.0"),
            ("jarvis-native", ""),
            (&"a".repeat(MAX_RUNTIME_ID_BYTES + 1), "0.1.0"),
            ("jarvis-native", &"1".repeat(MAX_RUNTIME_ID_BYTES + 1)),
            ("jarvis\0native", "0.1.0"),
            ("jarvis-native", "0.1\0.0"),
        ] {
            let error = RunRuntime::new(id, version)
                .expect_err("an unusable runtime identity must be refused");
            assert_eq!(
                error,
                RepositoryError::Conflict {
                    what: "run_runtime_identity"
                }
            );
        }
    }

    #[test]
    fn a_stored_run_reports_terminality_from_its_state() {
        let run = StoredRun {
            id: run_id(),
            workspace_id: workspace(),
            conversation_id: conversation(),
            principal_id: principal(),
            state: RunState::Completed,
            version: RunVersion::new(9),
            objective_ref: None,
            created_at: now(),
            started_at: Some(now()),
            updated_at: now(),
            completed_at: Some(now()),
            error_code: None,
            deadline_at: None,
            runtime: Some(RunRuntime::native()),
            budget: RunBudget::default(),
        };
        assert!(run.is_terminal());
    }

    #[test]
    fn event_visibility_round_trips_and_refuses_an_unknown_value() {
        for visibility in [EventVisibility::Public, EventVisibility::Operator] {
            assert_eq!(
                EventVisibility::parse(visibility.as_str()).expect("round-trips"),
                visibility,
            );
        }
        // An unrecognized visibility must not be silently treated as public: that
        // is the direction of error that would leak operator detail to a client.
        let error = EventVisibility::parse("secret").expect_err("unknown must be refused");
        assert_eq!(error.code(), "storage.row_corrupted");
    }
}
