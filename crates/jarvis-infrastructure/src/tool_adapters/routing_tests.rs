//! Tests for source-kind routing.
//!
//! Two carry more weight than the rest, and each exists because a plausible implementation gets it wrong in
//! a way no other test here would notice:
//!
//! - **`a_routed_call_reaches_the_executor_for_its_kind`** is the routing property. An implementation that
//!   dispatched to the wrong executor would pass every refusal test below — each of them refuses — while
//!   sending one source's tools to another's implementation.
//! - **`a_tool_from_another_source_is_never_dispatched_to_the_native_executor`** is the misroute the module
//!   exists to prevent. A "fall back to native, it is the common case" implementation passes the
//!   unrouted-refusal test if it refuses *nothing* only when the router is empty — which is exactly the
//!   shape a hurried version has.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module reached from
// the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::RoutingExecutor;
use jarvis_application::cancellation::CancellationScope;
use jarvis_application::tool_call::{ToolExecutionError, ToolExecutionRequest, ToolExecutor};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::tool::call::{ContentBlock, ToolArguments, ToolResultBody};
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};

/// An executor that records how many times it was asked to run something, and answers a fixed body.
///
/// The counter is the point: a routing test that asserted only the result could not tell a correct route
/// from a wrong one that happened to produce the same body. Counting which executor ran is what makes the
/// route observable.
struct CountingExecutor {
    /// How many calls this executor was asked to make.
    calls: Arc<AtomicUsize>,
    /// The text it answers with, so two executors in one fixture are distinguishable by result as well.
    text: &'static str,
}

impl ToolExecutor for CountingExecutor {
    fn execute<'a>(
        &'a self,
        _request: ToolExecutionRequest<'a>,
        _cancel: &'a CancellationScope,
    ) -> jarvis_application::tool_call::ToolExecutionFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = self.text;
        Box::pin(async move {
            let block = ContentBlock::text(text)
                .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))?;
            ToolResultBody::new(vec![block], None, Sensitivity::Internal)
                .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))
        })
    }
}

/// A usable identity for a tool of `kind`, with a distinct name per `name`.
fn identity(kind: SourceKind, name: &str) -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::new("mcp", name, 1)
            .expect("the fixture name is a usable segment"),
        source: ToolSource::new(
            kind,
            "fixture",
            ToolVersion::parse("1.0.0").expect("the fixture version parses"),
        )
        .expect("the fixture owner is usable"),
        // Not derived from a schema: these tests are about which executor runs, and every assertion compares
        // a route rather than a fingerprint. `ToolIdentity` has no constructor, only validated fields.
        schema_fingerprint: SchemaFingerprint::from_bytes([0x11; 32]),
    }
}

/// An executor pair with their call counters, for the two kinds a fixture routes.
fn two_kinds() -> (RoutingExecutor, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let native_calls = Arc::new(AtomicUsize::new(0));
    let mcp_calls = Arc::new(AtomicUsize::new(0));
    let router = RoutingExecutor::new([
        (
            SourceKind::Native,
            Arc::new(CountingExecutor {
                calls: Arc::clone(&native_calls),
                text: "native result",
            }) as Arc<dyn ToolExecutor>,
        ),
        (
            SourceKind::McpServer,
            Arc::new(CountingExecutor {
                calls: Arc::clone(&mcp_calls),
                text: "mcp result",
            }) as Arc<dyn ToolExecutor>,
        ),
    ]);
    (router, native_calls, mcp_calls)
}

/// The arguments a fixture call carries.
fn arguments() -> ToolArguments {
    ToolArguments::new("{}").expect("the fixture arguments are usable")
}

/// Runs one call through `router` and returns what it produced.
async fn run(
    router: &RoutingExecutor,
    identity: &ToolIdentity,
    arguments: &ToolArguments,
) -> Result<ToolResultBody, ToolExecutionError> {
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        caller: None,
        identity,
        display_name: "fixture",
        arguments,
        started_at: jarvis_domain::time::UtcTimestamp::parse("2026-10-03T00:00:00Z")
            .expect("the fixture instant is valid"),
        timeout_ms: 1_000,
    };
    router.execute(request, &cancel).await
}

#[tokio::test]
async fn a_routed_call_reaches_the_executor_for_its_kind() {
    // The routing property, asserted by **which executor ran** rather than by the result alone: the two
    // fixtures answer with different text, so a wrong route is detectable twice over.
    let (router, native_calls, mcp_calls) = two_kinds();
    let arguments = arguments();

    let native = identity(SourceKind::Native, "clock");
    let body = run(&router, &native, &arguments)
        .await
        .expect("the native kind is routed");
    assert_eq!(
        native_calls.load(Ordering::SeqCst),
        1,
        "the native executor must be the one that ran"
    );
    assert_eq!(
        mcp_calls.load(Ordering::SeqCst),
        0,
        "and the MCP executor must not have been consulted"
    );
    assert!(
        serde_json::to_string(&body)
            .expect("the body serializes")
            .contains("native result"),
        "the result must be the native executor's"
    );

    // The other direction, so this is not a router that always picks the first registered kind.
    let server = identity(SourceKind::McpServer, "read_file");
    let body = run(&router, &server, &arguments)
        .await
        .expect("the MCP kind is routed");
    assert_eq!(mcp_calls.load(Ordering::SeqCst), 1);
    assert_eq!(native_calls.load(Ordering::SeqCst), 1, "unchanged");
    assert!(
        serde_json::to_string(&body)
            .expect("the body serializes")
            .contains("mcp result"),
        "the result must be the MCP executor's"
    );
}

#[tokio::test]
async fn a_source_kind_with_no_executor_is_refused_rather_than_falling_back() {
    // **The fail-closed direction.** A fallback to the native executor is what a hurried version writes —
    // "native is the common case" — and its failure is a *misroute*: a server's tool name reaching the
    // daemon's own clock or filesystem. So an unrouted kind refuses, and the native counter proves no
    // executor ran.
    let (router, native_calls, mcp_calls) = two_kinds();
    let arguments = arguments();

    // `Connector` is a real source kind and deliberately not routed by this fixture.
    assert!(
        !router.routed_kinds().contains(&SourceKind::Connector),
        "the fixture must not route this kind, or the test proves nothing"
    );
    let connector = identity(SourceKind::Connector, "gmail_send");
    let error = run(&router, &connector, &arguments)
        .await
        .expect_err("an unrouted kind must be refused");

    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::NotFound),
        "an unrouted call is NotFound: the daemon has no implementation for it"
    );
    assert_eq!(
        native_calls.load(Ordering::SeqCst),
        0,
        "and no other executor may be reached as a fallback"
    );
    assert_eq!(mcp_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_tool_from_another_source_is_never_dispatched_to_the_native_executor() {
    // **The misroute, and the first version of this test did not actually test it.** That version routed a
    // `Native` tool and asserted native ran — which a fallback-to-native implementation does identically,
    // because the kind *is* routed. The mutation that adds the fallback was caught only by the
    // unrouted-refusal test above, which is how the gap was found: the test's name claimed a state
    // ("another source") its fixture never created.
    //
    // So this version registers **only** native and attempts a call from an MCP source. Routed by kind, it
    // is refused and native is never reached; with a fallback, native runs — which is the misroute: a
    // server's tool name in front of the daemon's own implementation.
    let native_calls = Arc::new(AtomicUsize::new(0));
    let router = RoutingExecutor::new([(
        SourceKind::Native,
        Arc::new(CountingExecutor {
            calls: Arc::clone(&native_calls),
            text: "native result",
        }) as Arc<dyn ToolExecutor>,
    )]);
    assert_eq!(router.routed_kinds(), vec![SourceKind::Native]);

    let arguments = arguments();
    // Same capability namespace as the native fixture would have, so a namespace-keyed router would also
    // send this the wrong way — the point is that the *source* decides.
    let server_tool = identity(SourceKind::McpServer, "read_file");
    let error = run(&router, &server_tool, &arguments)
        .await
        .expect_err("an MCP tool must not be served by the native executor");
    assert_eq!(error, ToolExecutionError::Failed(ToolErrorClass::NotFound));
    assert_eq!(
        native_calls.load(Ordering::SeqCst),
        0,
        "native must not have been reached as a fallback for another source"
    );

    // The complement: the native kind **is** served, so this is not a router that refuses everything.
    let native_tool = identity(SourceKind::Native, "clock");
    run(&router, &native_tool, &arguments)
        .await
        .expect("the native kind is routed");
    assert_eq!(native_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_empty_router_refuses_everything_rather_than_panicking() {
    // The degenerate configuration: valid, useless, and must not panic. Asserted because the alternative — a
    // `panic` or an `expect` on the lookup — is what a first version writes, and this crate denies `panic` in
    // library code precisely so that a refusal has to be a value.
    let router = RoutingExecutor::new([]);
    assert!(router.is_empty());
    assert_eq!(router.len(), 0);
    assert!(router.routed_kinds().is_empty());

    let arguments = arguments();
    let tool = identity(SourceKind::Native, "clock");
    let error = run(&router, &tool, &arguments)
        .await
        .expect_err("an empty router must refuse");
    assert_eq!(error, ToolExecutionError::Failed(ToolErrorClass::NotFound));
}

#[test]
fn the_routed_kinds_are_reported_in_a_stable_order() {
    // `BTreeMap` order, so a diagnostic and a test see the same list. `SourceKind` is `Ord`, so the order is
    // the declaration order of its variants rather than insertion order — which is what makes it stable
    // across two identically-configured daemons.
    let (router, _, _) = two_kinds();
    let kinds = router.routed_kinds();
    assert_eq!(kinds.len(), 2);
    // Insertion order was Native then McpServer; the report is sorted by the enum's own order.
    assert_eq!(kinds, vec![SourceKind::Native, SourceKind::McpServer]);

    // And a second router built from the pairs **reversed** reports the same list, so the order is the map's
    // rather than the caller's.
    let native: Arc<dyn ToolExecutor> = Arc::new(CountingExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        text: "native",
    });
    let mcp: Arc<dyn ToolExecutor> = Arc::new(CountingExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        text: "mcp",
    });
    let reversed = RoutingExecutor::new([
        (SourceKind::McpServer, Arc::clone(&mcp)),
        (SourceKind::Native, Arc::clone(&native)),
    ]);
    assert_eq!(
        reversed.routed_kinds(),
        kinds,
        "the order must be the map's, not the caller's insertion order"
    );
    assert!(reversed.executor_for(SourceKind::McpServer).is_some());
    assert!(reversed.executor_for(SourceKind::Native).is_some());
    assert!(
        reversed.executor_for(SourceKind::Runtime).is_none(),
        "a kind with no executor must report absent rather than a substitute"
    );
}
