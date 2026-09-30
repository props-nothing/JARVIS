//! Composition tests across the schema validator and the tool fabric's definition value.
//!
//! These live in `jarvis-infrastructure` because it is the **only** crate that depends on both
//! `jarvis-domain` (which owns [`ToolDefinition`] and [`ToolIdentity`]) and this crate's
//! `tool_schema` module. Neither side can test the other: the domain has no JSON dependency, and the
//! validator has no definition. A test that lived beside either could only restate one of them.
//!
//! The rule under test is `ACC-024`'s, reached through a pair of values rather than through a
//! display name: **a tool's identity must not survive a change to the schema calls are validated
//! against.** `ToolIdentity::authorizes` compares the capability, the source, and the schema
//! fingerprint — so if a definition's stated fingerprint described a *different* schema than the one
//! in force, an approval recorded against the identity would still match while the rules deciding
//! acceptance had changed.

// The workspace denies `clippy::panic`, and `clippy.toml`'s test allowances do not classify a
// `#[path]` module reached from the library. In a test a panic is the failure report.
#![allow(clippy::panic)]

use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::tool::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk, Scope,
};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};

use crate::tool_fingerprint::schema_fingerprint_of;
use crate::tool_schema::ToolSchema;

/// A definition whose identity claims the fingerprint of `schema_text`.
fn definition_for(schema_text: &str) -> ToolDefinition {
    ToolDefinition::new(
        ToolIdentity {
            capability: ToolCapability::parse("fs.read@1").expect("a canonical capability"),
            source: ToolSource::new(
                SourceKind::Native,
                "jarvis.core",
                ToolVersion::parse("1.0.0").expect("a valid version"),
            )
            .expect("a usable source"),
            schema_fingerprint: schema_fingerprint_of(schema_text),
        },
        "Read a file",
        "Read one file from the workspace.",
        vec![Effect::ReadOnly],
        Risk::Low,
        vec![Scope::new("fs.read").expect("a usable scope")],
        ApprovalHint::Allow,
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal)
            .expect("an ordered classification"),
        ExecutionDefaults::new(5_000, 1).expect("defaults in range"),
    )
    .expect("the definition is valid")
}

#[test]
fn a_definition_confirms_the_schema_that_its_identity_fingerprints() {
    let text = "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}},\
                \"required\":[\"path\"],\"additionalProperties\":false}";
    let parsed = ToolSchema::parse(text).expect("the schema is usable");
    let definition = definition_for(text);
    // The two values are produced by different code paths — one by the domain's constructor, one by
    // this crate's hasher — and this is the assertion that they agree for the same text.
    assert!(
        parsed
            .confirms(&definition.identity.schema_fingerprint)
            .is_ok(),
        "a definition must confirm the schema its identity fingerprints",
    );
}

#[test]
fn a_definition_that_states_another_schemas_fingerprint_is_refused_by_the_validator() {
    // The defect this composes away: the identity names one schema, the validator holds another.
    let reviewed = "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}}}";
    let looser = "{\"type\":\"object\"}";
    let definition = definition_for(reviewed);
    let in_force = ToolSchema::parse(looser).expect("the schema is usable");
    assert!(
        in_force
            .confirms(&definition.identity.schema_fingerprint)
            .is_err(),
        "the looser schema must not confirm the reviewed fingerprint",
    );
    // And the identity comparison alone CANNOT see this, which is why the check is needed: the two
    // definitions differ only in the fingerprint, so `authorizes` distinguishes them — but a caller
    // that compared the fingerprint would have no way to know which schema produced it.
    let swapped = definition_for(looser);
    assert!(
        !definition.identity.authorizes(&swapped.identity),
        "a changed schema fingerprint must change what the identity authorizes",
    );
}

#[test]
fn a_schema_bound_small_enough_to_be_reviewed_is_accepted_while_an_oversized_one_is_not() {
    // The two bounds meet here: the domain bounds an argument document at `MAX_ARGUMENT_BYTES`, this
    // module bounds a schema at `MAX_SCHEMA_BYTES`, and they are deliberately different numbers — an
    // argument is model output, a schema is reviewed configuration. A schema larger than the schema
    // bound must be refused even though it would fit in an argument document.
    use crate::tool_schema::MAX_SCHEMA_BYTES;
    use jarvis_domain::tool::call::MAX_ARGUMENT_BYTES;
    const _: () = assert!(MAX_SCHEMA_BYTES < MAX_ARGUMENT_BYTES);

    let padding = "a".repeat(MAX_SCHEMA_BYTES);
    assert!(
        ToolSchema::parse(&format!("{{\"title\":\"{padding}\"}}")).is_err(),
        "a schema over the schema bound must be refused",
    );
    assert!(
        ToolSchema::parse("{\"title\":\"short\"}").is_ok(),
        "a schema inside the bound must be accepted",
    );
}

#[test]
fn a_schema_with_no_fingerprint_source_is_not_claimable() {
    // A definition whose fingerprint came from `from_bytes` in a fixture is exactly the shape
    // `schema_fingerprint_of` was written to replace. This asserts the two are different values for
    // the same text, so a fixture-written fingerprint cannot masquerade as a computed one.
    let text = "{\"type\":\"object\"}";
    let computed = schema_fingerprint_of(text);
    let fixture = SchemaFingerprint::from_bytes([7; 32]);
    assert_ne!(computed, fixture);
    let parsed = ToolSchema::parse(text).expect("the schema is usable");
    assert!(parsed.confirms(&computed).is_ok());
    assert!(parsed.confirms(&fixture).is_err());
}
