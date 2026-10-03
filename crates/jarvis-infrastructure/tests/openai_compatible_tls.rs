//! The OpenAI-compatible adapter's **TLS transport** against a real TLS server on a loopback socket.
//!
//! What these can falsify, and why each exists:
//!
//! - a certificate whose root is **not** trusted is rejected, and the credential is never written;
//! - a certificate for a **different name** is rejected even though its chain is trusted;
//! - a certificate from an **injected** root is accepted and a streamed answer arrives, with SNI and
//!   ALPN as the evidence note records them, and the credential only in a header;
//! - a peer that closes the TCP connection **without `close_notify`** mid-body is a failure, not a
//!   completion.
//!
//! The server is `tokio-rustls`'s acceptor over a certificate authority generated per test with
//! `rcgen`, so nothing is shared between tests and no certificate is committed. The adapter dials the
//! name `localhost`, which resolves to loopback, so no packet leaves the machine.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::model::ModelProvider as _;
use jarvis_domain::ids::{ModelCallId, RunId};
use jarvis_domain::model::identity::{EndpointClass, ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{
    CallLimits, ContentBlock, InputItem, InputItems, ModelCallRequest, ModelStreamEvent,
    ModelStreamEventKind, PortableSettings, Role, RouteRequirements,
};
use jarvis_infrastructure::model_providers::openai_compatible::{
    ConfigError, OpenAiCompatibleProvider,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const CANARY: &str = "tls-test-key-not-a-real-credential";

/// A test certificate authority and a leaf it issued.
struct Pki {
    ca_der: CertificateDer<'static>,
    leaf_der: CertificateDer<'static>,
    leaf_key: Vec<u8>,
}

/// Issues a leaf for `names` from a fresh authority.
fn pki_for(names: &[&str]) -> Pki {
    let ca_key = rcgen::KeyPair::generate().expect("a CA key generates");
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("valid");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "JARVIS test CA");
    let ca_cert = ca_params.self_signed(&ca_key).expect("the CA self-signs");
    let issuer = rcgen::Issuer::new(ca_params, ca_key);

    let leaf_key = rcgen::KeyPair::generate().expect("a leaf key generates");
    let leaf_params = rcgen::CertificateParams::new(
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .expect("valid names");
    let leaf = leaf_params
        .signed_by(&leaf_key, &issuer)
        .expect("the leaf is issued");
    Pki {
        ca_der: ca_cert.der().clone(),
        leaf_der: leaf.der().clone(),
        leaf_key: leaf_key.serialize_der(),
    }
}

fn roots_trusting(pki: &Pki) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(pki.ca_der.clone())
        .expect("the CA is a valid root");
    roots
}

fn server_config(pki: &Pki) -> Arc<rustls::ServerConfig> {
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![pki.leaf_der.clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pki.leaf_key.clone())),
    )
    .expect("the leaf and key match");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// What the TLS server observed of one connection.
#[derive(Debug, Default)]
struct Observed {
    handshake_completed: bool,
    sni: Option<String>,
    alpn: Option<Vec<u8>>,
    request: Vec<u8>,
}

/// How the server ends the response.
#[derive(Clone, Copy)]
enum Ending {
    /// Writes the body and sends `close_notify`.
    Clean,
    /// Drops the TCP connection with no `close_notify`, leaving the body unfinished.
    Abrupt,
}

async fn serve_tls_once(
    config: Arc<rustls::ServerConfig>,
    response: Vec<u8>,
    ending: Ending,
) -> (u16, tokio::task::JoinHandle<Observed>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("an ephemeral loopback port binds");
    let port = listener.local_addr().expect("known").port();
    let handle = tokio::spawn(async move {
        let mut observed = Observed::default();
        let (tcp, _) = listener.accept().await.expect("one connection arrives");
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        let Ok(mut tls) = acceptor.accept(tcp).await else {
            return observed;
        };
        observed.handshake_completed = true;
        {
            let (_, connection) = tls.get_ref();
            observed.sni = connection.server_name().map(str::to_owned);
            observed.alpn = connection.alpn_protocol().map(<[u8]>::to_vec);
        }
        let mut buffer = [0_u8; 4096];
        loop {
            let read = match tls.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            observed.request.extend_from_slice(&buffer[..read]);
            if observed
                .request
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
        }
        let _ = tls.write_all(&response).await;
        let _ = tls.flush().await;
        match ending {
            Ending::Clean => {
                let _ = tls.shutdown().await;
            }
            // Dropping a rustls stream does not send `close_notify`; only the TCP connection ends.
            Ending::Abrupt => drop(tls),
        }
        observed
    });
    (port, handle)
}

fn chunked_sse(events: &[&str]) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut body = String::new();
    for event in events {
        let payload = format!("data: {event}\n\n");
        let _ = write!(body, "{:x}\r\n{payload}\r\n", payload.len());
    }
    body.push_str("0\r\n\r\n");
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    )
    .into_bytes()
}

const ANSWER: [&str; 3] = [
    r#"{"id":"c1","choices":[{"index":0,"delta":{"content":"Hello over TLS"},"finish_reason":null}]}"#,
    r#"{"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":3,"total_tokens":6}}"#,
    "[DONE]",
];

fn provider_with(port: u16, roots: rustls::RootCertStore) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new_tls_with_roots(
        ProviderId::parse("cloud.example").expect("valid"),
        "localhost",
        port,
        CANARY,
        vec![ModelId::parse("test-model").expect("valid")],
        roots,
    )
    .expect("the fixture configures")
    .with_timeout(std::time::Duration::from_secs(10))
}

fn request_for(provider: &OpenAiCompatibleProvider) -> ModelCallRequest {
    let model: ModelRef = provider.models()[0].clone();
    ModelCallRequest {
        call_id: ModelCallId::from_uuid(uuid::Uuid::from_u128(9)),
        run_id: RunId::from_uuid(uuid::Uuid::from_u128(5)),
        model,
        route_requirements: RouteRequirements::text(),
        input: InputItems::new(vec![InputItem::Message {
            role: Role::User,
            blocks: vec![ContentBlock::Text {
                text: "hello".to_owned(),
            }],
        }])
        .expect("valid"),
        tools: Vec::new(),
        tool_offers: Vec::new(),
        output_schema: None,
        settings: PortableSettings::default(),
        limits: CallLimits {
            deadline: None,
            max_output_tokens: None,
            max_cost_microunits: None,
        },
    }
}

fn context() -> jarvis_application::request_context::RequestContext {
    jarvis_application::run_service::api_request_context(
        jarvis_domain::ids::WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
        jarvis_domain::ids::PrincipalId::from_uuid(uuid::Uuid::from_u128(2)),
        jarvis_domain::ids::RequestId::from_uuid(uuid::Uuid::from_u128(3)),
        jarvis_domain::ids::CorrelationId::from_uuid(uuid::Uuid::from_u128(4)),
    )
}

async fn run(provider: &OpenAiCompatibleProvider) -> Vec<ModelStreamEvent> {
    let request = request_for(provider);
    let cancel = CancellationScope::new();
    let ctx = context();
    let mut stream = provider
        .open(&ctx, &request, &cancel)
        .await
        .expect("a stream opens");
    let mut events = Vec::new();
    while let Ok(Some(event)) = stream.next_event().await {
        events.push(event);
    }
    events
}

fn failure_code(events: &[ModelStreamEvent]) -> Option<String> {
    events.iter().find_map(|event| match &event.kind {
        ModelStreamEventKind::CallFailed { code, .. } => Some(code.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn a_certificate_from_an_injected_root_is_accepted_and_the_answer_streams() {
    let pki = pki_for(&["localhost"]);
    let (port, server) =
        serve_tls_once(server_config(&pki), chunked_sse(&ANSWER), Ending::Clean).await;
    let provider = provider_with(port, roots_trusting(&pki));
    assert_eq!(provider.endpoint_class(), EndpointClass::ApprovedCloud);

    let events = run(&provider).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.kind, ModelStreamEventKind::OutputTextDelta { delta, .. } if delta.contains("Hello over TLS"))),
        "the streamed text must arrive: {events:?}"
    );
    assert!(
        matches!(
            events
                .iter()
                .find(|event| event.kind.is_terminal())
                .map(|event| &event.kind),
            Some(ModelStreamEventKind::CallCompleted { .. })
        ),
        "the call must complete: {events:?}"
    );

    let observed = server.await.expect("the server task finishes");
    assert!(observed.handshake_completed);
    assert_eq!(
        observed.sni.as_deref(),
        Some("localhost"),
        "SNI is the verified name"
    );
    assert_eq!(observed.alpn.as_deref(), Some(&b"http/1.1"[..]));
    let request = String::from_utf8_lossy(&observed.request).into_owned();
    assert!(
        request.contains(&format!("Authorization: Bearer {CANARY}\r\n")),
        "the credential travels as a header: {request}"
    );
    assert!(
        request.contains(&format!("Host: localhost:{port}\r\n")),
        "a non-default port stays in the Host header: {request}"
    );
    let request_line = request.lines().next().unwrap_or_default();
    assert!(
        !request_line.contains(CANARY),
        "never in the URL: {request_line}"
    );
}

#[tokio::test]
async fn a_certificate_whose_root_is_not_trusted_is_rejected_and_no_credential_is_sent() {
    let pki = pki_for(&["localhost"]);
    let (port, server) =
        serve_tls_once(server_config(&pki), chunked_sse(&ANSWER), Ending::Clean).await;
    // The client trusts nothing, so the (otherwise valid) chain does not verify.
    let provider = provider_with(port, rustls::RootCertStore::empty());

    let events = run(&provider).await;
    assert_eq!(
        failure_code(&events).as_deref(),
        Some("model.provider_unavailable"),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ModelStreamEventKind::CallCompleted { .. })),
        "an unverified peer must never produce a completed call"
    );
    let observed = server.await.expect("the server task finishes");
    assert!(
        !observed.handshake_completed,
        "the server must not see a completed handshake"
    );
    assert!(
        observed.request.is_empty(),
        "no request bytes, and so no credential, reach an unverified peer"
    );
}

#[tokio::test]
async fn a_trusted_chain_for_a_different_name_is_rejected() {
    // The chain verifies against the injected root, but the certificate names another host.
    let pki = pki_for(&["other.example"]);
    let (port, server) =
        serve_tls_once(server_config(&pki), chunked_sse(&ANSWER), Ending::Clean).await;
    let provider = provider_with(port, roots_trusting(&pki));

    let events = run(&provider).await;
    assert_eq!(
        failure_code(&events).as_deref(),
        Some("model.provider_unavailable"),
        "{events:?}"
    );
    let observed = server.await.expect("the server task finishes");
    assert!(
        observed.request.is_empty(),
        "no credential reaches a peer whose name does not match"
    );
}

#[tokio::test]
async fn a_connection_dropped_without_close_notify_mid_body_is_a_failure_not_a_completion() {
    let pki = pki_for(&["localhost"]);
    // An announced chunk that is never finished, then the TCP connection ends with no `close_notify`.
    let mut body =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec();
    body.extend_from_slice(b"ff\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"cut");
    let (port, _server) = serve_tls_once(server_config(&pki), body, Ending::Abrupt).await;
    let provider = provider_with(port, roots_trusting(&pki));

    let events = run(&provider).await;
    assert!(
        failure_code(&events).is_some(),
        "a truncated TLS stream must fail: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, ModelStreamEventKind::CallCompleted { .. })),
        "and must never complete"
    );
}

#[test]
fn only_a_dns_name_is_a_tls_endpoint() {
    let build = |host: &str| {
        OpenAiCompatibleProvider::new_tls(
            ProviderId::parse("cloud.example").expect("valid"),
            host,
            443,
            CANARY,
            vec![ModelId::parse("test-model").expect("valid")],
        )
    };
    assert!(build("api.example.com").is_ok());
    // An address has no name to verify a certificate against.
    assert_eq!(
        build("203.0.113.7").err(),
        Some(ConfigError::InvalidTlsHost)
    );
    assert_eq!(build("127.0.0.1").err(), Some(ConfigError::InvalidTlsHost));
    assert_eq!(build("[::1]").err(), Some(ConfigError::InvalidTlsHost));
    // A URL, a path, or userinfo is not a host.
    assert_eq!(
        build("https://api.example.com").err(),
        Some(ConfigError::UrlShaped)
    );
    assert_eq!(
        build("user@api.example.com").err(),
        Some(ConfigError::UrlShaped)
    );
    assert_eq!(build("").err(), Some(ConfigError::UrlShaped));
    assert_eq!(build("not a host").err(), Some(ConfigError::InvalidTlsHost));
}

#[test]
fn the_plaintext_constructor_still_refuses_a_named_host() {
    let error = OpenAiCompatibleProvider::new(
        ProviderId::parse("cloud.example").expect("valid"),
        "api.example.com",
        443,
        CANARY,
        vec![ModelId::parse("test-model").expect("valid")],
    )
    .err();
    assert_eq!(error, Some(ConfigError::NotLoopback));
}
