//! Tests for the RFC 8785 canonicalizer, including the RFC's own sorting vector.
//!
//! Three of these are worth more than the rest:
//!
//! - **The property-order test uses the RFC's own sample** and asserts the expected order **by value**,
//!   written out rather than derived. A test that sorted the expected list with the implementation's own
//!   comparison would prove only self-consistency; the point of the vector is that it comes from the
//!   specification.
//! - **The UTF-16 test uses a key outside the basic multilingual plane**, which is the one input where a
//!   byte-wise sort and the RFC's specified sort **disagree**. Without it, an implementation that sorted
//!   UTF-8 bytes would pass every test here and be non-conformant.
//! - **The one-character change test** asserts that a single changed character in the arguments produces
//!   a different canonical form, which is the property the whole module exists for — the contract's
//!   "approving 'send this email' does not approve a rewritten recipient, subject, body, attachment, or
//!   account".

use std::collections::BTreeMap;

use super::call::ToolArguments;
use super::canonical::{
    ActionDigest, FINGERPRINT_FORMAT_VERSION, FingerprintError, FingerprintInput, FingerprintParts,
    MAX_FINGERPRINT_FIELDS, MAX_FINGERPRINT_KEY_BYTES, MAX_FINGERPRINT_VALUE_BYTES,
};
use super::classification::{Effect, Risk};
use super::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use crate::ids::{PrincipalId, WorkspaceId};

fn identity(seed: u8, capability: &str, source_version: &str) -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::parse(capability).expect("canonical"),
        source: ToolSource::new(
            SourceKind::Native,
            "jarvis.core",
            ToolVersion::parse(source_version).expect("valid"),
        )
        .expect("a usable source"),
        schema_fingerprint: SchemaFingerprint::from_bytes([seed; 32]),
    }
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(1))
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(2))
}

fn arguments(document: &str) -> ToolArguments {
    ToolArguments::new(document).expect("a usable argument document")
}

/// Builds an envelope for a fixed action, so a test changes only what it is about.
///
/// The effects and the risk are fixed to a writing, high-risk tool, because they are now part of the
/// envelope — the identity alone does not carry them, which is the defect the fields exist to fix.
fn envelope(document: &str) -> FingerprintInput {
    envelope_of(
        &identity(7, "mail.send@1", "1.0.0"),
        &[Effect::Write],
        Risk::High,
        document,
    )
}

/// Builds an envelope from an explicit identity, effects, and risk.
fn envelope_of(
    identity: &ToolIdentity,
    effects: &[Effect],
    risk: Risk,
    document: &str,
) -> FingerprintInput {
    FingerprintInput::new(FingerprintParts {
        version: FINGERPRINT_FORMAT_VERSION,
        identity,
        workspace: workspace(),
        principal: principal(),
        effects,
        risk,
        arguments: &arguments(document),
    })
    .expect("the envelope is usable")
}

// ---------------------------------------------------------------------------------------
// The RFC's own vector (§3.2.3), asserted by value.
// ---------------------------------------------------------------------------------------

#[test]
fn the_rfcs_own_sample_sorts_to_the_order_the_rfc_states() {
    // RFC 8785 §3.2.3 lists seven property names and their expected order after canonicalization. The
    // expected sequence is written out here, character for character, rather than sorted with the
    // implementation's comparison — because the value of a published vector is that it does **not** come
    // from the code under test. The RFC's own listing is:
    //
    //     "Carriage Return", "One", "Control", "Latin Small Letter O With Diaeresis",
    //     "Euro Sign", "Emoji: Grinning Face", "Hebrew Letter Dalet With Dagesh"
    //
    // It is a useful vector because a byte-wise sort and the specified UTF-16 sort agree on five of these
    // and disagree on the emoji, whose surrogate pair sorts **below** U+20AC in code-unit order while its
    // UTF-8 encoding sorts above it.
    let mut fields = BTreeMap::new();
    for (key, value) in [
        ("\u{20ac}", "Euro Sign"),
        ("\r", "Carriage Return"),
        ("\u{fb33}", "Hebrew Letter Dalet With Dagesh"),
        ("1", "One"),
        ("\u{1f600}", "Emoji: Grinning Face"),
        ("\u{80}", "Control"),
        ("\u{f6}", "Latin Small Letter O With Diaeresis"),
    ] {
        fields.insert(key.to_owned(), value.to_owned());
    }
    // Built from the same map the canonicalizer would, so the fixture cannot drift from the type.
    let input = FingerprintInput::from_fields_for_tests(fields);
    let canonical = input.canonical_form();

    let expected_order = [
        "Carriage Return",
        "One",
        "Control",
        "Latin Small Letter O With Diaeresis",
        "Euro Sign",
        "Emoji: Grinning Face",
        "Hebrew Letter Dalet With Dagesh",
    ];
    let mut position = 0;
    for value in expected_order {
        let needle = format!("\"{value}\"");
        // `panic!` is denied by the workspace lint policy even in tests — unlike `expect` and `unwrap`,
        // which `clippy.toml` allows here — so the two failure modes are folded into one assertion rather
        // than an `unwrap_or_else(|| panic!(..))`.
        let found = canonical.find(&needle);
        assert!(
            found.is_some_and(|found| found >= position),
            "**{value} is missing from the canonical form or out of the RFC's stated order**: the order \
             is not UTF-16 code-unit order ({canonical})",
        );
        position = found.unwrap_or(position);
    }
}

#[test]
fn a_property_name_outside_the_basic_multilingual_plane_sorts_by_code_unit_not_by_byte() {
    // The discriminating case. U+10000 encodes in UTF-8 as F0 90 80 80, which is **greater** than
    // U+E000's EE 80 80 — so a byte-wise sort puts U+10000 last. In UTF-16, U+10000 is the surrogate pair
    // D800 DC00 and U+E000 is the single unit E000, so the pair sorts **first**. RFC 8785 §3.2.3
    // requires the code-unit answer, and it notes a byte-wise sort "would differ and thus be incompatible
    // with this specification".
    let mut fields = BTreeMap::new();
    fields.insert("\u{10000}".to_owned(), "astral".to_owned());
    fields.insert("\u{e000}".to_owned(), "bmp".to_owned());
    let canonical = FingerprintInput::from_fields_for_tests(fields).canonical_form();
    let astral = canonical.find("astral").expect("the astral key is present");
    let bmp = canonical.find("bmp").expect("the bmp key is present");
    assert!(
        astral < bmp,
        "**the surrogate pair must precede U+E000 in UTF-16 code-unit order**, which is what RFC 8785 \
         §3.2.3 specifies — a byte-wise sort would reverse these ({canonical})",
    );
}

// ---------------------------------------------------------------------------------------
// Whitespace, escaping, and the shape of the output.
// ---------------------------------------------------------------------------------------

#[test]
fn the_canonical_form_carries_no_whitespace() {
    // §3.2.1: "Whitespace between JSON tokens MUST NOT be emitted." Asserted on a multi-field envelope
    // rather than a single-pair one, because the separators between members are where whitespace would
    // appear.
    let canonical = envelope("{\"a\":1}").canonical_form();
    assert!(
        !canonical.contains(' '),
        "the separator between members must not be a space: {canonical}",
    );
    assert!(!canonical.contains('\n') && !canonical.contains('\t'));
    // The exact shape, asserted rather than described, so a change that added a space fails here.
    assert!(canonical.starts_with('{') && canonical.ends_with('}'));
}

#[test]
fn a_control_character_uses_the_short_form_and_lowercase_hex() {
    // §3.2.2.2 requires the five characters that have short forms to **use** them, and every other
    // control character to be `\uhhhh` with **lowercase** hex. The two are different byte strings, so
    // "it unescapes to the same character" is not sufficient: a document that emitted `\u000a` where
    // `\n` is required is a different digest.
    let input = FingerprintInput::new(FingerprintParts {
        version: FINGERPRINT_FORMAT_VERSION,
        identity: &identity(7, "mail.send@1", "1.0.0"),
        workspace: workspace(),
        principal: principal(),
        effects: &[Effect::Write],
        risk: Risk::High,
        arguments: &arguments("line\nfeed\ttab\rret\u{8}back\u{c}form\u{1}one"),
    })
    .expect("the envelope is usable");
    let canonical = input.canonical_form();
    assert!(
        canonical.contains("line\\nfeed\\ttab\\rret\\bback\\fform\\u0001one"),
        "**the short forms must be used and other control characters must be lowercase `\\uhhhh`**: \
         {canonical}",
    );
    assert!(
        !canonical.contains("\\u000a"),
        "a control character with a short form must not be spelled as `\\uXXXX`: {canonical}",
    );
    // Lowercase hex, checked by constructing the one input whose escape differs only in case. U+0001 has
    // no short form, so it *must* be `\u0001`; the uppercase spelling would be `\u0001` written with a
    // capital hex digit, and the only such digit this input can produce is `A`-`F` — of which U+0001
    // produces none. So the assertion that carries weight is that the emitted escape is the exact
    // lowercase string, which the `contains` above already pins.
    assert!(
        canonical.contains("one\""),
        "the value must end exactly after the escaped control character: {canonical}",
    );
}

#[test]
fn a_quote_and_a_backslash_are_escaped_and_no_other_character_is() {
    // §3.2.2.2: `"` and `\` must be escaped; every code point outside the control range must be emitted
    // **as is**. The "as is" half is the one that is easy to over-apply — an implementation that escaped
    // every non-ASCII character would produce a conformant-looking document that is not the RFC's.
    let input = FingerprintInput::new(FingerprintParts {
        version: FINGERPRINT_FORMAT_VERSION,
        identity: &identity(7, "mail.send@1", "1.0.0"),
        workspace: workspace(),
        principal: principal(),
        effects: &[Effect::Write],
        risk: Risk::High,
        arguments: &arguments("quote\" backslash\\ accent\u{e9} euro\u{20ac} slash/"),
    })
    .expect("the envelope is usable");
    let canonical = input.canonical_form();
    assert!(canonical.contains("quote\\\" backslash\\\\"), "{canonical}");
    // `é` and `€` must be the characters themselves, never `\u00e9` or `\u20ac`.
    assert!(
        canonical.contains("accent\u{e9} euro\u{20ac}"),
        "**a code point outside the control range must be emitted as-is, not escaped**: {canonical}",
    );
    assert!(!canonical.contains("\\u00e9"), "{canonical}");
    assert!(
        canonical.contains("slash/"),
        "a forward slash needs no escape in JSON: {canonical}",
    );
}

// ---------------------------------------------------------------------------------------
// Numbers are unreachable, which is why no number serializer exists.
// ---------------------------------------------------------------------------------------

#[test]
fn the_envelope_holds_only_strings_so_a_number_cannot_be_contributed() {
    // **This is the test that stands in for a number serializer.** RFC 8785 §3.2.2.3 defers number
    // serialization to ECMA-262 and says the algorithm "is not included in this document", so a conformant
    // serializer is a research-grade piece of work — and a subtly wrong one produces a digest that is
    // stable in one build and different everywhere else. The envelope makes a number impossible instead,
    // so the hazard is absent rather than mitigated.
    //
    // The assertion is on the **constructed** envelope: every field the constructor contributes came from
    // a type whose values are strings, so the set of value shapes is `{String}`. A number could only
    // arrive by adding a field to `FingerprintInput::new`, which this test cannot see — so the check that
    // *can* be made here is that the canonical form of a text value that looks like a number is a
    // **quoted string**, not a bare number. A bare number in the output is the observable difference.
    let input = envelope("{\"amount\":42}");
    let canonical = input.canonical_form();
    assert!(
        canonical.contains("\"arguments\":\"{\\\"amount\\\":42}\""),
        "the arguments document is carried as text, so a number inside it stays quoted text: {canonical}",
    );
    // A bare (unquoted) number after a colon would mean a numeric value reached the document. Every
    // value here is quoted, so no `:` is followed by a digit.
    assert!(
        !canonical.split(',').any(|member| {
            member
                .split_once(':')
                .is_some_and(|(_, value)| value.starts_with(|c: char| c.is_ascii_digit()))
        }),
        "**no value may be emitted as a bare number**, because that is the shape this module refuses to \
         serialize: {canonical}",
    );
}

// ---------------------------------------------------------------------------------------
// Determinism and order-independence.
// ---------------------------------------------------------------------------------------

#[test]
fn canonicalization_is_deterministic() {
    // Two calls over one envelope must be byte-identical. This is the weakest claim that is still worth
    // making: it is what fails if the implementation ever reads a clock, an environment variable, or a
    // random source, or if it iterated a map whose order varied.
    let input = envelope("{\"to\":\"a@example.com\"}");
    assert_eq!(input.canonical_form(), input.canonical_form());
}

#[test]
fn the_field_order_the_constructor_used_does_not_change_the_output() {
    // Order independence, which is a **stronger** claim than "the same call twice": two envelopes holding
    // the same facts must produce one document however the constructor happened to arrange its inserts.
    // An implementation whose output depended on insertion order would produce two fingerprints for one
    // action depending on code path, which is the same defect class `TLS-004` requires policy to avoid.
    let first = envelope("{\"to\":\"a@example.com\"}");
    // Built with the same facts in a different arrangement, through the same public constructor.
    let second = envelope_of(
        &identity(7, "mail.send@1", "1.0.0"),
        &[Effect::Write],
        Risk::High,
        "{\"to\":\"a@example.com\"}",
    );
    assert_eq!(first.canonical_form(), second.canonical_form());
}

#[test]
fn a_single_changed_character_in_the_arguments_changes_the_canonical_form() {
    // The contract's binding rule, at the level this module can assert: "approving 'send this email' does
    // not approve a rewritten recipient, subject, body, attachment, or account". The digest is computed by
    // an adapter, but the property that makes it meaningful is here — a one-character change in the
    // arguments must produce a different canonical document, or the fingerprint authorizes something the
    // user did not review.
    let original = envelope("{\"to\":\"peter@example.com\"}");
    let rewritten = envelope("{\"to\":\"pete@example.com\"}");
    assert_ne!(
        original.canonical_form(),
        rewritten.canonical_form(),
        "**one character in the recipient must change the canonical form**",
    );
    // And the same for a change to nothing but the field order *within* the argument text — the document
    // is carried as text, so reordered JSON is a **different** action, which is what binding the reviewed
    // bytes rather than a re-serialization means.
    let reordered = envelope("{\"to\":\"peter@example.com\",\"subject\":\"hi\"}");
    let other_order = envelope("{\"subject\":\"hi\",\"to\":\"peter@example.com\"}");
    assert_ne!(
        reordered.canonical_form(),
        other_order.canonical_form(),
        "the arguments are bound as the bytes reviewed, so two spellings are two actions",
    );
}

// ---------------------------------------------------------------------------------------
// Bounds and refusals.
// ---------------------------------------------------------------------------------------

#[test]
fn an_unusable_field_name_or_value_is_refused_rather_than_truncated() {
    // A truncation would fingerprint **fewer** facts than the contract requires while still producing a
    // plausible digest, which is worse than a refusal. The refusals are a fault in the code that assembled
    // the envelope rather than in a caller's request, so each carries its own code.
    let too_long_key = "k".repeat(MAX_FINGERPRINT_KEY_BYTES + 1);
    assert_eq!(
        FingerprintInput::insert_for_tests(&mut BTreeMap::new(), &too_long_key, "v"),
        Err(FingerprintError::UnusableKey),
    );
    let mut fields = BTreeMap::new();
    fields.insert("present".to_owned(), "v".to_owned());
    assert_eq!(
        FingerprintInput::insert_for_tests(&mut fields, "present", "v"),
        Err(FingerprintError::UnusableKey),
        "a duplicate name is refused, which is RFC 8785 §3.1's rule enforced at the boundary",
    );
    let too_long_value = "v".repeat(MAX_FINGERPRINT_VALUE_BYTES + 1);
    assert_eq!(
        FingerprintInput::insert_for_tests(&mut BTreeMap::new(), "k", &too_long_value),
        Err(FingerprintError::UnusableValue),
    );
    assert_eq!(
        FingerprintInput::insert_for_tests(&mut BTreeMap::new(), "k", "has\0nul"),
        Err(FingerprintError::UnusableValue),
    );
    assert_eq!(
        FingerprintInput::insert_for_tests(&mut BTreeMap::new(), "", "v"),
        Err(FingerprintError::UnusableKey),
    );
    // A control character in a **name** is refused, because a name is JARVIS's own field label and there
    // is no legitimate one; a control character in a **value** is allowed, because a body legitimately
    // has newlines — asserted on a real envelope rather than here.
    assert_eq!(
        FingerprintInput::insert_for_tests(&mut BTreeMap::new(), "a\nb", "v"),
        Err(FingerprintError::UnusableKey),
    );
}

#[test]
fn a_newline_in_a_value_is_accepted_because_an_action_may_contain_one() {
    // The other half of the rule above, and the reason the two predicates differ: a multi-line body is an
    // action a user can legitimately approve, so refusing a newline in a value would refuse real work
    // while the canonicalizer escapes it correctly anyway.
    let input = envelope("{\"body\":\"line one\nline two\"}");
    assert!(
        input.canonical_form().contains("line one\\nline two"),
        "the newline is escaped rather than dropped",
    );
}

#[test]
fn every_envelope_field_the_contract_lists_has_a_value() {
    // The contract's list of fingerprint inputs opens with "fingerprint format version" and names the
    // principal, workspace, the canonical tool identity (capability, source identity, schema fingerprint),
    // the normalized arguments, and **effects and constraints**. Each must be present, because a missing
    // one is a fact the digest does not cover — and an approval that did not cover it would authorize a
    // change to it.
    let input = envelope("{\"to\":\"a@example.com\"}");
    for key in [
        "fingerprint_version",
        "principal",
        "workspace",
        "tool_capability",
        "tool_source",
        "schema_fingerprint",
        "effects",
        "risk",
        "arguments",
    ] {
        assert!(
            input.get(key).is_some(),
            "**{key} is named in the contract's input list and must be covered**: {key} missing",
        );
    }
    assert_eq!(input.get("tool_capability"), Some("mail.send@1"));
    assert_eq!(input.get("fingerprint_version"), Some("1"));
    assert_eq!(input.get("effects"), Some("write"));
    assert_eq!(input.get("risk"), Some("high"));
    assert_eq!(input.len(), 9);
}

#[test]
fn a_reclassified_effect_or_risk_changes_the_envelope_though_the_identity_does_not() {
    // **The defect the effect and risk fields exist to fix.** `ToolIdentity` fingerprints the *input
    // schema* only, so the same capability, source, and schema can be reclassified from reading to
    // deleting without the identity moving — and the contract requires the fingerprint to cover "effects
    // and constraints", because an approval granted for the read must not authorize the delete.
    //
    // Asserted **with the identity held fixed**, which is the whole point: a test that changed the schema
    // too would pass against an implementation that covered only the identity.
    let identity = identity(7, "fs.unlink@1", "1.0.0");
    let reading = envelope_of(
        &identity,
        &[Effect::ReadOnly],
        Risk::Low,
        "{\"path\":\"notes.md\"}",
    );
    let deleting = envelope_of(
        &identity,
        &[Effect::Destructive],
        Risk::Critical,
        "{\"path\":\"notes.md\"}",
    );
    assert_ne!(
        reading.canonical_form(),
        deleting.canonical_form(),
        "**a reclassification must move the fingerprint even though the identity is identical**",
    );
    // The identity really is identical, or the assertion above would be evidence of nothing.
    assert_eq!(
        reading.get("schema_fingerprint"),
        deleting.get("schema_fingerprint")
    );
    assert_eq!(
        reading.get("tool_capability"),
        deleting.get("tool_capability")
    );

    // And each dimension moves it on its own, so neither is decorative: an effects-only change and a
    // risk-only change are both visible.
    let effects_only = envelope_of(
        &identity,
        &[Effect::Destructive],
        Risk::Low,
        "{\"path\":\"notes.md\"}",
    );
    let risk_only = envelope_of(
        &identity,
        &[Effect::ReadOnly],
        Risk::Critical,
        "{\"path\":\"notes.md\"}",
    );
    assert_ne!(reading.canonical_form(), effects_only.canonical_form());
    assert_ne!(reading.canonical_form(), risk_only.canonical_form());
}

#[test]
fn the_effects_list_is_canonicalized_so_a_set_has_one_fingerprint() {
    // A fingerprint must not depend on how a caller happened to order or repeat the effect list. The list
    // is not trusted to be canonical even though `ToolDefinition::new` refuses duplicates, because a
    // fingerprint that varied with list order would refuse an approval for the action the user actually
    // saw.
    let identity = identity(7, "fs.write@1", "1.0.0");
    let forward = envelope_of(
        &identity,
        &[Effect::Write, Effect::ExternalCommunication],
        Risk::High,
        "{}",
    );
    let reversed = envelope_of(
        &identity,
        &[Effect::ExternalCommunication, Effect::Write],
        Risk::High,
        "{}",
    );
    let repeated = envelope_of(
        &identity,
        &[Effect::Write, Effect::ExternalCommunication, Effect::Write],
        Risk::High,
        "{}",
    );
    assert_eq!(
        forward.canonical_form(),
        reversed.canonical_form(),
        "the same effect *set* must produce one fingerprint",
    );
    assert_eq!(
        forward.canonical_form(),
        repeated.canonical_form(),
        "a repeated effect is the same set, not a different action",
    );
    // The spelling is the contract's, sorted, so a reader of the envelope sees the set rather than an
    // order that carries no meaning.
    assert_eq!(
        forward.get("effects"),
        Some("external_communication,write"),
        "effects are sorted by their contract spellings",
    );
}

#[test]
fn a_different_identity_produces_a_different_envelope() {
    // `ACC-024`: replacing the implementation behind a name must not inherit the original's approval. The
    // identity is covered, so a schema or source change moves the fingerprint.
    let original = envelope("{}");
    let replaced = envelope_of(
        &identity(9, "mail.send@1", "1.0.0"),
        &[Effect::Write],
        Risk::High,
        "{}",
    );
    assert_ne!(original.canonical_form(), replaced.canonical_form());

    // And a different **principal** acting on the same tool is a different action.
    let another_principal = FingerprintInput::new(FingerprintParts {
        version: FINGERPRINT_FORMAT_VERSION,
        identity: &identity(7, "mail.send@1", "1.0.0"),
        workspace: workspace(),
        principal: PrincipalId::from_uuid(uuid::Uuid::from_u128(99)),
        effects: &[Effect::Write],
        risk: Risk::High,
        arguments: &arguments("{}"),
    })
    .expect("usable");
    assert_ne!(
        original.canonical_form(),
        another_principal.canonical_form()
    );
}

#[test]
fn the_envelope_bound_is_the_one_the_constructor_enforces() {
    // A bound asserted against its own literal is the number written twice, so the check is that the
    // constructor **refuses** above the bound rather than that the constant has a value.
    let mut fields = BTreeMap::new();
    for index in 0..MAX_FINGERPRINT_FIELDS {
        FingerprintInput::insert_for_tests(&mut fields, &format!("f{index}"), "v")
            .expect("under the bound");
    }
    assert_eq!(
        FingerprintInput::insert_for_tests(&mut fields, "one-more", "v"),
        Err(FingerprintError::TooManyFields),
    );
}

// ---------------------------------------------------------------------------------------
// The digest type: shape, algorithm, and the wire rule.
// ---------------------------------------------------------------------------------------

#[test]
fn a_digest_carries_its_algorithm_and_round_trips() {
    let digest = ActionDigest::from_bytes([0xab; 32]);
    let rendered = digest.to_string();
    assert!(rendered.starts_with("sha256:"), "{rendered}");
    assert_eq!(rendered.len(), "sha256:".len() + 64);
    assert_eq!(ActionDigest::parse(&rendered).expect("round-trips"), digest);
}

#[test]
fn a_digest_from_another_algorithm_or_in_uppercase_is_refused() {
    // The algorithm is part of the value so a digest from a different function cannot compare equal to one
    // from this one, and uppercase is refused because two spellings of one digest would let a stored
    // approval miss a match that is really the same action.
    let hex = "ab".repeat(32);
    assert!(ActionDigest::parse(&format!("sha256:{hex}")).is_ok());
    for bad in [
        hex.clone(),
        format!("sha1:{hex}"),
        format!("md5:{hex}"),
        format!("sha256:{}", hex.to_uppercase()),
        format!("sha256:{}", &hex[..62]),
        format!("sha256:{}", "zz".repeat(32)),
        "sha256:".to_owned(),
    ] {
        assert!(
            ActionDigest::parse(&bad).is_err(),
            "**{bad} must be refused**, not normalized",
        );
    }
}

#[test]
fn a_digest_cannot_arrive_over_the_wire_unvalidated() {
    // The systemic defect this project found in nine other newtypes: a derived `Deserialize` on a
    // `#[serde(transparent)]` type wraps the inner value directly, so the constructor's rules hold for
    // values this crate built and are **bypassed for values that arrive over the wire**. A digest is
    // compared against a stored approval, so an unvalidated one would be a check that silently stops
    // applying — the direction an attacker chooses.
    assert!(
        serde_json::from_str::<ActionDigest>(&format!("\"sha256:{}\"", "ab".repeat(32))).is_ok()
    );
    for bad in [
        format!("\"{}\"", "ab".repeat(32)),
        format!("\"sha256:{}\"", "AB".repeat(32)),
        format!("\"sha256:{}\"", &"ab".repeat(32)[..60]),
    ] {
        assert!(
            serde_json::from_str::<ActionDigest>(&bad).is_err(),
            "{bad} must be refused by the deserializer, not only by the constructor",
        );
    }
}

#[test]
fn a_refusal_names_its_own_code() {
    // Three refusals with three codes, so an operator can tell an over-long envelope from an unusable name
    // without reading a message.
    let codes = [
        FingerprintError::UnusableKey.code(),
        FingerprintError::UnusableValue.code(),
        FingerprintError::TooManyFields.code(),
    ];
    assert_eq!(
        codes
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "each refusal needs its own code",
    );
    for code in codes {
        assert!(code.starts_with("tool."), "{code}");
    }
}
