//! Process plugin manifest: the typed, validated contract from
//! `docs/contracts/plugin-manifest.md`.
//!
//! `TLS-011` asked to "define the plugin manifest and process supervision contract". The *contract* is
//! accepted (`docs/contracts/plugin-manifest.md`); this module is the document it describes, as types a
//! reader and a writer share. **Discovery and installation grant nothing** — this module parses and
//! validates a manifest, and the contract is explicit that a parsed manifest is not a grant.
//!
//! ## What this module is, and what it deliberately is not
//!
//! It is the **shape and the field rules**: a manifest that claims a package-relative executable which
//! escapes its root, or a digest that is not a canonical `SHA-256`, or a display name carrying a control
//! character, is refused here, before anything is extracted or executed. `TLS-014` (provenance and
//! signature verification) and `TLS-015` (grants, launch, supervision) build on this; this module is
//! the boundary those slices must not have to re-implement.
//!
//! It is **not** a signature verifier and **not** an installer. The contract puts package verification
//! before extraction and execution, and that is `TLS-014`; the `package` block here carries the digest,
//! signature reference, and source so the verifier has a typed value to check, and nothing in this
//! module trusts them.
//!
//! ## Why the manifest is parsed through `parse`, and `Deserialize` is derived
//!
//! The same argument `ReleaseManifest` records (`BRN-061`): the type has a fallible constructor and
//! derives `Deserialize`, and the class that audit flags is "a validating constructor plus a
//! *bypassing* deserialization path". Here the one path is [`PluginManifest::parse`], which
//! deserializes and then calls [`PluginManifest::validate`], which returns a **typed** error naming
//! the exact field. A hand-written validating `Deserialize` would collapse every variant into serde's
//! single opaque error — which an operator cannot act on.
//!
//! ## Security posture
//!
//! Every value in a manifest is **untrusted input** (`AGENTS.md`: "plugins and external runtimes" are
//! untrusted). The rules below are the ones whose failure is a filesystem, execution, or log-integrity
//! fault rather than a cosmetic one:
//!
//! - `entrypoint.executable` is a **package-relative path that cannot escape its root** — no absolute
//!   path, no `..`, no Windows drive/ADS colon, no reserved device name, no trailing dot or space (both
//!   of which Windows strips, making two strings name one file);
//! - `id` and `publisher` are the **source identity** (the display name and executable filename never
//!   identify a plugin, per the contract), so `id` is a bounded slug and not free text;
//! - `arguments` and `environment_allowlist` are bounded and control-character-free, because they reach
//!   a process spawn and a log line; arguments are an **argv array** and are never shell-concatenated,
//!   which is why a shell metacharacter is data here rather than a refusal;
//! - `resource_limits` are positive and bounded, because a zero or unbounded limit is not a limit.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The manifest schema version this build understands.
///
/// A string, unlike the release manifest's integer, because the contract's own example writes
/// `"schema_version": "1.0.0"` — and the two documents are allowed to differ here: a plugin manifest's
/// version is a *semantic* version the contract chose, so this build matches the contract rather than
/// imposing the release manifest's shape on it.
pub const PLUGIN_MANIFEST_SCHEMA_VERSION: &str = "1.0.0";

/// The accepted size of a manifest document.
///
/// A manifest is a small fixed record — the contract's example is under 1 KiB. Bounding it means a
/// hostile document cannot exhaust memory before it is rejected, and it bounds the embedded
/// `configuration_schema` transitively.
pub const MAX_PLUGIN_MANIFEST_BYTES: u64 = 256 * 1024;

/// The longest a package-relative path or identifier may be.
pub const MAX_PLUGIN_TOKEN_BYTES: usize = 256;

/// The most capabilities one manifest may declare, across `provides` and `requests` together.
pub const MAX_PLUGIN_CAPABILITIES: usize = 64;

/// The most platforms one manifest may target.
pub const MAX_PLUGIN_PLATFORMS: usize = 16;

/// The most launch arguments one entrypoint may declare.
pub const MAX_PLUGIN_ARGUMENTS: usize = 64;

/// The most environment variable names one entrypoint may allow.
pub const MAX_PLUGIN_ENVIRONMENT: usize = 32;

/// The shortest a launch timeout may be, in milliseconds.
///
/// A startup timeout of zero would mean "never started", which is not a limit but a refusal with a
/// confusing name; one millisecond is the smallest value that can observe a process at all.
pub const MIN_PLUGIN_STARTUP_TIMEOUT_MS: u64 = 1;

/// The longest a launch timeout may be, in milliseconds (one minute).
///
/// A supervision timeout is a bound on a *child process*, so it is bounded here rather than trusted: an
/// unbounded startup timeout is a hang with no clock, which is the failure the limit exists to prevent.
pub const MAX_PLUGIN_STARTUP_TIMEOUT_MS: u64 = 60_000;

/// The largest a captured stream may be, in bytes (16 MiB).
pub const MAX_PLUGIN_STREAM_BYTES: u64 = 16 * 1024 * 1024;

/// The protocol kinds this build supports, per the contract's "versioned JARVIS runtime protocol and
/// MCP stdio".
///
/// A `kind` outside this set requires an accepted contract and evidence note, so it is refused by name
/// rather than carried through as an opaque string — the same closed-set rule the rest of the workspace
/// applies to a vocabulary a later layer switches on.
pub const SUPPORTED_PROTOCOL_KINDS: &[&str] = &["jarvis-runtime", "mcp-stdio"];

/// A parsed, validated process plugin manifest.
///
/// Build one with [`PluginManifest::parse`], which enforces every rule below. The fields are public so a
/// reader can inspect a manifest it has already validated, but a value constructed by struct literal is
/// **not** validated — the same caveat `ErrorEnvelope` records for the wire boundary, and the reason
/// `parse` is the documented entry point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// The manifest schema version. Must equal [`PLUGIN_MANIFEST_SCHEMA_VERSION`].
    pub schema_version: String,
    /// The meta-schema URL the document points at, when it carries one.
    ///
    /// **The JSON Schema `$schema` keyword, accepted rather than refused as an unknown field.** The
    /// contract's own example includes it, and `deny_unknown_fields` would otherwise reject the
    /// document this module exists to read — so the field is modelled, validated as a bounded reference
    /// when present, and otherwise optional. It is **not** consulted to choose a validator: accepting a
    /// URL from an untrusted document and fetching or honouring it is how a manifest would point JARVIS
    /// at a schema of the manifest's own choosing, which is the opposite of validating it.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema_url: Option<String>,
    /// The stable plugin identifier, a bounded lowercase slug such as `example.research-runtime`.
    ///
    /// Part of the **source identity**, so it is not the display `name`: the contract is explicit that
    /// "display name or executable filename never identifies a plugin".
    pub id: String,
    /// The human-readable name, shown to an operator. Never an identity.
    pub name: String,
    /// The plugin's semantic version.
    pub version: String,
    /// The publisher's stable identifier, part of the source identity.
    pub publisher: String,
    /// The package bytes' identity: digest, signature reference, and source.
    pub package: PackageIdentity,
    /// What this plugin is compatible with.
    pub compatibility: Compatibility,
    /// How the process is launched.
    pub entrypoint: Entrypoint,
    /// What the plugin provides and what it requests.
    pub capabilities: Capabilities,
    /// The plugin's configuration schema, an object the contract says "marks secret-reference fields,
    /// sensitive display fields, defaults, bounds, and restart requirements".
    ///
    /// Carried as a bounded JSON object rather than modelled field by field, because it describes the
    /// *plugin's own* configuration and JARVIS does not interpret it here. It must be an object: an
    /// array or scalar would not be a schema, and accepting one would let a manifest claim a
    /// configuration shape it cannot have.
    pub configuration_schema: serde_json::Value,
    /// The supervision limits.
    pub resource_limits: ResourceLimits,
    /// Operator-facing documentation references.
    pub documentation: Documentation,
}

/// The package bytes' identity: digest, signature reference, and source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    /// The `sha256:<64 lowercase hex>` digest of the package bytes.
    pub digest: String,
    /// The signature over those bytes, by reference.
    pub signature: SignatureReference,
    /// Where the package came from, a bounded reference for an operator to inspect.
    pub source: String,
}

/// A signature, by reference rather than inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureReference {
    /// The signature kind, for example `sigstore-bundle`.
    ///
    /// A bounded identifier rather than a closed set: signature *kinds* are `TLS-014`'s vocabulary and
    /// pinning it here would refuse a kind the verifier supports. The **reference** is validated as a
    /// package-relative path, which is the field whose mishandling is a filesystem fault.
    pub kind: String,
    /// The package-relative path to the signature material.
    pub value_ref: String,
}

/// What the plugin is compatible with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compatibility {
    /// The JARVIS version range this plugin supports, for example `>=0.1.0 <0.2.0`.
    pub jarvis: String,
    /// The process protocol this plugin speaks.
    pub protocol: ProtocolRequirement,
    /// The platform targets this plugin supports.
    pub platforms: Vec<String>,
}

/// The process protocol a plugin speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRequirement {
    /// The protocol kind, one of [`SUPPORTED_PROTOCOL_KINDS`].
    pub kind: String,
    /// The protocol versions this plugin accepts.
    pub versions: Vec<String>,
}

/// How the process is launched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entrypoint {
    /// The executable, a **package-relative path**. The contract forbids this escaping the verified
    /// installation root, and [`validate`](PluginManifest::validate) enforces it.
    pub executable: String,
    /// Fixed launch arguments, passed as an **argv array** and never shell-concatenated.
    pub arguments: Vec<String>,
    /// The working directory, a package-relative path or a named scope.
    pub working_directory: String,
    /// Environment variable **names** the process is allowed to receive, never values.
    pub environment_allowlist: Vec<String>,
}

/// What the plugin provides and what it requests.
///
/// `requests` is an **untrusted permission request**: installation creates no grant (the contract's
/// Permissions and Grants section), so this is data a reviewer reads, not authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Capabilities the plugin provides.
    pub provides: Vec<String>,
    /// Capabilities the plugin requests. Untrusted until reviewed and granted.
    pub requests: Vec<String>,
}

/// The supervision limits for a launched process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    /// How long startup may take, in milliseconds.
    pub startup_timeout_ms: u64,
    /// The largest protocol message accepted, in bytes.
    pub message_bytes: u64,
    /// The largest stdout capture, in bytes.
    pub stdout_bytes: u64,
    /// The largest stderr capture, in bytes.
    pub stderr_bytes: u64,
}

/// Operator-facing documentation references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct Documentation {
    /// Where setup instructions are.
    pub setup: String,
    /// Where the privacy statement is.
    pub privacy: String,
    /// Where support is reached.
    pub support: String,
}

impl PluginManifest {
    /// Parses and validates a manifest document.
    ///
    /// This is the **only** path that produces a validated manifest. It does not verify the package
    /// signature — that is `TLS-014` — and it grants nothing.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] describing the first problem found. Rejection messages never
    /// echo document contents back, because the document is untrusted input.
    pub fn parse(bytes: &[u8]) -> Result<Self, PluginManifestError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PLUGIN_MANIFEST_BYTES {
            return Err(PluginManifestError::TooLarge);
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| PluginManifestError::Malformed)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validates every field a reader, an extractor, or a supervisor depends on.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] describing the first problem found.
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        if self.schema_version != PLUGIN_MANIFEST_SCHEMA_VERSION {
            return Err(PluginManifestError::UnsupportedSchemaVersion {
                found: self.schema_version.clone(),
            });
        }
        if !is_plugin_id(&self.id) {
            return Err(PluginManifestError::InvalidIdentity { field: "id" });
        }
        if !is_plugin_id(&self.publisher) {
            return Err(PluginManifestError::InvalidIdentity { field: "publisher" });
        }
        // The display name is text an operator reads, so it is bounded and free of control characters
        // (which would corrupt a terminal or a log line) but otherwise unrestricted — it is not an
        // identity, so it is not held to the slug rule.
        if !is_display_text(&self.name) {
            return Err(PluginManifestError::EmptyField { field: "name" });
        }
        if !is_version_text(&self.version) {
            return Err(PluginManifestError::EmptyField { field: "version" });
        }

        self.package.validate()?;
        self.compatibility.validate()?;
        self.entrypoint.validate()?;
        self.capabilities.validate()?;
        self.resource_limits.validate()?;
        self.documentation.validate()?;

        if !self.configuration_schema.is_object() {
            return Err(PluginManifestError::ConfigurationSchemaNotAnObject);
        }
        // The `$schema` URL, when present, is a bounded reference for an operator to read. It is not
        // dereferenced or honoured (see the field's doc), so the rule is only that it is bounded text.
        if let Some(url) = &self.schema_url
            && !is_bounded_reference(url)
        {
            return Err(PluginManifestError::EmptyField { field: "$schema" });
        }
        Ok(())
    }
}

impl PackageIdentity {
    /// Validates the package identity's three fields.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] for the first field that fails.
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        if !is_canonical_sha256(&self.digest) {
            return Err(PluginManifestError::InvalidDigest);
        }
        // The signature reference is a package-relative path a verifier reads, so it is validated as
        // one: a reference that escaped the package would let a manifest point the verifier at a file
        // outside the package it claims to sign.
        if !is_package_relative_path(&self.signature.value_ref) {
            return Err(PluginManifestError::UnsafePath {
                field: "package.signature.value_ref",
            });
        }
        if !is_bounded_reference(&self.source) {
            return Err(PluginManifestError::EmptyField {
                field: "package.source",
            });
        }
        Ok(())
    }
}

impl Compatibility {
    /// Validates the compatibility block.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] for the first field that fails.
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        if !is_version_range(&self.jarvis) {
            return Err(PluginManifestError::InvalidVersionRange);
        }
        if !SUPPORTED_PROTOCOL_KINDS.contains(&self.protocol.kind.as_str()) {
            return Err(PluginManifestError::UnsupportedProtocol {
                kind: self.protocol.kind.clone(),
            });
        }
        if self.protocol.versions.is_empty() || self.protocol.versions.len() > MAX_PLUGIN_PLATFORMS
        {
            return Err(PluginManifestError::EmptyField {
                field: "compatibility.protocol.versions",
            });
        }
        for version in &self.protocol.versions {
            if !is_version_text(version) {
                return Err(PluginManifestError::EmptyField {
                    field: "compatibility.protocol.versions",
                });
            }
        }
        if self.platforms.is_empty() || self.platforms.len() > MAX_PLUGIN_PLATFORMS {
            return Err(PluginManifestError::EmptyField {
                field: "compatibility.platforms",
            });
        }
        for platform in &self.platforms {
            if !is_platform_token(platform) {
                return Err(PluginManifestError::InvalidPlatform {
                    platform: platform.clone(),
                });
            }
        }
        Ok(())
    }
}

impl Entrypoint {
    /// Validates the launch description.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] for the first field that fails.
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        if !is_package_relative_path(&self.executable) {
            return Err(PluginManifestError::UnsafePath {
                field: "entrypoint.executable",
            });
        }
        if !is_package_relative_path(&self.working_directory) {
            return Err(PluginManifestError::UnsafePath {
                field: "entrypoint.working_directory",
            });
        }
        if self.arguments.len() > MAX_PLUGIN_ARGUMENTS {
            return Err(PluginManifestError::TooManyArguments);
        }
        for argument in &self.arguments {
            // Arguments are an argv array, so a shell metacharacter is data and is not refused. A
            // control character is, because it reaches a log line and a terminal.
            if argument.len() > MAX_PLUGIN_TOKEN_BYTES || contains_control(argument) {
                return Err(PluginManifestError::UnsafeArgument);
            }
        }
        if self.environment_allowlist.len() > MAX_PLUGIN_ENVIRONMENT {
            return Err(PluginManifestError::TooManyEnvironmentNames);
        }
        for name in &self.environment_allowlist {
            if !is_environment_name(name) {
                return Err(PluginManifestError::InvalidEnvironmentName { name: name.clone() });
            }
        }
        Ok(())
    }
}

impl Capabilities {
    /// Validates the capability lists.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] for the first field that fails.
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        let total = self.provides.len() + self.requests.len();
        if total > MAX_PLUGIN_CAPABILITIES {
            return Err(PluginManifestError::TooManyCapabilities);
        }
        for capability in self.provides.iter().chain(self.requests.iter()) {
            if !is_capability_token(capability) {
                return Err(PluginManifestError::InvalidCapability {
                    capability: capability.clone(),
                });
            }
        }
        Ok(())
    }
}

impl ResourceLimits {
    /// Validates the supervision limits.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] for the first limit that fails.
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        if !(MIN_PLUGIN_STARTUP_TIMEOUT_MS..=MAX_PLUGIN_STARTUP_TIMEOUT_MS)
            .contains(&self.startup_timeout_ms)
        {
            return Err(PluginManifestError::LimitOutOfRange {
                field: "resource_limits.startup_timeout_ms",
            });
        }
        for (field, value) in [
            ("resource_limits.message_bytes", self.message_bytes),
            ("resource_limits.stdout_bytes", self.stdout_bytes),
            ("resource_limits.stderr_bytes", self.stderr_bytes),
        ] {
            // A zero-byte stream bound would mean "never capture anything", which is a refusal wearing
            // a limit's name; the upper bound keeps a hostile manifest from claiming an unbounded
            // capture the supervisor would then attempt.
            if value == 0 || value > MAX_PLUGIN_STREAM_BYTES {
                return Err(PluginManifestError::LimitOutOfRange { field });
            }
        }
        Ok(())
    }
}

impl Documentation {
    /// Validates the documentation references.
    ///
    /// # Errors
    ///
    /// Returns the [`PluginManifestError`] for the first reference that fails.
    pub fn validate(&self) -> Result<(), PluginManifestError> {
        for (field, value) in [
            ("documentation.setup", &self.setup),
            ("documentation.privacy", &self.privacy),
            ("documentation.support", &self.support),
        ] {
            if !is_bounded_reference(value) {
                return Err(PluginManifestError::EmptyField { field });
            }
        }
        Ok(())
    }
}

/// Why a plugin manifest could not be accepted.
///
/// Every variant is a distinct, actionable cause: "the executable escapes the package" and "the digest
/// is not canonical" need different operator responses, so they never collapse into one message.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PluginManifestError {
    /// The document exceeds [`MAX_PLUGIN_MANIFEST_BYTES`].
    #[error("the plugin manifest is larger than the accepted limit")]
    TooLarge,
    /// The document is not valid `JSON` or has an unknown field.
    #[error("the plugin manifest is malformed")]
    Malformed,
    /// The schema version is not one this build understands.
    #[error("the plugin manifest schema version is unsupported by this binary")]
    UnsupportedSchemaVersion {
        /// The version found in the document.
        found: String,
    },
    /// A source-identity field is not a valid plugin slug.
    #[error("a plugin source-identity field is not a valid identifier")]
    InvalidIdentity {
        /// The field name.
        field: &'static str,
    },
    /// A required field is empty, too long, or carries control characters.
    #[error("a required plugin manifest field is empty or malformed")]
    EmptyField {
        /// The field name.
        field: &'static str,
    },
    /// The package digest is not `sha256:<64 lowercase hex>`.
    #[error("the plugin package digest is not a canonical lowercase SHA-256")]
    InvalidDigest,
    /// A path field is absolute, a traversal, or otherwise escapes its root.
    #[error("a plugin manifest path escapes its package root")]
    UnsafePath {
        /// The field name.
        field: &'static str,
    },
    /// The JARVIS version range is not a bounded range expression.
    #[error("the plugin compatibility range is not a valid version range")]
    InvalidVersionRange,
    /// The protocol kind is not one this build supports.
    #[error("the plugin names a process protocol this build does not support")]
    UnsupportedProtocol {
        /// The protocol kind found in the document.
        kind: String,
    },
    /// A platform token is not a recognized target.
    #[error("a plugin platform token is not a recognized target")]
    InvalidPlatform {
        /// The platform token found in the document.
        platform: String,
    },
    /// More launch arguments were declared than the bound allows.
    #[error("the plugin declares more launch arguments than the accepted limit")]
    TooManyArguments,
    /// A launch argument is too long or carries a control character.
    #[error("a plugin launch argument is malformed")]
    UnsafeArgument,
    /// More environment names were declared than the bound allows.
    #[error("the plugin declares more environment names than the accepted limit")]
    TooManyEnvironmentNames,
    /// An environment allowlist entry is not a variable name.
    #[error("a plugin environment allowlist entry is not a variable name")]
    InvalidEnvironmentName {
        /// The rejected name.
        name: String,
    },
    /// More capabilities were declared than the bound allows.
    #[error("the plugin declares more capabilities than the accepted limit")]
    TooManyCapabilities,
    /// A capability token is not a valid identifier.
    #[error("a plugin capability token is not a valid identifier")]
    InvalidCapability {
        /// The rejected capability.
        capability: String,
    },
    /// A supervision limit is zero or above its ceiling.
    #[error("a plugin resource limit is zero or above its ceiling")]
    LimitOutOfRange {
        /// The field name.
        field: &'static str,
    },
    /// The configuration schema is not a JSON object.
    #[error("the plugin configuration schema is not a JSON object")]
    ConfigurationSchemaNotAnObject,
}

impl PluginManifestError {
    /// Returns the stable, namespaced error code.
    ///
    /// `jarvis.*`, following `ReleaseError`: these are the daemon's own failures about a document, not a
    /// plugin's vocabulary, and the emission boundary would rewrite a `plugin.*` code it does not own.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TooLarge => "jarvis.plugin_manifest_too_large",
            Self::Malformed => "jarvis.plugin_manifest_malformed",
            Self::UnsupportedSchemaVersion { .. } => "jarvis.plugin_schema_unsupported",
            Self::InvalidIdentity { .. } => "jarvis.plugin_identity_invalid",
            Self::EmptyField { .. } => "jarvis.plugin_field_empty",
            Self::InvalidDigest => "jarvis.plugin_digest_invalid",
            Self::UnsafePath { .. } => "jarvis.plugin_path_unsafe",
            Self::InvalidVersionRange => "jarvis.plugin_version_range_invalid",
            Self::UnsupportedProtocol { .. } => "jarvis.plugin_protocol_unsupported",
            Self::InvalidPlatform { .. } => "jarvis.plugin_platform_invalid",
            Self::TooManyArguments => "jarvis.plugin_arguments_too_many",
            Self::UnsafeArgument => "jarvis.plugin_argument_unsafe",
            Self::TooManyEnvironmentNames => "jarvis.plugin_environment_too_many",
            Self::InvalidEnvironmentName { .. } => "jarvis.plugin_environment_name_invalid",
            Self::TooManyCapabilities => "jarvis.plugin_capabilities_too_many",
            Self::InvalidCapability { .. } => "jarvis.plugin_capability_invalid",
            Self::LimitOutOfRange { .. } => "jarvis.plugin_limit_out_of_range",
            Self::ConfigurationSchemaNotAnObject => "jarvis.plugin_configuration_schema_invalid",
        }
    }
}

/// Returns `true` when `value` is a valid plugin source-identity slug.
///
/// A slug is lowercase alphanumeric with `.` and `-` separators, must contain at least one `.` (the
/// contract's own example is `example.research-runtime`), and is bounded. **Not free text:** the
/// display name never identifies a plugin, so this is the field that does, and a value that could carry
/// control characters or an unbounded length is refused rather than stored beside a grant that will be
/// compared against it.
#[must_use]
pub fn is_plugin_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PLUGIN_TOKEN_BYTES
        && value.contains('.')
        && !value.starts_with('.')
        && !value.ends_with('.')
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

/// Returns `true` when `value` is display text an operator may be shown.
///
/// Bounded and free of control characters, but otherwise unrestricted: this is a name, not an identity,
/// so it is allowed spaces and capitals. A control character is refused because it would corrupt a
/// terminal or a log line — the contract requires safe audit records and redacted diagnostics.
#[must_use]
pub fn is_display_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_PLUGIN_TOKEN_BYTES && !contains_control(value)
}

/// Returns `true` when `value` looks like a semantic version.
///
/// The same shape check `release::is_version_text` applies — a charset and length bound, not a full
/// `SemVer` parse. It exists so a hostile version string cannot carry control characters into a log.
#[must_use]
pub fn is_version_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.starts_with(|character: char| character.is_ascii_digit())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// Returns `true` when `value` is a `sha256:<64 lowercase hex>` digest.
///
/// The `sha256:` prefix is required, so a bare digest — which could be any algorithm — is refused rather
/// than compared against a `SHA-256` by a reader that assumed the algorithm. This is the same rule
/// `SchemaFingerprint` records: an algorithm prefix is what makes two different digests incomparable.
#[must_use]
pub fn is_canonical_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Returns `true` when `path` is a package-relative path that cannot escape its root.
///
/// **This is the rule the contract's entrypoint section turns on** — "executable paths are
/// package-relative, canonicalized, and cannot escape the verified installation root" — and every part
/// of it defends a way two strings could name one file or one string could leave the package:
///
/// - an **absolute** path or a leading separator is outside the package by construction;
/// - a `..` segment is a **traversal**, and it is *refused* rather than resolved, because resolving it
///   discards the fact that the manifest asked to leave and, with links, the lexical answer differs from
///   the filesystem's;
/// - a `.` segment or an empty segment (from `//` or a trailing separator) is a second spelling of a
///   path the manifest could have written plainly;
/// - a **colon** is a Windows drive (`C:`) or an alternate-data-stream (`file:stream`) separator;
/// - a **trailing dot or space** is stripped by Windows, so `bin/rt.` and `bin/rt` name one file;
/// - a **reserved device name** (`con`, `nul`, `aux`, `prn`, `com1`, `lpt1`, …) names a device whatever
///   its extension or case on Windows.
#[must_use]
pub fn is_package_relative_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PLUGIN_TOKEN_BYTES || contains_control(path) {
        return false;
    }
    if path.contains(':') || path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    // Both separators are treated as separators: a manifest is one document, and a `\` is a separator on
    // Windows whether or not the manifest author intended it as one.
    for segment in path.split(['/', '\\']) {
        // A `..` segment is caught by **two** rules here — this one and the trailing-dot rule below —
        // and that redundancy is deliberate rather than confusing: a mutation that disabled either alone
        // left the refusal standing (the test still passed), which is what defence in depth looks like.
        // The trailing-dot rule exists for `rt.` and the reserved-name rule for `con`; `..` satisfies the
        // dot rule for an unrelated reason, so removing the explicit `..` check would not — today — open a
        // traversal. It is written out anyway so the rule is *stated* rather than inferred from a
        // character class that happens to cover it.
        if segment.is_empty() || segment == "." || segment == ".." {
            return false;
        }
        if segment.ends_with(' ') || segment.ends_with('.') {
            return false;
        }
        if is_reserved_device_name(segment) {
            return false;
        }
    }
    true
}

/// The Windows reserved device names, matched on a segment's stem.
///
/// A module-level constant rather than one declared inside [`is_reserved_device_name`], because an item
/// declared after a statement is a confusing placement (`clippy::items_after_statements`) and there is
/// nothing function-local about the list.
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Returns `true` when `name` is a Windows reserved device name, with or without an extension.
///
/// The check is on the **stem** (the part before the first dot) so `con.txt` and `AUX.tar.gz` are
/// refused while `console.log` is not — a check on the whole name would refuse a legitimate file whose
/// name merely begins with one of these, and a check that ignored the extension would miss `nul.txt`,
/// which Windows still treats as the device.
#[must_use]
pub fn is_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// Returns `true` when `value` is a bounded reference (a URL or a relative path) for an operator.
#[must_use]
pub fn is_bounded_reference(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_PLUGIN_TOKEN_BYTES && !contains_control(value)
}

/// Returns `true` when `value` is a bounded version-range expression.
///
/// A range like `>=0.1.0 <0.2.0`: the operator characters, digits, and dots are allowed, nothing else.
/// This is a shape check so a hostile range cannot carry control characters through; interpreting the
/// range is `TLS-014`/`TLS-015`'s, against the version comparison the installer already has.
#[must_use]
pub fn is_version_range(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'-' | b'+' | b'<' | b'>' | b'=' | b' ' | b'*')
        })
        && value.bytes().any(|byte| byte.is_ascii_digit())
}

/// Returns `true` when `platform` is a bounded target token such as `windows-x86_64`.
#[must_use]
pub fn is_platform_token(platform: &str) -> bool {
    !platform.is_empty()
        && platform.len() <= 64
        && platform.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

/// Returns `true` when `capability` is a bounded capability token.
///
/// Capabilities look like `runtime.text`, `jarvis.tools.read:selected`, or `network:api.example.org` —
/// a namespaced name with an optional selector, so the allowed characters include `.`, `:`, `-`, `_`,
/// and `/`. Bounded and control-free, because it is compared against a grant.
#[must_use]
pub fn is_capability_token(capability: &str) -> bool {
    !capability.is_empty()
        && capability.len() <= MAX_PLUGIN_TOKEN_BYTES
        && capability.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b':' | b'-' | b'_' | b'/' | b'*')
        })
}

/// Returns `true` when `name` is an environment **variable name** such as `PATH` or `LANG`.
///
/// Uppercase letters, digits, and `_`, not starting with a digit. **Names, never values:** the contract
/// says the environment "contains scoped handles or short-lived credentials, not daemon ambient
/// secrets", so a manifest declares which variables its process may receive and the *values* are
/// supplied by JARVIS at launch, not by the document.
#[must_use]
pub fn is_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PLUGIN_TOKEN_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Returns `true` when `value` contains a control character.
///
/// Deliberately the ASCII control range plus `DEL`: those are the characters that corrupt a terminal, a
/// log line, or a JSON string's rendering, and they have no place in an identifier, a path, or a
/// version. A newline in a display name is the canonical example.
#[must_use]
pub fn contains_control(value: &str) -> bool {
    value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
}

#[cfg(test)]
mod tests;
