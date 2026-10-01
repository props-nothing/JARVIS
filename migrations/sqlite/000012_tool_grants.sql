-- Durable, configurable tool authorization: the store `TLS-015` names and the seam
-- `NativeReadOnlyGrants` was built to be replaced at.
--
-- Two tables, and the split is the same one the domain already draws: a **grant** confers, and a
-- **deny rule** refuses. They are separate tables rather than one row with a `kind` column because
-- they have different shapes and different precedence: a deny rule matches on any subset of
-- identity/principal/workspace/effects and is consulted **first**, before any grant, while a grant
-- names one exact tool identity with four ceilings. Folding them into one table would mean half the
-- columns are NULL in every row and the reader has to know which half apply.
--
-- **Why this migration exists at all.** `jarvis-infrastructure`'s `NativeReadOnlyGrants` grants any
-- native, low-risk, read-only tool to any authenticated principal of the profile. That is a
-- *constructor*, so the only way to change what a deployment allows is to change the code — and a
-- user who wants "`files.read` may touch `~/notes` without asking, but `email.send` always asks me"
-- has no way to say it. This table is where they say it. It is also the only way the read-only fast
-- path can be narrowed per tool without weakening the invariant that a grant is required.
--
-- **A grant is keyed by `(workspace_id, principal_id, capability)` and NOT by the full tool identity,
-- and the reason is the one `TLS-002` recorded for the registry.** `ToolIdentity` includes the schema
-- fingerprint, so a stored grant keyed by identity would stop applying the moment a tool was
-- recompiled with a different schema — and the grant would look *absent* rather than *replaced*,
-- which is a different error for the operator and the wrong one (`resolve_grant` distinguishes
-- `IdentityReplaced` precisely because it is actionable). Keying by capability lets the row be found
-- and the identity compared, so a replacement is reported as a replacement and `ACC-024` stays
-- enforceable. The stored `tool_identity_json` is then the identity the grant was issued against,
-- read back and compared by the evaluator rather than trusted.
--
-- **`version` is optimistic concurrency, and `status` is separate from it.** Editing a grant's
-- ceilings is an `UPDATE` of the same row at a higher version, while revoking is a `status` change
-- that keeps the row: the contract requires revocation to be auditable ("it cannot erase historical
-- audit"), so a revoked grant is remembered rather than deleted. A caller that read version 3 and
-- writes version 4 while a second caller also writes 4 is refused
-- `RepositoryError::VersionConflict` — the same fact the run and policy stores report.
--
-- **What is deliberately NOT stored: the effects and risk of the tool.** They live in the definition,
-- which is reviewed configuration, and copying them here would create a second source of truth that
-- could disagree with the executor's. The grant stores *ceilings* — what the operator is willing to
-- allow — and the evaluator checks the definition against them per call.
--
-- **`expires_at` is nullable, and NULL means "does not expire".** A standing grant for a read-only
-- tool is legitimate, which is why the domain's `Grant::expires_at` is an `Option` and why this column
-- is not `NOT NULL` with a sentinel far-future value: a sentinel would make "never expires" and
-- "expires in the year 9999" indistinguishable to a reader, and the sentinel would eventually arrive.

CREATE TABLE tool_grants (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL,
    principal_id        TEXT NOT NULL,
    -- The capability string the grant was issued for, e.g. `clock.now@1`. The key dimension, because
    -- it survives a tool recompilation the way the full identity cannot.
    capability          TEXT NOT NULL,
    -- The exact identity the grant was issued against, serialized. Read back and compared, never
    -- trusted as the key — see the header.
    tool_identity_json  TEXT NOT NULL,
    -- The scopes conferred, serialized as a JSON array of strings. An empty set is legal: a tool that
    -- declares no scope requirement needs none conferred.
    scopes_json         TEXT NOT NULL,
    -- The effects permitted, serialized as a JSON array. The evaluator refuses a request carrying an
    -- effect outside this set.
    effects_json        TEXT NOT NULL,
    -- The greatest risk permitted, as the contract string (`low`/`moderate`/`high`/`critical`).
    risk_ceiling        TEXT NOT NULL,
    -- The most sensitive argument permitted, as the contract string.
    sensitivity_ceiling TEXT NOT NULL,
    -- When the grant stops applying, or NULL for a standing grant.
    expires_at          TEXT,
    -- The lifecycle state: `active` or `revoked`. A revoked row is kept, not deleted.
    status              TEXT NOT NULL,
    -- Optimistic concurrency. A writer states the version it is replacing.
    version             INTEGER NOT NULL,
    -- Who configured it, for the audit. The principal the grant is *for* is `principal_id`; this is
    -- the operator who wrote it, and the two differ whenever an operator grants on someone's behalf.
    granted_by          TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

-- **The uniqueness the store depends on.** One grant per capability per principal per workspace, so a
-- second write for the same triple is a version conflict on an existing row rather than a second row
-- that the evaluator would have to choose between. Two rows matching one request would make the
-- decision depend on read order, which is exactly the non-determinism policy forbids.
CREATE UNIQUE INDEX idx_tool_grants_capability
    ON tool_grants (workspace_id, principal_id, capability);

-- Covers the read the evaluator performs on **every** tool call: every active grant for one principal
-- in one workspace. The partial predicate is not an optimisation — a revoked grant must not be
-- returned at all, and excluding it in the index means a revoked row cannot be read as active by a
-- query that forgot the status filter.
CREATE INDEX idx_tool_grants_active
    ON tool_grants (workspace_id, principal_id)
    WHERE status = 'active';

-- Deny rules: the one input the evaluator consults **before** any grant, so its read must be cheap and
-- whole. A rule names any subset of the four dimensions; an unnamed dimension is not a constraint,
-- which is why every one of these is nullable.
CREATE TABLE tool_deny_rules (
    id                  TEXT PRIMARY KEY,
    -- The workspace the rule was created in. NULL means "every workspace", which is a legal rule and
    -- the broadest one — so this column is nullable rather than defaulted to the creating workspace,
    -- or a global refusal would silently become a per-workspace one.
    workspace_id        TEXT,
    -- The capability it refuses, when it names one. Stored as a capability rather than an identity for
    -- the same reason a grant is: a refusal must keep refusing after a tool is recompiled.
    capability          TEXT,
    -- The principal it refuses, when it names one.
    principal_id        TEXT,
    -- The effects it refuses, serialized as a JSON array. A refusal naming an effect refuses any tool
    -- that *has* that effect, which is what makes "refuse every destructive tool" a one-row rule.
    effects_json        TEXT NOT NULL,
    -- A human-readable reason, shown to the principal whose call was refused.
    reason              TEXT NOT NULL,
    created_at          TEXT NOT NULL
);

-- The read the evaluator performs on every call, alongside the grants.
CREATE INDEX idx_tool_deny_rules_scope
    ON tool_deny_rules (workspace_id);

-- Record the new schema version. The minimum reader stays at 1: this migration only **adds** tables,
-- and a table an older binary never reads cannot break it — the same reasoning `000002`, `000008`, and
-- `000009` recorded, and the opposite of a migration that changes a shape an older reader names.
UPDATE schema_version
SET schema_version = 12,
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE schema_version < 12;
