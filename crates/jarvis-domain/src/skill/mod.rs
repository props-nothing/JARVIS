//! The skill contract's domain shape: a persisted procedure, its trust tier, and the
//! **authority-narrowing invariant** that governs what loading one may do.
//!
//! `docs/contracts/skill-contract.md` is the authority for the shape and
//! [ADR-0012](../../../../docs/adr/0012-governed-learning-loop.md) for the decision. This module
//! implements the part of that contract whose failure is a security fault rather than a cosmetic one —
//! section 3, *"Loading a skill may narrow the tool catalog available to a call; it may never widen
//! authority"* — together with the trust tiers that decide which scan findings may be overridden.
//!
//! ## Why the invariant is a requirement rather than a convention
//!
//! The contract is explicit: *"An implementation that cannot prove it is not complete, because the
//! failure mode — a procedure that grants itself a capability — is the single most dangerous thing a
//! plugin or skill ecosystem can carry."* So this module does not merely document the rule. It gives the
//! rule a **type** ([`SkillCatalogNarrowing`]) whose only operation either returns a catalog that is a
//! subset of the principal's grants or returns the **reason it was refused**, naming every dimension
//! that widened.
//!
//! The direction is the whole point. Every dimension is a **subset or a ceiling comparison** in the
//! narrowing sense:
//!
//! - a declared tool must be one the principal holds a grant for;
//! - declared scopes and effects must be **subsets** of the granted ones;
//! - declared risk and sensitivity must be **at or below** the granted ceilings;
//! - a declared approval must be **no weaker** than the granted one;
//! - declared timeout and attempts must be **at or below** the granted bounds.
//!
//! A predicate asked the other way — "does the grant cover the declared set", written as a superset test
//! on the wrong operand — carries exactly the skill the rule exists to stop. The test names therefore
//! state the rule in the direction that matters
//! (`a_skill_that_adds_a_tool_is_refused`, `a_relaxed_approval_is_refused`), and each dimension is
//! falsified by its own expansion rather than by one.
//!
//! ## What this module deliberately is not
//!
//! It is **not** a loader, a scanner, or a store. Progressive disclosure, the scan pipeline, quarantine,
//! the provenance lockfile, and the learning tools are `TLS-016`'s remaining work and `MEM-011`'s; they
//! need a content store and a filesystem this module has no business touching. What is here is the
//! vocabulary those slices must not re-derive, and the one invariant they must consult rather than
//! restate.
//!
//! It also does **not** define a new digest format. [`SkillContentHash`] reuses the domain's existing
//! `sha256:<hex>` rule and, deliberately, the type the tool fabric already owns for a derived digest —
//! see its own doc for why a fourth carrier of the same rule would be the defect rather than the fix.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::model::policy::Sensitivity;
use crate::tool::classification::{ApprovalHint, Effect, Risk, Scope};
use crate::tool::identity::{SchemaFingerprint, ToolIdentity};

/// The longest a skill name may be.
///
/// Bounded like every other identifier in the domain: a name reaches a context-manifest entry, a log
/// line, and a slash command, so an unbounded or control-character-bearing one is an integrity fault
/// rather than an inconvenience.
pub const MAX_SKILL_NAME_BYTES: usize = 128;

/// The most capabilities one skill may declare.
///
/// The same bound the plugin manifest uses for its capability list, and for the same reason: the
/// declaration is what the narrowing check iterates, so an unbounded list is an unbounded check before
/// any tool call happens.
pub const MAX_SKILL_CAPABILITIES: usize = 64;

/// The stable, namespaced name of a skill.
///
/// **A slug, not free text, and never an authorization input** — the contract says the human-readable
/// `name` "is not an authorization input", which is what makes this the field a lookup and a stored
/// reference use. A name that carried control characters or unbounded length would be an integrity
/// fault in the index, the manifest, and every log line that mentions the skill.
///
/// `Deserialize` goes through [`Self::new`] rather than being derived, so a name that arrived over the
/// wire is held to the same rule as one this crate built — the direction an attacker chooses. See
/// `crate::tool::classification::Scope` for the same fix and the full reasoning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SkillName(String);

impl SkillName {
    /// Validates and wraps a skill name.
    ///
    /// A dotted slug: at least one `.`, no empty part, lowercase alphanumerics and `.`/`-`/`_` only.
    /// The dot is required so a name is namespaced rather than a bare word that could collide with a
    /// future reserved command.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidIdentifier`] naming `skill_name`. An invalid name must not be
    /// coerced into a valid one, because a coerced name is a different skill.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        let invalid = || DomainError::InvalidIdentifier { kind: "skill_name" };
        if value.is_empty()
            || value.len() > MAX_SKILL_NAME_BYTES
            || !value.contains('.')
            || value.starts_with('.')
            || value.ends_with('.')
            || value.split('.').any(str::is_empty)
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || byte == b'.'
                    || byte == b'-'
                    || byte == b'_'
            })
        {
            return Err(invalid());
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SkillName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SkillName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

/// The digest covering a skill's body **and every file it can load**.
///
/// A newtype over the domain's derived-digest type rather than a fourth `sha256:<hex>` carrier. Three
/// types already carry that rule (`SchemaFingerprint`, `ActionDigest`, and the plugin package digest),
/// and the module that records the plugin digest warns about exactly this: *"This is the domain's third
/// carrier of the same rule"*. A fourth would be one more place for the rule to drift, and the reason the
/// contract names `content_hash` separately is **what it covers**, not how it is spelled.
///
/// So the coverage is the type's meaning: a reference file a procedure loads on demand is part of the
/// procedure, and a skill whose body is unchanged but whose reference changed is a **different** skill.
/// The type carries that by construction — a caller cannot build one from a body alone, because the
/// derivation ([`crate::skill::SkillContentHash`]'s only producer,
/// `jarvis_infrastructure::tool_fingerprint::skill_content_hash`) takes the body and the references
/// together. That is what makes ADR-0012's decision 5
/// enforceable rather than merely stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillContentHash(SchemaFingerprint);

impl SkillContentHash {
    /// Wraps an already-derived digest over the body **and every reference file**.
    ///
    /// **A constructor, not a derivation, because the domain owns no hash primitive.** The rule this
    /// module records elsewhere applies here too: `jarvis_domain` depends on `jiff`, `serde`,
    /// `thiserror`, and `uuid` and nothing else, so a `sha2` call would be a new dependency in the layer
    /// the direction rules forbid — and the domain's own derived digests (`SchemaFingerprint`,
    /// `ActionDigest`) are *derived in infrastructure* for exactly that reason.
    /// `jarvis_infrastructure::skill::content_hash_of` is the derivation, and it takes the body and the
    /// references together so the coverage the contract names cannot be forgotten at a call site.
    ///
    /// The type still carries the contract's *meaning* — what the hash covers — because a caller cannot
    /// name one without having hashed the referenced files: the derivation's signature requires them.
    #[must_use]
    pub const fn from_bytes(digest: [u8; 32]) -> Self {
        Self(SchemaFingerprint::from_bytes(digest))
    }

    /// Parses the contract's `sha256:<hex>` form.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `content_hash` when the prefix is missing or
    /// the digest is not exactly 64 lowercase hex characters — delegated to the domain type so the two
    /// spellings cannot disagree.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        SchemaFingerprint::parse(value).map(Self)
    }

    /// Returns the raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl fmt::Display for SkillContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One named reference file a skill may load on demand.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillReference {
    name: String,
    content: String,
}

impl SkillReference {
    /// Builds a reference file.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidIdentifier`] naming `reference_name` for an empty, over-long, or
    /// control-character-bearing name. The name reaches a manifest entry and a filesystem lookup, so the
    /// same rule every other bounded identifier follows applies.
    pub fn new(name: &str, content: &str) -> Result<Self, DomainError> {
        if name.is_empty()
            || name.len() > MAX_SKILL_NAME_BYTES
            || name.contains('\0')
            || name.trim().is_empty()
        {
            return Err(DomainError::InvalidIdentifier {
                kind: "reference_name",
            });
        }
        Ok(Self {
            name: name.to_owned(),
            content: content.to_owned(),
        })
    }

    /// Returns the reference's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the reference's content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// The set of reference files a skill can load.
///
/// **A set, not a vector**, because the content hash must be a function of *what the skill contains*
/// rather than of the order a caller happened to list it. A vector would make two equal skills hash
/// differently, and the grant binding in the contract's section 8 compares hashes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillReferences {
    files: BTreeSet<SkillReference>,
}

impl SkillReferences {
    /// Creates an empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a reference.
    ///
    /// Returns whether it was inserted, so a caller can detect a duplicate name with different content —
    /// which is a different skill under the same name and must not be silently resolved.
    pub fn insert(&mut self, reference: SkillReference) -> bool {
        self.files.replace(reference).is_none()
    }

    /// Returns whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Returns the number of reference files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Iterates the references in canonical order.
    ///
    /// Canonical order is why the underlying store is a set: the content hash must be a function of *what
    /// the skill contains*, not of the order a caller listed it, so the iteration order has one definition
    /// rather than one per hasher.
    pub fn iter(&self) -> impl Iterator<Item = &SkillReference> {
        self.files.iter()
    }
}

/// Where a skill came from.
///
/// Distinct from [`SkillTier`], and the distinction is the contract's: the **source** is the mechanism,
/// the **tier** is the trust. A skill installed from a pinned registry and one installed from an arbitrary
/// URL share a source kind and differ in tier, which is why a tier can never be inferred from a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Written by a user in this profile.
    Authored,
    /// Ships with JARVIS.
    Bundled,
    /// Fetched from a resolved external source.
    Installed,
    /// Written by an agent run.
    Learned,
}

impl SkillSource {
    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidIdentifier`] naming `skill_source` for an unknown value. An
    /// unrecognised source must not default to `Authored`, which would understate provenance — the same
    /// fail-closed direction `Risk::parse` records.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "authored" => Ok(Self::Authored),
            "bundled" => Ok(Self::Bundled),
            "installed" => Ok(Self::Installed),
            "learned" => Ok(Self::Learned),
            _ => Err(DomainError::InvalidIdentifier {
                kind: "skill_source",
            }),
        }
    }

    /// Returns the spelling the contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Authored => "authored",
            Self::Bundled => "bundled",
            Self::Installed => "installed",
            Self::Learned => "learned",
        }
    }
}

/// What a scan found.
///
/// Three levels, and the top one is special: the contract says *"A `dangerous` scan verdict is never
/// overridable at any tier."* That is not a policy a caller configures — it is a property of
/// [`SkillTier::may_override`], which refuses to override `Dangerous` even when a caller says it was
/// overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanVerdict {
    /// Nothing found.
    Clean,
    /// Something a user should read before deciding.
    Caution,
    /// Something that must not be loaded: exfiltration, injection, or destructive commands.
    Dangerous,
}

impl ScanVerdict {
    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidIdentifier`] naming `scan_verdict`. An unrecognised verdict must not
    /// become `Clean`, which is the fail-open direction on the one input that blocks a skill.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "clean" => Ok(Self::Clean),
            "caution" => Ok(Self::Caution),
            "dangerous" => Ok(Self::Dangerous),
            _ => Err(DomainError::InvalidIdentifier {
                kind: "scan_verdict",
            }),
        }
    }

    /// Returns the spelling the contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Caution => "caution",
            Self::Dangerous => "dangerous",
        }
    }
}

/// A skill's trust tier, which decides its install policy.
///
/// The tier comes from the **resolved source**, never from a field the skill declares — the contract is
/// explicit that *"a skill cannot promote its own tier by being re-imported from a higher-trust source
/// name"*. That is why this type has no `parse` from skill content: a tier is assigned by the installer
/// from where it actually fetched the skill, and a wire field naming one is ignored rather than honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillTier {
    /// Ships with JARVIS.
    Builtin,
    /// Published by the JARVIS project.
    Official,
    /// A reviewed registry the project pins.
    Trusted,
    /// User-installed from any other source.
    Community,
    /// Authored by an agent run.
    Learned,
}

impl SkillTier {
    /// Every tier, from most to least trusted.
    pub const ALL: &'static [Self] = &[
        Self::Builtin,
        Self::Official,
        Self::Trusted,
        Self::Community,
        Self::Learned,
    ];

    /// Returns whether this tier is gated by its scan result.
    ///
    /// `builtin` and `official` are "scanned for completeness, never blocked"; `trusted` blocks a
    /// dangerous finding but not otherwise; `community` and `learned` are gated. Returning a bool rather
    /// than a per-tier match arm at each call site is what keeps the policy in one place — an installer
    /// that re-derived it would be a second copy that could disagree.
    #[must_use]
    pub const fn is_scan_gated(self) -> bool {
        matches!(self, Self::Trusted | Self::Community | Self::Learned)
    }

    /// Returns whether this tier requires a learning grant before it may be installed.
    ///
    /// Only `learned`, per the contract's table: *"Deny-by-default; requires the learning grant and, by
    /// policy, review."*
    #[must_use]
    pub const fn requires_learning_grant(self) -> bool {
        matches!(self, Self::Learned)
    }

    /// Returns whether a finding of `verdict` may be overridden at this tier.
    ///
    /// **The rule the contract states as absolute, implemented as an absolute.** A `dangerous` finding is
    /// refused at every tier regardless of what the caller claims — a caller cannot pass a flag that
    /// overrides it, because the parameter is not consulted for that verdict at all. A `caution` finding
    /// is overridable only for `community`, and only when a user explicitly overrode it; `learned` is not
    /// overridable because the contract puts a learned write behind review rather than a warning, and a
    /// `Dangerous` verdict is never overridable at any tier.
    ///
    /// The asymmetry is the point: the override exists for caution-level findings a user has read, not for
    /// a verdict that names exfiltration, injection, or destructive commands.
    #[must_use]
    pub const fn may_override(self, verdict: ScanVerdict, explicit_override: bool) -> bool {
        match verdict {
            // Never, at any tier, whatever the caller says.
            ScanVerdict::Dangerous => false,
            // Nothing to override.
            ScanVerdict::Clean => true,
            // Caution: an explicit, recorded user override at `community` only.
            ScanVerdict::Caution => matches!(self, Self::Community) && explicit_override,
        }
    }
}

/// One capability a skill declares, with every bound it claims.
///
/// **One type for both the declared and the granted side**, and that is deliberate rather than
/// convenient: the invariant is a field-by-field comparison of the two, so giving them the same shape
/// makes the comparison obvious and impossible to write with the operands confused. On the granted side
/// `risk` and `sensitivity` are **ceilings**; on the declared side they are what the skill wants. The
/// comparison is the same either way, which is exactly the property that makes it safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCapability {
    /// The exact tool identity, so a recompiled tool is a different capability.
    pub identity: ToolIdentity,
    /// The scopes the capability needs.
    pub scopes: BTreeSet<Scope>,
    /// The effects the capability would exercise.
    pub effects: BTreeSet<Effect>,
    /// On the granted side a ceiling; on the declared side what the skill wants.
    pub risk: Risk,
    /// On the granted side a ceiling; on the declared side what the skill wants.
    pub sensitivity: Sensitivity,
    /// The approval posture. A declared one may be **stricter** than the granted one and never weaker.
    pub approval: ApprovalHint,
    /// The timeout in milliseconds.
    pub timeout_ms: u64,
    /// The greatest number of attempts.
    pub max_attempts: u32,
}

/// One way a skill's declaration **widened** authority beyond what the principal holds.
///
/// Each variant names the dimension, because an operator fixing one needs to know which: a single
/// "the skill expands authority" would send them to re-read the whole declaration. This is the same
/// argument `NewToolGrant::narrowing`'s four field-named conflicts make one layer over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillExpansion {
    /// The skill declares a tool the principal holds no grant for.
    UngrantedTool {
        /// The canonical capability string.
        capability: String,
    },
    /// The skill declares a scope the grant does not confer.
    AddedScope {
        /// The canonical capability string.
        capability: String,
        /// The scope that is not held.
        scope: String,
    },
    /// The skill declares an effect the grant does not permit.
    AddedEffect {
        /// The canonical capability string.
        capability: String,
        /// The effect that is not permitted.
        effect: &'static str,
    },
    /// The declared risk is above the granted ceiling.
    RaisedRisk {
        /// The canonical capability string.
        capability: String,
    },
    /// The declared sensitivity is above the granted ceiling.
    RaisedSensitivity {
        /// The canonical capability string.
        capability: String,
    },
    /// The declared approval posture is weaker than the granted one.
    RelaxedApproval {
        /// The canonical capability string.
        capability: String,
    },
    /// The declared timeout exceeds the granted bound.
    ExtendedTimeout {
        /// The canonical capability string.
        capability: String,
    },
    /// The declared attempt count exceeds the granted bound.
    RaisedAttempts {
        /// The canonical capability string.
        capability: String,
    },
}

impl SkillExpansion {
    /// Returns the stable, namespaced code for this expansion.
    ///
    /// `skill.*` rather than `jarvis.*`: this is the skill vocabulary the contract defines, and a caller
    /// branching on a code needs to tell "you added a scope" from "you relaxed the approval", because the
    /// two lead an operator to different edits.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UngrantedTool { .. } => "skill.ungranted_tool",
            Self::AddedScope { .. } => "skill.added_scope",
            Self::AddedEffect { .. } => "skill.added_effect",
            Self::RaisedRisk { .. } => "skill.raised_risk",
            Self::RaisedSensitivity { .. } => "skill.raised_sensitivity",
            Self::RelaxedApproval { .. } => "skill.relaxed_approval",
            Self::ExtendedTimeout { .. } => "skill.extended_timeout",
            Self::RaisedAttempts { .. } => "skill.raised_attempts",
        }
    }

    /// Returns the capability the expansion is about.
    #[must_use]
    pub fn capability(&self) -> &str {
        match self {
            Self::UngrantedTool { capability }
            | Self::AddedScope { capability, .. }
            | Self::AddedEffect { capability, .. }
            | Self::RaisedRisk { capability }
            | Self::RaisedSensitivity { capability }
            | Self::RelaxedApproval { capability }
            | Self::ExtendedTimeout { capability }
            | Self::RaisedAttempts { capability } => capability,
        }
    }
}

impl fmt::Display for SkillExpansion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UngrantedTool { capability } => {
                write!(formatter, "{capability} is not granted")
            }
            Self::AddedScope { capability, scope } => {
                write!(formatter, "{capability} requires the unheld scope {scope}")
            }
            Self::AddedEffect { capability, effect } => {
                write!(
                    formatter,
                    "{capability} declares the unpermitted effect {effect}"
                )
            }
            Self::RaisedRisk { capability } => {
                write!(formatter, "{capability} raises the risk ceiling")
            }
            Self::RaisedSensitivity { capability } => {
                write!(formatter, "{capability} raises the sensitivity ceiling")
            }
            Self::RelaxedApproval { capability } => {
                write!(formatter, "{capability} relaxes the approval requirement")
            }
            Self::ExtendedTimeout { capability } => {
                write!(formatter, "{capability} extends the timeout")
            }
            Self::RaisedAttempts { capability } => {
                write!(formatter, "{capability} raises the attempt count")
            }
        }
    }
}

/// A skill's declared capability set, and the operation that narrows it against a principal's grants.
///
/// **The type the contract's section 3 turns on.** Its only operation is [`Self::narrow`], which either
/// returns a catalog that is a subset of `granted` or returns the expansions that prevented it. There is
/// no accessor that hands back the declaration un-narrowed, because such an accessor is exactly how a
/// caller would load a skill without consulting the invariant — the shape `PluginManifest::parse` and the
/// plugin verifier's binding both use: give the value one construction path and make the rule part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCatalogNarrowing {
    declared: Vec<SkillCapability>,
}

impl SkillCatalogNarrowing {
    /// Builds a declaration from its capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `capabilities` when the declaration exceeds
    /// [`MAX_SKILL_CAPABILITIES`], and names `capability` when two entries share an identity. A duplicate
    /// identity is refused rather than merged, because "the same tool declared twice with different
    /// bounds" is a declaration whose meaning depends on which entry a checker reads first.
    pub fn new(declared: Vec<SkillCapability>) -> Result<Self, DomainError> {
        if declared.len() > MAX_SKILL_CAPABILITIES {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "capabilities",
            });
        }
        let mut seen: BTreeSet<&ToolIdentity> = BTreeSet::new();
        for capability in &declared {
            if !seen.insert(&capability.identity) {
                return Err(DomainError::ToolDefinitionInvalid {
                    field: "capability",
                });
            }
        }
        Ok(Self { declared })
    }

    /// Returns the number of declared capabilities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.declared.len()
    }

    /// Returns whether the skill declares no capability.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.declared.is_empty()
    }

    /// Narrows this declaration against a principal's grants.
    ///
    /// Returns the catalog the skill may use — which is the declaration, since every dimension has been
    /// proven at or below its grant — or **every** expansion that prevented it.
    ///
    /// **All expansions are collected rather than the first**, for the reason `failed_constraints` records
    /// one layer over: an author fixing one at a time discovers the next afterwards, and the list is
    /// bounded by the number of capabilities times eight.
    ///
    /// # Errors
    ///
    /// Returns `Err(expansions)` when any declared dimension widens authority. An empty `Vec` is never
    /// returned as an error: a declaration that narrows correctly returns `Ok`.
    pub fn narrow(
        &self,
        granted: &[SkillCapability],
    ) -> Result<Vec<SkillCapability>, Vec<SkillExpansion>> {
        let mut expansions: Vec<SkillExpansion> = Vec::new();
        for declared in &self.declared {
            let Some(grant) = granted
                .iter()
                .find(|grant| grant.identity == declared.identity)
            else {
                // An ungranted tool is one expansion and not eight: reporting the other seven dimensions
                // against a grant that does not exist would name comparisons that were never made.
                expansions.push(SkillExpansion::UngrantedTool {
                    capability: declared.identity.capability.to_string(),
                });
                continue;
            };
            expansions.extend(expansions_against(declared, grant));
        }
        if expansions.is_empty() {
            Ok(self.declared.clone())
        } else {
            Err(expansions)
        }
    }
}

/// Returns every dimension on which `declared` widens beyond `grant`.
///
/// Free and pure, so each dimension can be exercised directly and so the direction of each comparison is
/// stated once. **Every comparison is a subset or a `<=` in the narrowing direction**; there is no
/// superset test here, and the tests are named so that a reversal fails on the dimension it reversed.
fn expansions_against(declared: &SkillCapability, grant: &SkillCapability) -> Vec<SkillExpansion> {
    let capability = declared.identity.capability.to_string();
    let mut expansions = Vec::new();

    // Scopes and effects: a declared one the grant does not hold is **added authority**.
    for scope in &declared.scopes {
        if !grant.scopes.contains(scope) {
            expansions.push(SkillExpansion::AddedScope {
                capability: capability.clone(),
                scope: scope.as_str().to_owned(),
            });
        }
    }
    for effect in &declared.effects {
        if !grant.effects.contains(effect) {
            expansions.push(SkillExpansion::AddedEffect {
                capability: capability.clone(),
                effect: effect.as_contract_str(),
            });
        }
    }

    // The ceilings: declared must be **at or below** the grant's.
    if declared.risk > grant.risk {
        expansions.push(SkillExpansion::RaisedRisk {
            capability: capability.clone(),
        });
    }
    if declared.sensitivity > grant.sensitivity {
        expansions.push(SkillExpansion::RaisedSensitivity {
            capability: capability.clone(),
        });
    }

    // The approval posture: `Allow < Ask < Deny` by the domain type's own ordering, so a declared value
    // **below** the granted one is a demotion. Comparing in this direction is the whole point: a
    // reversed comparison would accept a skill that turns a required approval into an automatic action.
    if declared.approval < grant.approval {
        expansions.push(SkillExpansion::RelaxedApproval {
            capability: capability.clone(),
        });
    }

    // The execution bounds: declared must be **at or below** the grant's, so a declaration cannot buy
    // itself more time or more attempts than the authority it was given.
    if declared.timeout_ms > grant.timeout_ms {
        expansions.push(SkillExpansion::ExtendedTimeout {
            capability: capability.clone(),
        });
    }
    if declared.max_attempts > grant.max_attempts {
        expansions.push(SkillExpansion::RaisedAttempts { capability });
    }

    expansions
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
