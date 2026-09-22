//! End-to-end proof that the daemon actually serves and drains.
//!
//! A unit test can prove the router and the startup steps in isolation, but only
//! a real socket proves that `serve_until` binds, answers, and drains. This test
//! is the executable form of "the daemon serves".
//!
//! `expect` is allowed here because integration tests are a separate crate that
//! the workspace `allow-expect-in-tests` option does not classify as tests; a
//! panic in this file is a test failure, not a runtime hazard.
#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use jarvis_application::repository::conversation::{ConversationRepository as _, NewConversation};
use jarvis_application::repository::run::{
    EventVisibility, NewActivityEvent, NewRun, RunRepository as _, RunWrite, run_received_event,
};
use jarvis_domain::ids::{ConversationId, PrincipalId, RunId, WorkspaceId};
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
use jarvis_domain::time::UtcTimestamp;
use jarvis_infrastructure::auth::{ClientCredentialPath, ClientRegistry};
use jarvis_infrastructure::daemon::{DaemonConfig, RunningDaemon, start};
use jarvis_infrastructure::paths::ProfilePaths;
use jarvis_infrastructure::storage::Database;
use jarvis_infrastructure::storage::repositories::SqliteRepositories;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use uuid::Uuid;

fn profile_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("jarvis-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

/// Starts a daemon against `config`, enrolling an owner client if the profile has none.
///
/// A helper rather than an inline block because the restart test starts three daemons on
/// one profile, and the enrollment step is identical each time.
async fn start_daemon(
    config: &DaemonConfig,
    destination: &ClientCredentialPath,
    instance_tag: &str,
    started_at: &str,
) -> RunningDaemon {
    let (clients, _created) = ClientRegistry::new()
        .into_enrolled(destination)
        .expect("enrollment");
    start(
        config,
        clients,
        format!("0195f4e8-7f6a-7c21-8ab5-4f0f80fd{instance_tag}"),
        started_at.to_owned(),
    )
    .await
    .expect("startup succeeds")
}

/// Writes a run into the daemon's durable database and drives it to a mid-flight state,
/// as a daemon killed at that moment would have left it.
///
/// The run is written directly through the repository rather than through the API, so the
/// database ends up in the exact state the recovery pass is meant to find. Going through
/// the API would produce a run the daemon had already settled.
async fn write_interrupted_run(config: &DaemonConfig) -> (WorkspaceId, RunId) {
    let uuid = |value: u128| Uuid::from_u128(value);
    let at = UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid");
    let workspace = WorkspaceId::from_uuid(uuid(1));
    let conversation = ConversationId::from_uuid(uuid(2));
    let run_id = RunId::from_uuid(uuid(3));
    let principal = PrincipalId::from_uuid(uuid(4));

    let database = Database::open(&config.database_path)
        .await
        .expect("the durable database reopens");
    let repositories = SqliteRepositories::new(database.pool().clone());
    repositories
        .create_conversation(
            NewConversation::new(
                conversation,
                workspace,
                principal,
                None,
                "cli".to_owned(),
                at,
            )
            .expect("valid"),
        )
        .await
        .expect("conversation created");
    repositories
        .create(
            NewRun::new(
                run_id,
                workspace,
                conversation,
                principal,
                Some("objective-1".to_owned()),
                at,
            )
            .expect("valid"),
            run_received_event(run_id, at),
        )
        .await
        .expect("run created");
    repositories
        .transition(
            workspace,
            RunWrite::new(
                &RunTransition::new(
                    RunState::Received,
                    RunState::ContextBuilding,
                    RunVersion::FIRST,
                    TransitionActor::Controller,
                    TransitionReason::new("step").expect("valid"),
                    at,
                ),
                NewActivityEvent {
                    run_id,
                    sequence: 2,
                    event_type: "run.context_building".to_owned(),
                    payload_json: None,
                    visibility: EventVisibility::Public,
                    occurred_at: at,
                },
            ),
        )
        .await
        .expect("the run advances");
    (workspace, run_id)
}

/// Sends one minimal HTTP/1.1 request over a fresh connection.
async fn request(port: u16, target: &str) -> String {
    request_with_host(port, target, &format!("127.0.0.1:{port}")).await
}

/// Sends one request with an explicit `Host`, and `None` to omit the header.
///
/// The header is written as raw text rather than through a builder so the test can
/// produce shapes a builder cannot: a missing `Host`, and two `Host` headers.
async fn request_with_host(port: u16, target: &str, host: &str) -> String {
    let host_line = format!("Host: {host}\r\n");
    raw_request(port, target, &host_line).await
}

/// Sends one request with the given raw header block.
async fn raw_request(port: u16, target: &str, headers: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the daemon");
    let request = format!("GET {target} HTTP/1.1\r\n{headers}Connection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    String::from_utf8_lossy(&response).into_owned()
}

#[tokio::test]
async fn the_daemon_answers_health_over_a_real_socket_and_drains() {
    let root = profile_root("serve");
    let paths = ProfilePaths::portable(&root);
    paths.ensure_directories().expect("profile dirs");
    let config = DaemonConfig::from_profile(&paths);

    let destination = ClientCredentialPath::in_config_dir(paths.config_dir());
    let (clients, _created) = ClientRegistry::new()
        .into_enrolled(&destination)
        .expect("enrollment");

    let daemon = start(
        &config,
        clients,
        "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
        "2026-09-21T00:00:00Z".to_owned(),
    )
    .await
    .expect("startup succeeds");

    let port = daemon.local_addr().expect("bound address").port();

    // `serve_until` must be driven for the daemon to accept connections, so the
    // requests and the serve loop run concurrently. This is the shape a real
    // process has: the serve future is the main loop.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
        let _ = shutdown_rx.await;
    };

    let server = tokio::spawn(async move {
        let drained = daemon.serve_until(shutdown, Duration::from_secs(5)).await;
        (drained, config.discovery_path())
    });

    // A real request reaches the real surface.
    let response = request(port, "/health/live").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains(r#"{"status":"live"}"#), "{response}");

    // Readiness is observable over the wire too.
    let ready = request(port, "/health/ready").await;
    assert!(ready.starts_with("HTTP/1.1 200"), "{ready}");
    assert!(ready.contains(r#"{"status":"ready"}"#), "{ready}");

    // Draining stops admission and unpublishes discovery.
    shutdown_tx.send(()).expect("signal shutdown");
    let (drained, discovery_path) = server.await.expect("serve task joins");
    assert!(drained, "drain must complete within its bound");
    assert!(
        !discovery_path.exists(),
        "discovery must be removed after drain",
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_request_addressed_to_another_host_is_refused_over_a_real_socket() {
    // Abuse case `AB-004` (DNS rebinding) and threat `THR-009`: a malicious page
    // resolves its own name to `127.0.0.1` and posts to the local API. The browser
    // sends the attacker's name in `Host`, so refusing a `Host` that is not the
    // daemon's own authority is what stops it — and the bearer credential is
    // missing as well, so the two controls are independent.
    //
    // The router test asserts the middleware; this proves the control holds on the
    // wire, where a real client chooses the `Host` and where a proxy would rewrite
    // it. A loopback socket accepts a connection addressed to any name that
    // resolves to loopback, so accepting the connection is not the same as
    // accepting the request.
    let root = profile_root("host-reject");
    let paths = ProfilePaths::portable(&root);
    paths.ensure_directories().expect("profile dirs");
    let config = DaemonConfig::from_profile(&paths);

    let destination = ClientCredentialPath::in_config_dir(paths.config_dir());
    let (clients, _created) = ClientRegistry::new()
        .into_enrolled(&destination)
        .expect("enrollment");

    let daemon = start(
        &config,
        clients,
        "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e0b".to_owned(),
        "2026-09-21T00:00:00Z".to_owned(),
    )
    .await
    .expect("startup succeeds");

    let port = daemon.local_addr().expect("bound address").port();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
        let _ = shutdown_rx.await;
    };
    let server = tokio::spawn(async move {
        let drained = daemon.serve_until(shutdown, Duration::from_secs(5)).await;
        (drained, config.discovery_path())
    });

    // The daemon's own authority is accepted, which is what makes the refusals
    // below attributable to the `Host` rather than to a broken surface.
    let ok = request(port, "/health/live").await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");

    for (label, headers) in [
        (
            "localhost instead of the bound address",
            "Host: localhost\r\n".to_owned(),
        ),
        ("a non-loopback name", "Host: evil.example\r\n".to_owned()),
        (
            "a loopback address with the wrong port",
            format!("Host: 127.0.0.1:{}\r\n", port + 1),
        ),
        // The smuggling shape: a valid authority and a different one, so whatever
        // picks "the first" disagrees with whatever picks "the last".
        (
            "two Host headers",
            format!("Host: 127.0.0.1:{port}\r\nHost: evil.example\r\n"),
        ),
        ("no Host at all", String::new()),
    ] {
        let response = raw_request(port, "/health/live", &headers).await;
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "{label} must be refused, got: {response}",
        );
        assert!(response.contains("api.host_not_allowed"), "{response}");
    }

    shutdown_tx.send(()).expect("signal shutdown");
    let _ = server.await.expect("serve task joins");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn the_daemon_binds_loopback_only_over_a_real_socket() {
    let root = profile_root("loopback");
    let paths = ProfilePaths::portable(&root);
    paths.ensure_directories().expect("profile dirs");
    let config = DaemonConfig::from_profile(&paths);

    let destination = ClientCredentialPath::in_config_dir(paths.config_dir());
    let (clients, _created) = ClientRegistry::new()
        .into_enrolled(&destination)
        .expect("enrollment");

    let daemon = start(
        &config,
        clients,
        "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e0a".to_owned(),
        "2026-09-21T00:00:00Z".to_owned(),
    )
    .await
    .expect("startup succeeds");

    let address = daemon.local_addr().expect("bound address");
    // The published discovery names the loopback authority, never a wildcard.
    let record = jarvis_protocol::DiscoveryFile::parse(
        &std::fs::read(config.discovery_path()).expect("discovery exists"),
    )
    .expect("valid discovery");
    assert!(
        record.base_url.starts_with("http://127.0.0.1:"),
        "{}",
        record.base_url,
    );
    assert!(address.ip().is_loopback());

    daemon.begin_drain().expect("drain");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_restart_settles_the_run_the_previous_daemon_left_mid_flight() {
    // The executable form of the contract's restart rule: a run left non-terminal by a
    // daemon that stopped must be terminal after the next daemon starts, before that
    // daemon reports ready. This is the only check that exercises the real startup
    // order — migrations, then recovery, then readiness — against a real database file
    // that outlives the process that wrote it.
    use jarvis_domain::run::state::RunState;

    let root = profile_root("recovery");
    let paths = ProfilePaths::portable(&root);
    paths.ensure_directories().expect("profile dirs");
    let config = DaemonConfig::from_profile(&paths);
    let destination = ClientCredentialPath::in_config_dir(paths.config_dir());

    // A first start, so migrations have run and the schema exists. Then it drains,
    // leaving a database a later process will reopen.
    let first = start_daemon(&config, &destination, "7e11", "2026-09-21T00:00:00Z").await;
    // A fresh profile recovers nothing, which is the baseline the second start is
    // compared against.
    assert!(first.recovery().is_empty(), "a clean profile has no work");
    first.begin_drain().expect("drain");
    drop(first);

    let (workspace, run_id) = write_interrupted_run(&config).await;

    // The second start, which must settle the run the first one left.
    let second = start_daemon(&config, &destination, "7e12", "2026-09-21T00:01:00Z").await;

    // Reported, so the operator is told rather than left to poll a run that never ends.
    let recovery = second.recovery();
    assert_eq!(recovery.abandoned, 1, "the interrupted run is abandoned");
    assert_eq!(recovery.parked, 0);

    // Readiness is only reported after recovery, so a ready daemon already has the run
    // settled. Asserting the flag here is what ties "ready" to "recovered".
    assert!(
        second.readiness().is_ready(),
        "readiness must follow recovery",
    );

    // And the durable state is terminal, read back through a fresh connection.
    let database = Database::open(&config.database_path)
        .await
        .expect("reopens");
    let repositories = SqliteRepositories::new(database.pool().clone());
    let settled = repositories
        .load(workspace, run_id)
        .await
        .expect("the run loads");
    assert_eq!(
        settled.state,
        RunState::Failed,
        "a restart must settle an interrupted run, not leave it non-terminal",
    );
    assert!(settled.is_terminal());

    // A third start finds nothing, because the second one closed the run.
    second.begin_drain().expect("drain");
    drop(second);
    let third = start_daemon(&config, &destination, "7e13", "2026-09-21T00:02:00Z").await;
    assert!(
        third.recovery().is_empty(),
        "recovery must be idempotent across restarts: {:?}",
        third.recovery(),
    );
    third.begin_drain().expect("drain");

    let _ = std::fs::remove_dir_all(&root);
}
