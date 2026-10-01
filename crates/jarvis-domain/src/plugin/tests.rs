//! Tests for the plugin process lifecycle state machine.

use super::PluginState;

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
