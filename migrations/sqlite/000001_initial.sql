-- JARVIS initial SQLite schema.
--
-- This migration establishes the durability primitives the Foundation slice
-- needs: applied-migration bookkeeping, scoped maintenance locks, bounded
-- diagnostic findings, and the schema-version record that lets a binary refuse
-- to write state whose writer version it does not support.
--
-- Conventions (see docs/data/schema.md and docs/contracts/common-conventions.md):
--   * identifiers are canonical lowercase UUIDv7 text;
--   * absolute time is RFC 3339 UTC text with a trailing `Z`;
--   * workspace scope is a column, not an implicit filter.
--
-- `schema_migrations` is intentionally NOT created here: SQLx owns that table
-- and derives its content from the applied migration files. Creating it in a
-- migration would conflict with the migrator's own bookkeeping.

-- The compatibility record read at startup before any write is attempted.
CREATE TABLE schema_version (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version      INTEGER NOT NULL,
    -- The oldest binary that may still read this database.
    min_reader_version  INTEGER NOT NULL,
    -- The binary that last wrote it.
    writer_version      TEXT    NOT NULL,
    updated_at          TEXT    NOT NULL
);

-- Exactly one row exists, so the database is never in an "unversioned" state.
INSERT INTO schema_version (id, schema_version, min_reader_version, writer_version, updated_at)
VALUES (1, 1, 1, '0.1.0', '1970-01-01T00:00:00Z');

-- Scoped maintenance locks: migrations, backup/restore, and repair take one.
CREATE TABLE application_locks (
    scope               TEXT PRIMARY KEY,
    holder              TEXT NOT NULL,
    acquired_at         TEXT NOT NULL,
    expires_at          TEXT NOT NULL,
    -- A lock is advisory metadata; the authoritative guard is a held file lock.
    UNIQUE (scope, holder)
);

-- Bounded, safe operational findings produced by doctor and startup checks.
CREATE TABLE diagnostic_events (
    id                  TEXT PRIMARY KEY,
    scope               TEXT NOT NULL,
    severity            TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'error')),
    -- A stable machine code, never a free-form message or payload.
    code                TEXT NOT NULL,
    -- Safe operator detail: never a secret, payload, or raw internal error.
    detail              TEXT,
    occurred_at         TEXT NOT NULL
);

CREATE INDEX idx_diagnostic_events_scope_time
    ON diagnostic_events (scope, occurred_at);
