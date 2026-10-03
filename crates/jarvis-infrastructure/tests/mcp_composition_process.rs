//! End-to-end composition against **real spawned children**: the wiring the MCP slices were built for.
//!
//! Every piece is tested in isolation elsewhere — discovery spawns a child, registration admits a catalog,
//! the executor calls a tool — and what none of those can show is that the **order** holds when they are run
//! together: resolve, spawn, handshake, list, normalize, register, publish what was admitted, and dispatch by
//! identity. This file runs that whole chain against `examples/mcp_fixture_server.rs`.
//!
//! # What the composition tests can and cannot see
//!
//! `src/mcp/composition_tests.rs` covers the pure half — the launch specification, the secret resolution
//! ordering, the refusal codes — because those are values. What needs a process is that a declaration an
//! operator would write actually produces a callable tool, and that a *second* server's tools land on the
//! second server's session rather than the first's.
//!
//! # How the child is found
//!
//! Examples have no `CARGO_BIN_EXE_*` variable, so the path is derived from this test executable's own
//! location. A missing example is a **skip with a printed reason**, because running this test target alone
//! does not build examples — and a test that failed for an unrelated reason would train a reader to ignore it.

// The workspace denies `clippy::panic`/`unwrap_used`/`expect_used`, and neither a Cargo `tests/*.rs` target nor
// the example is classified by `clippy.toml`'s test allowances. In a test a panic *is* the failure report.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::tool_call::{ToolExecutionError, ToolExecutionRequest, ToolExecutor};
use jarvis_domain::tool::call::ToolArguments;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::registry::ToolRegistry;
use jarvis_infrastructure::config::mcp::{McpSection, McpServerSection};
use jarvis_infrastructure::config::secret::{MapSecretResolver, SecretReference};
use jarvis_infrastructure::mcp::composition::{
    ComposedMcpExecutor, compose_declared_servers, launch_spec_for,
};
use jarvis_infrastructure::mcp::process::DEFAULT_MCP_STARTUP_TIMEOUT_MS;

/// The fixture-selecting variable, mirrored from the example.
const FIXTURE_ENV: &str = "JARVIS_MCP_FIXTURE";

/// The standard fixture: one usable tool, target version preferred.
const FIXTURE_STANDARD: &str = "standard";

/// The text the fixture returns from a call, mirrored from the example.
const FIXTURE_CALL_TEXT: &str = "fixture result for read_file";

/// A call bound, short enough that a hung fixture fails the test rather than the harness.
const CALL_TIMEOUT_MS: u64 = 10_000;

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

/// A declaration naming `server` and launching the fixture with `fixture` selected.
///
/// **The environment carries the reference, and the resolver holds the value** — which is the shape a real
/// profile has: the file says `env:JARVIS_MCP_FIXTURE`, and the daemon's environment holds `standard`. That
/// the fixture is selected at all is therefore also the assertion that resolution happened and the resolved
/// literal reached the child: a child that received nothing could not know which fixture to be.
///
/// The locator is per-server, so two declarations in one fixture do not share a secret — which is what lets a
/// test hold one server's value and not the other's.
fn declaration_for(server: &str, program: &std::path::Path, _fixture: &str) -> McpServerSection {
    McpServerSection {
        name: server.to_owned(),
        program: program.to_string_lossy().into_owned(),
        args: Vec::new(),
        env: BTreeMap::from([(
            FIXTURE_ENV.to_owned(),
            SecretReference::Env(format!("JARVIS_FIXTURE_{}", server.to_uppercase())),
        )]),
        enabled: true,
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    }
}

/// A resolver holding the fixture selector for each declared server name.
fn resolver_for(names: &[(&str, &str)]) -> MapSecretResolver {
    let resolver = MapSecretResolver::new();
    for (server, fixture) in names {
        resolver.insert(
            format!("JARVIS_FIXTURE_{}", server.to_uppercase()),
            *fixture,
        );
    }
    resolver
}

/// An argument document the fixture's schema accepts (it takes any object).
fn arguments(document: &str) -> ToolArguments {
    ToolArguments::new(document).expect("the fixture arguments are usable text")
}

/// The instant a fixture call started at. Fixed rather than wall-clock: `UtcTimestamp` has no `now()` precisely
/// so a test asserts a value rather than whatever the clock read.
fn fixture_instant() -> jarvis_domain::time::UtcTimestamp {
    jarvis_domain::time::UtcTimestamp::parse("2026-10-03T00:00:00Z")
        .expect("the fixture instant is a valid RFC 3339 timestamp")
}

#[tokio::test]
async fn a_declared_server_becomes_a_callable_tool_through_the_whole_chain() {
    // **The wiring test.** Nothing below is new logic; what it proves is that the chain runs in order and that
    // an operator's declaration reaches a dispatched call. A unit test cannot show this, because the order is
    // the composition's own contribution.
    let Some(program) =
        program_or_skip("a_declared_server_becomes_a_callable_tool_through_the_whole_chain")
    else {
        return;
    };
    let section = McpSection {
        servers: vec![declaration_for("acme-files", &program, FIXTURE_STANDARD)],
    };
    let declarations = section
        .enabled_declarations()
        .expect("the fixture declaration must be valid");
    let secrets = resolver_for(&[("acme-files", FIXTURE_STANDARD)]);

    let mut registry = ToolRegistry::new();
    let composition = compose_declared_servers(&mut registry, &declarations, &secrets)
        .await
        .expect("the declarations are valid, so composition must not fail as a whole");
    assert!(
        composition.is_clean(),
        "the fixture must compose: {:?}",
        composition.refused()
    );
    assert_eq!(composition.servers().len(), 1);
    let server = &composition.servers()[0];
    assert_eq!(server.server().as_str(), "acme-files");
    assert_eq!(server.admitted().len(), 1, "the fixture offers one tool");
    assert!(server.report().is_clean(), "{:?}", server.report().refused);
    // The registry now holds the tool, so a grant naming it can be written.
    assert_eq!(registry.len(), 1);

    // And it is callable through the dispatcher the router would hold.
    let dispatcher = ComposedMcpExecutor::over(&composition);
    assert_eq!(dispatcher.servers(), 1);
    let identity = &server.admitted()[0];
    assert!(
        dispatcher.executor_for(identity).is_some(),
        "the composed dispatcher must serve the identity it admitted"
    );

    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    let result = dispatcher
        .execute(request, &cancel)
        .await
        .expect("the declared server's tool must run end to end");
    assert!(
        serde_json::to_string(&result)
            .expect("the body serializes")
            .contains(FIXTURE_CALL_TEXT),
        "the server's own text must reach the port's result"
    );

    // Dropped inside the runtime: the sessions' transports kill their children from `Drop`.
    drop(composition);
    drop(dispatcher);
}

#[tokio::test]
async fn a_server_whose_secret_is_missing_is_refused_while_the_others_compose() {
    // **Per-server failure, and it is the composition's own decision.** Aborting the whole composition would
    // mean a typo in one declaration takes down every server the operator configured, so one refusal must not
    // stop the others — while the reason still reaches the caller with the server's name.
    let Some(program) =
        program_or_skip("a_server_whose_secret_is_missing_is_refused_while_the_others_compose")
    else {
        return;
    };
    let section = McpSection {
        servers: vec![
            declaration_for("acme-files", &program, FIXTURE_STANDARD),
            declaration_for("other-files", &program, FIXTURE_STANDARD),
        ],
    };
    let declarations = section.enabled_declarations().expect("valid");
    // The resolver holds **only** the second server's secret, so the first cannot launch.
    let secrets = resolver_for(&[("other-files", FIXTURE_STANDARD)]);

    let mut registry = ToolRegistry::new();
    let composition = compose_declared_servers(&mut registry, &declarations, &secrets)
        .await
        .expect("a per-server failure is not a profile-level fault");
    assert!(
        !composition.is_clean(),
        "the missing secret must be reported"
    );
    assert_eq!(composition.servers().len(), 1, "the second server composed");
    assert_eq!(composition.servers()[0].server().as_str(), "other-files");
    assert_eq!(composition.refused().len(), 1);
    let (name, refusal) = &composition.refused()[0];
    assert_eq!(name, "acme-files", "the refusal must name the server");
    assert_eq!(
        refusal.code(),
        "jarvis.secret_unavailable",
        "and it must be the secret code, so the operator fixes the profile"
    );
    assert!(
        refusal.is_permanent(),
        "the same profile resolves the same way until it is edited"
    );

    drop(composition);
}

#[tokio::test]
async fn a_disabled_server_is_never_spawned() {
    // The `enabled` flag, asserted where it has an observable consequence: a disabled declaration with an
    // **unresolvable** secret composes cleanly, because nothing is resolved for a server that is not launched.
    // A composition that resolved before checking `enabled` would fail on a server the operator switched off.
    //
    // ⚠ **The first version of this test passed the already-filtered list**, so it proved that
    // `enabled_declarations()` filters rather than that the *composition* skips — and the resolvable-before-
    // enabled ordering claim was therefore untested. It passes `declarations()` now, which is the unfiltered
    // set, so the composition's own skip is what is exercised.
    let Some(program) = program_or_skip("a_disabled_server_is_never_spawned") else {
        return;
    };
    let mut disabled = declaration_for("acme-files", &program, FIXTURE_STANDARD);
    disabled.enabled = false;
    let section = McpSection {
        servers: vec![disabled],
    };
    // `declarations()` still validates it, because a disabled server is a reviewed record.
    let declarations = section
        .declarations()
        .expect("a disabled declaration is still validated");
    assert_eq!(declarations.len(), 1);
    assert!(
        !declarations[0].enabled,
        "the fixture must hand the composition a disabled declaration, or the skip is not exercised"
    );

    // An **empty** resolver, so a composition that resolved before honouring `enabled` would refuse this
    // server for a missing secret.
    let secrets = MapSecretResolver::new();
    let mut registry = ToolRegistry::new();
    let composition = compose_declared_servers(&mut registry, &declarations, &secrets)
        .await
        .expect("valid declarations, and no enabled server to fail");
    assert!(
        composition.is_clean(),
        "a disabled server's secret must never be resolved: {:?}",
        composition.refused()
    );
    assert!(composition.servers().is_empty(), "nothing was launched");
    assert!(composition.admitted_identities().is_empty());
    assert!(registry.is_empty());
}

#[tokio::test]
async fn every_server_in_one_profile_dispatches_to_its_own_session() {
    // **The multi-server property, and the reason `ComposedMcpExecutor` exists.** Every MCP server's tools
    // declare `SourceKind::McpServer`, so `RoutingExecutor` holds exactly one executor for the kind — meaning
    // the fan-out across servers has to happen inside it, keyed by identity. Two servers offering the same
    // tool name must still reach their own sessions, which is what this asserts.
    let Some(program) =
        program_or_skip("every_server_in_one_profile_dispatches_to_its_own_session")
    else {
        return;
    };
    let section = McpSection {
        servers: vec![
            declaration_for("acme-files", &program, FIXTURE_STANDARD),
            declaration_for("other-files", &program, FIXTURE_STANDARD),
        ],
    };
    let declarations = section.enabled_declarations().expect("valid");
    let secrets = resolver_for(&[
        ("acme-files", FIXTURE_STANDARD),
        ("other-files", FIXTURE_STANDARD),
    ]);

    let mut registry = ToolRegistry::new();
    let composition = compose_declared_servers(&mut registry, &declarations, &secrets)
        .await
        .expect("valid declarations, valid secrets");
    assert!(composition.is_clean(), "{:?}", composition.refused());
    assert_eq!(composition.servers().len(), 2);
    // Two identities, one capability each but different owners — so the two servers offer the *same tool name*
    // and are still distinct.
    let admitted = composition.admitted_identities();
    assert_eq!(admitted.len(), 2, "both servers admitted one tool each");
    assert_eq!(
        admitted[0].capability, admitted[1].capability,
        "the fixture must offer the same capability from both servers, or the dispatch is not tested"
    );
    assert_ne!(
        admitted[0].source.owner, admitted[1].source.owner,
        "and the owners must differ, which is what makes the identities distinct"
    );
    assert_eq!(
        composition.source_kinds(),
        std::iter::once(jarvis_domain::tool::identity::SourceKind::McpServer).collect(),
        "every MCP server shares one source kind — the fact the fan-out lives inside"
    );

    let dispatcher = ComposedMcpExecutor::over(&composition);
    assert_eq!(dispatcher.servers(), 2);

    // Each identity resolves, and each resolves to a *different* executor — asserted by the pointer rather than
    // by a result, because the two servers answer identically and a result cannot distinguish them.
    let first = dispatcher
        .executor_for(&admitted[0])
        .expect("the first server's identity must resolve");
    let second = dispatcher
        .executor_for(&admitted[1])
        .expect("the second server's identity must resolve");
    assert!(
        !std::sync::Arc::ptr_eq(first, second),
        "each server's tools must reach their own session, not a shared one"
    );

    drop(composition);
}

#[tokio::test]
async fn an_identity_no_server_admitted_is_refused_rather_than_served_by_another() {
    // **The misroute direction.** The dispatcher must not fall back to the first server: a tool name from one
    // server reaching another's process is the defect the identity key exists to prevent. `NotFound` matches
    // `RoutingExecutor`'s own answer for an unrouted kind.
    let Some(program) =
        program_or_skip("an_identity_no_server_admitted_is_refused_rather_than_served_by_another")
    else {
        return;
    };
    let section = McpSection {
        servers: vec![declaration_for("acme-files", &program, FIXTURE_STANDARD)],
    };
    let declarations = section.enabled_declarations().expect("valid");
    let secrets = resolver_for(&[("acme-files", FIXTURE_STANDARD)]);
    let mut registry = ToolRegistry::new();
    let composition = compose_declared_servers(&mut registry, &declarations, &secrets)
        .await
        .expect("valid");
    assert!(composition.is_clean(), "{:?}", composition.refused());

    let dispatcher = ComposedMcpExecutor::over(&composition);
    // The admitted identity has a fingerprint from the fixture; this one differs by fingerprint only, which is
    // the shape a re-schema has — the identity authority was recorded against is no longer this tool.
    let admitted = composition.admitted_identities();
    let mut re_schemed = admitted[0].clone();
    re_schemed.schema_fingerprint =
        jarvis_domain::tool::identity::SchemaFingerprint::from_bytes([0x5a; 32]);
    assert_ne!(
        re_schemed, admitted[0],
        "the fixture must differ, or it tests nothing"
    );
    assert!(
        dispatcher.executor_for(&re_schemed).is_none(),
        "an identity no server admitted must not resolve to one"
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
    let error = dispatcher
        .execute(request, &cancel)
        .await
        .expect_err("a re-schemed identity must be refused");
    assert_eq!(
        error,
        ToolExecutionError::Failed(ToolErrorClass::NotFound),
        "and it must be NotFound, not served by another server"
    );

    drop(composition);
}

#[tokio::test]
async fn a_launch_specification_for_a_declaration_resolves_the_fixture_selector() {
    // The resolution step reached through a declaration an operator would actually write, using the fixture's
    // own environment variable as the secret. It proves the *literal* reaches the child, because a child that
    // received nothing would not know which fixture to be — and the discovery below would fail rather than
    // pass on a default.
    let Some(program) =
        program_or_skip("a_launch_specification_for_a_declaration_resolves_the_fixture_selector")
    else {
        return;
    };
    let declaration = declaration_for("acme-files", &program, FIXTURE_STANDARD)
        .to_declaration()
        .expect("the fixture declaration must validate");
    let secrets = resolver_for(&[("acme-files", FIXTURE_STANDARD)]);
    let spec = launch_spec_for(&declaration, &secrets).expect("the declaration must resolve");

    assert_eq!(
        spec.env
            .iter()
            .find(|(key, _)| key == FIXTURE_ENV)
            .map(|(_, value)| value.as_str()),
        Some(FIXTURE_STANDARD),
        "the resolved literal must reach the specification, not the reference"
    );
    // And the reference itself never appears, which is the property `config/mcp.rs` enforces.
    assert!(
        !spec.env.iter().any(|(_, value)| value.contains("env:")),
        "a reference must never reach the process table"
    );
}
