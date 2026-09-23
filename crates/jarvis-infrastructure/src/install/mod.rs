//! Install, update, rollback, portable-mode, and uninstall behaviour.
//!
//! This module owns the one thing a user's *data* must be protected from: a
//! change to the *program*. JARVIS is installed as a versioned directory tree
//! with an atomically switched pointer, so an update either becomes the active
//! version or leaves the previous version exactly as it was. There is no partial
//! install state and no half-updated binary.
//!
//! ## The central boundary: program and data never share a directory that can be
//! replaced wholesale
//!
//! A profile ([`crate::paths::ProfilePaths`]) owns durable user state: the
//! database, the credential, logs, and runtime discovery. An install
//! ([`InstallLayout`]) owns replaceable program files. They are separate roots,
//! and every install operation refuses a path inside the profile. That refusal is
//! what makes "uninstall retains user data unless explicitly purged" a structural
//! property rather than a promise: removing versioned program directories cannot
//! reach the database because the database is not under any of them.
//!
//! ## Plan and execute are separate, and the plan is a pure value
//!
//! [`InstallPlan`] names every version, every directory, every file it will
//! write, and whether it removes anything, and it renders for an operator to
//! review. It is produced without touching the host, so it can be asserted on any
//! platform — the same discipline the service controllers and repair planner use.
//! [`apply`] executes it and verifies the postcondition afterwards.
//!
//! ## Activation is an atomic directory swap, not a file copy
//!
//! A version becomes active by replacing the `current` entry with a rename onto a
//! freshly created directory, then flushing the parent directory. A reader
//! therefore observes either the old version or the new one, never a mixture.
//! `std::fs::rename` is the atomic replace primitive on both Unix and Windows.
//!
//! ## Rollback needs the previous version to still exist
//!
//! Rolling back is a second activation of the version that was active before. It
//! only works while that directory is on disk, so `apply` never prunes the
//! currently active version and never prunes the version it just replaced. Pruning
//! beyond those two is a separate, explicitly requested operation.
//!
//! ## Portable mode is a resolved decision, not an environment variable
//!
//! Portable mode means the install root is an explicit operator-supplied path
//! rather than a per-user platform location. It is the same `--profile`-style
//! explicit choice the profile resolver already uses, and for the same reason: an
//! environment variable that redirected the install root would be a way to move
//! which binary a service starts.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::paths::ProfilePaths;

/// The directory name of the per-version program directories.
pub const VERSIONS_DIRECTORY: &str = "versions";

/// The directory name of the atomic active-version pointer.
///
/// On Unix this is a symlink; on Windows it is a directory containing a
/// `current.version` file, because creating a symlink on Windows needs either
/// elevation or developer mode and an install must not require either.
pub const CURRENT_DIRECTORY: &str = "current";

/// The file inside [`CURRENT_DIRECTORY`] naming the active version on Windows.
pub const CURRENT_VERSION_FILE: &str = "current.version";

/// The maximum accepted version length.
pub const MAX_VERSION_LEN: usize = 64;

// **There is deliberately no upper bound on how many version directories may exist.** A
// `MAX_RETAINED_VERSIONS = 8` used to be declared here, was referenced by no code, and could not be
// enforced by anything: `plan_prune` removes **one named version** and refuses the active one and
// the recorded rollback target, so it has no way to say "too many — drop the oldest". A count bound
// needs a bulk-prune operation, and none exists (the CLI has `status`, `update`, `rollback`, and
// `uninstall`, which is also what `FND-012` records).
//
// The safety property the constant gestured at is real and is enforced structurally instead, by the
// two per-version refusals: a prune pass over every installed version can never remove the active
// one or the rollback target, so it cannot remove every path back to a working state. That is
// asserted by `the_active_version_and_the_rollback_target_are_never_pruned` and
// `a_prune_all_never_removes_every_path_back_to_a_working_version`. Deleting the constant rather
// than wiring it to a fabricated consumer keeps "a declared bound is coverage" from being true by
// accident — the class this project has already found four times.

/// The prefix applied to every version directory name.
pub const VERSION_PREFIX: &str = "v";

/// Whether an install follows the operating system's per-user locations or an
/// explicit operator-supplied root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// Per-user platform locations.
    Standard,
    /// One explicit root chosen by the operator.
    Portable,
}

impl InstallMode {
    /// Returns the stable token used in diagnostics.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Portable => "portable",
        }
    }
}

/// Why an install operation could not be planned or applied.
///
/// Every variant is an operator-actionable cause. "The version string is unsafe"
/// and "the artifact is not signed" need completely different responses, so they
/// never collapse into one message.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InstallError {
    /// The version string is empty, too long, or contains unsafe characters.
    #[error("the version string is not usable in a directory name")]
    InvalidVersion,
    /// A path in the plan is inside the user's profile.
    #[error("an install path is inside the user profile and would risk user data")]
    PathInsideProfile {
        /// The rejected path.
        path: PathBuf,
    },
    /// A path in the plan is outside the install root.
    #[error("an install path is outside the install root")]
    PathOutsideInstallRoot {
        /// The rejected path.
        path: PathBuf,
    },
    /// The target version is already the active version.
    #[error("the requested version is already active")]
    AlreadyActive {
        /// The version that is already active.
        version: String,
    },
    /// No version is currently active, so there is nothing to update or roll back.
    #[error("no version is currently active")]
    NothingActive,
    /// The version a rollback would return to is no longer on disk.
    #[error("the previous version is no longer installed, so rollback is not possible")]
    PreviousVersionMissing {
        /// The version that would have been restored.
        version: String,
    },
    /// An artifact is not signed or does not verify.
    #[error("a release artifact did not verify, so it was not installed")]
    ArtifactUnverified {
        /// The stable code from the verification failure.
        code: String,
    },
    /// An update was requested but a running daemon holds the instance lock.
    #[error("a daemon is running; activation would race with it")]
    DaemonRunning,
    /// A file operation failed.
    #[error("an install file operation failed")]
    Io {
        /// The path the operation targeted.
        path: PathBuf,
    },
    /// The postcondition did not hold after applying.
    #[error("the install did not reach its postcondition")]
    PostconditionFailed {
        /// What the check observed.
        detail: String,
    },
    /// The plan was not confirmed.
    #[error("an install change requires explicit confirmation")]
    NotConfirmed,
    /// A purge was requested without the explicit destructive acknowledgement.
    #[error("removing user data requires an explicit purge acknowledgement")]
    PurgeNotAcknowledged,
    /// An unsupported combination of flags was requested.
    #[error("the requested install operation is not valid in this state")]
    Unsupported {
        /// Why it is unsupported.
        reason: String,
    },
}

impl InstallError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidVersion => "jarvis.install_invalid_version",
            Self::PathInsideProfile { .. } => "jarvis.install_path_inside_profile",
            Self::PathOutsideInstallRoot { .. } => "jarvis.install_path_outside_root",
            Self::AlreadyActive { .. } => "jarvis.install_already_active",
            Self::NothingActive => "jarvis.install_nothing_active",
            Self::PreviousVersionMissing { .. } => "jarvis.install_previous_version_missing",
            Self::ArtifactUnverified { .. } => "jarvis.install_artifact_unverified",
            Self::DaemonRunning => "jarvis.install_daemon_running",
            Self::Io { .. } => "jarvis.install_io",
            Self::PostconditionFailed { .. } => "jarvis.install_postcondition_failed",
            Self::NotConfirmed => "jarvis.install_not_confirmed",
            Self::PurgeNotAcknowledged => "jarvis.install_purge_not_acknowledged",
            Self::Unsupported { .. } => "jarvis.install_unsupported",
        }
    }

    /// Returns whether retrying the same operation unchanged could succeed.
    ///
    /// A version conflict, a missing previous version, and an unverified artifact
    /// are deterministic: retrying re-derives the same answer. A file operation
    /// may be retried because the cause can be a transient local condition.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Io { .. })
    }
}

/// Returns `true` when `version` is safe to use as a directory-name component.
///
/// Only ASCII alphanumerics, `.`, `-`, and `+` are accepted; the value must start
/// with a digit, may not start with a dot, and may not contain `..`. The leading
/// digit is what keeps a directory name unambiguous: with a `v` prefix, `vv1.0`
/// would otherwise read as the version `v1.0`, and a version string reaches a
/// directory name, so a separator, a drive colon, a control character, or a
/// traversal would let a hostile or mistyped version escape the install root.
#[must_use]
pub fn is_safe_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= MAX_VERSION_LEN
        && version.starts_with(|character: char| character.is_ascii_digit())
        && !version.contains("..")
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// The directory name for a version.
///
/// # Errors
///
/// Returns [`InstallError::InvalidVersion`] for an unusable version string.
pub fn version_directory_name(version: &str) -> Result<String, InstallError> {
    if !is_safe_version(version) {
        return Err(InstallError::InvalidVersion);
    }
    Ok(format!("{VERSION_PREFIX}{version}"))
}

/// Reads the version back out of a version directory name.
///
/// Returns `None` for a name that is not one, so a foreign directory in the
/// versions root is ignored rather than misparsed. The remainder is re-validated
/// as a version, because a leading `v` alone is not a version: `versions` and
/// `vv1.0` must not be read as versions `ersions` and `v1.0`.
#[must_use]
pub fn version_from_directory_name(name: &str) -> Option<&str> {
    let version = name.strip_prefix(VERSION_PREFIX)?;
    if is_safe_version(version) {
        Some(version)
    } else {
        None
    }
}

/// The install root and the versions beneath it.
#[derive(Debug, Clone)]
pub struct InstallLayout {
    mode: InstallMode,
    root: PathBuf,
    bin_directory: PathBuf,
}

impl InstallLayout {
    /// Builds an install layout rooted at `root`.
    ///
    /// The root is used verbatim, matching how a portable profile treats its root.
    #[must_use]
    pub fn new(mode: InstallMode, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            mode,
            bin_directory: root.join("bin"),
            root,
        }
    }

    /// Returns the resolved standard per-user install root for this platform.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::Unsupported`] when the operating system reports no
    /// usable per-user location. JARVIS never falls back to the current directory,
    /// because that would install into an unpredictable place.
    pub fn standard() -> Result<Self, InstallError> {
        // Standard portable-mode installs live beside the profile's *data local*
        // root, resolved through the same provider the profile uses, so the two
        // cannot disagree about the per-user base directory.
        let profile = ProfilePaths::standard().map_err(|_| InstallError::Unsupported {
            reason: "the operating system reported no usable per-user directory".to_owned(),
        })?;
        let base = profile
            .data_dir()
            .parent()
            .ok_or_else(|| InstallError::Unsupported {
                reason: "the per-user data directory has no parent".to_owned(),
            })?
            .to_path_buf();
        Ok(Self::new(InstallMode::Standard, base.join("install")))
    }

    /// Returns the install mode.
    #[must_use]
    pub const fn mode(&self) -> InstallMode {
        self.mode
    }

    /// Returns the install root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the directory holding every install version.
    #[must_use]
    pub fn versions_dir(&self) -> PathBuf {
        self.root.join(VERSIONS_DIRECTORY)
    }

    /// Returns the active-version pointer path.
    #[must_use]
    pub fn current_pointer(&self) -> PathBuf {
        self.root.join(CURRENT_DIRECTORY)
    }

    /// Returns the directory for one version.
    #[must_use]
    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.versions_dir()
            .join(format!("{VERSION_PREFIX}{version}"))
    }

    /// Returns the stable directory holding the active launcher.
    ///
    /// This is the path a service definition points at, so an update never leaves
    /// a service pointing at a stale version-specific path.
    #[must_use]
    pub fn bin_dir(&self) -> &Path {
        &self.bin_directory
    }

    /// Returns the exact path of an installed binary for a version.
    ///
    /// `installed_name` is the name recorded in the signed manifest, used verbatim.
    /// No executable suffix is appended: the manifest's name already carries it on
    /// a platform that needs one, and appending it again would produce
    /// `jarvisd.exe.exe` and a service definition pointing at nothing.
    #[must_use]
    pub fn binary_path(&self, version: &str, installed_name: &str) -> PathBuf {
        self.version_dir(version).join("bin").join(installed_name)
    }

    /// Returns the version directories currently present, newest name first.
    ///
    /// A name that is not a version directory is ignored, so a stray file or a
    /// foreign directory in the versions root cannot be mistaken for an install.
    #[must_use]
    pub fn installed_versions(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.versions_dir()) else {
            return Vec::new();
        };
        let mut versions: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter_map(|name| version_from_directory_name(&name).map(str::to_owned))
            .collect();
        // Sorted by name so the list is deterministic; a caller that needs
        // semantic ordering must say so, because lexical order is not version
        // order.
        versions.sort_unstable();
        versions
    }

    /// Returns the version recorded as active, if any.
    ///
    /// The answer comes from the pointer, not from "which directories exist": a
    /// version directory that exists but is not pointed at is not active.
    #[must_use]
    pub fn active_version(&self) -> Option<String> {
        active_version_of(&self.current_pointer())
    }

    /// Returns the previous version to roll back to, if one is recorded.
    #[must_use]
    pub fn previous_version(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.root.join("previous.version")).ok()?;
        let version = text.trim().to_owned();
        if version.is_empty() {
            None
        } else {
            Some(version)
        }
    }

    /// Returns whether `path` is inside this install root.
    ///
    /// Containment is checked on normalized components, so a lexical prefix like
    /// `/install` does not falsely contain `/installer`.
    #[must_use]
    pub fn contains(&self, candidate: &Path) -> bool {
        normalize(candidate).starts_with(normalize(&self.root))
    }

    /// Returns whether `path` is inside the profile and therefore touches user
    /// data.
    ///
    /// This is the guard that makes "uninstall retains data" structural. Anything
    /// the profile owns — above all the database — is refused by every install
    /// operation, so an install cannot delete user state even if it is asked to.
    #[must_use]
    pub fn is_inside_profile(paths: &ProfilePaths, candidate: &Path) -> bool {
        let target = normalize(candidate);
        paths
            .all_dirs()
            .iter()
            .any(|directory| target.starts_with(normalize(directory)))
    }
}

/// Returns the version named by an active-version pointer, if it is usable.
///
/// On Unix the pointer is a symlink; on Windows it is a directory holding a
/// `current.version` file. Both are read here so a caller does not branch on the
/// platform to ask a simple question.
#[must_use]
pub fn active_version_of(pointer: &Path) -> Option<String> {
    // Unix: the pointer is a symlink whose target names the version directory.
    if let Ok(target) = std::fs::read_link(pointer) {
        let name = target.file_name()?.to_str()?;
        return version_from_directory_name(name).map(str::to_owned);
    }

    // Windows: the pointer is a directory holding a version name.
    let recorded = std::fs::read_to_string(pointer.join(CURRENT_VERSION_FILE)).ok()?;
    let version = recorded.trim();
    if is_safe_version(version) {
        Some(version.to_owned())
    } else {
        // A pointer whose content is not a usable version is treated as "no
        // active version" rather than guessed at. Guessing is how a corrupted
        // pointer becomes an install that reports the wrong binary as active.
        None
    }
}

/// One file an install plan will write, with the bytes it will contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFile {
    /// The absolute destination path.
    pub destination: PathBuf,
    /// The content length, for the operator's preview. The bytes are not retained
    /// in the plan, so a plan stays small and renderable.
    pub size: u64,
}

/// What an install plan does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAction {
    /// Install a version that is not present and make it active.
    Install,
    /// Make an already-present version active.
    Activate,
    /// Make the recorded previous version active again.
    Rollback,
    /// Remove a version's directory.
    Prune,
    /// Remove installed program files, retaining user data.
    Uninstall,
    /// Remove installed program files **and** the user's profile.
    Purge,
}

impl InstallAction {
    /// Returns the stable token used in a rendered plan.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Activate => "activate",
            Self::Rollback => "rollback",
            Self::Prune => "prune",
            Self::Uninstall => "uninstall",
            Self::Purge => "purge",
        }
    }

    /// Returns whether the action removes something.
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Prune | Self::Uninstall | Self::Purge)
    }
}

/// A complete, unexecuted install change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    action: InstallAction,
    mode: InstallMode,
    summary: String,
    version: Option<String>,
    from_version: Option<String>,
    files: Vec<InstallFile>,
    removed: Vec<PathBuf>,
    destructive: bool,
    /// Paths that must be verified as signed artifacts before activation.
    verified_files: usize,
}

impl InstallPlan {
    /// Returns what the plan does.
    #[must_use]
    pub const fn action(&self) -> InstallAction {
        self.action
    }

    /// Returns the install mode.
    #[must_use]
    pub const fn mode(&self) -> InstallMode {
        self.mode
    }

    /// Returns a one-line summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the version the plan targets, when it has one.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Returns the version the plan comes from, for an update or rollback.
    #[must_use]
    pub fn from_version(&self) -> Option<&str> {
        self.from_version.as_deref()
    }

    /// Returns the files the plan writes.
    #[must_use]
    pub fn files(&self) -> &[InstallFile] {
        &self.files
    }

    /// Returns the paths the plan removes.
    #[must_use]
    pub fn removed(&self) -> &[PathBuf] {
        &self.removed
    }

    /// Returns whether the plan removes anything.
    #[must_use]
    pub const fn is_destructive(&self) -> bool {
        self.destructive
    }

    /// Returns how many files were verified as signed artifacts.
    #[must_use]
    pub const fn verified_files(&self) -> usize {
        self.verified_files
    }

    /// Renders the plan for an operator to review before confirming.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "install plan: {}", self.action.token());
        let _ = writeln!(out, "  mode:      {}", self.mode.token());
        let _ = writeln!(out, "  summary:   {}", self.summary);
        if let Some(version) = &self.version {
            let _ = writeln!(out, "  version:   {version}");
        }
        if let Some(from) = &self.from_version {
            let _ = writeln!(out, "  from:      {from}");
        }
        let _ = writeln!(
            out,
            "  destructive: {}",
            if self.destructive { "yes" } else { "no" }
        );
        let _ = writeln!(out, "  verified files: {}", self.verified_files);
        if !self.files.is_empty() {
            let _ = writeln!(out, "  writes {} file(s):", self.files.len());
            for file in &self.files {
                let _ = writeln!(
                    out,
                    "    {} ({} bytes)",
                    file.destination.display(),
                    file.size
                );
            }
        }
        if !self.removed.is_empty() {
            let _ = writeln!(out, "  removes {} path(s):", self.removed.len());
            for path in &self.removed {
                let _ = writeln!(out, "    {}", path.display());
            }
        }
        if self.action == InstallAction::Uninstall {
            let _ = writeln!(
                out,
                "  user data: retained (installed files only; pass --purge to remove it)"
            );
        }
        if self.action == InstallAction::Purge {
            let _ = writeln!(
                out,
                "  user data: REMOVED, including the database; this cannot be undone"
            );
        }
        out
    }

    /// Returns the plan's file destinations, for a containment assertion.
    fn every_path(&self) -> impl Iterator<Item = &Path> {
        self.files
            .iter()
            .map(|file| file.destination.as_path())
            .chain(self.removed.iter().map(PathBuf::as_path))
    }

    /// Validates every path the plan would touch.
    ///
    /// This runs at plan time and *again* at apply time, because a plan is data
    /// and could have been constructed elsewhere.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::PathOutsideInstallRoot`] for a path outside the
    /// root and [`InstallError::PathInsideProfile`] for a path that would touch
    /// user data.
    pub fn validate_paths(
        &self,
        layout: &InstallLayout,
        paths: &ProfilePaths,
    ) -> Result<(), InstallError> {
        for path in self.every_path() {
            if !layout.contains(path) {
                return Err(InstallError::PathOutsideInstallRoot {
                    path: path.to_path_buf(),
                });
            }
            if InstallLayout::is_inside_profile(paths, path) {
                return Err(InstallError::PathInsideProfile {
                    path: path.to_path_buf(),
                });
            }
        }
        Ok(())
    }
}

impl fmt::Display for InstallAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

use std::fmt::Write as _;

/// Normalizes a path lexically for containment comparison.
///
/// No filesystem access occurs, so a symlink cannot influence the answer. A
/// caller that must resist links resolves the real path first.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // `pop` past the root is a no-op, which keeps normalization
                // anchored rather than letting a `..` escape.
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

mod apply;
mod plan;

pub use apply::{InstallOutcome, VerifiedRelease, apply, verify_staged, version_is_complete};
pub use plan::{
    ExistingInstall, Installation, plan_install, plan_prune, plan_rollback, plan_uninstall,
    plan_update, stable_launcher,
};

#[cfg(test)]
mod tests;
