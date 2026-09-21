//! Builds and signs a **non-production test release** manifest.
//!
//! This is the CI half of `FND-011`. `Native targets` builds the binaries once per
//! target, and this example turns those binaries into a manifest with real
//! digests, then signs it with the committed **test key**. The journey then runs
//! `jarvis verify-release` and requires it to pass; a separate step corrupts one
//! artifact byte and requires the same command to **fail**, which is the property
//! the whole slice exists to prove.
//!
//! ```text
//! cargo run -p jarvis-infrastructure --example make_test_release -- <dir> <target> <version> <channel>
//! ```
//!
//! Every artifact it writes is named and marked `non-production-test`. It must
//! never be published, promoted, or trusted: `OWN-003` owns real signing
//! identities, and a test key has no trust outside this journey.

use std::path::Path;

use jarvis_infrastructure::release::{
    ArtifactEntry, Channel, RELEASE_MANIFEST_SCHEMA_VERSION, ReleaseManifest, TEST_SECRET_KEY,
    default_signature_path, hex_lower, sha256_file, sign_manifest,
};
use sha2::Digest as _;

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [directory, target, version, channel] = arguments.as_slice() else {
        eprintln!(
            "usage: make_test_release <directory> <target> <version> <stable|preview|development>"
        );
        return std::process::ExitCode::from(2);
    };

    match run(Path::new(directory), target, version, channel) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Builds, hashes, and signs a manifest for the artifacts in `directory`.
fn run(directory: &Path, target: &str, version: &str, channel: &str) -> Result<(), String> {
    let channel = match channel {
        "stable" => Channel::Stable,
        "preview" => Channel::Preview,
        "development" => Channel::Development,
        other => return Err(format!("unknown channel {other}")),
    };

    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let files = [
        (
            format!("jarvis{version}-{target}{suffix}"),
            format!("jarvis{suffix}"),
        ),
        (
            format!("jarvisd{version}-{target}{suffix}"),
            format!("jarvisd{suffix}"),
        ),
    ];

    let mut artifacts = Vec::with_capacity(files.len());
    for (published, installed) in &files {
        // The release packages a copy under the versioned name, so the manifest
        // describes the bytes that will actually be published, while `name`
        // records the stable name an install gives it.
        let source = directory.join(installed);
        let destination = directory.join(published);
        std::fs::copy(&source, &destination)
            .map_err(|error| format!("could not stage {}: {error}", destination.display()))?;

        let metadata = std::fs::metadata(&destination)
            .map_err(|error| format!("could not stat {}: {error}", destination.display()))?;
        artifacts.push(ArtifactEntry {
            target: target.to_owned(),
            kind: "binary".to_owned(),
            file: published.clone(),
            name: installed.clone(),
            sha256: sha256_file(&destination)
                .map_err(|error| format!("could not hash artifacts: {error}"))?,
            size: metadata.len(),
        });
    }

    let digest_of_digests = {
        // A stable build identity without shelling out to git: the combined
        // artifact digests identify exactly which bytes this describes.
        let mut hasher = sha2::Sha256::new();
        for artifact in &artifacts {
            hasher.update(artifact.sha256.as_bytes());
        }
        hex_lower(&hasher.finalize())
    };

    let manifest = ReleaseManifest {
        schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
        version: version.trim_start_matches('v').to_owned(),
        channel,
        target: target.to_owned(),
        api_major: 1,
        min_data_version: 1,
        build: format!("non-production-test/{digest_of_digests}"),
        published_at: now_rfc3339_utc(),
        artifacts,
    };
    manifest.validate().map_err(|error| {
        format!(
            "the locally built manifest is invalid ({}): {error}",
            error.code()
        )
    })?;

    let bytes = manifest
        .to_bytes()
        .map_err(|error| format!("could not serialize manifest: {error}"))?;
    let manifest_path = directory.join(format!("release-{version}-{target}.json"));
    std::fs::write(&manifest_path, &bytes)
        .map_err(|error| format!("could not write manifest: {error}"))?;

    let envelope = sign_manifest(TEST_SECRET_KEY, &bytes)
        .map_err(|error| format!("could not sign manifest: {error}"))?;
    let signature_path = default_signature_path(&manifest_path);
    std::fs::write(
        &signature_path,
        envelope
            .to_bytes()
            .map_err(|error| format!("could not serialize signature: {error}"))?,
    )
    .map_err(|error| format!("could not write signature: {error}"))?;

    println!("manifest:  {}", manifest_path.display());
    println!("signature: {}", signature_path.display());
    println!("artifacts: {}", manifest.artifacts.len());
    Ok(())
}

/// Returns the current instant as `YYYY-MM-DDTHH:MM:SSZ`.
fn now_rfc3339_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs());
    // A minimal civil-time conversion, so the example needs no date dependency.
    let days = seconds / 86_400;
    let (hours, minutes, secs) = (
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
    );
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{secs:02}Z")
}

/// Converts a count of days since the Unix epoch into a civil date.
///
/// Howard Hinnant's `civil_from_days`, which is exact for every value this
/// program produces and needs no external crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if month <= 2 { year + 1 } else { year }, month, day)
}
