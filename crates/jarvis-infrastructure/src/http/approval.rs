//! The approval surface: listing, reading, deciding, and withdrawing approval requests.
//!
//! The five routes `docs/contracts/approval-contract.md` names live here, and every handler does the
//! same three things: resolve the authenticated scope into a trusted
//! [`RequestContext`](jarvis_application::request_context::RequestContext), call
//! [`jarvis_application::approval_service`], and render the result. No handler holds an approval rule
//! of its own — the channel check, the assurance ladder, the expiry computation, and the idempotence
//! of a repeat all belong to the service, so there is one place to change them and one place to test
//! them.
//!
//! Four contract rules are visible in this file's shape:
//!
//! - **The body cannot assert the decider.** [`jarvis_protocol::DecideApprovalRequest`] has no field
//!   for the principal, channel, assurance, or time, so there is nothing to ignore — a field the
//!   handler had to remember not to read is a field that eventually gets read. The same structural
//!   argument `BRN-024` made for policy grants.
//! - **`Idempotency-Key` is required on `decide` and `cancel`.** The contract says so for both. A
//!   retry of a timed-out decision must reach the same answer rather than a second decision, and the
//!   service's own repeat handling is what makes the retry idempotent — the key is checked for
//!   presence, not recorded, because recording it would be a second mechanism for one guarantee.
//! - **Bodies are parsed by hand rather than with `axum::Json`.** The framework's rejection is its own
//!   response and not the shared envelope, so a client submitting a mistyped verb would receive a
//!   status it can parse nothing from. Every other command route on this surface parses by hand for
//!   the same reason, and `http::require_json_content_type` already handles the media-type half for
//!   every route at once.
//! - **A foreign-workspace record is indistinguishable from a missing one**, which the service
//!   enforces and this module does not need to re-check. The refusal is reported through the shared
//!   envelope with the service's own stable code.
//!
//! **The clock is read once per request.** `decided_at` and the lapsed computation both come from one
//! instant, so a decision cannot record an instant that disagrees with the expiry check that permitted
//! it — the same rule the policy surface's `evaluation_instant` follows.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use jarvis_application::approval_service::{ApprovalServiceError, Decision, DecisionCommand};
use jarvis_application::repository::approval::ApprovalCursor;
use jarvis_application::request_context::RequestContext;
use jarvis_domain::ids::ApprovalId;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::approval::{
    ApprovalChannel, ApprovalVersion, DecisionNote, DurableApproval,
};
use jarvis_domain::tool::canonical::ActionDigest;
use jarvis_protocol::{
    ApprovalDecisionResponse, ApprovalListView, ApprovalView, CancelApprovalRequest,
    DecideApprovalRequest, MAX_APPROVAL_PAGE, PreviewRowView,
};

use crate::http::{ApiState, AuthenticatedClient, RequestIdOf, error_response_for, runs};
use crate::storage::repositories::assurance_stored;
use crate::time::SystemClock;

/// Query parameters for the listing.
///
/// **`limit` and `cursor` are accepted; any other key is refused by name.**
///
/// That narrowing is deliberate rather than an omission. The contract's filters are "schema-defined:
/// state, risk, effect, requesting run/tool, and created/expiry time", and this build serves the
/// default view — because an unrecognised filter that was silently *ignored* would return a superset
/// of what a caller asked for, and on an approval listing that means showing prompts a client believed
/// it had excluded. Refusing an unknown key makes the unsupported filters the next increment rather
/// than a silently wrong answer, which is the same choice the write shapes make with
/// `deny_unknown_fields`.
///
/// Parsed from the URI by hand rather than with `axum`'s `Query` extractor, because that extractor
/// needs the `query` feature and this workspace's `axum` resolves with `http1`, `json`, and `tokio`
/// only — adding a feature to the dependency graph is a research-gate change, not a convenience.
/// Why a listing query was refused.
///
/// Two variants rather than one, because the refusals send a client to different inputs: an unknown
/// *filter* is a request this build does not serve, while an unusable *cursor* is a position this
/// listing never handed out. Both are `400`, and the codes differ so a client can tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryError {
    /// A key this build does not serve, or a limit that does not parse.
    Other,
    /// A cursor that is not a well-formed value this build produced.
    Cursor,
}

fn parse_list_query(uri: &Uri) -> Result<(Option<u32>, Option<ApprovalCursor>), QueryError> {
    let Some(query) = uri.query() else {
        return Ok((None, None));
    };
    let mut limit = None;
    let mut cursor = None;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').ok_or(QueryError::Other)?;
        match key {
            "limit" => limit = Some(value.parse::<u32>().map_err(|_| QueryError::Other)?),
            // **A bad cursor reports its own code, not the generic body defect.** Both are `400`, and
            // the distinction is what tells a client which of its own inputs to fix: an unknown
            // *filter* means the request asked for something this build does not serve, while an
            // unusable cursor means the position it is resuming from is not one this listing handed
            // out. Collapsing them would send a client to re-check a query string that was fine.
            "cursor" => cursor = Some(decode_cursor(value).ok_or(QueryError::Cursor)?),
            _ => return Err(QueryError::Other),
        }
    }
    Ok((limit, cursor))
}

/// Encodes a page position as an opaque query value.
///
/// **Opaque to the client, and the encoding is the reason rather than a flourish.** The contract says
/// the cursor is opaque, which is a promise about what a client may do with it: it may pass it back
/// and nothing else. A readable `expires_at,id` pair would invite a client to construct one — and a
/// constructed cursor would name a position the listing never produced, which is a way to *skip*
/// approvals rather than merely to read them oddly. Base64-with-a-prefix also keeps the value
/// URL-safe without percent-encoding, so a client cannot mangle it by re-encoding the query.
///
/// **It is not a security boundary.** Everything a cursor carries is data the client already received
/// (the last row's expiry and identifier) plus the channel it fetched on, and the store re-applies its
/// own workspace and channel predicates regardless of what the value claims. Opaqueness here is about
/// honesty of use, not about hiding anything: a hand-forged cursor can only ask for a legitimate page
/// of the caller's own view.
fn encode_cursor(cursor: &ApprovalCursor) -> String {
    let raw = format!(
        "{}|{}|{}",
        cursor.expires_at,
        cursor.id,
        cursor.channel.as_contract_str()
    );
    format!("v1.{}", base64url(raw.as_bytes()))
}

/// Decodes a cursor produced by [`encode_cursor`].
///
/// Returns `None` for anything that is not a well-formed cursor this build wrote, so a malformed or
/// truncated value is a `400 request.invalid_cursor` rather than a silent restart from page one — the
/// failure a client would never detect, because page one looks like a plausible answer.
fn decode_cursor(value: &str) -> Option<ApprovalCursor> {
    let encoded = value.strip_prefix("v1.")?;
    let raw = String::from_utf8(base64url_decode(encoded)?).ok()?;
    let mut parts = raw.split('|');
    let expires_at = UtcTimestamp::parse(parts.next()?).ok()?;
    let id = ApprovalId::parse(parts.next()?).ok()?;
    let channel = ApprovalChannel::parse(parts.next()?).ok()?;
    // A trailing segment means a field was injected into the payload, so the value is refused rather
    // than read with the extra part ignored — reading a prefix of something is how a parser accepts
    // a value the writer never produced.
    if parts.next().is_some() {
        return None;
    }
    Some(ApprovalCursor {
        expires_at,
        id,
        channel,
    })
}

/// Encodes bytes as URL-safe base64 without padding.
///
/// Hand-written rather than pulled from a crate: the alphabet is fixed and tiny, and this is an
/// encoding for one query parameter rather than a protocol. Adding `base64` for this would be a
/// dependency decision the research gate requires evidence for, for eleven lines.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |byte| u32::from(*byte));
        let b2 = chunk.get(2).map_or(0, |byte| u32::from(*byte));
        let packed = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((packed >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((packed >> 12) & 0x3f) as usize] as char);
        // The two trailing characters are emitted only when their bits came from real input, which is
        // what makes the encoding round-trip without a padding character.
        if chunk.len() > 1 {
            out.push(ALPHABET[((packed >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(packed & 0x3f) as usize] as char);
        }
    }
    out
}

/// Decodes URL-safe base64 without padding.
///
/// Returns `None` for a character outside the alphabet, so a hand-edited cursor is refused rather than
/// decoded into bytes nobody wrote.
fn base64url_decode(value: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(value.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits = 0_u32;
    for character in value.bytes() {
        let digit = match character {
            b'A'..=b'Z' => u32::from(character - b'A'),
            b'a'..=b'z' => u32::from(character - b'a') + 26,
            b'0'..=b'9' => u32::from(character - b'0') + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | digit;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulator >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Handles `GET /api/v1/approvals`.
pub async fn list_approvals(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    uri: Uri,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.approvals.as_ref() else {
        return runs::not_ready(request_id);
    };
    let (limit, cursor) = match parse_list_query(&uri) {
        Ok(parsed) => parsed,
        Err(QueryError::Cursor) => {
            return error_response_for(
                request_id,
                StatusCode::BAD_REQUEST,
                "request.invalid_cursor",
                "The page cursor is not valid for this request.",
                false,
            );
        }
        Err(QueryError::Other) => {
            return error_response_for(
                request_id,
                StatusCode::BAD_REQUEST,
                "request.invalid",
                "The request is not valid for this endpoint.",
                false,
            );
        }
    };
    let limit = limit.unwrap_or(MAX_APPROVAL_PAGE);
    let context = context_for(&client, request_id);
    // **The clock is read once and passed to the service**, so the lapse check the listing performs and
    // the `lapsed` flag every row reports are derived from one instant. Two clock reads would let a row
    // be excluded as lapsed while another row in the same response reported `lapsed: false` against the
    // same deadline.
    let now = clock_now();
    match service.list(&context, limit, cursor, now).await {
        Ok(page) => {
            let view = ApprovalListView {
                approvals: page
                    .approvals
                    .iter()
                    .map(|approval| view_of(approval, now))
                    .collect(),
                max_page: MAX_APPROVAL_PAGE,
                has_more: page.bounded,
                // Rendered from the store's own position, never from `approvals.last()` here: the
                // store knows the column the query ordered by, and deriving it in the handler would be
                // a second definition of the order that could drift from the `ORDER BY` it must match.
                next_cursor: page.next.as_ref().map(encode_cursor),
            };
            runs::json_response(request_id, StatusCode::OK, &view)
        }
        Err(error) => approval_error_response(request_id, &error),
    }
}

/// Handles `GET /api/v1/approvals/{approval_id}`.
pub async fn read_approval(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(approval_id): Path<String>,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.approvals.as_ref() else {
        return runs::not_ready(request_id);
    };
    let Ok(approval_id) = ApprovalId::parse(&approval_id) else {
        // A malformed identifier is reported as **not found**, not as a bad request, because the
        // contract requires an unknown approval and an unparseable one to be indistinguishable —
        // the same rule the run surface follows, and the reason the body is the not-found envelope
        // rather than a validation message that would confirm which identifiers exist.
        return not_found(request_id);
    };
    let context = context_for(&client, request_id);
    // One instant for the lapse check and the rendered `lapsed` flag, for the reason the listing states.
    let now = clock_now();
    match service.read(&context, approval_id, now).await {
        Ok(approval) => {
            // **Detail reads the trail, and a listing does not.** This is where the contract puts "prior
            // decision metadata": the operator's own note on a decision lives on that decision's
            // transition rather than in a column, so a listing would need one trail read per row — the
            // N+1 this split exists to avoid.
            //
            // A trail that cannot be read **fails the read** rather than being reported as an absent note.
            // Swallowing it would answer `decided_note: null` for a record that has one, which a client
            // cannot tell from "nobody wrote a comment" — the same "an error rendered as a legitimate
            // value" defect this project has fixed on the run and policy surfaces.
            let note = match service.last_decision_note(&context, approval_id).await {
                Ok(note) => note,
                Err(error) => return approval_error_response(request_id, &error),
            };
            let mut view = view_of(&approval, clock_now());
            view.decided_note = note;
            runs::json_response(request_id, StatusCode::OK, &view)
        }
        Err(error) => approval_error_response(request_id, &error),
    }
}

/// Handles `POST /api/v1/approvals/{approval_id}/decide`.
pub async fn decide_approval(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(approval_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.approvals.as_ref() else {
        return runs::not_ready(request_id);
    };
    if !headers.contains_key("idempotency-key") {
        return error_response_for(
            request_id,
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "An idempotency key is required.",
            false,
        );
    }
    let Ok(approval_id) = ApprovalId::parse(&approval_id) else {
        return not_found(request_id);
    };
    let request: DecideApprovalRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        // A malformed or unknown-field body is the caller's error. The rejected value is not echoed,
        // because it is caller-supplied text.
        Err(_) => {
            return error_response_for(
                request_id,
                StatusCode::BAD_REQUEST,
                "request.invalid",
                "The request body is not valid for this endpoint.",
                false,
            );
        }
    };
    let decision = match Decision::parse(&request.decision) {
        Ok(decision) => decision,
        Err(error) => return approval_error_response(request_id, &error),
    };
    // **The fingerprint is parsed here, at the trust boundary**, and a malformed one is a `request.invalid`
    // rather than a comparison that silently fails. The wire carries the contract's `sha256:<hex>` text and
    // the domain type validates its shape — algorithm prefix and 64 lowercase hex characters — so a client
    // that sent `md5:...`, a bare digest, or an uppercase one is told its request was malformed instead of
    // being told the action changed. The two are different failures: one means "fix your request", the
    // other means "re-approve", and reporting the second for the first would send a user to re-approve an
    // action whose fingerprint they never had a way to compute correctly.
    let Ok(fingerprint) = ActionDigest::parse(&request.action_fingerprint) else {
        return error_response_for(
            request_id,
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "The request body is not valid for this endpoint.",
            false,
        );
    };
    let Some(expected) = version_of(request.expected_version) else {
        // Version zero is refused by the domain's own `ApprovalVersion::new` contract, and a body
        // stating it is a caller mistake rather than a decision — reported as such rather than
        // clamped, because clamping would hide which version the caller meant.
        return approval_error_response(
            request_id,
            &ApprovalServiceError::Invalid {
                code: "request.invalid",
            },
        );
    };
    // The operator's comment is **validated at the boundary**, so an unusable one is a `request.invalid`
    // rather than a value that reaches the service and is silently dropped — which is what happened before
    // this round, when the field was deserialized and nothing stored it. An absent comment is `None` and
    // distinct from an empty one, which `DecisionNote::new` refuses.
    let note = match request.comment.as_deref().map(DecisionNote::new) {
        None => None,
        Some(Ok(note)) => Some(note),
        Some(Err(_)) => {
            return error_response_for(
                request_id,
                StatusCode::BAD_REQUEST,
                "request.invalid",
                "The request body is not valid for this endpoint.",
                false,
            );
        }
    };
    let context = context_for(&client, request_id);
    match service
        .decide(
            &context,
            approval_id,
            DecisionCommand {
                decision,
                expected_version: expected,
                fingerprint: &fingerprint,
                note: note.as_ref(),
            },
            clock_now(),
        )
        .await
    {
        Ok(decided) => runs::json_response(
            request_id,
            StatusCode::OK,
            &ApprovalDecisionResponse {
                approval: view_of(&decided.approval, clock_now()),
                applied: decided.applied,
            },
        ),
        Err(error) => approval_error_response(request_id, &error),
    }
}

/// Handles `POST /api/v1/approvals/{approval_id}/cancel`.
pub async fn cancel_approval(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    Path(approval_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.approvals.as_ref() else {
        return runs::not_ready(request_id);
    };
    if !headers.contains_key("idempotency-key") {
        return error_response_for(
            request_id,
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "An idempotency key is required.",
            false,
        );
    }
    let Ok(approval_id) = ApprovalId::parse(&approval_id) else {
        return not_found(request_id);
    };
    let request: CancelApprovalRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return error_response_for(
                request_id,
                StatusCode::BAD_REQUEST,
                "request.invalid",
                "The request body is not valid for this endpoint.",
                false,
            );
        }
    };
    let Some(expected) = version_of(request.expected_version) else {
        return approval_error_response(
            request_id,
            &ApprovalServiceError::Invalid {
                code: "request.invalid",
            },
        );
    };
    // The reason is validated here for the same reason the decision's comment is: one definition of
    // usable note text, at the boundary where a caller's string arrives, and an unusable one is a
    // `request.invalid` rather than a value that reaches the service and stops.
    let reason = match request.reason.as_deref().map(DecisionNote::new) {
        None => None,
        Some(Ok(note)) => Some(note),
        Some(Err(_)) => {
            return error_response_for(
                request_id,
                StatusCode::BAD_REQUEST,
                "request.invalid",
                "The request body is not valid for this endpoint.",
                false,
            );
        }
    };
    let context = context_for(&client, request_id);
    match service
        .cancel(
            &context,
            approval_id,
            expected,
            reason.as_ref(),
            clock_now(),
        )
        .await
    {
        Ok(cancelled) => runs::json_response(
            request_id,
            StatusCode::OK,
            &ApprovalDecisionResponse {
                approval: view_of(&cancelled.approval, clock_now()),
                applied: cancelled.applied,
            },
        ),
        Err(error) => approval_error_response(request_id, &error),
    }
}

/// The instant this request evaluates at.
///
/// A clock failure yields the **unix epoch**, which makes every real approval read as lapsed: the
/// fail-closed direction, because the alternative would be to treat an unreadable clock as "not yet
/// expired" and honour a request whose deadline may have passed. Reading once per request is what
/// keeps the recorded instant and the expiry check answering from the same value.
fn clock_now() -> UtcTimestamp {
    SystemClock::new().now().unwrap_or(UtcTimestamp::UNIX_EPOCH)
}

/// Builds the trusted context for a request.
///
/// Delegates to the run surface's own builder rather than assembling a second one: a context built at
/// more than one site is more than one place the principal, workspace, or channel could be wired
/// differently, and one of them being wrong would be invisible until the surface it belongs to was
/// exercised.
fn context_for(client: &AuthenticatedClient, request_id: Option<&str>) -> RequestContext {
    runs::context_for(client, request_id)
}

/// Converts a wire version into a domain version, refusing zero.
///
/// Zero is what an uninitialised counter produces, and the domain refuses it — so a body stating it
/// is a caller mistake rather than the first version, and clamping it to one would decide an approval
/// against a version the caller never named.
fn version_of(value: u64) -> Option<ApprovalVersion> {
    if value == 0 {
        None
    } else {
        Some(ApprovalVersion::new(value))
    }
}

/// Renders an approval as the wire view.
///
/// Every value is rendered through the **domain's own** spelling method, so a rename inside the domain
/// cannot silently change a client-visible value — the property the policy surface's view tests
/// assert, and the one whose absence let `rejected_view` render operator prose where the contract
/// specified a code.
///
/// `now` is passed in rather than read here so a listing renders every record against one instant, and
/// the `lapsed` flag cannot disagree between two rows in the same response.
fn view_of(approval: &DurableApproval, now: UtcTimestamp) -> ApprovalView {
    let [
        tool_id,
        tool_source_kind,
        tool_source_owner,
        tool_source_version,
    ] = identity_of(approval);
    ApprovalView {
        approval_id: approval.id.to_string(),
        workspace_id: approval.workspace.to_string(),
        requesting_principal_id: approval.requesting_principal.to_string(),
        run_id: approval.run.to_string(),
        tool_call_id: approval.tool_call.to_string(),
        tool_id,
        // The **source and schema** travel with the capability, because the contract's detail
        // requirement names "tool source/schema identity" and the rule it protects is `ACC-024`: an
        // approval binds to the implementation rather than to a name that can be re-pointed. A client
        // shown only the capability could not tell that the tool behind it was replaced, which is the
        // review decision detail exists to support. Rendered from the domain's own contract spellings,
        // so a client reads the same vocabulary the daemon stores.
        tool_source_kind,
        tool_source_owner,
        tool_source_version,
        schema_fingerprint: approval.identity.schema_fingerprint.to_string(),
        action_fingerprint: approval.action_digest.to_string(),
        risk: approval.risk.as_contract_str().to_owned(),
        effects: approval
            .effects
            .iter()
            .map(|effect| effect.as_contract_str().to_owned())
            .collect(),
        summary: approval.summary.clone(),
        preview: preview_rows(approval),
        allowed_channels: approval
            .allowed_channels
            .as_slice()
            .iter()
            .map(|channel| channel.as_contract_str().to_owned())
            .collect(),
        expires_at: approval.expires_at.to_string(),
        scope: approval.scope.as_contract_str().to_owned(),
        state: approval.state().as_contract_str().to_owned(),
        version: approval.version().get(),
        decided_by: approval.decided_by().map(|principal| principal.to_string()),
        decided_via: approval
            .decided_via()
            .map(|channel| channel.as_contract_str().to_owned()),
        // The contract's audit section requires the assurance recorded, and it is read from the **record**
        // rather than from a transition actor, so a later consumption cannot clear it. A non-decision
        // record has none, and it is omitted rather than rendered as `standard` — "not recorded" and "the
        // weakest level" are different answers and a client must be able to tell them apart.
        decided_assurance: approval
            .decided_assurance()
            .map(|assurance| assurance_stored(assurance).to_owned()),
        decided_at: approval.decided_at().map(|at| at.to_string()),
        // Filled by the **detail** read, which reads the trail; a listing leaves it absent rather than
        // performing one trail read per row. `None` here is therefore "not asked for on this route",
        // which is a different fact from "no note exists" — and the two are kept apart by the route
        // rather than by a sentinel.
        decided_note: None,
        lapsed: approval.is_lapsed_at(now),
    }
}

/// Renders the tool identity a view reports: the capability, its source kind, owner, and version.
///
/// **Extracted when `view_of` reached clippy's line bound, and the bound was a real signal rather than
/// noise.** The projection had grown long enough that a field could be dropped without a reader
/// noticing — which is exactly what happened to the schema fingerprint when it was added to the record
/// but not to this view. Splitting the identity out makes each group checkable against the contract
/// sentence that names it, and the four-element return is destructured at the call site so a dropped
/// element is a compile error rather than a missing field.
fn identity_of(approval: &DurableApproval) -> [String; 4] {
    [
        approval.identity.capability.to_string(),
        approval.identity.source.kind.as_contract_str().to_owned(),
        approval.identity.source.owner.clone(),
        approval.identity.source.version.to_string(),
    ]
}

/// Renders the preview rows, already redacted by the producer.
fn preview_rows(approval: &DurableApproval) -> Vec<PreviewRowView> {
    approval
        .preview
        .items()
        .iter()
        .map(|item| PreviewRowView {
            key: item.key.clone(),
            value: item.value.clone(),
        })
        .collect()
}

/// Returns the shared not-found envelope.
fn not_found(request_id: Option<&str>) -> Response {
    error_response_for(
        request_id,
        StatusCode::NOT_FOUND,
        "approval.not_found",
        "No such approval.",
        false,
    )
}
/// Renders a service refusal as the shared envelope.
///
/// The status follows the code rather than the other way round, because the contract's stable-error
/// list is what a client keys on — and `retryable` is the service's own answer, so the two cannot
/// disagree about whether a second attempt could succeed.
fn approval_error_response(request_id: Option<&str>, error: &ApprovalServiceError) -> Response {
    let status = match error {
        ApprovalServiceError::NotFound => StatusCode::NOT_FOUND,
        ApprovalServiceError::Unauthenticated => StatusCode::UNAUTHORIZED,
        ApprovalServiceError::ScopeDenied { .. } | ApprovalServiceError::InsufficientAssurance => {
            StatusCode::FORBIDDEN
        }
        ApprovalServiceError::VersionConflict { .. }
        | ApprovalServiceError::Expired
        | ApprovalServiceError::FingerprintMismatch
        | ApprovalServiceError::AlreadyConsumed
        | ApprovalServiceError::StateConflict => StatusCode::CONFLICT,
        ApprovalServiceError::Invalid { .. } | ApprovalServiceError::InvalidCursor => {
            StatusCode::BAD_REQUEST
        }
        ApprovalServiceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response_for(
        request_id,
        status,
        error.code(),
        message_of(error),
        error.retryable(),
    )
}

/// A stable, non-disclosing message for a refusal.
///
/// One sentence per variant, and none of them names another workspace's record, a hidden argument, or
/// a preview value — the contract requires the envelope to disclose none of those, and a message
/// built from the record would be the field through which one leaked.
fn message_of(error: &ApprovalServiceError) -> &'static str {
    match error {
        ApprovalServiceError::NotFound => "No such approval.",
        ApprovalServiceError::ScopeDenied { .. } => {
            "This client may not inspect or decide that approval."
        }
        ApprovalServiceError::Expired => "The approval request has expired.",
        ApprovalServiceError::FingerprintMismatch => {
            "The reviewed action is not the action this approval authorizes."
        }
        ApprovalServiceError::VersionConflict { .. } => {
            "The approval has changed since it was read."
        }
        ApprovalServiceError::AlreadyConsumed => "This approval has already been used.",
        ApprovalServiceError::StateConflict => {
            "The approval is not in a state this operation can act on."
        }
        ApprovalServiceError::Invalid { .. } => "The request is not valid for this endpoint.",
        ApprovalServiceError::InvalidCursor => "The page cursor is not valid for this request.",
        ApprovalServiceError::Unauthenticated => "Authentication is required.",
        ApprovalServiceError::InsufficientAssurance => {
            "This decision requires a stronger authentication assurance."
        }
        ApprovalServiceError::Storage(_) => "The request could not be completed.",
    }
}

#[cfg(test)]
#[path = "approval/tests.rs"]
mod tests;
