//! Tests for canonical tool identity and the `ACC-024` replacement rule.
//!
//! The subject of these tests is the rule that **replacing a tool's implementation behind the
//! same display name must not inherit the original's authorization**. That rule is only worth
//! asserting if a naive implementation would fail it, so each test here names what the naive
//! version would have done: comparing display names, comparing capabilities alone, or treating
//! the fingerprint as informational.
//!
//! A deliberately **distinct** helper builds every identity, because the rule has three
//! components and a test that varied all three at once could not say which one a failure came
//! from. Each test varies exactly the component under test.

use super::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk, Scope,
};
use super::definition::ToolDefinition;
use super::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use crate::error::DomainError;
use crate::model::policy::Sensitivity;

fn version(value: &str) -> ToolVersion {
    ToolVersion::parse(value).expect("the fixture version is canonical")
}

fn fingerprint(seed: u8) -> SchemaFingerprint {
    SchemaFingerprint::from_bytes([seed; 32])
}

/// Builds an identity for one capability from one source with one fingerprint.
///
/// Named arguments would be clearer here than positional ones, but the point of the helper is
/// that a test states **which** component it is changing; the tests below call it with the
/// component under test differing from the others.
fn identity(capability: &str, owner: &str, seed: u8) -> ToolIdentity {
    let capability = ToolCapability::parse(capability).expect("the fixture capability parses");
    let source = ToolSource::new(SourceKind::McpServer, owner, version("1.0.0"))
        .expect("the fixture source is valid");
    ToolIdentity {
        capability,
        source,
        schema_fingerprint: fingerprint(seed),
    }
}

fn scope(value: &str) -> Scope {
    Scope::new(value).expect("the fixture scope is valid")
}

/// The field a definition refusal named, or `None` for any other outcome.
///
/// `DomainError` carries rich variants but is not `PartialEq` — deliberately, since two
/// errors with the same message from different causes are not one error — so a test asserts
/// against the *field*, which is the part an operator acts on. Comparing a whole error would
/// also require `PartialEq` on the error type for the test's convenience, which is the tail
/// wagging the dog.
fn refused_field<T>(result: &Result<T, DomainError>) -> Option<&'static str> {
    match result.as_ref().err() {
        Some(DomainError::ToolDefinitionInvalid { field }) => Some(field),
        _ => None,
    }
}

/// The stable code of a refusal, or `None` if it was not one.
fn refusal_code<T>(result: &Result<T, DomainError>) -> Option<&'static str> {
    result.as_ref().err().map(DomainError::code)
}

/// The parsed value, or `None` on refusal. For the positive controls.
fn accepted<T>(result: Result<T, DomainError>) -> Option<T> {
    result.ok()
}

/// Builds a valid, ordinary definition for use as the base of a mutation.
fn definition(identity: ToolIdentity) -> ToolDefinition {
    ToolDefinition::new(
        identity,
        "Read a file",
        "Read one file from the workspace.",
        vec![Effect::ReadOnly],
        Risk::Low,
        vec![scope("fs.read")],
        ApprovalHint::Allow,
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal)
            .expect("the fixture classification is ordered"),
        ExecutionDefaults::new(5_000, 1).expect("the fixture defaults are in range"),
    )
    .expect("the fixture definition is valid")
}

// ---------------------------------------------------------------------------------------
// Capability parsing.
// ---------------------------------------------------------------------------------------

#[test]
fn a_capability_round_trips_through_its_canonical_form() {
    let capability = ToolCapability::parse("email.send@1").expect("the canonical form parses");
    assert_eq!(capability.namespace(), "email");
    assert_eq!(capability.name(), "send");
    assert_eq!(capability.major(), 1);
    assert_eq!(capability.to_string(), "email.send@1");
    // Re-parsing the rendered form must produce the same value, because a stored capability is
    // re-read from its own spelling: if the two disagreed, a persisted definition would parse
    // into something else.
    assert_eq!(
        ToolCapability::parse(&capability.to_string()).expect("the rendered form parses"),
        capability,
    );
}

#[test]
fn a_non_canonical_capability_is_refused_rather_than_normalized() {
    // Each of these has a plausible "helpful" reading, and accepting any of them would let two
    // spellings denote one identity — which is precisely how an approval for one spelling ends
    // up authorizing a tool described by another.
    for value in [
        "email.send",         // no major
        "email.send@",        // empty major
        "email.send@0",       // an uninitialised field reads as version zero
        "email.send@01",      // a leading zero is a second spelling of 1
        "Email.send@1",       // uppercase segment
        "email.Send@1",       // uppercase name
        "email..send@1",      // empty segment
        ".send@1",            // empty namespace
        "email.@1",           // empty name
        "email.send.x@1",     // a dot inside the name
        "email.send@1@2",     // two majors
        "emailsend@1",        // no dot
        "email.send@-1",      // negative
        "email.send@1.0",     // the release belongs on the version, not the capability
        "1email.send@1",      // a digit cannot start a segment
        "email.send@1000000", // over the bound
        "",
    ] {
        assert_eq!(
            refusal_code(&ToolCapability::parse(value)),
            Some("tool.identifier_not_canonical"),
            "{value:?} must be refused as non-canonical",
        );
    }
}

#[test]
fn building_a_capability_from_parts_enforces_the_parser_rule() {
    // A caller that splits an id itself must not be able to construct a value the parser would
    // have refused; otherwise validation lives only on one constructor and the type's
    // guarantee is only as strong as the constructor a caller happened to pick.
    assert!(ToolCapability::new("email", "send", 1).is_ok());
    for (namespace, name, major) in [
        ("Email", "send", 1),
        ("email", "Send", 1),
        ("email", "send", 0),
        ("", "send", 1),
        ("email", "", 1),
        ("email", "send", 1_000_000),
    ] {
        assert_eq!(
            refusal_code(&ToolCapability::new(namespace, name, major)),
            Some("tool.identifier_not_canonical"),
            "({namespace:?}, {name:?}, {major}) must be refused",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Schema fingerprints.
// ---------------------------------------------------------------------------------------

#[test]
fn a_fingerprint_carries_its_algorithm() {
    let digest = [0xab_u8; 32];
    let rendered = SchemaFingerprint::from_bytes(digest).to_string();
    assert!(
        rendered.starts_with("sha256:"),
        "the contract writes the algorithm, so it must be part of the value; got {rendered}",
    );
    assert_eq!(rendered.len(), "sha256:".len() + 64);
    assert_eq!(
        SchemaFingerprint::parse(&rendered).expect("the rendered form parses"),
        SchemaFingerprint::from_bytes(digest),
    );
}

#[test]
fn a_fingerprint_from_another_algorithm_is_refused() {
    // **This is the reason the algorithm is in the value.** Without it, a digest computed by a
    // different function would compare equal to one computed by this one whenever the bytes
    // matched, so switching algorithms would silently either invalidate every stored approval or
    // silently preserve one across a change that should have invalidated it. Requiring the
    // prefix makes the second outcome unrepresentable.
    let hex = "ab".repeat(32);
    assert!(SchemaFingerprint::parse(&format!("sha256:{hex}")).is_ok());
    for value in [
        format!("sha512:{hex}"),
        hex.clone(),
        format!("sha256:{hex}0"),
        format!("sha256:{}", "AB".repeat(32)),
        "sha256:".to_owned(),
        format!("sha256:{}", "gg".repeat(32)),
    ] {
        assert_eq!(
            refused_field(&SchemaFingerprint::parse(&value)),
            Some("schema_fingerprint"),
            "{value:?} must be refused",
        );
    }
}

// ---------------------------------------------------------------------------------------
// ACC-024: the replacement rule.
// ---------------------------------------------------------------------------------------

#[test]
fn the_same_name_from_a_different_source_is_not_the_same_tool() {
    // The scenario `ACC-024` describes: two MCP servers both publish a tool whose display name
    // is "Read a file" and whose capability is `fs.read@1`. A naive identity keyed on the
    // capability — or on the display name — would say the second is authorized by the first's
    // approval. Keying on the source as well is what makes it a **replacement**.
    let original = identity("fs.read@1", "acme.files", 1);
    let replacement = identity("fs.read@1", "attacker.files", 1);
    assert_ne!(original, replacement);
    assert!(
        !original.authorizes(&replacement),
        "an approval for one publisher's `fs.read@1` must not authorize another publisher's",
    );
    assert!(
        original.is_replacement_of(&replacement),
        "the two are replacements of one capability, which the caller needs told separately \
         from `no such tool`",
    );
    // And the replacement does not authorize the original either: the relation is symmetric,
    // because "which one was approved" must not depend on the comparison's direction.
    assert!(!replacement.authorizes(&original));
}

#[test]
fn a_changed_schema_under_the_same_source_is_not_the_same_tool() {
    // The other half of the same rule: one publisher, one capability, one version — and a
    // schema that changed. Without the fingerprint in identity this pair is indistinguishable,
    // and the approval for the old schema would cover calls made with the new one.
    let original = identity("fs.read@1", "acme.files", 1);
    let after_change = identity("fs.read@1", "acme.files", 2);
    assert_ne!(original, after_change);
    assert!(!original.authorizes(&after_change));
    assert!(original.is_replacement_of(&after_change));
}

#[test]
fn a_new_major_is_a_different_capability_not_a_replacement_of_the_old() {
    // The contract says a breaking change requires a new major identity. So `@2` is a
    // *different tool*, not a replacement of `@1` — and the two are distinguishable, which a
    // caller reporting a refusal needs: "the tool you approved was replaced" is a different
    // conversation from "that tool is not this one".
    let first = identity("fs.read@1", "acme.files", 1);
    let second = identity("fs.read@2", "acme.files", 1);
    assert!(!first.authorizes(&second));
    assert!(
        !first.is_replacement_of(&second),
        "a new major is a different capability, not a replacement of the old one",
    );
    assert_eq!(first.capability.namespace(), second.capability.namespace());
    assert_eq!(first.capability.name(), second.capability.name());
}

#[test]
fn an_identical_identity_authorizes_and_is_not_a_replacement() {
    // The positive control for all three tests above: if this failed, the rule would be
    // "nothing ever authorizes anything", which is a policy that refuses everything rather
    // than a correct comparison.
    let one = identity("fs.read@1", "acme.files", 1);
    let same = identity("fs.read@1", "acme.files", 1);
    assert!(one.authorizes(&same));
    assert!(!one.is_replacement_of(&same));
}

#[test]
fn the_display_name_is_not_part_of_identity() {
    // Asserted structurally, by building two definitions that differ **only** in display name
    // and requiring them to be the same tool. That is the contract's "names are aliases"
    // rule: the alias must not be able to change which implementation is authorized, and the
    // only way to guarantee it is for the alias to be absent from what a grant binds to.
    let base = identity("fs.read@1", "acme.files", 1);
    let first = definition(base.clone());
    let mut renamed = definition(base);
    renamed.display_name = "totally different alias".to_owned();
    assert!(
        first.is_same_tool_as(&renamed),
        "renaming a tool must not change its identity, or an alias would be authorization",
    );
}

// ---------------------------------------------------------------------------------------
// Definition invariants.
// ---------------------------------------------------------------------------------------

#[test]
fn a_definition_may_not_declare_no_effects() {
    // An empty list is not the same claim as `read_only`: one says nothing, the other says the
    // tool reads. Defaulting to `read_only` — the obvious convenience — would report a tool as
    // safe because its author omitted a field, which is the classification a reviewer trusts.
    let error = ToolDefinition::new(
        identity("fs.read@1", "acme.files", 1),
        "Read a file",
        "Read one file.",
        Vec::new(),
        Risk::Low,
        vec![scope("fs.read")],
        ApprovalHint::Allow,
        Idempotency::None,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
        ExecutionDefaults::new(5_000, 1).expect("in range"),
    )
    .expect_err("an effectless definition must be refused");
    assert_eq!(error.code(), "tool.definition_invalid");
}

#[test]
fn read_only_may_not_be_combined_with_another_effect() {
    // "Read and write" is a contradiction, and a policy branch that tested `has_effect(ReadOnly)`
    // would answer `true` for it. Refusing the combination means such a branch cannot be reached
    // with a definition that satisfies it only by accident.
    let error = ToolDefinition::new(
        identity("fs.read@1", "acme.files", 1),
        "Read a file",
        "Read one file.",
        vec![Effect::ReadOnly, Effect::Write],
        Risk::Moderate,
        vec![scope("fs.read")],
        ApprovalHint::Ask,
        Idempotency::None,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
        ExecutionDefaults::new(5_000, 1).expect("in range"),
    )
    .expect_err("read_only plus write must be refused");
    assert_eq!(error.code(), "tool.definition_invalid");
}

#[test]
fn a_high_risk_tool_with_no_scopes_may_not_default_to_allow() {
    // The risk label is a review artifact. A definition that says "this is high risk" and also
    // "do it without asking" makes the label decorative, so the combination is refused. `Ask`
    // and `Deny` remain available, which is what keeps a genuinely prompt-free read tool legal.
    let build = |approval| {
        ToolDefinition::new(
            identity("fs.write@1", "acme.files", 1),
            "Write a file",
            "Write one file.",
            vec![Effect::Write],
            Risk::High,
            Vec::new(),
            approval,
            Idempotency::None,
            DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
            ExecutionDefaults::new(5_000, 1).expect("in range"),
        )
    };
    assert_eq!(
        build(ApprovalHint::Allow)
            .expect_err("high risk with no scope and no prompt must be refused")
            .code(),
        "tool.definition_invalid",
    );
    assert!(build(ApprovalHint::Ask).is_ok());
    assert!(build(ApprovalHint::Deny).is_ok());
    // And a low-risk tool with no scopes may still default to allow, so the rule refuses the
    // dangerous combination rather than the ordinary one.
    assert!(
        ToolDefinition::new(
            identity("fs.read@1", "acme.files", 1),
            "Read a file",
            "Read one file.",
            vec![Effect::ReadOnly],
            Risk::Low,
            Vec::new(),
            ApprovalHint::Allow,
            Idempotency::NaturallyIdempotent,
            DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
            ExecutionDefaults::new(5_000, 1).expect("in range"),
        )
        .is_ok(),
    );
}

#[test]
fn the_capability_major_and_the_release_major_must_agree() {
    // These are two numbers that mean different things — one changes the tool, the other is a
    // release — so a definition where `@2` is implemented by `1.4.0` has no coherent reading.
    // Refusing it means the two cannot drift silently.
    let capability = ToolCapability::parse("fs.read@2").expect("canonical");
    let source =
        ToolSource::new(SourceKind::Native, "jarvis.native", version("1.0.0")).expect("valid");
    let mismatched = ToolIdentity {
        capability,
        source,
        schema_fingerprint: fingerprint(1),
    };
    assert_eq!(
        ToolDefinition::new(
            mismatched,
            "Read a file",
            "Read one file.",
            vec![Effect::ReadOnly],
            Risk::Low,
            vec![scope("fs.read")],
            ApprovalHint::Ask,
            Idempotency::NaturallyIdempotent,
            DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
            ExecutionDefaults::new(5_000, 1).expect("in range"),
        )
        .expect_err("a mismatched major must be refused")
        .code(),
        "tool.definition_invalid",
    );
}

#[test]
fn a_duplicate_effect_or_scope_is_refused() {
    // Duplicates are refused rather than deduplicated because the list is what a reviewer read.
    // Silently collapsing `[write, write]` to `[write]` would accept a definition whose declared
    // content the reviewer never saw.
    let duplicate_effects = ToolDefinition::new(
        identity("fs.write@1", "acme.files", 1),
        "Write a file",
        "Write one file.",
        vec![Effect::Write, Effect::Write],
        Risk::Moderate,
        vec![scope("fs.write")],
        ApprovalHint::Ask,
        Idempotency::CallerKeyed,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
        ExecutionDefaults::new(5_000, 1).expect("in range"),
    );
    assert!(duplicate_effects.is_err());

    let duplicate_scopes = ToolDefinition::new(
        identity("fs.write@1", "acme.files", 1),
        "Write a file",
        "Write one file.",
        vec![Effect::Write],
        Risk::Moderate,
        vec![scope("fs.write"), scope("fs.write")],
        ApprovalHint::Ask,
        Idempotency::CallerKeyed,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
        ExecutionDefaults::new(5_000, 1).expect("in range"),
    );
    assert!(duplicate_scopes.is_err());
}

// ---------------------------------------------------------------------------------------
// Closed enumerations.
// ---------------------------------------------------------------------------------------

#[test]
fn an_unknown_classification_value_is_refused_rather_than_defaulted() {
    // **Every one of these has a fail-open default that would be easy to write.** `Effect`,
    // `Risk`, `ApprovalHint`, and `Idempotency` all parse their closed sets, and the
    // dangerous version of each is a catch-all arm: an unknown effect ignored, an unknown risk
    // read as `Low`, an unknown approval read as `Allow`, an unknown idempotency read as
    // `NaturallyIdempotent` (skipping the reservation). Each is asserted by its own field name
    // so a change to any one arm is caught by the case for it.
    assert_eq!(refused_field(&Effect::parse("teleport")), Some("effects"),);
    assert_eq!(refused_field(&Risk::parse("severe")), Some("risk"));
    assert_eq!(
        refused_field(&ApprovalHint::parse("maybe")),
        Some("default_approval"),
    );
    assert_eq!(
        refused_field(&Idempotency::parse("probably")),
        Some("idempotency"),
    );
}

#[test]
fn every_effect_parses_from_its_contract_spelling() {
    // The positive control for the test above, and the guard on the spelling table: iterating
    // the constants means adding an effect without a parse arm fails here rather than making the
    // new effect unusable-but-silent.
    for effect in Effect::ALL {
        assert_eq!(
            accepted(Effect::parse(effect.as_contract_str())),
            Some(*effect),
        );
    }
    assert_eq!(
        Effect::ALL.len(),
        8,
        "the contract lists eight base effects"
    );
}

#[test]
fn consequential_effects_are_named_once() {
    // The classifier is what policy asks instead of keeping its own list, so a second list
    // would be the one that drifts. The assertion names the members rather than only counting
    // them, because a count would still pass if `Financial` were swapped for `Write`.
    for effect in Effect::ALL {
        let expected = matches!(
            effect,
            Effect::Destructive | Effect::Financial | Effect::Privileged | Effect::Physical
        );
        assert_eq!(
            effect.is_consequential(),
            expected,
            "{effect} consequentiality must be classified deliberately",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Cross-field classification rules.
// ---------------------------------------------------------------------------------------

#[test]
fn a_tool_may_not_classify_its_output_below_its_input() {
    // A tool that reads `Restricted` data cannot honestly declare a `Public` result: the result
    // is derived from the input, so the only direction that can be justified from the input
    // alone is upward. Allowing the other direction would let a tool launder a classification
    // by declaring a smaller one — the mechanism a data-exfiltration path would use.
    assert!(
        DataClasses::new(Sensitivity::Confidential, Sensitivity::Confidential).is_ok(),
        "equal is legitimate",
    );
    assert!(
        DataClasses::new(Sensitivity::Internal, Sensitivity::Restricted).is_ok(),
        "raising the output label is derivable — a summary may be more sensitive, not less",
    );
    assert_eq!(
        refused_field(&DataClasses::new(
            Sensitivity::Restricted,
            Sensitivity::Public
        )),
        Some("data_classes"),
    );
}

#[test]
fn execution_defaults_refuse_a_zero_timeout_or_attempt_budget() {
    // Neither describes an execution: zero attempts means the call can never be made, and a
    // zero timeout means it is refused before it starts. Both are what an unset field produces,
    // so accepting them would turn a construction mistake into a tool that silently never runs.
    assert_eq!(
        refused_field(&ExecutionDefaults::new(0, 1)),
        Some("timeout_ms"),
    );
    assert_eq!(
        refused_field(&ExecutionDefaults::new(5_000, 0)),
        Some("max_attempts"),
    );
    assert_eq!(
        refused_field(&ExecutionDefaults::new(
            super::classification::MAX_TOOL_TIMEOUT_MS + 1,
            1
        )),
        Some("timeout_ms"),
    );
    assert_eq!(
        refused_field(&ExecutionDefaults::new(
            5_000,
            super::classification::MAX_TOOL_ATTEMPTS + 1
        )),
        Some("max_attempts"),
    );
    assert!(ExecutionDefaults::new(5_000, 3).is_ok());
}

#[test]
fn a_scope_must_be_bounded_and_control_free() {
    assert!(Scope::new("email.send").is_ok());
    assert!(Scope::new("fs.read-write_v2").is_ok());
    for value in [
        "",
        "Email.Send",
        "email send",
        "email\tsend",
        "email\nsend",
        "email\0send",
        "email/send",
    ] {
        assert_eq!(
            refused_field(&Scope::new(value)),
            Some("required_scopes"),
            "{value:?} must be refused",
        );
    }
    assert!(Scope::new(&"a".repeat(super::classification::MAX_SCOPE_BYTES + 1)).is_err());
}

#[test]
fn a_display_name_may_contain_spaces_and_case_but_not_control_characters() {
    // The alias is shown to a user, so it is deliberately looser than an identity segment — and
    // the assertion states which characters that looseness excludes, because a control character
    // in an approval prompt is the whole reason for the bound.
    assert!(super::classification::is_display_name("Read a file"));
    assert!(super::classification::is_display_name("Send Email (Gmail)"));
    assert!(!super::classification::is_display_name(""));
    assert!(!super::classification::is_display_name("Read\u{7}a file"));
    assert!(!super::classification::is_display_name("Read\u{0}a file"));
}

// ---------------------------------------------------------------------------------------
// Serialization stability.
// ---------------------------------------------------------------------------------------

#[test]
fn an_identity_survives_a_json_round_trip() {
    // Identities are persisted in approvals and in the call ledger, so the wire form is
    // contract surface: a round trip that changed the value would let a stored approval miss a
    // matching call.
    let original = identity("fs.read@1", "acme.files", 7);
    let json = serde_json::to_string(&original).expect("an identity serializes");
    assert!(
        json.contains("\"fs.read@1\""),
        "the capability must render in its canonical form; got {json}",
    );
    let restored: ToolIdentity = serde_json::from_str(&json).expect("an identity deserializes");
    assert_eq!(restored, original);
    // And a non-canonical spelling on the wire is refused rather than normalized, which is the
    // deserialization half of the parse rule.
    let malformed = json.replace("fs.read@1", "fs.read@01");
    assert!(serde_json::from_str::<ToolIdentity>(&malformed).is_err());
}

#[test]
fn a_definition_survives_a_json_round_trip() {
    let original = definition(identity("fs.read@1", "acme.files", 7));
    let json = serde_json::to_string(&original).expect("a definition serializes");
    let restored: ToolDefinition = serde_json::from_str(&json).expect("a definition deserializes");
    assert_eq!(restored, original);
    assert_eq!(restored.capability(), original.capability());
}

#[test]
fn the_definition_reports_its_own_consequentiality() {
    // Computed from the effects rather than stored as a flag, so it cannot disagree with them.
    let read = definition(identity("fs.read@1", "acme.files", 1));
    assert!(!read.is_consequential());
    assert!(read.has_effect(Effect::ReadOnly));

    let destructive = ToolDefinition::new(
        identity("fs.delete@1", "acme.files", 1),
        "Delete a file",
        "Delete one file.",
        vec![Effect::Destructive],
        Risk::Critical,
        vec![scope("fs.delete")],
        ApprovalHint::Ask,
        Idempotency::CallerKeyed,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
        ExecutionDefaults::new(5_000, 1).expect("in range"),
    )
    .expect("a destructive definition with a scope is valid");
    assert!(destructive.is_consequential());
    assert!(!destructive.has_effect(Effect::ReadOnly));
}

#[test]
fn a_caller_keyed_tool_requires_a_key_and_the_others_do_not() {
    // The predicate the executor uses to decide whether a reservation is needed. A tool that
    // declares `none` must still need one — `None` means "not idempotent", and the reservation
    // is what makes a retry safe — so the two answers are deliberately different.
    assert!(Idempotency::CallerKeyed.requires_caller_key());
    assert!(!Idempotency::None.requires_caller_key());
    assert!(!Idempotency::NaturallyIdempotent.requires_caller_key());
}

#[test]
fn source_kind_classifies_externality_deliberately() {
    // The classifier exists so a source added later must be classified rather than defaulting to
    // trusted. The assertion names both sides, because a check that only asserted the external
    // ones would pass if a native source were reclassified as external.
    assert!(!SourceKind::Native.is_external());
    assert!(!SourceKind::Connector.is_external());
    assert!(SourceKind::McpServer.is_external());
    assert!(SourceKind::Runtime.is_external());
    assert!(SourceKind::Plugin.is_external());
}

// ---------------------------------------------------------------------------------------
// ACC-024, stated as the acceptance scenario rather than as its parts.
// ---------------------------------------------------------------------------------------

/// `ACC-024`: *"Replace an MCP/plugin tool schema/source behind the same display name.
/// Existing approval/grant cannot authorize the replacement."*
///
/// The tests above vary one identity component at a time, which says which component a
/// failure came from. This one runs the scenario as written, because a rule that holds for
/// each component separately could still be bypassed by a path that compares definitions —
/// and the scenario is what the acceptance criterion actually requires. It replaces **both**
/// the source and the schema behind one display name, which is what an operator's "I
/// reinstalled the MCP server" looks like, and it asserts the consequence in the terms an
/// approval would use: an approval recorded against the original does not cover the
/// replacement.
#[test]
fn an_approval_for_a_tool_does_not_authorize_a_replacement_behind_the_same_name() {
    let original = definition(identity("fs.read@1", "acme.files", 1));
    // The same display name, the same purpose, the same capability — a different publisher
    // **and** a different schema, which is what "replaced" means in practice.
    let replacement = definition(identity("fs.read@1", "other.files", 9));

    assert_eq!(
        original.display_name, replacement.display_name,
        "the scenario is specifically about a replacement that keeps the display name; if the \
         names differed this test would be about something easier and would not match ACC-024",
    );
    assert_eq!(
        original.capability(),
        replacement.capability(),
        "and it keeps the capability, so a check that compared capabilities alone would pass it",
    );
    assert!(
        !original.is_same_tool_as(&replacement),
        "an approval bound to the original definition must not authorize the replacement",
    );
    // Both directions, because "which one was approved" must not depend on argument order: an
    // asymmetric comparison would let a caller authorize a replacement by passing the arguments
    // the other way round.
    assert!(!replacement.is_same_tool_as(&original));
    // And the identity the approval recorded is the one that must be re-presented, so a caller
    // can name exactly what it holds a grant for.
    assert!(original.is_same_tool_as(&original.clone()));
    assert_eq!(
        original.identity.schema_fingerprint,
        fingerprint(1),
        "the original keeps its own fingerprint; the replacement's differs",
    );
    assert_ne!(
        original.identity.schema_fingerprint,
        replacement.identity.schema_fingerprint,
    );
}
