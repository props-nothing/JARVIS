//! Schema-version compatibility.
//!
//! A binary declares the highest schema version it can read. A database whose
//! `schema_version` is higher than that was written by a newer binary, and this
//! binary must refuse to write it rather than silently altering state it does
//! not understand (see `docs/data/migrations.md`).

use sqlx::{Row as _, SqlitePool};

use super::error::StorageError;

/// The schema version this binary targets and writes.
///
/// Bumped to `3` by `000003_idempotency.sql`, which adds the idempotency records
/// the local control API's `Idempotency-Key` requirement needs. The minimum reader
/// stays at `1`: both later migrations are purely additive, so a binary that
/// understands only the initial schema can still read a database that has the
/// added tables.
pub const TARGET_SCHEMA_VERSION: i64 = 5;

/// The lowest schema version this binary can still read.
///
/// `1` rather than `2`: migration from a supported prior version is required (see
/// `docs/data/migrations.md`), so a version-1 database must be readable and
/// migratable rather than refused.
pub const MIN_SUPPORTED_SCHEMA_VERSION: i64 = 1;

/// The compatibility state discovered in a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compatibility {
    /// The schema version present in the database.
    pub schema_version: i64,
    /// The oldest binary that may still read the database.
    pub min_reader_version: i64,
    /// The binary that last wrote the database.
    pub writer_version: String,
    /// The instant of the last compatibility update.
    pub updated_at: String,
}

impl Compatibility {
    /// Returns whether this binary may read the database.
    #[must_use]
    pub fn is_readable(&self) -> bool {
        self.schema_version >= MIN_SUPPORTED_SCHEMA_VERSION
            && self.schema_version <= TARGET_SCHEMA_VERSION
    }

    /// Returns whether the database needs a migration before writing.
    #[must_use]
    pub fn needs_migration(&self) -> bool {
        self.schema_version < TARGET_SCHEMA_VERSION
    }
}

/// Reads the compatibility record, refusing a newer-than-supported database.
///
/// # Errors
///
/// Returns [`StorageError::SchemaTooNew`] when the database is newer than this
/// binary, [`StorageError::CompatibilityRecord`] when the record is missing or
/// has the wrong shape, and [`StorageError::Query`] for a driver failure.
pub async fn read_compatibility(pool: &SqlitePool) -> Result<Compatibility, StorageError> {
    let row = sqlx::query(
        "SELECT schema_version, min_reader_version, writer_version, updated_at \
         FROM schema_version WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| StorageError::Query)?
    .ok_or(StorageError::CompatibilityRecord)?;

    let schema_version: i64 = row
        .try_get("schema_version")
        .map_err(|_| StorageError::CompatibilityRecord)?;
    if schema_version > TARGET_SCHEMA_VERSION {
        return Err(StorageError::SchemaTooNew {
            found: schema_version,
            supported: TARGET_SCHEMA_VERSION,
        });
    }

    Ok(Compatibility {
        schema_version,
        min_reader_version: row
            .try_get("min_reader_version")
            .map_err(|_| StorageError::CompatibilityRecord)?,
        writer_version: row
            .try_get("writer_version")
            .map_err(|_| StorageError::CompatibilityRecord)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| StorageError::CompatibilityRecord)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{Compatibility, MIN_SUPPORTED_SCHEMA_VERSION, TARGET_SCHEMA_VERSION};

    fn compatibility(schema_version: i64) -> Compatibility {
        Compatibility {
            schema_version,
            min_reader_version: 1,
            writer_version: "0.1.0".to_owned(),
            updated_at: "2026-09-21T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn the_target_version_is_readable_and_needs_no_migration() {
        let current = compatibility(TARGET_SCHEMA_VERSION);
        assert!(current.is_readable());
        assert!(!current.needs_migration());
    }

    #[test]
    fn an_older_but_supported_version_is_readable_and_needs_migration() {
        // Migration from a supported prior version is required, so version 1 must
        // be readable *and* reported as needing migration — not refused.
        let older = compatibility(MIN_SUPPORTED_SCHEMA_VERSION);
        assert!(older.is_readable());
        assert!(older.needs_migration());
    }

    #[test]
    fn the_supported_version_range_is_ordered() {
        // A minimum reader above the target would describe a database this binary
        // writes but refuses to read. Compared as values rather than asserted as
        // constants, so the check is a real comparison rather than a lint target.
        let minimum = MIN_SUPPORTED_SCHEMA_VERSION;
        let target = TARGET_SCHEMA_VERSION;
        assert!(
            minimum <= target,
            "the minimum reader must not exceed the target"
        );
    }

    #[test]
    fn a_newer_version_is_not_readable() {
        let newer = compatibility(TARGET_SCHEMA_VERSION + 1);
        assert!(!newer.is_readable());
    }
}
