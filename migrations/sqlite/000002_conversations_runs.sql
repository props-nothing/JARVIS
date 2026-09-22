-- JARVIS conversation, run, activity, and model-call state.
--
-- This migration adds the durable aggregates the Brain slice needs. It follows
-- the same conventions as the initial migration (see docs/data/schema.md and
-- docs/contracts/common-conventions.md):
--
--   * identifiers are canonical lowercase UUIDv7 text;
--   * absolute time is RFC 3339 UTC text with a trailing `Z`;
--   * workspace scope is a column, not an implicit filter, so a query that
--     forgets it is visibly missing a predicate rather than silently correct;
--   * mutable aggregates carry `version` for optimistic transitions.
--
-- Two constraints are load-bearing rather than decorative:
--
--   * `agent_runs` refuses a waiting state whose dependency is unset and a
--     terminal state whose completion instant is unset, which is the schema
--     document's "constraints preventing incompatible combinations". Without
--     them a row could read as waiting with nothing to wait for, or as finished
--     with no instant, and both would look plausible to a reader.
--   * every run-scoped table references `agent_runs` with a foreign key, so an
--     orphaned activity row or model call cannot outlive its run.

-- A durable conversation. Named `conversations`, not `sessions`: the identity
-- architecture reserves "session" for a bounded *authenticated interaction*
-- context (see docs/architecture/identity-workspaces.md), and the local control
-- API addresses this aggregate as `conversation_id`.
CREATE TABLE conversations (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL,
    owner_user_id   TEXT NOT NULL,
    title           TEXT,
    status          TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    channel_origin  TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    archived_at     TEXT,
    -- An archived conversation carries the instant it was archived, and an
    -- active one must not pretend it was.
    CHECK (
        (status = 'archived' AND archived_at IS NOT NULL)
        OR (status = 'active' AND archived_at IS NULL)
    )
);

CREATE INDEX idx_conversations_workspace_updated
    ON conversations (workspace_id, updated_at DESC);

-- Durable messages. `sequence` is per conversation and unique, so a replayed
-- append cannot create a second row at one position and silently reorder the
-- transcript.
CREATE TABLE messages (
    id                      TEXT PRIMARY KEY,
    workspace_id            TEXT NOT NULL,
    conversation_id         TEXT NOT NULL,
    role                    TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content_schema_version  INTEGER NOT NULL,
    content_ref_or_json     TEXT NOT NULL,
    sensitivity             TEXT NOT NULL,
    source                  TEXT NOT NULL,
    sequence                INTEGER NOT NULL CHECK (sequence >= 1),
    created_at              TEXT NOT NULL,
    deleted_at              TEXT,
    UNIQUE (conversation_id, sequence),
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE CASCADE
);

CREATE INDEX idx_messages_conversation_sequence
    ON messages (conversation_id, sequence);

-- One durable agent run. The state set matches `jarvis_domain::run::RunState`
-- exactly; a state added there without a migration here would fail this CHECK,
-- which is the intended loud failure rather than a silently unconstrained row.
CREATE TABLE agent_runs (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL,
    conversation_id     TEXT NOT NULL,
    parent_run_id       TEXT,
    principal_id        TEXT NOT NULL,
    objective_ref       TEXT,
    state               TEXT NOT NULL CHECK (state IN (
                            'received', 'context_building', 'planning',
                            'awaiting_model', 'awaiting_approval', 'executing_tool',
                            'observing', 'waiting', 'responding',
                            'completed', 'failed', 'cancelled'
                        )),
    version             INTEGER NOT NULL CHECK (version >= 1),
    runtime_id          TEXT,
    runtime_version     TEXT,
    context_manifest_id TEXT,
    plan_summary_ref    TEXT,
    waiting_kind        TEXT,
    waiting_ref         TEXT,
    deadline_at         TEXT,
    budget_json         TEXT,
    result_ref          TEXT,
    error_code          TEXT,
    error_ref           TEXT,
    created_at          TEXT NOT NULL,
    started_at          TEXT,
    updated_at          TEXT NOT NULL,
    completed_at        TEXT,
    -- A waiting run names what it waits for; nothing else does. A run that is
    -- merely not progressing has no dependency, and the two must not look alike.
    CHECK (
        (state IN ('awaiting_approval', 'waiting')
            AND waiting_kind IS NOT NULL AND waiting_ref IS NOT NULL)
        OR (state NOT IN ('awaiting_approval', 'waiting')
            AND waiting_kind IS NULL AND waiting_ref IS NULL)
    ),
    -- A terminal run carries the instant it finished; a live one must not.
    CHECK (
        (state IN ('completed', 'failed', 'cancelled') AND completed_at IS NOT NULL)
        OR (state NOT IN ('completed', 'failed', 'cancelled') AND completed_at IS NULL)
    ),
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE CASCADE
);

CREATE INDEX idx_agent_runs_workspace_conversation
    ON agent_runs (workspace_id, conversation_id, created_at);
CREATE INDEX idx_agent_runs_workspace_state
    ON agent_runs (workspace_id, state);

-- Durable steps. Each nondeterministic or externally visible operation is a step
-- with an idempotency key, so a retry reuses identity rather than creating a new
-- logical operation.
CREATE TABLE agent_steps (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL,
    run_id              TEXT NOT NULL,
    sequence            INTEGER NOT NULL CHECK (sequence >= 1),
    kind                TEXT NOT NULL,
    state               TEXT NOT NULL,
    version             INTEGER NOT NULL CHECK (version >= 1),
    idempotency_key     TEXT NOT NULL,
    input_fingerprint   TEXT,
    input_ref           TEXT,
    output_ref          TEXT,
    attempt_count       INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts        INTEGER NOT NULL CHECK (max_attempts >= 1),
    timeout_ms          INTEGER,
    next_attempt_at     TEXT,
    error_code          TEXT,
    created_at          TEXT NOT NULL,
    started_at          TEXT,
    completed_at        TEXT,
    UNIQUE (run_id, sequence),
    UNIQUE (workspace_id, run_id, idempotency_key),
    FOREIGN KEY (run_id) REFERENCES agent_runs (id) ON DELETE CASCADE
);

-- The public activity projection. It is append-oriented and is *not* the private
-- model trace: payloads are already-redacted public detail, and the client stream
-- replays from here in sequence order.
CREATE TABLE run_activity_events (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL,
    run_id          TEXT NOT NULL,
    sequence        INTEGER NOT NULL CHECK (sequence >= 1),
    event_type      TEXT NOT NULL,
    payload_json    TEXT,
    visibility      TEXT NOT NULL CHECK (visibility IN ('public', 'operator')),
    occurred_at     TEXT NOT NULL,
    UNIQUE (run_id, sequence),
    FOREIGN KEY (run_id) REFERENCES agent_runs (id) ON DELETE CASCADE
);

CREATE INDEX idx_run_activity_events_run_sequence
    ON run_activity_events (run_id, sequence);

-- One attempt of one logical model call. `UNIQUE(logical_call_id, attempt)` is
-- what makes a retry an *attempt* of the same logical call rather than a new
-- call, which is the distinction the retry-ownership rule depends on.
CREATE TABLE model_calls (
    id                          TEXT PRIMARY KEY,
    workspace_id                TEXT NOT NULL,
    run_id                      TEXT NOT NULL,
    step_id                     TEXT,
    logical_call_id             TEXT NOT NULL,
    attempt                     INTEGER NOT NULL CHECK (attempt >= 1),
    provider_id                 TEXT NOT NULL,
    model_id                    TEXT NOT NULL,
    model_revision              TEXT,
    route_decision_id           TEXT,
    state                       TEXT NOT NULL,
    request_fingerprint         TEXT,
    provider_request_id         TEXT,
    continuation_ref            TEXT,
    usage_json                  TEXT,
    estimated_cost_microunits   INTEGER,
    finish_reason               TEXT,
    error_code                  TEXT,
    started_at                  TEXT NOT NULL,
    first_output_at             TEXT,
    completed_at                TEXT,
    UNIQUE (logical_call_id, attempt),
    FOREIGN KEY (run_id) REFERENCES agent_runs (id) ON DELETE CASCADE
);

CREATE INDEX idx_model_calls_run
    ON model_calls (workspace_id, run_id, started_at);

-- Record the new schema version. The minimum reader stays at 1, because a binary
-- that only understands the initial schema can still read a database with the
-- added tables: the migration is purely additive.
UPDATE schema_version
SET schema_version = 2,
    min_reader_version = 1,
    writer_version = '0.1.0',
    updated_at = '1970-01-01T00:00:00Z'
WHERE id = 1;
