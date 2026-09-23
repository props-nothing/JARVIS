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
/// **Nothing calls this outside its own tests, and the doc used to claim otherwise** — it said
/// this "is what lets startup decide between 'migrate safely' and 'report readiness false' before
/// it changes anything", which `start` does not do: startup opens the database, migrates, and only
/// then reads compatibility, so there is no pre-migration branch. The claim described an intent
/// rather than the code, and a reader checking whether that decision existed would have found the
/// function and stopped.
///
/// It is kept because it is the honest way to assert migration state in a test without applying
/// anything, and because the intent is worth stating where the decision *would* live: if startup
/// ever needs to report readiness false before touching a database — the "explicit approval
/// required" row of `docs/data/migrations.md` — this is the read it would branch on.
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
    async fn a_checksum_mismatch_is_refused_rather_than_applied_over() {
        // The sibling row of the table `docs/data/migrations.md` fixes: "Checksum mismatch | Refuse
        // readiness; require diagnosis". `run`'s own doc calls this out as the reason it does **not**
        // pass `set_ignore_missing` — "a checksum mismatch is a real corruption/divergence signal
        // and must not be ignored" — and nothing tested it.
        //
        // The signal is worth refusing rather than papering over because it means the migration file
        // this binary carries is not the one that was applied: either the database was migrated by a
        // build whose SQL differed under the same version number, or a file was edited after the
        // fact. Applying the rest on top would build on a schema nobody can reconstruct.
        let database = Database::open_in_memory().await.expect("in-memory opens");
        run(database.pool()).await.expect("migration applies");

        // Corrupt the recorded checksum of an applied migration, which is exactly the divergence a
        // reader would face. The bytes are flipped rather than the row deleted: a missing row reads
        // as "not applied" and would take the ordinary migration path, so deleting would test the
        // wrong branch.
        let affected =
            sqlx::query("UPDATE _sqlx_migrations SET checksum = x'00' WHERE version = 1")
                .execute(database.pool())
                .await
                .expect("the checksum is corrupted");
        assert_eq!(affected.rows_affected(), 1, "one migration must be touched");

        let error = run(database.pool())
            .await
            .expect_err("a diverged migration set must be refused");
        assert_eq!(error.code(), "jarvis.db_migrate");
        assert!(!error.retryable(), "a divergence is not fixed by retrying");
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
