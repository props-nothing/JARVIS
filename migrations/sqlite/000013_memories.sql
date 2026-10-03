-- Durable memory: explicit, user-requested claims, scoped to a workspace.
--
-- This is the first slice of Milestone 4 and it stores exactly what the architecture says the initial
-- implementation stores: "only explicit user requests such as 'remember that...' and confirmed
-- preferences". There is no `confidence`, `valid_until`, embedding, or supersession column, because
-- nothing writes or reads them yet and a column nothing references is a claim the schema cannot keep.
--
-- **`workspace_id` is on every query, not a post-filter.** The repository takes the workspace as a
-- parameter on every read, so a statement cannot be written without it. A foreign workspace's memory
-- reads as absent, never as forbidden.
--
-- **`dedupe_key` makes an exact duplicate a no-op rather than a second row.** It is the lowercased,
-- whitespace-collapsed text; the unique index is per workspace, so the same claim in two workspaces is
-- two memories (isolation is the point) and the same claim twice in one is one.
--
-- **Forgetting is a hard delete.** The acceptance scenario for deletion requires that a forgotten
-- preference "no longer appears in lexical/semantic/entity retrieval, context, cache, export, derived
-- relation". A tombstone row that kept the text would defeat that, so the row is removed.
--
-- `memory_class` is constrained to the two classes this build writes; adding a class is a migration
-- that widens the CHECK, which is the point at which its lifecycle rules must exist.

CREATE TABLE memories (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL,
    memory_class        TEXT NOT NULL CHECK (memory_class IN ('preference', 'semantic')),
    canonical_text      TEXT NOT NULL,
    dedupe_key          TEXT NOT NULL,
    sensitivity         TEXT NOT NULL,
    source_kind         TEXT NOT NULL CHECK (source_kind IN ('user_request')),
    source_principal_id TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE UNIQUE INDEX memories_workspace_dedupe ON memories (workspace_id, dedupe_key);
CREATE INDEX memories_workspace_recent ON memories (workspace_id, created_at DESC, id);

-- Purely additive: a new table an older binary never reads cannot break it, so the minimum reader stays at 1.
UPDATE schema_version
SET schema_version = 13,
    updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE schema_version < 13;
