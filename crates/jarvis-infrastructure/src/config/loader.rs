//! Layered, versioned configuration.
//!
//! Precedence, from lowest to highest: built-in defaults, the config file, an
//! explicit environment allowlist, then command-line overrides. Every layer is
//! non-secret: secret material is referenced, never embedded.
//!
//! The configuration schema is versioned. A file whose `schema_version` this
//! binary does not support fails closed and is never rewritten, because an older
//! binary must not modify state whose writer version it does not understand.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::atomic::{read_bounded, write_atomic};
use super::secret::SecretReference;

/// The configuration schema version this binary writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The schema versions this binary can read.
pub const SUPPORTED_SCHEMA_VERSIONS: &[u32] = &[1];

/// Log verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and above.
    Warn,
    /// Informational and above.
    Info,
    /// Debug and above.
    Debug,
    /// Everything, including per-operation tracing.
    Trace,
}

/// The storage backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// Local SQLite, the default backend.
    Sqlite,
    /// PostgreSQL with pgvector, the server backend.
    Postgres,
}

/// Runtime behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    /// Log verbosity.
    pub log_level: LogLevel,
}

impl Default for RuntimeSection {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Info,
        }
    }
}

/// Storage selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSection {
    /// The selected backend kind.
    pub kind: StorageKind,
}

impl Default for StorageSection {
    fn default() -> Self {
        Self {
            kind: StorageKind::Sqlite,
        }
    }
}

/// Model routing policy selection.
///
/// The credential is a *reference*. A configuration file never carries a key
/// value, so the file is safe to read, copy, and include in diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSection {
    /// The model policy identifier to use.
    pub policy_id: String,
    /// A reference to the provider credential, if one is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_ref: Option<SecretReference>,
}

impl Default for ModelSection {
    fn default() -> Self {
        Self {
            policy_id: "default".to_owned(),
            api_key_ref: None,
        }
    }
}
/// Privacy and telemetry defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct PrivacySection {
    /// Whether local telemetry counters are enabled.
    pub telemetry: bool,
    /// Whether context may be sent to a remote model provider.
    pub allow_remote_context: bool,
}

/// A complete configuration document.
///
/// Unknown fields are rejected at every level, so a typo or an unsupported
/// forward-compatible key fails loudly instead of being ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The schema version this document was written with.
    pub schema_version: u32,
    /// Runtime behavior.
    #[serde(default)]
    pub runtime: RuntimeSection,
    /// Storage selection.
    #[serde(default)]
    pub storage: StorageSection,
    /// Model routing selection.
    #[serde(default)]
    pub model: ModelSection,
    /// Privacy defaults.
    #[serde(default)]
    pub privacy: PrivacySection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            runtime: RuntimeSection::default(),
            storage: StorageSection::default(),
            model: ModelSection::default(),
            privacy: PrivacySection::default(),
        }
    }
}

/// The explicit environment allowlist.
///
/// Only these variables may override configuration. Arbitrary
/// environment-to-key projection is forbidden, so an unexpected `JARVIS_*`
/// variable is reported as ignored rather than applied.
pub const ENV_ALLOWLIST: &[&str] = &[
    "JARVIS_LOG_LEVEL",
    "JARVIS_STORAGE_KIND",
    "JARVIS_MODEL_POLICY_ID",
];

/// Non-secret command-line overrides.
///
/// Each field is optional; `None` means "leave the lower layer's value".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigOverrides {
    /// Overrides the log level.
    pub log_level: Option<LogLevel>,
    /// Overrides the storage kind.
    pub storage_kind: Option<StorageKind>,
    /// Overrides the model policy identifier.
    pub model_policy_id: Option<String>,
}

/// The result of applying the environment layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvApplication {
    /// Allowlisted variables that were applied.
    pub applied: BTreeSet<String>,
    /// `JARVIS_`-prefixed variables that were present but not allowlisted.
    pub ignored: BTreeSet<String>,
}

impl Config {
    /// Parses a configuration document from TOML text.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] for malformed TOML or an unknown field,
    /// [`ConfigError::MissingVersion`] when `schema_version` is absent, and
    /// [`ConfigError::UnsupportedVersion`] when the version is not supported.
    /// An unsupported version is reported before any other field is trusted.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        // Read the version first so an unsupported document is rejected without
        // attempting to interpret fields this binary may not understand. A typed
        // probe is used instead of `toml::Value`, which keeps the parse bounded
        // and avoids depending on the crate's unbounded-value support.
        let probe: VersionProbe = toml::from_str(text).map_err(|_| ConfigError::Parse)?;
        let version = probe.schema_version.ok_or(ConfigError::MissingVersion)?;
        let version = u32::try_from(version).map_err(|_| ConfigError::UnsupportedVersion)?;
        if !SUPPORTED_SCHEMA_VERSIONS.contains(&version) {
            return Err(ConfigError::UnsupportedVersion);
        }

        let config: Self = toml::from_str(text).map_err(|_| ConfigError::Parse)?;
        Ok(config)
    }

    /// Serializes the document to TOML text.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if serialization fails, which is a
    /// programming error rather than an operator error.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string(self).map_err(|_| ConfigError::Parse)
    }

    /// Loads configuration from `path`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Read`] when the file is unreadable and any error
    /// from [`Config::from_toml`].
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let bytes = read_bounded(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| ConfigError::Parse)?;
        Self::from_toml(text)
    }

    /// Writes configuration to `path` atomically.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] on serialization failure and
    /// [`ConfigError::Write`] when the file cannot be atomically replaced.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        write_atomic(path, text.as_bytes())
    }

    /// Applies the environment layer from explicit key/value pairs.
    ///
    /// Only allowlisted keys are applied. Any other `JARVIS_`-prefixed key is
    /// recorded as ignored, which is how forbidden arbitrary projection is made
    /// visible instead of silent.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::EnvOverrideInvalid`] when an allowlisted variable
    /// holds a value the target type does not accept. The error names the key
    /// and never echoes the value.
    pub fn apply_env<'a>(
        &mut self,
        vars: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<EnvApplication, ConfigError> {
        let mut result = EnvApplication::default();

        for (key, value) in vars {
            if !ENV_ALLOWLIST.contains(&key) {
                if key.starts_with("JARVIS_") {
                    result.ignored.insert(key.to_owned());
                }
                continue;
            }

            match key {
                "JARVIS_LOG_LEVEL" => {
                    self.runtime.log_level =
                        parse_log_level(value).ok_or_else(|| ConfigError::EnvOverrideInvalid {
                            key: key.to_owned(),
                        })?;
                }
                "JARVIS_STORAGE_KIND" => {
                    self.storage.kind = parse_storage_kind(value).ok_or_else(|| {
                        ConfigError::EnvOverrideInvalid {
                            key: key.to_owned(),
                        }
                    })?;
                }
                "JARVIS_MODEL_POLICY_ID" => {
                    self.model.policy_id = validate_policy_id(value).ok_or_else(|| {
                        ConfigError::EnvOverrideInvalid {
                            key: key.to_owned(),
                        }
                    })?;
                }
                _ => {
                    // An allowlisted key with no handler is a programming error.
                    // Fail closed and name it rather than silently reporting that
                    // an override was applied when nothing happened.
                    return Err(ConfigError::EnvOverrideInvalid {
                        key: key.to_owned(),
                    });
                }
            }
            result.applied.insert(key.to_owned());
        }

        Ok(result)
    }

    /// Applies the command-line override layer, the highest precedence.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::EnvOverrideInvalid`] when an override value is not
    /// accepted, so an operator typo fails instead of being silently dropped.
    pub fn apply_overrides(&mut self, overrides: &ConfigOverrides) -> Result<(), ConfigError> {
        if let Some(level) = overrides.log_level {
            self.runtime.log_level = level;
        }
        if let Some(kind) = overrides.storage_kind {
            self.storage.kind = kind;
        }
        if let Some(policy_id) = &overrides.model_policy_id {
            self.model.policy_id =
                validate_policy_id(policy_id).ok_or(ConfigError::EnvOverrideInvalid {
                    key: "model.policy_id".to_owned(),
                })?;
        }
        Ok(())
    }

    /// Builds a configuration by applying every layer in precedence order.
    ///
    /// # Errors
    ///
    /// Propagates parse, read, and override errors from the individual layers.
    pub fn layered<'a, I>(
        file: Option<&Path>,
        env: I,
        overrides: &ConfigOverrides,
    ) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut config = match file {
            Some(path) if path.exists() => Self::load(path)?,
            _ => Self::default(),
        };
        config.apply_env(env)?;
        config.apply_overrides(overrides)?;
        Ok(config)
    }
}

/// Reads only the schema version from a document before the full parse.
///
/// The probe has no `deny_unknown_fields`, so it tolerates the rest of the
/// document and can report a version problem independently of field problems.
#[derive(Debug, Deserialize)]
struct VersionProbe {
    schema_version: Option<i64>,
}

/// Parses a log level name.
fn parse_log_level(value: &str) -> Option<LogLevel> {
    match value.to_ascii_lowercase().as_str() {
        "error" => Some(LogLevel::Error),
        "warn" | "warning" => Some(LogLevel::Warn),
        "info" => Some(LogLevel::Info),
        "debug" => Some(LogLevel::Debug),
        "trace" => Some(LogLevel::Trace),
        _ => None,
    }
}

/// Parses a storage kind name.
fn parse_storage_kind(value: &str) -> Option<StorageKind> {
    match value.to_ascii_lowercase().as_str() {
        "sqlite" => Some(StorageKind::Sqlite),
        "postgres" | "postgresql" => Some(StorageKind::Postgres),
        _ => None,
    }
}

/// Validates a model policy identifier.
fn validate_policy_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let valid = !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    valid.then(|| trimmed.to_owned())
}

/// Returns the canonical config file path for a config directory.
#[must_use]
pub fn config_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::{
        Config, ConfigOverrides, LogLevel, PrivacySection, SCHEMA_VERSION, StorageKind,
        config_file_path, parse_log_level,
    };
    use crate::config::ConfigError;
    use crate::config::atomic::MAX_CONFIG_BYTES;
    use crate::config::secret::SecretReference;

    const VALID: &str = r#"
schema_version = 1

[runtime]
log_level = "debug"

[storage]
kind = "sqlite"

[model]
policy_id = "local-default"
api_key_ref = "env:JARVIS_MODEL_KEY"

[privacy]
telemetry = false
allow_remote_context = false
"#;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-fnd004-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
        dir
    }

    #[test]
    fn a_valid_document_round_trips_through_toml() {
        let config = Config::from_toml(VALID).expect("valid document must parse");
        assert_eq!(config.schema_version, SCHEMA_VERSION);
        assert_eq!(config.runtime.log_level, LogLevel::Debug);
        assert_eq!(config.storage.kind, StorageKind::Sqlite);
        assert_eq!(config.model.policy_id, "local-default");
        assert_eq!(
            config.model.api_key_ref,
            Some(SecretReference::env("JARVIS_MODEL_KEY").expect("valid reference")),
        );

        let text = config.to_toml().expect("serialization succeeds");
        let reparsed = Config::from_toml(&text).expect("round trip must parse");
        assert_eq!(reparsed, config);
    }

    #[test]
    fn a_minimal_document_uses_documented_defaults() {
        let config = Config::from_toml("schema_version = 1").expect("minimal document must parse");
        assert_eq!(config, Config::default());
        assert_eq!(config.runtime.log_level, LogLevel::Info);
        assert_eq!(config.storage.kind, StorageKind::Sqlite);
        assert_eq!(config.privacy, PrivacySection::default());
        assert!(config.model.api_key_ref.is_none());
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_level() {
        for document in [
            "schema_version = 1\nunknown_key = 1\n",
            "schema_version = 1\n[runtime]\nlog_level = \"info\"\nunknown = true\n",
            "schema_version = 1\n[storage]\nkind = \"sqlite\"\npath = \"/tmp/db\"\n",
            "schema_version = 1\n[model]\npolicy_id = \"a\"\nkey = \"sk-live\"\n",
        ] {
            let error = Config::from_toml(document).expect_err("must be rejected");
            assert_eq!(error.code(), "jarvis.config_parse", "{document}");
        }
    }

    #[test]
    fn unsupported_and_missing_versions_fail_closed() {
        let empty = Config::from_toml("").expect_err("empty document has no version");
        // An empty document is valid TOML; the missing version is the finding.
        assert!(matches!(
            empty,
            ConfigError::MissingVersion | ConfigError::Parse
        ));

        let missing_field =
            Config::from_toml("[runtime]\nlog_level = \"info\"\n").expect_err("no schema_version");
        assert_eq!(missing_field.code(), "jarvis.config_missing_version");

        for future in [
            "schema_version = 2",
            "schema_version = 0",
            "schema_version = 999",
        ] {
            let error = Config::from_toml(future).expect_err("unsupported version");
            assert_eq!(
                error.code(),
                "jarvis.config_version_unsupported",
                "{future}"
            );
        }

        // A negative or non-integer version is not silently accepted.
        assert!(Config::from_toml("schema_version = -1").is_err());
        assert!(Config::from_toml("schema_version = \"1\"").is_err());
    }

    #[test]
    fn an_unsupported_version_is_not_rewritten() {
        let dir = temp_dir("no-rewrite");
        let path = config_file_path(&dir);
        let original = "schema_version = 2\n[runtime]\nlog_level = \"info\"\n";
        std::fs::write(&path, original).expect("write fixture");

        // Loading fails, so there is nothing to save and the file is untouched.
        assert!(Config::load(&path).is_err());
        let after = std::fs::read_to_string(&path).expect("file still readable");
        assert_eq!(after, original, "an unsupported file must not be modified");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn precedence_is_defaults_then_file_then_env_then_cli() {
        let dir = temp_dir("precedence");
        let path = config_file_path(&dir);
        Config::from_toml(VALID)
            .expect("valid")
            .save(&path)
            .expect("save succeeds");

        // The file set debug; the environment raises it to trace.
        let env = [("JARVIS_LOG_LEVEL", "trace")];
        let overrides = ConfigOverrides {
            log_level: Some(LogLevel::Error),
            storage_kind: None,
            model_policy_id: Some("cli-policy".to_owned()),
        };
        let config = Config::layered(Some(&path), env, &overrides).expect("layered config");

        // CLI wins over environment wins over file.
        assert_eq!(config.runtime.log_level, LogLevel::Error);
        assert_eq!(config.model.policy_id, "cli-policy");
        // The file value survives where no override applies.
        assert_eq!(config.storage.kind, StorageKind::Sqlite);
        assert_eq!(
            config.model.api_key_ref,
            Some(SecretReference::env("JARVIS_MODEL_KEY").expect("valid")),
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_allowlisted_environment_variables_are_applied() {
        let mut config = Config::default();
        let vars = [
            ("JARVIS_LOG_LEVEL", "warn"),
            ("JARVIS_STORAGE_KIND", "postgres"),
            ("JARVIS_MODEL_POLICY_ID", "env-policy"),
            // Not allowlisted: must be reported, never applied.
            ("JARVIS_UNKNOWN_SETTING", "1"),
            ("JARVIS_MODEL_SECTION_LOG_LEVEL", "trace"),
            // Unrelated ambient variables are ignored silently.
            ("PATH", "/usr/bin"),
            ("AWS_SECRET_ACCESS_KEY", "ambient"),
        ];

        let applied = config.apply_env(vars).expect("env layer applies");
        assert_eq!(config.runtime.log_level, LogLevel::Warn);
        assert_eq!(config.storage.kind, StorageKind::Postgres);
        assert_eq!(config.model.policy_id, "env-policy");
        assert_eq!(applied.applied.len(), 3);
        assert!(applied.ignored.contains("JARVIS_UNKNOWN_SETTING"));
        assert!(applied.ignored.contains("JARVIS_MODEL_SECTION_LOG_LEVEL"));
        assert!(
            !applied.ignored.contains("PATH"),
            "non-JARVIS variables are not JARVIS concerns",
        );
    }

    #[test]
    fn an_invalid_allowlisted_value_fails_without_echoing_it() {
        let mut config = Config::default();
        let secret_ish = "not-a-real-level";
        let error = config
            .apply_env([("JARVIS_LOG_LEVEL", secret_ish)])
            .expect_err("invalid value must fail");
        assert_eq!(error.code(), "jarvis.config_env_override_invalid");
        assert!(!error.to_string().contains(secret_ish));
        // The failed layer must not have partially applied anything.
        assert_eq!(config.runtime.log_level, LogLevel::Info);
    }

    #[test]
    fn invalid_command_line_overrides_are_rejected() {
        let mut config = Config::default();
        let error = config
            .apply_overrides(&ConfigOverrides {
                log_level: None,
                storage_kind: None,
                model_policy_id: Some("   ".to_owned()),
            })
            .expect_err("blank policy id must be rejected");
        assert_eq!(error.code(), "jarvis.config_env_override_invalid");
        assert_eq!(config.model.policy_id, "default");
    }

    #[test]
    fn a_secret_value_never_appears_in_text_or_debug() {
        // Simulate an operator mistake: a key value pasted where a reference
        // belongs. The document must fail to parse rather than store it.
        let leaked = "sk-live-super-secret-value";
        let document = format!(
            "schema_version = 1\n[model]\npolicy_id = \"a\"\napi_key_ref = \"{leaked}\"\n",
        );
        let error = Config::from_toml(&document).expect_err("a raw value is not a reference");
        assert_eq!(error.code(), "jarvis.config_parse");
        assert!(!format!("{error} {error:?}").contains(leaked));

        // A legitimate document's Debug and TOML show the reference, not a value.
        let config = Config::from_toml(VALID).expect("valid");
        let rendered = format!(
            "{config:?} {} {:?}",
            config.to_toml().expect("toml"),
            config.model
        );
        assert!(rendered.contains("env:JARVIS_MODEL_KEY"));
        assert!(!rendered.contains("sk-live"));
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_file() {
        let dir = temp_dir("atomic");
        let path = config_file_path(&dir);
        let config = Config::from_toml(VALID).expect("valid");
        config.save(&path).expect("save succeeds");
        assert!(path.exists());

        // Overwriting again must succeed and leave exactly the expected entries.
        config.save(&path).expect("second save succeeds");
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .expect("dir readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["config.toml".to_owned()],
            "no temp file may remain"
        );
        assert_eq!(Config::load(&path).expect("load"), config);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversized_config_file_is_rejected_before_parsing() {
        let dir = temp_dir("oversize");
        let path = config_file_path(&dir);
        let padding = "x".repeat(usize::try_from(MAX_CONFIG_BYTES).expect("limit fits usize") + 1);
        std::fs::write(&path, format!("# {padding}\nschema_version = 1\n")).expect("write fixture");

        let error = Config::load(&path).expect_err("oversized file must be rejected");
        assert_eq!(error.code(), "jarvis.config_too_large");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_toml_is_rejected() {
        let error = Config::from_toml("schema_version = 1\n[runtime\nlog_level = ")
            .expect_err("malformed TOML must be rejected");
        // A malformed document has no readable version, so either diagnostic is
        // correct and the document is refused.
        assert!(matches!(
            error,
            ConfigError::Parse | ConfigError::MissingVersion
        ));
    }

    #[test]
    fn every_allowlisted_key_has_a_handler() {
        // This makes the allowlist and the match arms impossible to diverge:
        // adding a key to one without the other fails here.
        use super::ENV_ALLOWLIST;

        let plausible = [
            ("JARVIS_LOG_LEVEL", "info"),
            ("JARVIS_STORAGE_KIND", "sqlite"),
            ("JARVIS_MODEL_POLICY_ID", "a"),
        ];
        assert_eq!(
            plausible.len(),
            ENV_ALLOWLIST.len(),
            "every allowlisted key needs a value in this test",
        );

        for (key, value) in plausible {
            assert!(ENV_ALLOWLIST.contains(&key), "{key} must be allowlisted");
            let mut config = Config::default();
            let applied = config
                .apply_env([(key, value)])
                .expect("allowlisted key must have a handler");
            assert!(
                applied.applied.contains(key),
                "{key} must be reported applied"
            );
        }
    }

    #[test]
    fn level_and_kind_names_are_parsed_case_insensitively() {
        assert_eq!(parse_log_level("WARN"), Some(LogLevel::Warn));
        assert_eq!(parse_log_level("Warning"), Some(LogLevel::Warn));
        assert_eq!(parse_log_level("bogus"), None);
    }

    #[test]
    fn a_missing_file_yields_defaults_rather_than_an_error() {
        let dir = temp_dir("absent");
        let path = config_file_path(&dir);
        let config = Config::layered(Some(&path), [], &ConfigOverrides::default())
            .expect("absent file must not fail");
        assert_eq!(config, Config::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
