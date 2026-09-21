//! Durable storage errors with stable, namespaced codes.
//!
//! Database errors are explicit types. They never carry a connection string,
//! a bound parameter, or a raw driver payload, because those can contain secrets
//! or unbounded untrusted text.

use std::path::PathBuf;

use thiserror::Error;

/// The minimum schema version a binary must find before it may write.
pub use crate::storage::schema::MIN_SUPPORTED_SCHEMA_VERSION;

/// An error raised by the storage adapter.
#[derive(Debug, Error)]
pub enum StorageError {
    /// The database file could not be opened or connected.
    #[error("the database could not be opened")]
    Open {
        /// The database path. Paths are safe to record; they hold no secret.
        path: PathBuf,
    },
    /// A migration failed.
    #[error("a database migration failed")]
    Migrate,
    /// The running SQLite library is older than the supported minimum.
    #[error("the bundled SQLite library is older than the supported minimum")]
    SqliteTooOld {
        /// The version reported by the connected handle.
        found: String,
        /// The minimum supported version.
        required: String,
    },
    /// The database's schema version is newer than this binary supports.
    ///
    /// The database is left untouched; the operator must upgrade the binary.
    #[error("the database was written by a newer JARVIS version")]
    SchemaTooNew {
        /// The version found in the database.
        found: i64,
        /// The highest version this binary understands.
        supported: i64,
    },
    /// The compatibility record is missing or malformed.
    #[error("the database compatibility record is missing or malformed")]
    CompatibilityRecord,
    /// A health or integrity check failed.
    #[error("a database integrity check failed")]
    Integrity {
        /// A bounded, safe description of what failed. Never row contents.
        detail: String,
    },
    /// A maintenance lock is held by another holder.
    #[error("the maintenance lock is held by another process")]
    LockHeld {
        /// The scope that is locked.
        scope: String,
    },
    /// A backup could not be created or verified.
    #[error("the database backup could not be created or verified")]
    Backup,
    /// A restore could not be completed.
    #[error("the database restore could not be completed")]
    Restore,
    /// A query failed.
    #[error("a database operation failed")]
    Query,
}

impl StorageError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Open { .. } => "jarvis.db_open",
            Self::Migrate => "jarvis.db_migrate",
            Self::SqliteTooOld { .. } => "jarvis.db_sqlite_too_old",
            Self::SchemaTooNew { .. } => "jarvis.db_schema_too_new",
            Self::CompatibilityRecord => "jarvis.db_compatibility_record",
            Self::Integrity { .. } => "jarvis.db_integrity",
            Self::LockHeld { .. } => "jarvis.db_lock_held",
            Self::Backup => "jarvis.db_backup",
            Self::Restore => "jarvis.db_restore",
            Self::Query => "jarvis.db_query",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    ///
    /// A held lock and a newer schema are deterministic conditions that a blind
    /// retry cannot fix. Connection and query failures may be transient.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Open { .. } | Self::Query => true,
            Self::Migrate
            | Self::SqliteTooOld { .. }
            | Self::SchemaTooNew { .. }
            | Self::CompatibilityRecord
            | Self::Integrity { .. }
            | Self::LockHeld { .. }
            | Self::Backup
            | Self::Restore => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StorageError;

    #[test]
    fn codes_are_namespaced_and_unique() {
        let errors = [
            StorageError::Open { path: "a".into() },
            StorageError::Migrate,
            StorageError::SqliteTooOld {
                found: "3.40.0".to_owned(),
                required: "3.51.3".to_owned(),
            },
            StorageError::SchemaTooNew {
                found: 5,
                supported: 1,
            },
            StorageError::CompatibilityRecord,
            StorageError::Integrity {
                detail: "d".to_owned(),
            },
            StorageError::LockHeld {
                scope: "migrate".to_owned(),
            },
            StorageError::Backup,
            StorageError::Restore,
            StorageError::Query,
        ];

        let mut codes: Vec<&str> = errors.iter().map(StorageError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len(), "codes must be unique");
        for code in codes {
            assert!(code.starts_with("jarvis."), "{code} must be namespaced");
        }
    }

    #[test]
    fn schema_and_lock_conditions_are_not_retryable() {
        assert!(
            !StorageError::SchemaTooNew {
                found: 2,
                supported: 1
            }
            .retryable()
        );
        assert!(
            !StorageError::LockHeld {
                scope: "s".to_owned()
            }
            .retryable()
        );
        assert!(StorageError::Query.retryable());
    }
}
