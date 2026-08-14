-- Durable task discussion and immutable task specification history.
--
-- Two records with distinct semantics. `task_comments` is the append-only
-- conversation around a task: what was investigated, what was decided, what a
-- worker found. `task_revisions` is the versioned record of the task's own
-- material fields, so a description that changes can be read back as it stood
-- at any point and restored without destroying the versions in between.

CREATE TABLE task_comments (
    -- Monotonic sequence: breaks ties inside a millisecond so chronological
    -- ordering and cursor pagination are stable across reads.
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    id          TEXT NOT NULL UNIQUE,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author_type TEXT NOT NULL,               -- 'user' | 'agent' | 'worker' | 'system'
    author_id   TEXT,                        -- agent id, user id, or worker id
    body        TEXT NOT NULL,
    worker_id   TEXT,                        -- worker run this comment reports on
    metadata    TEXT NOT NULL DEFAULT '{}',  -- JSON object
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX task_comments_task ON task_comments(task_id, seq);

CREATE TABLE task_revisions (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    -- Task-local revision number, starting at 1. The UNIQUE constraint is what
    -- makes numbering race-safe: a concurrent writer that computed the same
    -- next number fails its insert instead of overwriting.
    revision     INTEGER NOT NULL,
    -- JSON snapshot of every materially versioned field, written whole so a
    -- historical revision reads back without replaying the ones before it.
    snapshot     TEXT NOT NULL,
    author_type  TEXT NOT NULL,              -- 'user' | 'agent' | 'worker' | 'system'
    author_id    TEXT,
    source       TEXT NOT NULL,              -- which surface performed the mutation
    edit_summary TEXT,
    -- Set when this revision was produced by restoring an earlier one.
    restored_from INTEGER,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (task_id, revision)
);

CREATE INDEX task_revisions_task ON task_revisions(task_id, revision DESC);

-- The task's current revision number, and the token a caller supplies to prove
-- it is editing the version it last read. Zero means no history has been
-- written yet; the baseline backfill moves every existing task to 1.
ALTER TABLE tasks ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
