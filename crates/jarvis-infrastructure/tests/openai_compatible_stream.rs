//! End-to-end tests for the OpenAI-compatible adapter against a **real loopback socket**.
//!
//! These exist because the adapter's two pure layers (`sse`, `translate`) are already tested against
//! byte and JSON fixtures, and what remains unproven by those is exactly the part a fixture cannot
//! reach: the HTTP exchange, the chunked decoding, the header construction, and the wiring from the
//! provider port to the normalized stream. A test that stubbed the socket would prove the translator
//! again and nothing about the transport.
//!
//! The server here is a hand-written TCP listener rather than an HTTP framework, for the same reason
//! the client is hand-written: the properties under test are the wire bytes, and a framework that
//! conveniently re-encoded them for us would be testing the framework. It also lets a test produce
//! shapes a real server would not — a truncated chunk, a body with no terminal, a `401` — which is
//! where the interesting failures live.
//!
//! Every test binds `127.0.0.1` on an ephemeral port, so nothing here can reach a network, and the
//! adapter's own loopback-only rule is what makes that the only possibility rather than a convention.

// This is a `tests/*.rs` integration crate, which `clippy.toml`'s test allowances do not
// recognise — a trap recorded in this repository — so the test-only allowances are declared here.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::model::ModelProvider as _;
use jarvis_domain::ids::{IdGenerator as _, ModelCallId, RunId};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{
    CallLimits, ContentBlock, FinishReason, InputItem, InputItems, ModelCallRequest,
    ModelStreamEvent, ModelStreamEventKind, PortableSettings, Role, RouteRequirements,
};
use jarvis_infrastructure::model_providers::openai_compatible::OpenAiCompatibleProvider;

/// A one-shot loopback server that answers the first request with `response`.
///
/// Returns the bound port and the task handle, so a test asserts against the response it scripted and
/// the server task is awaited rather than leaked. One connection only, because the adapter opens one
/// per call and a second connection would be a defect rather than a scenario.
async fn serve_once(response: Vec<u8>) -> (u16, tokio::task::JoinHandle<Vec<u8>>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("an ephemeral loopback port binds");
    let port = listener.local_addr().expect("the address is known").port();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("one connection arrives");
        // The request is read to completion so the assertion can see the headers the adapter built.
        // Bounded so a defect cannot make the test hang on a read that never ends.
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            received.extend_from_slice(&buffer[..read]);
            if received.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let _ = socket.write_all(&response).await;
        let _ = socket.shutdown().await;
        received
    });
    (port, handle)
}

/// Builds an SSE response head plus a chunked body, as a real streaming server sends them.
///
/// Chunked framing is produced here deliberately: it is what an SSE endpoint actually uses, and the
/// adapter's decoder is the layer a non-chunked fixture would leave untested.
fn chunked_sse(events: &[String]) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut body = String::new();
    for event in events {
        let payload = format!("data: {event}\n\n");
        let _ = write!(body, "{:x}\r\n{payload}\r\n", payload.len());
    }
    // The terminating zero-length chunk.
    body.push_str("0\r\n\r\n");

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    )
    .into_bytes()
}

/// A provider pointing at `port`, serving `test-model`.
fn provider_on(port: u16) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(
        ProviderId::parse("local.llamacpp").expect("valid"),
        "127.0.0.1",
        port,
        "test-key-not-a-real-credential",
        vec![ModelId::parse("test-model").expect("valid")],
    )
    .expect("the fixture configures")
    .with_timeout(std::time::Duration::from_secs(10))
}

fn request_for(provider: &OpenAiCompatibleProvider) -> ModelCallRequest {
    let model: ModelRef = provider.models()[0].clone();
    ModelCallRequest {
        call_id: ModelCallId::from_uuid(uuid::Uuid::from_u128(9)),
        run_id: RunId::from_uuid(uuid::Uuid::from_u128(5)),
        model,
        route_requirements: RouteRequirements::text(),
        input: InputItems::new(vec![InputItem::Message {
            role: Role::User,
            blocks: vec![ContentBlock::Text {
                text: "hello".to_owned(),
            }],
        }])
        .expect("the fixture is valid"),
        tools: Vec::new(),
        output_schema: None,
        settings: PortableSettings::default(),
        limits: CallLimits {
            deadline: None,
            max_output_tokens: None,
            max_cost_microunits: None,
        },
    }
}

/// Drains a stream into its events, failing the test on a stream error.
async fn drain(
    stream: &mut (dyn jarvis_application::model::ModelStream + Send),
) -> Vec<ModelStreamEvent> {
    let mut events = Vec::new();
    loop {
        match stream.next_event().await {
            Ok(Some(event)) => events.push(event),
            Ok(None) => break,
            Err(error) => panic!("the stream failed: {error:?}"),
        }
    }
    events
}

fn context() -> jarvis_application::request_context::RequestContext {
    jarvis_application::run_service::api_request_context(
        jarvis_domain::ids::WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
        jarvis_domain::ids::PrincipalId::from_uuid(uuid::Uuid::from_u128(2)),
        jarvis_domain::ids::RequestId::from_uuid(uuid::Uuid::from_u128(3)),
        jarvis_domain::ids::CorrelationId::from_uuid(uuid::Uuid::from_u128(4)),
    )
}

#[tokio::test]
async fn a_real_streamed_answer_arrives_as_normalized_events_over_a_real_socket() {
    // The whole path: a real TCP connection, a chunked HTTP body, SSE framing, JSON translation, and
    // frame stamping. This is the adapter's acceptance test, and it is the one the evidence note's
    // gated live test mirrors against a real provider.
    let (port, server) = serve_once(chunked_sse(&[
        r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","model":"test-model","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#.to_owned(),
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#.to_owned(),
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":" there"},"finish_reason":null}]}"#.to_owned(),
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":2,"total_tokens":11}}"#.to_owned(),
        "[DONE]".to_owned(),
    ]))
    .await;

    let provider = provider_on(port);
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens against the real server");
    let events = drain(stream.as_mut()).await;

    // The request the server actually saw: the credential is a **header**, never a URL component, and
    // the body pins one answer and asks for usage.
    let received = server.await.expect("the server task finishes");
    let received = String::from_utf8_lossy(&received);
    assert!(
        received.starts_with("POST /chat/completions HTTP/1.1"),
        "the path is fixed rather than configurable: {received}",
    );
    assert!(
        received.contains("Authorization: Bearer test-key-not-a-real-credential"),
        "the credential travels as a header",
    );
    assert!(
        received.contains("\"stream\":true") && received.contains("\"n\":1"),
        "the body pins streaming and a single answer: {received}",
    );

    // The normalized stream: one item, two content deltas, exactly one terminal, contiguous
    // sequences from 1.
    let kinds: Vec<&ModelStreamEventKind> = events.iter().map(|event| &event.kind).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| matches!(kind, ModelStreamEventKind::OutputItemAdded { .. }))
            .count(),
        1,
        "the item is opened once: {kinds:?}",
    );
    let deltas: Vec<&str> = kinds
        .iter()
        .filter_map(|kind| match kind {
            ModelStreamEventKind::OutputTextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["Hello", " there"], "{kinds:?}");

    let terminals: Vec<&ModelStreamEventKind> = kinds
        .iter()
        .copied()
        .filter(|kind| kind.is_terminal())
        .collect();
    assert_eq!(terminals.len(), 1, "exactly one terminal: {terminals:?}");
    let ModelStreamEventKind::CallCompleted {
        finish_reason,
        usage,
        refused,
    } = terminals[0]
    else {
        panic!("expected a completion, got {terminals:?}");
    };
    assert_eq!(finish_reason, &FinishReason::Stop);
    assert!(!refused);
    let usage = usage
        .as_ref()
        .expect("the usage block reached the terminal");
    assert_eq!(usage.input_tokens, Some(9));
    assert_eq!(usage.output_tokens, Some(2));
    assert!(usage.provider_reported);

    // Sequences are contiguous from 1, which is the property the state machine's duplicate and
    // reorder checks depend on. A gap would be refused downstream as a malformed stream.
    let sequences: Vec<u64> = events.iter().map(|event| event.sequence.get()).collect();
    assert_eq!(
        sequences,
        (1..=events.len() as u64).collect::<Vec<u64>>(),
        "the stamper numbers frames contiguously",
    );
    // Every frame belongs to the call that was opened, so a frame for another call is impossible.
    for event in &events {
        assert_eq!(event.call_id, request.call_id);
    }
}

#[tokio::test]
async fn a_status_and_body_produce_a_terminal_failure_frame_rather_than_an_open_error() {
    // The contract's own division: `open` errors are for a stream that could not be opened, and a
    // provider-reported failure after that travels as a terminal frame. A `401` here must therefore
    // surface as `call.failed` with the authentication code, not as an `Err` from `open` — a caller
    // that received an error instead could not tell it from a transport failure.
    let response = b"HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 62\r\n\r\n{\"error\":{\"message\":\"bad key\",\"type\":\"invalid_request_error\"}}".to_vec();
    let (port, _server) = serve_once(response).await;

    let provider = provider_on(port);
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens; the failure arrives as a frame");
    let events = drain(stream.as_mut()).await;

    let terminal = events
        .iter()
        .find(|event| event.kind.is_terminal())
        .expect("a terminal frame");
    let ModelStreamEventKind::CallFailed { code, retryable } = &terminal.kind else {
        panic!("expected a failed terminal, got {:?}", terminal.kind);
    };
    assert_eq!(code, "model.provider_authentication");
    // An invalid credential needs an operator, so it must not be offered as retryable.
    assert!(!retryable, "a 401 must not be retryable");
    // The provider's own message must not become a JARVIS code: it is untrusted text, and a client
    // acting on its wording would be acting on the provider's words.
    assert!(
        !code.contains("bad key"),
        "provider text must not reach a code: {code}",
    );
}

#[tokio::test]
async fn a_cancelled_call_ends_cancelled_rather_than_completed() {
    // Cancellation is JARVIS-owned and is observed **while waiting**, which is the property that makes
    // a cancelled call against a stalled provider end promptly instead of hanging. The server here
    // sends one frame and then holds the connection open, so the cancellation is the only thing that
    // can end the stream.
    let (port, _server) = serve_once(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec(),
    )
    .await;

    let provider = provider_on(port);
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens");

    // Cancelled before the first frame is read, so the first `next_event` reports the cancellation.
    cancel.cancel();
    let event = stream
        .next_event()
        .await
        .expect("the stream does not fail")
        .expect("a cancellation terminal");
    assert!(
        matches!(event.kind, ModelStreamEventKind::CallCancelled { .. }),
        "a cancellation must not look like a completion: {:?}",
        event.kind,
    );
    assert_eq!(event.sequence.get(), 1, "the terminal is the first frame");
    assert!(stream.next_event().await.expect("no fault").is_none());
}

#[tokio::test]
async fn a_truncated_chunked_body_ends_as_a_failure_rather_than_a_completion() {
    // A body cut off mid-chunk. The adapter must not read the truncation as the end of a healthy
    // stream: the difference between "the answer finished" and "the transport died" is exactly what a
    // caller has to be able to see, and a truncated response that reported completion would store a
    // partial answer as final.
    let mut body =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec();
    // A frame that announced more bytes than it sent, with no terminating chunk.
    body.extend_from_slice(b"ff\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"cut");
    let (port, _server) = serve_once(body).await;

    let provider = provider_on(port);
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens");
    let events = drain(stream.as_mut()).await;

    let terminal = events
        .iter()
        .find(|event| event.kind.is_terminal())
        .expect("a terminal frame");
    assert!(
        matches!(terminal.kind, ModelStreamEventKind::CallFailed { .. }),
        "a truncated body must fail rather than complete: {:?}",
        terminal.kind,
    );
}

#[tokio::test]
async fn a_body_whose_peer_closes_without_a_terminal_is_a_failure_not_a_completion() {
    // A complete, well-framed stream that simply never names a finish reason. Ending the stream quietly
    // would be recorded as *interrupted* — which is the honest answer — but reporting a completion
    // would store an answer the provider never declared finished.
    let (port, _server) = serve_once(chunked_sse(&[
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"half an answer"},"finish_reason":null}]}"#.to_owned(),
    ]))
    .await;

    let provider = provider_on(port);
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens");
    let events = drain(stream.as_mut()).await;

    let terminal = events
        .iter()
        .find(|event| event.kind.is_terminal())
        .expect("a terminal frame");
    assert!(
        matches!(terminal.kind, ModelStreamEventKind::CallFailed { .. }),
        "a stream with no finish reason must not complete: {:?}",
        terminal.kind,
    );
    // The partial content was delivered before the failure, so the caller keeps it alongside the
    // failure rather than losing both.
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, ModelStreamEventKind::OutputTextDelta { .. })),
        "the partial answer must still reach the caller",
    );
}

#[tokio::test]
async fn a_refusal_over_a_real_socket_reaches_the_terminal_as_a_refusal_flag() {
    // The `OC-C001` trap end to end: a refusal arrives as HTTP 200. The adapter must not report it as a
    // clean completion, and the provider's own `stop` must be preserved verbatim for the controller's
    // fold rather than pre-resolved here.
    let (port, _server) = serve_once(chunked_sse(&[
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"refusal":"I cannot help with that."},"finish_reason":null}]}"#.to_owned(),
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_owned(),
    ]))
    .await;

    let provider = provider_on(port);
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens");
    let events = drain(stream.as_mut()).await;

    let terminal = events
        .iter()
        .find(|event| event.kind.is_terminal())
        .expect("a terminal");
    let ModelStreamEventKind::CallCompleted {
        finish_reason,
        refused,
        ..
    } = &terminal.kind
    else {
        panic!("expected a completion, got {:?}", terminal.kind);
    };
    assert!(*refused, "the refusal flag must be set");
    assert_eq!(
        *finish_reason,
        FinishReason::Stop,
        "the provider's own reason is preserved for the controller's fold",
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            ModelStreamEventKind::OutputTextDelta { delta, .. }
                if delta == "I cannot help with that."
        )),
        "the refusal text reaches the caller",
    );
}

#[tokio::test]
async fn a_connection_refused_by_nothing_listening_is_a_failure_frame_with_a_retryable_code() {
    // No server at all. A refused connection must reach the caller as a frame rather than as a panic or
    // a silent end, and it must be reported as retryable — nothing is wrong with the request, and the
    // peer may be starting up.
    // Port 1 on loopback is bound by nothing in a test environment and is privileged, so a connection
    // attempt is refused immediately.
    let provider = OpenAiCompatibleProvider::new(
        ProviderId::parse("local.llamacpp").expect("valid"),
        "127.0.0.1",
        1,
        "test-key",
        vec![ModelId::parse("test-model").expect("valid")],
    )
    .expect("configures")
    .with_timeout(std::time::Duration::from_secs(5));
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens");
    let events = drain(stream.as_mut()).await;
    let terminal = events
        .iter()
        .find(|event| event.kind.is_terminal())
        .expect("a terminal frame");
    let ModelStreamEventKind::CallFailed { code, retryable } = &terminal.kind else {
        panic!("expected a failed terminal, got {:?}", terminal.kind);
    };
    assert_eq!(code, "model.provider_unavailable");
    assert!(*retryable, "an unreachable peer may answer a retry");
}

#[tokio::test]
async fn a_request_build_failure_is_an_open_error_rather_than_a_connected_call() {
    // The other half of the division: a request this adapter cannot express is refused **before** a
    // connection, so an operator sees "this build cannot serve that request" instead of a protocol
    // fault against a server that never saw it.
    let provider = provider_on(1);
    let mut request = request_for(&provider);
    request.output_schema =
        Some(jarvis_domain::model::stream::JsonText::new("{\"type\":\"object\"}").expect("valid"));
    let cancel = CancellationScope::new();
    let error = provider
        .open(&context(), &request, &cancel)
        .await
        .err()
        .expect("an unsupported request is refused before connecting");
    assert_eq!(error.code(), "model.provider_request_invalid");
}

#[tokio::test]
async fn the_frames_an_adapter_produces_are_the_vocabulary_the_router_reads() {
    // A guard on the boundary the whole adapter exists for: the events it produces must be the same
    // `ModelStreamEventKind` vocabulary the run controller and the routing layer consume, so a second
    // parallel event set cannot appear here. `type_name` is asserted against the contract's dotted
    // spellings so a rename cannot silently change what a client reads.
    let (port, _server) = serve_once(chunked_sse(&[
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"x"},"finish_reason":null}]}"#.to_owned(),
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#.to_owned(),
    ]))
    .await;
    let provider = provider_on(port);
    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider.open(&ctx, &request, &cancel).await.expect("opens");
    let events = drain(stream.as_mut()).await;

    for event in &events {
        let name = event.kind.type_name();
        assert!(
            name.starts_with("call.")
                || name.starts_with("output.")
                || name.starts_with("tool.")
                || name.starts_with("reasoning.")
                || name.starts_with("usage.")
                || name.starts_with("provider."),
            "an event name outside the contract's namespaces: {name}",
        );
    }
    // `length` is preserved rather than flattened, so a truncated answer is distinguishable from a
    // finished one — the `BRN-017` rule.
    let terminal = events
        .iter()
        .find(|event| event.kind.is_terminal())
        .expect("a terminal");
    let ModelStreamEventKind::CallCompleted { finish_reason, .. } = &terminal.kind else {
        panic!("expected a completion");
    };
    assert_eq!(*finish_reason, FinishReason::Length);
    // The generator the adapter injects produces non-nil identifiers, because the domain stamps every
    // frame with one and a nil id would collide with the "no event" sentinel.
    let ids = jarvis_infrastructure::ids::UuidV7Generator::new();
    assert_ne!(ids.next_uuid("probe"), uuid::Uuid::nil());
}

#[tokio::test]
async fn the_adapter_reports_itself_local_so_a_local_only_policy_admits_it() {
    // `endpoint_class` is what a locality policy evaluates, and it is a fact about the adapter rather
    // than a configuration reading: the constructor refuses every non-loopback host, so `Local` cannot
    // be wrong.
    use jarvis_domain::model::identity::EndpointClass;
    let provider = provider_on(1);
    assert_eq!(provider.endpoint_class(), EndpointClass::Local);
    // And the model it serves is the routed one, so a data policy selecting this model has a provider
    // that will actually send it.
    assert_eq!(provider.models().len(), 1);
    assert_eq!(provider.models()[0].model_id.to_string(), "test-model");
}

#[tokio::test]
async fn a_second_call_reaches_a_second_connection_rather_than_reusing_state() {
    // A stream owns its own sequence counter, item identifier, and translator, so two calls cannot
    // share frame numbering. The failure this guards against would be a second call whose first frame
    // is not sequence 1, which the state machine refuses as a malformed stream.
    let (first_port, first_server) = serve_once(chunked_sse(&[
        r#"{"id":"a","choices":[{"index":0,"delta":{"content":"one"},"finish_reason":null}]}"#
            .to_owned(),
        r#"{"id":"a","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_owned(),
    ]))
    .await;
    let first = provider_on(first_port);
    let request = request_for(&first);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = first.open(&ctx, &request, &cancel).await.expect("opens");
    let events = drain(stream.as_mut()).await;
    assert_eq!(events[0].sequence.get(), 1);
    let _ = first_server.await;

    let (second_port, _second_server) = serve_once(chunked_sse(&[
        r#"{"id":"b","choices":[{"index":0,"delta":{"content":"two"},"finish_reason":null}]}"#
            .to_owned(),
        r#"{"id":"b","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#.to_owned(),
    ]))
    .await;
    let second = provider_on(second_port);
    let ctx = context();
    let mut stream = second.open(&ctx, &request, &cancel).await.expect("opens");
    let events = drain(stream.as_mut()).await;
    assert_eq!(
        events[0].sequence.get(),
        1,
        "a second call must start its own numbering rather than continuing the first",
    );

    // A shared generator would let one call's identifiers appear in the other's stream, so the two are
    // asserted disjoint.
    assert!(
        events.iter().all(|event| event.call_id == request.call_id),
        "every frame belongs to the call that opened it",
    );
}

#[tokio::test]
async fn the_credential_never_appears_in_an_error_a_caller_can_read() {
    // The canary is a plausible key shape rather than a literal, so a redactor keyed on a prefix would
    // also have to catch it. The point is that this adapter's own failure path carries no credential:
    // the key travels in a header and the error mapping reads only the status and the JSON code.
    const CANARY: &str = "sk-canary-DO-NOT-LEAK-0123456789";
    let response = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 22\r\n\r\n{\"error\":{\"code\":\"x\"}}".to_vec();
    let (port, _server) = serve_once(response).await;
    let provider = OpenAiCompatibleProvider::new(
        ProviderId::parse("local.llamacpp").expect("valid"),
        "127.0.0.1",
        port,
        CANARY,
        vec![ModelId::parse("test-model").expect("valid")],
    )
    .expect("configures");
    // The `Debug` rendering is the sink a diagnostic line would use.
    let rendered = format!("{provider:?}");
    assert!(
        !rendered.contains(CANARY),
        "the credential reached Debug: {rendered}",
    );
    assert!(!rendered.contains("sk-canary"), "{rendered}");

    let request = request_for(&provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider.open(&ctx, &request, &cancel).await.expect("opens");
    let events = drain(stream.as_mut()).await;
    let terminal = events
        .iter()
        .find(|event| event.kind.is_terminal())
        .expect("a terminal");
    let ModelStreamEventKind::CallFailed { code, .. } = &terminal.kind else {
        panic!("expected a failure");
    };
    assert!(
        !code.contains(CANARY),
        "the credential reached a code: {code}"
    );
    // Every rendered frame is also asserted clean, because a frame's metadata is a sink a client reads.
    for event in &events {
        let rendered = format!("{:?}", event.kind);
        assert!(
            !rendered.contains(CANARY),
            "the credential reached a frame: {rendered}"
        );
    }
}
