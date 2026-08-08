-- Per-skill provenance and usage tracking.
--
-- Skills on disk with no row get one seeded on first sight with
-- created_at = now, so a newly noticed skill's staleness clock starts at
-- discovery rather than at epoch. Only skills with created_by = 'agent'
-- are ever auto-curated; 'user' and 'installed' skills are outside curator
-- jurisdiction unless explicitly adopted.
CREATE TABLE skill_usage (
    skill_name      TEXT PRIMARY KEY,   -- lowercased canonical name
    created_by      TEXT NOT NULL,      -- 'user' | 'agent' | 'installed'
    origin_conversation_id TEXT,        -- set when created_by = 'agent'
    state           TEXT NOT NULL DEFAULT 'active', -- 'active' | 'stale' | 'archived'
    pinned          INTEGER NOT NULL DEFAULT 0,
    read_count      INTEGER NOT NULL DEFAULT 0,
    patch_count     INTEGER NOT NULL DEFAULT 0,
    last_read_at    TEXT,
    last_patched_at TEXT,
    created_at      TEXT NOT NULL,
    archived_at     TEXT
);
