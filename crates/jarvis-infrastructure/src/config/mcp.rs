//! The declared MCP servers a profile launches, as reviewed configuration.
//!
//! This is the input every MCP slice so far has been missing. `discover_stdio_server` takes a launch
//! specification and a **configured name**, `register_catalog` takes a trusted identity, and
//! `ToolRegistry` is where `RoutingExecutor`'s `McpServer` branch would dispatch — but nothing could
//! *declare* a server, so the composition had nothing to iterate: a daemon could not offer an MCP tool
//! because no profile could say one existed.
//!
//! # A server's environment is a list of *references*, never literals
//!
//! The environment is the one place an MCP server's credentials live — a GitHub token, an API key, a
//! database URL — and those are exactly what `AGENTS.md` forbids putting in a configuration file. So an
//! entry's **value** is a [`SecretReference`] resolved at launch, not a string.
//!
//! That is stricter than the literal form `McpLaunchSpec` takes, and the strictness is the point: a
//! `McpLaunchSpec` is built by the composition from validated configuration, so a literal reaching it would
//! have come from a file. Requiring a reference here means a credential **cannot** be written into a profile
//! in the first place, rather than being discouraged by a comment. The reference grammar already refuses a
//! locator outside the `JARVIS_` prefix, so a profile also cannot direct JARVIS to read an unrelated ambient
//! secret such as a cloud credential or a CI token.
//!
//! # Why the *resolved* program is required
//!
//! `McpLaunchSpec::program` is documented as "the **resolved** program path", because the child gets the
//! reviewed environment and `env_clear` removes `PATH` — so a bare `npx` cannot be resolved by the child and
//! must be resolved by the daemon at configuration time. That is why this section takes `program` and there
//! is no `command`-with-arguments shorthand for a PATH lookup: the shorthand reads as convenience and fails
//! at spawn time with `SpawnFailed`, which names the child rather than the configuration.
//!
//! # What is validated here, and what the launch specification validates
//!
//! The split follows the field's owner. This module validates the **identity** (the two name rules), the
//! declared argument and environment shape, and that a secret reference is well-formed. `McpLaunchSpec`'s
//! own `validate` re-checks the program, arguments, working directory, environment bounds, and the startup
//! timeout when the specification is built — so a value this module passes is still checked by the layer that
//! owns it, and neither restates the other's rules.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::ConfigError;
use super::secret::SecretReference;

/// A validated MCP server declaration: everything the composition needs to launch one server.
///
/// **A config-layer value rather than the adapter's `McpLaunchSpec`**, and that is the dependency
/// direction: this crate's `config` module is the layer that *reads a file*, while the adapter that builds a
/// child command is further out. Producing the specification here would make configuration depend on the
/// process launcher; producing this instead leaves the composition to translate one value into the other,
/// and keeps the validated shape testable without a process.
///
/// The environment stays as **references** rather than resolved strings, so this value is safe to hold,
/// compare, and report: resolution needs a [`super::secret::SecretResolver`], which only the composition
/// has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerDeclaration {
    /// The configuration identity, validated under both name rules.
    pub name: String,
    /// The resolved program path.
    pub program: PathBuf,
    /// The argument vector.
    pub args: Vec<String>,
    /// The environment the child may see, as references to be resolved at launch.
    pub env: BTreeMap<String, SecretReference>,
    /// Whether this server should run.
    pub enabled: bool,
    /// How long the child may take to become usable.
    pub startup_timeout_ms: u64,
}

/// A reviewed MCP server, as an operator declares it.
///
/// See the module doc for why the environment holds references and why the program is a resolved path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerSection {
    /// The server's configuration identity, unique within the profile.
    ///
    /// **The trusted half of the identity pair**, and the reason it is operator-declared: `registration`
    /// compares it against the owner a server's tools claim, so a server that declares another's name is
    /// refused rather than inheriting its approvals. It must satisfy **two** rules — a configuration identity
    /// and a tool source owner — and a name satisfying one but not the other would produce an all-refused
    /// catalog, so both are checked here.
    pub name: String,
    /// The resolved program to launch. See the module doc for why this is not a bare command name.
    pub program: String,
    /// The argument vector, passed to the child verbatim and never through a shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// The complete environment the child may see, as **references**.
    ///
    /// A `BTreeMap` rather than a list of pairs, so a variable declared twice is unrepresentable instead of
    /// silently taking one of the two values. The order does not matter: `McpLaunchSpec` sorts or passes the
    /// pairs verbatim, and a child reads its environment by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, SecretReference>,
    /// Whether this server is launched. Absent means enabled, so a declaration added and later disabled is a
    /// one-word change rather than a deletion that loses the reviewed record of what it was.
    #[serde(default = "default_enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
    /// How long the child may take to become usable, in milliseconds.
    ///
    /// Bounded by `McpLaunchSpec`'s own range rather than here, so the two cannot disagree about what is
    /// acceptable — the same reason the environment bound is imported rather than restated.
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_ms: u64,
}

/// The default for [`McpServerSection::enabled`]: a declared server runs.
fn default_enabled() -> bool {
    true
}

/// Reports whether the field can be omitted, so a serialized profile stays close to what an operator wrote.
///
/// **Takes `&bool` because serde requires it**, and the lint is allowed rather than the signature changed:
/// `skip_serializing_if` is invoked with a reference to the field, so a by-value signature does not compile.
/// The allow is scoped to this function and carries its reason, so it cannot be mistaken for a preference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_enabled(enabled: &bool) -> bool {
    *enabled
}

/// The default startup timeout, mirroring the launch specification's own.
fn default_startup_timeout() -> u64 {
    crate::mcp::process::DEFAULT_MCP_STARTUP_TIMEOUT_MS
}

impl McpServerSection {
    /// Validates the declaration and produces the domain value the composition reads.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidMcpServer`] naming the field at fault: an unusable name under either
    /// rule, an unusable program, an argument or environment list over its bound, or an unusable environment
    /// key.
    ///
    /// It is reported at startup rather than tolerated, so a profile never runs with a server declaration that
    /// silently does nothing.
    pub fn to_declaration(&self) -> Result<McpServerDeclaration, ConfigError> {
        let fail = |field: &'static str| ConfigError::InvalidMcpServer { field };
        // **Both name rules, and there are two.** `ServerConfigId` accepts dots, hyphens, and a leading
        // digit; a tool source owner additionally requires a leading letter and no empty dotted part. A name
        // passing the first and failing the second would refuse *every* tool the server offered, which reads
        // as a server defect for a name-shaped cause — the same pair `discovery` checks before spawning.
        if jarvis_domain::tool::registry::ServerConfigId::new(&self.name).is_err() {
            return Err(fail("name"));
        }
        if crate::mcp::server_source(&self.name).is_err() {
            return Err(fail("name"));
        }
        if self.program.is_empty() || self.program.contains('\0') {
            return Err(fail("program"));
        }
        // The argument bound is the launch specification's, imported rather than restated. The *content*
        // rule is deliberately not duplicated: `usable_token` in `process.rs` rejects a control character in
        // any argument, and re-deriving that here would be a second rule to keep in step.
        if self.args.len() > crate::mcp::process::MAX_MCP_ARGUMENTS {
            return Err(fail("args"));
        }
        if self.env.len() > crate::mcp::process::MAX_MCP_ENV_VARS {
            return Err(fail("env"));
        }
        // An environment key reaches the process table, so it is checked here for the same reason an argument
        // is: `env_clear` means the child sees exactly this map, and a key with a NUL or a `=` in it would
        // either fail the spawn or, in the `=` case, let a declared key name a different variable than the one
        // the child reads.
        for key in self.env.keys() {
            let usable = !key.is_empty()
                && key.len() <= crate::mcp::process::MAX_MCP_TOKEN_BYTES
                && !key.contains('\0')
                && !key.contains('=')
                && !key.chars().any(char::is_control);
            if !usable {
                return Err(fail("env"));
            }
        }
        Ok(McpServerDeclaration {
            name: self.name.clone(),
            program: PathBuf::from(&self.program),
            args: self.args.clone(),
            env: self.env.clone(),
            enabled: self.enabled,
            startup_timeout_ms: self.startup_timeout_ms,
        })
    }
}

/// The MCP servers this profile launches.
///
/// An optional table, so every existing profile loads unchanged — the same shape `[[tools.deny]]` uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpSection {
    /// The declared servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<McpServerSection>,
}

impl McpSection {
    /// Returns whether nothing is declared, so a serialized profile omits the table entirely.
    ///
    /// A method rather than a free function because `skip_serializing_if` takes a path, and this keeps the
    /// omission rule beside the type it describes — an empty `[mcp]` table written back would be noise an
    /// operator did not author.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Validates every declaration and returns them in declaration order.
    ///
    /// # Errors
    ///
    /// Returns the first refusal, with its field named, or [`ConfigError::InvalidMcpServer`] naming `name`
    /// when two declarations share a name.
    ///
    /// **A duplicate name is refused rather than deduplicated**, and it is the check this section exists for
    /// beyond per-field validation: the name is the trusted identity every registration and every grant is
    /// recorded against, so two servers claiming it would have one server's tools registered under an
    /// identity an operator believed belonged to the other. The second would then be refused with a
    /// source-collision the operator never caused — a symptom with no visible cause in the file.
    pub fn declarations(&self) -> Result<Vec<McpServerDeclaration>, ConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        let mut declarations = Vec::with_capacity(self.servers.len());
        for server in &self.servers {
            let declaration = server.to_declaration()?;
            if !seen.insert(declaration.name.clone()) {
                return Err(ConfigError::InvalidMcpServer { field: "name" });
            }
            declarations.push(declaration);
        }
        Ok(declarations)
    }

    /// Returns the declarations that are enabled.
    ///
    /// Separate from [`Self::declarations`] so "what did the operator declare" and "what should run" stay two
    /// questions: a disabled server is still a reviewed record that must remain valid, and a composition that
    /// filtered before validating would let a broken declaration sit in the file doing nothing.
    ///
    /// # Errors
    ///
    /// As [`Self::declarations`].
    pub fn enabled_declarations(&self) -> Result<Vec<McpServerDeclaration>, ConfigError> {
        Ok(self
            .declarations()?
            .into_iter()
            .filter(|declaration| declaration.enabled)
            .collect())
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
