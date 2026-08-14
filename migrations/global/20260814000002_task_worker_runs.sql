-- Every worker run attempted against a task, kept whole.
--
-- `tasks.worker_id` points at the run currently executing and is overwritten by
-- the next spawn, so a task retried three times remembers only the last one.
-- This table is the history: append-only, one row per attempt.
--
-- `worker_id` carries no foreign key on purpose. Tasks live in the instance
-- database and `worker_runs` lives in the per-agent database, so the reference
-- crosses a database boundary and cannot be enforced by SQLite. A run whose
-- worker row has been pruned still records that the attempt happened and how it
-- ended.
CREATE TABLE task_worker_runs (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    worker_id       TEXT NOT NULL,
    -- 1 for the first attempt on this task, incrementing per attempt.
    attempt         INTEGER NOT NULL,
    -- Who or what asked for this run, and through which surface.
    author_type     TEXT NOT NULL DEFAULT 'system',
    author_id       TEXT,
    agent_id        TEXT,
    channel_id      TEXT,
    started_at      TIMESTAMP NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    -- Null until the run reaches a terminal state.
    outcome_kind    TEXT,
    outcome_summary TEXT,
    ended_at        TIMESTAMP,
    UNIQUE (task_id, worker_id),
    UNIQUE (task_id, attempt)
);

CREATE INDEX task_worker_runs_task ON task_worker_runs(task_id, attempt DESC);
CREATE INDEX task_worker_runs_worker ON task_worker_runs(worker_id);

-- Resolving "is this task already being worked on" must not scan the table.
CREATE INDEX task_worker_runs_live ON task_worker_runs(task_id, ended_at);
