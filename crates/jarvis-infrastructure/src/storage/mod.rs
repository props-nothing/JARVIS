//! Durable local storage.
//!
//! SQLite is the default local backend (see `docs/architecture/storage-data.md`).
//! This module owns the connection profile, migrations, maintenance locks,
//! integrity checks, and backup/restore. It exposes no `SQLx` types to the domain
//! layer: callers receive typed values and coded errors.

pub mod backup;
pub mod connection;
pub mod error;
pub mod lock;
pub mod migrate;
pub mod schema;

pub use backup::{Backup, create as create_backup, restore as restore_backup};
pub use connection::{
    BUSY_TIMEOUT, Database, MAX_POOL_CONNECTIONS, MIN_SQLITE_VERSION, version_at_least,
};
pub use error::StorageError;
pub use lock::{LockScope, MaintenanceLock, acquire as acquire_lock, release as release_lock};
pub use schema::{Compatibility, MIN_SUPPORTED_SCHEMA_VERSION, TARGET_SCHEMA_VERSION};
