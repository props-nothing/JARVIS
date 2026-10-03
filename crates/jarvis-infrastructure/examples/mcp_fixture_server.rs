//! A minimal MCP server over stdio, used as a **real child process** by the discovery tests.
//!
//! It exists because discovery's load-bearing behavior is the spawn: the child must be a separate process
//! reached over a pipe, and an in-process fixture cannot falsify an implementation that drops the transport,
//! negotiates the wrong lifecycle, or never passes its configured environment through `env_clear`.
//!
//! # Why an example rather than a `[[bin]]`
//!
//! The `server` half of `rmcp` is a **dev-dependency** of this crate, so a `[[bin]]` target — which is built
//! with normal dependencies — could not link it. Examples are built with dev-dependencies, so this is the one
//! target kind that can both exist and speak the server protocol.
//!
//! # Why it matters that the harness is off
//!
//! Protocol framing is newline-delimited JSON **on stdout**. libtest writes its own progress banner to
//! stdout, which would interleave with the framing and make the child look like a server emitting garbage.
//! Cargo's `harness` **defaults to `false` for examples** (unlike a `[[test]]` target), so this program's
//! stdout already carries only protocol frames and no manifest flag is needed. Stated here because the
//! property is load-bearing and invisible: turning this example into a test target would break framing
//! without changing a line of it.
//!
//! # Which fixture it is, is chosen by the caller
//!
//! The parent passes [`FIXTURE_ENV`] in the launch spec's environment. That is deliberate: `build_command`
//! calls `env_clear()`, so nothing reaches this child that the parent did not list — which means a test that
//! sets this variable is also testing that the configured environment is actually delivered.

#![allow(clippy::panic)]

use std::borrow::Cow;

use rmcp::model::{
    Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, serve_server};

/// The environment variable naming which fixture to be.
pub const FIXTURE_ENV: &str = "JARVIS_MCP_FIXTURE";

/// The variable's value for the ordinary fixture: offers one usable tool, speaks the target version.
pub const FIXTURE_STANDARD: &str = "standard";

/// The variable's value for a peer that offers only one usable tool and an **older** protocol version.
///
/// This is the fixture that falsifies the claim that discovery refuses a legacy peer. It does not: discovery
/// negotiates downward within the versions both sides list, so this peer is a legitimate session at
/// `2025-06-18`.
pub const FIXTURE_LEGACY: &str = "legacy";

/// The variable's value for a peer whose only tool cannot be normalized.
///
/// The tool name contains a space, which is not a usable canonical name segment — so the normalizer refuses
/// it and the discovery must report an all-refused catalog rather than an empty one.
pub const FIXTURE_ALL_REFUSED: &str = "all-refused";

/// The tool name the standard and legacy fixtures offer.
pub const FIXTURE_TOOL_NAME: &str = "read_file";

/// The tool name the all-refused fixture offers.
pub const FIXTURE_UNUSABLE_TOOL_NAME: &str = "Read File";

/// The version the legacy fixture reports, and the only one it reports.
pub const FIXTURE_LEGACY_VERSION: ProtocolVersion = ProtocolVersion::V_2025_06_18;

/// An MCP server that behaves as the selected fixture.
#[derive(Debug, Clone)]
struct FixtureServer {
    /// The versions this fixture reports during discovery.
    supported: Vec<ProtocolVersion>,
    /// The tool name this fixture offers.
    tool: &'static str,
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("jarvis-mcp-fixture", "1.0.0"))
    }

    /// Overridden because the default reports only the SDK's newest revision, and the legacy fixture exists
    /// precisely to report something else.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(self.supported.clone())
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + '_ {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "additionalProperties": false,
        });
        // `Tool::new` wants an `Arc<JsonObject>`, so the schema is converted from the `json!` value rather
        // than built as a map literal — the same shape the unit tests use.
        let map = match schema.as_object() {
            Some(map) => map.clone(),
            None => return std::future::ready(Ok(ListToolsResult::with_all_items(Vec::new()))),
        };
        let tool = Tool::new(self.tool, "Read a file.", map);
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![tool])))
    }
}

/// Selects the fixture named by the environment, or `None` when the variable is absent.
///
/// `None` is **not** an error: it is how this program behaves when Cargo runs it as an example build, where
/// exiting immediately is the correct outcome rather than serving a protocol to nobody.
fn selected_fixture() -> Option<FixtureServer> {
    let name = std::env::var(FIXTURE_ENV).ok()?;
    let both = vec![ProtocolVersion::V_2026_07_28, FIXTURE_LEGACY_VERSION];
    let server = match name.as_str() {
        FIXTURE_STANDARD => FixtureServer {
            supported: both,
            tool: FIXTURE_TOOL_NAME,
        },
        FIXTURE_LEGACY => FixtureServer {
            supported: vec![FIXTURE_LEGACY_VERSION],
            tool: FIXTURE_TOOL_NAME,
        },
        FIXTURE_ALL_REFUSED => FixtureServer {
            supported: both,
            tool: FIXTURE_UNUSABLE_TOOL_NAME,
        },
        _ => return None,
    };
    Some(server)
}

fn main() {
    let Some(server) = selected_fixture() else {
        // Nothing was asked of this program, so there is nothing to do. Exiting zero keeps this target
        // harmless when Cargo compiles or runs it as an example rather than as a fixture.
        return;
    };

    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        // A runtime that cannot be built is not something a fixture can report over a protocol that needs a
        // runtime to speak, so the exit code is the report.
        std::process::exit(2);
    };

    let exit = runtime.block_on(async move {
        // `stdio()` gives the child the two pipes the parent created; the protocol runs on stdout and the
        // example harness is off by default, so nothing else writes there.
        match serve_server(server, rmcp::transport::io::stdio()).await {
            Ok(service) => {
                // Held, not discarded: `let _ =` would drop the `RunningService`, and its `Drop` cancels the
                // connection — the failure mode that made three tests in `mcp_conversation.rs` fail with
                // `TransportClosed` after their first exchange.
                let completion = service.waiting().await;
                if completion.is_ok() { 0 } else { 3 }
            }
            Err(_) => 4,
        }
    });
    std::process::exit(exit);
}
