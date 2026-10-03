//! The SQLite implementation of the memory store.
//!
//! This is the join between `000013_memories.sql` and [`MemoryRepository`]. Three rules are enforced by the
//! SQL rather than trusted to a caller:
//!
//! - **The workspace is in every statement.** There is no query here without `workspace_id = ?`, so a memory
//!   of another workspace can be neither read, ranked, nor deleted by naming its identifier — and a missing
//!   row and a foreign row are the same `NotFound`.
//! - **Uniqueness is the store's decision.** `remember` is one `INSERT ... ON CONFLICT DO NOTHING`; whether
//!   it inserted is read from the statement's own result, never from a prior `SELECT`, so two concurrent
//!   identical requests cannot both create.
//! - **Forgetting removes the row.** The text is gone from the database, which is what the deletion
//!   acceptance scenario requires. SQLite's own free-page reuse and the write-ahead log are the residual, and
//!   are the storage layer's concern (backup and retention), not something this adapter can claim to close.
//!
//! Ranking is done in Rust over a bounded newest-first candidate set, using the domain's pure
//! `lexical_relevance`, so the order is reproducible and the SQL carries no ranking logic that could disagree
//! with the domain's.

use sqlx::SqlitePool;

use jarvis_application::repository::memory::{
    MAX_SEARCH_SCAN, MemoryHit, MemoryPage, MemoryRepository, MemorySearch, RememberOutcome,
};
use jarvis_application::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{MemoryId, PrincipalId, WorkspaceId};
use jarvis_domain::memory::{Memory, MemoryClass, MemorySource, MemoryText, lexical_relevance};

use super::repositories::{parse_time, text};

/// The column list every `SELECT` shares, written once as a literal so each statement stays auditable.
macro_rules! memory_columns {
    () => {
        "id, workspace_id, memory_class, canonical_text, sensitivity, source_kind, \
         source_principal_id, created_at, updated_at"
    };
}

const LOAD_SQL: &str = concat!(
    "SELECT ",
    memory_columns!(),
    " FROM memories WHERE workspace_id = ? AND id = ?"
);

const NEWEST_SQL: &str = concat!(
    "SELECT ",
    memory_columns!(),
    " FROM memories WHERE workspace_id = ? ORDER BY created_at DESC, id ASC LIMIT ?"
);

/// The SQLite-backed memory store.
#[derive(Debug, Clone)]
pub struct SqliteMemoryRepository {
    pool: SqlitePool,
}

impl SqliteMemoryRepository {
    /// Builds the store over an existing pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl MemoryRepository for SqliteMemoryRepository {
    fn remember(&self, memory: &Memory) -> RepositoryFuture<'_, RememberOutcome> {
        let memory = memory.clone();
        Box::pin(async move {
            let key = memory.text.dedupe_key();
            let inserted = sqlx::query(
                "INSERT INTO memories (id, workspace_id, memory_class, canonical_text, dedupe_key, \
                 sensitivity, source_kind, source_principal_id, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (workspace_id, dedupe_key) DO NOTHING",
            )
            .bind(memory.id.to_string())
            .bind(memory.workspace.to_string())
            .bind(memory.class.as_str())
            .bind(memory.text.as_str())
            .bind(&key)
            .bind(memory.sensitivity.as_str())
            .bind(memory.source.kind())
            .bind(memory.source.principal().to_string())
            .bind(memory.created_at.to_string())
            .bind(memory.updated_at.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            if inserted.rows_affected() == 1 {
                return Ok(RememberOutcome::Created);
            }
            // The statement did nothing, so the claim already exists in this workspace.
            let row =
                sqlx::query("SELECT id FROM memories WHERE workspace_id = ? AND dedupe_key = ?")
                    .bind(memory.workspace.to_string())
                    .bind(&key)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|_| RepositoryError::Query)?
                    .ok_or(RepositoryError::Query)?;
            let id = MemoryId::parse(&text(&row, "id")?)
                .map_err(|_| RepositoryError::Corrupted { column: "id" })?;
            Ok(RememberOutcome::AlreadyKnown(id))
        })
    }

    fn load(&self, workspace: WorkspaceId, id: MemoryId) -> RepositoryFuture<'_, Memory> {
        Box::pin(async move {
            let row = sqlx::query(LOAD_SQL)
                .bind(workspace.to_string())
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?
                .ok_or(RepositoryError::NotFound)?;
            stored_memory(&row)
        })
    }

    fn list(&self, workspace: WorkspaceId, limit: u32) -> RepositoryFuture<'_, MemoryPage> {
        Box::pin(async move {
            // One more than asked, so "there is more" is observed rather than inferred from a full page.
            let rows = sqlx::query(NEWEST_SQL)
                .bind(workspace.to_string())
                .bind(i64::from(limit) + 1)
                .fetch_all(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            let bounded = rows.len() > limit as usize;
            let memories = rows
                .iter()
                .take(limit as usize)
                .map(stored_memory)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(MemoryPage { memories, bounded })
        })
    }

    fn search(
        &self,
        workspace: WorkspaceId,
        query: &str,
        limit: u32,
    ) -> RepositoryFuture<'_, MemorySearch> {
        let query = query.to_owned();
        Box::pin(async move {
            let rows = sqlx::query(NEWEST_SQL)
                .bind(workspace.to_string())
                .bind(i64::from(MAX_SEARCH_SCAN) + 1)
                .fetch_all(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            let scan_bounded = rows.len() > MAX_SEARCH_SCAN as usize;
            let mut hits = Vec::new();
            for row in rows.iter().take(MAX_SEARCH_SCAN as usize) {
                let memory = stored_memory(row)?;
                if let Some(relevance) = lexical_relevance(&query, memory.text.as_str()) {
                    hits.push(MemoryHit { memory, relevance });
                }
            }
            // A total order: relevance, then recency, then identity. Rows arrive newest-first, but the sort is
            // explicit so the order does not depend on an incidental property of the query.
            hits.sort_by(|left, right| {
                right
                    .relevance
                    .cmp(&left.relevance)
                    .then_with(|| right.memory.created_at.cmp(&left.memory.created_at))
                    .then_with(|| left.memory.id.cmp(&right.memory.id))
            });
            hits.truncate(limit as usize);
            Ok(MemorySearch { hits, scan_bounded })
        })
    }

    fn forget(&self, workspace: WorkspaceId, id: MemoryId) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            let deleted = sqlx::query("DELETE FROM memories WHERE workspace_id = ? AND id = ?")
                .bind(workspace.to_string())
                .bind(id.to_string())
                .execute(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            if deleted.rows_affected() == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
    }
}

/// Builds a [`Memory`] from a row, refusing anything JARVIS could not have written.
fn stored_memory(row: &sqlx::sqlite::SqliteRow) -> Result<Memory, RepositoryError> {
    let corrupted = |column: &'static str| RepositoryError::Corrupted { column };
    let id = MemoryId::parse(&text(row, "id")?).map_err(|_| corrupted("id"))?;
    let workspace =
        WorkspaceId::parse(&text(row, "workspace_id")?).map_err(|_| corrupted("workspace_id"))?;
    let class =
        MemoryClass::parse(&text(row, "memory_class")?).map_err(|_| corrupted("memory_class"))?;
    let memory_text =
        MemoryText::new(&text(row, "canonical_text")?).map_err(|_| corrupted("canonical_text"))?;
    let sensitivity =
        jarvis_application::context_assembly::parse_sensitivity(&text(row, "sensitivity")?)
            .ok_or_else(|| corrupted("sensitivity"))?;
    let source = match text(row, "source_kind")?.as_str() {
        "user_request" => MemorySource::UserRequest {
            principal: PrincipalId::parse(&text(row, "source_principal_id")?)
                .map_err(|_| corrupted("source_principal_id"))?,
        },
        _ => return Err(corrupted("source_kind")),
    };
    Ok(Memory {
        id,
        workspace,
        class,
        text: memory_text,
        sensitivity,
        source,
        created_at: parse_time(&text(row, "created_at")?, "created_at")?,
        updated_at: parse_time(&text(row, "updated_at")?, "updated_at")?,
    })
}

#[cfg(test)]
#[path = "memory_repository_tests.rs"]
mod tests;
