//! SQLite connection setup, health, and integrity checks.
//!
//! The connection is configured to match the Foundation evidence note exactly:
//! the bundled library (never a dynamically loaded extension), foreign keys on,
//! WAL journaling, full synchronous durability, trusted schema off, and a
//! bounded busy timeout. Every setting that matters for safety is queried back
//! rather than assumed, because SQLite silently ignores an unknown pragma.

use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions as _, Row as _, SqlitePool};

use super::error::StorageError;

/// The minimum supported SQLite version.
///
/// `3.51.3` is the first release containing the WAL-reset corruption repair, so
/// an older library is refused rather than risk silent corruption.
pub const MIN_SQLITE_VERSION: &str = "3.51.3";

/// The maximum number of pooled connections.
///
/// SQLite allows one writer at a time; a small pool avoids piling write
/// contenders behind the busy timeout.
pub const MAX_POOL_CONNECTIONS: u32 = 4;

/// The bounded time to wait for a held write lock before returning `SQLITE_BUSY`.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Owns the connection pool for one profile database.
#[derive(Debug, Clone)]
pub struct Database {
    pool: SqlitePool,
    path: PathBuf,
}

impl Database {
    /// Opens (creating if missing) the database at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Open`] when the file cannot be opened,
    /// [`StorageError::SqliteTooOld`] when the connected library is below
    /// [`MIN_SQLITE_VERSION`], and [`StorageError::Integrity`] when a
    /// safety-critical pragma did not take effect.
    pub async fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| StorageError::Open {
                path: path.to_path_buf(),
            })?;
        }

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .map_err(|_| StorageError::Open {
                path: path.to_path_buf(),
            })?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            // A prepared-statement cache is a memory/CPU tradeoff; 100 is the
            // SQLx default and is stated explicitly so a change is deliberate.
            .statement_cache_capacity(100)
            .busy_timeout(BUSY_TIMEOUT)
            // `trusted_schema` defaults to ON in SQLite 3.31+, which allows a
            // schema to invoke functions an attacker may have redefined. JARVIS
            // turns it off explicitly rather than relying on the default.
            .pragma("trusted_schema", "OFF")
            // Statement logging is off. SQLx defaults `statements_level` to
            // `Debug`, so a user raising the log level would otherwise copy
            // every statement and its bound parameters into the log pipeline.
            .log_statements(log::LevelFilter::Off);

        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_POOL_CONNECTIONS)
            .connect_with(options)
            .await
            .map_err(|_| StorageError::Open {
                path: path.to_path_buf(),
            })?;

        let database = Self {
            pool,
            path: path.to_path_buf(),
        };
        database.verify_runtime_version().await?;
        database.verify_pragmas().await?;
        Ok(database)
    }

    /// Opens an in-memory database, for tests only.
    ///
    /// # Errors
    ///
    /// Returns an error when the in-memory database cannot be created.
    pub async fn open_in_memory() -> Result<Self, StorageError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(|_| StorageError::Open {
                path: PathBuf::from(":memory:"),
            })?
            .foreign_keys(true)
            .pragma("trusted_schema", "OFF")
            .busy_timeout(BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            // A shared in-memory database disappears when the last connection
            // closes, so exactly one connection keeps the schema alive.
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|_| StorageError::Open {
                path: PathBuf::from(":memory:"),
            })?;

        Ok(Self {
            pool,
            path: PathBuf::from(":memory:"),
        })
    }

    /// Returns the connection pool.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Returns the database path, or `:memory:`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the SQLite version reported by the connected library.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Query`] if the version cannot be read.
    pub async fn sqlite_version(&self) -> Result<String, StorageError> {
        let row = sqlx::query("SELECT sqlite_version() AS version")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StorageError::Query)?;
        row.try_get("version").map_err(|_| StorageError::Query)
    }

    /// Refuses a library older than [`MIN_SQLITE_VERSION`].
    async fn verify_runtime_version(&self) -> Result<(), StorageError> {
        let found = self.sqlite_version().await?;
        if version_at_least(&found, MIN_SQLITE_VERSION) {
            Ok(())
        } else {
            Err(StorageError::SqliteTooOld {
                found,
                required: MIN_SQLITE_VERSION.to_owned(),
            })
        }
    }

    /// Re-queries the safety-critical pragmas and fails if one did not apply.
    async fn verify_pragmas(&self) -> Result<(), StorageError> {
        // Query each pragma individually: SQLite ignores an unknown pragma
        // silently, so the value must be read back, not assumed.
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StorageError::Query)?;
        if foreign_keys != 1 {
            return Err(StorageError::Integrity {
                detail: "foreign_keys is not enabled".to_owned(),
            });
        }

        let trusted_schema: i64 = sqlx::query_scalar("PRAGMA trusted_schema")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StorageError::Query)?;
        if trusted_schema != 0 {
            return Err(StorageError::Integrity {
                detail: "trusted_schema is not disabled".to_owned(),
            });
        }

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StorageError::Query)?;
        if !journal_mode.eq_ignore_ascii_case("wal") && self.path != Path::new(":memory:") {
            return Err(StorageError::Integrity {
                detail: "journal_mode is not wal".to_owned(),
            });
        }

        Ok(())
    }

    /// Returns whether the database answers a trivial query.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Query`] when the probe fails.
    pub async fn is_healthy(&self) -> Result<bool, StorageError> {
        let value: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StorageError::Query)?;
        Ok(value == 1)
    }

    /// Runs `PRAGMA integrity_check` and `PRAGMA foreign_key_check`.
    ///
    /// Both are required: `integrity_check` reports `ok` for a structurally
    /// sound file but does not detect foreign-key violations, and vice versa is
    /// also untrue. A clean database must pass both.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Integrity`] with a bounded detail when either
    /// check reports a problem, and [`StorageError::Query`] on a driver failure.
    pub async fn check_integrity(&self) -> Result<(), StorageError> {
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StorageError::Query)?;
        if !integrity.eq_ignore_ascii_case("ok") {
            return Err(StorageError::Integrity {
                detail: format!("integrity_check reported {}", summarise(&integrity)),
            });
        }

        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&self.pool)
            .await
            .map_err(|_| StorageError::Query)?;
        if !violations.is_empty() {
            return Err(StorageError::Integrity {
                detail: format!("{} foreign key violation(s)", violations.len()),
            });
        }

        Ok(())
    }

    /// Closes the pool, refusing to return while connections are checked out.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Truncates a diagnostic string so an unexpected payload cannot flood a log.
fn summarise(value: &str) -> String {
    const LIMIT: usize = 200;
    if value.chars().count() <= LIMIT {
        return value.to_owned();
    }
    let truncated: String = value.chars().take(LIMIT).collect();
    format!("{truncated}...")
}

/// Compares dotted numeric versions such as `3.51.3` numerically.
///
/// A textual comparison would rank `3.9.0` above `3.51.3`, which is exactly the
/// mistake that would admit a vulnerable library.
#[must_use]
pub fn version_at_least(found: &str, required: &str) -> bool {
    fn parts(value: &str) -> Vec<u64> {
        value
            .split(|character: char| !character.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse::<u64>().ok())
            .collect()
    }

    let found = parts(found);
    let required = parts(required);
    if found.is_empty() || required.is_empty() {
        return false;
    }
    for index in 0..found.len().max(required.len()) {
        let left = found.get(index).copied().unwrap_or(0);
        let right = required.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{Database, version_at_least};

    #[test]
    fn version_comparison_is_numeric_not_lexical() {
        // The case a string comparison gets wrong.
        assert!(version_at_least("3.51.3", "3.51.3"));
        assert!(version_at_least("3.51.4", "3.51.3"));
        assert!(version_at_least("3.52.0", "3.51.3"));
        assert!(version_at_least("4.0.0", "3.51.3"));
        assert!(
            !version_at_least("3.9.0", "3.51.3"),
            "3.9 must rank below 3.51"
        );
        assert!(
            !version_at_least("3.51.2", "3.51.3"),
            "the WAL-reset fix floor"
        );
        assert!(!version_at_least("3.46.0", "3.51.3"));
        // A shorter version is padded with zeroes.
        assert!(version_at_least("3.51", "3.51.0"));
        assert!(!version_at_least("3.50", "3.51.0"));
        assert!(!version_at_least("garbage", "3.51.3"));
    }

    #[tokio::test]
    async fn the_bundled_library_meets_the_minimum_version() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        let version = database.sqlite_version().await.expect("version readable");
        assert!(
            version_at_least(&version, super::MIN_SQLITE_VERSION),
            "bundled SQLite {version} is below the supported minimum",
        );
    }

    #[tokio::test]
    async fn a_new_database_is_healthy_and_integrity_clean() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        assert!(database.is_healthy().await.expect("health query"));
        database.check_integrity().await.expect("clean database");
    }

    #[tokio::test]
    async fn foreign_keys_are_actually_enabled() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(database.pool())
            .await
            .expect("pragma readable");
        assert_eq!(enabled, 1, "foreign key enforcement must be on");
    }

    #[tokio::test]
    async fn trusted_schema_is_actually_disabled() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        let enabled: i64 = sqlx::query_scalar("PRAGMA trusted_schema")
            .fetch_one(database.pool())
            .await
            .expect("pragma readable");
        assert_eq!(enabled, 0, "trusted_schema must be off");
    }

    #[tokio::test]
    async fn a_foreign_key_violation_is_detected() {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        sqlx::query("CREATE TABLE parent (id TEXT PRIMARY KEY)")
            .execute(database.pool())
            .await
            .expect("parent table");
        sqlx::query(
            "CREATE TABLE child (id TEXT PRIMARY KEY, parent_id TEXT REFERENCES parent(id))",
        )
        .execute(database.pool())
        .await
        .expect("child table");

        // Enforcement is on, so a dangling reference is refused at insert time.
        let refused = sqlx::query("INSERT INTO child (id, parent_id) VALUES ('c1', 'missing')")
            .execute(database.pool())
            .await;
        assert!(
            refused.is_err(),
            "an enforced foreign key must refuse the insert"
        );

        // A structurally sound file is not proof of referential integrity, so
        // create a dangling row with enforcement temporarily off and prove that
        // `foreign_key_check` (not `integrity_check`) is what catches it.
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(database.pool())
            .await
            .expect("disable enforcement");
        sqlx::query("INSERT INTO child (id, parent_id) VALUES ('c2', 'missing')")
            .execute(database.pool())
            .await
            .expect("insert with enforcement off");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(database.pool())
            .await
            .expect("re-enable enforcement");

        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(database.pool())
            .await
            .expect("integrity readable");
        assert!(
            integrity.eq_ignore_ascii_case("ok"),
            "integrity_check does not see foreign key violations",
        );

        let error = database
            .check_integrity()
            .await
            .expect_err("a foreign key violation must be reported");
        assert_eq!(error.code(), "jarvis.db_integrity");
    }

    #[tokio::test]
    async fn an_file_database_uses_wal_journaling() {
        let directory =
            std::env::temp_dir().join(format!("jarvis-fnd006-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("jarvis.sqlite");

        let database = Database::open(&path).await.expect("file database opens");
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(database.pool())
            .await
            .expect("journal mode readable");
        assert!(mode.eq_ignore_ascii_case("wal"), "journal_mode is {mode}");
        database.close().await;

        let _ = std::fs::remove_dir_all(&directory);
    }
}
