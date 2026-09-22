//! Golden fixtures for the model-stream contract's own examples.
//!
//! Round 7 recorded that `BRN-001` left the contract's `settings` block unmodelled, and that
//! the reason was structural: the evidence table listed the **tests that were written** rather
//! than the **contract fields that were covered**. A hand-written fixture list reproduces that
//! mistake exactly, because the same person chooses the same fields.
//!
//! So the fixtures here are **extracted from the contract document** rather than copied from
//! it. Editing the document's example without editing the type then fails the build, which is
//! what "no drift" has to mean to be worth anything.
//!
//! This module owns its extraction rather than sharing `jarvis-protocol`'s, because the
//! dependency runs the other way: `jarvis-protocol` does not depend on this crate, and this
//! crate may not depend on that one. The duplication is real, and it is the lesser cost —
//! a shared extractor would need a crate for it alone.

use std::path::PathBuf;

/// Returns the repository root, from this crate's manifest directory.
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the crate lives two levels under the repository root")
        .to_path_buf()
}

/// Extracts each JSON-fenced block tagged `json` from `relative`.
///
/// Returns an error when the document cannot be read, rather than an empty list: a contract
/// that cannot be read would otherwise let every assertion below pass by finding nothing,
/// which is the one failure a fixture test must not have.
fn examples_in(relative: &str) -> Result<Vec<(usize, String)>, String> {
    let path = repository_root().join(relative);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;

    let mut found = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        if line.trim() != "```json" {
            continue;
        }
        let mut depth = 0_i32;
        let mut started = false;
        let mut block = String::new();
        for (_, inner) in lines.by_ref() {
            if inner.trim() == "```" && (!started || depth == 0) {
                break;
            }
            for character in inner.chars() {
                match character {
                    '{' | '[' => {
                        depth += 1;
                        started = true;
                    }
                    '}' | ']' => depth -= 1,
                    _ => {}
                }
            }
            block.push_str(inner);
            block.push('\n');
            if started && depth <= 0 {
                break;
            }
        }
        if started {
            found.push((index + 2, block.trim().to_owned()));
        }
    }
    Ok(found)
}

/// Returns the first example in `relative` containing `needle`.
fn example_containing(relative: &str, needle: &str) -> (usize, String) {
    let examples = examples_in(relative).expect("the document reads");
    examples
        .into_iter()
        .find(|(_, json)| json.contains(needle))
        .unwrap_or_else(|| unreachable!("{relative} must document an example containing {needle}"))
}

/// Replaces the contract's elided identifiers with canonical ones.
///
/// **A finding, not a convenience.** The contract's examples write `"call_id": "019..."` —
/// an elision marker, because an example is not a captured trace. The identifier types
/// rightly refuse it: `"019..."` is not a canonical UUID, and accepting it would mean
/// accepting a second spelling of every identity in the system. So the examples are
/// **not directly parseable**, and a fixture test has to do what a client author does and
/// supply real values.
///
/// Substituting in document order rather than by key means a reordered example would put the
/// wrong identifier in the wrong field — and since all three identity types are distinct
/// newtypes, the parse would fail rather than quietly succeed, which is the direction to fail
/// in.
fn with_canonical_identifiers(json: &str) -> String {
    // Distinct values, so a transposition between two identifier fields is visible rather
    // than masked by both being the same string.
    let identifiers = [
        "0195f4f0-4c13-7bf4-89fb-f067adac13e1",
        "0195f4f0-4c13-7bf4-89fb-f067adac13e2",
        "0195f4f0-4c13-7bf4-89fb-f067adac13e3",
    ];
    let mut replaced = json.to_owned();
    for identifier in identifiers {
        // `replacen` with a count of one, so each marker is consumed in order and an example
        // with more markers than values is left visibly unparseable rather than silently
        // reusing the first value.
        replaced = replaced.replacen("\"019...\"", &format!("\"{identifier}\""), 1);
    }
    replaced
}

#[test]
fn the_contracts_request_example_parses_as_the_normalized_request() {
    // The fixture that would have caught round 7's defect. `ModelCallRequest` is the type an
    // adapter receives, so if the contract's own example does not deserialize into it, the two
    // disagree — and the disagreement is only invisible because `deny_unknown_fields` is
    // absent on some blocks.
    let (line, json) = example_containing("docs/contracts/model-stream.md", "route_requirements");
    let json = with_canonical_identifiers(&json);
    let parsed: super::ModelCallRequest = serde_json::from_str(&json)
        .unwrap_or_else(|error| unreachable!("model-stream.md line {line} must parse: {error}"));

    // Every field the example names must survive the parse. Asserting the values rather than
    // merely "it parsed" is the point: a block that is silently ignored still parses.
    assert!(!parsed.route_requirements.modalities.is_empty());
    assert!(parsed.route_requirements.requires_tool_calling());
    assert!(
        parsed.limits.deadline.is_some(),
        "the example names a deadline, and dropping it is exactly the defect this catches",
    );
    assert_eq!(parsed.limits.max_output_tokens, Some(2048));
    assert_eq!(parsed.limits.max_cost_microunits, None);
    assert!(parsed.tools.is_empty());
    assert!(parsed.output_schema.is_none());
}

#[test]
fn the_contracts_settings_block_is_modelled_rather_than_ignored() {
    // Round 7's specific finding: `settings` was absent from `BRN-001` entirely. The block in
    // the example is empty, so this asserts the *field* exists and round-trips — a
    // `deny_unknown_fields` block that the type lacks would be dropped without complaint.
    let (line, json) = example_containing("docs/contracts/model-stream.md", "route_requirements");
    let json = with_canonical_identifiers(&json);
    let parsed: super::ModelCallRequest = serde_json::from_str(&json)
        .unwrap_or_else(|error| unreachable!("model-stream.md line {line} must parse: {error}"));
    assert_eq!(
        parsed.settings,
        super::PortableSettings::default(),
        "an absent and an empty settings block are the same fact on the wire",
    );

    // And a *populated* settings block is accepted, which the empty example alone cannot show.
    let populated = json.replace(
        "\"settings\": {}",
        "\"settings\": {\"temperature_millis\":700,\"top_p_millis\":900,\"reasoning_effort\":\"low\"}",
    );
    assert_ne!(
        populated, json,
        "the example must contain an empty settings block"
    );
    let parsed: super::ModelCallRequest =
        serde_json::from_str(&populated).expect("a populated settings block parses");
    assert_eq!(parsed.settings.temperature_millis, Some(700));
    assert_eq!(parsed.settings.top_p_millis, Some(900));
}

#[test]
fn the_settings_blocks_range_rules_match_what_the_contract_states() {
    // The contract states the ranges in prose beside the example, so the prose and the
    // constants must agree. A range widened in one place and not the other is a silent
    // acceptance of a value the contract refuses.
    let document =
        std::fs::read_to_string(repository_root().join("docs/contracts/model-stream.md"))
            .expect("the contract reads");
    // "the temperature range is `0..=2000` (`0.0..=2.0`), and the nucleus range is
    // `1..=1000`" — asserted as text so a rewritten sentence forces a re-read.
    assert!(
        document.contains("0..=2000"),
        "the contract must still state the temperature range",
    );
    assert!(
        document.contains("1..=1000"),
        "the contract must still state the nucleus range",
    );
    assert_eq!(super::MAX_TEMPERATURE_MILLIS, 2_000);
    assert_eq!(super::MAX_TOP_P_MILLIS, 1_000);

    // An out-of-range value is refused rather than clamped, which the contract states
    // explicitly and which the type must therefore enforce.
    assert!(super::PortableSettings::new(Some(2_001), None, None).is_err());
    assert!(
        super::PortableSettings::new(Some(2_000), None, None).is_ok(),
        "the ceiling itself is inside the range",
    );
}

#[test]
fn the_contracts_stream_envelope_example_parses_as_a_stream_event() {
    // The envelope's field set — `call_id`, `event_id`, `sequence`, `type` — is what a client
    // parses every frame from, so an example that does not deserialize means no client can be
    // written from the contract.
    let examples = examples_in("docs/contracts/model-stream.md").expect("the contract reads");
    let mut checked = 0;
    for (line, json) in examples {
        // Only the frames are asserted; the request and the settings examples are covered
        // above and do not share the envelope's shape.
        if !json.contains(r#""sequence""#) || !json.contains(r#""type""#) {
            continue;
        }
        let json = with_canonical_identifiers(&json);
        let parsed: super::ModelStreamEvent = serde_json::from_str(&json).unwrap_or_else(|error| {
            unreachable!("model-stream.md line {line} must parse: {error}")
        });
        assert!(
            parsed.sequence.get() > 0,
            "the example is not the first frame"
        );
        checked += 1;
    }
    assert!(
        checked >= 1,
        "the contract documents at least one stream envelope example",
    );
}

#[test]
fn the_serialized_envelope_places_the_payload_beside_the_type_tag() {
    // The shape the contract documents, asserted from the *serializing* side rather than only
    // from a parse of the document. Deserializing proves the type accepts the contract's
    // example; serializing proves the type produces it, and only both together make the two
    // interchangeable. This is what caught a flattened envelope that put `item_id` and `delta`
    // at the top level where no client written from the contract would look for them.
    let event = super::ModelStreamEvent {
        call_id: super::ModelCallId::parse("0195f4f0-4c13-7bf4-89fb-f067adac13e1").expect("valid"),
        event_id: super::ModelStreamEventId::parse("0195f4f0-4c13-7bf4-89fb-f067adac13e2")
            .expect("valid"),
        sequence: super::Sequence::new(3),
        kind: super::ModelStreamEventKind::OutputTextDelta {
            item_id: "out-1".to_owned(),
            delta: "Hello".to_owned(),
        },
        provider_metadata: None,
    };
    let json = serde_json::to_string(&event).expect("serializes");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    // The tag is the contract's dotted name, not a Rust-derived spelling.
    assert_eq!(
        parsed["type"], "output.text.delta",
        "the type tag must be the contract's name: {json}",
    );
    // The variant's fields live under `payload`, which is where the contract puts them.
    assert_eq!(parsed["payload"]["item_id"], "out-1", "{json}");
    assert_eq!(parsed["payload"]["delta"], "Hello", "{json}");
    assert!(
        parsed.get("item_id").is_none(),
        "a payload field must not be flattened onto the envelope: {json}",
    );

    // Round trip: what was produced parses back to the same value, so the two shapes are one.
    let again: super::ModelStreamEvent = serde_json::from_str(&json).expect("round-trips");
    assert_eq!(again, event);
}

#[test]
fn the_extractor_finds_the_examples_it_is_meant_to_find() {
    // The extractor is what makes every assertion above meaningful, so it is tested directly:
    // one that found nothing would make this whole module pass vacuously.
    let examples = examples_in("docs/contracts/model-stream.md").expect("the contract reads");
    assert!(
        examples.len() >= 2,
        "the contract documents a request and at least one envelope; found {}",
        examples.len(),
    );
    for (line, json) in &examples {
        serde_json::from_str::<serde_json::Value>(json).unwrap_or_else(|error| {
            unreachable!("model-stream.md line {line} is not valid JSON: {error}")
        });
    }
    // An unreadable document is an error rather than an empty list, so a moved file fails
    // loudly instead of silently checking nothing.
    assert!(examples_in("docs/contracts/does-not-exist.md").is_err());
}
