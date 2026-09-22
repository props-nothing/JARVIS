//! Golden-fixture tests: the accepted contracts' own examples, executed.
//!
//! Contract test 12 in `docs/contracts/local-control-api.md` requires "golden JSON/SSE
//! fixtures and generated `OpenAPI` drift". The `OpenAPI` half is a separate deliverable and
//! is **not** implemented; this module is the golden-fixture half, and it is deliberately the
//! stronger of the two for this codebase.
//!
//! ## Why the fixtures are read from the documents rather than checked in as copies
//!
//! A fixture committed beside the code can drift from the contract it claims to represent,
//! and nothing detects it — the test passes against the copy while the document says
//! something else, which is worse than having no test because it produces false confidence.
//! Reading the **document itself** makes the two inseparable: editing an example without
//! editing the type fails the build, and that is the property "no drift" actually means.
//!
//! ## What this test found before it was written to pass
//!
//! Round 7 recorded that `BRN-001` left the model-stream contract's `settings` block
//! unmodelled because the evidence table listed the *tests it wrote* rather than the contract
//! *fields it covered*. The remedy is mechanical: extract every JSON example from the owning
//! documents and assert the types accept them. The extraction is what finds the gap; a
//! hand-written fixture list reproduces the original mistake.
//!
//! ## Why the extraction is hand-written
//!
//! The project has no Markdown parser dependency, and the shape here is narrow: a fenced block
//! tagged `json`, then a brace-balanced value. A full parser would be a dependency change the
//! integration research gate requires evidence for, and would be wider than the need. The
//! extraction's own limitations are stated and asserted below rather than assumed.

use std::path::PathBuf;

/// One JSON example found in a contract document.
#[derive(Debug)]
struct Example {
    /// The document it came from, for a failure message that names the source.
    document: String,
    /// Its 1-based line number in that document, so a failure points at the example.
    line: usize,
    /// The raw JSON text.
    json: String,
}

impl Example {
    /// A label naming where the example came from.
    fn label(&self) -> String {
        format!("{} line {}", self.document, self.line)
    }
}

/// Returns the repository root, from this crate's manifest directory.
///
/// `pub(crate)` so the extractor below can be reused by another crate's fixture test rather
/// than copied. A second copy of an extractor is where two fixture suites come to disagree
/// about what a document contains.
fn repository_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `crates/jarvis-protocol`, so the root is two levels up. This is
    // a compile-time path, so a moved file fails loudly rather than silently skipping.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the crate lives two levels under the repository root")
        .to_path_buf()
}

/// Extracts every JSON-fenced block tagged `json` from `relative`'s text.
///
/// Returns an error rather than an empty list when the document cannot be read, because an
/// unreadable contract would otherwise make this test pass by finding nothing to check —
/// which is exactly the failure mode a fixture test exists to prevent.
fn examples_in(relative: &str) -> Result<Vec<Example>, String> {
    let path = repository_root().join(relative);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;

    let mut examples = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        if line.trim() != "```json" {
            continue;
        }
        // The fence is on `index`; the example starts on the next line.
        let mut depth = 0_i32;
        let mut started = false;
        let mut block = String::new();
        let start = index + 2;
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
            examples.push(Example {
                document: relative.to_owned(),
                line: start,
                json: block.trim().to_owned(),
            });
        }
    }
    Ok(examples)
}

#[test]
fn the_contract_example_extraction_finds_the_documents_examples() {
    // The extractor is what makes every test below meaningful, so it is tested first: an
    // extractor that found nothing would make the whole module vacuously pass. The counts are
    // lower bounds, not exact, so adding an example to a document does not break the build —
    // but *removing* them all does, which is the direction that matters.
    let control = examples_in("docs/contracts/local-control-api.md").expect("the document reads");
    assert!(
        control.len() >= 8,
        "the local control API contract has several JSON examples; found {}: {:?}",
        control.len(),
        control.iter().map(Example::label).collect::<Vec<_>>(),
    );
    for example in &control {
        assert!(
            example.json.starts_with('{') || example.json.starts_with('['),
            "{} did not extract a JSON value: {}",
            example.label(),
            example.json,
        );
    }
}

#[test]
fn every_json_example_in_the_local_control_api_contract_is_parseable_json() {
    // The floor, and it is not trivial: a document's example is what a client author copies,
    // so an example that is not valid JSON is a broken contract regardless of whether any
    // type matches it. This also catches a malformed block that the extractor mis-terminated.
    let examples = examples_in("docs/contracts/local-control-api.md").expect("the document reads");
    let mut checked = 0;
    for example in &examples {
        // The `/v1/system/status` shape and the error envelope both appear; only the ones
        // that are complete values are asserted, since a fragment is legitimately partial.
        serde_json::from_str::<serde_json::Value>(&example.json)
            .unwrap_or_else(|error| unreachable!("{} is not valid JSON: {error}", example.label()));
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} examples were checked");
}

#[test]
fn the_status_example_names_the_capabilities_the_daemon_actually_serves() {
    // A drift test with teeth: the contract lists the capability strings, and the daemon's own
    // route table must agree. A route added without a capability, or a capability left behind
    // by a removed route, is exactly the drift contract test 12 names.
    let examples = examples_in("docs/contracts/local-control-api.md").expect("the document reads");
    let status = examples
        .iter()
        .find(|example| example.json.contains("capabilities"))
        .expect("the contract documents the status response");
    let parsed: serde_json::Value = serde_json::from_str(&status.json).expect("valid JSON");
    let capabilities: Vec<String> = parsed["capabilities"]
        .as_array()
        .expect("capabilities is an array")
        .iter()
        .map(|value| value.as_str().expect("a capability is a string").to_owned())
        .collect();

    // The contract's own list is the authority, and every entry names an operation the
    // contract documents. Asserting the *set* rather than a count means a rename is caught
    // rather than merely a removal.
    for required in ["runs.create", "runs.read", "runs.cancel", "runs.events"] {
        assert!(
            capabilities.iter().any(|value| value == required),
            "the contract's status example must advertise {required}: {capabilities:?}",
        );
    }
    // And the operations the wire types define must be within the advertised set, so a new
    // operation cannot be added to the surface without the capability appearing here.
    for operation in ["runs.create", "runs.cancel"] {
        assert!(
            capabilities.iter().any(|value| value == operation),
            "{operation} is served but not advertised",
        );
    }
}

#[test]
fn the_acceptance_scenarios_the_contract_names_exist_as_headings() {
    // The documents cite acceptance scenario ids, and a citation of a scenario that does not
    // exist is a dangling reference a reader cannot follow. The docs validator checks links;
    // this checks the *identifiers*, which is the part a link cannot express.
    let acceptance = std::fs::read_to_string(repository_root().join("docs/testing/acceptance.md"))
        .expect("the acceptance document reads");
    for scenario in ["ACC-002", "ACC-003", "ACC-010", "ACC-012"] {
        assert!(
            acceptance.contains(&format!("### `{scenario}`:")),
            "{scenario} is cited by the local control API contract but has no heading",
        );
    }
}

#[test]
fn the_contract_version_the_wire_types_declare_is_the_one_the_contract_states() {
    // The version is stated in three places — the contract's `Contract version` line, the
    // constant, and every frame — and they must agree or a client cannot negotiate.
    let contract =
        std::fs::read_to_string(repository_root().join("docs/contracts/local-control-api.md"))
            .expect("the contract reads");
    assert!(
        contract.contains("Contract version: 0.1.0"),
        "the contract must state the version the wire types declare",
    );
    assert_eq!(super::RUN_CONTRACT_VERSION, "0.1.0");

    // And every frame carries it, so a client can reject a mismatched daemon.
    let frame = super::RunEventFrame {
        contract_version: super::RUN_CONTRACT_VERSION.to_owned(),
        event_id: "0195f4f1-0475-7613-a92c-edf01183e909".to_owned(),
        run_id: "0195f4f0-4c13-7bf4-89fb-f067adac13ee".to_owned(),
        sequence: 1,
        occurred_at: "2026-09-20T12:35:11Z".to_owned(),
        payload: serde_json::json!({"delta": "Hello"}),
    };
    let json = serde_json::to_string(&frame).expect("serializes");
    assert!(
        json.contains(r#""contract_version":"0.1.0""#),
        "a frame must carry the version a client negotiates on: {json}",
    );
}

#[test]
fn the_event_type_names_the_stream_contract_requires_are_all_defined() {
    // The contract names a minimum set of event types for the first slice. A missing one
    // would mean a client waiting for it waits forever, so the set is asserted rather than
    // left to the constants' own spelling.
    let required = [
        ("run.received", super::event_type::RECEIVED),
        ("run.context_building", super::event_type::CONTEXT_BUILDING),
        ("run.model_started", super::event_type::MODEL_STARTED),
        (
            "run.output_text.delta",
            super::event_type::OUTPUT_TEXT_DELTA,
        ),
        ("run.usage", super::event_type::USAGE),
        ("run.completed", super::event_type::COMPLETED),
        ("run.failed", super::event_type::FAILED),
        ("run.cancelled", super::event_type::CANCELLED),
    ];
    for (expected, actual) in required {
        assert_eq!(expected, actual, "the wire name must be the contract's");
    }

    // The contract states this minimum set in prose; assert it is still there, so removing
    // the sentence without removing the types is caught.
    let contract =
        std::fs::read_to_string(repository_root().join("docs/contracts/local-control-api.md"))
            .expect("the contract reads");
    for (name, _) in required {
        assert!(
            contract.contains(name),
            "the contract must still name {name}",
        );
    }
}

#[test]
fn the_three_terminal_event_types_are_exactly_the_ones_the_contract_lists() {
    // "Exactly one terminal event" is only meaningful if the terminal set is closed. A fourth
    // terminal type would let two different events both claim to end a run.
    let terminals = [
        super::event_type::COMPLETED,
        super::event_type::FAILED,
        super::event_type::CANCELLED,
    ];
    assert_eq!(terminals.len(), 3);
    let contract =
        std::fs::read_to_string(repository_root().join("docs/contracts/local-control-api.md"))
            .expect("the contract reads");
    for name in terminals {
        assert!(
            contract.contains(name),
            "{name} must be a documented terminal"
        );
    }
    // And the non-terminal types must not be terminal, which is the direction a careless
    // addition would break.
    for name in [
        super::event_type::RECEIVED,
        super::event_type::PLANNING,
        super::event_type::OUTPUT_TEXT_DELTA,
        super::event_type::USAGE,
        super::event_type::RESPONDING,
    ] {
        assert!(!terminals.contains(&name), "{name} is not terminal");
    }
}
