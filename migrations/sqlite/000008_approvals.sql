-- Approval records: the durable half of `TLS-005`.
--
-- The domain half already exists — `jarvis_domain::tool::approval` holds the seven-state machine, the
-- transition table, the decision provenance, and the bounds — and its own TODO recorded the honest gap
-- that "durable" meant the *shape* was durable and serialization-tested rather than that a row existed.
-- This migration is the table that closes it, so an approval survives a restart with its decision, its
-- channel, and its version intact.
--
-- **Why a table rather than a column on `tool_calls`.** An approval is requested *before* the call it
-- authorizes exists — that is the whole point of asking a person first — so a record keyed by the call
-- could not represent the state this feature is in during the window that matters. It also has its own
-- lifecycle (revocable, expiring, one-shot-consumable) that outlives any single attempt.
--
-- **Every field of the decision is stored, and the two that are easy to omit are the ones with teeth:**
--
--   * `action_fingerprint` is stored so an approval can be matched to the exact action it approved.
--     The contract is explicit that "approving 'send this email' does not approve a rewritten
--     recipient, subject, body, attachment, or account", and a record that stored only the tool would
--     authorize any action of that tool. The *computation* of the fingerprint is still not implemented
--     (`BRN-049`'s sibling gap, recorded in `TLS-005`): the column exists and is written from the
--     domain value, and the research note that would let JARVIS compute one is not yet written.
--   * `decided_via` records the **channel** a decision came from. The contract lists `allowed_channels`
--     so the channel can be checked, and a stored decision that did not record which one it used would
--     make the check one-directional: JARVIS could refuse a disallowed channel going in but could never
--     say, afterwards, which channel a decision actually came from.
--
-- **`tool_identity_json` is the whole identity as one document, not five columns.** The identity is a
-- tuple — capability, source, provenance, schema fingerprint — and the domain type's serialization is
-- the canonical spelling of it, already round-trip tested. Splitting it into columns would create a
-- second definition of what the identity is, and the first thing that would drift is the fingerprint's
-- algorithm prefix. The cost is that a query cannot filter by source owner without parsing; no query
-- does, and adding an index over parsed JSON would be the optimization to add *when* one needs it.
--
-- **The preview and effects are JSON for a different reason: they are display material.** A preview is
-- rendered into a prompt and has no queryable structure, and the effects list is a set that policy
-- already re-derives from the tool's definition at evaluation time. Storing them is for the *record*
-- — what the user was shown and what they agreed to — which is why they are stored verbatim rather
-- than rebuilt from the tool, whose definition may since have changed.
--
-- **`state` has NO default**, deliberately. A row whose state was unset would be one whose lifecycle
-- position nobody chose, and a default of `pending` would make it look like an undecided request —
-- the fail-open direction, since a pending approval is one a later pass may decide. A `NOT NULL` column
-- with no default makes an unset state a write error instead of a plausible-looking row.
--
-- `version` starts at 1 in the writer, not here: the domain's `ApprovalVersion::FIRST` is 1 so an
-- uninitialised field cannot read as a valid stored version, and a column default would be a second
-- place that decides what "first" means.
--
-- **Three indexes, one per query the port actually has:**
--
--   * `idx_approvals_pending` covers `pending_in`, which lists what is waiting in a workspace. Keyed on
--     `(workspace_id, state, expires_at)` so the listing can also order by what lapses soonest.
--   * `idx_approvals_decided_by` covers `decided_by`, which answers "what did I approve?". Keyed on
--     `(workspace_id, decided_by, decided_at)` because the port's contract is "most recent first".
--   * `idx_approvals_tool_call` covers the lookup every tool call performs — "is there an approval for
--     this call?" — which is the hot path and would otherwise be a table scan per call.
--
-- `workspace_id` leads the first two because an approval from another workspace must be
-- **indistinguishable from a missing one**, which is the rule the local control API states for runs and
-- the same reason the port resolves scope in the query rather than filtering afterwards.

CREATE TABLE approvals (
    id                      TEXT PRIMARY KEY,
    workspace_id            TEXT NOT NULL,
    requesting_principal_id TEXT NOT NULL,
    run_id                  TEXT NOT NULL,
    tool_call_id            TEXT NOT NULL,
    tool_identity_json      TEXT NOT NULL,
    action_fingerprint      TEXT NOT NULL,
    risk                    TEXT NOT NULL,
    effects_json            TEXT NOT NULL,
    summary                 TEXT NOT NULL,
    preview_json            TEXT NOT NULL,
    allowed_channels_json   TEXT NOT NULL,
    expires_at              TEXT NOT NULL,
    scope                   TEXT NOT NULL,
    state                   TEXT NOT NULL,
    version                 INTEGER NOT NULL,
    decided_by              TEXT,
    decided_via             TEXT,
    decided_at              TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES agent_runs (id) ON DELETE CASCADE
);

-- The audit trail of decisions. One row per applied transition, written in the **same transaction** as
-- the state change so the trail cannot disagree with the state it describes — the architecture's
-- "persist state before publishing an event that claims the transition happened" rule applied to a
-- decision rather than to a run.
CREATE TABLE approval_transitions (
    id                  TEXT PRIMARY KEY,
    approval_id         TEXT NOT NULL,
    workspace_id        TEXT NOT NULL,
    from_state          TEXT NOT NULL,
    to_state            TEXT NOT NULL,
    prior_version       INTEGER NOT NULL,
    version             INTEGER NOT NULL,
    actor_kind          TEXT NOT NULL,
    actor_json          TEXT NOT NULL,
    occurred_at         TEXT NOT NULL,
    FOREIGN KEY (approval_id) REFERENCES approvals (id) ON DELETE CASCADE
);

CREATE INDEX idx_approvals_pending
    ON approvals (workspace_id, state, expires_at);

CREATE INDEX idx_approvals_decided_by
    ON approvals (workspace_id, decided_by, decided_at);

CREATE INDEX idx_approvals_tool_call
    ON approvals (workspace_id, tool_call_id);

CREATE INDEX idx_approval_transitions_approval
    ON approval_transitions (approval_id, occurred_at);

-- Record the new schema version. The minimum reader stays at 1: this migration only **adds** tables, and
-- a table an older binary never reads cannot break it. That is the same reasoning `000002` recorded when
-- it created the run tables, and the opposite of a migration that changes a shape an older reader names.
UPDATE schema_version
SET schema_version = 8,
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE schema_version < 8;
