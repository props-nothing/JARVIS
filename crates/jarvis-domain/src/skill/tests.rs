//! Tests for the skill contract's authority-narrowing invariant.
//!
//! **Two structural tests, and the rest is the invariant dimension by dimension.**
//! `a_skill_that_declares_the_whole_authority_is_accepted` establishes that the operation can accept
//! something — without it, every refusal below would be satisfied by an implementation that refuses
//! *everything*, which is the failure mode the plugin verifier's tests record. `a_skill_that_adds_a_tool_is_refused`
//! then establishes the direction, and each remaining test exercises one dimension by its own expansion,
//! so a reversed comparison fails on the dimension it reversed rather than being masked by another.
//!
//! Every refusal test asserts the **code**, not merely that something was refused. A test that only
//! checked `is_err` would pass for an implementation that refused the wrong dimension — which is exactly
//! what a reversed subset test does.

use std::collections::BTreeSet;

use crate::error::DomainError;
use crate::model::policy::Sensitivity;
use crate::skill::{
    ApprovalHint, Effect, MAX_SKILL_CAPABILITIES, Risk, ScanVerdict, Scope, SkillCapability,
    SkillCatalogNarrowing, SkillContentHash, SkillExpansion, SkillName, SkillReference,
    SkillReferences, SkillSource, SkillTier, ToolIdentity,
};
use crate::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolSource, ToolVersion,
};

/// A tool identity for `capability`.
fn identity(capability: &str) -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::parse(capability).expect("valid capability"),
        source: ToolSource::new(
            SourceKind::Native,
            "test.publisher",
            ToolVersion::parse("1.0.0").expect("valid version"),
        )
        .expect("valid source"),
        schema_fingerprint: SchemaFingerprint::from_bytes([0x11; 32]),
    }
}

/// A capability with every dimension supplied.
///
/// Takes the eight dimensions explicitly rather than providing them as methods, because the narrowing
/// tests need to vary exactly one at a time and a builder with defaults would make "which field did this
/// test change" a question about the fixture.
#[allow(clippy::too_many_arguments)]
fn capability(
    cap: &str,
    scopes: &[&str],
    effects: &[Effect],
    risk: Risk,
    sensitivity: Sensitivity,
    approval: ApprovalHint,
    timeout_ms: u64,
    max_attempts: u32,
) -> SkillCapability {
    SkillCapability {
        identity: identity(cap),
        scopes: scopes
            .iter()
            .map(|scope| Scope::new(scope).expect("valid scope"))
            .collect(),
        effects: effects.iter().copied().collect(),
        risk,
        sensitivity,
        approval,
        timeout_ms,
        max_attempts,
    }
}

/// A read-only capability granted with every ceiling at its most restrictive useful value.
///
/// The grant side of the fixture. A declared capability equal to this one is accepted; any expansion on
/// any dimension is refused, which is what makes the fixture usable for all eight dimensions.
fn grant(cap: &str) -> SkillCapability {
    capability(
        cap,
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Allow,
        5_000,
        1,
    )
}

/// A declaration that exactly matches the grant — the accepting case.
fn declared(cap: &str) -> SkillCapability {
    grant(cap)
}

/// Narrows `declared_entries` against `granted_entries`, returning the expansions.
fn expansions(
    declared_entries: Vec<SkillCapability>,
    granted: &[SkillCapability],
) -> Vec<SkillExpansion> {
    SkillCatalogNarrowing::new(declared_entries)
        .expect("the fixture declaration is well formed")
        .narrow(granted)
        .expect_err("the declaration must be refused")
}

#[test]
fn a_skill_that_declares_the_whole_authority_is_accepted() {
    // **The accepting case, and it is load-bearing.** Every other test in this file asserts a refusal, and
    // an implementation that refused every declaration would satisfy all of them. This is also the
    // contract's own required complement: *"a skill that declares a capability expansion is refused, and
    // its complement — a skill that declares a subset — is accepted"*.
    let narrowing =
        SkillCatalogNarrowing::new(vec![declared("files.read@1")]).expect("well formed");
    let catalog = narrowing
        .narrow(&[grant("files.read@1")])
        .expect("a declaration that only narrows must be accepted");
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].identity.capability.to_string(), "files.read@1");
}

#[test]
fn a_strict_subset_of_the_grant_is_accepted() {
    // The other half of the complement. A skill that asks for **less** than it holds is the ordinary,
    // useful case — the same direction the plugin grant's "narrower is ordinary" rule records — so an
    // implementation that demanded equality would refuse the thing the invariant exists to permit.
    let smaller = capability(
        "files.read@1",
        &[],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Ask, // stricter than the granted `Allow`, which is a narrowing
        1_000,             // shorter than the granted bound
        1,
    );
    let narrowing = SkillCatalogNarrowing::new(vec![smaller]).expect("well formed");
    assert!(
        narrowing.narrow(&[grant("files.read@1")]).is_ok(),
        "requesting less than the grant must be accepted",
    );
}

#[test]
fn a_skill_that_adds_a_tool_is_refused() {
    // **The direction the whole module exists for.** A declaration naming a tool the principal holds no
    // grant for is the "a procedure that grants itself a capability" failure the contract calls the single
    // most dangerous thing a skill ecosystem can carry. It is reported as **one** expansion and not eight,
    // because the other seven dimensions are comparisons against a grant that does not exist.
    let held = expansions(vec![declared("email.send@1")], &[grant("files.read@1")]);
    assert_eq!(
        held.len(),
        1,
        "an ungranted tool is one expansion, not eight comparisons against nothing: {held:?}",
    );
    assert_eq!(held[0].code(), "skill.ungranted_tool");
    assert_eq!(held[0].capability(), "email.send@1");
    // And the complement, so the refusal is attributable to the *identity* rather than to a declaration
    // that happens to be refused for another reason: the same grant holds when the identity matches.
    assert!(
        SkillCatalogNarrowing::new(vec![declared("files.read@1")])
            .expect("well formed")
            .narrow(&[grant("files.read@1")])
            .is_ok(),
    );
}

#[test]
fn an_added_scope_is_refused() {
    // A subset test on scopes, asserted by adding one. The reversal is what matters: a superset test on
    // the wrong operand would accept this and refuse the ordinary subset case below.
    let wider = capability(
        "files.read@1",
        &["files.read", "files.write"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Allow,
        5_000,
        1,
    );
    let found = expansions(vec![wider], &[grant("files.read@1")]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code(), "skill.added_scope");
    // The variant check as a `matches!` guard rather than a `match` whose second arm would have to
    // panic: this workspace denies `panic!` outside production and `clippy` refuses an unconditional
    // `assert!(false)`, so the assertion is stated as the positive condition it actually is.
    assert!(
        matches!(&found[0], SkillExpansion::AddedScope { scope, .. } if scope == "files.write"),
        "expected an added-scope expansion naming files.write, got {:?}",
        found[0],
    );
}

#[test]
fn an_added_effect_is_refused() {
    // The effect dimension. `Effect::Write` is one the grant does not permit, so the declaration is asking
    // for authority the grant does not confer — the same subset rule, on the classification that reaches
    // the policy evaluator's `failed_constraints`.
    let wider = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly, Effect::Write],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Allow,
        5_000,
        1,
    );
    let found = expansions(vec![wider], &[grant("files.read@1")]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code(), "skill.added_effect");
    assert!(
        matches!(&found[0], SkillExpansion::AddedEffect { effect, .. } if *effect == "write"),
        "expected an added-effect expansion naming write, got {:?}",
        found[0],
    );
}

#[test]
fn a_raised_risk_ceiling_is_refused() {
    // A ceiling comparison in the narrowing direction. `Moderate > Low`, so this is above the grant — and
    // the test is named for the *rule* rather than the mechanism, so a reversed comparison's failure
    // message states what went wrong.
    let wider = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Moderate,
        Sensitivity::Public,
        ApprovalHint::Allow,
        5_000,
        1,
    );
    let found = expansions(vec![wider], &[grant("files.read@1")]);
    assert_eq!(found[0].code(), "skill.raised_risk", "{found:?}");
    // And the equality case is accepted, so the comparison is `>` rather than `>=`: a declaration at
    // exactly the granted ceiling is the ordinary case, not an expansion.
    assert!(
        SkillCatalogNarrowing::new(vec![declared("files.read@1")])
            .expect("well formed")
            .narrow(&[grant("files.read@1")])
            .is_ok(),
    );
}

#[test]
fn a_raised_sensitivity_ceiling_is_refused() {
    let wider = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Internal,
        ApprovalHint::Allow,
        5_000,
        1,
    );
    let found = expansions(vec![wider], &[grant("files.read@1")]);
    assert_eq!(found[0].code(), "skill.raised_sensitivity", "{found:?}");
}

#[test]
fn a_relaxed_approval_is_refused() {
    // **The dimension with the reversal that would be quietest.** The domain orders `Allow < Ask < Deny`,
    // so a declared posture *below* the granted one is a demotion — the contract's
    // `approval_required -> automatic`. A comparison written the other way accepts exactly this case,
    // which is why the test asserts the code rather than only that something was refused.
    let granted = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Deny,
        5_000,
        1,
    );
    let relaxed = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Allow,
        5_000,
        1,
    );
    let found = expansions(vec![relaxed], &[granted]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code(), "skill.relaxed_approval");

    // **And the complement in the same test**, because this predicate has two directions and only one of
    // them is the defect. A skill that declares a *stricter* approval than it holds must be accepted: that
    // is a narrowing, and it is a thing an author legitimately does.
    let stricter = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Deny,
        5_000,
        1,
    );
    assert!(
        SkillCatalogNarrowing::new(vec![stricter])
            .expect("well formed")
            .narrow(&[capability(
                "files.read@1",
                &["files.read"],
                &[Effect::ReadOnly],
                Risk::Low,
                Sensitivity::Public,
                ApprovalHint::Allow,
                5_000,
                1,
            )])
            .is_ok(),
        "a stricter approval posture must be accepted",
    );
}

#[test]
fn an_extended_timeout_is_refused() {
    let wider = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Allow,
        60_000,
        1,
    );
    let found = expansions(vec![wider], &[grant("files.read@1")]);
    assert_eq!(found[0].code(), "skill.extended_timeout", "{found:?}");
}

#[test]
fn a_raised_attempt_count_is_refused() {
    let wider = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Allow,
        5_000,
        5,
    );
    let found = expansions(vec![wider], &[grant("files.read@1")]);
    assert_eq!(found[0].code(), "skill.raised_attempts", "{found:?}");
}

#[test]
fn every_dimension_is_reported_rather_than_only_the_first() {
    // All eight at once, asserted as the **set of codes** rather than as a count. An implementation that
    // returned on the first expansion would report one; one that reported the wrong dimension would report
    // a set that differs. Comparing the codes is what makes the assertion say which dimensions were found.
    let everything = capability(
        "files.read@1",
        &["files.read", "files.write"],
        &[Effect::ReadOnly, Effect::Write],
        Risk::Critical,
        Sensitivity::Confidential,
        ApprovalHint::Deny,
        60_000,
        9,
    );
    // The grant here is the *most permissive* the fixture can express, so nothing is refused for the
    // trivial reason: the declared approval is stricter than the granted one, which is a narrowing, and it
    // is the only dimension that does not expand.
    let permissive = capability(
        "files.read@1",
        &["files.read"],
        &[Effect::ReadOnly],
        Risk::Low,
        Sensitivity::Public,
        ApprovalHint::Allow,
        5_000,
        1,
    );
    let mut codes: Vec<&str> = expansions(vec![everything], &[permissive])
        .iter()
        .map(SkillExpansion::code)
        .collect();
    codes.sort_unstable();
    assert_eq!(
        codes,
        vec![
            "skill.added_effect",
            "skill.added_scope",
            "skill.extended_timeout",
            "skill.raised_attempts",
            "skill.raised_risk",
            "skill.raised_sensitivity",
        ],
        "every widening dimension must be reported, and `relaxed_approval` must not appear for a \
         stricter posture",
    );
}

#[test]
fn a_duplicate_declared_capability_is_refused_at_construction() {
    // Two entries for one identity, with different bounds, is a declaration whose meaning depends on which
    // entry a checker reads first. Refusing it at construction is what makes the narrowing check total: it
    // never has to answer "which of these two did you mean".
    let error =
        SkillCatalogNarrowing::new(vec![declared("files.read@1"), declared("files.read@1")])
            .expect_err("a duplicate identity must be refused");
    assert_eq!(error.code(), "tool.definition_invalid");
    assert!(
        matches!(
            error,
            DomainError::ToolDefinitionInvalid {
                field: "capability"
            }
        ),
        "the refusal must name the capability field, got {error:?}",
    );
}

#[test]
fn a_declaration_over_the_capability_bound_is_refused() {
    let many: Vec<SkillCapability> = (0..=MAX_SKILL_CAPABILITIES)
        .map(|index| declared(&format!("tools.t{index}@1")))
        .collect();
    assert_eq!(many.len(), MAX_SKILL_CAPABILITIES + 1);
    let error = SkillCatalogNarrowing::new(many).expect_err("an oversized declaration is refused");
    assert_eq!(error.code(), "tool.definition_invalid");
    assert!(
        matches!(
            error,
            DomainError::ToolDefinitionInvalid {
                field: "capabilities"
            }
        ),
        "the refusal must name the capabilities field, got {error:?}",
    );
}

#[test]
fn a_dangerous_verdict_is_never_overridable_at_any_tier() {
    // **The contract's absolute, asserted as an absolute.** The property is not "a caller rarely overrides
    // it" — it is that the parameter is not consulted at all for this verdict. So the assertion iterates
    // **every tier** and both states of the flag, which is what makes it a statement about the rule rather
    // than about one tier.
    for tier in SkillTier::ALL {
        for explicit in [false, true] {
            assert!(
                !tier.may_override(ScanVerdict::Dangerous, explicit),
                "{tier:?} must not override a dangerous verdict (explicit={explicit})",
            );
        }
    }
}

#[test]
fn a_caution_verdict_is_overridable_only_by_an_explicit_recorded_override_at_community() {
    // The override exists for caution-level findings a user has *read*, so it needs both an explicit
    // override and a tier where a user is the one deciding. Asserted as a full matrix rather than as one
    // case, so a rule that flipped a cell fails on the cell.
    for tier in SkillTier::ALL {
        let expected = matches!(tier, SkillTier::Community);
        assert_eq!(
            tier.may_override(ScanVerdict::Caution, true),
            expected,
            "{tier:?} with an explicit override",
        );
        assert!(
            !tier.may_override(ScanVerdict::Caution, false),
            "{tier:?} must not override a caution finding without an explicit override",
        );
    }
    // A clean verdict is not an override at all — there is nothing to override — so every tier passes.
    for tier in SkillTier::ALL {
        assert!(tier.may_override(ScanVerdict::Clean, false), "{tier:?}");
    }
}

#[test]
fn the_scan_gate_and_learning_grant_follow_the_contract_table() {
    // The tier table, asserted for every tier rather than for the ones a test happened to need: a tier
    // added without a decision fails here rather than defaulting to ungated.
    assert!(!SkillTier::Builtin.is_scan_gated());
    assert!(!SkillTier::Official.is_scan_gated());
    assert!(SkillTier::Trusted.is_scan_gated());
    assert!(SkillTier::Community.is_scan_gated());
    assert!(SkillTier::Learned.is_scan_gated());

    assert!(!SkillTier::Builtin.requires_learning_grant());
    assert!(!SkillTier::Official.requires_learning_grant());
    assert!(!SkillTier::Trusted.requires_learning_grant());
    assert!(!SkillTier::Community.requires_learning_grant());
    assert!(SkillTier::Learned.requires_learning_grant());
}

#[test]
fn a_skill_name_is_a_namespaced_slug_and_a_wire_name_goes_through_the_constructor() {
    // Valid names.
    for valid in ["example.research", "a.b", "team.tool_v2", "x.y-z_1"] {
        assert!(SkillName::new(valid).is_ok(), "{valid} must be valid");
    }
    // Invalid: no namespace, empty parts, uppercase, leading/trailing dot, over-long.
    for invalid in [
        "no_namespace",
        "",
        ".leading",
        "trailing.",
        "a..b",
        "Upper.Case",
        "a.b c",
    ] {
        let error = SkillName::new(invalid).expect_err("{invalid} must be refused");
        assert_eq!(error.code(), "jarvis.invalid_identifier", "{invalid}");
        assert!(
            matches!(error, DomainError::InvalidIdentifier { kind: "skill_name" }),
            "{invalid} must name the skill_name field, got {error:?}",
        );
    }
    assert!(SkillName::new(&format!("a.{}", "b".repeat(200))).is_err());

    // **The deserialization path goes through the constructor.** A derived `Deserialize` would wrap
    // whatever string arrived and produce a name the constructor would have refused — the direction an
    // attacker chooses, which is why these types hand-write it. The serde error carries the domain error's
    // *Display* text rather than its code, so the assertion here is that deserialization is refused at all;
    // the loop above is what pins the exact `kind`.
    let refused = serde_json::from_str::<SkillName>("\"not a slug\"")
        .expect_err("a wire name must be validated");
    assert!(
        refused.to_string().contains("canonical JARVIS identifier"),
        "the refusal must come from the identifier rule, got {refused}",
    );
    let accepted: SkillName = serde_json::from_str("\"example.research\"")
        .expect("a valid name deserializes through the constructor");
    assert_eq!(accepted.as_str(), "example.research");
}

#[test]
fn the_content_hash_covers_the_references_and_is_order_independent() {
    // **The contract's "covers every file the skill can load", and the order-independence that makes it a
    // function of content rather than of a caller's listing order.** A hash that covered only the body
    // would make a changed reference file the *same* skill, which is the property ADR-0012's decision 5
    // depends on being false.
    let mut first = SkillReferences::new();
    first.insert(SkillReference::new("a.md", "alpha").expect("valid"));
    first.insert(SkillReference::new("b.md", "beta").expect("valid"));

    // The same two files inserted in the other order.
    let mut second = SkillReferences::new();
    second.insert(SkillReference::new("b.md", "beta").expect("valid"));
    second.insert(SkillReference::new("a.md", "alpha").expect("valid"));

    assert_eq!(
        first.iter().map(SkillReference::name).collect::<Vec<_>>(),
        vec!["a.md", "b.md"],
        "iteration must be canonical, so a hasher over it is order-independent",
    );
    assert_eq!(
        first, second,
        "the sets must be equal regardless of insertion order"
    );

    // A changed reference is a different skill: the set differs, so a hash over it differs.
    let mut changed = SkillReferences::new();
    changed.insert(SkillReference::new("a.md", "alpha").expect("valid"));
    changed.insert(SkillReference::new("b.md", "CHANGED").expect("valid"));
    assert_ne!(first, changed);
}

#[test]
fn a_reference_name_is_bounded_and_refuses_an_empty_or_control_bearing_one() {
    assert!(SkillReference::new("setup.md", "text").is_ok());
    for invalid in ["", "   ", "with\0null"] {
        assert!(
            SkillReference::new(invalid, "text").is_err(),
            "{invalid:?} must be refused",
        );
    }
    assert!(SkillReference::new(&"n".repeat(300), "text").is_err());
}

#[test]
fn the_content_hash_carries_the_domains_canonical_digest_form() {
    // The type reuses `SchemaFingerprint` rather than adding a fourth `sha256:<hex>` carrier, so its
    // spelling must be exactly that form — otherwise a stored hash would be refused by a validator that
    // had every right to refuse it.
    let hash = SkillContentHash::from_bytes([0xab; 32]);
    assert_eq!(
        hash.to_string(),
        format!("sha256:{}", "ab".repeat(32)),
        "the content hash must render in the domain's canonical form",
    );
    let parsed = SkillContentHash::parse(&hash.to_string()).expect("round trips");
    assert_eq!(parsed, hash);
    // And an uppercase or unprefixed spelling is refused, as the domain type refuses it.
    assert!(SkillContentHash::parse(&hash.to_string().to_uppercase()).is_err());
    assert!(SkillContentHash::parse("deadbeef").is_err());
}

#[test]
fn the_sources_and_verdicts_refuse_an_unknown_value_rather_than_defaulting() {
    // Every parser here must fail closed. A source defaulting to `Authored` would understate provenance;
    // a verdict defaulting to `Clean` would let an unscannable skill through — the same direction
    // `Risk::parse` records as "an unrecognised risk must not become `Low`".
    assert_eq!(
        SkillSource::parse("learned").expect("valid"),
        SkillSource::Learned
    );
    assert!(SkillSource::parse("guessed").is_err());

    assert_eq!(
        ScanVerdict::parse("dangerous").expect("valid"),
        ScanVerdict::Dangerous
    );
    assert!(ScanVerdict::parse("probably-fine").is_err());

    // And every spelling round-trips, so the wire vocabulary has one definition rather than two.
    for source in [
        SkillSource::Authored,
        SkillSource::Bundled,
        SkillSource::Installed,
        SkillSource::Learned,
    ] {
        assert_eq!(
            SkillSource::parse(source.as_contract_str()).expect("round trips"),
            source,
        );
    }
    for verdict in [
        ScanVerdict::Clean,
        ScanVerdict::Caution,
        ScanVerdict::Dangerous,
    ] {
        assert_eq!(
            ScanVerdict::parse(verdict.as_contract_str()).expect("round trips"),
            verdict,
        );
    }
}

#[test]
fn every_expansion_code_is_namespaced_and_distinct() {
    // A caller branching on a code must be able to tell one dimension from another, and a code must not
    // collide with the `jarvis.*` namespace the domain's own errors use.
    let sample = |expansion: SkillExpansion| expansion.code();
    let codes = vec![
        sample(SkillExpansion::UngrantedTool {
            capability: "a".to_owned(),
        }),
        sample(SkillExpansion::AddedScope {
            capability: "a".to_owned(),
            scope: "s".to_owned(),
        }),
        sample(SkillExpansion::AddedEffect {
            capability: "a".to_owned(),
            effect: "e",
        }),
        sample(SkillExpansion::RaisedRisk {
            capability: "a".to_owned(),
        }),
        sample(SkillExpansion::RaisedSensitivity {
            capability: "a".to_owned(),
        }),
        sample(SkillExpansion::RelaxedApproval {
            capability: "a".to_owned(),
        }),
        sample(SkillExpansion::ExtendedTimeout {
            capability: "a".to_owned(),
        }),
        sample(SkillExpansion::RaisedAttempts {
            capability: "a".to_owned(),
        }),
    ];
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "codes must be distinct: {codes:?}"
    );
    for code in codes {
        assert!(code.starts_with("skill."), "{code} must be namespaced");
    }
}

/// A `BTreeSet` needs the `Effect` and `Scope` types to be ordered — a compile-time assertion that the
/// fixture's collections are the same ones the narrowing comparison iterates, so a change to their bounds
/// surfaces here rather than in a test body.
#[allow(dead_code)]
fn _collections_are_ordered() {
    let _: BTreeSet<Effect> = BTreeSet::new();
    let _: BTreeSet<Scope> = BTreeSet::new();
}
