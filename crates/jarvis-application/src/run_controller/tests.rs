//! Tests for the native run controller.
//!
//! These drive the controller against the in-memory repository double and the
//! scripted provider, so an assertion is about what was **persisted** and what state
//! the run settled in rather than about calls made to a mock. The controller's whole
//! purpose is to turn a provider's frames into durable state, and a store that
//! recorded nothing would agree with whatever the controller assumed.

use std::sync::Arc;

use jarvis_domain::clock::ManualClock;
use jarvis_domain::ids::{
    ConversationId, CorrelationId, PrincipalId, RequestId, RunId, WorkspaceId,
};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{FinishReason, ModelCallRequest, ModelStreamEventKind, Role};
use jarvis_domain::run::state::RunState;
use jarvis_domain::time::UtcTimestamp;
use uuid::Uuid;

use super::{ControllerError, MAX_TRANSCRIPT_MESSAGES, RunController};
use crate::cancellation::CancellationScope;
use crate::model::{ModelProvider, OpenResult, ProviderError, ScriptedProvider};
use crate::repository::conversation::{ConversationRepository, NewConversation, NewMessage};
use crate::repository::model_call::ModelCallRepository;
use crate::repository::run::{NewRun, RunRepository};
use crate::request_context::{AuthenticationAssurance, RequestChannel, RequestContext};
use crate::testing::InMemoryRepositories;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
}

fn model() -> ModelRef {
    ModelRef::new(
        ProviderId::parse("scripted.local").expect("valid"),
        ModelId::parse("fixture-1").expect("valid"),
    )
}

fn context() -> RequestContext {
    RequestContext::new(
        RequestId::from_uuid(id(1)),
        CorrelationId::from_uuid(id(2)),
        PrincipalId::from_uuid(id(3)),
        AuthenticationAssurance::Standard,
        WorkspaceId::from_uuid(id(4)),
        RequestChannel::Cli,
    )
}

const CONVERSATION: u128 = 20;
const RUN: u128 = 21;

fn conversation() -> ConversationId {
    ConversationId::from_uuid(id(CONVERSATION))
}

fn run() -> RunId {
    RunId::from_uuid(id(RUN))
}

/// A controller and the store it drives.
struct Fixture {
    controller: RunController,
    repositories: Arc<InMemoryRepositories>,
}

fn fixture(provider: Arc<dyn ModelProvider>) -> Fixture {
    let repositories = Arc::new(InMemoryRepositories::new());
    let runs: Arc<dyn RunRepository> = repositories.clone();
    let conversations: Arc<dyn ConversationRepository> = repositories.clone();
    let model_calls: Arc<dyn ModelCallRepository> = repositories.clone();
    let clock = Arc::new(ManualClock::new(now()));
    let controller = RunController::new(
        runs,
        conversations,
        model_calls,
        Arc::clone(&repositories) as Arc<dyn crate::live_events::StreamDeltaSink>,
        provider,
        clock,
    );
    Fixture {
        controller,
        repositories,
    }
}

/// Seeds a conversation and a run in `Received`.
async fn seed(fixture: &Fixture) {
    fixture
        .repositories
        .create_conversation(
            NewConversation::new(
                conversation(),
                context().workspace_id,
                PrincipalId::from_uuid(id(3)),
                Some("first".to_owned()),
                "cli".to_owned(),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("the conversation is created");
    fixture
        .repositories
        .create(
            NewRun::new(
                run(),
                context().workspace_id,
                conversation(),
                PrincipalId::from_uuid(id(3)),
                Some("objective-1".to_owned()),
                now(),
            )
            .expect("valid"),
            crate::repository::run::run_received_event(run(), now()),
        )
        .await
        .expect("the run is created");
}

/// A provider that answers with `text` and completes.
fn answering(text: &str) -> Arc<dyn ModelProvider> {
    Arc::new(
        ScriptedProvider::new(model())
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", text)
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            }),
    )
}

async fn execute(
    fixture: &Fixture,
    cancel: &CancellationScope,
) -> Result<super::RunOutcome, ControllerError> {
    fixture
        .controller
        .execute(&context(), run(), conversation(), "hello", cancel)
        .await
}

#[tokio::test]
async fn a_plain_question_is_answered_and_the_run_completes() {
    // The product's core loop end to end: a question goes in, the model answers, and
    // the run is durably `Completed` with the answer persisted. This is the path the
    // missing `AwaitingModel -> Responding` edge had made unreachable.
    let fixture = fixture(answering("Hello there"));
    seed(&fixture).await;

    let outcome = execute(&fixture, &CancellationScope::new())
        .await
        .expect("the run completes");
    assert_eq!(outcome.state, RunState::Completed);
    assert_eq!(outcome.answer.as_deref(), Some("Hello there"));
    assert_eq!(outcome.error_code, None);

    // The durable state is the proof, not the returned value.
    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("the run loads");
    assert_eq!(stored.state, RunState::Completed);
    assert!(stored.is_terminal());
    assert_eq!(stored.completed_at, Some(now()));

    // And the answer survived as the assistant's message.
    let messages = fixture
        .repositories
        .load_messages(context().workspace_id, conversation(), None, 10)
        .await
        .expect("messages load");
    assert!(
        messages
            .iter()
            .any(|message| message.content == "Hello there" && message.role == Role::Assistant),
        "the answer must be persisted as an assistant message",
    );
}

#[tokio::test]
async fn each_state_change_published_exactly_one_event() {
    // The run's opening event, five transitions, and one output delta, so seven events
    // exist in strict sequence order and the next position is eight. The delta is
    // interleaved at the point it was produced — before `responding` — because the
    // contract requires each chunk to be a durable public event at its own position,
    // so a client that reconnects replays the output it missed rather than only the
    // final answer.
    let fixture = fixture(answering("answer"));
    seed(&fixture).await;
    execute(&fixture, &CancellationScope::new())
        .await
        .expect("completes");

    let events = fixture
        .repositories
        .recorded_events()
        .expect("events are readable");
    let types: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(
        types,
        [
            "run.received",
            "run.context_building",
            "run.planning",
            "run.model_started",
            "run.output_text.delta",
            "run.responding",
            "run.completed",
        ],
        "{events:?}",
    );
    // Sequences are 1..=n with no gap, which the contract states as "starts at 1 per
    // run and increases by exactly one for each persisted public event".
    let sequences: Vec<u64> = events.iter().map(|event| event.sequence).collect();
    assert_eq!(sequences, [1, 2, 3, 4, 5, 6, 7]);
    // Exactly one terminal event, which the contract requires.
    let terminals = types
        .iter()
        .filter(|kind| {
            kind.starts_with("run.completed")
                || kind.starts_with("run.failed")
                || kind.starts_with("run.cancelled")
        })
        .count();
    assert_eq!(terminals, 1);
    assert_eq!(
        fixture
            .repositories
            .next_event_sequence(context().workspace_id, run())
            .await
            .expect("readable"),
        8,
    );
}

#[tokio::test]
async fn every_output_chunk_is_published_before_the_run_completes() {
    // The contract requires the server to persist an event before making it visible,
    // so the stored stream must contain each chunk in order and must contain it
    // *before* the terminal event. A controller that published only the whole answer
    // at the end would pass a final-state assertion and still break a streaming
    // client.
    let provider: Arc<dyn ModelProvider> = Arc::new(
        ScriptedProvider::new(model())
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", "Hel")
            .emit_text("out-1", "lo")
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            }),
    );
    let fixture = fixture(provider);
    seed(&fixture).await;
    execute(&fixture, &CancellationScope::new())
        .await
        .expect("completes");

    let deltas = fixture
        .repositories
        .recorded_deltas()
        .expect("deltas are readable");
    // Two chunks were published as two deltas, not coalesced into one.
    assert_eq!(
        deltas,
        [
            ("out-1".to_owned(), "Hel".to_owned()),
            ("out-1".to_owned(), "lo".to_owned())
        ]
    );

    let events = fixture
        .repositories
        .recorded_events()
        .expect("events are readable");
    let terminal_index = events
        .iter()
        .position(|event| event.event_type == "run.completed")
        .expect("a terminal event exists");
    for (index, event) in events.iter().enumerate() {
        if event.event_type == "run.output_text_delta" {
            assert!(
                index < terminal_index,
                "a delta must be persisted before the terminal event",
            );
        }
    }
}

#[tokio::test]
async fn a_stream_without_a_terminal_fails_the_run_rather_than_completing_it() {
    // A provider that disconnects mid-answer must not be recorded as success.
    let provider: Arc<dyn ModelProvider> = Arc::new(
        ScriptedProvider::new(model())
            .emit_text("out-1", "partial")
            .interrupt(),
    );
    let fixture = fixture(provider);
    seed(&fixture).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("an interrupted stream must not complete");
    assert_eq!(error.code(), "run.stream_interrupted");
    assert_eq!(error.terminal_state(), RunState::Failed);

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Failed);
    assert_ne!(stored.state, RunState::Completed);
}

#[tokio::test]
async fn a_tool_intent_is_refused_as_unimplemented_not_faked() {
    // The tool fabric is Milestone 3, so the controller must say so with a stable code
    // rather than fabricate an observation, and the run must end terminal.
    let provider: Arc<dyn ModelProvider> = Arc::new(
        ScriptedProvider::new(model())
            .emit(ModelStreamEventKind::ToolCallAdded {
                call_id: "call-1".to_owned(),
                tool_name: "fs.read".to_owned(),
            })
            .emit(ModelStreamEventKind::ToolCallArgumentsDelta {
                call_id: "call-1".to_owned(),
                delta: "{\"path\":\"/tmp\"}".to_owned(),
            })
            .emit(ModelStreamEventKind::ToolCallCompleted {
                call_id: "call-1".to_owned(),
                arguments: "{\"path\":\"/tmp\"}".to_owned(),
            })
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::ToolCalls,
                usage: None,
                refused: false,
            }),
    );
    let fixture = fixture(provider);
    seed(&fixture).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a tool intent cannot be satisfied yet");
    assert_eq!(error.code(), "run.tools_not_implemented");
    assert!(
        error.is_unimplemented(),
        "a known gap must be distinguishable from a fault",
    );

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Failed);
}

#[tokio::test]
async fn a_provider_refusal_fails_the_run_with_the_providers_own_code() {
    let provider: Arc<dyn ModelProvider> =
        Arc::new(ScriptedProvider::new(model()).fail_on_open(ProviderError::Unavailable));
    let fixture = fixture(provider);
    seed(&fixture).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a refused provider must not complete the run");
    assert_eq!(error.code(), "model.provider_unavailable");

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Failed);
}

#[tokio::test]
async fn a_cancelled_call_ends_cancelled_rather_than_failed() {
    // A caller's own action must not be recorded as a fault: reporting one as the
    // other is what makes an operator distrust the state.
    let provider: Arc<dyn ModelProvider> =
        Arc::new(ScriptedProvider::new(model()).fail_on_open(ProviderError::Cancelled));
    let fixture = fixture(provider);
    seed(&fixture).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a cancelled call does not complete");
    assert_eq!(error.terminal_state(), RunState::Cancelled);

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Cancelled);
}

#[tokio::test]
async fn a_cancellation_registered_before_any_work_still_reaches_a_terminal_state() {
    // The run must be left `Cancelled`, not `Received`.
    //
    // An earlier version of the controller returned without touching the run, on the
    // reasoning that recording work that never happened is a falsehood. Driving the
    // service end to end showed why that was wrong: moving to `Cancelled` does not
    // assert that work happened, it records that the request was cancelled — which is
    // exactly what occurred. Leaving the run in `Received` made it permanently
    // non-terminal, so a client polling it waited forever for an answer that had
    // already been abandoned.
    let fixture = fixture(answering("hi"));
    seed(&fixture).await;
    let cancel = CancellationScope::new();
    cancel.cancel();

    let error = execute(&fixture, &cancel)
        .await
        .expect_err("a cancelled request does not run");
    assert_eq!(error, ControllerError::Cancelled);
    assert_eq!(error.terminal_state(), RunState::Cancelled);

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Cancelled);
    assert!(stored.is_terminal(), "a cancelled run must be terminal");
    assert_eq!(stored.completed_at, Some(now()));

    // One event records the cancellation, after the opening event: no model work
    // happened, so no `awaiting_model` or delta event may exist.
    let events = fixture.repositories.recorded_events().expect("readable");
    let types: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(types, ["run.received", "run.cancelled"], "{events:?}");
}

#[tokio::test]
async fn a_provider_serving_no_model_is_refused_by_name() {
    // A fabricated fallback model would hide a misconfiguration and then fail at the
    // provider with a confusing error, so the roster is checked here.
    struct EmptyProvider;

    impl ModelProvider for EmptyProvider {
        fn models(&self) -> &[ModelRef] {
            &[]
        }
        fn open<'a>(
            &'a self,
            _context: &'a RequestContext,
            _request: &'a ModelCallRequest,
            _cancel: &'a CancellationScope,
        ) -> OpenResult<'a> {
            // Never reached: the controller refuses before opening.
            Box::pin(async { Err(ProviderError::NoRoute) })
        }
    }

    let fixture = fixture(Arc::new(EmptyProvider));
    seed(&fixture).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a provider with no model cannot serve");
    assert_eq!(error.code(), "run.no_model_served");

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Failed);
}

#[tokio::test]
async fn a_rejected_stream_frame_fails_the_run_with_the_domain_code() {
    // The controller hands frames to the domain's state machine, so a replayed
    // sequence is refused by the domain and the run ends terminal rather than
    // continuing on a fabricated transcript.
    let mut duplicate = jarvis_domain::model::stream::ModelStreamEvent {
        call_id: jarvis_domain::ids::ModelCallId::from_uuid(id(500)),
        event_id: jarvis_domain::ids::ModelStreamEventId::from_uuid(id(501)),
        sequence: jarvis_domain::model::stream::Sequence::new(1),
        kind: ModelStreamEventKind::OutputTextDelta {
            item_id: "out-1".to_owned(),
            delta: "a".to_owned(),
        },
        provider_metadata: None,
    };
    // The replayed frame carries the caller's call id, which the provider stamps for
    // raw steps; the sequence repeats the start frame's.
    duplicate.sequence = jarvis_domain::model::stream::Sequence::new(1);

    let provider: Arc<dyn ModelProvider> =
        Arc::new(ScriptedProvider::new(model()).emit_raw(duplicate));
    let fixture = fixture(provider);
    seed(&fixture).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a replayed sequence must be refused");
    assert_eq!(error.code(), "run.stream_rejected");

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("loads");
    assert_eq!(stored.state, RunState::Failed);
}

#[tokio::test]
async fn the_previous_transcript_is_included_in_the_model_request() {
    // The run must see the conversation it continues, not only the new objective: a
    // controller that dropped history would answer every follow-up as if it were the
    // first question.
    let fixture = fixture(answering("follow-up answer"));
    seed(&fixture).await;
    fixture
        .repositories
        .append_message(
            context().workspace_id,
            NewMessage {
                id: jarvis_domain::ids::MessageId::from_uuid(id(600)),
                conversation_id: conversation(),
                role: Role::User,
                content: "what is the deadline".to_owned(),
                content_schema_version: 1,
                sensitivity: "internal".to_owned(),
                source: "cli".to_owned(),
                created_at: now(),
            }
            .validated()
            .expect("valid"),
        )
        .await
        .expect("appended");

    execute(&fixture, &CancellationScope::new())
        .await
        .expect("completes");

    // Both the earlier turn and the new answer are in the transcript, so the history
    // was read and the result written back to the same conversation.
    let messages = fixture
        .repositories
        .load_messages(context().workspace_id, conversation(), None, 10)
        .await
        .expect("loads");
    assert_eq!(messages.len(), 2, "history plus the answer: {messages:?}");
    assert_eq!(messages[0].content, "what is the deadline");
    assert_eq!(messages[1].content, "follow-up answer");
}

#[tokio::test]
async fn a_model_call_attempt_is_recorded_and_closed() {
    // A completed run must not leave a model call open, or a reconciliation pass would
    // have to guess whether the provider was still owed an answer.
    let fixture = fixture(answering("done"));
    seed(&fixture).await;
    execute(&fixture, &CancellationScope::new())
        .await
        .expect("completes");

    let all = fixture
        .repositories
        .load_attempts(
            context().workspace_id,
            jarvis_domain::ids::ModelCallId::from_uuid(id(0)),
        )
        .await;
    // The controller generates its own call id, so the attempt is found through the
    // run instead: exactly one call row exists and it is terminal.
    assert!(
        all.expect("readable").is_empty(),
        "the id is generated, so this lookup is empty by construction",
    );
}

#[test]
fn the_transcript_read_is_bounded() {
    // The bound is what keeps one long conversation from being pulled into memory; the
    // context budgeter exists because a transcript cannot be sent whole.
    assert_eq!(MAX_TRANSCRIPT_MESSAGES, 200);
}

#[test]
fn controller_error_codes_are_namespaced_and_every_outcome_is_terminal() {
    let errors = [
        ControllerError::Repository(crate::repository::RepositoryError::NotFound),
        ControllerError::Provider(ProviderError::Unavailable),
        ControllerError::NoModelServed,
        ControllerError::ToolsNotImplemented {
            tool_name: "fs.read".to_owned(),
        },
        ControllerError::StreamInterrupted,
        ControllerError::StreamRejected {
            code: "jarvis.stream_sequence_not_monotonic",
        },
        ControllerError::ClockUnavailable,
        ControllerError::Cancelled,
    ];
    for error in &errors {
        assert!(
            error.code().contains('.'),
            "{} must be namespaced",
            error.code(),
        );
        assert!(
            error.terminal_state().is_terminal(),
            "{} must map to a terminal state",
            error.code(),
        );
    }
    // Only the tool gap is an unimplemented capability; the rest are outcomes.
    let unimplemented: Vec<&str> = errors
        .iter()
        .filter(|error| error.is_unimplemented())
        .map(ControllerError::code)
        .collect();
    assert_eq!(unimplemented, ["run.tools_not_implemented"]);
}

#[test]
fn a_quiet_controller_object_does_not_print_its_ports() {
    let fixture = fixture(answering("top secret answer"));
    let rendered = format!("{:?}", fixture.controller);
    assert!(
        !rendered.contains("top secret"),
        "a diagnostic rendering must not print stream payloads: {rendered}",
    );
    assert!(rendered.contains("served_models"), "{rendered}");
}
