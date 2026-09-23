//! Durable local storage.
//!
//! SQLite is the default local backend (see `docs/architecture/storage-data.md`).
//! This module owns the connection profile, migrations, integrity checks, and
//! backup/restore. It exposes no `SQLx` types to the domain
//! layer: callers receive typed values and coded errors.
//!
//! **`application_locks` has no reader and no writer.** The table is created by
//! `000001_initial.sql`, and `docs/data/migrations.md`'s Local Upgrade Flow told an operator to
//! "acquire profile maintenance lock" as step 1 of a schema upgrade. A module
//! (`storage::lock`) implemented exactly that, with tests that covered acquisition, refusal,
//! lease reclamation, and owner-checked release — and **nothing ever called it**, at any point.
//! It was deleted rather than left in place, because a table plus a tested module reads as an
//! implemented control, and an operator following the runbook would have had no way to perform
//! the step or to discover that it could not be performed.
//!
//! The single-instance guard is real and is not here: it is `lifecycle::InstanceGuard`, a
//! held file lock taken by `daemon` before startup work. The deleted module's own module doc
//! said so ("a lock row is *advisory metadata*, not the guard"), which is why deleting it
//! removes no protection — only a second, unused description of one.
//!
//! Deleting it also removed the workspace's **only** production call to `jiff::Timestamp::now()`,
//! which `docs/research/integrations/rust-foundation.md` already asserted was "not used in
//! library code" while it was. Time now enters through `domain::clock::Clock` and the
//! `SystemClock` adapter everywhere.

pub mod backup;
pub mod connection;
pub mod error;
pub mod migrate;
pub mod repositories;
pub mod schema;

pub use backup::{Backup, create as create_backup, restore as restore_backup};
pub use connection::{
    BUSY_TIMEOUT, Database, MAX_POOL_CONNECTIONS, MIN_SQLITE_VERSION, version_at_least,
};
pub use error::StorageError;
pub use schema::{Compatibility, MIN_SUPPORTED_SCHEMA_VERSION, TARGET_SCHEMA_VERSION};
