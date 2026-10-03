//! The tool-authorization surface: listing, reading, writing, and revoking tool grants.
//!
//! **This is the module that makes tool authorization *configurable* rather than compiled.** Until it
//! existed, what a deployment allowed was decided by `NativeReadOnlyGrants` — a constructor — so the only
//! way to change it was to change the code. An operator who wants "let `clock.now` work without asking, but
//! always ask about `email.send`" now writes a grant through this surface, and the tool pipeline consults it
//! on the next dispatch.
//!
//! Every handler does the same three things the approval surface's do: resolve the authenticated scope into a
//! trusted `RequestContext`, call [`jarvis_application::tool_grant_service`], and render the result. No
//! handler holds an authorization rule of its own — resolving the capability, checking that the grant only
//! narrows, and mapping a store conflict to a distinct code all belong to the service, so there is one place
//! to change them and one place to test them.
//!
//! Four decisions are visible in this file's shape:
//!
//! - **The body cannot assert the operator or the workspace.** [`WriteToolGrantRequest`] carries a **target
//!   principal** (a grant is *for* somebody, so that is a legitimate field) but no `granted_by` and no
//!   workspace: those come from the context, which only trusted code can build. The same structural argument
//!   `BRN-024` made for policy grants, and here it matters twice over — the workspace decides which
//!   configuration a grant lands in, and the operator is what the audit trail records.
//! - **`PUT` creates and `PATCH` replaces, and the version is what distinguishes them.** A create that
//!   silently replaced an existing grant would discard ceilings an operator configured and nobody asked to
//!   change, so the two verbs are separate routes with separate bodies rather than one route whose meaning
//!   depends on a field's incidental presence.
//! - **Bodies are parsed by hand rather than with `axum::Json`**, for the reason the approval surface
//!   records: the framework's rejection is its own response rather than the shared envelope, so a client
//!   submitting a mistyped verb would receive something it can parse nothing from.
//! - **A foreign-workspace grant is indistinguishable from a missing one.** The service enforces it, and this
//!   module must not "improve" on that by returning a different status — an authorization row is the most
//!   security-relevant in the profile, and a distinguishable refusal would confirm that another workspace has
//!   a grant for a tool.
//!
//! **The clock is read once per request**, because a row's `created_at` and the store's version check come
//! from one instant — a second read would let a row record a creation time that disagrees with the write that
//! produced it.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use jarvis_application::repository::tool_grant::{GrantListFilter, MAX_GRANT_PAGE};
use jarvis_application::tool_grant_service::{DenyRuleRequest, GrantRequest, GrantServiceError};
use jarvis_domain::ids::{PrincipalId, ToolDenyRuleId, ToolGrantId};
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::classification::Risk;
use jarvis_protocol::{
    DenyRuleCreatedView, DenyRuleView, GrantView, ToolDenyRuleListView, ToolGrantListView,
    WriteDenyRuleRequest, WriteToolGrantRequest,
};
use serde::Deserialize;

use crate::http::{ApiState, AuthenticatedClient, RequestIdOf, error_response_for, runs};
use crate::time::SystemClock;

/// Query parameters for a listing.
///
/// **`limit`, `principal`, and `active` are accepted; any other key is refused by name.** An unrecognised
/// filter that was silently ignored would return a superset of what a caller asked for — and on an
/// authorization listing that means showing grants a client believed it had excluded, which reads as
/// authority the profile does not have. Refusing an unknown key keeps each unserved filter the next increment
/// rather than a silently wrong answer.
///
/// Hand-written rather than `axum`'s `Query` extractor, because that needs the `query` feature and this
/// workspace's `axum` resolves without it — adding a dependency feature is a research-gate change, not a
/// convenience.
struct ListQuery {
    limit: u32,
    principal: Option<PrincipalId>,
    active: Option<bool>,
}

impl ListQuery {
    /// Parses the query string, refusing an unknown or malformed key.
    ///
    /// # Errors
    ///
    /// Returns the offending parameter name, so the refusal can name it rather than being generic.
    fn parse(uri: &Uri) -> Result<Self, String> {
        let mut query = Self {
            limit: MAX_GRANT_PAGE,
            principal: None,
            active: None,
        };
        let Some(raw) = uri.query() else {
            return Ok(query);
        };
        for pair in raw.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "limit" => {
                    // The store clamps to its own bound as well, so a caller asking for more receives a full
                    // page rather than a refusal — the contract's "bounded page size" behaviour.
                    let parsed = value.parse::<u32>().map_err(|_| "limit".to_owned())?;
                    query.limit = parsed.clamp(1, MAX_GRANT_PAGE);
                }
                "principal" => {
                    query.principal =
                        Some(PrincipalId::parse(value).map_err(|_| "principal".to_owned())?);
                }
                "active" => {
                    query.active = Some(match value {
                        "true" => true,
                        "false" => false,
                        _ => return Err("active".to_owned()),
                    });
                }
                other => return Err(other.to_owned()),
            }
        }
        Ok(query)
    }
}

/// `GET /api/v1/tool-grants`
pub async fn list_tool_grants(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    uri: Uri,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.tool_grants.as_ref() else {
        return runs::not_ready(request_id);
    };
    let query = match ListQuery::parse(&uri) {
        Ok(query) => query,
        Err(field) => return invalid_query(request_id, &field),
    };
    let scope = runs::resolve_scope(&client);
    let filter = GrantListFilter {
        principal: query.principal,
        active: query.active,
    };
    match service.list(scope.workspace_id, filter, query.limit).await {
        Ok(page) => runs::json_response(
            request_id,
            StatusCode::OK,
            &ToolGrantListView {
                grants: page.grants.iter().map(grant_view).collect(),
                has_more: page.bounded,
                max_page: MAX_GRANT_PAGE,
            },
        ),
        Err(error) => service_error(request_id, error),
    }
}

/// `GET /api/v1/tool-grants/{grant_id}`
pub async fn read_tool_grant(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(grant_id): Path<String>,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.tool_grants.as_ref() else {
        return runs::not_ready(request_id);
    };
    let Ok(id) = ToolGrantId::parse(&grant_id) else {
        return grant_not_found(request_id);
    };
    let scope = runs::resolve_scope(&client);
    match service.read(scope.workspace_id, id).await {
        Ok(grant) => runs::json_response(request_id, StatusCode::OK, &grant_view(&grant)),
        Err(error) => service_error(request_id, error),
    }
}

/// `PUT /api/v1/tool-grants` — **create only**.
pub async fn create_tool_grant(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    body: Bytes,
) -> Response {
    write_grant(state, client, request_id.as_deref(), body, None).await
}

/// `PATCH /api/v1/tool-grants` — **replace only**.
pub async fn replace_tool_grant(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    body: Bytes,
) -> Response {
    // The version is read **before** delegating, because the two verbs must not be interchangeable: a `PATCH`
    // with no version would create, and a `PUT` with one would replace — which is exactly the ambiguity the
    // separate routes exist to remove.
    let Ok(version) = serde_json::from_slice::<ExpectedVersionBody>(&body) else {
        return invalid_body(
            request_id.as_deref(),
            "expected_version is required to replace a grant",
        );
    };
    write_grant(
        state,
        client,
        request_id.as_deref(),
        body,
        Some(version.expected_version),
    )
    .await
}

/// The only shape a replace or a revoke body needs, read for its version alone.
///
/// A separate type from [`WriteToolGrantRequest`] so the version is **required** in these verbs: deriving it
/// from the shared request would make an absent version parse as a create, and a verb whose meaning depends
/// on a missing field is the defect the split routes remove.
#[derive(Deserialize)]
struct ExpectedVersionBody {
    /// The version the caller believes it is acting on.
    expected_version: u32,
}

/// Removes the `expected_version` field from a body whose verb carries the version separately.
///
/// **A structural edit rather than a textual one.** A body's text may contain the literal string
/// `expected_version` inside a scope name, a reason, or any other value, and a string search would corrupt
/// exactly the callers whose data happens to mention the word — removing a field is a statement about the
/// document's *shape*, so it is performed on the parsed shape. The body is already bounded by the
/// request-body middleware, so parsing it a second time costs nothing a caller can make unbounded.
///
/// Returns the input unchanged when it is not a JSON object, so the shared parse reports the malformed body
/// as `request.invalid` **once**. A second refusal invented here for the same input would give one mistake
/// two codes, and the caller would have no way to tell which one the daemon meant.
fn strip_expected_version(body: &[u8]) -> Vec<u8> {
    let Ok(serde_json::Value::Object(mut map)) = serde_json::from_slice::<serde_json::Value>(body)
    else {
        return body.to_vec();
    };
    map.remove("expected_version");
    // A `Value` that came from parsing always serializes, so the fallback is unreachable; returning the
    // original keeps it fail-*closed* rather than fail-open, because the original still lacks nothing the
    // shared parse needs to report.
    serde_json::to_vec(&serde_json::Value::Object(map)).unwrap_or_else(|_| body.to_vec())
}

/// Shared body handling for the two write verbs.
async fn write_grant(
    state: Arc<ApiState>,
    client: AuthenticatedClient,
    request_id: Option<&str>,
    body: Bytes,
    expected_version: Option<u32>,
) -> Response {
    let Some(service) = state.tool_grants.as_ref() else {
        return runs::not_ready(request_id);
    };
    // **The body is normalized for the verb before it is parsed, and this is the fix for a defect that made
    // `PATCH` unreachable.** `replace_tool_grant` requires `expected_version` in the body — that is how the
    // two verbs are kept from being interchangeable — but `WriteToolGrantRequest` carries
    // `#[serde(deny_unknown_fields)]` and does not model the field, so the *only* body `PATCH` documents was
    // the one its own parse refused with `request.invalid`. Every `PATCH` answered `400`, and nothing caught
    // it because no test of any kind invoked this handler: the route was asserted to exist and its codes were
    // asserted to be in the contract table, and both were true of a handler that could never succeed.
    //
    // Removed rather than added to the shared shape, because the shape is the **create** body: a
    // `WriteToolGrantRequest` that modelled `expected_version` would make an absent version parse as a create
    // and a supplied one silently change the verb, which is precisely the ambiguity the separate routes
    // exist to remove. The version arrives through `expected_version` — a typed argument, not a body field —
    // so stripping it here keeps the *verb* the only thing that decides, and the strip happens **after** the
    // version was read, so a malformed integer was already refused by `replace_tool_grant`'s own reader.
    let body: Bytes = match expected_version {
        Some(_) => Bytes::from(strip_expected_version(&body)),
        None => body,
    };
    let Ok(parsed) = serde_json::from_slice::<WriteToolGrantRequest>(&body) else {
        return invalid_body(request_id, "the body is not a well-formed grant request");
    };
    // The **context** carries the workspace, so it is read from there rather than resolved a second time: a
    // second `resolve_scope` call is a second place the scope is derived, and the one a later change updated
    // would be the one that decided the grant's workspace.
    let context = runs::context_for(&client, request_id);
    let workspace = context.workspace_id;
    let Ok(principal) = PrincipalId::parse(&parsed.principal_id) else {
        return invalid_body(request_id, "principal_id is not a canonical identifier");
    };
    let Ok(risk) = Risk::parse(&parsed.risk_ceiling) else {
        return invalid_body(request_id, "risk_ceiling is not a known risk level");
    };
    let Some(sensitivity) =
        jarvis_application::context_assembly::parse_sensitivity(&parsed.sensitivity_ceiling)
    else {
        return invalid_body(request_id, "sensitivity_ceiling is not a known sensitivity");
    };
    let expires_at = match parsed.expires_at.as_deref() {
        Some(value) => match UtcTimestamp::parse(value) {
            Ok(value) => Some(value),
            Err(_) => return invalid_body(request_id, "expires_at is not an RFC 3339 instant"),
        },
        None => None,
    };
    let Ok(now) = SystemClock::new().now() else {
        return clock_unavailable(request_id);
    };
    let request = GrantRequest {
        capability: parsed.capability,
        scopes: parsed.scopes,
        effects: parsed.effects,
        risk_ceiling: risk,
        sensitivity_ceiling: sensitivity,
        expires_at,
        workspace,
        principal,
        expected_version,
    };
    match service.write(&context, &request, now).await {
        Ok(grant) => runs::json_response(
            request_id,
            if expected_version.is_some() {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            },
            &grant_view(&grant),
        ),
        Err(error) => service_error(request_id, error),
    }
}

/// `POST /api/v1/tool-grants/{grant_id}/revoke`
pub async fn revoke_tool_grant(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(grant_id): Path<String>,
    body: Bytes,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.tool_grants.as_ref() else {
        return runs::not_ready(request_id);
    };
    let Ok(id) = ToolGrantId::parse(&grant_id) else {
        return grant_not_found(request_id);
    };
    let Ok(version) = serde_json::from_slice::<ExpectedVersionBody>(&body) else {
        return invalid_body(request_id, "expected_version is required to revoke a grant");
    };
    // The workspace comes from the authenticated client, so a caller cannot revoke another workspace's grant
    // — there is no field in which to name one.
    let scope = runs::resolve_scope(&client);
    let Ok(now) = SystemClock::new().now() else {
        return clock_unavailable(request_id);
    };
    match service
        .revoke(scope.workspace_id, id, version.expected_version, now)
        .await
    {
        Ok(grant) => runs::json_response(request_id, StatusCode::OK, &grant_view(&grant)),
        Err(error) => service_error(request_id, error),
    }
}

/// `GET /api/v1/tool-grants/deny-rules`
pub async fn list_deny_rules(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    uri: Uri,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.tool_grants.as_ref() else {
        return runs::not_ready(request_id);
    };
    let query = match ListQuery::parse(&uri) {
        Ok(query) => query,
        Err(field) => return invalid_query(request_id, &field),
    };
    let scope = runs::resolve_scope(&client);
    match service
        .list_deny_rules(scope.workspace_id, query.limit)
        .await
    {
        Ok(rules) => runs::json_response(
            request_id,
            StatusCode::OK,
            &ToolDenyRuleListView {
                rules: rules.iter().map(deny_rule_view).collect(),
            },
        ),
        Err(error) => service_error(request_id, error),
    }
}

/// `POST /api/v1/tool-grants/deny-rules`
pub async fn add_deny_rule(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    body: Bytes,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.tool_grants.as_ref() else {
        return runs::not_ready(request_id);
    };
    let Ok(parsed) = serde_json::from_slice::<WriteDenyRuleRequest>(&body) else {
        return invalid_body(
            request_id,
            "the body is not a well-formed deny-rule request",
        );
    };
    let context = runs::context_for(&client, request_id);
    let principal = match parsed.principal_id.as_deref() {
        Some(value) => match PrincipalId::parse(value) {
            Ok(value) => Some(value),
            Err(_) => {
                return invalid_body(request_id, "principal_id is not a canonical identifier");
            }
        },
        None => None,
    };
    let Ok(now) = SystemClock::new().now() else {
        return clock_unavailable(request_id);
    };
    let request = DenyRuleRequest {
        capability: parsed.capability.as_deref(),
        principal,
        effects: &parsed.effects,
        reason: &parsed.reason,
        // The wire field says whether the refusal is scoped to the caller's workspace. A profile-wide
        // refusal is the broadest form and stays expressible, which is why this is a required boolean rather
        // than an optional one defaulting to the narrow case.
        workspace_wide: parsed.workspace_wide,
    };
    match service.add_deny_rule(&context, &request, now).await {
        Ok(id) => runs::json_response(
            request_id,
            StatusCode::CREATED,
            &DenyRuleCreatedView {
                deny_rule_id: id.to_string(),
            },
        ),
        Err(error) => service_error(request_id, error),
    }
}

/// `DELETE /api/v1/tool-grants/deny-rules/{rule_id}`
pub async fn remove_deny_rule(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(rule_id): Path<String>,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.tool_grants.as_ref() else {
        return runs::not_ready(request_id);
    };
    let Ok(id) = ToolDenyRuleId::parse(&rule_id) else {
        return grant_not_found(request_id);
    };
    let scope = runs::resolve_scope(&client);
    match service.remove_deny_rule(scope.workspace_id, id).await {
        Ok(()) => runs::json_response(
            request_id,
            StatusCode::OK,
            &DenyRuleCreatedView {
                deny_rule_id: id.to_string(),
            },
        ),
        Err(error) => service_error(request_id, error),
    }
}

/// Renders the clock refusal.
///
/// A clock that cannot report is an environment fault, so the surface reports it rather than writing a row
/// with a guessed instant — a grant's creation time is what the audit trail shows, and a fabricated one is
/// worse than a refusal.
fn clock_unavailable(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::INTERNAL_SERVER_ERROR,
        "run.clock_unavailable",
        "The daemon clock reported no usable instant.",
        false,
    )
}

/// Renders the not-found refusal for a grant or a deny rule.
///
/// **The same code for both**, because a malformed identifier and an absent row must be indistinguishable:
/// distinguishing them would let a caller probe which identifiers exist, and the rule for this surface is
/// that another scope's resource reads as missing rather than as forbidden.
fn grant_not_found(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::NOT_FOUND,
        "tool.grant_not_found",
        "No such tool grant.",
        false,
    )
}

/// Renders a grant as the wire view.
///
/// Every value goes through the **domain's own** spelling method, so a rename inside the domain cannot
/// silently change a client-visible value.
fn grant_view(grant: &jarvis_application::repository::tool_grant::StoredToolGrant) -> GrantView {
    GrantView {
        grant_id: grant.id.to_string(),
        capability: grant.grant.identity.capability.to_string(),
        principal_id: grant.grant.principal.to_string(),
        workspace_id: grant.grant.workspace.to_string(),
        scopes: grant
            .grant
            .scopes
            .iter()
            .map(|scope| scope.as_str().to_owned())
            .collect(),
        effects: grant
            .grant
            .effects
            .iter()
            .map(|effect| effect.as_contract_str().to_owned())
            .collect(),
        risk_ceiling: grant.grant.risk_ceiling.as_contract_str().to_owned(),
        sensitivity_ceiling: grant.grant.sensitivity_ceiling.as_str().to_owned(),
        expires_at: grant.grant.expires_at.map(|value| value.to_string()),
        active: grant.active,
        version: grant.version,
        granted_by: grant.granted_by.to_string(),
        created_at: grant.created_at.to_string(),
        updated_at: grant.updated_at.to_string(),
    }
}

/// Renders a deny rule as the wire view.
fn deny_rule_view(
    rule: &jarvis_application::repository::tool_grant::StoredDenyRule,
) -> DenyRuleView {
    DenyRuleView {
        deny_rule_id: rule.id.to_string(),
        capability: rule.capability.clone(),
        principal_id: rule.rule.principal.map(|principal| principal.to_string()),
        workspace_id: rule.rule.workspace.map(|workspace| workspace.to_string()),
        effects: rule
            .rule
            .effects
            .iter()
            .map(|effect| effect.as_contract_str().to_owned())
            .collect(),
        reason: rule.reason.clone(),
        created_at: rule.created_at.to_string(),
    }
}

/// Renders a service error through the shared envelope.
fn service_error(request_id: Option<&str>, error: GrantServiceError) -> Response {
    // The status comes from the **error's own kind** rather than from the code string, so the two cannot
    // disagree: a code sent with a status that says something else is how a client branches on one and
    // reports the other.
    let status = match error {
        GrantServiceError::NotFound => StatusCode::NOT_FOUND,
        GrantServiceError::AlreadyExists
        | GrantServiceError::Widens { .. }
        | GrantServiceError::UnknownTool
        | GrantServiceError::UnknownEffect
        | GrantServiceError::InvalidScope
        | GrantServiceError::InvalidDenyRule => StatusCode::BAD_REQUEST,
        GrantServiceError::VersionConflict { .. } => StatusCode::CONFLICT,
        // A deployment whose pipeline is unavailable could not answer, which is neither the caller's fault
        // nor a missing resource — so it is a readiness refusal and the code says so.
        GrantServiceError::PipelineUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        GrantServiceError::Storage => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response_for(
        request_id,
        status,
        error.code(),
        &error.to_string(),
        error.retryable(),
    )
}

/// Renders a body refusal.
fn invalid_body(request_id: Option<&str>, message: &str) -> Response {
    error_response_for(
        request_id,
        StatusCode::BAD_REQUEST,
        "request.invalid",
        message,
        false,
    )
}

/// Renders an unknown-query-parameter refusal, naming the parameter.
fn invalid_query(request_id: Option<&str>, field: &str) -> Response {
    error_response_for(
        request_id,
        StatusCode::BAD_REQUEST,
        "request.invalid",
        &format!("Unsupported query parameter `{field}`."),
        false,
    )
}
