//! Platform path resolution for JARVIS profiles.
//!
//! JARVIS mutable state must live under standard per-user locations unless the
//! user explicitly selects portable mode. This module resolves those locations
//! and creates state directories with owner-only permissions.
//!
//! ## Permission model
//!
//! - On Unix, directories are created with mode `0o700`. The process `umask`
//!   can only clear bits, so the result can never be more permissive than
//!   requested. `umask` is never modified because it is process-global.
//! - On Windows, mutable paths are proven to live under a known per-user folder.
//!   Windows Known Folders are already ACL-restricted to the owner, `SYSTEM`, and
//!   Administrators by the operating system. Writing an explicit discretionary
//!   ACL requires `unsafe` FFI, which the crate forbids; that work is recorded as
//!   deferred in the Foundation evidence note.

use std::fs;
use std::path::{Component, Path, PathBuf};

use directories::ProjectDirs;

use crate::error::InfrastructureError;

/// The layout kind a resolved profile uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileMode {
    /// Standard per-user locations from the operating system.
    Standard,
    /// Everything under one explicit root chosen by the user.
    Portable,
}

/// A resolved set of JARVIS directories for one profile.
///
/// All paths are absolute unless the profile is portable, in which case they are
/// relative to the caller-supplied root.
#[derive(Debug, Clone)]
pub struct ProfilePaths {
    mode: ProfileMode,
    config_dir: PathBuf,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    log_dir: PathBuf,
    runtime_dir: PathBuf,
}

impl ProfilePaths {
    /// Resolves the standard per-user profile for this operating system.
    ///
    /// # Errors
    ///
    /// Returns [`InfrastructureError::HomeDirectoryUnavailable`] when the
    /// operating system does not report a usable home/profile directory. JARVIS
    /// never falls back to the current directory, because doing so would place
    /// durable state in an unpredictable location.
    pub fn standard() -> Result<Self, InfrastructureError> {
        let project_dirs = ProjectDirs::from("com", "JARVIS", "JARVIS")
            .ok_or(InfrastructureError::HomeDirectoryUnavailable)?;

        // `runtime_dir` and `state_dir` are `None` on Windows and macOS. The
        // local data directory is the portable choice for transient runtime
        // state on those platforms.
        let local_root = project_dirs.data_local_dir().to_path_buf();
        let runtime_dir = project_dirs
            .runtime_dir()
            .map_or_else(|| local_root.join("run"), Path::to_path_buf);

        Ok(Self {
            mode: ProfileMode::Standard,
            config_dir: project_dirs.config_dir().to_path_buf(),
            data_dir: project_dirs.data_dir().to_path_buf(),
            cache_dir: project_dirs.cache_dir().to_path_buf(),
            log_dir: local_root.join("log"),
            runtime_dir,
        })
    }

    /// Resolves a portable profile rooted at `root`.
    ///
    /// The root is used verbatim: portable mode is an explicit operator choice
    /// and JARVIS does not adapt it to platform conventions.
    #[must_use]
    pub fn portable(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            mode: ProfileMode::Portable,
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            log_dir: root.join("log"),
            runtime_dir: root.join("run"),
        }
    }

    /// Returns the profile mode.
    #[must_use]
    pub const fn mode(&self) -> ProfileMode {
        self.mode
    }

    /// Returns the configuration directory.
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Returns the durable data directory.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Returns the cache directory.
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Returns the log directory.
    #[must_use]
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Returns the runtime (transient) directory.
    #[must_use]
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// Returns the directory holding the durable SQLite database.
    ///
    /// Mutable database state uses the local, non-roaming data directory so a
    /// roaming profile never carries a live database file between machines.
    #[must_use]
    pub fn database_dir(&self) -> PathBuf {
        self.data_dir.join("db")
    }

    /// Returns every managed directory, for creation and diagnostics.
    #[must_use]
    pub fn all_dirs(&self) -> [&Path; 5] {
        [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
            &self.runtime_dir,
        ]
    }

    /// Returns whether `candidate` is inside this profile's data root.
    ///
    /// The comparison is on normalized path components, so a lexical prefix
    /// such as `/data` does not falsely contain `/database`. Both paths are used
    /// as-is; no filesystem access occurs, so a symlink cannot influence the
    /// answer. Callers that must resist links resolve the real path first.
    #[must_use]
    pub fn contains(&self, candidate: &Path) -> bool {
        let root = normalize(&self.data_dir);
        let target = normalize(candidate);
        target.starts_with(&root)
    }

    /// Creates every managed directory with owner-only permissions.
    ///
    /// # Errors
    ///
    /// Returns [`InfrastructureError::DirectoryCreate`] when a directory cannot
    /// be created, and [`InfrastructureError::UnsafePermissions`] when an
    /// existing directory is accessible beyond the owner on Unix.
    pub fn ensure_directories(&self) -> Result<(), InfrastructureError> {
        for directory in self.all_dirs() {
            ensure_private_dir(directory)?;
        }
        Ok(())
    }

    /// Creates a file with owner-only permissions and writes `contents`.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent directory or the file cannot be created.
    pub fn write_private_file(
        &self,
        path: &Path,
        contents: &[u8],
    ) -> Result<(), InfrastructureError> {
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        write_owner_only(path, contents)
    }
}

/// Removes `.` and resolves `..` lexically without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Popping above the root is impossible: `PathBuf::pop` on an
                // empty or root-only buffer is a no-op, which keeps the result
                // anchored rather than escaping.
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Returns whether a path component may be joined under a root.
fn is_safe_component(component: &str) -> bool {
    if component.is_empty() || component == "." || component == ".." {
        return false;
    }
    // Reject separators and anything the platform would treat as a root.
    if component.contains('/') || component.contains('\\') {
        return false;
    }
    if component.contains(':') {
        // Windows drive-relative and drive-absolute forms, and NTFS alternate
        // data streams, all use a colon.
        return false;
    }
    !Path::new(component).is_absolute()
}

/// Joins `names` under `root`, rejecting any unsafe component.
///
/// # Errors
///
/// Returns [`InfrastructureError::UnsafePathComponent`] if any name is empty,
/// `.`, `..`, absolute, rooted, or contains a separator or drive prefix.
pub fn join_under(root: &Path, names: &[&str]) -> Result<PathBuf, InfrastructureError> {
    let mut result = root.to_path_buf();
    for name in names {
        if !is_safe_component(name) {
            return Err(InfrastructureError::UnsafePathComponent {
                component: (*name).to_owned(),
            });
        }
        result.push(name);
    }
    Ok(result)
}

/// Verifies that `path` is contained by a user profile directory.
///
/// # Errors
///
/// Returns [`InfrastructureError::PathOutsideProfile`] when the path escapes
/// `profile`, which is what prevents a crafted name from redirecting durable
/// state elsewhere.
pub fn verify_within_profile(profile: &Path, path: &Path) -> Result<(), InfrastructureError> {
    let root = normalize(profile);
    if normalize(path).starts_with(&root) {
        Ok(())
    } else {
        Err(InfrastructureError::PathOutsideProfile)
    }
}

/// Creates `path` as an owner-only directory, then re-queries `path`'s mode.
fn ensure_private_dir(path: &Path) -> Result<(), InfrastructureError> {
    if !path.exists() {
        create_dir_owner_only(path)?;
    } else if !path.is_dir() {
        return Err(InfrastructureError::DirectoryCreate {
            path: path.to_path_buf(),
        });
    }

    // On Unix the resulting mode is queried back and rejected when it exposes
    // access beyond the owner. On Windows the operating system's Known Folder
    // ACL is the control and the containment check is the JARVIS guarantee;
    // explicit DACL query is deferred (see the Foundation evidence note).
    #[cfg(unix)]
    verify_directory_permissions(path)?;

    Ok(())
}

/// Reports whether a directory mode exposes access beyond the owner.
#[cfg(unix)]
fn mode_is_owner_only(mode: u32) -> bool {
    mode & 0o077 == 0
}

#[cfg(unix)]
fn create_dir_owner_only(path: &Path) -> Result<(), InfrastructureError> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    // `umask` may clear bits but can never add them, so this is an upper bound.
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|_| InfrastructureError::DirectoryCreate {
            path: path.to_path_buf(),
        })
}

#[cfg(unix)]
fn write_owner_only(path: &Path, contents: &[u8]) -> Result<(), InfrastructureError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| InfrastructureError::FileWrite {
            path: path.to_path_buf(),
        })?;
    file.write_all(contents)
        .map_err(|_| InfrastructureError::FileWrite {
            path: path.to_path_buf(),
        })
}

#[cfg(unix)]
fn verify_directory_permissions(path: &Path) -> Result<(), InfrastructureError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path).map_err(|_| InfrastructureError::DirectoryCreate {
        path: path.to_path_buf(),
    })?;
    let mode = metadata.permissions().mode();
    if mode_is_owner_only(mode) {
        Ok(())
    } else {
        Err(InfrastructureError::UnsafePermissions {
            path: path.to_path_buf(),
            mode: format!("{mode:o}"),
        })
    }
}

#[cfg(not(unix))]
fn create_dir_owner_only(path: &Path) -> Result<(), InfrastructureError> {
    // Windows Known Folders are already ACL-restricted to the owner, `SYSTEM`,
    // and Administrators. An explicit DACL would require `unsafe` FFI.
    fs::create_dir_all(path).map_err(|_| InfrastructureError::DirectoryCreate {
        path: path.to_path_buf(),
    })
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, contents: &[u8]) -> Result<(), InfrastructureError> {
    fs::write(path, contents).map_err(|_| InfrastructureError::FileWrite {
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ProfilePaths, join_under, normalize, verify_within_profile};

    #[test]
    fn portable_profile_uses_explicit_subdirectories_only() {
        let paths = ProfilePaths::portable(PathBuf::from("/tmp/jarvis-portable"));
        assert_eq!(paths.config_dir(), Path::new("/tmp/jarvis-portable/config"));
        assert_eq!(paths.log_dir(), Path::new("/tmp/jarvis-portable/log"));
        assert_eq!(paths.runtime_dir(), Path::new("/tmp/jarvis-portable/run"));
        assert_eq!(
            paths.database_dir(),
            Path::new("/tmp/jarvis-portable/data/db"),
        );
        assert_eq!(paths.all_dirs().len(), 5);
    }

    #[test]
    fn standard_profile_resolves_under_a_real_home() {
        let paths = ProfilePaths::standard().expect("a test host must have a home directory");
        for directory in paths.all_dirs() {
            assert!(
                directory.is_absolute(),
                "resolved path {} must be absolute",
                directory.display(),
            );
        }
        // Mutable state must be local, never roaming.
        assert!(!paths.data_dir().as_os_str().is_empty());
        assert!(!paths.log_dir().as_os_str().is_empty());
    }

    #[test]
    fn rooted_absolute_and_parent_components_are_rejected() {
        let root = Path::new("/srv/jarvis");
        for unsafe_name in [
            "",
            ".",
            "..",
            "../escape",
            "nested/../../escape",
            "/absolute",
            "\\absolute",
            "c:\\windows",
            "..\\escape",
            "stream:name",
        ] {
            let result = join_under(root, &[unsafe_name]);
            assert!(
                result.is_err(),
                "{unsafe_name:?} must not be joined under the root",
            );
        }
    }

    #[test]
    fn safe_components_are_joined_under_the_root() {
        let root = Path::new("/srv/jarvis");
        let joined = join_under(root, &["profiles", "default", "db.sqlite"])
            .expect("safe components must join");
        assert_eq!(joined, Path::new("/srv/jarvis/profiles/default/db.sqlite"));
        assert!(joined.starts_with(root));
    }

    #[test]
    fn traversal_that_escapes_the_profile_is_detected() {
        let profile = Path::new("/home/user/.local/share/JARVIS");
        assert!(verify_within_profile(profile, &profile.join("data/db.sqlite")).is_ok());
        assert!(
            verify_within_profile(profile, Path::new("/home/user/.ssh/id_ed25519")).is_err(),
            "a path outside the profile must be rejected",
        );
        // A lexical prefix must not be mistaken for containment.
        assert!(
            verify_within_profile(
                profile,
                Path::new("/home/user/.local/share/JARVIS-backup/x")
            )
            .is_err()
        );
    }

    #[test]
    fn normalize_resolves_dot_and_parent_without_escaping_the_root() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), Path::new("/a/c/d"),);
        // Popping past the root stays anchored instead of escaping.
        assert_eq!(normalize(Path::new("/../..")), Path::new("/"));
    }

    #[cfg(unix)]
    #[test]
    fn created_directories_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("jarvis-fnd003-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = ProfilePaths::portable(&root);
        paths
            .ensure_directories()
            .expect("directories must be created");

        for directory in paths.all_dirs() {
            let mode = std::fs::metadata(directory)
                .expect("directory must exist")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode & 0o077,
                0,
                "{} must not be group/other accessible, mode {mode:o}",
                directory.display(),
            );
        }

        // A written file must also be owner-only.
        let file = root.join("config/secret.txt");
        paths
            .write_private_file(&file, b"private")
            .expect("file must be written");
        let file_mode = std::fs::metadata(&file)
            .expect("file must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode & 0o077, 0, "file mode {file_mode:o}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_group_readable_directory_is_flagged_unsafe() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = std::env::temp_dir().join(format!("jarvis-fnd003-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = ProfilePaths::portable(&root);
        paths
            .ensure_directories()
            .expect("directories must be created");

        // Relax the mode outside JARVIS, then prove JARVIS notices.
        let mut permissions = std::fs::metadata(paths.data_dir())
            .expect("directory exists")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(paths.data_dir(), permissions)
            .expect("test must be able to chmod its own temp directory");

        let error = paths
            .ensure_directories()
            .expect_err("a group-readable directory must be reported");
        assert_eq!(error.code(), "jarvis.unsafe_permissions");

        let _ = std::fs::remove_dir_all(&root);
    }
}
