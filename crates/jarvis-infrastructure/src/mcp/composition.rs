//! Composing a declared MCP server into a launched session, a registered catalog, and an executor.
//!
//! This is the join the MCP slices have been building toward. Every piece exists and each is tested in
//! isolation — [`McpServerDeclaration`] is what an operator wrote, [`discover_stdio_server`] spawns and
//! handshakes, [`register_catalog`] admits the tools, [`publishable_pairs`] is what may be published, and
//! [`McpToolExecutor`] calls them. What did not exist is the step that runs them **in order**, and the order
//! is load-bearing rather than incidental:
//!
//! ```text
//! declaration -> resolve env -> spawn+handshake -> list -> normalize
//!             -> register (admit)  -> publish (admitted only) -> executor -> route by kind
//! ```
//!
//! # Why the environment is resolved *here* and nowhere earlier
//!
//! A declaration holds [`crate::config::secret::SecretReference`]s, and the secret rules require material to
//! be resolved "at the last responsible moment". For a process launch that moment is immediately before the
//! spawn: resolving earlier would hold credentials in memory across an arbitrary interval, and resolving later
//! is impossible because the child needs them in its environment.
//!
//! The resolution is also the only step that can fail **before** a process exists, which is why it runs
//! first: a missing secret should be reported as a configuration fault rather than as a server that failed to
//! start.
//!
//! # Why one failing server does not abort the others
//!
//! A profile may declare several servers, and the failure modes are per-server — one child missing, one
//! version incompatible, one catalog entirely refused. Aborting the whole composition on the first failure
//! would mean a typo in one declaration takes down every other server the operator configured, so each
//! server's outcome is reported independently and the daemon starts with the servers that worked.
//!
//! That is deliberately **not** applied to the secret resolution above: a reference that cannot resolve is a
//! configuration fault the operator must fix, and reporting it per-server is still what happens — it is one
//! server's failure like any other. The distinction is that a *profile-level* fault (a duplicate name, an
//! unusable identity) is refused before any process is spawned, because those are caught by the declaration's
//! own validation.
//!
//! # The executor is built from the **admitted** identities
//!
//! [`register_catalog`] returns a report, and [`publishable_pairs`] filters the catalog down to what the
//! registry actually admitted. The executor is built from that filtered set, so a tool this adapter refused
//! is not callable — the same distinction `registration.rs` makes one layer down, carried through to the
//! thing that would dispatch it.

use std::collections::BTreeSet;
use std::sync::Arc;

use jarvis_domain::tool::identity::{SourceKind, ToolIdentity};
use jarvis_domain::tool::registry::{ServerConfigId, ToolRegistry};

use super::discovery::{DiscoveredServer, McpStartupRefusal, discover_stdio_server};
use super::executor::McpToolExecutor;
use super::process::McpLaunchSpec;
use super::registration::{RegistrationReport, publishable_pairs, register_catalog};
use crate::config::ConfigError;
use crate::config::mcp::McpServerDeclaration;
use crate::config::secret::SecretResolver;

/// Why one declared server did not become callable.
///
/// Per-server, because a profile's servers fail independently and an operator needs to know which one. Each
/// variant names a different correction: resolve the secret, fix the launch, quarantine the server, or read
/// its catalog refusals.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpCompositionRefusal {
    /// An environment reference could not be resolved.
    #[error("mcp server `{server}` has an unresolvable environment reference")]
    SecretUnavailable {
        /// The declared server name.
        server: String,
    },
    /// The launch specification itself was refused.
    #[error("mcp server `{server}` has an unusable launch specification: {code}")]
    LaunchInvalid {
        /// The declared server name.
        server: String,
        /// The launcher's stable code.
        code: &'static str,
    },
    /// The server could not be spawned, handshaken, or listed.
    #[error("mcp server `{server}` could not be started: {code}")]
    StartupRefused {
        /// The declared server name.
        server: String,
        /// The adapter's stable code for the refusal.
        code: &'static str,
        /// Whether a retry could ever succeed.
        permanent: bool,
    },
    /// The server's catalog was admitted but nothing could be published from it.
    #[error("mcp server `{server}` offered no publishable tool: {code}")]
    NothingPublishable {
        /// The declared server name.
        server: String,
        /// The refusal's stable code.
        code: &'static str,
    },
}

impl McpCompositionRefusal {
    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SecretUnavailable { .. } => "jarvis.secret_unavailable",
            // **Three variants carry a code rather than owning one.** Each was classified by the layer that
            // understood it — the launcher, the startup classifier, the publish join — and restating those codes
            // here would be a second spelling of each. Naming them together says that in one place instead of
            // three.
            Self::LaunchInvalid { code, .. }
            | Self::StartupRefused { code, .. }
            | Self::NothingPublishable { code, .. } => code,
        }
    }

    /// Returns whether a retry could ever succeed.
    ///
    /// **Delegated to the refusal that understood the failure** rather than re-derived: the startup refusal
    /// already answered this for a version mismatch, a launch fault, and a catalog that was entirely refused,
    /// and a second answer here would be a second place to be wrong.
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        match self {
            // A name or an environment reference is read from configuration again on every attempt, so it
            // fails identically until an operator edits the profile.
            Self::SecretUnavailable { .. }
            | Self::LaunchInvalid { .. }
            | Self::NothingPublishable { .. } => true,
            Self::StartupRefused { permanent, .. } => *permanent,
        }
    }
}

/// A launched and admitted MCP server: its session, its executor, and what registration did.
pub struct ComposedMcpServer {
    /// The declared name, which is the trusted identity its tools carry.
    server: ServerConfigId,
    /// The live session, kept so the executor's own `Arc` is not the only reference and a caller can shut it
    /// down deliberately.
    ///
    /// **Held rather than dropped**, because dropping the last `Arc<RunningService>` kills the child: the
    /// session must outlive the composition for the executor to be usable.
    session: Arc<rmcp::service::RunningService<rmcp::RoleClient, super::client::JarvisClient>>,
    /// The executor for this server's tools.
    executor: Arc<McpToolExecutor>,
    /// What registration did, including the refusals an operator should see.
    ///
    /// Carried rather than discarded: `added`/`unchanged`/`refused` are the difference between "the server
    /// offers forty tools" and "thirty-nine were registered and one had a changed schema", and a caller
    /// holding only the executor cannot tell them apart.
    report: RegistrationReport,
    /// The identities that are now callable, which is what a router is built from.
    admitted: Vec<ToolIdentity>,
}

impl std::fmt::Debug for ComposedMcpServer {
    /// Reports the name, the callable count, and the registration counts — never the session or the tools.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComposedMcpServer")
            .field("server", &self.server)
            .field("admitted", &self.admitted.len())
            .field("added", &self.report.added)
            .field("unchanged", &self.report.unchanged)
            .field("refused", &self.report.refused.len())
            .finish_non_exhaustive()
    }
}

impl ComposedMcpServer {
    /// Returns the trusted configuration identity.
    #[must_use]
    pub fn server(&self) -> &ServerConfigId {
        &self.server
    }

    /// Returns the executor for this server's tools.
    #[must_use]
    pub fn executor(&self) -> &Arc<McpToolExecutor> {
        &self.executor
    }

    /// Returns what registration did.
    #[must_use]
    pub fn report(&self) -> &RegistrationReport {
        &self.report
    }

    /// Returns the identities that are now callable.
    #[must_use]
    pub fn admitted(&self) -> &[ToolIdentity] {
        &self.admitted
    }

    /// Returns the live session.
    #[must_use]
    pub fn session(
        &self,
    ) -> &Arc<rmcp::service::RunningService<rmcp::RoleClient, super::client::JarvisClient>> {
        &self.session
    }
}

/// What composing a profile's declared servers produced.
///
/// Both halves are returned rather than only the successes, because "the operator declared three servers" and
/// "two are running and one has an unresolvable token" are different operator facts and a caller holding only
/// the running set cannot tell them apart — the same reasoning [`super::NormalizedCatalog`] applies to dropped
/// tools.
#[derive(Debug, Default)]
pub struct McpComposition {
    /// The servers that launched and admitted at least one tool.
    composed: Vec<ComposedMcpServer>,
    /// The servers that did not, with the reason, in declaration order.
    refused: Vec<(String, McpCompositionRefusal)>,
}

impl McpComposition {
    /// Returns the composed servers.
    #[must_use]
    pub fn servers(&self) -> &[ComposedMcpServer] {
        &self.composed
    }

    /// Returns the refused servers with their reasons.
    #[must_use]
    pub fn refused(&self) -> &[(String, McpCompositionRefusal)] {
        &self.refused
    }

    /// Returns whether every declared server composed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.refused.is_empty()
    }

    /// Returns every identity across every composed server.
    ///
    /// The set a router is built from. Collected here rather than by the caller so the "admitted only" rule
    /// has one implementation: each server contributes exactly what `publishable_pairs` produced for it.
    #[must_use]
    pub fn admitted_identities(&self) -> Vec<ToolIdentity> {
        self.composed
            .iter()
            .flat_map(|server| server.admitted.iter().cloned())
            .collect()
    }

    /// Returns the distinct source kinds the composed servers produced.
    ///
    /// Every MCP server's tools share the `McpServer` kind, so this exists to make a caller's assumption
    /// checkable rather than to be interesting: a router registering these needs **one** executor per kind, and
    /// a test can assert that the kind is what it expects.
    #[must_use]
    pub fn source_kinds(&self) -> BTreeSet<SourceKind> {
        self.composed
            .iter()
            .flat_map(|server| server.admitted.iter())
            .map(|identity| identity.source.kind)
            .collect()
    }
}

/// Turns one declaration into a launch specification, resolving its environment.
///
/// # Errors
///
/// Returns [`McpCompositionRefusal::SecretUnavailable`] when a reference cannot be resolved, and
/// [`McpCompositionRefusal::LaunchInvalid`] when the specification the launcher would receive is refused —
/// **including the launcher's own `validate`**, so a declaration that passes this module's checks and fails
/// the launcher's is caught here rather than at the spawn with a message about the child.
pub fn launch_spec_for(
    declaration: &McpServerDeclaration,
    secrets: &dyn SecretResolver,
) -> Result<McpLaunchSpec, McpCompositionRefusal> {
    let server = declaration.name.clone();
    // Resolved **first**, so a missing credential is reported as a configuration fault rather than as a
    // server that failed to start. The environment is built in the map's own (sorted) order, which makes the
    // child's environment deterministic across two identical starts.
    let mut env = Vec::with_capacity(declaration.env.len());
    for (key, reference) in &declaration.env {
        let value =
            secrets
                .resolve(reference)
                .map_err(|_| McpCompositionRefusal::SecretUnavailable {
                    server: server.clone(),
                })?;
        env.push((key.clone(), value));
    }
    let spec = McpLaunchSpec {
        program: declaration.program.clone(),
        args: declaration.args.clone(),
        env,
        // **`None`: the child inherits no working directory from this adapter.** `McpLaunchSpec` documents
        // `None` as "the daemon's", which is why it is worth stating that the configuration does not set one:
        // a server that needs a directory should receive it as an argument, and inventing one here would put
        // an unreviewed path in the process table.
        working_dir: None,
        startup_timeout_ms: declaration.startup_timeout_ms,
    };
    spec.validate()
        .map_err(|error| McpCompositionRefusal::LaunchInvalid {
            server,
            code: error.code(),
        })?;
    Ok(spec)
}

/// Composes one launched server: registers its catalog and builds an executor over the admitted tools.
///
/// Takes the [`DiscoveredServer`] rather than the declaration, because discovery is the async half and this
/// is the pure half — so the ordering rule is enforced by the types rather than by a comment.
///
/// # Errors
///
/// Returns [`McpCompositionRefusal::NothingPublishable`] when the registry admitted nothing this adapter can
/// publish: a catalog whose every tool was refused, or two identities colliding on one capability. Both make
/// the server uncallable, and both are reported rather than producing a server with an empty executor.
pub fn compose_discovered(
    registry: &mut ToolRegistry,
    discovered: DiscoveredServer,
) -> Result<ComposedMcpServer, McpCompositionRefusal> {
    let name = discovered.server().as_str().to_owned();
    let server = discovered.server().clone();
    let catalog = discovered.catalog().clone();
    let report = register_catalog(registry, &server, &catalog);
    // **The publish set is the admitted set**, filtered by the registry rather than taken from the catalog —
    // see `registration::publishable_pairs` for what publishing the offered set would serve.
    let pairs = publishable_pairs(registry, &server, &catalog).map_err(|refusal| {
        McpCompositionRefusal::NothingPublishable {
            server: name.clone(),
            code: refusal.code(),
        }
    })?;
    if pairs.is_empty() {
        // A server that launched, handshook, and listed tools, none of which survived registration. Reported
        // as nothing-publishable rather than composed with an empty executor, because the latter is a server
        // an operator believes is running and which can call nothing.
        return Err(McpCompositionRefusal::NothingPublishable {
            server: name,
            // `catalog_empty` rather than a registration code: the registry may have *accepted* nothing
            // because the catalog's tools were all refused upstream, which `register_catalog`'s report
            // records per tool. This code says the composition produced no callable tool.
            code: "mcp.catalog_empty",
        });
    }
    let admitted: Vec<ToolIdentity> = pairs
        .iter()
        .map(|(definition, _)| definition.identity.clone())
        .collect();
    let (session, _, _diagnostics) = discovered.into_session();
    // The timeout comes from the tool's own declared execution default rather than from a constant here, so
    // a per-tool ceiling an operator reviewed is the one enforced. `pairs` carries the definitions, and the
    // first tool's default stands for the server — acceptable because the adapter normalizes every MCP tool
    // with the same default (`DEFAULT_MCP_TIMEOUT_MS`).
    let timeout_ms = pairs
        .first()
        .map_or(super::DEFAULT_MCP_TIMEOUT_MS, |(definition, _)| {
            definition.execution.timeout_ms
        });
    let executor = Arc::new(McpToolExecutor::new(
        Arc::clone(&session),
        admitted.clone(),
        timeout_ms,
    ));
    Ok(ComposedMcpServer {
        server,
        session,
        executor,
        report,
        admitted,
    })
}

/// Composes every enabled declaration in a profile.
///
/// `registry` is the daemon's live registry, so registrations accumulate across servers and a second server
/// claiming another's source is refused by the registry's own first-wins claim — which is what makes
/// impersonation impossible between two servers **in the same profile**, not merely between a profile and a
/// hostile catalog.
///
/// # Errors
///
/// Returns [`ConfigError`] when the declarations themselves are unusable, which is the one failure that
/// aborts: a profile-level fault (a duplicate name, an unusable identity) is refused before anything is
/// spawned, because spawning processes to discover that would report a configuration error as a runtime one.
/// Every *per-server* failure is reported in the returned value instead.
pub async fn compose_declared_servers(
    registry: &mut ToolRegistry,
    declarations: &[McpServerDeclaration],
    secrets: &dyn SecretResolver,
) -> Result<McpComposition, ConfigError> {
    let mut composition = McpComposition::default();
    for declaration in declarations {
        // A disabled declaration is skipped **here rather than filtered by the caller**, so the rule lives
        // with the composition that would otherwise launch it. The declaration has already been validated
        // (`declarations()` validates disabled entries too), so a disabled server is a reviewed record that
        // is simply not started.
        if !declaration.enabled {
            continue;
        }
        let spec = match launch_spec_for(declaration, secrets) {
            Ok(spec) => spec,
            Err(refusal) => {
                composition
                    .refused
                    .push((declaration.name.clone(), refusal));
                continue;
            }
        };
        let discovered = match discover_stdio_server(&spec, &declaration.name).await {
            Ok(discovered) => discovered,
            Err(refusal) => {
                composition.refused.push((
                    declaration.name.clone(),
                    McpCompositionRefusal::StartupRefused {
                        server: declaration.name.clone(),
                        code: refusal.code(),
                        permanent: refusal.is_permanent(),
                    },
                ));
                continue;
            }
        };
        match compose_discovered(registry, discovered) {
            Ok(server) => composition.composed.push(server),
            Err(refusal) => composition
                .refused
                .push((declaration.name.clone(), refusal)),
        }
    }
    Ok(composition)
}

/// Returns the refusal a `McpStartupRefusal` maps to, for a caller that wants the code without the enum.
///
/// Exposed so a composition test can assert the mapping without constructing a live server, and so the
/// mapping has one definition rather than being written out at each call site.
#[must_use]
pub fn startup_code(refusal: &McpStartupRefusal) -> &'static str {
    refusal.code()
}

/// One executor over **every** composed server, dispatching by identity.
///
/// # Why this exists, and why it is not the router's job
///
/// Every MCP server's tools carry `SourceKind::McpServer`, so `RoutingExecutor` registers exactly **one**
/// executor for that kind — and it can only hold one, by construction, because two executors for one kind
/// would leave the choice to insertion order. But a profile may declare several servers, each with its own
/// session and its own `McpToolExecutor`.
///
/// So the fan-out has to happen *inside* the MCP kind, and it is keyed by **identity** rather than by name: a
/// tool's identity already includes its source owner (`registration` compares that owner against the trusted
/// configuration identity), so an identity that matches one server's admitted set cannot match another's. The
/// dispatch is therefore a membership test rather than a string comparison, and there is no ambiguity for a
/// lookup order to resolve.
///
/// # An identity no server admitted is refused, not guessed at
///
/// `NotFound`, matching `RoutingExecutor`'s own answer for an unrouted kind: the alternative is picking a
/// server by position, which would send one server's tool name to another's process.
pub struct ComposedMcpExecutor {
    /// Each server's own executor, with the identities it admitted, in declaration order.
    ///
    /// A `Vec` rather than a map keyed by identity: the identities are what get matched, and holding them
    /// beside the executor that owns them keeps "which session serves this tool" a single lookup rather than
    /// two structures that could disagree.
    servers: Vec<(Arc<McpToolExecutor>, Vec<ToolIdentity>)>,
}

impl std::fmt::Debug for ComposedMcpExecutor {
    /// Reports the server count and the total callable tools, never a session.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let callable: usize = self
            .servers
            .iter()
            .map(|(_, identities)| identities.len())
            .sum();
        formatter
            .debug_struct("ComposedMcpExecutor")
            .field("servers", &self.servers.len())
            .field("callable", &callable)
            .finish_non_exhaustive()
    }
}

impl ComposedMcpExecutor {
    /// Builds a dispatcher over the servers a composition produced.
    ///
    /// Consumes the composition, because the executors are what it held and a caller that kept both would have
    /// two owners of one session — the session is `Arc`-shared, so this is about ownership clarity rather than
    /// safety, but the intent is the same.
    #[must_use]
    pub fn over(composition: &McpComposition) -> Self {
        Self {
            servers: composition
                .servers()
                .iter()
                .map(|server| (Arc::clone(server.executor()), server.admitted().to_vec()))
                .collect(),
        }
    }

    /// Returns how many servers are dispatched to.
    #[must_use]
    pub fn servers(&self) -> usize {
        self.servers.len()
    }

    /// Returns whether no server is dispatched to.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Returns the executor that serves `identity`, if one admitted it.
    #[must_use]
    pub fn executor_for(&self, identity: &ToolIdentity) -> Option<&Arc<McpToolExecutor>> {
        self.servers
            .iter()
            .find(|(_, identities)| identities.contains(identity))
            .map(|(executor, _)| executor)
    }
}

impl jarvis_application::tool_call::ToolExecutor for ComposedMcpExecutor {
    fn execute<'a>(
        &'a self,
        request: jarvis_application::tool_call::ToolExecutionRequest<'a>,
        cancel: &'a jarvis_application::cancellation::CancellationScope,
    ) -> jarvis_application::tool_call::ToolExecutionFuture<'a> {
        let Some(executor) = self.executor_for(request.identity) else {
            return Box::pin(async move {
                Err(jarvis_application::tool_call::ToolExecutionError::Failed(
                    jarvis_domain::tool::error_class::ToolErrorClass::NotFound,
                ))
            });
        };
        let executor = Arc::clone(executor);
        Box::pin(async move {
            jarvis_application::tool_call::ToolExecutor::execute(executor.as_ref(), request, cancel)
                .await
        })
    }
}

#[cfg(test)]
#[path = "composition_tests.rs"]
mod tests;
