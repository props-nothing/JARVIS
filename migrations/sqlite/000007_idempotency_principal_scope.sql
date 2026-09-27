-- Scope idempotency records to the principal, and not only to the workspace.
--
-- The contract says an `Idempotency-Key` is "scoped to authenticated principal, resolved
-- workspace/profile, client credential, operation, and API major" — **five** dimensions. The
-- `000003` table keyed on three of them (`workspace_id`, `operation`, `api_major`), and the
-- migration's own comment claimed all five, so the omission read as implemented.
--
-- **What that permitted, and why it is a security defect rather than a scoping nicety.** A local
-- profile has exactly ONE workspace (`DEFAULT_WORKSPACE_UUID`), shared by every enrolled client,
-- while each client resolves to its OWN principal. So two clients presented the same key inside one
-- workspace — and the replay path loaded the original run by workspace **only**, with no check that
-- the run belonged to the principal asking. Client B, guessing a key client A had used, received
-- A's `run_id` **and its `conversation_id`**, which is a handle onto A's conversation. The keys are
-- guessable in practice: the CLI derives one from the clock and its own process id.
--
-- Two remedies, and the second is what makes the first safe to rely on:
--
--   * The uniqueness key gains `principal_id`, so one principal's key cannot collide with
--     another's, and a replay is a replay of *one principal's own* command.
--   * The credential the record was created with is stored, because the contract scopes to it too
--     and a principal can hold more than one credential. Recording it means a rotation is
--     *observable* in the record rather than indistinguishable from a plain replay — and a
--     rotation should produce `idempotency.conflict`, not a silent match on the same key.
--
-- The table is **rebuilt** rather than altered in place, for one reason that decides the whole
-- shape: the `000003` `UNIQUE (workspace_id, operation, api_major, idempotency_key)` constraint is
-- strictly weaker than the five-part scope, so it would refuse a legitimate second principal's use
-- of a shared key *before* the stronger index could allow it — and SQLite cannot drop a table
-- constraint. Rebuilding also means the new columns are declared `NOT NULL` with no default and
-- populated in the same statement that creates the rows, which is stronger than adding a nullable
-- column and backfilling afterwards: a row with an unset principal is then **unrepresentable**
-- rather than merely absent.
--
-- **Existing rows are attributed from their run**, which is the only principal the record can
-- honestly be attributed to: `agent_runs.principal_id` is NOT NULL and the record's `run_id` is a
-- foreign key to it. That is exact rather than a guess — the run recorded who asked, and the record
-- is about that same request.
--
-- The client credential cannot be backfilled: nothing stored which credential created an old row,
-- and inventing one would claim a fact never observed. It is recorded as the `unknown` sentinel,
-- which cannot equal a real digest (a digest is hex, and domain-separated by a `credential:`
-- prefix). So an old record never matches on the credential dimension, and a replayed command after
-- this upgrade answers `idempotency.conflict` rather than silently resuming a record whose
-- credential is unknown. That is the fail-closed direction and the honest one.

CREATE TABLE idempotency_records_rebuilt (
    id                  TEXT PRIMARY KEY,
    idempotency_key     TEXT NOT NULL,
    workspace_id        TEXT NOT NULL,
    principal_id        TEXT NOT NULL,
    client_credential   TEXT NOT NULL,
    operation           TEXT NOT NULL,
    api_major           INTEGER NOT NULL,
    request_digest      TEXT NOT NULL,
    run_id              TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES agent_runs (id) ON DELETE CASCADE
);

INSERT INTO idempotency_records_rebuilt
    (id, idempotency_key, workspace_id, principal_id, client_credential, operation, api_major,
     request_digest, run_id, created_at)
SELECT
    id,
    idempotency_key,
    workspace_id,
    COALESCE(
        (SELECT principal_id FROM agent_runs WHERE agent_runs.id = idempotency_records.run_id),
        'unknown'
    ),
    'unknown',
    operation,
    api_major,
    request_digest,
    run_id,
    created_at
FROM idempotency_records;

DROP TABLE idempotency_records;

ALTER TABLE idempotency_records_rebuilt RENAME TO idempotency_records;

-- Recreated against the rebuilt table, since the old indexes went with the old table.
CREATE INDEX idx_idempotency_records_run
    ON idempotency_records (run_id);

-- The five-part scope, as one index. A named index rather than a table constraint because the
-- contract's scope is a statement about this surface that a test can read back and assert, and
-- because it is the form SQLite lets a later migration change.
CREATE UNIQUE INDEX idx_idempotency_records_scope ON idempotency_records (
    principal_id,
    workspace_id,
    client_credential,
    operation,
    api_major,
    idempotency_key
);

-- Record the new schema version. The minimum reader stays at 1: the table's shape changed, but every
-- column an older reader named still exists with the same meaning, and an older binary reaches
-- idempotency records only through this table's own queries.
UPDATE schema_version
SET schema_version = 7,
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE schema_version < 7;
