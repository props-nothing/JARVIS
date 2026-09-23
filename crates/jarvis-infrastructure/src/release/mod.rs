//! Release artifact verification: signed manifests and `SHA-256` digests.
//!
//! A release manifest is the document a user trusts to decide **which bytes to
//! execute**. If it is unsigned, transport security is the only thing standing
//! between a user and an altered artifact, and that protection ends the moment
//! the bytes leave the server. This module is the control that makes the
//! manifest and the artifact authentic, and it is deliberately a pure function
//! so a client, an installer, and CI all verify with the same code.
//!
//! ## The two checks, and why both are needed
//!
//! 1. **Manifest signature.** The `Ed25519` signature is verified over the exact
//!    bytes of the manifest document, against a public key from a trust store
//!    compiled into the binary. A digest alone proves nothing: an attacker who
//!    can rewrite an artifact can rewrite the digest beside it. The signature is
//!    what binds the digest to a key JARVIS trusts.
//! 2. **Artifact digest and size.** Each listed artifact is hashed and compared.
//!    This is what catches a truncated download, a stale mirror, and a
//!    byte-level modification of the payload even though the manifest is valid.
//!
//! ## Why the signature covers raw bytes, not a re-serialized value
//!
//! The signature is computed over the manifest document's **exact bytes**, never
//! over a parse-then-re-serialize round trip. A canonicalizing verifier silently
//! depends on both sides agreeing about key order, whitespace, and numeric form,
//! and the failure mode is a signature that verifies for one writer and not
//! another. Detached-over-bytes has no such ambiguity. Both the signer and the
//! verifier here use `manifest_bytes` / [`sign_manifest`][sign_manifest] from this module, so
//! the two sides cannot drift apart.
//!
//! ## Trust anchor
//!
//! Signing keys never live in the repository. The trust store is compiled in, and
//! a key passed as a command-line argument is **not** a trust anchor — a caller
//! who supplies the key can supply one they own, which proves nothing. Until
//! `OWN-003` establishes production identities, the built-in store contains only
//! the committed **non-production test key**, and the CLI says so loudly. A test
//! key is committed only because it has no trust outside tests and the artifacts
//! it signs are visibly marked non-production.

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// The manifest schema version this build writes and understands.
pub const RELEASE_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// The accepted size of a manifest document.
///
/// A manifest is a small fixed record. Bounding it means a hostile document
/// cannot exhaust memory before it is rejected.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// The accepted size of a signature envelope.
pub const MAX_SIGNATURE_BYTES: u64 = 8 * 1024;

/// The only signature algorithm this build accepts.
///
/// It is carried in the envelope so an algorithm downgrade or an unknown future
/// algorithm fails closed instead of being ignored.
pub const SIGNATURE_ALGORITHM: &str = "ed25519";

/// The maximum number of artifacts one manifest may list.
pub const MAX_ARTIFACTS: usize = 64;

/// The block size used while hashing an artifact.
///
/// Artifacts are streamed rather than read whole, so verifying a large installer
/// does not require memory proportional to its size.
pub const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// The key id of the committed **non-production** test key.
///
/// Anything signed with this key is a test artifact. It exists so the
/// verification mechanism is provable end to end before `OWN-003` establishes
/// production signing identities.
pub const TEST_KEY_ID: &str = "jarvis-test-release-1";

/// The `Ed25519` public half of [`TEST_KEY_ID`], as unpadded `base64url`.
///
/// This is a public key. Its private half is committed too, and that is
/// deliberate and safe: it has no trust, it signs only visibly-marked test
/// artifacts, and its presence is what lets CI prove verification without a
/// production secret. Production private keys must never be committed.
pub const TEST_PUBLIC_KEY: &str = "3-FGSAT2OcWUn4B6gNgOPsa0boxEP8g7h11d9O7N0Mo";

/// The `Ed25519` secret half of [`TEST_KEY_ID`], as unpadded `base64url`.
///
/// **This is a committed private key, and that is a deliberate, bounded
/// decision.** It is safe only because every property below holds, and it must be
/// deleted the moment any of them stops holding:
///
/// - it has no trust anywhere — [`TrustStore::builtin`] is the only consumer, and
///   a verification against it is reported as a test-key verification;
/// - every artifact it signs is visibly marked non-production, so a test-signed
///   release cannot be confused with a real one;
/// - it authorizes nothing: it cannot sign an update for an installed product,
///   because no released binary trusts it for production channels;
/// - its compromise costs nothing, because it protects nothing.
///
/// Production signing identities are `OWN-003`. A production private key must
/// never appear in this repository, in CI variables, in logs, or in artifacts.
pub const TEST_SECRET_KEY: &str = "Rs7txyASQM0Qv3dIRV59U6tXDX582g0lJ_msequ9Qw4";

/// The publication channel a release belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    /// The default channel for general use.
    Stable,
    /// An opt-in channel that may change behavior between builds.
    Preview,
    /// An opt-in channel for unreleased work.
    Development,
}

/// One published artifact and the digest that identifies its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEntry {
    /// The platform target, for example `x86_64-pc-windows-msvc`.
    pub target: String,
    /// The artifact kind, for example `archive` or `installer`.
    pub kind: String,
    /// The published download file name, relative to the manifest and never a path.
    pub file: String,
    /// The file name this artifact is installed **as**.
    ///
    /// A published download is usually version-named (`jarvisd0.1.0-x86_64.exe`),
    /// but an installed program must have a stable name, because a service
    /// definition points at it and an update must not leave the service starting a
    /// stale version-named path. The installed name is therefore recorded in the
    /// signed document rather than derived from the download name by string
    /// matching, which would break the first time a naming convention changed.
    pub name: String,
    /// The lowercase hex `SHA-256` of the artifact bytes.
    pub sha256: String,
    /// The exact artifact size in bytes.
    pub size: u64,
}

/// The publication record: what was released, from what build, and its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    /// The manifest schema version.
    pub schema_version: u32,
    /// The semantic version of the release.
    pub version: String,
    /// The channel this release was published on.
    pub channel: Channel,
    /// The primary target this manifest describes.
    pub target: String,
    /// The local API major version this build serves.
    pub api_major: u32,
    /// The minimum data/schema version this build can read.
    ///
    /// An older binary uses this to refuse newer state rather than rewriting it.
    pub min_data_version: u32,
    /// The build identity, normally the commit the artifacts were built from.
    pub build: String,
    /// The publication instant, RFC 3339 UTC with `Z`.
    pub published_at: String,
    /// The published artifacts.
    pub artifacts: Vec<ArtifactEntry>,
}

/// The detached signature over a manifest document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEnvelope {
    /// The signature algorithm. Must equal [`SIGNATURE_ALGORITHM`].
    pub algorithm: String,
    /// The key id the verifier must find in its trust store.
    pub key_id: String,
    /// The unpadded `base64url` signature over the manifest document bytes.
    pub signature: String,
}

/// Why a release could not be verified.
///
/// Every variant is a distinct, actionable cause: "the signature is invalid" and
/// "I do not trust that key" need completely different operator responses, so
/// they must never collapse into one message.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReleaseError {
    /// The manifest document exceeds [`MAX_MANIFEST_BYTES`].
    #[error("the release manifest is larger than the accepted limit")]
    ManifestTooLarge,
    /// The signature envelope exceeds [`MAX_SIGNATURE_BYTES`].
    #[error("the signature envelope is larger than the accepted limit")]
    SignatureTooLarge,
    /// The document is not valid `JSON` or has an unknown field.
    #[error("the release manifest is malformed")]
    ManifestMalformed,
    /// The signature envelope is not valid `JSON`.
    #[error("the signature envelope is malformed")]
    SignatureMalformed,
    /// The schema version is not one this build understands.
    #[error("the release manifest schema version is unsupported by this binary")]
    UnsupportedSchemaVersion {
        /// The version found in the document.
        found: u32,
    },
    /// The envelope names an algorithm this build refuses.
    #[error("the release signature algorithm is unsupported")]
    UnsupportedAlgorithm {
        /// The algorithm found in the envelope.
        found: String,
    },
    /// The envelope names a key that is not in the trust store.
    #[error("the release signing key is not trusted")]
    UnknownKey {
        /// The key id found in the envelope.
        key_id: String,
    },
    /// The signature is not a decodable 64-byte `Ed25519` signature.
    #[error("the release signature is malformed")]
    SignatureInvalid,
    /// The signature does not match the manifest bytes.
    #[error("the release manifest signature does not verify")]
    SignatureMismatch,
    /// The manifest lists no artifacts.
    #[error("the release manifest lists no artifacts")]
    NoArtifacts,
    /// Two entries name the same artifact file.
    #[error("the release manifest lists an artifact more than once")]
    DuplicateArtifact {
        /// The repeated file name.
        file: String,
    },
    /// An entry's file name is absolute, a path, or otherwise unsafe.
    #[error("a release artifact name is not a plain file name")]
    UnsafeArtifactName {
        /// The rejected name, which is operator input to correct.
        file: String,
    },
    /// An entry's digest is not 64 lowercase hex characters.
    #[error("a release artifact digest is not a canonical lowercase SHA-256")]
    InvalidDigest {
        /// The artifact whose digest was rejected.
        file: String,
    },
    /// A required manifest field is empty.
    #[error("a required release manifest field is empty")]
    EmptyField {
        /// The field name.
        field: &'static str,
    },
    /// The publication timestamp is not an RFC 3339 UTC `Z` instant.
    #[error("the release publication timestamp is not RFC 3339 UTC")]
    InvalidTimestamp,
    /// The artifact is not present beside the manifest.
    #[error("a listed release artifact is missing")]
    ArtifactMissing {
        /// The artifact file name.
        file: String,
    },
    /// The artifact exists but could not be read.
    #[error("a listed release artifact could not be read")]
    ArtifactUnreadable {
        /// The artifact file name.
        file: String,
    },
    /// The artifact bytes do not hash to the manifest's digest.
    #[error("a release artifact does not match its recorded digest")]
    DigestMismatch {
        /// The artifact file name.
        file: String,
    },
    /// The artifact size does not match the manifest.
    #[error("a release artifact does not match its recorded size")]
    SizeMismatch {
        /// The artifact file name.
        file: String,
    },
}

impl ReleaseError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ManifestTooLarge => "jarvis.release_manifest_too_large",
            Self::SignatureTooLarge => "jarvis.release_signature_too_large",
            Self::ManifestMalformed => "jarvis.release_manifest_malformed",
            Self::SignatureMalformed => "jarvis.release_signature_malformed",
            Self::UnsupportedSchemaVersion { .. } => "jarvis.release_schema_unsupported",
            Self::UnsupportedAlgorithm { .. } => "jarvis.release_algorithm_unsupported",
            Self::UnknownKey { .. } => "jarvis.release_key_unknown",
            Self::SignatureInvalid => "jarvis.release_signature_invalid",
            Self::SignatureMismatch => "jarvis.release_signature_mismatch",
            Self::NoArtifacts => "jarvis.release_no_artifacts",
            Self::DuplicateArtifact { .. } => "jarvis.release_duplicate_artifact",
            Self::UnsafeArtifactName { .. } => "jarvis.release_unsafe_artifact_name",
            Self::InvalidDigest { .. } => "jarvis.release_invalid_digest",
            Self::EmptyField { .. } => "jarvis.release_empty_field",
            Self::InvalidTimestamp => "jarvis.release_invalid_timestamp",
            Self::ArtifactMissing { .. } => "jarvis.release_artifact_missing",
            Self::ArtifactUnreadable { .. } => "jarvis.release_artifact_unreadable",
            Self::DigestMismatch { .. } => "jarvis.release_digest_mismatch",
            Self::SizeMismatch { .. } => "jarvis.release_size_mismatch",
        }
    }

    /// Returns whether verifying again, unchanged, could succeed.
    ///
    /// A digest or signature mismatch is deterministic and is never fixed by
    /// retrying; retrying it only re-downloads the same altered bytes and can
    /// turn a clear tamper signal into an ambiguous timeout. An unreadable
    /// artifact is retryable because the cause may be a transient filesystem
    /// condition on the *local* machine.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::ArtifactUnreadable { .. })
    }
}

/// Returns `true` when `value` is 64 lowercase hexadecimal characters.
///
/// Uppercase is rejected rather than normalized. Accepting both spellings of one
/// digest is how two different documents end up describing the same bytes.
#[must_use]
pub fn is_canonical_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Returns `true` when `file` is a plain file name and not a path.
///
/// A traversal (`../`), an absolute path, a Windows drive or alternate-data-stream
/// colon, a separator, a leading dot, or a control character is refused. The
/// artifact name comes from a document that is verified *after* it is parsed, so
/// it is treated as untrusted input at the point it would become a filesystem
/// access.
#[must_use]
pub fn is_plain_file_name(file: &str) -> bool {
    !file.is_empty()
        && !file.starts_with('.')
        && !file.contains("..")
        && !file
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'\\' | b':') || byte < 0x20 || byte == 0x7f)
}

/// Returns `true` when `value` looks like a semantic version.
///
/// This is a charset and shape check, not a full `SemVer` parse. It exists so a
/// hostile version string cannot carry control characters or newlines into an
/// operator's terminal or a log line.
#[must_use]
pub fn is_version_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.starts_with(|character: char| character.is_ascii_digit())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// Returns `true` when `value` is `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Only the one canonical form is accepted, matching the rest of the codebase: an
/// offset form or a missing `Z` is a second spelling of the same instant and is
/// rejected rather than normalized.
#[must_use]
pub fn is_rfc3339_utc(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 {
        return false;
    }
    let digits_at = |indices: &[usize]| indices.iter().all(|i| bytes[*i].is_ascii_digit());
    digits_at(&[0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18])
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z'
}

impl ReleaseManifest {
    /// Parses and validates a manifest document without verifying a signature.
    ///
    /// A trusted manifest must additionally pass
    /// [`TrustStore::verify_manifest`]. This function exists separately so the
    /// document shape can be validated independently of the trust decision.
    ///
    /// # Errors
    ///
    /// Returns the [`ReleaseError`] describing the first problem found. Rejection
    /// messages never echo document contents back, because the document is
    /// untrusted input.
    pub fn parse(bytes: &[u8]) -> Result<Self, ReleaseError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MANIFEST_BYTES {
            return Err(ReleaseError::ManifestTooLarge);
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| ReleaseError::ManifestMalformed)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validates every field a verifier or an installer depends on.
    ///
    /// # Errors
    ///
    /// Returns the [`ReleaseError`] describing the first problem found.
    pub fn validate(&self) -> Result<(), ReleaseError> {
        if self.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION {
            return Err(ReleaseError::UnsupportedSchemaVersion {
                found: self.schema_version,
            });
        }
        if !is_version_text(&self.version) {
            return Err(ReleaseError::EmptyField { field: "version" });
        }
        if self.build.trim().is_empty() || self.build.len() > 128 {
            return Err(ReleaseError::EmptyField { field: "build" });
        }
        if self.target.trim().is_empty() || self.target.len() > 64 {
            return Err(ReleaseError::EmptyField { field: "target" });
        }
        if !is_rfc3339_utc(&self.published_at) {
            return Err(ReleaseError::InvalidTimestamp);
        }
        if self.artifacts.is_empty() {
            return Err(ReleaseError::NoArtifacts);
        }
        if self.artifacts.len() > MAX_ARTIFACTS {
            return Err(ReleaseError::NoArtifacts);
        }

        let mut seen: Vec<&str> = Vec::with_capacity(self.artifacts.len());
        for artifact in &self.artifacts {
            if !is_plain_file_name(&artifact.file) {
                return Err(ReleaseError::UnsafeArtifactName {
                    file: artifact.file.clone(),
                });
            }
            // The installed name is a file name the installer writes, so it is a
            // path component and is validated as one.
            if !is_plain_file_name(&artifact.name) {
                return Err(ReleaseError::UnsafeArtifactName {
                    file: artifact.name.clone(),
                });
            }
            if seen.contains(&artifact.file.as_str()) {
                return Err(ReleaseError::DuplicateArtifact {
                    file: artifact.file.clone(),
                });
            }
            seen.push(artifact.file.as_str());
            if !is_canonical_sha256_hex(&artifact.sha256) {
                return Err(ReleaseError::InvalidDigest {
                    file: artifact.file.clone(),
                });
            }
            if artifact.target.trim().is_empty() || artifact.kind.trim().is_empty() {
                return Err(ReleaseError::EmptyField { field: "artifact" });
            }
        }
        Ok(())
    }

    /// Serializes the manifest to the exact bytes a signature covers.
    ///
    /// # Errors
    ///
    /// Returns an error only if serialization fails, which is a programming error
    /// for this shape.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ReleaseError> {
        serde_json::to_vec(self).map_err(|_| ReleaseError::ManifestMalformed)
    }

    /// Returns the artifact entry for `file`, if present.
    #[must_use]
    pub fn artifact(&self, file: &str) -> Option<&ArtifactEntry> {
        self.artifacts.iter().find(|entry| entry.file == file)
    }

    /// Returns the installed file name for a published download file.
    ///
    /// An installer installs `jarvisd` (a stable name), even though the download
    /// was `jarvisd0.1.0-x86_64-linux` (a version-named one). The mapping is
    /// recorded in the signed document, so it cannot be inferred incorrectly.
    #[must_use]
    pub fn installed_name(&self, file: &str) -> Option<&str> {
        self.artifact(file).map(|entry| entry.name.as_str())
    }
}

impl SignatureEnvelope {
    /// Parses a signature envelope.
    ///
    /// # Errors
    ///
    /// Returns [`ReleaseError::SignatureTooLarge`] or
    /// [`ReleaseError::SignatureMalformed`].
    pub fn parse(bytes: &[u8]) -> Result<Self, ReleaseError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SIGNATURE_BYTES {
            return Err(ReleaseError::SignatureTooLarge);
        }
        serde_json::from_slice(bytes).map_err(|_| ReleaseError::SignatureMalformed)
    }

    /// Serializes the envelope for publication beside the manifest.
    ///
    /// # Errors
    ///
    /// Returns an error only if serialization fails.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ReleaseError> {
        serde_json::to_vec(self).map_err(|_| ReleaseError::SignatureMalformed)
    }
}

/// The set of release signing public keys this build trusts.
///
/// A trust store is built once, from compiled-in material, and is never
/// constructed from a caller-supplied argument. A key supplied by the caller
/// cannot be a trust anchor, because a caller who can choose the key can choose
/// one they control.
#[derive(Debug, Clone)]
pub struct TrustStore {
    keys: Vec<(String, VerifyingKey)>,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TrustStore {
    /// Creates an empty trust store, which trusts nothing.
    #[must_use]
    pub fn new() -> Self {
        Self { keys: Vec::new() }
    }

    /// Returns the trust store this build ships with.
    ///
    /// Until `OWN-003` establishes production signing identities, this contains
    /// only the committed **non-production test key** ([`TEST_KEY_ID`]). A
    /// caller must check [`Self::contains`] for that id and tell the operator so
    /// a test-key verification is never mistaken for a production guarantee.
    ///
    /// # Panics
    ///
    /// Does not panic: an unparseable compiled-in test key yields an empty store,
    /// and verification then fails closed with
    /// [`ReleaseError::UnknownKey`] rather than trusting anything.
    #[must_use]
    pub fn builtin() -> Self {
        let mut store = Self::new();
        let _ = store.add(TEST_KEY_ID, TEST_PUBLIC_KEY);
        store
    }

    /// Adds a public key, given as unpadded `base64url`.
    ///
    /// # Errors
    ///
    /// Returns [`ReleaseError::SignatureInvalid`] when the key is not a decodable
    /// 32-byte `Ed25519` public key.
    pub fn add(&mut self, key_id: &str, public_key_base64url: &str) -> Result<(), ReleaseError> {
        let bytes = base64url_decode(public_key_base64url).ok_or(ReleaseError::SignatureInvalid)?;
        let array: [u8; ed25519_dalek::PUBLIC_KEY_LENGTH] = bytes
            .try_into()
            .map_err(|_| ReleaseError::SignatureInvalid)?;
        let key = VerifyingKey::from_bytes(&array).map_err(|_| ReleaseError::SignatureInvalid)?;
        self.keys.push((key_id.to_owned(), key));
        Ok(())
    }

    /// Returns whether `key_id` is trusted.
    #[must_use]
    pub fn contains(&self, key_id: &str) -> bool {
        self.keys.iter().any(|(id, _)| id == key_id)
    }

    /// Returns the number of trusted keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Returns whether the store trusts no key.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Verifies a manifest document against its detached signature.
    ///
    /// The signature is checked over `manifest_bytes` exactly as given, then the
    /// document is parsed and validated. Parsing after the signature check means
    /// no attacker-controlled document is interpreted before it is authenticated.
    ///
    /// # Errors
    ///
    /// Returns the [`ReleaseError`] describing why verification failed.
    pub fn verify_manifest(
        &self,
        manifest_bytes: &[u8],
        envelope_bytes: &[u8],
    ) -> Result<ReleaseManifest, ReleaseError> {
        let envelope = SignatureEnvelope::parse(envelope_bytes)?;

        if envelope.algorithm != SIGNATURE_ALGORITHM {
            return Err(ReleaseError::UnsupportedAlgorithm {
                found: envelope.algorithm,
            });
        }
        let Some((_, key)) = self.keys.iter().find(|(id, _)| *id == envelope.key_id) else {
            return Err(ReleaseError::UnknownKey {
                key_id: envelope.key_id,
            });
        };

        let signature_bytes =
            base64url_decode(&envelope.signature).ok_or(ReleaseError::SignatureInvalid)?;
        let signature = Signature::try_from(signature_bytes.as_slice())
            .map_err(|_| ReleaseError::SignatureInvalid)?;

        key.verify(manifest_bytes, &signature)
            .map_err(|_| ReleaseError::SignatureMismatch)?;

        ReleaseManifest::parse(manifest_bytes)
    }
}

/// One artifact that was verified, for the operator's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifact {
    /// The published download file name.
    pub file: String,
    /// The file name the artifact is installed as.
    pub name: String,
    /// The platform target the artifact is for.
    pub target: String,
    /// The verified size in bytes.
    pub size: u64,
}

/// Verifies every artifact a manifest lists against the files in `directory`.
///
/// The manifest must already have been authenticated by
/// [`TrustStore::verify_manifest`]; this function never trusts a digest that has
/// not been signed.
///
/// # Errors
///
/// Returns [`ReleaseError::ArtifactMissing`], [`ReleaseError::ArtifactUnreadable`],
/// [`ReleaseError::SizeMismatch`], or [`ReleaseError::DigestMismatch`].
pub fn verify_artifacts(
    manifest: &ReleaseManifest,
    directory: &Path,
) -> Result<Vec<VerifiedArtifact>, ReleaseError> {
    let mut verified = Vec::with_capacity(manifest.artifacts.len());
    for artifact in &manifest.artifacts {
        let path = directory.join(&artifact.file);
        let metadata = std::fs::metadata(&path).map_err(|_| ReleaseError::ArtifactMissing {
            file: artifact.file.clone(),
        })?;
        if !metadata.is_file() {
            return Err(ReleaseError::ArtifactMissing {
                file: artifact.file.clone(),
            });
        }
        // The size is checked before hashing so a truncated or substituted
        // artifact of the wrong length is caught without reading it, and the
        // digest confirms it afterwards.
        if metadata.len() != artifact.size {
            return Err(ReleaseError::SizeMismatch {
                file: artifact.file.clone(),
            });
        }
        let digest = sha256_file(&path)?;
        if digest != artifact.sha256 {
            return Err(ReleaseError::DigestMismatch {
                file: artifact.file.clone(),
            });
        }
        verified.push(VerifiedArtifact {
            file: artifact.file.clone(),
            name: artifact.name.clone(),
            target: artifact.target.clone(),
            size: metadata.len(),
        });
    }
    Ok(verified)
}

/// Returns the lowercase hex `SHA-256` of a file, streamed in bounded blocks.
///
/// # Errors
///
/// Returns [`ReleaseError::ArtifactUnreadable`] when the file cannot be opened or
/// read. The error names the file the caller must report, so the message an
/// operator sees identifies which download to retry.
pub fn sha256_file(path: &Path) -> Result<String, ReleaseError> {
    let mut file = std::fs::File::open(path).map_err(|_| ReleaseError::ArtifactUnreadable {
        file: path.display().to_string(),
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ReleaseError::ArtifactUnreadable {
                file: path.display().to_string(),
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Renders bytes as lowercase hexadecimal.
#[must_use]
pub fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Signs a manifest document using the committed test key material.
///
/// Used by the signer example and by tests. It lives beside
/// [`TrustStore::verify_manifest`] on purpose: signer and verifier must use one
/// definition of what the signature covers, and keeping them in one module is
/// what makes that guarantee structural rather than a convention.
///
/// # Errors
///
/// Returns [`ReleaseError`] when the secret key is not a decodable 32-byte value.
pub fn sign_manifest(
    secret_key_base64url: &str,
    manifest_bytes: &[u8],
) -> Result<SignatureEnvelope, ReleaseError> {
    let bytes = base64url_decode(secret_key_base64url).ok_or(ReleaseError::SignatureInvalid)?;
    let array: [u8; ed25519_dalek::SECRET_KEY_LENGTH] = bytes
        .try_into()
        .map_err(|_| ReleaseError::SignatureInvalid)?;
    let signing_key = SigningKey::from_bytes(&array);
    let signature = signing_key.sign(manifest_bytes);
    Ok(SignatureEnvelope {
        algorithm: SIGNATURE_ALGORITHM.to_owned(),
        key_id: TEST_KEY_ID.to_owned(),
        signature: base64url_encode(&signature.to_bytes()),
    })
}

/// Encodes bytes as unpadded `base64url`.
#[must_use]
pub fn base64url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |byte| u32::from(*byte));
        let b2 = chunk.get(2).map_or(0, |byte| u32::from(*byte));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(ALPHABET[((triple >> 18) & 0x3f) as usize]));
        out.push(char::from(ALPHABET[((triple >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(ALPHABET[((triple >> 6) & 0x3f) as usize]));
        }
        if chunk.len() > 2 {
            out.push(char::from(ALPHABET[(triple & 0x3f) as usize]));
        }
    }
    out
}

/// Decodes unpadded `base64url`, rejecting padding and any non-alphabet byte.
///
/// Returns `None` rather than a partial value, so a malformed signature can never
/// be truncated into a shorter one that happens to verify.
#[must_use]
pub fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    fn value_of(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut accumulator = 0_u32;
        for byte in chunk {
            accumulator = (accumulator << 6) | value_of(*byte)?;
        }
        // Shift the partial group up so its high bits land where they belong.
        accumulator <<= 6 * (4 - chunk.len());
        out.push(u8::try_from((accumulator >> 16) & 0xff).ok()?);
        if chunk.len() > 2 {
            out.push(u8::try_from((accumulator >> 8) & 0xff).ok()?);
        }
        if chunk.len() > 3 {
            out.push(u8::try_from(accumulator & 0xff).ok()?);
        }
    }
    Some(out)
}

/// Reads a file with a size bound, so a hostile document cannot exhaust memory.
///
/// # Errors
///
/// Returns [`ReleaseError::ManifestTooLarge`] or
/// [`ReleaseError::SignatureTooLarge`] based on which limit was exceeded.
pub fn read_bounded(
    path: &Path,
    limit: u64,
    too_large: ReleaseError,
) -> Result<Vec<u8>, ReleaseError> {
    let metadata = std::fs::metadata(path).map_err(|_| ReleaseError::ManifestMalformed)?;
    if metadata.len() > limit {
        return Err(too_large);
    }
    std::fs::read(path).map_err(|_| ReleaseError::ManifestMalformed)
}

impl fmt::Display for Channel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stable => "stable",
            Self::Preview => "preview",
            Self::Development => "development",
        })
    }
}

/// The path of the default detached signature beside a manifest.
///
/// A sidecar file is used instead of embedding the signature in the manifest,
/// because an embedded signature cannot cover the document that contains it.
#[must_use]
pub fn default_signature_path(manifest: &Path) -> PathBuf {
    let mut value = manifest.as_os_str().to_owned();
    value.push(".sig");
    PathBuf::from(value)
}

#[cfg(test)]
mod tests;
