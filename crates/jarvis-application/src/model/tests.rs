//! Tests for the model provider port and the scripted provider.
//!
//! The scripted provider is the only provider available before a real adapter
//! exists, so these tests are what makes the port's contract falsifiable today:
//! the scenarios are the ones `docs/planning/first-vertical-slice.md` lists under
//! "Scripted Provider Scenarios", and each asserts the *normalized* result rather
//! than the provider's own payloads.

use std::sync::Arc;

use jarvis_domain::ids::{CorrelationId, IdGenerator, PrincipalId, RequestId, WorkspaceId};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{
    CallLimits, FinishReason, InputItems, ModelCallRequest, ModelStreamEvent, ModelStreamEventKind,
    ModelStreamState, RouteRequirements, Sequence, StreamAdmission, StreamOutcome, ToolArguments,
    Usage,
};
use uuid::Uuid;

use super::{
    AdapterStream, CountingIds, ModelProvider, ModelStream, ProviderError, ScriptedProvider,
};
use crate::cancellation::CancellationScope;
use crate::request_context::{AuthenticationAssurance, RequestChannel, RequestContext};

const CALL: &str = "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d";

fn model() -> ModelRef {
    ModelRef::new(
        ProviderId::parse("scripted.local").expect("valid provider"),
        ModelId::parse("fixture-1").expect("valid model"),
    )
}

fn context() -> RequestContext {
    RequestContext::new(
        RequestId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5f").expect("valid"),
        CorrelationId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c60").expect("valid"),
        PrincipalId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5e").expect("valid"),
        AuthenticationAssurance::Standard,
        WorkspaceId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c61").expect("valid"),
        RequestChannel::Cli,
    )
}

fn request() -> ModelCallRequest {
    ModelCallRequest {
        call_id: jarvis_domain::ids::ModelCallId::parse(CALL).expect("valid"),
        run_id: jarvis_domain::ids::RunId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c62")
            .expect("valid"),
        route_requirements: RouteRequirements::text(),
        input: InputItems::new(Vec::new()).expect("an empty list is valid"),
        tools: Vec::new(),
        output_schema: None,
        settings: jarvis_domain::model::stream::PortableSettings::default(),
        limits: CallLimits {
            deadline: None,
            max_output_tokens: Some(512),
            max_cost_microunits: None,
        },
    }
}

/// A provider whose identifiers are a fixed sequence, so frames are reproducible.
fn deterministic() -> ScriptedProvider {
    ScriptedProvider::new(model()).with_ids(Arc::new(CountingIds::new()))
}

async fn drain(
    provider: &ScriptedProvider,
    request: &ModelCallRequest,
    cancel: &CancellationScope,
) -> Result<Vec<ModelStreamEvent>, ProviderError> {
    let context = context();
    let mut stream = provider.open(&context, request, cancel).await?;
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await? {
        events.push(event);
    }
    Ok(events)
}

#[test]
fn provider_error_codes_are_unique_and_namespaced() {
    let errors = [
        ProviderError::Authentication,
        ProviderError::RateLimited {
            retry_after_ms: Some(1_000),
        },
        ProviderError::Unavailable,
        ProviderError::InvalidRequest,
        ProviderError::Refused,
        ProviderError::Malformed,
        ProviderError::NoRoute,
        ProviderError::Cancelled,
    ];
    let mut codes: Vec<&str> = errors.iter().map(|error| error.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), errors.len(), "codes must be unique");
    for code in codes {
        assert!(
            code.starts_with("model."),
            "{code} must be namespaced for a model call",
        );
    }
}

#[test]
fn only_transient_failures_are_retryable() {
    // Retryability is decided in exactly one place. A blind retry of a refusal,
    // an invalid request, or a rejected credential cannot succeed, and retrying a
    // cancellation does the opposite of what the caller asked.
    assert!(ProviderError::Unavailable.retryable());
    assert!(
        ProviderError::RateLimited {
            retry_after_ms: None,
        }
        .retryable(),
    );
    for error in [
        ProviderError::Authentication,
        ProviderError::InvalidRequest,
        ProviderError::Refused,
        ProviderError::Malformed,
        ProviderError::NoRoute,
        ProviderError::Cancelled,
    ] {
        assert!(!error.retryable(), "{} must not be retryable", error.code());
    }
}

#[test]
fn a_scripted_stream_is_reproducible_frame_for_frame() {
    // Two runs of the same script under the same identifier generator must produce
    // identical frames. Without this, "deterministic" would only mean "the text
    // matched" while the event identifiers drifted on every run.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let build = || {
        deterministic()
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", "Hel")
            .emit_text("out-1", "lo")
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            })
    };
    let first = runtime
        .block_on(drain(&build(), &request(), &CancellationScope::new()))
        .expect("the stream opens");
    let second = runtime
        .block_on(drain(&build(), &request(), &CancellationScope::new()))
        .expect("the stream opens");
    assert_eq!(
        first, second,
        "the same script must produce the same frames"
    );
    assert!(!first.is_empty());
}

#[test]
fn the_first_frame_is_call_started_and_the_sequence_starts_at_one() {
    // 0 is the stream state's "nothing seen yet" cursor, so a provider that
    // numbered its first frame 0 would be refused by the state machine before any
    // payload was seen. This test pins the provider's numbering at the boundary.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(drain(
            &deterministic().emit_text("out-1", "hi"),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    assert_eq!(events[0].sequence, Sequence::new(1));
    assert!(
        matches!(
            events[0].kind,
            ModelStreamEventKind::CallStarted { model: Some(_) },
        ),
        "the provider must report the model it selected",
    );
    for pair in events.windows(2) {
        assert!(
            pair[1].sequence > pair[0].sequence,
            "the provider must number frames monotonically",
        );
    }
}

#[test]
fn a_normalized_stream_satisfies_the_domain_state_machine() {
    // The port and the domain must agree: a stream the provider produces has to be
    // accepted by `ModelStreamState` unchanged, or the two halves of the contract
    // would each be internally consistent and jointly wrong.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(drain(
            &deterministic()
                .emit(ModelStreamEventKind::OutputItemAdded {
                    item_id: "out-1".to_owned(),
                })
                .emit_text("out-1", "Hello")
                .emit(ModelStreamEventKind::OutputItemCompleted {
                    item_id: "out-1".to_owned(),
                })
                .emit(ModelStreamEventKind::CallCompleted {
                    finish_reason: FinishReason::Stop,
                    usage: Some(Usage {
                        output_tokens: Some(1),
                        provider_reported: true,
                        ..Usage::default()
                    }),
                    refused: false,
                }),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    let mut state = ModelStreamState::new(request().call_id);
    for event in &events {
        assert_eq!(
            state.accept(event).expect("the domain accepts the frame"),
            StreamAdmission::Accepted,
        );
    }
    assert_eq!(
        state.finish().expect("no unfinished tool call"),
        StreamOutcome::Terminal(FinishReason::Stop),
    );
}

#[test]
fn the_trusted_request_identity_reaches_the_adapter() {
    // A provider's request id correlates a provider-side record with a JARVIS
    // request. Asserting that the scripted provider echoes the *server-derived*
    // request id is what makes "the trusted context reached the adapter"
    // observable rather than claimed.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(drain(
            &deterministic().emit_text("out-1", "hi"),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    let metadata = events[0]
        .provider_metadata
        .as_ref()
        .expect("the start frame carries provider metadata");
    assert_eq!(
        metadata.request_id.as_deref(),
        Some(context().request_id.to_string().as_str()),
    );
}

#[test]
fn a_script_without_a_terminal_is_interrupted_not_completed() {
    // A provider that disconnects mid-answer must not be reported as a success.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(drain(
            &deterministic().emit_text("out-1", "partial").interrupt(),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    let mut state = ModelStreamState::new(request().call_id);
    for event in &events {
        state.accept(event).expect("each frame is accepted");
    }
    let outcome = state.finish().expect("no unfinished tool call");
    assert!(!outcome.is_terminal());
    assert_eq!(outcome, StreamOutcome::Interrupted { accepted: 2 });
}

#[test]
fn cancellation_closes_the_stream_with_a_terminal_event_not_a_silent_stop() {
    // The distinction under test: a cancelled call ends with `call.cancelled`, so
    // the run is recorded as cancelled. A bare stop would look identical to a
    // provider that died and would be recorded as an interrupted fault instead.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let cancel = CancellationScope::new();
    let provider =
        deterministic()
            .emit_text("out-1", "never")
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            });

    let events = runtime.block_on(async {
        let context = context();
        let request = request();
        let mut stream = provider
            .open(&context, &request, &cancel)
            .await
            .expect("the stream opens");
        // Take the start frame, then cancel before the next read.
        let mut events = Vec::new();
        events.push(
            stream
                .next_event()
                .await
                .expect("readable")
                .expect("a frame"),
        );
        cancel.cancel();
        while let Some(event) = stream.next_event().await.expect("readable") {
            events.push(event);
        }
        events
    });

    let mut state = ModelStreamState::new(request().call_id);
    for event in &events {
        state.accept(event).expect("each frame is accepted");
    }
    assert_eq!(
        state.finish().expect("no unfinished tool call"),
        StreamOutcome::Terminal(FinishReason::Cancelled),
        "a cancelled stream must end `call.cancelled`",
    );
    let last = events.last().expect("a terminal frame arrived");
    assert!(
        matches!(last.kind, ModelStreamEventKind::CallCancelled { .. },),
        "the terminal frame must be the cancellation, not the script's completion",
    );
}

#[test]
fn a_cancelled_scope_refuses_to_open_rather_than_returning_an_empty_stream() {
    // An empty stream would be read downstream as an interrupted call — a fault —
    // when the accurate answer is that the caller asked to stop.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let cancel = CancellationScope::new();
    cancel.cancel();

    let error = runtime
        .block_on(drain(
            &deterministic().emit_text("out-1", "hi"),
            &request(),
            &cancel,
        ))
        .expect_err("a cancelled open must fail");
    assert_eq!(error, ProviderError::Cancelled);
}

#[test]
fn frames_after_a_terminal_are_still_delivered_to_the_state_machine() {
    // The contract requires late frames to be ignored *and counted* by the state
    // machine, so the transport must not swallow them. Swallowing them would move
    // the decision into the adapter and hide a provider that kept talking.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(drain(
            &deterministic()
                .emit(ModelStreamEventKind::CallCompleted {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    refused: false,
                })
                .emit_text("out-1", "late"),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    assert_eq!(events.len(), 3);
    let mut state = ModelStreamState::new(request().call_id);
    state.accept(&events[0]).expect("the start is accepted");
    state.accept(&events[1]).expect("the terminal is accepted");
    assert_eq!(
        state
            .accept(&events[2])
            .expect("the late frame is admitted"),
        StreamAdmission::IgnoredAfterTerminal,
    );
    assert_eq!(state.late_frames_ignored(), 1);
}

#[test]
fn a_scripted_duplicate_sequence_is_refused_by_the_state_machine() {
    // The negative case a well-behaved provider can never produce, which is the
    // entire reason `ScriptStep::Raw` exists.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let mut duplicate = request_row_delta(2, "a");
    duplicate.sequence = Sequence::new(1);

    let events = runtime
        .block_on(drain(
            &deterministic().emit_raw(duplicate),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    let mut state = ModelStreamState::new(request().call_id);
    state.accept(&events[0]).expect("the start is accepted");
    let error = state
        .accept(&events[1])
        .expect_err("a replayed sequence must be refused");
    assert_eq!(error.code(), "jarvis.stream_sequence_not_monotonic");
}

#[test]
fn a_scripted_frame_for_another_call_is_refused_by_the_state_machine() {
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let mut foreign = request_row_delta(2, "a");
    foreign.call_id =
        jarvis_domain::ids::ModelCallId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c63")
            .expect("valid");

    let events = runtime
        .block_on(drain(
            &deterministic().emit_raw(foreign),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    let mut state = ModelStreamState::new(request().call_id);
    state.accept(&events[0]).expect("the start is accepted");
    let error = state
        .accept(&events[1])
        .expect_err("a frame for another call must be refused");
    assert_eq!(error.code(), "jarvis.stream_call_mismatch");
}

#[test]
fn a_raw_frame_is_still_assigned_a_fresh_identifier() {
    // An identifier is JARVIS's to assign. A raw step lets a test choose a
    // sequence, but a duplicated event identifier would make two frames
    // indistinguishable in an audit, so it is overwritten.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(drain(
            &deterministic().emit_raw(request_row_delta(2, "a")),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    let identifiers: Vec<_> = events.iter().map(|event| event.event_id).collect();
    let mut unique = identifiers.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        identifiers.len(),
        "identifiers must be unique"
    );
}

#[test]
fn an_unfinished_tool_call_cannot_be_finished_downstream() {
    // The scripted provider is what lets the *tool* half of the stream contract be
    // exercised before the tool fabric exists: a call added and never completed
    // must make `finish` refuse rather than hand out a half-assembled argument
    // string.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(drain(
            &deterministic()
                .emit(ModelStreamEventKind::ToolCallAdded {
                    call_id: "call-1".to_owned(),
                    tool_name: "fs.read".to_owned(),
                })
                .emit(ModelStreamEventKind::ToolCallArgumentsDelta {
                    call_id: "call-1".to_owned(),
                    delta: "{\"path\":".to_owned(),
                })
                .emit(ModelStreamEventKind::CallCompleted {
                    finish_reason: FinishReason::Stop,
                    usage: None,
                    refused: false,
                }),
            &request(),
            &CancellationScope::new(),
        ))
        .expect("the stream opens");

    let mut state = ModelStreamState::new(request().call_id);
    for event in &events {
        state.accept(event).expect("each frame is accepted");
    }
    assert_eq!(state.open_tool_call_ids(), ["call-1"]);
    let error = state
        .finish()
        .expect_err("an unfinished tool call must refuse to close the stream");
    assert_eq!(error.code(), "jarvis.unfinished_tool_call");

    // The argument string never became executable, which is the point of keeping
    // the unfinished state separate rather than reporting what arrived so far.
    assert_eq!(
        ToolArguments::Streaming {
            raw: "{\"path\":".to_owned(),
        }
        .executable_raw(),
        None,
    );
}

#[test]
fn the_counting_generator_produces_distinct_non_nil_identifiers() {
    let ids = CountingIds::new();
    let first = ids.next_uuid("model_stream_event");
    let second = ids.next_uuid("model_stream_event");
    assert_ne!(first, second);
    assert!(!first.is_nil(), "a nil identifier is not a usable frame id");
    assert_ne!(Uuid::from_u128(1), Uuid::nil());
}

#[tokio::test]
async fn an_adapter_stream_delivers_frames_then_ends_without_a_terminal() {
    // The shared implementation a real transport-backed adapter uses. A channel
    // that closes with no terminal must end the stream with `None`, which the
    // state machine records as *interrupted* — the accurate answer for a transport
    // that died, and never a fabricated completion.
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let cancel = CancellationScope::new();
    let ids: Arc<dyn IdGenerator> = Arc::new(CountingIds::new());
    let mut stream = AdapterStream::new(request().call_id, ids, cancel, receiver);

    sender
        .send(fixed_frame(
            1,
            ModelStreamEventKind::OutputTextDelta {
                item_id: "out-1".to_owned(),
                delta: "hi".to_owned(),
            },
        ))
        .await
        .expect("the channel accepts the frame");
    drop(sender);

    let first = stream
        .next_event()
        .await
        .expect("readable")
        .expect("a frame arrives");
    assert_eq!(first.sequence, Sequence::new(1));
    assert_eq!(
        stream.next_event().await.expect("readable"),
        None,
        "a closed channel with no terminal ends the stream",
    );
}

#[tokio::test]
async fn an_adapter_stream_reports_cancellation_while_waiting_for_a_frame() {
    // The defect this type exists to prevent: an adapter that observed
    // cancellation only between frames would hang a cancelled call whose provider
    // had gone quiet. `select!` observes the scope while the read is pending.
    let (sender, receiver) = tokio::sync::mpsc::channel::<ModelStreamEvent>(4);
    let cancel = CancellationScope::new();
    let ids: Arc<dyn IdGenerator> = Arc::new(CountingIds::new());
    let mut stream = AdapterStream::new(request().call_id, ids, cancel.clone(), receiver);

    // Cancel with the provider silent: nothing will ever be sent.
    let cancel_later = cancel.clone();
    tokio::spawn(async move {
        cancel_later.cancel();
    });

    let event = stream
        .next_event()
        .await
        .expect("readable")
        .expect("a cancellation frame is produced");
    assert!(
        matches!(event.kind, ModelStreamEventKind::CallCancelled { .. }),
        "a cancelled call must end `call.cancelled`, not hang",
    );
    assert_eq!(
        stream.next_event().await.expect("readable"),
        None,
        "the stream ends after the single terminal",
    );
    drop(sender);
}

#[tokio::test]
async fn an_adapter_stream_passes_an_adapters_own_terminal_through_once() {
    // A provider that reported its own outcome keeps it: the wrapper may not
    // replace a provider's `call.failed` with its own view.
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let cancel = CancellationScope::new();
    let ids: Arc<dyn IdGenerator> = Arc::new(CountingIds::new());
    let mut stream = AdapterStream::new(request().call_id, ids, cancel, receiver);

    sender
        .send(fixed_frame(
            1,
            ModelStreamEventKind::CallFailed {
                code: "model.provider_unavailable".to_owned(),
                retryable: true,
            },
        ))
        .await
        .expect("the channel accepts the frame");
    // A late frame after the terminal must not be re-read as a second terminal.
    sender
        .send(fixed_frame(
            2,
            ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            },
        ))
        .await
        .expect("the channel accepts the frame");
    drop(sender);

    let first = stream
        .next_event()
        .await
        .expect("readable")
        .expect("a frame arrives");
    assert!(matches!(
        first.kind,
        ModelStreamEventKind::CallFailed { .. },
    ));
    assert_eq!(
        stream.next_event().await.expect("readable"),
        None,
        "exactly one terminal is produced by the stream type itself",
    );
}

/// Builds a canonical frame without going through a provider.
fn fixed_frame(sequence: u64, kind: ModelStreamEventKind) -> ModelStreamEvent {
    ModelStreamEvent {
        call_id: jarvis_domain::ids::ModelCallId::parse(CALL).expect("valid"),
        event_id: jarvis_domain::ids::ModelStreamEventId::from_uuid(Uuid::from_u128(
            100_000 + u128::from(sequence),
        )),
        sequence: Sequence::new(sequence),
        kind,
        provider_metadata: None,
    }
}

#[test]
fn a_provider_that_fails_to_open_reports_the_mapped_error() {
    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    for error in [
        ProviderError::Authentication,
        ProviderError::Unavailable,
        ProviderError::Refused,
        ProviderError::NoRoute,
    ] {
        let provider = deterministic().fail_on_open(error);
        let got = runtime
            .block_on(drain(&provider, &request(), &CancellationScope::new()))
            .expect_err("the open must fail");
        assert_eq!(got, error);
        assert_eq!(got.code(), error.code());
    }
}

#[test]
fn serving_multiple_models_does_not_grant_anything_but_is_discoverable() {
    let second = ModelRef::new(
        ProviderId::parse("scripted.local").expect("valid"),
        ModelId::parse("fixture-2").expect("valid"),
    );
    let provider = deterministic().also_serving(second.clone());
    assert_eq!(provider.models().len(), 2);
    assert!(provider.models().contains(&model()));
    assert!(provider.models().contains(&second));
}

#[test]
fn a_quiet_provider_object_does_not_print_its_script() {
    // A script can carry prompt-derived model output, so a diagnostic rendering
    // must not duplicate it. The count is enough to debug a stalled stream.
    let provider = deterministic().emit_text("out-1", "top secret output");
    let rendered = format!("{provider:?}");
    assert!(
        !rendered.contains("top secret output"),
        "the debug rendering must not print stream payloads: {rendered}",
    );
    assert!(rendered.contains("script_steps"), "{rendered}");
}

#[test]
fn an_adapter_can_implement_the_port_without_this_modules_internals() {
    // The reason `ModelStream` is a trait: an adapter lives in another crate, so a
    // port returning a same-crate struct with a private buffer could only ever be
    // satisfied by the scripted provider — the signature would look general while
    // `BRN-003` could not implement it. This adapter is defined here, outside the
    // module, and uses only the public surface.
    struct MinimalAdapter {
        served: Vec<ModelRef>,
    }

    impl ModelProvider for MinimalAdapter {
        fn models(&self) -> &[ModelRef] {
            &self.served
        }

        fn open<'a>(
            &'a self,
            _context: &'a RequestContext,
            request: &'a ModelCallRequest,
            _cancel: &'a CancellationScope,
        ) -> super::OpenResult<'a> {
            let call_id = request.call_id;
            Box::pin(async move {
                let events = vec![fixed_frame(
                    1,
                    ModelStreamEventKind::CallCompleted {
                        finish_reason: FinishReason::Stop,
                        usage: None,
                        refused: false,
                    },
                )];
                let stream: super::BoxedModelStream<'a> = Box::new(VecStream { call_id, events });
                Ok(stream)
            })
        }
    }

    /// A hand-written stream, exactly as a real adapter's would be.
    struct VecStream {
        call_id: jarvis_domain::ids::ModelCallId,
        events: Vec<ModelStreamEvent>,
    }

    impl ModelStream for VecStream {
        fn next_event(&mut self) -> super::NextEventFuture<'_> {
            Box::pin(async move {
                let _ = self.call_id;
                Ok(self.events.pop())
            })
        }
    }

    let runtime = tokio::runtime::Runtime::new().expect("a runtime is available");
    let events = runtime
        .block_on(async {
            let context = context();
            let request = request();
            let adapter = MinimalAdapter {
                served: vec![model()],
            };
            let cancel = CancellationScope::new();
            let mut stream = adapter
                .open(&context, &request, &cancel)
                .await
                .expect("the adapter opens");
            let mut events = Vec::new();
            while let Some(event) = stream.next_event().await? {
                events.push(event);
            }
            Ok::<_, ProviderError>(events)
        })
        .expect("the adapter's stream drains");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].kind,
        ModelStreamEventKind::CallCompleted { .. },
    ));
}

/// Builds a text-delta frame at `sequence` for the canonical call.
fn request_row_delta(sequence: u64, text: &str) -> ModelStreamEvent {
    ModelStreamEvent {
        call_id: jarvis_domain::ids::ModelCallId::parse(CALL).expect("valid"),
        event_id: jarvis_domain::ids::ModelStreamEventId::from_uuid(Uuid::from_u128(9_999)),
        sequence: Sequence::new(sequence),
        kind: ModelStreamEventKind::OutputTextDelta {
            item_id: "out-1".to_owned(),
            delta: text.to_owned(),
        },
        provider_metadata: None,
    }
}
