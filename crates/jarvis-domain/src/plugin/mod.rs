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
use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;

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

/// The longest accepted plugin identifier, publisher, or version segment.
pub const MAX_PLUGIN_IDENTIFIER_BYTES: usize = 128;

/// Returns whether `value` is a usable plugin identifier or publisher.
///
/// **The contract's "Stable Identity and Provenance" rule made a rule.** It says `id`, `publisher`,
/// package digest, signature identity, source, version, and protocol form the installed source
/// identity, and that "display name or executable filename never identifies a plugin". So this is the
/// shape `id` and `publisher` must have: a lowercase dotted slug — `example.research-runtime`,
/// `example.org` — because a dotted vendor/product name is what a publisher publishes under, and a
/// value carrying capitals, control characters, or an unbounded length would be an identity two
/// spellings could denote.
///
/// **One definition, used by both the domain and the manifest validation.** A plugin's identity is a
/// property of the *plugin*, so the rule lives here; `jarvis_infrastructure::plugin` parses a manifest
/// and consults this rule rather than holding its own copy. A second copy is how two layers come to
/// disagree about which identifiers are valid — the "two spellings of one rule" defect this project
/// keeps finding.
///
/// The parts between dots must be non-empty (so `a..b`, `.a`, and `a.` are each a second spelling of
/// something that would otherwise parse), a hyphen is allowed because vendors publish under them, and a
/// leading dot is refused for the same reason.
#[must_use]
pub fn is_plugin_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_PLUGIN_IDENTIFIER_BYTES {
        return false;
    }
    let Some(first) = value.chars().next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let usable = |character: char| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '.'
            || character == '-'
    };
    if !value.chars().all(usable) {
        return false;
    }
    // A dot is required — a plugin names its owner and product, and a bare `runtime` could name any
    // vendor's — and every dotted part must be non-empty.
    value.contains('.') && value.split('.').all(|part| !part.is_empty())
}

/// An update's relationship to an installed plugin's source identity.
///
/// The contract: "An update is a new source identity. Existing grants carry forward only under an
/// explicit policy that proves publisher continuity and no capability/schema/effect expansion;
/// otherwise they require review." This describes the *source* half of that question, and
/// [`PluginGrant::carry_forward_to`] answers the *expansion* half; what belongs here is the arithmetic
/// a policy has no business re-deriving.
///
/// **Continuity is only in the safe direction.** The same publisher and the same identifier with a
/// **newer version** is `Continuity`; a version that is *older*, or equal, is not — an "update" to a
/// version already installed is a replay, and one to an older version is a downgrade, and neither is an
/// update the contract describes. Two packages that differ only in their publisher are `NoContinuity`
/// even if the identifier matches, which is the impersonation case the identity is a tuple to prevent.
///
/// **Continuity does not imply a grant carries forward, and the two are deliberately separate types.**
/// This answers "is this the same plugin, newer?"; whether a grant survives additionally needs "does the
/// new version grant nothing more than the old one?" — see [`PluginGrant::carry_forward_to`]. A caller
/// that collapsed the two would carry a grant across an update that *expanded* its capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceContinuity {
    /// Same publisher, same identifier, a strictly newer version: the shape an update has.
    Continuity,
    /// Not the same installed source: a different publisher or identifier, or not a newer version.
    NoContinuity,
}

impl SourceContinuity {
    /// Classifies `candidate` against `installed`.
    ///
    /// **A pure function so the classification is checkable without a grant store.** `TLS-015` decides
    /// whether to carry grants forward; this says only whether the two source identities stand in the
    /// update relationship at all, so the dangerous half — that a grant survived — is decided by a
    /// policy that can refuse even when continuity holds.
    #[must_use]
    pub fn classify(installed: &PluginSourceIdentity, candidate: &PluginSourceIdentity) -> Self {
        // Same publisher and same identifier, and the candidate is strictly newer. A different
        // publisher is a different identity even for the same identifier — the case impersonation would
        // use — and a version that is not newer is a replay or a downgrade, not an update.
        if installed.publisher == candidate.publisher
            && installed.id == candidate.id
            && installed.version < candidate.version
        {
            Self::Continuity
        } else {
            Self::NoContinuity
        }
    }

    /// Returns whether this is the update relationship.
    #[must_use]
    pub const fn is_continuity(self) -> bool {
        matches!(self, Self::Continuity)
    }

    /// Returns the spelling the plugin contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Continuity => "continuity",
            Self::NoContinuity => "no_continuity",
        }
    }
}

/// A plugin version, `major.minor.patch` with an optional pre-release tag.
///
/// **Ordered, because an update is defined by the ordering.** The contract says "an update is a new
/// source identity", and a newer version is what distinguishes one from a replay; a version that could
/// only be compared as text would order `1.10.0` before `1.9.0`. So the type parses to three integers
/// and compares on them, and a pre-release sorts **before** its release (`1.0.0-rc.1 < 1.0.0`), which is
/// what `SemVer` requires and what a "newer?" question depends on.
///
/// The identifier is bounded and control-free like everything else that reaches a store and a log; the
/// comparison is numeric on the three components, and the pre-release is compared by the presence of one
/// (a release sorts above any of its pre-releases) rather than by a full pre-release ordering, which no
/// caller here needs and which would be untested complexity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginVersion {
    major: u32,
    minor: u32,
    patch: u32,
    pre_release: Option<String>,
}

impl PluginVersion {
    /// Parses `major.minor.patch` with an optional `-pre.release` suffix.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the `version` field for anything that is
    /// not the canonical form. A leading zero, a missing component, or a non-numeric component is
    /// refused rather than normalized — matching the tool identity's version rule, because two spellings
    /// of one version would let a stored identity miss a match that is really the same release.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let invalid = || DomainError::ToolDefinitionInvalid { field: "version" };
        let (release, pre_release) = match value.split_once('-') {
            Some((release, pre)) => {
                if pre.is_empty() || pre.len() > MAX_PLUGIN_IDENTIFIER_BYTES {
                    return Err(invalid());
                }
                // A pre-release is compared by its *presence* here, but it is still validated so it
                // cannot carry a control character into a store or a log.
                let usable = |character: char| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
                };
                if !pre.chars().all(usable) {
                    return Err(invalid());
                }
                (release, Some(pre.to_owned()))
            }
            None => (value, None),
        };
        let mut components = release.split('.');
        let major = parse_component(components.next()).ok_or_else(invalid)?;
        let minor = parse_component(components.next()).ok_or_else(invalid)?;
        let patch = parse_component(components.next()).ok_or_else(invalid)?;
        // Exactly three components: a fourth would be a version this type does not order.
        if components.next().is_some() {
            return Err(invalid());
        }
        Ok(Self {
            major,
            minor,
            patch,
            pre_release,
        })
    }

    /// The `major.minor.patch` string, without any pre-release.
    #[must_use]
    pub fn release_text(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parses one version component, refusing a sign, a leading zero, and non-digits.
fn parse_component(value: Option<&str>) -> Option<u32> {
    let value = value?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // A leading zero is refused rather than normalized: `01` and `1` naming one version is the two
    // spellings defect the canonical form exists to prevent.
    if value.len() > 1 && value.starts_with('0') {
        return None;
    }
    value.parse::<u32>().ok()
}

impl PartialOrd for PluginVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PluginVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre_release, &other.pre_release) {
                // `SemVer`: a pre-release sorts **below** its release, so `1.0.0-rc.1 < 1.0.0`. The
                // pre-release strings themselves are not ordered against each other — no caller here
                // asks, and a half-ordered pre-release would be an untested claim about which.
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            })
    }
}

impl fmt::Display for PluginVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.release_text())?;
        if let Some(pre) = &self.pre_release {
            write!(formatter, "-{pre}")?;
        }
        Ok(())
    }
}

/// The installed source identity of a plugin: the tuple an approval binds to and an update is measured
/// against.
///
/// The contract: "`id`, `publisher`, package digest, signature identity, source, version, and protocol
/// form the installed source identity." Those seven facts are here, and the point of a **tuple** rather
/// than an `id` string is the same one `ToolIdentity` records: a name can be re-pointed, so binding a
/// grant to `id` alone would let a different package — new publisher, new bytes, new signature —
/// inherit every grant the original had.
///
/// **The display name is deliberately absent**, because the contract says it never identifies a plugin;
/// and the version is typed and ordered, because `SourceContinuity` asks whether one source is *newer*
/// than another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSourceIdentity {
    /// The stable plugin identifier, a dotted slug.
    pub id: String,
    /// The publisher, a dotted slug.
    pub publisher: String,
    /// The package bytes' digest, `sha256:<64 lowercase hex>`.
    pub package_digest: String,
    /// The signature identity, an opaque bounded string.
    ///
    /// Opaque rather than validated against a signature *kind*, because signature kinds are `TLS-014`'s
    /// vocabulary; what is checked here is that it is bounded and control-free, like every other value
    /// that reaches a store and a log.
    pub signature_identity: String,
    /// Where the package came from.
    pub source: String,
    /// The package's version.
    pub version: PluginVersion,
    /// The process protocol kind, one of the contract's supported kinds.
    pub protocol: String,
}

impl PluginSourceIdentity {
    /// Builds and validates an installed source identity.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the first field that fails. The identifier
    /// and publisher are held to [`is_plugin_identifier`], the digest to the contract's `sha256:` form,
    /// and the source and signature identity to a bounded, control-free text rule.
    pub fn new(
        id: &str,
        publisher: &str,
        package_digest: &str,
        signature_identity: &str,
        source: &str,
        version: PluginVersion,
        protocol: &str,
    ) -> Result<Self, DomainError> {
        if !is_plugin_identifier(id) {
            return Err(DomainError::ToolDefinitionInvalid { field: "id" });
        }
        if !is_plugin_identifier(publisher) {
            return Err(DomainError::ToolDefinitionInvalid { field: "publisher" });
        }
        if !is_canonical_package_digest(package_digest) {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "package_digest",
            });
        }
        for (field, value) in [
            ("signature_identity", signature_identity),
            ("source", source),
            ("protocol", protocol),
        ] {
            if !is_bounded_identity_text(value) {
                return Err(DomainError::ToolDefinitionInvalid { field });
            }
        }
        Ok(Self {
            id: id.to_owned(),
            publisher: publisher.to_owned(),
            package_digest: package_digest.to_owned(),
            signature_identity: signature_identity.to_owned(),
            source: source.to_owned(),
            version,
            protocol: protocol.to_owned(),
        })
    }
}

/// Returns whether `value` is a canonical package digest, `sha256:<64 lowercase hex>`.
///
/// **The algorithm prefix is required**, for the reason `ActionDigest` and `SchemaFingerprint` record: a
/// bare digest could be any algorithm, and two digests are only comparable when the algorithm is part of
/// the value. This is the domain's third carrier of the same rule, and they agree deliberately — a
/// package digest, an action digest, and a schema fingerprint that disagreed about the form would let a
/// value written by one be refused by another while looking identical to a reader.
#[must_use]
pub fn is_canonical_package_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Returns whether `value` is bounded, non-empty, control-free identity text.
///
/// The rule for the fields whose *content* this crate does not interpret — a signature identity, a
/// source reference, a protocol name — and whose failure is a corrupt log line or a store row carrying
/// an unbounded value rather than a semantic one.
#[must_use]
pub fn is_bounded_identity_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_PLUGIN_IDENTIFIER_BYTES
        && !value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
}

/// A durable grant for a plugin, scoped to a workspace and a granting principal.
///
/// The contract's "Permissions and Grants" section, as a value. It is explicit that manifest `requests`
/// are **untrusted permission requests** and that installation creates **no** grant — so this type is
/// built by an operator's grant decision, never by parsing a manifest, and the manifest's `requests`
/// list is only ever *compared against* it.
///
/// The grant binds the **whole [`PluginSourceIdentity`]** rather than the `id`, which is the `ACC-024`
/// shape the tool fabric already uses: identity includes the publisher, the package digest, and the
/// signature identity, so a replacement behind the same `id` does not inherit the grant. It also binds
/// the workspace and the granting principal, because the contract lists both, and a grant applied "by
/// plugin" would authorize one workspace's plugin across another's.
///
/// **An expansion cannot be expressed as a check on a single field, so it is one method.** The
/// contract names three kinds of expansion — capability, schema, and effect — and whether an update
/// grants more is answered in [`Self::grants_no_more_than`], the one place the question is asked, rather
/// than at each call site. Its direction is the crux: the check is a **subset** on each dimension, since
/// an update that grants *more* than the version an operator approved is the expansion the rule exists
/// to refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginGrant {
    /// The exact installed source identity this grant covers.
    pub identity: PluginSourceIdentity,
    /// The workspace it applies in.
    pub workspace: crate::ids::WorkspaceId,
    /// The principal that issued it.
    pub principal: crate::ids::PrincipalId,
    /// The capability selectors it confers.
    ///
    /// The contract's "specific capability/resource/tool selectors". A `BTreeSet` rather than a
    /// `Vec`, so two grants that confer the same set compare equal regardless of the order they were
    /// read in — the same reason `Grant` in the tool policy module uses a set.
    pub capabilities: std::collections::BTreeSet<PluginCapabilitySelector>,
    /// The greatest risk the granted capabilities may reach.
    pub risk_ceiling: crate::tool::classification::Risk,
    /// The greatest sensitivity of data the plugin may receive.
    pub sensitivity_ceiling: Sensitivity,
    /// When it stops applying, if it does.
    pub expires_at: Option<UtcTimestamp>,
}

impl PluginGrant {
    /// Returns whether this grant applies to `identity` in `workspace` for `principal` at `now`.
    ///
    /// All four conditions together, for the reason the tool `Grant::applies_at` records: a grant that
    /// failed any one is not "close enough". The identity comparison is a whole-tuple equality, so a
    /// package whose bytes, publisher, or signature changed is a different plugin and the grant does not
    /// apply — `ACC-024`'s rule at this layer. An expired grant is **not applicable**, so a caller's
    /// reason is "no grant" with an expiry check available separately.
    #[must_use]
    pub fn applies_at(
        &self,
        identity: &PluginSourceIdentity,
        workspace: crate::ids::WorkspaceId,
        principal: crate::ids::PrincipalId,
        now: UtcTimestamp,
    ) -> bool {
        &self.identity == identity
            && self.workspace == workspace
            && self.principal == principal
            && !self.is_expired_at(now)
    }

    /// Returns whether the grant names this identity but has expired at `now`.
    ///
    /// Used to tell the two refusal cases apart for the operator, since both refuse.
    #[must_use]
    pub fn names_identity_but_expired(
        &self,
        identity: &PluginSourceIdentity,
        workspace: crate::ids::WorkspaceId,
        principal: crate::ids::PrincipalId,
        now: UtcTimestamp,
    ) -> bool {
        &self.identity == identity
            && self.workspace == workspace
            && self.principal == principal
            && self.is_expired_at(now)
    }

    /// Returns whether the grant has expired at `now`.
    #[must_use]
    pub fn is_expired_at(&self, now: UtcTimestamp) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }

    /// Returns whether `self` grants **no more** than `earlier` — the "no expansion" test.
    ///
    /// The "no capability/schema/effect expansion" half of the contract's carry-forward rule, and the
    /// direction is the whole point: an update that grants **more** than the version an operator approved
    /// is an expansion, so the check is a **subset** on every dimension, not a superset. A superset test
    /// would carry a grant across exactly the update it exists to stop — that was this method's first
    /// version, and two tests caught the inversion immediately.
    ///
    /// Each comparison is in the safe direction: every capability `self` confers must already be conferred
    /// by `earlier`; and every ceiling (risk, sensitivity) must be `<=` the earlier one, because a ceiling
    /// is a maximum, so a lower one is narrower. The third dimension the contract names — the **schema** —
    /// is carried by the whole identity, which [`Self::carry_forward_to`] compares separately, so it is not
    /// re-derived here.
    #[must_use]
    pub fn grants_no_more_than(&self, earlier: &Self) -> bool {
        self.capabilities.is_subset(&earlier.capabilities)
            && self.risk_ceiling <= earlier.risk_ceiling
            && self.sensitivity_ceiling <= earlier.sensitivity_ceiling
    }

    /// Returns whether a grant written for `self`'s plugin may carry forward to `candidate`.
    ///
    /// **This is the one place the contract's carry-forward rule is answered**, so it has a single
    /// definition rather than a continuity check at one call site and an expansion check at another —
    /// which is how a caller comes to apply one and forget the other. Three conditions:
    ///
    /// 1. **the source is the same plugin, newer** ([`SourceContinuity::classify`] is continuity);
    /// 2. **the same workspace and principal**, because a grant does not migrate between them;
    /// 3. **no expansion** ([`Self::grants_no_more_than`]), so an update that grants nothing more carries
    ///    the grant and one that grants more does not.
    ///
    /// A `false` means the grant does not carry forward and the new version requires review — the
    /// contract's "otherwise they require review".
    #[must_use]
    pub fn carry_forward_to(&self, candidate: &Self) -> bool {
        candidate.workspace == self.workspace
            && candidate.principal == self.principal
            && SourceContinuity::classify(&self.identity, &candidate.identity).is_continuity()
            && candidate.grants_no_more_than(self)
    }
}

/// One capability selector a grant confers, in the contract's `namespace.name[:selector]` form.
///
/// The contract's grants bind "specific capability/resource/tool selectors", and a selector is what
/// makes a grant *narrow*: `jarvis.tools.read:selected` confers reads of named tools rather than all
/// reads. The form is validated so it cannot carry control characters into a persisted grant or an
/// operator display, and it is bounded for the same reason every other stored string here is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginCapabilitySelector(String);

impl PluginCapabilitySelector {
    /// Validates and wraps a capability selector.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the `capabilities` field when the value is
    /// empty, over-long, or contains anything outside the characters a namespaced selector uses.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        let usable = !value.is_empty()
            && value.len() <= MAX_PLUGIN_IDENTIFIER_BYTES
            && value.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '.' | ':' | '-' | '_' | '/' | '*')
            });
        if !usable {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "capabilities",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the selector text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginCapabilitySelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PluginCapabilitySelector {
    /// Deserializes **through [`Self::new`]**, not by wrapping the string.
    ///
    /// The derived deserializer would produce a selector the constructor refuses, making the bound true
    /// of values this crate built and false of values that arrived over the wire — the direction an
    /// attacker chooses. This is the same fix `Scope` and `WorkspaceRelativePath` record, and it is
    /// required rather than optional because a selector reaches a persisted grant.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for PluginCapabilitySelector {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
