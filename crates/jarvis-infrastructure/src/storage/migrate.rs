//! Embedded migrations and the migration runner.
//!
//! Migrations are compiled into the binary rather than read from disk, so a
//! packaged install cannot run a different set than the one it shipped with. The
//! build script re-runs when `migrations/sqlite` changes, which keeps the
//! embedded copy from silently going stale.

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;

use super::error::StorageError;

/// The migrations compiled into this binary.
///
/// The path is relative to this source file. `.gitattributes` pins `*.sql` to
/// LF, because the embedded checksum is computed from the file bytes and a line
/// ending change would alter it.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations/sqlite");

/// Applies every pending migration.
///
/// # Errors
///
/// Returns [`StorageError::Migrate`] when the migrator reports any failure,
/// including a checksum mismatch on an already-applied migration. A checksum
/// mismatch is a real corruption/divergence signal and must not be ignored with
/// `set_ignore_missing`.
pub async fn run(pool: &SqlitePool) -> Result<(), StorageError> {
    MIGRATOR.run(pool).await.map_err(|_| StorageError::Migrate)
}

/// Returns whether any migration is still pending, without applying it.
///
/// This is what lets startup decide between "migrate safely" and "report
/// readiness false" before it changes anything.
///
/// # Errors
///
/// Returns [`StorageError::Migrate`] when the applied set cannot be read.
pub async fn has_pending(pool: &SqlitePool) -> Result<bool, StorageError> {
    let applied: Vec<_> = MIGRATOR.iter().map(|migration| migration.version).collect();
    let recorded = sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    Ok(applied.iter().any(|version| !recorded.contains(version)))
}

#[cfg(test)]
mod tests {
    use super::{MIGRATOR, has_pending, run};
    use crate::storage::connection::Database;
    use crate::storage::schema::{TARGET_SCHEMA_VERSION, read_compatibility};

    #[tokio::test]
    async fn the_embedded_migration_set_is_not_empty() {
        assert!(
            MIGRATOR.iter().count() >= 1,
            "at least one migration must be embedded",
        );
    }

    #[tokio::test]
    async fn a_fresh_database_migrates_and_reports_the_target_version() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        assert!(
            has_pending(database.pool())
                .await
                .expect("pending readable"),
            "a fresh database must have pending migrations",
        );

        run(database.pool()).await.expect("migration applies");

        assert!(
            !has_pending(database.pool())
                .await
                .expect("pending readable"),
            "nothing may remain pending after a run",
        );

        let compatibility = read_compatibility(database.pool())
            .await
            .expect("compatibility readable");
        assert_eq!(compatibility.schema_version, TARGET_SCHEMA_VERSION);
        assert!(compatibility.writer_version.starts_with("0."));
    }

    #[tokio::test]
    async fn running_the_migrator_twice_is_idempotent() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        run(database.pool()).await.expect("first run applies");
        run(database.pool()).await.expect("second run is a no-op");

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(database.pool())
            .await
            .expect("migration count readable");
        let count = usize::try_from(count).expect("a row count is non-negative");
        assert_eq!(count, MIGRATOR.iter().count());
    }

    #[tokio::test]
    async fn the_initial_migration_creates_the_durability_tables() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        run(database.pool()).await.expect("migration applies");

        for table in ["schema_version", "application_locks", "diagnostic_events"] {
            let found: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
            )
            .bind(table)
            .fetch_one(database.pool())
            .await
            .expect("catalog readable");
            assert_eq!(found, 1, "{table} must exist after migration");
        }
    }

    #[tokio::test]
    async fn a_foreign_key_violation_is_prevented_after_migration() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        run(database.pool()).await.expect("migration applies");

        // The migration itself must not leave an invalid database.
        database
            .check_integrity()
            .await
            .expect("clean after migration");
    }
}
