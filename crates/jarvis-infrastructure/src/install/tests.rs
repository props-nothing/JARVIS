//! Tests for install, update, rollback, portable mode, and uninstall.
//!
//! The tests that matter are the ones about **user data** and **refusal**. An
//! install that works is easy; the properties `FND-012` exists to prove are that:
//!
//! - an uninstall removes program files and cannot reach the database;
//! - a purge is refused without an explicit acknowledgement;
//! - a destructive action is refused while a daemon holds the lock;
//! - a plan whose paths leave the install root is refused;
//! - a rollback with no restorable version refuses rather than silently doing
//!   nothing;
//! - a version string cannot escape the install root.
//!
//! Each refusal test is written so that removing the corresponding check makes it
//! fail.

use std::path::PathBuf;

use super::apply::{VerifiedRelease, apply, version_is_complete};
use super::plan::ExistingInstall;
use super::{
    InstallAction, InstallError, InstallLayout, InstallMode, Installation, is_safe_version,
    plan_install, plan_prune, plan_rollback, plan_uninstall, plan_update, version_directory_name,
    version_from_directory_name,
};
use crate::paths::ProfilePaths;
use crate::release::{
    ArtifactEntry, Channel, RELEASE_MANIFEST_SCHEMA_VERSION, ReleaseManifest, TEST_SECRET_KEY,
    sha256_file, sign_manifest,
};

/// Observes the current state of a layout.
fn observe(layout: &InstallLayout) -> ExistingInstall<'_> {
    ExistingInstall::observe(layout)
}

/// A unique temporary root for one test.
fn temp_root(label: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "jarvis-install-{label}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
    ));
    std::fs::create_dir_all(&path).expect("temporary root is creatable");
    path
}

/// A layout plus a profile under the same test root.
///
/// They are siblings on purpose: that is the real arrangement, and it means a test
/// that accidentally removes the profile is caught by the data assertions below.
fn fixture(label: &str) -> (InstallLayout, ProfilePaths, PathBuf) {
    let root = temp_root(label);
    let layout = InstallLayout::new(InstallMode::Portable, root.join("install"));
    let profile = ProfilePaths::portable(root.join("profile"));
    profile.ensure_directories().expect("profile dirs");
    (layout, profile, root)
}

/// The platform target the tests build releases for.
const TARGET: &str = "x86_64-pc-windows-msvc";

/// Builds a genuinely signed and verified release whose download directory holds
/// real bytes for `jarvis`.
///
/// The bytes are real and the signature is real, so a test that passes here proves
/// the install path is wired to the verification rather than bypassing it. A
/// synthetic `VerifiedArtifact` would not: it would test the type without testing
/// the gate.
fn verified_release(root: &std::path::Path, version: &str, contents: &[u8]) -> VerifiedRelease {
    let directory = root.join(format!("download-{version}"));
    std::fs::create_dir_all(&directory).expect("download dir");

    // The published name is version-named; the installed name is stable. That is
    // the real arrangement a service definition depends on.
    let published = format!("jarvis{version}-{TARGET}");
    let installed = "jarvis".to_owned();
    let path = directory.join(&published);
    std::fs::write(&path, contents).expect("artifact is writable");

    let manifest = ReleaseManifest {
        schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
        version: version.to_owned(),
        channel: Channel::Development,
        target: TARGET.to_owned(),
        api_major: 1,
        min_data_version: 1,
        build: "test-build".to_owned(),
        published_at: "2026-09-21T00:00:00Z".to_owned(),
        artifacts: vec![ArtifactEntry {
            target: TARGET.to_owned(),
            kind: "binary".to_owned(),
            file: published.clone(),
            name: installed,
            sha256: sha256_file(&path).expect("hashes"),
            size: u64::try_from(contents.len()).expect("fits"),
        }],
    };
    let bytes = manifest.to_bytes().expect("serializes");
    let envelope = sign_manifest(TEST_SECRET_KEY, &bytes).expect("signs");

    VerifiedRelease::verify(
        &bytes,
        &envelope.to_bytes().expect("serializes"),
        &directory,
    )
    .expect("a genuine signed release verifies")
}

/// Plans and applies an install or update for `version`, returning the outcome.
fn install_version(
    layout: &InstallLayout,
    profile: &ProfilePaths,
    root: &std::path::Path,
    version: &str,
) -> InstallAction {
    let release = verified_release(root, version, version.as_bytes());
    let state = observe(layout);
    let plan = if state.state.active.is_none() {
        plan_install(
            &state,
            profile,
            release.manifest(),
            release.artifacts(),
            TARGET,
        )
        .expect("a first install plans")
    } else {
        plan_update(
            &state,
            profile,
            release.manifest(),
            release.artifacts(),
            TARGET,
        )
        .expect("an update plans")
    };
    let outcome = apply(layout, profile, &plan, Some(&release), true, false).expect("applies");
    outcome.action
}

#[test]
fn a_first_install_stages_files_and_activates_the_version() {
    let (layout, profile, root) = fixture("install");
    let release = verified_release(&root, "0.1.0", b"jarvis-binary");
    let state = observe(&layout);

    let plan = plan_install(
        &state,
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("a first install plans");
    assert_eq!(plan.action(), InstallAction::Install);
    assert!(!plan.is_destructive());
    assert!(plan.from_version().is_none(), "nothing to update from");

    // The plan names the stable installed path, not the versioned download.
    assert_eq!(
        plan.files()[0].destination,
        layout.version_dir("0.1.0").join("bin").join("jarvis"),
    );

    let outcome = apply(&layout, &profile, &plan, Some(&release), true, false).expect("applies");
    assert_eq!(outcome.active.as_deref(), Some("0.1.0"));
    assert!(outcome.verified);
    assert_eq!(outcome.staged, 1);

    let after = Installation::observe(&layout);
    assert_eq!(after.active.as_deref(), Some("0.1.0"));
    assert!(after.installed.contains(&"0.1.0".to_owned()));
    assert!(after.previous.is_none(), "a first install has no previous");

    // The installed bytes are the verified bytes, and under the stable name.
    let installed = layout.binary_path("0.1.0", "jarvis");
    assert_eq!(
        std::fs::read(&installed).expect("readable"),
        b"jarvis-binary"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_update_records_the_previous_version_so_rollback_is_possible() {
    let (layout, profile, root) = fixture("update");

    assert_eq!(
        install_version(&layout, &profile, &root, "0.1.0"),
        InstallAction::Install,
    );

    let release = verified_release(&root, "0.2.0", b"second");
    let plan = plan_update(
        &observe(&layout),
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("an update plans");
    assert_eq!(plan.from_version(), Some("0.1.0"));
    assert_eq!(plan.version(), Some("0.2.0"));
    let outcome = apply(&layout, &profile, &plan, Some(&release), true, false).expect("applies");
    assert_eq!(outcome.active.as_deref(), Some("0.2.0"));

    let after = Installation::observe(&layout);
    assert_eq!(after.active.as_deref(), Some("0.2.0"));
    assert_eq!(
        after.previous.as_deref(),
        Some("0.1.0"),
        "the replaced version must be recorded for rollback"
    );
    assert_eq!(after.installed.len(), 2, "the old version is retained");

    // Rollback returns to 0.1.0 and leaves 0.2.0 on disk.
    let rollback = plan_rollback(&observe(&layout), &profile).expect("a rollback plans");
    assert_eq!(rollback.version(), Some("0.1.0"));
    let outcome = apply(&layout, &profile, &rollback, None, true, false).expect("rolls back");
    assert_eq!(outcome.active.as_deref(), Some("0.1.0"));
    let after = Installation::observe(&layout);
    assert!(
        after.has_version("0.2.0"),
        "rollback must not delete the new version"
    );
    // The rolled-back version's bytes are still the old ones.
    assert_eq!(
        std::fs::read(layout.binary_path("0.1.0", "jarvis")).expect("readable"),
        b"0.1.0",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_rollback_with_no_restorable_version_is_refused() {
    let (layout, profile, root) = fixture("rollback-missing");

    // Nothing installed at all.
    assert_eq!(
        plan_rollback(&observe(&layout), &profile),
        Err(InstallError::NothingActive),
    );

    // A version is active but no previous is recorded.
    install_version(&layout, &profile, &root, "0.1.0");

    assert!(
        matches!(
            plan_rollback(&observe(&layout), &profile),
            Err(InstallError::Unsupported { .. })
        ),
        "a rollback with no recorded previous must refuse rather than no-op",
    );

    // A previous version is recorded but its directory was removed.
    std::fs::remove_dir_all(layout.version_dir("0.1.0")).expect("removable");
    let mut state = observe(&layout);
    state.state.previous = Some("0.0.9".to_owned());
    assert_eq!(
        plan_rollback(&state, &profile),
        Err(InstallError::PreviousVersionMissing {
            version: "0.0.9".to_owned()
        }),
        "a rollback whose target is gone must say so, not silently succeed",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_uninstall_removes_program_files_and_preserves_user_data() {
    // This is the central FND-012 property. The database holds user memory; an
    // uninstall that removed it would be indistinguishable from data loss.
    let (layout, profile, root) = fixture("uninstall");
    install_version(&layout, &profile, &root, "0.1.0");

    // Seed user state that must survive.
    let database = profile.data_dir().join("jarvis.sqlite");
    std::fs::write(&database, b"durable user memory").expect("seed database");
    let credential = profile.config_dir().join("client-credential");
    std::fs::write(&credential, b"enrolled").expect("seed credential");

    let uninstall = plan_uninstall(&observe(&layout), &profile, false).expect("plans");
    assert_eq!(uninstall.action(), InstallAction::Uninstall);
    assert!(uninstall.is_destructive());
    // The rendered plan must name the retained data path, so an operator is told
    // where their data still is.
    assert!(uninstall.render().contains("user data: retained"));

    let outcome = apply(&layout, &profile, &uninstall, None, true, false).expect("applies");
    assert!(outcome.removed >= 2);
    assert!(
        outcome.active.is_none(),
        "no version is active after uninstall"
    );

    assert!(
        database.is_file(),
        "the database must survive an uninstall that did not purge",
    );
    assert!(
        credential.is_file(),
        "the credential must survive an uninstall that did not purge",
    );
    assert!(
        std::fs::read(&database).expect("readable") == b"durable user memory",
        "user data must be unchanged, not merely present",
    );
    assert!(
        !layout.version_dir("0.1.0").exists(),
        "program files must actually be removed",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_purge_is_refused_without_the_explicit_acknowledgement() {
    // A purge deletes the database. It must be impossible for a caller to reach it
    // by passing the same arguments an uninstall uses.
    let (layout, profile, root) = fixture("purge-refused");
    install_version(&layout, &profile, &root, "0.1.0");
    let database = profile.data_dir().join("jarvis.sqlite");
    std::fs::write(&database, b"precious").expect("seed");

    let purge = plan_uninstall(&observe(&layout), &profile, true).expect("plans");
    assert_eq!(purge.action(), InstallAction::Purge);
    assert!(purge.render().contains("REMOVED"));

    assert_eq!(
        apply(&layout, &profile, &purge, None, true, false),
        Err(InstallError::PurgeNotAcknowledged),
        "a purge without the acknowledgement must be refused",
    );
    assert!(
        database.is_file(),
        "a refused purge must not delete anything"
    );

    // And confirmed but unacknowledged is still refused.
    assert_eq!(
        apply(&layout, &profile, &purge, None, true, false),
        Err(InstallError::PurgeNotAcknowledged),
    );

    // Acknowledged, it proceeds and removes the data it named.
    let outcome = apply(&layout, &profile, &purge, None, true, true).expect("purges");
    assert!(outcome.removed >= 2);
    assert!(
        !database.exists(),
        "an acknowledged purge removes user data"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn every_action_refuses_without_confirmation() {
    let (layout, profile, root) = fixture("unconfirmed");
    let release = verified_release(&root, "0.1.0", b"bytes");
    let plan = plan_install(
        &observe(&layout),
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("plans");

    assert_eq!(
        apply(&layout, &profile, &plan, Some(&release), false, true),
        Err(InstallError::NotConfirmed),
        "confirmation is required, and the purge flag cannot substitute for it",
    );
    assert!(
        !layout.version_dir("0.1.0").exists(),
        "a refused plan must change nothing",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_destructive_action_is_refused_while_a_daemon_holds_the_lock() {
    // Swapping the binary beneath a live daemon is the race this guard exists for.
    let (layout, profile, root) = fixture("locking");
    let release = verified_release(&root, "0.1.0", b"bytes");
    let plan = plan_install(
        &observe(&layout),
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("plans");

    let lock_path = profile.runtime_dir().join("jarvis.lock");
    std::fs::create_dir_all(profile.runtime_dir()).expect("runtime dir");
    let guard = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    guard.try_lock().expect("a held exclusive lock");

    assert_eq!(
        apply(&layout, &profile, &plan, Some(&release), true, false),
        Err(InstallError::DaemonRunning),
        "an install must refuse while the instance lock is held",
    );
    // A refused activation must not even have staged the version directory: the
    // guard runs before any file is written.
    assert!(
        !layout.version_dir("0.1.0").exists(),
        "a refused install must not stage files",
    );

    drop(guard);
    // With the lock released the same plan applies, which proves the refusal was
    // caused by the lock and not by a broken fixture.
    apply(&layout, &profile, &plan, Some(&release), true, false)
        .expect("applies once the lock is free");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_install_path_inside_the_profile_is_refused() {
    let (layout, profile, root) = fixture("profile-guard");
    let release = verified_release(&root, "0.1.0", b"bytes");
    let mut plan = plan_install(
        &observe(&layout),
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("plans");

    // Point a planned file at the database.
    plan.files[0].destination = profile.data_dir().join("jarvis.sqlite");
    assert!(
        matches!(
            plan.validate_paths(&layout, &profile),
            Err(InstallError::PathOutsideInstallRoot { .. }
                | InstallError::PathInsideProfile { .. },)
        ),
        "a plan that targets user data must be refused",
    );
    assert_eq!(
        apply(&layout, &profile, &plan, Some(&release), true, false),
        Err(InstallError::PathOutsideInstallRoot {
            path: profile.data_dir().join("jarvis.sqlite")
        }),
        "apply must re-check, because a plan is data",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_path_outside_the_install_root_is_refused() {
    let (layout, profile, root) = fixture("outside");
    let release = verified_release(&root, "0.1.0", b"bytes");
    let mut plan = plan_install(
        &observe(&layout),
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("plans");

    plan.files[0].destination = root.join("elsewhere").join("jarvis");
    assert_eq!(
        plan.validate_paths(&layout, &profile),
        Err(InstallError::PathOutsideInstallRoot {
            path: root.join("elsewhere").join("jarvis")
        }),
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_unsafe_version_is_refused_before_it_becomes_a_directory() {
    // A version string reaches a directory name, so it is validated as a path
    // component and not merely as a display string.
    for bad in [
        "",
        "..",
        "../../escape",
        "1.0/2.0",
        r"1.0\2.0",
        "C:1.0",
        ".hidden",
        "1.0\nEVIL",
        &"9".repeat(super::MAX_VERSION_LEN + 1),
    ] {
        assert!(
            !is_safe_version(bad),
            "{bad:?} must be refused as a version"
        );
        assert_eq!(
            version_directory_name(bad),
            Err(InstallError::InvalidVersion),
            "{bad:?} must not produce a directory name",
        );
    }
    assert!(is_safe_version("0.1.0"));
    assert!(is_safe_version("1.0.0-rc.1+build.5"));
    assert_eq!(
        version_directory_name("0.1.0").expect("valid"),
        "v0.1.0".to_owned()
    );
    assert_eq!(version_from_directory_name("v0.1.0"), Some("0.1.0"));
    assert_eq!(version_from_directory_name("v"), None);
    // `vv1.0` would otherwise strip one `v` and read as the version `v1.0`.
    assert_eq!(version_from_directory_name("vv1.0"), None);
    assert_eq!(version_from_directory_name("versions"), None);
}

#[test]
fn a_foreign_directory_in_the_versions_root_is_not_an_installed_version() {
    let (layout, _profile, root) = fixture("foreign");
    std::fs::create_dir_all(layout.versions_dir().join("not-a-version")).expect("creatable");
    std::fs::create_dir_all(layout.versions_dir().join("v0.1.0")).expect("creatable");
    std::fs::write(layout.versions_dir().join("loose-file"), b"x").expect("writable");

    let installed = layout.installed_versions();
    assert_eq!(
        installed,
        vec!["0.1.0".to_owned()],
        "only real version directories count",
    );
    assert!(!observe(&layout).state.has_version("not-a-version"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_corrupt_active_pointer_reports_nothing_active_rather_than_guessing() {
    let (layout, _profile, root) = fixture("corrupt-pointer");
    std::fs::create_dir_all(layout.current_pointer()).expect("creatable");
    std::fs::write(
        layout.current_pointer().join(super::CURRENT_VERSION_FILE),
        "../../escape",
    )
    .expect("writable");

    assert_eq!(
        layout.active_version(),
        None,
        "an unusable pointer must not be interpreted as a version",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_update_with_nothing_installed_is_refused_rather_than_called_an_update() {
    let (layout, profile, root) = fixture("update-empty");
    let release = verified_release(&root, "0.2.0", b"bytes");

    assert_eq!(
        plan_update(
            &observe(&layout),
            &profile,
            release.manifest(),
            release.artifacts(),
            TARGET
        ),
        Err(InstallError::NothingActive),
        "an update implies a version to update from and a rollback target",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn installing_the_active_version_again_is_refused() {
    let (layout, profile, root) = fixture("already-active");
    install_version(&layout, &profile, &root, "0.1.0");
    let release = verified_release(&root, "0.1.0", b"0.1.0");

    assert_eq!(
        plan_install(
            &observe(&layout),
            &profile,
            release.manifest(),
            release.artifacts(),
            TARGET
        ),
        Err(InstallError::AlreadyActive {
            version: "0.1.0".to_owned()
        }),
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_active_version_and_the_rollback_target_are_never_pruned() {
    let (layout, profile, root) = fixture("prune-guard");
    install_version(&layout, &profile, &root, "0.1.0");
    install_version(&layout, &profile, &root, "0.2.0");

    // 0.2.0 is active and 0.1.0 is the rollback target.
    assert!(
        matches!(
            plan_prune(&observe(&layout), &profile, "0.2.0"),
            Err(InstallError::Unsupported { .. })
        ),
        "the active version is never pruned",
    );
    assert!(
        matches!(
            plan_prune(&observe(&layout), &profile, "0.1.0"),
            Err(InstallError::Unsupported { .. })
        ),
        "the rollback target is never pruned",
    );
    assert!(
        matches!(
            plan_prune(&observe(&layout), &profile, "9.9.9"),
            Err(InstallError::Unsupported { .. })
        ),
        "an uninstalled version is not pruned",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_prunable_version_is_removed_and_others_are_kept() {
    let (layout, profile, root) = fixture("prune");

    for version in ["0.1.0", "0.2.0", "0.3.0"] {
        install_version(&layout, &profile, &root, version);
    }

    // 0.3.0 active, 0.2.0 rollback target, so 0.1.0 is prunable.
    let plan = plan_prune(&observe(&layout), &profile, "0.1.0").expect("plans");
    assert!(plan.is_destructive());
    let outcome = apply(&layout, &profile, &plan, None, true, false).expect("prunes");
    assert_eq!(outcome.removed, 1);

    let after = Installation::observe(&layout);
    assert!(!after.has_version("0.1.0"));
    assert!(after.has_version("0.2.0"));
    assert!(after.has_version("0.3.0"));
    assert_eq!(after.active.as_deref(), Some("0.3.0"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_uninstall_that_was_interrupted_converges_when_re_run() {
    // An uninstall must be idempotent, or a retry after a partial failure fails
    // forever on the paths it already removed.
    let (layout, profile, root) = fixture("converge");
    install_version(&layout, &profile, &root, "0.1.0");

    let uninstall = plan_uninstall(&observe(&layout), &profile, false).expect("plans");
    apply(&layout, &profile, &uninstall, None, true, false).expect("first uninstall");

    // Re-observing shows nothing installed, so the plan refuses rather than
    // reporting a successful removal of nothing.
    assert!(matches!(
        plan_uninstall(&observe(&layout), &profile, false),
        Err(InstallError::Unsupported { .. })
    ));

    // Applying the original plan again still converges without error, because
    // removing an already-absent path is treated as success.
    apply(&layout, &profile, &uninstall, None, true, false).expect("re-run converges");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_portable_layout_keeps_every_path_under_its_root() {
    let (layout, profile, root) = fixture("portable");
    assert_eq!(layout.mode(), InstallMode::Portable);
    assert_eq!(profile.mode().token(), "portable");
    assert!(layout.root().starts_with(&root));
    assert!(layout.versions_dir().starts_with(&root));
    assert!(
        layout
            .version_dir("0.1.0")
            .starts_with(layout.versions_dir())
    );
    assert!(layout.contains(&layout.binary_path("0.1.0", "jarvisd")));

    // A lexical prefix must not be treated as containment.
    assert!(!layout.contains(&root.join("installer").join("jarvis")));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_staged_version_is_complete_once_its_binary_exists() {
    let (layout, profile, root) = fixture("complete");
    install_version(&layout, &profile, &root, "0.1.0");

    // The install stages the artifact under its stable name `jarvis`.
    assert!(version_is_complete(&layout, "0.1.0", "jarvis"));
    assert!(!version_is_complete(&layout, "0.1.0", "jarvisd"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_release_with_no_artifact_for_the_target_is_refused() {
    let (layout, profile, root) = fixture("wrong-target");
    // Build a release for a different platform by hand: the signature and digest
    // are real, only the target differs.
    let other = "aarch64-apple-darwin";
    let directory = root.join("other-target");
    std::fs::create_dir_all(&directory).expect("creatable");
    let path = directory.join("jarvis");
    std::fs::write(&path, b"bytes").expect("writable");
    let manifest = ReleaseManifest {
        schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
        version: "0.1.0".to_owned(),
        channel: Channel::Development,
        target: other.to_owned(),
        api_major: 1,
        min_data_version: 1,
        build: "test-build".to_owned(),
        published_at: "2026-09-21T00:00:00Z".to_owned(),
        artifacts: vec![ArtifactEntry {
            target: other.to_owned(),
            kind: "binary".to_owned(),
            file: "jarvis".to_owned(),
            name: "jarvis".to_owned(),
            sha256: sha256_file(&path).expect("hashes"),
            size: 5,
        }],
    };
    let bytes = manifest.to_bytes().expect("serializes");
    let envelope = sign_manifest(TEST_SECRET_KEY, &bytes).expect("signs");
    let release = VerifiedRelease::verify(
        &bytes,
        &envelope.to_bytes().expect("serializes"),
        &directory,
    )
    .expect("verifies");

    assert_eq!(
        plan_install(
            &observe(&layout),
            &profile,
            release.manifest(),
            release.artifacts(),
            TARGET
        ),
        Err(InstallError::ArtifactUnverified {
            code: "jarvis.install_no_artifact_for_target".to_owned()
        }),
        "installing a release with no artifact for this platform must refuse",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_stable_launcher_path_is_not_version_specific() {
    // A service definition points at this path, so it must not contain a version.
    let (layout, _profile, root) = fixture("launcher");
    let launcher = super::plan::stable_launcher(&layout, "jarvisd");
    let text = launcher.to_string_lossy();
    assert!(!text.contains("versions"), "{text}");
    assert!(!text.contains("v0."), "{text}");
    assert!(launcher.starts_with(layout.root()));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn codes_are_namespaced_and_unique() {
    let errors = [
        InstallError::InvalidVersion,
        InstallError::PathInsideProfile {
            path: PathBuf::from("a"),
        },
        InstallError::PathOutsideInstallRoot {
            path: PathBuf::from("b"),
        },
        InstallError::AlreadyActive {
            version: "1".to_owned(),
        },
        InstallError::NothingActive,
        InstallError::PreviousVersionMissing {
            version: "1".to_owned(),
        },
        InstallError::ArtifactUnverified {
            code: "c".to_owned(),
        },
        InstallError::DaemonRunning,
        InstallError::Io {
            path: PathBuf::from("c"),
        },
        InstallError::PostconditionFailed {
            detail: "d".to_owned(),
        },
        InstallError::NotConfirmed,
        InstallError::PurgeNotAcknowledged,
        InstallError::Unsupported {
            reason: "e".to_owned(),
        },
    ];

    let mut codes: Vec<&str> = errors.iter().map(InstallError::code).collect();
    for code in &codes {
        assert!(code.starts_with("jarvis.install_"), "{code}");
    }
    codes.sort_unstable();
    let count = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), count, "codes must be unique");

    // A deterministic refusal is not made better by retrying it.
    assert!(
        !InstallError::AlreadyActive {
            version: "1".to_owned()
        }
        .retryable()
    );
    assert!(!InstallError::PurgeNotAcknowledged.retryable());
    assert!(
        InstallError::Io {
            path: PathBuf::from("x")
        }
        .retryable()
    );
}

#[test]
fn an_unverified_release_artifact_list_cannot_produce_an_install_plan() {
    // `plan_install` takes verified artifacts, so an empty verified list has
    // nothing to stage and must refuse rather than produce an empty install.
    let (layout, profile, root) = fixture("unverified");
    let release = verified_release(&root, "0.1.0", b"bytes");
    assert_eq!(
        plan_install(&observe(&layout), &profile, release.manifest(), &[], TARGET),
        Err(InstallError::ArtifactUnverified {
            code: "jarvis.install_no_artifact_for_target".to_owned()
        }),
        "a release with no verified artifacts must not become an install",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_install_without_a_verified_release_is_refused_before_staging() {
    // The typed gate: an install or activate action with no verified release has
    // no bytes to install, so it must refuse rather than create an empty version
    // directory that `current` then points at.
    let (layout, profile, root) = fixture("no-release");
    let release = verified_release(&root, "0.1.0", b"bytes");
    let plan = plan_install(
        &observe(&layout),
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("plans");

    assert_eq!(
        apply(&layout, &profile, &plan, None, true, false),
        Err(InstallError::ArtifactUnverified {
            code: "jarvis.install_no_verified_release".to_owned()
        }),
    );
    assert!(
        !layout.version_dir("0.1.0").exists(),
        "a refused install must not create the version directory",
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_release_digest_mismatch_from_disk_is_surfaced_as_unverified() {
    // The apply path re-verifies bytes from disk, so a download that changed after
    // verification is refused before anything is staged.
    let (_layout, _profile, root) = fixture("digest");
    let staging = root.join("download");
    std::fs::create_dir_all(&staging).expect("creatable");
    // The file is written at a *different length* than the manifest records, so
    // the size check refuses it before any hashing is attempted.
    std::fs::write(staging.join("jarvis"), b"tampered").expect("writable");

    let manifest = ReleaseManifest {
        schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
        version: "0.1.0".to_owned(),
        channel: Channel::Development,
        target: TARGET.to_owned(),
        api_major: 1,
        min_data_version: 1,
        build: "test-build".to_owned(),
        published_at: "2026-09-21T00:00:00Z".to_owned(),
        artifacts: vec![ArtifactEntry {
            target: TARGET.to_owned(),
            kind: "binary".to_owned(),
            file: "jarvis".to_owned(),
            name: "jarvis".to_owned(),
            sha256: "b".repeat(64),
            // The file on disk is 8 bytes.
            size: 5,
        }],
    };
    assert_eq!(
        super::apply::verify_staged(&manifest, &staging),
        Err(InstallError::ArtifactUnverified {
            code: "jarvis.release_size_mismatch".to_owned()
        }),
        "a file that does not match its signed size must be refused",
    );

    // With the real size but a wrong digest, the digest check refuses it.
    let mut wrong_digest = manifest.clone();
    wrong_digest.artifacts[0].size = 8;
    assert_eq!(
        super::apply::verify_staged(&wrong_digest, &staging),
        Err(InstallError::ArtifactUnverified {
            code: "jarvis.release_digest_mismatch".to_owned()
        }),
        "a file that does not match its signed digest must be refused",
    );

    // With the real digest and size it verifies, so the refusals came from the
    // mismatches and not from an unusable fixture.
    let mut correct = manifest.clone();
    correct.artifacts[0].sha256 = sha256_file(&staging.join("jarvis")).expect("hashes");
    correct.artifacts[0].size = 8;
    assert!(super::apply::verify_staged(&correct, &staging).is_ok());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_prune_all_never_removes_every_path_back_to_a_working_version() {
    let (layout, profile, root) = fixture("prune-all");

    for version in ["0.1.0", "0.2.0", "0.3.0"] {
        install_version(&layout, &profile, &root, version);
    }

    // The active version (0.3.0) and the rollback target (0.2.0) are refused, so
    // only 0.1.0 can be pruned. A bulk prune can therefore never remove every
    // path back to a working version.
    let state = observe(&layout);
    let prunable: Vec<String> = state
        .state
        .installed
        .iter()
        .filter(|version| plan_prune(&state, &profile, version).is_ok())
        .cloned()
        .collect();
    assert_eq!(prunable, vec!["0.1.0".to_owned()]);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_install_on_a_fresh_layout_plans_without_touching_the_host() {
    // Building a plan must be side-effect free, or "preview then confirm" cannot
    // be trusted.
    let (layout, profile, root) = fixture("pure-plan");
    let release = verified_release(&root, "0.1.0", b"bytes");

    let plan = plan_install(
        &observe(&layout),
        &profile,
        release.manifest(),
        release.artifacts(),
        TARGET,
    )
    .expect("plans");
    assert_eq!(plan.files().len(), 1);
    assert!(
        !layout.version_dir("0.1.0").exists(),
        "planning must not create the version directory",
    );
    assert!(!layout.root().join("previous.version").exists());
    let rendered = plan.render();
    assert!(rendered.contains("install plan: install"));
    assert!(rendered.contains("destructive: no"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn existing_install_reports_state_from_the_pointer_not_from_directories() {
    // A version directory that exists but is not pointed at is not active. Asking
    // "which directory exists" would report a broken install as working.
    let (layout, _profile, root) = fixture("state-source");
    std::fs::create_dir_all(layout.version_dir("0.1.0").join("bin")).expect("creatable");
    std::fs::create_dir_all(layout.version_dir("0.2.0").join("bin")).expect("creatable");

    let existing = ExistingInstall::observe(&layout);
    assert_eq!(existing.state.installed.len(), 2);
    assert_eq!(
        existing.state.active, None,
        "existing directories do not make a version active",
    );
    assert!(!existing.state.is_installed());
    std::fs::remove_dir_all(&root).ok();
}
