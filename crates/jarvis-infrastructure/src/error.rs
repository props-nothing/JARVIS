//! Typed infrastructure errors with stable, namespaced codes.
//!
//! Adapter errors are explicit types at the library boundary. Codes mirror the
//! `jarvis.` namespace used by domain errors so the error envelope can present
//! one vocabulary. Messages never contain secrets or full untrusted payloads.

use std::path::PathBuf;

use thiserror::Error;

/// An error raised by an operating-system adapter.
#[derive(Debug, Error)]
pub enum InfrastructureError {
    /// No usable home or profile directory was reported by the operating system.
    #[error("the operating system did not report a usable home directory")]
    HomeDirectoryUnavailable,
    /// A required directory could not be created or is not a directory.
    #[error("could not prepare the required directory")]
    DirectoryCreate {
        /// The directory that failed. Paths are safe to record; they contain no
        /// secret material.
        path: PathBuf,
    },
    /// A file could not be created or written.
    #[error("could not write the required file")]
    FileWrite {
        /// The file that failed.
        path: PathBuf,
    },
    /// A directory is accessible beyond its owner on a platform that can detect
    /// it. JARVIS refuses to use it rather than silently accepting the exposure.
    #[error("directory permissions are broader than the owner only")]
    UnsafePermissions {
        /// The directory with unsafe permissions.
        path: PathBuf,
        /// The observed mode, in octal, for operator diagnostics.
        mode: String,
    },
    /// A path component would escape its intended root.
    #[error("a path component is absolute, rooted, or contains a parent reference")]
    UnsafePathComponent {
        /// The rejected component. It is echoed back because it is operator
        /// input the user must correct; callers must not log it alongside
        /// secrets.
        component: String,
    },
    /// A resolved path is not contained by the profile root it must live under.
    #[error("the resolved path is outside the profile root")]
    PathOutsideProfile,
}

impl InfrastructureError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::HomeDirectoryUnavailable => "jarvis.home_directory_unavailable",
            Self::DirectoryCreate { .. } => "jarvis.directory_create",
            Self::FileWrite { .. } => "jarvis.file_write",
            Self::UnsafePermissions { .. } => "jarvis.unsafe_permissions",
            Self::UnsafePathComponent { .. } => "jarvis.unsafe_path_component",
            Self::PathOutsideProfile => "jarvis.path_outside_profile",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    ///
    /// Permission and containment failures are deterministic and are not fixed
    /// by retrying. A creation or write failure may be transient (a busy
    /// filesystem, a full disk that later frees space), so it is reported
    /// retryable and the caller decides with operation-specific policy.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::DirectoryCreate { .. } | Self::FileWrite { .. } => true,
            Self::HomeDirectoryUnavailable
            | Self::UnsafePermissions { .. }
            | Self::UnsafePathComponent { .. }
            | Self::PathOutsideProfile => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InfrastructureError;

    #[test]
    fn codes_are_namespaced_and_unique() {
        let errors = [
            InfrastructureError::HomeDirectoryUnavailable,
            InfrastructureError::DirectoryCreate { path: "a".into() },
            InfrastructureError::FileWrite { path: "b".into() },
            InfrastructureError::UnsafePermissions {
                path: "c".into(),
                mode: "755".to_owned(),
            },
            InfrastructureError::UnsafePathComponent {
                component: "..".to_owned(),
            },
            InfrastructureError::PathOutsideProfile,
        ];

        let mut codes: Vec<&str> = errors.iter().map(InfrastructureError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len(), "codes must be unique");
        for code in codes {
            assert!(code.starts_with("jarvis."), "{code} must be namespaced");
        }
    }

    #[test]
    fn security_failures_are_not_retryable() {
        assert!(
            !InfrastructureError::UnsafePermissions {
                path: "c".into(),
                mode: "755".to_owned(),
            }
            .retryable()
        );
        assert!(!InfrastructureError::PathOutsideProfile.retryable());
        assert!(InfrastructureError::FileWrite { path: "d".into() }.retryable());
    }
}
