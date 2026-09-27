//! Tests for the systemic defect found by auditing every `#[serde(transparent)]` newtype.
//!
//! The defect: a validated newtype that derives `Deserialize` has a **derived** deserializer which calls
//! `String::deserialize` (or `Vec::deserialize`) and wraps the result **directly**, never going through
//! the constructor. So every bound, every NUL rule, and every non-emptiness invariant held for values
//! this crate built and was **bypassed for values that arrived over the wire** — which is the direction
//! an attacker chooses, and the one every downstream caller assumed was already covered.
//!
//! `WorkspaceRelativePath` was fixed for this in the filesystem round. Auditing the rest of the crate
//! found **nine more instances**, four of them in modules written before the audit existed. Each is
//! asserted below against the rule its own constructor enforces, because a fix that is not tested is a
//! claim — and the reason this defect survived so long is that no test ever sent a bad value *in*.
//!
//! **The shape of every assertion here is the same**, and it is the shape that was missing: the valid
//! value must round-trip, and the value the constructor refuses must be refused **on the way in**.

use super::approval::{AllowedChannels, ApprovalChannel};
use super::call::{MAX_ARGUMENT_BYTES, MAX_RESULT_BYTES, ResultPayload, ToolArguments};
use super::classification::Scope;
use super::registry::{ListVersion, ServerConfigId};
use crate::model::stream::{JsonText, MAX_JSON_TEXT_BYTES};
use crate::run::state::{MAX_REASON_BYTES, TransitionReason};

/// Deserializes `json` into `T`, reporting whether it was accepted.
fn accepts<T: serde::de::DeserializeOwned>(json: &str) -> bool {
    serde_json::from_str::<T>(json).is_ok()
}

// ---------------------------------------------------------------------------------------
// Bounded text: the byte bound and the NUL rule must hold on the way in.
// ---------------------------------------------------------------------------------------

#[test]
fn a_scope_cannot_arrive_over_the_wire_with_a_character_the_constructor_refuses() {
    assert!(accepts::<Scope>(r#""fs.read""#));
    // Each of these is refused by `Scope::new` and must be refused by the deserializer too. The control
    // character is the one with teeth: a scope reaches a persisted grant and an operator display, and a
    // derived impl would have carried a newline or an escape straight into both.
    for bad in [
        r#""""#,
        r#""Fs.Read""#,
        r#""fs read""#,
        r#""fs\nread""#,
        r#""fs\u0007read""#,
        r#""fs/send""#,
    ] {
        assert!(
            !accepts::<Scope>(bad),
            "{bad} must be refused by the deserializer, not only by the constructor",
        );
    }
}

#[test]
fn a_transition_reason_cannot_arrive_over_the_wire_unbounded_or_nul_carrying() {
    // A reason is written by a caller and **read back from an audit row**, so the reader is the path an
    // operator's own tooling takes. A derived impl let an over-long or NUL-carrying reason into the
    // record through that reader — the bound protecting the writer rather than the record.
    assert!(accepts::<TransitionReason>(r#""started""#));
    assert!(!accepts::<TransitionReason>(r#""""#));
    assert!(!accepts::<TransitionReason>(r#""has\u0000nul""#));
    let over = format!("\"{}\"", "a".repeat(MAX_REASON_BYTES + 1));
    assert!(!accepts::<TransitionReason>(&over));
    let at_bound = format!("\"{}\"", "a".repeat(MAX_REASON_BYTES));
    assert!(accepts::<TransitionReason>(&at_bound));
}

#[test]
fn a_json_document_cannot_arrive_over_the_wire_unbounded_or_nul_carrying() {
    // Pre-existing: written before the audit, and fixed by it. A document beyond the bound reaching the
    // gateway as a requested output schema would spend a context budget the bound exists to cap.
    //
    // **The document is carried as a JSON *string*, not as a bare object** — `#[serde(transparent)]`
    // makes the wire form the inner string — so the fixture is an escaped document rather than
    // `{"a":1}`. The first version of this line used the bare object and failed, which is the wire form
    // correcting the test rather than the other way round.
    assert!(accepts::<JsonText>(r#""{\"a\":1}""#));
    assert!(!accepts::<JsonText>(r#""""#));
    assert!(!accepts::<JsonText>(r#""nul\u0000here""#));
    let over = format!("\"{}\"", "a".repeat(MAX_JSON_TEXT_BYTES + 1));
    assert!(!accepts::<JsonText>(&over));
}

#[test]
fn tool_arguments_and_result_payloads_keep_their_own_bounds_on_the_way_in() {
    // **Two different bounds, and the deserializer must respect the right one for each.** A value
    // between them is legal as a result payload and illegal as an argument; a single shared
    // deserializer would have been the "one type governs both bounds" defect again, at the wire.
    let between = format!("\"{}\"", "x".repeat(MAX_ARGUMENT_BYTES + 1));
    assert!(
        !accepts::<ToolArguments>(&between),
        "a document over the ARGUMENT bound must be refused as arguments",
    );
    assert!(
        accepts::<ResultPayload>(&between),
        "and accepted as a result payload, because its bound is larger",
    );
    let over_result = format!("\"{}\"", "x".repeat(MAX_RESULT_BYTES + 1));
    assert!(
        !accepts::<ResultPayload>(&over_result),
        "a payload over the RESULT bound must be refused",
    );
    // And both refuse a NUL and an empty document, since both go through the same shared text rule.
    for bad in [r#""""#, r#""nul\u0000""#] {
        assert!(!accepts::<ToolArguments>(bad), "{bad} as arguments");
        assert!(!accepts::<ResultPayload>(bad), "{bad} as a result payload");
    }
}

#[test]
fn a_server_config_id_and_a_list_version_cannot_arrive_over_the_wire_malformed() {
    // Both are part of a **cache key**. A malformed one arriving from a stored record would produce a key
    // that no lookup matches — so the cache would miss forever — or, worse, a key that collides with
    // another scope's if the rule the constructor enforces was the thing distinguishing them.
    assert!(accepts::<ServerConfigId>(r#""files-primary""#));
    assert!(!accepts::<ServerConfigId>(r#""""#));
    assert!(!accepts::<ServerConfigId>(r#""Files Primary""#));
    assert!(!accepts::<ServerConfigId>(r#""files\u0000primary""#));

    assert!(accepts::<ListVersion>(r#""rev-1""#));
    assert!(!accepts::<ListVersion>(r#""""#));
    assert!(!accepts::<ListVersion>(r#""rev\n1""#));
}

#[test]
fn an_empty_channel_set_cannot_arrive_over_the_wire() {
    // **The instance with the clearest consequence.** An approval with no permitted channel can never be
    // decided, so it would sit `Pending` until it expired — the "no legal way out" shape this project has
    // found seven times. The constructor refuses the empty set to make that unrepresentable; a derived
    // deserializer restored it, so a stored or received approval could carry the exact state the
    // constructor exists to prevent.
    assert!(accepts::<AllowedChannels>(r#"["cli"]"#));
    assert!(accepts::<AllowedChannels>(r#"["cli","desktop"]"#));
    assert!(
        !accepts::<AllowedChannels>("[]"),
        "an empty permission set must be refused on the way in, or the unreachable state returns",
    );
    // And an unknown channel name is refused as well, so the closed set is enforced on the wire rather
    // than only at construction.
    assert!(!accepts::<AllowedChannels>(r#"["telegram"]"#));
}

// ---------------------------------------------------------------------------------------
// The valid values still round-trip.
// ---------------------------------------------------------------------------------------

#[test]
fn every_fixed_type_still_round_trips_a_valid_value() {
    // The positive control for every test above: a deserializer that refused *everything* would satisfy
    // each of them, and the fix would then be worse than the defect.
    let scope = serde_json::to_string(&Scope::new("fs.read").expect("valid")).expect("serializes");
    assert_eq!(
        serde_json::from_str::<Scope>(&scope).expect("deserializes"),
        Scope::new("fs.read").expect("valid"),
    );

    let reason =
        serde_json::to_string(&TransitionReason::new("started").expect("valid")).expect("ser");
    assert_eq!(
        serde_json::from_str::<TransitionReason>(&reason).expect("de"),
        TransitionReason::new("started").expect("valid"),
    );

    let document =
        serde_json::to_string(&JsonText::new(r#"{"a":1}"#).expect("valid")).expect("ser");
    assert_eq!(
        serde_json::from_str::<JsonText>(&document).expect("de"),
        JsonText::new(r#"{"a":1}"#).expect("valid"),
    );

    let server = serde_json::to_string(&ServerConfigId::new("files").expect("valid")).expect("ser");
    assert_eq!(
        serde_json::from_str::<ServerConfigId>(&server).expect("de"),
        ServerConfigId::new("files").expect("valid"),
    );

    let version = serde_json::to_string(&ListVersion::new("rev-1").expect("valid")).expect("ser");
    assert_eq!(
        serde_json::from_str::<ListVersion>(&version).expect("de"),
        ListVersion::new("rev-1").expect("valid"),
    );

    let channels = AllowedChannels::new(vec![ApprovalChannel::Cli]).expect("valid");
    let channels_json = serde_json::to_string(&channels).expect("ser");
    assert_eq!(
        serde_json::from_str::<AllowedChannels>(&channels_json).expect("de"),
        channels,
    );

    let arguments = ToolArguments::new("{}").expect("valid");
    let arguments_json = serde_json::to_string(&arguments).expect("ser");
    assert_eq!(
        serde_json::from_str::<ToolArguments>(&arguments_json).expect("de"),
        arguments,
    );

    let payload = ResultPayload::new("{}").expect("valid");
    let payload_json = serde_json::to_string(&payload).expect("ser");
    assert_eq!(
        serde_json::from_str::<ResultPayload>(&payload_json).expect("de"),
        payload,
    );
}

#[test]
fn a_type_that_holds_a_validated_value_rejects_a_bad_one_nested_inside_it() {
    // The systemic version of the rule: the defect mattered because these types are **fields** of
    // contract types, so a bad value reached through a parent. A `ContentBlock` whose payload is
    // oversized must be refused as a whole, not only when the payload is deserialized directly.
    let oversized = format!(
        r#"{{"type":"text","text":"{}"}}"#,
        "x".repeat(MAX_RESULT_BYTES + 1)
    );
    assert!(
        !accepts::<super::call::ContentBlock>(&oversized),
        "a block carrying an out-of-bounds payload must be refused through its parent",
    );
    // And a well-formed block still deserializes, so the refusal is about the bound.
    assert!(accepts::<super::call::ContentBlock>(
        r#"{"type":"text","text":"ok"}"#
    ));
}
