//! Contract tests for the SQLite memory store, against a real migrated database.
//!
//! The properties that matter are the database's own: a unique index that must make an exact duplicate a
//! no-op, a `workspace_id` in every statement, and a hard delete that must remove the text. A fake would agree
//! with whatever the code assumed about them.

use std::sync::Arc;

use super::SqliteMemoryRepository;
use crate::storage::connection::Database;
use crate::storage::migrate;
use jarvis_application::repository::RepositoryError;
use jarvis_application::repository::memory::{MAX_SEARCH_SCAN, MemoryRepository, RememberOutcome};
use jarvis_domain::ids::{MemoryId, PrincipalId, WorkspaceId};
use jarvis_domain::memory::{Memory, MemoryClass, MemorySource, MemoryText};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::time::UtcTimestamp;

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(0x1000))
}

fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(0x1001))
}

fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(0x2000))
}

/// A memory whose identity and creation instant derive from `n`, so ordering is controlled by the test.
fn memory(n: u128, workspace: WorkspaceId, text: &str) -> Memory {
    let created = UtcTimestamp::parse(&format!("2026-10-03T12:{:02}:00Z", n % 60)).expect("valid");
    Memory {
        id: MemoryId::from_uuid(uuid::Uuid::from_u128(0x3000 + n)),
        workspace,
        class: MemoryClass::Preference,
        text: MemoryText::new(text).expect("valid"),
        sensitivity: Sensitivity::Internal,
        source: MemorySource::UserRequest {
            principal: principal(),
        },
        created_at: created,
        updated_at: created,
    }
}

async fn repository() -> (Database, Arc<SqliteMemoryRepository>) {
    let database = Database::open_in_memory().await.expect("in-memory opens");
    migrate::run(database.pool()).await.expect("migrates");
    let store = Arc::new(SqliteMemoryRepository::new(database.pool().clone()));
    (database, store)
}

#[tokio::test]
async fn a_remembered_memory_reads_back_exactly() {
    let (_database, store) = repository().await;
    let original = memory(1, workspace(), "Client proposals should be concise");
    assert_eq!(
        store.remember(&original).await.expect("stores"),
        RememberOutcome::Created
    );
    let loaded = store.load(workspace(), original.id).await.expect("loads");
    assert_eq!(
        loaded, original,
        "every field, including provenance, survives storage"
    );
}

#[tokio::test]
async fn an_exact_duplicate_is_a_no_op_that_names_the_existing_memory() {
    let (_database, store) = repository().await;
    let first = memory(1, workspace(), "Client proposals should be concise");
    store.remember(&first).await.expect("stores");

    // Different identity, different spelling of the same claim.
    let second = memory(2, workspace(), "client   PROPOSALS should be concise");
    assert_eq!(
        store.remember(&second).await.expect("stores"),
        RememberOutcome::AlreadyKnown(first.id)
    );
    assert_eq!(
        store
            .list(workspace(), 10)
            .await
            .expect("lists")
            .memories
            .len(),
        1,
        "a duplicate must not create a second row"
    );
    assert_eq!(
        store.load(workspace(), second.id).await.err(),
        Some(RepositoryError::NotFound),
        "and the rejected duplicate's identity must not exist"
    );
}

#[tokio::test]
async fn identical_memories_in_two_workspaces_never_cross() {
    // ACC-032 at the store: the same text, twice, in two workspaces is two memories, and each workspace sees
    // only its own through list, search, load, and forget.
    let (_database, store) = repository().await;
    let mine = memory(1, workspace(), "Prefers concise client proposals");
    let theirs = memory(2, other_workspace(), "Prefers concise client proposals");
    store.remember(&mine).await.expect("stores");
    assert_eq!(
        store.remember(&theirs).await.expect("stores"),
        RememberOutcome::Created,
        "another workspace's identical claim is not a duplicate"
    );

    for (owner, own, foreign) in [
        (workspace(), mine.id, theirs.id),
        (other_workspace(), theirs.id, mine.id),
    ] {
        let listed = store.list(owner, 10).await.expect("lists").memories;
        assert_eq!(listed.iter().map(|m| m.id).collect::<Vec<_>>(), vec![own]);
        let found = store
            .search(owner, "concise proposals", 10)
            .await
            .expect("searches");
        assert_eq!(
            found.hits.iter().map(|h| h.memory.id).collect::<Vec<_>>(),
            vec![own]
        );
        assert_eq!(
            store.load(owner, foreign).await.err(),
            Some(RepositoryError::NotFound),
            "a foreign memory reads as absent"
        );
        assert_eq!(
            store.forget(owner, foreign).await.err(),
            Some(RepositoryError::NotFound),
            "and cannot be deleted by naming it"
        );
    }
    // Neither delete attempt removed anything.
    assert!(store.load(workspace(), mine.id).await.is_ok());
    assert!(store.load(other_workspace(), theirs.id).await.is_ok());
}

#[tokio::test]
async fn search_ranks_by_relevance_then_recency_and_returns_nothing_for_no_match() {
    let (_database, store) = repository().await;
    // Oldest, full match.
    store
        .remember(&memory(
            1,
            workspace(),
            "client proposals should be concise",
        ))
        .await
        .expect("stores");
    // Newer, full match: wins the tie on recency.
    store
        .remember(&memory(
            2,
            workspace(),
            "always keep client proposals short",
        ))
        .await
        .expect("stores");
    // Newest, half match.
    store
        .remember(&memory(3, workspace(), "client invoices are paid monthly"))
        .await
        .expect("stores");
    // No match at all.
    store
        .remember(&memory(4, workspace(), "the office is in Utrecht"))
        .await
        .expect("stores");

    let found = store
        .search(workspace(), "client proposals", 10)
        .await
        .expect("searches");
    let order: Vec<_> = found
        .hits
        .iter()
        .map(|hit| (hit.memory.id, hit.relevance))
        .collect();
    assert_eq!(
        order,
        vec![
            (MemoryId::from_uuid(uuid::Uuid::from_u128(0x3002)), 1_000),
            (MemoryId::from_uuid(uuid::Uuid::from_u128(0x3001)), 1_000),
            (MemoryId::from_uuid(uuid::Uuid::from_u128(0x3003)), 500),
        ],
        "relevance first, then the newer memory, and the non-matching one is absent"
    );
    assert!(!found.scan_bounded);

    assert!(
        store
            .search(workspace(), "weather forecast", 10)
            .await
            .expect("searches")
            .hits
            .is_empty()
    );
    // The limit applies after ranking.
    assert_eq!(
        store
            .search(workspace(), "client proposals", 1)
            .await
            .expect("searches")
            .hits
            .len(),
        1
    );
}

#[tokio::test]
async fn a_forgotten_memory_is_gone_from_every_read_and_its_text_is_removed() {
    let (database, store) = repository().await;
    let doomed = memory(1, workspace(), "my locker code is 4821");
    store.remember(&doomed).await.expect("stores");
    store.forget(workspace(), doomed.id).await.expect("forgets");

    assert_eq!(
        store.load(workspace(), doomed.id).await.err(),
        Some(RepositoryError::NotFound)
    );
    assert!(
        store
            .list(workspace(), 10)
            .await
            .expect("lists")
            .memories
            .is_empty()
    );
    assert!(
        store
            .search(workspace(), "locker code", 10)
            .await
            .expect("searches")
            .hits
            .is_empty()
    );
    // And not merely hidden: no row holds the text.
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM memories WHERE canonical_text LIKE '%4821%'")
            .fetch_one(database.pool())
            .await
            .expect("counts");
    assert_eq!(
        remaining, 0,
        "forgetting must remove the text, not tombstone it"
    );
    // The claim can be remembered afresh, because the dedupe key went with the row.
    assert_eq!(
        store
            .remember(&memory(2, workspace(), "my locker code is 4821"))
            .await
            .expect("stores"),
        RememberOutcome::Created
    );
    // Forgetting twice reports the second as absent.
    assert_eq!(
        store.forget(workspace(), doomed.id).await.err(),
        Some(RepositoryError::NotFound)
    );
}

#[tokio::test]
async fn a_listing_is_newest_first_and_says_when_it_stopped_short() {
    let (_database, store) = repository().await;
    for n in 1..=3 {
        store
            .remember(&memory(
                n,
                workspace(),
                &format!("memory number {n} about nothing"),
            ))
            .await
            .expect("stores");
    }
    let page = store.list(workspace(), 2).await.expect("lists");
    assert_eq!(
        page.memories.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![
            MemoryId::from_uuid(uuid::Uuid::from_u128(0x3003)),
            MemoryId::from_uuid(uuid::Uuid::from_u128(0x3002)),
        ]
    );
    assert!(page.bounded, "one memory remained, so the page must say so");
    assert!(!store.list(workspace(), 3).await.expect("lists").bounded);
}

#[tokio::test]
async fn a_row_the_domain_cannot_interpret_is_corruption_not_absence() {
    let (database, store) = repository().await;
    let good = memory(1, workspace(), "a perfectly good memory");
    store.remember(&good).await.expect("stores");
    // A sensitivity label JARVIS never writes.
    sqlx::query("UPDATE memories SET sensitivity = 'top-secret-ish' WHERE id = ?")
        .bind(good.id.to_string())
        .execute(database.pool())
        .await
        .expect("corrupts");
    assert_eq!(
        store.load(workspace(), good.id).await.err(),
        Some(RepositoryError::Corrupted {
            column: "sensitivity"
        })
    );
}

#[tokio::test]
async fn the_database_refuses_a_class_or_source_this_build_does_not_write() {
    let (database, _store) = repository().await;
    let attempt = |class: &'static str, source: &'static str| {
        let pool = database.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO memories (id, workspace_id, memory_class, canonical_text, dedupe_key, \
                 sensitivity, source_kind, source_principal_id, created_at, updated_at) \
                 VALUES ('x', 'w', ?, 't', 'k', 'internal', ?, 'p', 'c', 'u')",
            )
            .bind(class)
            .bind(source)
            .execute(&pool)
            .await
        }
    };
    assert!(attempt("preference", "user_request").await.is_ok());
    // A model-inferred source has no place in this schema, so it cannot be written even by a buggy caller.
    sqlx::query("DELETE FROM memories")
        .execute(database.pool())
        .await
        .expect("clears");
    assert!(attempt("preference", "model_inferred").await.is_err());
    assert!(attempt("procedural", "user_request").await.is_err());
}

#[tokio::test]
async fn a_search_over_more_memories_than_the_scan_bound_reports_it() {
    let (database, store) = repository().await;
    // Inserted directly: the bound is the store's, and a thousand round trips through `remember` would only
    // slow the test.
    let mut transaction = database.pool().begin().await.expect("begins");
    for n in 0..=MAX_SEARCH_SCAN {
        sqlx::query(
            "INSERT INTO memories (id, workspace_id, memory_class, canonical_text, dedupe_key, \
             sensitivity, source_kind, source_principal_id, created_at, updated_at) \
             VALUES (?, ?, 'semantic', ?, ?, 'internal', 'user_request', ?, ?, ?)",
        )
        .bind(MemoryId::from_uuid(uuid::Uuid::from_u128(0x9000 + u128::from(n))).to_string())
        .bind(workspace().to_string())
        .bind(format!("filler note {n}"))
        .bind(format!("filler note {n}"))
        .bind(principal().to_string())
        .bind("2026-10-03T12:00:00Z")
        .bind("2026-10-03T12:00:00Z")
        .execute(&mut *transaction)
        .await
        .expect("inserts");
    }
    transaction.commit().await.expect("commits");
    let found = store
        .search(workspace(), "filler", 5)
        .await
        .expect("searches");
    assert!(
        found.scan_bounded,
        "more than the bound exists, and the search must say it did not see them all"
    );
    assert_eq!(found.hits.len(), 5);
}
