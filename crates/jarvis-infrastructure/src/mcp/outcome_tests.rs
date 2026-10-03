//! Tests for MCP call-outcome and error normalization.
//!
//! Four carry more weight than the rest, and each exists because a plausible implementation gets it
//! wrong in a way no other test here would notice:
//!
//! - **`a_closed_transport_is_unsettled_and_a_send_failure_is_not`** is the whole reason the two are
//!   separate classes. Collapsing them makes an at-most-once call look safely repeatable, so the two
//!   assertions are written together and the complement follows them.
//! - **`only_a_proven_idempotent_tool_may_retry_a_reported_failure`** is the retry-safety property in
//!   the direction that matters: an untrusted server's `isError` must not become a licence to repeat
//!   a write.
//! - **`a_refused_result_names_the_unmapped_kind`** keeps a deliberate limitation visible rather than
//!   letting an image be coerced into text or into a fabricated artifact reference.
//! - **`an_over_bound_result_is_refused_rather_than_truncated`** is the fail-safe direction: a
//!   truncated JSON document is invalid and a truncated text result silently misleads.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module
// reached from the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use std::time::Duration;

use super::{
    McpCallOutcome, McpOutcomeRejection, classify_error_code, classify_service_error,
    normalize_call_response, normalize_call_result,
};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::tool::call::{
    ContentBlock, MAX_RESULT_BLOCKS, MAX_RESULT_BYTES, ToolResultBody,
};
use jarvis_domain::tool::classification::Idempotency;
use jarvis_domain::tool::error_class::{RetryPosture, ToolErrorClass};
use rmcp::RoleClient;
use rmcp::ServiceError;
use rmcp::model::{
    AudioContent, CallToolResponse, CallToolResult, ContentBlock as McpBlock, EmbeddedResource,
    ErrorCode, ErrorData, ImageContent, InputRequiredResult, Resource, ResourceContents,
    TextContent,
};
use rmcp::transport::{DynamicTransportError, TokioChildProcess};

/// Builds a success result carrying one text block.
fn text_result(text: &str) -> CallToolResult {
    CallToolResult::success(vec![McpBlock::Text(TextContent::new(text))])
}

/// Normalizes a result that must be accepted, returning its body.
fn accepted(result: &CallToolResult) -> ToolResultBody {
    match normalize_call_result(result).expect("the result must be accepted") {
        McpCallOutcome::Succeeded(body) => body,
        McpCallOutcome::Failed(class) => panic!("expected a body, got a failure: {class:?}"),
    }
}

/// The class a reported failure produces, taken through the public path so the test is about the
/// mapping rather than about a constant.
fn classify_reported_class() -> ToolErrorClass {
    match normalize_call_result(&CallToolResult::error(vec![McpBlock::Text(
        TextContent::new("failed"),
    )]))
    .expect("a reported failure must normalize")
    {
        McpCallOutcome::Failed(class) => class,
        McpCallOutcome::Succeeded(_) => panic!("a reported failure must not produce a body"),
    }
}

#[test]
fn a_text_result_becomes_one_bounded_block() {
    let body = accepted(&text_result("hello"));

    assert_eq!(body.content.len(), 1);
    assert_eq!(body.sensitivity, Sensitivity::Internal);
    assert_eq!(body.content[0].size_bytes(), 5);
}

#[test]
fn structured_content_is_mapped_before_the_text_blocks() {
    // `structuredContent` is the field a `2026-07-28` server is directed to use for a
    // machine-readable result, so it must not be lost behind a human-readable summary.
    let mut result = text_result("summary only");
    result.structured_content = Some(serde_json::json!({"count": 2}));

    let body = accepted(&result);
    assert_eq!(body.content.len(), 2);
    match &body.content[0] {
        ContentBlock::Json { value } => assert_eq!(value.as_str(), r#"{"count":2}"#),
        other => panic!("expected the structured block first, got {other:?}"),
    }
}

#[test]
fn a_refused_result_names_the_unmapped_kind() {
    // The limitation is stated rather than hidden: JARVIS has no artifact store, so there is nothing
    // honest to map a base64 image to.
    let mut with_image = text_result("see attached");
    with_image
        .content
        .push(McpBlock::Image(ImageContent::new("aGk=", "image/png")));
    let rejection = normalize_call_result(&with_image)
        .expect_err("an image must be refused rather than coerced");
    assert_eq!(
        rejection,
        McpOutcomeRejection::UnsupportedContent { kind: "image" }
    );
    assert_eq!(rejection.code(), "mcp.content_unsupported");

    // Audio and a blob resource are refused by name too, so the operator can tell them apart.
    let mut with_audio = text_result("listen");
    with_audio
        .content
        .push(McpBlock::Audio(AudioContent::new("aGk=", "audio/wav")));
    assert_eq!(
        normalize_call_result(&with_audio).expect_err("audio must be refused"),
        McpOutcomeRejection::UnsupportedContent { kind: "audio" }
    );

    let mut with_blob = text_result("blob follows");
    with_blob
        .content
        .push(McpBlock::Resource(EmbeddedResource::new(
            ResourceContents::blob("aGk=", "file:///x.bin"),
        )));
    assert_eq!(
        normalize_call_result(&with_blob).expect_err("a blob resource must be refused"),
        McpOutcomeRejection::UnsupportedContent {
            kind: "blob_resource"
        }
    );
}

#[test]
fn a_text_resource_maps_while_a_resource_link_does_not() {
    // The boundary between content and a reference to content. A text resource *is* its text; a link
    // is a URI, and presenting it as the thing it points at is the conflation to avoid.
    let mut with_text_resource = text_result("ignored");
    with_text_resource.content = vec![McpBlock::Resource(EmbeddedResource::new(
        ResourceContents::text("file body", "file:///x.txt"),
    ))];
    let body = accepted(&with_text_resource);
    assert_eq!(body.content.len(), 1);

    let mut with_link = text_result("see link");
    with_link.content.push(McpBlock::ResourceLink(Resource::new(
        "file:///x.txt",
        "x.txt",
    )));
    assert_eq!(
        normalize_call_result(&with_link).expect_err("a resource link must be refused"),
        McpOutcomeRejection::UnsupportedContent {
            kind: "resource_link"
        }
    );
}

#[test]
fn a_result_with_no_mappable_block_is_refused_rather_than_empty() {
    // A "successful" result carrying nothing is a server defect worth surfacing, and the domain
    // refuses an empty body anyway.
    let empty = CallToolResult::success(Vec::new());
    assert_eq!(
        normalize_call_result(&empty).expect_err("an empty result must be refused"),
        McpOutcomeRejection::EmptyContent
    );
}

#[test]
fn an_over_bound_result_is_refused_rather_than_truncated() {
    // A truncated JSON document is invalid and a truncated text result silently misleads, so the
    // total is refused rather than cut to fit.
    let oversized = "x".repeat(MAX_RESULT_BYTES + 1);
    let rejection = normalize_call_result(&text_result(&oversized))
        .expect_err("an over-bound result must be refused");
    assert!(matches!(
        rejection,
        McpOutcomeRejection::ResultRefused { .. }
    ));

    // The block-count bound is asserted separately so one bound cannot stand in for the other:
    // `MAX_RESULT_BLOCKS`-plus-one small blocks would pass a per-block byte check.
    let many: Vec<McpBlock> = (0..=MAX_RESULT_BLOCKS)
        .map(|index| McpBlock::Text(TextContent::new(format!("block {index}"))))
        .collect();
    assert_eq!(
        normalize_call_result(&CallToolResult::success(many)).expect_err("too many blocks"),
        McpOutcomeRejection::TooManyBlocks {
            max: MAX_RESULT_BLOCKS
        }
    );
}

#[test]
fn a_result_that_reports_failure_is_a_classified_failure_not_a_rejected_one() {
    // The tool ran and said it did not work, which is information rather than a malformed response.
    let reported = CallToolResult::error(vec![McpBlock::Text(TextContent::new("disk full"))]);
    assert_eq!(
        normalize_call_result(&reported).expect("a reported failure must normalize"),
        McpCallOutcome::Failed(ToolErrorClass::ProviderError)
    );
}

#[test]
fn only_a_proven_idempotent_tool_may_retry_a_reported_failure() {
    // The retry-safety property. An untrusted server's `isError` must not become a licence to repeat
    // a write: `ProviderError` is `OnlyIfIdempotent`, so a tool that declared `None` may not retry.
    let class = classify_reported_class();
    assert_eq!(class.retry_posture(), RetryPosture::OnlyIfIdempotent);
    assert!(!class.retryable_for(Idempotency::None));
    // The complement, so this is not an implementation that refuses every retry.
    assert!(class.retryable_for(Idempotency::NaturallyIdempotent));
    assert!(class.retryable_for(Idempotency::CallerKeyed));
}

#[test]
fn a_closed_transport_is_unsettled_and_a_send_failure_is_not() {
    // The asymmetry, in one test so the two directions cannot drift. Collapsing them would make an
    // at-most-once call look safely repeatable.
    let closed = classify_service_error(&ServiceError::TransportClosed);
    assert_eq!(closed, ToolErrorClass::ProviderError);
    assert!(closed.is_unsettled(), "a lost stream may have completed");
    assert!(
        !closed.retryable_for(Idempotency::None),
        "a write must not be repeated after a lost stream"
    );

    let not_sent =
        classify_service_error(&ServiceError::TransportSend(DynamicTransportError::new::<
            TokioChildProcess,
            RoleClient,
        >(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe",
        ))));
    assert_eq!(not_sent, ToolErrorClass::Unavailable);
    assert!(
        !not_sent.is_unsettled(),
        "a request that never left cannot have taken effect"
    );
    // Its posture is `Safe`, so even a non-idempotent tool may retry — because there is no effect to
    // duplicate. That is the assertion the two classes exist to keep separate.
    assert!(not_sent.retryable_for(Idempotency::None));
}

#[test]
fn a_timeout_is_unsettled_and_never_retryable_for_an_unproven_tool() {
    let class = classify_service_error(&ServiceError::Timeout {
        timeout: Duration::from_secs(30),
    });
    assert_eq!(class, ToolErrorClass::Timeout);
    assert!(class.is_unsettled());
    assert!(!class.retryable_for(Idempotency::None));
    assert!(class.retryable_for(Idempotency::NaturallyIdempotent));

    let cancelled = classify_service_error(&ServiceError::Cancelled { reason: None });
    assert_eq!(cancelled, ToolErrorClass::Cancelled);
    assert!(!cancelled.retryable_for(Idempotency::NaturallyIdempotent));
}

#[test]
fn a_bound_exhaustion_is_a_limit_and_not_a_provider_fault() {
    // Asserted separately so `LimitExceeded` is not absorbed by the `ProviderError` default: the two
    // have different retry postures and different operator meanings.
    let rounds =
        classify_service_error(&ServiceError::InputRequiredRoundsExceeded { max_rounds: 3 });
    assert_eq!(rounds, ToolErrorClass::LimitExceeded);
    assert_eq!(rounds.retry_posture(), RetryPosture::Never);

    let lagged = classify_service_error(&ServiceError::SubscriptionLagged { capacity: 8 });
    assert_eq!(lagged, ToolErrorClass::LimitExceeded);
}

#[test]
fn every_specified_error_code_is_classified_deliberately() {
    // Each code is asserted by value rather than by "something was returned", so the fallback arm
    // cannot satisfy the table. The one code that shares the `ProviderError` meaning is asserted both
    // here and separately from the fallback below, so that "landed in the catch-all" and "was named"
    // are distinguishable statements.
    let table = [
        (
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
            ToolErrorClass::Unavailable,
        ),
        (ErrorCode::RESOURCE_NOT_FOUND, ToolErrorClass::NotFound),
        (ErrorCode::METHOD_NOT_FOUND, ToolErrorClass::NotFound),
        (ErrorCode::INVALID_PARAMS, ToolErrorClass::SchemaInvalid),
        (ErrorCode::INVALID_REQUEST, ToolErrorClass::SchemaInvalid),
        (ErrorCode::PARSE_ERROR, ToolErrorClass::SchemaInvalid),
        // The two the peer states about the **request** before doing anything with it. Grouped with the
        // version refusal above because all three are refused at the door.
        (ErrorCode::HEADER_MISMATCH, ToolErrorClass::Unavailable),
        (
            ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
            ToolErrorClass::Unavailable,
        ),
        // The one that reported a failure of its own after accepting the request.
        (ErrorCode::INTERNAL_ERROR, ToolErrorClass::ProviderError),
    ];
    for (code, expected) in table {
        assert_eq!(
            classify_error_code(code),
            expected,
            "code {} must classify as {expected:?}",
            code.0
        );
    }
}

#[test]
fn an_unrecognised_error_code_defaults_pessimistically_rather_than_to_unavailable() {
    // `ProviderError` is the pessimistic choice: its posture may permit a retry only for a
    // proven-idempotent tool and it is unsettled, where `Unavailable` would assert that nothing was
    // delivered — an assertion this code cannot make about a code it does not know. This is the
    // control for the `_ =>` arm the table above cannot reach.
    let unknown = classify_error_code(ErrorCode(-32050));
    assert_eq!(unknown, ToolErrorClass::ProviderError);
    assert!(unknown.is_unsettled());
    assert!(!unknown.retryable_for(Idempotency::None));
}

#[test]
fn the_legacy_resource_not_found_code_is_classified_as_not_found() {
    // **The assertion an earlier spelling of this module's doc made impossible to write.** The
    // surrounding range is partly implementation-reserved (this file's own `-32050` control is in that
    // part), so "every code" is not a total property — but `-32002` is one the specification *names*,
    // and `2026-07-28` renumbered it to `-32602` while telling clients they "SHOULD still accept
    // `-32002` from servers implementing earlier versions". Discovery accepts such a peer by design.
    // Removing the legacy arm is the mutation: a legacy server's not-found would then read as a
    // provider fault, so the detector names the direction rather than only the value.
    //
    // **Asserted on the literal, not on the SDK constant.** `ErrorCode::RESOURCE_NOT_FOUND` *is*
    // `-32002`, so asserting through it would test nothing about the number the renumbering moved —
    // and the literal is what the spec text actually names.
    let legacy = classify_error_code(ErrorCode(-32002));
    assert_eq!(
        legacy,
        ToolErrorClass::NotFound,
        "a legacy peer's not-found must not read as a provider fault"
    );
    assert!(!legacy.is_unsettled());

    // The modern number carries **two** meanings, and this pins that the argument meaning is the one
    // chosen: a caller shown `tool.not_found` for a listed tool is sent hunting for something absent
    // instead of fixing its input. The renumbered pair is genuinely distinct at the wire level, which
    // is why the two arms above cannot be collapsed into one.
    assert_eq!(ErrorCode(-32002).0, -32002);
    assert_ne!(
        ErrorCode(-32002),
        ErrorCode::INVALID_PARAMS,
        "the legacy and current codes must remain distinguishable to a client"
    );
}

#[test]
fn an_unsupported_version_is_not_unsettled_and_may_be_retried() {
    // The one classification that is a judgement: an incompatible peer cannot be *served* by a
    // retry, but the class's posture is about effect duplication, which is certainly impossible
    // here — a request refused at the door took no effect. The quarantine decision belongs to
    // health, not to this mapping, and this test pins which of the two facts this class asserts.
    let class = classify_error_code(ErrorCode::UNSUPPORTED_PROTOCOL_VERSION);
    assert_eq!(class.retry_posture(), RetryPosture::Safe);
    assert!(!class.is_unsettled());
}

#[test]
fn a_request_refused_at_the_door_is_never_reported_as_unsettled() {
    // **The property, asserted separately from the values, because the values alone let the defect
    // back in one arm at a time.** A header mismatch and a missing client capability both mean the
    // peer refused the request before dispatching it, so the one thing that must hold is that such a
    // call is **not unsettled** — `is_unsettled` is what routes a call to reconciliation, and a call
    // the server never accepted must not enter a pass that exists for calls that might have run.
    //
    // This is the assertion that fails under the mutation this round fixed: restoring either code to
    // the `ProviderError` catch-all makes its class unsettled, and the loop names which code did it.
    // `Unavailable`, the class they now share with the version refusal, is `Safe` and settled.
    for code in [
        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
        ErrorCode::HEADER_MISMATCH,
        ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
    ] {
        let class = classify_error_code(code);
        assert!(
            !class.is_unsettled(),
            "code {} was refused at the door, so its outcome is not ambiguous",
            code.0
        );
        assert_eq!(
            class.retry_posture(),
            RetryPosture::Safe,
            "code {} ran nothing, so a retry cannot duplicate an effect",
            code.0
        );
    }

    // The control, so the assertions above cannot be satisfied by classifying *everything* as
    // settled-and-safe: a peer that accepted the request and then failed its own way **is** unsettled,
    // which is the fact that makes this a boundary rather than a constant.
    let internal = classify_error_code(ErrorCode::INTERNAL_ERROR);
    assert!(
        internal.is_unsettled(),
        "a failure reported after the request was accepted may already have taken effect"
    );
    assert_ne!(internal.retry_posture(), RetryPosture::Safe);
}

#[test]
fn every_outcome_rejection_code_is_namespaced_and_distinct() {
    let rejections = [
        McpOutcomeRejection::EmptyContent,
        McpOutcomeRejection::TooManyBlocks { max: 1 },
        McpOutcomeRejection::UnsupportedContent { kind: "image" },
        McpOutcomeRejection::ResultRefused { field: "content" },
        McpOutcomeRejection::NotACompletedResult { kind: "task" },
    ];
    let codes: Vec<&str> = rejections.iter().map(McpOutcomeRejection::code).collect();

    assert!(
        codes.iter().all(|code| code.starts_with("mcp.")),
        "{codes:?}"
    );
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "codes must be distinct: {codes:?}"
    );
}

#[test]
fn a_success_result_is_never_classified_as_a_failure() {
    // The complement of `only_a_proven_idempotent_tool...`: the classifier must not be an
    // implementation that reports every call as failed. `is_error` is **optional** on the wire, so
    // absent and `false` must both be successes — and the constructors set `false` while a peer on an
    // older revision may omit it entirely.
    let explicit = text_result("ok");
    assert_eq!(explicit.is_error, Some(false));
    assert!(matches!(
        normalize_call_result(&explicit).expect("accepted"),
        McpCallOutcome::Succeeded(_)
    ));

    // The absent field, which is what an earlier-protocol server sends.
    let mut absent = text_result("ok");
    absent.is_error = None;
    assert!(matches!(
        normalize_call_result(&absent).expect("accepted"),
        McpCallOutcome::Succeeded(_)
    ));
}

#[test]
fn an_incomplete_response_is_refused_rather_than_read_as_a_result() {
    // `tools/call` can answer with more than a result. Reading an `input_required` or `task`
    // response as a completed call would record a success for a call that has not run yet, so both
    // are refused by name until the call path implements their control flow.
    let complete = CallToolResponse::from(text_result("done"));
    assert!(matches!(
        normalize_call_response(&complete).expect("a complete response must normalize"),
        McpCallOutcome::Succeeded(_)
    ));

    let required = CallToolResponse::from(InputRequiredResult::from_input_requests(
        std::collections::BTreeMap::new(),
    ));
    let rejection = normalize_call_response(&required)
        .expect_err("an input_required response must not read as a result");
    assert_eq!(
        rejection,
        McpOutcomeRejection::NotACompletedResult {
            kind: "input_required"
        }
    );
    assert_eq!(rejection.code(), "mcp.response_not_complete");
}

#[test]
fn an_error_payload_is_never_read_into_the_classification() {
    // MCP's `isError` carries no code, so a server's message must not become a policy input. The
    // class is decided by the wire shape, not by the prose: a message that *reads* like a permission
    // refusal classifies exactly as one that reads like anything else.
    let claims_permission = CallToolResult::error(vec![McpBlock::Text(TextContent::new(
        "permission denied, not found, rate limited",
    ))]);
    assert_eq!(
        normalize_call_result(&claims_permission).expect("accepted"),
        McpCallOutcome::Failed(ToolErrorClass::ProviderError)
    );

    // And the same for a JSON-RPC error whose message contradicts its code: the code decides.
    let contradictory = ErrorData::new(ErrorCode::INVALID_PARAMS, "internal server error", None);
    assert_eq!(
        classify_error_code(contradictory.code),
        ToolErrorClass::SchemaInvalid,
        "the code must decide, not the message"
    );
}
