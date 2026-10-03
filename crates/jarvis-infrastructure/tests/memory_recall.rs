//! The memory journey through a **real daemon**: remember through the API, ask, and see the memory in the
//! request the model provider actually received — then restart the daemon and see it again.
//!
//! This is `ACC-030` ("Remember Across Restart") for the slice that exists: a user asks JARVIS to remember a
//! preference, a later run's model input carries it, and a restart of the daemon does not lose it. The proof
//! is the **wire request** a fake OpenAI-compatible server records, not a mock's call count, so a defect
//! anywhere between the HTTP route, the store, the recall, the context budget, and the adapter fails here.
//!
//! The negative leg matters as much as the positive one: a run whose objective matches nothing must send
//! **no** memory, so the model is not handed every remembered line on every question.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use jarvis_infrastructure::auth::{ClientCredentialPath, ClientRegistry};
use jarvis_infrastructure::config::{Config, MapSecretResolver};
use jarvis_infrastructure::daemon::{DaemonConfig, RunningDaemon, start};
use jarvis_infrastructure::paths::ProfilePaths;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const MEMORY: &str = "Client proposals should be concise";

fn profile_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("jarvis-mem-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}
/// A fake OpenAI-compatible server that answers one request and records it.
///
/// Hand-written rather than an HTTP framework, for the same reason the adapter's own socket test is:
/// the properties under test are the wire bytes, and a framework that re-encoded them for us would
/// be exercising the framework. It accepts up to `connections` connections, because a run makes one
/// call and a second would be a defect rather than a scenario.
async fn fake_provider(
    answer: &str,
    connections: usize,
) -> (u16, Arc<tokio::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("an ephemeral loopback port binds");
    let port = listener.local_addr().expect("the address is known").port();
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let body = chunked_sse(answer);
    tokio::spawn(async move {
        for _ in 0..connections {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                // The head is read first so the recorded request is the head **and** the body; the
                // body length comes from `Content-Length`, so this reads exactly the request rather
                // than guessing at a boundary.
                let mut received = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let read = match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    received.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&received);
                    if let Some(length) = content_length(&text)
                        && text
                            .split_once("\r\n\r\n")
                            .is_some_and(|(_, body)| body.len() >= length)
                    {
                        break;
                    }
                    if received.len() > 1 << 20 {
                        break;
                    }
                }
                recorded
                    .lock()
                    .await
                    .push(String::from_utf8_lossy(&received).into_owned());
                let _ = socket.write_all(&body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (port, seen)
}

/// Reads the declared `Content-Length`, if the head has arrived.
fn content_length(head: &str) -> Option<usize> {
    head.lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|value| value.trim().parse().ok())
}

/// Builds a chunked SSE response carrying one streamed answer, as a real server sends it.
fn chunked_sse(answer: &str) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut body = String::new();
    let frames = [
        r#"{"id":"c-1","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#
            .to_owned(),
        format!(
            r#"{{"id":"c-1","choices":[{{"index":0,"delta":{{"content":"{answer}"}},"finish_reason":null}}]}}"#
        ),
        r#"{"id":"c-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}"#
            .to_owned(),
    ];
    for frame in &frames {
        let payload = format!("data: {frame}\n\n");
        let _ = write!(body, "{:x}\r\n{payload}\r\n", payload.len());
    }
    body.push_str("0\r\n\r\n");
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    )
    .into_bytes()
}

/// Sends one HTTP/1.1 request and returns `(status_line, body)`.
async fn http(
    port: u16,
    method: &str,
    target: &str,
    headers: &[(&str, String)],
    body: &str,
) -> (String, String) {
    let authority = format!("127.0.0.1:{port}");
    let mut head =
        format!("{method} {target} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    for (name, value) in headers {
        use std::fmt::Write as _;
        let _ = write!(head, "{name}: {value}\r\n");
    }
    if !body.is_empty() {
        use std::fmt::Write as _;
        let _ = write!(head, "Content-Length: {}\r\n", body.len());
    }
    head.push_str("\r\n");

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the daemon");
    stream
        .write_all(format!("{head}{body}").as_bytes())
        .await
        .expect("write request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    let text = String::from_utf8_lossy(&response).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    (
        head.lines().next().unwrap_or_default().to_owned(),
        body.to_owned(),
    )
}

/// Starts a daemon over the profile at `root`, wired to a fake provider on `port`.
///
/// Called twice over one root to model a restart: the database persists, the process-local state does not.
async fn start_daemon(root: &std::path::Path, port: u16) -> (RunningDaemon, String) {
    let paths = ProfilePaths::portable(root);
    paths.ensure_directories().expect("profile dirs");
    let destination = ClientCredentialPath::in_config_dir(paths.config_dir());
    let (clients, credential) = ClientRegistry::new()
        .into_enrolled(&destination)
        .expect("enrollment");
    // A fresh profile issues a credential; a restarted one already holds it, so it is read back from the
    // owner-only file the way the CLI reads it.
    let token = credential.unwrap_or_else(|| {
        jarvis_infrastructure::auth::load_client_credential(&destination)
            .expect("the enrolled credential is on disk")
    });
    let config = Config::from_toml(&format!(
        "schema_version = 4\n\n[model]\npolicy_id = \"default\"\napi_key_ref = \"env:JARVIS_MODEL_KEY\"\n\n\
         [model.provider]\nid = \"local.fake\"\nhost = \"127.0.0.1\"\nport = {port}\nmodels = [\"fake-model\"]\n"
    ))
    .expect("the document parses");
    let secrets = MapSecretResolver::new();
    secrets.insert("JARVIS_MODEL_KEY", "test-key-not-a-real-credential");
    let provider =
        jarvis_infrastructure::model_providers::resolve(&config, &secrets).expect("composes");
    let daemon_config = DaemonConfig::from_profile(&paths).with_provider(provider);
    let daemon = start(
        &daemon_config,
        clients,
        "0195f4e8-7f6a-7c21-8ab5-4f0f80fd9002".to_owned(),
        "2026-10-03T00:00:00Z".to_owned(),
    )
    .await
    .expect("startup succeeds");
    (daemon, token)
}

fn headers(token: &str, with_key: bool) -> Vec<(&'static str, String)> {
    let mut list = vec![
        ("Authorization", format!("Bearer {token}")),
        ("Jarvis-Api-Version", "1".to_owned()),
        ("Content-Type", "application/json".to_owned()),
    ];
    if with_key {
        list.push(("Idempotency-Key", format!("key-{}", uuid::Uuid::now_v7())));
    }
    list
}

/// Creates a run and waits for it to finish, returning its final state.
async fn ask(port: u16, token: &str, text: &str) -> String {
    let body =
        format!(r#"{{"input":{{"type":"text","text":"{text}"}},"runtime":"jarvis-native"}}"#);
    let (status, response) = http(port, "POST", "/api/v1/runs", &headers(token, true), &body).await;
    assert_eq!(status, "HTTP/1.1 202 Accepted", "{response}");
    let run_id = serde_json::from_str::<serde_json::Value>(&response).expect("json")["run_id"]
        .as_str()
        .expect("a run id")
        .to_owned();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let (_, body) = http(
            port,
            "GET",
            &format!("/api/v1/runs/{run_id}"),
            &headers(token, false),
            "",
        )
        .await;
        let state = serde_json::from_str::<serde_json::Value>(&body).expect("json")["state"]
            .as_str()
            .expect("a state")
            .to_owned();
        if matches!(state.as_str(), "completed" | "failed" | "cancelled") {
            return state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    "timed out".to_owned()
}

/// Serves a daemon until the returned sender fires.
fn serve(
    daemon: RunningDaemon,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<bool>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        daemon
            .serve_until(
                async move {
                    let _ = rx.await;
                },
                std::time::Duration::from_secs(5),
            )
            .await
    });
    (tx, handle)
}

/// The first daemon's two runs: the matching question carried the memory, the unrelated one did not.
async fn assert_first_leg(seen: &Arc<tokio::sync::Mutex<Vec<String>>>) {
    let received = seen.lock().await;
    assert_eq!(received.len(), 2, "one provider call per run");
    let relevant = &received[0];
    assert!(
        relevant.contains(MEMORY),
        "a run whose question matches the memory must send it to the model: {relevant}"
    );
    assert!(
        relevant.contains("remembered earlier by the user"),
        "and label where it came from: {relevant}"
    );
    // The question is still the last thing the model is asked.
    let memory_at = relevant.find(MEMORY).expect("present");
    let question_at = relevant
        .find("Please draft the proposals for the client")
        .expect("the question is present");
    assert!(
        memory_at < question_at,
        "background first, the question last"
    );
    let unrelated = &received[1];
    assert!(
        !unrelated.contains(MEMORY),
        "a run whose question matches nothing must not be handed the memory: {unrelated}"
    );
}

#[tokio::test]
async fn a_remembered_preference_reaches_the_model_and_survives_a_restart() {
    let root = profile_root("journey");

    // ---- First daemon: remember, then ask two different questions.
    let (provider_port, seen) = fake_provider("done", 2).await;
    let (daemon, token) = start_daemon(&root, provider_port).await;
    let port = daemon.local_addr().expect("bound").port();
    let (stop, server) = serve(daemon);

    let (status, body) = http(
        port,
        "POST",
        "/api/v1/memories",
        &headers(&token, false),
        &format!(r#"{{"text":"{MEMORY}"}}"#),
    )
    .await;
    assert_eq!(status, "HTTP/1.1 201 Created", "{body}");

    assert_eq!(
        ask(port, &token, "Please draft the proposals for the client").await,
        "completed"
    );
    assert_eq!(
        ask(port, &token, "What is the capital of France").await,
        "completed"
    );

    assert_first_leg(&seen).await;
    stop.send(()).expect("signal");
    assert!(
        server.await.expect("joins"),
        "the first daemon drains cleanly"
    );

    // ---- Restart: a new process over the same profile still recalls it.
    let (provider_port, seen) = fake_provider("done again", 1).await;
    let (daemon, token) = start_daemon(&root, provider_port).await;
    let port = daemon.local_addr().expect("bound").port();
    let (stop, server) = serve(daemon);

    let (_, listing) = http(port, "GET", "/api/v1/memories", &headers(&token, false), "").await;
    assert!(
        listing.contains(MEMORY),
        "the memory survives a restart: {listing}"
    );
    assert_eq!(
        ask(port, &token, "Draft the client proposals").await,
        "completed"
    );
    assert!(
        seen.lock().await[0].contains(MEMORY),
        "and is recalled by a run after the restart"
    );

    // ---- Forgetting is effective for recall too.
    let id = serde_json::from_str::<serde_json::Value>(&listing).expect("json")["memories"][0]
        ["memory_id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let (status, _) = http(
        port,
        "DELETE",
        &format!("/api/v1/memories/{id}"),
        &headers(&token, false),
        "",
    )
    .await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    stop.send(()).expect("signal");
    assert!(server.await.expect("joins"));

    // ---- A third daemon: the forgotten memory is not recalled by a run that would have matched it.
    let (provider_port, seen) = fake_provider("last", 1).await;
    let (daemon, token) = start_daemon(&root, provider_port).await;
    let port = daemon.local_addr().expect("bound").port();
    let (stop, server) = serve(daemon);
    assert_eq!(
        ask(port, &token, "Draft the client proposals").await,
        "completed"
    );
    assert!(
        !seen.lock().await[0].contains(MEMORY),
        "a forgotten memory must not reach the model"
    );
    stop.send(()).expect("signal");
    assert!(server.await.expect("joins"));
    let _ = std::fs::remove_dir_all(&root);
}
