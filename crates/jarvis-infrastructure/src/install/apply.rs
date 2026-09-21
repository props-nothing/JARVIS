//! Applying an install plan: staging, atomic activation, and verification.
//!
//! ## The order is the safety property
//!
//! 1. Re-validate the plan's paths, because a plan is data.
//! 2. Refuse while a daemon holds the instance lock: activating a new version
//!    under a running daemon would swap the binary beneath a live process.
//! 3. Check the postcondition **before** applying, so a plan whose outcome already
//!    holds is refused instead of "succeeding" without changing anything.
//! 4. Stage every file into a fresh version directory, then flush the directory.
//! 5. Activate by replacing the pointer, then flush the parent directory.
//! 6. Check the postcondition **again**. A failed check restores the previous
//!    pointer, so a failed activation leaves the old version active rather than a
//!    half-switched install.
//!
//! Nothing in this module writes into the user profile. The only paths it touches
//! are inside the install root, and a purge — the sole exception — reaches profile
//! directories only because the caller asked for a purge explicitly.

use std::fs;
use std::path::{Path, PathBuf};

use crate::lifecycle::appears_unheld;
use crate::paths::ProfilePaths;
use crate::release::{ReleaseManifest, VerifiedArtifact, verify_artifacts};

use super::{InstallAction, InstallError, InstallLayout, InstallPlan, Installation};
// The pointer file name is only read on Windows; on Unix the pointer is a symlink
// whose target names the version, so the constant is unused there and importing it
// unconditionally fails a Linux build with warnings denied.
#[cfg(not(unix))]
use super::CURRENT_VERSION_FILE;

/// What applying a plan did, for the operator's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    /// The action that ran.
    pub action: InstallAction,
    /// The version now active.
    pub active: Option<String>,
    /// How many files were staged.
    pub staged: usize,
    /// How many paths were removed.
    pub removed: usize,
    /// Whether the postcondition was verified.
    pub verified: bool,
    /// A plain description of the verified state.
    pub detail: String,
}

/// A verified release, ready to be installed.
///
/// This is the type that makes "only verified bytes become the running program" a
/// compile-time property rather than a code-review property: [`apply`] accepts
/// bytes only through this value, and the only way to build one is
/// [`VerifiedRelease::verify`], which verifies the manifest signature and every
/// artifact digest first. A caller cannot pass a directory of loose files.
#[derive(Debug)]
pub struct VerifiedRelease {
    manifest: ReleaseManifest,
    directory: PathBuf,
    artifacts: Vec<VerifiedArtifact>,
}

impl VerifiedRelease {
    /// Verifies a signed manifest and every artifact it lists.
    ///
    /// The manifest signature is checked against the trust store compiled into
    /// this binary, then each artifact's digest and size are checked from disk.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::ArtifactUnverified`] carrying the stable release
    /// code from the first failure, so an operator sees *why* — an unknown key and
    /// a tampered byte must not look the same.
    pub fn verify(
        manifest_bytes: &[u8],
        signature_bytes: &[u8],
        directory: &Path,
    ) -> Result<Self, InstallError> {
        let store = crate::release::TrustStore::builtin();
        let manifest = store
            .verify_manifest(manifest_bytes, signature_bytes)
            .map_err(|error| InstallError::ArtifactUnverified {
                code: error.code().to_owned(),
            })?;
        let artifacts = verify_artifacts(&manifest, directory).map_err(|error| {
            InstallError::ArtifactUnverified {
                code: error.code().to_owned(),
            }
        })?;
        Ok(Self {
            manifest,
            directory: directory.to_path_buf(),
            artifacts,
        })
    }

    /// Returns the verified manifest.
    #[must_use]
    pub fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    /// Returns the verified artifacts.
    #[must_use]
    pub fn artifacts(&self) -> &[VerifiedArtifact] {
        &self.artifacts
    }

    /// Returns the directory the verified bytes are in.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Returns the version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    /// Re-verifies the artifacts from disk.
    ///
    /// Called by [`apply`] immediately before staging. A plan is data that may have
    /// travelled between planning and applying, so the bytes about to become the
    /// running program are re-confirmed rather than trusted from an earlier check.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::ArtifactUnverified`] when a digest or size no longer
    /// matches.
    pub fn revalidate(&self) -> Result<(), InstallError> {
        verify_artifacts(&self.manifest, &self.directory)
            .map(|_| ())
            .map_err(|error| InstallError::ArtifactUnverified {
                code: error.code().to_owned(),
            })
    }
}

/// Applies `plan` and verifies its postcondition.
///
/// `confirmed` must be `true`. There is no default, for the same reason a repair
/// has none: an install that ran without a decision is exactly the failure mode
/// this design exists to prevent.
///
/// `acknowledged_purge` must be `true` for a purge and is **ignored otherwise**, so
/// a caller cannot accidentally pass it and turn a data-retaining uninstall into a
/// destructive one.
///
/// # Errors
///
/// Returns [`InstallError::NotConfirmed`], [`InstallError::PurgeNotAcknowledged`],
/// [`InstallError::DaemonRunning`], [`InstallError::PathInsideProfile`],
/// [`InstallError::AlreadyActive`], or [`InstallError::PostconditionFailed`].
pub fn apply(
    layout: &InstallLayout,
    paths: &ProfilePaths,
    plan: &InstallPlan,
    release: Option<&VerifiedRelease>,
    confirmed: bool,
    acknowledged_purge: bool,
) -> Result<InstallOutcome, InstallError> {
    if !confirmed {
        return Err(InstallError::NotConfirmed);
    }

    // A purge is the one action allowed to touch user data, and only when the
    // caller acknowledged it explicitly. Checking the flag against the action
    // means a caller cannot pass `true` and have it change a non-purge plan.
    if plan.action() == InstallAction::Purge && !acknowledged_purge {
        return Err(InstallError::PurgeNotAcknowledged);
    }

    // Validate the plan's paths again. It is data, and the profile guard is what
    // makes "retains data" structural rather than intended.
    if plan.action() == InstallAction::Purge {
        // For a purge, the removed *profile* paths are the point, but every
        // install-root path must still be inside the root.
        for path in plan.files().iter().map(|file| file.destination.as_path()) {
            if !layout.contains(path) {
                return Err(InstallError::PathOutsideInstallRoot {
                    path: path.to_path_buf(),
                });
            }
        }
    } else {
        plan.validate_paths(layout, paths)?;
    }

    let before = Installation::observe(layout);

    // A version-changing action must refuse while a daemon is live. Removing files
    // is equally unsafe, so the check covers every mutating action.
    guard_daemon_not_running(paths)?;

    match plan.action() {
        InstallAction::Install | InstallAction::Activate => {
            if before.active.as_deref() == plan.version() {
                return Err(InstallError::AlreadyActive {
                    version: plan.version().unwrap_or_default().to_owned(),
                });
            }
        }
        InstallAction::Rollback => {
            let Some(target) = plan.version() else {
                return Err(InstallError::NothingActive);
            };
            if !before.has_version(target) {
                return Err(InstallError::PreviousVersionMissing {
                    version: target.to_owned(),
                });
            }
        }
        InstallAction::Prune | InstallAction::Uninstall | InstallAction::Purge => {}
    }

    require_release_for_activation(plan, release)?;

    let staged = match plan.action() {
        InstallAction::Install | InstallAction::Activate => {
            let release = release.ok_or(InstallError::ArtifactUnverified {
                code: "jarvis.install_no_verified_release".to_owned(),
            })?;
            stage_files(layout, plan, release)?
        }
        _ => 0,
    };

    let removed = remove_paths(layout, plan)?;

    // Activation last, so a failure while staging or removing leaves the previous
    // version active. The pointer is the single switch.
    let activated = match plan.action() {
        InstallAction::Install | InstallAction::Activate | InstallAction::Rollback => {
            activate(layout, plan)?
        }
        InstallAction::Prune | InstallAction::Uninstall | InstallAction::Purge => {
            // These actions do not activate anything, so the pointer is only
            // touched when its target was removed.
            if matches!(
                plan.action(),
                InstallAction::Uninstall | InstallAction::Purge
            ) {
                clear_pointer(layout)?;
            }
            false
        }
    };

    let after = Installation::observe(layout);
    let desired = desired_version(plan);
    let reached = match plan.action() {
        InstallAction::Install | InstallAction::Activate | InstallAction::Rollback => {
            after.active.as_deref() == desired.as_deref()
        }
        InstallAction::Uninstall | InstallAction::Purge => after.active.is_none(),
        InstallAction::Prune => plan
            .version()
            .is_none_or(|version| !after.has_version(version)),
    };

    if !reached {
        // Restore the previous pointer so a failed activation does not leave the
        // install pointing at a version that is not there.
        if activated {
            rollback_pointer(layout, before.active.as_deref())?;
        }
        return Err(InstallError::PostconditionFailed {
            detail: format!(
                "expected active version {desired:?}, observed {:?}",
                after.active
            ),
        });
    }

    Ok(InstallOutcome {
        action: plan.action(),
        active: after.active,
        staged,
        removed,
        verified: true,
        detail: match desired {
            Some(version) => format!("version {version} is active"),
            None => "no version is active".to_owned(),
        },
    })
}

/// Stops an activation whose postcondition cannot be reached when no release was
/// supplied.
///
/// An install or activate action with no verified release has nothing to stage, so
/// it is refused before any file is written.
fn require_release_for_activation(
    plan: &InstallPlan,
    release: Option<&VerifiedRelease>,
) -> Result<(), InstallError> {
    let activating = matches!(
        plan.action(),
        InstallAction::Install | InstallAction::Activate
    );
    if activating && release.is_none() {
        return Err(InstallError::ArtifactUnverified {
            code: "jarvis.install_no_verified_release".to_owned(),
        });
    }
    Ok(())
}

/// Verifies a release's artifacts and stages them into the version directory.
///
/// The release is re-verified **here, from the files on disk**, rather than trusted
/// from the planning step. A plan is data that may have travelled, and these bytes
/// are about to become the running program, so this is the last place where
/// confirming they are the signed bytes is still possible.
///
/// Files are **copied** from the verified download directory into the version
/// directory. Nothing is written onto the source, so a staging failure cannot
/// corrupt the artifact that was just verified.
fn stage_files(
    layout: &InstallLayout,
    plan: &InstallPlan,
    release: &VerifiedRelease,
) -> Result<usize, InstallError> {
    let Some(version) = plan.version() else {
        return Err(InstallError::InvalidVersion);
    };
    release.revalidate()?;

    let version_dir = layout.version_dir(version);
    let bin_dir = version_dir.join("bin");
    ensure_private_dir(&version_dir)?;
    ensure_private_dir(&bin_dir)?;

    let mut staged = 0_usize;
    for artifact in release.artifacts() {
        let source = release.directory().join(&artifact.file);
        // The installed name is the stable one from the signed manifest, not the
        // version-named download file. A service points at the stable name.
        let destination = bin_dir.join(&artifact.name);
        let bytes = fs::read(&source).map_err(|_| InstallError::Io {
            path: source.clone(),
        })?;
        // The size is re-checked against the verified value, so a file that grew
        // or shrank since verification is refused rather than staged.
        if bytes.len() as u64 != artifact.size {
            return Err(InstallError::ArtifactUnverified {
                code: "jarvis.install_size_mismatch".to_owned(),
            });
        }
        write_private_file(&destination, &bytes)?;
        staged += 1;
    }
    flush_directory(&bin_dir);
    flush_directory(&version_dir);
    Ok(staged)
}

/// Verifies a release's artifacts from a directory.
///
/// Exposed so a caller can check the bytes it is about to install without building
/// a plan, and so the verification step has a nameable, testable entry point.
///
/// # Errors
///
/// Returns [`InstallError::ArtifactUnverified`] carrying the stable release code.
pub fn verify_staged(
    manifest: &ReleaseManifest,
    directory: &Path,
) -> Result<Vec<VerifiedArtifact>, InstallError> {
    verify_artifacts(manifest, directory).map_err(|error| InstallError::ArtifactUnverified {
        code: error.code().to_owned(),
    })
}

/// Removes every path a plan lists, newest listed first.
fn remove_paths(layout: &InstallLayout, plan: &InstallPlan) -> Result<usize, InstallError> {
    let mut removed = 0_usize;
    for path in plan.removed() {
        if !layout.contains(path) && plan.action() != InstallAction::Purge {
            return Err(InstallError::PathOutsideInstallRoot { path: path.clone() });
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => {
                fs::remove_dir_all(path).map_err(|_| InstallError::Io { path: path.clone() })?;
                removed += 1;
            }
            Ok(_) => {
                fs::remove_file(path).map_err(|_| InstallError::Io { path: path.clone() })?;
                removed += 1;
            }
            // Already absent is success: an uninstall that was interrupted and
            // re-run must converge rather than fail.
            Err(_) => {}
        }
    }
    Ok(removed)
}

/// Makes the plan's version active.
///
/// On Unix the pointer is a symlink replaced atomically. On Windows, where
/// creating a symlink needs elevation or developer mode, the pointer is a
/// directory holding a version name, and the name is written atomically through a
/// temp file and a rename, so a reader never sees a half-written name.
fn activate(layout: &InstallLayout, plan: &InstallPlan) -> Result<bool, InstallError> {
    let Some(version) = plan.version() else {
        return Err(InstallError::InvalidVersion);
    };
    let target = layout.version_dir(version);
    if !target.is_dir() {
        return Err(InstallError::Io { path: target });
    }

    // Record the version being replaced so a rollback has a target.
    if let Some(previous) = Installation::observe(layout).active {
        write_previous(layout, &previous)?;
    }

    write_pointer(&layout.current_pointer(), version)?;
    flush_directory(layout.root());
    Ok(true)
}

/// Writes an active-version pointer.
fn write_pointer(pointer: &Path, version: &str) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let target = PathBuf::from(super::VERSION_PREFIX.to_owned() + version);
        let staged = pointer.with_extension("next");
        let _ = fs::remove_file(&staged);
        symlink(
            PathBuf::from(super::VERSIONS_DIRECTORY).join(&target),
            &staged,
        )
        .map_err(|_| InstallError::Io {
            path: pointer.to_path_buf(),
        })?;
        // `rename` onto an existing symlink replaces it atomically.
        fs::rename(&staged, pointer).map_err(|_| InstallError::Io {
            path: pointer.to_path_buf(),
        })
    }
    #[cfg(not(unix))]
    {
        // The pointer is a directory containing a version name. The name is
        // replaced atomically, so a concurrent reader sees either the old name or
        // the new one.
        ensure_private_dir(pointer)?;
        let file = pointer.join(CURRENT_VERSION_FILE);
        let staged = file.with_extension("next");
        fs::write(&staged, version).map_err(|_| InstallError::Io {
            path: staged.clone(),
        })?;
        fs::rename(&staged, &file).map_err(|_| InstallError::Io { path: file })?;
        Ok(())
    }
}

/// Rewrites the active pointer to a version, or clears it.
fn rollback_pointer(layout: &InstallLayout, version: Option<&str>) -> Result<(), InstallError> {
    match version {
        Some(version) => write_pointer(&layout.current_pointer(), version),
        None => clear_pointer(layout),
    }
}

/// Removes the active-version pointer.
fn clear_pointer(layout: &InstallLayout) -> Result<(), InstallError> {
    let pointer = layout.current_pointer();
    match fs::symlink_metadata(&pointer) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(&pointer).map_err(|_| InstallError::Io {
                path: pointer.clone(),
            })?;
        }
        Ok(_) => {
            fs::remove_file(&pointer).map_err(|_| InstallError::Io {
                path: pointer.clone(),
            })?;
        }
        Err(_) => {}
    }
    let _ = fs::remove_file(layout.root().join("previous.version"));
    Ok(())
}

/// Records the version a rollback would return to.
fn write_previous(layout: &InstallLayout, version: &str) -> Result<(), InstallError> {
    let path = layout.root().join("previous.version");
    let staged = path.with_extension("next");
    fs::write(&staged, version).map_err(|_| InstallError::Io {
        path: staged.clone(),
    })?;
    fs::rename(&staged, &path).map_err(|_| InstallError::Io { path })?;
    Ok(())
}

/// Refuses to change an install while a daemon holds the instance lock.
fn guard_daemon_not_running(paths: &ProfilePaths) -> Result<(), InstallError> {
    let lock = paths.runtime_dir().join("jarvis.lock");
    if !lock.exists() || appears_unheld(&lock) {
        Ok(())
    } else {
        Err(InstallError::DaemonRunning)
    }
}

/// Returns the version the plan should leave active.
fn desired_version(plan: &InstallPlan) -> Option<String> {
    match plan.action() {
        InstallAction::Install | InstallAction::Activate | InstallAction::Rollback => {
            plan.version().map(str::to_owned)
        }
        InstallAction::Prune | InstallAction::Uninstall | InstallAction::Purge => None,
    }
}

/// Creates a directory and every missing parent with owner-only permissions.
fn ensure_private_dir(path: &Path) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(|_| InstallError::Io {
            path: path.to_path_buf(),
        })
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path).map_err(|_| InstallError::Io {
            path: path.to_path_buf(),
        })
    }
}

/// Writes a file with owner-only permissions.
fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o700)
            .open(path)
            .map_err(|_| InstallError::Io {
                path: path.to_path_buf(),
            })?;
        file.write_all(contents).map_err(|_| InstallError::Io {
            path: path.to_path_buf(),
        })?;
        file.sync_all().map_err(|_| InstallError::Io {
            path: path.to_path_buf(),
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents).map_err(|_| InstallError::Io {
            path: path.to_path_buf(),
        })
    }
}

/// Flushes a directory entry so a completed rename survives a crash.
///
/// A rename is atomic, but its durability is a separate property. Opening the
/// directory and syncing it is the portable way to make the new entry durable.
fn flush_directory(path: &Path) {
    #[cfg(unix)]
    {
        if let Ok(handle) = fs::File::open(path) {
            let _ = handle.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Returns whether a version directory is present and holds a binary.
#[must_use]
pub fn version_is_complete(layout: &InstallLayout, version: &str, installed_name: &str) -> bool {
    layout.binary_path(version, installed_name).is_file()
}
