//! Secret references resolved through the environment.
//!
//! Ordinary configuration never contains secret material. It contains a
//! *reference* to a secret, and the value is resolved only at the last
//! responsible moment (see the security rules in `AGENTS.md`). This module
//! implements the environment-provider reference form for the Foundation slice;
//! connector, provider, and OS credential-store references are owned by later
//! slices and are represented as typed references now.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ConfigError;

/// The environment variable prefix a JARVIS provider reference may name.
const ENV_PREFIX: &str = "JARVIS_";

/// A reference to a secret, never the secret value itself.
///
/// A reference is safe to persist, log, and include in diagnostics. It carries
/// a provider and a locator; only a [`SecretResolver`] turns it into material.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretReference {
    /// Reads the value from the named process environment variable.
    ///
    /// The locator must begin with `JARVIS_` so a configuration file cannot
    /// direct JARVIS to read an unrelated ambient secret such as a cloud
    /// credential or a CI token.
    Env(String),
}

impl SecretReference {
    /// Creates an environment-backed reference, validating the locator.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::SecretReferenceInvalid`] when the locator does not
    /// carry the required `JARVIS_` prefix or contains characters that are not
    /// valid in an environment variable name.
    pub fn env(name: impl Into<String>) -> Result<Self, ConfigError> {
        let name = name.into();
        let valid = name.starts_with(ENV_PREFIX)
            && !name.is_empty()
            && !name.contains('=')
            && !name.contains('\0')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        if valid {
            Ok(Self::Env(name))
        } else {
            Err(ConfigError::SecretReferenceInvalid)
        }
    }

    /// Parses the textual contract form, `env:JARVIS_NAME`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::SecretReferenceInvalid`] for an unknown provider
    /// scheme or an invalid locator.
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        let (provider, locator) = value
            .split_once(':')
            .ok_or(ConfigError::SecretReferenceInvalid)?;
        match provider {
            "env" => Self::env(locator),
            _ => Err(ConfigError::SecretReferenceInvalid),
        }
    }

    /// Returns the provider scheme name.
    #[must_use]
    pub const fn provider(&self) -> &'static str {
        match self {
            Self::Env(_) => "env",
        }
    }

    /// Returns the locator within the provider.
    #[must_use]
    pub fn locator(&self) -> &str {
        match self {
            Self::Env(name) => name,
        }
    }
}

/// `Debug` prints the reference, never a secret value.
impl fmt::Debug for SecretReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SecretReference({}:{})",
            self.provider(),
            self.locator(),
        )
    }
}

/// `Display` emits the contract text form, `env:JARVIS_NAME`.
impl fmt::Display for SecretReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.provider(), self.locator())
    }
}

impl Serialize for SecretReference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SecretReference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReferenceVisitor;

        impl serde::de::Visitor<'_> for ReferenceVisitor {
            type Value = SecretReference;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a secret reference such as \"env:JARVIS_MODEL_KEY\"")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                SecretReference::parse(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(ReferenceVisitor)
    }
}

/// Resolves a reference to its current value.
///
/// Resolution is fallible because the environment may be unset. It is performed
/// at the last responsible moment and never during configuration parsing, so a
/// missing secret does not make the configuration itself unreadable.
pub trait SecretResolver: fmt::Debug + Send + Sync {
    /// Resolves `reference` to its value.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::SecretUnavailable`] when the reference cannot be
    /// resolved. The error never contains the secret value.
    fn resolve(&self, reference: &SecretReference) -> Result<String, ConfigError>;
}

/// Resolves references from an explicit, bounded map.
///
/// This is the deterministic implementation used by tests and by callers that
/// already hold resolved values.
#[derive(Debug, Default)]
pub struct MapSecretResolver {
    values: Mutex<BTreeMap<String, String>>,
}

impl MapSecretResolver {
    /// Creates an empty resolver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a value for `locator`.
    pub fn insert(&self, locator: impl Into<String>, value: impl Into<String>) {
        if let Ok(mut values) = self.values.lock() {
            values.insert(locator.into(), value.into());
        }
    }
}

impl SecretResolver for MapSecretResolver {
    fn resolve(&self, reference: &SecretReference) -> Result<String, ConfigError> {
        let values = self
            .values
            .lock()
            .map_err(|_| ConfigError::SecretUnavailable)?;
        values
            .get(reference.locator())
            .cloned()
            .ok_or(ConfigError::SecretUnavailable)
    }
}

/// Reads a variable from the real process environment.
///
/// A non-UTF-8 environment value is reported as unavailable rather than being
/// lossily converted, because a silently altered secret is worse than a clear
/// failure.
fn read_process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Resolves references from the process environment.
///
/// The environment is read at resolution time, never cached in a JARVIS record.
/// The lookup is injectable so the reader can be tested without mutating the
/// process environment, which Rust 2024 marks `unsafe` and this crate forbids.
#[derive(Debug, Clone, Copy)]
pub struct EnvSecretResolver {
    lookup: fn(&str) -> Option<String>,
}

impl EnvSecretResolver {
    /// Creates a resolver that reads the real process environment.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lookup: read_process_env,
        }
    }

    /// Creates a resolver with a fixed environment view.
    ///
    /// The view returns `true`/`false` for whether a variable exists and its
    /// value, so a test or an operator with a captured environment can drive it.
    #[must_use]
    pub const fn with_lookup(lookup: fn(&str) -> Option<String>) -> Self {
        Self { lookup }
    }
}

impl Default for EnvSecretResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretResolver for EnvSecretResolver {
    fn resolve(&self, reference: &SecretReference) -> Result<String, ConfigError> {
        match reference {
            SecretReference::Env(name) => (self.lookup)(name).ok_or(ConfigError::SecretUnavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EnvSecretResolver, MapSecretResolver, SecretReference, SecretResolver};
    use crate::config::ConfigError;

    #[test]
    fn reference_round_trips_as_text() {
        let reference = SecretReference::env("JARVIS_MODEL_KEY").expect("valid reference");
        assert_eq!(reference.to_string(), "env:JARVIS_MODEL_KEY");
        assert_eq!(reference.provider(), "env");
        assert_eq!(reference.locator(), "JARVIS_MODEL_KEY");

        let parsed = SecretReference::parse("env:JARVIS_MODEL_KEY").expect("valid text form");
        assert_eq!(parsed, reference);
    }

    #[test]
    fn debug_prints_the_reference_not_a_value() {
        let reference = SecretReference::env("JARVIS_MODEL_KEY").expect("valid reference");
        let debug = format!("{reference:?}");
        assert_eq!(debug, "SecretReference(env:JARVIS_MODEL_KEY)");
    }

    #[test]
    fn invalid_references_are_rejected() {
        for value in [
            "",
            "JARVIS_MODEL_KEY",          // no provider scheme
            "env:",                      // empty locator
            "env:MODEL_KEY",             // missing required prefix
            "env:jarvis_lower",          // lowercase is not an env name JARVIS owns
            "env:AWS_SECRET_ACCESS_KEY", // unrelated ambient secret
            "file:/etc/passwd",          // unknown provider
            "vault:secret/model",        // unknown provider
            "env:JARVIS_KEY=value",      // injection attempt
        ] {
            assert!(
                SecretReference::parse(value).is_err(),
                "{value:?} must be rejected",
            );
        }
    }

    #[test]
    fn resolver_returns_a_value_or_a_safe_error() {
        let resolver = MapSecretResolver::new();
        resolver.insert("JARVIS_MODEL_KEY", "s3cr3t");
        let present = SecretReference::env("JARVIS_MODEL_KEY").expect("valid");
        assert_eq!(resolver.resolve(&present).expect("resolves"), "s3cr3t");

        let missing = SecretReference::env("JARVIS_ABSENT_KEY").expect("valid");
        let error = resolver.resolve(&missing).expect_err("must be unavailable");
        assert_eq!(error.code(), "jarvis.secret_unavailable");
        assert!(
            !error.to_string().contains("s3cr3t"),
            "the error must not contain a value",
        );
    }

    #[test]
    fn env_resolver_reads_only_the_referenced_variable() {
        // A fixed, injected environment view: no process-global mutation, so the
        // test is hermetic and needs no `unsafe`.
        fn lookup(name: &str) -> Option<String> {
            (name == "JARVIS_FND004_TEST").then(|| "value-from-env".to_owned())
        }

        let resolver = EnvSecretResolver::with_lookup(lookup);
        let present = SecretReference::env("JARVIS_FND004_TEST").expect("valid reference");
        assert_eq!(
            resolver.resolve(&present).expect("resolves"),
            "value-from-env"
        );

        let absent = SecretReference::env("JARVIS_FND004_ABSENT").expect("valid reference");
        let error = resolver.resolve(&absent).expect_err("must be unavailable");
        assert_eq!(error.code(), "jarvis.secret_unavailable");
    }

    #[test]
    fn the_default_env_resolver_reads_the_real_process_environment() {
        // The point of this test is that the default lookup is wired to the real
        // environment: an unset variable reports unavailable rather than
        // panicking or inventing a value.
        let reference = SecretReference::env("JARVIS_FND004_DEFINITELY_UNSET").expect("valid");
        let error = EnvSecretResolver::default()
            .resolve(&reference)
            .expect_err("a variable this test never sets must be unavailable");
        assert_eq!(error.code(), "jarvis.secret_unavailable");
    }

    #[test]
    fn reference_serde_is_the_text_form() {
        #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
        struct Holder {
            api_key_ref: Option<SecretReference>,
        }

        let holder = Holder {
            api_key_ref: Some(SecretReference::env("JARVIS_MODEL_KEY").expect("valid")),
        };
        let json = serde_json::to_string(&holder).expect("serialization succeeds");
        assert_eq!(json, r#"{"api_key_ref":"env:JARVIS_MODEL_KEY"}"#);
        let parsed: Holder = serde_json::from_str(&json).expect("deserialization succeeds");
        assert_eq!(parsed, holder);

        // A raw secret value is not a valid reference and must be rejected.
        assert!(serde_json::from_str::<Holder>(r#"{"api_key_ref":"sk-live-abcdef"}"#).is_err());
    }

    #[test]
    fn config_error_variants_are_reachable_from_this_module() {
        // Documents that the resolver surface returns the config error type.
        let error = ConfigError::SecretUnavailable;
        assert_eq!(error.code(), "jarvis.secret_unavailable");
    }
}
