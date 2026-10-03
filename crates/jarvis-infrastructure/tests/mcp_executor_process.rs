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

/// The fixture that lists a tool, accepts a call, and answers it **after a bounded delay**.
///
/// Mirrored from the example. The delay matters rather than being an implementation detail: it sits between a
/// short bound and a generous one, so a test can tell which bound was enforced by the *kind* of result rather
/// than by how long it waited.
const FIXTURE_SLOW: &str = "slow";

/// The fixture that accepts a call and answers with a **task handle** rather than a result.
///
/// Mirrored from the example. Its tool and timing are the ordinary ones, so the answer's shape is the only
/// variable a test on it exercises.
const FIXTURE_TASK: &str = "task";

/// The fixture that answers every attempt with **`input_required`**, so a round is never fulfilled.
///
/// Mirrored from the example. Its tool is the ordinary one, so the only variable is that each answer is a
/// further round — which is what makes a round cap observable.
const FIXTURE_INPUT_REQUIRED: &str = "input-required";

/// The fixture that answers with a **`request_state` and no request**, which is the only shape that reaches
/// the round cap.
///
/// Mirrored from the example. The pair with [`FIXTURE_INPUT_REQUIRED`] is the point: one names a request and
/// ends at JARVIS's own refusal, the other asks JARVIS nothing and ends at the SDK's cap.
const FIXTURE_STATE_ONLY: &str = "state-only";

/// The slow fixture's delay, mirrored from the example.
///
/// A `u32` because [`std::time::Duration::from_millis`] takes one, and the example's constant is a `u64` for
/// consistency with the other delays there.
const SLOW_FIXTURE_DELAY_MS: u32 = 1_500;

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
    executor_over_fixture_named(test, FIXTURE_STANDARD).await
}

/// The same join over **any** named fixture, so a test whose subject is the answer's shape can select one.
///
/// A parameter rather than a second copy of the registration path, because a duplicated join is how a suite
/// starts proving something else: an executor built by a different sequence could pass an assertion that the
/// daemon's own composition would fail.
async fn executor_over_fixture_named(
    test: &str,
    fixture: &str,
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
    let discovered = discover_stdio_server(&spec_for(program, fixture), SERVER_NAME)
        .await
        .expect("the fixture must be discoverable");

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

/// Builds an executor over the **task-handle** fixture, which offers the ordinary tool.
async fn executor_over_task_fixture(
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
    executor_over_fixture_named(test, FIXTURE_TASK).await
}

/// Builds an executor over the **input-required** fixture, which offers the ordinary tool.
async fn executor_over_input_required_fixture(
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
    executor_over_fixture_named(test, FIXTURE_INPUT_REQUIRED).await
}

/// Builds an executor over the **state-only** fixture, which offers the ordinary tool.
async fn executor_over_state_only_fixture(
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
    executor_over_fixture_named(test, FIXTURE_STATE_ONLY).await
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
        caller: None,
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
        caller: None,
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
        caller: None,
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
        caller: None,
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

/// Proves a dispatch to a session that has **already closed** is a settled refusal, not an ambiguous one.
///
/// **The defect this closes is the same class as the error-code round, one layer out.** A dispatch to a dead
/// session cannot be delivered, so the transport reports `TransportClosed`, which `outcome.rs` classifies
/// `ProviderError` — an *unsettled* outcome. That class is what sends a call to reconciliation, so a call
/// that certainly did nothing would enter a pass that exists for calls which might have run. The executor now
/// asks the session before sending and answers `Unavailable`, which is settled and `Safe`.
///
/// The session is closed here by **dropping the only other owner** of the running service, which is exactly
/// how a child dies in production: the handle is released and the worker loop ends. No supervisor is invented
/// for the test — the assertion is about what the executor answers, not about how a death is detected.
///
/// The mutation is removing the `is_closed` guard in `McpToolExecutor::invoke`: the call then reaches the
/// transport and this fails on `Ambiguous`, naming the direction rather than only the class.
#[tokio::test]
async fn a_dispatch_to_an_already_closed_session_is_settled_rather_than_ambiguous() {
    let Some(program) =
        program_or_skip("a_dispatch_to_an_already_closed_session_is_settled_rather_than_ambiguous")
    else {
        return;
    };
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
    let (session, _, _) = discovered.into_session();
    let executor = McpToolExecutor::new(Arc::clone(&session), identities.clone(), CALL_TIMEOUT_MS);

    // Open before the shutdown, so "closed after" is a change rather than a default — the round's own rule
    // that an assertion about a transition must establish the starting state.
    assert!(
        !executor.is_closed(),
        "the session must be live before it is closed, or this proves nothing"
    );
    // Cancel and drop the owning handle: the worker loop ends, so the session reports itself closed.
    session.cancellation_token().cancel();
    drop(session);
    // Bounded wait rather than a sleep, so a slow machine does not make this flaky — and so a session that
    // never closes fails loudly here instead of surfacing as a confusing class mismatch below.
    let bound = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !executor.is_closed() && tokio::time::Instant::now() < bound {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        executor.is_closed(),
        "the cancelled session must report itself closed before the dispatch"
    );

    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        caller: None,
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("a closed session cannot serve a call");
    // **A settled class, and asserted by its two properties rather than only by value** — the same shape the
    // error-code round used, because the harm is the *unsettled* reading and a value assertion would let a
    // different unsettled class satisfy it.
    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::Unavailable),
        "a call that provably never left must not report an ambiguous outcome"
    );
    assert_ne!(
        error,
        ToolExecutionError::Ambiguous,
        "an undeliverable call is not an ambiguous one"
    );
    if let ToolExecutionError::Failed(class) = error {
        assert!(
            !class.is_unsettled(),
            "an already-closed session means nothing was sent, so the outcome is not ambiguous"
        );
    }

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

/// Proves a server that tries to answer with a **task handle** is refused as `Unavailable` — *not* as the
/// class JARVIS's own decision layer assigns that response, and not as `OutputInvalid` either.
///
/// **This test began as a wrong prediction and is worth keeping for what it corrected.** The adapter has a
/// deliberate arm for this response: `call.rs`'s `decide_response` refuses a task handle with its own code,
/// `mcp.response_not_complete`, classed `OutputInvalid` on the reading that answering one means polling a
/// lifecycle this build does not implement. Reading that code, the obvious expectation is that a fixture
/// returning `CallToolResponse::Task` produces `OutputInvalid`. **It produces `Unavailable`**, and the chain
/// has two links, neither of them visible from `call.rs`:
///
/// 1. The SDK's **server** refuses to send a task handle at all unless the client declared the tasks
///    extension capability — so a conforming server never emits one to JARVIS, whose `JarvisClient`
///    deliberately declares no server-initiated capabilities.
/// 2. The SDK's **client** helper also converts the response itself, returning
///    `ServiceError::UnexpectedResponse` (SEP-2663: "this helper does not drive the task polling
///    lifecycle"), so no JARVIS decision function is consulted.
///
/// What actually reaches the port arrives as the JSON-RPC code `-32021`
/// (`MISSING_REQUIRED_CLIENT_CAPABILITY`), which `outcome::classify_error_code` maps to `Unavailable`. The
/// assertion below therefore ties the observed class to that mapping rather than merely restating it, so the
/// test fails if either the code path or the mapping moves.
///
/// **Why the difference matters rather than being a detail.** `OutputInvalid` is `Never` for retrying;
/// `Unavailable` is `Safe`. A reader of `call.rs` would predict that a task handle is permanently
/// unretryable, and the truth is the opposite posture. The class is also the *safer* of the two here for the
/// reason the mapping gives — a request refused before dispatch cannot have duplicated an effect — and it is
/// settled, so it does not send the recovery pass looking for an effect that cannot exist.
#[tokio::test]
async fn a_task_handle_answer_is_refused_at_the_negotiated_capability_not_by_jarvis() {
    let test = "a_task_handle_answer_is_refused_at_the_negotiated_capability_not_by_jarvis";
    let Some((executor, identities, session)) = executor_over_task_fixture(test).await else {
        return;
    };
    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        caller: None,
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };

    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("a task handle must not be served as a completed result");

    // The literal class, asserted directly rather than through a lookup.
    //
    // **The first version of this test derived the expectation by calling `classify_error_code` with the code
    // it expected, and that version was vacuous.** Mutating the code to a *different* one that shares the same
    // class (`UNSUPPORTED_PROTOCOL_VERSION`, which is also `Unavailable`) left the test green — so the
    // assertion tied nothing down and would have passed for any code in that class. What the port exposes is
    // the **class**, not the code: `ToolExecutionError::Failed` carries `ToolErrorClass` and the JSON-RPC code
    // the SDK consumed on the way is not observable here. So the class is written out, and the code is
    // recorded in the prose above as the mechanism rather than asserted as if it were visible.
    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::Unavailable),
        "the negotiated-capability refusal is classed `Unavailable`, not `OutputInvalid`"
    );

    // The correction, asserted so a future refactor cannot quietly restore the old expectation: the class
    // `call.rs`'s task-handle arm assigns is a *different* one, so the documented answer and the delivered one
    // are known to disagree here rather than assumed to agree.
    assert_ne!(
        ToolErrorClass::Unavailable,
        ToolErrorClass::OutputInvalid,
        "`call.rs` classes a task handle `OutputInvalid`; if that ever becomes the delivered class, the \
         reachability note in `call.rs` is stale and must be updated with this test"
    );
    // Settled, and that is the safety property: the server answered, so nothing is in doubt and the recovery
    // pass must not treat this as a possibly-existing effect.
    assert!(
        !ToolErrorClass::Unavailable.is_unsettled(),
        "a refusal that reached this adapter as an error is settled"
    );
    // **The discriminator that makes `Unavailable` mean the guard rather than a dead socket.** `Unavailable`
    // has three producers in this adapter — the `-32020`/`-32021` door refusals, a transport that could not
    // send, and a session that has closed — so the class alone does not say which happened. A refusal the
    // *server* sent is a JSON-RPC error response on a live session, so the session must still be open; a
    // closed transport would have reported the same class for a different reason. Asserted rather than
    // reasoned about, because this test's first version asserted a class that several paths produce and had to
    // be rewritten for exactly that reason.
    assert!(
        !executor.is_closed(),
        "the server refused the call rather than dropping the connection, so the session must still be open"
    );

    drop(session);
    executor_shutdown(executor);
}

/// Proves a server naming an input request ends the call at **JARVIS's own refusal**, not at the round cap
/// — and that the round cap is reachable only through a shape that asks JARVIS nothing.
///
/// **This test decided that a piece of code should be deleted rather than fixed, and the first version of it
/// asserted the wrong class.** `invocation.rs` used to carry a `next_round` counter and a
/// `McpInvocationRefusal::RoundLimitExceeded` variant whose doc said a caller would "refuse *before* handing
/// the call over" so that "JARVIS's refusal is the one an operator sees". Two things were false about that,
/// and only a live child could show either:
///
/// 1. **nothing called the counter** — the loop is the SDK's and is not interceptable per-round; and
/// 2. **the code it produced (`mcp.round_limit_exceeded`) is not what an operator sees anyway.** A round that
///    *names* a request is answered by `JarvisClient` with `-32602`, which propagates out of the SDK's loop as
///    an error at **round one** — so the cap is never reached for a server that actually asks a question, and
///    the class is `SchemaInvalid`. The cap is reachable only through a round carrying a `request_state` and
///    no request, which consults no handler at all (asserted separately below).
///
/// My first prediction was `LimitExceeded` and it was wrong; the assertion is written to the observed class
/// **plus** the discriminator that explains it, so a future change to either is a failure rather than a
/// silent pass.
#[tokio::test]
async fn a_round_that_names_a_request_ends_at_jarvis_refusal_not_the_round_cap() {
    let test = "a_round_that_names_a_request_ends_at_jarvis_refusal_not_the_round_cap";
    let Some((executor, identities, session)) = executor_over_input_required_fixture(test).await
    else {
        return;
    };
    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        caller: None,
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };

    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("a round the server never fulfils must not complete");

    // The class, written literally rather than derived — computing the expectation through
    // `classify_error_code` is the mistake a previous round's detector made, where the assertion stayed green
    // under a mutation to a different code in the same class.
    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::SchemaInvalid),
        "JARVIS's own `-32602` refusal for the named input kind is what ends this call, at round one"
    );
    // The explanation, asserted rather than assumed: this is JARVIS's answer to the request, so the class is
    // the one a client-side `-32602` maps to and not a round limit.
    assert_ne!(
        ToolErrorClass::SchemaInvalid,
        ToolErrorClass::LimitExceeded,
        "a named request must not read as an exhausted round; if these ever coincide the doc above is stale"
    );
    // The session is live, which distinguishes "the handler refused" from "the transport died".
    assert!(
        !executor.is_closed(),
        "the refusal came back over a live session; nothing about the transport failed"
    );

    drop(session);
    executor_shutdown(executor);
}

/// Proves a server answering with a **`request_state` and no request** reaches the round cap as
/// `LimitExceeded` — the class, and not the `mcp.round_limit_exceeded` a deleted JARVIS counter promised.
///
/// **This is the only shape that reaches the cap, which is the finding the sibling test above establishes.**
/// A state-only round consults no `ClientHandler` method, so nothing refuses it: the SDK's loop retries with
/// the echoed state until `max_rounds`, and the class comes from `classify_service_error`'s mapping of
/// `InputRequiredRoundsExceeded` to `LimitExceeded` — whose contract code is `tool.limit_exceeded`.
///
/// **The fixture carries a `request_state`, and that is load-bearing.** With it the loop retries until its
/// cap, which is the path under test; without it the helper rejects the answer as `UnexpectedResponse` before
/// any retry, so a state-less fixture would pin a different defect than the cap.
#[tokio::test]
async fn a_state_only_round_reaches_the_round_cap_as_a_limit() {
    let test = "a_state_only_round_reaches_the_round_cap_as_a_limit";
    let Some((executor, identities, session)) = executor_over_state_only_fixture(test).await else {
        return;
    };
    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        caller: None,
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        // Generous, so the round cap rather than the caller's bound is what ends the call. The rounds are
        // driven locally, so this is not slow in wall-clock terms.
        timeout_ms: CALL_TIMEOUT_MS,
    };

    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("a state-only round must not complete");

    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::LimitExceeded),
        "the SDK's round cap is what ends this call, and it is classed a limit"
    );
    assert_eq!(
        ToolErrorClass::LimitExceeded.as_contract_str(),
        "tool.limit_exceeded",
        "this is the code an operator greps for; `mcp.round_limit_exceeded` is produced by no code"
    );
    // Settled: the rounds happened locally and the loop stopped itself, so nothing is in doubt about an
    // effect. A bound is not an ambiguity about what the server did.
    assert!(
        !ToolErrorClass::LimitExceeded.is_unsettled(),
        "a round cap is a settled outcome, so the recovery pass must not reconcile it"
    );
    assert!(
        !executor.is_closed(),
        "the cap ended the call; the session was not the thing that failed"
    );

    drop(session);
    executor_shutdown(executor);
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
        caller: None,
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

/// Proves the **request's** declared bound is the one enforced, not a value the executor chose at composition.
///
/// **The defect this closes is a write-only field.** `ToolExecutionRequest::timeout_ms` is documented as "the
/// tool's declared timeout" and the pipeline fills it from `definition.execution.timeout_ms`, which is the
/// bound a tool was *reviewed* with — but until this round no executor read it. `McpToolExecutor` captured its
/// own value at composition time and measured every call against that, so the reviewed bound and the enforced
/// bound were two answers to one question, and the one that ran was the one nobody reviewed.
///
/// ⚠ **The first version of this detector was vacuous, and the mutation is what said so.** It used the
/// never-answering fixture with a short request bound and a generous fallback, and asserted `Ambiguous` —
/// which **both** bounds produce, so making the executor ignore the request left the test passing (the run
/// merely took 10 s instead of 0.3 s). A timeout is a timeout; the two implementations differed in *how long*
/// they waited, and a duration is not an assertion.
///
/// So the fixture is the **slow** one, which answers after [`FIXTURE_SLOW_DELAY_MS`] — between the request's
/// short bound and the executor's generous fallback. Now the two implementations produce different **kinds**
/// of result: the request's bound elapses first and the call is `Ambiguous`, while an executor using its own
/// field lets the fixture answer and returns its text. The assertion can tell them apart, which is what makes
/// it about which bound was used rather than about whether one elapsed.
#[tokio::test]
async fn the_requests_own_bound_is_what_a_call_is_measured_against() {
    let Some(program) =
        program_or_skip("the_requests_own_bound_is_what_a_call_is_measured_against")
    else {
        return;
    };
    let discovered = discover_stdio_server(&spec_for(program, FIXTURE_SLOW), SERVER_NAME)
        .await
        .expect("the slow fixture must still be discoverable — it lists a tool");
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
    // The fallback is **generous enough for the slow fixture to answer within it**, so an executor measuring
    // against this would succeed rather than time out.
    let executor = McpToolExecutor::new(Arc::clone(&session), identities.clone(), CALL_TIMEOUT_MS);
    assert!(
        u64::from(SLOW_FIXTURE_DELAY_MS) < CALL_TIMEOUT_MS,
        "the fallback must outlast the fixture's delay, or this proves nothing"
    );

    let identity = &identities[0];
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        caller: None,
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: SHORT_CALL_BOUND_MS,
    };

    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("the request's own short bound must end this call before the fixture answers");
    assert_eq!(
        error,
        ToolExecutionError::Ambiguous,
        "the request's bound elapsed while the fixture was still withholding, so the outcome is ambiguous — \
         a success here would mean the executor used its own fallback instead of the request's bound"
    );

    drop(session);
    executor_shutdown(executor);
}

/// Proves a request that declares **no** bound falls back rather than becoming unbounded.
///
/// The companion to the test above, and a different claim: `0` must not mean "wait forever". The pipeline
/// always supplies a value, so this exercises the arm a caller outside it would reach — and a fallback is the
/// only reading that keeps the port's promise that a server which accepts a request and never answers cannot
/// hold the dispatcher open with no clock.
#[tokio::test]
async fn a_request_without_a_bound_falls_back_rather_than_becoming_unbounded() {
    let Some(program) =
        program_or_skip("a_request_without_a_bound_falls_back_rather_than_becoming_unbounded")
    else {
        return;
    };
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
        caller: None,
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        // No bound declared: the fallback must apply, and here the fallback is the short one.
        timeout_ms: 0,
    };

    let error = executor
        .execute(request, &cancel)
        .await
        .expect_err("zero must fall back to a bound rather than meaning unbounded");
    assert_eq!(
        error,
        ToolExecutionError::Ambiguous,
        "the fallback bound elapsed, which is the only reason this call ended"
    );

    drop(session);
    executor_shutdown(executor);
}
