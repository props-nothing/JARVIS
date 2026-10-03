//! Contract tests for the SQLite resume store, against a real migrated database.

use jarvis_application::repository::RepositoryError;
use jarvis_application::repository::resume::{
    RESUME_RECORD_VERSION, ResumeCall, ResumeObservation, ResumeRecord, RunResumeRepository,
};
use jarvis_domain::ids::{ApprovalId, ConversationId, RunId, WorkspaceId};

use super::SqliteResumeRepository;
use crate::storage::connection::Database;
use crate::storage::migrate;

async fn database() -> Database {
    let database = Database::open_in_memory().await.expect("in-memory opens");
    migrate::run(database.pool()).await.expect("migrates");
    database
}

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

fn record(workspace: u128, run: u128, objective: &str) -> ResumeRecord {
    ResumeRecord {
        version: RESUME_RECORD_VERSION,
        run: RunId::from_uuid(id(run)),
        workspace: WorkspaceId::from_uuid(id(workspace)),
        conversation: ConversationId::from_uuid(id(7)),
        approval: ApprovalId::from_uuid(id(8)),
        turn_index: 2,
        objective: objective.to_owned(),
        objective_message: None,
        calls: vec![ResumeCall {
            call_id: "call-1".to_owned(),
            capability: "files.read@1".to_owned(),
            arguments: r#"{"path":"/tmp"}"#.to_owned(),
        }],
        waiting_index: 0,
        settled: Vec::<ResumeObservation>::new(),
    }
}

#[tokio::test]
async fn a_record_round_trips_and_a_save_replaces_the_previous_one() {
    let database = database().await;
    let store = SqliteResumeRepository::new(database.pool().clone());
    let first = record(1, 2, "first objective");
    store.save(&first).await.expect("saves");
    assert_eq!(
        store.load(first.workspace, first.run).await.expect("loads"),
        Some(first.clone())
    );

    let second = record(1, 2, "second objective");
    store.save(&second).await.expect("replaces");
    assert_eq!(
        store
            .load(second.workspace, second.run)
            .await
            .expect("loads"),
        Some(second),
        "a parked run has one current record"
    );
}

#[tokio::test]
async fn a_foreign_workspace_reads_as_absent_and_cannot_discard() {
    let database = database().await;
    let store = SqliteResumeRepository::new(database.pool().clone());
    let mine = record(1, 2, "mine");
    store.save(&mine).await.expect("saves");

    let foreign = WorkspaceId::from_uuid(id(99));
    assert_eq!(store.load(foreign, mine.run).await.expect("loads"), None);
    store.discard(foreign, mine.run).await.expect("a no-op");
    assert!(
        store
            .load(mine.workspace, mine.run)
            .await
            .expect("loads")
            .is_some()
    );

    store
        .discard(mine.workspace, mine.run)
        .await
        .expect("discards");
    assert_eq!(
        store.load(mine.workspace, mine.run).await.expect("loads"),
        None
    );
    store
        .discard(mine.workspace, mine.run)
        .await
        .expect("discarding twice is not an error");
}

#[tokio::test]
async fn an_oversized_record_is_refused_rather_than_stored() {
    let database = database().await;
    let store = SqliteResumeRepository::new(database.pool().clone());
    let huge = record(1, 2, &"x".repeat(1_100_000));
    let error = store.save(&huge).await.expect_err("too large");
    assert!(
        matches!(error, RepositoryError::Conflict { .. }),
        "{error:?}"
    );
    assert_eq!(
        store.load(huge.workspace, huge.run).await.expect("loads"),
        None
    );
}

#[tokio::test]
async fn a_record_of_an_unknown_version_is_corruption_not_a_guess() {
    let database = database().await;
    let pool = database.pool().clone();
    let store = SqliteResumeRepository::new(pool.clone());
    let mut stored = record(1, 2, "objective");
    stored.version = 99;
    let json = serde_json::to_string(&stored).expect("serializes");
    sqlx::query(
        "INSERT INTO run_resume_states (workspace_id, run_id, approval_id, state_version, \
         state_json, created_at) VALUES (?, ?, ?, 99, ?, '2026-01-01T00:00:00Z')",
    )
    .bind(stored.workspace.to_string())
    .bind(stored.run.to_string())
    .bind(stored.approval.to_string())
    .bind(json)
    .execute(&pool)
    .await
    .expect("inserts");
    let error = store
        .load(stored.workspace, stored.run)
        .await
        .expect_err("unknown version");
    assert!(
        matches!(error, RepositoryError::Corrupted { .. }),
        "{error:?}"
    );
}
