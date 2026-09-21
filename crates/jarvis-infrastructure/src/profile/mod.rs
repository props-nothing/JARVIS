//! Profile resolution: a standard per-user profile or an explicit portable root.
//!
//! Production resolution has exactly two inputs: the operating system's standard
//! per-user locations, or a root the operator names explicitly. There is no
//! environment override. That is deliberate: an environment variable that
//! redirects JARVIS's profile would be a way to move durable state, credentials,
//! and the discovery file for an installed product, and on Windows the platform
//! directory provider ignores `APPDATA`/`LOCALAPPDATA` overrides anyway, so such a
//! variable would exist only as added attack surface.
//!
//! Clean-machine isolation therefore uses [`ProfilePaths::portable`] through the
//! explicit `--profile <DIR>` argument, which is an operator decision the process
//! can see, not ambient environment state.
//!
//! Keeping the decision in one function means the precedence is stated once and
//! cannot drift between the daemon and the client.

use std::path::PathBuf;

use crate::error::InfrastructureError;
use crate::paths::ProfilePaths;

/// How a profile was selected, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    /// An explicit portable root named by the operator.
    Portable,
    /// Standard per-user locations from the operating system.
    Standard,
}

impl ProfileSource {
    /// Returns the stable name used in diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Standard => "standard",
        }
    }
}

/// A resolved profile together with how it was chosen.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    /// The resolved directory layout.
    pub paths: ProfilePaths,
    /// How the layout was selected.
    pub source: ProfileSource,
}

/// Resolves the profile for this process.
///
/// Precedence is explicit so it cannot be guessed: an operator-supplied portable
/// root wins, then the standard platform profile.
///
/// # Errors
///
/// Returns [`InfrastructureError::HomeDirectoryUnavailable`] when no usable
/// directory source exists.
pub fn resolve(explicit: Option<PathBuf>) -> Result<ResolvedProfile, InfrastructureError> {
    if let Some(root) = explicit {
        return Ok(ResolvedProfile {
            paths: ProfilePaths::portable(root),
            source: ProfileSource::Portable,
        });
    }

    Ok(ResolvedProfile {
        paths: ProfilePaths::standard()?,
        source: ProfileSource::Standard,
    })
}

/// Resolves the profile with no explicit root.
///
/// # Errors
///
/// Returns [`InfrastructureError::HomeDirectoryUnavailable`] when no usable
/// directory source exists.
pub fn resolve_default() -> Result<ResolvedProfile, InfrastructureError> {
    resolve(None)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::paths::ProfileMode;
    use crate::profile::{ProfileSource, resolve, resolve_default};

    #[test]
    fn an_explicit_root_wins_and_is_portable() {
        let root = PathBuf::from("jarvis-explicit-root");
        let resolved = resolve(Some(root.clone())).expect("resolves");
        assert_eq!(resolved.source, ProfileSource::Portable);
        assert_eq!(resolved.paths.mode(), ProfileMode::Portable);
        assert_eq!(resolved.paths.config_dir(), root.join("config"));
        assert_eq!(resolved.paths.data_dir(), root.join("data"));
        // The root is the parent of every managed directory, which is what makes
        // a portable profile a complete isolation boundary.
        assert!(
            resolved
                .paths
                .contains(&root.join("data").join("db").join("jarvis.sqlite"))
        );
        assert!(!resolved.paths.contains(&PathBuf::from("/somewhere/else")));
    }

    #[test]
    fn with_no_root_the_standard_profile_is_used() {
        // This exercises the production fallback the packaged binary uses.
        let resolved = resolve_default().expect("a home directory exists");
        assert_eq!(resolved.source, ProfileSource::Standard);
        assert_eq!(resolved.paths.mode(), ProfileMode::Standard);
    }

    #[test]
    fn the_sources_have_stable_names() {
        assert_eq!(ProfileSource::Portable.name(), "portable");
        assert_eq!(ProfileSource::Standard.name(), "standard");
    }
}
