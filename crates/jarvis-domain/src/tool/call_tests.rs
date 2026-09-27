//! Tests for tool error classes, call intents, and bounded result storage.
//!
//! The subject with the most riding on it is [`ToolErrorClass::retryable_for`], because the wrong
//! answer in one direction sends a message twice. Every test here that asserts a retry decision
//! names the **other** answer it is not making, since a test that only asserted `true` would pass
//! against an implementation that always said `true`.

use super::call::{
    ContentBlock, MAX_ARGUMENT_BYTES, MAX_PROVIDER_REFERENCE_BYTES, MAX_RESULT_BLOCKS,
    MAX_RESULT_BYTES, ToolArguments, ToolCallIntent, ToolResultBody,
};
use super::classification::Idempotency;
use super::error_class::{RetryPosture, ToolErrorClass};
use crate::error::DomainError;
use crate::ids::ToolCallId;
use crate::model::policy::Sensitivity;

fn call_id() -> ToolCallId {
    ToolCallId::from_uuid(uuid::Uuid::from_u128(7))
}

fn arguments(value: &str) -> ToolArguments {
    ToolArguments::new(value).expect("the fixture arguments are usable")
}

/// The field a refusal named, or `None`. `DomainError` is deliberately not `PartialEq`.
fn field_of<T>(result: &Result<T, DomainError>) -> Option<&'static str> {
    match result.as_ref().err() {
        Some(DomainError::ToolDefinitionInvalid { field }) => Some(field),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Error classes: the closed set.
// ---------------------------------------------------------------------------------------

#[test]
fn every_error_class_parses_from_its_contract_code_and_the_set_is_sixteen() {
    // The positive control for the refusal test below, and the guard on the table: iterating the
    // constants means a code added without a parse arm fails here rather than being unreachable.
    for class in ToolErrorClass::ALL {
        assert_eq!(
            ToolErrorClass::parse(class.as_contract_str()).ok(),
            Some(*class),
            "{} must parse back to itself",
            class.as_contract_str(),
        );
    }
    assert_eq!(
        ToolErrorClass::ALL.len(),
        16,
        "the tool contract lists sixteen error classes",
    );
}

#[test]
fn an_error_class_outside_the_contract_is_refused() {
    // **Refused rather than mapped to a catch-all.** A provider's own error text must be mapped by
    // its adapter, and refusing an unknown code is what makes that mapping mandatory: a class that
    // fell through as `Unknown` would reach a client as a code the contract does not define.
    for value in [
        "tool.something_else",
        "provider.rate_limit",
        "RATE_LIMITED",
        "",
        "tool.",
    ] {
        assert_eq!(
            field_of(&ToolErrorClass::parse(value)),
            Some("error_class"),
            "{value:?} must be refused",
        );
    }
}

#[test]
fn the_error_class_codes_are_namespaced_and_unique() {
    // These codes cross the emission boundary, which accepts a code whose first segment is a
    // namespace JARVIS owns. `tool` is one of the nine, so each of these survives — and asserting it
    // through the boundary type is what checks the agreement rather than restating the list.
    let mut codes: Vec<&str> = ToolErrorClass::ALL
        .iter()
        .map(|class| class.as_contract_str())
        .collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(
        codes.len(),
        ToolErrorClass::ALL.len(),
        "codes must be unique"
    );
    for code in codes {
        assert_eq!(
            crate::error::ErrorCode::new(code).as_str(),
            code,
            "{code} must be accepted by the boundary rather than rewritten",
        );
        assert!(
            code.starts_with("tool."),
            "{code} must be namespaced under tool"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Retry posture: where the danger lives.
// ---------------------------------------------------------------------------------------

#[test]
fn a_class_refused_before_the_effect_is_safe_to_retry_for_any_tool() {
    // Unavailable and RateLimited both mean the request was refused **before** anything happened, so
    // there is nothing to duplicate and the tool's own declaration is irrelevant. Asserted for all
    // three idempotency values, because `Safe` resolving differently per tool would defeat the
    // posture: the point is that a tool which declared nothing is still retryable here.
    for class in [ToolErrorClass::Unavailable, ToolErrorClass::RateLimited] {
        assert_eq!(class.retry_posture(), RetryPosture::Safe);
        for idempotency in [
            Idempotency::None,
            Idempotency::CallerKeyed,
            Idempotency::NaturallyIdempotent,
        ] {
            assert!(
                class.retryable_for(idempotency),
                "{class} is refused before the effect, so {idempotency} does not matter",
            );
        }
    }
}

#[test]
fn an_unsettled_class_needs_the_tools_own_declaration() {
    // **This is the resolution the posture exists for.** A timeout or provider error may or may not
    // have taken effect, so the same class answers differently for two tools — and the tool whose
    // author declared nothing (`Idempotency::None`, the default) is the one that must not be
    // retried. Asserted in both directions for each, since a test that only asserted the `true` half
    // would pass against an implementation that always said `true`, which is the direction that
    // sends twice.
    for class in [ToolErrorClass::Timeout, ToolErrorClass::ProviderError] {
        assert_eq!(class.retry_posture(), RetryPosture::OnlyIfIdempotent);
        assert!(
            class.retryable_for(Idempotency::CallerKeyed),
            "{class} on a caller-keyed tool may be retried when the same key is reused",
        );
        assert!(
            class.retryable_for(Idempotency::NaturallyIdempotent),
            "{class} on a convergent tool may be retried",
        );
        assert!(
            !class.retryable_for(Idempotency::None),
            "{class} on a tool that declared no repetition story must NOT be retried — a second \
             send of the same message is not a retry but a second message",
        );
    }
}

#[test]
fn every_class_that_fails_again_unchanged_is_never_retried_for_any_tool() {
    // The remaining eleven classes. Asserted against **all three** idempotency values, including the
    // permissive ones: a refusal that consulted the tool's declaration would let an idempotent tool
    // retry a rejected approval, which needs a new decision rather than a repeat.
    let never: [ToolErrorClass; 11] = [
        ToolErrorClass::NotFound,
        ToolErrorClass::SchemaInvalid,
        ToolErrorClass::PermissionDenied,
        ToolErrorClass::ApprovalRequired,
        ToolErrorClass::ApprovalRejected,
        ToolErrorClass::ApprovalExpired,
        ToolErrorClass::Conflict,
        ToolErrorClass::Cancelled,
        ToolErrorClass::ProviderAuth,
        ToolErrorClass::OutputInvalid,
        ToolErrorClass::OutcomeAmbiguous,
    ];
    for class in never {
        assert_eq!(
            class.retry_posture(),
            RetryPosture::Never,
            "{class} must not permit a retry",
        );
        for idempotency in [
            Idempotency::None,
            Idempotency::CallerKeyed,
            Idempotency::NaturallyIdempotent,
        ] {
            assert!(
                !class.retryable_for(idempotency),
                "{class} fails again unchanged for {idempotency}",
            );
        }
    }
    // `LimitExceeded` is the sixteenth and is checked separately only because it is the one whose
    // posture could plausibly be argued either way: a result larger than the bound will be larger
    // again, so a retry cannot succeed, and retrying would spend a budget to fail identically.
    assert_eq!(
        ToolErrorClass::LimitExceeded.retry_posture(),
        RetryPosture::Never
    );
    assert!(!ToolErrorClass::LimitExceeded.retryable_for(Idempotency::NaturallyIdempotent));
}

#[test]
fn an_ambiguous_outcome_is_unsettled_but_never_retryable() {
    // The two questions are not opposites, and this is the case that proves it. `OutcomeAmbiguous`
    // means the effect may have happened, so the call is unsettled and must be reconciled — while
    // being the one class the contract explicitly says must **not** be retried automatically. An
    // implementation that derived retryability from unsettledness would get this exactly backwards.
    assert!(ToolErrorClass::OutcomeAmbiguous.is_unsettled());
    assert_eq!(
        ToolErrorClass::OutcomeAmbiguous.retry_posture(),
        RetryPosture::Never,
    );
    assert!(!ToolErrorClass::OutcomeAmbiguous.retryable_for(Idempotency::NaturallyIdempotent));
}

#[test]
fn unsettledness_is_classified_for_every_class_deliberately() {
    // A closure rather than a list of the three, so a class added later must be classified here and
    // fails rather than defaulting to settled. A default of "settled" would let an unknown outcome be
    // reported as a definite failure, which is the direction that loses the reconciliation.
    for class in ToolErrorClass::ALL {
        let expected = matches!(
            class,
            ToolErrorClass::Timeout
                | ToolErrorClass::ProviderError
                | ToolErrorClass::OutcomeAmbiguous
        );
        assert_eq!(
            class.is_unsettled(),
            expected,
            "{class} unsettledness must be classified deliberately",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Arguments.
// ---------------------------------------------------------------------------------------

#[test]
fn an_argument_document_is_bounded_by_bytes_and_refuses_an_empty_one() {
    assert!(ToolArguments::new("{}").is_ok());
    assert_eq!(
        field_of(&ToolArguments::new("")),
        Some("arguments"),
        "an empty argument document is what an unpopulated field produces",
    );
    assert_eq!(field_of(&ToolArguments::new("a\0b")), Some("arguments"));
    assert!(ToolArguments::new(&"a".repeat(MAX_ARGUMENT_BYTES)).is_ok());
    assert_eq!(
        field_of(&ToolArguments::new(&"a".repeat(MAX_ARGUMENT_BYTES + 1))),
        Some("arguments"),
    );
    // The bound is on **bytes**, so a multi-byte string reaches it sooner. Asserted explicitly,
    // because a character-counted bound would accept this and the difference is a factor of four.
    let multi_byte = "é".repeat(MAX_ARGUMENT_BYTES / 2 + 1);
    assert!(
        multi_byte.chars().count() <= MAX_ARGUMENT_BYTES,
        "the fixture must be under the bound by character count, or it proves nothing",
    );
    assert_eq!(
        field_of(&ToolArguments::new(&multi_byte)),
        Some("arguments"),
        "a string under the character bound but over the byte bound must be refused",
    );
}

// ---------------------------------------------------------------------------------------
// Results.
// ---------------------------------------------------------------------------------------

#[test]
fn a_result_needs_at_least_one_block_and_bounds_themselves() {
    assert_eq!(
        field_of(&ToolResultBody::new(
            Vec::new(),
            None,
            Sensitivity::Internal
        )),
        Some("content"),
        "an empty result is not a result",
    );
    let too_many: Vec<ContentBlock> = (0..=MAX_RESULT_BLOCKS)
        .map(|_| ContentBlock::text("x").expect("a one-byte block is usable"))
        .collect();
    assert_eq!(
        field_of(&ToolResultBody::new(too_many, None, Sensitivity::Internal)),
        Some("content"),
        "the block count is bounded separately from the byte total",
    );
}

#[test]
fn a_results_total_byte_bound_catches_many_legal_blocks() {
    // **This is the test that distinguishes a total bound from a per-block one.** Each block is
    // legal on its own — each is under the block bound — so a per-block check alone would accept an
    // arbitrarily large result, and the total is what actually holds. The fixture builds blocks whose
    // *sum* exceeds the bound while each individual block is under it, which is the only shape that
    // can distinguish the two.
    let per_block = MAX_RESULT_BYTES / MAX_RESULT_BLOCKS + 1;
    let blocks: Vec<ContentBlock> = (0..MAX_RESULT_BLOCKS)
        .map(|_| ContentBlock::text(&"x".repeat(per_block)).expect("each block is under the bound"))
        .collect();
    // Every block is legal and there are exactly as many as the block bound allows, so the only
    // check that can refuse this is the total.
    assert_eq!(blocks.len(), MAX_RESULT_BLOCKS);
    assert!(
        blocks
            .iter()
            .all(|block| block.size_bytes() < MAX_RESULT_BYTES as u64),
        "no single block may exceed the total bound, or this would not isolate the total check",
    );

    // **Refused at construction.** The bound's purpose is that the invalid value cannot be built, so
    // a constructor that returned it and left `is_within_bound` as advice would be a bound a caller
    // has to remember to apply — and a caller that forgot would hold a result 64 times the limit.
    assert_eq!(
        field_of(&ToolResultBody::new(blocks, None, Sensitivity::Internal,)),
        Some("result"),
        "the sum of {MAX_RESULT_BLOCKS} legal blocks must be refused by the constructor",
    );
}

#[test]
fn the_total_bound_is_satisfiable_so_the_check_is_not_vacuous() {
    // The positive control for the test above: if every result were over the bound, that test would
    // pass against a constructor that refused everything. One block at exactly the total is within
    // it — the comparison is `<=`, so a result of exactly the bound is legal.
    let exact = ToolResultBody::new(
        vec![ContentBlock::text(&"x".repeat(MAX_RESULT_BYTES)).expect("at the bound is usable")],
        None,
        Sensitivity::Internal,
    )
    .expect("a single block at the total bound is accepted");
    assert_eq!(exact.size_bytes(), MAX_RESULT_BYTES as u64);
    assert!(exact.is_within_bound(), "exactly the bound is within it");
    // And **above** it is refused, which is the other side of the boundary: without this the test
    // would pass against a check that never refused anything.
    assert_eq!(
        field_of(&ToolResultBody::new(
            vec![
                ContentBlock::text(&"x".repeat(MAX_RESULT_BYTES)).expect("usable"),
                ContentBlock::text("x").expect("usable"),
            ],
            None,
            Sensitivity::Internal,
        )),
        Some("result"),
        "one byte over the total must be refused, or the bound is not enforced",
    );
}

#[test]
fn a_block_is_bounded_by_the_result_bound_rather_than_the_argument_bound() {
    // **The regression test for a real defect.** The first version of `ContentBlock` held a
    // `ToolArguments`, which bounds a payload at `MAX_ARGUMENT_BYTES` — the *argument* limit. That
    // made the larger result bound unreachable: no block could exceed 64 KiB, so `MAX_RESULT_BYTES`
    // could never be reached by one block and a tool legitimately returning a 100 KiB file would
    // have been refused with a bound the operator's limit did not explain. The two bounds differ on
    // purpose — an argument is model output, a result carries file contents — so they need two types.
    let between_the_bounds = "x".repeat(MAX_ARGUMENT_BYTES + 1);
    assert!(
        ToolArguments::new(&between_the_bounds).is_err(),
        "the fixture must be over the ARGUMENT bound, or it proves nothing",
    );
    assert!(
        ContentBlock::text(&between_the_bounds).is_ok(),
        "a block payload between the argument and result bounds must be accepted; bounding it by \
         the argument limit makes the larger result bound unreachable",
    );
}

#[test]
fn an_artifact_reference_carries_its_shape_and_not_its_bytes() {
    // A result that inlined a file would put the file in every durable record and every model
    // context that mentions it. The reference carries what a consumer needs to decide whether to
    // fetch — identity, media type, size — and the size is what lets it refuse a large one without
    // fetching first.
    let block = ContentBlock::artifact("artifact-1", "text/plain", 4096)
        .expect("a shaped artifact is accepted");
    assert_eq!(
        block.size_bytes(),
        4096,
        "an artifact counts as its declared size"
    );
    let result = ToolResultBody::new(vec![block], None, Sensitivity::Internal)
        .expect("the result is accepted");
    assert!(result.has_artifacts());
    assert!(result.is_within_bound());
}

#[test]
fn an_unshaped_artifact_is_refused() {
    // A zero-byte artifact is what an unpopulated field produces, and a consumer that trusted the
    // declared size would fetch an empty document. The other refusals are the usual text rules.
    for (id, media_type, size) in [
        ("", "text/plain", 1),
        ("artifact-1", "", 1),
        ("artifact-1", "text/plain", 0),
        ("artifact\u{0}", "text/plain", 1),
        (
            &"a".repeat(MAX_PROVIDER_REFERENCE_BYTES + 1),
            "text/plain",
            1,
        ),
    ] {
        let refusal = ContentBlock::artifact(id, media_type, size)
            .expect_err("an unshaped artifact must be refused");
        assert!(
            matches!(refusal, DomainError::ToolDefinitionInvalid { .. }),
            "an unshaped artifact names a field rather than being a generic failure",
        );
    }
}

#[test]
fn a_provider_reference_is_bounded_and_optional() {
    let block = ContentBlock::text("ok").expect("usable");
    assert!(
        ToolResultBody::new(vec![block.clone()], None, Sensitivity::Internal).is_ok(),
        "a provider reference is optional",
    );
    assert!(
        ToolResultBody::new(
            vec![block.clone()],
            Some("safe-opaque-id"),
            Sensitivity::Internal
        )
        .is_ok(),
    );
    assert_eq!(
        field_of(&ToolResultBody::new(
            vec![block],
            Some(&"a".repeat(MAX_PROVIDER_REFERENCE_BYTES + 1)),
            Sensitivity::Internal,
        )),
        Some("provider_reference"),
    );
}

#[test]
fn a_result_carries_its_classification() {
    // The classification is what decides whether the result may be placed in a model's context, so a
    // result that reached that decision unlabelled would have to be guessed at. Asserted as a round
    // trip for two different labels, because a constructor that dropped the field would default both
    // to the same value and a single-label assertion would not notice.
    for sensitivity in [Sensitivity::Internal, Sensitivity::Restricted] {
        let result = ToolResultBody::new(
            vec![ContentBlock::text("ok").expect("usable")],
            None,
            sensitivity,
        )
        .expect("accepted");
        assert_eq!(result.sensitivity, sensitivity);
    }
}

// ---------------------------------------------------------------------------------------
// Call intents.
// ---------------------------------------------------------------------------------------

#[test]
fn an_intent_carries_no_identity_or_grant() {
    // **Structural, and the point of the type.** The contract says principal, workspace, and grants
    // are "trusted context, not accepted from model arguments". An intent that carried any of them
    // would invite a caller to take them from here, and the trust boundary would move to whatever
    // produced the intent — which is commonly a model.
    let intent = ToolCallIntent::new(
        call_id(),
        "fs.read@1",
        arguments(r#"{"path":"notes.md"}"#),
        Some("Read the file the user mentioned."),
    )
    .expect("the intent is usable");
    // The fields that exist are the call's own identity, the name the model used, the arguments, and
    // a display summary. There is no field to put a principal or a grant in, which is what makes the
    // rule structural: the assertion below is about the *type*, and its absence is the guarantee.
    assert_eq!(intent.call_id, call_id());
    assert_eq!(intent.capability, "fs.read@1");
    assert_eq!(intent.arguments.as_str(), r#"{"path":"notes.md"}"#);
    assert!(intent.reason_summary.is_some());
}

#[test]
fn an_intent_accepts_a_name_that_is_not_a_canonical_capability() {
    // A model emits a name, and a name is resolved against the registry rather than believed.
    // `fs.read` with no major, or a name no tool uses, is a legitimate **intent** — it is what
    // produces `tool.not_found`. Parsing it into a canonical capability here would make an
    // unresolvable name unrepresentable, so the caller could not report the case at all.
    assert!(
        ToolCallIntent::new(call_id(), "fs.read", arguments("{}"), None).is_ok(),
        "a name without a major must be representable, because resolving it is the registry's job",
    );
    assert!(
        ToolCallIntent::new(call_id(), "not.a.tool@9", arguments("{}"), None).is_ok(),
        "a name no tool uses must be representable, since that is what produces tool.not_found",
    );
    // But it is still bounded, because a name reaches a durable record and a log line.
    assert_eq!(
        field_of(&ToolCallIntent::new(
            call_id(),
            &"a".repeat(super::identity::MAX_TOOL_SEGMENT_BYTES * 3 + 1),
            arguments("{}"),
            None,
        )),
        Some("capability"),
    );
    assert_eq!(
        field_of(&ToolCallIntent::new(call_id(), "", arguments("{}"), None)),
        Some("capability"),
    );
}

#[test]
fn an_intents_display_never_renders_its_arguments() {
    // The arguments come from a model and may embed untrusted document content, and this type's
    // `Display` reaches logs. Asserted by looking for the secret-looking payload in the rendered
    // form, because the rule is "never log untrusted payload bytes" and the only way to check it is
    // to try to find them.
    let payload = r#"{"password":"hunter2"}"#;
    let intent = ToolCallIntent::new(call_id(), "fs.read@1", arguments(payload), None)
        .expect("the intent is usable");
    let rendered = intent.to_string();
    assert!(
        !rendered.contains("hunter2"),
        "the argument bytes must not reach a log line; got {rendered}",
    );
    assert!(
        rendered.contains("fs.read@1") && rendered.contains("argument byte"),
        "what it does render is the name and the size; got {rendered}",
    );
}

#[test]
fn a_reason_summary_is_bounded_and_optional() {
    assert!(
        ToolCallIntent::new(call_id(), "fs.read@1", arguments("{}"), None).is_ok(),
        "a reason summary is optional — it is display context, not a required field",
    );
    assert_eq!(
        field_of(&ToolCallIntent::new(
            call_id(),
            "fs.read@1",
            arguments("{}"),
            Some(&"a".repeat(MAX_PROVIDER_REFERENCE_BYTES + 1)),
        )),
        Some("reason_summary"),
    );
}

#[test]
fn an_intent_survives_a_json_round_trip() {
    // Intents are persisted with the call ledger, so the wire form is contract surface.
    let original = ToolCallIntent::new(
        call_id(),
        "fs.read@1",
        arguments(r#"{"path":"a.md"}"#),
        Some("because"),
    )
    .expect("usable");
    let json = serde_json::to_string(&original).expect("an intent serializes");
    let restored: ToolCallIntent = serde_json::from_str(&json).expect("an intent deserializes");
    assert_eq!(restored, original);
}

#[test]
fn a_result_survives_a_json_round_trip() {
    let original = ToolResultBody::new(
        vec![
            ContentBlock::json(r#"{"count":3}"#).expect("usable"),
            ContentBlock::artifact("artifact-1", "text/plain", 12).expect("usable"),
        ],
        Some("safe-opaque-id"),
        Sensitivity::Confidential,
    )
    .expect("accepted");
    let json = serde_json::to_string(&original).expect("a result serializes");
    let restored: ToolResultBody = serde_json::from_str(&json).expect("a result deserializes");
    assert_eq!(restored, original);
    assert!(restored.has_artifacts());
    // The block kind is tagged on the wire, so a consumer can refuse a kind it does not recognize
    // rather than passing it through as opaque.
    assert!(json.contains(r#""type":"json""#), "{json}");
    assert!(json.contains(r#""type":"artifact""#), "{json}");
}

#[test]
fn a_block_kind_is_tagged_so_an_unknown_one_is_refused_on_the_wire() {
    // The contract requires unknown content blocks to be "bounded and preserved safely or marked
    // unsupported" — never silently accepted. A tagged enum means deserializing an unknown kind
    // fails rather than yielding a block whose meaning is unexamined.
    let unknown = r#"{"type":"shell_command","value":"{}"}"#;
    assert!(
        serde_json::from_str::<ContentBlock>(unknown).is_err(),
        "an unknown content block kind must not deserialize into something plausible",
    );
}
