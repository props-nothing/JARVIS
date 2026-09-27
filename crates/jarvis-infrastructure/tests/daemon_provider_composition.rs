//! Proof that a configured model provider is what a real run actually calls.
//!
//! The adapter's own socket test proves the *transport*: that a chunked SSE body becomes a normalized
//! stream. What it cannot prove is composition — that the daemon a client reaches is wired to that
//! adapter rather than to the scripted provider, that a route selected from the adapter's served
//! models reaches the wire request, and that the answer is durable. Those are properties of the
//! assembled product, and they live on the far side of the HTTP surface, the policy service, and the
//! startup sequence.
//!
//! So this file starts a **real daemon** over a **real socket**, points it at a **local fake
//! OpenAI-compatible server**, creates a run through the real control API, and reads the run back.
//! The fake server records the request body it received, so the test can check what the daemon
//! actually sent rather than what a mock was told to expect.
//!
//! Everything binds `127.0.0.1`, and the adapter's own loopback rule is what makes that the only
//! possibility rather than a convention — so nothing here can reach a network.

// This is a `tests/*.rs` integration crate, which `clippy.toml`'s test allowances do not recognise —
// a trap recorded in this repository — so the test-only allowances are declared here.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use jarvis_infrastructure::auth::{ClientCredentialPath, ClientRegistry};
use jarvis_infrastructure::config::{
    Config, ConfigOverrides, MapSecretResolver, ProviderSection, config_file_path,
};
use jarvis_infrastructure::daemon::{DaemonConfig, RunningDaemon, start};
use jarvis_infrastructure::paths::ProfilePaths;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

fn profile_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("jarvis-prov-{tag}-{}", std::process::id()));
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

/// Starts a daemon whose provider is an adapter pointed at `port`.
///
/// Returns the daemon, a credential, and the model references the composed provider serves — the last
/// so a test can assert *which* provider was wired without reaching into the daemon's internals.
async fn start_with_adapter(
    root: &std::path::Path,
    port: u16,
    model: &str,
) -> (RunningDaemon, String, Vec<String>) {
    let paths = ProfilePaths::portable(root);
    paths.ensure_directories().expect("profile dirs");
    let destination = ClientCredentialPath::in_config_dir(paths.config_dir());
    let (clients, credential) = ClientRegistry::new()
        .into_enrolled(&destination)
        .expect("enrollment");
    let token = credential.expect("a fresh profile issues a credential");

    // The provider is composed through the **production** path: a configuration document and a secret
    // resolver, exactly as `jarvisd` composes it. Building the adapter directly would prove the
    // daemon can hold one, not that an operator's document reaches it.
    let config = Config::from_toml(&format!(
        "schema_version = 2\n\n[model]\npolicy_id = \"default\"\napi_key_ref = \"env:JARVIS_MODEL_KEY\"\n\n\
         [model.provider]\nid = \"local.fake\"\nhost = \"127.0.0.1\"\nport = {port}\nmodels = [\"{model}\"]\n"
    ))
    .expect("the fixture document parses");
    let secrets = MapSecretResolver::new();
    secrets.insert("JARVIS_MODEL_KEY", "test-key-not-a-real-credential");
    let provider =
        jarvis_infrastructure::model_providers::resolve(&config, &secrets).expect("composes");

    let daemon_config = DaemonConfig::from_profile(&paths).with_provider(provider);
    let models = daemon_config.provider_models();
    let daemon = start(
        &daemon_config,
        clients,
        "0195f4e8-7f6a-7c21-8ab5-4f0f80fd9001".to_owned(),
        "2026-09-27T00:00:00Z".to_owned(),
    )
    .await
    .expect("startup succeeds");
    (daemon, token, models)
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

/// Authenticated headers for a run request.
fn run_headers(token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {token}")),
        ("Jarvis-Api-Version", "1".to_owned()),
        ("Content-Type", "application/json".to_owned()),
        ("Idempotency-Key", format!("key-{}", uuid::Uuid::now_v7())),
    ]
}

#[tokio::test]
async fn a_run_through_the_daemon_reaches_the_configured_provider_and_is_durable() {
    // The end-to-end claim: an operator configures an OpenAI-compatible endpoint, a client creates a
    // run, and the answer the provider streamed is what the run recorded. Every layer in between —
    // the layering of configuration, the composition of the adapter, the route selection, the HTTP
    // exchange, and the persistence — has to be correct for this to hold.
    let root = profile_root("composed");
    let answer = "composed provider answer";
    let (port, seen) = fake_provider(answer, 1).await;
    let (daemon, token, models) = start_with_adapter(&root, port, "fake-model").await;
    let daemon_port = daemon.local_addr().expect("bound").port();

    // The provider the daemon actually holds is the adapter, not the scripted fallback. Asserted
    // directly so a composition bug is named here rather than surfacing as a confusing run failure.
    assert_eq!(
        models,
        vec!["local.fake/fake-model".to_owned()],
        "the daemon must be wired to the configured adapter",
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
        let _ = shutdown_rx.await;
    };
    let server = tokio::spawn(async move {
        daemon
            .serve_until(shutdown, std::time::Duration::from_secs(5))
            .await
    });

    let create_body = r#"{"input":{"type":"text","text":"hello"},"runtime":"jarvis-native"}"#;
    let (status, body) = http(
        daemon_port,
        "POST",
        "/api/v1/runs",
        &run_headers(&token),
        create_body,
    )
    .await;
    assert_eq!(status, "HTTP/1.1 202 Accepted", "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("the body is JSON");
    let run_id = parsed["run_id"].as_str().expect("a run id").to_owned();

    // The run is driven asynchronously, so the read is polled until it reaches a terminal state. A
    // bounded number of attempts keeps a hang from becoming a hung test.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut final_state = String::new();
    while std::time::Instant::now() < deadline {
        let (status, body) = http(
            daemon_port,
            "GET",
            &format!("/api/v1/runs/{run_id}"),
            &[
                ("Jarvis-Api-Version", "1".to_owned()),
                ("Authorization", format!("Bearer {token}")),
            ],
            "",
        )
        .await;
        assert_eq!(status, "HTTP/1.1 200 OK", "{body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("the body is JSON");
        let state = parsed["state"].as_str().expect("a state").to_owned();
        if state == "completed" || state == "failed" || state == "cancelled" {
            final_state = state;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        final_state, "completed",
        "a run against a healthy provider must complete, not fail",
    );

    // The request the fake provider received is the proof of what the daemon sent. The routed model is
    // the adapter's own, and the body pins one streamed answer — a run that reached the scripted
    // provider would have produced neither.
    let received = seen.lock().await;
    let received = received
        .first()
        .expect("the provider was called exactly once");
    assert!(
        received.starts_with("POST /chat/completions HTTP/1.1"),
        "{received}",
    );
    assert!(
        received.contains("\"model\":\"fake-model\""),
        "the request must name the routed model: {received}",
    );
    assert!(
        received.contains("Authorization: Bearer test-key-not-a-real-credential"),
        "the credential travels as a header: {received}",
    );
    assert!(received.contains("\"stream\":true"), "{received}");

    // And the answer is durable: the run's own events carry the text the provider streamed, which is
    // what a client replaying the stream after the fact would read.
    let (status, events) = http(
        daemon_port,
        "GET",
        &format!("/api/v1/runs/{run_id}/events"),
        &[
            ("Jarvis-Api-Version", "1".to_owned()),
            ("Authorization", format!("Bearer {token}")),
            ("Accept", "text/event-stream".to_owned()),
        ],
        "",
    )
    .await;
    assert_eq!(status, "HTTP/1.1 200 OK", "{events}");
    assert!(
        events.contains(answer),
        "the provider's answer must be durable in the run's events: {events}",
    );

    shutdown_tx.send(()).expect("signal shutdown");
    server.await.expect("serve task joins");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn an_unconfigured_daemon_composes_the_scripted_provider() {
    // The default must stay the deterministic one: a profile that names no endpoint must not reach
    // the network, and must say so in its model id so an operator can tell which source served a run.
    let root = profile_root("unconfigured");
    let paths = ProfilePaths::portable(&root);
    paths.ensure_directories().expect("profile dirs");
    let config = DaemonConfig::from_profile(&paths);
    assert!(
        config.provider_models().is_empty(),
        "no provider was configured, so the config names none",
    );

    // And the composition helper the daemon calls resolves an empty document to the scripted provider.
    let document = Config::from_toml("schema_version = 2").expect("minimal document");
    let provider =
        jarvis_infrastructure::model_providers::resolve(&document, &MapSecretResolver::new())
            .expect("composes");
    assert_eq!(
        provider
            .models()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["scripted.local/scripted-echo".to_owned()],
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_configuration_the_adapter_refuses_stops_the_daemon_from_being_wired_to_it() {
    // A configured-but-invalid endpoint is a **startup refusal**, not a silent fallback to the
    // scripted provider. Falling back would let an operator see successful runs while their real
    // endpoint was never contacted — the failure this test exists to prevent.
    let section = ProviderSection {
        id: "local.fake".to_owned(),
        host: "api.example.com".to_owned(),
        port: 443,
        models: vec!["fake-model".to_owned()],
    };
    let mut document =
        Config::from_toml("schema_version = 2\n\n[model]\npolicy_id = \"default\"\n")
            .expect("document");
    document.model.provider = Some(section);
    document.model.api_key_ref = Some(
        jarvis_infrastructure::config::SecretReference::env("JARVIS_MODEL_KEY").expect("valid"),
    );
    let secrets = MapSecretResolver::new();
    secrets.insert("JARVIS_MODEL_KEY", "test-key");
    let error = jarvis_infrastructure::model_providers::resolve(&document, &secrets)
        .err()
        .expect("a non-loopback endpoint must be refused");
    assert_eq!(error.code(), "model.adapter_endpoint_not_loopback");

    // The layering is exercised too, so an operator's actual path is covered: the document is what a
    // file holds, and the environment layer is applied on top of it.
    let root = profile_root("refused");
    let paths = ProfilePaths::portable(&root);
    paths.ensure_directories().expect("profile dirs");
    let path = config_file_path(paths.config_dir());
    document.save(&path).expect("the document saves");
    let layered = Config::layered(Some(&path), [], &ConfigOverrides::default()).expect("layers");
    assert_eq!(
        layered.model.provider.expect("the provider survives").host,
        "api.example.com",
    );
    let _ = std::fs::remove_dir_all(&root);
}
