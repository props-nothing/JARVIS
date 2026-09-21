//! Scoped maintenance locks.
//!
//! A lock row is *advisory metadata*, not the guard. The authoritative guard for
//! a single local profile is a held file lock (see the Foundation evidence note
//! on single-instance ownership). These rows make the holder, scope, and expiry
//! visible to doctor and repair, and they give a lease-style bound so a crashed
//! holder does not block maintenance forever.

use jiff::Timestamp;
use sqlx::{Row as _, SqlitePool};

use super::error::StorageError;

/// The scopes a Foundation maintenance operation may lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockScope {
    /// Applying migrations.
    Migrate,
    /// Creating or verifying a backup.
    Backup,
    /// Restoring from a backup.
    Restore,
}

impl LockScope {
    /// Returns the stable scope name stored in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Migrate => "migrate",
            Self::Backup => "backup",
            Self::Restore => "restore",
        }
    }
}

/// An acquired maintenance lock.
#[derive(Debug)]
pub struct MaintenanceLock {
    scope: LockScope,
    holder: String,
}

impl MaintenanceLock {
    /// Returns the scope this lock covers.
    #[must_use]
    pub const fn scope(&self) -> LockScope {
        self.scope
    }

    /// Returns the holder identifier.
    #[must_use]
    pub fn holder(&self) -> &str {
        &self.holder
    }
}

/// Acquires `scope` for `holder` until `expires_at`.
///
/// An existing lock is reclaimed only when it has expired, so a crashed holder
/// does not block maintenance permanently while a live holder still does.
///
/// # Errors
///
/// Returns [`StorageError::LockHeld`] when another unexpired holder owns the
/// scope and [`StorageError::Query`] for a driver failure.
pub async fn acquire(
    pool: &SqlitePool,
    scope: LockScope,
    holder: &str,
    expires_at: Timestamp,
) -> Result<MaintenanceLock, StorageError> {
    let now = Timestamp::now().to_string();
    let expires = expires_at.to_string();

    // Claim the row only when it is absent or its lease has passed. A single
    // statement keeps the check and the write atomic.
    let result = sqlx::query(
        "INSERT INTO application_locks (scope, holder, acquired_at, expires_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(scope) DO UPDATE SET \
             holder = excluded.holder, \
             acquired_at = excluded.acquired_at, \
             expires_at = excluded.expires_at \
         WHERE application_locks.expires_at <= ?",
    )
    .bind(scope.as_str())
    .bind(holder)
    .bind(&now)
    .bind(&expires)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|_| StorageError::Query)?;

    if result.rows_affected() == 0 {
        return Err(StorageError::LockHeld {
            scope: scope.as_str().to_owned(),
        });
    }

    Ok(MaintenanceLock {
        scope,
        holder: holder.to_owned(),
    })
}

/// Releases `lock`, but only if this holder still owns it.
///
/// The holder check is what prevents a reclaimed lease from being released by
/// the stale owner.
///
/// # Errors
///
/// Returns [`StorageError::Query`] on a driver failure.
pub async fn release(pool: &SqlitePool, lock: &MaintenanceLock) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM application_locks WHERE scope = ? AND holder = ?")
        .bind(lock.scope.as_str())
        .bind(lock.holder())
        .execute(pool)
        .await
        .map_err(|_| StorageError::Query)?;
    Ok(())
}

/// Returns the current holder of `scope`, if any.
///
/// # Errors
///
/// Returns [`StorageError::Query`] on a driver failure.
pub async fn current_holder(
    pool: &SqlitePool,
    scope: LockScope,
) -> Result<Option<(String, String)>, StorageError> {
    let row = sqlx::query("SELECT holder, expires_at FROM application_locks WHERE scope = ?")
        .bind(scope.as_str())
        .fetch_optional(pool)
        .await
        .map_err(|_| StorageError::Query)?;

    match row {
        Some(row) => {
            let holder: String = row.try_get("holder").map_err(|_| StorageError::Query)?;
            let expires: String = row.try_get("expires_at").map_err(|_| StorageError::Query)?;
            Ok(Some((holder, expires)))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::{LockScope, acquire, current_holder, release};
    use crate::storage::connection::Database;
    use crate::storage::migrate::run;

    async fn migrated() -> Database {
        let database = Database::open_in_memory().await.expect("in-memory opens");
        run(database.pool()).await.expect("migration applies");
        database
    }

    fn later() -> Timestamp {
        Timestamp::now()
            .checked_add(jiff::ToSpan::hours(1))
            .expect("one hour ahead is representable")
    }

    fn earlier() -> Timestamp {
        Timestamp::now()
            .checked_sub(jiff::ToSpan::hours(1))
            .expect("one hour behind is representable")
    }

    #[tokio::test]
    async fn a_free_scope_can_be_acquired() {
        let database = migrated().await;
        let lock = acquire(database.pool(), LockScope::Migrate, "holder-a", later())
            .await
            .expect("a free scope is acquirable");
        assert_eq!(lock.scope(), LockScope::Migrate);
        assert_eq!(lock.holder(), "holder-a");
    }

    #[tokio::test]
    async fn a_held_scope_is_refused() {
        let database = migrated().await;
        acquire(database.pool(), LockScope::Migrate, "holder-a", later())
            .await
            .expect("first acquire succeeds");

        let error = acquire(database.pool(), LockScope::Migrate, "holder-b", later())
            .await
            .expect_err("a held scope must be refused");
        assert_eq!(error.code(), "jarvis.db_lock_held");
        assert!(!error.retryable());

        // The original holder is unchanged by the failed attempt.
        let holder = current_holder(database.pool(), LockScope::Migrate)
            .await
            .expect("holder readable");
        assert_eq!(
            holder.map(|(holder, _)| holder),
            Some("holder-a".to_owned())
        );
    }

    #[tokio::test]
    async fn an_expired_lease_can_be_reclaimed() {
        let database = migrated().await;
        acquire(
            database.pool(),
            LockScope::Backup,
            "crashed-holder",
            earlier(),
        )
        .await
        .expect("acquire with a past expiry succeeds");

        let lock = acquire(database.pool(), LockScope::Backup, "new-holder", later())
            .await
            .expect("an expired lease must be reclaimable");
        assert_eq!(lock.holder(), "new-holder");
    }

    #[tokio::test]
    async fn release_frees_the_scope_but_only_for_the_owner() {
        let database = migrated().await;
        let lock = acquire(database.pool(), LockScope::Restore, "holder-a", later())
            .await
            .expect("acquire succeeds");

        // A stale holder must not be able to release a scope it no longer owns.
        let stale = super::MaintenanceLock {
            scope: LockScope::Restore,
            holder: "holder-a".to_owned(),
        };
        release(database.pool(), &stale)
            .await
            .expect("release is best effort");
        assert!(
            current_holder(database.pool(), LockScope::Restore)
                .await
                .expect("readable")
                .is_none(),
            "the owning holder may release",
        );

        release(database.pool(), &lock)
            .await
            .expect("a second release is harmless");
    }

    #[tokio::test]
    async fn scopes_are_independent() {
        let database = migrated().await;
        acquire(database.pool(), LockScope::Migrate, "a", later())
            .await
            .expect("migrate scope");
        acquire(database.pool(), LockScope::Backup, "b", later())
            .await
            .expect("backup scope is independent");
        acquire(database.pool(), LockScope::Restore, "c", later())
            .await
            .expect("restore scope is independent");
    }
}
