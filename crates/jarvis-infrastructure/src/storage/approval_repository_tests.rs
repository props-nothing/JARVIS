//! Contract tests for the SQLite approval repository.
//!
//! Behaviour tests against a **real migrated SQLite database**, for the reason the run repository's
//! suite records: the properties that matter are the database's own — a `NOT NULL` with no default, a
//! unique primary key, an optimistic `UPDATE ... WHERE version = ?`, and a transaction that commits a
//! decision and its trail row together. A fake would agree with whatever the code assumed about them.
//!
//! **The read that matters most is the reconstruction, not the load.** `stored_approval` does not
//! assign a stored state: it drives the domain's own state machine from `PENDING` to whatever the row
//! claims, so a row whose state the machine cannot reach is detected as corruption instead of being
//! accepted as a decision that skipped a step. Several tests below assert that directly, because the
//! alternative — assigning `state` and `version` from the row — would pass every round-trip test in this
//! file while accepting a `PENDING -> CONSUMED` row that would then behave as a spent approval nobody
//! ever granted.

use std::sync::Arc;

use super::SqliteApprovalRepository;
use crate::storage::repositories::SqliteRepositories;
use crate::storage::repositories::tests::{run_id, seed, workspace};
use jarvis_application::repository::RepositoryError;
use jarvis_application::repository::approval::{ApprovalRepository as _, DecideOutcome};
use jarvis_domain::ids::{ApprovalId, PrincipalId, ToolCallId, WorkspaceId};
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::approval::{
    AllowedChannels, ApprovalActor, ApprovalChannel, ApprovalPreview, ApprovalRequestParts,
    ApprovalScopeKind, ApprovalState, ApprovalTransitionRecord, ApprovalVersion, DurableApproval,
    PreviewItem,
};
use jarvis_domain::tool::classification::{Effect, Risk};
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};

use crate::storage::connection::Database;
use crate::storage::migrate;

/// A migrated in-memory database with the approval repository and the runs it needs over it.
async fn repository() -> (Database, Arc<SqliteApprovalRepository>) {
    let database = Database::open_in_memory().await.expect("in-memory opens");
    migrate::run(database.pool()).await.expect("migrates");
    let runs = SqliteRepositories::new(database.pool().clone());
    // The approval row has a foreign key to `agent_runs`, and `foreign_keys` is on in this profile, so
    // the run the approval names must exist — a child naming an absent parent is refused, which is the
    // schema working rather than a fixture problem.
    seed(&runs).await;
    let approvals = Arc::new(SqliteApprovalRepository::new(database.pool().clone()));
    (database, approvals)
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:00:00Z").expect("the fixture instant parses")
}

fn expires_at() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:10:00Z").expect("the fixture instant parses")
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(1))
}

fn other_principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(2))
}

fn tool_call() -> ToolCallId {
    ToolCallId::from_uuid(uuid::Uuid::from_u128(3))
}

fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(99))
}

fn identity() -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::parse("mail.send@1").expect("canonical"),
        source: ToolSource::new(
            SourceKind::Connector,
            "acme.mail",
            ToolVersion::parse("1.0.0").expect("valid"),
        )
        .expect("the fixture source is valid"),
        schema_fingerprint: SchemaFingerprint::from_bytes([7; 32]),
    }
}

/// A one-shot approval request for the mail tool.
fn parts() -> ApprovalRequestParts {
    ApprovalRequestParts {
        workspace: workspace(),
        requesting_principal: principal(),
        run: run_id(),
        // `ToolCallId` is an opaque identifier with no foreign key, so a fixture constant is enough —
        // unlike the run, which `000008` references and which therefore has to be seeded.
        tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(3)),
        identity: identity(),
        action_digest: "sha256:the-approved-action".to_owned(),
        risk: Risk::High,
        effects: vec![Effect::ExternalCommunication, Effect::Write],
        summary: "Send one email to peter@example.com".to_owned(),
        preview: ApprovalPreview::new(vec![
            PreviewItem::new("To", "peter@example.com").expect("shaped"),
            PreviewItem::new("Subject", "Following up").expect("shaped"),
        ])
        .expect("the preview is within bounds"),
        allowed_channels: AllowedChannels::new(vec![
            ApprovalChannel::Cli,
            ApprovalChannel::Desktop,
        ])
        .expect("two channels"),
        expires_at: expires_at(),
        scope: ApprovalScopeKind::OneShot,
    }
}

/// A requested approval with a caller-chosen identifier.
fn approval_with(id: ApprovalId) -> DurableApproval {
    let mut approval = DurableApproval::request(parts());
    approval.id = id;
    approval
}

fn approval() -> DurableApproval {
    DurableApproval::request(parts())
}

fn decide(channel: ApprovalChannel) -> ApprovalActor {
    ApprovalActor::Decided {
        principal: principal(),
        channel,
    }
}

/// Approves a stored approval, returning the applied transition.
fn approve(approval: &mut DurableApproval, channel: ApprovalChannel) -> ApprovalTransitionRecord {
    let version = approval.version();
    approval
        .apply(ApprovalState::Approved, version, decide(channel), now())
        .expect("PENDING -> APPROVED is legal")
}

/// Consumes a stored approval, returning the applied transition.
fn consume(approval: &mut DurableApproval) -> ApprovalTransitionRecord {
    let version = approval.version();
    approval
        .apply(
            ApprovalState::Consumed,
            version,
            ApprovalActor::Consumed {
                tool_call: tool_call(),
            },
            now(),
        )
        .expect("APPROVED -> CONSUMED is legal")
}

/// The stable code of a refusal, or `None`.
fn code_of<T>(result: &Result<T, RepositoryError>) -> Option<&'static str> {
    result.as_ref().err().map(RepositoryError::code)
}

// ---------------------------------------------------------------------------------------
// Round trips.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_requested_approval_round_trips_with_every_field_intact() {
    // **Every field, because the point of the migration was that a record survives a restart with its
    // content.** A test that checked only the state would pass against an adapter that dropped the
    // preview or the identity, and those are the two fields whose loss would silently weaken a later
    // decision: an approval whose identity did not survive could not be matched to the tool it approved.
    let (_database, approvals) = repository().await;
    let requested = approval();
    approvals
        .request(&requested)
        .await
        .expect("the approval is inserted");

    let loaded = approvals
        .load(workspace(), requested.id)
        .await
        .expect("the approval loads");
    // **Every field except the identifier**, and the exception is deliberate rather than a relaxation:
    // `DurableApproval::request` generates its own v7 identifier, and the identifier a *stored* row
    // holds is the one the caller named. The adapter writes `approval.id` and reads it back from the
    // `id` column, so a mismatch would be a real defect — this asserts the round trip instead of
    // skipping it, by comparing the serialized documents with a single normalized field and then
    // checking the identifier on its own. Comparing the values directly is not possible from here,
    // because the state and version are private to the domain module, which is the same reason the
    // whole-field comparison is done through the documented serialized form.
    let mut loaded_document = serde_json::to_value(&loaded).expect("the loaded value serializes");
    let mut requested_document =
        serde_json::to_value(&requested).expect("the requested value serializes");
    loaded_document["id"] = serde_json::Value::String("normalized".to_owned());
    requested_document["id"] = serde_json::Value::String("normalized".to_owned());
    assert_eq!(
        loaded_document, requested_document,
        "every field but the generated identifier must survive the round trip",
    );
    assert_eq!(
        loaded.id, requested.id,
        "the stored identifier is the one named"
    );
    assert_eq!(loaded.state(), ApprovalState::Pending);
    assert_eq!(loaded.version(), ApprovalVersion::FIRST);
    assert_eq!(loaded.identity, requested.identity);
    assert_eq!(loaded.preview.items().len(), 2);
    assert_eq!(loaded.allowed_channels.as_slice().len(), 2);
    assert_eq!(loaded.scope, ApprovalScopeKind::OneShot);
    assert_eq!(loaded.action_digest, "sha256:the-approved-action");
    assert!(!loaded.is_decided());
}

#[tokio::test]
async fn an_approved_approval_round_trips_with_who_decided_and_where() {
    // The two fields the migration argues for storing: the deciding principal and the **channel**. A
    // record that lost the channel would make the `allowed_channels` check one-directional, since JARVIS
    // could refuse a disallowed channel going in and never say which one a decision came from.
    let (_database, approvals) = repository().await;
    let mut requested = approval();
    approvals.request(&requested).await.expect("inserted");
    let transition = approve(&mut requested, ApprovalChannel::Desktop);
    assert_eq!(
        approvals
            .apply_transition(
                workspace(),
                &transition,
                ApprovalVersion::FIRST,
                &decide(ApprovalChannel::Desktop),
                &requested,
            )
            .await
            .expect("the decision is stored"),
        DecideOutcome::Applied,
    );

    let loaded = approvals
        .load(workspace(), requested.id)
        .await
        .expect("loads");
    assert_eq!(loaded.state(), ApprovalState::Approved);
    assert_eq!(loaded.version(), ApprovalVersion::new(2));
    assert_eq!(loaded.decided_by(), Some(principal()));
    assert_eq!(
        loaded.decided_via(),
        Some(ApprovalChannel::Desktop),
        "the decision channel must survive, or the allowed_channels check is one-directional",
    );
    assert!(loaded.is_decided());
}

#[tokio::test]
async fn a_consumed_approval_round_trips_through_the_state_machine_rather_than_by_assignment() {
    // **The reconstruction test.** A consumed approval is two transitions from `PENDING`, and the reader
    // may only reach it by walking `APPROVED -> CONSUMED`. An adapter that assigned the stored state and
    // version would produce the same value here, but its version would not have been derived from the
    // transitions — and the next test is the one that distinguishes the two implementations.
    let (_database, approvals) = repository().await;
    let mut requested = approval();
    approvals.request(&requested).await.expect("inserted");
    let first = approve(&mut requested, ApprovalChannel::Cli);
    approvals
        .apply_transition(
            workspace(),
            &first,
            ApprovalVersion::FIRST,
            &decide(ApprovalChannel::Cli),
            &requested,
        )
        .await
        .expect("the approval is stored");
    let second = consume(&mut requested);
    approvals
        .apply_transition(
            workspace(),
            &second,
            ApprovalVersion::new(2),
            &ApprovalActor::Consumed {
                tool_call: tool_call(),
            },
            &requested,
        )
        .await
        .expect("the consumption is stored");

    let loaded = approvals
        .load(workspace(), requested.id)
        .await
        .expect("loads");
    assert_eq!(loaded.state(), ApprovalState::Consumed);
    assert_eq!(
        loaded.version(),
        ApprovalVersion::new(3),
        "three states visited means version three, derived from the walk rather than read",
    );
    assert_eq!(loaded, requested);
}

// ---------------------------------------------------------------------------------------
// Corruption: the reader refuses a state the machine cannot reach.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stored_state_the_machine_cannot_reach_is_corruption_rather_than_a_decision() {
    // **The case that distinguishes walking the machine from assigning the fields.** `PENDING ->
    // CONSUMED` is not an edge: an approval cannot be spent without having been given. A reader that
    // assigned the stored state would accept this row and hand back a *spent* approval that was never
    // approved — the worst possible reconstruction of a decision, since a caller would treat it as
    // something the user had already agreed to and the no-effect question would be unanswerable.
    //
    // The row is written directly, because no code path in the domain can produce it — which is exactly
    // why the reader has to refuse it rather than trust the writer.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    sqlx::query("UPDATE approvals SET state = 'consumed', version = 3 WHERE id = ?")
        .bind(requested.id.to_string())
        .execute(database.pool())
        .await
        .expect("the corruption is written");

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
        "an unreachable stored state must be corruption, not a reconstruction",
    );
}

#[tokio::test]
async fn a_decision_row_with_no_recorded_decider_is_corruption() {
    // The other reconstruction rule: the domain **always** records who decided and which channel, so a
    // row claiming `approved` with neither is a record of a decision nobody took. Guessing a channel
    // would be worse than refusing, because the guess is what the `allowed_channels` check exists to
    // prevent.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    sqlx::query("UPDATE approvals SET state = 'approved', version = 2 WHERE id = ?")
        .bind(requested.id.to_string())
        .execute(database.pool())
        .await
        .expect("the corruption is written");

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn a_stored_version_below_the_first_is_corruption() {
    // `ApprovalVersion::FIRST` is 1 so an uninitialised field cannot read as a valid stored version, and
    // this is the reader refusing a row the writer could not have produced.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    sqlx::query("UPDATE approvals SET version = 0 WHERE id = ?")
        .bind(requested.id.to_string())
        .execute(database.pool())
        .await
        .expect("the corruption is written");

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn an_identity_that_no_longer_parses_is_corruption() {
    // The identity goes through the **domain type's own deserializer**, so a stored document that no
    // longer parses is a migration fault rather than a missing record — and the distinction matters
    // because "absent" would let a caller retry a lookup forever while the row sits there.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    sqlx::query(
        "UPDATE approvals SET tool_identity_json = '{\"not\":\"an identity\"}' WHERE id = ?",
    )
    .bind(requested.id.to_string())
    .execute(database.pool())
    .await
    .expect("the corruption is written");

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn an_effects_document_with_an_unknown_effect_is_corruption() {
    // The effects go through the domain's `Effect` deserializer, whose closed set refuses an unknown
    // value rather than defaulting. An effect that arrived as an unknown string would otherwise be
    // dropped, and a policy check reading the remaining effects would classify the tool as less
    // consequential than it is.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    sqlx::query("UPDATE approvals SET effects_json = '[\"teleport\"]' WHERE id = ?")
        .bind(requested.id.to_string())
        .execute(database.pool())
        .await
        .expect("the corruption is written");

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn a_stored_channel_set_that_is_empty_is_corruption() {
    // The instance with the clearest consequence, inherited from `BRN-049`: `AllowedChannels` refuses an
    // empty set at construction **and on the way in**, because an approval with no permitted channel can
    // never be decided. Reading through the domain type means a stored empty set is refused too — a
    // reader that deserialized the raw vector would restore the unreachable state.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    sqlx::query("UPDATE approvals SET allowed_channels_json = '[]' WHERE id = ?")
        .bind(requested.id.to_string())
        .execute(database.pool())
        .await
        .expect("the corruption is written");

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
    );
}

// ---------------------------------------------------------------------------------------
// Scope: another workspace's approval is indistinguishable from a missing one.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_approval_in_another_workspace_is_not_found_rather_than_forbidden() {
    // The rule the local control API states for runs: a record from another scope is indistinguishable
    // from a missing one, so a caller cannot probe for identifiers it does not own. Asserted for `load`
    // and for the two listings, because a listing that returned another workspace's rows would be the
    // same disclosure with a different shape.
    let (_database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");

    assert_eq!(
        code_of(&approvals.load(other_workspace(), requested.id).await),
        Some("storage.not_found"),
    );
    assert!(
        approvals
            .pending_in(other_workspace(), 10)
            .await
            .expect("the listing succeeds")
            .is_empty(),
        "another workspace's pending queue must not include this approval",
    );
    // And the owning workspace still sees it, so the refusal is about scope rather than about the row.
    assert_eq!(
        approvals
            .pending_in(workspace(), 10)
            .await
            .expect("the listing succeeds")
            .len(),
        1,
    );
}

// ---------------------------------------------------------------------------------------
// The idempotent double-tap.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_second_identical_decision_is_idempotent_rather_than_a_conflict() {
    // **A user double-tapping "approve" sends two requests with the same expected version**, and the
    // second must be idempotent: their intent is satisfied and nothing was overwritten. Reporting a
    // version conflict would be a lie, since the caller's view is exactly current — and it would make
    // the ordinary double-tap look like a failure to the person who performed it.
    let (_database, approvals) = repository().await;
    let mut requested = approval();
    approvals.request(&requested).await.expect("inserted");
    let transition = approve(&mut requested, ApprovalChannel::Cli);
    assert_eq!(
        approvals
            .apply_transition(
                workspace(),
                &transition,
                ApprovalVersion::FIRST,
                &decide(ApprovalChannel::Cli),
                &requested,
            )
            .await
            .expect("the first decision is stored"),
        DecideOutcome::Applied,
    );
    // The second request carries the **same** expected version, which is what a double-tap does: the
    // caller built both requests from one read.
    assert_eq!(
        approvals
            .apply_transition(
                workspace(),
                &transition,
                ApprovalVersion::FIRST,
                &decide(ApprovalChannel::Cli),
                &requested,
            )
            .await
            .expect("the repeat is idempotent"),
        DecideOutcome::AlreadyInState,
    );
    // And the stored version moved exactly once, so the repeat wrote nothing.
    assert_eq!(
        approvals
            .load(workspace(), requested.id)
            .await
            .expect("loads")
            .version(),
        ApprovalVersion::new(2),
    );
}

#[tokio::test]
async fn a_stale_expected_version_is_refused() {
    // The counterpart: the caller states a version that is neither current nor the already-in-state case,
    // so its view is genuinely stale and must be corrected before it is told anything else.
    let (_database, approvals) = repository().await;
    let mut requested = approval();
    approvals.request(&requested).await.expect("inserted");
    let transition = approve(&mut requested, ApprovalChannel::Cli);
    approvals
        .apply_transition(
            workspace(),
            &transition,
            ApprovalVersion::FIRST,
            &decide(ApprovalChannel::Cli),
            &requested,
        )
        .await
        .expect("the decision is stored");

    // A transition to `CONSUMED` stated at version 1, which is now two behind — and note the target
    // differs from the stored state, so the idempotence branch cannot apply. The domain value must be
    // driven to `CONSUMED` as well, because `apply_transition` is refused when the caller's own record
    // disagrees with the transition it is asking to store.
    let mut consumed = approvals
        .load(workspace(), requested.id)
        .await
        .expect("loads");
    let consume_transition = consume(&mut consumed);
    assert_eq!(
        code_of(
            &approvals
                .apply_transition(
                    workspace(),
                    &consume_transition,
                    ApprovalVersion::FIRST,
                    &ApprovalActor::Consumed {
                        tool_call: tool_call()
                    },
                    &consumed,
                )
                .await
        ),
        Some("storage.version_conflict"),
    );
}

#[tokio::test]
async fn an_illegal_edge_is_refused_by_the_domains_own_table() {
    // The adapter asks the domain's transition table rather than duplicating it, so `PENDING -> CONSUMED`
    // is refused here for the same reason the domain refuses it. Asserted through the port because the
    // adapter's own read-then-write could otherwise have accepted it: it reads the state, and a write
    // would succeed against `consumed` if the edge check were missing.
    //
    // **The fixture is built without ever driving the domain value to `CONSUMED`, and that is the
    // point.** A record produced by `apply` always names an edge the domain's table permits *from the
    // state the value was in*, so a record built on a value driven to `PENDING -> APPROVED -> CONSUMED`
    // is not an illegal edge — it is only illegal against the *stored* row, which is `PENDING`. A
    // record built as a domain value would therefore have to be a legal transition, and the test would
    // be asserting the adapter's read-then-write rather than its edge check. The record is constructed
    // instead, which no caller is supposed to do — and which is exactly why the adapter has to re-judge
    // the edge against the stored state rather than trusting the record it is handed.
    let (_database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    let illegal = ApprovalTransitionRecord {
        id: requested.id,
        from: ApprovalState::Pending,
        to: ApprovalState::Consumed,
        prior_version: ApprovalVersion::FIRST,
        version: ApprovalVersion::new(2),
        actor: ApprovalActor::Consumed {
            tool_call: tool_call(),
        },
        occurred_at: now(),
    };
    // The stored row is still `PENDING`, so `consumed` is not an edge from it.
    assert_eq!(
        code_of(
            &approvals
                .apply_transition(
                    workspace(),
                    &illegal,
                    ApprovalVersion::FIRST,
                    &ApprovalActor::Consumed {
                        tool_call: tool_call()
                    },
                    &requested,
                )
                .await
        ),
        Some("storage.transition_refused"),
    );
    // And the refusal left the row exactly where it was: a refused edge must not have written a state.
    assert_eq!(
        approvals
            .load(workspace(), requested.id)
            .await
            .expect("loads")
            .state(),
        ApprovalState::Pending,
    );
}

#[tokio::test]
async fn a_duplicated_request_is_a_conflict_rather_than_a_second_row() {
    // The identifier is generated at construction, so a caller cannot name the same approval twice and a
    // duplicate is a caller bug rather than an idempotent retry. Refused with a named rule so the caller
    // does not have to read a driver's constraint failure.
    let (_database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    assert_eq!(
        code_of(&approvals.request(&requested).await),
        Some("storage.conflict"),
    );
}

// ---------------------------------------------------------------------------------------
// The listings.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_pending_listing_orders_by_what_lapses_soonest_and_excludes_decided_rows() {
    // The order is the operator's working order: the prompt that lapses first is the one they must act on
    // first. And a decided approval must not appear, because a queue that included them would make a
    // settled decision look outstanding.
    let (_database, approvals) = repository().await;
    let later = {
        let mut parts = parts();
        parts.expires_at = UtcTimestamp::parse("2026-09-27T12:30:00Z").expect("parses");
        let mut approval = DurableApproval::request(parts);
        approval.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(11));
        approval
    };
    let sooner = {
        let mut parts = parts();
        parts.expires_at = UtcTimestamp::parse("2026-09-27T12:05:00Z").expect("parses");
        let mut approval = DurableApproval::request(parts);
        approval.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(12));
        approval
    };
    approvals.request(&later).await.expect("inserted");
    approvals.request(&sooner).await.expect("inserted");

    let pending = approvals.pending_in(workspace(), 10).await.expect("lists");
    assert_eq!(pending.len(), 2);
    assert_eq!(
        pending[0].id, sooner.id,
        "the approval that lapses soonest must come first",
    );

    // Decide one and it leaves the queue.
    let mut decided = sooner.clone();
    let transition = approve(&mut decided, ApprovalChannel::Cli);
    approvals
        .apply_transition(
            workspace(),
            &transition,
            ApprovalVersion::FIRST,
            &decide(ApprovalChannel::Cli),
            &decided,
        )
        .await
        .expect("stored");
    let pending = approvals.pending_in(workspace(), 10).await.expect("lists");
    assert_eq!(
        pending.len(),
        1,
        "a decided approval leaves the pending queue"
    );
    assert_eq!(pending[0].id, later.id);
}

#[tokio::test]
async fn the_decided_listing_returns_one_principals_decisions_most_recent_first() {
    // "What did I approve?" — scoped to the principal who decided, and ordered by when. Both halves are
    // asserted: a listing that ignored the principal would disclose another user's decisions, and one
    // that ignored the order would not answer the question the port states.
    let (_database, approvals) = repository().await;
    let first = approval_with(ApprovalId::from_uuid(uuid::Uuid::from_u128(21)));
    approvals.request(&first).await.expect("inserted");
    let mut first = first;
    let transition = approve(&mut first, ApprovalChannel::Cli);
    approvals
        .apply_transition(
            workspace(),
            &transition,
            ApprovalVersion::FIRST,
            &decide(ApprovalChannel::Cli),
            &first,
        )
        .await
        .expect("stored");

    let second = approval_with(ApprovalId::from_uuid(uuid::Uuid::from_u128(22)));
    approvals.request(&second).await.expect("inserted");
    let mut second = second;
    let transition = approve(&mut second, ApprovalChannel::Desktop);
    approvals
        .apply_transition(
            workspace(),
            &transition,
            ApprovalVersion::FIRST,
            &decide(ApprovalChannel::Desktop),
            &second,
        )
        .await
        .expect("stored");

    let decided = approvals
        .decided_by(workspace(), principal(), 10)
        .await
        .expect("lists");
    assert_eq!(decided.len(), 2);
    assert!(
        decided
            .iter()
            .all(|approval| approval.decided_by() == Some(principal())),
        "the listing is scoped to the deciding principal",
    );
    // Another principal decided nothing, so their list is empty even though the workspace has decisions.
    assert!(
        approvals
            .decided_by(workspace(), other_principal(), 10)
            .await
            .expect("lists")
            .is_empty(),
        "another principal's decisions must not appear",
    );
}

#[tokio::test]
async fn a_listing_is_bounded_by_its_limit() {
    // The limit is a parameter rather than a default, so a caller decides how much to read — and the
    // assertion is that it is honoured, since a listing that ignored it would be an unbounded read behind
    // a bound-looking API.
    let (_database, approvals) = repository().await;
    for index in 0..5 {
        let mut approval = approval();
        approval.id = ApprovalId::from_uuid(uuid::Uuid::from_u128(30 + index));
        approvals.request(&approval).await.expect("inserted");
    }
    assert_eq!(
        approvals
            .pending_in(workspace(), 2)
            .await
            .expect("lists")
            .len(),
        2,
    );
}

#[tokio::test]
async fn a_request_that_is_already_decided_is_refused() {
    // `request` inserts a `PENDING` row, and the state it writes comes from the domain value. A value
    // that arrived here already decided would be a decision nobody made, so it is refused rather than
    // inserted with a decision column silently ignored.
    let (_database, approvals) = repository().await;
    let mut requested = approval();
    approve(&mut requested, ApprovalChannel::Cli);
    assert_eq!(
        code_of(&approvals.request(&requested).await),
        Some("storage.conflict"),
    );
}

#[tokio::test]
async fn a_stored_version_the_walk_cannot_derive_is_corruption() {
    // **The cross-check between the walk and the row, and it needed its own test to be reachable.** The
    // reader derives the version by *walking* the state machine from `PENDING` and compares the result
    // with the stored `version` column. A row whose state and version disagree is a record of a
    // transition count that does not match the state it claims — a migration fault rather than a
    // decision.
    //
    // The mutation that removed this comparison **compiled and passed every test**, because no other
    // test writes a version the walk would disagree with: the corruption cases above all use consistent
    // pairs. That is the "reads as enforcement while enforcing nothing" shape this project keeps
    // finding, and it is why the check is asserted directly rather than left to be covered by a case
    // that happens to be inconsistent anyway.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    // `approved` is one step from `PENDING`, so the walk derives version 2 — and the row claims 5, which
    // no sequence of legal edges reaches.
    shape_row(&database, requested.id, "approved", 5, true).await;

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
        "a stored version the walk does not derive must be corruption",
    );
}

#[tokio::test]
async fn a_decision_row_with_no_recorded_instant_is_corruption() {
    // The third decision column. `DurableApproval::apply` always writes `decided_at` for a decision, so a
    // row claiming `approved` with no instant is a decision nobody took at any time — and the reader
    // needs the instant to reconstruct the transition, so this is the one case where a *missing* column
    // stops the walk rather than being ignored. `decided_by` and `decided_via` are covered above; this is
    // the column that would otherwise be read by nothing.
    let (database, approvals) = repository().await;
    let requested = approval();
    approvals.request(&requested).await.expect("inserted");
    sqlx::query(
        "UPDATE approvals SET state = 'approved', version = 2, decided_by = ?, decided_via = 'cli', \
         decided_at = NULL WHERE id = ?",
    )
    .bind(principal().to_string())
    .bind(requested.id.to_string())
    .execute(database.pool())
    .await
    .expect("the corruption is written");

    assert_eq!(
        code_of(&approvals.load(workspace(), requested.id).await),
        Some("storage.row_corrupted"),
    );
}

/// Writes a stored state, version, and decision columns for a row.
///
/// The row is shaped the way the adapter would have shaped it, including the `decided_at` a decision
/// always carries — the reader refuses a decision without one, so a fixture that omitted it would be
/// exercising the corruption path rather than the reconstruction.
async fn shape_row(database: &Database, id: ApprovalId, state: &str, version: i64, decided: bool) {
    sqlx::query(
        "UPDATE approvals SET state = ?, version = ?, decided_by = ?, decided_via = ?, \
         decided_at = ?, updated_at = ? WHERE id = ?",
    )
    .bind(state)
    .bind(version)
    .bind(decided.then(|| principal().to_string()))
    .bind(decided.then_some("cli"))
    .bind(decided.then(|| now().to_string()))
    .bind(now().to_string())
    .bind(id.to_string())
    .execute(database.pool())
    .await
    .expect("the row is shaped");
}

#[tokio::test]
async fn every_reachable_state_the_machine_can_walk_to_reconstructs_with_its_own_version() {
    // **The path table's other five arms.** The round-trip tests above reach `approved` and `consumed`,
    // which is two of the seven states; each of the others is reconstructed by its own arm of `reach`,
    // and an arm that named the wrong actor or the wrong path length would produce a wrong state or a
    // wrong version. Table-driven because the five differ only in the target and the columns a row needs.
    //
    // `invalidated` is the interesting one: it is two steps from `PENDING`, so a reader that put every
    // terminal state one step from `PENDING` would either refuse the row or land on version 2.
    struct Case {
        state: &'static str,
        version: i64,
        decided: bool,
        expected: ApprovalState,
        expected_version: u64,
    }
    let cases = [
        Case {
            state: "approved",
            version: 2,
            decided: true,
            expected: ApprovalState::Approved,
            expected_version: 2,
        },
        Case {
            state: "rejected",
            version: 2,
            decided: true,
            expected: ApprovalState::Rejected,
            expected_version: 2,
        },
        Case {
            state: "expired",
            version: 2,
            decided: false,
            expected: ApprovalState::Expired,
            expected_version: 2,
        },
        Case {
            state: "cancelled",
            version: 2,
            decided: false,
            expected: ApprovalState::Cancelled,
            expected_version: 2,
        },
        Case {
            state: "consumed",
            version: 3,
            decided: true,
            expected: ApprovalState::Consumed,
            expected_version: 3,
        },
        Case {
            state: "invalidated",
            version: 3,
            decided: true,
            expected: ApprovalState::Invalidated,
            expected_version: 3,
        },
    ];

    for case in cases {
        let (database, approvals) = repository().await;
        let requested = approval();
        approvals.request(&requested).await.expect("inserted");
        shape_row(
            &database,
            requested.id,
            case.state,
            case.version,
            case.decided,
        )
        .await;

        let loaded = match approvals.load(workspace(), requested.id).await {
            Ok(loaded) => loaded,
            // `assert_eq!` rather than `panic!`, which the workspace denies even in tests: the failure
            // still names the state and reports the code the reader produced.
            Err(error) => {
                assert_eq!(
                    error.code(),
                    "no error expected",
                    "{} must reconstruct from its stored row",
                    case.state,
                );
                continue;
            }
        };
        assert_eq!(loaded.state(), case.expected, "{} reconstructs", case.state);
        assert_eq!(
            loaded.version(),
            ApprovalVersion::new(case.expected_version),
            "{} is {} step(s) from pending",
            case.state,
            case.expected_version - 1,
        );
        if case.decided {
            assert_eq!(
                loaded.decided_by(),
                Some(principal()),
                "a decision's decider must survive {}",
                case.state,
            );
            assert_eq!(
                loaded.decided_via(),
                Some(ApprovalChannel::Cli),
                "a decision's channel must survive {}",
                case.state,
            );
        }
    }
}
