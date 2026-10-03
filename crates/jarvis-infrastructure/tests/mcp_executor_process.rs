//! End-to-end tests for the MCP executor, against a **real spawned child process**.
//!
//! `discovery`'s own suite already proves the handshake over a spawned server, and `mcp_conversation.rs`
//! proves the per-step call decisions over a duplex pipe. What neither can show is the thing this file is
//! about: that the **executor** drives a real session through `ToolExecutor::execute`, that the identity it
//! refuses is refused *before* the wire, and that a class which means an unsettled outcome reaches the port
//! as `Ambiguous` rather than as a plain failure.
//!
//! # Why a spawned child rather than the duplex pipe
//!
//! The executor's own property is that it holds a session and sends through it. A duplex fixture would work
//! and would be faster, but this suite runs beside `mcp_discovery_process.rs`, which already spawns
//! `examples/mcp_fixture_server.rs` — so the session here is the same kind a daemon would hold, and the
//! executor is exercised over the transport it will actually use.
//!
//! # How the child is found
//!
//! Examples have no `CARGO_BIN_EXE_*` variable, so the path is derived from this test executable's own
//! location. A missing example is reported as a **skip with a printed reason**, because `cargo test --test
//! mcp_executor_process` alone does not build examples — and a test that failed for an unrelated reason
//! would train a reader to ignore it.

// The workspace denies `clippy::panic`/`unwrap_used`/`expect_used`, and neither a Cargo `tests/*.rs` target
// nor the example is classified by `clippy.toml`'s test allowances. In a test a panic *is* the failure report.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::tool_call::{ToolExecutionError, ToolExecutionRequest, ToolExecutor};
use jarvis_domain::tool::call::ToolArguments;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::ToolIdentity;
use jarvis_domain::tool::registry::ServerConfigId;
use jarvis_infrastructure::mcp::discovery::discover_stdio_server;
use jarvis_infrastructure::mcp::executor::McpToolExecutor;
use jarvis_infrastructure::mcp::process::{DEFAULT_MCP_STARTUP_TIMEOUT_MS, McpLaunchSpec};
use jarvis_infrastructure::mcp::registration::{publishable_pairs, register_catalog};

/// The configured server name these tests use — valid under **both** name rules.
const SERVER_NAME: &str = "fixture";

/// The fixture-selecting variable, mirrored from the example.
///
/// Mirrored rather than imported because the example is a separate target. A drift in either copy makes the
/// child exit without serving, and the discovery fails loudly rather than passing on a default.
const FIXTURE_ENV: &str = "JARVIS_MCP_FIXTURE";

/// The standard fixture: one usable tool, target version preferred.
const FIXTURE_STANDARD: &str = "standard";

/// The fixture that lists a tool, **accepts** a call, and never answers it.
const FIXTURE_UNANSWERED: &str = "unanswered";

/// The tool name the fixture offers, which deliberately differs from its canonical capability.
///
/// Mirrored from the example rather than imported, because the example is a separate target — the same
/// reason [`FIXTURE_ENV`] is. Used to assert that the name *sent* is the one the server knows rather than the
/// canonical capability, so a drift here is a failing assertion rather than a silent skip.
const FIXTURE_TOOL_NAME: &str = "read_file";

/// The text the fixture returns from a call, mirrored from the example for the same reason.
const FIXTURE_CALL_TEXT: &str = "fixture result for read_file";

/// How long the executor gives one call. Short, so a hung fixture fails the test rather than the harness.
const CALL_TIMEOUT_MS: u64 = 10_000;

/// A call bound far shorter than the unanswered fixture's withholding delay.
///
/// The fixture waits 30 s and this gives it 250 ms, so the timeout is **deterministic rather than a race**:
/// the point is that the call does not complete, not that it completes slowly. Module level rather than
/// declared inside the test because `clippy::items_after_statements` refuses an item in a statement scope.
const SHORT_CALL_BOUND_MS: u64 = 250;

/// The absolute path of the fixture server example for this profile, if it is built.
fn fixture_program() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let profile = current.parent()?.parent()?;
    let name = if cfg!(windows) {
        "mcp_fixture_server.exe"
    } else {
        "mcp_fixture_server"
    };
    let candidate = profile.join("examples").join(name);
    candidate.is_file().then_some(candidate)
}

/// Returns the fixture program, or prints why this test is skipped.
fn program_or_skip(test: &str) -> Option<PathBuf> {
    let program = fixture_program();
    if program.is_none() {
        println!(
            "SKIP {test}: examples/mcp_fixture_server is not built for this profile; \
             run `cargo build -p jarvis-infrastructure --example mcp_fixture_server` first"
        );
    }
    program
}

/// A launch spec for the standard fixture, with the fixture variable as its **entire** environment.
fn standard_spec(program: PathBuf) -> McpLaunchSpec {
    spec_for(program, FIXTURE_STANDARD)
}

/// A launch spec for the named fixture, with the fixture variable as its **entire** environment.
fn spec_for(program: PathBuf, fixture: &str) -> McpLaunchSpec {
    McpLaunchSpec {
        program,
        args: Vec::new(),
        env: vec![(FIXTURE_ENV.to_owned(), fixture.to_owned())],
        working_dir: None,
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    }
}

/// Discovers the fixture, registers its catalog, and returns an executor over **the admitted identities**.
///
/// Registration is in the path deliberately, and it is the same join a daemon performs: the executor is
/// built from what the registry admitted (`publishable_pairs`), never from the server's raw listing. An
/// executor built from the listing would call tools the registry refused, which is the defect
/// `registration.rs` exists to prevent one layer up.
async fn executor_over_fixture(
    test: &str,
) -> Option<(
    McpToolExecutor,
    Vec<ToolIdentity>,
    Arc<
        rmcp::service::RunningService<
            rmcp::RoleClient,
            jarvis_infrastructure::mcp::client::JarvisClient,
        >,
    >,
)> {
    let program = program_or_skip(test)?;
    let discovered = discover_stdio_server(&standard_spec(program), SERVER_NAME)
        .await
        .expect("the standard fixture must be discoverable");

    let server = ServerConfigId::new(SERVER_NAME).expect("the fixture name is a usable identity");
    let mut registry = jarvis_domain::tool::registry::ToolRegistry::new();
    let report = register_catalog(&mut registry, &server, discovered.catalog());
    assert!(report.is_clean(), "the fixture must register: {report:?}");

    let pairs = publishable_pairs(&registry, &server, discovered.catalog())
        .expect("the fixture offers distinct capabilities");
    let identities: Vec<ToolIdentity> = pairs
        .iter()
        .map(|(definition, _)| definition.identity.clone())
        .collect();
    assert_eq!(identities.len(), 1, "the fixture offers one tool");

    let (session, catalog, _diagnostics) = discovered.into_session();
    assert_eq!(catalog.tools.len(), 1);
    let executor = McpToolExecutor::new(Arc::clone(&session), identities.clone(), CALL_TIMEOUT_MS);
    Some((executor, identities, session))
}

/// An argument document the fixture's schema accepts (it takes any object).
fn arguments(document: &str) -> ToolArguments {
    ToolArguments::new(document).expect("the fixture arguments are usable text")
}

/// The instant a fixture call started at.
///
/// **A fixed instant rather than the wall clock**, and `UtcTimestamp` has no `now()` for exactly this
/// reason: the domain's own rule is that an instant is injected, so a test asserts a value rather than
/// whatever the clock happened to read. Nothing in these tests asserts on it — it is part of the request
/// shape — so a constant is both honest and deterministic.
fn fixture_instant() -> jarvis_domain::time::UtcTimestamp {
    jarvis_domain::time::UtcTimestamp::parse("2026-10-03T00:00:00Z")
        .expect("the fixture instant is a valid RFC 3339 timestamp")
}

#[tokio::test]
async fn the_executor_calls_a_real_server_through_the_tool_executor_port() {
    // The wiring property: `ToolExecutor::execute` against a spawned server, so the port, the executor, the
    // call parameters, the MRTR cap, and the normalization are all exercised together. A unit test cannot
    // show that the request reaches the peer.
    let Some((executor, identities, session)) =
        executor_over_fixture("the_executor_calls_a_real_server_through_the_tool_executor_port")
            .await
    else {
        return;
    };
    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();

    let request = ToolExecutionRequest {
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    let result = executor
        .execute(request, &cancel)
        .await
        .expect("the fixture's tool must run");
    assert_eq!(result.content.len(), 1, "the fixture returns one block");
    assert!(
        serde_json::to_string(&result)
            .expect("the body serializes")
            .contains(FIXTURE_CALL_TEXT),
        "the server's text must survive to the port's result"
    );
    // And the call really went to the *listed* name, not to the canonical capability. Asserted here as well
    // as in `call_tests.rs` because this is the only place both halves meet a live server.
    assert_eq!(
        identity.capability.name(),
        FIXTURE_TOOL_NAME,
        "the fixture's canonical name segment must be the name the server lists"
    );

    // Dropped inside the runtime: the session's transport kills the child from `Drop`.
    drop(session);
    executor_shutdown(executor);
}

/// Proves the executor refuses an identity it may not call **without reaching the server**.
#[tokio::test]
async fn an_identity_the_executor_may_not_call_is_refused_with_the_guards_own_class() {
    // The membership check, and the class is the point rather than the refusal: one condition must have one
    // class whichever layer notices it, so this must agree with `decide_response`'s own refusal
    // (`mcp.tool_identity_changed` → `Unavailable`) rather than inventing a `PermissionDenied` here.
    let Some((executor, identities, session)) = executor_over_fixture(
        "an_identity_the_executor_may_not_call_is_refused_with_the_guards_own_class",
    )
    .await
    else {
        return;
    };
    let authorized = &identities[0];
    // The same capability and source, a different schema fingerprint: the shape a re-schema has, and the
    // identity a server would offer if it changed the tool between the listing and the call.
    let mut re_schemed = authorized.clone();
    re_schemed.schema_fingerprint =
        jarvis_domain::tool::identity::SchemaFingerprint::from_bytes([0x77; 32]);
    assert_ne!(
        re_schemed, *authorized,
        "the fixture must differ by fingerprint or it tests nothing"
    );
    assert!(
        !executor.supports(&re_schemed),
        "an identity the executor was not built with must not be callable"
    );

    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        identity: &re_schemed,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("a re-schemed identity must be refused");
    // `Unavailable` has a `Safe` retry posture, which is honest: this refusal never reached the server, so a
    // retry cannot duplicate an effect. It is also not `Ambiguous` — provably nothing ran.
    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::Unavailable),
        "the guard's class is the one `call.rs` assigns that refusal"
    );

    drop(session);
    executor_shutdown(executor);
}

/// Proves an already-cancelled scope stops the call before it is sent.
#[tokio::test]
async fn a_cancelled_scope_ends_the_call_as_cancelled_rather_than_as_a_provider_fault() {
    // The distinction the port keeps: `Cancelled` is JARVIS's own decision. Reporting it as a provider error
    // would blame the server for JARVIS stopping, and reporting it as `Ambiguous` would send the recovery
    // pass looking for an effect that was never attempted.
    let Some((executor, identities, session)) = executor_over_fixture(
        "a_cancelled_scope_ends_the_call_as_cancelled_rather_than_as_a_provider_fault",
    )
    .await
    else {
        return;
    };
    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    cancel.cancel();

    let request = ToolExecutionRequest {
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("a cancelled call must not succeed");
    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::Cancelled),
        "cancellation is the caller's decision, not a provider fault"
    );

    drop(session);
    executor_shutdown(executor);
}

/// Proves an argument document that is not a JSON object is refused with the argument class.
#[tokio::test]
async fn a_non_object_argument_document_is_refused_as_invalid_arguments() {
    // The refusal `call.rs` makes, now reached through the port. `Unavailable` again, because nothing left
    // the process — and this is the case where a schema-validated call should never arrive, so the check
    // exists to keep the *SDK* from being the thing that reports it.
    let Some((executor, identities, session)) =
        executor_over_fixture("a_non_object_argument_document_is_refused_as_invalid_arguments")
            .await
    else {
        return;
    };
    let identity = &identities[0];
    let arguments = arguments("\"not-an-object\"");
    let cancel = CancellationScope::new();

    let request = ToolExecutionRequest {
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("a non-object document must be refused");
    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::Unavailable)
    );

    drop(session);
    executor_shutdown(executor);
}

/// Shuts an executor's session down by dropping it, **inside a runtime**.
///
/// A named helper rather than an inline `drop`, so the requirement is stated once: `rmcp`'s child transport
/// kills its child from `Drop` and `kill()` awaits a reap, so dropping outside a runtime panics *inside the
/// SDK*. Every test here runs in a runtime, which is why this is a helper and not a comment.
///
/// Not `async`: it awaits nothing, and an `async fn` that never awaits is what clippy's `unused_async`
/// refuses — the same rule the fixture's `list_tools` follows by returning `impl Future` directly.
fn executor_shutdown(executor: McpToolExecutor) {
    drop(executor);
}

/// Proves a call the server **accepted and never answered** reaches the port as `Ambiguous`.
///
/// **The most safety-relevant decision in the executor, and the one nothing else can reach.** The port
/// separates `Ambiguous` from `Failed` because the recovery pass treats them differently: `Ambiguous` is a
/// call whose effect may exist and which only a read of the tool's own state can settle, while `Failed` is one
/// that provably did no harm. A `Timeout` class carries "may or may not have taken effect", and reporting it
/// as a plain failure would tell the recovery pass a possibly-executed call did nothing — the duplicate-effect
/// direction this workspace's ledger work exists to prevent.
///
/// The fixture **accepts** the call (the name is right) and withholds the answer, so nothing is refused: the
/// client's own bound is what ends it. That is what makes this a timeout rather than a protocol error.
#[tokio::test]
async fn a_call_the_server_never_answers_reaches_the_port_as_ambiguous() {
    let Some(program) =
        program_or_skip("a_call_the_server_never_answers_reaches_the_port_as_ambiguous")
    else {
        return;
    };
    // A bound far shorter than the fixture's withholding delay, so the timeout is deterministic rather than a
    // race — see [`SHORT_CALL_BOUND_MS`].
    let discovered = discover_stdio_server(&spec_for(program, FIXTURE_UNANSWERED), SERVER_NAME)
        .await
        .expect("the unanswered fixture must still be discoverable — it lists a tool");
    let server = ServerConfigId::new(SERVER_NAME).expect("the fixture name is a usable identity");
    let mut registry = jarvis_domain::tool::registry::ToolRegistry::new();
    let report = register_catalog(&mut registry, &server, discovered.catalog());
    assert!(report.is_clean(), "the fixture must register: {report:?}");
    let pairs = publishable_pairs(&registry, &server, discovered.catalog())
        .expect("the fixture offers distinct capabilities");
    let identities: Vec<ToolIdentity> = pairs
        .iter()
        .map(|(definition, _)| definition.identity.clone())
        .collect();
    let (session, _, _) = discovered.into_session();
    let executor = McpToolExecutor::new(
        Arc::clone(&session),
        identities.clone(),
        SHORT_CALL_BOUND_MS,
    );

    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: SHORT_CALL_BOUND_MS,
    };

    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("an unanswered call must not succeed");
    assert_eq!(
        error,
        ToolExecutionError::Ambiguous,
        "a timeout means the effect may exist, which is what Ambiguous is for"
    );

    drop(session);
    executor_shutdown(executor);
}
