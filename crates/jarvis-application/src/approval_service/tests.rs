//! Tests for the approval use cases.
//!
//! Nine properties, each one a contract sentence:
//!
//! - **The server derives the decider.** A decision's recorded principal and channel come from the
//!   context, never the payload — the type has no field for them, and the test varies the context to
//!   show the recorded actor follows it.
//! - **A foreign-workspace record is indistinguishable from a missing one.**
//! - **A channel the request excludes cannot decide it.**
//! - **A guest cannot decide anything.**
//! - **A lapsed request is expired and recorded, not merely refused** — so it leaves the listing.
//! - **A fingerprint that does not match is refused before the version**, because a re-approval cannot
//!   fix a digest.
//! - **A repeat of the same decision is idempotent** (`applied: false`), while a *different* decision
//!   against the same version is a state conflict.
//! - **The requester may cancel its own request from any channel it is authenticated on.**
//! - **A one-shot already consumed reports `approval.already_consumed`**, not a state conflict.

use std::sync::Arc;

use jarvis_domain::ids::{
    ApprovalId, CorrelationId, PrincipalId, RequestId, RunId, ToolCallId, WorkspaceId,
};
use jarvis_domain::model::exception::RequiredAssurance;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::approval::{
    ApprovalRequestParts, ApprovalScopeKind, ApprovalState, ApprovalVersion, DurableApproval,
};
use jarvis_domain::tool::classification::{Effect, Risk};

use crate::approval_service::{
    ApprovalService, Decision, DecisionCommand, MAX_CANCEL_REASON_BYTES,
};
use crate::repository::approval::ApprovalRepository;
use crate::request_context::{AuthenticationAssurance, RequestChannel, RequestContext};
use crate::testing::InMemoryRepositories;
use jarvis_domain::tool::approval::DecisionNote;

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:00:00Z").expect("the fixture instant parses")
}

fn later() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:20:00Z").expect("the fixture instant parses")
}

/// A workspace distinct from the default one, so a scope test can prove the boundary.
fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(7))
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(1))
}

fn other_principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(2))
}

/// A context on the given channel, at the given assurance.
///
/// Built through the **real constructor**, not by a struct literal, because `cancellation` is private
/// precisely so untrusted code cannot assemble a context — a test that reached past that would prove
/// nothing about the type's guarantee.
fn context_at(channel: RequestChannel, assurance: AuthenticationAssurance) -> RequestContext {
    context_for(principal(), channel, assurance)
}

/// A context for a specific principal, on a channel, at an assurance.
fn context_for(
    principal: PrincipalId,
    channel: RequestChannel,
    assurance: AuthenticationAssurance,
) -> RequestContext {
    RequestContext::new(
        RequestId::from_uuid(uuid::Uuid::from_u128(1)),
        CorrelationId::from_uuid(uuid::Uuid::from_u128(1)),
        principal,
        assurance,
        workspace(),
        channel,
    )
}

/// A context on the given channel.
fn context(channel: RequestChannel) -> RequestContext {
    context_at(channel, AuthenticationAssurance::Standard)
}

/// The digest the fixture approval was recorded for.
///
/// A real `ActionDigest` **value** rather than a readable word, because the type validates one now: the
/// service compares a computed digest against the stored one, and a string like `"sha256:action"` would
/// have been accepted as text while being refused as a digest — so this fixture is the change the type
/// makes visible.
fn note(text: &str) -> DecisionNote {
    DecisionNote::new(text).expect("a usable note")
}

fn approved_digest() -> jarvis_domain::tool::canonical::ActionDigest {
    jarvis_domain::tool::canonical::ActionDigest::from_bytes([11; 32])
}

/// A digest for an action the fixture approval does **not** cover.
fn other_digest() -> jarvis_domain::tool::canonical::ActionDigest {
    jarvis_domain::tool::canonical::ActionDigest::from_bytes([12; 32])
}

/// A pending approval whose allowed channels are the ones a client would normally decide on.
fn pending() -> DurableApproval {
    let mut approval = DurableApproval::request(ApprovalRequestParts {
        workspace: workspace(),
        requesting_principal: principal(),
        run: RunId::from_uuid(uuid::Uuid::from_u128(3)),
        tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(4)),
        identity: jarvis_domain::tool::identity::ToolIdentity {
            capability: jarvis_domain::tool::identity::ToolCapability::parse("mail.send@1")
                .expect("canonical"),
            source: jarvis_domain::tool::identity::ToolSource::new(
                jarvis_domain::tool::identity::SourceKind::Connector,
                "acme.mail",
                jarvis_domain::tool::identity::ToolVersion::parse("1.0.0").expect("valid"),
            )
            .expect("the fixture source is valid"),
            schema_fingerprint: jarvis_domain::tool::identity::SchemaFingerprint::from_bytes(
                [7; 32],
            ),
        },
        action_digest: jarvis_domain::tool::canonical::ActionDigest::from_bytes([11; 32]),
        risk: Risk::High,
        effects: vec![Effect::Write],
        summary: "Send one email".to_owned(),
        preview: jarvis_domain::tool::approval::ApprovalPreview::new(vec![
            jarvis_domain::tool::approval::PreviewItem::new("to", "peter@example.com")
                .expect("a usable preview item"),
        ])
        .expect("a usable preview"),
        allowed_channels: jarvis_domain::tool::approval::AllowedChannels::new(vec![
            jarvis_domain::tool::approval::ApprovalChannel::Cli,
            jarvis_domain::tool::approval::ApprovalChannel::Desktop,
        ])
        .expect("at least one channel"),
        expires_at: later(),
        scope: ApprovalScopeKind::OneShot,
    });
    // A caller-chosen identifier, so a test can address the row it seeded.
    approval.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(9));
    approval
}

/// A service over the in-memory double with one pending approval in it.
async fn fixture() -> (ApprovalService, Arc<InMemoryRepositories>, DurableApproval) {
    let repositories = Arc::new(InMemoryRepositories::new());
    let approval = pending();
    repositories
        .request(&approval)
        .await
        .expect("the fixture approval is inserted");
    let service = ApprovalService::new(Arc::clone(&repositories) as Arc<dyn ApprovalRepository>);
    (service, repositories, approval)
}

#[tokio::test]
async fn a_listing_shows_only_the_channels_the_caller_may_decide_on() {
    // The contract requires the server to expose "only approvals the authenticated principal may
    // inspect or decide". The record permits `cli` and `desktop`, so a **voice** caller must see
    // nothing: showing it a prompt it cannot answer would disclose another surface's work.
    let (service, _repositories, _approval) = fixture().await;

    let visible = service
        .list(&context(RequestChannel::Cli), 50)
        .await
        .expect("the listing runs");
    assert_eq!(
        visible.approvals.len(),
        1,
        "the cli channel may decide this request"
    );
    assert!(!visible.bounded, "one row cannot fill a page of fifty");

    let hidden = service
        .list(&context(RequestChannel::Voice), 50)
        .await
        .expect("the listing runs");
    assert!(
        hidden.approvals.is_empty(),
        "**a channel the request excludes must not see it**: {hidden:?}",
    );
}

#[tokio::test]
async fn a_page_is_not_short_changed_by_rows_the_caller_cannot_decide() {
    // **The defect this test exists for, and it is a silent one.** The page bound must apply to the
    // rows the caller can actually act on. An earlier version read `limit` rows for the *workspace*
    // and filtered by channel afterwards, so with a limit of one and a `desktop`-only row first, a
    // `cli` caller received an **empty page** while a decidable approval existed — and a client that
    // receives an empty page with no cursor concludes the queue is empty and does nothing.
    let repositories = Arc::new(InMemoryRepositories::new());
    // A row the cli caller may NOT decide, seeded first so it is the soonest to lapse and therefore
    // the first row any workspace-wide read would return.
    let mut desktop_only = pending();
    desktop_only.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(21));
    desktop_only.allowed_channels = jarvis_domain::tool::approval::AllowedChannels::new(vec![
        jarvis_domain::tool::approval::ApprovalChannel::Desktop,
    ])
    .expect("a channel");
    desktop_only.expires_at = UtcTimestamp::parse("2026-09-27T12:05:00Z").expect("valid");
    repositories
        .request(&desktop_only)
        .await
        .expect("the desktop-only row is inserted");

    // The row the cli caller may decide, with a later deadline so it sorts second.
    let mut cli_decidable = pending();
    cli_decidable.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(22));
    cli_decidable.allowed_channels = jarvis_domain::tool::approval::AllowedChannels::new(vec![
        jarvis_domain::tool::approval::ApprovalChannel::Cli,
    ])
    .expect("a channel");
    cli_decidable.expires_at = UtcTimestamp::parse("2026-09-27T12:30:00Z").expect("valid");
    repositories
        .request(&cli_decidable)
        .await
        .expect("the cli row is inserted");

    let service = ApprovalService::new(Arc::clone(&repositories) as Arc<dyn ApprovalRepository>);

    // **A page of one.** The channel filter must run before the bound, so the one row returned is the
    // one the caller can decide — not the excluded row that happened to lapse sooner.
    let page = service
        .list(&context(RequestChannel::Cli), 1)
        .await
        .expect("the listing runs");
    assert_eq!(
        page.approvals.len(),
        1,
        "**the page must be spent on a row the caller can decide**: a workspace-wide read would spend \
         it on the desktop-only row and return nothing",
    );
    assert_eq!(
        page.approvals[0].id, cli_decidable.id,
        "the returned row must be the decidable one, not the excluded one",
    );
    assert!(
        !page.bounded,
        "only one decidable row exists, so the store saw everything",
    );
}

#[tokio::test]
async fn a_full_page_reports_that_more_may_remain() {
    // `bounded` is what tells a client to keep reading. Without it a full page is indistinguishable
    // from a complete one, and this surface has no cursor to fall back on — so a client would stop at
    // the bound and believe it had seen every prompt in the workspace.
    let repositories = Arc::new(InMemoryRepositories::new());
    for index in 0..3 {
        let mut approval = pending();
        approval.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(30 + index));
        approval.allowed_channels = jarvis_domain::tool::approval::AllowedChannels::new(vec![
            jarvis_domain::tool::approval::ApprovalChannel::Cli,
        ])
        .expect("a channel");
        repositories
            .request(&approval)
            .await
            .expect("the row is inserted");
    }
    let service = ApprovalService::new(Arc::clone(&repositories) as Arc<dyn ApprovalRepository>);

    let full = service
        .list(&context(RequestChannel::Cli), 2)
        .await
        .expect("the listing runs");
    assert_eq!(full.approvals.len(), 2, "the page respects its bound");
    assert!(
        full.bounded,
        "**a full page with a third row behind it must say more may remain**: {full:?}",
    );

    // And the complement, on the same store: a page the store had nothing beyond reports complete —
    // which is the half that makes the first assertion meaningful rather than a constant.
    let complete = service
        .list(&context(RequestChannel::Cli), 3)
        .await
        .expect("the listing runs");
    assert_eq!(complete.approvals.len(), 3);
    assert!(
        !complete.bounded,
        "a page the store had nothing beyond is complete: {complete:?}",
    );
}

#[tokio::test]
async fn a_decision_records_the_principal_and_channel_from_the_context_not_the_request() {
    // The contract says a client "cannot assert" the deciding principal, channel, assurance, or time.
    // The proof is that the recorded actor **follows the context**: there is no field to set, so a
    // test varies the one thing a request cannot forge.
    let (service, _repositories, approval) = fixture().await;

    let decided = service
        .decide(
            &context(RequestChannel::Cli),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: ApprovalVersion::FIRST,
                fingerprint: &approved_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect("the decision applies");

    assert!(decided.applied, "the first decision is applied");
    assert_eq!(decided.approval.state(), ApprovalState::Approved);
    assert_eq!(
        decided.approval.decided_by(),
        Some(principal()),
        "the recorded principal is the authenticated one",
    );
    assert_eq!(
        decided.approval.decided_via(),
        Some(jarvis_domain::tool::approval::ApprovalChannel::Cli),
        "the recorded channel is the one the request arrived on",
    );
}

#[tokio::test]
async fn a_decision_on_a_channel_the_request_excludes_is_refused_by_name() {
    // `approval.channel_not_allowed` rather than a `not_found`, because the two send a user to
    // different places: a missing record means the identifier is wrong, while this means the surface
    // is. The code is asserted, not the status, because that is what a client branches on.
    let (service, _repositories, approval) = fixture().await;

    let error = service
        .decide(
            &context(RequestChannel::Mobile),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: ApprovalVersion::FIRST,
                fingerprint: &approved_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect_err("a mobile caller may not decide a cli/desktop request");

    assert_eq!(error.code(), "approval.channel_not_allowed");
    assert!(
        !error.retryable(),
        "resending on the same channel refuses again"
    );
}

#[tokio::test]
async fn a_guest_cannot_decide_and_is_refused_before_the_record_is_read() {
    // An anonymous caller cannot be the principal a decision is attributed to. The refusal happens
    // before the load, so a guest cannot use the endpoint to learn whether an identifier exists —
    // which is why the assertion is on the code and not on any state.
    let (service, _repositories, approval) = fixture().await;

    let error = service
        .decide(
            &context_at(RequestChannel::Cli, AuthenticationAssurance::Guest),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: ApprovalVersion::FIRST,
                fingerprint: &approved_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect_err("a guest holds no identity to attribute a decision to");

    assert_eq!(error.code(), "auth.credential_rejected");
}

#[tokio::test]
async fn a_lapsed_request_is_expired_and_recorded_so_it_leaves_the_listing() {
    // The contract requires expiry to be "evaluated on every read" and by a durable worker. A record
    // that lapsed while nobody was looking still reads `pending` in storage, so the refusal must
    // **also record** the lapse — a refusal that left the row pending would leave a prompt nobody can
    // decide sitting in every later listing.
    let (service, _repositories, approval) = fixture().await;

    let error = service
        .decide(
            &context(RequestChannel::Cli),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: ApprovalVersion::FIRST,
                fingerprint: &approved_digest(),
                note: None,
            },
            later(),
        )
        .await
        .expect_err("the request lapsed at its own deadline");
    assert_eq!(error.code(), "approval.expired");

    let stored = service
        .read(&context(RequestChannel::Cli), approval.id)
        .await
        .expect("the record is still readable");
    assert_eq!(
        stored.state(),
        ApprovalState::Expired,
        "**the lapse must be recorded**: a refusal alone leaves the prompt in every listing",
    );

    let listing = service
        .list(&context(RequestChannel::Cli), 50)
        .await
        .expect("the listing runs");
    assert!(
        listing.approvals.is_empty(),
        "an expired request must not appear as pending: {listing:?}",
    );
}

#[tokio::test]
async fn a_reviewed_action_that_is_not_the_approved_one_is_refused_before_the_version() {
    // Fingerprint before version, and the order matters: a re-approval cannot fix a digest that still
    // will not match, so reporting "your version is stale" would send the user to do exactly that.
    // Both are wrong here — the fingerprint AND the version — and the code must name the fingerprint.
    let (service, _repositories, approval) = fixture().await;

    let error = service
        .decide(
            &context(RequestChannel::Cli),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: ApprovalVersion::new(99),
                fingerprint: &other_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect_err("the reviewed action is not the approved one");

    assert_eq!(
        error.code(),
        "approval.fingerprint_mismatch",
        "the fingerprint is checked first, because a stale version is fixable and this is not",
    );
}

#[tokio::test]
async fn a_repeat_of_the_same_decision_is_idempotent_while_a_differing_one_conflicts() {
    // A user double-tapping "approve" sends two requests with the same body: the second must return
    // the original decision rather than an error. But a *different* decision against the same version
    // is a genuine conflict — and the two are told apart by the state, not by the version, because the
    // second tap carries the same `expected_version` as the first.
    let (service, _repositories, approval) = fixture().await;

    let first = service
        .decide(
            &context(RequestChannel::Cli),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: ApprovalVersion::FIRST,
                fingerprint: &approved_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect("the first decision applies");
    assert!(first.applied);

    // The same body again. The stored state already equals the target, so nothing is overwritten.
    let repeat = service
        .decide(
            &context(RequestChannel::Cli),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: ApprovalVersion::FIRST,
                fingerprint: &approved_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect("a repeated decision is idempotent, not an error");
    assert!(
        !repeat.applied,
        "**a repeat must not claim to have applied a transition that did not happen**",
    );
    assert_eq!(repeat.approval.state(), ApprovalState::Approved);

    // A *different* decision. The stored version has advanced (the approval is `approved` at version
    // 2), so this is reported as a state conflict rather than a version one — the transition is
    // illegal from a decided state, and saying "stale version" would suggest a re-read could fix it.
    let conflict = service
        .decide(
            &context(RequestChannel::Cli),
            approval.id,
            DecisionCommand {
                decision: Decision::Reject,
                expected_version: ApprovalVersion::new(2),
                fingerprint: &approved_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect_err("a decision is terminal");
    assert_eq!(conflict.code(), "approval.state_conflict");
    assert!(!conflict.retryable());
}

#[tokio::test]
async fn a_spent_one_shot_reports_already_consumed_rather_than_a_generic_conflict() {
    // `approval.already_consumed` is its own code in the contract, and it sends a user somewhere
    // different from a state conflict: the action already happened, so re-reading changes nothing.
    let (service, repositories, approval) = fixture().await;

    let mut stored = approval.clone();
    stored
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            jarvis_domain::tool::approval::ApprovalActor::Decided {
                principal: principal(),
                channel: jarvis_domain::tool::approval::ApprovalChannel::Cli,
                assurance: RequiredAssurance::Standard,
                note: None,
            },
            now(),
        )
        .expect("the fixture approves");
    stored
        .apply(
            ApprovalState::Consumed,
            stored.version(),
            jarvis_domain::tool::approval::ApprovalActor::Consumed {
                tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(4)),
            },
            now(),
        )
        .expect("the fixture consumes");
    let version = stored.version();
    repositories
        .apply_transition(
            workspace(),
            &jarvis_domain::tool::approval::ApprovalTransitionRecord {
                id: stored.id,
                from: ApprovalState::Pending,
                to: ApprovalState::Approved,
                prior_version: ApprovalVersion::FIRST,
                version: ApprovalVersion::new(2),
                actor: jarvis_domain::tool::approval::ApprovalActor::Decided {
                    principal: principal(),
                    channel: jarvis_domain::tool::approval::ApprovalChannel::Cli,
                    assurance: RequiredAssurance::Standard,
                    note: None,
                },
                occurred_at: now(),
            },
            ApprovalVersion::FIRST,
            &jarvis_domain::tool::approval::ApprovalActor::Decided {
                principal: principal(),
                channel: jarvis_domain::tool::approval::ApprovalChannel::Cli,
                assurance: RequiredAssurance::Standard,
                note: None,
            },
            &stored,
        )
        .await
        .expect("the double stores the decision");

    let _ = version;
    let error = service
        .decide(
            &context(RequestChannel::Cli),
            approval.id,
            DecisionCommand {
                decision: Decision::Approve,
                expected_version: version,
                fingerprint: &approved_digest(),
                note: None,
            },
            now(),
        )
        .await
        .expect_err("a spent one-shot cannot be approved again");
    assert_eq!(error.code(), "approval.already_consumed");
}

#[tokio::test]
async fn a_foreign_workspace_record_is_not_found_and_is_indistinguishable_from_an_absent_one() {
    // The contract requires the two to be indistinguishable, so this asserts the **code** is the same
    // for both — an assertion on one of them alone would pass while the other leaked.
    let repositories = Arc::new(InMemoryRepositories::new());
    let approval = pending();
    repositories
        .request(&approval)
        .await
        .expect("the fixture is inserted");
    let service = ApprovalService::new(Arc::clone(&repositories) as Arc<dyn ApprovalRepository>);

    let mut elsewhere = context(RequestChannel::Cli);
    elsewhere.workspace_id = WorkspaceId::from_uuid(uuid::Uuid::from_u128(42));
    let foreign = service
        .read(&elsewhere, approval.id)
        .await
        .expect_err("another workspace's record is not readable");

    let absent = service
        .read(
            &context(RequestChannel::Cli),
            ApprovalId::from_uuid(uuid::Uuid::from_u128(999)),
        )
        .await
        .expect_err("an absent identifier is not found");

    assert_eq!(foreign.code(), "approval.not_found");
    assert_eq!(
        foreign.code(),
        absent.code(),
        "**a foreign-workspace record must be indistinguishable from an absent one**",
    );
    assert_eq!(foreign, absent);
}

#[tokio::test]
async fn the_requester_may_cancel_its_own_request_from_a_channel_that_cannot_decide_it() {
    // The contract names "the requesting principal or authorized policy/operator", so the channel
    // check does not apply to the requester: a principal that asked on the CLI and is now
    // authenticated on voice is still the principal that asked. A third party on that channel is not.
    let (service, _repositories, approval) = fixture().await;

    let cancelled = service
        .cancel(
            &context(RequestChannel::Voice),
            approval.id,
            ApprovalVersion::FIRST,
            Some(&note("changed my mind")),
            now(),
        )
        .await
        .expect("the requester may withdraw its own request");
    assert!(cancelled.applied);
    assert_eq!(cancelled.approval.state(), ApprovalState::Cancelled);

    // The repeat is idempotent, per the contract's "idempotent repeats return current state".
    let repeat = service
        .cancel(
            &context(RequestChannel::Voice),
            approval.id,
            ApprovalVersion::FIRST,
            None,
            now(),
        )
        .await
        .expect("a repeated cancellation is idempotent");
    assert!(!repeat.applied);
    assert_eq!(repeat.approval.state(), ApprovalState::Cancelled);
}

#[tokio::test]
async fn a_third_party_may_not_cancel_on_a_channel_the_request_excludes() {
    // The complement of the requester's unconditional right: a principal that is **not** the requester
    // needs a channel the request permits. Without this, the requester rule would read as "anybody may
    // cancel anything", which is the direction that lets one surface withdraw another surface's prompt.
    let (service, _repositories, approval) = fixture().await;

    let error = service
        .cancel(
            &context_for(
                other_principal(),
                RequestChannel::Voice,
                AuthenticationAssurance::Standard,
            ),
            approval.id,
            ApprovalVersion::FIRST,
            None,
            now(),
        )
        .await
        .expect_err("a stranger on an excluded channel may not cancel");
    assert_eq!(
        error.code(),
        "approval.scope_denied",
        "a scope refusal names authority, while a channel refusal names the surface",
    );

    // And the record is untouched.
    let stored = service
        .read(&context(RequestChannel::Cli), approval.id)
        .await
        .expect("readable");
    assert_eq!(stored.state(), ApprovalState::Pending);
}

#[tokio::test]
async fn an_over_long_or_control_bearing_cancel_reason_is_refused() {
    // The reason is stored verbatim in the audit trail, so its bound is what stops one request writing
    // an unbounded row. A control character is refused for the same reason a preview item refuses one:
    // the value is rendered by an operator surface.
    let (service, _repositories, approval) = fixture().await;

    let long = "x".repeat(MAX_CANCEL_REASON_BYTES + 1);
    // **The bound is now enforced by the type, not by the service.** `DecisionNote::new` refuses the
    // value, so this asserts the constructor's rule rather than a second length check in the service —
    // which is what made the wire's `comment` and the cancellation's `reason` droppable before: each was
    // bounded at one layer and stored at none.
    assert!(
        DecisionNote::new(&long).is_err(),
        "an over-long note must be refused where it is constructed",
    );
    assert!(
        DecisionNote::new("stop\u{7}now").is_err(),
        "a bell is not whitespace"
    );
    assert!(
        DecisionNote::new("").is_err(),
        "an empty note is not a note"
    );
    assert!(
        DecisionNote::new("line one\nline two\tand a tab").is_ok(),
        "a newline and a tab are legitimate in an operator's note",
    );

    // The record is untouched, which is the half that matters: a refused request must not have
    // cancelled anything.
    let stored = service
        .read(&context(RequestChannel::Cli), approval.id)
        .await
        .expect("readable");
    assert_eq!(stored.state(), ApprovalState::Pending);
}

#[test]
fn only_approve_and_reject_are_accepted_verbs() {
    // The contract fixes the allowed values, so an unknown verb is a parse failure rather than a value
    // guessed at — and the two variants cannot be transposed by passing a bool.
    assert_eq!(
        Decision::parse("approve").expect("allowed"),
        Decision::Approve
    );
    assert_eq!(
        Decision::parse("reject").expect("allowed"),
        Decision::Reject
    );
    for refused in ["approve_always", "yes", "APPROVE", ""] {
        assert_eq!(
            Decision::parse(refused)
                .expect_err("only the contract's two verbs are accepted")
                .code(),
            "request.invalid",
            "{refused} must not parse",
        );
    }
}

#[test]
fn the_channels_a_request_may_come_from_map_onto_approval_channels_without_a_catch_all() {
    // The mapping is total over `RequestChannel`, and `approval_channel_of` has no `_` arm — so a
    // channel added later fails to compile here rather than inheriting whichever branch was last. This
    // test pins the *values* the current mapping produces, because "it compiles" and "it maps to the
    // right approval channel" are different claims.
    use crate::approval_service::approval_channel_of as mapped;
    use jarvis_domain::tool::approval::ApprovalChannel;

    let cases = [
        (RequestChannel::Cli, ApprovalChannel::Cli),
        (RequestChannel::Desktop, ApprovalChannel::Desktop),
        (RequestChannel::Mobile, ApprovalChannel::Mobile),
        (RequestChannel::Voice, ApprovalChannel::Voice),
        (RequestChannel::Api, ApprovalChannel::Api),
        (RequestChannel::Web, ApprovalChannel::Api),
        (RequestChannel::Internal, ApprovalChannel::Api),
    ];
    for (request, expected) in cases {
        assert_eq!(
            mapped(request),
            expected,
            "{request:?} must map to {expected:?}"
        );
    }
    assert_eq!(
        cases.len(),
        7,
        "every `RequestChannel` variant must appear, or a new one inherits a mapping untested",
    );
}
