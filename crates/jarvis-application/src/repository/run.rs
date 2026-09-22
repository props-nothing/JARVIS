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
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::state::{RunState, RunVersion};
use jarvis_domain::time::UtcTimestamp;

/// The largest accepted objective reference.
///
/// A reference, not the objective text: large content lives in artifacts with
/// relational metadata (see the schema document), so an unbounded string here
/// would be the one place large content crept back into a row.
pub const MAX_REFERENCE_BYTES: usize = 512;

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
}

impl NewRun {
    /// Builds a run creation request.
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

/// One atomic run write: a transition, the dependency it enters or leaves, and the
/// activity event that describes it.
///
/// Bundled because `docs/architecture/storage-data.md` defines "transition run state
/// and append its durable activity event" as **one** use case. Passing the three
/// parts separately would let a caller move the state without its event, which is
/// the exact window the atomicity requirement exists to close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWrite<'a> {
    /// The transition to apply. Its edge legality is checked by the repository,
    /// which delegates to the domain's transition table.
    pub transition: &'a RunTransition,
    /// The dependency being entered, required when the target state is a waiting
    /// state and refused when it is not.
    pub waiting: Option<WaitingOn>,
    /// The activity event describing the transition.
    pub event: NewActivityEvent,
}

impl<'a> RunWrite<'a> {
    /// Builds a write that leaves the run in a non-waiting state.
    #[must_use]
    pub fn new(transition: &'a RunTransition, event: NewActivityEvent) -> Self {
        Self {
            transition,
            waiting: None,
            event,
        }
    }

    /// Attaches the dependency the run is entering.
    #[must_use]
    pub fn waiting_on(mut self, waiting: WaitingOn) -> Self {
        self.waiting = Some(waiting);
        self
    }

    /// Returns whether this write is internally consistent.
    ///
    /// A waiting target must name a dependency and a non-waiting target must not,
    /// which is the same rule the schema's `CHECK` enforces. Checking it here means
    /// the mismatch is a typed refusal rather than a constraint failure the caller
    /// has to decode.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.transition.to.is_waiting() == self.waiting.is_some()
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
}

#[cfg(test)]
mod tests {
    use super::{EventVisibility, MAX_REFERENCE_BYTES, NewRun, StoredRun};
    use crate::repository::RepositoryError;
    use jarvis_domain::ids::{ConversationId, PrincipalId, RunId, WorkspaceId};
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
