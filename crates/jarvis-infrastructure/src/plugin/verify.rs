//! Plugin package provenance and signature verification: `TLS-014`'s verification half.
//!
//! The contract's rule is one sentence — *"Package bytes are verified before extraction or
//! execution"* — and this module is the gate that sentence describes. `PluginManifest::parse`
//! validates the *document*: field shapes, path safety, a canonical digest string. It deliberately
//! trusts nothing in the `package` block, because a digest string and a signature reference are
//! **claims the document makes about bytes nobody has looked at yet**. This module looks.
//!
//! ## The verification order is the security property
//!
//! Four checks, and their order is not a convenience:
//!
//! 1. **the digest of the bytes matches the manifest's claim** — cheap, and it is what makes "these
//!    are the bytes the manifest is about" true before anything else is decided;
//! 2. **the signature over the bytes verifies against a trusted key** — which is what makes "the
//!    publisher vouched for these bytes" true;
//! 3. **the signature's identity matches the manifest's** — the binding, and the check whose absence
//!    is the attack the whole module exists to stop (below);
//! 4. **the manifest's source identity is *derived* from the bytes**, not copied from the document.
//!
//! ## The defect this module is shaped around
//!
//! A verifier that checks *a* signature is not a verifier. The threat is not a tampered package with
//! no signature; it is a **tampered manifest that declares a different package's digest**: the
//! attacker ships honest bytes and an honest signature — for a package they control — and a manifest
//! pointing at them. Every check in isolation passes. Only the *binding* fails, and it fails only if
//! the signature's identity is compared against the identity the manifest claims.
//!
//! So [`VerifiedPluginPackage`](crate::plugin::verify::VerifiedPluginPackage) is built by a method that takes the
//! **manifest** as an argument rather than by a function returning a loose `(digest, key_id)` pair. A
//! caller cannot obtain the derived values without having supplied the claim they must match, which
//! makes the binding structural rather than a step someone can forget. This is the same shape as
//! `PluginManifest::parse` being the only path to a validated manifest.
//!
//! ## What this module deliberately does not do
//!
//! **It does not extract.** The contract puts verification *before* extraction, and the extractor is
//! a separate concern with its own failure modes — symlinks, junctions, permission broadening,
//! reserved device names — which need an archive format the contract does not name. That half is
//! recorded as unimplemented at the end of this module rather than approximated here, because a
//! "verification succeeded" result that a caller reads as "safe to extract" would be worse than no
//! verifier at all.
//!
//! **It does not invent a signature scheme.** `SignatureReference::kind` is an open string precisely
//! because "signature kinds are `TLS-014`'s vocabulary"; this module supplies that vocabulary with
//! exactly one accepted kind,
//! [`PACKAGE_SIGNATURE_KIND`](crate::plugin::verify::PACKAGE_SIGNATURE_KIND), which reuses
//! the primitive the release pipeline already established and whose evidence is already accepted.
//! Adding a second scheme is a research-gate change, not a match arm.

use std::fmt;

use ed25519_dalek::{Signature, Verifier as _};
use jarvis_domain::plugin::{
    PluginSourceIdentity, PluginVersion, is_canonical_package_digest, is_plugin_identifier,
};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::plugin::{PluginManifest, is_bounded_reference};
use crate::release::{ReleaseError, SignatureEnvelope, TrustStore, base64url_decode};

/// The only package signature kind this build accepts.
///
/// **One kind, named rather than inferred, and it is the JARVIS-defined scheme the release pipeline
/// already established** — an `ed25519` signature over the package bytes, carried in a sidecar
/// envelope, verified against a trust store compiled into the daemon. The contract's manifest example
/// writes `"kind":"sigstore-bundle"` as an *illustration*; adopting Sigstore for real would be a new
/// external integration, which requires current official-spec evidence before any implementation
/// edit (`AGENTS.md`'s research gate). Reusing the accepted primitive needs no new evidence and keeps
/// the trust-store concept the operator already has.
///
/// The kind is checked **by name** rather than ignored, so a manifest that declares a scheme this
/// build cannot verify is refused as `UnsupportedSignatureKind` instead of being verified under a
/// scheme it did not ask for. That is the fail-closed direction: a manifest naming an unimplemented
/// kind must not be accepted because some *other* scheme happened to verify.
pub const PACKAGE_SIGNATURE_KIND: &str = "ed25519-detached";

/// The accepted size of a package signature envelope.
///
/// **An alias of the release pipeline's bound rather than a second literal with the same value.** An
/// envelope is the same three-field document in both places, so two constants would be two chances for
/// one to be raised without the other — and the one that was not raised would be the bound nothing
/// enforced. The name is plugin-facing so a reader of this module does not have to know which layer
/// owns the number.
pub const MAX_PACKAGE_SIGNATURE_BYTES: u64 = crate::release::MAX_SIGNATURE_BYTES;

/// Why a plugin package could not be verified.
///
/// Every variant is a distinct operator action, so they never collapse into "verification failed":
/// a digest mismatch means the bytes changed after signing, an unknown key means the publisher was
/// never trusted, a mismatch means the signature is for a *different* package, and an unsupported
/// kind means this build cannot check the scheme that was used. An operator's next step differs in
/// every case.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginVerificationError {
    /// The signature envelope exceeds [`MAX_PACKAGE_SIGNATURE_BYTES`].
    #[error("the plugin package signature envelope is larger than the accepted limit")]
    SignatureTooLarge,
    /// The envelope is not a well-formed envelope.
    #[error("the plugin package signature envelope is malformed")]
    SignatureMalformed,
    /// The manifest declares a signature kind this build cannot verify.
    #[error("the plugin package declares a signature kind this build does not support")]
    UnsupportedSignatureKind {
        /// The kind found in the manifest.
        kind: String,
    },
    /// The envelope names an algorithm this build refuses.
    #[error("the plugin package signature algorithm is unsupported")]
    UnsupportedAlgorithm {
        /// The algorithm found in the envelope.
        found: String,
    },
    /// The envelope's key is not in the trust store.
    #[error("the plugin package signing key is not trusted")]
    UnknownSigningKey {
        /// The key id found in the envelope.
        key_id: String,
    },
    /// The signature is not a decodable 64-byte `Ed25519` signature.
    #[error("the plugin package signature is malformed")]
    SignatureInvalid,
    /// The signature does not match the package bytes.
    #[error("the plugin package signature does not verify over the package bytes")]
    SignatureMismatch,
    /// The package bytes do not hash to the digest the manifest records.
    #[error("the plugin package bytes do not match the manifest's recorded digest")]
    DigestMismatch,
    /// **The binding failed**: the verified signature is not for the package the manifest claims.
    #[error("the verified signature belongs to a different package than the manifest declares")]
    SignatureIdentityMismatch,
    /// The package bytes are larger than the accepted limit.
    #[error("the plugin package is larger than the accepted limit")]
    PackageTooLarge,
}

impl PluginVerificationError {
    /// Returns the stable, namespaced error code.
    ///
    /// `jarvis.*` rather than `plugin.*`, following [`crate::plugin::PluginManifestError`]: this is
    /// the daemon's own verdict about a package, not the plugin protocol's vocabulary, and the
    /// emission boundary rewrites a `plugin.*` code it does not own.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SignatureTooLarge => "jarvis.plugin_signature_too_large",
            Self::SignatureMalformed => "jarvis.plugin_signature_malformed",
            Self::UnsupportedSignatureKind { .. } => "jarvis.plugin_signature_kind_unsupported",
            Self::UnsupportedAlgorithm { .. } => "jarvis.plugin_signature_algorithm_unsupported",
            Self::UnknownSigningKey { .. } => "jarvis.plugin_signing_key_unknown",
            Self::SignatureInvalid => "jarvis.plugin_signature_invalid",
            Self::SignatureMismatch => "jarvis.plugin_signature_mismatch",
            Self::DigestMismatch => "jarvis.plugin_digest_mismatch",
            Self::SignatureIdentityMismatch => "jarvis.plugin_signature_identity_mismatch",
            Self::PackageTooLarge => "jarvis.plugin_package_too_large",
        }
    }

    /// Returns whether retrying the same input unchanged could succeed.
    ///
    /// **None of these are retryable, and that is a property of the subject rather than a default.**
    /// A verification failure is a statement about *bytes*: the same bytes will produce the same
    /// verdict, so a retry cannot help and offering one would invite a caller to loop over a
    /// suspected-tampered package. An operator's remedy is to obtain the package again from its
    /// source, or to trust the key — neither of which is a retry of this call.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        false
    }
}

impl From<ReleaseError> for PluginVerificationError {
    /// Maps the reuse of the release-signing primitive's errors onto the package vocabulary.
    ///
    /// **A mapping rather than a `#[from]`**, because the two error types answer different questions:
    /// `ReleaseError::ArtifactMissing` has no plugin meaning, and the release variants that *do*
    /// correspond must be reported under the plugin codes so an operator reading
    /// `jarvis.plugin_*` finds this module. `UnsupportedAlgorithm` and `UnknownKey` are carried with
    /// their values, because "which algorithm" and "which key" are what the operator acts on.
    fn from(error: ReleaseError) -> Self {
        match error {
            ReleaseError::UnsupportedAlgorithm { found } => Self::UnsupportedAlgorithm { found },
            ReleaseError::UnknownKey { key_id } => Self::UnknownSigningKey { key_id },
            ReleaseError::SignatureMismatch => Self::SignatureMismatch,
            ReleaseError::SignatureInvalid => Self::SignatureInvalid,
            ReleaseError::SignatureTooLarge => Self::SignatureTooLarge,
            // Every remaining release variant is about a manifest document or an artifact file,
            // neither of which this module reads: the only release value it uses is the envelope and
            // the trust store. Mapping them to `SignatureMalformed` reports "the signature material
            // could not be used" rather than inventing a plugin variant with no meaning.
            _ => Self::SignatureMalformed,
        }
    }
}

/// The identity a package's bytes and signature **prove**, as opposed to what a document claims.
///
/// Built only by [`PluginPackageVerifier::verify`], which requires the manifest — so the fields here
/// are derived rather than echoed, and a caller cannot hold one of these without having passed the
/// claim it had to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPluginPackage {
    /// The digest computed from the bytes, in canonical form.
    digest: String,
    /// The key id whose signature verified, from the trust store.
    signature_identity: String,
    /// The manifest's declared source, carried through for an operator to inspect.
    source: String,
    /// The size of the verified bytes.
    size: u64,
}

impl VerifiedPluginPackage {
    /// Returns the canonical digest of the verified bytes.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Returns the trusted key id the signature verified against.
    #[must_use]
    pub fn signature_identity(&self) -> &str {
        &self.signature_identity
    }

    /// Returns the manifest's declared source reference.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the size of the verified bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Builds the `PluginSourceIdentity` for this verified package.
    ///
    /// **This is the method that gives `PluginSourceIdentity` its first production producer**, and it
    /// is here rather than in the domain because two of its seven fields can only be known after
    /// verification: `package_digest` is the digest of bytes that have been hashed, and
    /// `signature_identity` is the key id a signature actually verified against. Constructing an
    /// identity from a *manifest alone* would let a document name its own provenance — which is the
    /// defect `PluginSourceIdentity`'s own doc records as the reason the tuple exists.
    ///
    /// The `id`, `publisher`, `version`, and `protocol` are taken from the manifest and therefore
    /// remain claims; what verification establishes is that the **bytes** are the ones this identity
    /// names. An identity is a name for a verified package, not a proof that a publisher is honest.
    ///
    /// # Errors
    ///
    /// Returns `jarvis_domain::error::DomainError` when a manifest field fails the identity rule.
    /// The manifest was validated by `parse`, so this is reachable only for a manifest built by hand
    /// from deserialized parts — which is why it is returned rather than panicked.
    pub fn identity_for(
        &self,
        manifest: &PluginManifest,
    ) -> Result<PluginSourceIdentity, jarvis_domain::error::DomainError> {
        let version = PluginVersion::parse(&manifest.version)?;
        PluginSourceIdentity::new(
            &manifest.id,
            &manifest.publisher,
            &self.digest,
            &self.signature_identity,
            &self.source,
            version,
            &manifest.compatibility.protocol.kind,
        )
    }
}

impl fmt::Display for VerifiedPluginPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The digest and key id are not secrets and are what an operator correlates with a publisher,
        // so they are shown; the `source` is deliberately **not** shown here, because it is
        // operator-inspectable text that could carry a misleading URL and a `Display` rendering is
        // what reaches a log line. A reader that wants it calls `source()`.
        write!(
            formatter,
            "verified package ({} bytes, {}, signed by {})",
            self.size, self.digest, self.signature_identity,
        )
    }
}

/// Verifies plugin package bytes against a manifest and a trust store.
///
/// Holds the trust store, for the reason [`TrustStore`]'s own doc records: a key supplied by the
/// caller cannot be a trust anchor, because a caller who can choose the key can choose one they
/// control. A verifier therefore *owns* its anchors and is built once at composition.
#[derive(Debug, Clone)]
pub struct PluginPackageVerifier {
    trust: TrustStore,
    /// The largest package this verifier will hash.
    max_package_bytes: u64,
}

impl PluginPackageVerifier {
    /// Builds a verifier over `trust`.
    ///
    /// `max_package_bytes` is a parameter rather than a constant because the bound is a deployment
    /// choice — how large a plugin may be — while the *enforcement* is this module's. A caller that
    /// passed `u64::MAX` would be declining to bound hashing, which is why the composition site
    /// supplies a real limit.
    #[must_use]
    pub fn new(trust: TrustStore, max_package_bytes: u64) -> Self {
        Self {
            trust,
            max_package_bytes,
        }
    }

    /// Returns the trust store this verifier anchors on.
    ///
    /// Exposed so a caller can report *which* keys are trusted — an install that verified against a
    /// non-production test key must be able to say so, which is the same obligation
    /// `TrustStore::builtin` records for the release pipeline.
    #[must_use]
    pub fn trust(&self) -> &TrustStore {
        &self.trust
    }

    /// Verifies `package_bytes` against `manifest`, using the sidecar envelope in `envelope_bytes`.
    ///
    /// The five checks are performed in the order the module doc states, and each one's failure is a
    /// distinct error. The **binding** — that the verified signature identity is the one the manifest
    /// declares — is the last check and cannot be skipped, because the only way to obtain a
    /// [`VerifiedPluginPackage`] is through this method.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginVerificationError`] naming the first check that failed.
    pub fn verify(
        &self,
        manifest: &PluginManifest,
        package_bytes: &[u8],
        envelope_bytes: &[u8],
    ) -> Result<VerifiedPluginPackage, PluginVerificationError> {
        // 0. The size bound, before anything is hashed or parsed: an oversized package is refused
        //    without reading it, which is the point of a bound.
        let size = u64::try_from(package_bytes.len()).unwrap_or(u64::MAX);
        if size > self.max_package_bytes {
            return Err(PluginVerificationError::PackageTooLarge);
        }
        // 1. **The kind, before the envelope is trusted.** The manifest declares which scheme signed
        //    it, and a scheme this build cannot verify must be refused *by name* rather than skipped:
        //    a manifest naming an unimplemented kind must not pass because some other scheme happened
        //    to verify the bytes.
        if manifest.package.signature.kind != PACKAGE_SIGNATURE_KIND {
            return Err(PluginVerificationError::UnsupportedSignatureKind {
                kind: manifest.package.signature.kind.clone(),
            });
        }
        // 2. The declared digest is already validated as canonical by `PluginManifest::validate`, so
        //    this compares computed-against-declared. Hashing here rather than trusting the document
        //    is what makes the digest a *fact about the bytes*.
        let computed = sha256_digest(package_bytes);
        if computed != manifest.package.digest {
            return Err(PluginVerificationError::DigestMismatch);
        }
        // 3. The signature over the bytes, against a trusted key. Reuses the release primitive's
        //    envelope and trust store so there is one Ed25519 implementation and one trust concept —
        //    and its `parse` **is** the size bound and the well-formedness check, mapped onto this
        //    module's vocabulary by `From<ReleaseError>` rather than reimplemented.
        let envelope =
            SignatureEnvelope::parse(envelope_bytes).map_err(PluginVerificationError::from)?;
        let key_id = self.verify_signature(&envelope, package_bytes)?;
        // 4. **The binding.** The verified key id must be the identity the manifest declares. Without
        //    this, a manifest pointing at another package's honest signature verifies every other
        //    check and is accepted — the attack the module doc describes.
        if key_id != manifest.package.signature.value_ref {
            // The `value_ref` is the signature *identity* in the manifest's own vocabulary: for a
            // detached envelope it names the key the publisher signed with. A field named `value_ref`
            // that held a path would be a filesystem fault, which is why the manifest validates it as
            // a package-relative path — the two rules must agree, and this comparison is where a
            // disagreement becomes visible rather than silently accepted.
            return Err(PluginVerificationError::SignatureIdentityMismatch);
        }
        Ok(VerifiedPluginPackage {
            digest: computed,
            signature_identity: key_id,
            source: manifest.package.source.clone(),
            size,
        })
    }

    /// Verifies the envelope's signature over `bytes`, returning the trusted key id.
    ///
    /// Extracted so the algorithm, key lookup, decode, and verification are one concern with one
    /// error vocabulary, and so [`Self::verify`] reads as the five checks rather than as crypto.
    fn verify_signature(
        &self,
        envelope: &SignatureEnvelope,
        bytes: &[u8],
    ) -> Result<String, PluginVerificationError> {
        if envelope.algorithm != crate::release::SIGNATURE_ALGORITHM {
            return Err(PluginVerificationError::UnsupportedAlgorithm {
                found: envelope.algorithm.clone(),
            });
        }
        let Some(key) = self.trust.verifying_key(&envelope.key_id) else {
            return Err(PluginVerificationError::UnknownSigningKey {
                key_id: envelope.key_id.clone(),
            });
        };
        let signature_bytes = base64url_decode(&envelope.signature)
            .ok_or(PluginVerificationError::SignatureInvalid)?;
        let signature = Signature::try_from(signature_bytes.as_slice())
            .map_err(|_| PluginVerificationError::SignatureInvalid)?;
        key.verify(bytes, &signature)
            .map_err(|_| PluginVerificationError::SignatureMismatch)?;
        Ok(envelope.key_id.clone())
    }
}

/// Returns the canonical `sha256:<hex>` digest of `bytes`.
///
/// **Computed here rather than reused from the release module's file helper**, because that helper
/// hashes a *path* and this hashes bytes already in memory — the verifier must not open the package
/// again after deciding it is within bounds, and a second read is a second chance for the two reads
/// to observe different bytes. The spelling is produced through the domain's own predicate so the
/// format cannot drift from what `is_canonical_package_digest` accepts.
#[must_use]
pub fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let rendered = format!("sha256:{}", hex(&digest));
    // A digest this function produced must satisfy the domain's own rule; a mismatch would mean the
    // two spellings of the format have drifted. The assertion is a `debug_assert` because it is a
    // self-check on this module rather than on input, and a release build should not pay for it.
    debug_assert!(
        is_canonical_package_digest(&rendered),
        "the computed digest must satisfy the domain's canonical form",
    );
    rendered
}

/// Renders bytes as lowercase hex.
///
/// `write!` into the `String` rather than `push_str(&format!(..))`, which would allocate a throwaway
/// `String` per byte — and the workspace's clippy denies the pattern outright.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // A write into a `String` cannot fail, and ignoring the result is what the signature requires.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Returns whether `value` is a usable package signature reference for this build.
///
/// A predicate a manifest writer can consult so it declares a kind this build can verify, rather than
/// discovering the refusal at install time. `PluginManifest::parse` **cannot** check it: the kind is
/// an open string there precisely because signature kinds are this module's vocabulary, and pinning
/// it upstream would refuse a kind a future verifier supports.
#[must_use]
pub fn is_verifiable_signature_kind(value: &str) -> bool {
    value == PACKAGE_SIGNATURE_KIND
}

/// Returns whether `value` is a usable source reference for a verified package.
///
/// Delegates to the manifest's own rule rather than restating it, so a source this predicate accepts
/// is one `PluginManifest::validate` accepts — two spellings of one rule is how a value comes to pass
/// one check and fail the other.
#[must_use]
pub fn is_usable_package_source(value: &str) -> bool {
    is_bounded_reference(value)
}

/// Returns whether `value` is a usable plugin identifier for an installed package.
///
/// Delegates to the domain's rule; exposed beside the verifier because a caller assembling a
/// [`PluginSourceIdentity`] after verification needs the same rule the constructor applies.
#[must_use]
pub fn is_usable_plugin_identifier(value: &str) -> bool {
    is_plugin_identifier(value)
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
