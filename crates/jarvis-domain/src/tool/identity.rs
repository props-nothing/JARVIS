//! Canonical tool identity: the capability, its source, and its schema fingerprint.
//!
//! The tool contract states the rule this module exists to make structural:
//!
//! > Tool names are human-readable aliases; authorization binds a canonical tool ID,
//! > source identity, schema fingerprint, workspace, and resource scope. Discovery
//! > cache changes cannot retarget an existing approval to a different implementation.
//!
//! So an identity here is a **tuple**, not a name. A display name is an alias that
//! two different tools may share; [`ToolIdentity`] is what an approval binds to, and
//! [`ToolIdentity::authorizes`] is the one place that decides whether a grant recorded
//! against one identity still covers another. Everything that could make two tools
//! distinguishable — the implementation source, its version, and a fingerprint over
//! the input schema — is an input to that decision, because "same name" is not a
//! statement about the same tool.
//!
//! The consequence the contract calls out (`ACC-024`) is that replacing a tool's
//! schema or source **behind the same display name** must not be authorized by the
//! approval recorded for the original. Three separate facts enforce that: the name is
//! not part of identity at all, the fingerprint is over the schema rather than over
//! the name, and the source carries an owner and version so two servers publishing
//! `fs.read` are different tools rather than one tool with two backends.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// The longest accepted namespace, name, or version suffix.
///
/// Bounded for the same reason every other bound here is: this text reaches persisted
/// approval records, logs, and operator output, so it is bounded where it is *accepted*
/// rather than where it is displayed.
pub const MAX_TOOL_SEGMENT_BYTES: usize = 64;

/// The largest accepted major version.
///
/// A bound rather than `u32::MAX`, because the major version is part of a stored identity
/// and an unbounded integer invites a value that a client cannot round-trip; the limit is
/// far above any plausible tool's history.
pub const MAX_TOOL_MAJOR: u32 = 999_999;

/// Returns whether `value` is a usable publisher identifier.
///
/// **Deliberately not [`is_segment`].** The contract's own example owner is `google.gmail`, so a
/// publisher identifier is a *dotted* name — it names a vendor and a product, the same way a
/// package name does. Reusing the capability's segment rule here is the mistake this comment
/// exists to prevent, and it fails in the quiet direction: the fixture in this module's tests
/// used `acme.files`, so the rule was refused at construction and thirteen tests failed on the
/// *fixture* rather than on the rule each of them was about.
///
/// Hyphens are allowed because vendors publish under them (`internal-files`); a leading digit is
/// not, so the name cannot look like a version; and dots are allowed **between** non-empty parts
/// rather than anywhere, because `a..b` or a trailing dot would let two spellings denote one
/// owner.
fn is_owner(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_TOOL_SEGMENT_BYTES * 2 {
        return false;
    }
    let Some(first) = value.chars().next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let usable_character = |character: char| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
            || character == '.'
    };
    if !value.chars().all(usable_character) {
        return false;
    }
    // Every dotted part must be non-empty: `a..b`, `.a`, and `a.` are each a second spelling of
    // something that would otherwise parse.
    value.split('.').all(|part| !part.is_empty())
}

/// Returns whether `value` is a usable namespace or name segment.
///
/// Deliberately narrow: lowercase ASCII letters, digits, and underscores, starting with a
/// letter. A dot cannot appear because the canonical form uses dots as separators, so
/// allowing one inside a segment would make `a.b.c` ambiguous between one name with a dot
/// and a three-part id — the ambiguity that lets two spellings denote one identity.
fn is_segment(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_TOOL_SEGMENT_BYTES {
        return false;
    }
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    value.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    })
}

/// Parses a decimal component with no sign, no leading zero, and a bound.
fn parse_decimal(value: &str, max: u32) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // A leading zero is refused rather than normalized: `01` and `1` denoting the same
    // version would be two spellings of one identity, which is exactly what the canonical
    // form exists to prevent.
    if value.len() > 1 && value.starts_with('0') {
        return None;
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| *parsed <= max && *parsed > 0)
}

/// A canonical tool capability: `namespace.name@major`.
///
/// The major version is part of the identity because the contract says a breaking
/// input or effect change "requires a new major tool identity" — so `email.send@1` and
/// `email.send@2` are two different tools, and an approval for the first cannot authorize
/// the second. Parsing is strict in the same way [`crate::ids`] identifiers are: only the
/// canonical spelling is accepted, so two spellings never denote two identities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolCapability {
    namespace: String,
    name: String,
    major: u32,
}

impl ToolCapability {
    /// Parses a canonical capability from `namespace.name@major`.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolIdentifierNotCanonical`] for anything that is not exactly
    /// the canonical form: a missing or repeated `@`, a missing or repeated dot, an empty or
    /// invalid segment, a non-decimal major, a leading zero, or a major of `0`. A major of
    /// `0` is refused specifically because it is what an uninitialised field produces, so
    /// accepting it would let a construction mistake read as version zero of a real tool.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let not_canonical = || DomainError::ToolIdentifierNotCanonical;
        let (capability, major) = value.split_once('@').ok_or_else(not_canonical)?;
        // A second `@` means the input is not the canonical form; splitting once would
        // otherwise accept `a.b@1@2` with a major that fails to parse, which reports the
        // wrong reason.
        if major.contains('@') {
            return Err(not_canonical());
        }
        let (namespace, name) = capability.split_once('.').ok_or_else(not_canonical)?;
        // Exactly one dot: a second one would make the name non-canonical, and the segment
        // check below refuses it anyway, but the explicit check names the rule.
        if name.contains('.') {
            return Err(not_canonical());
        }
        if !is_segment(namespace) || !is_segment(name) {
            return Err(not_canonical());
        }
        let major = parse_decimal(major, MAX_TOOL_MAJOR).ok_or_else(not_canonical)?;
        Ok(Self {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            major,
        })
    }

    /// Builds a capability from already-validated parts.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolIdentifierNotCanonical`] when either segment is invalid or
    /// the major is out of range, so a caller that splits an id itself cannot construct one
    /// the parser would have refused.
    pub fn new(namespace: &str, name: &str, major: u32) -> Result<Self, DomainError> {
        if !is_segment(namespace) || !is_segment(name) || major == 0 || major > MAX_TOOL_MAJOR {
            return Err(DomainError::ToolIdentifierNotCanonical);
        }
        Ok(Self {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            major,
        })
    }

    /// Returns the namespace segment.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the name segment.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the major version.
    #[must_use]
    pub const fn major(&self) -> u32 {
        self.major
    }
}

impl fmt::Display for ToolCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}@{}", self.namespace, self.name, self.major)
    }
}

impl std::str::FromStr for ToolCapability {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ToolCapability {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ToolCapability {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// A tool implementation version, `major.minor.patch` with an optional pre-release suffix.
///
/// Separate from the capability's major because they answer different questions. The
/// capability's major is *identity* — it changes the tool. This is the implementation's
/// version, which is informational until it changes the schema fingerprint; a minor or patch
/// release that alters the input schema changes the fingerprint and therefore the identity,
/// while one that only fixes a bug does not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolVersion {
    major: u32,
    minor: u32,
    patch: u32,
    pre_release: Option<String>,
}

impl ToolVersion {
    /// Parses `major.minor.patch` or `major.minor.patch-suffix`.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the `version` field when the form
    /// is wrong. Unlike the capability this is a definition field rather than an identity, so
    /// the error names the field an operator has to go and fix.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let invalid = || DomainError::ToolDefinitionInvalid { field: "version" };
        let (numbers, pre_release) = match value.split_once('-') {
            Some((numbers, suffix)) => {
                if suffix.is_empty()
                    || suffix.len() > MAX_TOOL_SEGMENT_BYTES
                    || !suffix
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                {
                    return Err(invalid());
                }
                (numbers, Some(suffix.to_owned()))
            }
            None => (value, None),
        };
        let mut parts = numbers.split('.');
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        // `parse_decimal` refuses zero, which is right for a capability major and wrong here:
        // `1.0.0` is the ordinary first release. Zero is allowed for all three components as
        // long as the *major* agrees with the capability, which `ToolDefinition` checks.
        let component = |value: &str| {
            if value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
                || (value.len() > 1 && value.starts_with('0'))
            {
                return Err(invalid());
            }
            value.parse::<u32>().map_err(|_| invalid())
        };
        Ok(Self {
            major: component(major)?,
            minor: component(minor)?,
            patch: component(patch)?,
            pre_release,
        })
    }

    /// Returns the major component.
    #[must_use]
    pub const fn major(&self) -> u32 {
        self.major
    }
}

impl fmt::Display for ToolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre_release) = &self.pre_release {
            write!(formatter, "-{pre_release}")?;
        }
        Ok(())
    }
}

impl Serialize for ToolVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ToolVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Where a tool implementation came from.
///
/// The kinds are the ones the tool fabric lists as `source`: native, connector, MCP server,
/// runtime, plugin. A typed enum rather than a string because the classification is a policy
/// input — a remote MCP server and a native tool reach the same pipeline through different trust
/// assumptions, and a string would let a typo select the wrong branch. **The policy branch that
/// consumes it does not exist yet** (`BRN-063` corrected this doc, which said "policy branches on
/// it" as though one did); what exists is the exhaustive [`SourceKind::ALL`] table and the test that
/// requires every kind to be classified, so the input is ready and cannot silently default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Built into the daemon.
    Native,
    /// A first-party connector.
    Connector,
    /// An external MCP server.
    McpServer,
    /// An isolated external agent runtime.
    Runtime,
    /// A process plugin.
    Plugin,
}

impl SourceKind {
    /// Every source kind, in a fixed order.
    ///
    /// **Added by `BRN-063`, because the externality classification was not enumerable and therefore
    /// not enforced.** `is_external` is a `matches!` over three named variants, so a variant added to
    /// the enum compiles with no answer here and is treated as **not external** — the fail-open
    /// direction, where a new remote source would be read as trusted. With this list,
    /// `source_kind_classifies_externality_deliberately` compares it against a table written out by
    /// hand, so adding a variant fails the length check and the author must state which side it is on.
    /// The same technique `ToolCallState::ALL` and `LedgerOperation::ALL` use.
    pub const ALL: &'static [Self] = &[
        Self::Native,
        Self::Connector,
        Self::McpServer,
        Self::Runtime,
        Self::Plugin,
    ];

    /// Returns the spelling the tool contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Connector => "connector",
            Self::McpServer => "mcp_server",
            Self::Runtime => "runtime",
            Self::Plugin => "plugin",
        }
    }

    /// Returns whether the source is external to the daemon.
    ///
    /// **No production caller yet, and `BRN-063` corrected this doc.** It read "used by policy rather
    /// than by this module", but nothing in `jarvis_domain::tool::policy` calls it: policy branches on
    /// effects and risk, not on the source kind, and the tool fabric has no HTTP surface, so this
    /// predicate is reachable from its own tests alone. It is kept and defined here — rather than in a
    /// future policy module — so the classification has **one home**.
    ///
    /// **The "must be classified deliberately" hope is now enforced.** `ALL` is the enumeration, and
    /// `source_kind_classifies_externality_deliberately` compares it against a table written out by
    /// hand, so a source kind added to the enum fails the length check rather than silently defaulting
    /// to **not external** — which is the fail-open direction that matters, where a new remote source
    /// would be read as trusted.
    #[must_use]
    pub const fn is_external(self) -> bool {
        matches!(self, Self::McpServer | Self::Runtime | Self::Plugin)
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// The immutable provenance of a tool implementation.
///
/// `owner` is who published it and `version` is what they published. Both are part of
/// identity because the contract says a discovery cache change "cannot retarget an existing
/// approval to a different implementation": if identity were the kind and the id alone, a
/// second MCP server configured with the same id would silently inherit the first server's
/// approvals.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ToolSource {
    /// The source kind.
    pub kind: SourceKind,
    /// The publisher's identifier, for example `google.gmail` or an MCP server name.
    pub owner: String,
    /// The source's own version.
    pub version: ToolVersion,
}

impl ToolSource {
    /// Builds a source, validating the owner and version.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `source.owner` or `source.version`
    /// when the value is unusable.
    pub fn new(kind: SourceKind, owner: &str, version: ToolVersion) -> Result<Self, DomainError> {
        if !is_owner(owner) {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "source.owner",
            });
        }
        Ok(Self {
            kind,
            owner: owner.to_owned(),
            version,
        })
    }
}

impl<'de> Deserialize<'de> for ToolSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The wire form, so a missing field is still an error and the field set is not restated in a
        /// tuple.
        #[derive(Deserialize)]
        struct Wire {
            kind: SourceKind,
            owner: String,
            version: ToolVersion,
        }
        let source = Wire::deserialize(deserializer)?;
        // **Through [`Self::new`] rather than derived.** `ToolSource` is a field of
        // [`ToolIdentity`], which is read back from `tool_identity_json` on every approval and
        // ledger load — so an invalid owner (an empty string, uppercase, a NUL) would otherwise
        // reach the identity that a grant, an approval, and a deduplication key all bind to. The
        // owner is also what makes a same-named tool from a different publisher a *different* tool
        // (`ACC-024`), so a malformed one is an identity defect rather than a cosmetic one.
        Self::new(source.kind, &source.owner, source.version).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ToolSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}@{}", self.kind, self.owner, self.version)
    }
}

/// A `SHA-256` fingerprint over a tool's input schema.
///
/// **The algorithm is part of the value.** The contract writes the fingerprint as
/// `sha256:...`, and this type requires that prefix rather than accepting a bare digest, so a
/// fingerprint produced by a different algorithm cannot compare equal to one produced by this
/// one. Without the algorithm in the value, two different documents that happened to digest
/// to the same bytes under different functions would be indistinguishable, and a switch of
/// algorithm would silently invalidate or silently *preserve* an existing approval depending
/// on which way the comparison fell.
///
/// The domain carries the value and validates its shape; **computing** it from a document is an
/// adapter's job, because hashing is a concrete implementation and the domain layer must not depend on
/// one.
///
/// **⚠ That `jarvis-infrastructure` provides the computation is a claim this doc made for several rounds
/// while the function did not exist.** The only construction path was [`Self::from_bytes`], which every
/// caller in the product — and in the tests — used with a hand-written seed, so a schema change could not
/// move an identity and `ACC-024`'s "a release that alters the input schema changes the fingerprint and
/// therefore the identity" had nothing behind it. `jarvis_infrastructure::tool_fingerprint::schema_fingerprint_of`
/// is the derivation now, and it domain-separates with a `tool-schema:` prefix so a schema fingerprint
/// cannot collide with an action fingerprint over the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaFingerprint([u8; 32]);

impl SchemaFingerprint {
    /// The algorithm prefix this type accepts and emits.
    pub const ALGORITHM: &'static str = "sha256";

    /// The number of hex characters a digest has.
    const HEX_LENGTH: usize = 64;

    /// Wraps an already-computed digest.
    #[must_use]
    pub const fn from_bytes(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Parses the contract's `sha256:<hex>` form.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `schema_fingerprint` when the
    /// prefix is missing or names another algorithm, or when the digest is not exactly 64
    /// lowercase hex characters. Uppercase is refused rather than lowercased for the same
    /// reason identifiers reject it: two spellings of one digest would let a stored approval
    /// miss a match that is really the same schema.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let invalid = || DomainError::ToolDefinitionInvalid {
            field: "schema_fingerprint",
        };
        let hex = value
            .strip_prefix(Self::ALGORITHM)
            .and_then(|rest| rest.strip_prefix(':'))
            .ok_or_else(invalid)?;
        if hex.len() != Self::HEX_LENGTH
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid());
        }
        let mut digest = [0_u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            let pair = &hex[index * 2..index * 2 + 2];
            *byte = u8::from_str_radix(pair, 16).map_err(|_| invalid())?;
        }
        Ok(Self(digest))
    }

    /// Returns the raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for SchemaFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(Self::ALGORITHM)?;
        formatter.write_str(":")?;
        for byte in &self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for SchemaFingerprint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SchemaFingerprint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// The tuple an authorization decision binds to.
///
/// **A display name is deliberately absent.** The contract calls names "human-readable
/// aliases", and an alias is exactly the thing that can be re-pointed — so including one here
/// would make the alias authoritative, which is the defect `ACC-024` describes rather than the
/// rule that prevents it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ToolIdentity {
    /// The canonical capability, including its major version.
    pub capability: ToolCapability,
    /// The implementation's provenance.
    pub source: ToolSource,
    /// A fingerprint over the input schema.
    pub schema_fingerprint: SchemaFingerprint,
}

impl ToolIdentity {
    /// Returns whether a grant or approval recorded against `self` authorizes `candidate`.
    ///
    /// **This is the only place the question is answered**, so the rule has one definition
    /// rather than a comparison repeated at each call site — and a call site that compared
    /// only the capability would be a second, weaker answer living beside this one.
    ///
    /// All three components must match. The source check is what makes a replacement
    /// *behind the same name* insufficient: two tools with one capability from two owners are
    /// different tools, and the schema fingerprint check is what makes a same-owner schema
    /// change insufficient. There is deliberately no "same name" shortcut for the same reason
    /// the name is not a field.
    #[must_use]
    pub fn authorizes(&self, candidate: &Self) -> bool {
        self == candidate
    }

    /// Returns whether `self` and `candidate` share a capability but differ otherwise.
    ///
    /// Kept separate from `authorizes` because the two answers are used differently: a caller
    /// that must explain a refusal needs to know that the tool still exists and is simply not
    /// the one that was approved, which is a different operator conversation from "no such
    /// tool". A single boolean would collapse those.
    #[must_use]
    pub fn is_replacement_of(&self, candidate: &Self) -> bool {
        self.capability == candidate.capability && self != candidate
    }
}

impl fmt::Display for ToolIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} [{} {}]",
            self.capability, self.source, self.schema_fingerprint
        )
    }
}
