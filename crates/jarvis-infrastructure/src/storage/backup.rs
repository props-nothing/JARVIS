//! Database backup and restore.
//!
//! A live SQLite database is three files: the main file, `-wal`, and `-shm`.
//! Copying the main file alone while the database is open yields a torn snapshot
//! that may be missing committed transactions. This module therefore uses
//! SQLite's online backup API, which produces a consistent snapshot through the
//! database rather than through the filesystem.
//!
//! A backup is verified by running an integrity check on the *copy*, not on the
//! source: the point of a backup is that the copy is usable.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use std::str::FromStr as _;

use super::connection::{BUSY_TIMEOUT, Database};
use super::error::StorageError;

/// A created and verified backup.
#[derive(Debug, Clone)]
pub struct Backup {
    path: PathBuf,
    bytes: u64,
}

impl Backup {
    /// Returns the backup path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the backup size in bytes.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Creates a verified backup of `source` at `destination`.
///
/// # Errors
///
/// Returns [`StorageError::Backup`] when the snapshot or its verification
/// fails. The destination is removed on failure so a partial file cannot be
/// mistaken for a usable backup.
pub async fn create(source: &Database, destination: &Path) -> Result<Backup, StorageError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|_| StorageError::Backup)?;
    }
    // A stale file at the destination would be silently merged by the backup
    // API, so it is removed first and the snapshot starts clean.
    if destination.exists() {
        std::fs::remove_file(destination).map_err(|_| StorageError::Backup)?;
    }

    let destination_str = destination.to_string_lossy().into_owned();
    // `VACUUM INTO` is the online-backup entry point exposed through SQL. It
    // reads a consistent snapshot of the source database, including WAL content.
    let result = sqlx::query("VACUUM INTO ?")
        .bind(&destination_str)
        .execute(source.pool())
        .await;

    if result.is_err() {
        let _ = std::fs::remove_file(destination);
        return Err(StorageError::Backup);
    }

    // Verify the copy itself before reporting success.
    let verification = verify(destination).await;
    if let Err(error) = verification {
        let _ = std::fs::remove_file(destination);
        return Err(error);
    }

    let bytes = std::fs::metadata(destination)
        .map_err(|_| StorageError::Backup)?
        .len();

    Ok(Backup {
        path: destination.to_path_buf(),
        bytes,
    })
}

/// Verifies that `path` is a usable JARVIS database snapshot.
///
/// # Errors
///
/// Returns [`StorageError::Backup`] when the file cannot be opened and
/// [`StorageError::Integrity`] when it fails an integrity or foreign-key check.
pub async fn verify(path: &Path) -> Result<(), StorageError> {
    let database = Database::open(path)
        .await
        .map_err(|_| StorageError::Backup)?;
    let result = database.check_integrity().await;
    database.close().await;
    result
}

/// Restores `backup` into `destination`.
///
/// The backup is verified *before* it overwrites anything, so a corrupt backup
/// cannot destroy the current database.
///
/// # Errors
///
/// Returns [`StorageError::Restore`] when the backup cannot be copied into place
/// and [`StorageError::Integrity`] when the backup fails verification.
pub async fn restore(backup: &Path, destination: &Path) -> Result<(), StorageError> {
    verify(backup).await?;

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|_| StorageError::Restore)?;
    }

    // Copy to a sibling temp file, then replace. A crash mid-copy leaves the
    // previous database intact rather than a half-written one.
    let temporary = destination.with_extension("restore-tmp");
    let _ = std::fs::remove_file(&temporary);

    std::fs::copy(backup, &temporary).map_err(|_| StorageError::Restore)?;

    // A WAL left beside the destination belongs to the *old* database; keeping
    // it would corrupt the restore, so it is removed as part of the swap.
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar_path(destination, suffix);
        let _ = std::fs::remove_file(&sidecar);
    }

    std::fs::rename(&temporary, destination).map_err(|_| {
        let _ = std::fs::remove_file(&temporary);
        StorageError::Restore
    })?;

    // Verify the restored database in place before declaring success.
    let database = Database::open(destination)
        .await
        .map_err(|_| StorageError::Restore)?;
    let result = database.check_integrity().await;
    database.close().await;
    result
}

/// Builds the `-wal` or `-shm` path for a database file.
fn sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut name = database.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Opens a read-only pool, used by doctor-style inspection.
///
/// # Errors
///
/// Returns [`StorageError::Open`] when the file cannot be opened read-only.
pub async fn open_read_only(path: &Path) -> Result<SqlitePool, StorageError> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .map_err(|_| StorageError::Open {
            path: path.to_path_buf(),
        })?
        .read_only(true)
        .busy_timeout(BUSY_TIMEOUT);

    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| StorageError::Open {
            path: path.to_path_buf(),
        })
}

#[cfg(test)]
mod tests {
    use super::{create, open_read_only, restore, sidecar_path};
    use crate::storage::connection::Database;
    use crate::storage::migrate::run;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-fnd006-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    async fn populated(dir: &std::path::Path) -> Database {
        let database = Database::open(&dir.join("jarvis.sqlite"))
            .await
            .expect("database opens");
        run(database.pool()).await.expect("migration applies");
        sqlx::query(
            "INSERT INTO diagnostic_events (id, scope, severity, code, occurred_at) \
             VALUES ('e1', 'startup', 'info', 'test.marker', '2026-09-21T00:00:00Z')",
        )
        .execute(database.pool())
        .await
        .expect("seed row");
        database
    }

    #[tokio::test]
    async fn a_backup_is_created_verified_and_non_empty() {
        let dir = temp_dir("backup-ok");
        let database = populated(&dir).await;
        let backup_path = dir.join("backup.sqlite");

        let backup = create(&database, &backup_path)
            .await
            .expect("backup succeeds");
        assert!(backup.path().exists());
        assert!(backup.bytes() > 0, "a backup must not be empty");

        database.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_backup_preserves_committed_rows() {
        let dir = temp_dir("backup-data");
        let database = populated(&dir).await;
        let backup_path = dir.join("backup.sqlite");
        create(&database, &backup_path)
            .await
            .expect("backup succeeds");

        let restored = Database::open(&backup_path).await.expect("backup opens");
        let marker: i64 =
            sqlx::query_scalar("SELECT count(*) FROM diagnostic_events WHERE code = 'test.marker'")
                .fetch_one(restored.pool())
                .await
                .expect("count readable");
        assert_eq!(marker, 1, "the committed row must be present in the backup");

        restored.close().await;
        database.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn restoring_replaces_the_destination_with_the_backup_content() {
        let dir = temp_dir("restore");
        let database = populated(&dir).await;
        let backup_path = dir.join("backup.sqlite");
        create(&database, &backup_path)
            .await
            .expect("backup succeeds");

        // Divert the live database from the backup by deleting the marker.
        sqlx::query("DELETE FROM diagnostic_events WHERE code = 'test.marker'")
            .execute(database.pool())
            .await
            .expect("delete marker");
        database.close().await;

        let live_path = dir.join("jarvis.sqlite");
        restore(&backup_path, &live_path)
            .await
            .expect("restore succeeds");

        let restored = Database::open(&live_path).await.expect("restored opens");
        let marker: i64 =
            sqlx::query_scalar("SELECT count(*) FROM diagnostic_events WHERE code = 'test.marker'")
                .fetch_one(restored.pool())
                .await
                .expect("count readable");
        assert_eq!(marker, 1, "restore must bring the marker back");
        restored.close().await;

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_corrupt_backup_is_refused_and_the_live_database_survives() {
        let dir = temp_dir("restore-corrupt");
        let database = populated(&dir).await;
        database.close().await;

        let live_path = dir.join("jarvis.sqlite");
        let corrupt = dir.join("corrupt.sqlite");
        std::fs::write(&corrupt, b"this is not a sqlite database").expect("write corrupt fixture");

        let error = restore(&corrupt, &live_path)
            .await
            .expect_err("a corrupt backup must be refused");
        assert!(
            matches!(error.code(), "jarvis.db_backup" | "jarvis.db_integrity"),
            "unexpected code {}",
            error.code(),
        );

        // The previous database must still open and be intact.
        let survivor = Database::open(&live_path)
            .await
            .expect("the live database must be preserved");
        survivor.check_integrity().await.expect("still intact");
        survivor.close().await;

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_failed_backup_leaves_no_partial_file() {
        let dir = temp_dir("backup-fail");
        let database = populated(&dir).await;
        database.close().await;

        // Closing the pool makes the source unusable, so the snapshot fails.
        let closed = Database::open(&dir.join("jarvis.sqlite"))
            .await
            .expect("reopen");
        closed.close().await;

        let destination = dir.join("partial.sqlite");
        let result = create(&closed, &destination).await;
        assert!(result.is_err(), "backing up a closed pool must fail");
        assert!(
            !destination.exists(),
            "a failed backup must not leave a partial file",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_read_only_pool_can_inspect_without_writing() {
        let dir = temp_dir("readonly");
        let database = populated(&dir).await;
        database.close().await;

        let pool = open_read_only(&dir.join("jarvis.sqlite"))
            .await
            .expect("read-only opens");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM diagnostic_events")
            .fetch_one(&pool)
            .await
            .expect("read works");
        assert_eq!(count, 1);

        // A write must be refused by the read-only connection.
        let write = sqlx::query("DELETE FROM diagnostic_events")
            .execute(&pool)
            .await;
        assert!(write.is_err(), "a read-only pool must refuse a write");

        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecar_paths_are_derived_from_the_database_name() {
        let base = std::path::Path::new("/tmp/jarvis.sqlite");
        assert_eq!(
            sidecar_path(base, "-wal"),
            std::path::Path::new("/tmp/jarvis.sqlite-wal"),
        );
        assert_eq!(
            sidecar_path(base, "-shm"),
            std::path::Path::new("/tmp/jarvis.sqlite-shm"),
        );
    }
}
