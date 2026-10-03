//! Tests for the MCP invocation guard.
//!
//! Four carry more weight than the rest, and each exists because a plausible implementation gets it
//! wrong in a way no other test here would notice:
//!
//! - **`a_changed_schema_beneath_the_same_capability_is_refused`** is `ACC-024`'s rule. A guard that
//!   compared only the capability would pass every other identity test here and authorize a tool whose
//!   schema changed after approval.
//! - **`every_multi_round_trip_input_kind_is_refused_by_name`** pins the decision, and the *by name* half
//!   matters: a single generic refusal would make the three reasons indistinguishable to an operator.
//! - **`a_refused_round_is_refused_as_a_whole`** is why the round is judged rather than the offending
//!   request: servicing the acceptable ones would tell the server JARVIS engaged with its demand.
//! - **`an_empty_round_is_refused_because_it_cannot_be_answered`** keeps a continuation with no question
//!   from becoming a retry loop.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module
// reached from the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]
// The deprecated types are refused here, so they must be *built* to be refused. The SDK marks sampling,
// roots, and the older elicitation shape deprecated because SEP-2577 replaced them — and a client cannot
// refuse a request it cannot parse. Allowing the lint is therefore the honest position: the deprecation
// is why these fixtures exist, not something this file is ignoring.
#![allow(deprecated)]

use std::collections::BTreeMap;

use super::{
    InputKind, McpInvocationRefusal, authorizes_invocation, decide_input_request,
    decide_input_round,
};
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use rmcp::model::{
    CreateMessageRequest, CreateMessageRequestParams, ElicitRequest, ElicitRequestParams,
    InputRequest, InputRequests, ListRootsRequest,
};

/// A usable tool identity for one server, with a fingerprint distinguished by `schema_seed`.
///
/// The seed is a `u8` rather than a schema document because `SchemaFingerprint` is a 32-byte digest with
/// no public hash constructor: `from_bytes` is the domain's injection point for a test, and it is used
/// here to make two fingerprints **differ** rather than to pretend they describe a schema.
fn identity(owner: &str, schema_seed: u8) -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::new("mcp", "read_file", 1).expect("the fixture is canonical"),
        source: ToolSource::new(
            SourceKind::McpServer,
            owner,
            ToolVersion::parse("1.0.0").expect("the fixture version parses"),
        )
        .expect("the fixture owner is usable"),
        schema_fingerprint: SchemaFingerprint::from_bytes([schema_seed; 32]),
    }
}

/// A `CreateMessage` (sampling) request.
fn sampling() -> InputRequest {
    InputRequest::CreateMessage(CreateMessageRequest::new(
        CreateMessageRequestParams::default(),
    ))
}

/// A `ListRoots` (roots) request.
fn roots() -> InputRequest {
    InputRequest::ListRoots(ListRootsRequest::default())
}

/// An `Elicitation` request, built from its wire form.
///
/// Deserialized rather than constructed, because `ElicitRequestParams` is a `#[non_exhaustive]` enum whose
/// variants carry required fields — so the wire form is the only complete description of a request this
/// build can produce without writing out a fixture that a future revision would invalidate.
fn elicitation() -> InputRequest {
    let params: ElicitRequestParams = serde_json::from_value(serde_json::json!({
        "mode": "form",
        "message": "which file should I read?",
        "requestedSchema": {"type": "object", "properties": {}},
    }))
    .expect("the elicitation fixture must deserialize");
    InputRequest::Elicitation(ElicitRequest::new(params))
}

#[test]
fn an_unchanged_identity_authorizes_invocation() {
    // The accepting case, without which every refusal below would be satisfied by a guard that refuses
    // everything — the failure mode this project keeps recording.
    let granted = identity("acme-files", 1);
    let offered = identity("acme-files", 1);

    assert_eq!(authorizes_invocation(&granted, &offered), Ok(()));
}

#[test]
fn a_changed_schema_beneath_the_same_capability_is_refused() {
    // `ACC-024`: the schema fingerprint is part of identity precisely so a tool replaced behind the same
    // name is not covered by the approval recorded for the original. A guard comparing only the
    // capability would authorize this.
    let granted = identity("acme-files", 1);
    let offered = identity("acme-files", 2);

    assert_eq!(
        granted.capability.to_string(),
        offered.capability.to_string(),
        "the capability must be identical, so the capability is not what distinguishes them"
    );
    assert_eq!(
        authorizes_invocation(&granted, &offered),
        Err(McpInvocationRefusal::ToolIdentityChanged)
    );
}

#[test]
fn a_different_server_offering_one_capability_is_refused() {
    // The source owner is the second distinguishing component: two servers offering `mcp.read_file@1`
    // are different tools, so one server's grant must not authorize the other's.
    let granted = identity("server-a", 1);
    let offered = identity("server-b", 1);

    assert_eq!(
        authorizes_invocation(&granted, &offered),
        Err(McpInvocationRefusal::ToolIdentityChanged)
    );
    assert_eq!(
        McpInvocationRefusal::ToolIdentityChanged.code(),
        "mcp.tool_identity_changed"
    );
}

#[test]
fn every_multi_round_trip_input_kind_is_refused_by_name() {
    // The decision, asserted per kind. Refusing "by name" is the load-bearing half: a single generic
    // refusal would leave an operator unable to tell a model-budget request from a filesystem
    // enumeration from a request to prompt the user.
    let cases = [
        (sampling(), InputKind::Sampling),
        (roots(), InputKind::Roots),
        (elicitation(), InputKind::Elicitation),
    ];
    for (request, expected) in cases {
        let refusal = decide_input_request(&request)
            .expect_err("every input request kind must be refused by this build");
        match refusal {
            McpInvocationRefusal::InputRefused { kind, reason } => {
                assert_eq!(kind, expected, "the kind must match the request");
                assert_eq!(
                    kind,
                    InputKind::of(&request).expect("the fixture is classifiable")
                );
                assert_eq!(refusal.code(), kind.code());
                assert!(
                    reason.contains("JARVIS"),
                    "the reason must name what JARVIS declines to spend: {reason}"
                );
            }
            other => panic!("expected InputRefused for {expected:?}, got {other:?}"),
        }
    }

    // And the three codes are distinct, so the assertions above are not three spellings of one code.
    let codes = [
        InputKind::Sampling.code(),
        InputKind::Roots.code(),
        InputKind::Elicitation.code(),
    ];
    let mut unique = codes.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), codes.len(), "{codes:?}");
}

#[test]
fn a_root_request_names_the_information_it_would_disclose() {
    // The reason for roots is the one worth pinning in words: it is the variant that looks like a
    // convenience and is an enumeration of the workspace layout. Asserted on the *reason* rather than on
    // the code, because the code alone would not show a reader why the refusal is a decision.
    let refusal = decide_input_request(&roots()).expect_err("roots must be refused");
    let McpInvocationRefusal::InputRefused { reason, .. } = refusal else {
        panic!("expected an InputRefused");
    };
    assert!(
        reason.contains("directories are passed as tool parameters"),
        "{reason}"
    );
}

#[test]
fn a_refused_round_is_refused_as_a_whole() {
    // A round is answered as a unit: servicing the acceptable requests while refusing one would tell the
    // server that JARVIS engaged with its demand, and the server chose the set. Here a refusal is paired
    // with another refusal — there is no servicing path at all in this build — so what this test pins is
    // that the *round* has a single decision and that it is stable.
    let mut round: InputRequests = BTreeMap::new();
    round.insert("b".to_owned(), elicitation());
    round.insert("a".to_owned(), sampling());

    let refusal = decide_input_round(&round).expect_err("a refusing round must be refused");
    // The map is sorted, so "a" is judged first and the answer does not depend on insertion order.
    assert_eq!(
        refusal,
        McpInvocationRefusal::InputRefused {
            kind: InputKind::Sampling,
            reason: InputKind::Sampling.reason(),
        }
    );

    // Reversed insertion order, same answer: the decision is a function of the set, not of the order the
    // server happened to serialize it in.
    let mut reversed: InputRequests = BTreeMap::new();
    reversed.insert("a".to_owned(), sampling());
    reversed.insert("b".to_owned(), elicitation());
    assert_eq!(
        decide_input_round(&reversed).expect_err("refused"),
        refusal,
        "the answer must not depend on insertion order"
    );
}

#[test]
fn an_empty_round_is_refused_because_it_cannot_be_answered() {
    // `input_required` with nothing asked is not a question, and retrying on that basis would loop. It is
    // refused separately from a named kind, because the reason is different: nothing was requested.
    let empty: InputRequests = BTreeMap::new();
    assert_eq!(
        decide_input_round(&empty),
        Err(McpInvocationRefusal::InputKindUnknown)
    );
    assert_eq!(
        McpInvocationRefusal::InputKindUnknown.code(),
        "mcp.input_kind_unknown"
    );
}

// The relationship between JARVIS's round bound and the SDK's default is a **compile-time** assertion in
// `invocation.rs` rather than a test, because both sides are constants and clippy rejects a runtime
// comparison of them as "this assertion has a constant value" — correctly, since it cannot fail at run
// time. Moving it to a `const` assertion strengthens it: it holds on every build, not only when the suite
// runs. Checking it also corrected a wrong belief recorded in a comment there (the SDK default is 10).
//
// A `the_round_bound_is_reached_at_the_bound_and_not_before` test used to sit here, exercising a
// `next_round` counter. Both were deleted with the function: nothing called it, and a test whose only
// purpose is to keep a `pub fn` alive is exactly what made the dead code look exercised.

#[test]
fn every_refusal_code_is_namespaced_and_distinct() {
    let refusals = [
        McpInvocationRefusal::ToolIdentityChanged,
        McpInvocationRefusal::InputRefused {
            kind: InputKind::Sampling,
            reason: "x",
        },
        McpInvocationRefusal::InputRefused {
            kind: InputKind::Roots,
            reason: "x",
        },
        McpInvocationRefusal::InputRefused {
            kind: InputKind::Elicitation,
            reason: "x",
        },
        McpInvocationRefusal::InputKindUnknown,
    ];
    let codes: Vec<&str> = refusals.iter().map(McpInvocationRefusal::code).collect();

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
