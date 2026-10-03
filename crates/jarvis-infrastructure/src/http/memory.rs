//! The memory surface: remember, list, search, read, and forget.
//!
//! Every handler does what the tool-grant surface's do: resolve the authenticated scope into a trusted
//! `RequestContext`, call [`jarvis_application::memory_service`], and render the result. The workspace and the
//! principal come from the context, so the body cannot name either. A memory in another workspace reads as
//! missing, never as forbidden.
//!
//! No new error codes: a refusal is `request.invalid`, a missing memory is `resource.not_found`, and a store
//! failure carries its own `storage.*` code, so the contract's table needs no new rows.
//!
//! Bodies are parsed by hand rather than with `axum::Json`, for the reason the other surfaces record: the
//! framework's rejection is its own response rather than the shared envelope.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use jarvis_application::memory_service::MemoryServiceError;
use jarvis_domain::ids::MemoryId;
use jarvis_domain::memory::{MAX_MEMORY_PAGE, Memory, MemoryClass};
use jarvis_protocol::{
    MemoryHitView, MemoryListView, MemorySearchView, MemoryView, RememberRequest, RememberedView,
};

use crate::http::{ApiState, AuthenticatedClient, RequestIdOf, error_response_for, runs};
use crate::time::SystemClock;

/// The longest accepted search query, in bytes.
const MAX_QUERY_BYTES: usize = 512;

/// `POST /api/v1/memories`
pub async fn remember(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    body: Bytes,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.memories.as_ref() else {
        return runs::not_ready(request_id);
    };
    let Ok(parsed) = serde_json::from_slice::<RememberRequest>(&body) else {
        return invalid(request_id, "the body is not a well-formed remember request");
    };
    let Ok(class) = MemoryClass::parse(&parsed.class) else {
        return invalid(request_id, "class is not a supported memory class");
    };
    let Some(sensitivity) =
        jarvis_application::context_assembly::parse_sensitivity(&parsed.sensitivity)
    else {
        return invalid(request_id, "sensitivity is not a known sensitivity");
    };
    let Ok(now) = jarvis_domain::clock::Clock::now(&SystemClock::new()) else {
        return clock_unavailable(request_id);
    };
    let context = runs::context_for(&client, request_id);
    match service
        .remember(&context, class, &parsed.text, sensitivity, now)
        .await
    {
        Ok(remembered) => runs::json_response(
            request_id,
            if remembered.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            &RememberedView {
                memory: memory_view(&remembered.memory),
                created: remembered.created,
            },
        ),
        Err(error) => service_error(request_id, &error),
    }
}

/// Parsed query parameters; an unknown key is refused by name rather than ignored.
struct Query {
    limit: u32,
    q: Option<String>,
}

impl Query {
    fn parse(uri: &Uri, allow_query: bool) -> Result<Self, String> {
        let mut query = Self {
            limit: MAX_MEMORY_PAGE,
            q: None,
        };
        let Some(raw) = uri.query() else {
            return Ok(query);
        };
        for pair in raw.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "limit" => {
                    let parsed = value.parse::<u32>().map_err(|_| "limit".to_owned())?;
                    query.limit = parsed.clamp(1, MAX_MEMORY_PAGE);
                }
                "q" if allow_query => {
                    let decoded = percent_decode(value).ok_or_else(|| "q".to_owned())?;
                    if decoded.len() > MAX_QUERY_BYTES {
                        return Err("q".to_owned());
                    }
                    query.q = Some(decoded);
                }
                other => return Err(other.to_owned()),
            }
        }
        Ok(query)
    }
}

/// Decodes `%XX` escapes and `+` as a space, refusing malformed input or non-UTF-8 output.
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = value.get(index + 1..index + 3)?;
                output.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).ok()
}

/// `GET /api/v1/memories`
pub async fn list_memories(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    uri: Uri,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.memories.as_ref() else {
        return runs::not_ready(request_id);
    };
    let query = match Query::parse(&uri, false) {
        Ok(query) => query,
        Err(field) => return invalid_query(request_id, &field),
    };
    let scope = runs::resolve_scope(&client);
    match service.list(scope.workspace_id, query.limit).await {
        Ok(page) => runs::json_response(
            request_id,
            StatusCode::OK,
            &MemoryListView {
                memories: page.memories.iter().map(memory_view).collect(),
                has_more: page.bounded,
                max_page: MAX_MEMORY_PAGE,
            },
        ),
        Err(error) => service_error(request_id, &error),
    }
}

/// `GET /api/v1/memories/search?q=...`
pub async fn search_memories(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    uri: Uri,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.memories.as_ref() else {
        return runs::not_ready(request_id);
    };
    let query = match Query::parse(&uri, true) {
        Ok(query) => query,
        Err(field) => return invalid_query(request_id, &field),
    };
    let Some(text) = query.q.filter(|text| !text.trim().is_empty()) else {
        return invalid(request_id, "q is required and must not be empty");
    };
    let scope = runs::resolve_scope(&client);
    match service.search(scope.workspace_id, &text, query.limit).await {
        Ok(search) => runs::json_response(
            request_id,
            StatusCode::OK,
            &MemorySearchView {
                hits: search
                    .hits
                    .iter()
                    .map(|hit| MemoryHitView {
                        memory: memory_view(&hit.memory),
                        relevance: hit.relevance,
                    })
                    .collect(),
                scan_bounded: search.scan_bounded,
            },
        ),
        Err(error) => service_error(request_id, &error),
    }
}

/// `GET /api/v1/memories/{memory_id}`
pub async fn read_memory(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(memory_id): Path<String>,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.memories.as_ref() else {
        return runs::not_ready(request_id);
    };
    // A malformed identifier and an absent memory are indistinguishable, so a caller cannot probe which
    // identifiers exist.
    let Ok(id) = MemoryId::parse(&memory_id) else {
        return not_found(request_id);
    };
    let scope = runs::resolve_scope(&client);
    match service.read(scope.workspace_id, id).await {
        Ok(memory) => runs::json_response(request_id, StatusCode::OK, &memory_view(&memory)),
        Err(error) => service_error(request_id, &error),
    }
}

/// `DELETE /api/v1/memories/{memory_id}`
pub async fn forget_memory(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(memory_id): Path<String>,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.memories.as_ref() else {
        return runs::not_ready(request_id);
    };
    let Ok(id) = MemoryId::parse(&memory_id) else {
        return not_found(request_id);
    };
    let scope = runs::resolve_scope(&client);
    match service.forget(scope.workspace_id, id).await {
        Ok(()) => runs::json_response(
            request_id,
            StatusCode::OK,
            &serde_json::json!({ "memory_id": id.to_string(), "forgotten": true }),
        ),
        Err(error) => service_error(request_id, &error),
    }
}

/// Renders a memory as the wire view.
fn memory_view(memory: &Memory) -> MemoryView {
    MemoryView {
        memory_id: memory.id.to_string(),
        workspace_id: memory.workspace.to_string(),
        class: memory.class.as_str().to_owned(),
        text: memory.text.as_str().to_owned(),
        sensitivity: memory.sensitivity.as_str().to_owned(),
        source_kind: memory.source.kind().to_owned(),
        source_principal_id: memory.source.principal().to_string(),
        created_at: memory.created_at.to_string(),
        updated_at: memory.updated_at.to_string(),
    }
}

fn service_error(request_id: Option<&str>, error: &MemoryServiceError) -> Response {
    match error {
        MemoryServiceError::Refused(refusal) => invalid(request_id, refusal.reason()),
        MemoryServiceError::NotFound => not_found(request_id),
        MemoryServiceError::Storage(storage) => error_response_for(
            request_id,
            StatusCode::INTERNAL_SERVER_ERROR,
            storage.code(),
            &storage.to_string(),
            storage.retryable(),
        ),
    }
}

fn not_found(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::NOT_FOUND,
        "resource.not_found",
        "No such memory.",
        false,
    )
}

fn invalid(request_id: Option<&str>, message: &str) -> Response {
    error_response_for(
        request_id,
        StatusCode::BAD_REQUEST,
        "request.invalid",
        message,
        false,
    )
}

fn invalid_query(request_id: Option<&str>, field: &str) -> Response {
    error_response_for(
        request_id,
        StatusCode::BAD_REQUEST,
        "request.invalid",
        &format!("Unsupported or malformed query parameter `{field}`."),
        false,
    )
}

fn clock_unavailable(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::INTERNAL_SERVER_ERROR,
        "run.clock_unavailable",
        "The daemon clock reported no usable instant.",
        false,
    )
}
