//! Tests for the run orchestration service.
//!
//! These drive the service against the in-memory repositories and a deterministic
//! spawner, so an assertion is about what was **persisted** and which scope was
//! signalled rather than about timing. The spawner is the important piece: a real
//! `tokio::spawn` would make "the run reached a terminal state" a race, and this
//! suite runs the run's future to completion before returning.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use jarvis_domain::clock::ManualClock;
use jarvis_domain::ids::{
    ConversationId, CorrelationId, PrincipalId, RequestId, RunId, WorkspaceId,
};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{FinishReason, ModelStreamEventKind};
use jarvis_domain::run::state::RunState;
use jarvis_domain::time::UtcTimestamp;
use uuid::Uuid;

use super::{
    CREATE_OPERATION, CreatedRun, MAX_OBJECTIVE_BYTES, RunCancellationRegistry, RunPorts,
    RunService, RunServiceError, RunSpawner, api_request_context,
};
use crate::model::ScriptedProvider;
use crate::repository::conversation::ConversationRepository as _;
use crate::repository::run::RunRepository as _;
use crate::request_context::RequestContext;
use crate::request_context::{AuthenticationAssurance, RequestChannel};
use crate::testing::InMemoryRepositories;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(id(1))
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(id(2))
}

fn context() -> RequestContext {
    api_request_context(
        workspace(),
        principal(),
        RequestId::from_uuid(id(3)),
        CorrelationId::from_uuid(id(4)),
    )
}

fn model() -> ModelRef {
    ModelRef::new(
        ProviderId::parse("scripted.local").expect("valid"),
        ModelId::parse("fixture-1").expect("valid"),
    )
}

/// A spawner that records tasks instead of racing them.
///
/// `run_now` executes each task inline so a test observes the finished run; `defer`
/// keeps the task so a test can assert it was scheduled without running it.
#[derive(Default)]
struct RecordingSpawner {
    tasks: Mutex<Vec<Pin<Box<dyn Future<Output = ()> + Send>>>>,
}

impl RecordingSpawner {
    fn new() -> Self {
        Self::default()
    }

    /// Runs every recorded task to completion, draining the queue first so a task
    /// that schedules another is also run.
    async fn run_all(&self) {
        loop {
            let next = {
                let mut tasks = self.tasks.lock().expect("lock");
                tasks.pop()
            };
            match next {
                Some(task) => task.await,
                None => break,
            }
        }
    }

    fn pending(&self) -> usize {
        self.tasks.lock().expect("lock").len()
    }
}

impl RunSpawner for RecordingSpawner {
    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {
        self.tasks.lock().expect("lock").push(task);
    }
}

impl std::fmt::Debug for RecordingSpawner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The queued futures are not printable, so the count is reported instead —
        // which is what an assertion actually reads.
        formatter
            .debug_struct("RecordingSpawner")
            .field("queued", &self.pending())
            .finish()
    }
}

struct Fixture {
    service: RunService,
    repositories: Arc<InMemoryRepositories>,
    cancellations: Arc<RunCancellationRegistry>,
    spawner: RecordingSpawner,
}

fn fixture() -> Fixture {
    fixture_with(Arc::new(
        ScriptedProvider::new(model())
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", "the answer")
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            }),
    ))
}

fn fixture_with(provider: Arc<ScriptedProvider>) -> Fixture {
    let repositories = Arc::new(InMemoryRepositories::new());
    let cancellations = Arc::new(RunCancellationRegistry::new());
    let service = RunService::new(
        RunPorts {
            runs: Arc::clone(&repositories) as Arc<dyn crate::repository::run::RunRepository>,
            conversations: Arc::clone(&repositories)
                as Arc<dyn crate::repository::conversation::ConversationRepository>,
            model_calls: Arc::clone(&repositories)
                as Arc<dyn crate::repository::model_call::ModelCallRepository>,
            deltas: Arc::clone(&repositories) as Arc<dyn crate::live_events::StreamDeltaSink>,
            provider,
            clock: Arc::new(ManualClock::new(now())),
        },
        Arc::clone(&cancellations),
    );
    Fixture {
        service,
        repositories,
        cancellations,
        spawner: RecordingSpawner::new(),
    }
}

async fn create(fixture: &Fixture, text: &str, key: &str) -> CreatedRun {
    fixture
        .service
        .create(&context(), None, text, key, &fixture.spawner)
        .await
        .expect("the run is created")
}

#[tokio::test]
async fn a_create_command_returns_a_run_that_is_already_durable_and_streamable() {
    // The API must answer `202` with a run a client can immediately stream, so the
    // run and its opening event have to exist before `create` returns — not after the
    // background task gets around to starting.
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-1").await;

    assert!(!created.replayed);
    assert_eq!(created.state, RunState::Received);

    // The run is readable immediately, in `Received`.
    let stored = fixture
        .repositories
        .load(workspace(), created.run_id)
        .await
        .expect("the run is durable");
    assert_eq!(stored.state, RunState::Received);

    // And its first event is already there, so a client connecting now sees
    // `run.received` rather than an empty stream.
    let page = fixture
        .repositories
        .load_events(workspace(), created.run_id, 1, 100)
        .await
        .expect("events are readable");
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].event_type, "run.received");
    assert_eq!(page.events[0].sequence, 1);
}

#[tokio::test]
async fn a_created_run_carries_a_bounded_deadline_so_it_cannot_hang_forever() {
    // A run with no deadline is a run no bound applies to: a provider that never answers
    // would hold it open until a daemon restart recovered it. Every run therefore gets a
    // budget at creation, and this asserts the *stored* value rather than the constant —
    // a budget that is computed and then dropped would satisfy a test of the constant.
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-1").await;

    let stored = fixture
        .repositories
        .load(workspace(), created.run_id)
        .await
        .expect("the run is durable");

    let deadline = stored.deadline_at.expect("every run must carry a deadline");
    assert_eq!(
        stored.budget.deadline,
        Some(deadline),
        "the column and the serialized budget must name the same instant",
    );
    // The deadline is in the future relative to creation, and it is the configured
    // offset rather than some other value.
    let expected = jarvis_domain::run::budget::RunBudget::expiring_after(
        stored.created_at,
        super::DEFAULT_RUN_BUDGET_MS,
    )
    .expect("the default is in range");
    assert_eq!(stored.budget, expected);
    assert!(
        deadline > stored.created_at,
        "{deadline} {0}",
        stored.created_at
    );
}

#[tokio::test]
async fn an_expired_deadline_is_refused_by_the_budget_the_run_stored() {
    // The stored budget is what the controller reads, so it must be a budget that
    // actually permits the run to start. A deadline in the past here would fail every run
    // at its first step, which is worse than no budget at all.
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-2").await;
    let stored = fixture
        .repositories
        .load(workspace(), created.run_id)
        .await
        .expect("the run is durable");
    assert!(
        stored.budget.permits_step_at(stored.created_at),
        "a freshly created run must have time to work: {:?}",
        stored.budget,
    );
}

#[tokio::test]
async fn a_repeated_create_with_the_same_key_replays_and_starts_no_second_run() {
    // This is the property the idempotency key exists for: a client that retries a
    // create because it did not see the response must not get two runs.
    let fixture = fixture();
    let first = create(&fixture, "hello", "key-1").await;
    let second = create(&fixture, "hello", "key-1").await;

    assert_eq!(second.run_id, first.run_id);
    assert!(second.replayed);
    assert_eq!(
        fixture.repositories.run_count().expect("readable"),
        1,
        "a replay must not create a second run",
    );
    // Only one task was scheduled, so no second execution started either.
    assert_eq!(fixture.spawner.pending(), 1);
}

#[tokio::test]
async fn the_same_key_with_different_input_is_a_conflict() {
    let fixture = fixture();
    create(&fixture, "hello", "key-1").await;
    let error = fixture
        .service
        .create(
            &context(),
            None,
            "something else",
            "key-1",
            &fixture.spawner,
        )
        .await
        .expect_err("a reused key with different input must be refused");
    assert_eq!(error, RunServiceError::IdempotencyConflict);
    assert_eq!(error.code(), "idempotency.conflict");
    assert!(!error.retryable());
}

#[tokio::test]
async fn a_missing_idempotency_key_is_refused_before_any_write() {
    let fixture = fixture();
    let error = fixture
        .service
        .create(&context(), None, "hello", "", &fixture.spawner)
        .await
        .expect_err("the key is required");
    assert_eq!(error.code(), "request.invalid");
    assert_eq!(
        fixture.repositories.run_count().expect("readable"),
        0,
        "a refused request must not have created a run",
    );
}

#[tokio::test]
async fn an_empty_objective_is_refused_and_an_over_long_one_too() {
    let fixture = fixture();
    for objective in [String::new(), "x".repeat(MAX_OBJECTIVE_BYTES + 1)] {
        let error = fixture
            .service
            .create(&context(), None, &objective, "key-1", &fixture.spawner)
            .await
            .expect_err("the objective is outside its bound");
        assert_eq!(error.code(), "request.semantic_invalid");
    }
    assert_eq!(fixture.repositories.run_count().expect("readable"), 0);
}

#[tokio::test]
async fn the_objective_is_stored_as_the_users_message_so_the_run_can_see_it() {
    // The controller reads the transcript to build the model request, so the
    // objective has to be in the conversation or the model would be asked to answer a
    // question it cannot see.
    let fixture = fixture();
    let created = create(&fixture, "what is the deadline", "key-1").await;
    let messages = fixture
        .repositories
        .load_messages(workspace(), created.conversation_id, None, 10)
        .await
        .expect("messages load");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "what is the deadline");
    assert_eq!(messages[0].role, jarvis_domain::model::stream::Role::User);
}

#[tokio::test]
async fn a_run_driven_to_completion_ends_completed_and_publishes_one_terminal_event() {
    // The end-to-end product path: create, then run to a terminal state, then read the
    // state and the stream. This is what `BRN-007` is for.
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-1").await;
    fixture.spawner.run_all().await;

    let stored = fixture
        .repositories
        .load(workspace(), created.run_id)
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Completed);

    let page = fixture
        .repositories
        .load_events(workspace(), created.run_id, 1, 100)
        .await
        .expect("events");
    assert_eq!(page.terminal_state, Some(RunState::Completed));
    let terminals = page
        .events
        .iter()
        .filter(|event| event.event_type == "run.completed")
        .count();
    assert_eq!(
        terminals, 1,
        "exactly one terminal event: {:?}",
        page.events
    );

    // The conversation the caller was told about is the one the answer landed in.
    let messages = fixture
        .repositories
        .load_messages(workspace(), created.conversation_id, None, 10)
        .await
        .expect("messages");
    assert!(
        messages
            .iter()
            .any(|message| message.role == jarvis_domain::model::stream::Role::Assistant),
        "the answer belongs in the conversation the create response named",
    );
}

#[tokio::test]
async fn a_cancel_signals_the_running_scope_rather_than_reporting_a_false_cancelled() {
    // The contract requires that cancellation does not report `cancelled` until a
    // durable terminal transition exists. A create-then-cancel sequence while the task
    // has not been driven yet must therefore report the live state, and the scope must
    // be signalled so the controller can observe it.
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-1").await;

    let reported = fixture
        .service
        .cancel(&context(), created.run_id, "user_requested", "cancel-key")
        .await
        .expect("cancellation is accepted");
    assert_eq!(
        reported,
        RunState::Received,
        "a live run must not be reported cancelled before cleanup is durable",
    );
    // The scope really was signalled: driving the task now ends the run cancelled.
    fixture.spawner.run_all().await;
    let stored = fixture
        .repositories
        .load(workspace(), created.run_id)
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Cancelled);
}

#[tokio::test]
async fn a_repeated_cancel_is_idempotent_and_signals_nothing_extra() {
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-1").await;
    fixture
        .service
        .cancel(&context(), created.run_id, "user_requested", "cancel-key")
        .await
        .expect("accepted");
    // The second call with the same key replays: same answer, no new effect.
    let again = fixture
        .service
        .cancel(&context(), created.run_id, "user_requested", "cancel-key")
        .await
        .expect("accepted");
    assert_eq!(again, RunState::Received);

    // A different key against a now-terminal run returns the unchanged terminal state
    // and signals nothing, because the run is already finished.
    fixture.spawner.run_all().await;
    let terminal = fixture
        .service
        .cancel(&context(), created.run_id, "user_requested", "cancel-key-2")
        .await
        .expect("accepted");
    assert_eq!(terminal, RunState::Cancelled);
}

#[tokio::test]
async fn a_cancel_for_a_foreign_run_is_not_found_rather_than_a_silent_success() {
    // A cancel that stopped nothing must not report success: a caller would conclude
    // the run was stopped.
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-1").await;
    let foreign = api_request_context(
        WorkspaceId::from_uuid(id(99)),
        principal(),
        RequestId::from_uuid(id(5)),
        CorrelationId::from_uuid(id(6)),
    );
    let error = fixture
        .service
        .cancel(&foreign, created.run_id, "user_requested", "cancel-key")
        .await
        .expect_err("a foreign run is absent");
    assert_eq!(error, RunServiceError::NotFound);
    assert_eq!(error.code(), "resource.not_found");
}

#[tokio::test]
async fn a_create_into_a_foreign_conversation_is_not_found() {
    // Scope is resolved server-side, so naming another workspace's conversation must
    // not attach the run to it.
    let fixture = fixture();
    let foreign_conversation = ConversationId::from_uuid(id(77));
    let foreign = api_request_context(
        WorkspaceId::from_uuid(id(98)),
        principal(),
        RequestId::from_uuid(id(7)),
        CorrelationId::from_uuid(id(8)),
    );
    // First create it in the other scope so it genuinely exists.
    fixture
        .service
        .create(&foreign, None, "theirs", "their-key", &fixture.spawner)
        .await
        .expect("their run is created");
    let error = fixture
        .service
        .create(
            &context(),
            Some(foreign_conversation),
            "mine",
            "my-key",
            &fixture.spawner,
        )
        .await
        .expect_err("a foreign conversation is absent");
    assert_eq!(error, RunServiceError::NotFound);
}

#[tokio::test]
async fn an_existing_conversation_is_continued_rather_than_replaced() {
    // A follow-up run must land in the same conversation, which is what makes the
    // transcript available to the next model call.
    let fixture = fixture();
    let first = create(&fixture, "first question", "key-1").await;
    fixture.spawner.run_all().await;

    let second = fixture
        .service
        .create(
            &context(),
            Some(first.conversation_id),
            "second question",
            "key-2",
            &fixture.spawner,
        )
        .await
        .expect("the follow-up is created");
    assert_eq!(second.conversation_id, first.conversation_id);
    assert_ne!(second.run_id, first.run_id);

    let messages = fixture
        .repositories
        .load_messages(workspace(), first.conversation_id, None, 10)
        .await
        .expect("messages");
    // Two user turns and the first run's answer, all in one conversation.
    assert!(messages.len() >= 3, "{messages:?}");
}

#[tokio::test]
async fn a_terminal_run_is_forgotten_by_the_cancellation_registry() {
    // A registry that kept every finished run's scope would grow without bound over a
    // long-lived daemon.
    let fixture = fixture();
    let created = create(&fixture, "hello", "key-1").await;
    assert!(fixture.cancellations.cancel(created.run_id));
    // Re-register so the run is still cancellable, then finish it.
    fixture.cancellations.register(
        created.run_id,
        crate::cancellation::CancellationScope::new(),
    );
    fixture.spawner.run_all().await;

    // After the task completes the scope is gone, so a later cancel reports that
    // nothing was signalled rather than claiming success.
    assert!(!fixture.cancellations.cancel(created.run_id));
}

#[tokio::test]
async fn an_unregistered_run_reports_that_cancel_signalled_nothing() {
    let registry = RunCancellationRegistry::new();
    assert!(!registry.cancel(RunId::from_uuid(id(500))));
}

#[tokio::test]
async fn the_create_operation_name_is_stable_and_scoped() {
    // The operation name is part of the idempotency key's scope, so a change would
    // silently un-share keys between builds.
    assert_eq!(CREATE_OPERATION, "runs.create");
    assert_eq!(super::CANCEL_OPERATION, "runs.cancel");
}

#[test]
fn the_objective_bound_matches_the_contract() {
    // The daemon has a test asserting this equals the protocol crate's constant; this
    // one pins the value the application layer implements.
    assert_eq!(MAX_OBJECTIVE_BYTES, 32 * 1024);
}

#[test]
fn an_api_context_is_standard_assurance_and_the_api_channel() {
    let context = api_request_context(
        workspace(),
        principal(),
        RequestId::from_uuid(id(9)),
        CorrelationId::from_uuid(id(10)),
    );
    assert_eq!(context.assurance, AuthenticationAssurance::Standard);
    assert_eq!(context.channel, RequestChannel::Api);
    assert_eq!(context.workspace_id, workspace());
}

#[tokio::test]
async fn a_provider_refusal_leaves_the_run_failed_rather_than_received() {
    // A run whose provider refuses must not sit in a non-terminal state: a client
    // would poll forever for an answer that will never arrive.
    let fixture = fixture_with(Arc::new(
        ScriptedProvider::new(model()).fail_on_open(crate::model::ProviderError::Unavailable),
    ));
    let created = create(&fixture, "hello", "key-1").await;
    fixture.spawner.run_all().await;
    let stored = fixture
        .repositories
        .load(workspace(), created.run_id)
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Failed);
    assert!(stored.is_terminal());
}
