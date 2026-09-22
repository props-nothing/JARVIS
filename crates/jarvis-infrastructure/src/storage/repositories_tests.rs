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
    EventVisibility, IdempotencyClaim, NewActivityEvent, NewIdempotencyRecord, NewRun,
    RunRepository as _, RunWrite, WaitingOn, run_received_event,
};
use jarvis_domain::ids::{ConversationId, MessageId, ModelCallId, PrincipalId, RunId, WorkspaceId};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::Role;
use jarvis_domain::run::budget::RunBudget;
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

/// A second conversation, for the second workspace's run.
fn other_conversation_id() -> ConversationId {
    ConversationId::from_uuid(id(30))
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
    seed_conversation_only(repositories).await;
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
            run_received_event(run_id(), now()),
        )
        .await
        .expect("the run is created");
}

/// Creates only the conversation, for a test that wants to create the run itself.
///
/// Separate from [`seed`] because a test about a run's *creation* needs to control what
/// the run is created with, and one that has already inserted a run cannot insert a
/// second with the same identity.
async fn seed_conversation_only(repositories: &SqliteRepositories) {
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
        .create(duplicate, run_received_event(run_id(), now()))
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
    assert_eq!(
        sequence, 2,
        "creation owns sequence 1, so the first transition is 2",
    );

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
        3,
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
                event(run_id(), 2, "run.context_building"),
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
            RunWrite::new(&stale, event(run_id(), 3, "run.context_building")),
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
        3,
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
            RunWrite::new(&illegal, event(run_id(), 2, "run.responding")),
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
                event(run_id(), 2, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    // Both stale and illegal from the caller's point of view.
    let stale_and_illegal = transition(RunState::Received, RunState::Responding, RunVersion::FIRST);
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(&stale_and_illegal, event(run_id(), 3, "run.responding")),
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
async fn a_plain_question_and_answer_run_persists_through_to_completed() {
    // The product's core loop, end to end through the repository: a text question is
    // asked, a model answers, and the run completes. This is the case the earlier
    // happy-path test missed — it drove `Planning -> Responding` and so never
    // exercised `AwaitingModel` at all, which is why a missing `AwaitingModel ->
    // Responding` edge went unnoticed until the state machine was read against the
    // architecture's "ask a model for either a final response or typed tool intent".
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let path = [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::AwaitingModel),
        (RunState::AwaitingModel, RunState::Responding),
        (RunState::Responding, RunState::Completed),
    ];
    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = transition(*from, *to, version);
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 2, "run.step"));
        version = repositories
            .transition(workspace(), write)
            .await
            .expect("every step of the plain answer path is legal")
            .version;
    }

    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("loads");
    assert_eq!(run.state, RunState::Completed);
    assert_eq!(run.completed_at, Some(now()));
}

#[tokio::test]
async fn a_failure_while_responding_is_persisted_as_failed_not_completed() {
    // `ACC-012`: a disconnect mid-run "never becomes false `Completed`". With the
    // corrected edges this is expressible, and the persisted state proves it.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let path = [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::AwaitingModel),
        (RunState::AwaitingModel, RunState::Responding),
    ];
    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = transition(*from, *to, version);
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 2, "run.step"));
        version = repositories
            .transition(workspace(), write)
            .await
            .expect("legal")
            .version;
    }

    let failed = transition(RunState::Responding, RunState::Failed, version);
    repositories
        .transition(
            workspace(),
            RunWrite::new(&failed, event(run_id(), 6, "run.failed")),
        )
        .await
        .expect("a failure while responding must be legal");

    let run = repositories
        .load(workspace(), run_id())
        .await
        .expect("loads");
    assert_eq!(run.state, RunState::Failed);
    assert!(run.is_terminal());
    assert_ne!(
        run.state,
        RunState::Completed,
        "a failed answer must never read as completed",
    );
}

#[tokio::test]
async fn reaching_a_terminal_state_records_the_completion_instant() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    // The plain answer path, which is the one a client actually drives.
    let path = [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::AwaitingModel),
        (RunState::AwaitingModel, RunState::Responding),
        (RunState::Responding, RunState::Completed),
    ];
    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = transition(*from, *to, version);
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 2, "run.step"));
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
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 2, "run.step"));
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
            RunWrite::new(&cancel, event(run_id(), 5, "run.cancelled")),
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
            RunWrite::new(&late, event(run_id(), 6, "run.planning")),
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
        let write = RunWrite::new(&transition, event(run_id(), index as u64 + 2, "run.step"));
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
            RunWrite::new(&into_waiting, event(run_id(), 7, "run.waiting")),
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
    let write = RunWrite::new(&into_waiting, event(run_id(), 7, "run.waiting")).waiting_on(waiting);
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
            RunWrite::new(&resumed, event(run_id(), 8, "run.planning")),
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
            RunWrite::new(&onward, event(run_id(), 2, "run.context_building")).waiting_on(waiting),
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
                event(run_id(), 2, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    // Append at the same position the first *transition* occupies. The UNIQUE(run_id,
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
                event(run_id(), 2, "run.planning"),
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
        .create(orphan, run_received_event(run_id(), now()))
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
                    event(run_id(), 2, "run.context_building"),
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
        3,
        "both the opening and the transition event must have survived",
    );
    database.close().await;
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn a_public_event_page_excludes_operator_events() {
    // The visibility filter is a query predicate, not a presentation choice. If it
    // were applied after the read, the operator row would already have been loaded on
    // a client-facing path, which is a leak that has not happened yet rather than a
    // non-leak.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let mut hidden = event(run_id(), 2, "run.operator_note");
    hidden.visibility = EventVisibility::Operator;
    // Written directly, because a transition only ever appends a public event and
    // the point of this test is that a stored operator row stays unreadable.
    sqlx::query(
        "INSERT INTO run_activity_events (\
             id, workspace_id, run_id, sequence, event_type, payload_json, visibility, occurred_at\
         ) VALUES (?, ?, ?, ?, ?, NULL, 'operator', ?)",
    )
    .bind(Uuid::now_v7().to_string())
    .bind(workspace().to_string())
    .bind(run_id().to_string())
    .bind(2_i64)
    .bind("run.operator_note")
    .bind(now().to_string())
    .execute(repositories.pool())
    .await
    .expect("the operator row is written");

    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(
                    RunState::Received,
                    RunState::ContextBuilding,
                    RunVersion::FIRST,
                ),
                event(run_id(), 3, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    let page = repositories
        .load_events(workspace(), run_id(), 1, 100)
        .await
        .expect("readable");
    // The opening event and the transition are returned; the operator row is not.
    assert_eq!(page.events.len(), 2, "{:?}", page.events);
    assert_eq!(page.events[0].event_type, "run.received");
    assert_eq!(page.events[1].event_type, "run.context_building");
    assert!(
        !page
            .events
            .iter()
            .any(|event| event.event_type == "run.operator_note"),
        "an operator-only event must never reach a client page",
    );
}

#[tokio::test]
async fn a_terminal_run_reports_its_terminal_state_to_a_late_stream() {
    // A stream that connects after the run finished must be able to deliver the
    // terminal event from retention and close. Without the terminal state on the page
    // it would wait for a terminal that was published before it connected.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    for (index, (from, to)) in [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::Failed),
    ]
    .into_iter()
    .enumerate()
    {
        let version = RunVersion::new(u64::try_from(index).expect("small") + 1);
        repositories
            .transition(
                workspace(),
                RunWrite::new(
                    &transition(from, to, version),
                    event(run_id(), version.get() + 1, "e"),
                ),
            )
            .await
            .expect("applies");
    }

    let page = repositories
        .load_events(workspace(), run_id(), 1, 100)
        .await
        .expect("readable");
    assert_eq!(page.terminal_state, Some(RunState::Failed));
    // The opening event plus the three transitions.
    assert_eq!(page.events.len(), 4);
}

#[tokio::test]
async fn a_non_terminal_run_reports_no_terminal_state() {
    // The negative case matters: reporting a terminal state for a live run would make
    // a stream close on a run that is still working.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    let page = repositories
        .load_events(workspace(), run_id(), 1, 100)
        .await
        .expect("readable");
    assert_eq!(page.terminal_state, None);
    // Only the opening event exists: creation published `run.received`.
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].event_type, "run.received");
}

#[tokio::test]
async fn an_event_page_is_scoped_to_its_workspace() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    assert_eq!(
        repositories
            .load_events(other_workspace(), run_id(), 1, 100)
            .await
            .expect_err("a foreign run is not readable"),
        RepositoryError::NotFound,
    );
}

#[tokio::test]
async fn an_event_page_resumes_strictly_after_the_requested_sequence() {
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
                event(run_id(), 2, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    // `>= from` with `from = 3` returns nothing: sequence 2 is delivered once.
    let page = repositories
        .load_events(workspace(), run_id(), 3, 100)
        .await
        .expect("readable");
    assert!(page.events.is_empty(), "{:?}", page.events);
}

#[tokio::test]
async fn a_first_claim_succeeds_and_an_identical_repeat_replays() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    let record = || NewIdempotencyRecord {
        key: "0195f4f0-18dc-729b-bb34-07e8c7627f21".to_owned(),
        workspace_id: workspace(),
        operation: "runs.create".to_owned(),
        request_digest: "digest-a".to_owned(),
        run_id: run_id(),
        created_at: now(),
    };
    assert_eq!(
        repositories
            .claim_idempotency(record())
            .await
            .expect("claims"),
        IdempotencyClaim::Claimed,
    );
    // The same key with the same digest is a replay carrying the original run, which
    // is what makes a retried create safe.
    assert_eq!(
        repositories
            .claim_idempotency(record())
            .await
            .expect("replays"),
        IdempotencyClaim::Replay(run_id()),
    );
}

#[tokio::test]
async fn a_reused_key_with_different_input_is_a_conflict() {
    // This is the refusal that stops one key being used for two different requests.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    repositories
        .claim_idempotency(NewIdempotencyRecord {
            key: "key-1".to_owned(),
            workspace_id: workspace(),
            operation: "runs.create".to_owned(),
            request_digest: "digest-a".to_owned(),
            run_id: run_id(),
            created_at: now(),
        })
        .await
        .expect("claims");

    assert_eq!(
        repositories
            .claim_idempotency(NewIdempotencyRecord {
                key: "key-1".to_owned(),
                workspace_id: workspace(),
                operation: "runs.create".to_owned(),
                request_digest: "digest-b".to_owned(),
                run_id: other_run_id(),
                created_at: now(),
            })
            .await
            .expect("answers"),
        IdempotencyClaim::Conflict,
    );
}

#[tokio::test]
async fn the_same_key_in_two_workspaces_does_not_replay_across_them() {
    // Idempotency is scoped to the resolved workspace, so one client's key must not
    // replay another workspace's run even when the opaque key string matches.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    // The other workspace needs its own conversation and run, because a record may
    // only name a run that exists.
    repositories
        .create_conversation(
            NewConversation::new(
                other_conversation_id(),
                other_workspace(),
                principal(),
                None,
                "cli".to_owned(),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("the other conversation is created");
    repositories
        .create(
            NewRun::new(
                other_run_id(),
                other_workspace(),
                other_conversation_id(),
                principal(),
                None,
                now(),
            )
            .expect("valid"),
            run_received_event(other_run_id(), now()),
        )
        .await
        .expect("the other run is created");

    repositories
        .claim_idempotency(NewIdempotencyRecord {
            key: "shared-key".to_owned(),
            workspace_id: workspace(),
            operation: "runs.create".to_owned(),
            request_digest: "digest-a".to_owned(),
            run_id: run_id(),
            created_at: now(),
        })
        .await
        .expect("claims");

    assert_eq!(
        repositories
            .claim_idempotency(NewIdempotencyRecord {
                key: "shared-key".to_owned(),
                workspace_id: other_workspace(),
                operation: "runs.create".to_owned(),
                request_digest: "digest-a".to_owned(),
                run_id: other_run_id(),
                created_at: now(),
            })
            .await
            .expect("claims in the other scope"),
        IdempotencyClaim::Claimed,
    );
}

#[tokio::test]
async fn a_claim_naming_an_absent_run_is_not_a_conflict() {
    // The foreign key would refuse this row anyway, but it would report the same
    // `Conflict` a concurrent claim produces. A caller would then be told "someone
    // else used this key" when the truth is "there is no such run".
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    assert_eq!(
        repositories
            .claim_idempotency(NewIdempotencyRecord {
                key: "key-for-a-ghost".to_owned(),
                workspace_id: workspace(),
                operation: "runs.create".to_owned(),
                request_digest: "digest-a".to_owned(),
                // Never created.
                run_id: RunId::from_uuid(id(999)),
                created_at: now(),
            })
            .await
            .expect_err("a record must name a real run"),
        RepositoryError::NotFound,
    );
}

#[tokio::test]
async fn an_empty_idempotency_key_is_refused() {
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    assert_eq!(
        repositories
            .claim_idempotency(NewIdempotencyRecord {
                key: String::new(),
                workspace_id: workspace(),
                operation: "runs.create".to_owned(),
                request_digest: "digest-a".to_owned(),
                run_id: run_id(),
                created_at: now(),
            })
            .await
            .expect_err("an empty key is not a key"),
        RepositoryError::Conflict {
            what: "idempotency_key"
        },
    );
}

#[tokio::test]
async fn a_published_delta_is_durable_and_ordered_after_the_transition() {
    // The delta sink is what makes a run's output replayable. It must assign the next
    // sequence itself and store the payload the contract defines, so a client that
    // reconnects sees the text it missed at the position it belongs.
    use jarvis_application::live_events::{OUTPUT_TEXT_DELTA_EVENT, StreamDeltaSink as _};
    use jarvis_protocol::event_type; // the wire constant the adapter must agree with

    assert_eq!(
        OUTPUT_TEXT_DELTA_EVENT,
        event_type::OUTPUT_TEXT_DELTA,
        "the port's event name and the wire constant must not drift",
    );

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
                event(run_id(), 2, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    let sequence = repositories
        .output_text_delta(
            workspace(),
            run_id(),
            "out-1".to_owned(),
            "Hello".to_owned(),
            now(),
        )
        .await
        .expect("publishes");
    // Sequence 3 follows the opening event (1) and the transition (2): the sink
    // assigns the next position rather than its own counter.
    assert_eq!(sequence, 3);

    let page = repositories
        .load_events(workspace(), run_id(), 1, 100)
        .await
        .expect("readable");
    assert_eq!(page.events.len(), 3);
    let delta = &page.events[2];
    assert_eq!(delta.event_type, event_type::OUTPUT_TEXT_DELTA);
    assert_eq!(delta.sequence, 3);
    let payload = delta
        .payload_json
        .as_deref()
        .expect("a delta carries a payload");
    assert!(payload.contains(r#""delta":"Hello""#), "{payload}");
    assert!(payload.contains(r#""item_id":"out-1""#), "{payload}");
    // The stored payload is a fixed shape, so prompt text or a secret cannot appear.
    let parsed: serde_json::Value = serde_json::from_str(payload).expect("valid JSON");
    assert_eq!(parsed.as_object().expect("object").len(), 2);
}

#[tokio::test]
async fn a_run_budget_and_deadline_round_trip_through_real_columns() {
    // `deadline_at` and `budget_json` are schema columns that had no port to populate
    // them, so they were always NULL. This proves the write and the read agree, and that
    // the column and the serialized budget name the same instant — two representations of
    // one fact are exactly where a silent disagreement can live.
    let (_database, repositories) = repository().await;
    seed_conversation_only(&repositories).await;

    let deadline = UtcTimestamp::parse("2026-09-22T13:00:00Z").expect("valid");
    let budget = RunBudget::with_deadline(deadline)
        .with_step_timeout(30_000)
        .expect("in range");
    repositories
        .create(
            NewRun::with_budget(
                run_id(),
                workspace(),
                conversation_id(),
                principal(),
                Some("objective-1".to_owned()),
                now(),
                budget,
            )
            .expect("valid"),
            run_received_event(run_id(), now()),
        )
        .await
        .expect("the run is created");

    let stored = repositories
        .load(workspace(), run_id())
        .await
        .expect("the run loads");
    assert_eq!(
        stored.deadline_at,
        Some(deadline),
        "the column must hold it"
    );
    assert_eq!(
        stored.budget, budget,
        "the serialized budget must round-trip"
    );
    assert_eq!(stored.budget.deadline, stored.deadline_at);
    assert_eq!(stored.budget.step_timeout_ms, Some(30_000));
}

#[tokio::test]
async fn a_run_created_without_a_budget_reads_back_without_one() {
    // The negative direction. An unset budget must read back as unset rather than as a
    // budget of zero, which would make every step expire immediately.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;

    let stored = repositories
        .load(workspace(), run_id())
        .await
        .expect("the run loads");
    assert_eq!(stored.deadline_at, None);
    assert_eq!(stored.budget, RunBudget::default());
    assert_eq!(stored.budget.deadline, None);
    assert_eq!(stored.budget.max_output_tokens, None);
}

#[tokio::test]
async fn an_unreadable_stored_budget_is_reported_rather_than_read_as_unbounded() {
    // A budget that cannot be interpreted must not become "no budget". Reading it as
    // unbounded would re-fund a run whose deadline had passed — the failure mode the
    // column exists to prevent — so corruption is reported by name.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    sqlx::query("UPDATE agent_runs SET budget_json = ? WHERE id = ?")
        .bind("{not json")
        .bind(run_id().to_string())
        .execute(repositories.pool())
        .await
        .expect("the corruption is written");

    assert_eq!(
        repositories
            .load(workspace(), run_id())
            .await
            .expect_err("an unreadable budget is not a valid read"),
        RepositoryError::Corrupted {
            column: "budget_json"
        },
    );
}

#[tokio::test]
async fn the_recovery_read_carries_the_deadline_and_budget_too() {
    // The startup pass reads runs through a different query, so it needs the same columns
    // or an interrupted run would be settled without its budget being consulted.
    let (_database, repositories) = repository().await;
    seed_conversation_only(&repositories).await;
    let deadline = UtcTimestamp::parse("2026-09-22T13:00:00Z").expect("valid");
    repositories
        .create(
            NewRun::with_budget(
                run_id(),
                workspace(),
                conversation_id(),
                principal(),
                None,
                now(),
                RunBudget::with_deadline(deadline),
            )
            .expect("valid"),
            run_received_event(run_id(), now()),
        )
        .await
        .expect("the run is created");

    let incomplete = repositories.incomplete_runs().await.expect("readable");
    let entry = incomplete.first().expect("the run is interrupted");
    assert_eq!(entry.run.deadline_at, Some(deadline), "{entry:?}");
    assert_eq!(entry.run.budget.deadline, Some(deadline), "{entry:?}");
}

#[tokio::test]
async fn reported_usage_and_its_lifted_cost_round_trip_through_real_columns() {
    // The ceiling check reads usage back through this adapter, so the write and the read
    // must agree — and the cost must land in its own column *from the same source* as the
    // block, because a cost query reads the column while the ceiling reads the block.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    let call_id = model_call_id();
    repositories
        .record_attempt(NewModelCall {
            id: call_id,
            workspace_id: workspace(),
            run_id: run_id(),
            logical_call_id: call_id,
            attempt: 1,
            model: model(),
            request_fingerprint: None,
            started_at: now(),
        })
        .await
        .expect("the attempt is recorded");

    let usage = jarvis_domain::model::stream::Usage {
        input_tokens: Some(100),
        output_tokens: Some(2049),
        cached_input_tokens: Some(10),
        reasoning_tokens: Some(5),
        provider_reported: true,
        estimated_cost_microunits: Some(5678),
        currency: Some("usd".to_owned()),
    };
    repositories
        .record_outcome(
            workspace(),
            call_id,
            ModelCallOutcome {
                state: ModelCallState::Completed,
                provider_request_id: Some("req-1".to_owned()),
                continuation_ref: None,
                usage: Some(usage.clone()),
                estimated_cost_microunits: usage.estimated_cost_microunits,
                finish_reason: Some(jarvis_domain::model::stream::FinishReason::Stop),
                error_code: None,
                first_output_at: None,
                completed_at: Some(now()),
            },
        )
        .await
        .expect("the outcome is recorded");

    let stored = repositories
        .load_attempt(workspace(), call_id)
        .await
        .expect("the attempt loads");
    assert_eq!(stored.state, ModelCallState::Completed);

    // Read the two columns directly, because `StoredModelCall` deliberately exposes only
    // what a caller needs and usage is not part of it. The adapter's own write is what is
    // under test here.
    let row =
        sqlx::query("SELECT usage_json, estimated_cost_microunits FROM model_calls WHERE id = ?")
            .bind(call_id.to_string())
            .fetch_one(repositories.pool())
            .await
            .expect("the row is readable");
    let encoded: Option<String> = sqlx::Row::try_get(&row, "usage_json").expect("usage_json");
    let encoded = encoded.expect("the usage block is stored");
    let decoded: jarvis_domain::model::stream::Usage =
        serde_json::from_str(&encoded).expect("the stored usage is readable");
    assert_eq!(decoded, usage, "the whole block must round-trip");
    assert_eq!(decoded.output_tokens, Some(2049));

    let cost: Option<i64> =
        sqlx::Row::try_get(&row, "estimated_cost_microunits").expect("the cost column");
    assert_eq!(
        cost,
        Some(5678),
        "the cost is written to its own column from the same source",
    );
}

#[tokio::test]
async fn a_call_with_no_reported_usage_stores_no_block_and_no_cost() {
    // The negative direction, and the one a ceiling must not misread: an unreported usage is
    // absent, never a measured zero, so a ceiling cannot be checked against it.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    let call_id = model_call_id();
    repositories
        .record_attempt(NewModelCall {
            id: call_id,
            workspace_id: workspace(),
            run_id: run_id(),
            logical_call_id: call_id,
            attempt: 1,
            model: model(),
            request_fingerprint: None,
            started_at: now(),
        })
        .await
        .expect("the attempt is recorded");
    repositories
        .record_outcome(
            workspace(),
            call_id,
            ModelCallOutcome {
                state: ModelCallState::Completed,
                provider_request_id: None,
                continuation_ref: None,
                usage: None,
                estimated_cost_microunits: None,
                finish_reason: None,
                error_code: None,
                first_output_at: None,
                completed_at: Some(now()),
            },
        )
        .await
        .expect("the outcome is recorded");

    let row =
        sqlx::query("SELECT usage_json, estimated_cost_microunits FROM model_calls WHERE id = ?")
            .bind(call_id.to_string())
            .fetch_one(repositories.pool())
            .await
            .expect("the row is readable");
    let encoded: Option<String> = sqlx::Row::try_get(&row, "usage_json").expect("usage_json");
    assert_eq!(encoded, None, "an unreported usage stores no block");
    let cost: Option<i64> =
        sqlx::Row::try_get(&row, "estimated_cost_microunits").expect("the cost column");
    assert_eq!(cost, None, "and no cost, rather than a measured zero");
}

#[tokio::test]
async fn a_delta_for_a_foreign_run_is_refused_rather_than_orphaned() {
    use jarvis_application::live_events::StreamDeltaSink as _;

    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    assert_eq!(
        repositories
            .output_text_delta(
                other_workspace(),
                run_id(),
                "out-1".to_owned(),
                "leak".to_owned(),
                now(),
            )
            .await
            .expect_err("a foreign run is absent"),
        RepositoryError::NotFound,
    );
}

#[tokio::test]
async fn an_interrupted_run_is_read_back_for_recovery_and_settled_through_real_sql() {
    // The whole recovery path against a migrated database: the read that finds the
    // interrupted run, and the optimistic write that settles it. The in-memory port
    // double agrees with whatever the code assumed; only the SQL proves the predicate,
    // the ordering, and the version guard are the ones the daemon will actually use.
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
                event(run_id(), 2, "run.context_building"),
            ),
        )
        .await
        .expect("applies");

    let incomplete = repositories
        .incomplete_runs()
        .await
        .expect("the interrupted run is readable");
    assert_eq!(incomplete.len(), 1, "{incomplete:?}");
    assert_eq!(incomplete[0].run.id, run_id());
    // The workspace travels with the entry, because the read is unscoped and the write
    // needs a scope.
    assert_eq!(incomplete[0].workspace_id, workspace());
    assert_eq!(incomplete[0].run.state, RunState::ContextBuilding);

    let settled = repositories
        .transition(
            workspace(),
            RunWrite::new(
                &RunTransition::new(
                    RunState::ContextBuilding,
                    RunState::Failed,
                    incomplete[0].run.version,
                    TransitionActor::Supervisor,
                    reason("interrupted_while_working"),
                    now(),
                ),
                event(run_id(), 3, "run.failed"),
            ),
        )
        .await
        .expect("the recovery applies");
    assert_eq!(settled.state, RunState::Failed);

    // And a second pass finds nothing, because the predicate is on the stored state.
    assert!(
        repositories
            .incomplete_runs()
            .await
            .expect("readable")
            .is_empty(),
        "a settled run must no longer be offered for recovery",
    );
}

#[tokio::test]
async fn recovery_finds_interrupted_runs_in_every_workspace_at_once() {
    // Scoping this read to one workspace would leave every other workspace's runs
    // non-terminal forever with no symptom, so the unscoped read is the property to
    // pin: two workspaces, one query, both runs.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    repositories
        .create_conversation(
            NewConversation::new(
                other_conversation_id(),
                other_workspace(),
                principal(),
                None,
                "cli".to_owned(),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("created");
    repositories
        .create(
            NewRun::new(
                other_run_id(),
                other_workspace(),
                other_conversation_id(),
                principal(),
                None,
                now(),
            )
            .expect("valid"),
            run_received_event(other_run_id(), now()),
        )
        .await
        .expect("created");

    let incomplete = repositories.incomplete_runs().await.expect("readable");
    assert_eq!(incomplete.len(), 2, "{incomplete:?}");
    let workspaces: Vec<WorkspaceId> = incomplete.iter().map(|entry| entry.workspace_id).collect();
    assert!(workspaces.contains(&workspace()), "{workspaces:?}");
    assert!(workspaces.contains(&other_workspace()), "{workspaces:?}");
}

#[tokio::test]
async fn a_terminal_run_is_never_offered_for_recovery() {
    // "Terminal runs remain terminal" starts at the read: a completed, failed, or
    // cancelled run must not appear at all, or the pass would revisit it.
    let (_database, repositories) = repository().await;
    seed(&repositories).await;
    let (run_id, other_run_id) = (run_id(), other_run_id());

    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(RunState::Received, RunState::Cancelled, RunVersion::FIRST),
                event(run_id, 2, "run.cancelled"),
            ),
        )
        .await
        .expect("cancels");

    repositories
        .create_conversation(
            NewConversation::new(
                other_conversation_id(),
                other_workspace(),
                principal(),
                None,
                "cli".to_owned(),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("created");
    repositories
        .create(
            NewRun::new(
                other_run_id,
                other_workspace(),
                other_conversation_id(),
                principal(),
                None,
                now(),
            )
            .expect("valid"),
            run_received_event(other_run_id, now()),
        )
        .await
        .expect("created");
    repositories
        .transition(
            other_workspace(),
            RunWrite::new(
                &transition(RunState::Received, RunState::Failed, RunVersion::FIRST),
                event(other_run_id, 2, "run.failed"),
            ),
        )
        .await
        .expect("fails");

    assert!(
        repositories
            .incomplete_runs()
            .await
            .expect("readable")
            .is_empty(),
        "cancelled and failed runs are terminal",
    );
}

#[tokio::test]
async fn recovery_waiting_fields_are_read_back_for_a_parked_run() {
    // A parked run's dependency is what makes it *resumable* rather than lost work, so
    // the read must carry those two fields. Dropping them would classify every parked
    // run as abandoned, and the distinction would vanish silently.
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
                event(run_id(), 2, "run.context_building"),
            ),
        )
        .await
        .expect("applies");
    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(
                    RunState::ContextBuilding,
                    RunState::Planning,
                    RunVersion::new(2),
                ),
                event(run_id(), 3, "run.planning"),
            ),
        )
        .await
        .expect("applies");

    let incomplete = repositories.incomplete_runs().await.expect("readable");
    let entry = incomplete.first().expect("the run is interrupted");
    // A working run has no dependency, which is a real value and not a missing one.
    assert_eq!(entry.waiting_kind, None, "{entry:?}");
    assert_eq!(entry.waiting_ref, None, "{entry:?}");

    // Now park it, and prove the fields come back. `Waiting` is reached from
    // `Observing`, not directly from `AwaitingModel` — the first version of this test
    // tried the short path and the machine refused it, which is the machine being right.
    for (from, to, version, sequence, name) in [
        (
            RunState::Planning,
            RunState::AwaitingModel,
            3,
            4,
            "run.awaiting_model",
        ),
        (
            RunState::AwaitingModel,
            RunState::ExecutingTool,
            4,
            5,
            "run.executing_tool",
        ),
        (
            RunState::ExecutingTool,
            RunState::Observing,
            5,
            6,
            "run.observing",
        ),
    ] {
        repositories
            .transition(
                workspace(),
                RunWrite::new(
                    &transition(from, to, RunVersion::new(version)),
                    event(run_id(), sequence, name),
                ),
            )
            .await
            .expect("applies");
    }
    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &transition(RunState::Observing, RunState::Waiting, RunVersion::new(6)),
                event(run_id(), 7, "run.waiting"),
            )
            .waiting_on(WaitingOn::new("timer", "wake-1").expect("valid")),
        )
        .await
        .expect("applies");

    let incomplete = repositories.incomplete_runs().await.expect("readable");
    let entry = incomplete.first().expect("the parked run is found");
    assert_eq!(entry.waiting_kind.as_deref(), Some("timer"), "{entry:?}");
    assert_eq!(entry.waiting_ref.as_deref(), Some("wake-1"), "{entry:?}");
    assert!(
        jarvis_domain::run::recovery::needs_recovery(entry.run.state),
        "a waiting run must still be offered for recovery",
    );
}
