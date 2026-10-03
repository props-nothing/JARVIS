//! End-to-end discovery against a **real child process**.
//!
//! `mcp_conversation.rs` proves the client and the mapping layers work against an in-process fixture over a
//! duplex pipe. That fixture cannot falsify anything about the *spawn*: whether the transport is actually
//! passed to the client, whether the configured environment survives `env_clear`, whether the negotiated
//! lifecycle is the one the adapter assumes, or whether a failure path names the field at fault. Every test
//! here spawns `examples/mcp_fixture_server.rs` through `spawn_stdio_server`, so the child is a separate
//! process reached over pipes and its stdout carries only protocol frames.
//!
//! # How the child is found
//!
//! Examples have no `CARGO_BIN_EXE_*` variable, so the path is derived from this test executable's own
//! location: both live under the same profile directory, in `deps/` and `examples/` respectively. A missing
//! example is reported as a **skip with a printed reason** rather than a failure, because `cargo test -p
//! jarvis-infrastructure --test mcp_discovery_process` alone does not build examples — and a test that failed
//! for a reason unrelated to what it asserts would train a reader to ignore it.

// The workspace denies `clippy::panic`/`unwrap_used`/`expect_used`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets — a `#[path]` module reached from the library
// is not one, and neither is the example. In a test a panic *is* the failure report.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use jarvis_domain::tool::identity::SourceKind;
use jarvis_domain::tool::registry::ServerConfigId;
use jarvis_infrastructure::mcp::discovery::{McpStartupRefusal, discover_stdio_server};
use jarvis_infrastructure::mcp::process::{DEFAULT_MCP_STARTUP_TIMEOUT_MS, McpLaunchSpec};

/// The configured server name these tests use.
///
/// Deliberately a single lowercase word, because it must be usable under **two** rules — a configuration
/// identity and a tool source owner — and a name that satisfies one and not the other is its own refusal.
/// The name is chosen to be unambiguously valid here so the tests that follow are measuring discovery rather
/// than the name rules.
const SERVER_NAME: &str = "fixture";

/// The fixture-selecting variable, mirrored from the example.
///
/// Mirrored rather than imported because the example is a separate target: it cannot be a dependency of this
/// test, and a shared constant would have to live in a library module to be reachable from both. The value is
/// asserted by the test that spawns each fixture — a drift in either copy makes the child exit without
/// serving, and the discovery fails loudly rather than passing on a default.
const FIXTURE_ENV: &str = "JARVIS_MCP_FIXTURE";

/// The standard fixture: one usable tool, target version preferred.
const FIXTURE_STANDARD: &str = "standard";

/// The legacy fixture: one usable tool, and **only** the older version.
const FIXTURE_LEGACY: &str = "legacy";

/// The all-refused fixture: its only tool has an unusable name.
const FIXTURE_ALL_REFUSED: &str = "all-refused";

/// The tool name the standard and legacy fixtures offer.
const FIXTURE_TOOL_NAME: &str = "read_file";

/// The version the legacy fixture reports.
const LEGACY_VERSION_LITERAL: &str = "2025-06-18";

/// The absolute path of the fixture server example for this profile.
///
/// `None` when the example has not been built for this profile, which the caller reports as a skip. Derived
/// from `current_exe` rather than a hard-coded `target/debug` so a `--release` run finds the right directory.
fn fixture_program() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    // <profile>/deps/<test> -> <profile>
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
///
/// A `None` return is a **skip**, and printing the command that fixes it is the point: silent skips are how a
/// suite stops testing anything without saying so.
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

/// A launch spec for the given fixture, with the fixture variable as its **entire** environment.
///
/// The environment is the mechanism, not incidental: `build_command` calls `env_clear()`, so the child sees
/// exactly this list. That every test below reaches its fixture at all is therefore also the assertion that
/// the configured environment is delivered — a server that inherited nothing and was given nothing could not
/// know which fixture to be.
fn spec_for(program: PathBuf, fixture: &str) -> McpLaunchSpec {
    McpLaunchSpec {
        program,
        args: Vec::new(),
        env: vec![(FIXTURE_ENV.to_owned(), fixture.to_owned())],
        working_dir: None,
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    }
}

#[tokio::test]
async fn discovery_spawns_a_real_child_and_returns_the_tools_it_offers() {
    let Some(program) =
        program_or_skip("discovery_spawns_a_real_child_and_returns_the_tools_it_offers")
    else {
        return;
    };
    let spec = spec_for(program, FIXTURE_STANDARD);

    let discovered = discover_stdio_server(&spec, SERVER_NAME)
        .await
        .expect("the standard fixture must be discoverable");

    // The identity is the **configured** name, and the source kind is what the adapter declared — not
    // whatever the child called itself. This is the trusted half of the pair `registration.rs` compares.
    assert_eq!(discovered.server().as_str(), SERVER_NAME);
    let catalog = discovered.catalog();
    assert_eq!(
        catalog.tools.len(),
        1,
        "the fixture offers exactly one tool"
    );
    let definition = &catalog.tools[0];
    assert_eq!(definition.identity.source.kind, SourceKind::McpServer);
    assert_eq!(
        definition.identity.source.owner, SERVER_NAME,
        "the tool's owner must be the configured server name"
    );
    assert!(
        definition.identity.capability.name() == FIXTURE_TOOL_NAME,
        "the capability's name must be the offered name **untransformed**, got {}",
        definition.identity.capability.name()
    );
    assert_eq!(
        definition.identity.capability.namespace(),
        "mcp",
        "the namespace is JARVIS's contract namespace, not the server's name"
    );
    assert!(
        catalog.rejected.is_empty(),
        "the standard fixture's tool must not be refused, got {:?}",
        catalog.rejected
    );
    // The schema is carried alongside, and **in the same order**. A registration whose definition has no
    // schema is a tool that resolves and cannot be called, which is why alignment is asserted rather than
    // assumed from two equal-length vectors.
    assert_eq!(catalog.schemas.len(), catalog.tools.len());
    assert!(
        catalog.schemas[0].contains("path"),
        "the schema must be the one the child sent, got {:?}",
        catalog.schemas[0]
    );

    let (returned, failure) = discovered.shutdown().await;
    assert_eq!(failure, None, "shutting down a healthy child must not fail");
    assert_eq!(returned.tools.len(), 1, "shutdown must return the catalog");
}

#[tokio::test]
async fn a_peer_offering_only_an_older_version_is_discovered_rather_than_refused() {
    // **This test exists to falsify a belief.** The natural reading of "the adapter targets `2026-07-28` and
    // does not use the `Auto` fallback" is that an older peer is refused. It is not: discovery resolves a
    // version from the intersection of both sides' lists, so a peer reporting only `2025-06-18` negotiates
    // down and is served. Recording that here means the negotiated version cannot silently become something
    // a caller assumes otherwise — and `protocol_version()` is exposed precisely so this is observable.
    let Some(program) =
        program_or_skip("a_peer_offering_only_an_older_version_is_discovered_rather_than_refused")
    else {
        return;
    };
    let spec = spec_for(program, FIXTURE_LEGACY);

    let discovered = discover_stdio_server(&spec, SERVER_NAME)
        .await
        .expect("a peer one revision behind must still be discoverable");

    let version = discovered
        .protocol_version()
        .expect("discovery must record the version it negotiated");
    assert_eq!(
        version.as_str(),
        LEGACY_VERSION_LITERAL,
        "the session must run at the only version the peer offers"
    );
    assert_eq!(
        discovered.catalog().tools.len(),
        1,
        "an older peer's tools must still be normalized"
    );

    let (_, failure) = discovered.shutdown().await;
    assert_eq!(failure, None);
}

#[tokio::test]
async fn a_catalog_whose_every_tool_is_refused_is_a_named_refusal_not_an_empty_one() {
    // The distinction this test exists for: "the server has no tools" and "the server has tools this adapter
    // cannot represent" are different operator facts, and both produce an empty `tools` vector. A discovery
    // that returned an empty catalog here would leave an operator with a server that appears to offer nothing.
    let Some(program) = program_or_skip(
        "a_catalog_whose_every_tool_is_refused_is_a_named_refusal_not_an_empty_one",
    ) else {
        return;
    };
    let spec = spec_for(program, FIXTURE_ALL_REFUSED);

    let refusal = discover_stdio_server(&spec, SERVER_NAME)
        .await
        .expect_err("a catalog with no usable tool must be refused");
    match &refusal {
        McpStartupRefusal::NoUsableTools { first_rejection } => {
            // The reason is carried, not just the fact, so the operator output names what to change.
            assert_eq!(
                first_rejection.code(),
                "mcp.tool_name_invalid",
                "the refusal must name the field at fault"
            );
        }
        other => panic!("expected an all-refused catalog, got {other:?}"),
    }
    assert_eq!(refusal.code(), "mcp.no_usable_tools");
    assert!(
        refusal.is_permanent(),
        "the same definitions will be refused the same way next time"
    );
}

#[tokio::test]
async fn an_unusable_configured_name_is_refused_before_any_process_is_spawned() {
    // **The ordering assertion.** The program is deliberately one that cannot launch, so *if* the name were
    // validated after the spawn the result would be a launch refusal instead. Getting the name refusal proves
    // the check precedes the process — which is the whole point of validating a name at all: a configuration
    // defect should not be discovered by starting a process and interpreting what it does.
    let spec = McpLaunchSpec {
        program: PathBuf::from("/nonexistent/jarvis/mcp-fixture-that-cannot-exist"),
        args: Vec::new(),
        env: Vec::new(),
        working_dir: None,
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    };

    // A space is not usable under either name rule.
    let refusal = discover_stdio_server(&spec, "Read File")
        .await
        .expect_err("an unusable name must be refused");
    assert_eq!(
        refusal,
        McpStartupRefusal::ServerNameInvalid,
        "the name must be refused before the launch is attempted"
    );

    // The complement: the *same* impossible program with a usable name does reach the launch, so the
    // assertions above are about ordering rather than about the program being unspawnable.
    let launch = discover_stdio_server(&spec, SERVER_NAME)
        .await
        .expect_err("an absent program must fail the launch");
    assert!(
        matches!(launch, McpStartupRefusal::Launch { .. }),
        "a missing program must be a launch refusal, got {launch:?}"
    );
    assert_eq!(launch.code(), "mcp.spawn_failed");
    assert!(
        !launch.is_permanent(),
        "an absent program may be installed before the next attempt"
    );
}

#[tokio::test]
async fn a_name_that_is_a_valid_identity_but_not_a_valid_owner_is_refused_up_front() {
    // **The two-rule case.** A leading digit is accepted by `ServerConfigId` and rejected as a tool source
    // owner. Were only the identity checked, every tool from this server would be refused by the normalizer
    // and the operator would see an all-refused catalog — a server-shaped defect for a name-shaped cause.
    let Some(program) = program_or_skip(
        "a_name_that_is_a_valid_identity_but_not_a_valid_owner_is_refused_up_front",
    ) else {
        return;
    };
    let spec = spec_for(program, FIXTURE_STANDARD);

    // Accepted as a configuration identity, so the refusal cannot be coming from that rule.
    assert!(
        ServerConfigId::new("1fixture").is_ok(),
        "this test is only meaningful while the identity rule accepts a leading digit"
    );

    let refusal = discover_stdio_server(&spec, "1fixture")
        .await
        .expect_err("a name that is not a usable owner must be refused");
    assert_eq!(refusal, McpStartupRefusal::ServerNameInvalid);
    assert_eq!(refusal.code(), "mcp.server_config_invalid");
}

#[tokio::test]
async fn a_child_that_is_not_a_server_fails_as_a_startup_refusal_carrying_its_stderr() {
    // The failure path an operator will actually meet: a configured program that runs but does not speak MCP.
    // Two properties at once — the discovery must fail rather than hang until the timeout, and the refusal
    // must carry what the child wrote, because "the transport closed" sends an operator to the wrong place.
    //
    // The child is this platform's shell, given a command that writes a recognisable line to **stderr** and
    // exits. An absolute interpreter path is required: `env_clear` removed `PATH`.
    let shell = {
        #[cfg(windows)]
        {
            std::env::var_os("COMSPEC").map_or_else(
                || PathBuf::from(r"C:\Windows\System32\cmd.exe"),
                PathBuf::from,
            )
        }
        #[cfg(not(windows))]
        {
            PathBuf::from("/bin/sh")
        }
    };
    if !shell.is_file() {
        println!("SKIP a_child_that_is_not_a_server...: no shell interpreter at {shell:?}");
        return;
    }
    let (flag, script) = if cfg!(windows) {
        (
            "/C".to_owned(),
            "echo not-a-mcp-server 1>&2 & exit 7".to_owned(),
        )
    } else {
        (
            "-c".to_owned(),
            "echo not-a-mcp-server 1>&2; exit 7".to_owned(),
        )
    };
    let spec = McpLaunchSpec {
        program: shell,
        args: vec![flag, script],
        env: Vec::new(),
        working_dir: None,
        // The shortest sensible bound, so a child that hung would fail the test quickly rather than after a
        // minute of the harness waiting.
        startup_timeout_ms: 1_000,
    };

    let refusal = discover_stdio_server(&spec, SERVER_NAME)
        .await
        .expect_err("a program that is not an MCP server must be refused");
    match &refusal {
        McpStartupRefusal::StartupFailed { diagnostics, .. }
        | McpStartupRefusal::TimedOut { diagnostics }
        | McpStartupRefusal::ListingFailed { diagnostics, .. } => {
            assert!(
                diagnostics.contains("not-a-mcp-server"),
                "the refusal must carry the child's stderr, got {diagnostics:?}"
            );
        }
        other => panic!("expected a failure carrying diagnostics, got {other:?}"),
    }
    assert!(
        !refusal.is_permanent(),
        "a program that ran and exited may work on a later attempt"
    );
}
