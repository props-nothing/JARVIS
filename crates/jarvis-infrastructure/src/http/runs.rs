//! The run resource surface of the local control API.
//!
//! Four endpoints from `docs/contracts/local-control-api.md` live here: create, read,
//! cancel, and the event stream. Each handler does three things and nothing more —
//! resolve the authenticated scope, parse the command, call the service — because the
//! orchestration belongs in `jarvis_application::run_service` and a handler that
//! reimplemented it would be untestable without an HTTP client.
//!
//! Four contract rules are enforced here rather than assumed:
//!
//! - **Every refusal uses the common error envelope.** `axum`'s `Json` extractor
//!   rejects a bad body with a plain-text status, which would give a client a status it
//!   can see and nothing it can parse, so the body is read and parsed by hand and every
//!   failure maps to a code.
//! - **The scope is resolved server-side.** The workspace and principal come from the
//!   authenticated client, never from the request body, so a caller cannot address
//!   another workspace's run by naming it.
//! - **`Idempotency-Key` is required**, and its absence is `request.invalid` rather
//!   than a silently non-idempotent create.
//! - **A run's state is projected once.** The wire states are a *coarser* set than the
//!   controller's twelve, and the projection is total over the non-terminal states,
//!   which is what the domain deliberately deferred to this boundary.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jarvis_application::request_context::RequestContext;
use jarvis_application::run_service::{CreatedRun, RunService, RunServiceError};
use jarvis_domain::ids::{
    ConversationId, CorrelationId, PrincipalId, RequestId, RunId, WorkspaceId,
};
use jarvis_domain::run::state::RunState;
use jarvis_protocol::run::run_links;
use jarvis_protocol::{
    CancelRunRequest, CreateRunRequest, CreateRunResponse, MAX_RUN_INPUT_BYTES, NATIVE_RUNTIME,
    RUN_CONTRACT_VERSION, RunEventFrame, RunView, SseEvent,
};

use crate::http::{ApiState, AuthenticatedClient, error_response};

/// The header carrying a client's idempotency key.
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";

/// The header a client uses to resume a stream.
pub const LAST_EVENT_ID_HEADER: &str = "last-event-id";

/// The longest accepted event identifier.
const MAX_EVENT_ID_BYTES: usize = 128;

/// The longest accepted idempotency key.
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;

/// The default workspace of a local profile.
///
/// A fixed identifier rather than a row that must exist first: a local profile has
/// exactly one workspace until multi-workspace administration exists, and deriving it
/// from a table would make every request depend on a lookup that cannot yet fail
/// meaningfully. It is still server-side, so a caller cannot choose its own scope.
pub const DEFAULT_WORKSPACE_UUID: u128 = 0x018f_2b3c_4d5e_7a6b_8c9d_0e1f_2a3b_4c5d;

/// The request-scoped values a handler needs, resolved from the authenticated client.
#[derive(Debug, Clone, Copy)]
pub struct RequestScope {
    /// The resolved workspace.
    pub workspace_id: WorkspaceId,
    /// The resolved principal.
    pub principal_id: PrincipalId,
}

/// Resolves the workspace and principal for an authenticated client.
///
/// Both are derived from the *authenticated* identity, never from a header or a body
/// field. The principal is a deterministic digest of the client id, so one client
/// always resolves to one principal without the daemon storing a mapping it could
/// lose.
#[must_use]
pub fn resolve_scope(client: &AuthenticatedClient) -> RequestScope {
    RequestScope {
        workspace_id: WorkspaceId::from_uuid(uuid::Uuid::from_u128(DEFAULT_WORKSPACE_UUID)),
        principal_id: PrincipalId::from_uuid(principal_for(&client.client_id)),
    }
}

/// Derives a stable principal identifier from a client id.
fn principal_for(client_id: &str) -> uuid::Uuid {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(client_id.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // The all-zero and all-one identifiers are refused by the domain's parse, and a nil
    // principal would be indistinguishable from "unset".
    let candidate = uuid::Uuid::from_bytes(bytes);
    if candidate.is_nil() || candidate.is_max() {
        uuid::Uuid::from_u128(1)
    } else {
        candidate
    }
}

/// Projects a domain state onto the client-visible wire state.
///
/// The contract exposes a **coarser** set than the controller tracks, so several
/// domain states collapse: `Planning` reads as `context_building` (a decision is being
/// taken about what to do next, which a client sees as part of building the request),
/// and `AwaitingModel`, `AwaitingApproval`, `ExecutingTool`, `Observing`, and `Waiting`
/// all read as `model_running` (work is outstanding). The mapping is total over the
/// non-terminal states, so a client never sees a state it cannot act on, and the run's
/// activity events carry the finer detail.
///
/// It lives here rather than in the domain because
/// `docs/architecture/agent-runtime.md` states that "state names are domain concepts,
/// not UI strings".
#[must_use]
pub const fn wire_state(state: RunState) -> &'static str {
    match state {
        RunState::Received => "received",
        RunState::ContextBuilding | RunState::Planning => "context_building",
        RunState::AwaitingModel
        | RunState::AwaitingApproval
        | RunState::ExecutingTool
        | RunState::Observing
        | RunState::Waiting => "model_running",
        RunState::Responding => "responding",
        RunState::Completed => "completed",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
    }
}

/// Handles `POST /api/v1/runs`.
pub async fn create_run(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(service) = state.runs.as_ref() else {
        return not_ready();
    };
    let Some(key) = idempotency_key(&headers) else {
        return invalid_request("An idempotency key is required.");
    };
    let command: CreateRunRequest = match serde_json::from_slice(&body) {
        Ok(command) => command,
        // A malformed body is the caller's error. The rejected value is not echoed,
        // because it is caller-supplied text.
        Err(_) => return invalid_request("The request body is not valid for this endpoint."),
    };

    // Refused rather than defaulted: a client that asked for an external runtime must
    // not silently receive a native one.
    if command.runtime != NATIVE_RUNTIME {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.semantic_invalid",
            "The requested runtime is not supported by this build.",
            false,
        );
    }
    let input = command.input.text_value();
    if input.is_empty() || input.len() > MAX_RUN_INPUT_BYTES {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.semantic_invalid",
            "The run input is empty or over the bounded limit.",
            false,
        );
    }

    let conversation = match command.conversation_id.as_deref() {
        Some(value) => match ConversationId::parse(value) {
            Ok(parsed) => Some(parsed),
            Err(_) => {
                return error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "request.semantic_invalid",
                    "The conversation identifier is not a valid identifier.",
                    false,
                );
            }
        },
        None => None,
    };

    let context = context_for(&client);
    match service
        .create(&context, conversation, input, &key, state.spawner.as_ref())
        .await
    {
        Ok(created) => created_response(&created),
        Err(error) => service_error_response(&error),
    }
}

/// Handles `GET /api/v1/runs/{run_id}`.
pub async fn read_run(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    Path(run_id): Path<String>,
) -> Response {
    let Some(service) = state.runs.as_ref() else {
        return not_ready();
    };
    // A value that cannot be an identifier cannot denote a resource, and reporting it as
    // `not_found` keeps a foreign run and a malformed one indistinguishable, which the
    // contract requires.
    let Ok(run) = RunId::parse(&run_id) else {
        return not_found();
    };
    let context = context_for(&client);
    match service.read(&context, run).await {
        Ok(stored) => json_response(
            StatusCode::OK,
            &RunView {
                run_id: stored.id.to_string(),
                conversation_id: stored.conversation_id.to_string(),
                state: wire_state(stored.state).to_owned(),
                version: stored.version.get(),
                created_at: stored.created_at.to_string(),
                started_at: stored.started_at.map(|value| value.to_string()),
                updated_at: stored.updated_at.to_string(),
                completed_at: stored.completed_at.map(|value| value.to_string()),
                error_code: stored.error_code,
            },
        ),
        Err(error) => service_error_response(&error),
    }
}

/// Handles `POST /api/v1/runs/{run_id}/cancel`.
pub async fn cancel_run(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    body: Bytes,
) -> Response {
    let Some(service) = state.runs.as_ref() else {
        return not_ready();
    };
    let Some(key) = idempotency_key(&headers) else {
        return invalid_request("An idempotency key is required.");
    };
    let Ok(run) = RunId::parse(&run_id) else {
        return not_found();
    };
    // An empty body is accepted because the contract's example supplies only a reason,
    // and requiring a body would refuse a legitimate minimal cancel.
    let reason = if body.is_empty() {
        "user_requested".to_owned()
    } else {
        match serde_json::from_slice::<CancelRunRequest>(&body) {
            Ok(command) => command.reason,
            Err(_) => return invalid_request("The request body is not valid for this endpoint."),
        }
    };

    let context = context_for(&client);
    // The status depends on whether the run had already finished *before* this command,
    // so it is read first: the contract returns `200` with the unchanged terminal state
    // for a finished run and `202` for one whose cleanup is in flight.
    let was_terminal = service
        .read(&context, run)
        .await
        .is_ok_and(|stored| stored.state.is_terminal());
    match service.cancel(&context, run, &reason, &key).await {
        Ok(current) => json_response(
            if was_terminal {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            &serde_json::json!({ "state": wire_state(current) }),
        ),
        Err(error) => service_error_response(&error),
    }
}

/// Handles `GET /api/v1/runs/{run_id}/events`.
///
/// **Partial, and labelled as such.** This delivers the retained public events as SSE
/// frames and then closes; a client follows a run by reconnecting with `Last-Event-ID`
/// until it receives a terminal event. A run's *live* follow — holding the connection
/// open and pushing each new event as it is published — is **not implemented**: a
/// streaming response body needs a `Stream` implementation, and this crate has neither
/// a stream crate nor `axum`'s `sse` feature in its reviewed dependency set. Adding
/// either is a dependency change the integration research gate requires evidence for,
/// and hand-writing a `Stream` would be an unreviewed async state machine on the
/// security-relevant path. The gap is recorded in `BRN-007`.
///
/// Everything else the contract requires of the stream is honoured: ordered frames,
/// exactly one terminal event, a `409` rather than a silent gap for an unavailable
/// resume position, and nothing but a real event consuming a `sequence`.
pub async fn run_events(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Response {
    let Some(service) = state.runs.as_ref() else {
        return not_ready();
    };
    let Ok(run) = RunId::parse(&run_id) else {
        return not_found();
    };
    let context = context_for(&client);

    // The resume position is resolved against the retained events rather than trusted as
    // a number, so a position for an event this daemon no longer has is a `409` instead
    // of a silent skip, which the contract forbids.
    let from_sequence = match headers
        .get(LAST_EVENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        None => 1,
        Some(value) => match resolve_resume(service, &context, run, value).await {
            Ok(sequence) => sequence,
            Err(response) => return *response,
        },
    };

    match service.events(&context, run, from_sequence).await {
        Ok(page) => {
            let mut body = String::new();
            for event in &page.events {
                // A stored payload is already-redacted public detail, so it is passed
                // through unchanged. A missing or unparseable payload becomes `null`
                // rather than an invented object, because a client can detect `null` and
                // cannot detect a plausible-looking substitute.
                let payload = event
                    .payload_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok())
                    .unwrap_or(serde_json::Value::Null);
                let frame = RunEventFrame {
                    contract_version: RUN_CONTRACT_VERSION.to_owned(),
                    event_id: event.id.to_string(),
                    run_id: event.run_id.to_string(),
                    sequence: event.sequence,
                    occurred_at: event.occurred_at.to_string(),
                    payload,
                };
                let Ok(data) = serde_json::to_string(&frame) else {
                    return internal_failure();
                };
                body.push_str(
                    &SseEvent {
                        id: frame.event_id.clone(),
                        event_type: event.event_type.clone(),
                        data,
                    }
                    .render(),
                );
            }
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/event-stream"),
                    // A stream that may be cached is a stream a client can be shown
                    // stale output from.
                    (header::CACHE_CONTROL, "no-store"),
                ],
                body,
            )
                .into_response()
        }
        Err(error) => service_error_response(&error),
    }
}

/// Resolves a `Last-Event-ID` value to the sequence after it.
///
/// Returns the refusal as a `Response` when the identifier is unknown, because "resume
/// from an event I do not have" must not silently become "start from the beginning" —
/// that would deliver a gap as if it were complete.
async fn resolve_resume(
    service: &RunService,
    context: &RequestContext,
    run: RunId,
    last_event_id: &str,
) -> Result<u64, Box<Response>> {
    if last_event_id.is_empty() || last_event_id.len() > MAX_EVENT_ID_BYTES {
        return Err(Box::new(replay_unavailable()));
    }
    // The identifier only has to match one retained event, and one page is bounded, so
    // scanning the retained events from the start is the honest way to find it: a
    // second lookup path could disagree with the stream about what is retained.
    let page = match service.events(context, run, 1).await {
        Ok(page) => page,
        Err(error) => return Err(Box::new(service_error_response(&error))),
    };
    match page
        .events
        .iter()
        .find(|event| event.id.to_string() == last_event_id)
    {
        Some(event) => Ok(event.sequence.saturating_add(1)),
        None => Err(Box::new(replay_unavailable())),
    }
}

/// Returns the `Idempotency-Key` value, when present and usable.
///
/// A key that is empty or over its bound is treated as absent, so a caller learns that
/// a key is required rather than having an unusable one accepted.
#[must_use]
pub fn idempotency_key(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(IDEMPOTENCY_HEADER)?.to_str().ok()?.trim();
    if value.is_empty() || value.len() > MAX_IDEMPOTENCY_KEY_BYTES || value.contains('\0') {
        return None;
    }
    Some(value.to_owned())
}

/// Builds a request context for an authenticated command.
///
/// The identifiers are generated here rather than accepted from the caller: a
/// caller-supplied correlation id would let one client forge another's trace.
fn context_for(client: &AuthenticatedClient) -> RequestContext {
    let scope = resolve_scope(client);
    RequestContext::new(
        RequestId::from_uuid(uuid::Uuid::now_v7()),
        CorrelationId::from_uuid(uuid::Uuid::now_v7()),
        scope.principal_id,
        client.assurance,
        scope.workspace_id,
        client.channel,
    )
}

/// Renders a created run as the contract's `202` response.
fn created_response(created: &CreatedRun) -> Response {
    let body = CreateRunResponse {
        run_id: created.run_id.to_string(),
        conversation_id: created.conversation_id.to_string(),
        state: wire_state(created.state).to_owned(),
        // The persisted instant, not one invented here, so the response and the stored
        // run cannot report different creation times.
        created_at: created.created_at.to_string(),
        links: run_links(&created.run_id.to_string()),
    };
    json_response(StatusCode::ACCEPTED, &body)
}

/// Maps a service error to its contract status and envelope.
fn service_error_response(error: &RunServiceError) -> Response {
    let status = match error {
        RunServiceError::Invalid { .. } => StatusCode::BAD_REQUEST,
        RunServiceError::NotFound => StatusCode::NOT_FOUND,
        RunServiceError::IdempotencyConflict => StatusCode::CONFLICT,
        RunServiceError::Storage(_) | RunServiceError::Controller(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    error_response(status, error.code(), error.message(), error.retryable())
}

/// The refusal for a caller's invalid request.
fn invalid_request(message: &str) -> Response {
    error_response(StatusCode::BAD_REQUEST, "request.invalid", message, false)
}

/// The refusal for a stream whose resume position is unavailable.
fn replay_unavailable() -> Response {
    error_response(
        StatusCode::CONFLICT,
        "stream.replay_unavailable",
        "The requested stream position is not available.",
        false,
    )
}

/// The refusal for a missing resource, which a malformed identifier also produces.
fn not_found() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "resource.not_found",
        "No such resource.",
        false,
    )
}

/// The refusal for a run surface that is not configured.
fn not_ready() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "service.not_ready",
        "The run surface is not available yet.",
        true,
    )
}

/// The refusal for a response this daemon could not render.
fn internal_failure() -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal.failure",
        "The response could not be rendered.",
        true,
    )
}

/// Serializes a value into an `application/json` response.
fn json_response(status: StatusCode, value: &impl serde::Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(_) => internal_failure(),
    }
}
