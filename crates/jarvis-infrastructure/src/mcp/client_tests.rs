//! Tests for MCP client startup decisions.
//!
//! Four carry more weight than the rest, and each exists because a plausible implementation gets it
//! wrong in a way no other test here would notice:
//!
//! - **`an_incompatible_peer_is_permanent_even_though_its_class_says_retry`** is the module's whole
//!   reason for existing. `ToolErrorClass::Unavailable`'s posture is `Safe`, so a caller that consulted
//!   only the class would retry forever against a server that cannot be served.
//! - **`a_transient_startup_failure_is_not_quarantined`** is the complement: treating a restarting
//!   server as permanently incompatible is the opposite error and equally bad.
//! - **`the_preference_order_is_what_decides_between_two_shared_versions`** pins JARVIS's only
//!   contribution to negotiation, since the selection rule itself is the SDK's.
//! - **`a_nested_incompatibility_is_still_recognised_as_permanent`** is why the fallback case recurses:
//!   the SDK reports a discover failure and a legacy failure together, and the outer one does not say
//!   the server is incompatible — only the inner one does.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module
// reached from the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use std::borrow::Cow;

use super::{
    TARGET_PROTOCOL_VERSION, classify_startup_error, is_permanent_startup_failure,
    negotiate_protocol_version, preferred_protocol_versions, requires_initialize_handshake,
};
use jarvis_domain::tool::classification::Idempotency;
use jarvis_domain::tool::error_class::{RetryPosture, ToolErrorClass};
use rmcp::model::{ErrorCode, ErrorData, ProtocolVersion};
use rmcp::service::ClientInitializeError;
use rmcp::transport::DynamicTransportError;

/// An incompatible-version startup failure, which is the case the note's error table names.
fn incompatible(
    client: Vec<ProtocolVersion>,
    server: Vec<ProtocolVersion>,
) -> ClientInitializeError {
    ClientInitializeError::NoCompatibleProtocolVersion {
        client_supported: client,
        server_supported: server,
    }
}

#[test]
fn the_preference_order_is_newest_first_and_complete() {
    // The order is JARVIS's only contribution to negotiation, so it is asserted as an order rather than
    // as a set. Asserted against the expected *sequence* rather than against `KNOWN_VERSIONS` reversed,
    // because a test that recomputed the implementation's own derivation would agree with it however it
    // was wrong.
    let preferred = preferred_protocol_versions();

    assert_eq!(preferred.first(), Some(&TARGET_PROTOCOL_VERSION));
    assert_eq!(
        preferred,
        vec![
            ProtocolVersion::V_2026_07_28,
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2024_11_05,
        ]
    );
    // A single-entry list would have no fallback, which is what the note's "negotiated older
    // compatibility" depends on.
    assert!(preferred.len() > 1);
    // No duplicates: the target is added first and then filtered out of the derived remainder, and a
    // filter that missed it would offer the same version twice.
    let mut unique = preferred.clone();
    unique.sort_by_key(ProtocolVersion::to_string);
    unique.dedup();
    assert_eq!(unique.len(), preferred.len(), "{preferred:?}");
}

#[test]
fn the_preference_order_is_what_decides_between_two_shared_versions() {
    // The selection rule is the SDK's ("the first client-preferred version the server supports"), so
    // what this test pins is that JARVIS's order is the input to it — newest first. The input is
    // deliberately reversed relative to the preference, so an implementation that read the server's
    // order instead would pick the older one and fail here.
    let shared =
        negotiate_protocol_version(&[ProtocolVersion::V_2025_06_18, ProtocolVersion::V_2026_07_28]);
    assert_eq!(shared, Some(TARGET_PROTOCOL_VERSION));

    // And a peer that shares only an older revision gets the newest shared one rather than nothing.
    let older = negotiate_protocol_version(&[ProtocolVersion::V_2025_03_26]);
    assert_eq!(older, Some(ProtocolVersion::V_2025_03_26));
}

#[test]
fn a_peer_sharing_nothing_negotiates_nothing() {
    // `None` is the exact condition the SDK raises its incompatible-version error for, so a caller can
    // check before connecting. An **empty** list is the only way to express "shares nothing" from
    // outside the SDK: `ProtocolVersion` has a `Deserialize` that matches known revisions and no public
    // constructor, so a made-up future version cannot be built here — which is why this test uses the
    // empty set rather than an unknown revision.
    assert_eq!(negotiate_protocol_version(&[]), None);
}

#[test]
fn an_incompatible_peer_is_permanent_even_though_its_class_says_retry() {
    // The module's reason for existing, asserted as the *combination*: the class's posture is
    // permissive and the failure is still not retryable.
    let failure = classify_startup_error(&incompatible(
        vec![TARGET_PROTOCOL_VERSION],
        vec![ProtocolVersion::V_2024_11_05],
    ));

    assert_eq!(failure.code, "mcp.startup_no_compatible_version");
    assert!(failure.is_permanent());
    assert_eq!(failure.class, ToolErrorClass::Unavailable);
    // The class alone would say yes...
    assert_eq!(failure.class.retry_posture(), RetryPosture::Safe);
    assert!(failure.class.retryable_for(Idempotency::None));
    // ...and the combination says no. This is the assertion that catches a caller reading the class.
    assert!(!failure.is_retryable_for(Idempotency::None));
    assert!(!failure.is_retryable_for(Idempotency::NaturallyIdempotent));
}

#[test]
fn a_transient_startup_failure_is_not_quarantined() {
    // The complement, so the permanence rule is not "everything is permanent". A dropped connection may
    // succeed on the next attempt, and quarantining it would take a healthy server out of service.
    let closed = ClientInitializeError::ConnectionClosed("peer went away".to_owned());
    let failure = classify_startup_error(&closed);

    assert!(!failure.is_permanent());
    assert_eq!(failure.class, ToolErrorClass::ProviderError);
    assert_eq!(failure.code, "mcp.startup_connection_closed");
    // `ProviderError` is `OnlyIfIdempotent`, and the combination preserves that: a startup that may have
    // been partially delivered is not blindly repeated for an unproven tool.
    assert!(!failure.is_retryable_for(Idempotency::None));
    assert!(failure.is_retryable_for(Idempotency::NaturallyIdempotent));
    assert!(!is_permanent_startup_failure(&closed));
}

#[test]
fn a_missing_preferred_version_is_a_jarvis_defect_and_permanent() {
    // A configuration defect rather than a peer failure: retrying the same call repeats it.
    let failure = classify_startup_error(&ClientInitializeError::NoPreferredProtocolVersion);

    assert_eq!(failure.code, "mcp.startup_no_preferred_version");
    assert!(failure.is_permanent());
    assert!(!failure.is_retryable_for(Idempotency::NaturallyIdempotent));
}

#[test]
fn a_cancelled_startup_is_cancelled_rather_than_unavailable() {
    // Not a peer failure and not something a retry repairs, but not permanent either: the caller may
    // simply ask again.
    let failure = classify_startup_error(&ClientInitializeError::Cancelled);

    assert_eq!(failure.class, ToolErrorClass::Cancelled);
    assert_eq!(failure.code, "mcp.startup_cancelled");
    assert!(!failure.is_permanent());
    assert!(!failure.is_retryable_for(Idempotency::NaturallyIdempotent));
}

#[test]
fn a_json_rpc_error_is_classified_by_its_code_not_its_message() {
    // The same rule the call-outcome layer applies: prose is not a policy input.
    let with_permissive_prose = ErrorData::new(
        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
        "everything is fine, please retry",
        None,
    );
    let failure =
        classify_startup_error(&ClientInitializeError::JsonRpcError(with_permissive_prose));

    assert_eq!(failure.code, "mcp.startup_jsonrpc_error");
    assert!(failure.is_permanent(), "the code decides, not the message");
    assert!(!failure.is_retryable_for(Idempotency::NaturallyIdempotent));

    // A code that is not about versions is neither permanent nor a schema problem.
    let transient = ErrorData::new(ErrorCode::INTERNAL_ERROR, "boom", None);
    let failure = classify_startup_error(&ClientInitializeError::JsonRpcError(transient));
    assert!(!failure.is_permanent());
    assert_eq!(failure.class, ToolErrorClass::ProviderError);
}

#[test]
fn a_nested_incompatibility_is_still_recognised_as_permanent() {
    // The SDK reports a discover failure and a legacy fallback failure together. The outer variant says
    // only "both failed" — the inner one is what says the server is incompatible — so an implementation
    // that classified only the outer failure would miss it and retry forever.
    let nested = ClientInitializeError::LegacyFallbackFailed {
        discover: Box::new(incompatible(
            vec![TARGET_PROTOCOL_VERSION],
            vec![ProtocolVersion::V_2024_11_05],
        )),
        fallback: Box::new(ClientInitializeError::Cancelled),
    };
    let failure = classify_startup_error(&nested);

    assert_eq!(failure.code, "mcp.startup_legacy_fallback_failed");
    assert!(
        failure.is_permanent(),
        "the inner incompatibility must survive the outer classification"
    );
    assert!(!failure.is_retryable_for(Idempotency::NaturallyIdempotent));

    // The control: the same outer variant with a *transient* inner failure is not permanent, so the
    // assertion above is about the nesting rather than about the outer variant always being permanent.
    let transient = ClientInitializeError::LegacyFallbackFailed {
        discover: Box::new(ClientInitializeError::Cancelled),
        fallback: Box::new(ClientInitializeError::Cancelled),
    };
    assert!(!classify_startup_error(&transient).is_permanent());
}

#[test]
fn a_transport_error_is_carried_rather_than_swallowed() {
    let wrapped = ClientInitializeError::TransportError {
        error: DynamicTransportError::from_parts(
            "test-transport",
            std::any::TypeId::of::<()>(),
            Box::new(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "broken pipe",
            )),
        ),
        context: Cow::Borrowed("sending discover"),
    };
    let failure = classify_startup_error(&wrapped);

    assert_eq!(failure.code, "mcp.startup_transport_error");
    assert!(!failure.is_permanent());
    assert_eq!(failure.class, ToolErrorClass::ProviderError);
}

#[test]
fn an_unusable_peer_answer_is_output_invalid_rather_than_retryable_forever() {
    // An answer whose id does not correlate, or one that does not match what a client can use, is a
    // protocol problem and not a transport one. `OutputInvalid` is `Never` for retrying, so a peer
    // answering the wrong question is not papered over by repetition.
    let uncorrelated = ClientInitializeError::UncorrelatedErrorResponse {
        expected: rmcp::model::RequestId::Number(1),
        received: rmcp::model::RequestId::Number(2),
    };
    let failure = classify_startup_error(&uncorrelated);

    assert_eq!(failure.class, ToolErrorClass::OutputInvalid);
    assert_eq!(failure.code, "mcp.startup_output_invalid");
    assert_eq!(failure.class.retry_posture(), RetryPosture::Never);
    assert!(!failure.is_retryable_for(Idempotency::NaturallyIdempotent));

    // The same for the other two shapes of "the answer was not one a client can use".
    for unexpected in [
        ClientInitializeError::ExpectedInitResponse(None),
        ClientInitializeError::ExpectedInitResult(None),
    ] {
        assert_eq!(
            classify_startup_error(&unexpected).class,
            ToolErrorClass::OutputInvalid
        );
    }
}

#[test]
fn the_handshake_question_is_about_the_session_not_about_the_target_revision() {
    // A session may legitimately negotiate an older revision whose lifecycle differs, so behaviour
    // written against the target must ask rather than assume — and the question is "does this session
    // have a handshake", which is **not** "is this exactly the revision we targeted". The distinction is
    // asserted rather than described: `2025-11-25` is not the target and *does* have a handshake.
    assert!(!requires_initialize_handshake(&TARGET_PROTOCOL_VERSION));
    assert!(requires_initialize_handshake(
        &ProtocolVersion::V_2025_11_25
    ));
    assert!(requires_initialize_handshake(
        &ProtocolVersion::V_2024_11_05
    ));
}

#[test]
fn every_startup_code_is_namespaced_and_distinct() {
    let failures = [
        classify_startup_error(&incompatible(Vec::new(), Vec::new())),
        classify_startup_error(&ClientInitializeError::NoPreferredProtocolVersion),
        classify_startup_error(&ClientInitializeError::Cancelled),
        classify_startup_error(&ClientInitializeError::ConnectionClosed(String::new())),
        classify_startup_error(&ClientInitializeError::JsonRpcError(ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            "boom",
            None,
        ))),
        classify_startup_error(&ClientInitializeError::ExpectedInitResponse(None)),
    ];
    let codes: Vec<&str> = failures.iter().map(|failure| failure.code).collect();

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

/// Proves the real client's refusal helper carries JARVIS's own reason for **all three** input kinds.
///
/// **The defect this closes is a default rather than an omission, and it is the worst kind.** The SDK's
/// multi-round-trip loop drives `input_required` rounds itself, routing each request to the client handler;
/// `decide_response`'s `InputRequired` arm is therefore unreachable in production, and what a server meets is
/// whatever `JarvisClient` answers. `JarvisClient` implemented nothing, so the defaults applied — and for
/// **roots** the default is `Ok(ListRootsResult::default())`, a *success* carrying an empty list. A server
/// asking JARVIS for the shape of its filesystem, which `invocation.rs` refuses as an information-disclosure
/// primitive, was told it had none. Elicitation fared no better: the default returns
/// `ElicitResult { action: Decline }`, which reaches the server as a *user decision* that no user made. Only
/// sampling was refused by default (`method_not_found`).
///
/// This asserts the **construction** — one helper, three kinds, each carrying its own reason and a
/// `-32602` code — so the three overrides cannot drift apart. `mcp_conversation.rs` asserts the wire half,
/// where a real server sends the request and the refusal is what comes back.
#[test]
#[allow(deprecated)]
fn every_input_kind_is_refused_with_its_own_reason() {
    use crate::mcp::invocation::InputKind;

    // `invalid_request` is `-32600`/`-32602`-family rather than `method_not_found`: the peer must learn the
    // request was *understood and declined*, not that JARVIS does not implement the method. The difference
    // matters to a server deciding whether to retry with a different shape.
    for kind in [
        InputKind::Sampling,
        InputKind::Roots,
        InputKind::Elicitation,
    ] {
        let error = super::refused_input(kind);
        assert_eq!(
            error.code,
            ErrorCode::INVALID_REQUEST,
            "{kind:?} must be refused as an invalid request rather than as an unimplemented method"
        );
        assert_eq!(
            error.message.as_ref(),
            kind.reason(),
            "{kind:?} must carry its own contract reason, not a shared message"
        );
    }

    // And the three reasons are distinct, which is what makes the assertion above meaningful: three calls to
    // one helper returning one string would satisfy per-kind equality only if the reasons were the same.
    let reasons: Vec<&str> = [
        InputKind::Sampling,
        InputKind::Roots,
        InputKind::Elicitation,
    ]
    .into_iter()
    .map(InputKind::reason)
    .collect();
    let mut unique = reasons.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        3,
        "each kind names its own correction: {reasons:?}"
    );
}
