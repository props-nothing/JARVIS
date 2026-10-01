//! Tests for package provenance and signature verification.
//!
//! **The test this file exists for is [`a_manifest_pointing_at_another_packages_signature_is_refused`]**,
//! because it is the one failure a verifier that checks "a signature" rather than "this package's
//! signature" does not catch. Every other test here establishes a check works; that one establishes the
//! checks are *bound to each other*.
//!
//! The fixtures sign real bytes with a real `Ed25519` key and verify against a real trust store, so the
//! crypto paths are exercised rather than stubbed. A stubbed verifier would pass every test about
//! *ordering* and prove nothing about whether a signature is actually checked — and "does it verify the
//! signature" is the question this module's existence rests on.

use ed25519_dalek::{Signer as _, SigningKey};

use crate::plugin::PluginManifest;
use crate::plugin::verify::{
    MAX_PACKAGE_SIGNATURE_BYTES, PACKAGE_SIGNATURE_KIND, PluginPackageVerifier,
    PluginVerificationError, is_usable_package_source, is_usable_plugin_identifier,
    is_verifiable_signature_kind, sha256_digest,
};
use crate::release::{SIGNATURE_ALGORITHM, SignatureEnvelope, TrustStore, base64url_encode};

/// The key id the fixtures sign with.
const KEY_ID: &str = "test-publisher-1";

/// Deterministic signing material, so a failure is reproducible.
///
/// A fixed seed rather than a random key: a test that failed once and passed on re-run because a new
/// key was generated is a test whose evidence is worthless.
fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7_u8; 32])
}

/// A trust store containing only [`KEY_ID`].
fn trust() -> TrustStore {
    let mut store = TrustStore::new();
    store
        .add(
            KEY_ID,
            &base64url_encode(&signing_key().verifying_key().to_bytes()),
        )
        .expect("the fixture key is a valid Ed25519 public key");
    store
}

/// The repository root, from this crate's manifest directory.
fn repository_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the crate lives two levels under the repository root")
        .to_path_buf()
}

/// The contract's manifest example, extracted from the document rather than copied.
///
/// **Read from the contract, like the sibling manifest test module does**, so the verifier's fixtures
/// are documents the *contract* accepts. A hand-written JSON literal here would be a second opinion
/// about what a manifest looks like, and the two would drift apart exactly where a field moved.
fn contract_example() -> String {
    let text = std::fs::read_to_string(repository_root().join("docs/contracts/plugin-manifest.md"))
        .expect("the plugin manifest contract reads");
    let start = text
        .find("```json")
        .expect("the contract carries a JSON example");
    let body = &text[start + "```json".len()..];
    let end = body.find("```").expect("the example fence closes");
    body[..end].trim().to_owned()
}

/// A bound large enough that the size check is not what a fixture happens to trip over.
const GENEROUS_BOUND: u64 = 8 * 1024 * 1024;

/// Builds a manifest claiming `digest` and signature identity `signature_ref`, then parses it.
///
/// Three values are substituted into the contract's own example, and **each substitution is asserted to
/// have happened**: the digest placeholder (which the contract documents as a placeholder), the
/// signature kind (because the contract's example illustrates `sigstore-bundle` while this build
/// verifies `ed25519-detached`), and the signature reference (a per-package value no example can
/// carry). Asserting each means a change to the contract's example fails here rather than silently
/// producing a document that parses to something else.
fn manifest_for(digest: &str, signature_ref: &str, source: &str) -> PluginManifest {
    let raw = contract_example();
    for expected in [
        r#""digest": "sha256:...""#,
        r#""kind":"sigstore-bundle""#,
        r#""value_ref":"package/signature.json""#,
    ] {
        assert!(
            raw.contains(expected),
            "the contract's example must still carry `{expected}`, or this fixture is testing \
             something the contract no longer says",
        );
    }
    let document = raw
        .replace(r#""digest": "sha256:...""#, &format!(r#""digest": "{digest}""#))
        .replace(
            r#""kind":"sigstore-bundle""#,
            &format!(r#""kind":"{PACKAGE_SIGNATURE_KIND}""#),
        )
        .replace(
            r#""value_ref":"package/signature.json""#,
            &format!(r#""value_ref":"{signature_ref}""#),
        )
        // The source is substituted too, because the fixture asserts on it and the contract's example
        // carries a URL specific to its own illustration.
        .replace(
            r#""source": "https://example.org/releases/1.2.3""#,
            &format!(r#""source": "{source}""#),
        );
    PluginManifest::parse(document.as_bytes()).expect("the fixture manifest is valid")
}

/// A manifest whose declared digest and signature identity both describe `bytes`.
fn honest_manifest(bytes: &[u8], source: &str) -> PluginManifest {
    manifest_for(&sha256_digest(bytes), KEY_ID, source)
}

/// Signs `bytes` and returns the envelope document.
fn envelope_for(bytes: &[u8]) -> Vec<u8> {
    let signature = signing_key().sign(bytes);
    let envelope = SignatureEnvelope {
        algorithm: SIGNATURE_ALGORITHM.to_owned(),
        key_id: KEY_ID.to_owned(),
        signature: base64url_encode(&signature.to_bytes()),
    };
    envelope
        .to_bytes()
        .expect("the fixture envelope serializes")
}

/// A verifier over the fixture trust store.
fn verifier() -> PluginPackageVerifier {
    PluginPackageVerifier::new(trust(), GENEROUS_BOUND)
}

#[test]
fn an_honest_package_verifies_and_the_digest_is_derived_from_the_bytes() {
    // The accepting case, asserted first so every refusal below is a *refusal* rather than an
    // implementation that refuses everything. A verifier that rejected all input would satisfy the
    // whole rest of this file.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://example.org/releases/1.2.3");
    let verified = verifier()
        .verify(&manifest, bytes, &envelope_for(bytes))
        .expect("an honest package verifies");

    // The digest is the one computed from the bytes, and it equals what the document claimed — the
    // equality is what makes it a *fact* rather than an echo.
    assert_eq!(verified.digest(), sha256_digest(bytes));
    assert_eq!(verified.digest(), manifest.package.digest);
    assert_eq!(verified.signature_identity(), KEY_ID);
    assert_eq!(verified.source(), "https://example.org/releases/1.2.3");
    assert_eq!(verified.size(), u64::try_from(bytes.len()).expect("fits"));
}

#[test]
fn a_manifest_pointing_at_another_packages_signature_is_refused() {
    // **The attack this module exists for.** The attacker controls a package A and signs it honestly;
    // they ship a manifest for package B that declares A's digest and A's key. Every check in
    // isolation passes — the kind is supported, the digest matches the bytes, the signature is a valid
    // signature by a *trusted* key. Only the **binding** fails: A's signature is not B's signature.
    //
    // This is the mutation target. Removing the `key_id != value_ref` comparison turns every other
    // test in this file green and this one red, which is exactly what makes it the load-bearing check.
    let bytes = b"the package bytes";
    // The attacker's manifest claims a **different** signer than the one that will sign the bytes.
    let manifest = manifest_for(
        &sha256_digest(bytes),
        "some-other-publisher-key",
        "https://example.org/releases/1.2.3",
    );
    let error = verifier()
        .verify(&manifest, bytes, &envelope_for(bytes))
        .expect_err("a signature for a different identity must be refused");
    assert_eq!(
        error,
        PluginVerificationError::SignatureIdentityMismatch,
        "the failure must name the binding, not the signature or the digest",
    );
}

#[test]
fn tampered_bytes_are_refused_by_the_digest_before_the_signature_is_consulted() {
    // The contract's "tampered archive rejection" (test 2). One byte changes, the signature is still
    // the honest one over the *original* bytes — so this refusal is the digest's, and the ordering
    // matters: the digest is cheaper and it reports the more actionable cause ("these are not the
    // bytes that were signed", not "the signature is bad").
    let original = b"the package bytes";
    let manifest = honest_manifest(original, "https://example.org/releases/1.2.3");
    let mut tampered = original.to_vec();
    tampered[0] ^= 0x01;
    let error = verifier()
        .verify(&manifest, &tampered, &envelope_for(original))
        .expect_err("tampered bytes must be refused");
    assert_eq!(error, PluginVerificationError::DigestMismatch);
}

#[test]
fn a_signature_from_an_untrusted_key_is_refused_and_names_the_key() {
    // "I do not trust that key" and "the signature is wrong" need different operator responses — the
    // first means the publisher was never trusted, the second that the bytes were swapped. The key id
    // is carried because it is what the operator looks up or adds to the trust store.
    let bytes = b"the package bytes";
    let manifest = manifest_for(
        &sha256_digest(bytes),
        "unknown-key",
        "https://example.org/x",
    );
    // The envelope names a key that is not in the store. Its `value_ref` matches, so the binding is
    // not what fails here — the lookup is.
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&envelope_for(bytes)).expect("the fixture envelope parses");
    *envelope
        .get_mut("key_id")
        .expect("the envelope carries a key id") =
        serde_json::Value::String("unknown-key".to_owned());
    let document = serde_json::to_vec(&envelope).expect("reserializes");

    let error = verifier()
        .verify(&manifest, bytes, &document)
        .expect_err("an untrusted key must be refused");
    assert_eq!(
        error,
        PluginVerificationError::UnknownSigningKey {
            key_id: "unknown-key".to_owned(),
        },
    );
}

#[test]
fn a_signature_that_does_not_match_the_bytes_is_refused() {
    // The bytes and the signature disagree. The digest check is satisfied because the manifest claims
    // the digest of *these* bytes, so this reaches the signature check — which is what distinguishes
    // it from the tampered-bytes case above.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://example.org/x");
    let other = b"entirely different bytes";
    let error = verifier()
        .verify(&manifest, bytes, &envelope_for(other))
        .expect_err("a signature over other bytes must be refused");
    assert_eq!(error, PluginVerificationError::SignatureMismatch);
}

#[test]
fn a_signature_kind_this_build_cannot_verify_is_refused_by_name() {
    // **Fail closed on the kind, which is the manifest's own claim about how it was signed.** The
    // alternative — ignoring the kind and verifying whatever envelope arrives — would accept a
    // manifest declaring an unimplemented scheme because some *other* scheme happened to verify its
    // bytes. That is a silent downgrade, so the kind is checked first and refused by name.
    //
    // The document is the contract's own example with **only** the digest filled and the kind left as
    // the contract illustrates it, so this asserts the real case: the example names `sigstore-bundle`,
    // which this build does not verify.
    let bytes = b"the package bytes";
    let raw = contract_example();
    let document = raw.replace(
        r#""digest": "sha256:...""#,
        &format!(r#""digest": "{}""#, sha256_digest(bytes)),
    );
    // The manifest **parses** — the kind is an open string upstream on purpose — and the verifier is
    // what refuses it. That split is deliberate: pinning the kind in the manifest validator would
    // refuse a kind a future verifier supports.
    let manifest =
        PluginManifest::parse(document.as_bytes()).expect("the document itself is valid");
    let error = verifier()
        .verify(&manifest, bytes, &envelope_for(bytes))
        .expect_err("an unverifiable kind must be refused by name");
    assert_eq!(
        error,
        PluginVerificationError::UnsupportedSignatureKind {
            kind: "sigstore-bundle".to_owned(),
        },
    );
}

#[test]
fn an_algorithm_downgrade_in_the_envelope_is_refused() {
    // The envelope names its own algorithm, and a downgrade must be refused rather than attempted: an
    // envelope claiming `rsa-sha1` is asking the verifier to use a scheme this build does not
    // implement, and "the signature happens to verify" must never be reachable by naming another
    // algorithm. Reuses `SIGNATURE_ALGORITHM` from the release primitive rather than a literal.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://example.org/x");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&envelope_for(bytes)).expect("parses");
    *envelope
        .get_mut("algorithm")
        .expect("the envelope names an algorithm") =
        serde_json::Value::String("rsa-sha1".to_owned());
    let document = serde_json::to_vec(&envelope).expect("reserializes");
    let error = verifier()
        .verify(&manifest, bytes, &document)
        .expect_err("a downgrade must be refused");
    assert_eq!(
        error,
        PluginVerificationError::UnsupportedAlgorithm {
            found: "rsa-sha1".to_owned(),
        },
    );
}

#[test]
fn an_oversized_package_is_refused_without_being_hashed() {
    // The bound is checked **first**, which is its whole purpose: an oversized package must be refused
    // without reading it. Asserted by building a verifier whose bound is smaller than the bytes, so
    // the refusal cannot come from any later check.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://example.org/x");
    let tiny = PluginPackageVerifier::new(trust(), 4);
    assert_eq!(
        tiny.verify(&manifest, bytes, &envelope_for(bytes))
            .expect_err("an oversized package must be refused"),
        PluginVerificationError::PackageTooLarge,
    );
}

#[test]
fn an_oversized_envelope_is_refused_before_it_is_parsed() {
    // The same bound for the envelope, at its own constant. A malformed *and* oversized envelope must
    // report the size, because the size is the cheaper and more actionable fact.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://example.org/x");
    let oversized = vec![b'{'; usize::try_from(MAX_PACKAGE_SIGNATURE_BYTES).expect("fits") + 1];
    assert_eq!(
        verifier()
            .verify(&manifest, bytes, &oversized)
            .expect_err("an oversized envelope must be refused"),
        PluginVerificationError::SignatureTooLarge,
    );
}

#[test]
fn a_malformed_envelope_is_refused_rather_than_defaulted() {
    // Not JSON at all. The refusal is a typed error, never a default envelope with empty fields —
    // which would be a signature that verifies nothing yet looks present.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://example.org/x");
    assert_eq!(
        verifier()
            .verify(&manifest, bytes, b"not json")
            .expect_err("a malformed envelope must be refused"),
        PluginVerificationError::SignatureMalformed,
    );
}

#[test]
fn a_verified_package_produces_the_source_identity() {
    // **The method that gives `PluginSourceIdentity` its first production producer.** Its
    // `package_digest` and `signature_identity` come from the *verification*, so an identity built
    // this way describes bytes that were checked — which is the property a manifest alone cannot
    // provide, because a document can name its own provenance.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://example.org/releases/1.2.3");
    let verified = verifier()
        .verify(&manifest, bytes, &envelope_for(bytes))
        .expect("verifies");
    let identity = verified
        .identity_for(&manifest)
        .expect("the manifest's fields satisfy the identity rule");

    assert_eq!(identity.id, "example.research-runtime");
    assert_eq!(identity.publisher, "example.org");
    // These two are the derived ones, and they are what make the identity trustworthy: the digest of
    // the bytes that were hashed, and the key that actually verified.
    assert_eq!(identity.package_digest, sha256_digest(bytes));
    assert_eq!(identity.signature_identity, KEY_ID);
    assert_eq!(identity.source, "https://example.org/releases/1.2.3");
    assert_eq!(identity.version.release_text(), "1.2.3");
    assert_eq!(identity.protocol, "jarvis-runtime");
    // And the derived identity is what a grant would be bound to, which is the tuple's whole point.
    assert!(
        jarvis_domain::plugin::is_canonical_package_digest(&identity.package_digest),
        "the derived digest must satisfy the domain's canonical form",
    );
}

#[test]
fn the_error_codes_are_namespaced_and_distinct_per_cause() {
    // Every variant must carry its own code, because a client branching on a code needs to tell
    // "untrusted key" from "wrong bytes". A single collapsed code would make the distinction useless
    // at exactly the boundary where it matters.
    let errors = [
        PluginVerificationError::SignatureTooLarge,
        PluginVerificationError::SignatureMalformed,
        PluginVerificationError::UnsupportedSignatureKind {
            kind: "k".to_owned(),
        },
        PluginVerificationError::UnsupportedAlgorithm {
            found: "a".to_owned(),
        },
        PluginVerificationError::UnknownSigningKey {
            key_id: "k".to_owned(),
        },
        PluginVerificationError::SignatureInvalid,
        PluginVerificationError::SignatureMismatch,
        PluginVerificationError::DigestMismatch,
        PluginVerificationError::SignatureIdentityMismatch,
        PluginVerificationError::PackageTooLarge,
    ];
    let mut codes: Vec<&str> = errors.iter().map(PluginVerificationError::code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), errors.len(), "codes must be distinct");
    for code in &codes {
        assert!(
            code.starts_with("jarvis.plugin_"),
            "{code} must be namespaced"
        );
    }
    // **And none is retryable**, which is a claim about the subject: the same bytes produce the same
    // verdict, so offering a retry would invite a caller to loop over a suspected-tampered package.
    for error in &errors {
        assert!(!error.retryable(), "{error:?} must not be retryable");
    }
}

#[test]
fn the_digest_helper_produces_a_domain_canonical_value() {
    // A drift check between this module's spelling and the domain's predicate: if they disagreed, a
    // computed digest would be refused by a validator that had every right to refuse it.
    let digest = sha256_digest(b"");
    assert!(jarvis_domain::plugin::is_canonical_package_digest(&digest));
    // The known SHA-256 of the empty input, so this is checked against the algorithm and not only
    // against the predicate — a helper that returned a fixed string would satisfy the predicate.
    assert_eq!(
        digest,
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
}

#[test]
fn the_delegating_predicates_agree_with_the_rules_they_consult() {
    // Each predicate exists so a caller (or a manifest writer) can check a value against the same rule
    // the parser or the constructor applies. Delegation is the property: a second spelling of one rule
    // is how a value comes to pass one check and fail the other.
    assert!(is_verifiable_signature_kind(PACKAGE_SIGNATURE_KIND));
    assert!(!is_verifiable_signature_kind("sigstore-bundle"));

    assert!(is_usable_package_source("https://example.org/x"));
    assert!(!is_usable_package_source(""));
    assert!(!is_usable_package_source(&"a".repeat(300)));

    assert!(is_usable_plugin_identifier("example.tool"));
    assert!(!is_usable_plugin_identifier("Example Tool"));
}

#[test]
fn the_verified_package_rendering_shows_the_digest_and_key_but_never_the_source() {
    // A `Display` rendering reaches a log line, and the source is operator-inspectable text that could
    // carry a misleading URL. The digest and the key id are what an operator correlates with a
    // publisher, so they are shown; the source is reachable through its accessor only.
    let bytes = b"the package bytes";
    let manifest = honest_manifest(bytes, "https://misleading.example.org/not-the-real-source");
    let verified = verifier()
        .verify(&manifest, bytes, &envelope_for(bytes))
        .expect("verifies");
    let rendered = verified.to_string();
    assert!(rendered.contains(KEY_ID), "{rendered}");
    assert!(rendered.contains(&sha256_digest(bytes)), "{rendered}");
    assert!(
        !rendered.contains("misleading.example.org"),
        "the source must not reach a rendered line: {rendered}",
    );
    // The accessor still returns it, so nothing was lost — only kept out of a log line.
    assert_eq!(
        verified.source(),
        "https://misleading.example.org/not-the-real-source",
    );
}
