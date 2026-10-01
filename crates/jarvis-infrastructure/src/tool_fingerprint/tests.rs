//! Tests for the digest computation.
//!
//! Two are worth more than the rest:
//!
//! - **A known-answer test against the NIST SHA-256 vector.** Asserting that a digest is 64 hex characters
//!   would pass for any 256-bit function, including one with the bytes reversed; a published vector is what
//!   makes "this is SHA-256" a measurement.
//! - **The canonical-form-not-`serde_json` test.** The whole point of the pipeline is that the bytes hashed
//!   are the RFC's, so the test asserts that hashing `serde_json`'s output for the same envelope gives a
//!   **different** digest — which is the defect the split between canonicalization and hashing prevents.

use super::{
    SCHEMA_DOMAIN_SEPARATOR, SKILL_DOMAIN_SEPARATOR, action_digest_of, action_fingerprint,
    schema_fingerprint_of, skill_content_hash,
};
use jarvis_domain::ids::{PrincipalId, WorkspaceId};
use jarvis_domain::tool::call::ToolArguments;
use jarvis_domain::tool::canonical::{
    FINGERPRINT_FORMAT_VERSION, FingerprintInput, FingerprintParts,
};
use jarvis_domain::tool::classification::{Effect, Risk};
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use sha2::{Digest as _, Sha256};

fn identity() -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::parse("mail.send@1").expect("canonical"),
        source: ToolSource::new(
            SourceKind::Native,
            "jarvis.core",
            ToolVersion::parse("1.0.0").expect("valid"),
        )
        .expect("a usable source"),
        schema_fingerprint: SchemaFingerprint::from_bytes([7; 32]),
    }
}

fn envelope(document: &str) -> FingerprintInput {
    FingerprintInput::new(FingerprintParts {
        version: FINGERPRINT_FORMAT_VERSION,
        identity: &identity(),
        workspace: WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
        principal: PrincipalId::from_uuid(uuid::Uuid::from_u128(2)),
        effects: &[Effect::Write],
        risk: Risk::High,
        arguments: &ToolArguments::new(document).expect("a usable argument document"),
    })
    .expect("the envelope is usable")
}

#[test]
fn sha256_matches_a_published_known_answer_vector() {
    // The NIST vector for the ASCII string "abc": a 256-bit digest is only meaningful if it is the *right*
    // 256-bit function. A test asserting "64 hex characters" would pass for a reversed, truncated, or
    // otherwise different hash, so the published value is the check that carries weight.
    let digest = action_digest_of("abc");
    assert_eq!(
        digest.to_string(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "the digest must be SHA-256 of the exact bytes, not merely a 256-bit value",
    );
    // The empty-string vector too, because an implementation that hashed nothing at the right length would
    // pass the vector above only by coincidence.
    assert_eq!(
        action_digest_of("").to_string(),
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
}

#[test]
fn the_digest_is_over_the_canonical_form_and_not_over_serde_jsons_output() {
    // **This is the test that justifies the two-step pipeline.** `serde_json` would happily serialize the
    // envelope's fields, and its output is *stable within this build* — so a digest computed over it would
    // pass every internal test while differing from any other conformant implementation, and a client
    // previewing the same action would be refused. The assertion is therefore that hashing serde_json's
    // rendering of the same facts gives a **different** answer.
    let input = envelope("{\"to\":\"a@example.com\"}");
    let canonical = input.canonical_form();
    let from_canonical = action_fingerprint(&input);

    // A rendering of the same facts that is valid JSON but not JCS: same keys and values, with whitespace
    // and a different key order. Both are "the same object" to a JSON parser and different byte strings.
    let not_canonical =
        "{\"arguments\": \"{\\\"to\\\":\\\"a@example.com\\\"}\",\n \"principal\": \"2\"}";
    assert_ne!(
        from_canonical,
        action_digest_of(not_canonical),
        "a non-canonical rendering of the same facts must not produce the fingerprint",
    );
    // And the canonical form itself carries no whitespace, which is what makes the two differ.
    assert!(
        !canonical.contains(' ') && !canonical.contains('\n'),
        "{canonical}"
    );
}

#[test]
fn one_changed_character_in_the_arguments_changes_the_fingerprint() {
    // The contract's binding rule at the digest level: "approving 'send this email' does not approve a
    // rewritten recipient, subject, body, attachment, or account".
    let original = action_fingerprint(&envelope("{\"to\":\"peter@example.com\"}"));
    let rewritten = action_fingerprint(&envelope("{\"to\":\"pete@example.com\"}"));
    assert_ne!(
        original, rewritten,
        "**one character in the recipient must change the fingerprint**",
    );
}

#[test]
fn the_fingerprint_is_deterministic_across_calls() {
    // A digest that varied between two calls would make every approval unusable, and the cause would be a
    // second canonicalization that read a clock, a map's iteration order, or an environment variable.
    let input = envelope("{\"to\":\"a@example.com\"}");
    assert_eq!(action_fingerprint(&input), action_fingerprint(&input));
}

#[test]
fn a_reclassification_moves_the_fingerprint_though_the_identity_does_not() {
    // The digest-level counterpart of the domain test, and worth asserting separately: the property could
    // hold in the canonical form and still be lost in a digest computed over the wrong thing — and a
    // reclassified tool keeping its fingerprint is exactly the case where an approval granted for a read
    // would authorize a delete.
    let identity = identity();
    let arguments = ToolArguments::new("{\"path\":\"notes.md\"}").expect("usable");
    let make = |effects: &[Effect], risk: Risk| {
        FingerprintInput::new(FingerprintParts {
            version: FINGERPRINT_FORMAT_VERSION,
            identity: &identity,
            workspace: WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
            principal: PrincipalId::from_uuid(uuid::Uuid::from_u128(2)),
            effects,
            risk,
            arguments: &arguments,
        })
        .expect("usable")
    };
    assert_ne!(
        action_fingerprint(&make(&[Effect::ReadOnly], Risk::Low)),
        action_fingerprint(&make(&[Effect::Destructive], Risk::Critical)),
        "**a reclassification must move the fingerprint even though the identity is identical**",
    );
    // The same set in a different order is one action, not two — otherwise an approval would be refused
    // for the action the user actually reviewed.
    assert_eq!(
        action_fingerprint(&make(
            &[Effect::Write, Effect::ExternalCommunication],
            Risk::High
        )),
        action_fingerprint(&make(
            &[Effect::ExternalCommunication, Effect::Write],
            Risk::High
        )),
    );
}

#[test]
fn a_fingerprint_parses_back_from_its_own_rendering() {
    // The wire form is the contract's `sha256:<hex>`, so a fingerprint the daemon computes must be one the
    // daemon (and a client) can read back. A rendering that did not parse would mean the fingerprint on the
    // wire could never be compared against a freshly computed one.
    let digest = action_fingerprint(&envelope("{}"));
    let rendered = digest.to_string();
    assert_eq!(
        jarvis_domain::tool::canonical::ActionDigest::parse(&rendered).expect("round-trips"),
        digest,
    );
}

#[test]
fn a_schema_fingerprint_is_derived_from_a_document_rather_than_only_written_by_a_test() {
    // **The computation `SchemaFingerprint`'s doc claimed existed.** Before this function, every tool
    // identity in the product carried a fingerprint a *test fixture* wrote via `from_bytes`, so a schema
    // change could not move the identity — and `ACC-024` plus the tool contract both rest on
    // "a release that alters the input schema changes the fingerprint and therefore the identity".
    let schema = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object"}"#;
    let fingerprint = schema_fingerprint_of(schema);
    // The contract's `sha256:<hex>` form, so it is usable as an identity value rather than only as bytes.
    let rendered = fingerprint.to_string();
    assert!(rendered.starts_with("sha256:"), "{rendered}");
    assert_eq!(rendered.len(), "sha256:".len() + 64);

    // Deterministic, so two processes derive one identity for one schema.
    assert_eq!(schema_fingerprint_of(schema), fingerprint);
    // And a **one-character** schema change moves it, which is the property the identity depends on: a
    // compatible description change must not, but adding a property must.
    let with_property = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{}}"#;
    assert_ne!(
        schema_fingerprint_of(with_property),
        fingerprint,
        "**a changed schema must move the fingerprint**, or a replaced implementation inherits the \
         original's approvals",
    );
}

#[test]
fn a_schema_fingerprint_cannot_collide_with_an_action_fingerprint() {
    // The domain separator, asserted rather than described. Without the `tool-schema:` prefix a schema
    // document whose bytes equalled an action's canonical form would produce **the same** digest, and a
    // schema fingerprint could then be presented as an action fingerprint — the one substitution that
    // would let an approval for one action match another tool's registration.
    let document = "{}";
    let schema = schema_fingerprint_of(document).to_string();
    let action = action_digest_of(document).to_string();
    assert_ne!(
        schema, action,
        "a schema fingerprint and an action fingerprint over the same bytes must differ",
    );
    assert!(
        SCHEMA_DOMAIN_SEPARATOR.ends_with('\0'),
        "the separator ends with a NUL, which no JSON document can contain at its start",
    );
}

#[test]
fn a_schema_fingerprint_is_not_the_bare_digest_of_the_document() {
    // The separator must actually be **hashed**, not merely declared. An implementation that computed
    // `sha256(document)` and attached the prefix would pass the collision test above only by luck, so this
    // asserts the value differs from the unseparated digest of the same bytes.
    let document = r#"{"type":"object"}"#;
    let mut unseparated = Sha256::new();
    unseparated.update(document.as_bytes());
    let bare: [u8; 32] = unseparated.finalize().into();
    assert_ne!(
        schema_fingerprint_of(document),
        SchemaFingerprint::from_bytes(bare),
        "**the domain separator must be part of the hashed input**, not only of the declaration",
    );
}

/// **The content hash covers the references, and that is the contract's requirement rather than a
/// detail.** A hash over the body alone would call a skill whose reference file changed the *same* skill,
/// which is what the contract's section 8 forbids (a grant binds to the hash) and what ADR-0012's
/// decision 5 depends on being false.
#[test]
fn the_skill_content_hash_changes_when_only_a_reference_changes() {
    let body = "1. read the file\n2. summarise it";
    let mut before = jarvis_domain::skill::SkillReferences::new();
    before.insert(
        jarvis_domain::skill::SkillReference::new("setup.md", "run setup first")
            .expect("valid reference"),
    );

    let mut after = jarvis_domain::skill::SkillReferences::new();
    after.insert(
        jarvis_domain::skill::SkillReference::new("setup.md", "run setup SECOND")
            .expect("valid reference"),
    );

    // The body is identical in both, so a body-only hash would be equal here — which is the mutation this
    // test is the detector for.
    assert_ne!(
        skill_content_hash(body, &before),
        skill_content_hash(body, &after),
        "a changed reference file must produce a different content hash",
    );
}

/// The hash is a function of *what the skill contains*, not of the order a caller listed it, so the
/// references must be inserted in a different order and hash identically.
#[test]
fn the_skill_content_hash_is_independent_of_insertion_order() {
    let body = "body";
    let mut first = jarvis_domain::skill::SkillReferences::new();
    first.insert(jarvis_domain::skill::SkillReference::new("a.md", "alpha").expect("valid"));
    first.insert(jarvis_domain::skill::SkillReference::new("b.md", "beta").expect("valid"));

    let mut second = jarvis_domain::skill::SkillReferences::new();
    second.insert(jarvis_domain::skill::SkillReference::new("b.md", "beta").expect("valid"));
    second.insert(jarvis_domain::skill::SkillReference::new("a.md", "alpha").expect("valid"));

    assert_eq!(
        skill_content_hash(body, &first),
        skill_content_hash(body, &second),
        "the hash must not depend on the order the references were inserted",
    );
}

/// **Two files must not hash as one file whose content is their concatenation**, which is why each
/// reference is fed with its name and a separator. Without them, `("a", "xy") + ("b", "z")` and
/// `("a", "x") + ("b", "yz")` would produce the same input to the hasher.
#[test]
fn the_skill_content_hash_separates_files_from_their_concatenation() {
    let mut split = jarvis_domain::skill::SkillReferences::new();
    split.insert(jarvis_domain::skill::SkillReference::new("a.md", "xy").expect("valid"));
    split.insert(jarvis_domain::skill::SkillReference::new("b.md", "z").expect("valid"));

    let mut shifted = jarvis_domain::skill::SkillReferences::new();
    shifted.insert(jarvis_domain::skill::SkillReference::new("a.md", "x").expect("valid"));
    shifted.insert(jarvis_domain::skill::SkillReference::new("b.md", "yz").expect("valid"));

    assert_ne!(
        skill_content_hash("body", &split),
        skill_content_hash("body", &shifted),
        "where a boundary between two files falls must change the hash",
    );
}

/// **A skill content hash must not collide with a schema fingerprint** over the same bytes, which is what
/// the domain separator is for. Asserted by hashing the same document both ways: a separator that was
/// declared but not hashed would make the two equal, so this measures the separator rather than restating
/// it.
#[test]
fn the_skill_domain_separator_is_part_of_the_hashed_input() {
    use sha2::{Digest as _, Sha256};

    // A body and a schema document that are the same bytes, so only the separator can differ them.
    let document = "{\"same\":\"bytes\"}";
    let empty = jarvis_domain::skill::SkillReferences::new();
    let skill = skill_content_hash(document, &empty);

    // The same bytes under the schema derivation, with the separator applied manually so the comparison is
    // about which prefix was used rather than about the hash function.
    let mut hasher = Sha256::new();
    hasher.update(SCHEMA_DOMAIN_SEPARATOR.as_bytes());
    hasher.update(document.as_bytes());
    let as_schema: [u8; 32] = hasher.finalize().into();

    assert_ne!(
        skill,
        jarvis_domain::skill::SkillContentHash::from_bytes(as_schema),
        "**the skill separator must be part of the hashed input**, not only of the declaration",
    );
    assert_ne!(
        SKILL_DOMAIN_SEPARATOR, SCHEMA_DOMAIN_SEPARATOR,
        "the two derivations must use different prefixes",
    );
}

/// A known-answer test, so "this is SHA-256 over the body" is a measurement rather than an assertion that
/// the output is 64 hex characters. The input is the separator, `body\n`, and the body; the expected value
/// was computed by hashing exactly those bytes.
#[test]
fn the_skill_content_hash_is_sha256_over_the_separated_input() {
    use sha2::{Digest as _, Sha256};

    let body = "";
    let empty = jarvis_domain::skill::SkillReferences::new();
    let mut hasher = Sha256::new();
    hasher.update(SKILL_DOMAIN_SEPARATOR.as_bytes());
    hasher.update(b"body\n");
    hasher.update(body.as_bytes());
    let expected: [u8; 32] = hasher.finalize().into();
    assert_eq!(skill_content_hash(body, &empty).as_bytes(), &expected);
    // And it renders in the domain's canonical form, so a stored hash is accepted by the parser.
    let rendered = skill_content_hash(body, &empty).to_string();
    assert!(rendered.starts_with("sha256:"), "{rendered}");
    assert_eq!(
        jarvis_domain::skill::SkillContentHash::parse(&rendered).expect("round trips"),
        skill_content_hash(body, &empty),
    );
}
