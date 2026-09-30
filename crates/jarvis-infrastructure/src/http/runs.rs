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

use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jarvis_application::repository::run::RunRuntime;
use jarvis_application::request_context::RequestContext;
use jarvis_application::run_service::{CreatedRun, RunOptions, RunService, RunServiceError};
use jarvis_domain::ids::{
    ConversationId, CorrelationId, ModelDataPolicyId, PrincipalId, RequestId, RunId, WorkspaceId,
};
use jarvis_domain::model::policy::PolicyVersionRef;
use jarvis_domain::run::retry::{RetryError, RetryPolicy};
use jarvis_domain::run::state::RunState;
use jarvis_protocol::run::run_links;
use jarvis_protocol::{
    CancelRunRequest, CreateRunRequest, CreateRunResponse, MAX_CANCEL_REASON_BYTES,
    MAX_RUN_INPUT_BYTES, ModelPolicyRef, NATIVE_RUNTIME, RUN_CONTRACT_VERSION, RetryRequest,
    RunEventFrame, RunLimitsView, RunUsageView, RunView, SseEvent,
};

use crate::http::{ApiState, AuthenticatedClient, RequestIdOf, error_response_for};

/// The header carrying a client's idempotency key.
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";

/// The runtime version this build records on every run it creates.
///
/// The package version rather than the wire contract version, because the two answer different
/// questions: the contract version says which *protocol* an event frame speaks, while this says
/// which build executed the run — and a resume needs the second, since a runtime's behaviour can
/// change without the wire shape changing.
const BUILD_RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

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
///
/// The arms are spelled from `jarvis_protocol::run::run_state` rather than from inline
/// literals, so the vocabulary is defined once. That was not true before: the seven strings
/// existed only as literals in this `match` and as prose in the contract, and the test that
/// covered this function compared the *number* of distinct results — so a rename here kept
/// the count at seven, kept the three terminal names, and left the projection and the
/// contract disagreeing about what a client sees.
#[must_use]
pub const fn wire_state(state: RunState) -> &'static str {
    use jarvis_protocol::run::run_state as wire;
    match state {
        RunState::Received => wire::RECEIVED,
        RunState::ContextBuilding | RunState::Planning => wire::CONTEXT_BUILDING,
        RunState::AwaitingModel
        | RunState::AwaitingApproval
        | RunState::ExecutingTool
        | RunState::Observing
        | RunState::Waiting => wire::MODEL_RUNNING,
        RunState::Responding => wire::RESPONDING,
        RunState::Completed => wire::COMPLETED,
        RunState::Failed => wire::FAILED,
        RunState::Cancelled => wire::CANCELLED,
    }
}

/// Handles `POST /api/v1/runs`.
pub async fn create_run(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.runs.as_ref() else {
        return not_ready(request_id);
    };
    let Some(key) = idempotency_key(&headers) else {
        return invalid_request(request_id, "An idempotency key is required.");
    };
    let command: CreateRunRequest = match serde_json::from_slice(&body) {
        Ok(command) => command,
        // A malformed body is the caller's error. The rejected value is not echoed,
        // because it is caller-supplied text.
        Err(_) => {
            return invalid_request(
                request_id,
                "The request body is not valid for this endpoint.",
            );
        }
    };

    // Refused rather than defaulted: a client that asked for an external runtime must
    // not silently receive a native one.
    if command.runtime != NATIVE_RUNTIME {
        return error_response_for(
            request_id,
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.semantic_invalid",
            "The requested runtime is not supported by this build.",
            false,
        );
    }
    // The runtime the caller named is **validated**, and the recorded runtime comes from the build
    // rather than from the request: the check above refuses what this build cannot serve, so the
    // only runtime that can reach a row is this one. The version is this build's own, because the
    // version that matters for a resume is the one that actually executed the run — a client cannot
    // claim a runtime version it is not running.
    match RunRuntime::new(command.runtime.as_str(), BUILD_RUNTIME_VERSION) {
        Ok(_) => {}
        Err(_) => {
            return error_response_for(
                request_id,
                StatusCode::UNPROCESSABLE_ENTITY,
                "request.semantic_invalid",
                "The requested runtime identity is not usable.",
                false,
            );
        }
    }
    let input = command.input.text_value();
    if input.is_empty() || input.len() > MAX_RUN_INPUT_BYTES {
        return error_response_for(
            request_id,
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
                return error_response_for(
                    request_id,
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "request.semantic_invalid",
                    "The conversation identifier is not a valid identifier.",
                    false,
                );
            }
        },
        None => None,
    };

    // Both optional caller statements are validated **before** any run exists, and they are one
    // helper because they share a property that matters: each is a field the daemon must act on or
    // refuse. Before this the policy was accepted and never read — a failure indistinguishable, from
    // the client's side, from the policy being applied — and the retry bounds would have been a
    // value the domain constructor refused *after* the budget was built, or worse, one silently
    // clamped to something the caller did not ask for and cannot detect.
    let requested = match parse_run_options(&command) {
        Ok(options) => options,
        Err(message) => {
            return error_response_for(
                request_id,
                StatusCode::UNPROCESSABLE_ENTITY,
                "request.semantic_invalid",
                message,
                false,
            );
        }
    };

    let context = context_for(&client, request_id);
    match service
        .create(
            &context,
            conversation,
            input,
            &key,
            requested,
            state.spawner.as_ref(),
        )
        .await
    {
        Ok(created) => created_response(request_id, &created),
        Err(error) => service_error_response(request_id, &error),
    }
}

/// Parses the caller's optional statements about the run, or names what was wrong with them.
///
/// Returns one [`RunOptions`] rather than two `Result`s so the handler validates both in one place
/// and cannot handle one and forget the other — the same reason the fields are grouped on the
/// service side.
fn parse_run_options(command: &CreateRunRequest) -> Result<RunOptions, &'static str> {
    let policy = parse_policy_reference(command.model_policy.as_ref())?;
    let retry = parse_retry_policy(command.retry)?;
    Ok(RunOptions::new().with_policy(policy).with_retry(retry))
}

/// Parses the client's retry policy, when one was supplied.
///
/// `None` means the caller left the retry decision to JARVIS, which is **not** the same as asking
/// for no retries — that is `max_attempts: 1`, and the field's own doc draws the distinction. The
/// message names the bound that was violated rather than echoing the rejected number, because an
/// operator's next step differs by which bound moved.
fn parse_retry_policy(request: Option<RetryRequest>) -> Result<Option<RetryPolicy>, &'static str> {
    let Some(request) = request else {
        return Ok(None);
    };
    match RetryPolicy::new(
        request.max_attempts,
        request.base_backoff_ms,
        request.max_backoff_ms,
    ) {
        Ok(policy) => Ok(Some(policy)),
        Err(RetryError::AttemptsOutOfRange) => {
            Err("The retry attempt count must be between 1 and the daemon's maximum.")
        }
        Err(RetryError::BackoffOutOfRange) => {
            Err("The retry backoff must be ordered and within the daemon's maximum.")
        }
    }
}

/// Parses the client's model policy reference, when one was supplied.
///
/// `None` means the caller did not pin a version, so the workspace's active policy governs the
/// run — which is the only resolution a client could have named, since the workspace is resolved
/// server-side and the policy identifier is derived from it.
fn parse_policy_reference(
    reference: Option<&ModelPolicyRef>,
) -> Result<Option<PolicyVersionRef>, &'static str> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let Ok(policy_id) = ModelDataPolicyId::parse(&reference.policy_id) else {
        return Err("The model policy identifier is not a valid identifier.");
    };
    // Version 0 cannot denote a stored version — the contract numbers versions from 1 — so it is
    // refused rather than passed down to become a not-found, which would report "no such policy"
    // for a value that could never have named one.
    if reference.version == 0 {
        return Err("The model policy version must be at least 1.");
    }
    Ok(Some(PolicyVersionRef {
        policy_id,
        version: reference.version,
    }))
}

/// Handles `GET /api/v1/runs/{run_id}`.
pub async fn read_run(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(run_id): Path<String>,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.runs.as_ref() else {
        return not_ready(request_id);
    };
    // A value that cannot be an identifier cannot denote a resource, and reporting it as
    // `not_found` keeps a foreign run and a malformed one indistinguishable, which the
    // contract requires.
    let Ok(run) = RunId::parse(&run_id) else {
        return not_found(request_id);
    };
    let context = context_for(&client, request_id);
    // The usage is read **before** the run is answered so a failure to read it is not reported as a
    // successful read with a missing field. The two reads are separate ports, and answering as soon
    // as the run loaded would make a storage fault on the call table indistinguishable from a run
    // that consumed nothing — the exact conflation the `Option` exists to prevent.
    let usage = match service.usage(&context, run).await {
        Ok(usage) => usage,
        Err(error) => return service_error_response(request_id, &error),
    };
    match service.read(&context, run).await {
        Ok(stored) => json_response(
            request_id,
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
                // The limit the run is held to, so `run.deadline_exceeded` is explicable to the
                // client that receives it. The value is the run's stored column, which
                // `NewRun::with_budget` derived from the budget the controller reads — so the
                // instant published here is the one enforced, not a second computation of it.
                deadline_at: stored.deadline_at.map(|value| value.to_string()),
                // Summed across the run's attempts rather than taken from one call, so the figure a
                // client compares against the ceilings is the quantity the ceilings are documented
                // to bound. `None` when nothing was recorded — not a zeroed block, which would say
                // a run whose provider reported nothing consumed nothing.
                usage: usage.map(|usage| RunUsageView {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cached_input_tokens: usage.cached_input_tokens,
                    reasoning_tokens: usage.reasoning_tokens,
                    provider_reported: usage.provider_reported,
                    estimated_cost_microunits: usage.estimated_cost_microunits,
                    currency: usage.currency,
                }),
                // The ceilings `usage` is compared against, from the run's own stored budget — the
                // same value the controller's `exceeded_by` reads, so a client cannot be shown a
                // limit the daemon did not enforce. Omitted entirely when neither is set, which is
                // the fresh-install case before a policy supplies one.
                limits: stored
                    .budget
                    .has_consumption_ceiling()
                    .then_some(RunLimitsView {
                        max_output_tokens: stored.budget.max_output_tokens,
                        max_cost_microunits: stored.budget.max_cost_microunits,
                    }),
            },
        ),
        Err(error) => service_error_response(request_id, &error),
    }
}

/// Handles `POST /api/v1/runs/{run_id}/cancel`.
pub async fn cancel_run(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    body: Bytes,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.runs.as_ref() else {
        return not_ready(request_id);
    };
    let Some(key) = idempotency_key(&headers) else {
        return invalid_request(request_id, "An idempotency key is required.");
    };
    let Ok(run) = RunId::parse(&run_id) else {
        return not_found(request_id);
    };
    // An empty body is accepted because the contract's example supplies only a reason,
    // and requiring a body would refuse a legitimate minimal cancel.
    let reason = if body.is_empty() {
        "user_requested".to_owned()
    } else {
        match serde_json::from_slice::<CancelRunRequest>(&body) {
            Ok(command) => command.reason,
            Err(_) => {
                return invalid_request(
                    request_id,
                    "The request body is not valid for this endpoint.",
                );
            }
        }
    };
    // The contract bounds the reason, and `MAX_CANCEL_REASON_BYTES` was declared for exactly
    // this and **enforced nowhere** — while `MAX_RUN_INPUT_BYTES` is applied one route above it.
    // A declared bound that nothing checks is the shape where the constant reads as coverage, so
    // the check lives here rather than being assumed from the constant's existence.
    //
    // A NUL byte is refused for the same reason it is refused on the run input: the reason is
    // stored on a durable event, and a value that truncates at a NUL would be persisted
    // differently from how it was validated.
    if reason.is_empty() || reason.len() > MAX_CANCEL_REASON_BYTES || reason.contains('\0') {
        return invalid_request(
            request_id,
            "The cancellation reason is empty or over the bounded limit.",
        );
    }

    let context = context_for(&client, request_id);
    // The status is derived from the state the **service** returns, not from a separate
    // pre-read. That is the same read that decides whether to signal the run, so there is
    // no window between the two in which the run can finish.
    //
    // A pre-read was the first version and it was a real defect: the run could complete
    // between the pre-read and the service's own read, so a run the service found
    // *terminal* was reported as `202` with cleanup in flight. The contract returns `200`
    // with the unchanged terminal state for a run that has already finished, and two reads
    // of one fact is what let the answer disagree with the command that produced it. This
    // was caught by a real-daemon journey, not by a unit test.
    match service.cancel(&context, run, &reason, &key).await {
        Ok(current) => json_response(
            request_id,
            if current.is_terminal() {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            &serde_json::json!({ "state": wire_state(current) }),
        ),
        Err(error) => service_error_response(request_id, &error),
    }
}

/// Handles `GET /api/v1/runs/{run_id}/events`.
///
/// This delivers the retained public events as SSE frames and then **follows the run live**:
/// the contract requires that an "initial connection replays retained events from sequence 1,
/// then follows live events", and the response body is a stream that ends after the terminal
/// event rather than a body written once.
///
/// The live half is built from three parts, and the division is what makes it robust:
///
/// - **The durable store is the source of truth.** The stream's position is a *sequence number*,
///   and every read is `load_events(from_sequence)`. Nothing is delivered from a channel, so a
///   live stream and a replayed one cannot disagree about content or order.
/// - **The wake-up carries no payload.** `RunService::subscribe` yields a notification, not an
///   event: several deltas may publish between two reads (so one read delivers a batch) and a
///   notification may arrive with nothing new (so the read returns nothing). Both are handled by
///   the same idempotent "read from the position" step, which is why a slow or lagging follower
///   catches up rather than losing output — the contract's own requirement that a slow consumer
///   "can replay from its last delivered event".
/// - **The stream ends on the durable terminal state, not on the notification.** A run that
///   finishes between two reads is seen in the read, so a last notification that never arrives
///   cannot leave a client waiting after its run is over.
///
/// The request-level refusals are decided **before** the body streams: an `Accept` that excludes
/// the media type is `400`, and an unavailable resume position is `409`. That ordering is not
/// cosmetic — once the response has begun, a status code can no longer express a decision.
pub async fn run_events(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.runs.as_ref() else {
        return not_ready(request_id);
    };
    // This endpoint serves `text/event-stream` and nothing else, so a caller that states it will
    // accept something else is asking for a representation this route cannot produce. Refused
    // before the run is read, because the refusal is about the request rather than the resource:
    // answering `404` for a well-formed `Accept` mismatch would send a client looking for a run
    // that is right there.
    if !accepts_event_stream(&headers) {
        return error_response_for(
            request_id,
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "This endpoint serves `text/event-stream`.",
            false,
        );
    }
    let Ok(run) = RunId::parse(&run_id) else {
        return not_found(request_id);
    };
    let context = context_for(&client, request_id);

    // The resume position is resolved against the retained events rather than trusted as
    // a number, so a position for an event this daemon no longer has is a `409` instead
    // of a silent skip, which the contract forbids.
    let from_sequence = match headers
        .get(LAST_EVENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        None => 1,
        Some(value) => match resolve_resume(request_id, service, &context, run, value).await {
            Ok(sequence) => sequence,
            Err(response) => return *response,
        },
    };

    // The subscription is taken **before** the first read, so an event published between the read
    // and the subscribe cannot be missed. The next read is by *sequence*, not by arrival, so anything
    // published in the window is still read — and taking the subscription first means the notification
    // for it is not missed either. Taking it after the first read would leave a window in which a
    // wake-up is sent before there is a receiver for it.
    let subscription = match service.subscribe(&context, run).await {
        Ok(subscription) => subscription,
        Err(error) => return service_error_response(request_id, &error),
    };

    // Everything up to the first frame is decided *before* the body begins, so a refusal still has a
    // status code to travel in.
    //
    // **The follower's position is derived from the replayed page, not from `from_sequence`.** That
    // is a real defect fix, and the reason is worth recording because the bug was invisible to a unit
    // test: initialising the position to `from_sequence` meant the *next* read returned the same page
    // again, so every replayed event was delivered twice and the run's stream replayed from sequence
    // 1 in the middle of the body. Two terminal events, and a sequence list of
    // `[1..7, 1..7]` — which is exactly what the disconnect journey reported, because it counts
    // terminals and checks contiguity by sequence. A stream that only ever *replayed and closed* could
    // not exhibit this: it performed one read and never advanced past it.
    //
    // The position is therefore the page's own last sequence plus one, and for an empty page it falls
    // back to `from_sequence` — the run may have nothing after the resume point yet, and the next read
    // must start where the request asked rather than at 1.
    let (initial, position) = match service.events(&context, run, from_sequence).await {
        Ok(page) => {
            let next = page
                .events
                .last()
                .map_or(from_sequence, |event| event.sequence.saturating_add(1));
            match render_page(&page) {
                Ok(frames) => (frames, next),
                Err(()) => return internal_failure(request_id),
            }
        }
        Err(error) => return service_error_response(request_id, &error),
    };

    let follower = LiveFollow {
        service: Arc::clone(service),
        context,
        run,
        next_sequence: position,
        subscription,
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            // A stream that may be cached is a stream a client can be shown stale output from.
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(follow_stream(
            follower,
            initial,
            state.keepalive_interval,
            state.stream_overrun_timeout,
        )),
    )
        .into_response()
}

/// Renders one page of stored events into complete SSE frames.
///
/// # Errors
///
/// Returns `Err(())` when a frame cannot be serialized, which is a programming error rather than a
/// condition: the payloads are already-validated JSON and the identifiers are UUID text.
fn render_page(
    page: &jarvis_application::repository::run::RunEventPage,
) -> Result<Vec<String>, ()> {
    let mut frames = Vec::with_capacity(page.events.len());
    for event in &page.events {
        frames.push(render_event(event)?);
    }
    Ok(frames)
}

/// Renders one stored event as a complete SSE frame.
///
/// Uses [`SseEvent::render`], the same function the protocol crate's framing test asserts, so the
/// live path and the wire type cannot produce two spellings of one frame — which is why this handler
/// never builds a frame string itself.
///
/// # Errors
///
/// Returns `Err(())` when the frame cannot be serialized.
fn render_event(
    event: &jarvis_application::repository::run::StoredActivityEvent,
) -> Result<String, ()> {
    // A stored payload is already-redacted public detail, so it is passed through unchanged. A
    // missing or unparseable payload becomes `null` rather than an invented object, because a client
    // can detect `null` and cannot detect a plausible-looking substitute.
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
    let data = serde_json::to_string(&frame).map_err(|_| ())?;
    Ok(SseEvent {
        id: frame.event_id,
        event_type: event.event_type.clone(),
        data,
    }
    .render())
}

/// The state a live follow carries while it works.
///
/// The read position lives here rather than in the response body, because it is the *follower's*
/// state: it is a sequence number, and it is what makes a live stream and a replayed one agree about
/// where the client is.
struct LiveFollow {
    service: Arc<RunService>,
    context: RequestContext,
    run: RunId,
    /// The next sequence to read, which is where the last delivered event left off.
    next_sequence: u64,
    /// Wake-ups that a run's stream has grown.
    subscription: jarvis_application::live_events::LiveSubscription,
}

impl LiveFollow {
    /// Reads the next batch of frames after the current position.
    ///
    /// Returns `Ok(frames)` plus whether the run has reached a durable terminal state, or `Err(())`
    /// when a frame cannot be rendered. The position advances past every event considered —
    /// including one that failed to render — because re-reading it would repeat the defect rather
    /// than recover from it.
    async fn read(&mut self) -> Result<(Vec<String>, bool), ()> {
        let page = self
            .service
            .events(&self.context, self.run, self.next_sequence)
            .await
            .map_err(|_| ())?;
        let mut frames = Vec::with_capacity(page.events.len());
        for event in &page.events {
            self.next_sequence = event.sequence.saturating_add(1);
            frames.push(render_event(event)?);
        }
        // The terminal condition comes from the run's **durable state**, not from a string match on
        // the last frame: the state is the fact, and an event type is one projection of it.
        Ok((frames, page.terminal_state.is_some()))
    }
}

/// The depth of the channel between the follow task and the response body.
///
/// Bounded, like every other buffer in this workspace. A full channel applies backpressure to the
/// *follow task* rather than dropping frames, which is the correct direction: the run itself is
/// unaffected — its events are durable — so a slow client slows only its own delivery, and the
/// contract's "a slow consumer can replay from its last delivered event" holds because the store
/// still has everything the channel had not yet handed over.
///
/// **Backpressure alone was not the whole contract.** A full channel parks this task, and a parked
/// task is invisible to the client: the connection stays open, nothing is sent, and a follower that
/// has stopped reading cannot tell that from a run that has nothing to say. The contract says a slow
/// consumer "is disconnected", so [`send_bounded`] adds the missing half — a bound on how long one
/// hand-off may take — and this constant remains what makes that bound necessary rather than
/// theoretical.
///
/// `pub(crate)` rather than private because the test that proves the disconnect must flood with more
/// frames than this, and that precondition is asserted rather than assumed: a test whose trigger sits
/// *inside* the bound it is testing passes against the defect.
pub(crate) const FOLLOW_CHANNEL_DEPTH: usize = 32;

/// Hands one piece of a stream to a follower, or reports that the follower is not keeping up.
///
/// Returns `false` when the follower should be disconnected: either because the channel is closed
/// (the client went away), or because the hand-off took longer than the daemon will wait. The second
/// case is the one this function exists for, and its shape is deliberate — nothing is dropped and
/// nothing is skipped, because the signal tells the client to reconnect and a **reconnect re-reads
/// from a sequence position**. That is why delivery is bounded by *time* rather than by a position:
/// a dropped frame would be a permanent gap in the client's view, whereas a disconnected client
/// resumes and loses nothing.
///
/// The wait is per *piece*, not per batch, so the bound describes the thing that is actually slow —
/// how long one frame takes to reach one follower — rather than how long a page took to render. A
/// batch of frames from one read must each clear it in turn, which is the same statement made for
/// every frame rather than an average over the batch.
async fn send_bounded(
    sender: &tokio::sync::mpsc::Sender<Bytes>,
    piece: String,
    timeout: Duration,
) -> bool {
    matches!(
        tokio::time::timeout(timeout, sender.send(Bytes::from(piece))).await,
        Ok(Ok(())),
    )
}

/// Where a follow task leaves the reason it gave up, for the response body to deliver last.
///
/// **The signal cannot travel through the follow channel, and that is a property of the failure
/// rather than an inconvenience.** The one moment an overrun exists is the moment that channel is
/// full — a follower that were keeping up would never trigger the bound — so a signal sent through it
/// would be blocked behind the very congestion it is describing, and `try_send` would simply fail.
/// The signal therefore travels *beside* the channel and is delivered by the body once the channel
/// closes, which is the first moment the body can be sure the queue is drained and therefore the
/// first moment the client is genuinely going to read it.
///
/// A `watch` rather than a `Mutex<Option<String>>`: the body's `poll_next` must not block or need a
/// lock to decide, and `watch::Receiver::borrow_and_update` is a synchronous read of the latest
/// value. It is written once, by the only task that can decide an overrun happened.
#[derive(Clone)]
struct OverrunSignal {
    sender: tokio::sync::watch::Sender<Option<String>>,
}

impl OverrunSignal {
    /// Creates a signal that has nothing to report yet.
    fn new() -> (Self, tokio::sync::watch::Receiver<Option<String>>) {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        (Self { sender }, receiver)
    }

    /// Records that the follower fell behind, so the body delivers the signal before it ends.
    ///
    /// `send_replace` rather than `send`, because the value is read by the body and an unobserved send
    /// would be dropped: this is a piece of *state* ("this stream ended in an overrun"), not an event,
    /// and a receiver that reads it a moment later must still find it.
    fn record_overrun(&self) {
        self.sender
            .send_replace(Some(jarvis_protocol::run::stream_overrun_frame()));
    }
}

/// Builds the response body that follows a run to its terminal event.
///
/// A task feeding a bounded channel, with the body draining it — the same shape
/// [`AdapterStream`](jarvis_application::model::AdapterStream) uses for a provider's frames, and for
/// the same reason: the follow loop is ordinary `async` code, so it never needs a hand-written
/// `poll` that owns a future borrowing the state it must also reach. That self-reference is what
/// makes a manual `Stream` implementation hard to review, and it is avoided here rather than
/// solved.
///
/// **A keepalive comment is emitted while the loop waits**, which is the one thing in this loop that
/// happens on a timer. The contract requires it ("Keepalives are SSE comments and do not consume
/// sequence numbers") and it is load-bearing rather than decorative: a run that is *thinking* has no
/// events to send, and a connection that stays silent for long enough is dropped by whatever sits
/// between the daemon and the client — a proxy, or the operating system. The comment is a **comment**,
/// so it carries no `id:` and a client's parser skips it, which is exactly why it cannot be a
/// synthetic event: a client resuming from `Last-Event-ID` would be sent back to a position that
/// never existed.
///
/// The task **always** closes the channel, on every exit path, because the response body ends when
/// the channel closes: a task that left it open would hold a client's connection open forever.
fn follow_stream(
    follower: LiveFollow,
    initial: Vec<String>,
    keepalive: Duration,
    overrun: Duration,
) -> impl futures_core::Stream<Item = Result<Bytes, std::convert::Infallible>> {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(FOLLOW_CHANNEL_DEPTH);
    let (signal, signal_receiver) = OverrunSignal::new();

    tokio::spawn(async move {
        // The initial page is sent first, so a replayed run's frames are delivered before the task
        // waits on anything — and a run that is already terminal closes the channel on the first
        // iteration below without waiting for a notification that will never come.
        for frame in initial {
            if !send_bounded(&sender, frame, overrun).await {
                // The initial page is where a client that never intended to read is caught, and it is
                // the one place the signal cannot be distinguished from a vanished peer — so it is
                // recorded either way, and the body delivers it if there is a body left to deliver to.
                signal.record_overrun();
                return;
            }
        }

        let mut follower = follower;
        // Created once, before the loop, so the interval is measured between keepalives rather than
        // restarted by each read. A per-iteration timer would be reset by every wake-up, and a run
        // publishing steadily would then never emit one — which is harmless, but it would also mean
        // a run publishing *slowly* emitted keepalives at a rate the interval does not describe.
        let mut ticker = tokio::time::interval(keepalive);
        // A missed tick's worth of waiting must not be repaid as a burst, which is what the default
        // `Burst` behaviour does: a delayed loop would emit several comments at once, which is
        // traffic with no purpose. `Delay` reschedules from now, so the steady state is one comment
        // per interval however late the loop was.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick completes immediately, so it is consumed here: without this the stream would
        // emit a keepalive as its first output, before any event, which a client could observe and
        // which would be a comment that says nothing happened to a client that had just connected.
        ticker.tick().await;

        loop {
            match follower.read().await {
                Ok((frames, terminal)) => {
                    for frame in frames {
                        if !send_bounded(&sender, frame, overrun).await {
                            signal.record_overrun();
                            return;
                        }
                    }
                    if terminal {
                        // The contract: "the server closes after delivering the terminal event".
                        // **No overrun signal here**, and the distinction is the whole reason the
                        // signal is a separate event: this stream ended because the run ended, which
                        // the client already knows, whereas the stalled path ends because the
                        // *connection* failed while the run continues. Sending a signal beside a
                        // terminal would also tell a client to reconnect to a run that is over.
                        return;
                    }
                }
                Err(()) => {
                    // A read that fails after the response has begun cannot become a status code, so
                    // the stream ends. The client reconnects with `Last-Event-ID` and receives a
                    // `409` if the position is genuinely gone, which is the contract's own recovery
                    // path rather than a silent gap. No overrun signal: a failure to *read the store*
                    // is not a follower that fell behind, and labelling it as one would send a client
                    // to re-read a position that may well fail again for the same reason.
                    return;
                }
            }
            // The wait is a `select` rather than a plain `wake().await` because the keepalive must
            // fire **while** the follower is waiting: a notification cannot arrive during a run that
            // is thinking, so a loop that only awaited one would be silent for the whole gap it
            // exists to cover. `biased` puts the notification first, so an event that arrives at the
            // same moment as a tick is delivered rather than preceded by a comment.
            tokio::select! {
                biased;
                live = follower.subscription.wake() => {
                    // A wake-up carries no payload, so it is permission to read again rather than an
                    // event. A receiver whose publisher is gone reports `false`, which ends the
                    // connection: waiting forever for an update nothing can send is the one outcome
                    // to avoid.
                    if !live {
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if !send_bounded(
                        &sender,
                        jarvis_protocol::run::keepalive_frame(),
                        overrun,
                    )
                    .await
                    {
                        signal.record_overrun();
                        return;
                    }
                }
            }
        }
    });

    ReceiverStream {
        receiver,
        signal: signal_receiver,
        overrun_delivered: false,
    }
}

/// The response body: a stream over frames the follow task has rendered.
///
/// Implements `futures_core::Stream` by delegating to the channel's own `poll_recv`, so no future is
/// stored and no waker is at risk of being dropped. `Ready(None)` means the follow task closed the
/// channel, which is how the body learns the run is over.
///
/// **The overrun signal is delivered after the channel closes, not through it.** That ordering is
/// forced rather than chosen: the channel is full at exactly the moment an overrun exists — a
/// follower that were reading would never trigger the bound — so a signal queued ahead of the
/// undelivered frames could never be sent, and one queued behind them would be read only by the
/// follower that was already too far behind to reach it. Delivering it as the body's **last** item
/// means the client reads every frame the daemon did manage to hand over and *then* learns why the
/// stream stopped, which is what makes the reconnect instruction actionable: it says where to resume
/// from, and the position it names is the last frame the client actually received.
struct ReceiverStream {
    receiver: tokio::sync::mpsc::Receiver<Bytes>,
    signal: tokio::sync::watch::Receiver<Option<String>>,
    /// Whether the overrun frame has been handed over.
    ///
    /// **A flag rather than relying on the channel, and this is a defect I introduced and the
    /// compiler could not see.** `watch::Receiver::borrow_and_update` marks the value *seen*; it does
    /// **not** consume it, so a second poll returns the same `Some(frame)` — and since a `Stream` is
    /// polled again after every `Ready`, the body would have emitted the overrun frame for ever
    /// instead of ending. The frame is one-shot by contract, so the one-shot-ness is tracked here.
    overrun_delivered: bool,
}

impl futures_core::Stream for ReceiverStream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // `Infallible` as the error type: a frame is already-rendered text, so there is no per-item
        // failure to report — the only way this ends is the channel closing.
        match self.receiver.poll_recv(context) {
            Poll::Ready(Some(item)) => Poll::Ready(Some(Ok(item))),
            Poll::Ready(None) => {
                // The task has finished. If it recorded an overrun, this is the stream's last item.
                if self.overrun_delivered {
                    return Poll::Ready(None);
                }
                // Cloned before the flag is set, so the `Ref` guard does not overlap the mutation.
                let frame = self.signal.borrow_and_update().clone();
                match frame {
                    Some(frame) => {
                        self.overrun_delivered = true;
                        Poll::Ready(Some(Ok(Bytes::from(frame))))
                    }
                    None => Poll::Ready(None),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Returns whether a request's `Accept` permits an event stream.
///
/// The contract states that `GET /api/v1/runs/{run_id}/events` "requires
/// `Accept: text/event-stream`", and until this nothing checked it — the handler took the header
/// map and read only `Last-Event-ID`. The requirement was decoration, and it is worth saying why
/// that mattered: the CLI, which is the reference client for this surface, did **not send the
/// header either**, so the two defects hid each other. A conforming client and a permissive server
/// look identical from every test that drives one against the other, which is why the client half
/// is fixed in the same change.
///
/// **An absent `Accept` is inside the rule**, matching how this surface treats an absent
/// `Content-Type`: a request that states no preference can be served the only representation there
/// is. A **present** header that excludes `text/event-stream` is refused, which is the case worth
/// catching — a client that asked for JSON gets an error it can read rather than an event stream it
/// cannot parse.
///
/// The refusal is `400 request.invalid` rather than a `406`, because the contract's minimum-code
/// table has no `406` and inventing a status/code pair is a protocol change. `request.invalid` is
/// what this surface already uses for a malformed request header (a missing idempotency key, an
/// over-long cancel reason), so this stays inside the published envelope.
fn accepts_event_stream(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::ACCEPT) else {
        return true;
    };
    value.to_str().is_ok_and(accepts_event_stream_value)
}

/// Reports whether one `Accept` header value admits `text/event-stream`.
///
/// Split out so the rule is a pure function a test can drive with any spelling. The parsing is
/// deliberately the small honest subset of RFC 9110 this needs: a comma-separated list of media
/// ranges, each an optional type and subtype that may be `*`. A range's parameters are ignored,
/// because `text/event-stream; charset=utf-8` names the same type, and treating a parameter as a
/// different one would refuse the most ordinary client.
///
/// `*/*` and `text/*` both permit it. So does a bare `*`, which is not valid but is unambiguous —
/// and refusing a caller that said "anything" would be the wrong direction, since the refusal exists
/// to catch a caller that said "JSON".
#[must_use]
fn accepts_event_stream_value(value: &str) -> bool {
    value.split(',').any(|range| {
        let essence = range
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match essence.split_once('/') {
            Some((kind, subtype)) => {
                (kind == "text" || kind == "*") && (subtype == "event-stream" || subtype == "*")
            }
            // A bare `*` is a whole-range wildcard.
            None => essence == "*",
        }
    })
}

/// Resolves a `Last-Event-ID` value to the sequence after it.
///
/// Returns the refusal as a `Response` when the identifier is unknown, because "resume
/// from an event I do not have" must not silently become "start from the beginning" —
/// that would deliver a gap as if it were complete.
///
/// **Resolved by lookup, not by scanning a page, and that distinction was a defect.** This used to
/// read one page of retained events (`service.events(context, run, 1)`) and search it, while a page
/// is bounded to `MAX_EVENT_PAGE` (**500**) and a run's stream is not — the controller publishes one
/// durable event per streamed output chunk, so a long answer passes 500 events as a matter of
/// course. A client whose last-seen event was sequence 501 was therefore refused with
/// `stream.replay_unavailable`, a code the contract reserves for a position that is **no longer
/// retained**, while the event sat in the store. The page bound leaked out of the transport and
/// became a claim about retention.
///
/// The lookup also removes a subtler cost: the scan re-read one page to discover nothing, on a
/// request a client makes on every reconnect. The comment that stood here claimed scanning was
/// "the honest way to find it" because "a second lookup path could disagree with the stream about
/// what is retained" — which is true of *retention* and not of *identity*, and the two questions
/// are now answered by different things: retention by this row existing, framing by `load_events`.
async fn resolve_resume(
    request_id: Option<&str>,
    service: &RunService,
    context: &RequestContext,
    run: RunId,
    last_event_id: &str,
) -> Result<u64, Box<Response>> {
    if last_event_id.is_empty() || last_event_id.len() > MAX_EVENT_ID_BYTES {
        return Err(Box::new(replay_unavailable(request_id)));
    }
    match service.event_sequence(context, run, last_event_id).await {
        Ok(sequence) => Ok(sequence.saturating_add(1)),
        // A run the caller cannot see and an event the daemon no longer retains are the same
        // refusal to a client — `409`, never a silent restart — which is why both arms map here.
        // The service reports `NotFound` for either, so nothing distinguishes them on the wire.
        Err(_) => Err(Box::new(replay_unavailable(request_id))),
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
///
/// The request identifier, though, is the one the middleware already derived — passed in rather
/// than minted again, because a refusal's envelope names a `request_id` and the daemon's own
/// diagnostics record a `request_id`, and two ids for one request would make the correlation the
/// contract asks for impossible. The correlation id stays independently minted: it groups a flow
/// across requests, while this identifies one.
pub(crate) fn context_for(
    client: &AuthenticatedClient,
    request_id: Option<&str>,
) -> RequestContext {
    let scope = resolve_scope(client);
    let request_id = request_id
        .and_then(|value| RequestId::parse(value).ok())
        .unwrap_or_else(|| RequestId::from_uuid(uuid::Uuid::now_v7()));
    RequestContext::new(
        request_id,
        CorrelationId::from_uuid(uuid::Uuid::now_v7()),
        scope.principal_id,
        client.assurance,
        scope.workspace_id,
        client.channel,
    )
    // The credential's digest, which the idempotency scope records. Absent rather than empty when the
    // extractor could not find one, because "no credential" and "a credential whose digest is empty"
    // are different facts and the run service distinguishes them.
    .with_client_credential(
        (!client.credential_digest.is_empty()).then(|| client.credential_digest.clone()),
    )
}

/// Renders a created run as the contract's `202` response.
///
/// Carries a `Location` header naming the created resource, as the contract's create step
/// requires. It is a real defect to omit it rather than a cosmetic one: a `201`/`202` without
/// `Location` gives a client a status it can see and no way to address what it just created,
/// so every client would have to build the path from the id itself — which is exactly how two
/// clients come to disagree about a URL the daemon owns. The value is the same relative path
/// the response body already exposes as `links.self`, taken from one builder so the header and
/// the body cannot name different resources.
fn created_response(request_id: Option<&str>, created: &CreatedRun) -> Response {
    let run_id = created.run_id.to_string();
    let links = run_links(&run_id);
    let body = CreateRunResponse {
        run_id: created.run_id.to_string(),
        conversation_id: created.conversation_id.to_string(),
        state: wire_state(created.state).to_owned(),
        // The persisted instant, not one invented here, so the response and the stored
        // run cannot report different creation times.
        created_at: created.created_at.to_string(),
        links,
    };
    let mut response = json_response(request_id, StatusCode::ACCEPTED, &body);
    if let Ok(value) = header::HeaderValue::from_str(&body.links.self_path) {
        // A `HeaderValue` rejects control characters, and this value is a fixed path built from
        // a validated UUID, so a failure here would mean the path builder changed. Dropping the
        // header rather than panicking keeps a malformed path from turning into a 500 after the
        // run was already created — the run exists and the body still addresses it.
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// Maps a service error to its contract status and envelope.
///
/// Exposed to the crate as `service_error_response_for_test` so a test can drive the mapping
/// directly. A route-level test would have to win a race against the controller to produce a
/// version conflict at all, and a test that cannot reliably reach its subject proves nothing about
/// it — this is the function that decides the status, so it is the function to assert.
#[cfg(test)]
pub(crate) fn service_error_response_for_test(error: &RunServiceError) -> Response {
    service_error_response(None, error)
}

/// Maps a service error to its contract status and envelope.
fn service_error_response(request_id: Option<&str>, error: &RunServiceError) -> Response {
    let status = match error {
        RunServiceError::Invalid { .. } => StatusCode::BAD_REQUEST,
        // A named policy that does not exist is a 404 like any other absent resource, and it is
        // the contract's own `model.policy_not_found` rather than a generic not-found: the
        // caller's remedy is to read the policy list, which a generic "no such resource" would
        // not suggest.
        RunServiceError::NotFound | RunServiceError::PolicyNotFound => StatusCode::NOT_FOUND,
        // The policy is in force and admits no compliant model, so the request cannot be carried
        // out as sent. **403** rather than 400: the body is well-formed and the caller is
        // authorized — its workspace's own policy refuses the call — and a 400 would tell a client
        // its request was malformed, sending it to inspect the payload rather than the rules. It is
        // the same reasoning the effective-route probe follows in reverse: there a refusal is a
        // `200` because the probe *asked* what would comply, while here the caller asked for work
        // to be done and the answer is that this policy forbids it.
        RunServiceError::PolicyUnsatisfied { .. } => StatusCode::FORBIDDEN,
        RunServiceError::IdempotencyConflict | RunServiceError::Conflict => StatusCode::CONFLICT,
        // A durable precondition that no longer holds is a conflict, and it is the **same** conflict
        // the policy route answers with `409 resource.version_conflict` — one concept, one status.
        // It used to arrive through `Storage(_)` and be reported as a `500` carrying
        // `storage.version_conflict`, a code this surface does not document: the contract lists
        // `resource.version_conflict` for a precondition and `internal.failure` for a `500`. So a
        // client was told the server had faulted when its own view was merely stale, which is the one
        // case where retrying after a re-read succeeds.
        RunServiceError::Storage(_) | RunServiceError::Controller(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    error_response_for(
        request_id,
        status,
        error.code(),
        error.message(),
        error.retryable(),
    )
}

/// The refusal for a caller's invalid request.
fn invalid_request(request_id: Option<&str>, message: &str) -> Response {
    error_response_for(
        request_id,
        StatusCode::BAD_REQUEST,
        "request.invalid",
        message,
        false,
    )
}

/// The refusal for a stream whose resume position is unavailable.
fn replay_unavailable(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::CONFLICT,
        "stream.replay_unavailable",
        "The requested stream position is not available.",
        false,
    )
}

/// The refusal for a missing resource, which a malformed identifier also produces.
fn not_found(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::NOT_FOUND,
        "resource.not_found",
        "No such resource.",
        false,
    )
}

/// The refusal for a run surface that is not configured.
pub(crate) fn not_ready(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::SERVICE_UNAVAILABLE,
        "service.not_ready",
        // A message that names the *surface*, because one function cannot describe two: the run and
        // approval surfaces each report their own, so a client is told which subsystem is missing
        // rather than a generic sentence that would send it looking at the wrong one.
        "This surface is not available yet.",
        true,
    )
}

/// The refusal for a response this daemon could not render.
///
/// Carries the request identifier, because this is the refusal the contract names specifically:
/// "internal failures return a request ID and generic message while preserving structured
/// diagnostics in redacted local logs". A client reporting this has no other handle to quote, and
/// the identifier is also returned as a response header so it can be read without parsing the body.
fn internal_failure(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal.failure",
        "The response could not be rendered.",
        true,
    )
}

/// Serializes a value into an `application/json` response.
pub(crate) fn json_response(
    request_id: Option<&str>,
    status: StatusCode,
    value: &impl serde::Serialize,
) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(_) => internal_failure(request_id),
    }
}
