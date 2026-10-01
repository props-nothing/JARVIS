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
//!
//! **A second round found the same defect reached by a different route, which is why the first sweep's
//! filter was incomplete.** Round 90 looked for `#[serde(transparent)]`, so it could not see
//! `PreviewItem` (a struct with public fields and a derived impl) or `ApprovalSummary` (which did not
//! exist yet as a type — the summary was a bare `String` whose bound, `MAX_SUMMARY_BYTES`, was checked
//! nowhere in production). The rule was never about the attribute: a type with a validating constructor
//! or a documented bound must have **some** path from the wire through that check, and "derived
//! `Deserialize`" is only one of the ways to skip it.
//!
//! **A third round widened the sweep from newtypes to every type with a constructor that returns
//! `Result<Self>`**, and found nine more in `jarvis-domain` alone. These are ordinary structs whose
//! fields are read but whose construction — the *cross-field* rules included — the derived impl skipped
//! entirely. Two candidates the audit flagged are **deliberately left derived** and asserted here by
//! their reasoning rather than by a test: a type read only through a validating `parse` has no
//! unvalidated path, and a custom impl would collapse that parse's precise error variants into one
//! `serde` error.

use super::approval::{
    AllowedChannels, ApprovalChannel, ApprovalPreview, ApprovalSummary, MAX_PREVIEW_ITEMS,
    MAX_PREVIEW_TEXT_BYTES, MAX_SUMMARY_BYTES, PreviewItem,
};
use super::call::{
    MAX_ARGUMENT_BYTES, MAX_PROVIDER_REFERENCE_BYTES, MAX_RESULT_BLOCKS, MAX_RESULT_BYTES,
    ResultPayload, ToolArguments, ToolCallIntent, ToolResultBody,
};
use super::classification::{
    DataClasses, ExecutionDefaults, MAX_TOOL_ATTEMPTS, MAX_TOOL_TIMEOUT_MS, Risk, Scope,
};
use super::definition::ToolDefinition;
use super::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use super::ledger::ReservationKey;
use super::path_grant::{PathGrant, PathMode, WorkspaceRelativePath};
use super::registry::{ListVersion, ServerConfigId};
use crate::ids::{PrincipalId, ToolCallId, WorkspaceId};
use crate::model::stream::{JsonText, MAX_JSON_TEXT_BYTES};
use crate::run::retry::MAX_ATTEMPTS;
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

#[test]
fn an_approval_preview_cannot_arrive_over_the_wire_unvalidated() {
    // **Round 90's sweep looked for `#[serde(transparent)]`, so it missed these two by attribute.** They
    // are the same class of defect by a different route: each has a validating constructor and each
    // derived — or, for `PreviewItem`, a struct with public fields and a derived impl — let a value the
    // constructor refuses in. A preview is **read back from `preview_json` on every approval load**, and
    // it is rendered inside the prompt a user reads to decide, so the reader is a path this value takes.
    assert!(accepts::<PreviewItem>(
        r#"{"key":"To","value":"peter@example.com"}"#
    ));
    // An empty half is refused, so a row that shows nothing cannot reach the prompt.
    assert!(!accepts::<PreviewItem>(r#"{"key":"","value":"x"}"#));
    assert!(!accepts::<PreviewItem>(r#"{"key":"To","value":""}"#));
    // A control character is refused: a preview is the one place a crafted string could make the prompt
    // misrepresent the action, which is why the rule is there rather than cosmetic.
    assert!(!accepts::<PreviewItem>(
        r#"{"key":"To","value":"bell\u0007"}"#
    ));
    // Over the text bound is refused.
    let long = "x".repeat(MAX_PREVIEW_TEXT_BYTES + 1);
    assert!(!accepts::<PreviewItem>(&format!(
        r#"{{"key":"To","value":"{long}"}}"#
    )));

    // The count bound holds through the whole preview, which the derived transparent impl did not enforce.
    let one = r#"{"key":"To","value":"x"}"#;
    let at_bound = format!("[{}]", vec![one; MAX_PREVIEW_ITEMS].join(","));
    assert!(accepts::<ApprovalPreview>(&at_bound));
    let over = format!("[{}]", vec![one; MAX_PREVIEW_ITEMS + 1].join(","));
    assert!(
        !accepts::<ApprovalPreview>(&over),
        "a preview over the item bound must be refused on the way in",
    );
    // An empty preview is accepted, because whether a preview must show something is about the tool
    // rather than a domain invariant — the same boundary `ApprovalPreview::new` documents.
    assert!(accepts::<ApprovalPreview>("[]"));
}

#[test]
fn an_approval_summary_cannot_arrive_over_the_wire_untested_for_its_own_bound() {
    // **The bound that was checked nowhere.** `MAX_SUMMARY_BYTES` and the control-character rule existed
    // and `is_usable_summary` encoded them, but the summary was a bare `String` on both the request and
    // the record, so no production path enforced either — a summary is the line a user reads in the
    // consent prompt, and it was bounded only by the caller's restraint.
    assert!(accepts::<ApprovalSummary>(r#""Send one email""#));
    assert!(!accepts::<ApprovalSummary>(r#""""#));
    assert!(!accepts::<ApprovalSummary>(r#""bell\u0007""#));
    let over = format!("\"{}\"", "a".repeat(MAX_SUMMARY_BYTES + 1));
    assert!(!accepts::<ApprovalSummary>(&over));
    let at_bound = format!("\"{}\"", "a".repeat(MAX_SUMMARY_BYTES));
    assert!(accepts::<ApprovalSummary>(&at_bound));
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

    let summary = ApprovalSummary::new("Send one email").expect("valid");
    let summary_json = serde_json::to_string(&summary).expect("ser");
    assert_eq!(
        serde_json::from_str::<ApprovalSummary>(&summary_json).expect("de"),
        summary,
    );

    let item = PreviewItem::new("To", "peter@example.com").expect("valid");
    let item_json = serde_json::to_string(&item).expect("ser");
    assert_eq!(
        serde_json::from_str::<PreviewItem>(&item_json).expect("de"),
        item,
    );

    let preview = ApprovalPreview::new(vec![item]).expect("valid");
    let preview_json = serde_json::to_string(&preview).expect("ser");
    assert_eq!(
        serde_json::from_str::<ApprovalPreview>(&preview_json).expect("de"),
        preview,
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

// ---------------------------------------------------------------------------------------
// Composed structs: an ordinary struct whose constructor enforces a rule the derived impl skipped.
// ---------------------------------------------------------------------------------------

/// A valid `ToolSource` JSON document, to be mutated one field at a time.
const VALID_SOURCE: &str = r#"{"kind":"connector","owner":"acme.mail","version":"1.0.0"}"#;

#[test]
fn a_tool_source_cannot_arrive_over_the_wire_with_an_owner_the_constructor_refuses() {
    // `ToolSource` is a field of `ToolIdentity`, which is read back from `tool_identity_json` on every
    // approval and ledger load. The owner is what makes a same-named tool from another publisher a
    // *different* tool (`ACC-024`), so a malformed one is an identity defect rather than a cosmetic one.
    assert!(accepts::<ToolSource>(VALID_SOURCE));
    assert!(!accepts::<ToolSource>(
        &VALID_SOURCE.replace("acme.mail", "")
    ));
    assert!(!accepts::<ToolSource>(
        &VALID_SOURCE.replace("acme.mail", "Acme.Mail")
    ));
    assert!(!accepts::<ToolSource>(
        &VALID_SOURCE.replace("acme.mail", "a..b")
    ));
    assert!(!accepts::<ToolSource>(
        &VALID_SOURCE.replace("acme.mail", "1bad")
    ));
}

#[test]
fn data_classes_cannot_arrive_over_the_wire_laundering_a_classification() {
    // The rule is a cross-field one — output never below input — so a derived impl was the *only* thing
    // standing between a document and the forbidden pair. Refused, because lowering an output label is
    // not derivable from the input and would let a tool declare a smaller classification than it handles.
    assert!(accepts::<DataClasses>(
        r#"{"input":"internal","output":"confidential"}"#
    ));
    assert!(accepts::<DataClasses>(
        r#"{"input":"internal","output":"internal"}"#
    ));
    assert!(!accepts::<DataClasses>(
        r#"{"input":"confidential","output":"public"}"#
    ));
}

#[test]
fn execution_defaults_cannot_arrive_over_the_wire_zeroed_or_over_bounded() {
    // A zero timeout or a zero attempt count is not an execution, and the upper bounds are what keep a
    // definition from claiming a longer default than a run step may take.
    assert!(accepts::<ExecutionDefaults>(
        r#"{"timeout_ms":1000,"max_attempts":2}"#
    ));
    assert!(!accepts::<ExecutionDefaults>(
        r#"{"timeout_ms":0,"max_attempts":2}"#
    ));
    assert!(!accepts::<ExecutionDefaults>(
        r#"{"timeout_ms":1000,"max_attempts":0}"#
    ));
    let over_timeout = format!(
        r#"{{"timeout_ms":{},"max_attempts":2}}"#,
        MAX_TOOL_TIMEOUT_MS + 1
    );
    assert!(!accepts::<ExecutionDefaults>(&over_timeout));
    let over_attempts = format!(
        r#"{{"timeout_ms":1000,"max_attempts":{}}}"#,
        MAX_TOOL_ATTEMPTS + 1
    );
    assert!(!accepts::<ExecutionDefaults>(&over_attempts));
}

#[test]
fn a_path_grant_cannot_arrive_over_the_wire_conferring_nothing_or_twice() {
    // An empty mode list is recorded as access the user gave and authorizes nothing; a duplicate is a
    // list narrowed from what a reviewer read. Both are refused by the constructor, so both must be
    // refused on the way in — a path grant is the shape a grant store will read.
    assert!(accepts::<PathGrant>(
        r#"{"root":"data/notes","modes":["read"]}"#
    ));
    assert!(!accepts::<PathGrant>(r#"{"root":"data/notes","modes":[]}"#));
    assert!(!accepts::<PathGrant>(
        r#"{"root":"data/notes","modes":["read","read"]}"#
    ));
    // And the root is a validating newtype in its own right, so a traversal cannot ride in through it.
    assert!(!accepts::<PathGrant>(
        r#"{"root":"../escape","modes":["read"]}"#
    ));
}

/// A valid `ReservationKey` JSON document, to be mutated one field at a time.
fn valid_key_json(idempotency_key: &str) -> String {
    format!(
        concat!(
            r#"{{"identity":{{"capability":"fs.read@1","#,
            r#""source":{{"kind":"connector","owner":"acme.files","version":"1.0.0"}},"#,
            r#""schema_fingerprint":"sha256:{}"}},"#,
            r#""workspace":"{}","principal":"{}","idempotency_key":"{}"}}"#,
        ),
        "0".repeat(64),
        WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
        PrincipalId::from_uuid(uuid::Uuid::from_u128(2)),
        idempotency_key,
    )
}

#[test]
fn a_reservation_key_cannot_arrive_over_the_wire_with_an_empty_idempotency_key() {
    // An empty key would make every unkeyed call collide with every other — precisely the duplicate the
    // reservation exists to prevent — so the rule must hold for a key read from a durable row, which is
    // the type the five-part uniqueness guarantee is stated over.
    assert!(accepts::<ReservationKey>(&valid_key_json("key-1")));
    assert!(!accepts::<ReservationKey>(&valid_key_json("")));
}

#[test]
fn a_tool_definition_cannot_arrive_over_the_wire_the_constructor_would_refuse() {
    // **The instance whose own doc claimed the opposite.** `ToolDefinition` said "constructed only
    // through `new`, so every instance satisfies the cross-field rules above" — while deriving
    // `Deserialize`, which built one field at a time and enforced none of them. It is a field of
    // `DiscoveredCatalog`, so a persisted or MCP-provided catalog is exactly the document that reached
    // it. Each bad document below is one rule `new` owns.
    let base = r#"{"identity":{"capability":"fs.read@1","source":{"kind":"connector","owner":"acme.files","version":"1.0.0"},"schema_fingerprint":"sha256:0000000000000000000000000000000000000000000000000000000000000000"},"display_name":"Read a file","purpose":"Read one file.","source_version":"1.0.0","effects":["read_only"],"risk":"low","required_scopes":[],"default_approval":"ask","idempotency":"naturally_idempotent","data_classes":{"input":"internal","output":"internal"},"execution":{"timeout_ms":1000,"max_attempts":1}}"#;
    assert!(accepts::<ToolDefinition>(base), "the fixture must be valid");
    // An empty effect list, refused rather than defaulted to `read_only`.
    assert!(!accepts::<ToolDefinition>(
        &base.replace(r#""effects":["read_only"]"#, r#""effects":[]"#)
    ));
    // A duplicate effect.
    assert!(!accepts::<ToolDefinition>(&base.replace(
        r#""effects":["read_only"]"#,
        r#""effects":["write","write"]"#
    )));
    // `read_only` combined with another effect is a contradiction.
    assert!(!accepts::<ToolDefinition>(&base.replace(
        r#""effects":["read_only"]"#,
        r#""effects":["read_only","write"]"#
    )));
    // An empty purpose.
    assert!(!accepts::<ToolDefinition>(
        &base.replace(r#""purpose":"Read one file.""#, r#""purpose":""#)
    ));
    // An unusable display name.
    assert!(!accepts::<ToolDefinition>(&base.replace(
        r#""display_name":"Read a file""#,
        r#""display_name":""#
    )));
    // A capability major that disagrees with the source release major.
    assert!(!accepts::<ToolDefinition>(&base.replace(
        r#""capability":"fs.read@1""#,
        r#""capability":"fs.read@2""#
    )));
    // A consequential tool that defaults to `Allow` with no scopes.
    assert!(!accepts::<ToolDefinition>(
        &base.replace(r#""risk":"low""#, r#""risk":"high""#).replace(
            r#""default_approval":"ask""#,
            r#""default_approval":"allow""#
        )
    ));
}

#[test]
fn a_tool_result_body_cannot_arrive_over_the_wire_empty_or_over_the_total_bound() {
    // The total byte bound is the one the constructor's own doc calls "the bound with teeth": a
    // per-block bound alone admits `MAX_RESULT_BLOCKS` blocks each at the block limit. A derived impl
    // rebuilt an empty or over-total result for any document that carried one.
    assert!(accepts::<ToolResultBody>(
        r#"{"content":[{"type":"text","text":"ok"}],"sensitivity":"internal"}"#
    ));
    // An empty block list is not a result.
    assert!(!accepts::<ToolResultBody>(
        r#"{"content":[],"sensitivity":"internal"}"#
    ));
    // More blocks than the list bound.
    let block = r#"{"type":"text","text":"ok"}"#;
    let over_blocks = format!(
        r#"{{"content":[{}],"sensitivity":"internal"}}"#,
        vec![block; MAX_RESULT_BLOCKS + 1].join(",")
    );
    assert!(!accepts::<ToolResultBody>(&over_blocks));
    // Within the per-block bound but over the TOTAL: the case the count bound cannot catch. Each block's
    // payload is at the block limit, so two of them exceed the total if the total is the tighter sum.
    let per_block = format!(r#"{{"type":"json","value":"{}"}}"#, "x".repeat(200 * 1024));
    let over_total = format!(
        r#"{{"content":[{}],"sensitivity":"internal"}}"#,
        vec![per_block; 2].join(",")
    );
    assert!(!accepts::<ToolResultBody>(&over_total));
}

#[test]
fn a_tool_call_intent_cannot_arrive_over_the_wire_unbounded() {
    // The capability and the reason summary are bounded only in the constructor, and both reach an
    // operator-visible surface: an unbounded capability is an unbounded lookup key and log field, and
    // the reason summary reaches a log line and an audit record.
    let call_id = ToolCallId::from_uuid(uuid::Uuid::from_u128(3));
    assert!(accepts::<ToolCallIntent>(&format!(
        r#"{{"call_id":"{call_id}","capability":"fs.read","arguments":"{{}}"}}"#
    )));
    // An empty capability.
    assert!(!accepts::<ToolCallIntent>(&format!(
        r#"{{"call_id":"{call_id}","capability":"","arguments":"{{}}"}}"#
    )));
    // A capability beyond the bound.
    let long = "a".repeat(super::identity::MAX_TOOL_SEGMENT_BYTES * 3 + 1);
    assert!(!accepts::<ToolCallIntent>(&format!(
        r#"{{"call_id":"{call_id}","capability":"{long}","arguments":"{{}}"}}"#
    )));
    // A reason summary beyond the bound, which reaches a log line and an audit record.
    let long_summary = "a".repeat(MAX_PROVIDER_REFERENCE_BYTES + 1);
    assert!(!accepts::<ToolCallIntent>(&format!(
        r#"{{"call_id":"{call_id}","capability":"fs.read","arguments":"{{}}","reason_summary":"{long_summary}"}}"#
    )));
}

#[test]
fn a_retry_policy_cannot_arrive_over_the_wire_out_of_range() {
    // **The one of these that is actually stored.** `RetryPolicy` is a field of `RunBudget`, read back
    // from `agent_runs.budget_json` on every run load, so a derived impl let a zero attempt count (a run
    // that can never make a call) or a backoff above the ceiling reach the controller's retry decision.
    assert!(accepts::<crate::run::retry::RetryPolicy>(
        r#"{"max_attempts":3,"base_backoff_ms":250,"max_backoff_ms":2000}"#
    ));
    assert!(!accepts::<crate::run::retry::RetryPolicy>(
        r#"{"max_attempts":0,"base_backoff_ms":250,"max_backoff_ms":2000}"#
    ));
    let over = format!(
        r#"{{"max_attempts":{},"base_backoff_ms":250,"max_backoff_ms":2000}}"#,
        MAX_ATTEMPTS + 1
    );
    assert!(!accepts::<crate::run::retry::RetryPolicy>(&over));
    // A base above the ceiling is refused rather than clamped, because a clamped value is one the caller
    // did not choose and cannot detect.
    assert!(!accepts::<crate::run::retry::RetryPolicy>(
        r#"{"max_attempts":3,"base_backoff_ms":5000,"max_backoff_ms":2000}"#
    ));
    // A backoff above the hard ceiling.
    let over_backoff = format!(
        r#"{{"max_attempts":3,"base_backoff_ms":250,"max_backoff_ms":{}}}"#,
        crate::run::retry::MAX_BACKOFF_MS + 1
    );
    assert!(!accepts::<crate::run::retry::RetryPolicy>(&over_backoff));
}

/// Asserts that a value survives `serde_json` unchanged.
fn round_trips<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serializes");
    let restored = serde_json::from_str::<T>(&json).expect("deserializes");
    assert_eq!(restored, *value, "a valid value must round-trip");
}

/// A valid identity fixture for the composed-type round trips.
fn sample_identity() -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::parse("fs.read@1").expect("valid"),
        source: ToolSource::new(
            SourceKind::Connector,
            "acme.files",
            ToolVersion::parse("1.0.0").expect("valid"),
        )
        .expect("valid"),
        schema_fingerprint: SchemaFingerprint::parse(&format!("sha256:{}", "0".repeat(64)))
            .expect("valid"),
    }
}

#[test]
fn every_composed_type_still_round_trips_a_valid_value() {
    // The positive control, for the same reason as the newtype one: a deserializer that refused
    // everything would satisfy every assertion above.
    round_trips(
        &ToolSource::new(
            SourceKind::Connector,
            "acme.mail",
            ToolVersion::parse("1.0.0").expect("valid"),
        )
        .expect("valid"),
    );
    round_trips(
        &DataClasses::new(
            crate::model::policy::Sensitivity::Internal,
            crate::model::policy::Sensitivity::Confidential,
        )
        .expect("valid"),
    );
    round_trips(&ExecutionDefaults::new(1000, 2).expect("valid"));
    round_trips(
        &PathGrant::new(
            WorkspaceRelativePath::parse("data/notes").expect("valid"),
            vec![PathMode::Read],
        )
        .expect("valid"),
    );
    round_trips(
        &ReservationKey::new(
            sample_identity(),
            WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
            PrincipalId::from_uuid(uuid::Uuid::from_u128(2)),
            "key-1",
        )
        .expect("valid"),
    );
    round_trips(&crate::run::retry::RetryPolicy::new(3, 250, 2000).expect("valid"));
    round_trips(
        &ToolResultBody::new(
            vec![super::call::ContentBlock::text("ok").expect("valid")],
            None,
            crate::model::policy::Sensitivity::Internal,
        )
        .expect("valid"),
    );
    round_trips(
        &ToolCallIntent::new(
            ToolCallId::from_uuid(uuid::Uuid::from_u128(3)),
            "fs.read",
            ToolArguments::new("{}").expect("valid"),
            Some("because"),
        )
        .expect("valid"),
    );
    // A definition written by `new`, round-tripped: the check that a `ToolDefinition` reaching this
    // crate's boundary can survive storage and come back unchanged.
    round_trips(
        &ToolDefinition::new(
            sample_identity(),
            "Read a file",
            "Read one file.",
            vec![super::classification::Effect::ReadOnly],
            Risk::Low,
            Vec::new(),
            super::classification::ApprovalHint::Ask,
            super::classification::Idempotency::NaturallyIdempotent,
            DataClasses::new(
                crate::model::policy::Sensitivity::Internal,
                crate::model::policy::Sensitivity::Internal,
            )
            .expect("valid"),
            ExecutionDefaults::new(1000, 1).expect("valid"),
        )
        .expect("valid"),
    );
}
