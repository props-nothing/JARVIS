//! Layered, versioned configuration for JARVIS.
//!
//! Configuration is non-secret by construction: a file may contain a
//! [`SecretReference`], never a value. Precedence is defaults, file, environment
//! allowlist, then command-line overrides. Writes are atomic, and an unsupported
//! schema version fails closed without modifying the file.

pub mod atomic;
pub mod loader;
pub mod secret;

pub use atomic::{MAX_CONFIG_BYTES, read_bounded, read_tail_window, write_atomic};
pub use loader::{
    Config, ConfigOverrides, ENV_ALLOWLIST, EnvApplication, LogLevel, ModelSection, PrivacySection,
    RuntimeSection, SCHEMA_VERSION, SUPPORTED_SCHEMA_VERSIONS, StorageKind, StorageSection,
    config_file_path,
};
pub use secret::{EnvSecretResolver, MapSecretResolver, SecretReference, SecretResolver};

use std::path::PathBuf;

use thiserror::Error;

/// An error raised while reading, parsing, or writing configuration.
///
/// Variants carry no configuration values except where the value is operator
/// input the user must correct, and never carry secret material.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[error("the configuration file could not be read")]
    Read {
        /// The file that failed.
        path: PathBuf,
    },
    /// The configuration file exceeds the accepted size.
    #[error("the configuration file is larger than the accepted limit")]
    TooLarge {
        /// The accepted limit in bytes, which is not sensitive.
        limit: u64,
    },
    /// The document is malformed TOML or contains an unknown field.
    #[error("the configuration document is malformed or has an unknown field")]
    Parse,
    /// The document has no `schema_version`.
    #[error("the configuration document has no schema_version")]
    MissingVersion,
    /// The document's `schema_version` is not supported by this binary.
    #[error("the configuration schema version is not supported by this binary")]
    UnsupportedVersion,
    /// An allowlisted environment or command-line override held an invalid value.
    #[error("a configuration override for this setting is invalid")]
    EnvOverrideInvalid {
        /// The setting key that was rejected. The value is never included.
        key: String,
    },
    /// A secret reference is malformed or names a provider that is not allowed.
    #[error("the secret reference is malformed or uses a disallowed provider")]
    SecretReferenceInvalid,
    /// A secret reference could not be resolved. The value is never included.
    #[error("the referenced secret is unavailable")]
    SecretUnavailable,
    /// The configuration could not be written.
    #[error("the configuration file could not be written")]
    Write {
        /// The file that failed.
        path: PathBuf,
    },
}

impl ConfigError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Read { .. } => "jarvis.config_read",
            Self::TooLarge { .. } => "jarvis.config_too_large",
            Self::Parse => "jarvis.config_parse",
            Self::MissingVersion => "jarvis.config_missing_version",
            Self::UnsupportedVersion => "jarvis.config_version_unsupported",
            Self::EnvOverrideInvalid { .. } => "jarvis.config_env_override_invalid",
            Self::SecretReferenceInvalid => "jarvis.secret_reference_invalid",
            Self::SecretUnavailable => "jarvis.secret_unavailable",
            Self::Write { .. } => "jarvis.config_write",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Read { .. } | Self::Write { .. } => true,
            Self::TooLarge { .. }
            | Self::Parse
            | Self::MissingVersion
            | Self::UnsupportedVersion
            | Self::EnvOverrideInvalid { .. }
            | Self::SecretReferenceInvalid
            | Self::SecretUnavailable => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigError;

    #[test]
    fn codes_are_namespaced_and_unique() {
        let errors = [
            ConfigError::Read { path: "a".into() },
            ConfigError::TooLarge { limit: 1 },
            ConfigError::Parse,
            ConfigError::MissingVersion,
            ConfigError::UnsupportedVersion,
            ConfigError::EnvOverrideInvalid {
                key: "k".to_owned(),
            },
            ConfigError::SecretReferenceInvalid,
            ConfigError::SecretUnavailable,
            ConfigError::Write { path: "b".into() },
        ];

        let mut codes: Vec<&str> = errors.iter().map(ConfigError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len(), "codes must be unique");
        for code in codes {
            assert!(code.starts_with("jarvis."), "{code} must be namespaced");
        }
    }

    #[test]
    fn security_and_input_failures_are_not_retryable() {
        assert!(!ConfigError::Parse.retryable());
        assert!(!ConfigError::SecretUnavailable.retryable());
        assert!(ConfigError::Write { path: "c".into() }.retryable());
    }
}
