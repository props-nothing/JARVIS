//! Contract tests for the SQLite repository adapters.
//!
//! These are behaviour tests against a **real migrated SQLite database**, not
//! against a mock, because the properties that matter here are the database's own:
//! an optimistic `UPDATE ... WHERE version = ?`, a transaction that commits the
//! state and its event together, a foreign key that refuses an orphan, and a
//! `UNIQUE` constraint that makes a retry a new attempt. A fake would agree with
//! whatever the code assumed about them.
//!
//! The suite is deliberately weighted toward refusal paths. "The row was written"
//! is the easy half; the value is in proving that a stale version, a duplicate
//! attempt, a foreign workspace, and a terminal run are all *refused*, and that a
//! refusal changed nothing.

use super::SqliteRepositories;
use jarvis_application::repository::RepositoryError;
use jarvis_application::repository::conversation::{
    ConversationRepository as _, NewConversation, NewMessage,
};
use jarvis_application::repository::model_call::{
    ModelCallOutcome, ModelCallRepository as _, ModelCallState, NewModelCall,
};
use jarvis_application::repository::run::{
    EventVisibility, NewActivityEvent, NewRun, RunRepository as _, RunWrite, WaitingOn,
};
use jarvis_domain::ids::{ConversationId, MessageId, ModelCallId, PrincipalId, RunId, WorkspaceId};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::Role;
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
use jarvis_domain::time::UtcTimestamp;
use uuid::Uuid;

use crate::storage::connection::Database;
use crate::storage::migrate;

/// A migrated in-memory database with repositories over it.
async fn repository() -> (Database, SqliteRepositories) {
    let database = Database::open_in_memory().await.expect("in-memory opens");
    migrate::run(database.pool()).await.expect("migrates");
    let repositories = SqliteRepositories::new(database.pool().clone());
    (database, repositories)
}

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(id(1))
}

/// A second workspace, for isolation tests.
fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(id(2))
}

fn conversation_id() -> ConversationId {
    ConversationId::from_uuid(id(3))
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(id(4))
}

fn run_id() -> RunId {
    RunId::from_uuid(id(5))
}

fn other_run_id() -> RunId {
    RunId::from_uuid(id(6))
}

fn model_call_id() -> ModelCallId {
    ModelCallId::from_uuid(id(7))
}

fn logical_call_id() -> ModelCallId {
    ModelCallId::from_uuid(id(8))
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
}

fn reason(text: &str) -> TransitionReason {
    TransitionReason::new(text).expect("valid")
}

fn model() -> ModelRef {
    ModelRef::new(
        ProviderId::parse("scripted.local").expect("valid"),
        ModelId::parse("fixture-1").expect("valid"),
    )
}

/// Creates the conversation and run the run tests need.
async fn seed(repositories: &SqliteRepositories) {
    repositories
        .create_conversation(
            NewConversation::new(
                conversation_id(),
                workspace(),
                principal(),
                Some("first".to_owned()),
                "cli".to_owned(),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("the conversation is created");
    repositories
        .create(
            NewRun::new(
                run_id(),
                workspace(),
                conversation_id(),
                principal(),
                Some("objective-1".to_owned()),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("the run is created");
}

/// Builds an event at `sequence` for `run`.
fn event(run: RunId, sequence: u64, event_type: &str) -> NewActivityEvent {
    NewActivityEvent {
        run_id: run,
        sequence,
        event_type: event_type.to_owned(),
        payload_json: None,
        visibility: EventVisibility::Public,
        occurred_at: now(),
    }
}

/// Builds a transition from the run's current state.
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

/// Builds a message at `sequence`'s id offset with the given content.
fn message(id_value: u128, content: &str, role: Role) -> NewMessage {
    NewMessage {
        id: MessageId::from_uuid(id(id_value)),
        conversation_id: conversation_id(),
        role,
        content: content.to_owned(),
        content_schema_version: 1,
        sensitivity: "internal".to_owned(),
        source: "cli".to_owned(),
        created_at: now(),
    }
    .validated()
    .expect("the fixture is valid")
}

/// Builds a model-call attempt with the given row id, attempt number, and call id.
fn attempt(id_value: u128, attempt_number: u32, logical: ModelCallId) -> NewModelCall {
    NewModelCall {
        id: ModelCallId::from_uuid(id(id_value)),
        workspace_id: workspace(),
        run_id: run_id(),
        logical_call_id: logical,
        attempt: attempt_number,
        model: model(),
        request_fingerprint: None,
        started_at: now(),
    }
    .validated()
    .expect("the fixture is valid")
}

/// Builds an outcome in `state`, completed at `completed_at` when given.
fn outcome(state: ModelCallState, completed_at: Option<UtcTimestamp>) -> ModelCallOutcome {
    ModelCallOutcome {
        state,
        provider_request_id: None,
        continuation_ref: None,
        usage: None,
        estimated_cost_microunits: None,
        finish_reason: None,
        error_code: None,
        first_output_at: None,
        completed_at,
    }
    .validated()
    .expect("the fixture is valid")
}

#[tokio::test]
async fn a_created_run_loads_in_received_at_version_one() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("the run loads");
    assert_eq!(run.state, RunState::Received);
    assert_eq!(run.version, RunVersion::FIRST);
    assert_eq!(run.conversation_id, conversation_id());
    assert_eq!(run.objective_ref.as_deref(), Some("objective-1"));
    assert!(!run.is_terminal());
    assert_eq!(run.completed_at, None);
}

#[tokio::test]
async fn a_run_in_another_workspace_is_indistinguishable_from_a_missing_one() {
    // The local control API requires a foreign run to be indistinguishable from a
    // missing one, so both must produce the same error rather than a forbidden
    // result that would confirm the run exists.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let missing = repositories
        .load(workspace(), other_run_id())
        .await
        .expect_err("an absent run must be refused");
    let foreign = repositories
        .load(other_workspace(), run_id())
        .await
        .expect_err("a foreign run must be refused");
    assert_eq!(missing, RepositoryError::NotFound);
    assert_eq!(foreign, RepositoryError::NotFound);
    assert_eq!(missing.code(), foreign.code());
}

#[tokio::test]
async fn a_duplicate_run_is_refused_rather_than_silently_created_twice() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let duplicate = NewRun::new(
        run_id(),
        workspace(),
        conversation_id(),
        principal(),
        None,
        now(),
    )
    .expect("valid");
    let error = repositories
        .create(duplicate)
        .await
        .expect_err("a duplicate run must be refused");
    assert_eq!(error.code(), "storage.conflict");
}

#[tokio::test]
async fn a_transition_persists_the_state_and_its_event_together() {
    // The storage architecture names this the first required atomic use case. The
    // assertion is that *both* landed, which a two-statement implementation would
    // pass only by luck.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let sequence = repositories
        .next_event_sequence(workspace(), run_id())
        .await
        .expect("the sequence is readable");
    assert_eq!(sequence, 1, "a fresh run's first event is sequence 1");

    let updated = repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(
                    RunState::Received,
                    RunState::ContextBuilding,
                    RunVersion::FIRST,
                ),
                event(run_id(), sequence, "run.context_building"),
            ),
        )
        .await
        .expect("the transition is applied");

    assert_eq!(updated.state, RunState::ContextBuilding);
    assert_eq!(updated.version.get(), 2);

    // The event is durable in the same call: the next sequence reflects it.
    assert_eq!(
        repositories
            .next_event_sequence(workspace(), run_id())
            .await
            .expect("readable"),
        2,
    );
}

#[tokio::test]
async fn a_stale_version_is_refused_and_nothing_changes() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(
                    RunState::Received,
                    RunState::ContextBuilding,
                    RunVersion::FIRST,
                ),
                event(run_id(), 1, "run.context_building"),
            ),
        )
        .await
        .expect("the first transition applies");

    // A second writer still holding version 1.
    let stale = transition(
        RunState::Received,
        RunState::ContextBuilding,
        RunVersion::FIRST,
    );
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(&stale, event(run_id(), 2, "run.context_building")),
        )
        .await
        .expect_err("a stale write must be refused");
    assert_eq!(error.code(), "storage.version_conflict");

    // The refusal is total: the state did not move and no event was appended.
    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("loads");
    assert_eq!(run.state, RunState::ContextBuilding);
    assert_eq!(run.version.get(), 2);
    assert_eq!(
        repositories
            .next_event_sequence(workspace(), run_id())
            .await
            .expect("readable"),
        2,
        "the refused transition must not have appended an event",
    );
}

#[tokio::test]
async fn an_edge_the_diagram_lacks_is_refused() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    // `Received -> Responding` is not an edge. The `state = ?` predicate cannot
    // catch this on its own — it only proves the run was in the expected state —
    // so the adapter asks the domain's transition table. An earlier version of this
    // method inferred the edge from the predicate and **accepted** this transition,
    // which is exactly the defect this test exists to keep closed.
    let illegal = transition(RunState::Received, RunState::Responding, RunVersion::FIRST);
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(&illegal, event(run_id(), 1, "run.responding")),
        )
        .await
        .expect_err("an invented edge must be refused");
    assert_eq!(error.code(), "storage.transition_refused");

    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("loads");
    assert_eq!(
        run.state,
        RunState::Received,
        "the state must not have moved"
    );
    assert_eq!(run.version, RunVersion::FIRST);
}

#[tokio::test]
async fn a_version_conflict_is_reported_before_an_illegal_edge() {
    // The ordering the domain establishes is preserved through the adapter: a stale
    // caller is told its view is stale, not that its edge is illegal, because the
    // edge may be legal from the current state.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(
                    RunState::Received,
                    RunState::ContextBuilding,
                    RunVersion::FIRST,
                ),
                event(run_id(), 1, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    // Both stale and illegal from the caller's point of view.
    let stale_and_illegal = transition(RunState::Received, RunState::Responding, RunVersion::FIRST);
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(&stale_and_illegal, event(run_id(), 2, "run.responding")),
        )
        .await
        .expect_err("must be refused");
    assert_eq!(
        error.code(),
        "storage.version_conflict",
        "the stale view must be reported first",
    );
}

#[tokio::test]
async fn reaching_a_terminal_state_records_the_completion_instant() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    // Walk to a terminal state through legal edges.
    let path = [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::Responding),
        (RunState::Responding, RunState::Completed),
    ];
    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = transition(*from, *to, version);
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 1, "run.step"));
        let updated = repositories
            .transition(workspace(), write)
            .await
            .expect("the step is legal");
        version = updated.version;
    }

    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("loads");
    assert_eq!(run.state, RunState::Completed);
    assert!(run.is_terminal());
    assert_eq!(
        run.completed_at,
        Some(now()),
        "a terminal run must record when it finished",
    );
}

#[tokio::test]
async fn a_terminal_run_refuses_every_later_transition() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    // Cancellation is legal from `AwaitingModel`, so walk a legal path there first.
    let path = [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::AwaitingModel),
    ];
    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = transition(*from, *to, version);
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 1, "run.step"));
        version = repositories
            .transition(workspace(), write)
            .await
            .expect("legal")
            .version;
    }

    let cancel = transition(RunState::AwaitingModel, RunState::Cancelled, version);
    let cancelled = repositories
        .transition(
            workspace(),
            RunWrite::new(&cancel, event(run_id(), 4, "run.cancelled")),
        )
        .await
        .expect("cancellation from awaiting_model is legal");
    assert!(cancelled.is_terminal());
    assert_eq!(cancelled.state, RunState::Cancelled);

    // Any later write is refused with the terminal reason, not a version reason, so
    // the caller is not sent to debug concurrency.
    let late = transition(RunState::Cancelled, RunState::Planning, cancelled.version);
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(&late, event(run_id(), 5, "run.planning")),
        )
        .await
        .expect_err("a terminal run must absorb");
    assert_eq!(error.code(), "storage.transition_refused");
}

#[tokio::test]
async fn a_waiting_state_records_its_dependency_and_clears_it_on_advance() {
    // The schema's constraint requires a waiting run to name its dependency, so a
    // waiting row with nothing to wait for must be unrepresentable rather than
    // merely invalid.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let path = [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::AwaitingModel),
        (RunState::AwaitingModel, RunState::ExecutingTool),
        (RunState::ExecutingTool, RunState::Observing),
    ];
    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = transition(*from, *to, version);
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 1, "run.step"));
        version = repositories
            .transition(workspace(), write)
            .await
            .expect("legal")
            .version;
    }

    // Entering `Waiting` without a dependency must be refused by the write's own
    // consistency check rather than by the schema constraint.
    let into_waiting = transition(RunState::Observing, RunState::Waiting, version);
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(&into_waiting, event(run_id(), 6, "run.waiting")),
        )
        .await
        .expect_err("a waiting state without a dependency must be refused");
    assert_eq!(error.code(), "storage.conflict");
    assert_eq!(
        repositories
            .load(workspace(), run_id())
            .await
            .expect("loads")
            .state,
        RunState::Observing,
        "the refused write must not have moved the state",
    );

    // With a dependency it is accepted and readable through the resume read.
    let waiting = WaitingOn::new("timer", "wake-1").expect("valid");
    let write = RunWrite::new(&into_waiting, event(run_id(), 6, "run.waiting")).waiting_on(waiting);
    repositories
        .transition(workspace(), write)
        .await
        .expect("a waiting transition with a dependency is legal");

    let resume = repositories
        .load_for_resume(workspace(), run_id())
        .await
        .expect("loads for resume");
    assert_eq!(resume.run.state, RunState::Waiting);
    assert_eq!(resume.waiting_kind.as_deref(), Some("timer"));
    assert_eq!(resume.waiting_ref.as_deref(), Some("wake-1"));

    // Advancing out of the waiting state clears the dependency, so a live run never
    // reads as parked on something it has already resumed from.
    let version = resume.run.version;
    let resumed = transition(RunState::Waiting, RunState::Planning, version);
    repositories
        .transition(
            workspace(),
            RunWrite::new(&resumed, event(run_id(), 7, "run.planning")),
        )
        .await
        .expect("resuming is legal");

    let resume = repositories
        .load_for_resume(workspace(), run_id())
        .await
        .expect("loads for resume");
    assert_eq!(resume.run.state, RunState::Planning);
    assert_eq!(resume.waiting_kind, None, "the dependency must be cleared");
    assert_eq!(resume.waiting_ref, None);
}

#[tokio::test]
async fn a_non_waiting_state_carrying_a_dependency_is_refused() {
    // The other direction of the same rule: a progressing run that still names a
    // dependency would misreport what it is doing.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let onward = transition(
        RunState::Received,
        RunState::ContextBuilding,
        RunVersion::FIRST,
    );
    let waiting = WaitingOn::new("timer", "wake-1").expect("valid");
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(&onward, event(run_id(), 1, "run.context_building")).waiting_on(waiting),
        )
        .await
        .expect_err("a dependency on a non-waiting state must be refused");
    assert_eq!(error.code(), "storage.conflict");
    assert_eq!(
        repositories
            .load(workspace(), run_id())
            .await
            .expect("loads")
            .state,
        RunState::Received,
    );
}

#[tokio::test]
async fn a_duplicate_event_sequence_is_refused() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(
                    RunState::Received,
                    RunState::ContextBuilding,
                    RunVersion::FIRST,
                ),
                event(run_id(), 1, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    // Append at the same position the first event occupies. The UNIQUE(run_id,
    // sequence) constraint is what keeps "sequence increases by exactly one" true.
    let next = repositories
        .load(workspace(), run_id())
        .await
        .expect("loads")
        .version;
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(RunState::ContextBuilding, RunState::Planning, next),
                event(run_id(), 1, "run.planning"),
            ),
        )
        .await
        .expect_err("a reused sequence must be refused");
    assert_eq!(error.code(), "storage.conflict");

    // And the state did not move with it.
    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("loads");
    assert_eq!(run.state, RunState::ContextBuilding);
}

#[tokio::test]
async fn next_event_sequence_refuses_an_absent_run_rather_than_answering_one() {
    // A plausible-looking sequence 1 for a run that does not exist is worse than an
    // error: a caller would use it and write an orphan event.
    let (_database, repositories) = repository().await;
    let error = repositories
        .next_event_sequence(workspace(), run_id())
        .await
        .expect_err("an absent run must be refused");
    assert_eq!(error, RepositoryError::NotFound);
}

#[tokio::test]
async fn a_message_appends_at_increasing_positions() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let first = repositories
        .append_message(workspace(), message(10, "hello", Role::User))
        .await
        .expect("appends");
    let second = repositories
        .append_message(workspace(), message(11, "hi", Role::Assistant))
        .await
        .expect("appends");
    assert_eq!(first, 1);
    assert_eq!(second, 2, "the position must advance by one");
}

#[tokio::test]
async fn messages_load_in_order_and_are_bounded() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    for index in 0..5_u128 {
        repositories
            .append_message(
                workspace(),
                message(100 + index, &format!("message {index}"), Role::User),
            )
            .await
            .expect("appends");
    }

    let all = repositories
        .load_messages(workspace(), conversation_id(), None, 100)
        .await
        .expect("loads");
    assert_eq!(all.len(), 5);
    assert_eq!(all[0].sequence, 1);
    assert_eq!(all[4].sequence, 5);
    assert_eq!(all[0].content, "message 0");

    // A bounded page, and a resume strictly after a position.
    let page = repositories
        .load_messages(workspace(), conversation_id(), Some(2), 2)
        .await
        .expect("loads");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].sequence, 3);
    assert_eq!(page[1].sequence, 4);
}

#[tokio::test]
async fn messages_for_a_foreign_conversation_are_refused_not_empty() {
    // An empty page for a foreign conversation would read as "no messages", which
    // is a different and misleading fact from "you may not read this".
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let error = repositories
        .load_messages(other_workspace(), conversation_id(), None, 10)
        .await
        .expect_err("a foreign conversation must be refused");
    assert_eq!(error, RepositoryError::NotFound);
}

#[tokio::test]
async fn a_conversation_loads_with_its_archive_state() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let conversation = repositories
        .load_conversation(workspace(), conversation_id())
        .await
        .expect("loads");
    assert!(!conversation.archived, "a new conversation is active");
    assert_eq!(conversation.title.as_deref(), Some("first"));
}

#[tokio::test]
async fn a_model_call_attempt_records_and_loads() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    repositories
        .record_attempt(attempt(7, 1, logical_call_id()))
        .await
        .expect("records");

    let call = repositories
        .load_attempt(workspace(), model_call_id())
        .await
        .expect("loads");
    assert_eq!(call.attempt, 1);
    assert_eq!(call.state, ModelCallState::Pending);
    assert_eq!(call.logical_call_id, logical_call_id());
    assert_eq!(call.model_id.to_string(), "fixture-1");

    let mut completed = outcome(ModelCallState::Completed, Some(now()));
    completed.provider_request_id = Some("provider-request-1".to_owned());
    repositories
        .record_outcome(workspace(), model_call_id(), completed)
        .await
        .expect("records the outcome");

    let call = repositories
        .load_attempt(workspace(), model_call_id())
        .await
        .expect("loads");
    assert_eq!(call.state, ModelCallState::Completed);
    assert_eq!(
        call.provider_request_id.as_deref(),
        Some("provider-request-1")
    );
}

#[tokio::test]
async fn a_retry_is_a_new_attempt_of_the_same_logical_call() {
    // The distinction the retry-ownership rule depends on: retrying reuses the
    // logical identity and gets a new attempt number, and the first attempt's
    // recorded outcome is never overwritten.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    repositories
        .record_attempt(attempt(7, 1, logical_call_id()))
        .await
        .expect("records the first attempt");

    // The same attempt number again is refused, not merged.
    let duplicate = attempt(77, 1, logical_call_id());
    let error = repositories
        .record_attempt(duplicate)
        .await
        .expect_err("a reused attempt number must be refused");
    assert_eq!(error.code(), "storage.conflict");

    // A second attempt of the same logical call is accepted.
    repositories
        .record_attempt(attempt(78, 2, logical_call_id()))
        .await
        .expect("a new attempt number is legal");

    let attempts = repositories
        .load_attempts(workspace(), logical_call_id())
        .await
        .expect("loads the chain");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].attempt, 1, "attempts must load in order");
    assert_eq!(attempts[1].attempt, 2);
}

#[tokio::test]
async fn a_recorded_outcome_cannot_be_rewritten_by_a_late_writer() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    repositories
        .record_attempt(attempt(7, 1, logical_call_id()))
        .await
        .expect("records");

    let completed = outcome(ModelCallState::Completed, Some(now()));
    repositories
        .record_outcome(workspace(), model_call_id(), completed)
        .await
        .expect("records the outcome");

    // A late writer reporting a different outcome must be refused: the recorded
    // result is the record.
    let mut late = outcome(ModelCallState::Failed, Some(now()));
    late.error_code = Some("model.provider_unavailable".to_owned());
    let error = repositories
        .record_outcome(workspace(), model_call_id(), late)
        .await
        .expect_err("a terminal outcome must not be rewritten");
    assert_eq!(error.code(), "storage.version_conflict");

    let call = repositories
        .load_attempt(workspace(), model_call_id())
        .await
        .expect("loads");
    assert_eq!(
        call.state,
        ModelCallState::Completed,
        "the first recorded outcome must survive",
    );
}

#[tokio::test]
async fn an_orphaned_run_is_refused_by_the_foreign_key() {
    // A run in a conversation that does not exist is refused by the schema rather
    // than stored as a dangling reference.
    let (_database, repositories) = repository().await;
    let orphan = NewRun::new(
        run_id(),
        workspace(),
        conversation_id(),
        principal(),
        None,
        now(),
    )
    .expect("valid");
    let error = repositories
        .create(orphan)
        .await
        .expect_err("a run without its conversation must be refused");
    assert!(!error.retryable());
}

#[tokio::test]
async fn an_uninterpretable_stored_state_is_corruption_not_absence() {
    // Writing an unknown state directly is the only way to produce this, and it
    // stands in for a migration problem. The read must report corruption rather
    // than `NotFound`, because treating it as absent would turn a schema problem
    // into apparent data loss.
    let (database, repositories) = repository().await;
    seed(&repositories).await;

    // The CHECK constraint would refuse this through the normal path, so the row is
    // written with the constraint temporarily disabled — the same technique the
    // integrity-check test uses.
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(database.pool())
        .await
        .expect("the pragma is settable");
    sqlx::query("UPDATE agent_runs SET state = 'invented_state' WHERE id = ?")
        .bind(run_id().to_string())
        .execute(database.pool())
        .await
        .expect("the row is rewritten");
    sqlx::query("PRAGMA ignore_check_constraints = OFF")
        .execute(database.pool())
        .await
        .expect("the pragma is reset");

    let error = repositories
        .load(workspace(), run_id())
        .await
        .expect_err("an uninterpretable state must be reported");
    assert_eq!(error.code(), "storage.row_corrupted");
}

#[tokio::test]
async fn the_transition_and_event_survive_a_reopen() {
    // Durability rather than in-process visibility: the state and its event are
    // read back through a fresh pool over the same file.
    let directory = std::env::temp_dir().join(format!("jarvis-repo-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&directory).expect("temp dir");
    let path = directory.join("jarvis.sqlite");

    {
        let database = Database::open(&path).await.expect("opens");
        migrate::run(database.pool()).await.expect("migrates");
        let repositories = SqliteRepositories::new(database.pool().clone());
        seed(&repositories).await;
        repositories
            .transition(
                workspace(),
                RunWrite::new(
                    &transition(
                        RunState::Received,
                        RunState::ContextBuilding,
                        RunVersion::FIRST,
                    ),
                    event(run_id(), 1, "run.context_building"),
                ),
            )
            .await
            .expect("applies");
        database.close().await;
    }

    let database = Database::open(&path).await.expect("reopens");
    let repositories = SqliteRepositories::new(database.pool().clone());
    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("the run survives the reopen");
    assert_eq!(run.state, RunState::ContextBuilding);
    assert_eq!(run.version.get(), 2);
    assert_eq!(
        repositories
            .next_event_sequence(workspace(), run_id())
            .await
            .expect("readable"),
        2,
        "the activity event must have survived too",
    );
    database.close().await;
    let _ = std::fs::remove_dir_all(&directory);
}
