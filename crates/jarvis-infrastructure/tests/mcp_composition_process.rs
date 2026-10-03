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
use std::sync::Arc;

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

/// The fixture whose tool answers with the child's own working directory.
const FIXTURE_CWD: &str = "cwd";

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
        // No directory by default: a child inherits the daemon's, which is the honest default the field's
        // documentation records. The test below sets one explicitly.
        working_directory: None,
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    }
}

/// A declaration whose child runs in `directory`, for the working-directory test.
fn declaration_working_in(
    server: &str,
    program: &std::path::Path,
    directory: &std::path::Path,
) -> McpServerSection {
    let mut declaration = declaration_for(server, program, FIXTURE_CWD);
    declaration.working_directory = Some(directory.to_string_lossy().into_owned());
    declaration
}

#[tokio::test]
async fn a_declared_working_directory_is_the_one_a_spawned_child_actually_runs_in() {
    // **The only way to prove a launch directory was applied.** A specification field can be asserted and a
    // built command can be inspected, but neither shows that `current_dir` reached the operating system — and
    // "the value was threaded through" is a different claim from "the child ran there". The fixture's `cwd`
    // tool answers with `std::env::current_dir()`, so the text is where the process *is*.
    //
    // The gap this closes is half of an `AGENTS.md` requirement: "isolate process environment variables and
    // working directories". The environment was already cleared explicitly; the directory was whatever the
    // daemon happened to be started in, because `McpLaunchSpec::working_dir` was validated, applied by
    // `build_command`, and set by nobody.
    let Some(program) =
        program_or_skip("a_declared_working_directory_is_the_one_a_spawned_child_actually_runs_in")
    else {
        return;
    };
    // A directory this test creates, so the assertion is about a path that exists and whose name nothing else
    // in the tree uses — a shared temp root would let a child that ignored the setting look correct.
    let directory = std::env::temp_dir().join("jarvis-mcp-cwd-test");
    std::fs::create_dir_all(&directory).expect("the fixture directory must be creatable");

    let section = McpSection {
        servers: vec![declaration_working_in("acme-files", &program, &directory)],
    };
    let declarations = section.enabled_declarations().expect("valid");
    let secrets = resolver_for(&[("acme-files", FIXTURE_CWD)]);
    let mut registry = ToolRegistry::new();
    let composition = compose_declared_servers(&mut registry, &declarations, &secrets)
        .await
        .expect("valid");
    assert!(composition.is_clean(), "{:?}", composition.refused());

    let server = &composition.servers()[0];
    let identity = &server.admitted()[0];
    let dispatcher = ComposedMcpExecutor::over(&composition);
    let arguments = arguments("{}");
    let cancel = CancellationScope::new();
    let request = ToolExecutionRequest {
        identity,
        display_name: "where_am_i",
        arguments: &arguments,
        started_at: fixture_instant(),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    let result = dispatcher
        .execute(request, &cancel)
        .await
        .expect("the cwd fixture's tool must run");
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            jarvis_domain::tool::call::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    // **Asserted by the directory's own name rather than by a resolved-path comparison**, because Windows
    // reports a canonicalized form whose prefix differs from `temp_dir()`'s spelling — and a test that failed
    // for that reason would be reporting the platform rather than the defect. The name is unique to this test,
    // so a child that inherited some other directory cannot contain it.
    assert!(
        text.contains("jarvis-mcp-cwd-test"),
        "the child must run in the declared directory, but it reported {text:?}"
    );

    let _ = composition.shutdown().await;
    let _ = std::fs::remove_dir_all(&directory);
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
    assert!(
        composition.health().is_empty(),
        "a disabled server composes nothing, so the probe has no row"
    );
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
    // and are still distinct. **Walked from the servers rather than from a flattened composition accessor,
    // because the flattened form was removed**: it had one caller (here) and existed only to be asserted on,
    // so the property is now stated against the servers the dispatcher is actually built from.
    let admitted: Vec<_> = composition
        .servers()
        .iter()
        .flat_map(|server| server.admitted().iter().cloned())
        .collect();
    assert_eq!(admitted.len(), 2, "both servers admitted one tool each");
    assert_eq!(
        admitted[0].capability, admitted[1].capability,
        "the fixture must offer the same capability from both servers, or the dispatch is not tested"
    );
    assert_ne!(
        admitted[0].source.owner, admitted[1].source.owner,
        "and the owners must differ, which is what makes the identities distinct"
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
        !Arc::ptr_eq(first, second),
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
    let admitted = composition.servers()[0].admitted().to_vec();
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
async fn shutting_a_composition_down_closes_every_session() {
    // **The lifecycle property, and its absence was a real gap.** Composing a server and dropping the
    // composition would leave each child alive only as long as the *dispatcher's* `Arc` happened to, and nothing
    // could ask them to stop — the drain would exit with the processes still running, which for a supervised
    // server means it is orphaned rather than shut down. This asserts the explicit stop closes each session.
    let Some(program) = program_or_skip("shutting_a_composition_down_closes_every_session") else {
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
        .expect("valid");
    assert!(composition.is_clean(), "{:?}", composition.refused());
    assert_eq!(composition.servers().len(), 2);

    // The sessions are open before the shutdown, so "closed after" is a change rather than a default.
    for server in composition.servers() {
        assert!(
            !server.session().is_closed(),
            "a composed server's session must be open before the drain"
        );
    }

    // A dispatcher is built and **kept** across the shutdown, because that is the arrangement a daemon has:
    // the router holds the executor while the composition is stopped. A shutdown that required sole ownership
    // of the session would fail here, which is what makes this a test of the real shape.
    let dispatcher = ComposedMcpExecutor::over(&composition);
    let unclean = composition.shutdown().await;
    assert!(
        unclean.is_empty(),
        "the fixture's servers must stop cleanly, got {unclean:?}"
    );
    // And the dispatcher is still usable as a value — it holds its own `Arc`, so a shutdown must not have
    // required consuming it.
    assert_eq!(dispatcher.servers(), 2);
}

#[tokio::test]
async fn a_lifecycle_probe_records_each_servers_health() {
    // The `list` probe from the note's operational section, over the composed servers. Asserted with a live
    // child because `health` reports what the *session* says, and a fixture that never spawned could only
    // report a constant.
    let Some(program) = program_or_skip("a_lifecycle_probe_records_each_servers_health") else {
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

    let report = composition.health();
    assert_eq!(report.len(), 1, "one server, one row");
    let row = &report[0];
    assert_eq!(row.name, "acme-files");
    assert_eq!(row.tools, 1, "the row reports what registration admitted");
    assert!(
        row.version.is_some(),
        "a live session must report the version it negotiated"
    );
    assert!(
        !row.closed,
        "and it must not be closed while the composition is alive"
    );

    // The complement: after the shutdown the same probe reports the server closed, so `closed` is a state the
    // probe reads rather than a constant it prints.
    let sessions: Vec<_> = composition
        .servers()
        .iter()
        .map(jarvis_infrastructure::mcp::composition::ComposedMcpServer::session)
        .collect();
    let _ = composition.shutdown().await;
    for session in &sessions {
        assert!(
            session.is_closed(),
            "a stopped session must report itself closed"
        );
    }
}

#[tokio::test]
async fn a_partially_refused_catalog_is_reported_beside_the_tool_that_was_admitted() {
    // **The operator fact the composition's own types were built to expose, and nothing could see it.** A
    // server offering one usable tool and one malformed one composes *successfully* — `is_clean()` is true,
    // because the refusal that matters to it is a whole-server one — so before this round the malformed tool
    // left no trace anywhere: not in `McpComposition::refused`, and not in any log. The only signal was a
    // capability missing from a catalog an operator would have to diff by hand.
    //
    // `ComposedMcpServer::report` was carried for exactly this and had no production caller; the daemon now
    // reads it at startup. This asserts the half a daemon-side log line cannot prove: that the report the
    // composition *holds* distinguishes "one tool" from "two offered, one refused".
    let Some(program) = program_or_skip(
        "a_partially_refused_catalog_is_reported_beside_the_tool_that_was_admitted",
    ) else {
        return;
    };
    let section = McpSection {
        servers: vec![declaration_for("acme-files", &program, "partial")],
    };
    let declarations = section.enabled_declarations().expect("valid");
    let secrets = resolver_for(&[("acme-files", "partial")]);
    let mut registry = ToolRegistry::new();
    let composition = compose_declared_servers(&mut registry, &declarations, &secrets)
        .await
        .expect("valid");

    // The server composed, so the partial refusal is invisible at the composition level — which is the whole
    // reason the per-server report has to be read.
    assert!(
        composition.is_clean(),
        "a server with one usable tool is not a whole-server refusal: {:?}",
        composition.refused()
    );
    assert_eq!(composition.servers().len(), 1);

    let server = &composition.servers()[0];
    let report = server.report();
    assert_eq!(
        report.added, 1,
        "the usable tool must be registered, or this is the all-refused case instead"
    );
    // **The registration report is empty here, and that is the defect this test was written to expose.**
    // The malformed tool was refused by the *normalizer*, so registration never saw it and `is_clean()` stayed
    // true. Asserting zero is deliberate: it documents why a registration-only check cannot answer the
    // question, so a future reader does not "fix" this assertion into a false one.
    assert_eq!(
        report.refused.len(),
        0,
        "registration never saw the malformed tool — it was refused upstream"
    );

    let refusals = server.refusals();
    assert_eq!(
        refusals.len(),
        1,
        "the malformed tool must be reported through the unified accessor rather than dropped"
    );
    // The refusal names the tool the *server* used and carries a stable code, so an operator can find the
    // listing entry rather than a position in it.
    assert!(
        refusals[0].name.contains("Read File"),
        "the refusal must name the offered tool, got {:?}",
        refusals[0].name
    );
    assert_eq!(
        refusals[0].code, "mcp.tool_name_invalid",
        "a name with a space is refused for its shape, and the code says so"
    );
    // And the admitted set is exactly the usable tool, so the report and the callable set agree.
    assert_eq!(server.admitted().len(), 1);
    assert_eq!(
        report.registered() + refusals.len(),
        2,
        "the server offered two tools: one registered, one refused, and the two must sum to the listing"
    );

    // **The probe's own count had the same defect this round fixed, independently.** `McpServerHealth::refused`
    // was fed from `report.refused.len()` — registration's half — so on this very server it read **0** while a
    // tool was refused. A probe built to report a server's health undercounting its refusals is the same "the
    // fact is recorded where nothing can check it" shape one layer up, and this is the assertion that catches
    // it: the row must agree with the accessor, not with the stage that happens to be visible to it.
    let rows = composition.health();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].refused,
        refusals.len(),
        "the health row must count every refusal stage, not only registration's"
    );
    assert_ne!(
        rows[0].refused,
        report.refused.len(),
        "and it must disagree with the registration-only count here — otherwise this test proves nothing"
    );

    let _ = composition.shutdown().await;
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
