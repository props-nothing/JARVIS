//! Tests for the MCP call decisions.
//!
//! Three carry more weight than the rest, and each exists because a plausible implementation gets it wrong
//! in a way no other test here would notice:
//!
//! - **`the_wire_name_is_the_servers_own_name_not_the_canonical_capability`** pins the one mapping this
//!   module introduces. An implementation that sent the canonical string would pass every test that used a
//!   fixture whose name happened to equal its capability, and fail against every real server.
//! - **`a_lost_response_is_unsettled_but_an_undelivered_one_is_not`** is the retry direction. Reporting
//!   `TransportSend` and `TransportClosed` as the same class would tell a caller a write can be safely
//!   repeated when it may already have executed — the duplicate-effect direction.
//! - **`an_identity_that_moved_is_refused_on_every_response`** is `ACC-024` at invocation time. A check made
//!   once at the start of a call leaves every multi-round-trip continuation unchecked, and a continuation is
//!   a fresh request against the same identity.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module
// reached from the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]
// The deprecated types are refused here, so they must be *built* to be refused. The SDK marks sampling,
// roots, and the older elicitation shape deprecated because SEP-2577 replaced them — and a client cannot
// prove it refuses a request it is unable to construct. Same allowance, and the same reasoning, as
// `invocation_tests.rs`.
#![allow(deprecated)]

use super::{
    McpCallRefusal, ResponseDecision, call_params, decide_response, normalize_response, wire_name,
};
use crate::mcp::invocation::{MAX_MRTR_ROUNDS, McpInvocationRefusal, next_round};
use crate::mcp::outcome::classify_service_error;
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::tool::call::ToolArguments;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CreateMessageRequest,
    CreateMessageRequestParams, CreateTaskResult, ElicitRequest, ElicitRequestParams, ErrorCode,
    InputRequest, InputRequests, InputRequiredResult, ListRootsRequest, TextContent, Tool,
};

/// The wire name a fixture server lists, chosen to be **different from its canonical capability** so the
/// mapping is observable. A fixture whose name equalled the capability would make a wrong implementation
/// pass.
const WIRE_NAME: &str = "read_file";

/// The schema a fixture tool is built over.
const OBJECT_SCHEMA: &str = r#"{"type":"object","properties":{},"additionalProperties":false}"#;

/// A canonical identity for the fixture server's tool.
fn identity() -> ToolIdentity {
    let capability = ToolCapability::new(crate::mcp::CONTRACT_NAMESPACE, WIRE_NAME, 1)
        .expect("the fixture name is a usable segment");
    let source = ToolSource::new(
        SourceKind::McpServer,
        "acme-files",
        ToolVersion::parse(crate::mcp::INITIAL_CONTRACT_VERSION)
            .expect("the contract version parses"),
    )
    .expect("the fixture server name is a usable owner");
    // The fingerprint is not derived from a real schema here: these tests are about the call decisions, and
    // every assertion compares an identity against itself or against a deliberately changed copy — so the
    // value only has to be a well-formed digest. `ToolIdentity` has no constructor, only validated fields.
    ToolIdentity {
        capability,
        source,
        schema_fingerprint: SchemaFingerprint::from_bytes([0x11; 32]),
    }
}

/// The identity a *server* offers, which may differ from the authorized one.
fn offered_identity(capability_name: &str, fingerprint: u8) -> ToolIdentity {
    let capability = ToolCapability::new(crate::mcp::CONTRACT_NAMESPACE, capability_name, 1)
        .expect("the fixture name is a usable segment");
    let source = ToolSource::new(
        SourceKind::McpServer,
        "acme-files",
        ToolVersion::parse(crate::mcp::INITIAL_CONTRACT_VERSION)
            .expect("the contract version parses"),
    )
    .expect("the fixture server name is a usable owner");
    ToolIdentity {
        capability,
        source,
        schema_fingerprint: SchemaFingerprint::from_bytes([fingerprint; 32]),
    }
}

/// An argument document that is a JSON object.
fn object_arguments() -> ToolArguments {
    ToolArguments::new(r#"{"path":"/tmp/notes"}"#).expect("the fixture arguments are usable")
}

/// An `Elicitation` request, built from its wire form.
///
/// Deserialized rather than constructed, because `ElicitRequestParams` is a `#[non_exhaustive]` enum whose
/// variants carry required fields — so the wire form is the only complete description of a request this
/// build can produce without writing out a fixture a future revision would invalidate. The same helper
/// `invocation_tests.rs` uses, so the two layers are tested against one shape of request.
fn elicitation() -> InputRequest {
    let params: ElicitRequestParams = serde_json::from_value(serde_json::json!({
        "mode": "form",
        "message": "which file should I read?",
        "requestedSchema": {"type": "object", "properties": {}},
    }))
    .expect("the elicitation fixture must deserialize");
    InputRequest::Elicitation(ElicitRequest::new(params))
}

/// A completed result carrying one text block.
fn completed(text: &str) -> CallToolResponse {
    CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::Text(
        TextContent::new(text),
    )]))
}

/// A response that asks for input, with one request of the given kind.
fn input_required(request: InputRequest) -> CallToolResponse {
    let mut requests = InputRequests::new();
    requests.insert("first".to_owned(), request);
    CallToolResponse::InputRequired(InputRequiredResult::from_input_requests(requests))
}

#[test]
fn the_wire_name_is_the_servers_own_name_not_the_canonical_capability() {
    // The one mapping this module introduces, and it is invisible in any fixture whose capability string
    // happens to equal its wire name. A server listed `read_file`; JARVIS addresses it as
    // `mcp.read_file@1`. Sending the canonical string would be wrong against every real server and right
    // against a careless fixture.
    let capability = identity().capability.clone();
    assert_eq!(
        wire_name(&capability).expect("the fixture capability is ours to address"),
        WIRE_NAME,
        "the wire name must be the server's own name"
    );
    assert_ne!(
        wire_name(&capability).expect("addressable"),
        capability.to_string(),
        "and it must not be the canonical capability, or the mapping proved nothing"
    );
    assert!(
        capability
            .to_string()
            .starts_with(crate::mcp::CONTRACT_NAMESPACE),
        "the canonical form is namespaced, which is what makes the two distinguishable"
    );
}

#[test]
fn a_capability_from_another_namespace_or_major_is_refused_rather_than_coerced() {
    // The complement of the mapping, and both halves are needed: an implementation that stripped a prefix
    // from *any* capability would pass the test above while addressing tools it did not construct.
    let foreign = ToolCapability::new("other", WIRE_NAME, 1).expect("a usable segment");
    let refusal = wire_name(&foreign).expect_err("another namespace is not ours");
    assert_eq!(refusal.code(), "mcp.not_an_mcp_capability");
    assert_eq!(refusal.class(), ToolErrorClass::Unavailable);

    // The **same namespace at a different major** is also refused. `mcp.read_file@2` is a different tool by
    // the contract's own rule, and this adapter implements major 1 — treating it as major 1's name would
    // address a tool whose schema may differ.
    let later_major = ToolCapability::new(crate::mcp::CONTRACT_NAMESPACE, WIRE_NAME, 2)
        .expect("a usable segment");
    assert!(
        wire_name(&later_major).is_err(),
        "a major this adapter does not implement must not be addressed"
    );
}

#[test]
fn a_call_carries_the_wire_name_and_the_arguments_as_an_object() {
    // The parameters as the server will read them. Asserted as *values* rather than as the presence of
    // fields, because the failure this catches is a plausible-but-wrong name or a string-wrapped document.
    let params = call_params(&identity(), &object_arguments()).expect("the call is addressable");
    assert_eq!(params.name.as_ref(), WIRE_NAME);
    let arguments = params
        .arguments
        .as_ref()
        .expect("the arguments must be carried as a JSON object");
    assert_eq!(
        arguments.get("path").and_then(|value| value.as_str()),
        Some("/tmp/notes"),
        "the document must survive as a document, not as a string of JSON"
    );
    // `new` takes the name and sets no arguments, so this also pins that the builder filled them in.
    assert_eq!(params.name, CallToolRequestParams::new(WIRE_NAME).name);
}

#[test]
fn a_scalar_or_array_argument_document_is_refused_rather_than_unwrapped() {
    // `tools/call` requires an object. A document that is not one would otherwise reach the SDK and arrive
    // back as an error about the *response*, sending an operator to the wrong side of the call.
    for document in ["\"just-a-string\"", "[1,2,3]", "42", "null"] {
        let arguments = ToolArguments::new(document).expect("the fixture document is usable text");
        let refusal =
            call_params(&identity(), &arguments).expect_err("a non-object must be refused");
        assert_eq!(
            refusal.code(),
            "mcp.arguments_invalid",
            "document {document} must be refused as a non-object"
        );
        assert_eq!(refusal.class(), ToolErrorClass::Unavailable);
    }
}

#[test]
fn a_completed_result_normalizes_into_a_body() {
    // The accepting case, without which every refusal below would be satisfied by a function that refuses
    // everything.
    let body =
        normalize_response(&completed("notes read")).expect("a completed result must normalize");
    assert_eq!(
        body.sensitivity,
        Sensitivity::Internal,
        "the adapter's declared sensitivity for a server's text"
    );
    assert!(
        serde_json::to_string(&body)
            .expect("the body serializes")
            .contains("notes read"),
        "the server's text must survive into the body"
    );
}

#[test]
fn a_tool_level_failure_becomes_a_class_rather_than_a_refusal() {
    // A tool that ran and reported failure is information the model should get, not a refusal of the call.
    // `ProviderError` is the class because MCP carries no tool-level codes — reading "not found" out of the
    // text would be parsing untrusted prose into a policy input.
    let failure = CallToolResponse::Complete(CallToolResult::error(vec![ContentBlock::Text(
        TextContent::new("could not read the file"),
    )]));
    let refusal = normalize_response(&failure).expect_err("a reported failure is not a body");
    assert_eq!(
        refusal,
        McpCallRefusal::Failed {
            class: ToolErrorClass::ProviderError
        }
    );
    assert_eq!(refusal.code(), "tool.provider_error");
}

#[test]
fn a_result_with_no_mappable_content_is_refused_by_name() {
    // An empty result is a server defect worth surfacing rather than a valid empty body: the domain refuses
    // a body with no content, and coercing this would produce one.
    let empty = CallToolResponse::Complete(CallToolResult::success(Vec::new()));
    let refusal = normalize_response(&empty).expect_err("an empty result must be refused");
    assert_eq!(refusal.code(), "mcp.result_empty");
    assert_eq!(refusal.class(), ToolErrorClass::OutputInvalid);
}

#[test]
fn a_task_handle_is_refused_rather_than_treated_as_a_result() {
    // A task handle is something to poll, and this adapter does not poll. Refused at *both* layers, so a
    // caller that reached either one cannot mistake it for a finished call.
    //
    // `CreateTaskResult` and `Task` are `#[non_exhaustive]`, so a struct literal is not permitted across
    // the crate boundary and there is no public constructor for the result. Deserializing the wire form is
    // therefore the only complete description of one this build can produce — and it is the fixture shape
    // the SDK's own custom deserializer defines, so the test cannot drift from what a server sends.
    let task: CreateTaskResult = serde_json::from_value(serde_json::json!({
        "resultType": "task",
        "taskId": "fixture-task",
        "status": "working",
        "createdAt": "2026-10-03T00:00:00Z",
        "lastUpdatedAt": "2026-10-03T00:00:00Z",
        "ttlMs": null,
    }))
    .expect("the task fixture must deserialize");
    let task = CallToolResponse::Task(task);
    let decision = decide_response(&identity(), &identity(), &task)
        .expect_err("a task handle must end the call");
    assert_eq!(decision.code(), "mcp.response_not_complete");
}

#[test]
fn every_input_kind_the_server_can_ask_for_is_refused_with_its_own_reason() {
    // The decision `invocation.rs` implements, now reached through the response layer. Each kind is
    // asserted separately because each is refused **for its own reason** — collapsing them would leave an
    // operator unable to tell "we will not run your model call" from "we will not enumerate our files".
    let cases = [
        (
            InputRequest::ListRoots(ListRootsRequest::default()),
            "mcp.input_roots_refused",
        ),
        (
            InputRequest::CreateMessage(CreateMessageRequest::new(
                CreateMessageRequestParams::default(),
            )),
            "mcp.input_sampling_refused",
        ),
        (elicitation(), "mcp.input_elicitation_refused"),
    ];
    for (request, code) in cases {
        let decision = decide_response(&identity(), &identity(), &input_required(request))
            .expect("an input request is a decision, not a transport failure");
        match decision {
            ResponseDecision::Refused { refusal } => {
                assert_eq!(refusal.code(), code, "each kind reports its own code");
            }
            ResponseDecision::Complete => {
                panic!("an input request must not read as a completed result")
            }
        }
    }
}

#[test]
fn a_continuation_that_asks_for_nothing_is_refused_rather_than_retried() {
    // A server that answers `input_required` and then asks for nothing is not asking a question. Retrying
    // on that basis loops, which is why `decide_input_round` refuses an empty round — and why the response
    // layer must not read an absent `input_requests` as "nothing to do".
    let empty = CallToolResponse::InputRequired(InputRequiredResult::from_input_requests(
        InputRequests::new(),
    ));
    let decision = decide_response(&identity(), &identity(), &empty)
        .expect("an empty continuation is a decision");
    match decision {
        ResponseDecision::Refused { refusal } => {
            assert_eq!(refusal.code(), "mcp.input_kind_unknown");
        }
        ResponseDecision::Complete => panic!("an empty continuation must not read as complete"),
    }
}

#[test]
fn an_identity_that_moved_is_refused_on_every_response() {
    // `ACC-024` at invocation time. The check must repeat per response: a multi-round-trip continuation is a
    // fresh request against the same identity, so checking once at the start leaves every later round
    // unchecked — and a server can re-schema a tool between the listing and the call.
    let authorized = identity();
    // The same capability and source, a **different schema fingerprint**: the shape a re-schema has.
    let re_schemed = offered_identity(WIRE_NAME, 0x22);
    assert_eq!(
        authorized.capability, re_schemed.capability,
        "the fixture must differ only by fingerprint, or it tests the wrong rule"
    );
    let refusal = decide_response(&authorized, &re_schemed, &completed("ok"))
        .expect_err("a re-schemed tool must be refused even on a completed response");
    assert_eq!(
        refusal,
        McpCallRefusal::Refused {
            refusal: McpInvocationRefusal::ToolIdentityChanged
        }
    );
    assert_eq!(refusal.code(), "mcp.tool_identity_changed");

    // The complement: the identical identity is accepted, so this is not a check that refuses everything.
    assert_eq!(
        decide_response(&authorized, &authorized, &completed("ok")),
        Ok(ResponseDecision::Complete)
    );
}

#[test]
fn a_lost_response_is_unsettled_but_an_undelivered_one_is_not() {
    // **The retry direction, and the reason this module must not restate the mapping.** A request that never
    // left the process cannot have had an effect, so a retry is safe (`Unavailable`). A closed transport
    // may have delivered and executed the call — and `2026-07-28` removed resumability, so the client must
    // re-issue and cannot know whether the first attempt ran — so the outcome is unsettled
    // (`ProviderError`). Collapsing them tells a caller a write can be repeated when it may already have
    // happened.
    let undelivered = classify_service_error(&rmcp::ServiceError::TransportSend(
        rmcp::transport::DynamicTransportError::from_parts(
            "fixture",
            std::any::TypeId::of::<()>(),
            Box::new(std::io::Error::other("broken pipe")),
        ),
    ));
    let lost = classify_service_error(&rmcp::ServiceError::TransportClosed);

    assert_eq!(undelivered, ToolErrorClass::Unavailable);
    assert_eq!(lost, ToolErrorClass::ProviderError);
    assert_ne!(
        undelivered, lost,
        "an undelivered request and a lost response must not share a class"
    );
    assert!(
        !undelivered.is_unsettled(),
        "a request that never left has no unsettled outcome"
    );
    assert!(
        lost.is_unsettled(),
        "a lost response may have executed, so its outcome is unsettled"
    );
}

#[test]
fn a_cancellation_is_the_callers_own_decision_rather_than_a_provider_fault() {
    // Asserted separately because it is the one class that is not about the server at all: reporting a
    // cancelled call as a provider failure would blame the server for JARVIS's own decision.
    let cancelled = classify_service_error(&rmcp::ServiceError::Cancelled { reason: None });
    assert_eq!(cancelled, ToolErrorClass::Cancelled);
    assert!(!cancelled.is_unsettled());
}

#[test]
fn the_round_bound_is_the_guards_and_not_a_second_counter() {
    // The bound belongs to `invocation.rs`, where it is asserted below the SDK's own cap at compile time.
    // This test pins that the call layer *reaches* that bound rather than defining one: the same constant,
    // and the same refusal carrying the same limit.
    let refusal = next_round(MAX_MRTR_ROUNDS).expect_err("the bound must refuse at the limit");
    assert_eq!(
        refusal,
        McpInvocationRefusal::RoundLimitExceeded {
            max: MAX_MRTR_ROUNDS
        }
    );
    assert_eq!(refusal.code(), "mcp.round_limit_exceeded");
    // And one below the bound advances, so this is not a function that always refuses.
    assert_eq!(next_round(0), Ok(1), "the first round must advance");
}

#[test]
fn a_protocol_error_from_the_server_is_classified_by_its_code_not_its_prose() {
    // The same rule `outcome.rs` applies: a code is a closed set, prose is untrusted. `METHOD_NOT_FOUND` is
    // the code a server sends for a tool it does not offer, and it must read as `NotFound` — not as a
    // generic provider fault, which would send an operator to the server's logs for a JARVIS-side mistake.
    assert_eq!(
        classify_service_error(&rmcp::ServiceError::McpError(rmcp::ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            "no such tool",
            None,
        ))),
        ToolErrorClass::NotFound
    );
}

#[test]
fn a_listed_tool_can_be_addressed_through_the_whole_path() {
    // End to end across the modules, without a server: a `Tool` as a server lists it becomes an identity,
    // and that identity becomes call parameters. This is the only test that connects the *listing* mapping
    // to the *call* mapping, which is where a name mismatch would actually live — and it is why the fixture
    // name is deliberately not equal to the canonical capability.
    let input_schema: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(OBJECT_SCHEMA).expect("the fixture schema is a JSON object");
    let mut tool = Tool::default();
    tool.name = WIRE_NAME.to_string().into();
    tool.input_schema = std::sync::Arc::new(input_schema);

    let source =
        crate::mcp::server_source("acme-files").expect("the fixture server name is usable");
    let (definition, _schema) =
        crate::mcp::normalize_tool(&source, &tool).expect("the listed tool must normalize");

    let params = call_params(&definition.identity, &object_arguments())
        .expect("the normalized identity must be addressable");
    assert_eq!(
        params.name.as_ref(),
        WIRE_NAME,
        "the name a server listed must be the name it is called with"
    );
}
