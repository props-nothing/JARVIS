//! Layered, versioned configuration.
//!
//! Precedence, from lowest to highest: built-in defaults, the config file, an
//! explicit environment allowlist, then command-line overrides. Every layer is
//! non-secret: secret material is referenced, never embedded.
//!
//! The configuration schema is versioned. A file whose `schema_version` this
//! binary does not support fails closed and is never rewritten, because an older
//! binary must not modify state whose writer version it does not understand.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::atomic::{read_bounded, write_atomic};
use super::mcp::McpSection;
use super::secret::SecretReference;

/// The largest deny reason a reviewed refusal may carry.
///
/// **Imported from the port rather than restated**, because the bound an operator may *write* and the bound
/// the store will *accept* must be the same number: two literals would drift, and the drift would show up as
/// a configuration file that loads and then fails at the pipeline. Re-exported so a caller reading the config
/// module does not have to know which layer owns it.
pub use jarvis_application::repository::tool_grant::MAX_GRANT_NOTE_BYTES;

/// The configuration schema version this binary writes.
///
/// Version 2 added the optional `[model.provider]` table. The version moved because a
/// document that **names a provider endpoint** is not one a version-1 binary can read:
/// that binary's `deny_unknown_fields` would reject the unknown table as a *parse*
/// failure, when the accurate diagnostic is "written by a newer JARVIS". A file with no
/// `[model.provider]` table loads unchanged under both versions, so an existing profile
/// is not rewritten merely because the binary was upgraded.
///
/// Version 3 added the optional `[tools]` table, whose `[[tools.deny]]` entries are the
/// operator's **reviewable refusals**. The version moved for exactly the version-2 reason:
/// a version-2 binary's `deny_unknown_fields` would report an unknown `[tools]` table as a
/// parse failure rather than as "written by a newer JARVIS". A file with no `[tools]` table
/// loads unchanged under 1, 2, and 3 — so this bump rewrites nothing that did not use the
/// new capability.
pub const SCHEMA_VERSION: u32 = 3;

/// The schema versions this binary can read.
///
/// All three are accepted. Version 1 remains readable because the only difference is an
/// optional table, so refusing it would lock an operator out of their own profile for no
/// security benefit — and the same is true of version 2.
pub const SUPPORTED_SCHEMA_VERSIONS: &[u32] = &[1, 2, 3];

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

/// A configured model provider endpoint.
///
/// This is **operator configuration for reaching an endpoint**, and it deliberately does not
/// validate the host. Whether an endpoint is admissible is a property of the *adapter* — the
/// openai-compatible adapter refuses a non-loopback host because this build has no TLS — and
/// duplicating that predicate here would be a second implementation of one rule that could
/// disagree with the first. An invalid value therefore fails at startup with the adapter's own
/// code, from the one place that owns the decision.
///
/// The credential is **not** here: it stays in [`ModelSection::api_key_ref`], which is a
/// *reference* resolved at startup and never written back to the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSection {
    /// The provider identifier recorded on a `model_calls` row and shown in diagnostics.
    pub id: String,
    /// The endpoint host. An address, never a URL; the port is a separate field for that reason.
    pub host: String,
    /// The endpoint port.
    pub port: u16,
    /// The prefix the completion route is mounted under, when the server uses one.
    ///
    /// Absent means `/chat/completions` at the origin. Present means the route is mounted under a
    /// prefix — `"/v1"` for Ollama, vLLM, LM Studio, and most gateways. Not validated here: the
    /// adapter owns that rule, for the same reason it owns the host rule, so there is one
    /// implementation and one error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<String>,
    /// The models this endpoint serves. At least one is required.
    pub models: Vec<String>,
    /// Maps a served model's JARVIS identifier to the name the provider's API expects.
    ///
    /// Needed because the two namespaces genuinely differ. A JARVIS model id is a lowercase dotted
    /// slug — one spelling per identity, which is what makes a routing decision and a persisted row
    /// comparable — while a provider may name a model with characters that grammar excludes:
    /// Ollama's `glm-5.3-flash:cloud`, a versioned `model@2026-01`, a namespaced `org/model`. Without
    /// this mapping the adapter can only address a model whose provider name happens to already be a
    /// legal JARVIS id, which silently excludes real endpoints.
    ///
    /// An entry whose key is not one of `models` is refused at composition, because a mapping for a
    /// model the endpoint does not serve is a configuration mistake that would otherwise be invisible:
    /// the mapped name would never be used, and the operator would believe it had been.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_names: BTreeMap<String, String>,
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
    /// The provider endpoint, when a real provider is configured.
    ///
    /// `None` is the deterministic default: the daemon composes the scripted provider, so
    /// a profile with no provider answers runs without reaching any network endpoint. That
    /// is the correct default for a fresh install and for every test, and it means an
    /// operator opts **in** to a real provider rather than discovering one was already in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderSection>,
}

impl Default for ModelSection {
    fn default() -> Self {
        Self {
            policy_id: "default".to_owned(),
            api_key_ref: None,
            provider: None,
        }
    }
}
/// A reviewed refusal, as an operator declares it.
///
/// **This is the configuration form of a `tool_deny_rules` row**, and both exist deliberately rather than one
/// being redundant. A stored rule is written at runtime through `/api/v1/tool-grants/deny-rules` and is
/// scoped to one profile; a reviewed rule is part of the profile's *shipped configuration*, so it is present
/// at startup, survives a database reset, and can be reviewed in a diff. The pipeline merges them — see
/// `StoredGrants` — because a refusal can only narrow, so losing one is the direction that re-authorizes
/// something an operator had forbidden.
///
/// `reason` is required and validated at composition rather than defaulted: the reason is what a refused
/// principal is shown, and a refusal with no explanation is one a user cannot act on. The bound is the same
/// one the store enforces, imported rather than restated, so the two cannot disagree about what is storable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DenyRuleSection {
    /// The capability to refuse, e.g. `clock.now@1`. Absent means the rule names no capability and is
    /// constrained by its other dimensions alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// The effects to refuse, as contract spellings. Empty means "not constrained by effects".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<String>,
    /// The reason shown to a refused principal.
    pub reason: String,
}

impl DenyRuleSection {
    /// Returns the rule as the domain's `DenyRule` plus the capability it names.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] when the reason is empty, over its bound, or carries a NUL, when an
    /// effect spelling is not one the contract defines, when the capability is not a canonical one, or when
    /// the rule names nothing at all.
    ///
    /// **The empty rule is refused here rather than at the pipeline**, and that is a deliberate departure
    /// from the domain's own rule that an empty `DenyRule` matches nothing. A defaulted record must not
    /// over-match, because that would refuse everything; a rule *written by an operator* that names nothing
    /// is a configuration mistake, and accepting it would ship a refusal that silently does nothing while
    /// appearing in the config file — the same reasoning `NewDenyRule::validated` records for the stored form.
    pub fn to_rule(&self) -> Result<ReviewedDenyRule, ConfigError> {
        let fail = |field: &'static str| ConfigError::InvalidToolDenyRule { field };
        if self.reason.is_empty()
            || self.reason.len() > MAX_GRANT_NOTE_BYTES
            || self.reason.contains('\0')
        {
            return Err(fail("reason"));
        }
        let capability = match self.capability.as_deref() {
            Some(value) => {
                // Validated by parsing, so a typo is a startup failure rather than a rule that matches no
                // identity and therefore refuses nothing — which would look like a working refusal in the
                // config file and do nothing at the pipeline.
                let parsed = jarvis_domain::tool::identity::ToolCapability::parse(value)
                    .map_err(|_| fail("capability"))?;
                Some(parsed.to_string())
            }
            None => None,
        };
        let effects: BTreeSet<jarvis_domain::tool::classification::Effect> = self
            .effects
            .iter()
            .map(|value| {
                jarvis_domain::tool::classification::Effect::parse(value)
                    .map_err(|_| fail("effects"))
            })
            .collect::<Result<_, _>>()?;
        if capability.is_none() && effects.is_empty() {
            return Err(fail("rule"));
        }
        Ok(ReviewedDenyRule {
            capability,
            rule: jarvis_domain::tool::policy::DenyRule {
                // **Always `None`.** A reviewed rule names a capability, which the adapter expands into the
                // identities that currently offer it. Naming an identity here would make the refusal stop
                // applying after a tool was recompiled — the direction that loses a restriction.
                identity: None,
                // The principal and workspace are the *deployment's*, not the file's: a configuration file
                // that could name a principal would make one profile's refusal another's, and the
                // authenticated scope is resolved server-side by the same rule every other surface follows.
                principal: None,
                workspace: None,
                effects,
            },
            reason: self.reason.clone(),
        })
    }
}

/// A validated reviewed refusal: the domain rule plus the capability it names.
///
/// A pair rather than a bare `DenyRule`, because the domain type has no capability field and the
/// expansion from a capability to the identities offering it cannot be done from the rule alone — the same
/// reason `StoredDenyRule` exists beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedDenyRule {
    /// The capability the rule refuses, when it names one.
    pub capability: Option<String>,
    /// The rule as the evaluator consumes it.
    pub rule: jarvis_domain::tool::policy::DenyRule,
    /// The reason shown to a refused principal.
    pub reason: String,
}

/// Tool authorization as reviewed configuration.
///
/// **The reviewed counterpart of the durable grant store.** A deployment that wants its policy reviewable
/// in a diff — and present before any database exists — declares it here, and the daemon merges these
/// refusals with the stored ones at composition. The table is optional, so every existing profile loads
/// unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsSection {
    /// The refusals this deployment ships.
    ///
    /// **Only refusals, and no reviewed grants, and that asymmetry is the design rather than an
    /// omission.** A grant is *authority*, and authority belongs in a durable row an operator can list,
    /// revoke, and audit — a reviewed grant would be authority that no surface can withdraw, which is the
    /// defect the grant store was built to remove. A refusal is a *restriction*, and losing one is the
    /// fail-open direction, so shipping refusals in configuration is the safe half to make declarative.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<DenyRuleSection>,
}

impl ToolsSection {
    /// Validates every reviewed refusal.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidToolDenyRule`] naming the field of the first unusable entry. Validated
    /// at composition rather than when the pipeline reads it, so a profile with a broken refusal **fails
    /// startup** instead of running with a refusal that silently does nothing.
    pub fn reviewed_rules(&self) -> Result<Vec<ReviewedDenyRule>, ConfigError> {
        self.deny.iter().map(DenyRuleSection::to_rule).collect()
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
    /// Tool authorization, as reviewed configuration.
    ///
    /// **Last in the document, and it is the only table that ships *refusals* rather than *authority*.**
    /// See [`ToolsSection::deny`] for why grants are deliberately absent here.
    #[serde(default)]
    pub tools: ToolsSection,
    /// The MCP servers this profile launches, as reviewed configuration.
    ///
    /// **A second table about tools, and it is not part of `tools` on purpose.** That table holds
    /// *refusals* — restrictions on tools JARVIS already has — while this one declares *sources* of tools it
    /// does not have yet. Merging them would put a launchable process and an authorization rule in one
    /// namespace, and the two are validated against different rules: a refusal names a capability or an
    /// effect, a server names an identity, a program, and an environment of secret references.
    #[serde(default, skip_serializing_if = "McpSection::is_empty")]
    pub mcp: McpSection,
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
            tools: ToolsSection::default(),
            mcp: McpSection::default(),
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

    /// A complete document at the version this binary writes.
    ///
    /// Carries a `[[tools.deny]]` entry as well as every other table, so the round-trip test exercises the
    /// reviewed-refusal shape rather than a document that happens not to use it — the same reason `VALID`
    /// already names a provider-adjacent model policy rather than a minimal one.
    const VALID: &str = r#"
schema_version = 3

[runtime]
log_level = "debug"

[storage]
kind = "sqlite"

[model]
policy_id = "local-default"
api_key_ref = "env:JARVIS_MODEL_KEY"

[[tools.deny]]
capability = "email.send@1"
reason = "outbound mail is never sent unattended"

[[tools.deny]]
effects = ["external_communication", "write"]
reason = "destructive tools are refused profile-wide"

[privacy]
telemetry = false
allow_remote_context = false
"#;

    /// The same document as a version-1 binary would have written: no provider table, and
    /// the older version number. Kept as a literal rather than derived from [`VALID`] so the
    /// upgrade test cannot drift into testing the current shape against itself.
    const VERSION_ONE: &str = r#"
schema_version = 1

[model]
policy_id = "local-default"
api_key_ref = "env:JARVIS_MODEL_KEY"
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
        let config = Config::from_toml("schema_version = 3").expect("minimal document must parse");
        assert_eq!(config, Config::default());
        assert_eq!(config.runtime.log_level, LogLevel::Info);
        assert_eq!(config.storage.kind, StorageKind::Sqlite);
        assert_eq!(config.privacy, PrivacySection::default());
        assert!(config.model.api_key_ref.is_none());
        assert!(config.model.provider.is_none());
        assert!(config.tools.deny.is_empty());
    }

    #[test]
    fn a_version_one_document_remains_readable_and_defaults_to_no_provider() {
        // The upgrade path that matters: a profile written by the version-1 binary must load,
        // and must not acquire a provider it never configured. A version bump that silently
        // locked an operator out of their own profile would be a migration defect.
        let config = Config::from_toml(VERSION_ONE).expect("a version-1 document must still parse");
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.model.policy_id, "local-default");
        assert!(
            config.model.provider.is_none(),
            "a document with no provider table must not gain one"
        );
    }

    #[test]
    fn a_document_that_names_a_provider_parses_and_round_trips() {
        let document = format!(
            "{VALID}\n[model.provider]\nid = \"local.llamacpp\"\nhost = \"127.0.0.1\"\n\
             port = 8080\nmodels = [\"qwen2.5-7b-instruct\"]\n"
        );
        let config = Config::from_toml(&document).expect("a provider table must parse");
        let provider = config
            .model
            .provider
            .as_ref()
            .expect("a configured provider");
        assert_eq!(provider.id, "local.llamacpp");
        assert_eq!(provider.host, "127.0.0.1");
        assert_eq!(provider.port, 8080);
        assert_eq!(provider.models, vec!["qwen2.5-7b-instruct".to_owned()]);

        // The credential stays a reference through a round trip, so a save cannot write a value.
        let text = config.to_toml().expect("serialization succeeds");
        assert!(text.contains("env:JARVIS_MODEL_KEY"), "{text}");
        assert_eq!(Config::from_toml(&text).expect("round trip"), config);
    }

    #[test]
    fn a_provider_table_field_is_required_and_an_unknown_one_is_rejected() {
        // A misspelled key must fail rather than being ignored, because a silently dropped
        // `port` would leave the adapter dialling the wrong endpoint.
        for document in [
            "schema_version = 2\n[model.provider]\nid = \"a\"\nhost = \"127.0.0.1\"\nmodels = []\n",
            "schema_version = 2\n[model.provider]\nid = \"a\"\nhost = \"127.0.0.1\"\nport = 1\n",
            "schema_version = 2\n[model.provider]\nid = \"a\"\nhost = \"127.0.0.1\"\nport = 1\nmodels = []\napi_key = \"sk-live\"\n",
        ] {
            let error = Config::from_toml(document).expect_err("must be rejected");
            assert_eq!(error.code(), "jarvis.config_parse", "{document}");
        }
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_level() {
        for document in [
            "schema_version = 1\nunknown_key = 1\n",
            "schema_version = 1\n[runtime]\nlog_level = \"info\"\nunknown = true\n",
            "schema_version = 1\n[storage]\nkind = \"sqlite\"\npath = \"/tmp/db\"\n",
            "schema_version = 1\n[model]\npolicy_id = \"a\"\nkey = \"sk-live\"\n",
            "schema_version = 2\n[model]\npolicy_id = \"a\"\nsecret = \"sk-live\"\n",
            // The reviewed-refusal table rejects unknown keys at its own level too, so a typo in a refusal
            // is a parse failure rather than a silently ignored field on the one input that narrows.
            "schema_version = 3\n[[tools.deny]]\ncapability = \"clock.now@1\"\nreason = \"x\"\nprincipal_id = \"a\"\n",
            "schema_version = 3\n[tools]\nunknown = []\n",
        ] {
            let error = Config::from_toml(document).expect_err("must be rejected");
            assert_eq!(error.code(), "jarvis.config_parse", "{document}");
        }
    }

    /// The reviewed refusals a document declares, as rules the evaluator can consume.
    ///
    /// A `[[tools.deny]]` entry is **the configuration form of a `tool_deny_rules` row**, and it exists so a
    /// deployment's policy can be reviewed in a diff and present before any database exists. These tests
    /// assert the *validation* — what an operator may write and what is refused — because that is the part
    /// that decides whether a refusal silently does nothing.
    #[test]
    fn a_reviewed_refusal_becomes_a_capability_keyed_rule() {
        let config = Config::from_toml(VALID).expect("valid document must parse");
        let rules = config
            .tools
            .reviewed_rules()
            .expect("both entries are usable");
        assert_eq!(rules.len(), 2, "both declared entries must produce a rule");

        // The first names a capability and must keep it: the *adapter* expands a capability into the
        // identities offering it, and it cannot do that if the capability is dropped here.
        assert_eq!(rules[0].capability.as_deref(), Some("email.send@1"));
        assert!(
            rules[0].rule.identity.is_none(),
            "a reviewed rule names a capability, never an identity — an identity-named refusal would stop \
             applying after a tool was recompiled, which loses a restriction",
        );
        assert_eq!(rules[0].reason, "outbound mail is never sent unattended");

        // The second names effects and no capability, which is the "refuse every destructive tool" form.
        // It must survive with **no** capability, because a capability-less rule that acquired one would
        // refuse one tool instead of the class the operator named.
        assert_eq!(rules[1].capability, None);
        assert_eq!(
            rules[1].rule.effects.len(),
            2,
            "both named effects must be parsed: {:?}",
            rules[1].rule.effects,
        );
        assert!(rules[1].rule.principal.is_none() && rules[1].rule.workspace.is_none());
    }

    #[test]
    fn a_refusal_that_names_nothing_is_refused_rather_than_stored() {
        // **The fail-open case this validation exists for.** A rule with no capability and no effects would
        // be accepted by the domain's own `DenyRule`, whose `matches` reports it as naming nothing — so it
        // would appear in the configuration file, be listed as a refusal, and refuse nothing at all. An
        // operator would believe they had forbidden something. Refusing it at load time is the only place
        // the mistake is still visible.
        for (field, document) in [
            (
                "rule",
                "schema_version = 3\n[[tools.deny]]\nreason = \"names nothing\"\n",
            ),
            // An empty reason is refused because the reason is what a refused principal is *shown*, and a
            // refusal with no explanation is one they cannot act on.
            (
                "reason",
                "schema_version = 3\n[[tools.deny]]\ncapability = \"clock.now@1\"\nreason = \"\"\n",
            ),
            // An unknown effect spelling is refused rather than dropped, because a dropped effect would
            // broaden the refusal to the whole tool — the opposite of what the operator wrote.
            (
                "effects",
                "schema_version = 3\n[[tools.deny]]\neffects = [\"teleport\"]\nreason = \"x\"\n",
            ),
            // A capability that is not canonical would match no identity, so the refusal would do nothing.
            (
                "capability",
                "schema_version = 3\n[[tools.deny]]\ncapability = \"not a capability\"\nreason = \"x\"\n",
            ),
        ] {
            let config = Config::from_toml(document).expect("the document itself is well formed");
            let error = config
                .tools
                .reviewed_rules()
                .expect_err("an unusable refusal must be refused");
            assert_eq!(
                error.code(),
                "jarvis.config_tool_deny_invalid",
                "{document}"
            );
            match error {
                ConfigError::InvalidToolDenyRule { field: named } => {
                    assert_eq!(named, field, "{document}");
                }
                other => {
                    // The variant check rather than a `panic!`, which this workspace denies outside tests
                    // and `clippy` refuses as an unconditional escape. `assert!` with a rendered value is
                    // both the diagnostic and the idiom the workspace uses.
                    assert!(
                        matches!(other, ConfigError::InvalidToolDenyRule { .. }),
                        "the refusal must name its field, got {other:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn a_profile_with_no_tools_table_declares_no_reviewed_refusal() {
        // The ordinary state, and it must be **empty rather than an error**: a profile with no `[tools]`
        // table ships no reviewed refusal, which is a complete configuration rather than an incomplete one.
        // A default of "refused because absent" would make every existing profile fail to start.
        let config = Config::from_toml("schema_version = 3").expect("minimal document must parse");
        assert!(config.tools.deny.is_empty());
        assert!(
            config
                .tools
                .reviewed_rules()
                .expect("no rules is not an error")
                .is_empty(),
        );
    }

    #[test]
    fn a_version_two_document_remains_readable_and_declares_no_refusal() {
        // The upgrade path, and the reason the version moved for a third time: a version-2 binary's
        // `deny_unknown_fields` would report an unknown `[tools]` table as a *parse* failure, when the
        // accurate diagnostic is "written by a newer JARVIS". A version-2 document must still load, and
        // must not acquire a refusal that was never declared in it.
        let version_two = "schema_version = 2\n[model]\npolicy_id = \"local-default\"\n";
        let config = Config::from_toml(version_two).expect("a version-2 document must still parse");
        assert_eq!(config.schema_version, 2);
        assert!(
            config.tools.deny.is_empty(),
            "a document with no tools table must not gain a reviewed refusal",
        );
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
            "schema_version = 4",
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

        // All supported versions are accepted, so each upgrade path is real rather than only documented.
        // `3` must not appear in the rejecting list above.

        // A negative or non-integer version is not silently accepted.
        assert!(Config::from_toml("schema_version = -1").is_err());
        assert!(Config::from_toml("schema_version = \"1\"").is_err());
    }

    #[test]
    fn an_unsupported_version_is_not_rewritten() {
        let dir = temp_dir("no-rewrite");
        let path = config_file_path(&dir);
        let original = "schema_version = 4\n[runtime]\nlog_level = \"info\"\n";
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
