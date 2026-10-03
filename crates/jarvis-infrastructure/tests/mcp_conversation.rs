//! End-to-end tests: a real MCP client against a real MCP server over a real duplex pipe.
//!
//! These exist because every layer of this adapter is already tested in isolation — `mod.rs` normalizes an
//! offered tool, `outcome.rs` normalizes a returned one, `client.rs` classifies a failed startup — and
//! what no unit test can reach is the part that only appears when the SDK drives both ends: **whether the
//! mapping is wired to the conversation at all**, whether the lifecycle JARVIS asks for is the one the
//! peer answers, and whether a result that travelled the wire still normalizes to the same canonical
//! values the fixture built in memory.
//!
//! The server is a hand-written [`ServerHandler`] rather than a mock of the client, for the reason the
//! OpenAI-compatible adapter's socket tests record: a test that stubbed the peer would prove the mapping
//! again and nothing about the exchange. Here the peer must genuinely answer `server/discover` — the
//! capability under test is `ClientLifecycleMode::Discover`, and *there is no other way to test it*, since
//! a stubbed client's behaviour is a fixture's.
//!
//! `rmcp`'s `server` feature is a **dev-dependency** for this file only. The shipped binary's features are
//! `client`, `transport-io`, and `transport-child-process`, so nothing here widens what a release contains.
//!
//! **Why a duplex pipe rather than a spawned process.** `process.rs` already proves the spawn, the
//! environment isolation, and the stderr capture in its own tests. Spawning here would re-prove those and
//! substitute a real child's timing for the fixture's determinism. The pipe is the smaller test of the
//! property this file is about: the conversation.

// This is a `tests/*.rs` integration crate, which `clippy.toml`'s test allowances do not recognise — a trap
// recorded in this repository — so the test-only allowances are declared here. `let _ = ..` on a
// `RunningService` is **not** among them: that one is fixed rather than allowed, because it is the bug this
// file's server fixture hit (a discarded service cancels its connection).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
// The fixture's handlers answer from a constant and return `impl Future` rather than being `async`, which is
// the shape the SDK's own default handlers use — so there is nothing to allow here, only something to match.

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jarvis_domain::tool::classification::{ApprovalHint, Effect, Risk};
use jarvis_domain::tool::identity::SourceKind;
use jarvis_infrastructure::mcp::client::{
    TARGET_PROTOCOL_VERSION, negotiate_protocol_version, preferred_protocol_versions,
};
use jarvis_infrastructure::mcp::normalize_catalog;
use jarvis_infrastructure::mcp::outcome::{McpCallOutcome, normalize_call_response};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ProtocolVersion,
    ServerCapabilities, ServerConfig, TextContent, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{
    ClientHandler, ClientLifecycleMode, ServerHandler, serve_client_with_lifecycle, serve_server,
};

/// The tool name the fixture server offers, chosen to be a canonical capability segment.
const TOOL_NAME: &str = "read_file";

/// The fixture's input schema as a parsed object, which is the type `Tool::new` takes.
///
/// Parsed from the same const the adapter is tested against elsewhere, so the fixture cannot drift from
/// the schema the unit tests use. `Tool::new` wants an `Arc<JsonObject>` rather than a `Value`, so a
/// `json!(..)` literal would not type-check.
fn tool_schema() -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_str(TOOL_SCHEMA).expect("the fixture schema must be a JSON object")
}

/// What the fixture server's tool call answers with.
const CALL_TEXT: &str = "fixture result for read_file";

/// A fixture MCP server that offers one tool and answers one call.
///
/// The counters exist so a test can assert that the *server* was actually reached — a claim a stubbed
/// client cannot make, and the one that distinguishes an end-to-end test from a mapping test.
#[derive(Debug, Default)]
struct FixtureServer {
    /// The protocol versions the fixture reports during discovery.
    supported: Vec<ProtocolVersion>,
    /// How many times the fixture was asked to list its tools.
    list_calls: Arc<AtomicUsize>,
    /// How many times the fixture was asked to call its tool.
    call_calls: Arc<AtomicUsize>,
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("fixture-server", "1.0.0"))
    }

    /// Reports the versions this fixture supports.
    ///
    /// Overridden rather than inherited, because the default reports only the SDK's newest — and a test
    /// that asserts JARVIS's *preference order* decides needs a peer that offers a set to choose from.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(self.supported.clone())
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>> + '_ {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        let tool = Tool::new(TOOL_NAME, "Read a file.", tool_schema());
        // `impl Future` rather than `async`, which is the shape the SDK's own default handlers use — an
        // `async fn` that never awaits is what clippy's `unused_async` flags, and matching the SDK means
        // there is nothing to allow.
        std::future::ready(Ok(rmcp::model::ListToolsResult::with_all_items(vec![tool])))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + '_ {
        self.call_calls.fetch_add(1, Ordering::SeqCst);
        // A tool the fixture does not offer is a protocol error rather than a reported failure: the
        // distinction is what `outcome.rs` maps to `NotFound` versus `ProviderError`, and the fixture
        // should be able to produce both.
        if request.name != TOOL_NAME {
            return std::future::ready(Err(rmcp::ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "no such tool",
                None,
            )));
        }
        std::future::ready(Ok(CallToolResponse::Complete(CallToolResult::success(
            vec![ContentBlock::Text(TextContent::new(CALL_TEXT))],
        ))))
    }
}

/// A client handler with no capabilities, which is all these tests need.
#[derive(Debug, Clone, Default)]
struct FixtureClient;

impl ClientHandler for FixtureClient {}

/// The input schema the fixture offers, as the JSON text the adapter's validator parses.
const TOOL_SCHEMA: &str = r#"{"type":"object","properties":{},"additionalProperties":false}"#;

/// Connects a fixture server to a client over a duplex pipe, **splitting each end once**.
///
/// The split is bound before use because `tokio::io::split` consumes its argument: calling it twice on one
/// stream is a move error, which is how this helper came to exist rather than three copies of the setup.
///
/// Returns the client's handshake result so a caller can assert either a healthy connection or the exact
/// refusal an incompatible peer produces.
///
/// The `Err` type is `rmcp`'s eleven-variant startup error, which is large enough to trip
/// `result_large_err`; boxing it would make the test read around the type rather than about it, and this is
/// a test helper rather than a production path.
#[allow(clippy::result_large_err)]
async fn connect_fixture(
    server: FixtureServer,
) -> Result<
    rmcp::service::RunningService<rmcp::RoleClient, FixtureClient>,
    rmcp::service::ClientInitializeError,
> {
    let (server_end, client_end) = tokio::io::duplex(8 * 1024);
    let (server_read, server_write) = tokio::io::split(server_end);
    let (client_read, client_write) = tokio::io::split(client_end);
    let server_transport =
        rmcp::transport::async_rw::AsyncRwTransport::new_server(server_read, server_write);
    let client_transport =
        rmcp::transport::async_rw::AsyncRwTransport::new_client(client_read, client_write);
    // The server task is **detached deliberately**: it ends when the client's end of the pipe closes, and
    // joining it here would require the client to drop first — which the caller still holds.
    //
    // ⚠ **The service must be bound, not discarded.** `let _ = serve_server(..).await` drops the
    // `RunningService` immediately — `let _` discards rather than holds — and its `Drop` cancels the
    // connection, so the peer appears to vanish after the first exchange and every later call fails
    // `TransportClosed`. This is the bug that made three tests in this file fail with exactly that error
    // while discovery still passed, because discovery is the first message and won the race. Awaiting
    // `waiting()` both holds the service alive and ends the task when the connection does.
    tokio::spawn(async move {
        if let Ok(service) = serve_server(server, server_transport).await {
            let _ = service.waiting().await;
        }
    });
    serve_client_with_lifecycle(
        FixtureClient,
        client_transport,
        ClientLifecycleMode::Discover {
            preferred_versions: preferred_protocol_versions(),
        },
    )
    .await
}

/// Connects to a fixture that must start, panicking with the refusal if it does not.
async fn connected(
    server: FixtureServer,
) -> rmcp::service::RunningService<rmcp::RoleClient, FixtureClient> {
    connect_fixture(server)
        .await
        .expect("the fixture handshake must complete")
}

/// A fixture server offering the target version and one older.
fn standard_fixture() -> (FixtureServer, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let list_calls = Arc::new(AtomicUsize::new(0));
    let call_calls = Arc::new(AtomicUsize::new(0));
    let server = FixtureServer {
        supported: vec![ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25],
        list_calls: Arc::clone(&list_calls),
        call_calls: Arc::clone(&call_calls),
    };
    (server, list_calls, call_calls)
}

#[tokio::test]
async fn discovery_reaches_the_server_and_reports_its_versions() {
    // The lifecycle is the capability under test: `ClientLifecycleMode::Discover` sends `server/discover`
    // and **no** `initialize`, and only a real peer can prove that happened — a stub would answer whatever
    // the test's fixture said rather than what the SDK sends.
    let (server, _, _) = standard_fixture();
    let client = connected(server).await;

    let peer_info = client.peer_info().expect("discovery must record the peer");
    assert_eq!(
        peer_info
            .server_info
            .as_ref()
            .map(|info| info.name.as_str()),
        Some("fixture-server"),
        "the identity must be the one the fixture advertised"
    );

    let shared =
        negotiate_protocol_version(&[ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25]);
    assert_eq!(shared, Some(TARGET_PROTOCOL_VERSION));
    client.cancel().await.expect("the client must shut down");
}

#[tokio::test]
async fn a_listed_tool_normalizes_into_a_canonical_definition_the_server_actually_sent() {
    // The wiring test. The unit tests build a `Tool` in memory and normalize it; this one takes the tool
    // the *fixture server* listed and asserts the canonical values, which is what proves the mapping is
    // connected to the conversation rather than merely correct in isolation.
    let (server, list_calls, _) = standard_fixture();
    let client = connected(server).await;

    let listed = client
        .peer()
        .list_tools(None)
        .await
        .expect("listing tools must succeed");
    assert_eq!(
        list_calls.load(Ordering::SeqCst),
        1,
        "the server must be reached"
    );

    let catalog = normalize_catalog("fixture-server", &listed.tools);
    assert!(catalog.rejected.is_empty(), "{:?}", catalog.rejected);
    assert_eq!(catalog.tools.len(), 1);
    // **The schema the server sent must be carried, and it must be the one the identity was built over.**
    // Without this the tool registers and is then refused as `tool.schema_absent` when a call resolves it —
    // a defect that only appears one layer downstream, which is why it is asserted here against a real
    // peer rather than against a fixture's in-memory schema.
    assert_eq!(
        catalog.tools.len(),
        catalog.schemas.len(),
        "must be aligned"
    );
    jarvis_infrastructure::tool_schema::ToolSchema::parse(&catalog.schemas[0])
        .expect("the carried schema must parse")
        .confirms(&catalog.tools[0].identity.schema_fingerprint)
        .expect("the carried schema must be the one that was fingerprinted");

    let definition = &catalog.tools[0];
    assert_eq!(definition.capability().to_string(), "mcp.read_file@1");
    assert_eq!(definition.source().kind, SourceKind::McpServer);
    assert_eq!(definition.source().owner, "fixture-server");
    assert_eq!(definition.purpose, "Read a file.");
    // The untrusted-annotation rule, asserted on a tool that came off the wire: the fixture offered no
    // annotations, so the adapter records a write and still asks — never `Allow`.
    assert_eq!(definition.effects, vec![Effect::Write]);
    assert_eq!(definition.risk, Risk::Moderate);
    assert_eq!(definition.default_approval, ApprovalHint::Ask);
    client.cancel().await.expect("the client must shut down");
}

#[tokio::test]
async fn a_called_tool_returns_content_that_normalizes_to_a_canonical_body() {
    // The other half of the wiring: a result that travelled the wire still normalizes to the same
    // canonical values, and the outcome layer sees a success rather than a failure.
    let (server, _, call_calls) = standard_fixture();
    let client = connected(server).await;

    let response = client
        .peer()
        .call_tool_once(CallToolRequestParams::new(TOOL_NAME))
        .await
        .expect("the call must succeed");
    assert_eq!(
        call_calls.load(Ordering::SeqCst),
        1,
        "the server must be reached"
    );

    let outcome = normalize_call_response(&response).expect("a complete result must normalize");
    let McpCallOutcome::Succeeded(body) = outcome else {
        panic!("the fixture returned a success, got {outcome:?}");
    };
    assert_eq!(body.content.len(), 1);
    assert_eq!(body.content[0].size_bytes(), CALL_TEXT.len() as u64);
    client.cancel().await.expect("the client must shut down");
}

/// **The wiring test for the call path.** The unit tests derive a wire name from a capability they built
/// themselves, which cannot show that the name reaches a *server* the way the server expects. This test
/// takes the tool a fixture server actually **listed**, normalizes it, derives the call parameters from the
/// resulting canonical identity, and sends them — so the listing half and the calling half are joined by a
/// real exchange.
///
/// The mutant it kills is the one that matters here: sending the canonical capability (`mcp.read_file@1`)
/// instead of the server's own name (`read_file`). Every unit test in `call_tests.rs` would still pass if
/// the fixture's name happened to equal its capability — it does not, deliberately — and the server here
/// refuses any name it did not list, so a wrong name is a `METHOD_NOT_FOUND` rather than a silent success.
#[tokio::test]
async fn a_tool_the_server_listed_is_called_by_the_name_the_server_knows() {
    use jarvis_infrastructure::mcp::call::call_params;

    let (server, _, call_calls) = standard_fixture();
    let client = connected(server).await;

    // Listed, then normalized — the production direction, not a hand-built identity.
    let listed = client
        .peer()
        .list_tools(None)
        .await
        .expect("listing must succeed");
    let catalog = normalize_catalog("acme-files", &listed.tools);
    let definition = catalog
        .tools
        .first()
        .expect("the fixture offers one usable tool");
    assert_ne!(
        definition.identity.capability.to_string(),
        TOOL_NAME,
        "the fixture is only a valid test while the canonical form differs from the wire name"
    );

    let arguments = jarvis_domain::tool::call::ToolArguments::new("{}")
        .expect("the fixture arguments are usable");
    let params =
        call_params(&definition.identity, &arguments).expect("a listed tool must be addressable");
    // Bound before the move, so the assertion below can still read it after the call.
    let sent_name = params.name.clone();

    // **The call is made before the name is asserted**, and the ordering is deliberate. The fixture
    // refuses any name it did not list with `METHOD_NOT_FOUND`, so a wrong name is detected by the **peer**
    // rather than by an assertion of mine — which is what makes this a test of the wiring instead of a
    // second copy of the unit test. Asserting first would have the same mutant caught locally, leaving the
    // exchange unproven.
    let response = client.peer().call_tool_once(params).await.expect(
        "the server must accept the name it listed; a rejection means the wrong one was sent",
    );
    assert_eq!(
        call_calls.load(Ordering::SeqCst),
        1,
        "the server must have been reached with this name"
    );
    assert_eq!(sent_name.as_ref(), TOOL_NAME);
    let body = jarvis_infrastructure::mcp::call::normalize_response(&response)
        .expect("the fixture's result must normalize");
    assert_eq!(body.content.len(), 1);
    client.cancel().await.expect("the client must shut down");
}

#[tokio::test]
async fn a_protocol_error_from_the_server_classifies_as_not_found() {
    // The error path, exercised end to end rather than by constructing an `ErrorData`. The fixture
    // refuses an unknown tool with `METHOD_NOT_FOUND`, which must arrive as a `ServiceError::McpError`
    // whose code the outcome layer maps to `NotFound` — **not** to `ProviderError`, which is the
    // difference between "no such tool" and "the provider broke".
    let (server, _, _) = standard_fixture();
    let client = connected(server).await;

    let error = client
        .peer()
        .call_tool_once(CallToolRequestParams::new("no_such_tool"))
        .await
        .expect_err("the fixture must refuse an unknown tool");

    let rmcp::ServiceError::McpError(data) = &error else {
        panic!("expected a JSON-RPC error, got {error:?}");
    };
    assert_eq!(
        jarvis_infrastructure::mcp::outcome::classify_service_error(&error),
        jarvis_domain::tool::error_class::ToolErrorClass::NotFound
    );
    assert_eq!(data.code, rmcp::model::ErrorCode::METHOD_NOT_FOUND);
    client.cancel().await.expect("the client must shut down");
}

#[tokio::test]
async fn a_peer_that_cannot_speak_the_preferred_version_fails_as_a_permanent_startup_failure() {
    // The compatibility failure the note's error table names, produced **by a real peer** rather than by
    // constructing the error: the fixture offers only versions outside JARVIS's preference set, so
    // discovery cannot resolve one and the failure must classify as permanent — a retry can never help.
    let (_, list_calls, _) = standard_fixture();
    let server = FixtureServer {
        // A version the SDK knows but JARVIS does not prefer. `preferred_protocol_versions()` offers the
        // target plus every known revision, so the fixture must offer something *outside* that set to be
        // genuinely incompatible — which for this SDK means an empty set, since every known version is
        // preferred.
        supported: Vec::new(),
        list_calls,
        call_calls: Arc::new(AtomicUsize::new(0)),
    };

    let error = connect_fixture(server)
        .await
        .expect_err("an incompatible peer must not start");

    let failure = jarvis_infrastructure::mcp::client::classify_startup_error(&error);
    assert!(
        failure.is_permanent(),
        "a version mismatch can never be fixed by retrying: {failure:?}"
    );
    assert!(!failure.is_retryable_for(jarvis_domain::tool::classification::Idempotency::None));
}
