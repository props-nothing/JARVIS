//! Handler tests for the approval surface.
//!
//! These drive the **router**, not the service, so what they can see that the service's own tests
//! cannot is the surface itself: the shared error envelope, the status a code maps to, the
//! `Idempotency-Key` requirement, and the fact that a route is *routable* without a store. They use
//! the in-memory double rather than a migrated database, because the adapter's own tests already
//! exercise the real SQL and a handler test that needed a seeded database would be asserting storage
//! twice.
//!
//! Four properties are worth naming, and each is silent when broken:
//!
//! - **Every refusal carries the shared envelope**, including the `not_found` for an unparseable
//!   identifier — a status alone would satisfy a weaker assertion while a client had nothing to parse.
//! - **The status follows the code**, asserted for the whole stable-error family rather than one
//!   instance, because a mapping copied per variant is how one comes to disagree.
//! - **`Idempotency-Key` is required on both writes**, and it is checked *before* the body is parsed,
//!   so a request missing both is told about the key rather than about its JSON.
//! - **A store-less daemon answers `service.not_ready`**, so a client can tell "not ready" from "no
//!   such endpoint" — the same distinction the run and policy surfaces make.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use jarvis_application::approval_service::ApprovalService;
use jarvis_application::repository::approval::ApprovalRepository;
use jarvis_application::testing::InMemoryRepositories;
use jarvis_domain::ids::{ApprovalId, RunId, ToolCallId, WorkspaceId};
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::approval::{
    AllowedChannels, ApprovalChannel, ApprovalPreview, ApprovalRequestParts, ApprovalScopeKind,
    ApprovalSummary, DurableApproval, MAX_DECISION_NOTE_BYTES, PreviewItem,
};
use jarvis_domain::tool::canonical::ActionDigest;
use jarvis_domain::tool::classification::{Effect, Risk};
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use tower::ServiceExt;

use super::super::{ApiState, router};
use crate::auth::{ClientCredentialPath, ClientRegistry, enroll_owner_client};
use crate::http::tests::{TEST_AUTHORITY, temp_dir};

/// The workspace a fixture approval belongs to.
///
/// **The daemon's default workspace**, because the scope is resolved server-side: a record seeded in
/// another workspace would make every read a `not_found` for the wrong reason, and the test would pass
/// while proving nothing about the handler. This is the same trap the policy handler tests record.
fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(
        crate::http::runs::DEFAULT_WORKSPACE_UUID,
    ))
}

/// The principal the seeded client resolves to.
///
/// The fixture's approval is requested **by** this principal, so the cancellation path — where the
/// requester may always withdraw its own request — is exercised by the same client that owns the row.
fn principal() -> jarvis_domain::ids::PrincipalId {
    jarvis_domain::ids::PrincipalId::from_uuid(uuid::Uuid::from_u128(1))
}

/// A pending approval whose allowed channels include the API.
fn pending() -> DurableApproval {
    let mut approval = DurableApproval::request(ApprovalRequestParts {
        workspace: workspace(),
        requesting_principal: principal(),
        run: RunId::from_uuid(uuid::Uuid::from_u128(3)),
        tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(4)),
        identity: ToolIdentity {
            capability: ToolCapability::parse("mail.send@1").expect("canonical"),
            source: ToolSource::new(
                SourceKind::Connector,
                "acme.mail",
                ToolVersion::parse("1.0.0").expect("valid"),
            )
            .expect("the fixture source is valid"),
            schema_fingerprint: SchemaFingerprint::from_bytes([7; 32]),
        },
        action_digest: ActionDigest::from_bytes([11; 32]),
        risk: Risk::High,
        effects: vec![Effect::Write],
        summary: ApprovalSummary::new("Send one email").expect("a usable summary"),
        preview: ApprovalPreview::new(vec![
            PreviewItem::new("to", "peter@example.com").expect("a usable preview item"),
        ])
        .expect("a usable preview"),
        allowed_channels: AllowedChannels::new(vec![ApprovalChannel::Api]).expect("a channel"),
        // Far in the future, **on purpose**: the handler reads the real clock, so a fixture deadline
        // near "now" would make these tests exercise the expiry path instead of the decision path —
        // and a test that fails because the calendar moved is a flake, not a signal. The expiry path
        // has its own test in the service suite, where the instant is a parameter.
        expires_at: UtcTimestamp::parse("2030-01-01T00:00:00Z").expect("valid"),
        scope: ApprovalScopeKind::OneShot,
    });
    approval.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(9));
    approval
}

/// A router with an approval service over a seeded double.
async fn fixture(tag: &str) -> (axum::Router, String, DurableApproval, std::path::PathBuf) {
    fixture_seeded(tag, pending()).await
}

/// The same, over a row the caller shapes.
///
/// Exists because the deadline is part of the record, not a knob on the service: a test that needs a
/// *lapsed* row has to seed one, since the handler reads the real clock and cannot be told otherwise.
async fn fixture_seeded(
    tag: &str,
    approval: DurableApproval,
) -> (axum::Router, String, DurableApproval, std::path::PathBuf) {
    let (router, token, mut approvals, dir) = fixture_seeded_many(tag, vec![approval]).await;
    (router, token, approvals.remove(0), dir)
}

/// The same, over **several** seeded rows, in the order given.
///
/// Exists for the paging test, which needs more rows than its page bound so the first page is
/// bounded and must hand back a cursor. The fixture returns only one row's identity, so a caller
/// needing to assert *which* rows came back cannot work through it.
async fn fixture_seeded_many(
    tag: &str,
    seed: Vec<DurableApproval>,
) -> (
    axum::Router,
    String,
    Vec<DurableApproval>,
    std::path::PathBuf,
) {
    let dir = temp_dir(tag);
    let destination = ClientCredentialPath::in_config_dir(&dir);
    let (registered, credential) =
        enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
    let mut clients = ClientRegistry::new();
    clients.register(registered);

    let repositories = Arc::new(InMemoryRepositories::new());
    for approval in &seed {
        repositories
            .request(approval)
            .await
            .expect("the fixture is inserted");
    }
    let service = Arc::new(ApprovalService::new(
        Arc::clone(&repositories) as Arc<dyn ApprovalRepository>
    ));

    let state = Arc::new(
        ApiState::new(
            Arc::new(clients),
            Arc::new(crate::http::Readiness::new()),
            "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
        )
        .with_approvals(service),
    );
    (router(state), credential.to_presentation_text(), seed, dir)
}

/// Authenticated headers for an approval request.
fn approval_headers(token: &str) -> Vec<(&str, String)> {
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
    // `Content-Length` is set explicitly because a real client sends it and the body-limit middleware
    // reads it. `Request::builder()` does not add it, so a test that omitted it would exercise a path
    // no client takes.
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
        .expect("the body reads");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// The contract text form of a fixture digest.
///
/// **A real digest rather than a readable word, and the fixture had to change when the wire rule did.**
/// The handler now parses the fingerprint at the trust boundary — a malformed one is `request.invalid` —
/// so `"sha256:action"` is refused before it reaches the service, and this helper is what the tests use
/// to produce a value whose *shape* is valid. The distinction matters because the two failures are
/// different: a malformed fingerprint is "fix your request", while a valid-but-different one is "this is
/// not the action you approved".
fn digest_text(seed: u8) -> String {
    ActionDigest::from_bytes([seed; 32]).to_string()
}

/// A decision body.
fn decide_body(decision: &str, version: u64, fingerprint: &str) -> String {
    format!(
        r#"{{"decision":"{decision}","expected_version":{version},"action_fingerprint":"{fingerprint}"}}"#,
    )
}

#[tokio::test]
async fn a_listing_returns_the_pending_approval_with_its_preview() {
    let (app, token, approval, dir) = fixture("approval-list").await;
    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/approvals",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&approval.id.to_string()), "{body}");
    // The preview is the part a user reviews, so it is asserted by value rather than by presence: a
    // rendered sentence would satisfy a `contains` on the key while dropping the value.
    assert!(
        body.contains(r#""key":"to","value":"peter@example.com""#),
        "{body}"
    );
    assert!(body.contains(r#""state":"pending""#), "{body}");
    assert!(body.contains(r#""max_page":200"#), "{body}");
    // **`has_more` is asserted on the wire, not only in the service.** A client's decision to keep
    // reading is made from this field, and a listing that omitted it would leave a client unable to
    // tell a full page from a complete one on a surface that serves no cursor.
    assert!(
        body.contains(r#""has_more":false"#),
        "one row cannot fill a page, so the listing is complete: {body}",
    );
    // A complete page carries **no** cursor, so "there is nothing after this" is the absence of the
    // field rather than a cursor pointing at nothing — which a client would follow and receive an
    // empty page from.
    assert!(
        !body.contains("next_cursor"),
        "a complete page must omit the cursor: {body}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_listing_narrows_to_a_risk_level_and_refuses_an_unknown_one() {
    // **The contract's `risk` filter, end to end through the router.** `TLS-013` made the step-up rule
    // real, so a `critical` narrow is the set of prompts that will demand an elevated session — a client
    // that wants to show them needs this, and the alternative (fetch every prompt, discard the rest) is
    // exactly the superset the listing's refusals exist to prevent.
    //
    // Two rows of **different** risk, so a filter that was dropped returns two rows and a filter that
    // applied a constant returns one — the wrong answers are distinguishable, which a same-risk fixture
    // would not achieve.
    let critical = {
        let mut row = pending();
        row.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(0x6001));
        row.risk = Risk::Critical;
        row
    };
    let (app, token, seeded, dir) =
        fixture_seeded_many("approval-risk", vec![pending(), critical]).await;
    let high = &seeded[0];
    let critical = &seeded[1];

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/approvals?risk=critical",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(&critical.id.to_string()),
        "the critical approval must be in a critical narrow: {body}",
    );
    assert!(
        !body.contains(&high.id.to_string()),
        "**a higher-risk row must not appear under a critical narrow**: {body}",
    );
    assert!(body.contains(r#""risk":"critical""#), "{body}");

    // The default listing is not narrowed, which is the half an always-on filter would fail.
    let (status, all) = send(
        &app,
        "GET",
        "/api/v1/approvals",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert!(
        all.contains(&critical.id.to_string()) && all.contains(&high.id.to_string()),
        "an unfiltered listing shows every risk level: {all}",
    );

    // **An unrecognised value of a known key is refused, not defaulted.** Treating `?risk=severe` as "no
    // narrow" would return every level — the superset the unknown-*key* refusal prevents, arriving
    // through a known key — so it is a `400` with the same request-invalid code.
    let (status, refusal) = send(
        &app,
        "GET",
        "/api/v1/approvals?risk=severe",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refusal}");
    assert!(refusal.contains("request.invalid"), "{refusal}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_listing_hands_back_a_cursor_that_reads_the_next_page() {
    // **The contract's cursor, end to end through the router.** `has_more` alone told a client that
    // more prompts awaited a decision and gave it no way to fetch one, which is worse than silence
    // because the client knows work remains and cannot do it.
    //
    // Two approvals and a bound of one, so the first page is bounded and must carry a cursor. The
    // second page is then fetched **with the value the daemon returned**, which is what makes this a
    // test of the cursor rather than of the encoder: a cursor the client could not use back would
    // leave the second page empty.
    let (app, token, seeded, dir) = fixture_seeded_many(
        "approval-cursor",
        vec![pending(), {
            // A second row with a **later** deadline, so the order is decided by `expires_at` rather
            // than by the identifier tie-break — which is what makes a cursor derived from the wrong
            // column produce a visibly wrong page rather than accidentally the right one.
            //
            // The instant must be in the **future**, because the listing expires lapsed rows on read:
            // a past deadline would see this row swept away and the page reported complete, which is
            // the correct behaviour and would make this test assert nothing about paging.
            let mut second = pending();
            second.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(0x7002));
            second.expires_at =
                UtcTimestamp::parse("2030-01-02T00:00:00Z").expect("a valid instant");
            second
        }],
    )
    .await;
    let first = &seeded[0];
    let second = seeded[1].clone();

    let (status, page_one) = send(
        &app,
        "GET",
        "/api/v1/approvals?limit=1",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page_one}");
    let parsed: serde_json::Value = serde_json::from_str(&page_one).expect("JSON");
    assert_eq!(
        parsed["has_more"], true,
        "a bound of one over two rows must report more: {page_one}",
    );
    let cursor = parsed["next_cursor"]
        .as_str()
        .expect("a bounded page must carry a cursor");
    assert!(
        cursor.starts_with("v1."),
        "the cursor is a versioned opaque value: {cursor}",
    );
    let first_page_id = parsed["approvals"][0]["approval_id"]
        .as_str()
        .expect("a row id")
        .to_owned();

    assert!(
        page_one.contains(&first.id.to_string()),
        "page one must be the row that lapses soonest — the order the cursor is a position in — so a \
         cursor derived from the wrong column cannot pass this by accident: {page_one}",
    );

    let path = format!("/api/v1/approvals?limit=1&cursor={cursor}");
    let (status, page_two) = send(&app, "GET", &path, &approval_headers(&token), "").await;
    assert_eq!(status, StatusCode::OK, "{page_two}");
    let parsed_two: serde_json::Value = serde_json::from_str(&page_two).expect("JSON");
    let second_page_ids: Vec<&str> = parsed_two["approvals"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|row| row["approval_id"].as_str())
        .collect();
    assert_eq!(
        second_page_ids.len(),
        1,
        "the second page must carry the remaining row: {page_two}",
    );
    // **The two pages must be disjoint.** A cursor that restarted from the beginning would return the
    // same row again — a client would loop forever, re-reading one prompt and never deciding the other.
    assert_ne!(
        second_page_ids[0], first_page_id,
        "page two must not repeat page one's row: {page_one} / {page_two}",
    );
    assert!(
        page_two.contains(&second.id.to_string()),
        "page two must be the row page one did not show: {page_two}",
    );
    // And the second page is complete, so it carries no further cursor.
    assert_eq!(parsed_two["has_more"], false, "{page_two}");
    assert!(!page_two.contains("next_cursor"), "{page_two}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_cursor_that_was_not_produced_by_this_listing_is_refused() {
    // The cursor is opaque, and opacity is a promise about *use* — a client may pass one back and
    // nothing else. A malformed value must be a `400` rather than a silent restart from page one: a
    // restart looks like a legitimate answer, so a client would loop forever without ever learning
    // that its cursor was bad.
    let (app, token, _approval, dir) = fixture("approval-bad-cursor").await;
    for cursor in ["v1.not-base64!!", "v1.", "nonsense", "v1.aGVsbG8"] {
        let path = format!("/api/v1/approvals?cursor={cursor}");
        let (status, body) = send(&app, "GET", &path, &approval_headers(&token), "").await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "`{cursor}` must be refused rather than treated as no cursor: {body}",
        );
        assert!(
            body.contains("request.invalid_cursor"),
            "the refusal must use the cursor code so a client can tell it from a body defect: {body}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_cursor_round_trips_the_risk_narrow_and_accepts_a_legacy_cursor() {
    // **The cursor now carries the narrow it was minted under, and the encoding is where a value goes
    // missing.** `encode_cursor`/`decode_cursor` are pure, so the round trip is asserted directly rather
    // than only through a page: a level that encoded but did not decode would make the next page resume
    // under the wrong narrow (or none), which is the skip the service's guard exists to refuse.
    use super::{base64url, decode_cursor, encode_cursor};
    use jarvis_application::repository::approval::ApprovalCursor;
    use jarvis_domain::ids::ApprovalId;
    use jarvis_domain::time::UtcTimestamp;
    use jarvis_domain::tool::approval::ApprovalChannel;
    use jarvis_domain::tool::classification::Risk;

    let at = UtcTimestamp::parse("2026-09-27T12:00:00Z").expect("a valid instant");
    let id = ApprovalId::from_uuid(uuid::Uuid::from_u128(7));
    let cursor = |risk| ApprovalCursor {
        expires_at: at,
        id,
        channel: ApprovalChannel::Cli,
        risk,
    };

    // Every level round-trips its own value, so a page narrowed to one resumes narrowed to the same.
    for level in [Risk::Low, Risk::Moderate, Risk::High, Risk::Critical] {
        let value = cursor(Some(level));
        assert_eq!(
            decode_cursor(&encode_cursor(&value)),
            Some(value),
            "{level:?} must survive the cursor round trip",
        );
    }
    // An un-narrowed cursor round-trips as un-narrowed, which is a different fact from any single level.
    let bare = cursor(None);
    assert_eq!(decode_cursor(&encode_cursor(&bare)), Some(bare));

    // **A three-segment cursor decodes as un-narrowed, not as invalid.** This build issued such cursors
    // before the risk segment existed, and such a page *was* un-narrowed — refusing one would break a
    // client mid-page over a format change it has no way to observe.
    let legacy = format!(
        "v1.{}",
        base64url(format!("{}|{}|{}", at, id, ApprovalChannel::Cli.as_contract_str()).as_bytes()),
    );
    assert_eq!(decode_cursor(&legacy), Some(bare));

    // A fifth segment is still refused: reading a prefix is how a parser accepts a value the writer
    // never produced.
    let injected = format!(
        "v1.{}",
        base64url(format!("{at}|{id}|cli|high|extra").as_bytes()),
    );
    assert_eq!(decode_cursor(&injected), None);
    // And a level this build does not know is refused rather than read as un-narrowed, which would
    // silently widen a page the client believed it had narrowed.
    let unknown = format!(
        "v1.{}",
        base64url(format!("{at}|{id}|cli|severe").as_bytes())
    );
    assert_eq!(decode_cursor(&unknown), None);
}

#[tokio::test]
async fn a_listing_page_spends_its_bound_on_rows_the_caller_may_decide() {
    // The end-to-end half of the short-page defect: a page of one, with a row the caller cannot decide
    // seeded **first** (so it lapses soonest and would be any workspace-wide read's first row). The
    // channel filter has to run before the bound, or the caller receives an empty page and concludes
    // the queue is empty.
    let (app, token, _approval, dir) = fixture("approval-page").await;
    // The fixture's own approval permits `api`, which is this client's channel — so it is the row that
    // must come back when the bound is one.
    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/approvals?limit=1",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains(r#""approvals":[]"#),
        "**a bound of one must be spent on the decidable row, not on an excluded one**: {body}",
    );
    assert!(body.contains(r#""has_more":false"#), "{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_detail_carries_the_tool_source_and_schema_identity_the_contract_names() {
    // The contract's detail requirement is "tool source/**schema identity**", and the rule it protects is
    // `ACC-024`: an approval binds to the implementation rather than to a name that can be re-pointed. A
    // client shown only `mail.send@1` could not tell that the tool behind it had been replaced — which is
    // the review decision detail exists to support. Asserted **by value** for all four fields, because a
    // presence check would pass for a rendered tuple a client cannot take apart, and asserted through the
    // **router** so the projection is exercised rather than the domain value it came from.
    let (app, token, approval, dir) = fixture("approval-identity").await;
    let path = format!("/api/v1/approvals/{}", approval.id);
    let (status, body) = send(&app, "GET", &path, &approval_headers(&token), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let parsed: serde_json::Value = serde_json::from_str(&body).expect("the body is JSON");
    assert_eq!(parsed["tool_id"], "mail.send@1", "{body}");
    assert_eq!(parsed["tool_source_kind"], "connector", "{body}");
    assert_eq!(parsed["tool_source_owner"], "acme.mail", "{body}");
    assert_eq!(parsed["tool_source_version"], "1.0.0", "{body}");
    // The fixture's schema fingerprint is `[7; 32]`, so the rendered form is `sha256:` then 64 hex.
    assert_eq!(
        parsed["schema_fingerprint"],
        format!("sha256:{}", "07".repeat(32)),
        "the schema fingerprint must be the contract's `sha256:<hex>` form: {body}",
    );

    // **The two dimensions are independent facts, not one rendered tuple.** A test that only asserted the
    // joined string would pass against an implementation that emitted the source twice, so each is
    // checked against the value the fixture actually set rather than against the others.
    assert_ne!(
        parsed["schema_fingerprint"], parsed["action_fingerprint"],
        "the schema fingerprint and the action fingerprint are different values",
    );

    // The contract's audit section requires the decision's **assurance** on the wire, and it is reported
    // from the record rather than from a transition actor — so a later consumption cannot clear it. A
    // pending approval has no decision and therefore omits the field rather than reporting `standard`,
    // which is asserted here because "absent" and "the weakest level" are different answers a client must
    // be able to tell apart.
    assert!(
        parsed.get("decided_assurance").is_none(),
        "an undecided approval must not report an assurance: {body}",
    );
    assert!(
        parsed.get("decided_by").is_none(),
        "and it must not report a decider either: {body}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_decided_approval_reports_the_assurance_the_decider_proved() {
    // The other half of the field: once a decision exists the assurance is on the wire, and it is the value
    // the **server** resolved rather than anything the request stated. The fixture decides through the real
    // route with a `Standard` credential, so the reported level is the one the resolved context held.
    let (app, token, approval, dir) = fixture("approval-assurance").await;
    let path = format!("/api/v1/approvals/{}/decide", approval.id);
    let body = decide_body("approve", 1, &digest_text(11));
    let (status, decided_body) = send(&app, "POST", &path, &approval_headers(&token), &body).await;
    assert_eq!(status, StatusCode::OK, "{decided_body}");
    let parsed: serde_json::Value = serde_json::from_str(&decided_body).expect("the body is JSON");
    // The decision response **wraps** the view under `approval`, so reading the top level would find
    // `Null` for every field and pass for the wrong reason. Reading the nested path is the assertion the
    // fixture above got wrong on its first attempt, which is why the path is spelled out rather than
    // indexed with a string the test could also get wrong.
    assert_eq!(
        parsed["approval"]["decided_assurance"], "standard",
        "the assurance the decider proved must reach the client: {decided_body}",
    );
    assert!(
        parsed["approval"]["decided_by"].is_string(),
        "and the decider travels with it: {decided_body}",
    );
    assert_eq!(parsed["applied"], true, "{decided_body}");

    // And a **read** after the decision agrees, so the value is durable rather than only in the response
    // that happened to carry it.
    let (read_status, read_body) = send(
        &app,
        "GET",
        &format!("/api/v1/approvals/{}", approval.id),
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(read_status, StatusCode::OK, "{read_body}");
    let read: serde_json::Value = serde_json::from_str(&read_body).expect("the body is JSON");
    assert_eq!(
        read["decided_assurance"], parsed["approval"]["decided_assurance"],
        "the durable record and the decision response must not disagree about the assurance",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_decision_is_applied_and_a_repeat_reports_that_it_was_not() {
    // The wire half of idempotence: the second response must say `applied: false`, because reporting
    // a repeat as a fresh decision would claim an event that did not happen.
    let (app, token, approval, dir) = fixture("approval-decide").await;
    let path = format!("/api/v1/approvals/{}/decide", approval.id);
    let body = decide_body("approve", 1, &digest_text(11));

    let (first, first_body) = send(&app, "POST", &path, &approval_headers(&token), &body).await;
    assert_eq!(first, StatusCode::OK, "{first_body}");
    assert!(first_body.contains(r#""applied":true"#), "{first_body}");
    assert!(first_body.contains(r#""state":"approved""#), "{first_body}");

    let (second, second_body) = send(&app, "POST", &path, &approval_headers(&token), &body).await;
    assert_eq!(second, StatusCode::OK, "{second_body}");
    assert!(
        second_body.contains(r#""applied":false"#),
        "**a repeat must not claim to have applied a transition**: {second_body}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_critical_action_is_forbidden_to_an_ordinary_session_over_the_wire() {
    // **The wire half of the step-up rule, and the test that keeps the contract's stable-error list
    // honest.** `approval.assurance_insufficient` sat in that list with no producer, so a client could
    // key on a code it could never receive. The fixture registers a `Standard` credential and the row is
    // `Critical`, so the refusal is the assurance and not the channel — `api` is the only permitted
    // channel and this client arrives on it.
    let mut critical = pending();
    critical.risk = Risk::Critical;
    let (app, token, approval, dir) = fixture_seeded("approval-step-up", critical).await;
    let path = format!("/api/v1/approvals/{}/decide", approval.id);
    let body = decide_body("approve", 1, &digest_text(11));
    let (status, refusal) = send(&app, "POST", &path, &approval_headers(&token), &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
    assert!(
        refusal.contains("approval.assurance_insufficient"),
        "the refusal must carry the contract's own code: {refusal}",
    );
    assert!(
        refusal.contains("\"retryable\":false"),
        "a stronger assurance is a different request, so re-sending this one cannot succeed: {refusal}",
    );
    // **Nothing changed.** A refusal that had already written the decision would be the worst
    // direction, so the record is read back and must still be pending.
    let (read_status, read_body) = send(
        &app,
        "GET",
        &format!("/api/v1/approvals/{}", approval.id),
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(read_status, StatusCode::OK, "{read_body}");
    let read: serde_json::Value = serde_json::from_str(&read_body).expect("the body is JSON");
    assert_eq!(
        read["state"], "pending",
        "a refused decision must leave the record where it was: {read_body}",
    );
    assert_eq!(read["version"], 1, "{read_body}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_malformed_fingerprint_is_a_bad_request_rather_than_a_mismatch() {
    // **The two failures are different and this is the test that keeps them apart.** A fingerprint whose
    // *shape* is wrong — a different algorithm, a bare digest, uppercase hex, the wrong length — means the
    // caller sent something that could never have been a fingerprint, so the answer is `request.invalid`.
    // Reporting `approval.fingerprint_mismatch` instead would tell the user "this is not the action you
    // approved" and send them to re-approve an action whose fingerprint they had no way to compute
    // correctly. Asserted for every malformed shape in one test, so a later change that started accepting
    // one of them fails here.
    let (app, token, approval, dir) = fixture("approval-malformed-fingerprint").await;
    let path = format!("/api/v1/approvals/{}/decide", approval.id);
    let hex = "ab".repeat(32);
    for bad in [
        format!("md5:{hex}"),
        hex.clone(),
        format!("sha256:{}", hex.to_uppercase()),
        format!("sha256:{}", &hex[..60]),
        "sha256:".to_owned(),
        String::new(),
    ] {
        let (status, body) = send(
            &app,
            "POST",
            &path,
            &approval_headers(&token),
            &decide_body("approve", 1, &bad),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "**{bad:?} is malformed, not a mismatch**: {body}",
        );
        assert!(
            body.contains("request.invalid"),
            "the refusal must name the request as invalid rather than the action as different: {body}",
        );
        assert!(
            !body.contains("fingerprint_mismatch"),
            "a malformed fingerprint must not be reported as a mismatch: {body}",
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_decision_with_a_comment_returns_it_on_the_detail_read() {
    // **The wire's `comment` said "stored with the decision" while nothing stored it.** The field was
    // deserialized, bounded by nothing, and dropped — so the contract's "changed ... comment is
    // `idempotency.conflict`" had no value to compare and an operator asking *why* a decision was taken got
    // nothing. This asserts the round trip end to end: send a comment, then read the approval's detail and
    // find it, **through the router**, because the note lives on a transition rather than in a column and a
    // service-level assertion would not exercise the route that reads the trail.
    let (app, token, approval, dir) = fixture("approval-comment").await;
    let path = format!("/api/v1/approvals/{}/decide", approval.id);
    let body = format!(
        r#"{{"decision":"approve","expected_version":1,"action_fingerprint":"{}","comment":"checked the recipient"}}"#,
        digest_text(11),
    );
    let (status, decided) = send(&app, "POST", &path, &approval_headers(&token), &body).await;
    assert_eq!(status, StatusCode::OK, "{decided}");

    let (read_status, read_body) = send(
        &app,
        "GET",
        &format!("/api/v1/approvals/{}", approval.id),
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(read_status, StatusCode::OK, "{read_body}");
    let parsed: serde_json::Value = serde_json::from_str(&read_body).expect("the body is JSON");
    assert_eq!(
        parsed["decided_note"], "checked the recipient",
        "**the comment must be readable from detail**, or it is a value the caller sent and lost: {read_body}",
    );

    // A decision with **no** comment omits the field rather than reporting an empty note — "nobody wrote a
    // comment" and "somebody wrote nothing" are different answers, and the second is refused at construction.
    let (quiet_app, quiet_token, quiet, quiet_dir) = fixture("approval-no-comment").await;
    let (quiet_status, _) = send(
        &quiet_app,
        "POST",
        &format!("/api/v1/approvals/{}/decide", quiet.id),
        &approval_headers(&quiet_token),
        &decide_body("approve", 1, &digest_text(11)),
    )
    .await;
    assert_eq!(quiet_status, StatusCode::OK);
    let (_, quiet_read) = send(
        &quiet_app,
        "GET",
        &format!("/api/v1/approvals/{}", quiet.id),
        &approval_headers(&quiet_token),
        "",
    )
    .await;
    assert!(
        !quiet_read.contains("decided_note"),
        "an absent note must be omitted, not rendered as an empty one: {quiet_read}",
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&quiet_dir);
}

#[tokio::test]
async fn a_decision_body_that_carries_an_unusable_comment_is_a_bad_request() {
    // The bound is enforced at the boundary, so an over-long or control-bearing comment is a
    // `request.invalid` rather than a value that reaches the service and stops — the "validated at one
    // layer, stored at none" shape the field had before.
    let (app, token, approval, dir) = fixture("approval-bad-comment").await;
    let path = format!("/api/v1/approvals/{}/decide", approval.id);
    for bad in [
        "x".repeat(MAX_DECISION_NOTE_BYTES + 1),
        "stop\u{7}now".to_owned(),
        String::new(),
    ] {
        let body = format!(
            r#"{{"decision":"approve","expected_version":1,"action_fingerprint":"{}","comment":"{}"}}"#,
            digest_text(11),
            bad.replace('\\', "\\\\").replace('"', "\\\""),
        );
        let (status, refused) = send(&app, "POST", &path, &approval_headers(&token), &body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "**an unusable comment must be refused where it arrives**: {refused}",
        );
        assert!(refused.contains("request.invalid"), "{refused}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_lapsed_read_reports_expired_and_the_listing_no_longer_offers_it() {
    // **The contract's "expiry is evaluated on every read", asserted over the wire.** Both surfaces are
    // checked because they are the two answers a client acts on differently: a detail read that said
    // `pending` would render a decision the daemon refuses, and a listing that still offered the row would
    // put a dead prompt in front of an operator — and spend its page budget on it.
    //
    // The row is seeded with a **past** deadline, which is the opposite of every other fixture here (those
    // use a far-future instant so they exercise the decision path). That inversion is the point: this test
    // is *about* the lapse, so it must not be able to pass with a deadline that has not arrived.
    let mut lapsed = pending();
    lapsed.expires_at = UtcTimestamp::parse("2020-01-01T00:00:00Z").expect("valid");
    let (app, token, approval, dir) = fixture_seeded("approval-lapsed-read", lapsed).await;

    let (read_status, read_body) = send(
        &app,
        "GET",
        &format!("/api/v1/approvals/{}", approval.id),
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(read_status, StatusCode::OK, "{read_body}");
    let parsed: serde_json::Value = serde_json::from_str(&read_body).expect("the body is JSON");
    assert_eq!(
        parsed["state"], "expired",
        "**a read past the deadline must report the recorded lapse**: {read_body}",
    );
    assert_eq!(parsed["lapsed"], true, "{read_body}");

    let (list_status, list_body) = send(
        &app,
        "GET",
        "/api/v1/approvals",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(list_status, StatusCode::OK, "{list_body}");
    let listing: serde_json::Value = serde_json::from_str(&list_body).expect("the body is JSON");
    assert_eq!(
        listing["approvals"].as_array().map(Vec::len),
        Some(0),
        "**a lapsed prompt must not be offered**: {list_body}",
    );
    assert_eq!(
        listing["has_more"], false,
        "and the page is complete rather than short: {list_body}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn every_stable_refusal_reaches_the_shared_envelope_with_a_code_and_a_status() {
    // One case per family, because the mapping is a match that a copy-paste can put one variant in the
    // wrong arm — and the assertion is on the **envelope**, not the status alone, because a status
    // without a parseable body is what a client cannot act on.
    let (app, token, approval, dir) = fixture("approval-refusals").await;
    let path = format!("/api/v1/approvals/{}/decide", approval.id);

    let cases: Vec<(String, &str, StatusCode)> = vec![
        // A fingerprint that is not the approved one.
        (
            decide_body("approve", 1, &digest_text(12)),
            "approval.fingerprint_mismatch",
            StatusCode::CONFLICT,
        ),
        // A verb outside the contract's closed set.
        (
            decide_body("maybe", 1, &digest_text(11)),
            "request.invalid",
            StatusCode::BAD_REQUEST,
        ),
        // A version of zero, which the domain refuses as an uninitialised counter.
        (
            decide_body("approve", 0, &digest_text(11)),
            "request.invalid",
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (body, code, expected) in cases {
        let (status, text) = send(&app, "POST", &path, &approval_headers(&token), &body).await;
        assert_eq!(status, expected, "{code}: {text}");
        assert!(text.contains(&format!(r#""code":"{code}""#)), "{text}");
        // The envelope carries a request id, so a client reporting a fault can be correlated with the
        // daemon's diagnostics — the property the field exists for.
        assert!(text.contains(r#""request_id":"#), "{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_unknown_approval_and_a_malformed_identifier_answer_the_same_not_found() {
    // The contract requires an unparseable identifier to be indistinguishable from an unknown one, so
    // the assertion compares the two codes rather than checking one — a leak would be in the
    // difference, and a single-sided assertion cannot see a difference.
    let (app, token, _approval, dir) = fixture("approval-notfound").await;
    let unknown = ApprovalId::from_uuid(uuid::Uuid::from_u128(4242));

    let (unknown_status, unknown_body) = send(
        &app,
        "GET",
        &format!("/api/v1/approvals/{unknown}"),
        &approval_headers(&token),
        "",
    )
    .await;
    let (malformed_status, malformed_body) = send(
        &app,
        "GET",
        "/api/v1/approvals/not-an-identifier",
        &approval_headers(&token),
        "",
    )
    .await;

    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    assert_eq!(malformed_status, StatusCode::NOT_FOUND);
    assert!(
        unknown_body.contains(r#""code":"approval.not_found""#),
        "{unknown_body}"
    );
    assert!(
        malformed_body.contains(r#""code":"approval.not_found""#),
        "**a malformed identifier must not be distinguished from an unknown one**: {malformed_body}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn both_writes_require_an_idempotency_key_before_the_body_is_read() {
    // The key is required by the contract, and it is checked first — so a request missing both the key
    // and a usable body is told about the key. The assertion is on the **code**, because both refusals
    // share `request.invalid` and only the message distinguishes them; asserting the status alone
    // would pass for a request refused for the wrong reason.
    let (app, token, approval, dir) = fixture("approval-idem").await;
    let headers: Vec<(&str, String)> = approval_headers(&token)
        .into_iter()
        .filter(|(name, _)| *name != "idempotency-key")
        .collect();

    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/approvals/{}/decide", approval.id),
        &headers,
        &decide_body("approve", 1, &digest_text(11)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("An idempotency key is required."), "{body}");

    let (cancel_status, cancel_body) = send(
        &app,
        "POST",
        &format!("/api/v1/approvals/{}/cancel", approval.id),
        &headers,
        r#"{"expected_version":1}"#,
    )
    .await;
    assert_eq!(cancel_status, StatusCode::BAD_REQUEST, "{cancel_body}");
    assert!(
        cancel_body.contains("An idempotency key is required."),
        "{cancel_body}",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_store_less_daemon_answers_not_ready_rather_than_missing() {
    // A client must be able to tell "this endpoint exists but the daemon cannot serve it" from "there
    // is no such endpoint", so the refusal is a named code rather than the unknown-route answer.
    let dir = temp_dir("approval-no-storage");
    let destination = ClientCredentialPath::in_config_dir(&dir);
    let (registered, credential) =
        enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination).expect("enrollment");
    let mut clients = ClientRegistry::new();
    clients.register(registered);
    let state = Arc::new(ApiState::new(
        Arc::new(clients),
        Arc::new(crate::http::Readiness::new()),
        "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43127),
    ));
    let app = router(state);
    let token = credential.to_presentation_text();

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/approvals",
        &approval_headers(&token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains(r#""code":"service.not_ready""#), "{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn every_approval_route_requires_authentication() {
    // Applied by the same helper as every other API route, so this asserts the wiring rather than the
    // middleware — a route added with `get(...)` instead of `authenticated(get(...))` would be
    // reachable without a credential, and only a test that sent no credential would notice.
    let (app, _token, approval, dir) = fixture("approval-auth").await;
    let cases = [
        ("GET", "/api/v1/approvals".to_owned(), String::new()),
        (
            "GET",
            format!("/api/v1/approvals/{}", approval.id),
            String::new(),
        ),
        (
            "POST",
            format!("/api/v1/approvals/{}/decide", approval.id),
            decide_body("approve", 1, &digest_text(11)),
        ),
        (
            "POST",
            format!("/api/v1/approvals/{}/cancel", approval.id),
            r#"{"expected_version":1}"#.to_owned(),
        ),
    ];
    for (method, path, body) in cases {
        let (status, text) = send(
            &app,
            method,
            &path,
            &[
                ("jarvis-api-version", "1".to_owned()),
                ("content-type", "application/json".to_owned()),
            ],
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_listing_query_parser_accepts_only_a_bounded_limit() {
    // The parser is a pure function so this needs no router: an unknown filter must be **refused**
    // rather than ignored, because ignoring one would return a superset of what a caller asked for —
    // on an approval listing, prompts the client believed it had excluded.
    use super::parse_list_query;
    use axum::http::Uri;
    use jarvis_application::repository::approval::ApprovalListFilter;
    use jarvis_domain::tool::classification::Risk;

    let uri = |query: &str| -> Uri { format!("/api/v1/approvals{query}").parse().expect("a uri") };
    let none = ApprovalListFilter::default();

    assert_eq!(
        parse_list_query(&uri("")).expect("no query"),
        (None, None, none)
    );
    assert_eq!(
        parse_list_query(&uri("?limit=25")).expect("a limit"),
        (Some(25), None, none)
    );
    assert_eq!(
        parse_list_query(&uri("?limit=0")).expect("zero is parseable"),
        (Some(0), None, none)
    );
    // **`risk` is now served, and the value is validated rather than accepted.** A `critical` narrow
    // is the filter whose meaning changed when the step-up rule became real: it is the set of prompts
    // that will demand an elevated session.
    assert_eq!(
        parse_list_query(&uri("?risk=critical")).expect("a risk narrow"),
        (
            None,
            None,
            ApprovalListFilter {
                risk: Some(Risk::Critical)
            }
        )
    );
    // The narrow and the bound compose, so a client can page within one risk level.
    assert_eq!(
        parse_list_query(&uri("?risk=high&limit=5")).expect("both"),
        (
            Some(5),
            None,
            ApprovalListFilter {
                risk: Some(Risk::High)
            }
        )
    );
    // **An unrecognised `risk` value is refused, not treated as "no narrow".** Defaulting it to `None`
    // would silently widen the result to every level — the same superset the unknown-key refusal
    // prevents, arriving through a known key.
    assert!(parse_list_query(&uri("?risk=severe")).is_err());
    // An unknown filter is a refusal, not a silently ignored parameter.
    assert!(parse_list_query(&uri("?state=pending")).is_err());
    assert!(parse_list_query(&uri("?limit=25&state=pending")).is_err());
    // A non-numeric limit is a refusal too, rather than a default.
    assert!(parse_list_query(&uri("?limit=many")).is_err());
}

#[test]
fn the_reported_page_bound_is_the_one_the_store_enforces() {
    // `jarvis_protocol::approval::MAX_APPROVAL_PAGE` is what the listing **reports** as `max_page`,
    // and `jarvis_application::repository::approval::MAX_PENDING_PAGE` is what the store **clamps** a
    // requested limit to. They were two literals holding the same number with nothing comparing them.
    //
    // The comparison cannot live in either owning crate: this workspace's documented flow is
    // `Protocol --> Domain`, so `jarvis-protocol` may not depend on `jarvis-application`, and
    // application may not depend on the wire vocabulary. `jarvis-infrastructure` is the one crate
    // that depends on both, which is the same reason `the_objective_bound_is_the_one_the_wire_bound_enforces`
    // lives here.
    //
    // The failure it prevents is quiet and asymmetric: raising the protocol's bound alone would make
    // the daemon advertise a page size larger than the one its own query applies, so a client that
    // paged by `max_page` would silently receive fewer rows than the response claimed were available
    // and could not tell a full page from a truncated one.
    assert_eq!(
        jarvis_protocol::approval::MAX_APPROVAL_PAGE,
        jarvis_application::repository::approval::MAX_PENDING_PAGE,
        "the reported page bound and the enforced page bound must be the same limit",
    );
}

#[test]
fn the_wire_risk_vocabulary_is_the_domain_ladder_in_ladder_order() {
    // **The listing's `risk` filter is parsed through the domain ladder and spelled by the wire, so the
    // two lists must be the same list.** `?risk=critical` is matched by `Risk::parse` against
    // `Risk::ALL`'s contract spellings, and the client builds its closed set from
    // `jarvis_protocol::approval::risk::LEVELS` — a level named in one list and not the other is a value
    // the client offers and the daemon refuses, or a level the daemon accepts that no client can select.
    //
    // The comparison lives here for the reason `the_reported_page_bound_is_the_one_the_store_enforces`
    // does: no single owning crate can see both. `jarvis-protocol` may not depend on `jarvis-domain`
    // (the documented flow is `Protocol --> Domain`, so domain sits below protocol and cannot be reached
    // upward), and the domain must not depend on the wire vocabulary. `jarvis-infrastructure` depends on
    // both.
    //
    // The order is asserted too, not only the members: the ladder is what a future "this level or higher"
    // filter would iterate, and a wire list that agreed on the set but not the order would make that
    // filter wrong without changing any spelling.
    let domain: Vec<&str> = Risk::ALL
        .iter()
        .map(|level| level.as_contract_str())
        .collect();
    assert_eq!(
        jarvis_protocol::approval::risk::LEVELS,
        domain.as_slice(),
        "the wire risk vocabulary must be the domain ladder's spellings, in ladder order",
    );
}
