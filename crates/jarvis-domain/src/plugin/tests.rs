//! Tests for the plugin process lifecycle state machine, the installed source identity, and grants.

use super::{
    PluginCapabilitySelector, PluginGrant, PluginSourceIdentity, PluginState, PluginVersion,
    SourceContinuity, is_canonical_package_digest, is_plugin_identifier,
};
use crate::ids::{PrincipalId, WorkspaceId};
use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;
use crate::tool::classification::Risk;

#[test]
fn every_edge_the_contract_requires_is_present_and_the_two_it_forbids_are_absent() {
    // The edges are the contract's operations. Asserting the set rather than sampling it is what keeps
    // the table and the contract from drifting: a removed edge would silently forbid a legal operation,
    // and an added one would silently permit an operation the contract never named.
    //
    // **The two absent edges are the point of the test, and both are `-> Enabled`.**
    // `Verified -> Enabled` being absent is the contract's "install without enabling": a verified
    // package can only become `InstalledDisabled`. `Quarantined -> Enabled` being absent is stronger
    // than the contract's "never silently re-enables on package update" — *no* transition re-enables a
    // quarantined plugin, so a supervisor that forgot to check a package version cannot violate it; the
    // only way out of quarantine is `Disabled`, from which an operator re-enables.
    let expected: [(PluginState, &[PluginState]); 8] = [
        (PluginState::Discovered, &[PluginState::Verified]),
        (PluginState::Verified, &[PluginState::InstalledDisabled]),
        (
            PluginState::InstalledDisabled,
            &[PluginState::Enabled, PluginState::Removing],
        ),
        (
            PluginState::Enabled,
            &[
                PluginState::Unhealthy,
                PluginState::Quarantined,
                PluginState::Disabled,
            ],
        ),
        (
            PluginState::Unhealthy,
            &[
                PluginState::Enabled,
                PluginState::Quarantined,
                PluginState::Disabled,
            ],
        ),
        (
            PluginState::Quarantined,
            &[PluginState::Disabled, PluginState::Removing],
        ),
        (
            PluginState::Disabled,
            &[PluginState::Enabled, PluginState::Removing],
        ),
        (PluginState::Removing, &[PluginState::Removed]),
    ];
    for (from, targets) in expected {
        for to in PluginState::ALL {
            let legal = targets.contains(&to);
            assert_eq!(
                from.can_transition_to(to),
                legal,
                "{from} -> {to} legality must be {legal}",
            );
        }
    }
    assert!(
        !PluginState::Verified.can_transition_to(PluginState::Enabled),
        "**install must never enable** — a verified package becomes installed-disabled",
    );
    assert!(
        !PluginState::Quarantined.can_transition_to(PluginState::Enabled),
        "**nothing may re-enable a quarantined plugin directly** — the only exit is disabled",
    );
}

#[test]
fn every_non_terminal_state_has_a_legal_target_and_removed_has_none() {
    // The "no legal way out" shape this project has found repeatedly: a state with no target is a plugin
    // that can neither run nor be removed. The assertion is against the table (`can_transition_to`), not
    // against `allowed_targets`, which is written out separately — comparing the two would be comparing
    // the table to itself.
    for state in PluginState::ALL {
        let any_target = PluginState::ALL
            .iter()
            .copied()
            .any(|to| state.can_transition_to(to));
        assert_eq!(
            any_target,
            state.has_any_transition(),
            "{state}'s transition existence must agree with its terminal classification",
        );
        // And the written-out target list must agree with the table, so the two cannot drift.
        assert_eq!(
            state.allowed_targets().len(),
            PluginState::ALL
                .iter()
                .copied()
                .filter(|to| state.can_transition_to(*to))
                .count(),
            "{state}'s allowed_targets must be exactly the legal edges",
        );
    }
    assert!(
        PluginState::Removed.allowed_targets().is_empty(),
        "a removed plugin is terminal",
    );
}

#[test]
fn terminal_states_absorb_and_no_state_transitions_to_itself() {
    for state in PluginState::ALL {
        assert!(
            !state.can_transition_to(state),
            "{state} -> {state} is not an edge: a no-op transition would record progress that did \
             not happen",
        );
    }
    for target in PluginState::ALL {
        assert!(
            !PluginState::Removed.can_transition_to(target),
            "nothing leaves a removed plugin, including to {target}",
        );
    }
    // Only `Removed` is terminal, which the absorbing loop above depends on.
    let terminal: Vec<PluginState> = PluginState::ALL
        .iter()
        .copied()
        .filter(|state| state.is_terminal())
        .collect();
    assert_eq!(terminal, [PluginState::Removed]);
}

#[test]
fn running_and_installed_are_classified_deliberately() {
    // `is_running` answers "should a child process exist", which the supervisor asks rather than
    // matching the pair at each call site. An unhealthy plugin is running badly, so it *is* running.
    for state in PluginState::ALL {
        let expected_running = matches!(state, PluginState::Enabled | PluginState::Unhealthy);
        assert_eq!(state.is_running(), expected_running, "{state} running-ness");
    }
    // A discovered or removed plugin is not installed; everything from installed-disabled to removing
    // is. `Removed` is the distinction the removal operation turns on.
    assert!(!PluginState::Discovered.is_installed());
    assert!(!PluginState::Verified.is_installed());
    assert!(!PluginState::Removed.is_installed());
    for state in [
        PluginState::InstalledDisabled,
        PluginState::Enabled,
        PluginState::Unhealthy,
        PluginState::Quarantined,
        PluginState::Disabled,
        PluginState::Removing,
    ] {
        assert!(state.is_installed(), "{state} is installed");
    }
}

#[test]
fn every_state_parses_from_its_contract_spelling_and_an_unknown_one_is_refused() {
    for state in PluginState::ALL {
        assert_eq!(
            PluginState::parse(state.as_contract_str()).ok(),
            Some(state),
            "every state round-trips through its contract spelling",
        );
    }
    assert_eq!(PluginState::ALL.len(), 9, "the contract lists nine states");
    // The spellings are asserted literally rather than from the constants, so a variant that renamed
    // its own spelling is caught against the contract rather than against itself.
    assert_eq!(
        PluginState::InstalledDisabled.as_contract_str(),
        "installed_disabled"
    );
    assert_eq!(PluginState::Quarantined.as_contract_str(), "quarantined");
    // An unrecognised position is refused rather than defaulted — a defaulted lifecycle position is a
    // plausible row nobody chose, which is why the contract's own column has no default.
    assert!(PluginState::parse("running").is_err());
    assert!(PluginState::parse("").is_err());
    assert!(PluginState::parse("ENABLED").is_err());
}

#[test]
fn quarantine_and_disabled_can_both_be_removed_but_a_running_plugin_cannot_be() {
    // A running plugin must be disabled or quarantined before removal, so the process is stopped before
    // its files are taken away — "remove while the daemon remains healthy" cannot leave a child running
    // against a package that no longer exists. This is a property of the table, not a supervisor check.
    for from in [PluginState::Enabled, PluginState::Unhealthy] {
        assert!(
            !from.can_transition_to(PluginState::Removing),
            "{from} must be disabled or quarantined before it is removed",
        );
        assert!(from.can_transition_to(PluginState::Disabled));
        assert!(from.can_transition_to(PluginState::Quarantined));
    }
    for from in [
        PluginState::Disabled,
        PluginState::Quarantined,
        PluginState::InstalledDisabled,
    ] {
        assert!(
            from.can_transition_to(PluginState::Removing),
            "{from} may be removed",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Installed source identity.
// ---------------------------------------------------------------------------------------

/// A package digest, for the fixtures.
fn digest() -> String {
    format!("sha256:{}", "ab".repeat(32))
}

/// An installed source identity with caller-chosen id, publisher, and version.
fn identity(id: &str, publisher: &str, version: &str) -> PluginSourceIdentity {
    PluginSourceIdentity::new(
        id,
        publisher,
        &digest(),
        "sigstore-bundle:example.org",
        "https://example.org/releases",
        PluginVersion::parse(version).expect("the fixture version parses"),
        "jarvis-runtime",
    )
    .expect("the fixture identity is valid")
}

#[test]
fn an_identity_is_a_tuple_so_a_different_publisher_is_a_different_plugin() {
    // The contract's rule that `id`, `publisher`, and the rest "form the installed source identity" is
    // what makes `SourceContinuity` a comparison of *tuples* rather than of an `id` string. Two packages
    // with the same `id` from different publishers must not be the same identity — that is the
    // impersonation case a grant bound to a name would inherit.
    let ours = identity("example.research-runtime", "example.org", "1.0.0");
    let impostor = identity("example.research-runtime", "attacker.example", "1.0.1");
    assert_ne!(ours, impostor);
    assert_eq!(
        SourceContinuity::classify(&ours, &impostor),
        SourceContinuity::NoContinuity,
        "**a different publisher is never continuity**, even for the same id and a newer version",
    );
}

#[test]
fn continuity_requires_the_same_source_and_a_strictly_newer_version() {
    let installed = identity("example.research-runtime", "example.org", "1.2.3");
    // The ordinary update: same publisher, same id, newer version.
    let newer = identity("example.research-runtime", "example.org", "1.2.4");
    assert!(SourceContinuity::classify(&installed, &newer).is_continuity());
    // A different id is not the same plugin.
    assert!(
        !SourceContinuity::classify(
            &installed,
            &identity("example.other", "example.org", "1.2.4")
        )
        .is_continuity()
    );
    // 🔎 **The same version is a replay, not an update**, and an older one is a downgrade — neither is
    // the update the contract describes, and both would let a re-install claim continuity it does not have.
    assert!(
        !SourceContinuity::classify(
            &installed,
            &identity("example.research-runtime", "example.org", "1.2.3")
        )
        .is_continuity(),
        "the same version must not be continuity",
    );
    assert!(
        !SourceContinuity::classify(
            &installed,
            &identity("example.research-runtime", "example.org", "1.2.2")
        )
        .is_continuity(),
        "an older version must not be continuity",
    );
}

#[test]
fn a_version_compares_numerically_and_a_pre_release_sorts_below_its_release() {
    // **The ordering is what makes "an update is a newer source identity" a real question.** Text
    // comparison would order `1.10.0` before `1.9.0`, so the version is parsed to integers and compared
    // on them.
    let parse = |value: &str| PluginVersion::parse(value).expect("parses");
    assert!(parse("1.10.0") > parse("1.9.0"), "1.10.0 sorts above 1.9.0");
    assert!(parse("2.0.0") > parse("1.99.99"));
    assert!(parse("1.2.10") > parse("1.2.9"));
    // `SemVer`: a pre-release sorts **below** its release, so a release is an update to its own rc.
    assert!(parse("1.0.0") > parse("1.0.0-rc.1"));
    assert!(parse("1.0.0-rc.1") < parse("1.0.0"));
    // And it round-trips through `Display`, so a stored spelling and the parsed value agree.
    assert_eq!(parse("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
    assert_eq!(parse("1.2.3").to_string(), "1.2.3");
}

#[test]
fn a_malformed_version_is_refused_rather_than_normalized() {
    for (value, why) in [
        ("1.2", "two components"),
        ("1.2.3.4", "four components"),
        ("1.2.x", "a non-numeric component"),
        ("01.2.3", "a leading zero"),
        ("1.2.3-", "an empty pre-release"),
        ("v1.2.3", "a leading v"),
        ("1.2.3+build", "build metadata this type does not order"),
        ("", "empty"),
    ] {
        assert!(
            PluginVersion::parse(value).is_err(),
            "{value} must be refused: {why}",
        );
    }
}

#[test]
fn a_bare_digest_is_refused_because_the_algorithm_is_part_of_the_value() {
    // The domain's third carrier of this rule (`ActionDigest`, `SchemaFingerprint`, and now a package
    // digest), and they must agree: a value written by one and refused by another while looking identical
    // is two spellings of one rule.
    assert!(is_canonical_package_digest(&digest()));
    for bad in [
        "ab".repeat(32),
        format!("sha512:{}", "ab".repeat(32)),
        format!("sha256:{}", "AB".repeat(32)),
        format!("sha256:{}", "ab".repeat(31)),
        "sha256:".to_owned(),
    ] {
        assert!(!is_canonical_package_digest(&bad), "{bad} must be refused");
    }
}

#[test]
fn an_identity_field_that_is_not_a_slug_is_refused_at_construction() {
    // The identity is built through `new`, so a value that is not a slug cannot be constructed — the
    // same shape rule `ToolIdentity`'s parts enforce. The display name is *not* checked here (it is not an
    // identity), which is the whole point of separating them.
    assert!(is_plugin_identifier("example.research-runtime"));
    assert!(is_plugin_identifier("example.org"));
    for bad in [
        "Example.Runtime",
        "example",
        "example..runtime",
        "example.",
        "example runtime",
        "example\truntime",
    ] {
        assert!(!is_plugin_identifier(bad), "{bad} must be refused");
    }
    // And the constructor refuses each field by name, so a caller knows which one to correct.
    let err = PluginSourceIdentity::new(
        "Example.Runtime",
        "example.org",
        &digest(),
        "sig",
        "src",
        PluginVersion::parse("1.0.0").expect("parses"),
        "jarvis-runtime",
    )
    .expect_err("an invalid id is refused");
    assert_eq!(err.code(), "tool.definition_invalid");
    // A bad digest is refused too, by its own field — the two failures are distinguishable.
    assert!(
        PluginSourceIdentity::new(
            "example.research-runtime",
            "example.org",
            "ab",
            "sig",
            "src",
            PluginVersion::parse("1.0.0").expect("parses"),
            "jarvis-runtime",
        )
        .is_err(),
        "a bare digest is refused at construction",
    );
    // And empty, control-carrying identity text is refused.
    assert!(
        PluginSourceIdentity::new(
            "example.research-runtime",
            "example.org",
            &digest(),
            "sig\u{0}",
            "src",
            PluginVersion::parse("1.0.0").expect("parses"),
            "jarvis-runtime",
        )
        .is_err(),
        "a control character in the signature identity is refused",
    );
}

// ---------------------------------------------------------------------------------------
// Grants, and the carry-forward rule.
// ---------------------------------------------------------------------------------------

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(1))
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(2))
}

fn at(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse(value).expect("the fixture instant parses")
}

/// A grant over the given identity, with one capability and explicit ceilings.
fn grant(
    identity: PluginSourceIdentity,
    capabilities: &[&str],
    risk_ceiling: Risk,
    sensitivity_ceiling: Sensitivity,
) -> PluginGrant {
    PluginGrant {
        identity,
        workspace: workspace(),
        principal: principal(),
        capabilities: capabilities
            .iter()
            .map(|value| PluginCapabilitySelector::new(value).expect("a valid selector"))
            .collect(),
        risk_ceiling,
        sensitivity_ceiling,
        expires_at: None,
    }
}

#[test]
fn a_grant_applies_only_to_the_exact_source_identity_workspace_and_principal() {
    // The contract binds a grant to the installed source identity, the workspace, and the granting
    // principal, and the identity is a **whole tuple** — so a package whose bytes or publisher changed is
    // a different plugin and the grant does not apply. This is `ACC-024`'s rule at the plugin layer: a
    // replacement behind the same `id` must not inherit the grant.
    let installed = identity("example.research-runtime", "example.org", "1.0.0");
    let the_grant = grant(
        installed.clone(),
        &["runtime.text"],
        Risk::Low,
        Sensitivity::Internal,
    );
    let now = at("2026-01-01T00:00:00Z");
    assert!(the_grant.applies_at(&installed, workspace(), principal(), now));

    // A different workspace, a different principal, and a different publisher each refuse on their own.
    assert!(!the_grant.applies_at(
        &installed,
        WorkspaceId::from_uuid(uuid::Uuid::from_u128(99)),
        principal(),
        now,
    ));
    assert!(!the_grant.applies_at(
        &installed,
        workspace(),
        PrincipalId::from_uuid(uuid::Uuid::from_u128(98)),
        now,
    ));
    // 🔎 **The same id and version, a different publisher** — the impersonation case. A grant keyed on
    // `id` alone would apply here, which is exactly the defect the tuple prevents.
    let impostor = identity("example.research-runtime", "attacker.example", "1.0.0");
    assert!(
        !the_grant.applies_at(&impostor, workspace(), principal(), now),
        "**a grant must not apply to the same id from a different publisher**",
    );
}

#[test]
fn an_expired_grant_does_not_apply_but_is_distinguishable_from_a_missing_one() {
    let installed = identity("example.research-runtime", "example.org", "1.0.0");
    let mut the_grant = grant(
        installed.clone(),
        &["runtime.text"],
        Risk::Low,
        Sensitivity::Internal,
    );
    the_grant.expires_at = Some(at("2026-06-01T00:00:00Z"));
    // Boundary: an expiry at `T` does not permit use at `T` — the same convention the tool `Grant` uses.
    assert!(!the_grant.applies_at(
        &installed,
        workspace(),
        principal(),
        at("2026-06-01T00:00:00Z")
    ));
    assert!(the_grant.applies_at(
        &installed,
        workspace(),
        principal(),
        at("2026-05-31T23:59:59Z")
    ));
    // The operator-facing distinction: "your grant expired" versus "you have no grant".
    assert!(the_grant.names_identity_but_expired(
        &installed,
        workspace(),
        principal(),
        at("2026-06-01T00:00:00Z"),
    ));
}

#[test]
fn carry_forward_requires_continuity_and_no_expansion() {
    // The contract's rule, at one call site: "Existing grants carry forward only under an explicit policy
    // that proves publisher continuity and no capability/schema/effect expansion."
    let installed = identity("example.research-runtime", "example.org", "1.0.0");
    let earlier = grant(
        installed.clone(),
        &["runtime.text"],
        Risk::Low,
        Sensitivity::Internal,
    );

    // An update that grants exactly the same thing carries forward.
    let same = grant(
        identity("example.research-runtime", "example.org", "1.1.0"),
        &["runtime.text"],
        Risk::Low,
        Sensitivity::Internal,
    );
    assert!(
        earlier.carry_forward_to(&same),
        "an identical newer version carries forward"
    );

    // 🔎 **An update that ADDS a capability does not.** This is the dangerous direction — a grant that
    // survived would authorize a capability the operator never approved.
    let expanded_capability = grant(
        identity("example.research-runtime", "example.org", "1.1.0"),
        &["runtime.text", "artifacts.write"],
        Risk::Low,
        Sensitivity::Internal,
    );
    assert!(
        !earlier.carry_forward_to(&expanded_capability),
        "**a grant must not carry forward across a capability expansion**",
    );

    // A raised risk ceiling is an effect expansion.
    let raised_risk = grant(
        identity("example.research-runtime", "example.org", "1.1.0"),
        &["runtime.text"],
        Risk::High,
        Sensitivity::Internal,
    );
    assert!(
        !earlier.carry_forward_to(&raised_risk),
        "**a raised risk ceiling is an effect expansion**",
    );

    // A widened sensitivity ceiling is a data expansion.
    let widened = grant(
        identity("example.research-runtime", "example.org", "1.1.0"),
        &["runtime.text"],
        Risk::Low,
        Sensitivity::Restricted,
    );
    assert!(
        !earlier.carry_forward_to(&widened),
        "**a widened sensitivity ceiling is an expansion**",
    );

    // And a new publisher is not continuity, so nothing carries forward even with no expansion.
    let impostor = grant(
        identity("example.research-runtime", "attacker.example", "1.1.0"),
        &["runtime.text"],
        Risk::Low,
        Sensitivity::Internal,
    );
    assert!(
        !earlier.carry_forward_to(&impostor),
        "a different publisher is not continuity",
    );
}

#[test]
fn a_narrowing_update_carries_forward_because_it_grants_nothing_more() {
    // The complement, so the expansion test is not vacuous: an update that grants *less* is still a
    // superset in the safe direction and carries forward. Without this half, an implementation that
    // refused every carry-forward would satisfy the expansion assertions.
    let installed = identity("example.research-runtime", "example.org", "1.0.0");
    let broad = grant(
        installed,
        &["runtime.text", "artifacts.write"],
        Risk::High,
        Sensitivity::Confidential,
    );
    let narrowed = grant(
        identity("example.research-runtime", "example.org", "1.1.0"),
        &["runtime.text"],
        Risk::Low,
        Sensitivity::Internal,
    );
    assert!(
        broad.carry_forward_to(&narrowed),
        "a narrowing update grants nothing the old grant did not",
    );
    // And a grant does not migrate between workspaces or principals, however narrow it is.
    let mut other_workspace = narrowed.clone();
    other_workspace.workspace = WorkspaceId::from_uuid(uuid::Uuid::from_u128(42));
    assert!(
        !broad.carry_forward_to(&other_workspace),
        "a grant does not migrate between workspaces",
    );
}

#[test]
fn a_capability_selector_that_is_not_a_namespaced_name_is_refused() {
    for good in [
        "runtime.text",
        "jarvis.tools.read:selected",
        "network:api.example.org",
    ] {
        assert!(
            PluginCapabilitySelector::new(good).is_ok(),
            "{good} is a valid selector",
        );
    }
    for bad in ["", "Runtime.Text", "runtime text", "runtime\ntext"] {
        assert!(
            PluginCapabilitySelector::new(bad).is_err(),
            "{bad} must be refused",
        );
    }
    // **The deserializer goes through `new`**, so a selector that arrived over the wire is held to the
    // same rule as one this code built — the `Scope`/`WorkspaceRelativePath` fix, required because a
    // selector reaches a persisted grant.
    assert!(serde_json::from_str::<PluginCapabilitySelector>(r#""runtime.text""#).is_ok(),);
    assert!(
        serde_json::from_str::<PluginCapabilitySelector>(r#""Runtime Text""#).is_err(),
        "a selector the constructor refuses must be refused on the way in",
    );
}
