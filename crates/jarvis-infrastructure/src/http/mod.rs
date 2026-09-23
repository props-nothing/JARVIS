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

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use axum::{Json, Router};
use jarvis_application::model::ModelProvider;
use jarvis_application::policy_service::PolicyService;
use jarvis_application::request_context::{AuthenticationAssurance, RequestChannel};
use jarvis_application::run_service::{RunService, RunSpawner};
use jarvis_domain::time::UtcTimestamp;
use jarvis_protocol::ErrorEnvelope;
use serde::Serialize;

use crate::auth::ClientRegistry;
use crate::auth::credential::CredentialError;

pub mod policy;
pub mod runs;

pub use runs::{RequestScope, resolve_scope, wire_state};

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
    /// The run orchestration service, absent when no storage is configured.
    ///
    /// Optional so the Foundation surface (health and status) can be built without a
    /// database. A run route reached without one answers `service.not_ready` rather
    /// than being unroutable, so a client gets a parseable envelope instead of the
    /// generic unknown-route refusal.
    pub runs: Option<Arc<RunService>>,
    /// The model data policy service, absent when no storage is configured.
    ///
    /// Optional for the same reason `runs` is: the Foundation surface must build without a
    /// database, and a policy route reached without one answers `service.not_ready` — a
    /// parseable envelope — rather than being unroutable.
    pub policies: Option<Arc<PolicyService>>,
    /// The candidate inventory an effective-route probe evaluates.
    ///
    /// Absent when no provider is configured, because a probe with no candidates would report
    /// `model.policy_unsatisfied` for something the policy never refused.
    pub inventory: Option<Arc<ProviderInventory>>,
    /// How a run's execution is scheduled.
    pub spawner: Arc<dyn RunSpawner>,
}

/// The model candidates a daemon can route a call to.
///
/// Built once from the composed provider rather than per request, because the inventory is a
/// property of the process — which provider is configured, and where its endpoint sits — and
/// recomputing it per request would invite a second answer to the same question. It holds
/// `RouteCandidate`s whose retention and capability evidence are **absent**, which is the
/// honest state until `BRN-011` measures them: a policy demanding documented evidence refuses
/// every candidate rather than having a claim invented on its behalf.
#[derive(Debug)]
pub struct ProviderInventory {
    candidates: Vec<jarvis_domain::model::routing::RouteCandidate>,
    now: Option<UtcTimestamp>,
}

impl ProviderInventory {
    /// Builds the inventory from a provider and a clock.
    ///
    /// The clock is read here so every request through one daemon evaluates evidence freshness
    /// against the same instant, which is what makes two probes in one run reproducible.
    #[must_use]
    pub fn new(provider: &dyn ModelProvider, now: Option<UtcTimestamp>) -> Self {
        // Built by the *same* function the run path uses, rather than by a second copy of this
        // loop. Two builders would let the diagnostic probe and a real run disagree about which
        // models exist — the failure an operator would least likely see, because the probe would
        // report a route the run never took.
        Self {
            candidates: jarvis_application::run_service::route_candidates(provider),
            now,
        }
    }

    /// Returns the candidates.
    #[must_use]
    pub fn candidates(&self) -> &[jarvis_domain::model::routing::RouteCandidate] {
        &self.candidates
    }

    /// Returns the instant the inventory was built, when a clock was available.
    #[must_use]
    pub const fn built_at(&self) -> Option<UtcTimestamp> {
        self.now
    }
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The client registry and the run ports are not printed: one holds credential
        // verifiers and the other can hold a connection string.
        formatter
            .debug_struct("ApiState")
            .field("instance_id", &self.instance_id)
            .field("bound_authority", &self.bound_authority)
            .field("ready", &self.readiness.is_ready())
            .field("runs_configured", &self.runs.is_some())
            .finish_non_exhaustive()
    }
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
            runs: None,
            policies: None,
            inventory: None,
            spawner: Arc::new(jarvis_application::run_service::TokioSpawner),
        }
    }

    /// Attaches the run orchestration service.
    #[must_use]
    pub fn with_runs(mut self, runs: Arc<RunService>) -> Self {
        self.runs = Some(runs);
        self
    }

    /// Attaches the model data policy service.
    #[must_use]
    pub fn with_policies(
        mut self,
        policies: Arc<PolicyService>,
        inventory: Arc<ProviderInventory>,
    ) -> Self {
        self.policies = Some(policies);
        self.inventory = Some(inventory);
        self
    }

    /// Overrides how a run's execution is scheduled.
    #[must_use]
    pub fn with_spawner(mut self, spawner: Arc<dyn RunSpawner>) -> Self {
        self.spawner = spawner;
        self
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
    // Every `/api/v1` route needs the same authentication layer, so it is applied by
    // one helper rather than repeated. A route that forgot it would be reachable
    // without a credential, which is why the wrapping is a function and not a
    // copy-paste at each call site.
    let authenticated = |route: MethodRouter<Arc<ApiState>>| {
        route.layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_authentication,
        ))
    };
    Router::new()
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness_handler))
        .route("/api/v1/system/status", authenticated(get(system_status)))
        // The run routes are always *routable* and answer `service.not_ready` when no
        // storage is configured, rather than being absent. A client then receives a
        // parseable envelope instead of the generic unknown-route refusal, which would
        // tell it the endpoint does not exist.
        .route("/api/v1/runs", authenticated(post(runs::create_run)))
        .route("/api/v1/runs/{run_id}", authenticated(get(runs::read_run)))
        .route(
            "/api/v1/runs/{run_id}/cancel",
            authenticated(post(runs::cancel_run)),
        )
        .route(
            "/api/v1/runs/{run_id}/events",
            authenticated(get(runs::run_events)),
        )
        // The policy routes follow the same shape as the run routes: always routable, and
        // answering `service.not_ready` when no storage is configured, so a client receives a
        // parseable envelope rather than the generic unknown-route refusal.
        .route(
            "/api/v1/model-data-policy",
            authenticated(get(policy::read_active_policy).put(policy::put_active_policy)),
        )
        .route(
            "/api/v1/model-data-policy/effective",
            authenticated(get(policy::read_effective_route)),
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

/// The authenticated identity of the caller, as a route extractor.
///
/// A route that names this parameter cannot run without a credential: the extraction
/// fails with the same envelope [`require_authentication`] produces, and it fails
/// *before* the handler body runs. That makes "this handler requires authentication" a
/// property of its signature rather than a promise the middleware layer has to keep in
/// a separate file — the two cannot drift.
///
/// It deliberately carries only what a handler needs to build trusted context: the
/// client id, the assurance level, and the channel. The credential itself is not here,
/// so a handler cannot log it or place it in a response by accident.
#[derive(Debug, Clone)]
pub struct AuthenticatedClient {
    /// The stable client identifier the credential authenticated as.
    pub client_id: String,
    /// How strongly the caller's identity was proven.
    pub assurance: AuthenticationAssurance,
    /// The channel this request arrived on.
    pub channel: RequestChannel,
}

impl<S> FromRequestParts<S> for AuthenticatedClient
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // The extraction reads a value the middleware already recorded, so it has
        // nothing to wait for. `FromRequestParts` requires an `async` signature, so this
        // yields once rather than pretending to do asynchronous work — the alternative
        // would be an `allow` for `unused_async`, which would hide a genuinely
        // unnecessary `async` if one were introduced later.
        std::future::ready(()).await;

        // An absent identity would mean a route was wired without the authentication
        // layer. That must fail rather than proceed unauthenticated, so it produces the
        // same refusal the middleware does.
        let Some(client_id) = parts.extensions.get::<AuthenticatedClientId>() else {
            return Err(unauthenticated());
        };
        Ok(Self {
            client_id: client_id.0.clone(),
            assurance: AuthenticationAssurance::Standard,
            // A local API client is one that reached the loopback surface with a valid
            // credential; the channel is not caller-supplied, so it cannot claim to be
            // something more privileged.
            channel: RequestChannel::Api,
        })
    }
}

/// The client id the authentication middleware verified.
#[derive(Debug, Clone)]
pub struct AuthenticatedClientId(String);

/// Requires a valid local credential and a supported API version.
async fn require_authentication(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    mut request: Request,
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
    let Ok(client) = state.clients.authenticate(presented) else {
        return unauthenticated();
    };

    // The verified identity is recorded for the extractor, so a handler's signature
    // can require it. Only the id is stored: the credential is not, so it cannot reach
    // a log or a response through a handler.
    request
        .extensions_mut()
        .insert(AuthenticatedClientId(client.client_id.clone()));

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
    use super::{ApiState, ProviderInventory, Readiness, authority_of, router};
    use crate::auth::{ClientCredentialPath, ClientRegistry, enroll_owner_client};
    use crate::storage::repositories::SqliteRepositories;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use jarvis_application::policy_service::PolicyService;
    use jarvis_application::repository::policy::ModelDataPolicyRepository;
    use jarvis_application::repository::run::RunRepository as _;
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

    // ---- The run resource surface ----

    /// The run-surface fixture: a migrated database with the scripted provider.
    ///
    /// A real migrated database rather than a double, because these tests are about the
    /// HTTP boundary and a double at both layers would let a serialization or scope
    /// defect survive. The database is in-memory so the fixture stays fast.
    async fn runs_fixture(tag: &str) -> (axum::Router, String) {
        let (app, token, _repositories) = runs_fixture_with(tag, FixtureProvider::Answers).await;
        (app, token)
    }

    /// The same fixture, but also handing back the repositories.
    ///
    /// A separate accessor rather than a wider return type on [`runs_fixture`], because most of
    /// these tests are about the HTTP surface and never look at storage; forcing each of them to
    /// destructure a handle they ignore would be noise. This one exists so a test can assert what
    /// a *real* create wrote — the difference between "the handler accepted the field" and "the
    /// field reached a row", which is exactly the defect this fixture is used for.
    async fn runs_fixture_with_storage(
        tag: &str,
    ) -> (axum::Router, String, Arc<SqliteRepositories>) {
        runs_fixture_with(tag, FixtureProvider::Answers).await
    }

    /// Which provider the run fixture composes.
    ///
    /// A parameter rather than a second fixture, because only the provider differs between them:
    /// duplicating the enrollment, the migration, and the state assembly would mean two copies of
    /// the composition that could drift — and a drifting fixture is how a test asserts a rule the
    /// daemon does not keep.
    #[derive(Debug, Clone, Copy)]
    enum FixtureProvider {
        /// Answers and completes, so a run reaches `Completed`.
        Answers,
        /// Refuses to open, so a run reaches `Failed` with a code.
        Refuses,
    }

    async fn runs_fixture_with(
        tag: &str,
        kind: FixtureProvider,
    ) -> (axum::Router, String, Arc<SqliteRepositories>) {
        use crate::storage::repositories::SqliteRepositories;
        use crate::storage::{Database, migrate};
        use jarvis_application::live_events::StreamDeltaSink;
        use jarvis_application::model::{ModelProvider, ProviderError};
        use jarvis_application::run_service::{
            RunCancellationRegistry, RunPorts, RunService, TokioSpawner,
        };
        use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
        use jarvis_domain::model::stream::{FinishReason, ModelStreamEventKind};

        let dir = temp_dir(tag);
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
        let mut clients = ClientRegistry::new();
        clients.register(registered);

        let database = Database::open_in_memory().await.expect("in-memory opens");
        migrate::run(database.pool()).await.expect("migrates");
        let repositories = Arc::new(SqliteRepositories::new(database.pool().clone()));
        let model = ModelRef::new(
            ProviderId::parse("scripted.local").expect("valid"),
            ModelId::parse("fixture-1").expect("valid"),
        );
        let answering = jarvis_application::model::ScriptedProvider::new(model)
            .emit(ModelStreamEventKind::OutputItemAdded {
                item_id: "out-1".to_owned(),
            })
            .emit_text("out-1", "hello from the scripted provider")
            .emit(ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            });
        let provider: Arc<dyn ModelProvider> = match kind {
            FixtureProvider::Answers => Arc::new(answering),
            // `Refused` rather than `Unavailable`, because it is not retryable: a retryable
            // failure leaves the run live so another attempt can be made, and the fixture's
            // purpose is to reach a terminal `Failed` with a code.
            FixtureProvider::Refuses => Arc::new(answering.fail_on_open(ProviderError::Refused)),
        };
        let service = Arc::new(RunService::new(
            RunPorts {
                runs: Arc::clone(&repositories)
                    as Arc<dyn jarvis_application::repository::run::RunRepository>,
                conversations: Arc::clone(&repositories)
                    as Arc<
                        dyn jarvis_application::repository::conversation::ConversationRepository,
                    >,
                model_calls: Arc::clone(&repositories)
                    as Arc<dyn jarvis_application::repository::model_call::ModelCallRepository>,
                deltas: Arc::clone(&repositories) as Arc<dyn StreamDeltaSink>,
                provider,
                clock: Arc::new(crate::time::SystemClock::new()),
                // The run fixture attaches its policy store so a create can resolve the policy it
                // records, which is what makes the policy reference in a create request reach the
                // run. `None` here would make every run in these tests policy-less.
                policies: Some(Arc::clone(&repositories) as Arc<dyn ModelDataPolicyRepository>),
            },
            Arc::new(RunCancellationRegistry::new()),
        ));

        let state = Arc::new(
            ApiState::new(
                Arc::new(clients),
                Arc::new(Readiness::new()),
                "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
            )
            .with_runs(service)
            .with_spawner(Arc::new(TokioSpawner)),
        );
        (
            router(state),
            credential.to_presentation_text(),
            repositories,
        )
    }

    /// Authenticated headers for a run request.
    fn run_headers(token: &str) -> Vec<(&str, String)> {
        vec![
            ("authorization", format!("Bearer {token}")),
            ("jarvis-api-version", "1".to_owned()),
            ("content-type", "application/json".to_owned()),
            ("idempotency-key", format!("key-{}", uuid::Uuid::now_v7())),
        ]
    }

    /// Sends a request that may carry a body.
    async fn send(
        app: &axum::Router,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> (StatusCode, String) {
        let overrides_host = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"));
        let mut builder = Request::builder().uri(path).method(method);
        if !overrides_host {
            builder = builder.header("host", TEST_AUTHORITY);
        }
        for (name, value) in headers {
            builder = builder.header(*name, value.clone());
        }
        // `Content-Length` is set explicitly because a real client sends it and the
        // body-limit middleware reads it. `Request::builder()` does not add it, so a
        // test that omitted it would exercise a path no client takes.
        if !body.is_empty() {
            builder = builder.header("content-length", body.len().to_string());
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::from(body.to_owned())).expect("builds"))
            .await
            .expect("router responds");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body readable");
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// A create body without a policy reference, which is the ordinary case.
    ///
    /// The field is optional because only the daemon can resolve a workspace's active policy: the
    /// identifier is derived from the workspace, and the workspace is resolved server-side. An
    /// earlier version of this fixture sent `{"policy_id":"scripted-test","version":1}` — an
    /// identifier that is not a valid `ModelDataPolicyId`, so it was a policy no workspace could
    /// ever hold. It went unnoticed because the daemon ignored the field.
    fn create_body(text: &str) -> String {
        format!(
            r#"{{"conversation_id":null,"input":{{"type":"text","text":"{text}"}},"runtime":"jarvis-native"}}"#
        )
    }

    /// Creates a run and returns its identifier.
    async fn create_run(app: &axum::Router, token: &str, text: &str) -> String {
        let (status, body) = send(
            app,
            "POST",
            "/api/v1/runs",
            &run_headers(token),
            &create_body(text),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        parsed["run_id"].as_str().expect("a run id").to_owned()
    }

    #[tokio::test]
    async fn a_create_returns_the_contracts_accepted_shape() {
        let (app, token) = runs_fixture("runs-create").await;
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/runs",
            &run_headers(&token),
            &create_body("hello"),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");

        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        // The contract's response shape: identity, state, instant, and links.
        assert_eq!(parsed["state"], "received");
        assert!(
            parsed["conversation_id"]
                .as_str()
                .is_some_and(|id| id.len() == 36),
            "{body}",
        );
        assert!(
            parsed["created_at"]
                .as_str()
                .is_some_and(|at| at.ends_with('Z')),
            "{body}"
        );
        let run_id = parsed["run_id"].as_str().expect("a run id");
        assert_eq!(parsed["links"]["self"], format!("/api/v1/runs/{run_id}"));
        assert_eq!(
            parsed["links"]["events"],
            format!("/api/v1/runs/{run_id}/events"),
        );
        // The response must not carry the prompt, any provider configuration, or a
        // path.
        assert!(!body.contains("model_policy"), "{body}");
        assert!(!body.contains("/home"), "{body}");
    }

    #[tokio::test]
    async fn a_create_without_an_idempotency_key_is_refused_with_a_code() {
        // The contract requires the key, and its absence must be a named refusal rather
        // than a silently non-idempotent create.
        let (app, token) = runs_fixture("runs-no-key").await;
        let headers = vec![
            ("authorization", format!("Bearer {token}")),
            ("jarvis-api-version", "1".to_owned()),
            ("content-type", "application/json".to_owned()),
        ];
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/runs",
            &headers,
            &create_body("hello"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains(r#""code":"request.invalid""#), "{body}");
    }

    #[tokio::test]
    async fn a_create_with_an_unknown_field_is_refused() {
        // The contract rejects unknown command fields, and the refusal must be the
        // envelope rather than the extractor's plain text.
        let (app, token) = runs_fixture("runs-unknown-field").await;
        let body = r#"{"input":{"type":"text","text":"hi"},"runtime":"jarvis-native","model_policy":{"policy_id":"p","version":1},"api_key":"secret"}"#;
        let (status, response) =
            send(&app, "POST", "/api/v1/runs", &run_headers(&token), body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert!(
            response.contains(r#""code":"request.invalid""#),
            "{response}"
        );
        // The rejected value is caller-supplied text and must not be echoed.
        assert!(!response.contains("secret"), "{response}");
    }

    #[tokio::test]
    async fn an_unsupported_runtime_is_refused_rather_than_defaulted() {
        // A client that asked for an external runtime must not silently receive a
        // native one, or it would believe a capability it does not have is in use.
        let (app, token) = runs_fixture("runs-runtime").await;
        let body = r#"{"input":{"type":"text","text":"hi"},"runtime":"langgraph","model_policy":{"policy_id":"p","version":1}}"#;
        let (status, response) =
            send(&app, "POST", "/api/v1/runs", &run_headers(&token), body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            response.contains(r#""code":"request.semantic_invalid""#),
            "{response}",
        );
    }

    #[tokio::test]
    async fn a_created_run_records_the_runtime_that_executed_it() {
        // The runtime named in a create request was validated by the handler and then **discarded**:
        // `agent_runs.runtime_id` and `runtime_version` appeared in the schema and were referenced
        // by no code at all, so every row carried `NULL` while
        // `agent-runtime.md`'s resume step required "validate runtime identity/version".
        //
        // Asserted at the handler rather than only at the adapter, because the defect was in the
        // handoff: the service could store a runtime correctly while the handler never told it one.
        // Reading the row back through the repository is what makes that falsifiable — a handler
        // that accepted the field and dropped it again would pass any assertion on the response.
        let (app, token, repositories) = runs_fixture_with_storage("runs-runtime-recorded").await;
        let run_id = create_run(&app, &token, "hello").await;

        let stored = repositories
            .load(
                jarvis_domain::ids::WorkspaceId::from_uuid(uuid::Uuid::from_u128(
                    crate::http::runs::DEFAULT_WORKSPACE_UUID,
                )),
                jarvis_domain::ids::RunId::parse(&run_id).expect("the response carries an id"),
            )
            .await
            .expect("the created run loads");

        let runtime = stored.runtime.expect("the row records a runtime");
        assert_eq!(
            runtime.id,
            jarvis_application::repository::run::NATIVE_RUNTIME_ID,
            "the row must name the runtime that executed it",
        );
        // Asserted against the build's own version rather than merely non-empty. An earlier
        // version of this assertion was `!runtime.version.is_empty()`, and a mutation that read
        // the `runtime_id` column twice — so the version silently became the id — passed it. The
        // version is what a resume compares, so the check has to name the value.
        assert_eq!(
            runtime.version,
            env!("CARGO_PKG_VERSION"),
            "the version is the executing build's, which is what a resume compares",
        );
    }

    #[test]
    fn the_native_runtime_literal_matches_the_protocols() {
        // `jarvis-application` cannot depend on `jarvis-protocol` — the flow runs protocol ->
        // nothing app-side — so the identifier `jarvis-native` exists twice. This is the cross-check
        // the workspace applies to every such duplicated contract string, and it is asserted here
        // because this is a crate that can name both. Without it, editing one literal would leave
        // the app recording a runtime id no client recognises as the native one.
        assert_eq!(
            jarvis_application::repository::run::NATIVE_RUNTIME_ID,
            jarvis_protocol::run::NATIVE_RUNTIME,
            "the recorded runtime id and the contract's must be one value",
        );
    }

    #[tokio::test]
    async fn a_run_is_readable_after_creation_and_the_state_is_a_wire_state() {
        let (app, token) = runs_fixture("runs-read").await;
        let run_id = create_run(&app, &token, "hello").await;

        let (status, body) = send(
            &app,
            "GET",
            &format!("/api/v1/runs/{run_id}"),
            &run_headers(&token),
            "",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(parsed["run_id"], run_id);
        // The wire state set is coarser than the domain's, and this asserts the value
        // is one a client can act on rather than a domain name that leaked through.
        let state = parsed["state"].as_str().expect("a state");
        assert!(
            [
                "received",
                "context_building",
                "model_running",
                "responding",
                "completed",
                "failed",
                "cancelled",
            ]
            .contains(&state),
            "{state} is not a client-visible state",
        );
        assert!(parsed["version"].as_u64().is_some_and(|v| v >= 1), "{body}");
    }

    #[tokio::test]
    async fn an_unknown_run_is_not_found_and_a_malformed_id_is_indistinguishable() {
        // The contract requires a foreign run and a missing run to be
        // indistinguishable, and a malformed identifier must not be a different answer
        // either — that would tell a caller which identifiers exist.
        let (app, token) = runs_fixture("runs-not-found").await;
        let missing = "0195f4f0-4c13-7bf4-89fb-f067adac13ee";
        let (unknown_status, unknown_body) = send(
            &app,
            "GET",
            &format!("/api/v1/runs/{missing}"),
            &run_headers(&token),
            "",
        )
        .await;
        let (malformed_status, malformed_body) = send(
            &app,
            "GET",
            "/api/v1/runs/not-an-identifier",
            &run_headers(&token),
            "",
        )
        .await;
        assert_eq!(unknown_status, StatusCode::NOT_FOUND);
        assert_eq!(malformed_status, StatusCode::NOT_FOUND);
        assert_eq!(unknown_body, malformed_body);
        assert!(
            unknown_body.contains(r#""code":"resource.not_found""#),
            "{unknown_body}"
        );
    }

    #[tokio::test]
    async fn a_repeated_create_with_one_key_replays_the_same_run() {
        // The property the key exists for, asserted at the HTTP boundary where a client
        // actually relies on it.
        let (app, token) = runs_fixture("runs-idempotent").await;
        let headers = run_headers(&token);
        let body = create_body("hello");
        let (first_status, first) = send(&app, "POST", "/api/v1/runs", &headers, &body).await;
        let (second_status, second) = send(&app, "POST", "/api/v1/runs", &headers, &body).await;
        assert_eq!(first_status, StatusCode::ACCEPTED);
        assert_eq!(second_status, StatusCode::ACCEPTED);
        let first: serde_json::Value = serde_json::from_str(&first).expect("valid");
        let second: serde_json::Value = serde_json::from_str(&second).expect("valid");
        assert_eq!(first["run_id"], second["run_id"]);
        assert_eq!(first["conversation_id"], second["conversation_id"]);
    }

    #[tokio::test]
    async fn reusing_one_key_for_different_input_is_a_conflict() {
        let (app, token) = runs_fixture("runs-conflict").await;
        let headers = run_headers(&token);
        send(
            &app,
            "POST",
            "/api/v1/runs",
            &headers,
            &create_body("hello"),
        )
        .await;
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/runs",
            &headers,
            &create_body("something else"),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains(r#""code":"idempotency.conflict""#), "{body}");
    }

    #[tokio::test]
    async fn the_event_stream_uses_sse_framing_and_one_terminal_event() {
        let (app, token) = runs_fixture("runs-stream").await;
        let run_id = create_run(&app, &token, "hello").await;

        // The run is driven on a real runtime, so the stream is polled until it reports
        // a terminal state. Polling rather than sleeping keeps the test fast and
        // deterministic in what it asserts, without asserting a timing.
        let mut body = String::new();
        for _ in 0..200 {
            let (status, current) = send(
                &app,
                "GET",
                &format!("/api/v1/runs/{run_id}/events"),
                &run_headers(&token),
                "",
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{current}");
            body = current;
            if body.contains("event: run.completed")
                || body.contains("event: run.failed")
                || body.contains("event: run.cancelled")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Framing: an `id:`, an `event:`, a `data:` line, and a blank line terminator.
        assert!(body.contains("\nid: "), "{body}");
        assert!(body.contains("\nevent: "), "{body}");
        assert!(body.contains("\ndata: {"), "{body}");
        assert!(body.ends_with("\n\n"), "{body}");

        // The first event is the run's opening event at sequence 1.
        assert!(body.contains("event: run.received"), "{body}");
        assert!(body.contains(r#""sequence":1"#), "{body}");

        // Exactly one terminal event, and it is the completion.
        let terminals = ["run.completed", "run.failed", "run.cancelled"]
            .iter()
            .map(|kind| body.matches(kind).count())
            .sum::<usize>();
        assert_eq!(terminals, 1, "{body}");
        assert!(body.contains("event: run.completed"), "{body}");
        // The delta reached the stream, which is what a streaming client is for.
        assert!(body.contains("event: run.output_text.delta"), "{body}");
    }

    #[tokio::test]
    async fn an_unavailable_resume_position_is_a_conflict_not_a_silent_restart() {
        // The contract is explicit: a missing or no-longer-retained position returns
        // `409` rather than silently skipping a gap.
        let (app, token) = runs_fixture("runs-resume").await;
        let run_id = create_run(&app, &token, "hello").await;
        let mut headers = run_headers(&token);
        headers.push((
            "last-event-id",
            "0195f4f1-0475-7613-a92c-edf01183e909".to_owned(),
        ));
        let (status, body) = send(
            &app,
            "GET",
            &format!("/api/v1/runs/{run_id}/events"),
            &headers,
            "",
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            body.contains(r#""code":"stream.replay_unavailable""#),
            "{body}",
        );
    }

    #[tokio::test]
    async fn a_cancel_reason_is_bounded_by_the_constant_declared_for_it() {
        // `MAX_CANCEL_REASON_BYTES` was **declared and never enforced** while `MAX_RUN_INPUT_BYTES`
        // is applied one route above it. A declared bound that nothing checks is the shape where
        // the constant itself reads as the coverage, so this asserts the refusal rather than the
        // constant's existence — and the run input's own bound is asserted the same way beside it.
        let (app, token) = runs_fixture("runs-cancel-reason-bound").await;
        let run_id = create_run(&app, &token, "hello").await;
        let headers = run_headers(&token);

        let over = "r".repeat(jarvis_protocol::MAX_CANCEL_REASON_BYTES + 1);
        let body = serde_json::json!({ "reason": over }).to_string();
        let (status, response) = send(
            &app,
            "POST",
            &format!("/api/v1/runs/{run_id}/cancel"),
            &headers,
            &body,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "an over-long reason must be refused: {response}",
        );
        assert!(
            response.contains(r#""code":"request.invalid""#),
            "the refusal must use the contract's code: {response}",
        );

        // Exactly at the bound is accepted, so the check is a bound rather than an
        // approximation of one.
        let at_bound = "r".repeat(jarvis_protocol::MAX_CANCEL_REASON_BYTES);
        let body = serde_json::json!({ "reason": at_bound }).to_string();
        let (status, response) = send(
            &app,
            "POST",
            &format!("/api/v1/runs/{run_id}/cancel"),
            &headers,
            &body,
        )
        .await;
        assert!(
            status == StatusCode::ACCEPTED || status == StatusCode::OK,
            "a reason at the bound must be accepted: {status}: {response}",
        );

        // And an empty one is refused, because the contract requires a reason on a cancel: an
        // absent explanation is not the same as a stated one, and the endpoint already defaults
        // the *omitted body* to `user_requested`.
        let (status, response) = send(
            &app,
            "POST",
            &format!("/api/v1/runs/{run_id}/cancel"),
            &headers,
            r#"{"reason":""}"#,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "an empty reason must be refused: {response}",
        );
    }

    #[tokio::test]
    async fn a_failed_runs_terminal_event_carries_its_code() {
        // The three payload builders in `jarvis_protocol` had no caller and every activity event
        // was written with `payload_json: None`, so a client following the event stream learned
        // that a run failed without learning why — while the code sat on the run's own row, which
        // requires a *separate* read. The terminal event is the last thing such a client receives,
        // which makes it the wrong place to omit the answer.
        //
        // Asserted on the **stored events** rather than on the wire, because the handler renders
        // whatever the repository holds: a payload that never reached the row could not be
        // delivered however the handler rendered it.
        let (app, token, _repositories) =
            runs_fixture_with("runs-terminal-payload", FixtureProvider::Refuses).await;
        // A provider that always refuses fails the run before acceptance, which is the path that
        // reaches `Step::failed` with a code.
        let run_id = create_run(&app, &token, "hello").await;
        let headers = run_headers(&token);
        for _ in 0..200 {
            let (_, current) =
                send(&app, "GET", &format!("/api/v1/runs/{run_id}"), &headers, "").await;
            let parsed: serde_json::Value = serde_json::from_str(&current).expect("valid");
            if ["completed", "failed", "cancelled"]
                .contains(&parsed["state"].as_str().unwrap_or_default())
            {
                break;
            }
        }

        let (_, events) = send(
            &app,
            "GET",
            &format!("/api/v1/runs/{run_id}/events"),
            &headers,
            "",
        )
        .await;
        assert!(
            events.contains("run.failed"),
            "the fixture must reach a failed terminal: {events}",
        );
        assert!(
            events.contains(r#""code":"#),
            "a failed run's terminal event must carry its code: {events}",
        );
        // And the code on the event is the one the run's own row reports, so a streaming client
        // and a polling client cannot be told different reasons.
        let (_, current) = send(&app, "GET", &format!("/api/v1/runs/{run_id}"), &headers, "").await;
        let parsed: serde_json::Value = serde_json::from_str(&current).expect("valid");
        let code = parsed["error_code"].as_str().expect("a failed run's code");
        assert!(
            events.contains(code),
            "the event must carry the run's own code ({code}): {events}",
        );
    }

    #[tokio::test]
    async fn the_stored_failure_payload_matches_the_protocol_builders_shape() {
        // The application layer hand-builds this payload because it has no JSON *dependency*, and
        // `jarvis-protocol` cannot be depended on from there — the flow runs protocol -> nothing
        // app-side. That leaves **two** definitions of one wire shape: the hand-built string and
        // `jarvis_protocol::run::failed_payload`. This is the cross-check the workspace uses for
        // every such duplicated literal (the event-name constants have the same test), and it is
        // cheap: without it, editing one would silently disagree with the other, and a client
        // parsing the shape would see a field only one producer emits.
        //
        // Asserted over every terminal code the controller can produce, so the class fails rather
        // than one instance.
        for code in [
            "run.no_model_served",
            "run.context_objective_dropped",
            "run.stream_interrupted",
            "run.budget_output_tokens_exceeded",
            "model.provider_refused",
        ] {
            let hand_built = format!("{{\"code\":\"{code}\",\"retryable\":false}}");
            let from_protocol =
                serde_json::to_string(&jarvis_protocol::run::failed_payload(code, false))
                    .expect("the builder serializes");
            assert_eq!(
                hand_built, from_protocol,
                "the hand-built payload and the protocol's builder must agree on the shape",
            );
        }
    }

    #[tokio::test]
    async fn the_usage_event_type_and_payload_match_the_protocol_shape() {
        use jarvis_domain::model::stream::Usage;
        // The application layer cannot depend on `jarvis-protocol`, so the `run.usage` event type
        // and its payload shape exist twice: once as a local literal and hand-built string in
        // `jarvis_application::run_controller`, once as `jarvis_protocol::event_type::USAGE` and
        // `jarvis_protocol::run::usage_payload`. This is the cross-check the workspace applies to
        // every such duplicated contract string, and both halves are asserted because either
        // drifting breaks a different kind of client: a wrong event type is ignored as unknown, a
        // wrong field name is read as absent.
        assert_eq!(
            jarvis_application::run_controller::USAGE_EVENT_TYPE,
            jarvis_protocol::event_type::USAGE,
            "an event type is what a client switches on",
        );

        // The field **names** must agree, asserted by parsing both as JSON and comparing key sets
        // for the case both shapes can express — a provider that reported both counters.
        let both = Usage {
            input_tokens: Some(11),
            output_tokens: Some(22),
            provider_reported: true,
            ..Usage::default()
        };
        let hand_built: serde_json::Value = serde_json::from_str(
            &jarvis_application::run_controller::usage_payload_for_wire(&both),
        )
        .expect("the hand-built payload is valid JSON");
        let from_protocol = jarvis_protocol::run::usage_payload(11, 22);
        for key in ["input_tokens", "output_tokens"] {
            assert_eq!(
                hand_built.get(key),
                from_protocol.get(key),
                "both shapes must carry {key} with the same value",
            );
        }

        // And a counter the provider did **not** report is omitted from the hand-built shape rather
        // than written as zero — the contract's "unknown is not zero", which the protocol builder
        // cannot express at all because it takes plain integers. That difference is the reason the
        // hand-built one exists, so it is asserted rather than left implicit.
        let only_output = Usage {
            output_tokens: Some(22),
            provider_reported: true,
            ..Usage::default()
        };
        let omitted: serde_json::Value = serde_json::from_str(
            &jarvis_application::run_controller::usage_payload_for_wire(&only_output),
        )
        .expect("valid JSON");
        assert_eq!(omitted.get("output_tokens"), Some(&serde_json::json!(22)));
        assert!(
            omitted.get("input_tokens").is_none(),
            "an unreported counter must be absent, not zero: {omitted}",
        );
    }

    #[tokio::test]
    async fn a_cancel_reaches_a_terminal_state_and_is_truthful_about_the_timing() {
        let (app, token) = runs_fixture("runs-cancel").await;
        let run_id = create_run(&app, &token, "hello").await;
        let headers = run_headers(&token);
        let (status, body) = send(
            &app,
            "POST",
            &format!("/api/v1/runs/{run_id}/cancel"),
            &headers,
            r#"{"reason":"user_requested"}"#,
        )
        .await;
        // `202` while cleanup is in flight, never a claim that the run is already
        // cancelled. `200` is allowed only when the run had genuinely finished before the
        // command arrived, which the body's state then shows.
        assert!(
            status == StatusCode::ACCEPTED || status == StatusCode::OK,
            "{status}: {body}",
        );
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        let reported = parsed["state"].as_str().expect("a state");
        assert!(
            status != StatusCode::OK || ["completed", "failed", "cancelled"].contains(&reported),
            "a 200 must report a terminal state: {body}",
        );

        // And the run reaches a terminal state that a client can observe. **This
        // assertion originally accepted any terminal state and used a permissive loop**,
        // which accepted `completed` for a run that was cancelled — encoding the very
        // defect the contract prohibits. A real-daemon journey is what exposed it: the
        // scripted provider finishes in milliseconds, so a cancel that arrives after
        // completion must not be reported as having cancelled anything, and a cancel that
        // arrives *during* the run must end it `Cancelled` rather than `Completed`.
        let mut observed = None;
        for _ in 0..200 {
            let (_, current) = send(
                &app,
                "GET",
                &format!("/api/v1/runs/{run_id}"),
                &run_headers(&token),
                "",
            )
            .await;
            let parsed: serde_json::Value = serde_json::from_str(&current).expect("valid");
            let state = parsed["state"].as_str().unwrap_or_default().to_owned();
            if ["completed", "failed", "cancelled"].contains(&state.as_str()) {
                observed = Some(state);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let observed = observed.expect("a cancelled run must reach a terminal state");
        // The command was accepted as `202`, so the run was live when it arrived and the
        // only truthful terminal for it is `cancelled`. `completed` would mean the
        // cancellation was signalled and then ignored.
        if status == StatusCode::ACCEPTED {
            assert_eq!(
                observed, "cancelled",
                "a run whose cancel was accepted as in-flight must end cancelled, not {observed}",
            );
        }
    }

    #[tokio::test]
    async fn the_run_surface_requires_authentication_like_every_other_api_route() {
        // A run route must not be reachable without a credential. The extractor makes
        // this a property of the handler signature, and this asserts it on the wire.
        let (app, _token) = runs_fixture("runs-auth").await;
        for (method, path) in [
            ("POST", "/api/v1/runs"),
            ("GET", "/api/v1/runs/0195f4f0-4c13-7bf4-89fb-f067adac13ee"),
            (
                "POST",
                "/api/v1/runs/0195f4f0-4c13-7bf4-89fb-f067adac13ee/cancel",
            ),
            (
                "GET",
                "/api/v1/runs/0195f4f0-4c13-7bf4-89fb-f067adac13ee/events",
            ),
        ] {
            let headers = vec![
                ("host", TEST_AUTHORITY.to_owned()),
                ("jarvis-api-version", "1".to_owned()),
                ("content-type", "application/json".to_owned()),
            ];
            let (status, body) = send(&app, method, path, &headers, &create_body("hello")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}: {body}");
            assert!(
                body.contains(r#""code":"auth.credential_rejected""#),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn the_run_surface_rejects_a_browser_origin_and_a_forwarded_header() {
        // The checks apply to the run routes as they do everywhere else; a new route is
        // exactly where a missing layer would go unnoticed.
        let (app, token) = runs_fixture("runs-origin").await;
        for extra in [
            ("origin", "https://evil.invalid".to_owned()),
            ("x-forwarded-host", "evil.invalid".to_owned()),
        ] {
            let mut headers = run_headers(&token);
            headers.push(extra);
            let (status, body) = send(
                &app,
                "POST",
                "/api/v1/runs",
                &headers,
                &create_body("hello"),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert!(
                body.starts_with('{'),
                "the refusal must be the envelope: {body}"
            );
        }
    }

    #[tokio::test]
    async fn the_run_surface_rejects_a_body_over_the_bound_before_authentication() {
        // The body cap is outermost, so an oversized body is reported as
        // `request.too_large` regardless of the credentials presented.
        let (app, _token) = runs_fixture("runs-too-large").await;
        let headers = vec![
            ("host", TEST_AUTHORITY.to_owned()),
            ("jarvis-api-version", "1".to_owned()),
        ];
        let oversized = "x".repeat(70 * 1024);
        let (status, body) = send(&app, "POST", "/api/v1/runs", &headers, &oversized).await;
        assert_eq!(
            status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "{}",
            &body[..body.len().min(200)]
        );
        assert!(body.contains(r#""code":"request.too_large""#), "{body}");
    }

    #[tokio::test]
    async fn a_run_surface_without_storage_answers_not_ready_rather_than_missing() {
        // A client must be able to tell "this endpoint exists but the daemon cannot
        // serve it" from "there is no such endpoint", so the refusal is a named code and
        // not the unknown-route answer.
        let dir = temp_dir("runs-no-storage");
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
        let mut clients = ClientRegistry::new();
        clients.register(registered);
        let state = Arc::new(ApiState::new(
            Arc::new(clients),
            Arc::new(Readiness::new()),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
        ));
        let app = router(state);
        let token = credential.to_presentation_text();
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/runs",
            &run_headers(&token),
            &create_body("hello"),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains(r#""code":"service.not_ready""#), "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_wire_state_projection_is_total_and_coarser_than_the_domain() {
        // Every domain state must have a wire state: a missing arm would be a panic on
        // the request path, and `total` is a claim worth checking rather than assuming.
        use jarvis_domain::run::state::RunState;
        let all = [
            RunState::Received,
            RunState::ContextBuilding,
            RunState::Planning,
            RunState::AwaitingModel,
            RunState::AwaitingApproval,
            RunState::ExecutingTool,
            RunState::Observing,
            RunState::Waiting,
            RunState::Responding,
            RunState::Completed,
            RunState::Failed,
            RunState::Cancelled,
        ];
        let wire: Vec<&str> = all.iter().copied().map(super::wire_state).collect();
        // Coarser: twelve domain states map to at most seven wire states.
        let unique: std::collections::BTreeSet<&str> = wire.iter().copied().collect();
        assert!(unique.len() < all.len(), "{wire:?}");
        assert_eq!(unique.len(), 7, "{unique:?}");
        // The terminal states are one-to-one, so a client never sees a finished run
        // described by a non-terminal wire state.
        assert_eq!(super::wire_state(RunState::Completed), "completed");
        assert_eq!(super::wire_state(RunState::Failed), "failed");
        assert_eq!(super::wire_state(RunState::Cancelled), "cancelled");
    }

    /// A policy surface fixture backed by a real migrated database.
    ///
    /// A real store rather than a double, for the same reason the run fixture uses one: these
    /// tests exist to falsify the boundary, and a double at the storage layer would let a scope
    /// or serialization defect pass. The workspace is a parameter because cross-workspace
    /// isolation is one of the properties under test, and it can only be tested by asking as
    /// someone else.
    async fn policy_fixture(tag: &str) -> (axum::Router, String, Arc<SqliteRepositories>) {
        use crate::storage::repositories::SqliteRepositories;
        use crate::storage::{Database, migrate};

        let dir = temp_dir(tag);
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
        let mut clients = ClientRegistry::new();
        clients.register(registered);

        let database = Database::open_in_memory().await.expect("in-memory opens");
        migrate::run(database.pool()).await.expect("migrates");
        let repositories = Arc::new(SqliteRepositories::new(database.pool().clone()));

        // A provider serving one model, reporting itself as a **cloud** endpoint. Chosen
        // because a `LocalOnly` policy has to refuse it, which makes the refusal arm reachable;
        // a local provider here would make the refusal test pass for the wrong reason, since a
        // policy that was never read would also permit a local candidate.
        let provider = jarvis_application::model::ScriptedProvider::new(model_ref())
            .with_endpoint_class(jarvis_domain::model::identity::EndpointClass::ApprovedCloud);
        let policies = Arc::new(PolicyService::new(
            Arc::clone(&repositories) as Arc<dyn ModelDataPolicyRepository>
        ));
        let inventory = Arc::new(ProviderInventory::new(&provider, Some(policy_now())));

        let state = Arc::new(
            ApiState::new(
                Arc::new(clients),
                Arc::new(Readiness::new()),
                "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
            )
            .with_policies(policies, inventory),
        );

        (
            router(state),
            credential.to_presentation_text(),
            repositories,
        )
    }

    fn model_ref() -> jarvis_domain::model::identity::ModelRef {
        use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
        ModelRef::new(
            ProviderId::parse("scripted.local").expect("valid"),
            ModelId::parse("fixture-1").expect("valid"),
        )
    }

    /// Authenticated headers for a policy read.
    ///
    /// Carries the API version because every `/api/*` route requires it: a read that omitted it
    /// would be refused as `api.version_unsupported` before reaching the handler, so a test built
    /// on these headers would pass while asserting nothing about the policy surface.
    fn policy_headers(token: &str) -> Vec<(&str, String)> {
        vec![
            ("authorization", format!("Bearer {token}")),
            ("jarvis-api-version", "1".to_owned()),
        ]
    }

    /// Reads a policy endpoint as an authenticated local client.
    async fn policy_get(app: &axum::Router, token: &str, path: &str) -> (StatusCode, String) {
        send(app, "GET", path, &policy_headers(token), "").await
    }

    /// The instant the fixtures evaluate against.
    fn policy_now() -> jarvis_domain::time::UtcTimestamp {
        jarvis_domain::time::UtcTimestamp::parse("2026-09-21T00:00:00Z").expect("valid")
    }

    fn policy_id(value: u128) -> jarvis_domain::ids::ModelDataPolicyId {
        jarvis_domain::ids::ModelDataPolicyId::from_uuid(uuid::Uuid::from_u128(value))
    }

    /// The ruleset the policy fixtures store: a locality rule that a cloud route fails.
    ///
    /// The locality rule is the discriminating part. Every other field is left at
    /// [`PolicyRules::permissive`]'s value on purpose: the inventory attaches no retention or
    /// training-use evidence (`BRN-011` has not measured any), so a ruleset that demanded
    /// *documented* evidence would refuse every candidate and make the compliant arm of the
    /// route test unreachable — a test that cannot show a policy permitting anything does not
    /// show a policy deciding.
    fn local_only_rules() -> jarvis_domain::model::policy::PolicyRules {
        use jarvis_domain::model::policy::{Locality, PolicyRules};
        PolicyRules {
            locality: Locality::LocalOnly,
            ..PolicyRules::permissive()
        }
    }

    /// Stores an **active** policy version in the workspace the API resolves.
    ///
    /// The workspace is [`runs::DEFAULT_WORKSPACE_UUID`] rather than an arbitrary identifier,
    /// because the handler derives the scope server-side from the authenticated client. Seeding
    /// any other workspace would make every test read "no policy in force" and pass a
    /// `policy_not_found` assertion for the wrong reason.
    async fn seed_policy(
        repositories: &SqliteRepositories,
        policy_id: jarvis_domain::ids::ModelDataPolicyId,
        version: u32,
    ) {
        seed_policy_in_workspace(
            repositories,
            policy_id,
            version,
            jarvis_domain::ids::WorkspaceId::from_uuid(uuid::Uuid::from_u128(
                crate::http::runs::DEFAULT_WORKSPACE_UUID,
            )),
            "local-only",
        )
        .await;
    }

    /// Stores an active policy version owned by `workspace`.
    async fn seed_policy_in_workspace(
        repositories: &SqliteRepositories,
        policy_id: jarvis_domain::ids::ModelDataPolicyId,
        version: u32,
        workspace: jarvis_domain::ids::WorkspaceId,
        name: &str,
    ) {
        use jarvis_application::repository::policy::NewPolicyVersion;
        use jarvis_domain::model::policy::ModelDataPolicyStatus;

        repositories
            .insert_version(NewPolicyVersion {
                policy_id,
                version,
                workspace_id: workspace,
                name: name.to_owned(),
                status: ModelDataPolicyStatus::Active,
                rules: local_only_rules(),
                created_at: policy_now(),
            })
            .await
            .expect("policy inserts");
    }

    #[tokio::test]
    async fn the_active_policy_endpoint_reports_the_stored_rules() {
        let (app, token, repositories) = policy_fixture("policy-active").await;
        seed_policy(&repositories, policy_id(1), 1).await;

        let (status, body) = policy_get(&app, &token, "/api/v1/model-data-policy").await;

        assert_eq!(status, StatusCode::OK);
        // The contract's spellings, not the domain's `Display`: a wire value is a published
        // identifier and a rename in the domain must not silently rename it.
        assert!(body.contains(r#""status":"active""#), "{body}");
        assert!(body.contains(r#""locality":"local_only""#), "{body}");
        assert!(body.contains(r#""version":1"#), "{body}");
        let _ = std::fs::remove_dir_all(temp_dir("policy-active"));
    }

    #[tokio::test]
    async fn the_active_policy_endpoint_reports_absence_rather_than_inventing_one() {
        let (app, token, _) = policy_fixture("policy-none").await;

        let (status, body) = policy_get(&app, &token, "/api/v1/model-data-policy").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        // A workspace with no policy is a real state. Reporting a permissive default instead
        // would say "everything is allowed" on an operator's behalf who never said so.
        assert!(body.contains("model.policy_not_found"), "{body}");
        let _ = std::fs::remove_dir_all(temp_dir("policy-none"));
    }

    /// A submission body, so the write tests differ only in the fields they vary.
    fn policy_body(expected_version: u32, locality: &str) -> String {
        format!(
            r#"{{"name":"operator policy","expected_version":{expected_version},"rules":{{"locality":"{locality}","maximum_provider_retention":"provider_default_allowed","provider_training_use":"provider_default_allowed","telemetry":"local_only","allowed_residency_regions":[],"maximum_sensitivity":"confidential","allow_fallback":"denied"}}}}"#
        )
    }

    /// Writes a policy as an authenticated local client.
    async fn policy_put(
        app: &axum::Router,
        token: &str,
        body: &str,
        key: Option<&str>,
    ) -> (StatusCode, String) {
        let mut headers = policy_headers(token);
        headers.push(("content-type", "application/json".to_owned()));
        if let Some(key) = key {
            headers.push(("idempotency-key", key.to_owned()));
        }
        send(app, "PUT", "/api/v1/model-data-policy", &headers, body).await
    }

    #[tokio::test]
    async fn a_put_creates_the_first_policy_and_reports_the_version_it_chose() {
        let (app, token, _) = policy_fixture("policy-put-first").await;

        let (status, body) =
            policy_put(&app, &token, &policy_body(0, "local_only"), Some("key-1")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        // Version 1, chosen by the daemon: the request carried no version at all, and the reply
        // is where a client learns what it created. A client that had named the version could
        // collide or skip, which is why the shape has no such field.
        assert!(body.contains(r#""version":1"#), "{body}");
        assert!(body.contains(r#""locality":"local_only""#), "{body}");

        // And it is in force, read back through the other route rather than trusted from the
        // write's own response — a write that answered correctly without storing anything would
        // pass a single-request assertion.
        let (read_status, read_body) = policy_get(&app, &token, "/api/v1/model-data-policy").await;
        assert_eq!(read_status, StatusCode::OK, "{read_body}");
        assert!(read_body.contains(r#""version":1"#), "{read_body}");
        let _ = std::fs::remove_dir_all(temp_dir("policy-put-first"));
    }

    #[tokio::test]
    async fn a_put_without_an_idempotency_key_is_refused_before_it_writes() {
        let (app, token, _) = policy_fixture("policy-put-nokey").await;

        let (status, body) = policy_put(&app, &token, &policy_body(0, "local_only"), None).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("request.invalid"), "{body}");
        // Nothing was written, so a client that retries with a key starts from version 0 rather
        // than discovering a version it never intended to create.
        let (read_status, _) = policy_get(&app, &token, "/api/v1/model-data-policy").await;
        assert_eq!(read_status, StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(temp_dir("policy-put-nokey"));
    }

    #[tokio::test]
    async fn a_put_refuses_a_locality_this_build_does_not_support() {
        let (app, token, _) = policy_fixture("policy-put-badvalue").await;

        let (status, body) = policy_put(
            &app,
            &token,
            &policy_body(0, "send_it_anywhere"),
            Some("key-1"),
        )
        .await;

        // A refusal rather than a stored-but-unapplied rule. The merge can only narrow a rule it
        // understands, so an unrecognized value would be one this layer wrote and the selector
        // never enforced — which is the edit direction that widens a policy.
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("request.semantic_invalid"), "{body}");
        let (read_status, _) = policy_get(&app, &token, "/api/v1/model-data-policy").await;
        assert_eq!(read_status, StatusCode::NOT_FOUND, "nothing was stored");
        let _ = std::fs::remove_dir_all(temp_dir("policy-put-badvalue"));
    }

    #[tokio::test]
    async fn a_stale_put_is_a_conflict_and_leaves_the_policy_in_force() {
        let (app, token, _) = policy_fixture("policy-put-stale").await;
        policy_put(&app, &token, &policy_body(0, "local_only"), Some("key-1"))
            .await
            .0
            .is_success()
            .then_some(())
            .expect("the first write succeeds");

        // The caller still believes nothing exists. A conflict, not a second version created on
        // a stale base.
        let (status, body) = policy_put(
            &app,
            &token,
            &policy_body(0, "approved_cloud_allowed"),
            Some("key-2"),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body.contains("resource.version_conflict"), "{body}");
        // The submitted rules were LOOSER than what is in force, and the stored policy must be
        // untouched — asserting only the status would pass against an implementation that wrote
        // the row and then reported a conflict.
        let (_, read_body) = policy_get(&app, &token, "/api/v1/model-data-policy").await;
        assert!(read_body.contains(r#""version":1"#), "{read_body}");
        assert!(
            read_body.contains(r#""locality":"local_only""#),
            "{read_body}"
        );
        let _ = std::fs::remove_dir_all(temp_dir("policy-put-stale"));
    }

    #[tokio::test]
    async fn a_put_cannot_widen_the_policy_already_in_force() {
        let (app, token, _) = policy_fixture("policy-put-widen").await;
        let (first, first_body) =
            policy_put(&app, &token, &policy_body(0, "local_only"), Some("key-1")).await;
        assert_eq!(first, StatusCode::OK, "{first_body}");

        // Version 2 is accepted — the precondition is satisfied — but the merge keeps the
        // stricter locality. This is the assertion that makes the write endpoint safe without an
        // approval step: a request body can only narrow a workspace policy.
        let (second, second_body) = policy_put(
            &app,
            &token,
            &policy_body(1, "approved_cloud_allowed"),
            Some("key-2"),
        )
        .await;

        assert_eq!(second, StatusCode::OK, "{second_body}");
        assert!(second_body.contains(r#""version":2"#), "{second_body}");
        assert!(
            second_body.contains(r#""locality":"local_only""#),
            "a submission must not widen the policy in force: {second_body}",
        );
        assert!(
            !second_body.contains("approved_cloud_allowed"),
            "{second_body}",
        );
        let _ = std::fs::remove_dir_all(temp_dir("policy-put-widen"));
    }

    #[tokio::test]
    async fn a_read_after_a_write_describes_the_same_stored_row() {
        // A client that PUTs and immediately GETs must see one document, so the two paths render
        // through one function. Reading back the allow-lists matters too: they were absent on the
        // write, and the read must not turn "this layer restricts no provider" into an empty set
        // that would narrow the policy to nothing on a read-modify-write.
        let (app, token, _) = policy_fixture("policy-round-trip").await;
        let (put_status, put_body) =
            policy_put(&app, &token, &policy_body(0, "local_only"), Some("key-1")).await;
        assert_eq!(put_status, StatusCode::OK, "{put_body}");

        let (get_status, get_body) = policy_get(&app, &token, "/api/v1/model-data-policy").await;
        assert_eq!(get_status, StatusCode::OK, "{get_body}");

        assert!(!put_body.contains("allowed_providers"), "{put_body}");
        assert!(!get_body.contains("allowed_providers"), "{get_body}");
        assert!(
            get_body.contains(r#""allow_fallback":"denied""#),
            "{get_body}"
        );
        // The read must agree with the write on every rule it reports, so the two bodies cannot
        // describe different stored state.
        for fragment in [
            r#""locality":"local_only""#,
            r#""maximum_sensitivity":"confidential""#,
            r#""allow_fallback":"denied""#,
        ] {
            assert!(put_body.contains(fragment), "write: {put_body}");
            assert!(get_body.contains(fragment), "read: {get_body}");
        }
        let _ = std::fs::remove_dir_all(temp_dir("policy-round-trip"));
    }

    #[tokio::test]
    async fn the_put_route_requires_authentication_like_every_other_api_route() {
        let (app, _, _) = policy_fixture("policy-put-auth").await;

        let (status, body) = send(
            &app,
            "PUT",
            "/api/v1/model-data-policy",
            &[
                ("jarvis-api-version", "1".to_owned()),
                ("content-type", "application/json".to_owned()),
                ("idempotency-key", "key-1".to_owned()),
            ],
            &policy_body(0, "local_only"),
        )
        .await;

        // A write is the operation where a missing authorization check is worst: an unauthenticated
        // caller could install rules for a workspace it does not own.
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        let _ = std::fs::remove_dir_all(temp_dir("policy-put-auth"));
    }

    #[tokio::test]
    async fn the_effective_route_endpoint_refuses_a_candidate_the_policy_excludes() {
        let (app, token, repositories) = policy_fixture("policy-refused").await;
        seed_policy(&repositories, policy_id(1), 1).await;

        let (status, body) = policy_get(&app, &token, "/api/v1/model-data-policy/effective").await;

        // A refusal is a successful evaluation, so it is a 200 with the refusal in the body
        // rather than an error status. Returning 4xx would tell a client its request was
        // malformed when the request was fine and the *policy* was the thing that said no.
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("model.policy_unsatisfied"), "{body}");
        // The rejected candidate must be named, so a caller can see *which* model failed and
        // not merely that something did.
        assert!(
            body.contains(r#""model":"scripted.local/fixture-1""#),
            "{body}"
        );
        // The **code**, not the prose: this assertion is what distinguishes a rendered code from
        // the domain's `Display`, which says "locality violated" and would satisfy a client
        // looking for "locality" while breaking one switching on the contract's vocabulary.
        assert!(body.contains(r#""reason":"locality_violated""#), "{body}");
        // `compliant` is **omitted**, not `null`: the response type skips the field when it is
        // absent, so a client's `Option` reads `None` either way and the body carries no
        // placeholder route that could be mistaken for a selection.
        assert!(!body.contains("compliant"), "{body}");
        let _ = std::fs::remove_dir_all(temp_dir("policy-refused"));
    }

    #[tokio::test]
    async fn the_effective_route_endpoint_serves_a_local_candidate_under_a_local_only_policy() {
        // The same policy and the same provider as the refusal test, so the only difference is
        // the candidate's endpoint class. Without this the refusal test could pass because the
        // route table is broken rather than because the policy excluded a cloud model.
        //
        // The provider is one whose endpoint class is `Local` rather than the fixture's, since
        // that is the property under test. Nothing else about it differs.
        use jarvis_domain::model::identity::EndpointClass;

        let dir = temp_dir("policy-compliant");
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
        let mut clients = ClientRegistry::new();
        clients.register(registered);

        let database = crate::storage::Database::open_in_memory()
            .await
            .expect("in-memory opens");
        crate::storage::migrate::run(database.pool())
            .await
            .expect("migrates");
        let repositories = Arc::new(SqliteRepositories::new(database.pool().clone()));
        seed_policy(&repositories, policy_id(1), 1).await;

        // `ScriptedProvider` reports `EndpointClass::Local`, and the fixture policy is
        // `LocalOnly`, so the one candidate must be selected. A policy that demanded documented
        // retention would refuse it instead; this ruleset does not, which is what makes the
        // compliant arm reachable at all.
        let provider = jarvis_application::model::ScriptedProvider::new(model_ref());
        assert_eq!(
            jarvis_application::model::ModelProvider::endpoint_class(&provider),
            EndpointClass::Local,
            "the compliant arm depends on this provider being local"
        );
        let inventory = Arc::new(ProviderInventory::new(&provider, Some(policy_now())));
        let policies = Arc::new(PolicyService::new(
            repositories as Arc<dyn ModelDataPolicyRepository>,
        ));
        let state = Arc::new(
            ApiState::new(
                Arc::new(clients),
                Arc::new(Readiness::new()),
                "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
            )
            .with_policies(policies, inventory),
        );
        let app = router(state);

        let (status, body) = policy_get(
            &app,
            &credential.to_presentation_text(),
            "/api/v1/model-data-policy/effective",
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains(r#""compliant":{"#), "{body}");
        assert!(!body.contains("error_code"), "{body}");
        assert!(body.contains("fixture-1"), "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn both_policy_endpoints_report_not_ready_without_a_store() {
        // The daemon starts serving health before it has migrations or a policy store, so the
        // unconfigured arm is a real startup state and not a defensive branch. It must be
        // `service.not_ready` rather than a 500: nothing failed, the daemon is not up yet.
        let fixture = fixture("policy-unwired");
        for path in [
            "/api/v1/model-data-policy",
            "/api/v1/model-data-policy/effective",
        ] {
            let (status, body) = policy_get(&fixture.app, &fixture.token, path).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {body}");
            assert!(body.contains("service.not_ready"), "{path}: {body}");
        }
        let _ = std::fs::remove_dir_all(&fixture.dir);
    }

    #[tokio::test]
    async fn the_policy_surface_hides_another_workspaces_policy() {
        // The client's workspace is resolved server-side and is always the nil workspace in
        // these fixtures, so a policy owned by a different workspace must read as absent. This
        // is the local-control-API rule that another scope's resource is indistinguishable from
        // a missing one, and it is worth a test because "404 for both" is also what a broken
        // query returns.
        use jarvis_domain::ids::WorkspaceId;

        let (app, token, repositories) = policy_fixture("policy-scope").await;
        seed_policy_in_workspace(
            &repositories,
            policy_id(1),
            1,
            WorkspaceId::from_uuid(uuid::Uuid::from_u128(0xABCD)),
            "someone-elses",
        )
        .await;

        let (status, body) = policy_get(&app, &token, "/api/v1/model-data-policy").await;

        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(body.contains("model.policy_not_found"), "{body}");
        // The other workspace's name must not leak into the response either.
        assert!(!body.contains("someone-elses"), "{body}");

        // The row genuinely exists and genuinely belongs to the other workspace, so the 404 is
        // scope filtering rather than a failed insert. Reading it back by its own workspace is
        // what makes that distinction; without it the test would pass if `insert_version` had
        // silently stored nothing at all.
        let stored = repositories
            .load_version(
                WorkspaceId::from_uuid(uuid::Uuid::from_u128(0xABCD)),
                jarvis_domain::model::policy::PolicyVersionRef {
                    policy_id: policy_id(1),
                    version: 1,
                },
            )
            .await
            .expect("the other workspace's policy is stored");
        assert_eq!(stored.name, "someone-elses");
        let _ = std::fs::remove_dir_all(temp_dir("policy-scope"));
    }
}
