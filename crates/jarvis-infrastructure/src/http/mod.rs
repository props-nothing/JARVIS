//! The daemon's local HTTP surface.
//!
//! Foundation exposes only the endpoints the local control API contract
//! requires: two unauthenticated health probes, and an authenticated status
//! endpoint. Run resources are owned by the Brain milestone.
//!
//! Security properties enforced here rather than assumed:
//!
//! * the listener binds a parsed loopback address only;
//! * a request carrying an `Origin` header is rejected, because no browser
//!   origin is trusted in local mode;
//! * the `Host` header must equal the active numeric loopback authority, so a
//!   request addressed to another name cannot reach the local API;
//! * forwarded host/proto headers are rejected outright, because no proxy is
//!   trusted in local mode;
//! * `/health/live` and `/health/ready` expose a status token and nothing else;
//! * everything under `/api/v1` requires a valid local credential.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use jarvis_protocol::ErrorEnvelope;
use serde::Serialize;

use crate::auth::ClientRegistry;
use crate::auth::credential::CredentialError;

/// The maximum accepted request body, in bytes.
///
/// Axum's extractor default does not constrain raw body consumption, so the
/// limit is applied globally as a layer.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;

/// The API major version this build serves.
pub const API_MAJOR: u32 = 1;

/// The product version reported to an authenticated caller.
pub const SERVER_VERSION: &str = "0.1.0";

/// The header carrying the API major version.
pub const API_VERSION_HEADER: &str = "jarvis-api-version";

/// Readiness, which is deliberately separate from process liveness.
#[derive(Debug)]
pub struct Readiness {
    ready: AtomicBool,
    reason: std::sync::Mutex<Option<String>>,
}

impl Readiness {
    /// Creates a not-ready state, which is the correct state during startup.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            reason: std::sync::Mutex::new(None),
        }
    }

    /// Marks the daemon ready.
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
        if let Ok(mut reason) = self.reason.lock() {
            *reason = None;
        }
    }

    /// Marks the daemon not ready, recording a safe reason code.
    pub fn mark_not_ready(&self, reason: impl Into<String>) {
        // Set not-ready before recording the reason so a reader never observes
        // `ready` together with a reason.
        self.ready.store(false, Ordering::SeqCst);
        if let Ok(mut guard) = self.reason.lock() {
            *guard = Some(reason.into());
        }
    }

    /// Returns whether the daemon is ready.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Returns the current not-ready reason code, if any.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|guard| guard.clone())
    }
}

impl Default for Readiness {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state for the HTTP handlers.
#[derive(Debug)]
pub struct ApiState {
    /// The enrolled clients used for authentication.
    pub clients: Arc<ClientRegistry>,
    /// The readiness flag.
    pub readiness: Arc<Readiness>,
    /// The daemon instance identifier.
    pub instance_id: String,
    /// The loopback authority this daemon actually bound.
    ///
    /// The `Host` header is validated against this value rather than against a
    /// hardcoded port, because the port is ephemeral and assigned by the operating
    /// system. Comparing to a constant would reject the daemon's own address, and
    /// accepting any loopback host would let `localhost`, a different loopback
    /// address, or a foreign port reach the control surface.
    pub bound_authority: String,
}

impl ApiState {
    /// Builds state for a daemon bound at `address`.
    #[must_use]
    pub fn new(
        clients: Arc<ClientRegistry>,
        readiness: Arc<Readiness>,
        instance_id: String,
        address: SocketAddr,
    ) -> Self {
        Self {
            clients,
            readiness,
            instance_id,
            bound_authority: authority_of(address),
        }
    }
}

/// Returns the `Host` value that addresses `address`.
///
/// The IPv6 form is bracketed because that is what a client sends in the `Host`
/// header, and comparing a bracketed value against a bare address would reject
/// every IPv6 request.
#[must_use]
pub fn authority_of(address: SocketAddr) -> String {
    match address.ip() {
        IpAddr::V4(ip) => format!("{ip}:{}", address.port()),
        IpAddr::V6(ip) => format!("[{ip}]:{}", address.port()),
    }
}

/// The `{"status":"live"}` body.
#[derive(Debug, Serialize)]
struct StatusToken {
    status: &'static str,
}

/// The authenticated status body.
#[derive(Debug, Serialize)]
struct SystemStatus {
    instance_id: String,
    server_version: &'static str,
    api_major: u32,
    state: &'static str,
    profile: &'static str,
    storage: StorageStatus,
    capabilities: Vec<&'static str>,
}

/// The bounded storage status sub-object.
#[derive(Debug, Serialize)]
struct StorageStatus {
    kind: &'static str,
    status: &'static str,
}

/// Builds the router for `state`.
///
/// Layer order is load-bearing, and the first entry is the **outermost** layer:
/// `Router::layer` wraps the existing stack, so each `.layer` call puts the new
/// layer *outside* the previous ones. The order below is therefore the order a
/// request is checked in, and it is deliberate:
///
/// 1. the body cap, so an oversized body is measured before any other work;
/// 2. the local-authority check, because a request addressed elsewhere should not
///    reach a credential comparison at all;
/// 3. the browser-`Origin` check;
/// 4. version negotiation and authentication, per route;
/// 5. routing, with the envelope-returning fallback.
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness_handler))
        .route(
            "/api/v1/system/status",
            get(system_status).layer(middleware::from_fn_with_state(
                Arc::clone(&state),
                require_authentication,
            )),
        )
        .fallback(unknown_route)
        .layer(middleware::from_fn(reject_browser_origin))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_local_authority,
        ))
        // Outermost, so the cap is the first thing a request meets and its refusal
        // cannot be pre-empted by a version or credential error. The extractor
        // default does not bound raw body consumption, so this is the only bound.
        .layer(middleware::from_fn(limit_request_body))
        .with_state(state)
}

/// Refuses a request whose declared body exceeds the bound, in the shared envelope.
///
/// `tower_http`'s `RequestBodyLimitLayer` would enforce a limit, but it returns its
/// **own** response: a bare `413` with a plain-text body. That satisfies "the body
/// is bounded" while breaking two other contract rules at once — an unknown route
/// or an oversized body must return the common error envelope, and every
/// `/api/v1` response must be `application/json`. A client would get a status it
/// can see and nothing it can parse.
///
/// The check is on `Content-Length` only, which is the honest scope: a chunked body
/// declares no length and streaming it is the caller's problem to bound per route
/// when a route that reads a body exists. This never silently accepts an over-limit
/// request, because a body that declares a length cannot exceed the bound without
/// declaring it.
async fn limit_request_body(request: Request, next: Next) -> Response {
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > MAX_REQUEST_BODY_BYTES) {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request.too_large",
            "The request body exceeds the bounded limit.",
            false,
        );
    }
    next.run(request).await
}

/// Returns the shared error envelope for a route this surface does not serve.
///
/// This is required rather than cosmetic. The contract states that an unknown route
/// returns the **common error envelope**, and `axum`'s default fallback is a bare
/// `404` with an **empty body** — so a client would receive a status it can see and
/// nothing it can parse, violating the "unknown routes return the common error
/// envelope" contract for every unrecognized path.
///
/// The message is fixed rather than derived from the path. Reflecting the requested
/// path would echo attacker-supplied text back to the caller, which is the same
/// reason the `Host` refusal names neither the expected nor the received value.
async fn unknown_route() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "resource.not_found",
        "No such resource.",
        false,
    )
}

/// Liveness: the process event loop can answer. No dependency is required.
async fn liveness() -> impl IntoResponse {
    Json(StatusToken { status: "live" })
}

/// Readiness: required state can serve admitted requests.
async fn readiness_handler(State(state): State<Arc<ApiState>>) -> Response {
    if state.readiness.is_ready() {
        (StatusCode::OK, Json(StatusToken { status: "ready" })).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(StatusToken {
                status: "not_ready",
            }),
        )
            .into_response()
    }
}

/// The headers that describe a proxy hop.
///
/// No proxy is trusted in local mode, so their presence is itself the problem:
/// a request that arrives with one is either going through infrastructure JARVIS
/// does not have or is lying about its own origin. Either way the right answer is
/// to refuse it rather than to try to interpret it.
const FORWARDED_HEADERS: [&str; 4] = [
    "forwarded",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
];

/// Rejects a request that is not addressed to this daemon's own loopback authority.
///
/// This is the `Host` half of the local API contract, and it is a distinct control
/// from authentication. Authentication answers "is the caller allowed"; this
/// answers "was this request addressed to this daemon", which matters because a
/// loopback service can otherwise be reached by any name that resolves to
/// loopback — `localhost`, a different `127.0.0.0/8` address, or a DNS name that
/// points at `127.0.0.1`. Binding is not a substitute: an ephemeral port on
/// `127.0.0.1` accepts every one of those.
///
/// The comparison is on the exact bound authority, so it is independent of the
/// port the operating system chose. A missing `Host` is refused too, because HTTP/1.1
/// requires it and a client that omits it cannot be reasoned about.
async fn require_local_authority(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    for name in FORWARDED_HEADERS {
        if headers.contains_key(name) {
            return error_response(
                StatusCode::FORBIDDEN,
                "api.forwarded_header_not_allowed",
                "Forwarding headers are not accepted by the local API.",
                false,
            );
        }
    }

    let mut hosts = headers.get_all(header::HOST).iter();
    let Some(presented) = hosts.next() else {
        // HTTP/1.1 requires `Host`. A client that omits it cannot be reasoned
        // about, so absence is a refusal rather than an exemption from the check.
        return error_response(
            StatusCode::BAD_REQUEST,
            "api.host_not_allowed",
            "The request must address this daemon's loopback authority.",
            false,
        );
    };
    // A request carrying more than one `Host` must be refused outright, and this
    // is not merely tidiness. When two headers are present, a proxy and an origin
    // can legitimately disagree about which one is authoritative, and that
    // disagreement is the basis of request smuggling. Reading "the first one" would
    // pick a winner by convention that the other party need not share, so the only
    // safe answer is to reject the ambiguity.
    if hosts.next().is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "api.host_not_allowed",
            "The request must address this daemon's loopback authority.",
            false,
        );
    }

    let matches_authority = presented
        .to_str()
        .is_ok_and(|host| host.trim() == state.bound_authority);
    if !matches_authority {
        // The response names neither the expected authority nor the received one:
        // the expected value is the daemon's own address, and echoing either would
        // reflect attacker-supplied text back to the caller.
        return error_response(
            StatusCode::BAD_REQUEST,
            "api.host_not_allowed",
            "The request must address this daemon's loopback authority.",
            false,
        );
    }

    next.run(request).await
}

/// Rejects any request that claims a browser origin.
///
/// No CORS response headers are ever emitted, so a browser client cannot use the
/// local API regardless of origin.
async fn reject_browser_origin(request: Request, next: Next) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return error_response(
            StatusCode::FORBIDDEN,
            "api.origin_not_allowed",
            "Browser origins are not accepted by the local API.",
            false,
        );
    }
    next.run(request).await
}

/// Requires a valid local credential and a supported API version.
async fn require_authentication(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    // Version negotiation comes first: an unsupported client should learn the
    // supported major rather than receive an authentication prompt.
    match headers
        .get(API_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value.trim() == API_MAJOR.to_string() => {}
        _ => {
            return error_response(
                StatusCode::UPGRADE_REQUIRED,
                "api.version_unsupported",
                "Supported API major: 1.",
                false,
            );
        }
    }

    let Some(presented) = bearer_token(&headers) else {
        return unauthenticated();
    };
    if state.clients.authenticate(presented).is_err() {
        return unauthenticated();
    }

    let mut response = next.run(request).await;
    if let Ok(value) = header::HeaderValue::from_str(&API_MAJOR.to_string()) {
        response.headers_mut().insert(API_VERSION_HEADER, value);
    }
    if let Ok(value) = header::HeaderValue::from_str(SERVER_VERSION) {
        response
            .headers_mut()
            .insert("jarvis-server-version", value);
    }
    response
}

/// Extracts the bearer token from an `Authorization` header.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        Some(token.trim())
    } else {
        None
    }
}

/// Returns the one authentication failure response.
///
/// Unknown, malformed, and revoked credentials all produce this exact value, so
/// a caller cannot distinguish which case occurred.
fn unauthenticated() -> Response {
    error_response(
        StatusCode::UNAUTHORIZED,
        "auth.credential_rejected",
        "The local client credential was not accepted.",
        false,
    )
}

/// Authenticated, bounded operational status.
async fn system_status(State(state): State<Arc<ApiState>>) -> Json<SystemStatus> {
    // Never includes filesystem paths, secret references, connection strings,
    // environment values, or another client's data.
    Json(SystemStatus {
        instance_id: state.instance_id.clone(),
        server_version: SERVER_VERSION,
        api_major: API_MAJOR,
        state: if state.readiness.is_ready() {
            "ready"
        } else {
            "not_ready"
        },
        profile: "default",
        storage: StorageStatus {
            kind: "sqlite",
            status: "ready",
        },
        capabilities: vec!["system.status"],
    })
}

/// Builds an error response from the shared envelope.
fn error_response(status: StatusCode, code: &str, message: &str, retryable: bool) -> Response {
    let envelope = ErrorEnvelope::new(code, message, retryable);
    match envelope.to_bytes() {
        Ok(bytes) => (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(_) => (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            "{}".as_bytes(),
        )
            .into_response(),
    }
}

/// Maps a credential error to its API status.
#[must_use]
pub fn credential_status(error: CredentialError) -> StatusCode {
    match error {
        CredentialError::EntropyUnavailable => StatusCode::INTERNAL_SERVER_ERROR,
        CredentialError::Malformed | CredentialError::Rejected => StatusCode::UNAUTHORIZED,
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiState, Readiness, authority_of, router};
    use crate::auth::{ClientCredentialPath, ClientRegistry, enroll_owner_client};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use tower::ServiceExt as _;

    /// The authority the test fixture pretends the daemon bound.
    ///
    /// A fixed value rather than a real bind, because these tests assert the
    /// validation logic and must not depend on an ephemeral port. The daemon's own
    /// test asserts that the real bound address reaches this same field.
    const TEST_AUTHORITY: &str = "127.0.0.1:43127";

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jarvis-fnd007-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    struct Fixture {
        app: axum::Router,
        token: String,
        readiness: Arc<Readiness>,
        dir: std::path::PathBuf,
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = temp_dir(tag);
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");

        let mut clients = ClientRegistry::new();
        clients.register(registered);
        let readiness = Arc::new(Readiness::new());

        let state = Arc::new(ApiState::new(
            Arc::new(clients),
            Arc::clone(&readiness),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
        ));

        Fixture {
            app: router(state),
            token: credential.to_presentation_text(),
            readiness,
            dir,
        }
    }

    /// Sends a request with a valid `Host`, which every other test relies on.
    ///
    /// A `host` entry in `headers` genuinely **replaces** the default rather than
    /// adding a second header. This matters: `Request::builder().header()` appends,
    /// so the first version of this helper produced two `Host` headers and the
    /// middleware read the valid one — which made a Host-rejection test fail while
    /// hiding that duplicate headers were being accepted at all.
    async fn get(app: &axum::Router, path: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
        let overrides_host = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"));
        let mut builder = Request::builder().uri(path).method("GET");
        if !overrides_host {
            builder = builder.header("host", TEST_AUTHORITY);
        }
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).expect("request builds"))
            .await
            .expect("router responds");

        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body readable");
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// Sends a request with no `Host` at all, to exercise the missing-header path.
    async fn get_without_host(app: &axum::Router, path: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .uri(path)
            .method("GET")
            // An empty-valued Host is removed by the builder, so the header is
            // genuinely absent rather than present-and-blank.
            .body(Body::empty())
            .expect("request builds");
        let response = app.clone().oneshot(request).await.expect("router responds");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body readable");
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn liveness_succeeds_while_not_ready() {
        let fixture = fixture("live");
        let (status, body) = get(&fixture.app, "/health/live", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"status":"live"}"#);
        assert!(!fixture.readiness.is_ready(), "liveness is not readiness");
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn readiness_reports_not_ready_before_startup_completes() {
        let fixture = fixture("ready-no");
        let (status, body) = get(&fixture.app, "/health/ready", &[]).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, r#"{"status":"not_ready"}"#);
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn readiness_reports_ready_after_startup() {
        let fixture = fixture("ready-yes");
        fixture.readiness.mark_ready();
        let (status, body) = get(&fixture.app, "/health/ready", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"status":"ready"}"#);
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_failed_startup_leaves_readiness_false_with_a_reason() {
        let fixture = fixture("ready-failed");
        fixture.readiness.mark_ready();
        fixture.readiness.mark_not_ready("jarvis.db_migrate");
        assert!(!fixture.readiness.is_ready());
        assert_eq!(
            fixture.readiness.reason().as_deref(),
            Some("jarvis.db_migrate")
        );

        let (status, _) = get(&fixture.app, "/health/ready", &[]).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn health_probes_expose_only_a_status_token() {
        let fixture = fixture("minimal");
        for path in ["/health/live", "/health/ready"] {
            let (_, body) = get(&fixture.app, path, &[]).await;
            for forbidden in [
                "version",
                "profile",
                "storage",
                "path",
                "instance",
                "capabilit",
            ] {
                assert!(
                    !body.to_ascii_lowercase().contains(forbidden),
                    "{path} leaked {forbidden}: {body}",
                );
            }
        }
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn an_unauthenticated_status_request_is_refused_without_detail() {
        let fixture = fixture("no-auth");
        let (status, body) = get(
            &fixture.app,
            "/api/v1/system/status",
            &[("jarvis-api-version", "1")],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("auth.credential_rejected"), "{body}");
        assert!(!body.contains("instance_id"), "no detail: {body}");
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_wrong_credential_gets_the_same_response_as_a_missing_one() {
        let fixture = fixture("wrong-auth");
        let missing = get(
            &fixture.app,
            "/api/v1/system/status",
            &[("jarvis-api-version", "1")],
        )
        .await;

        let wrong_token = format!("{}x", fixture.token);
        let wrong = get(
            &fixture.app,
            "/api/v1/system/status",
            &[
                ("jarvis-api-version", "1"),
                ("authorization", &format!("Bearer {wrong_token}")),
            ],
        )
        .await;

        assert_eq!(
            missing, wrong,
            "unknown and wrong must be indistinguishable"
        );
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn an_authenticated_status_request_returns_bounded_state() {
        let fixture = fixture("auth-ok");
        fixture.readiness.mark_ready();

        let authorization = format!("Bearer {}", fixture.token);
        let (status, body) = get(
            &fixture.app,
            "/api/v1/system/status",
            &[
                ("jarvis-api-version", "1"),
                ("authorization", authorization.as_str()),
            ],
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(r#""state":"ready""#));
        assert!(body.contains(r#""api_major":1"#));
        assert!(body.contains(r#""kind":"sqlite""#));
        // The token must never be echoed back.
        assert!(!body.contains(&fixture.token), "the credential was echoed");
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_missing_or_unsupported_api_version_is_reported_safely() {
        let fixture = fixture("version");
        let authorization = format!("Bearer {}", fixture.token);

        for version in [None, Some("2"), Some("0"), Some("abc")] {
            let mut headers = vec![("authorization", authorization.as_str())];
            if let Some(value) = version {
                headers.push(("jarvis-api-version", value));
            }
            let (status, body) = get(&fixture.app, "/api/v1/system/status", &headers).await;
            assert_eq!(status, StatusCode::UPGRADE_REQUIRED, "version {version:?}");
            assert!(body.contains("api.version_unsupported"), "{body}");
            assert!(body.contains("Supported API major: 1"), "{body}");
        }
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    /// Sends a request with a body, for the limit tests.
    ///
    /// `content-length` is set explicitly because `Request::builder()` does **not**
    /// set it — an HTTP client does. Omitting it made the first version of this
    /// helper produce a request no real client would send, and the limit check
    /// (which reads the declared length) never fired, so the test asserted an auth
    /// error instead of the limit error it was written for.
    async fn send_body(
        app: &axum::Router,
        path: &str,
        method: &str,
        body: Vec<u8>,
    ) -> (StatusCode, String, Option<String>) {
        let declared = body.len().to_string();
        let request = Request::builder()
            .uri(path)
            .method(method)
            .header("host", TEST_AUTHORITY)
            .header("content-type", "application/json")
            .header("content-length", declared)
            .body(Body::from(body))
            .expect("request builds");
        let response = app.clone().oneshot(request).await.expect("router responds");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body readable");
        (
            status,
            String::from_utf8_lossy(&body).into_owned(),
            content_type,
        )
    }

    #[tokio::test]
    async fn an_over_limit_body_is_refused_with_the_shared_envelope() {
        // Contract required test 5, and the contract's rule that every `/api/v1`
        // response is `application/json`. A limiter that returns its own plain-text
        // body would satisfy "the body is bounded" while breaking both: a client
        // gets a status it can see and nothing it can parse, and a non-JSON content
        // type on an API route.
        let fixture = fixture("too-large");
        let oversized = vec![b'a'; super::MAX_REQUEST_BODY_BYTES + 1];
        let (status, body, content_type) =
            send_body(&fixture.app, "/api/v1/system/status", "POST", oversized).await;

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert_eq!(
            content_type.as_deref(),
            Some("application/json"),
            "an API error must be JSON, got {content_type:?} with body {body:?}",
        );
        // Asserting on the raw body first gives a readable diagnostic when the
        // response is not the envelope; the typed parse then proves the shape.
        assert!(
            body.contains("request.too_large"),
            "the limit refusal must use the envelope, got {body:?}",
        );
        let envelope: jarvis_protocol::ErrorEnvelope =
            serde_json::from_str(&body).expect("the limit refusal is the envelope");
        assert_eq!(envelope.code(), "request.too_large");
        assert!(!envelope.error.retryable);
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_body_at_the_limit_is_not_refused_for_its_size() {
        // The control. Without it, a limiter that refused every body would pass the
        // test above. This request is still refused, but for a different reason:
        // there is no `POST` route at this milestone, so the answer must be the
        // route error rather than the limit error.
        let fixture = fixture("at-limit");
        let at_limit = vec![b'a'; super::MAX_REQUEST_BODY_BYTES];
        let (status, body, content_type) =
            send_body(&fixture.app, "/api/v1/system/status", "POST", at_limit).await;

        assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert_eq!(content_type.as_deref(), Some("application/json"));
        let envelope: jarvis_protocol::ErrorEnvelope =
            serde_json::from_str(&body).expect("the envelope");
        assert_ne!(envelope.code(), "request.too_large");
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_browser_origin_is_rejected_on_every_route() {
        let fixture = fixture("origin");
        let authorization = format!("Bearer {}", fixture.token);

        for path in ["/health/live", "/health/ready", "/api/v1/system/status"] {
            let (status, body) = get(
                &fixture.app,
                path,
                &[
                    ("origin", "https://evil.example"),
                    ("jarvis-api-version", "1"),
                    ("authorization", authorization.as_str()),
                ],
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
            assert!(body.contains("api.origin_not_allowed"), "{body}");
        }
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn an_unknown_route_returns_the_shared_envelope() {
        // The contract says an unknown route returns the **common error envelope**,
        // not merely a 404. Asserting only the status is what let this pass while
        // the response had an empty body: `axum`'s default fallback is a bare 404,
        // so a client would get a status with nothing to parse and no machine code.
        let fixture = fixture("unknown");
        let (status, body) = get(&fixture.app, "/api/v1/nope", &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let envelope: jarvis_protocol::ErrorEnvelope =
            serde_json::from_str(&body).expect("an unknown route must return the envelope");
        assert_eq!(envelope.code(), "resource.not_found");
        assert!(!envelope.error.retryable);
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_request_addressed_to_another_host_is_rejected() {
        // The `Host` half of the contract. Loopback is not an authentication
        // boundary and binding is not an address filter: an ephemeral port on
        // `127.0.0.1` accepts a connection addressed to `localhost`, to another
        // `127.0.0.0/8` address, or to a DNS name that resolves to loopback. Each of
        // those must be refused, and each is a shape a loopback-only check would
        // wrongly accept.
        let fixture = fixture("host");
        let authorization = format!("Bearer {}", fixture.token);

        for host in [
            "localhost:43127",
            "127.0.0.2:43127",
            // A different port is a different authority even on the same address.
            "127.0.0.1:9999",
            "evil.example",
            // A name that looks like the authority but is not it.
            "127.0.0.1:43127.evil.example",
        ] {
            let (status, body) = get(
                &fixture.app,
                "/api/v1/system/status",
                &[
                    ("host", host),
                    ("jarvis-api-version", "1"),
                    ("authorization", authorization.as_str()),
                ],
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "host {host}");
            assert!(body.contains("api.host_not_allowed"), "{body}");
            // The refusal must not echo the received value back, because it is
            // attacker-supplied text.
            assert!(
                !body.contains(host),
                "the received host was reflected: {body}"
            );
        }
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_request_with_two_host_headers_is_rejected() {
        // Found while writing the test above: the helper appended a second `Host`
        // instead of replacing it, and the check read the first one — so a request
        // carrying a valid authority AND a different one was accepted. That is the
        // request-smuggling shape: two parties disagreeing about which `Host` is
        // authoritative. The only safe answer is to refuse the ambiguity.
        let fixture = fixture("two-hosts");
        let authorization = format!("Bearer {}", fixture.token);

        let request = Request::builder()
            .uri("/api/v1/system/status")
            .method("GET")
            .header("host", TEST_AUTHORITY)
            .header("host", "evil.example")
            .header("jarvis-api-version", "1")
            .header("authorization", authorization.as_str())
            .body(Body::empty())
            .expect("request builds");
        let response = fixture
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn a_request_without_a_host_header_is_rejected() {
        // HTTP/1.1 requires `Host`. A client that omits it cannot be reasoned
        // about, so absence is a refusal rather than an exemption from the check.
        let fixture = fixture("no-host");
        let (status, body) = get_without_host(&fixture.app, "/health/live").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("api.host_not_allowed"), "{body}");
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn the_daemons_own_authority_is_accepted() {
        // The control for the two tests above: without it, a check that rejected
        // everything would pass them. This is also what proves the comparison uses
        // the bound authority rather than a hardcoded one.
        let fixture = fixture("host-ok");
        let (status, body) = get(&fixture.app, "/health/live", &[]).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, r#"{"status":"live"}"#);
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn forwarding_headers_are_rejected_on_every_route() {
        // The contract rejects forwarded host/proto headers because no proxy is
        // trusted in local mode. Their presence is itself the problem, so they are
        // refused before any other reasoning, and they must be refused even on the
        // unauthenticated health probes.
        let fixture = fixture("forwarded");
        let authorization = format!("Bearer {}", fixture.token);

        for name in [
            "forwarded",
            "x-forwarded-host",
            "x-forwarded-proto",
            "x-real-ip",
        ] {
            for path in ["/health/live", "/api/v1/system/status"] {
                let header_value = if name == "forwarded" {
                    "for=198.51.100.7;host=evil.example"
                } else if name == "x-forwarded-proto" {
                    "https"
                } else {
                    "evil.example"
                };
                let (status, body) = get(
                    &fixture.app,
                    path,
                    &[
                        (name, header_value),
                        ("jarvis-api-version", "1"),
                        ("authorization", authorization.as_str()),
                    ],
                )
                .await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{name} on {path}");
                assert!(body.contains("api.forwarded_header_not_allowed"), "{body}");
            }
        }
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[test]
    fn the_authority_format_brackets_ipv6() {
        // A client sends `[::1]:port` in `Host`, so a bare-address comparison would
        // reject every IPv6 request.
        use std::net::{Ipv6Addr, SocketAddr};
        assert_eq!(
            authority_of(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127)),
            TEST_AUTHORITY
        );
        assert_eq!(
            authority_of(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 43127)),
            "[::1]:43127",
        );
    }

    #[tokio::test]
    async fn a_revoked_client_is_refused() {
        let dir = temp_dir("revoked");
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");

        let mut clients = ClientRegistry::new();
        clients.register(registered);
        clients.revoke("owner").expect("revocation");

        let state = Arc::new(ApiState::new(
            Arc::new(clients),
            Arc::new(Readiness::new()),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
        ));
        let app = router(state);

        let authorization = format!("Bearer {}", credential.to_presentation_text());
        let (status, _) = get(
            &app,
            "/api/v1/system/status",
            &[
                ("jarvis-api-version", "1"),
                ("authorization", authorization.as_str()),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
