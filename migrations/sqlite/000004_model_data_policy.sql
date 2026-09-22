-- JARVIS model data policies and route decisions.
--
-- `docs/contracts/model-data-policy.md` fixes three tables: `model_data_policies`
-- stores **immutable versions**, `model_policy_exceptions` stores separately
-- revocable relaxations, and `model_route_decisions` stores the resolved policy,
-- the requested and effective data policy, the evidence references, the candidate
-- rejection reasons, and the exception reference — with model calls referencing the
-- decision they used.
--
-- Three decisions are deliberate:
--
--   * **A policy version is immutable.** "Changing rules creates a new version", so
--     the row is insert-only and `(policy_id, version)` is unique. An UPDATE could
--     silently rewrite the rules a past route decision was made under, which is the
--     one thing the contract's "historical records retain the policy version needed
--     to explain a past decision" rule exists to prevent.
--   * **Rules are stored as JSON, not as columns.** `PolicyRules` is a typed struct
--     with seven enums and four sets, and flattening it into twenty columns would
--     create twenty places for the stored form and the domain type to disagree —
--     with no single reader that could notice. The version number plus a
--     `rules_json` blob is what the domain already serializes and validates, so
--     there is exactly one representation.
--   * **`route_decisions` is a separate table rather than columns on
--     `model_calls`.** One decision can be reused by a retry's later attempt, and a
--     decision is an auditable artifact in its own right — the contract says the
--     considered candidates and their rejection reasons are auditable without
--     storing prompt content, which is a record with its own identity.
--
-- `policy_id` and `version` are separate columns rather than one composite string,
-- because the contract's `PolicyVersionRef` is a pair and an operator asking "what
-- changed in version 4" should not have to parse a value to find out.

CREATE TABLE model_data_policies (
    -- Globally unique row identity. The natural key is the UNIQUE constraint below.
    id                  TEXT PRIMARY KEY,
    -- The policy this version belongs to. One logical policy, many versions.
    policy_id           TEXT NOT NULL,
    -- The immutable version number, from 1.
    version             INTEGER NOT NULL CHECK (version >= 1),
    workspace_id        TEXT NOT NULL,
    -- The operator-facing name, so a workspace with several policies is readable.
    name                TEXT NOT NULL,
    -- The lifecycle state. Only an active version is resolved for a new call.
    status              TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    -- The typed rules, serialized. See the header for why this is not columns.
    rules_json          TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    -- One version exists once. A re-insert is a conflict, not an update.
    UNIQUE (policy_id, version),
    -- A workspace cannot have two active policies with one name at the same version,
    -- which is what stops a rename from shadowing an existing policy.
    UNIQUE (workspace_id, name, version)
);

-- Resolving a workspace's active policy is the read every call performs, so it is
-- indexed rather than scanned.
CREATE INDEX idx_model_data_policies_workspace_status
    ON model_data_policies (workspace_id, status);

-- Separately revocable relaxations. An exception is a durable record with its own
-- identity because the contract requires issuance, expiry, revocation, and
-- single-use state to be tracked independently of the policy it relaxes.
CREATE TABLE model_policy_exceptions (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL,
    -- The rule the exception relaxes, named rather than described.
    rule                TEXT NOT NULL,
    -- Why it was granted and what it permits. Bounded text, never content.
    reason              TEXT NOT NULL,
    -- Who granted it, and the scope it applies to.
    granted_by          TEXT NOT NULL,
    scope_ref           TEXT,
    -- Expiry is required: the contract says exceptions "expire by default".
    expires_at          TEXT NOT NULL,
    -- Revocation is a state rather than a delete, so the record survives to explain
    -- why a past call was permitted.
    revoked_at          TEXT,
    -- A single-use exception is consumed once; this records when.
    consumed_at         TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_model_policy_exceptions_workspace_rule
    ON model_policy_exceptions (workspace_id, rule);

-- One persisted route decision per model call, reusable across a retry's attempts.
CREATE TABLE model_route_decisions (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL,
    -- The policy version this decision was made under. Both columns are stored so a
    -- decision can be read without joining the policy table, which matters because
    -- the policy may since have been archived.
    policy_id           TEXT NOT NULL,
    policy_version      INTEGER NOT NULL CHECK (policy_version >= 1),
    -- The requested and effective data policy, and the considered candidates with
    -- their rejection reasons. Serialized for the same reason the rules are.
    requested_json      TEXT NOT NULL,
    effective_json      TEXT NOT NULL,
    -- The evidence notes that supported the selection.
    evidence_refs_json  TEXT,
    -- The candidates that were rejected, with reason codes.
    rejections_json     TEXT,
    exception_id        TEXT,
    decided_at          TEXT NOT NULL,
    FOREIGN KEY (exception_id) REFERENCES model_policy_exceptions (id) ON DELETE SET NULL
);

CREATE INDEX idx_model_route_decisions_workspace_decided
    ON model_route_decisions (workspace_id, decided_at);

-- Record the new schema version. The minimum reader stays at 1, because the
-- migration is purely additive: every existing table and column is untouched.
UPDATE schema_version
SET schema_version = 4,
    min_reader_version = 1,
    writer_version = '0.1.0',
    updated_at = '1970-01-01T00:00:00Z'
WHERE id = 1;
