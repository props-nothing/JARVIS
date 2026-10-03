//! The supervisor against **real spawned children**: a server that dies after a successful start is
//! relaunched under a bound, and only if it still offers exactly the tools it did.
//!
//! The child is "killed" by cancelling its session's worker, which closes the transport and so ends the child
//! the same way a crash does from the executor's point of view: `is_closed` turns true and a call is refused.
//! A skipped test prints why, because this target alone does not build the fixture example.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::tool_call::{ToolExecutionError, ToolExecutionRequest, ToolExecutor};
use jarvis_domain::tool::call::ToolArguments;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::registry::ToolRegistry;
use jarvis_infrastructure::config::mcp::{McpSection, McpServerSection};
use jarvis_infrastructure::config::secret::{MapSecretResolver, SecretReference};
use jarvis_infrastructure::mcp::composition::{McpComposition, compose_declared_servers};
use jarvis_infrastructure::mcp::executor::McpToolExecutor;
use jarvis_infrastructure::mcp::process::DEFAULT_MCP_STARTUP_TIMEOUT_MS;
use jarvis_infrastructure::mcp::supervisor::{
    BUDGET_EXHAUSTED_CODE, IDENTITY_CHANGED_CODE, McpRestartPolicy, McpSupervisionEvent,
    McpSupervisor,
};
use tokio::time::Instant;

const FIXTURE_ENV: &str = "JARVIS_MCP_FIXTURE";
const FIXTURE_STANDARD: &str = "standard";
const FIXTURE_CWD: &str = "cwd";
const FIXTURE_CALL_TEXT: &str = "fixture result for read_file";
const SERVER: &str = "acme-files";
const CALL_TIMEOUT_MS: u64 = 10_000;

fn program_or_skip(test: &str) -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let profile = current.parent()?.parent()?;
    let name = if cfg!(windows) {
        "mcp_fixture_server.exe"
    } else {
        "mcp_fixture_server"
    };
    let candidate = profile.join("examples").join(name);
    if candidate.is_file() {
        Some(candidate)
    } else {
        println!("SKIP {test}: examples/mcp_fixture_server is not built for this profile");
        None
    }
}

fn section(program: &std::path::Path) -> McpSection {
    McpSection {
        servers: vec![McpServerSection {
            name: SERVER.to_owned(),
            program: program.to_string_lossy().into_owned(),
            args: Vec::new(),
            env: BTreeMap::from([(
                FIXTURE_ENV.to_owned(),
                SecretReference::Env("JARVIS_FIXTURE_ACME_FILES".to_owned()),
            )]),
            enabled: true,
            working_directory: None,
            startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
        }],
    }
}

fn secrets(fixture: &str) -> MapSecretResolver {
    let resolver = MapSecretResolver::new();
    resolver.insert("JARVIS_FIXTURE_ACME_FILES".to_owned(), fixture);
    resolver
}

fn policy(max_consecutive_failures: u32) -> McpRestartPolicy {
    McpRestartPolicy {
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(8),
        max_consecutive_failures,
        stable_window: Duration::from_secs(60),
    }
}

/// Composes the standard fixture and returns what a pass needs.
async fn composed(
    program: &std::path::Path,
) -> (
    McpComposition,
    McpSupervisor,
    Vec<(String, Arc<McpToolExecutor>)>,
) {
    let declarations = section(program).enabled_declarations().expect("valid");
    let mut registry = ToolRegistry::new();
    let composition =
        compose_declared_servers(&mut registry, &declarations, &secrets(FIXTURE_STANDARD))
            .await
            .expect("valid");
    assert!(composition.is_clean(), "{:?}", composition.refused());
    let servers = composition
        .servers()
        .iter()
        .map(|server| (SERVER.to_owned(), Arc::clone(server.executor())))
        .collect();
    (
        composition,
        McpSupervisor::new(declarations, policy(5)),
        servers,
    )
}

/// Ends the child's session and waits until the executor reports it closed.
async fn kill(executor: &McpToolExecutor) {
    executor.session().cancellation_token().cancel();
    for _ in 0..200 {
        if executor.is_closed() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the session did not close");
}

async fn call(
    executor: &McpToolExecutor,
    identity: &jarvis_domain::tool::identity::ToolIdentity,
) -> Result<jarvis_domain::tool::call::ToolResultBody, ToolExecutionError> {
    let arguments = ToolArguments::new("{}").expect("usable text");
    let request = ToolExecutionRequest {
        identity,
        display_name: "read_file",
        arguments: &arguments,
        started_at: jarvis_domain::time::UtcTimestamp::parse("2026-10-03T00:00:00Z")
            .expect("valid"),
        timeout_ms: CALL_TIMEOUT_MS,
    };
    executor.execute(request, &CancellationScope::new()).await
}

#[tokio::test]
async fn a_server_that_died_after_a_successful_start_is_relaunched_and_callable_again() {
    let Some(program) = program_or_skip(
        "a_server_that_died_after_a_successful_start_is_relaunched_and_callable_again",
    ) else {
        return;
    };
    let (composition, mut supervisor, servers) = composed(&program).await;
    let executor = &servers[0].1;
    let identity = composition.servers()[0].admitted()[0].clone();

    // A healthy server is left alone: nothing to report, and the same session afterwards.
    let now = Instant::now();
    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), now)
        .await;
    assert!(events.is_empty(), "{events:?}");

    kill(executor).await;
    // The dead server refuses dispatch with the settled, safe class: this is the state being repaired.
    assert_eq!(
        call(executor, &identity).await.expect_err("dead"),
        ToolExecutionError::Failed(ToolErrorClass::Unavailable)
    );

    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), now)
        .await;
    assert_eq!(
        events,
        vec![McpSupervisionEvent::Restarted {
            server: SERVER.to_owned(),
            attempt: 1
        }]
    );
    assert!(!executor.is_closed());
    let body = call(executor, &identity).await.expect("callable again");
    assert!(
        serde_json::to_string(&body)
            .expect("serializes")
            .contains(FIXTURE_CALL_TEXT)
    );

    let _ = composition.shutdown().await;
}

#[tokio::test]
async fn a_server_that_returns_with_different_tools_is_quarantined_not_substituted() {
    let Some(program) = program_or_skip(
        "a_server_that_returns_with_different_tools_is_quarantined_not_substituted",
    ) else {
        return;
    };
    let (composition, mut supervisor, servers) = composed(&program).await;
    let executor = &servers[0].1;
    kill(executor).await;

    // The same declaration, but the child now selects the fixture that offers a different tool.
    let now = Instant::now();
    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_CWD), now)
        .await;
    assert_eq!(
        events,
        vec![McpSupervisionEvent::Quarantined {
            server: SERVER.to_owned(),
            code: IDENTITY_CHANGED_CODE
        }]
    );
    assert!(
        executor.is_closed(),
        "the changed server must not be swapped in"
    );
    assert!(supervisor.is_quarantined(SERVER));

    // And it stays given up on, even with a healthy declaration and a due instant.
    let later = now + Duration::from_secs(3600);
    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), later)
        .await;
    assert!(events.is_empty(), "{events:?}");
    assert!(executor.is_closed());

    let _ = composition.shutdown().await;
}

#[tokio::test]
async fn a_server_that_keeps_dying_exhausts_its_budget_and_waits_between_attempts() {
    let Some(program) =
        program_or_skip("a_server_that_keeps_dying_exhausts_its_budget_and_waits_between_attempts")
    else {
        return;
    };
    let (composition, _, servers) = composed(&program).await;
    let declarations = section(&program).enabled_declarations().expect("valid");
    let mut supervisor = McpSupervisor::new(declarations, policy(1));
    let executor = &servers[0].1;

    kill(executor).await;
    let now = Instant::now();
    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), now)
        .await;
    assert!(matches!(
        events.as_slice(),
        [McpSupervisionEvent::Restarted { attempt: 1, .. }]
    ));

    // It dies again straight away. Before the delay has elapsed, nothing is attempted...
    kill(executor).await;
    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), now)
        .await;
    assert!(events.is_empty(), "{events:?}");

    // ...and once it has, the restart did not reset the budget, so the server is given up on.
    let due = now + Duration::from_secs(2);
    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), due)
        .await;
    assert_eq!(
        events,
        vec![McpSupervisionEvent::Quarantined {
            server: SERVER.to_owned(),
            code: BUDGET_EXHAUSTED_CODE
        }]
    );
    assert!(executor.is_closed());

    let _ = composition.shutdown().await;
}

#[tokio::test]
async fn a_restarted_server_that_stays_up_has_its_budget_restored() {
    let Some(program) = program_or_skip("a_restarted_server_that_stays_up_has_its_budget_restored")
    else {
        return;
    };
    let (composition, _, servers) = composed(&program).await;
    let declarations = section(&program).enabled_declarations().expect("valid");
    let mut supervisor = McpSupervisor::new(declarations, policy(1));
    let executor = &servers[0].1;

    kill(executor).await;
    let now = Instant::now();
    supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), now)
        .await;
    // Observed open after the stable window: the attempt count is forgotten.
    let stable = now + Duration::from_secs(61);
    supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), stable)
        .await;

    kill(executor).await;
    let events = supervisor
        .supervise_once(&servers, &secrets(FIXTURE_STANDARD), stable)
        .await;
    assert!(
        matches!(
            events.as_slice(),
            [McpSupervisionEvent::Restarted { attempt: 1, .. }]
        ),
        "a server that stayed up must get a fresh budget, got {events:?}"
    );

    let _ = composition.shutdown().await;
}
