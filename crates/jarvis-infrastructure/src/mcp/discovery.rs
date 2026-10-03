//! Discovering a local MCP server's tools: spawn, handshake, list, normalize.
//!
//! This is the caller the earlier slices left open. `process.rs` spawns and isolates the child,
//! `client.rs` decides which versions to negotiate, `mod.rs` normalizes the listing, and `registration.rs`
//! admits or refuses it — every piece existed, and nothing joined them.
//!
//! # The identity is configuration, and it is a parameter
//!
//! [`discover_stdio_server`] takes the server's **configuration name** as an argument rather than deriving it
//! from the launch spec. A launch spec holds a program, arguments, and an environment, and none of those is a
//! name: deriving one from the program path would produce something like `usr/bin/mcp-server`, which is not a
//! usable owner, and the natural repair — slugging the path — is exactly the silent transformation that makes
//! two servers' tools share an owner. The configured name is the trusted identity
//! ([`ServerConfigId`]); the name the *server* reports is untrusted, arrives later in the catalog, and is what
//! `registration.rs` compares this one against.
//!
//! # The handshake is discovered, and a legacy peer is recorded rather than refused
//!
//! [`rmcp::ClientLifecycleMode::Discover`] sends `server/discover` and never `initialize`, so a `2026-07-28`
//! peer sees the stateless lifecycle. It is **not** the `Auto` mode: `Auto`'s fallback is precisely an
//! `initialize` handshake, so using it would mean this adapter sometimes speaks a lifecycle it was not
//! reviewed against.
//!
//! Discovery does **not** constrain the versions it will accept to post-`2026-07-28` ones. `preferred_versions`
//! is the candidate list, and the SDK picks from its intersection with the peer's set — a peer reporting
//! `2025-06-18` is a **legitimate discovery** that negotiates downward. Worth stating plainly because the
//! opposite is easy to assume: the negotiated version is therefore **recorded** on the handle
//! ([`DiscoveredServer::protocol_version`]) so a caller can see which lifecycle it got, rather than resting on
//! a floor this code does not enforce.
//!
//! # The listing may be a cache, and the caller should know
//!
//! `Peer::list_tools` consults the SDK's per-connection response cache before the wire, honouring the
//! `ttlMs`/`cacheScope` the server supplied. A discovery therefore reflects the server's current listing *or*
//! its cached one within that TTL — not necessarily a fresh round trip. Recorded rather than worked around:
//! the cache is per-connection and this connection is new, so the first `tools/list` after a spawn reaches the
//! server. Only a *re*-listing on a long-lived connection could be served from it.
//!
//! # A discovery leaves a running server, and what happens to it is the caller's decision
//!
//! [`DiscoveredServer`] owns the client, and the client owns the transport — which **kills the child when it
//! is dropped**. The child is therefore not a leak to be tidied later; it is alive precisely because this
//! handle is. A caller that wanted the tools only calls [`DiscoveredServer::shutdown`]; one that wants to call
//! tools keeps the handle. Both are legitimate and neither is chosen here.
//!
//! # Failures carry the child's stderr, bounded
//!
//! A server that exits during the handshake explains itself on stderr, and a discovery that reported only
//! "the transport closed" would send an operator to the wrong place. The diagnostics handle is read on the
//! failure path and attached to the refusal, so the reason travels with the error. It is **not** attached on
//! success, because a healthy run does not need a chatty server's banner.

use std::time::Duration;

use jarvis_domain::tool::classification::Idempotency;
use jarvis_domain::tool::registry::ServerConfigId;

use super::client::classify_startup_error;
use super::outcome::classify_service_error;
use super::process::{McpDiagnostics, McpLaunchSpec, spawn_stdio_server};
use super::{McpToolRejection, NormalizedCatalog, normalize_catalog};

/// How long a discovery may take from spawn to a listed catalog.
///
/// A bound on the whole exchange rather than on the spawn alone, because a server that starts and then never
/// answers `tools/list` would otherwise hold a daemon's startup open with no clock — the same reason
/// `process.rs` bounds the startup timeout. One minute matches the longest accepted startup timeout, so the
/// two cannot disagree about how long a server is expected to take.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(60_000);

/// Why a discovery did not produce a catalog.
///
/// Each variant names a different operator conversation: a launch defect, a name that is not a usable
/// identity, a startup that failed, a listing that failed or took too long, or a listing that could not be
/// normalized at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpStartupRefusal {
    /// The configured name is not usable as a configuration identity or as a tool source owner.
    ///
    /// Checked before anything is spawned, because a name this adapter cannot use is a configuration defect
    /// and starting a process to discover that would report it as a runtime failure instead.
    #[error("mcp server configuration identity is not usable")]
    ServerNameInvalid,
    /// The child could not be launched.
    ///
    /// The [`super::process::McpProcessError`] **code** is carried rather than the enum, because this refusal
    /// reaches operator output and the code is the stable form: `mcp.program_invalid` names which
    /// configuration line to look at, while "launch failed" does not.
    #[error("mcp server could not be launched: {code}")]
    Launch {
        /// The process refusal's stable code.
        code: &'static str,
    },
    /// The peer does not share a protocol version with JARVIS.
    ///
    /// **Permanent** rather than transient: a peer whose version set excludes ours answers the same way
    /// forever, so this is a quarantine decision rather than a retry. `client.rs` draws that distinction; this
    /// variant carries its answer rather than restating the rule.
    #[error("mcp server shares no supported protocol version with JARVIS")]
    NoCompatibleVersion,
    /// The peer's startup failed for a reason that is not a version mismatch.
    #[error("mcp server startup failed: {code}")]
    StartupFailed {
        /// The stable code, from the startup classification.
        code: &'static str,
        /// Whether a retry could ever succeed, from the same classification.
        permanent: bool,
        /// The child's bounded stderr tail, when it wrote anything.
        diagnostics: String,
    },
    /// The listing failed after a successful handshake.
    #[error("mcp server could not list its tools: {code}")]
    ListingFailed {
        /// The stable code, from the service-error classification.
        code: &'static str,
        /// Whether a retry could ever succeed, derived from the class's retry posture.
        permanent: bool,
        /// The child's bounded stderr tail, when it wrote anything.
        diagnostics: String,
    },
    /// The discovery did not finish inside [`DISCOVERY_TIMEOUT`].
    #[error("mcp discovery did not finish within {}ms", DISCOVERY_TIMEOUT.as_millis())]
    TimedOut {
        /// The child's bounded stderr tail, when it wrote anything.
        diagnostics: String,
    },
    /// Every tool the server offered was refused by the normalizer.
    ///
    /// **Refused rather than reported as an empty catalog**, and the distinction is the point: a server whose
    /// every tool this adapter cannot represent is a configuration problem an operator must see, while a
    /// server offering no tools at all is a legitimate empty listing. Both would otherwise arrive as a catalog
    /// with nothing in it, and a caller could not tell them apart afterwards.
    ///
    /// Only the **first** refusal is carried. It is the one that names the field at fault, and a server that
    /// offered forty unusable tools does not need forty copies of the same reason in an error value.
    #[error("every tool the mcp server offered was refused, starting with: {first_rejection}")]
    NoUsableTools {
        /// The refusal of the first offered tool, so the reason is actionable without a second lookup.
        first_rejection: McpToolRejection,
    },
}

impl McpStartupRefusal {
    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ServerNameInvalid => "mcp.server_config_invalid",
            // **Three variants carry a code rather than owning one.** Each was classified by the layer that
            // understood it — the process builder, the startup classifier, the service-error classifier — and
            // restating those codes here would be a second spelling of each. Naming them together says that in
            // one place instead of three.
            Self::Launch { code }
            | Self::StartupFailed { code, .. }
            | Self::ListingFailed { code, .. } => code,
            Self::NoCompatibleVersion => "mcp.startup_no_compatible_version",
            Self::TimedOut { .. } => "mcp.discovery_timeout",
            Self::NoUsableTools { .. } => "mcp.no_usable_tools",
        }
    }

    /// Returns whether a retry could ever succeed.
    ///
    /// **The quarantine question, answered in one place so a caller does not answer it differently.** A
    /// version mismatch and an all-refused catalog are permanent, because neither changes by retrying: the
    /// peer's version set is fixed, and the definitions that were refused are the definitions it sent.
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        match self {
            // Three permanent reasons that do not depend on anything transient: the peer implements the
            // version set it implements, the definitions it sent are the ones this adapter refused, and a
            // configured name is read from configuration again on every attempt.
            Self::NoCompatibleVersion | Self::NoUsableTools { .. } | Self::ServerNameInvalid => {
                true
            }
            // **Delegated, not decided.** The startup and service-error classifiers already answered whether
            // the underlying failure is permanent; re-deriving it here would be a second answer to one
            // question, and the one that reached a log line could be the one nobody acted on.
            Self::StartupFailed { permanent, .. } | Self::ListingFailed { permanent, .. } => {
                *permanent
            }
            // The child may start next time: the program may be installed, or the load may pass.
            Self::Launch { .. } | Self::TimedOut { .. } => false,
        }
    }
}

/// Converts a service error into a listing refusal.
///
/// **The posture is read, not invented.** `retryable_for` is asked with `Idempotency::None` — the most
/// conservative input — so a class retryable only for a naturally idempotent call reports as non-retryable
/// here. Listing has no side effect at all, which makes the question nearly academic, but asking it the strict
/// way means a class added later that is retryable in some other sense cannot accidentally make a discovery
/// loop.
fn listing_refusal(error: &rmcp::ServiceError, diagnostics: String) -> McpStartupRefusal {
    let class = classify_service_error(error);
    McpStartupRefusal::ListingFailed {
        code: class.as_contract_str(),
        permanent: !class.retryable_for(Idempotency::None),
        diagnostics,
    }
}

/// A discovered server: its tools, and the live connection they came from.
///
/// **Holding this keeps the child alive.** The client owns the transport and the transport kills the child on
/// drop, so a caller that wanted only the catalog calls [`Self::shutdown`] and one that wants to call tools
/// keeps the handle.
pub struct DiscoveredServer {
    /// The client, for `tools/call` and any later re-listing. Also the owner of the child.
    client: rmcp::service::RunningService<rmcp::RoleClient, DiscoveryClient>,
    /// The server's trusted configuration identity.
    server: ServerConfigId,
    /// The normalized catalog: what the server offered, and what was refused.
    catalog: NormalizedCatalog,
    /// The child's bounded stderr tail, still readable while the server runs.
    ///
    /// The **reason this handle exists**: the transport is moved into the client, and the tail lives in a
    /// field of the process that is consumed on the way. Without a shared handle, a session that fails after
    /// the handshake would have no way to ask the child why.
    diagnostics: McpDiagnostics,
}

impl std::fmt::Debug for DiscoveredServer {
    /// Reports counts, the identity, and the negotiated version — never the tools or the child's output.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiscoveredServer")
            .field("server", &self.server)
            .field("tools", &self.catalog.tools.len())
            .field("rejected", &self.catalog.rejected.len())
            .field("protocol_version", &self.protocol_version())
            .finish_non_exhaustive()
    }
}

impl DiscoveredServer {
    /// Returns the trusted configuration identity.
    #[must_use]
    pub fn server(&self) -> &ServerConfigId {
        &self.server
    }

    /// Returns the normalized catalog.
    #[must_use]
    pub fn catalog(&self) -> &NormalizedCatalog {
        &self.catalog
    }

    /// Returns the client, for calling tools.
    #[must_use]
    pub fn client(&self) -> &rmcp::service::RunningService<rmcp::RoleClient, DiscoveryClient> {
        &self.client
    }

    /// Returns the child's bounded stderr tail.
    #[must_use]
    pub fn diagnostics(&self) -> &McpDiagnostics {
        &self.diagnostics
    }

    /// Returns the version this session negotiated, as the session recorded it.
    ///
    /// Exposed because **the adapter's guarantees depend on it**: `2026-07-28` is the lifecycle this code was
    /// reviewed against, and a session that negotiated downward — which discovery *permits*, see the module
    /// doc — is one whose behaviour a caller may want to treat differently. `None` only when the peer's
    /// discovery result carried no usable version, which the handshake treats as a failure.
    ///
    /// **Owned rather than borrowed.** `peer_info` hands back a fresh `Arc` of the recorded info, so a
    /// reference into it would dangle the moment the guard is dropped. The version is small and cloned on
    /// demand, which also means a caller never holds a lock while formatting it.
    #[must_use]
    pub fn protocol_version(&self) -> Option<rmcp::model::ProtocolVersion> {
        self.client
            .peer_info()
            .map(|info| info.protocol_version.clone())
    }

    /// Ends the session and returns the catalog.
    ///
    /// Cancelling the client stops the worker, which drops the transport, which kills the child — so this is
    /// the deliberate stop rather than an incidental one. It must be awaited **inside a runtime**, which is
    /// also true of dropping the handle: `rmcp`'s child transport must not be dropped outside one.
    ///
    /// # Errors
    ///
    /// Returns the catalogue alongside the failure's code, so a caller that needs the catalog anyway is not
    /// forced to discard it because the session would not stop cleanly.
    pub async fn shutdown(self) -> (NormalizedCatalog, Option<&'static str>) {
        let Self {
            client,
            catalog,
            diagnostics: _,
            server: _,
        } = self;
        let outcome = match client.cancel().await {
            Ok(_) => None,
            Err(_) => Some("mcp.shutdown_failed"),
        };
        (catalog, outcome)
    }
}

/// A client handler with no server-initiated capabilities.
///
/// **Empty on purpose.** Every input request the SDK could route here is refused by `invocation.rs`, and a
/// client that declared capabilities it refuses to service would be advertising them. Declaring none is the
/// honest posture: the server learns from it that JARVIS will not run model calls, enumerate directories, or
/// prompt its user on request.
///
/// The default `get_info` is the SDK's stock client identity and capability set, which is what makes "declares
/// nothing" true rather than merely intended.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryClient;

impl rmcp::ClientHandler for DiscoveryClient {}

/// Spawns a configured MCP server, discovers what it offers, and normalizes the listing.
///
/// `name` is the **configured** server identity, not a name from the server. See the module doc for why it is
/// a parameter rather than something derived here.
///
/// Must be called from within a Tokio runtime: `rmcp`'s transports cannot be constructed outside one, and this
/// spawns the child and drives the handshake.
///
/// # Errors
///
/// Returns the [`McpStartupRefusal`] naming the reason. A launch failure, a version mismatch, a listing that
/// failed, and a server that offered nothing this adapter can represent are distinguishable, and each is a
/// different operator action.
pub async fn discover_stdio_server(
    spec: &McpLaunchSpec,
    name: &str,
) -> Result<DiscoveredServer, McpStartupRefusal> {
    // **Both name rules are checked, and they are two different rules.** A configuration identity accepts dots
    // and hyphens; a tool source owner accepts dots and hyphens but rejects a leading digit and an empty
    // dotted part. A name can therefore be a valid identity and an unusable owner, in which case the
    // normalizer would refuse every tool and the result would be an all-refused catalog that looks like a
    // server defect. Checking both here reports it as what it is.
    let server = ServerConfigId::new(name).map_err(|_| McpStartupRefusal::ServerNameInvalid)?;
    if super::server_source(name).is_err() {
        return Err(McpStartupRefusal::ServerNameInvalid);
    }

    let process = spawn_stdio_server(spec)
        .map_err(|error| McpStartupRefusal::Launch { code: error.code() })?;
    // The handle is taken **before** the transport is moved out, because the process struct is consumed by the
    // move and its diagnostics field would go with it.
    let diagnostics = process.diagnostics_handle();
    let transport = process.into_transport();

    let lifecycle = rmcp::ClientLifecycleMode::Discover {
        preferred_versions: super::client::preferred_protocol_versions(),
    };
    let started = tokio::time::timeout(
        DISCOVERY_TIMEOUT,
        rmcp::serve_client_with_lifecycle(DiscoveryClient, transport, lifecycle),
    )
    .await;

    let client = match started {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => {
            let failure = classify_startup_error(&error);
            // A peer sharing no version is reported as incompatible rather than as a generic startup failure,
            // because the operator action differs — one is "upgrade the server or JARVIS", the other is "read
            // the server's output". The code already encodes that difference, so this branches on it rather
            // than re-deriving the distinction.
            if failure.code == "mcp.startup_no_compatible_version" {
                return Err(McpStartupRefusal::NoCompatibleVersion);
            }
            return Err(McpStartupRefusal::StartupFailed {
                code: failure.code,
                permanent: failure.is_permanent(),
                diagnostics: diagnostics.text(),
            });
        }
        Err(_) => {
            return Err(McpStartupRefusal::TimedOut {
                diagnostics: diagnostics.text(),
            });
        }
    };

    let listed = tokio::time::timeout(DISCOVERY_TIMEOUT, client.peer().list_tools(None))
        .await
        .map_err(|_| McpStartupRefusal::TimedOut {
            diagnostics: diagnostics.text(),
        })?
        .map_err(|error| listing_refusal(&error, diagnostics.text()))?;

    let catalog = normalize_catalog(name, &listed.tools);
    // Offered nothing is a legitimate empty listing; offered something and having it all refused is a
    // configuration defect. The two must not arrive as the same empty catalog.
    if catalog.tools.is_empty()
        && let Some(first) = catalog.rejected.first()
    {
        return Err(McpStartupRefusal::NoUsableTools {
            first_rejection: first.rejection.clone(),
        });
    }

    Ok(DiscoveredServer {
        client,
        server,
        catalog,
        diagnostics,
    })
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
