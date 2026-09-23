-- JARVIS model policy exceptions, in their implemented shape.
--
-- `docs/contracts/model-data-policy.md` requires "separately revocable relaxations" with
-- "issued, expiry, revocation, and single-use state", and `docs/data/schema.md` sketches the
-- table as:
--
--   id, workspace_id, policy_id, policy_version, granting_principal_id,
--   rule_key, constrained_value_json, provider_model_task_scope_json,
--   reason_ref, assurance, state, issued_at, expires_at, revoked_at, consumed_at
--
-- `000004_model_data_policy.sql` created an earlier, placeholder shape for this table ahead of
-- any code that used it. **That table had no writer**, which is what makes replacing it here a
-- migration rather than a data loss: `model_policy_exceptions` was created for the code this
-- migration accompanies, and nothing had ever inserted a row. The `DROP` is therefore safe by
-- construction, and it is a `DROP` rather than a set of `ALTER`s because the column list
-- changes shape (two scope columns become one serialized scope, `rule` becomes `rule_key`,
-- `granted_by` becomes `granting_principal_id`, `created_at` becomes `issued_at`, and `state`
-- and `assurance` are added) — SQLite cannot express that as additive `ALTER TABLE` statements.
--
-- Three deliberate divergences from the sketch in `docs/data/schema.md`, each because the
-- alternative is a second source of truth:
--
--   * **`state` is NOT stored.** The sketch lists it, but this record's state is *derived*
--     from `revoked_at`, `consumed_at`, and `expires_at` — a stored `state` column would be a
--     second answer to "is this usable", and the two would have to be kept in step by every
--     writer. Expiry in particular is a function of *when the question is asked*, so a stored
--     value would be wrong the moment the clock passed `expires_at`. `ExceptionState` is
--     computed at the decision instant instead, which is also what makes a decision replayable.
--   * **The scope is one serialized column, not two.** The sketch splits a "constrained value"
--     from a "provider/model/task scope"; the two are one `ExceptionScope` value whose fields
--     are validated together, so splitting them would be two places for the stored form and the
--     domain type to disagree about, for instance, whether a `region` is part of the value or
--     part of the scope. This is the same reasoning that stores policy rules as `rules_json`
--     rather than twenty columns, and the value is re-parsed through the domain type on every
--     read, so a row this build cannot interpret is reported as corruption rather than passed on.
--   * **`assurance` is stored.** The sketch is right about this and it is worth saying why it is
--     kept rather than recomputed: it records what the granting principal actually held, so an
--     operator who stepped up for a grant approved *that* relaxation. Re-deriving it from the
--     rule later would let a change to the step-up policy retroactively rewrite what a past
--     grant meant.
--
-- The rule key is stored rather than the rule's *effect*, so a reader can tell which rule was
-- relaxed without decoding the scope. Revocation and consumption are recorded as instants rather
-- than by deleting the row, because the contract requires an exception to remain explainable
-- after it stops being usable — "a past call was permitted by this" is precisely the case a
-- delete would destroy.

DROP TABLE IF EXISTS model_policy_exceptions;

CREATE TABLE model_policy_exceptions (
    id                      TEXT PRIMARY KEY,
    workspace_id            TEXT NOT NULL,
    -- The immutable policy version this exception was granted against. Both columns are stored,
    -- like `model_route_decisions`, so the exception can be read without joining the policy
    -- table, which matters because the policy may since have been archived or replaced.
    policy_id               TEXT NOT NULL,
    policy_version          INTEGER NOT NULL CHECK (policy_version >= 1),
    -- Who granted it, and thus who is accountable for it.
    granting_principal_id   TEXT NOT NULL,
    -- The one rule it relaxes, as one of the domain's `PolicyRuleKey` spellings.
    rule_key                TEXT NOT NULL,
    -- The serialized `ExceptionScope`: what the relaxation applies to and what value it permits.
    scope_json              TEXT NOT NULL,
    -- An operator-authored label, bounded and never content. `CHECK` on length here as well as
    -- in the domain, because the bound is a property of the stored value rather than only of
    -- the API that writes it.
    reason_ref              TEXT NOT NULL CHECK (length(reason_ref) BETWEEN 1 AND 1024),
    -- The assurance the granting principal held. Stored, not derived; see the header.
    assurance               TEXT NOT NULL CHECK (assurance IN ('standard', 'elevated')),
    -- Whether one use consumes it. A `CHECK` on a two-value integer rather than a nullable flag,
    -- so "single use" is a stated fact rather than an absence.
    single_use              INTEGER NOT NULL CHECK (single_use IN (0, 1)),
    issued_at               TEXT NOT NULL,
    -- Always present: the contract says exceptions "expire by default", and an exception with no
    -- expiry would be an unbounded relaxation.
    expires_at              TEXT NOT NULL,
    -- Revocation and consumption as instants, never a delete. `CHECK (expires_at > issued_at)` is
    -- enforced in the domain rather than here because SQLite compares TEXT lexically and RFC 3339
    -- UTC strings do order correctly — but the domain refuses an already-expired grant for a
    -- reason a constraint cannot express.
    revoked_at              TEXT,
    consumed_at             TEXT
);

-- Reading the usable exceptions for a workspace is what a route selection performs, so it is
-- indexed on the workspace rather than scanned.
CREATE INDEX idx_model_policy_exceptions_workspace
    ON model_policy_exceptions (workspace_id, expires_at);

-- Record the new schema version. The minimum reader stays at 1: every other table and column is
-- untouched, and the only table whose shape changed is one that had no reader and no writer.
UPDATE schema_version
SET schema_version = 5,
    min_reader_version = 1,
    writer_version = '0.1.0',
    updated_at = '1970-01-01T00:00:00Z'
WHERE id = 1;
