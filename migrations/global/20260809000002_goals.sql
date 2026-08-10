-- User-defined goals: high-level, persistent objectives that orient agent
-- work. Tasks link to goals via goal_id; goals are completed by the user,
-- not auto-closed when linked tasks finish.

CREATE TABLE goals (
    id           TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
    title        TEXT NOT NULL,
    description  TEXT,
    status       TEXT NOT NULL DEFAULT 'active',  -- active, paused, completed, abandoned
    priority     TEXT NOT NULL DEFAULT 'medium',  -- critical, high, medium, low
    due_date     TEXT,                             -- ISO 8601 date, nullable
    notes        TEXT,                             -- agent-writable progress notes
    metadata     TEXT DEFAULT '{}',               -- JSON, extensible
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at TEXT
);

CREATE INDEX goals_status ON goals(status);
CREATE INDEX goals_priority ON goals(status, priority);

ALTER TABLE tasks ADD COLUMN goal_id TEXT REFERENCES goals(id);
CREATE INDEX tasks_goal ON tasks(goal_id);
