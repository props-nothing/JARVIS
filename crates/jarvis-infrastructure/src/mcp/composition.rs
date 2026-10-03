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

use std::sync::Arc;
use std::time::Duration;

use jarvis_domain::tool::identity::ToolIdentity;
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
    /// The executor for this server's tools.
    executor: Arc<McpToolExecutor>,
    /// What registration did, including the refusals an operator should see.
    ///
    /// Carried rather than discarded: `added`/`unchanged`/`refused` are the difference between "the server
    /// offers forty tools" and "thirty-nine were registered and one had a changed schema", and a caller
    /// holding only the executor cannot tell them apart.
    report: RegistrationReport,
    /// Tools the **normalizer** refused before registration ever saw them.
    ///
    /// Separate from `report.refused` because the two happen at different stages and only this one can be
    /// missing while the server still composes: a normalization refusal means the offered entry was never a
    /// candidate, so `register_catalog` had nothing to refuse and `is_clean()` stayed true. Without this field
    /// a malformed tool on an otherwise healthy server left no trace at all.
    normalized_refusals: Vec<super::McpToolRefusal>,
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
            .field("normalized_refusals", &self.normalized_refusals.len())
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

    /// Returns what registration did, including the per-tool refusals.
    ///
    /// **The read that makes a partial refusal visible, and until it had a caller the report was carried for
    /// nothing.** `is_clean()` is true for any server that admitted at least one tool, so a catalog of forty
    /// where one was refused produced a server that looked healthy while offering thirty-nine — the operator
    /// fact this report exists to distinguish. `daemon::compose_mcp_servers` now reads it at startup: each
    /// refusal is logged at `warn` with its capability and stable code, and the totals at `debug`. A refusal
    /// that is *not* a whole-server one would otherwise leave no trace at all.
    #[must_use]
    pub fn report(&self) -> &RegistrationReport {
        &self.report
    }

    /// Returns the identities that are now callable.
    #[must_use]
    pub fn admitted(&self) -> &[ToolIdentity] {
        &self.admitted
    }

    /// Returns every offered tool that never became callable, from **both** stages.
    ///
    /// Registration's refusals first (they name a canonical capability), then the normalizer's (they name the
    /// server's own spelling, which is what an operator has to find in the server's listing). Both are
    /// returned rather than one, because a caller asking "what did this server offer that I cannot call" is
    /// asking about the whole set — returning only registration's would answer with the tools the server could
    /// have served and not the ones it named badly enough to be unusable.
    #[must_use]
    pub fn refusals(&self) -> Vec<super::McpToolRefusal> {
        let mut refusals: Vec<super::McpToolRefusal> = self
            .report
            .refused
            .iter()
            .map(|refused| super::McpToolRefusal {
                name: refused.capability.clone(),
                code: refused.refusal.code(),
            })
            .collect();
        refusals.extend(self.normalized_refusals.iter().cloned());
        refusals
    }

    /// Returns the live session.
    ///
    /// Read from the executor, which is the single owner: a supervisor may replace the session after the
    /// child dies, so a copy held here would go stale and be the one that gets read.
    #[must_use]
    pub fn session(
        &self,
    ) -> Arc<rmcp::service::RunningService<rmcp::RoleClient, super::client::JarvisClient>> {
        self.executor.session()
    }
}

/// One server's lifecycle row, as reported by [`McpComposition::health`].
///
/// A plain data record rather than a formatted string: a drain logs it and an operator probe prints it, and a
/// caller that wants a summary should not have to parse one back out of prose.
#[derive(Debug, Clone)]
pub struct McpServerHealth {
    /// The declared name.
    pub name: String,
    /// How many identities registration admitted for this server.
    pub tools: usize,
    /// The protocol version the session negotiated, or `None` when the session recorded none.
    pub version: Option<String>,
    /// Whether the session reports itself closed.
    pub closed: bool,
    /// How many offered tools were refused for this server.
    pub refused: usize,
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

    /// Stops every composed server's session, **inside the runtime**.
    ///
    /// Consumes the composition, because a session that has been cancelled must not be dispatched to and an
    /// `Arc` left behind would keep the child alive. The sessions are cancelled rather than merely dropped, and
    /// the difference is what a drain needs: `rmcp` kills the child from `Drop`, which cannot report a failure
    /// and cannot be waited for, while `cancel()` stops the service's worker first — so the child is asked to
    /// terminate through the protocol before the transport reaps it.
    ///
    /// Returns the servers whose session would not stop cleanly, so a caller can log them. A failure to stop is
    /// **not** an error a caller can act on during a drain: the process is exiting, and the alternative to
    /// reporting it is discarding it.
    pub async fn shutdown(self) -> Vec<String> {
        let mut unclean = Vec::new();
        for server in self.composed {
            let name = server.server.as_str().to_owned();
            // **The cancellation token, not `cancel`.** `RunningService::cancel(self)` consumes the service and
            // `close` needs `&mut`, and neither is reachable here: the session is shared with the dispatcher's
            // executor, so the `Arc` has two holders and can be neither moved out of nor borrowed mutably.
            // `cancellation_token()` hands back a token that *owns a clone of the signal*, which is the one
            // non-consuming way to stop the worker — and stopping the worker closes the transport, which is what
            // kills the child.
            let session = server.executor.session();
            let token = session.cancellation_token();
            // The executor goes first, so a dispatch cannot be admitted after the cancel is signalled.
            drop(server.executor);
            token.cancel();
            // **A bounded wait for the worker, so the caller learns whether the child actually stopped.** The
            // token does not await, and a drain that assumed it had would exit while the child might still be
            // running. `is_closed` is the service's own answer to "has the worker finished", polled to a bound
            // rather than waited on indefinitely: a server that will not stop must not hold the drain open past
            // its grace window, and the fact is reported instead.
            if !wait_until_closed(&session).await {
                unclean.push(name);
            }
        }
        unclean
    }
    /// Returns whether every declared server composed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.refused.is_empty()
    }

    /// Reports each server's name, tool count, negotiated version, and whether its session is closed.
    ///
    /// **The probe the note's operational section asks for, and it reports what the session says rather than
    /// what the composition assumed.** A composed server is a child process, so the questions an operator has
    /// are "is it still there", "which protocol version did it agree to", and "how many tools did it
    /// contribute" — and the first of those has to come from the session, because a child that died after
    /// composition leaves a handle that looks healthy until something asks it.
    ///
    /// Deliberately **not** named `health` with a boolean: a closed session is not a failure the daemon can
    /// repair — it is a fact reported beside the version and the count, and a caller deciding whether to
    /// quarantine acts on all three.
    ///
    /// **It reports; it does not act, and that limit is the honest part.** Calling this does not prune, stop,
    /// or quarantine anything, and it cannot see a child that dies a moment later. Acting on a closed
    /// row — restart or quarantine — is [`super::supervisor::McpSupervisor`]'s job, which reads the
    /// single-server form ([`McpToolExecutor::is_closed`]); a drain also reads this to report which servers
    /// were already gone.
    #[must_use]
    pub fn health(&self) -> Vec<McpServerHealth> {
        self.composed
            .iter()
            .map(|server| McpServerHealth {
                name: server.server.as_str().to_owned(),
                tools: server.admitted.len(),
                version: server
                    .session()
                    .peer_info()
                    .map(|info| info.protocol_version.to_string()),
                closed: server.executor.is_closed(),
                refused: server.refusals().len(),
            })
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
        // **The operator's directory when one was declared, and the daemon's when not.** `McpLaunchSpec::validate`
        // checks the value (a usable UTF-8 token), so an unusable one is refused before a process exists, and
        // `build_command` applies it. `None` still means "inherit the daemon's working directory" — a real
        // exposure, now one an operator can close by declaring `working_directory`.
        working_dir: declaration.working_dir.clone(),
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
    // **The normalizer's refusals are captured before the catalog is consumed, because nothing else carries
    // them.** `register_catalog` only ever sees tools that survived normalization, so a malformed entry is
    // invisible to the registration report — and the server still composes, which is what made the omission
    // silent. Taken here so the refusals travel with the server they belong to.
    let normalized_refusals: Vec<super::McpToolRefusal> = catalog
        .rejected
        .iter()
        .map(super::refusal_for_rejected)
        .collect();
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
    let executor = Arc::new(McpToolExecutor::new(session, admitted.clone(), timeout_ms));
    Ok(ComposedMcpServer {
        server,
        executor,
        report,
        normalized_refusals,
        admitted,
    })
}

/// How long to wait before re-spawning a server whose first attempt failed transiently.
///
/// **A real delay, not a yield**, and short because it is bounded work on the startup path: the daemon is
/// still coming up, and a server that needs longer than this to become startable is one an operator should
/// hear about rather than wait on. The purpose is not to outlast a long outage — that is
/// [`super::supervisor`], which restarts a server that dies after a successful start — but to stop a *transient* failure (a child that lost a race with its own
/// socket, a machine briefly out of file descriptors) from costing the whole session.
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// How many times a **transient** startup failure is attempted before the server is reported refused.
///
/// One retry rather than a loop: `is_permanent()` already filters the failures a retry cannot fix, so what
/// remains here is the genuinely transient kind, and a second attempt is the cheap way to absorb it. A
/// repeated failure is treated as evidence that the fault is not transient after all — which is the same
/// conclusion `is_permanent` would have reached had the classifier known the difference.
const TRANSIENT_STARTUP_ATTEMPTS: u32 = 2;

/// The sleep a retry waits before a re-spawn, in a form a deterministic test can replace.
///
/// **Injectable, and that is required rather than tidy.** The test that exercises the retry must not spend
/// [`RETRY_BACKOFF`] of real time, and more importantly must not *depend* on a real delay for its meaning: a
/// mutation that removed the retry would still pass a test that only asserted elapsed time loosely. An enum
/// with one production variant and one test variant makes "did it retry" an observable fact — the recording
/// sleep counts, so the assertion is on the count rather than on a clock.
#[derive(Debug, Clone, Copy, Default)]
enum Backoff {
    /// The real delay, used by every production path.
    #[default]
    Real,
    /// A no-op that records how many times it was called — test-only, and never constructed in production.
    ///
    /// The counter lives behind a leaked `Box` because the value is held by a `&` inside the retry loop and
    /// must outlive it; a test that wants the count reaches it through [`Backoff::counting`]'s returned
    /// handle rather than through the enum, so the shared state cannot be read by a path that has no test.
    #[cfg(test)]
    Recording(&'static std::sync::atomic::AtomicU32),
}

impl Backoff {
    /// Returns a recording backoff and the counter it writes to.
    ///
    /// The counter is leaked deliberately: it must outlive the composition's local scope, and a test that
    /// leaked one `AtomicU32` is not a leak that matters.
    #[cfg(test)]
    fn counting() -> (Self, &'static std::sync::atomic::AtomicU32) {
        let counter: &'static std::sync::atomic::AtomicU32 =
            Box::leak(Box::new(std::sync::atomic::AtomicU32::new(0)));
        (Self::Recording(counter), counter)
    }

    /// Waits the backoff, or records the intent to.
    async fn wait(self) {
        match self {
            Self::Real => tokio::time::sleep(RETRY_BACKOFF).await,
            #[cfg(test)]
            Self::Recording(counter) => {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// Attempts one declaration, retrying **only** a transient failure.
///
/// Returns the discovered server, or the refusal to report. `attempts` is how many spawns are permitted, and
/// is a parameter rather than the constant so the retry is a property this module can test with one attempt
/// (no retry) as its control.
async fn discover_with_retry(
    declaration: &McpServerDeclaration,
    spec: &McpLaunchSpec,
    backoff: Backoff,
    attempts: u32,
) -> Result<DiscoveredServer, McpStartupRefusal> {
    let mut attempt = 1;
    loop {
        match discover_stdio_server(spec, &declaration.name).await {
            Ok(discovered) => return Ok(discovered),
            Err(refusal) => {
                // **The classifier's own answer decides, and it is asked in one place.** A permanent failure
                // means a retry cannot help — the peer implements the version set it implements, the catalog
                // it sent is the one that was refused — so the server is quarantined by *reporting* it rather
                // than by a state machine that does not exist yet. A transient one is retried, bounded.
                if refusal.is_permanent() || attempt >= attempts {
                    return Err(refusal);
                }
                log::warn!(
                    "mcp server startup failed transiently, retrying: server={} attempt={attempt} code={}",
                    declaration.name,
                    refusal.code()
                );
                attempt += 1;
                backoff.wait().await;
            }
        }
    }
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
    compose_declared_servers_with(
        registry,
        declarations,
        secrets,
        Backoff::Real,
        TRANSIENT_STARTUP_ATTEMPTS,
    )
    .await
}

/// Composes the declared servers with an explicit backoff and attempt count.
///
/// Exists so a test can exercise the **retry decision** without spending the real delay, and so
/// `attempts = 1` gives that test its control: one attempt means no retry, which is what makes "did the
/// retry happen" a difference the test observes rather than something both paths happen to do.
///
/// # Errors
///
/// As [`compose_declared_servers`].
async fn compose_declared_servers_with(
    registry: &mut ToolRegistry,
    declarations: &[McpServerDeclaration],
    secrets: &dyn SecretResolver,
    backoff: Backoff,
    attempts: u32,
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
        let discovered = match discover_with_retry(declaration, &spec, backoff, attempts).await {
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
        Self::over_servers(composition.servers())
    }

    /// Builds a dispatcher over an explicit slice of composed servers.
    ///
    /// Exposed beside [`Self::over`] because the **daemon holds its composition and needs the dispatcher at
    /// the same time**: the sessions must outlive the router that dispatches to them, so consuming the
    /// composition would drop the very sessions the dispatcher is holding. Both entry points build the same
    /// value; this one does not require the composition to be moved.
    #[must_use]
    pub fn over_servers(servers: &[ComposedMcpServer]) -> Self {
        Self {
            servers: servers
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

/// How long a drain waits for one server's worker to finish after it is cancelled.
///
/// Short, and deliberately shorter than the daemon's own grace window: a server that will not stop must not
/// hold the drain open, and the caller reports it rather than waiting. One second is enough for a local child
/// whose transport has been closed, and a server that needs longer is a fact worth surfacing.
const SHUTDOWN_POLL_BOUND: Duration = Duration::from_millis(1_000);

/// How often the worker's state is checked while waiting.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Waits, to a bound, for a cancelled session's worker to finish.
///
/// Returns `true` when the session reports itself closed and `false` when the bound elapsed. Polling rather
/// than awaiting because the one non-consuming shutdown the SDK offers is a cancel signal, which does not
/// produce a future to await — see [`McpComposition::shutdown`] for why the consuming form is unreachable.
async fn wait_until_closed(
    session: &rmcp::service::RunningService<rmcp::RoleClient, super::client::JarvisClient>,
) -> bool {
    let bound = tokio::time::Instant::now() + SHUTDOWN_POLL_BOUND;
    while tokio::time::Instant::now() < bound {
        if session.is_closed() {
            return true;
        }
        tokio::time::sleep(SHUTDOWN_POLL_INTERVAL).await;
    }
    // A last check, because the worker may have finished during the final sleep — reporting a server unclean
    // because the poll interval landed unluckily would be a false alarm an operator would chase.
    session.is_closed()
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
