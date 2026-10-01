//! The plugin process lifecycle: the state machine from `docs/contracts/plugin-manifest.md`.
//!
//! The contract lists nine lifecycle states and the operations that move between them, and it states
//! two rules that are load-bearing rather than descriptive:
//!
//! > Quarantine survives daemon restart and never silently re-enables on package update.
//!
//! and, of the install operation, "install without enabling". Both are properties of *which edges are
//! legal*, so they belong to a state machine rather than to the supervisor that would perform them —
//! the same reasoning that put `ApprovalState` and `RunState` in this crate. `TLS-015` enforces the
//! transitions; this module is the table it must consult rather than re-derive.
//!
//! ## Why the table is data, not a guard
//!
//! [`PluginState::can_transition_to`] is one `matches!` table, exactly as `ApprovalState`'s is, and for
//! the reason that round recorded: a terminal-state guard written as an early `return false` above a
//! list of edges was a mutation that **compiled and passed every test**, because no arm had a terminal
//! source. Absorbing is therefore a property of the table — a terminal state simply has no arm — rather
//! than a branch that can be silently deleted.
//!
//! ## The two rules the table encodes
//!
//! - **Install never enables.** `Verified -> Enabled` is not an edge; the only way into `Enabled` is
//!   through `InstalledDisabled` (first enable) or `Disabled`/`Unhealthy` (a re-enable). A verified
//!   package becomes `InstalledDisabled`, which is the contract's "install without enabling" made
//!   structural — an installer that enabled on install would be a code change, not a table change.
//! - **Quarantine never re-enables directly.** `Quarantined -> Enabled` is deliberately **absent**: the
//!   only path out of quarantine is `Disabled`, from which an operator may re-enable. That is stronger
//!   than "do not re-enable on update" — it means *no* transition re-enables a quarantined plugin, so
//!   the rule cannot be violated by a supervisor that forgets to check the package version.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// Where a plugin process is in its lifecycle.
///
/// The nine states the contract lists, in the order it lists them. `Removed` is the only terminal
/// state: a removed package is not reconsidered, because the contract's removal operation is the end of
/// the record and a re-install is a new source identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// Found, not yet verified. Discovery grants nothing.
    Discovered,
    /// Provenance and signature verified, not installed.
    Verified,
    /// Installed but not enabled, and holding no grant.
    InstalledDisabled,
    /// Running and reachable.
    Enabled,
    /// Running but failing its health check.
    Unhealthy,
    /// Withheld from running because of crashes, protocol violations, an output flood,
    /// signature/provenance failure, or denied-resource attempts.
    ///
    /// Survives a daemon restart, and the only transition out of it is to [`Self::Disabled`].
    Quarantined,
    /// Installed, healthy, and intentionally not running.
    Disabled,
    /// Removal in progress; the process is stopped and credentials are revoked.
    Removing,
    /// Removed. Terminal.
    Removed,
}

impl PluginState {
    /// Every state, so a totality claim can be checked rather than stated.
    ///
    /// A list rather than a bare enum, mirroring `Effect::ALL` and `Risk::ALL`: a caller that needs
    /// "every state" — an exhaustive table test, a closed set for a command line — otherwise writes its
    /// own list, and adding a state then leaves that caller silently incomplete.
    pub const ALL: [Self; 9] = [
        Self::Discovered,
        Self::Verified,
        Self::InstalledDisabled,
        Self::Enabled,
        Self::Unhealthy,
        Self::Quarantined,
        Self::Disabled,
        Self::Removing,
        Self::Removed,
    ];

    /// Returns whether the state is terminal.
    ///
    /// Only `Removed` is: it is the end of the record. Every other state can be left, which the
    /// `has_any_transition` test asserts against this same classification.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Removed)
    }

    /// Returns whether the plugin's process may be running in this state.
    ///
    /// `Enabled` and `Unhealthy` both mean a process exists — an unhealthy plugin is one that is
    /// running badly, which is why it can recover to `Enabled` — while a disabled or quarantined one is
    /// not running. The supervisor asks this rather than matching the pair at each call site, so
    /// "should a child process exist" has one answer.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Enabled | Self::Unhealthy)
    }

    /// Returns whether the plugin is installed (as opposed to discovered or removed).
    ///
    /// Everything from `InstalledDisabled` onward except `Removed` — a removed plugin is no longer
    /// installed, which is the distinction the contract's removal operation turns on.
    #[must_use]
    pub const fn is_installed(self) -> bool {
        matches!(
            self,
            Self::InstalledDisabled
                | Self::Enabled
                | Self::Unhealthy
                | Self::Quarantined
                | Self::Disabled
                | Self::Removing
        )
    }

    /// Returns whether a transition from `self` to `to` is legal.
    ///
    /// The edges are the contract's operations, and the two rules that are easy to get wrong are
    /// **absent** edges rather than guards:
    ///
    /// - `Verified -> Enabled` is not here, so a package cannot be enabled by the install operation;
    /// - `Quarantined -> Enabled` is not here, so nothing re-enables a quarantined plugin directly —
    ///   the only way out is `Disabled`, and an operator re-enables from there.
    ///
    /// A transition to the **same** state is refused, including for a terminal state. It is not a legal
    /// edge, because accepting it would let a second removal of a removed plugin record a transition
    /// that looks like progress while the effect stays single.
    #[must_use]
    pub const fn can_transition_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Discovered, Self::Verified)
                | (Self::Verified, Self::InstalledDisabled)
                | (
                    // `InstalledDisabled` and `Disabled` have the same targets, so they share an arm.
                    // **`Enabled` and `Unhealthy` deliberately do not**: their target sets both include
                    // the *other* state, and merging them would make `Enabled -> Enabled` and
                    // `Unhealthy -> Unhealthy` legal — the self-transition this table refuses.
                    Self::InstalledDisabled | Self::Disabled,
                    Self::Enabled | Self::Removing
                )
                | (
                    Self::Enabled,
                    Self::Unhealthy | Self::Quarantined | Self::Disabled
                )
                | (
                    Self::Unhealthy,
                    Self::Enabled | Self::Quarantined | Self::Disabled
                )
                | (Self::Quarantined, Self::Disabled | Self::Removing)
                | (Self::Removing, Self::Removed)
        )
    }

    /// Returns the legal targets of `self`, for a caller that must enumerate them.
    ///
    /// Written out rather than derived by filtering [`Self::ALL`], for the reason `RunState` records:
    /// a derived list would answer from the same expression it is meant to check, so a test comparing
    /// the two would be comparing the table to itself.
    #[must_use]
    pub const fn allowed_targets(self) -> &'static [Self] {
        match self {
            Self::Discovered => &[Self::Verified],
            Self::Verified => &[Self::InstalledDisabled],
            // The same targets, and merged so the written-out list matches the table's merged arm — but
            // written out per state rather than collapsed, because this list answers "what may this
            // state do" for a reader and the table answers the same question for the compiler.
            Self::InstalledDisabled | Self::Disabled => &[Self::Enabled, Self::Removing],
            Self::Enabled => &[Self::Unhealthy, Self::Quarantined, Self::Disabled],
            Self::Unhealthy => &[Self::Enabled, Self::Quarantined, Self::Disabled],
            Self::Quarantined => &[Self::Disabled, Self::Removing],
            Self::Removing => &[Self::Removed],
            Self::Removed => &[],
        }
    }

    /// Returns whether any target is reachable from `self`.
    ///
    /// The complement of [`Self::is_terminal`], stated separately so the "no legal way out" property
    /// has one consultable definition — a state with no legal target is a plugin that can neither run
    /// nor be removed, which is the shape this project has found repeatedly.
    #[must_use]
    pub const fn has_any_transition(self) -> bool {
        !self.is_terminal()
    }

    /// Returns the spelling the plugin contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Verified => "verified",
            Self::InstalledDisabled => "installed_disabled",
            Self::Enabled => "enabled",
            Self::Unhealthy => "unhealthy",
            Self::Quarantined => "quarantined",
            Self::Disabled => "disabled",
            Self::Removing => "removing",
            Self::Removed => "removed",
        }
    }

    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the `lifecycle_state` field for anything
    /// outside the closed set — the same refusal `Risk::parse` makes for an unknown risk. An
    /// unrecognised lifecycle position must not default to a state, because the contract's own `state`
    /// column has no default for the same reason: a defaulted position is a plausible row nobody chose.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_contract_str() == value)
            .ok_or(DomainError::ToolDefinitionInvalid {
                field: "lifecycle_state",
            })
    }
}

impl fmt::Display for PluginState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
