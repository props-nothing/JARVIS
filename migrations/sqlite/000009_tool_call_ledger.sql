-- The durable tool-call ledger: the persistence half of `TLS-006`.
--
-- The domain half already exists — `jarvis_domain::tool::ledger` holds the eleven state machine, the
-- transition table, the five-dimension reservation key, and the `ACC-025` recovery classification — and
-- its own TODO recorded the honest gap that "durable" meant the *shape* was durable and serialization
-- tested rather than that a row existed. Its most load-bearing sentence is a claim about a *process*:
--
--   *"the reservation is atomic only within one process's memory: the adapter that makes it atomic
--   across processes is part of the persistence work, not of this module."*
--
-- This migration is the table that makes that claim true. An in-memory map can make a duplicate
-- submission find the existing row when both arrive at one daemon; it cannot when they arrive at two,
-- and a second process is exactly the case that produces a second side effect.
--
-- **Why one table rather than a column on an existing one.** The ledger row is per *attempt*, not per
-- call and not per run: an invocation that is retried has several rows sharing one reservation key, and
-- the attempt number is what distinguishes them. `agent_steps` is a step, not an attempt, and `agent_runs`
-- is a run. Neither can represent "the same reservation asked for twice".
--
-- **`tool_identity_json` and `idempotency_key` are separate columns even though both are key material**,
-- because they have different lifetimes and different readers: the identity is what the unique index
-- deduplicates on and what an operator reads to see *which tool* collided, while the key is the caller's
-- own string, bounded and validated at the port. Folding them into one hashed column would make a
-- collision undiagnosable — the operator would learn that *something* collided and not what.
--
-- **The unique index is over the serialized identity *text*, and that is only sound because the domain's
-- serializer is deterministic.** `ToolIdentity` is a struct whose fields serde writes in declaration
-- order, so two equal identities produce byte-identical documents and the index deduplicates them. A
-- representation whose serialization could reorder fields — a map, or a struct with a hash-set field —
-- would make this index silently stop deduplicating while still existing, which is the "exists but
-- enforced nowhere" shape this project keeps finding. The adapter asserts the determinism rather than
-- assuming it.
--
-- **`dispatched_at` is a column, not a derivation from `state`.** This is the `TLS-006` defect recorded
-- in the domain: `may_have_effected()` was once `state.was_dispatched()`, so a call that reached
-- `EXECUTING` and then ended `FAILED` — a state that is *not* dispatched — answered "no effect", and a
-- dispatched call that failed would have been retried, duplicating the effect. Dispatch is a thing that
-- happened once. Persisting it separately means the fact survives the state moving on, in the database
-- exactly as it does in the domain.
--
-- **`outcome` is nullable and that is the point**: `NULL` means the outcome is unknown, which is a
-- different fact from "it failed". A terminal row with a `NULL` outcome must be refused by the reader
-- rather than defaulted, because defaulting it would let a row claim an unknown outcome as a recorded
-- failure and a caller would then retry it.
--
-- **`no_effect_confirmed` is an integer, not a nullable boolean**, so there is no third state to
-- interpret. The reader accepts 0 and 1 and refuses anything else as corruption: a stored 2 is a value
-- the writer could not have produced, which is a migration fault rather than a claim about effects.
--
-- **`state` has NO default**, for the reason `000008` recorded for approvals: a row whose lifecycle
-- position nobody chose must be a write error rather than a plausible-looking row, and defaulting to
-- `requested` is the fail-open direction — a requested call is one a later pass may retry.
--
-- **Three indexes, one per query the port has:**
--
--   * `idx_tool_call_records_reservation` is the **unique** one and it is what makes the reservation
--     atomic: two concurrent reserves of the same key cannot both insert, so one gets a constraint
--     failure and is answered with the row that won. Leading with `workspace_id` and `principal_id`
--     keeps the five-dimension scope in the key rather than in a filter applied afterwards.
--   * `idx_tool_call_records_unsettled` covers the startup scan for dispatched calls with no outcome.
--     It is keyed on `(state, updated_at)` and **deliberately not scoped by workspace**, because
--     startup recovery is a whole-profile concern: a scan scoped to one workspace would leave every
--     other workspace's unsettled calls unsettled, with no symptom — the same reasoning
--     `RunRepository::incomplete_runs` records.
--   * `idx_tool_call_records_run` answers "which calls did this run make", which is how an operator
--     finds the call a run is waiting on.

CREATE TABLE tool_call_records (
    id                  TEXT PRIMARY KEY,
    call_id             TEXT NOT NULL,
    workspace_id        TEXT NOT NULL,
    principal_id        TEXT NOT NULL,
    run_id              TEXT NOT NULL,
    tool_identity_json  TEXT NOT NULL,
    idempotency_key     TEXT NOT NULL,
    attempt             INTEGER NOT NULL,
    operation           TEXT NOT NULL,
    state               TEXT NOT NULL,
    version             INTEGER NOT NULL,
    dispatched_at       TEXT,
    outcome             TEXT,
    no_effect_confirmed INTEGER NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES agent_runs (id) ON DELETE CASCADE
);

-- The audit trail of ledger transitions. One row per applied transition, written in the **same
-- transaction** as the state change, for the reason `000008`'s approval trail records: a trail row
-- describing a transition that is not durable, or a durable transition with no trail row, are both
-- states where the audit record and the thing it audits disagree. For a tool call the consequence is
-- sharper than for a run — the trail is what says whether an effect was *dispatched*, which is the one
-- fact a retry decision turns on.
CREATE TABLE tool_call_transitions (
    id                  TEXT PRIMARY KEY,
    record_id           TEXT NOT NULL,
    workspace_id        TEXT NOT NULL,
    from_state          TEXT NOT NULL,
    to_state            TEXT NOT NULL,
    prior_version       INTEGER NOT NULL,
    version             INTEGER NOT NULL,
    outcome             TEXT,
    occurred_at         TEXT NOT NULL,
    FOREIGN KEY (record_id) REFERENCES tool_call_records (id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX idx_tool_call_records_reservation
    ON tool_call_records (workspace_id, principal_id, tool_identity_json, idempotency_key);

CREATE INDEX idx_tool_call_records_unsettled
    ON tool_call_records (state, updated_at);

CREATE INDEX idx_tool_call_records_run
    ON tool_call_records (workspace_id, run_id);

CREATE INDEX idx_tool_call_transitions_record
    ON tool_call_transitions (record_id, occurred_at);

-- Record the new schema version. The minimum reader stays at 1: this migration only **adds** tables, and
-- a table an older binary never reads cannot break it. That is the same reasoning `000002` and `000008`
-- recorded, and the opposite of a migration that changes a shape an older reader names.
UPDATE schema_version
SET schema_version = 9,
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE schema_version < 9;
