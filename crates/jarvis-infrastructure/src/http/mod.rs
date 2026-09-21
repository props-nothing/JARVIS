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
//! * `/health/live` and `/health/ready` expose a status token and nothing else;
//! * everything under `/api/v1` requires a valid local credential.

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
use tower_http::limit::RequestBodyLimitLayer;

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
        .layer(middleware::from_fn(reject_browser_origin))
        // A global byte cap, because the extractor default does not bound raw
        // body consumption.
        .layer(RequestBodyLimitLayer::new(MAX_REQUEST_BODY_BYTES))
        .with_state(state)
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
    use super::{ApiState, Readiness, router};
    use crate::auth::{ClientCredentialPath, ClientRegistry, enroll_owner_client};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt as _;

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

        let state = Arc::new(ApiState {
            clients: Arc::new(clients),
            readiness: Arc::clone(&readiness),
            instance_id: "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
        });

        Fixture {
            app: router(state),
            token: credential.to_presentation_text(),
            readiness,
            dir,
        }
    }

    async fn get(app: &axum::Router, path: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
        let mut builder = Request::builder().uri(path).method("GET");
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
        let fixture = fixture("unknown");
        let (status, _) = get(&fixture.app, "/api/v1/nope", &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&fixture.dir);
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

        let state = Arc::new(ApiState {
            clients: Arc::new(clients),
            readiness: Arc::new(Readiness::new()),
            instance_id: "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
        });
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
