//! Tests for MCP tool normalization.
//!
//! Five of these carry more weight than the rest, and each exists because a plausible
//! implementation gets it wrong in a way no other test here would notice:
//!
//! - **`a_server_cannot_annotate_its_tool_out_of_an_approval_prompt`** is the security-critical
//!   one. `readOnlyHint` is the only annotation that *reduces* the recorded danger, so a server
//!   sending an untrusted hint is exactly where a tool's approval posture could be lost.
//! - **`a_schema_this_workspace_cannot_validate_is_refused_rather_than_weakened`** is the fail-open
//!   direction: dropping an unimplemented keyword validates *less* than the server asked for while
//!   reporting success.
//! - **`a_changed_input_schema_is_a_different_identity`** is `ACC-024`: a schema change behind the
//!   same tool name must not be covered by the approval recorded for the original.
//! - **`an_unusable_tool_name_is_refused_rather_than_transformed`** is why nothing is lowercased or
//!   trimmed; a mangle would collide two different tools on one capability.
//! - **`an_annotated_version_is_never_read_from_the_server`** pins the source identity to reviewed
//!   configuration, because a server that could set its own version could relabel its tools.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module
// reached from the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use std::sync::Arc;

use super::{
    CONTRACT_NAMESPACE, DEFAULT_MCP_MAX_ATTEMPTS, DEFAULT_MCP_TIMEOUT_MS, INITIAL_CONTRACT_MAJOR,
    MAX_MCP_PURPOSE_BYTES, MAX_MCP_TOOLS, McpToolRejection, normalize_catalog, normalize_tool,
    server_source,
};
use jarvis_domain::tool::classification::{ApprovalHint, Effect, Idempotency, Risk};
use jarvis_domain::tool::identity::SourceKind;
use rmcp::model::{Tool, ToolAnnotations};

/// A JSON Schema every tool here can be validated against.
const OBJECT_SCHEMA: &str = r#"{"type":"object","properties":{},"additionalProperties":false}"#;

/// Builds a `Tool` fixture by field assignment.
///
/// `rmcp`'s `Tool` is `#[non_exhaustive]`, so a struct literal is not available from outside the
/// crate — not even with functional update syntax. Assignment after `Default` is the supported path.
fn mcp_tool(name: &str, schema: &str, annotations: Option<ToolAnnotations>) -> Tool {
    let input_schema: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(schema).expect("the fixture schema must be a JSON object");
    let mut tool = Tool::default();
    tool.name = name.to_string().into();
    tool.input_schema = Arc::new(input_schema);
    tool.annotations = annotations;
    tool
}

/// Builds an annotations fixture, setting only the hints given.
fn annotations(
    read_only: Option<bool>,
    destructive: Option<bool>,
    open_world: Option<bool>,
    idempotent: Option<bool>,
) -> ToolAnnotations {
    let mut annotations = ToolAnnotations::default();
    annotations.read_only_hint = read_only;
    annotations.destructive_hint = destructive;
    annotations.open_world_hint = open_world;
    annotations.idempotent_hint = idempotent;
    annotations
}

/// Normalizes a tool that must be accepted.
fn accepted(name: &str, annotations: Option<ToolAnnotations>) -> super::NormalizedCatalog {
    let catalog = normalize_catalog("acme-files", &[mcp_tool(name, OBJECT_SCHEMA, annotations)]);
    assert!(
        catalog.rejected.is_empty(),
        "the tool must be accepted, but was refused: {:?}",
        catalog.rejected
    );
    catalog
}

#[test]
fn a_server_cannot_annotate_its_tool_out_of_an_approval_prompt() {
    // The claim under test is the security one: a `readOnlyHint` from an untrusted server may
    // label a tool, but may not remove the prompt.
    let catalog = accepted("read_file", Some(annotations(Some(true), None, None, None)));
    let definition = &catalog.tools[0];

    assert_eq!(definition.effects, vec![Effect::ReadOnly]);
    assert_eq!(definition.risk, Risk::Low);
    assert_eq!(
        definition.default_approval,
        ApprovalHint::Ask,
        "an annotation must never produce `Allow`; the prompt is the thing it would remove"
    );
    assert!(
        definition.required_scopes.is_empty(),
        "a server cannot vouch for a JARVIS scope"
    );
}

#[test]
fn a_hint_that_raises_danger_is_believed_and_a_hint_that_lowers_it_is_only_a_label() {
    // The asymmetry, asserted in one test so the two directions cannot drift apart. A destructive
    // write tool is believed and recorded as high risk even though it did not claim read-only.
    let catalog = accepted(
        "delete_file",
        Some(annotations(None, Some(true), None, None)),
    );
    let definition = &catalog.tools[0];

    assert_eq!(definition.effects, vec![Effect::Write, Effect::Destructive]);
    assert_eq!(definition.risk, Risk::High);
    assert_eq!(definition.default_approval, ApprovalHint::Ask);

    // And the control: the *same* shape without the destructive hint is not raised, so the
    // assertion above is about the hint rather than about every write tool being high risk.
    let plain = accepted("write_file", Some(annotations(None, None, None, None)));
    assert_eq!(plain.tools[0].risk, Risk::Moderate);
}

#[test]
fn annotations_that_are_read_only_and_open_world_are_refused_rather_than_trimmed() {
    // `Effect::ReadOnly` may not be combined with another effect, so this pair has no representation
    // here. Refusing keeps the contradiction visible; dropping one effect would record a claim
    // nobody made.
    let conflicting = annotations(Some(true), None, Some(true), None);
    let rejection = normalize_tool(
        &server_source("acme-files").expect("the fixture server name is usable"),
        &mcp_tool("search", OBJECT_SCHEMA, Some(conflicting)),
    )
    .expect_err("a contradictory pair must be refused");
    assert_eq!(rejection, McpToolRejection::AnnotationsConflict);
    assert_eq!(rejection.code(), "mcp.annotations_conflict");

    // The complement, so this is not an implementation that refuses every annotation: an
    // open-world tool that does *not* claim read-only is accepted.
    let open_world_only = accepted("search", Some(annotations(None, None, Some(true), None)));
    assert_eq!(
        open_world_only.tools[0].effects,
        vec![Effect::Write, Effect::ExternalCommunication]
    );
}

#[test]
fn an_accepted_tool_carries_its_validated_schema_so_the_call_path_can_validate_arguments() {
    // **The gap this closes:** an accepted tool used to arrive with no schema at all, because the
    // adapter parsed the schema to fingerprint it and then dropped the text. Downstream, a
    // `ResolvedTool` whose `input_schema` is `None` is refused as `tool.schema_absent` — so a tool this
    // adapter had just accepted would register and then be uncallable. The assertion is on the *pair*
    // being present and consistent, since neither half alone is enough.
    let catalog = accepted("read_file", None);

    assert_eq!(
        catalog.tools.len(),
        catalog.schemas.len(),
        "must be aligned"
    );
    let schema = &catalog.schemas[0];
    assert!(!schema.is_empty(), "an accepted tool must carry a schema");
    // The text must be the document the fingerprint was computed over, which is what
    // `ToolSchema::confirms` will require of a caller that stores it: re-parsing the schema and
    // fingerprinting *that* is the only way to check the pair agrees, and it is what the call path
    // does. A schema substituted from elsewhere under the same identity is the defect this prevents.
    let parsed =
        crate::tool_schema::ToolSchema::parse(schema).expect("the carried schema must parse");
    parsed
        .confirms(&catalog.tools[0].identity.schema_fingerprint)
        .expect("the carried schema must be the one that was fingerprinted");
}

#[test]
fn a_carried_schema_stays_aligned_with_its_tool_under_reordering() {
    // The ordering invariant, asserted where it could break. Definitions and schemas are sorted as
    // **pairs** and then split, so the alignment is structural rather than conventional — but a future
    // refactor that sorted each vector separately would still compile, and this is the test that fails
    // when it does.
    //
    // ⚠ **The two tools must have DIFFERENT schemas, and the first version of this test did not.** With
    // one schema shared by both, reversing the schema list is unobservable and the mutant that mis-aligns
    // them **survived** — the test passed for a reason it was not about. Distinct schemas make the
    // association directly checkable, which is what turns this from a description into a detector.
    let alpha_schema = r#"{"type":"object","properties":{"alpha":{"type":"string"}},"additionalProperties":false}"#;
    let zebra_schema = r#"{"type":"object","properties":{"zebra":{"type":"integer"}},"additionalProperties":false}"#;
    let catalog = normalize_catalog(
        "acme-files",
        &[
            // Zebra first, so the input order is *opposite* to capability order — an implementation that
            // relied on the server's order would pair them the other way round.
            mcp_tool("zebra", zebra_schema, None),
            mcp_tool("alpha", alpha_schema, None),
        ],
    );

    assert_eq!(catalog.tools.len(), 2);
    assert_eq!(catalog.schemas.len(), 2);
    assert_eq!(
        catalog.tools[0].capability().to_string(),
        "mcp.alpha@1",
        "the definitions must be in capability order"
    );
    // The direct association, asserted **through the schema's content rather than its bytes.**
    //
    // ⚠ A first version compared the carried text to the input literal and failed, and the reason is worth
    // keeping: `serde_json::Map` is a `BTreeMap`, so the document is re-serialized with **sorted keys**
    // before it is carried and fingerprinted. That is a property rather than a nuisance — it makes the
    // identity canonical, so a server reordering its schema's keys does not present a new tool — but it
    // means byte equality against the offered text is the wrong assertion. The property is which tool the
    // schema describes, so that is what is checked.
    let alpha: serde_json::Value =
        serde_json::from_str(&catalog.schemas[0]).expect("the carried schema must be JSON");
    let zebra: serde_json::Value =
        serde_json::from_str(&catalog.schemas[1]).expect("the carried schema must be JSON");
    assert!(
        alpha["properties"].get("alpha").is_some(),
        "the first tool is `alpha`, so the first schema must be the alpha one: {}",
        catalog.schemas[0]
    );
    assert!(
        zebra["properties"].get("zebra").is_some(),
        "the second tool is `zebra`, so the second schema must be the zebra one: {}",
        catalog.schemas[1]
    );
    // And the fingerprint consistency follows from it rather than substituting for it.
    for (definition, schema) in catalog.tools.iter().zip(&catalog.schemas) {
        crate::tool_schema::ToolSchema::parse(schema)
            .expect("every carried schema must parse")
            .confirms(&definition.identity.schema_fingerprint)
            .unwrap_or_else(|_| {
                panic!(
                    "the schema carried beside {} must be its own",
                    definition.capability()
                )
            });
    }
}

#[test]
fn a_reordered_schema_presents_the_same_identity() {
    // The consequence of the canonicalization above, asserted separately because it is a security-relevant
    // property rather than a detail: if key order affected the fingerprint, a server could reorder its
    // schema's keys to present a *new* tool identity while changing nothing that matters — invalidating
    // every existing grant for it (fail-closed, but noisy) or, worse in the other direction, making two
    // spellings of one schema look like two tools.
    let source = server_source("acme-files").expect("the fixture server name is usable");
    let ordered = r#"{"type":"object","properties":{"a":{"type":"string"}}}"#;
    let reordered = r#"{"properties":{"a":{"type":"string"}},"type":"object"}"#;

    let (first, first_schema) =
        normalize_tool(&source, &mcp_tool("read", ordered, None)).expect("accepted");
    let (second, second_schema) =
        normalize_tool(&source, &mcp_tool("read", reordered, None)).expect("accepted");

    assert_eq!(
        first.identity.schema_fingerprint, second.identity.schema_fingerprint,
        "key order must not change the identity"
    );
    assert_eq!(
        first_schema, second_schema,
        "the canonical text must be equal"
    );
    assert!(
        first.is_same_tool_as(&second),
        "a reordered schema is the same tool"
    );
}

#[test]
fn a_refused_tool_contributes_no_schema() {
    // The complement, so the assertion above is not satisfied by a catalog that carries a schema for
    // every offered tool regardless of acceptance — which would leave a refused tool's schema in a list
    // whose index no longer means anything.
    let catalog = normalize_catalog(
        "acme-files",
        &[
            mcp_tool("good", OBJECT_SCHEMA, None),
            mcp_tool("bad", r#"{"type":"object","pattern":"x"}"#, None),
        ],
    );

    assert_eq!(catalog.tools.len(), 1);
    assert_eq!(
        catalog.schemas.len(),
        1,
        "only accepted tools contribute a schema"
    );
}

#[test]
fn a_schema_this_workspace_cannot_validate_is_refused_rather_than_weakened() {
    // `pattern` is a keyword this workspace deliberately does not implement, so a schema using it
    // would otherwise validate less than its author asked for.
    let patterned = r#"{"type":"object","properties":{"n":{"type":"string","pattern":"^a+$"}}}"#;
    let catalog = normalize_catalog("acme-files", &[mcp_tool("greet", patterned, None)]);

    assert!(catalog.tools.is_empty(), "the tool must not be offered");
    assert_eq!(catalog.rejected.len(), 1);
    assert_eq!(
        catalog.rejected[0].rejection,
        McpToolRejection::SchemaRejected
    );
    assert_eq!(catalog.rejected[0].name, "greet");
}

#[test]
fn a_changed_input_schema_is_a_different_identity() {
    // `ACC-024`: replacing a tool's schema behind the same name must not be covered by the approval
    // recorded for the original.
    let source = server_source("acme-files").expect("the fixture server name is usable");
    // `normalize_tool` returns the definition **and its validated schema**; the definition is what this
    // test is about, so the pair is destructured rather than the return type being widened back.
    let (first, first_schema) =
        normalize_tool(&source, &mcp_tool("read", OBJECT_SCHEMA, None)).expect("accepted");
    let widened =
        r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#;
    let (second, second_schema) =
        normalize_tool(&source, &mcp_tool("read", widened, None)).expect("accepted");

    assert_eq!(
        first.capability().to_string(),
        second.capability().to_string(),
        "both are the same capability, so the name is not what distinguishes them"
    );
    assert_ne!(
        first.identity.schema_fingerprint, second.identity.schema_fingerprint,
        "the fingerprint must differ, or the identity cannot"
    );
    assert!(
        !first.is_same_tool_as(&second),
        "a schema change must not read as the same tool"
    );
    // And each definition's own fingerprint still describes its own carried schema, so the widening is
    // reflected in the pair rather than only in the identity.
    assert_ne!(
        first_schema, second_schema,
        "the two schemas must differ, or the fingerprints differed for some other reason"
    );
    for (definition, schema) in [(&first, &first_schema), (&second, &second_schema)] {
        crate::tool_schema::ToolSchema::parse(schema)
            .expect("the carried schema must parse")
            .confirms(&definition.identity.schema_fingerprint)
            .expect("the carried schema must be the one that was fingerprinted");
    }
}

#[test]
fn an_annotated_version_is_never_read_from_the_server() {
    // The source identity is reviewed configuration. Neither the tool's own metadata nor anything
    // else the server sends can move it, because a server that set its own version could relabel
    // its tools as new implementations.
    let catalog = accepted("read", None);
    let source = catalog.tools[0].source();

    assert_eq!(source.kind, SourceKind::McpServer);
    assert_eq!(source.owner, "acme-files");
    assert_eq!(source.version.major(), INITIAL_CONTRACT_MAJOR);
    assert_eq!(
        catalog.tools[0].capability().namespace(),
        CONTRACT_NAMESPACE
    );
    assert_eq!(
        catalog.tools[0].capability().major(),
        INITIAL_CONTRACT_MAJOR
    );
    assert_eq!(catalog.source.as_ref(), Some(source));
}

#[test]
fn an_unusable_tool_name_is_refused_rather_than_transformed() {
    // A name is refused, never lowercased or trimmed. A mangle would let two different offered
    // tools collide on one capability, and the canonical form exists to prevent exactly that.
    let source = server_source("acme-files").expect("the fixture server name is usable");
    for unusable in ["Read_File", "read-file", "read.file", "9read", "read file"] {
        let result = normalize_tool(&source, &mcp_tool(unusable, OBJECT_SCHEMA, None));
        assert_eq!(
            result.expect_err("an unusable name must be refused"),
            McpToolRejection::ToolNameInvalid,
            "`{unusable}` must be refused rather than repaired"
        );
    }
}

#[test]
fn a_server_name_that_is_a_valid_owner_but_not_a_valid_segment_does_not_refuse_its_tools() {
    // The correction, pinned: a capability's namespace is a *segment*, so a server legitimately
    // named with a hyphen (as real servers are) cannot be used as one. The namespace is a constant
    // and the server is the source owner, so hyphens and dots stay legal where they belong.
    let catalog = normalize_catalog(
        "brave-search",
        &[mcp_tool("web_search", OBJECT_SCHEMA, None)],
    );

    assert!(catalog.rejected.is_empty(), "{:?}", catalog.rejected);
    assert_eq!(
        catalog.tools[0].capability().to_string(),
        "mcp.web_search@1"
    );
    assert_eq!(catalog.tools[0].source().owner, "brave-search");
}

#[test]
fn two_servers_offering_one_tool_name_produce_one_capability_but_two_identities() {
    // Why the namespace is a constant rather than the server: the server is not lost, because it is
    // the source owner, and `authorizes` requires all three components. A grant for one server's
    // `read_file` must not cover another's.
    let first = normalize_catalog("server-a", &[mcp_tool("read_file", OBJECT_SCHEMA, None)]);
    let second = normalize_catalog("server-b", &[mcp_tool("read_file", OBJECT_SCHEMA, None)]);

    assert_eq!(
        first.tools[0].capability().to_string(),
        second.tools[0].capability().to_string()
    );
    assert!(
        !first.tools[0].is_same_tool_as(&second.tools[0]),
        "one server's approval must not authorize another server's tool of the same name"
    );
}

#[test]
fn an_over_long_description_is_refused_rather_than_truncated() {
    let source = server_source("acme-files").expect("the fixture server name is usable");
    let mut tool = mcp_tool("read_file", OBJECT_SCHEMA, None);
    tool.description = Some("x".repeat(MAX_MCP_PURPOSE_BYTES + 1).into());

    let rejection =
        normalize_tool(&source, &tool).expect_err("an over-long description must be refused");
    assert_eq!(rejection, McpToolRejection::DescriptionTooLong);
    assert_eq!(rejection.code(), "mcp.description_too_long");
}

#[test]
fn a_listing_past_the_tool_bound_is_refused_past_the_bound_rather_than_truncated() {
    // A server controls this count, and a listing is re-read on every discovery refresh, so the
    // bound belongs at the acceptance boundary. The surplus is refused individually so the operator
    // can see how many were dropped rather than finding a silently shorter catalog.
    let tools: Vec<Tool> = (0..=MAX_MCP_TOOLS)
        .map(|index| mcp_tool(&format!("tool_{index}"), OBJECT_SCHEMA, None))
        .collect();
    let catalog = normalize_catalog("acme-files", &tools);

    assert_eq!(catalog.tools.len(), MAX_MCP_TOOLS);
    assert_eq!(catalog.rejected.len(), 1);
    assert_eq!(
        catalog.rejected[0].rejection,
        McpToolRejection::TooManyTools
    );
    assert_eq!(catalog.rejected[0].name, format!("tool_{MAX_MCP_TOOLS}"));
}

#[test]
fn an_invalid_server_name_fails_the_whole_catalog_rather_than_one_tool() {
    // There is nothing to be a source for, so every tool is refused with that reason rather than
    // being attributed to a source that does not exist.
    let catalog = normalize_catalog("Acme Files", &[mcp_tool("read", OBJECT_SCHEMA, None)]);

    assert!(catalog.source.is_none());
    assert!(catalog.tools.is_empty());
    assert_eq!(catalog.rejected.len(), 1);
    assert_eq!(
        catalog.rejected[0].rejection,
        McpToolRejection::ServerNameInvalid
    );
}

#[test]
fn a_side_effecting_tool_is_not_retryable_by_default() {
    // MCP has no redelivery, so a retry after a lost response is a second side effect rather than a
    // recovery of the first.
    let catalog = accepted("write_file", Some(annotations(None, None, None, None)));
    let definition = &catalog.tools[0];

    assert_eq!(definition.idempotency, Idempotency::None);
    assert!(definition.has_effect(Effect::Write));
}

#[test]
fn a_claim_of_idempotency_widens_retrying_but_never_lowers_danger() {
    // Only the safe directions are taken from an untrusted hint: it may make a write tool
    // retryable, and it may not reduce the risk or the approval posture.
    let catalog = accepted(
        "write_file",
        Some(annotations(None, None, None, Some(true))),
    );
    let definition = &catalog.tools[0];

    assert_eq!(definition.idempotency, Idempotency::NaturallyIdempotent);
    assert_eq!(definition.risk, Risk::Moderate);
    assert_eq!(definition.default_approval, ApprovalHint::Ask);
}

#[test]
fn a_missing_description_falls_back_to_the_name_rather_than_dropping_the_tool() {
    // A blank purpose is refused by the domain, so a tool with no description would otherwise be
    // lost for a reason its author cannot act on.
    let catalog = accepted("read_file", None);
    assert_eq!(catalog.tools[0].purpose, "read_file");

    let mut described = mcp_tool("read_file", OBJECT_SCHEMA, None);
    described.description = Some("Read a file.".into());
    let with_description = normalize_catalog("acme-files", &[described]);
    assert_eq!(with_description.tools[0].purpose, "Read a file.");
}

#[test]
fn the_catalog_reports_refusals_alongside_acceptances_and_is_ordered_stably() {
    // "Three tools" and "three tools of which one was dropped" are different operator facts, so both
    // halves are returned. Ordering is asserted with the input reversed, so it is the function's
    // behavior rather than the input's order.
    let tools = [
        mcp_tool("zebra", OBJECT_SCHEMA, None),
        mcp_tool("broken", r#"{"type":"object","pattern":"x"}"#, None),
        mcp_tool("alpha", OBJECT_SCHEMA, None),
    ];
    let catalog = normalize_catalog("acme-files", &tools);

    assert_eq!(
        catalog
            .tools
            .iter()
            .map(|tool| tool.capability().to_string())
            .collect::<Vec<_>>(),
        vec!["mcp.alpha@1", "mcp.zebra@1"]
    );
    assert_eq!(catalog.rejected.len(), 1);
    assert_eq!(catalog.rejected[0].name, "broken");
}

#[test]
fn every_rejection_code_is_namespaced_and_distinct() {
    // A control for the claim that a rejection is attributable to this adapter rather than to the
    // tool fabric: if two reasons shared a code, the distinctness asserted in the tests above would
    // be an artifact of which variant each happened to construct.
    let rejections = [
        McpToolRejection::ServerNameInvalid,
        McpToolRejection::ToolNameInvalid,
        McpToolRejection::DescriptionTooLong,
        McpToolRejection::TooManyTools,
        McpToolRejection::SchemaRejected,
        McpToolRejection::AnnotationsConflict,
        McpToolRejection::DefinitionInvalid { field: "purpose" },
    ];
    let codes: Vec<&str> = rejections.iter().map(McpToolRejection::code).collect();

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
fn the_execution_defaults_are_bounded_and_single_attempt() {
    // Asserted on the assembled definition rather than on the constants, because a constant
    // compared against itself is a check the compiler folds away: what matters is that the
    // definition a server's tool produces carries exactly one attempt and a bounded timeout.
    let catalog = accepted("read_file", None);
    let execution = catalog.tools[0].execution;

    assert_eq!(execution.max_attempts, DEFAULT_MCP_MAX_ATTEMPTS);
    assert_eq!(execution.max_attempts, 1);
    assert_eq!(execution.timeout_ms, DEFAULT_MCP_TIMEOUT_MS);
    assert!(execution.timeout_ms > 0);
    assert!(!catalog.tools[0].is_consequential());
}
