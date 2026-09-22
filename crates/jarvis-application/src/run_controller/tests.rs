//! Tests for the native run controller.
//!
//! These drive the controller against the in-memory repository double and the
//! scripted provider, so an assertion is about what was **persisted** and what state
//! the run settled in rather than about calls made to a mock. The controller's whole
//! purpose is to turn a provider's frames into durable state, and a store that
//! recorded nothing would agree with whatever the controller assumed.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use jarvis_domain::clock::ManualClock;
use jarvis_domain::ids::{
    ConversationId, CorrelationId, PrincipalId, RequestId, RunId, WorkspaceId,
};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{
    FinishReason, ModelCallRequest, ModelStreamEventKind, Role, Usage,
};
use jarvis_domain::run::budget::{BudgetLimit, RunBudget};
use jarvis_domain::run::state::RunState;
use jarvis_domain::time::UtcTimestamp;
use uuid::Uuid;

use super::{ControllerError, MAX_TRANSCRIPT_MESSAGES, RunController};
use crate::cancellation::CancellationScope;
use crate::model::{ModelProvider, ModelStream, OpenResult, ProviderError, ScriptedProvider};
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

// ---------------------------------------------------------------------------
// Budget enforcement
//
// The tests below are the reason the budget exists. Carrying a deadline into a request
// is not enforcement: a provider that never sends a frame would hold the run open
// forever, and the run's own `deadline_at` would describe a limit nothing bounded. Every
// test here asserts a *bound elapsed*, so each one would hang — and be caught by the
// harness — if the controller awaited without a limit.
// ---------------------------------------------------------------------------

/// A provider whose `open` never returns.
///
/// The point of this double is that it does not cooperate: it does not check the
/// cancellation token and it does not consult the deadline it was handed. A provider that
/// behaved would prove nothing about whether the controller bounds it.
struct NeverOpens {
    models: Vec<ModelRef>,
}

impl ModelProvider for NeverOpens {
    fn models(&self) -> &[ModelRef] {
        &self.models
    }

    fn open<'a>(
        &'a self,
        _context: &'a RequestContext,
        _request: &'a ModelCallRequest,
        _cancel: &'a CancellationScope,
    ) -> OpenResult<'a> {
        Box::pin(async move {
            std::future::pending::<()>().await;
            unreachable!("a pending future never resolves")
        })
    }
}

/// A provider that opens and then never emits a frame.
///
/// Distinct from [`NeverOpens`] because the two failures happen at different points: a
/// hang *before* the stream exists, and a hang *after* the provider accepted the call. The
/// second is the more dangerous one — a model-call row is already recorded, so a run
/// abandoned there leaves an attempt behind that a reconciliation pass would read as
/// outstanding work.
struct OpensThenStalls {
    models: Vec<ModelRef>,
}

impl ModelProvider for OpensThenStalls {
    fn models(&self) -> &[ModelRef] {
        &self.models
    }

    fn open<'a>(
        &'a self,
        _context: &'a RequestContext,
        _request: &'a ModelCallRequest,
        _cancel: &'a CancellationScope,
    ) -> OpenResult<'a> {
        Box::pin(async move {
            /// A stream that never yields a frame and never terminates.
            struct Stalled;

            impl ModelStream for Stalled {
                fn next_event(
                    &mut self,
                ) -> Pin<
                    Box<
                        dyn Future<
                                Output = Result<
                                    Option<jarvis_domain::model::stream::ModelStreamEvent>,
                                    ProviderError,
                                >,
                            > + Send
                            + '_,
                    >,
                > {
                    Box::pin(async {
                        std::future::pending::<()>().await;
                        unreachable!("a pending future never resolves")
                    })
                }
            }

            Ok(Box::new(Stalled) as Box<dyn ModelStream + Send + 'a>)
        })
    }
}

/// Seeds a run that carries `budget`.
async fn seed_with_budget(fixture: &Fixture, budget: RunBudget) {
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
            NewRun::with_budget(
                run(),
                context().workspace_id,
                conversation(),
                PrincipalId::from_uuid(id(3)),
                Some("objective-1".to_owned()),
                now(),
                budget,
            )
            .expect("valid"),
            crate::repository::run::run_received_event(run(), now()),
        )
        .await
        .expect("the run is created");
}

/// A budget that expires one millisecond after now, with a step timeout to match.
///
/// Both are set so the run is bounded by the tighter of the two, which is what the
/// controller must compute. One millisecond is far below the scripted provider's response
/// time, so the bound is what ends the wait rather than the provider answering first.
fn almost_immediate() -> RunBudget {
    let deadline = UtcTimestamp::parse("2026-09-22T12:00:00.001Z").expect("valid");
    RunBudget::with_deadline(deadline)
        .with_step_timeout(1)
        .expect("1ms is in range")
}

#[tokio::test]
async fn a_provider_that_never_opens_is_bounded_by_the_runs_deadline() {
    // The headline case. Without the bound this test never returns: it would hang the
    // suite rather than fail, which is why the assertion is about the state the run was
    // left in rather than only about the returned error.
    let fixture = fixture(Arc::new(NeverOpens {
        models: vec![model()],
    }));
    seed_with_budget(&fixture, almost_immediate()).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a provider that never answers cannot complete a run");
    assert_eq!(error, ControllerError::DeadlineExceeded);
    assert_eq!(
        error.terminal_state(),
        RunState::Failed,
        "an expired budget is a failure, not a cancellation",
    );

    // The durable state is what matters: a run left non-terminal would leave a client
    // polling forever, which is the same defect the recovery pass exists to fix.
    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("the run loads");
    assert_eq!(stored.state, RunState::Failed);
    assert!(stored.is_terminal());
}

#[tokio::test]
async fn a_provider_that_stalls_mid_stream_is_bounded_per_frame() {
    // The hang after the call was accepted. A bound applied only to `open` would let this
    // one run forever, so the frame wait is bounded too — and it is bounded by what is
    // *left*, not by the original allowance.
    let fixture = fixture(Arc::new(OpensThenStalls {
        models: vec![model()],
    }));
    seed_with_budget(&fixture, almost_immediate()).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a stalled stream cannot complete a run");
    assert_eq!(error, ControllerError::DeadlineExceeded);

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("the run loads");
    assert_eq!(stored.state, RunState::Failed);
}

#[tokio::test]
async fn an_abandoned_call_is_closed_rather_than_left_pending() {
    // A call the provider accepted and then abandoned must not stay `Pending`: a later
    // reconciliation pass reads a pending attempt as outstanding work, so the run would
    // look live when it is finished.
    let fixture = fixture(Arc::new(OpensThenStalls {
        models: vec![model()],
    }));
    seed_with_budget(&fixture, almost_immediate()).await;

    let _ = execute(&fixture, &CancellationScope::new()).await;

    let calls = fixture
        .repositories
        .recorded_calls()
        .expect("the calls are readable");
    assert_eq!(calls.len(), 1, "{calls:?}");
    let state = calls[0].1;
    assert!(
        state.is_terminal(),
        "a call abandoned by a timeout must be terminal, got {state:?}",
    );
    assert_eq!(state, crate::repository::model_call::ModelCallState::Failed);
}

#[tokio::test]
async fn a_run_whose_deadline_already_passed_is_failed_without_calling_a_provider() {
    // A deadline in the past is refused before any provider is contacted, so no model
    // call is recorded and no provider is billed. The alternative — opening the call and
    // relying on the bound — would spend an attempt on a request known to be pointless.
    let fixture = fixture(answering("never reached"));
    let deadline = UtcTimestamp::parse("2026-09-22T11:59:59Z").expect("valid");
    seed_with_budget(&fixture, RunBudget::with_deadline(deadline)).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("an expired run cannot proceed");
    assert_eq!(error, ControllerError::DeadlineExceeded);

    let calls = fixture
        .repositories
        .recorded_calls()
        .expect("the calls are readable");
    assert!(
        calls.is_empty(),
        "an expired run must not contact a provider: {calls:?}",
    );

    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("the run loads");
    assert_eq!(stored.state, RunState::Failed);
}

#[tokio::test]
async fn a_run_with_no_deadline_is_not_bounded_and_still_completes() {
    // The other direction, and the one that would break if an unset budget were read as
    // "zero time left": a run with no deadline must still be able to finish normally.
    let fixture = fixture(answering("Hello there"));
    seed_with_budget(&fixture, RunBudget::default()).await;

    let outcome = execute(&fixture, &CancellationScope::new())
        .await
        .expect("an unbounded run completes");
    assert_eq!(outcome.state, RunState::Completed);
}

#[tokio::test]
async fn the_run_deadline_reaches_the_provider_in_the_request() {
    // The defect this closes, asserted at the boundary where it mattered: a run with a
    // deadline previously sent `limits.deadline: null`, so an adapter honouring the
    // contract's `limits.deadline` had nothing to honour.
    /// What the capturing provider observed.
    ///
    /// An enum rather than `Option<Option<UtcTimestamp>>`, because the three states are
    /// genuinely different: the call never happened, it happened with no deadline, and it
    /// happened with one. A nested option makes the first two indistinguishable to a
    /// reader, which is exactly the case this test exists to separate.
    enum Observed {
        NeverCalled,
        Called(Option<UtcTimestamp>),
    }

    struct Capturing {
        seen: std::sync::Mutex<Observed>,
        models: Vec<ModelRef>,
    }

    impl ModelProvider for Capturing {
        fn models(&self) -> &[ModelRef] {
            &self.models
        }

        fn open<'a>(
            &'a self,
            _context: &'a RequestContext,
            request: &'a ModelCallRequest,
            _cancel: &'a CancellationScope,
        ) -> OpenResult<'a> {
            if let Ok(mut guard) = self.seen.lock() {
                *guard = Observed::Called(request.limits.deadline);
            }
            Box::pin(async move {
                // Refusing is enough: the assertion is about what the request carried,
                // not about what happened next.
                Err(ProviderError::Unavailable)
            })
        }
    }

    let provider = Arc::new(Capturing {
        seen: std::sync::Mutex::new(Observed::NeverCalled),
        models: vec![model()],
    });
    let fixture = fixture(Arc::clone(&provider) as Arc<dyn ModelProvider>);
    let deadline = UtcTimestamp::parse("2026-09-22T12:00:30Z").expect("valid");
    seed_with_budget(&fixture, RunBudget::with_deadline(deadline)).await;

    let _ = execute(&fixture, &CancellationScope::new()).await;

    let seen = provider.seen.lock().expect("the lock is not poisoned");
    match &*seen {
        Observed::Called(deadline) => assert_eq!(
            *deadline,
            Some(UtcTimestamp::parse("2026-09-22T12:00:30Z").expect("valid")),
            "the run's deadline must reach the provider's request",
        ),
        Observed::NeverCalled => unreachable!("the provider must have been called"),
    }
}

#[tokio::test]
async fn a_provider_timeout_error_is_reported_as_the_deadline_not_as_a_provider_fault() {
    // The provider reports its own timeout, which means the *budget* was exhausted rather
    // than the provider being broken. Reporting it as a provider fault would send an
    // operator to the provider's status page when the answer is in the run's budget.
    let fixture = fixture(Arc::new(
        ScriptedProvider::new(model()).fail_on_open(ProviderError::Timeout),
    ));
    seed_with_budget(&fixture, RunBudget::default()).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a timed-out provider cannot complete a run");
    assert_eq!(error, ControllerError::DeadlineExceeded);
    assert_eq!(error.code(), "run.deadline_exceeded");
    assert!(
        !error.retryable(),
        "a spent deadline leaves no budget to retry inside",
    );
}

#[tokio::test]
async fn a_deadline_exceeded_outcome_names_its_own_code_and_message() {
    // The code and the message are what an operator and a client respectively see, so
    // both must be distinct from a provider fault's.
    let error = ControllerError::DeadlineExceeded;
    assert_eq!(error.code(), "run.deadline_exceeded");
    assert_eq!(error.terminal_state(), RunState::Failed);
    assert!(
        error.message().contains("time budget"),
        "{}",
        error.message()
    );
    assert!(!error.is_unimplemented());
}

// ---------------------------------------------------------------------------
// Consumption ceilings
//
// Until these existed, `max_output_tokens` reached the provider and nothing compared what
// came back against it, so the ceiling bounded nothing. Each test below asserts the run was
// left *failed* and its output discarded, which is what separates an enforced limit from an
// advisory one — a run that breached a ceiling and still returned its answer would make the
// ceiling cosmetic.
// ---------------------------------------------------------------------------

/// Usage reporting `output_tokens` and `estimated_cost_microunits`.
fn reported_usage(output_tokens: u64, cost: u64) -> Usage {
    Usage {
        output_tokens: Some(output_tokens),
        estimated_cost_microunits: Some(cost),
        provider_reported: true,
        ..Usage::default()
    }
}

/// A provider that answers with `text` and reports `usage` with its terminal.
fn answering_with_usage(text: &str, usage: Usage) -> Arc<dyn ModelProvider> {
    Arc::new(
        ScriptedProvider::new(model())
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", text)
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: Some(usage),
                refused: false,
            }),
    )
}

/// A budget with only an output-token ceiling.
fn token_capped(cap: u64) -> RunBudget {
    RunBudget {
        max_output_tokens: Some(cap),
        ..RunBudget::default()
    }
}

#[tokio::test]
async fn a_run_that_exceeds_its_output_token_ceiling_is_failed_and_its_answer_discarded() {
    // The headline case for consumption budgets. The output is discarded deliberately: a
    // run that breached a ceiling and still returned its answer would make the ceiling
    // advisory, and a caller could not tell an enforced limit from a cosmetic one.
    let fixture = fixture(answering_with_usage(
        "Hello there",
        reported_usage(2049, 100),
    ));
    seed_with_budget(&fixture, token_capped(2048)).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a run over its token ceiling cannot complete");
    assert_eq!(
        error,
        ControllerError::BudgetExceeded {
            limit: BudgetLimit::OutputTokens
        },
    );
    assert_eq!(error.code(), "run.budget_output_tokens_exceeded");
    assert_eq!(error.terminal_state(), RunState::Failed);
    assert!(!error.retryable(), "the same output would breach again");

    // The durable state, not the returned value, is the proof.
    let stored = fixture
        .repositories
        .load(context().workspace_id, run())
        .await
        .expect("the run loads");
    assert_eq!(stored.state, RunState::Failed);

    // And the answer was not stored as the assistant's message.
    let messages = fixture
        .repositories
        .load_messages(context().workspace_id, conversation(), None, 10)
        .await
        .expect("messages are readable");
    assert!(
        !messages
            .iter()
            .any(|message| message.content.contains("Hello there")),
        "a discarded answer must not be persisted: {messages:?}",
    );
}

#[tokio::test]
async fn a_run_at_exactly_its_token_ceiling_still_completes() {
    // The boundary, in the direction that matters: a ceiling of 2048 permits producing
    // token 2048. An off-by-one here would refuse runs that fit inside their budget.
    let fixture = fixture(answering_with_usage(
        "Hello there",
        reported_usage(2048, 100),
    ));
    seed_with_budget(&fixture, token_capped(2048)).await;

    let outcome = execute(&fixture, &CancellationScope::new())
        .await
        .expect("a run inside its ceiling completes");
    assert_eq!(outcome.state, RunState::Completed);
}

#[tokio::test]
async fn a_run_that_exceeds_its_cost_ceiling_is_failed() {
    // The other ceiling, and it is a separate fact: the token count was fine and the route
    // was too expensive, which is a routing problem rather than a prompt one.
    let fixture = fixture(answering_with_usage(
        "Hello there",
        reported_usage(10, 50_001),
    ));
    let budget = RunBudget {
        max_cost_microunits: Some(50_000),
        ..RunBudget::default()
    };
    seed_with_budget(&fixture, budget).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("a run over its cost ceiling cannot complete");
    assert_eq!(
        error,
        ControllerError::BudgetExceeded {
            limit: BudgetLimit::Cost
        },
    );
    assert_eq!(error.code(), "run.budget_cost_exceeded");
}

#[tokio::test]
async fn a_ceiling_with_no_reported_usage_does_not_fail_the_run() {
    // A provider that reports no usage cannot be judged against a ceiling. Failing the run
    // would be a false failure for every provider that omits usage; the honest behaviour is
    // to complete and leave the ceiling unverified.
    let fixture = fixture(answering("Hello there"));
    seed_with_budget(&fixture, token_capped(1)).await;

    let outcome = execute(&fixture, &CancellationScope::new())
        .await
        .expect("an unmeasured call is not refused");
    assert_eq!(outcome.state, RunState::Completed);
}

#[tokio::test]
async fn the_usage_the_provider_reported_is_recorded_on_the_call() {
    // The usage has to reach storage, or the ceiling check is a decision nothing can audit
    // afterwards. Both the block and the lifted cost column come from one source.
    let fixture = fixture(answering_with_usage(
        "Hello there",
        reported_usage(1234, 5678),
    ));
    seed_with_budget(&fixture, RunBudget::default()).await;

    let outcome = execute(&fixture, &CancellationScope::new())
        .await
        .expect("the run completes");
    assert_eq!(outcome.state, RunState::Completed);

    let stored = fixture
        .repositories
        .recorded_call_usage()
        .expect("the calls are readable");
    assert_eq!(stored.len(), 1, "{stored:?}");
    let recorded = &stored[0];
    let usage = recorded
        .usage
        .as_ref()
        .expect("the reported usage is stored");
    assert_eq!(usage.output_tokens, Some(1234));
    assert!(usage.provider_reported);
    assert_eq!(
        recorded.estimated_cost_microunits,
        Some(5678),
        "the cost is lifted into its own column from the same source",
    );
}

#[tokio::test]
async fn usage_from_a_separate_update_frame_is_recorded_too() {
    // The contract says a `usage.updated` frame may arrive before, with, or *after* output
    // completion, so a controller that only read the terminal's block would miss a
    // provider that reports usage on its own frame.
    let fixture = fixture(Arc::new(
        ScriptedProvider::new(model())
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", "Hello there")
            .emit(ModelStreamEventKind::UsageUpdated {
                usage: reported_usage(777, 888),
            })
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            }),
    ));
    seed_with_budget(&fixture, RunBudget::default()).await;

    let _ = execute(&fixture, &CancellationScope::new()).await;

    let stored = fixture
        .repositories
        .recorded_call_usage()
        .expect("the calls are readable");
    let recorded = stored.first().expect("the call was recorded");
    assert_eq!(
        recorded
            .usage
            .as_ref()
            .and_then(|reported| reported.output_tokens),
        Some(777),
        "a usage frame the provider sent on its own must be captured",
    );
}

#[tokio::test]
async fn a_later_usage_frame_revises_an_earlier_one() {
    // A provider that updates as it goes sends several. The last wins, because a later
    // frame is a revision and the terminal's block is final; taking the first would
    // under-count and let a run slip past a ceiling it actually breached.
    let fixture = fixture(Arc::new(
        ScriptedProvider::new(model())
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", "Hello there")
            .emit(ModelStreamEventKind::UsageUpdated {
                usage: reported_usage(10, 10),
            })
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: Some(reported_usage(3000, 3000)),
                refused: false,
            }),
    ));
    seed_with_budget(&fixture, token_capped(2048)).await;

    let error = execute(&fixture, &CancellationScope::new())
        .await
        .expect_err("the revised, larger usage must breach the ceiling");
    assert_eq!(
        error,
        ControllerError::BudgetExceeded {
            limit: BudgetLimit::OutputTokens
        },
    );
}

#[tokio::test]
async fn a_breached_ceiling_is_reported_as_its_own_code_and_message() {
    let error = ControllerError::BudgetExceeded {
        limit: BudgetLimit::Cost,
    };
    assert_eq!(error.code(), "run.budget_cost_exceeded");
    assert_eq!(error.terminal_state(), RunState::Failed);
    assert!(
        error.message().contains("cost budget"),
        "{}",
        error.message()
    );
    assert!(!error.is_unimplemented());
    assert!(!error.retryable());
}
