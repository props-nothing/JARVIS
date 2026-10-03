//! Contract tests for the SQLite tool-call ledger repository.
//!
//! Behaviour tests against a **real migrated SQLite database**, for the reason the run and approval
//! suites record: the properties that matter are the database's own — a unique index over four columns,
//! an optimistic `UPDATE ... WHERE version = ?`, a `NULL` that must be refused rather than defaulted, and
//! a transaction that commits a transition with its trail row. A fake would agree with whatever the code
//! assumed about them, and the one property this adapter exists for — **that a second process cannot take
//! a reservation another process holds** — is a property of the index, so a fake could not test it at all.
//!
//! **The load-bearing test is `two_connections_racing_the_same_key_produce_one_grant`**, and it is worth
//! saying why it is written with two file-backed connections rather than two `SqliteToolCallRepository`
//! values over one pool. Two repositories over one pool share a connection pool, so a test written that
//! way would pass for an adapter that read, decided, and then wrote — the very shape this adapter exists
//! to rule out. Two connections to one *file* are two independent SQLite sessions, which is what
//! reproduces the cross-process case on one host.
//!
//! **The second test that is easy to get wrong is the identity-determinism one.** The unique index
//! deduplicates on the serialized identity text, so the reservation only works if two equal identities
//! serialize identically. That is an assumption about a *serializer*, and an assumption a uniqueness
//! guarantee rests on has to be asserted rather than believed.

use std::sync::Arc;

use super::SqliteToolCallRepository;
use crate::storage::connection::Database;
use crate::storage::migrate;
use crate::storage::repositories::SqliteRepositories;
use crate::storage::repositories::tests::{run_id, seed, workspace};
use jarvis_application::repository::RepositoryError;
use jarvis_application::repository::tool_call::ToolCallRepository as _;
use jarvis_domain::ids::{PrincipalId, ToolCallId, ToolCallRecordId, WorkspaceId};
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use jarvis_domain::tool::ledger::{
    LedgerEntry, LedgerOperation, ReservationKey, ReservationOutcome, ToolCallState,
    ToolCallTransition, ToolCallVersion,
};

/// A migrated in-memory database with the ledger repository over it.
async fn repository() -> (Database, Arc<SqliteToolCallRepository>) {
    let database = Database::open_in_memory().await.expect("in-memory opens");
    migrate::run(database.pool()).await.expect("migrates");
    let runs = SqliteRepositories::new(database.pool().clone());
    // The ledger row has a foreign key to `agent_runs` and `foreign_keys` is on in this profile, so the
    // run the row names must exist — a child naming an absent parent is refused, which is the schema
    // working rather than a fixture problem.
    seed(&runs).await;
    let ledger = Arc::new(SqliteToolCallRepository::new(database.pool().clone()));
    (database, ledger)
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:00:00Z").expect("the fixture instant parses")
}

fn later() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T12:01:00Z").expect("the fixture instant parses")
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(1))
}

fn other_principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(2))
}

fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(99))
}

fn tool_call() -> ToolCallId {
    ToolCallId::from_uuid(uuid::Uuid::from_u128(3))
}

/// The fixture identity. A real one, because the unique index folds it into key material.
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

/// The fixture reservation key.
fn key() -> ReservationKey {
    ReservationKey::new(identity(), workspace(), principal(), "send-once").expect("a valid key")
}

/// A freshly reserved row.
fn entry() -> LedgerEntry {
    LedgerEntry::reserve(
        tool_call(),
        1,
        run_id(),
        key(),
        LedgerOperation::Execute,
        now(),
    )
}

/// A reserved row with a caller-chosen record identifier.
fn entry_with(id: ToolCallRecordId) -> LedgerEntry {
    let mut entry = entry();
    entry.id = id;
    entry
}

/// Returns a shortest legal path from `from` to `to`, or `None` if the table has none.
///
/// **Derived from the domain's own table rather than written out here.** The first version of these
/// helpers hardcoded the sequence and went straight from `REQUESTED` to `EXECUTING` — which is not an
/// edge, because the real path runs through `VALIDATED` and `RESERVED`. A test helper that restates the
/// state machine is a second copy of it, and the copy that a test walks is the one that will still look
/// right after the table changes. Breadth-first over `ToolCallState::ALL` using the domain's own
/// `can_transition_to` means the walk cannot disagree with the machine it is exercising.
fn path_between(from: ToolCallState, to: ToolCallState) -> Option<Vec<ToolCallState>> {
    if from == to {
        return None;
    }
    let mut frontier = vec![vec![from]];
    let mut seen = vec![from];
    while let Some(paths) = {
        let current = std::mem::take(&mut frontier);
        (!current.is_empty()).then_some(current)
    } {
        let mut next = Vec::new();
        for path in paths {
            let last = *path.last().expect("a path has at least one state");
            for candidate in ToolCallState::ALL {
                if !last.can_transition_to(*candidate) || seen.contains(candidate) {
                    continue;
                }
                let mut extended = path.clone();
                extended.push(*candidate);
                if *candidate == to {
                    // The first `from` is dropped, so the result is the states to *enter*.
                    return Some(extended[1..].to_vec());
                }
                seen.push(*candidate);
                next.push(extended);
            }
        }
        frontier = next;
    }
    None
}

/// Drives a row to `to` along a legal path, storing every transition.
///
/// Every step is stored, because the adapter's `apply_transition` takes one transition and enforces the
/// optimistic version — a helper that applied several steps in memory and stored only the last one would
/// leave the stored version behind the caller's and the test would fail for a reason that is about the
/// helper rather than the adapter.
async fn drive(
    ledger: &SqliteToolCallRepository,
    entry: &mut LedgerEntry,
    to: ToolCallState,
    outcome: Option<ToolErrorClass>,
) {
    let Some(path) = path_between(entry.state(), to) else {
        // Every call in this file names a target the table reaches, so this branch is unreachable. It
        // asserts rather than panics because the workspace denies `panic!` even in tests, and the
        // assertion still reports the two states, which is what a reader needs if it ever fires.
        assert!(
            entry.state().can_transition_to(to),
            "the domain's table must have a path from {} to {to}",
            entry.state(),
        );
        return;
    };
    // The scope comes from the row's **own key**, not from a fixture constant: the adapter resolves an
    // operation inside the workspace it is given, so a helper that passed the wrong one would report
    // `not_found` for a row that exists — which is a mistake in the helper rather than a property of the
    // adapter. Reading it from the key also makes this helper usable for a row deliberately placed in
    // another workspace, which the unscoped-scan test needs.
    let scope = entry.key.workspace;
    for step in path {
        let expected = entry.version();
        let is_last = step == to;
        let transition = entry
            .apply(
                step,
                expected,
                if is_last { outcome } else { None },
                later(),
            )
            .expect("the walk follows the domain's own table");
        ledger
            .apply_transition(scope, &transition, expected, entry)
            .await
            .expect("each step of a legal walk is storable");
    }
}

/// The stable code of a refusal, or `None`.
fn code_of<T>(result: &Result<T, RepositoryError>) -> Option<&'static str> {
    result.as_ref().err().map(RepositoryError::code)
}

// ---------------------------------------------------------------------------------------
// The transition: state, version, dispatch fact, and the trail row.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_full_walk_round_trips_with_its_version_dispatch_fact_and_outcome() {
    // **The round trip asserts the dispatch column, and that column is the point of the table.**
    // `may_have_effected()` answers `false` for a dispatched call if dispatch is derived from the
    // current state, and that inversion is the `TLS-006` defect that duplicates an effect. So the row is
    // driven to a terminal state — where the state no longer says "dispatched" — and the reloaded row
    // must still say it was.
    let (_database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    drive(
        &ledger,
        &mut row,
        ToolCallState::Succeeded,
        Some(ToolErrorClass::ProviderError),
    )
    .await;

    let stored = ledger
        .load(workspace(), row.id)
        .await
        .expect("the row loads");
    assert_eq!(stored.state(), ToolCallState::Succeeded);
    assert_eq!(
        stored.version(),
        row.version(),
        "the stored version is the one the walk reached",
    );
    assert!(
        stored.was_dispatched(),
        "**a terminal row must still record that it was dispatched** — deriving this from the state is                          the defect that retries a call whose effect may have landed",
    );
    assert_eq!(stored.outcome(), Some(ToolErrorClass::ProviderError));
    assert!(
        stored.may_have_effected(),
        "a dispatched call with no proof of no effect may have effected"
    );
    assert_eq!(
        stored.attempt(),
        1,
        "the attempt number must survive, or a retry cannot be reported coherently",
    );
    assert_eq!(stored.key, row.key, "the reservation key must survive");
    assert_eq!(stored.id, row.id, "the stored identifier is the one named");
}

#[tokio::test]
async fn every_transition_appends_a_trail_row_in_the_same_transaction() {
    // The trail is not decoration: it is the durable answer to "was this call ever dispatched", and a
    // state change that committed without it would leave the state and the audit of it disagreeing.
    // The count is asserted rather than merely "some rows exist", because a trail that recorded one row
    // per *row* rather than per transition would be a plausible-looking audit with the wrong cardinality.
    let (database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    let path = path_between(ToolCallState::Requested, ToolCallState::Succeeded)
        .expect("the table has a path");
    assert!(
        path.len() >= 3,
        "the path to success runs through validated, reserved and executing",
    );
    drive(
        &ledger,
        &mut row,
        ToolCallState::Succeeded,
        Some(ToolErrorClass::ProviderError),
    )
    .await;

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tool_call_transitions WHERE record_id = ?")
            .bind(row.id.to_string())
            .fetch_one(database.pool())
            .await
            .expect("the trail is readable");
    assert_eq!(
        rows,
        i64::try_from(path.len()).expect("a small count"),
        "one trail row per applied transition",
    );

    // And the trail names the dispatch, which is the fact a later retry decision turns on.
    let dispatched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tool_call_transitions WHERE record_id = ? AND to_state = 'executing'",
    )
    .bind(row.id.to_string())
    .fetch_one(database.pool())
    .await
    .expect("readable");
    assert_eq!(dispatched, 1, "exactly one dispatch must be recorded");
}

#[tokio::test]
async fn a_stale_expected_version_is_refused() {
    // The optimistic predicate. A caller working from a stale view must be told its view is stale before
    // it is told anything else, because an edge computed from a stale state may be legal from the current
    // one — and for a ledger the consequence is a second dispatch.
    let (_database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    drive(&ledger, &mut row, ToolCallState::Validated, None).await;

    // A transition built by the caller from a view one version behind.
    let mut stale = row.clone();
    let transition = {
        let version = stale.version();
        stale
            .apply(
                ToolCallState::Denied,
                version,
                Some(ToolErrorClass::PermissionDenied),
                later(),
            )
            .expect("VALIDATED -> DENIED is legal")
    };
    assert_eq!(
        code_of(
            &ledger
                .apply_transition(workspace(), &transition, ToolCallVersion::FIRST, &stale,)
                .await
        ),
        Some("storage.version_conflict"),
    );
}

#[tokio::test]
async fn an_illegal_edge_is_refused_by_the_domains_own_table() {
    // The adapter re-judges the edge against the **stored** state through the domain's table, rather
    // than trusting the transition record it is handed. The record is constructed here because no caller
    // is supposed to build one that disagrees with its value — which is exactly why the adapter has to
    // check rather than believe.
    let (_database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    let illegal = ToolCallTransition {
        id: row.id,
        from: ToolCallState::Requested,
        to: ToolCallState::Executing,
        prior_version: ToolCallVersion::FIRST,
        version: ToolCallVersion::new(2),
        outcome: None,
        occurred_at: later(),
    };
    assert_eq!(
        code_of(
            &ledger
                .apply_transition(workspace(), &illegal, ToolCallVersion::FIRST, &row)
                .await
        ),
        Some("storage.transition_refused"),
        "REQUESTED -> EXECUTING is not an edge: the reservation cannot be skipped",
    );
    // And the refusal wrote nothing, so a refused edge cannot have moved the row.
    assert_eq!(
        ledger
            .load(workspace(), row.id)
            .await
            .expect("loads")
            .state(),
        ToolCallState::Requested,
    );
}

#[tokio::test]
async fn a_row_in_another_workspace_is_not_found_rather_than_forbidden() {
    // The rule the local control API states for runs: a record from another scope is indistinguishable
    // from a missing one, so a caller cannot probe for identifiers it does not own.
    let (_database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    assert_eq!(
        code_of(&ledger.load(other_workspace(), row.id).await),
        Some("storage.not_found"),
    );
    assert_eq!(
        ledger
            .load(workspace(), row.id)
            .await
            .expect("the owning workspace still sees it")
            .id,
        row.id,
    );
    assert_eq!(
        code_of(
            &ledger
                .load(
                    workspace(),
                    ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(404))
                )
                .await
        ),
        Some("storage.not_found"),
    );
}

// ---------------------------------------------------------------------------------------
// Corruption: a stored value the writer could not have produced.
// ---------------------------------------------------------------------------------------

/// Corrupts one column of a stored row and returns the load's code.
///
/// The statement is `&'static str` rather than `&str` because `sqlx::query` requires its SQL to outlive
/// the call. That is the same rule that makes a run-time-built query impossible in the adapter, and it is
/// why every statement there is a constant — so the requirement surfacing in a test helper is the rule
/// working rather than an inconvenience.
async fn corruption_code(
    database: &Database,
    id: ToolCallRecordId,
    statement: &'static str,
) -> Option<&'static str> {
    sqlx::query(statement)
        .bind(id.to_string())
        .execute(database.pool())
        .await
        .expect("the corruption is written");
    let ledger = SqliteToolCallRepository::new(database.pool().clone());
    code_of(&ledger.load(workspace(), id).await)
}

#[tokio::test]
async fn a_stored_state_the_domain_does_not_know_is_corruption() {
    // Not "absent": skipping an uninterpretable row converts a migration problem into apparent data
    // loss, and for a ledger that means a call whose outcome is recorded would look like one that never
    // happened.
    let (database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET state = 'teleporting' WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn a_terminal_row_with_no_recorded_outcome_is_corruption() {
    // `apply` records an outcome for every terminal transition, so a terminal row without one is a row
    // claiming to have finished without saying what happened. Defaulting it would let a caller read an
    // unknown outcome as a recorded failure and retry it.
    let (database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    drive(
        &ledger,
        &mut row,
        ToolCallState::Succeeded,
        Some(ToolErrorClass::ProviderError),
    )
    .await;
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET outcome = NULL WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn a_dispatched_row_with_no_dispatch_instant_is_corruption_not_harmless() {
    // **The invariant with teeth, and the one whose failure duplicates an effect.** A stored `SUCCEEDED`
    // with a `NULL` dispatch column claims an effect happened without the dispatch that caused it, and
    // reading it as harmless is exactly the inversion `TLS-006` fixed: `may_have_effected()` would say
    // `false` for a call that reached the provider.
    let (database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    drive(
        &ledger,
        &mut row,
        ToolCallState::Succeeded,
        Some(ToolErrorClass::ProviderError),
    )
    .await;
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET dispatched_at = NULL WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
        "a state that requires a dispatch must not be readable without one",
    );
}

#[tokio::test]
async fn a_version_below_the_first_is_corruption() {
    // `ToolCallVersion::FIRST` is 1 so an uninitialised field cannot read as a valid stored version.
    // The refusal is the domain's `from_stored`, which exists for this reader rather than being
    // re-implemented here.
    let (database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET version = 0 WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn a_zero_attempt_is_corruption() {
    // "Attempt zero" has no meaning, and `reserve` normalises it, so a stored zero is a row the writer
    // could not have produced.
    let (database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET attempt = 0 WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn a_no_effect_claim_that_is_neither_zero_nor_one_is_corruption() {
    // The column is an integer rather than a nullable boolean so there is no third state to interpret,
    // and the reader accepts exactly 0 and 1. A stored 2 is a value the writer cannot produce, and
    // treating any non-zero value as `true` would accept it as a claim that an effect definitely did not
    // happen — the one claim that must never be made loosely.
    let (database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET no_effect_confirmed = 2 WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn an_outcome_the_domain_does_not_know_is_corruption() {
    // The outcome goes through the domain's own parser, whose closed set refuses an unknown value
    // rather than defaulting — a class that fell through would reach a caller as a code the contract
    // does not define.
    let (database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    drive(
        &ledger,
        &mut row,
        ToolCallState::Succeeded,
        Some(ToolErrorClass::ProviderError),
    )
    .await;
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET outcome = 'tool.teleport_failed' WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn an_identity_that_no_longer_parses_is_corruption() {
    // The identity goes through the domain type's own deserializer, so a stored document that no longer
    // parses is a migration fault — and "absent" would let a caller retry a lookup forever while the row
    // sits there.
    let (database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    assert_eq!(
        corruption_code(&database, row.id, "UPDATE tool_call_records SET tool_identity_json = '{\"not\":\"an identity\"}' WHERE id = ?").await,
        Some("storage.row_corrupted"),
    );
}

#[tokio::test]
async fn an_operation_the_domain_does_not_know_is_corruption() {
    // An unrecognised operation must not be read as `EXECUTE`, which is the fail-open direction: a row
    // opened for a read would look like one that dispatches.
    let (database, ledger) = repository().await;
    let row = entry();
    ledger.reserve(&row).await.expect("granted");
    assert_eq!(
        corruption_code(
            &database,
            row.id,
            "UPDATE tool_call_records SET operation = 'observe' WHERE id = ?"
        )
        .await,
        Some("storage.row_corrupted"),
    );
}

// ---------------------------------------------------------------------------------------
// The reconciliation scan.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_reconciliation_scan_returns_exactly_the_dispatched_calls_with_no_outcome() {
    // **The scan `ACC-025` depends on.** A crash between `EXECUTING` and the outcome leaves such a row,
    // and a startup pass must find them without knowing their keys. Three rows exercise the three cases
    // that must be told apart: one dispatched and unsettled (found), one dispatched and finished
    // (excluded — its outcome is known), and one that never dispatched (excluded — nothing can have
    // happened).
    let (_database, ledger) = repository().await;

    let mut stranded = entry_with(ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(201)));
    ledger.reserve(&stranded).await.expect("granted");
    drive(&ledger, &mut stranded, ToolCallState::Executing, None).await;

    let mut finished = entry_with(ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(202)));
    finished.key =
        ReservationKey::new(identity(), workspace(), principal(), "finished").expect("valid");
    ledger.reserve(&finished).await.expect("granted");
    drive(
        &ledger,
        &mut finished,
        ToolCallState::Succeeded,
        Some(ToolErrorClass::ProviderError),
    )
    .await;

    let mut untouched = entry_with(ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(203)));
    untouched.key =
        ReservationKey::new(identity(), workspace(), principal(), "untouched").expect("valid");
    ledger.reserve(&untouched).await.expect("granted");

    let found = ledger.possibly_effecting(10).await.expect("the scan runs");
    assert_eq!(
        found.records.len(),
        1,
        "exactly one row was dispatched and left unsettled"
    );
    assert_eq!(found.records[0].id, stranded.id);
    assert!(
        found.records[0].may_have_effected(),
        "a row found by this scan may have effected, which is why it must be reconciled and not retried",
    );
    assert!(
        !found.bounded,
        "the store held nothing beyond this page, so the caller must be told it saw everything",
    );

    // **The bound is honoured, and a full page says so.** Reading exactly the limit cannot distinguish
    // "there are more" from "that was all", which is why the adapter probes with one row more — and for
    // this scan the difference is durable work left unsettled rather than a slightly shorter list.
    let zero = ledger.possibly_effecting(0).await.expect("the scan runs");
    assert_eq!(zero.records.len(), 0);
    assert!(
        zero.bounded,
        "a zero-limit page against a non-empty store must report that more was held",
    );
}

#[tokio::test]
async fn a_stranded_call_is_settled_through_real_sql_by_the_recovery_pass() {
    // **The whole recovery path against a migrated database**, and the one test that proves the pass
    // works with the real adapter rather than only with a double. The in-memory double agrees with
    // whatever the code assumed; only the SQL proves that the conversion predicate, the ordering, and the
    // version guard are the ones a daemon will actually use — and the conversion predicate is the piece
    // that most needs it, because its narrowness (`executing` alone) is what makes the paging terminate.
    let (_database, ledger) = repository().await;
    let mut stranded = entry();
    ledger.reserve(&stranded).await.expect("granted");
    drive(&ledger, &mut stranded, ToolCallState::Executing, None).await;

    let port: Arc<dyn jarvis_application::repository::tool_call::ToolCallRepository> =
        Arc::new((*ledger).clone());
    let report = jarvis_application::tool_recovery::reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs against the real adapter");

    assert_eq!(report.reconciling, 1, "{report:?}");
    assert!(report.is_complete(), "{report:?}");

    let stored = ledger.load(workspace(), stranded.id).await.expect("loads");
    assert_eq!(
        stored.state(),
        ToolCallState::Reconciling,
        "the real adapter must have written the conversion",
    );
    assert!(
        stored.was_dispatched(),
        "the dispatch fact must survive the recovery write, or the retry decision loses its input",
    );

    // And the pass is idempotent through the real predicate: a second run finds nothing to convert,
    // because the conversion input excludes a row already in `reconciling` and the row is now there.
    let again = jarvis_application::tool_recovery::reconcile_tool_calls(&port, later())
        .await
        .expect("the second pass runs");
    assert_eq!(again.changed(), 0, "{again:?}");
    assert!(again.is_complete());
    // The row is still reported as outstanding work, which is the honest description of a call whose
    // outcome only a provider read can establish.
    assert_eq!(
        ledger
            .possibly_effecting(10)
            .await
            .expect("readable")
            .records
            .len(),
        1,
        "a converted call remains work awaiting reconciliation",
    );
    assert_eq!(
        ledger
            .awaiting_conversion(10)
            .await
            .expect("readable")
            .records
            .len(),
        0,
        "**it must no longer be offered for conversion**, or the paging would never terminate",
    );
}

#[tokio::test]
async fn a_stranded_pre_dispatch_reservation_is_settled_so_a_retry_is_not_told_to_wait() {
    // **The harm the pass exists to remove, applied to the five states it could not reach.** A call that
    // claimed its reservation and then lost its daemon still holds the unique index entry — so a retry of
    // that key is answered `InFlight`, i.e. *wait on a process that is gone*. Settling it as `CANCELLED`
    // is what releases the key's *meaning*: the reservation row stays (a reservation is durable), but the
    // state says the call was stopped rather than that somebody is running it.
    //
    // This drives the **real SQL**, not the double, because the whole defect was in the predicate string:
    // the double mirrors whatever set the tests assume, and only the database proves which rows the
    // adapter's `state IN (...)` list actually returns.
    let (_database, ledger) = repository().await;
    let mut reserved = entry();
    ledger.reserve(&reserved).await.expect("granted");
    drive(&ledger, &mut reserved, ToolCallState::Reserved, None).await;

    let port: Arc<dyn jarvis_application::repository::tool_call::ToolCallRepository> =
        Arc::new((*ledger).clone());
    let report = jarvis_application::tool_recovery::reconcile_tool_calls(&port, later())
        .await
        .expect("the pass runs against the real adapter");

    assert_eq!(
        report.cancelled, 1,
        "**the real predicate must offer a pre-dispatch row**: {report:?}",
    );
    assert_eq!(report.reconciling, 0, "{report:?}");

    let stored = ledger.load(workspace(), reserved.id).await.expect("loads");
    assert_eq!(stored.state(), ToolCallState::Cancelled);
    assert_eq!(
        stored.outcome(),
        Some(ToolErrorClass::Cancelled),
        "a terminal row written with no outcome is refused as corruption by this adapter's own reader",
    );
    assert!(
        !stored.was_dispatched(),
        "nothing reached the provider, so no dispatch may be recorded",
    );
}

#[tokio::test]
async fn the_conversion_scan_covers_every_state_the_pass_can_settle() {
    // **A predicate string and a domain enum can drift with nothing to catch it.** `awaiting_conversion`
    // names its states in SQL, while `classify_interrupted` names the states the pass settles in Rust;
    // they are the same set expressed twice, and the first version got it wrong — `executing` alone,
    // which left five settled-able states unreachable and made `report.cancelled` dead. Nothing failed,
    // because both the string and the table compile perfectly on their own.
    //
    // So this asserts the two agree **through the real SQL**: a row is placed in every non-terminal
    // state, and the scan must return exactly those states that need a write. A state added to
    // `ToolCallState` later with no entry in the predicate fails here rather than being silently
    // unreachable at startup.
    use jarvis_domain::tool::ledger::classify_interrupted;

    let (_database, ledger) = repository().await;
    let mut expected: Vec<&'static str> = Vec::new();
    for state in ToolCallState::ALL.iter().copied() {
        let Some(action) = classify_interrupted(state) else {
            continue;
        };
        if !action.needs_write() {
            // `reconciling` — the scan excludes the pass's own output on purpose.
            continue;
        }
        let mut row = entry();
        row.key = ReservationKey::new(
            identity(),
            workspace(),
            principal(),
            state.as_contract_str(),
        )
        .expect("valid");
        row.id = ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(9000 + u128::from(state as u8)));
        ledger.reserve(&row).await.expect("granted");
        if state != ToolCallState::Requested {
            drive(&ledger, &mut row, state, None).await;
        }
        expected.push(state.as_contract_str());
    }

    let found = ledger
        .awaiting_conversion(100)
        .await
        .expect("the scan runs");
    let mut actual: Vec<&str> = found
        .records
        .iter()
        .map(|entry| entry.state().as_contract_str())
        .collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        actual, expected,
        "the conversion predicate must offer exactly the states the classification can settle",
    );
    assert_eq!(
        expected.len(),
        6,
        "six non-terminal states need a write; a change here means the state machine grew or shrank \
         and the predicate must be revisited",
    );
}

#[tokio::test]
async fn the_reconciliation_scan_is_unscoped_and_finds_another_workspaces_stranded_call() {
    // Deliberately unscoped, for the reason `RunRepository::incomplete_runs` records: startup
    // reconciliation is a whole-profile concern, and a scan scoped to one workspace would leave every
    // other workspace's unsettled calls unsettled **with no symptom**. The row here is the proof that a
    // scope filter would lose it.
    let (_database, ledger) = repository().await;
    let mut row = entry();
    row.key = ReservationKey::new(identity(), other_workspace(), principal(), "elsewhere")
        .expect("valid");
    ledger.reserve(&row).await.expect("granted");
    drive(&ledger, &mut row, ToolCallState::Reconciling, None).await;

    let found = ledger.possibly_effecting(10).await.expect("the scan runs");
    assert_eq!(
        found.records.len(),
        1,
        "**an unscoped scan must find it**: a workspace filter here would hide a stranded effect, which is                          the failure with no symptom",
    );
    assert_eq!(found.records[0].key.workspace, other_workspace());
}

// ---------------------------------------------------------------------------------------
// The reservation: the property this adapter exists for.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_first_reservation_is_granted_and_a_second_is_not() {
    // The base case, and the assertion is on the **outcome** rather than on "it returned an error",
    // because a duplicate is a verdict: the second caller must learn that its intent was already
    // satisfied, not that its request was malformed.
    let (_database, ledger) = repository().await;
    let first = entry();
    assert_eq!(
        ledger.reserve(&first).await.expect("the store answers"),
        ReservationOutcome::Granted,
    );

    // A *different* row with the same key is what a retry looks like: the caller built a fresh attempt.
    let second = entry();
    assert_ne!(second.id, first.id, "a retry is a different row");
    let outcome = ledger.reserve(&second).await.expect("the store answers");
    assert!(
        !outcome.is_granted(),
        "the second reservation must not be granted, or the call is dispatched twice: {outcome:?}",
    );
    assert_eq!(
        outcome,
        ReservationOutcome::InFlight {
            state: ToolCallState::Requested
        },
        "a reserved-but-undispatched row means somebody holds it",
    );
}

#[tokio::test]
async fn two_connections_racing_the_same_key_produce_one_grant() {
    // **The reason this adapter exists.** Two connections to one *file* are two independent SQLite
    // sessions, which is the closest reproduction of two processes on one host. The in-memory ledger
    // cannot see the other session's row, and the whole point of persisting the key is that the store can.
    //
    // A version of this test using two `SqliteToolCallRepository` values over one pool would pass for a
    // read-then-decide-then-insert adapter, so it would not test the property at all — the pool hands out
    // connections from one set of views, and the two reserves would serialize behind one another.
    let directory =
        std::env::temp_dir().join(format!("jarvis-ledger-race-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&directory).expect("the temp directory is creatable");
    let path = directory.join("jarvis.sqlite");

    {
        let database = Database::open(&path)
            .await
            .expect("the file database opens");
        migrate::run(database.pool()).await.expect("migrates");
        let runs = SqliteRepositories::new(database.pool().clone());
        seed(&runs).await;
    }

    // Two separate opens, so two separate sessions over the same file.
    let left = Database::open(&path).await.expect("left opens");
    let right = Database::open(&path).await.expect("right opens");
    let left_repository = SqliteToolCallRepository::new(left.pool().clone());
    let right_repository = SqliteToolCallRepository::new(right.pool().clone());

    let left_outcome = left_repository
        .reserve(&entry())
        .await
        .expect("the left store answers");
    let right_outcome = right_repository
        .reserve(&entry())
        .await
        .expect("the right store answers");

    assert!(
        left_outcome.is_granted(),
        "the first connection must win: {left_outcome:?}",
    );
    assert!(
        !right_outcome.is_granted(),
        "**the second connection must not be granted**, which is the property an in-memory ledger                          cannot provide: {right_outcome:?}",
    );
    assert_eq!(
        right_outcome,
        ReservationOutcome::InFlight {
            state: ToolCallState::Requested
        },
    );

    // And exactly one row exists, which is what makes the duplicate impossible rather than merely
    // reported — a second row would let a later reader pick the wrong one.
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM tool_call_records")
        .fetch_one(left.pool())
        .await
        .expect("the catalog is readable");
    assert_eq!(rows, 1, "one reservation means one row");

    drop(left);
    drop(right);
    std::fs::remove_dir_all(&directory).ok();
}

#[tokio::test]
async fn equal_identities_serialize_identically_so_the_unique_index_can_work() {
    // **An assumption a uniqueness guarantee rests on, asserted rather than believed.** The unique index
    // is over the serialized identity *text*, so two equal identities that produced different documents
    // would both insert and the reservation would silently stop deduplicating — while the index still
    // existed, which is the "exists but enforced nowhere" shape. `ToolIdentity` is a struct of fields
    // serde writes in declaration order, so this holds; a field that was a map or a set would break it.
    let first = serde_json::to_string(&identity()).expect("the identity serializes");
    let second = serde_json::to_string(&identity()).expect("the identity serializes");
    assert_eq!(
        first, second,
        "equal identities must produce byte-identical documents, or the unique index stops deduplicating",
    );

    // And the store enforces it: two rows built from independently constructed equal identities collide.
    let (_database, ledger) = repository().await;
    let build = |record: u128| {
        let mut entry = LedgerEntry::reserve(
            tool_call(),
            1,
            run_id(),
            ReservationKey::new(identity(), workspace(), principal(), "send-once")
                .expect("a valid key"),
            LedgerOperation::Execute,
            now(),
        );
        entry.id = ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(record));
        entry
    };
    assert_eq!(
        ledger.reserve(&build(1)).await.expect("answers"),
        ReservationOutcome::Granted,
    );
    assert!(
        !ledger
            .reserve(&build(2))
            .await
            .expect("answers")
            .is_granted(),
        "two independently built equal identities must collide in the store",
    );
}

#[tokio::test]
async fn a_second_attempt_at_one_reservation_key_is_refused_rather_than_stored_as_another_row() {
    // **The claim this test exists to disprove, in three documents.** `000009_tool_call_ledger.sql`,
    // `docs/data/schema.md`, and `jarvis_domain::tool::ledger` all describe the ledger as "one row per
    // *attempt*", where "an invocation that is retried has several rows sharing one reservation key,
    // and the attempt number is what distinguishes them". The unique index says otherwise: it is
    // `(workspace_id, principal_id, tool_identity_json, idempotency_key)`, so a second row for the same
    // key is refused **whatever** its `attempt` column holds — and that refusal is the reservation
    // guarantee, because two rows for one key would let two processes each dispatch.
    //
    // So the two are asserted together, and it is the pair that decides which document is right: the
    // same-key second attempt is *not* a second row, and the reservation is the thing that says so.
    let (database, ledger) = repository().await;

    let first = entry();
    assert_eq!(
        ledger.reserve(&first).await.expect("answers"),
        ReservationOutcome::Granted,
    );

    // The same key, a different record identifier, and **attempt 2** — which is exactly the row the
    // "several rows sharing one key" reading predicts would insert cleanly.
    let mut second = LedgerEntry::reserve(
        tool_call(),
        2,
        run_id(),
        key(),
        LedgerOperation::Execute,
        now(),
    );
    second.id = ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(2));
    let outcome = ledger.reserve(&second).await.expect("answers");
    assert!(
        !outcome.is_granted(),
        "**a second attempt at one key must not be a second row**: if it were, two processes could \
         both hold the same reservation and both dispatch — got {outcome:?}",
    );
    assert_eq!(
        outcome,
        ReservationOutcome::InFlight {
            state: ToolCallState::Requested
        },
        "the stored row is offered back to the retrying caller rather than duplicated",
    );

    // And exactly one row is stored, which is the difference between "reported as a duplicate" and
    // "made impossible". A count is the assertion, because a second row that a later reader might pick
    // is the defect the unique index exists to prevent.
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM tool_call_records")
        .fetch_one(database.pool())
        .await
        .expect("the catalog is readable");
    assert_eq!(
        rows, 1,
        "one reservation key means one row, whatever attempt number is presented",
    );
}

#[tokio::test]
async fn a_reservation_key_scoped_by_fewer_dimensions_would_collide_and_the_full_key_does_not() {
    // Four keys that differ in exactly one dimension each. Every one must be granted, because a key
    // naming fewer dimensions would let one scope's row answer another's question — and here the
    // consequence is a second side effect rather than a leaked identifier.
    let (_database, ledger) = repository().await;
    let variants = [
        ReservationKey::new(identity(), workspace(), principal(), "send-once").expect("valid"),
        ReservationKey::new(identity(), workspace(), principal(), "send-twice").expect("valid"),
        ReservationKey::new(identity(), workspace(), other_principal(), "send-once")
            .expect("valid"),
        ReservationKey::new(identity(), other_workspace(), principal(), "send-once")
            .expect("valid"),
    ];
    for (index, variant) in variants.into_iter().enumerate() {
        let mut row = LedgerEntry::reserve(
            tool_call(),
            1,
            run_id(),
            variant,
            LedgerOperation::Execute,
            now(),
        );
        row.id = ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(100 + index as u128));
        assert_eq!(
            ledger.reserve(&row).await.expect("answers"),
            ReservationOutcome::Granted,
            "variant {index} differs from the others in one dimension and must be its own reservation",
        );
    }
}

#[tokio::test]
async fn a_settled_duplicate_reports_the_recorded_outcome_rather_than_refusing() {
    // A finished call is the one duplicate shape a caller may act on: the effect is recorded, so the
    // right response is to **read the answer**. The variant carries the state, the class, and the
    // no-effect claim, because a caller re-asking has to know whether the first call worked.
    let (_database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    drive(
        &ledger,
        &mut row,
        ToolCallState::Succeeded,
        Some(ToolErrorClass::Timeout),
    )
    .await;

    let outcome = ledger.reserve(&entry()).await.expect("answers");
    assert_eq!(
        outcome,
        ReservationOutcome::AlreadyTerminal {
            state: ToolCallState::Succeeded,
            outcome: Some(ToolErrorClass::Timeout),
            no_effect_confirmed: false,
        },
        "a settled duplicate must carry the recorded outcome, so the caller can read it",
    );
    assert!(
        !outcome.permits_dispatch(),
        "a dispatched call that succeeded without a no-effect proof must not be retried",
    );
}

#[tokio::test]
async fn an_unsettled_duplicate_asks_for_reconciliation_and_names_the_row() {
    // **The one duplicate shape whose right answer is "do not touch it".** A reconciling row has an
    // unknown outcome, so the caller must establish it rather than retry — and it needs the row to do
    // that, which is why the variant carries the identifier rather than only the state.
    let (_database, ledger) = repository().await;
    let mut row = entry();
    ledger.reserve(&row).await.expect("granted");
    drive(&ledger, &mut row, ToolCallState::Reconciling, None).await;

    let outcome = ledger.reserve(&entry()).await.expect("answers");
    assert_eq!(
        outcome,
        ReservationOutcome::Unsettled { row: row.id },
        "an unknown outcome must be reported as unsettled and name the row to read",
    );
    assert!(
        !outcome.permits_dispatch(),
        "an unsettled call must never be retried, even though it might never have happened",
    );
}

#[tokio::test]
async fn a_parked_run_with_a_resume_record_keeps_its_waiting_call_and_its_own_run_row() {
    // **Restart must not undo a pending approval.** Both recovery reads are predicates over real SQL, so
    // only the database proves them: a waiting call whose run has a resume record is not offered to the
    // tool pass, and the parked run itself is not offered to the run pass. Without the record, both are.
    use jarvis_application::repository::resume::{
        RESUME_RECORD_VERSION, ResumeRecord, RunResumeRepository as _,
    };
    use jarvis_application::repository::run::{RunRepository as _, RunWrite, WaitingOn};
    use jarvis_domain::run::lifecycle::RunTransition;
    use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};

    let (database, ledger) = repository().await;
    let runs = SqliteRepositories::new(database.pool().clone());
    let resumes =
        crate::storage::resume_repository::SqliteResumeRepository::new(database.pool().clone());

    let mut waiting = entry();
    ledger.reserve(&waiting).await.expect("granted");
    drive(&ledger, &mut waiting, ToolCallState::WaitingApproval, None).await;

    let path = [
        (RunState::Received, RunState::ContextBuilding),
        (RunState::ContextBuilding, RunState::Planning),
        (RunState::Planning, RunState::AwaitingModel),
        (RunState::AwaitingModel, RunState::ExecutingTool),
        (RunState::ExecutingTool, RunState::AwaitingApproval),
    ];
    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = RunTransition::new(
            *from,
            *to,
            version,
            TransitionActor::Controller,
            TransitionReason::new("step").expect("valid"),
            now(),
        );
        let event = jarvis_application::repository::run::NewActivityEvent {
            run_id: run_id(),
            sequence: index as u64 + 2,
            event_type: "run.step".to_owned(),
            payload_json: None,
            visibility: jarvis_application::repository::run::EventVisibility::Public,
            occurred_at: now(),
        };
        let mut write = RunWrite::new(&transition, event);
        if *to == RunState::AwaitingApproval {
            write = write.waiting_on(WaitingOn::new("approval", "a-1").expect("valid"));
        }
        version = runs
            .transition(workspace(), write)
            .await
            .expect("legal")
            .version;
    }

    let record = ResumeRecord {
        version: RESUME_RECORD_VERSION,
        run: run_id(),
        workspace: workspace(),
        conversation: jarvis_domain::ids::ConversationId::from_uuid(uuid::Uuid::from_u128(0x50)),
        approval: jarvis_domain::ids::ApprovalId::from_uuid(uuid::Uuid::from_u128(0x51)),
        turn_index: 1,
        objective: "x".to_owned(),
        objective_message: None,
        calls: Vec::new(),
        waiting_index: 0,
        settled: Vec::new(),
    };
    resumes.save(&record).await.expect("saves");
    assert_eq!(resumes.parked().await.expect("reads"), vec![record.clone()]);

    let with_record = runs.incomplete_runs().await.expect("reads");
    assert!(
        with_record.runs.is_empty(),
        "a parked run with a record is waiting, not interrupted"
    );
    let tool_scan = ledger.awaiting_conversion(10).await.expect("reads");
    assert!(
        tool_scan.records.is_empty(),
        "its waiting call is not stranded"
    );

    resumes
        .discard(workspace(), run_id())
        .await
        .expect("discards");
    assert_eq!(runs.incomplete_runs().await.expect("reads").runs.len(), 1);
    assert_eq!(
        ledger
            .awaiting_conversion(10)
            .await
            .expect("reads")
            .records
            .len(),
        1
    );
}
