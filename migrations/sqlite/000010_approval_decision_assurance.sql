-- `approval_transitions.actor_json` already serializes the `ApprovalActor`, so the decision's assurance
-- needs no new column: it travels inside the existing actor document, and the `Decided` variant gained
-- the field. This migration exists to record that decision explicitly and to give the value a
-- **queryable** projection, because the audit requirement is that an operator can ask which decisions
-- were made by a stepped-up caller — and a value only reachable by parsing a JSON document inside a
-- row-per-transition table is a value an operator cannot audit.
--
-- The column is nullable, and that is deliberate rather than an oversight. Every existing row was written
-- before the assurance was recorded, so it has no value to be back-filled with: defaulting them to
-- `standard` would assert that an unknown caller held an ordinary credential, which is a claim the
-- record never established. NULL means "not recorded", which is the honest state, and a reader reports
-- it as absent rather than as the weakest level.
ALTER TABLE approvals ADD COLUMN decided_assurance TEXT;

-- The audit query this column exists for: which decisions were made at a given assurance, newest first.
-- Partial, because a null assurance is "not recorded" rather than a level to index.
CREATE INDEX idx_approvals_decided_assurance
    ON approvals (workspace_id, decided_assurance, decided_at)
    WHERE decided_assurance IS NOT NULL;
