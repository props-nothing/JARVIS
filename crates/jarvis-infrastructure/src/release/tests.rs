//! Tests for release manifest and artifact verification.
//!
//! The tests that matter here are the **rejection** tests. A verifier that
//! accepts a valid artifact is easy and nearly worthless on its own; the property
//! `FND-011` exists to prove is that an altered artifact, an altered manifest, an
//! unknown key, and a downgraded algorithm are all refused. Each rejection test
//! below is written so that deleting the corresponding check makes it fail.

use std::path::{Path, PathBuf};

use super::{
    ArtifactEntry, Channel, MAX_ARTIFACTS, ReleaseError, ReleaseManifest, SIGNATURE_ALGORITHM,
    SignatureEnvelope, TEST_KEY_ID, TEST_SECRET_KEY, TrustStore, base64url_decode,
    base64url_encode, default_signature_path, hex_lower, is_canonical_sha256_hex,
    is_plain_file_name, is_rfc3339_utc, is_version_text, sha256_file, sign_manifest,
    verify_artifacts,
};

/// A unique temporary directory for one test.
fn temp_dir(label: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "jarvis-release-{label}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
    ));
    std::fs::create_dir_all(&path).expect("temporary directory is creatable");
    path
}

/// Writes `bytes` to `directory/file` and returns its digest and size.
fn write_artifact(directory: &Path, file: &str, bytes: &[u8]) -> (String, u64) {
    let path = directory.join(file);
    std::fs::write(&path, bytes).expect("artifact is writable");
    (
        sha256_file(&path).expect("artifact is hashable"),
        u64::try_from(bytes.len()).expect("size fits"),
    )
}

/// Builds a manifest listing whatever artifacts exist in `directory`.
fn manifest_for(directory: &Path, files: &[(&str, &[u8])]) -> ReleaseManifest {
    let artifacts = files
        .iter()
        .map(|(file, bytes)| {
            let (sha256, size) = write_artifact(directory, file, bytes);
            ArtifactEntry {
                target: "x86_64-pc-windows-msvc".to_owned(),
                kind: "archive".to_owned(),
                file: (*file).to_owned(),
                name: (*file).to_owned(),
                sha256,
                size,
            }
        })
        .collect();

    ReleaseManifest {
        schema_version: super::RELEASE_MANIFEST_SCHEMA_VERSION,
        version: "0.1.0-test".to_owned(),
        channel: Channel::Development,
        target: "x86_64-pc-windows-msvc".to_owned(),
        api_major: 1,
        min_data_version: 1,
        build: "test-build".to_owned(),
        published_at: "2026-09-21T00:00:00Z".to_owned(),
        artifacts,
    }
}

/// Signs a manifest with the committed test key and returns both documents.
fn signed(manifest: &ReleaseManifest) -> (Vec<u8>, Vec<u8>) {
    let bytes = manifest.to_bytes().expect("manifest serializes");
    let envelope = sign_manifest(TEST_SECRET_KEY, &bytes).expect("test key signs");
    (bytes, envelope.to_bytes().expect("envelope serializes"))
}

#[test]
fn the_committed_test_key_is_a_real_usable_keypair() {
    // If the committed pair were inconsistent, every other test in this file
    // would fail for a reason unrelated to the checks under test. This pins the
    // pair so a regenerated key cannot silently desynchronize.
    let mut store = TrustStore::new();
    store
        .add(TEST_KEY_ID, super::TEST_PUBLIC_KEY)
        .expect("the committed public key parses");
    assert!(store.contains(TEST_KEY_ID));

    let manifest = manifest_for(&temp_dir("keypair"), &[("jarvis.zip", b"payload")]);
    let (bytes, envelope) = signed(&manifest);
    store
        .verify_manifest(&bytes, &envelope)
        .expect("a test-key signature verifies");
}

#[test]
fn the_builtin_store_trusts_only_the_non_production_test_key() {
    // The trust store must not accidentally contain anything else. A second key
    // appearing here would be a trust anchor nobody reviewed.
    let store = TrustStore::builtin();
    assert_eq!(store.len(), 1);
    assert!(store.contains(TEST_KEY_ID));
    assert!(!store.contains("jarvis-production-1"));
}

#[test]
fn a_valid_signed_manifest_with_matching_artifacts_verifies() {
    let directory = temp_dir("happy");
    let manifest = manifest_for(
        &directory,
        &[("jarvis.zip", b"payload"), ("jarvisd.exe", b"daemon")],
    );
    let (bytes, envelope) = signed(&manifest);

    let verified = TrustStore::builtin()
        .verify_manifest(&bytes, &envelope)
        .expect("a valid manifest verifies");
    assert_eq!(verified, manifest);

    let artifacts = verify_artifacts(&verified, &directory).expect("artifacts verify");
    assert_eq!(artifacts.len(), 2);
    assert_eq!(artifacts[0].file, "jarvis.zip");
    assert_eq!(artifacts[0].size, 7);
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn an_altered_artifact_byte_is_rejected() {
    // The central property of FND-011: a single flipped payload byte must be
    // refused even though the manifest and its signature are untouched.
    let directory = temp_dir("tamper-artifact");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"the original archive bytes")]);
    let (bytes, envelope) = signed(&manifest);
    let verified = TrustStore::builtin()
        .verify_manifest(&bytes, &envelope)
        .expect("valid manifest");

    // Flip one byte, keeping the length identical so only the digest can catch it.
    std::fs::write(directory.join("jarvis.zip"), b"the Original archive bytes")
        .expect("artifact is writable");

    assert_eq!(
        verify_artifacts(&verified, &directory),
        Err(ReleaseError::DigestMismatch {
            file: "jarvis.zip".to_owned()
        }),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_truncated_artifact_is_caught_by_size_before_the_digest() {
    let directory = temp_dir("truncated");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"a longer archive payload")]);
    let (bytes, envelope) = signed(&manifest);
    let verified = TrustStore::builtin()
        .verify_manifest(&bytes, &envelope)
        .expect("valid manifest");

    std::fs::write(directory.join("jarvis.zip"), b"a longer").expect("artifact is writable");
    assert_eq!(
        verify_artifacts(&verified, &directory),
        Err(ReleaseError::SizeMismatch {
            file: "jarvis.zip".to_owned()
        }),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_missing_artifact_is_rejected() {
    let directory = temp_dir("missing");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let (bytes, envelope) = signed(&manifest);
    let verified = TrustStore::builtin()
        .verify_manifest(&bytes, &envelope)
        .expect("valid manifest");

    std::fs::remove_file(directory.join("jarvis.zip")).ok();
    assert_eq!(
        verify_artifacts(&verified, &directory),
        Err(ReleaseError::ArtifactMissing {
            file: "jarvis.zip".to_owned()
        }),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn an_altered_manifest_body_invalidates_the_signature() {
    // Rewriting a digest inside the manifest is the attack a digest-only scheme
    // cannot detect; the signature is what refuses it.
    let directory = temp_dir("tamper-manifest");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let (bytes, envelope) = signed(&manifest);

    let text = String::from_utf8(bytes).expect("utf8");
    let altered = text.replace(&manifest.artifacts[0].sha256, &"a".repeat(64));
    assert_ne!(altered, text, "the test must actually change the document");

    assert_eq!(
        TrustStore::builtin().verify_manifest(altered.as_bytes(), &envelope),
        Err(ReleaseError::SignatureMismatch),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_signature_from_an_untrusted_key_is_refused_as_unknown_key() {
    // A caller who can choose the key they verify against can choose one they
    // own. Only the compiled-in store is a trust anchor.
    let directory = temp_dir("unknown-key");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let bytes = manifest.to_bytes().expect("serializes");

    // A different, valid Ed25519 keypair that is not in the trust store.
    let other_secret = [7_u8; 32];
    let other = ed25519_dalek::SigningKey::from_bytes(&other_secret);
    let envelope = SignatureEnvelope {
        algorithm: SIGNATURE_ALGORITHM.to_owned(),
        key_id: "jarvis-attacker-1".to_owned(),
        signature: base64url_encode(&ed25519_dalek::Signer::sign(&other, &bytes).to_bytes()),
    };

    assert_eq!(
        TrustStore::builtin().verify_manifest(&bytes, &envelope.to_bytes().expect("serializes")),
        Err(ReleaseError::UnknownKey {
            key_id: "jarvis-attacker-1".to_owned()
        }),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn an_unknown_algorithm_fails_closed_instead_of_being_ignored() {
    let directory = temp_dir("algorithm");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let (bytes, envelope) = signed(&manifest);

    let text = String::from_utf8(envelope).expect("utf8");
    let downgraded = text.replace(SIGNATURE_ALGORITHM, "rsa-sha1");
    assert_ne!(downgraded, text);

    assert_eq!(
        TrustStore::builtin().verify_manifest(bytes.as_slice(), downgraded.as_bytes()),
        Err(ReleaseError::UnsupportedAlgorithm {
            found: "rsa-sha1".to_owned()
        }),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_corrupted_signature_value_is_rejected_rather_than_truncated() {
    let directory = temp_dir("bad-signature");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let (bytes, envelope) = signed(&manifest);

    for bad in ["***", "AAAA", &"A".repeat(86)] {
        let text = String::from_utf8(envelope.clone()).expect("utf8");
        let start = text.find("\"signature\":\"").expect("field present") + 13;
        let end = start + text[start..].find('"').expect("closing quote");
        let mut replaced = text.clone();
        replaced.replace_range(start..end, bad);
        let result = TrustStore::builtin().verify_manifest(bytes.as_slice(), replaced.as_bytes());
        assert!(
            matches!(
                result,
                Err(ReleaseError::SignatureInvalid | ReleaseError::SignatureMismatch)
            ),
            "a malformed signature must be refused, got {result:?}",
        );
    }
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_manifest_with_an_unsupported_schema_version_is_refused() {
    // The document must be *validly signed* for this to test the schema check:
    // altering a signed document fails the signature first (a separate test
    // covers that), so a version bump that is never signed would prove nothing
    // about the version check.
    let directory = temp_dir("schema");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let text = String::from_utf8(manifest.to_bytes().expect("serializes")).expect("utf8");
    let bumped = text.replace("\"schema_version\":1", "\"schema_version\":2");
    assert_ne!(bumped, text, "the test must actually change the document");

    let envelope = sign_manifest(TEST_SECRET_KEY, bumped.as_bytes()).expect("signs");
    assert_eq!(
        TrustStore::builtin()
            .verify_manifest(bumped.as_bytes(), &envelope.to_bytes().expect("serializes")),
        Err(ReleaseError::UnsupportedSchemaVersion { found: 2 }),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn traversal_and_path_shaped_artifact_names_are_refused() {
    // The name is resolved against a download directory, so it is a
    // filesystem-affecting value taken from a document.
    for bad in [
        "../escape.zip",
        "sub/dir.zip",
        r"sub\dir.zip",
        "/absolute.zip",
        r"C:\windows\evil.exe",
        ".hidden",
        "has:stream",
        "",
    ] {
        let mut manifest = manifest_for(&temp_dir("names"), &[("ok.zip", b"x")]);
        manifest.artifacts[0].file = bad.to_owned();
        let result = manifest.validate();
        assert!(
            matches!(
                result,
                Err(ReleaseError::UnsafeArtifactName { .. } | ReleaseError::EmptyField { .. })
            ),
            "{bad:?} must be refused, got {result:?}",
        );
    }
    assert!(is_plain_file_name("jarvis-0.1.0-x86_64.zip"));
}

#[test]
fn a_non_canonical_digest_is_refused() {
    let directory = temp_dir("digest-shape");
    let mut manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);

    for bad in [
        "ABC".to_owned(),
        "a".repeat(63),
        "A".repeat(64),
        format!("{}z", "a".repeat(63)),
        String::new(),
    ] {
        manifest.artifacts[0].sha256 = bad.clone();
        assert_eq!(
            manifest.validate(),
            Err(ReleaseError::InvalidDigest {
                file: "jarvis.zip".to_owned()
            }),
            "{bad:?} must be refused",
        );
    }
    assert!(is_canonical_sha256_hex(&"0123456789abcdef".repeat(4)));
}

#[test]
fn duplicate_artifact_entries_are_refused() {
    let directory = temp_dir("duplicate");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let mut duplicated = manifest.clone();
    let first = duplicated.artifacts[0].clone();
    duplicated.artifacts.push(first);

    assert_eq!(
        duplicated.validate(),
        Err(ReleaseError::DuplicateArtifact {
            file: "jarvis.zip".to_owned()
        }),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn an_empty_or_oversized_artifact_list_is_refused() {
    let directory = temp_dir("counts");
    let mut manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);

    let entry = manifest.artifacts[0].clone();
    manifest.artifacts.clear();
    assert_eq!(manifest.validate(), Err(ReleaseError::NoArtifacts));

    for index in 0..=MAX_ARTIFACTS {
        let mut copy = entry.clone();
        copy.file = format!("artifact-{index}.zip");
        manifest.artifacts.push(copy);
    }
    assert_eq!(manifest.validate(), Err(ReleaseError::NoArtifacts));
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn empty_and_malformed_fields_are_refused() {
    let directory = temp_dir("fields");
    let base = manifest_for(&directory, &[("jarvis.zip", b"payload")]);

    for label in ["version", "build", "target", "published_at"] {
        let mut manifest = base.clone();
        match label {
            "version" => manifest.version = " ".to_owned(),
            "build" => manifest.build = String::new(),
            "target" => manifest.target = String::new(),
            _ => manifest.published_at = "2026-09-21 00:00:00".to_owned(),
        }
        assert!(
            manifest.validate().is_err(),
            "{label} must be refused when invalid",
        );
    }
    assert!(base.validate().is_ok());
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn non_canonical_timestamps_and_versions_are_refused() {
    // Offsets and missing `Z` are a second spelling of the same instant, so they
    // are refused rather than normalized.
    assert!(is_rfc3339_utc("2026-09-21T00:00:00Z"));
    for bad in [
        "2026-09-21T00:00:00+02:00",
        "2026-09-21T00:00:00",
        "2026-09-21 00:00:00Z",
        "2026-09-21T00:00:00z",
        "20260921T000000Z",
        "",
    ] {
        assert!(!is_rfc3339_utc(bad), "{bad:?} must be refused");
    }

    assert!(is_version_text("0.1.0"));
    assert!(is_version_text("1.0.0-rc.1+build.5"));
    for bad in ["v0.1.0", "0.1.0\nEVIL", "", "0.1.0 "] {
        assert!(!is_version_text(bad), "{bad:?} must be refused");
    }
}

#[test]
fn an_oversized_manifest_is_refused_before_it_is_parsed() {
    let mut huge = vec![b'{'];
    huge.extend(std::iter::repeat_n(
        b'a',
        usize::try_from(super::MAX_MANIFEST_BYTES).expect("fits") + 1,
    ));
    assert_eq!(
        ReleaseManifest::parse(&huge),
        Err(ReleaseError::ManifestTooLarge),
    );
    assert_eq!(
        SignatureEnvelope::parse(&vec![
            b' ';
            usize::try_from(super::MAX_SIGNATURE_BYTES)
                .expect("fits")
                + 1
        ]),
        Err(ReleaseError::SignatureTooLarge),
    );
}

#[test]
fn an_unknown_manifest_field_is_refused() {
    let directory = temp_dir("unknown-field");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let text = String::from_utf8(manifest.to_bytes().expect("serializes")).expect("utf8");
    let injected = text.replace("\"channel\":", "\"injected\":true,\"channel\":");

    assert_eq!(
        ReleaseManifest::parse(injected.as_bytes()),
        Err(ReleaseError::ManifestMalformed),
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_same_manifest_bytes_sign_and_verify_across_a_write() {
    // The signature covers the raw document, so the exact bytes that were signed
    // must be the bytes that are verified. A read that re-serialized would drift.
    let directory = temp_dir("roundtrip");
    let manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);
    let bytes = manifest.to_bytes().expect("serializes");
    let envelope = sign_manifest(TEST_SECRET_KEY, &bytes).expect("signs");

    let manifest_path = directory.join("manifest.json");
    let signature_path = default_signature_path(&manifest_path);
    std::fs::write(&manifest_path, &bytes).expect("writable");
    std::fs::write(
        &signature_path,
        envelope.to_bytes().expect("envelope serializes"),
    )
    .expect("writable");

    let read_manifest = std::fs::read(&manifest_path).expect("readable");
    let read_signature = std::fs::read(&signature_path).expect("readable");
    let verified = TrustStore::builtin()
        .verify_manifest(&read_manifest, &read_signature)
        .expect("verifies after a write and read");
    assert_eq!(verified, manifest);
    assert_eq!(
        signature_path.extension().and_then(|value| value.to_str()),
        Some("sig")
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_path_shaped_installed_name_is_refused() {
    // The installed name is written as a file at install time, so it is a path
    // component and must be validated exactly like the download name.
    let directory = temp_dir("installed-name");
    let mut manifest = manifest_for(&directory, &[("jarvis.zip", b"payload")]);

    for bad in ["../evil", "a/b", r"a\b", "/abs", ".hidden", ""] {
        manifest.artifacts[0].name = bad.to_owned();
        assert_eq!(
            manifest.validate(),
            Err(ReleaseError::UnsafeArtifactName {
                file: bad.to_owned()
            }),
            "{bad:?} must be refused as an installed name",
        );
    }
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn the_installed_name_is_exposed_separately_from_the_download_name() {
    let directory = temp_dir("installed-lookup");
    let mut manifest = manifest_for(&directory, &[("jarvisd0.1.0-x86_64.zip", b"payload")]);
    manifest.artifacts[0].name = "jarvisd".to_owned();
    manifest.validate().expect("valid");

    assert_eq!(
        manifest.installed_name("jarvisd0.1.0-x86_64.zip"),
        Some("jarvisd"),
        "a service points at the stable installed name, not the versioned download",
    );
    assert_eq!(manifest.installed_name("absent.zip"), None);
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn codes_are_namespaced_and_unique() {
    let errors = [
        ReleaseError::ManifestTooLarge,
        ReleaseError::SignatureTooLarge,
        ReleaseError::ManifestMalformed,
        ReleaseError::SignatureMalformed,
        ReleaseError::UnsupportedSchemaVersion { found: 2 },
        ReleaseError::UnsupportedAlgorithm {
            found: "rsa".to_owned(),
        },
        ReleaseError::UnknownKey {
            key_id: "k".to_owned(),
        },
        ReleaseError::SignatureInvalid,
        ReleaseError::SignatureMismatch,
        ReleaseError::NoArtifacts,
        ReleaseError::DuplicateArtifact {
            file: "f".to_owned(),
        },
        ReleaseError::UnsafeArtifactName {
            file: "f".to_owned(),
        },
        ReleaseError::InvalidDigest {
            file: "f".to_owned(),
        },
        ReleaseError::EmptyField { field: "version" },
        ReleaseError::InvalidTimestamp,
        ReleaseError::ArtifactMissing {
            file: "f".to_owned(),
        },
        ReleaseError::ArtifactUnreadable {
            file: "f".to_owned(),
        },
        ReleaseError::DigestMismatch {
            file: "f".to_owned(),
        },
        ReleaseError::SizeMismatch {
            file: "f".to_owned(),
        },
    ];

    let mut codes: Vec<&str> = errors.iter().map(ReleaseError::code).collect();
    for code in &codes {
        assert!(code.starts_with("jarvis.release_"), "{code}");
    }
    codes.sort_unstable();
    let count = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), count, "codes must be unique");

    // Retrying a deterministic mismatch only re-fetches the same altered bytes,
    // so it must not be advertised as retryable.
    assert!(
        !ReleaseError::DigestMismatch {
            file: "f".to_owned()
        }
        .retryable()
    );
    assert!(!ReleaseError::SignatureMismatch.retryable());
    assert!(
        ReleaseError::ArtifactUnreadable {
            file: "f".to_owned()
        }
        .retryable()
    );
}

#[test]
fn base64url_round_trips_every_length_class() {
    for length in 0..=32_usize {
        let bytes: Vec<u8> = (0..length)
            .map(|value| u8::try_from(value).expect("small"))
            .collect();
        let encoded = base64url_encode(&bytes);
        assert_eq!(
            base64url_decode(&encoded).as_deref(),
            Some(bytes.as_slice()),
            "{length}"
        );
    }

    // Padding and non-alphabet bytes are refused, not ignored.
    for bad in ["AAA=", "AA==", "****", "A", "AAAA\n"] {
        assert!(base64url_decode(bad).is_none(), "{bad:?} must be refused");
    }
}

#[test]
fn hex_lower_is_lowercase_and_fixed_width() {
    assert_eq!(hex_lower(&[0x00, 0x0f, 0xff]), "000fff");
    assert_eq!(hex_lower(&[]), "");
    assert_eq!(hex_lower(b"abc").len(), 6);
}
