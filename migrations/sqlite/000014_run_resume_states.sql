-- What a run parked on an approval needs in order to continue.
--
-- A run waiting on a human is not running anywhere: the task that drove it has ended and the daemon
-- may restart before anyone decides. This row is the batch of tool calls the model proposed, which of
-- them already produced results, and which one is waiting, so resuming reads durable state rather than
-- reconstructing a transcript.
--
-- **At most one row per run, replaced when the run parks again.** A stale row would resume the wrong
-- call, so the primary key is the run and a write replaces.
--
-- **The row is written before the run is parked and removed when the run leaves the wait.** A run that
-- reads `awaiting_approval` therefore always has the state to continue from, or is failed instead.
--
-- `state_json` is bounded by a CHECK as well as by the writer: the record carries model-proposed
-- arguments and tool output, and an unbounded column would let one batch grow the database without
-- limit. `state_version` lets a reader refuse a record whose shape it does not know.
--
-- The tables are not foreign-keyed to `runs`: the rest of this schema scopes by `workspace_id` in every
-- statement instead, and a cascading delete would let a pruned run silently remove evidence.

CREATE TABLE run_resume_states (
    workspace_id  TEXT NOT NULL,
    run_id        TEXT NOT NULL,
    approval_id   TEXT NOT NULL,
    state_version INTEGER NOT NULL,
    state_json    TEXT NOT NULL CHECK (length(state_json) <= 1048576),
    created_at    TEXT NOT NULL,
    PRIMARY KEY (workspace_id, run_id)
);

-- Purely additive: a new table an older binary never reads cannot break it, so the minimum reader stays at 1.
UPDATE schema_version
SET schema_version = 14,
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE schema_version < 14;