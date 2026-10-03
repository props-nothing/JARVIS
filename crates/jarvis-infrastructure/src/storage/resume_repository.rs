//! The SQLite implementation of the run resume store.
//!
//! The join between `000014_run_resume_states.sql` and [`RunResumeRepository`]. The record is stored as
//! one JSON document beside the columns a query needs, with its own version: the shape of a parked
//! batch is the controller's to evolve, and a reader that meets a version it does not know reports
//! corruption rather than guessing at fields.
//!
//! Every statement names the workspace, so a record of another workspace reads as absent.

use sqlx::SqlitePool;

use jarvis_application::repository::resume::{
    MAX_PARKED_SCAN, MAX_RESUME_RECORD_BYTES, RESUME_RECORD_VERSION, ResumeRecord,
    RunResumeRepository,
};
use jarvis_application::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{RunId, WorkspaceId};

use super::repositories::text;

/// The SQLite-backed resume store.
#[derive(Debug, Clone)]
pub struct SqliteResumeRepository {
    pool: SqlitePool,
}

impl SqliteResumeRepository {
    /// Builds the store over an existing pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl RunResumeRepository for SqliteResumeRepository {
    fn save(&self, record: &ResumeRecord) -> RepositoryFuture<'_, ()> {
        let record = record.clone();
        Box::pin(async move {
            let json = serde_json::to_string(&record).map_err(|_| RepositoryError::Query)?;
            if json.len() > MAX_RESUME_RECORD_BYTES {
                return Err(RepositoryError::Conflict {
                    what: "resume_record_too_large",
                });
            }
            sqlx::query(
                "INSERT INTO run_resume_states \
                 (workspace_id, run_id, approval_id, state_version, state_json, created_at) \
                 VALUES (?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) \
                 ON CONFLICT (workspace_id, run_id) DO UPDATE SET \
                 approval_id = excluded.approval_id, state_version = excluded.state_version, \
                 state_json = excluded.state_json",
            )
            .bind(record.workspace.to_string())
            .bind(record.run.to_string())
            .bind(record.approval.to_string())
            .bind(i64::from(record.version))
            .bind(&json)
            .execute(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            Ok(())
        })
    }

    fn load(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> RepositoryFuture<'_, Option<ResumeRecord>> {
        Box::pin(async move {
            let Some(row) = sqlx::query(
                "SELECT state_version, state_json FROM run_resume_states \
                 WHERE workspace_id = ? AND run_id = ?",
            )
            .bind(workspace.to_string())
            .bind(run.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?
            else {
                return Ok(None);
            };
            let record = parse_record(&row)?;
            // The record's own identity must be the row's, or a hand-edited row could resume a run
            // with another run's calls.
            if record.workspace != workspace || record.run != run {
                return Err(RepositoryError::Corrupted {
                    column: "state_json",
                });
            }
            Ok(Some(record))
        })
    }

    fn parked(&self) -> RepositoryFuture<'_, Vec<ResumeRecord>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT workspace_id, run_id, state_version, state_json FROM run_resume_states \
                 ORDER BY created_at ASC, run_id ASC LIMIT ?",
            )
            .bind(i64::from(MAX_PARKED_SCAN))
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let mut records = Vec::with_capacity(rows.len());
            for row in &rows {
                // One uninterpretable row is reported rather than skipped: skipping would leave its run
                // parked with nobody aware, the failure this table exists to prevent.
                records.push(parse_record(row)?);
            }
            Ok(records)
        })
    }

    fn discard(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            sqlx::query("DELETE FROM run_resume_states WHERE workspace_id = ? AND run_id = ?")
                .bind(workspace.to_string())
                .bind(run.to_string())
                .execute(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            Ok(())
        })
    }
}

/// Reads one stored record, refusing a version this build does not know.
fn parse_record(row: &sqlx::sqlite::SqliteRow) -> Result<ResumeRecord, RepositoryError> {
    let version: i64 =
        sqlx::Row::try_get(row, "state_version").map_err(|_| RepositoryError::Corrupted {
            column: "state_version",
        })?;
    if version != i64::from(RESUME_RECORD_VERSION) {
        return Err(RepositoryError::Corrupted {
            column: "state_version",
        });
    }
    serde_json::from_str(&text(row, "state_json")?).map_err(|_| RepositoryError::Corrupted {
        column: "state_json",
    })
}

#[cfg(test)]
#[path = "resume_repository_tests.rs"]
mod tests;
