//! RFC 8785 (JCS) canonicalization for the action fingerprint.
//!
//! `TLS-005` asks for "durable approval records and action fingerprinting", and until this module the
//! fingerprint was **a type with no computation**: the approval contract's own Implementation Status
//! said "the digest is **compared and never produced**", so a client could assert one and the rule
//! "approving 'send this email' does not approve a rewritten recipient, subject, body, attachment, or
//! account" rested on the caller's honesty. This module produces it.
//!
//! The canonical form is [RFC 8785](https://www.rfc-editor.org/rfc/rfc8785) (the JSON Canonicalization
//! Scheme), which the contract names: "use a researched deterministic JSON canonicalization such as RFC
//! 8785 and SHA-256, encoded with an explicit algorithm prefix". The evidence note is
//! `docs/research/integrations/rfc8785-canonicalization.md`.
//!
//! ## What this implements, and the one thing it refuses
//!
//! The JCS rules this module implements are the ones §3.2.2.2 and §3.2.3 state:
//!
//! - **No whitespace** between tokens (§3.2.1).
//! - **Strings**: `"` and `\` escaped, the five short forms `\b \t \n \f \r` for the characters that
//!   have them, and every other control character as **lowercase** `\uhhhh`. Every code point outside
//!   the control range is emitted as-is (§3.2.2.2, per ECMA-262 §24.3.2.2).
//! - **Property names sorted as arrays of UTF-16 code units**, compared as unsigned integers,
//!   independent of locale (§3.2.3).
//! - **The result is UTF-8** (§3.2.4), which a Rust `String` is.
//!
//! **The one thing it refuses is numbers, and the refusal is the security decision in this module.**
//! RFC 8785 §3.2.2.3 defers number serialization to ECMA-262 §7.1.12.1 and then says the algorithm "is
//! **not included in this document**"; Appendix B is a table of samples. A conformant number serializer
//! therefore needs shortest-round-trip double formatting (the Ryu family) plus ECMAScript's exponent
//! and negative-zero rules, and a subtly wrong one produces a digest that is **stable inside a single
//! build and different from every other implementation** — every internal test passes while a client
//! previewing the same action computes a different fingerprint and is refused. That is precisely the
//! failure the contract's "round-trip test across Rust and any client that previews/verifies
//! fingerprints" exists to prevent.
//!
//! The number problem is removable because the fingerprinted document is **JARVIS's own envelope**, not
//! arbitrary JSON: every value contributed is a **string**. So [`FingerprintInput`] has no field through
//! which a number, a boolean, `null`, an array, or a nested object could arrive, and the absence is
//! *structural* rather than a documented convention a later edit could break. Consequently `NaN` and
//! `Infinity` — which §3.2.2.3 requires an implementation to abort on — cannot even be expressed.
//!
//! The cost is honest and named: this module **cannot** canonicalize an arbitrary third-party document,
//! so no general-purpose JCS function is exported and the gap is recorded in `TLS-005` and in the
//! evidence note's "Open Questions".
//!
//! ## Why the values are text and the arguments are carried as text
//!
//! Two reasons, and the second is the one worth keeping. First, text needs no number serializer. Second,
//! an arguments document carried **as the bytes it arrived as** binds the fingerprint to *what the user
//! reviewed* rather than to a re-serialization of it — and a re-serialization is a second definition of
//! the action that could differ from the one shown in the approval preview.

use std::collections::BTreeMap;

/// The fingerprint format version this module produces.
///
/// **Part of the canonical input, deliberately.** The contract's list of fingerprint inputs begins
/// "fingerprint format version", because a future change to *which* fields are covered must produce a
/// different digest for the same action — otherwise an approval recorded under the old field set would
/// silently match an action judged under the new one. It is a string in the envelope rather than a
/// separate parameter so it cannot be forgotten at a call site.
pub const FINGERPRINT_FORMAT_VERSION: &str = "1";

/// The largest number of envelope fields one fingerprint may cover.
///
/// Bounded because the envelope is assembled from tool- and connector-supplied parts, and an unbounded
/// map is the shape that turns a metadata field into an unbounded allocation. The bound is generous
/// against the contract's own list (which names nine kinds of input) and the refusal is explicit rather
/// than a silent truncation, because a truncated envelope would fingerprint *fewer* facts than the
/// contract requires while still producing a plausible digest.
pub const MAX_FINGERPRINT_FIELDS: usize = 64;

/// The longest accepted envelope field name.
pub const MAX_FINGERPRINT_KEY_BYTES: usize = 128;

/// The longest accepted envelope field value.
///
/// Larger than a key because a value legitimately carries an arguments document. It is deliberately the
/// same magnitude as [`super::call::MAX_ARGUMENT_BYTES`] rather than a second, unrelated bound: an
/// envelope that could hold a value too large to be a tool argument would bound nothing meaningful.
pub const MAX_FINGERPRINT_VALUE_BYTES: usize = super::call::MAX_ARGUMENT_BYTES;

/// Why an envelope could not be used to compute a fingerprint.
///
/// A separate error rather than a [`DomainError`](crate::error::DomainError) variant because every case
/// is a **construction** fault in code JARVIS owns — the envelope is built by the executor, not sent by
/// a caller — and giving these their own type keeps them out of the client-visible error vocabulary
/// where they would be codes nobody could act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintError {
    /// A field name was empty, over-long, or contained a control character.
    UnusableKey,
    /// A field value was over-long or contained a NUL byte.
    UnusableValue,
    /// The envelope held more fields than the bound allows.
    TooManyFields,
}

impl FingerprintError {
    /// Returns a stable, namespaced code for diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnusableKey => "tool.fingerprint_key_invalid",
            Self::UnusableValue => "tool.fingerprint_value_invalid",
            Self::TooManyFields => "tool.fingerprint_too_many_fields",
        }
    }
}

impl std::fmt::Display for FingerprintError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::UnusableKey => "a fingerprint envelope field name is unusable",
            Self::UnusableValue => "a fingerprint envelope field value is unusable",
            Self::TooManyFields => "the fingerprint envelope has too many fields",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for FingerprintError {}

/// Returns whether `value` is a usable envelope field name.
///
/// Narrower than a value because a key names a field in a document JARVIS defined, so there is no reason
/// to accept control characters, whitespace, or an empty name. A key that could carry a control
/// character would be emitted into the canonical form escaped and then be **wrong in a way that is hard
/// to see** — two processes agreeing on a document whose keys nobody can read.
fn is_usable_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FINGERPRINT_KEY_BYTES
        && !value.contains('\0')
        && !value.chars().any(char::is_control)
}

/// Returns whether `value` is a usable envelope field value.
///
/// A value **may** contain a newline or a tab: an arguments document legitimately holds a multi-line
/// body, and the canonicalizer escapes those characters rather than dropping them, so refusing them here
/// would refuse an action a user could legitimately approve. A NUL is refused for the reason every other
/// boundary in this project refuses one — it is the byte that truncates a C string downstream.
fn is_usable_value(value: &str) -> bool {
    value.len() <= MAX_FINGERPRINT_VALUE_BYTES && !value.contains('\0')
}

/// The facts an envelope is built from.
///
/// **A struct rather than seven parameters**, following `ApprovalRequestParts` and the controller's own
/// grouped helpers: several of these are identifiers of the same shape, and a positional call would make a
/// principal/workspace transposition possible at the one call site that assembles a fingerprint.
#[derive(Debug, Clone, Copy)]
pub struct FingerprintParts<'a> {
    /// The format version [`FINGERPRINT_FORMAT_VERSION`] or a successor.
    pub version: &'a str,
    /// The exact tool identity being invoked.
    pub identity: &'a super::identity::ToolIdentity,
    /// The workspace the action happens in. Resolved server-side.
    pub workspace: crate::ids::WorkspaceId,
    /// The principal the action is made as.
    pub principal: crate::ids::PrincipalId,
    /// The tool's declared effects.
    ///
    /// **Not redundant with the identity, and that is the defect this field fixes.** `ToolIdentity`
    /// fingerprints the **input schema** only, so an effect reclassification does not move it: the same
    /// capability, source, and schema can go from `ReadOnly` to `Destructive` and the identity is
    /// unchanged. The contract nevertheless requires the fingerprint to cover "effects and constraints",
    /// and the tool fabric names "effect/risk classification" in what an approval binds to — because an
    /// approval granted for a read must not authorize a delete after the tool was reclassified. Effects
    /// are therefore covered here rather than assumed to be implied by the schema.
    pub effects: &'a [super::classification::Effect],
    /// The tool's declared risk, for the same reason as [`Self::effects`]: a risk reclassification leaves
    /// the identity untouched while changing what a user is agreeing to.
    pub risk: super::classification::Risk,
    /// The arguments the call is being made with.
    pub arguments: &'a super::call::ToolArguments,
}

/// The envelope a fingerprint is computed over: a flat object whose values are all strings.
///
/// **Every value is a `String`, and the absence of other types is the point.** RFC 8785's only
/// genuinely hard rule is number serialization, and this type makes a number impossible to contribute —
/// so the canonicalizer cannot be subtly wrong about one. Booleans, `null`, arrays, and nested objects
/// are excluded for the same reason, not because they would be difficult: a nested value would need the
/// recursive property sort, and the envelope is deliberately one level so the sort is over a set of
/// names rather than over a tree whose shape the fingerprint would then depend on.
///
/// A `BTreeMap` rather than a `Vec` of pairs, so a duplicate field name cannot exist. RFC 8785 §3.1
/// requires that "JSON objects MUST NOT exhibit duplicate property names", and with a map that is a
/// property of the type instead of a rule the canonicalizer would have to check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FingerprintInput {
    fields: BTreeMap<String, String>,
}

impl FingerprintInput {
    /// Builds an envelope from the action's own facts.
    ///
    /// # Errors
    ///
    /// Returns [`FingerprintError`] naming the rule that failed. Each refusal is a fault in the code
    /// that assembled the envelope rather than in a caller's request, because these values come from the
    /// tool identity, the run, and the arguments — all JARVIS's own.
    pub fn new(parts: FingerprintParts<'_>) -> Result<Self, FingerprintError> {
        let mut fields = BTreeMap::new();
        // The format version is first in the contract's list and is a **field rather than a parameter**
        // so a future change to the covered set changes every digest, which is what stops an old
        // approval from silently matching an action judged under a new field set.
        Self::insert(&mut fields, "fingerprint_version", parts.version)?;
        // The principal and workspace are part of the fingerprint because an approval is for one
        // principal acting in one workspace: the same tool call made by another principal is a different
        // action, and the contract lists both.
        Self::insert(&mut fields, "principal", &parts.principal.to_string())?;
        Self::insert(&mut fields, "workspace", &parts.workspace.to_string())?;
        // The canonical capability, the source (which carries the owner and version), and the schema
        // fingerprint — the three parts of the identity a grant or approval binds to. Recorded
        // separately rather than as one rendered tuple, so a reader of the envelope can see which part
        // changed, and so a change to the tuple's spelling does not silently change every fingerprint.
        Self::insert(
            &mut fields,
            "tool_capability",
            &parts.identity.capability.to_string(),
        )?;
        Self::insert(
            &mut fields,
            "tool_source",
            &parts.identity.source.to_string(),
        )?;
        Self::insert(
            &mut fields,
            "schema_fingerprint",
            &parts.identity.schema_fingerprint.to_string(),
        )?;
        // **The effects and the risk, because the identity does not carry them.** `schema_fingerprint`
        // covers the *input schema* alone, so a tool reclassified from read to destructive keeps its
        // identity and would otherwise keep its fingerprint — and an approval granted for the read would
        // authorize the delete. The contract requires "effects and constraints" in the fingerprinted
        // object, and this is the pair that makes the requirement true rather than assumed.
        //
        // Effects are **sorted and deduplicated through their contract spellings**, so the same *set*
        // produces one digest however the caller's list was ordered or whether it repeated an entry. The
        // list is not trusted to be canonical: `ToolDefinition::new` refuses duplicates, but a fingerprint
        // must not depend on a caller having gone through that constructor, because a fingerprint that
        // varied with list order would refuse an approval for the action the user actually saw.
        let mut effects: Vec<&str> = parts
            .effects
            .iter()
            .map(|effect| effect.as_contract_str())
            .collect();
        effects.sort_unstable();
        effects.dedup();
        Self::insert(&mut fields, "effects", &effects.join(","))?;
        Self::insert(&mut fields, "risk", parts.risk.as_contract_str())?;
        // **The arguments document is carried as text, not re-parsed.** The bytes are what the user was
        // shown in the approval preview, so binding them is binding the reviewed action; a
        // re-serialization would be a second definition of the action that could differ from it.
        Self::insert(&mut fields, "arguments", parts.arguments.as_str())?;
        if fields.len() > MAX_FINGERPRINT_FIELDS {
            return Err(FingerprintError::TooManyFields);
        }
        Ok(Self { fields })
    }

    /// Adds one field, refusing an unusable name or value.
    fn insert(
        fields: &mut BTreeMap<String, String>,
        key: &str,
        value: &str,
    ) -> Result<(), FingerprintError> {
        if !is_usable_key(key) || fields.contains_key(key) {
            return Err(FingerprintError::UnusableKey);
        }
        if !is_usable_value(value) {
            return Err(FingerprintError::UnusableValue);
        }
        if fields.len() >= MAX_FINGERPRINT_FIELDS {
            return Err(FingerprintError::TooManyFields);
        }
        let _ = fields.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    /// Returns the value recorded for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// Returns the number of fields.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Always `false` for an envelope built by [`Self::new`]. Present because a `len` without an
    /// `is_empty` is a lint, and answering `true` here would contradict the constructor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Builds an envelope from an already-validated field map.
    ///
    /// **For tests only, and it is a test seam rather than a public API** — the RFC's own sorting vector
    /// is a set of seven keys that are not JARVIS's own field names, so it cannot be built through
    /// [`Self::new`]. Exposed behind `#[cfg(test)]` so production code has exactly one constructor, which
    /// keeps "every field was validated" true of every envelope the daemon builds.
    #[cfg(test)]
    pub(crate) fn from_fields_for_tests(fields: BTreeMap<String, String>) -> Self {
        Self { fields }
    }

    /// Inserts one field through the real validation path, for tests.
    ///
    /// Returns the same refusals [`Self::new`] produces, so a bounds test asserts the **rule** rather
    /// than a copy of it.
    #[cfg(test)]
    pub(crate) fn insert_for_tests(
        fields: &mut BTreeMap<String, String>,
        key: &str,
        value: &str,
    ) -> Result<(), FingerprintError> {
        Self::insert(fields, key, value)
    }

    /// Returns the RFC 8785 canonical form of this envelope.
    ///
    /// The steps are §3.2.1 through §3.2.4 exactly: no whitespace, names and values escaped per
    /// §3.2.2.2, names sorted as UTF-16 code units per §3.2.3, and a UTF-8 result per §3.2.4. The
    /// property order here is already the sorted order, because the map is keyed on the name — but the
    /// **comparison** is what the RFC specifies, and a `BTreeMap`'s own order is Rust's byte-wise `Ord`
    /// on `String`, which differs from UTF-16 order for names outside the basic multilingual plane. So
    /// the names are ordered explicitly below rather than trusted to the map's iteration order.
    #[must_use]
    pub fn canonical_form(&self) -> String {
        let mut names: Vec<&String> = self.fields.keys().collect();
        names.sort_by(|left, right| compare_utf16(left, right));
        let mut out = String::with_capacity(self.fields.len() * 32);
        out.push('{');
        for (index, name) in names.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            escape_into(&mut out, name);
            out.push(':');
            // The value was validated as usable at insert, so the lookup cannot miss.
            escape_into(&mut out, &self.fields[*name]);
        }
        out.push('}');
        out
    }
}

/// Compares two names as arrays of UTF-16 code units, per RFC 8785 §3.2.3.
///
/// **This is not the same as comparing the strings' UTF-8 bytes**, which is what a `BTreeMap` does. The
/// RFC is explicit that sorting "is based on pure value comparisons, where code units are treated as
/// unsigned integers", and that a UTF-8 or UTF-32 sort "would also work" for determinism but "would
/// differ" from the specification. For pure ASCII the two agree, which is why a byte-wise implementation
/// looks correct until the first non-ASCII key — and then two processes disagree about one fingerprint,
/// which is the exact failure this module exists to prevent. So the comparison is exact rather than
/// fortunate.
fn compare_utf16(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left_units = left.encode_utf16();
    let mut right_units = right.encode_utf16();
    loop {
        match (left_units.next(), right_units.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            // The shorter string precedes when it is a prefix of the longer, which the RFC states as
            // "if there is no index position at which they differ, then the shorter string
            // lexicographically precedes the longer string".
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(left_unit), Some(right_unit)) => match left_unit.cmp(&right_unit) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            },
        }
    }
}

/// Appends `value` to `out` as a JSON string, per RFC 8785 §3.2.2.2.
///
/// The rules, each of which a plausible implementation gets wrong in a different way:
///
/// - `"` and `\` **must** be escaped. A literal quote or backslash ends or corrupts the document, so
///   this is the difference between valid JSON and a fingerprint over something else.
/// - The five characters with short forms are `U+0008`, `U+0009`, `U+000A`, `U+000C`, `U+000D`, and
///   they **must** use them (`\b \t \n \f \r`) — not `\u0008` and so on. A document that emitted a
///   control character as `\u000a` where `\n` is required is a *different byte string* and therefore a
///   different digest, so "it unescapes to the same character" is not good enough.
/// - Every other character in `U+0000..=U+001F` **must** be `\uhhhh` with **lowercase** hex. Uppercase
///   hex is the mistake that is invisible in a diff and changes every digest containing one.
/// - Every code point outside the control range is emitted **as-is**. This is what §3.2.2.2's "as is"
///   requires: `é` is not `\u00e9`, and a non-ASCII character must not be escaped.
fn escape_into(out: &mut String, value: &str) {
    use std::fmt::Write as _;
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            control if control < '\u{20}' => {
                // Lowercase, and exactly four hex digits: `u32::from(control)` is at most `0x1f`, so a
                // narrower format would be fine today and wrong the moment the range widened.
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// A one-way digest over an action, in the contract's `sha256:<hex>` form.
///
/// **The algorithm is part of the value**, exactly as it is for
/// [`SchemaFingerprint`](super::identity::SchemaFingerprint): the contract
/// writes a fingerprint as `sha256:...`, and requiring the prefix rather than accepting a bare digest
/// means a fingerprint produced by a different function cannot compare equal to one produced by this
/// one. Without the algorithm in the value, a switch of algorithm would silently invalidate — or
/// silently *preserve* — an existing approval, depending on which way the comparison fell.
///
/// This type is the **domain's** carrier: it validates the shape and compares. Computing one is an
/// adapter's job, because hashing is a concrete implementation and the domain layer must not depend on a
/// hashing crate — which is why `jarvis-infrastructure` owns the SHA-256 and this type is only ever built
/// from bytes an adapter produced or parsed from the contract's text form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionDigest([u8; 32]);

impl ActionDigest {
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
    /// Returns [`FingerprintError::UnusableValue`] when the prefix is missing or names another
    /// algorithm, or when the digest is not exactly 64 **lowercase** hex characters. Uppercase is refused
    /// rather than lowercased for the same reason identifiers reject it: two spellings of one digest
    /// would let a stored approval miss a match that is really the same action, and — worse in this
    /// direction — the two spellings would compare *unequal* for one action while looking identical to a
    /// reader.
    pub fn parse(value: &str) -> Result<Self, FingerprintError> {
        let hex = value
            .strip_prefix(Self::ALGORITHM)
            .and_then(|rest| rest.strip_prefix(':'))
            .ok_or(FingerprintError::UnusableValue)?;
        if hex.len() != Self::HEX_LENGTH
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(FingerprintError::UnusableValue);
        }
        let mut digest = [0_u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            let pair = &hex[index * 2..index * 2 + 2];
            *byte = u8::from_str_radix(pair, 16).map_err(|_| FingerprintError::UnusableValue)?;
        }
        Ok(Self(digest))
    }

    /// Returns the raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for ActionDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(Self::ALGORITHM)?;
        formatter.write_str(":")?;
        for byte in &self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl serde::Serialize for ActionDigest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for ActionDigest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        // **Through [`Self::parse`] rather than a derived impl**, so a fingerprint that arrives over the
        // wire is held to the same rule a stored one is. A derived impl on a newtype wraps the inner
        // value directly, which is the systemic defect `wire_validation_tests` records for nine other
        // types in this crate — an invalid digest would then be *stored* and later compared, which is a
        // check that silently stops applying rather than failing.
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}
