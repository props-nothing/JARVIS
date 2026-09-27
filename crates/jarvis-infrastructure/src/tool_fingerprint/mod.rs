//! The SHA-256 half of the action fingerprint.
//!
//! `jarvis_domain::tool::canonical` owns the RFC 8785 canonical form and the digest's **shape**; this
//! module owns the **computation**, because hashing is a concrete implementation and the domain layer must
//! not depend on a hashing crate. That split is the same one `SchemaFingerprint` records, and it is the
//! reason `TLS-005` could say "the fingerprint is a type without a computation" honestly for several
//! rounds: the type existed and the function that produced one did not.
//!
//! **The pipeline is three steps and each is one function**, so a caller cannot perform two of them:
//!
//! ```text
//! FingerprintInput::canonical_form()   ->  the RFC 8785 bytes
//! action_digest_of(canonical)          ->  ActionDigest
//! ```
//!
//! `action_digest_of` takes the **canonical document** rather than the envelope, deliberately: a caller
//! that hashed something else — `serde_json`'s default output, say — would produce a digest that is stable
//! inside this build and different from every other implementation, which is the failure the contract's
//! cross-language round-trip requirement exists to catch. Taking the canonical form's own output means the
//! only way to reach a digest is through the canonicalization.
//!
//! **SHA-256 is used, not merely available.** The credential verifier already uses it for a different
//! purpose (`jarvis_infrastructure::auth::credential`), and the digest here is domain-separated by the
//! `sha256:` prefix and by the envelope's own `fingerprint_version` field — so a value computed for one
//! purpose cannot be presented as one computed for the other.

use sha2::{Digest as _, Sha256};

use jarvis_domain::tool::canonical::ActionDigest;

/// Computes the action digest of an RFC 8785 canonical document.
///
/// # Contract
///
/// `canonical` must be the output of
/// [`FingerprintInput::canonical_form`](jarvis_domain::tool::canonical::FingerprintInput::canonical_form).
/// The parameter type is a `&str` rather than the envelope because the boundary between canonicalization
/// and hashing is where a second canonicalization could be introduced, and passing the bytes across it
/// keeps the two steps visibly separate. `jarvis_infrastructure`'s `action_fingerprint` is the function a
/// caller should normally use: it performs both steps and cannot be called with anything but an envelope.
#[must_use]
pub fn action_digest_of(canonical: &str) -> ActionDigest {
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    ActionDigest::from_bytes(digest)
}

/// Computes the action fingerprint of an envelope: canonicalize, then hash.
///
/// **This is the function to call**, and having one is what stops a caller from performing only the
/// canonicalization or only the hash. It is infallible because
/// [`FingerprintInput`](jarvis_domain::tool::canonical::FingerprintInput) validates every field as it is
/// inserted, so an envelope that exists is one that can be fingerprinted — a `Result` here would force
/// every call site to handle a case the type system already excludes.
#[must_use]
pub fn action_fingerprint(
    input: &jarvis_domain::tool::canonical::FingerprintInput,
) -> ActionDigest {
    action_digest_of(&input.canonical_form())
}

/// Computes the fingerprint of a tool's **input schema**.
///
/// **This is the computation `SchemaFingerprint`'s own doc claimed and did not have.** That type says
/// "computing it from a document is an adapter's job … `jarvis-infrastructure` provides the computation",
/// and until this function the only construction paths were `from_bytes` in test fixtures — so every tool
/// identity in the product carried a fingerprint a *test* wrote, and a schema change could not move the
/// identity because nothing derived one. `ACC-024` and the tool contract both depend on that derivation:
/// "a release that alters the input schema changes the fingerprint and therefore the identity".
///
/// **The document is fingerprinted as the bytes given, not re-serialized**, and that is the same rule the
/// action fingerprint follows for arguments: re-encoding would make the digest depend on a serializer's
/// choice of key order, whitespace, and number form, so two JARVIS processes — or a JARVIS process and a
/// published IDL — would derive different fingerprints for one schema. The caller supplies the schema's
/// canonical text and owns that choice; this function does not invent one.
///
/// The digest is **domain-separated** with a `tool-schema:` prefix before hashing, so a schema document
/// whose bytes happened to equal an action's canonical form cannot produce the same digest as that action.
/// Domain separation is cheap here and the collision it prevents is the one that would let a schema
/// fingerprint be presented as an action fingerprint.
#[must_use]
pub fn schema_fingerprint_of(schema: &str) -> jarvis_domain::tool::identity::SchemaFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(SCHEMA_DOMAIN_SEPARATOR.as_bytes());
    hasher.update(schema.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    jarvis_domain::tool::identity::SchemaFingerprint::from_bytes(digest)
}

/// The domain separator for a schema fingerprint.
///
/// A `const` rather than an inline literal so its bytes are stated once, and with a trailing NUL that no
/// JSON document can begin with (JSON text starts with `{`, `[`, a quote, a digit, `t`, `f`, or `n`), so
/// the prefix cannot be confused with a document that merely starts with the same letters.
pub const SCHEMA_DOMAIN_SEPARATOR: &str = "tool-schema:\0";

#[cfg(test)]
mod tests;
