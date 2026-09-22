-- JARVIS idempotency records.
--
-- The local control API contract requires `Idempotency-Key` on run creation and
-- cancellation: a repeated command with the same canonical request returns the
-- original resource, and the same key with different input is a conflict. This
-- migration adds the table that makes that atomic.
--
-- Two decisions are deliberate:
--
--   * The stored value is a **digest of the canonical request**, not the request
--     body. Comparing digests is what answers "same key, different input?", and
--     storing the body would retain caller text in a durable row for no benefit.
--   * The uniqueness key includes the **workspace**, the **operation**, and the
--     **API major** because the contract scopes idempotency to authenticated
--     principal, resolved workspace, client credential, operation, and API major.
--     Without them, one client's key could replay another client's run.
--
-- The table is append-only in practice: a record is written once when a command is
-- first accepted, and a later request only reads it.

CREATE TABLE idempotency_records (
    -- Globally unique row identity. The natural key is the UNIQUE constraint
    -- below; this column exists so a row can be referenced without repeating a
    -- composite key.
    id                  TEXT PRIMARY KEY,
    -- The caller-supplied key, already validated for shape at the boundary.
    idempotency_key     TEXT NOT NULL,
    workspace_id        TEXT NOT NULL,
    -- The scoped operation, such as `runs.create` or `runs.cancel`.
    operation           TEXT NOT NULL,
    -- The API major the command was made under.
    api_major           INTEGER NOT NULL,
    -- The digest of the canonical request body.
    request_digest      TEXT NOT NULL,
    -- The run the first use of this key produced.
    run_id              TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    -- One key means one resource, and only within its own scope and operation.
    UNIQUE (workspace_id, operation, api_major, idempotency_key),
    FOREIGN KEY (run_id) REFERENCES agent_runs (id) ON DELETE CASCADE
);

CREATE INDEX idx_idempotency_records_run
    ON idempotency_records (run_id);

-- Record the new schema version. The minimum reader stays at 1, because the
-- migration is purely additive: a binary that understands schema version 2 can
-- still read a database that has this extra table.
UPDATE schema_version
SET schema_version = 3,
    min_reader_version = 1,
    writer_version = '0.1.0',
    updated_at = '1970-01-01T00:00:00Z'
WHERE id = 1;
