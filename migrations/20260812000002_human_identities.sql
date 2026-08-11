-- Human anchors (execution-plan 3.1a). One anchor memory per human; the
-- mapping resolves a participant key to its curated anchor in-turn, and
-- reflection merges new observations into the same anchor instead of
-- creating duplicates.
CREATE TABLE IF NOT EXISTS human_identities (
    human_id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL UNIQUE REFERENCES memories(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_human_identities_memory
    ON human_identities(memory_id);
