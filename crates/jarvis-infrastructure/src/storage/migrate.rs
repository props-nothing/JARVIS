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

    /// Builds a version-5 database at `path`, applying every migration before `000006` by hand.
    ///
    /// Applied outside the migrator on purpose: the point is to reach an older schema **without**
    /// this binary's migrations, and the first five are the exact bytes this build still ships, so a
    /// hand-written approximation would test a schema nobody has. `SqlStr` is itself `SqlSafeStr`,
    /// so the embedded SQL passes through unchanged — the one place in this crate where dynamic SQL
    /// is right, because the statement is not input but the migration the build already executes.
    async fn seed_version_five(path: &std::path::Path) {
        let database = Database::open(path).await.expect("the file database opens");
        // The migrator's own ledger, in the shape sqlx creates it. Written here because the fixture
        // applies the migrations outside the migrator, and the migrator reads this table to decide
        // what is pending: without it, `run` would see every migration as unapplied and try to
        // create tables that already exist.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS _sqlx_migrations ( \
                 version BIGINT PRIMARY KEY, \
                 description TEXT NOT NULL, \
                 installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
                 success BOOLEAN NOT NULL, \
                 checksum BLOB NOT NULL, \
                 execution_time BIGINT NOT NULL \
             )",
        )
        .execute(database.pool())
        .await
        .expect("the migration ledger is creatable");
        for migration in MIGRATOR.iter().filter(|migration| migration.version < 6) {
            sqlx::query(migration.sql.clone())
                .execute(database.pool())
                .await
                .expect("a prior migration applies");
            sqlx::query(
                "INSERT INTO _sqlx_migrations \
                 (version, description, installed_on, success, checksum, execution_time) \
                 VALUES (?, ?, '1970-01-01T00:00:00Z', 1, ?, 0)",
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(database.pool())
            .await
            .expect("the applied migration is recorded");
        }
        // The compatibility record the earlier migrations left behind. Read back by the caller as
        // the precondition the test depends on: if the hand-application were wrong, the upgrade
        // would be tested from a schema that never existed.
        sqlx::query("UPDATE schema_version SET schema_version = 5 WHERE id = 1")
            .execute(database.pool())
            .await
            .expect("the compatibility record is set");
    }

    /// Writes one conversation, run, and model call at version 5, before the delivery columns.
    ///
    /// The parent rows come first because `agent_runs.conversation_id` and
    /// `model_calls.run_id` are foreign keys and `foreign_keys` is on in this profile: a child row
    /// naming a parent that does not exist is refused, which is the schema working rather than a
    /// fixture problem. Returns the workspace so the caller can read the rows back.
    async fn seed_version_five_rows(path: &std::path::Path) -> jarvis_domain::ids::WorkspaceId {
        let workspace = jarvis_domain::ids::WorkspaceId::from_uuid(uuid::Uuid::now_v7());
        let conversation = uuid::Uuid::now_v7();
        let run_id = uuid::Uuid::now_v7();
        let database = Database::open(path)
            .await
            .expect("the file database reopens");
        sqlx::query(
            "INSERT INTO conversations \
             (id, workspace_id, owner_user_id, status, channel_origin, created_at, updated_at) \
             VALUES (?, ?, ?, 'active', 'local_cli', '2026-01-01T00:00:00Z', \
                     '2026-01-01T00:00:00Z')",
        )
        .bind(conversation.to_string())
        .bind(workspace.to_string())
        .bind(uuid::Uuid::now_v7().to_string())
        .execute(database.pool())
        .await
        .expect("a version-5 conversation row is writable");
        sqlx::query(
            "INSERT INTO agent_runs \
             (id, workspace_id, conversation_id, principal_id, state, version, objective_ref, \
              created_at, updated_at, runtime_id, runtime_version) \
             VALUES (?, ?, ?, ?, 'received', 1, 'obj', '2026-01-01T00:00:00Z', \
                     '2026-01-01T00:00:00Z', 'jarvis-native', '0.1.0')",
        )
        .bind(run_id.to_string())
        .bind(workspace.to_string())
        .bind(conversation.to_string())
        .bind(uuid::Uuid::now_v7().to_string())
        .execute(database.pool())
        .await
        .expect("a version-5 run row is writable");
        // The row whose survival the test is really about: `ALTER TABLE ADD COLUMN` on a table with
        // rows is the case a fresh-database test cannot exercise.
        sqlx::query(
            "INSERT INTO model_calls \
             (id, workspace_id, run_id, logical_call_id, attempt, provider_id, model_id, \
              state, started_at) \
             VALUES (?, ?, ?, ?, 1, 'scripted.local', 'fixture-1', 'pending', \
                     '2026-01-01T00:00:00Z')",
        )
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(workspace.to_string())
        .bind(run_id.to_string())
        .bind(uuid::Uuid::now_v7().to_string())
        .execute(database.pool())
        .await
        .expect("a version-5 model call is writable");
        workspace
    }

    #[tokio::test]
    async fn an_idempotency_record_written_before_the_scope_change_survives_the_rebuild() {
        // **`000007` REBUILDS a table, which no earlier migration here had done**, so the upgrade path
        // needs its own evidence rather than relying on the `ADD COLUMN` case above. A rebuild that
        // dropped rows would silently lose every pending idempotency key on upgrade — and the loss
        // would surface as a client's retried command creating a *second* run, which is exactly what
        // the key exists to prevent.
        //
        // Two properties are asserted, and the second is the one with teeth. The row must survive, and
        // its `principal_id` must be **attributed from its run** rather than left blank: the run
        // recorded who asked, and the record is about that same request, so attributing it is exact.
        // A blank principal would make the row match nobody — indistinguishable from a key that was
        // never used.
        let directory =
            std::env::temp_dir().join(format!("jarvis-migrate-scope-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).expect("the temp directory is creatable");
        let path = directory.join("jarvis.sqlite");

        seed_version_five(&path).await;
        let workspace = seed_version_five_rows(&path).await;
        // The idempotency record, written at the **pre-`000007`** shape. Three scope dimensions, as
        // `000003` created it — this is the row an upgrading install actually has.
        let key = "0195f4f0-18dc-729b-bb34-07e8c7627f21";
        {
            let database = Database::open(&path)
                .await
                .expect("the file database opens");
            let run_id: String =
                sqlx::query_scalar("SELECT id FROM agent_runs WHERE workspace_id = ? LIMIT 1")
                    .bind(workspace.to_string())
                    .fetch_one(database.pool())
                    .await
                    .expect("the seeded run is readable");
            sqlx::query(
                "INSERT INTO idempotency_records \
                 (id, idempotency_key, workspace_id, operation, api_major, request_digest, \
                  run_id, created_at) \
                 VALUES (?, ?, ?, 'runs.create', 1, 'digest-a', ?, '2026-01-01T00:00:00Z')",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(key)
            .bind(workspace.to_string())
            .bind(&run_id)
            .execute(database.pool())
            .await
            .expect("a pre-scope idempotency record is writable");
        }

        let database = Database::open(&path)
            .await
            .expect("the file database reopens");
        run(database.pool()).await.expect("the upgrade applies");

        // The row survived the rebuild, and its principal came from the run it names.
        let (survived, principal, credential): (String, String, String) = sqlx::query_as(
            "SELECT idempotency_key, principal_id, client_credential FROM idempotency_records \
             WHERE workspace_id = ?",
        )
        .bind(workspace.to_string())
        .fetch_one(database.pool())
        .await
        .expect("the record survived the rebuild");
        assert_eq!(survived, key, "the key must survive the table rebuild");
        assert_eq!(
            principal,
            jarvis_domain::ids::PrincipalId::parse(
                &sqlx::query_scalar::<_, String>(
                    "SELECT principal_id FROM agent_runs WHERE workspace_id = ? LIMIT 1"
                )
                .bind(workspace.to_string())
                .fetch_one(database.pool())
                .await
                .expect("the run's principal is readable"),
            )
            .expect("the stored principal parses")
            .to_string(),
            "the record's principal must be attributed from the run it names, not left blank",
        );
        // The credential is the `unknown` sentinel, because nothing observed which one created the
        // row — so an upgraded record never matches on that dimension, which is fail-closed.
        assert_eq!(
            credential, "unknown",
            "a credential that was never recorded must not be invented",
        );

        // The table's shape, asserted rather than assumed: the rebuilt table must carry the five-part
        // scope as a unique index, because that is what the contract says the scope *is*.
        let scope: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_index_info('idx_idempotency_records_scope') ORDER BY seqno",
        )
        .fetch_all(database.pool())
        .await
        .expect("the index catalog is readable");
        assert_eq!(
            scope,
            vec![
                "principal_id".to_owned(),
                "workspace_id".to_owned(),
                "client_credential".to_owned(),
                "operation".to_owned(),
                "api_major".to_owned(),
                "idempotency_key".to_owned(),
            ],
            "the uniqueness scope must be the contract's five dimensions",
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn a_database_from_a_supported_prior_version_upgrades_in_place() {
        // `AGENTS.md` requires "test migrations from supported prior versions", and `000006` is the
        // first migration here that **alters a table which can already hold rows**: the four before
        // it created tables or replaced one that had no writer, so a fresh-database test was enough.
        // Adding columns to `model_calls` is different — an existing row must survive the upgrade
        // with the new columns present and `NULL`, which is the state the read treats as "written
        // before the measurement existed" rather than as corruption.
        let directory =
            std::env::temp_dir().join(format!("jarvis-migrate-upgrade-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).expect("the temp directory is creatable");
        let path = directory.join("jarvis.sqlite");

        seed_version_five(&path).await;
        {
            let database = Database::open(&path)
                .await
                .expect("the file database opens");
            let before = read_compatibility(database.pool())
                .await
                .expect("compatibility readable");
            assert_eq!(
                before.schema_version, 5,
                "the fixture must stand at version 5",
            );
        }
        seed_version_five_rows(&path).await;

        // Now upgrade with this binary's own migrator.
        let database = Database::open(&path)
            .await
            .expect("the file database reopens");
        run(database.pool()).await.expect("the upgrade applies");

        let after = read_compatibility(database.pool())
            .await
            .expect("compatibility readable");
        assert_eq!(
            after.schema_version, TARGET_SCHEMA_VERSION,
            "the upgrade must record the version this binary writes",
        );
        assert_eq!(
            after.min_reader_version, 1,
            "an additive migration must not raise the minimum reader, or an older binary would \
             be refused a database it can still read",
        );

        // The new columns exist, and every pre-existing row reports `NULL` for both rather than a
        // default. `NULL` is what the read treats as "this row predates the measurement", which is
        // a different fact from a call that measured zero deltas — inventing a `0` here would make
        // every upgraded database claim its old calls were measured bursts.
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('model_calls') WHERE name IN \
             ('last_output_at', 'output_delta_count') ORDER BY name",
        )
        .fetch_all(database.pool())
        .await
        .expect("the column catalog is readable");
        assert_eq!(
            columns,
            vec!["last_output_at".to_owned(), "output_delta_count".to_owned()],
            "both delivery columns must exist after the upgrade",
        );

        // The row written at version 5 survived, and reports `NULL` for both added columns rather
        // than a default. **This is the assertion that makes the test about an upgrade rather than
        // about a schema**: `0` here would claim the old call was measured and delivered nothing,
        // which is a burst verdict invented for a call nobody measured. The read treats `NULL` as
        // "this row predates the measurement", and that reading is only available because the
        // column was added without a backfill.
        let surviving: (i64, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT count(*), max(last_output_at), max(output_delta_count) FROM model_calls",
        )
        .fetch_one(database.pool())
        .await
        .expect("the upgraded rows are readable");
        assert_eq!(surviving.0, 1, "the pre-upgrade row must survive");
        assert_eq!(
            surviving.1, None,
            "a row written before the column existed must read as NULL, not as a default instant",
        );
        assert_eq!(
            surviving.2, None,
            "and NULL for the count, which is a different fact from a measured zero",
        );

        // And the database is still sound, which is the property an `ALTER TABLE` could break.
        database
            .check_integrity()
            .await
            .expect("the upgraded database is intact");

        let _ = std::fs::remove_dir_all(&directory);
    }
}
