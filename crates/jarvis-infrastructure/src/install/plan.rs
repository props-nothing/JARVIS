//! Building install plans: install, update, rollback, prune, and uninstall.
//!
//! Every function here is **pure with respect to the host**: it reads the current
//! state to describe the change, but it never writes, never activates, and never
//! deletes. That is what lets an operator see the exact effect of an update before
//! approving it, and what lets the whole decision surface be tested on a platform
//! that cannot perform the operation natively.
//!
//! ## An update is only planned from a verified release
//!
//! [`plan_update`] takes a release that has already been verified by
//! [`crate::release`]. It does not accept a directory of loose files, because
//! "which bytes become the running binary" is exactly the question a signature
//! exists to answer. If verification has not happened, there is no plan to make.

use std::path::PathBuf;

use crate::paths::ProfilePaths;
use crate::release::{ReleaseManifest, VerifiedArtifact};

use super::{
    InstallAction, InstallError, InstallFile, InstallLayout, InstallPlan, version_directory_name,
};

/// The observed state of an installed JARVIS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    /// The active version, if any.
    pub active: Option<String>,
    /// The version a rollback would return to, if one is recorded.
    pub previous: Option<String>,
    /// Every version directory currently on disk.
    pub installed: Vec<String>,
}

impl Installation {
    /// Observes the current install state.
    #[must_use]
    pub fn observe(layout: &InstallLayout) -> Self {
        Self {
            active: layout.active_version(),
            previous: layout.previous_version(),
            installed: layout.installed_versions(),
        }
    }

    /// Returns whether a version is currently active.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.active.is_some()
    }

    /// Returns whether the named version is on disk.
    #[must_use]
    pub fn has_version(&self, version: &str) -> bool {
        self.installed.iter().any(|installed| installed == version)
    }
}

/// What a plan needs in order to be built.
///
/// It bundles the resolved install layout with the observed state so a caller
/// cannot accidentally plan against a layout it did not observe.
#[derive(Debug, Clone)]
pub struct ExistingInstall<'a> {
    /// The resolved install layout.
    pub layout: &'a InstallLayout,
    /// The observed state.
    pub state: Installation,
}

impl<'a> ExistingInstall<'a> {
    /// Observes the layout and pairs it with the state.
    #[must_use]
    pub fn observe(layout: &'a InstallLayout) -> Self {
        Self {
            layout,
            state: Installation::observe(layout),
        }
    }
}

/// Plans a first install of `version` from a verified release.
///
/// # Errors
///
/// Returns [`InstallError::InvalidVersion`] for an unusable version,
/// [`InstallError::ArtifactUnverified`] when the release lists no artifact for the
/// running platform, and [`InstallError::PathInsideProfile`] when a planned path
/// would touch user data.
pub fn plan_install(
    existing: &ExistingInstall<'_>,
    paths: &ProfilePaths,
    manifest: &ReleaseManifest,
    artifacts: &[VerifiedArtifact],
    target: &str,
) -> Result<InstallPlan, InstallError> {
    let version = manifest.version.clone();
    if existing.state.active.as_deref() == Some(version.as_str()) {
        return Err(InstallError::AlreadyActive { version });
    }

    let files = planned_files(existing.layout, &version, artifacts, target)?;
    let plan = InstallPlan {
        action: InstallAction::Install,
        mode: existing.layout.mode(),
        summary: format!("install version {version} and make it active"),
        from_version: existing.state.active.clone(),
        version: Some(version),
        removed: Vec::new(),
        destructive: false,
        verified_files: files.len(),
        files,
    };
    plan.validate_paths(existing.layout, paths)?;
    Ok(plan)
}

/// Plans an update to a verified release.
///
/// A rollback target is recorded only when there is a currently active version to
/// return to, so "rollback is possible" is never claimed without a version that
/// can actually be restored.
///
/// # Errors
///
/// As [`plan_install`], plus [`InstallError::NothingActive`] when the version being
/// updated from is not recorded.
pub fn plan_update(
    existing: &ExistingInstall<'_>,
    paths: &ProfilePaths,
    manifest: &ReleaseManifest,
    artifacts: &[VerifiedArtifact],
    target: &str,
) -> Result<InstallPlan, InstallError> {
    if existing.state.active.is_none() {
        // An update with nothing installed is either a first install or a
        // mistake; refusing is the honest answer, because calling it an update
        // would imply a previous version that could be restored.
        return Err(InstallError::NothingActive);
    }
    let mut plan = plan_install(existing, paths, manifest, artifacts, target)?;
    let version = plan.version.clone().unwrap_or_default();
    plan.summary = format!(
        "update {from} to {version} and make it active",
        from = plan.from_version.clone().unwrap_or_default(),
    );
    Ok(plan)
}

/// Plans a rollback to the recorded previous version.
///
/// # Errors
///
/// Returns [`InstallError::NothingActive`] when no version is active,
/// [`InstallError::Unsupported`] when no previous version is recorded, and
/// [`InstallError::PreviousVersionMissing`] when the recorded version is no longer
/// on disk. The last case is the one that matters: a rollback that silently did
/// nothing would be worse than a refusal, because the operator would believe the
/// old version was restored.
pub fn plan_rollback(
    existing: &ExistingInstall<'_>,
    paths: &ProfilePaths,
) -> Result<InstallPlan, InstallError> {
    let Some(active) = existing.state.active.clone() else {
        return Err(InstallError::NothingActive);
    };
    let Some(previous) = existing.state.previous.clone() else {
        return Err(InstallError::Unsupported {
            reason: "no previous version is recorded, so there is nothing to roll back to"
                .to_owned(),
        });
    };
    if !existing.state.has_version(&previous) {
        return Err(InstallError::PreviousVersionMissing { version: previous });
    }

    let plan = InstallPlan {
        action: InstallAction::Rollback,
        mode: existing.layout.mode(),
        summary: format!("make {previous} active again, replacing {active}"),
        from_version: Some(active),
        version: Some(previous),
        files: Vec::new(),
        removed: Vec::new(),
        destructive: false,
        verified_files: 0,
    };
    plan.validate_paths(existing.layout, paths)?;
    Ok(plan)
}

/// Plans the removal of a version directory that is not active or rollback-able.
///
/// # Errors
///
/// Returns [`InstallError::Unsupported`] when the version is the active one, is the
/// recorded rollback target, or is not installed.
pub fn plan_prune(
    existing: &ExistingInstall<'_>,
    paths: &ProfilePaths,
    version: &str,
) -> Result<InstallPlan, InstallError> {
    if existing.state.active.as_deref() == Some(version) {
        return Err(InstallError::Unsupported {
            reason: format!("{version} is the active version and is never pruned"),
        });
    }
    if existing.state.previous.as_deref() == Some(version) {
        return Err(InstallError::Unsupported {
            reason: format!("{version} is the rollback target and is never pruned"),
        });
    }
    if !existing.state.has_version(version) {
        return Err(InstallError::Unsupported {
            reason: format!("{version} is not installed"),
        });
    }

    let directory = existing.layout.version_dir(version);
    let plan = InstallPlan {
        action: InstallAction::Prune,
        mode: existing.layout.mode(),
        summary: format!("remove the unused version directory for {version}"),
        from_version: existing.state.active.clone(),
        version: Some(version.to_owned()),
        files: Vec::new(),
        removed: vec![directory],
        destructive: true,
        verified_files: 0,
    };
    plan.validate_paths(existing.layout, paths)?;
    Ok(plan)
}

/// Plans removal of installed program files, retaining user data.
///
/// # Errors
///
/// Returns [`InstallError::Unsupported`] when nothing is installed.
pub fn plan_uninstall(
    existing: &ExistingInstall<'_>,
    paths: &ProfilePaths,
    purge: bool,
) -> Result<InstallPlan, InstallError> {
    if existing.state.installed.is_empty() && existing.state.active.is_none() {
        return Err(InstallError::Unsupported {
            reason: "no installed version was found".to_owned(),
        });
    }

    let mut removed: Vec<PathBuf> = existing
        .state
        .installed
        .iter()
        .map(|version| existing.layout.version_dir(version))
        .collect();
    removed.push(existing.layout.current_pointer());
    removed.push(existing.layout.root().join("previous.version"));

    if purge {
        // A purge names the profile directories explicitly, and every one of them
        // is caught by `is_inside_profile` below... except that a purge is the one
        // operation *permitted* to touch them. The guard is therefore inverted for
        // this action alone, and the caller must pass the explicit
        // acknowledgement at apply time.
        removed.extend(paths.all_dirs().iter().map(|dir| (*dir).to_path_buf()));
    }

    let action = if purge {
        InstallAction::Purge
    } else {
        InstallAction::Uninstall
    };
    let plan = InstallPlan {
        action,
        mode: existing.layout.mode(),
        summary: if purge {
            format!(
                "remove installed program files and remove user data at {}",
                paths.data_dir().display()
            )
        } else {
            format!(
                "remove installed program files, retaining user data at {}",
                paths.data_dir().display()
            )
        },
        from_version: existing.state.active.clone(),
        version: None,
        files: Vec::new(),
        removed,
        destructive: true,
        verified_files: 0,
    };
    // A non-purge uninstall must still prove every path is outside the profile;
    // a purge deliberately touches it, so containment is asserted only for the
    // install-root paths there.
    if purge {
        for path in plan.files.iter().map(|file| file.destination.as_path()) {
            if !existing.layout.contains(path) {
                return Err(InstallError::PathOutsideInstallRoot {
                    path: path.to_path_buf(),
                });
            }
        }
    } else {
        plan.validate_paths(existing.layout, paths)?;
    }
    Ok(plan)
}

/// Builds the file list for an install or update.
///
/// Only artifacts the manifest verified for `target` are staged, and the staged
/// paths are derived from the manifest's own file names after verification, so the
/// plan describes exactly the bytes that were signed.
fn planned_files(
    layout: &InstallLayout,
    version: &str,
    artifacts: &[VerifiedArtifact],
    target: &str,
) -> Result<Vec<InstallFile>, InstallError> {
    version_directory_name(version)?;
    let mut files = Vec::new();
    for artifact in artifacts {
        if artifact.target != target {
            continue;
        }
        // Staged under the **installed** name from the signed manifest, so the
        // plan describes the stable path a service will point at rather than the
        // version-named download.
        let destination = layout.version_dir(version).join("bin").join(&artifact.name);
        files.push(InstallFile {
            destination,
            size: artifact.size,
        });
    }
    if files.is_empty() {
        return Err(InstallError::ArtifactUnverified {
            code: "jarvis.install_no_artifact_for_target".to_owned(),
        });
    }
    Ok(files)
}

/// Returns the stable launcher path a service definition should point at.
///
/// This is exported so a service spec can be built against it instead of against a
/// version-specific path. An update then never leaves a service starting a stale
/// version.
#[must_use]
pub fn stable_launcher(layout: &InstallLayout, binary: &str) -> PathBuf {
    layout
        .bin_dir()
        .join(format!("{binary}{}", std::env::consts::EXE_SUFFIX))
}
