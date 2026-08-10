-- Autonomy run history. One row per autonomy channel run; the run summary and
-- actions come from the autonomy_complete tool, and wake_event_ids records
-- which wake events the run consumed ("woken by" provenance). Scoped to the
-- agent by database file, like cron_jobs and wake_events.

CREATE TABLE IF NOT EXISTS autonomy_runs (
    id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finished_at TEXT,
    duration_secs INTEGER,
    status TEXT NOT NULL DEFAULT 'running',
    summary TEXT,
    actions TEXT NOT NULL DEFAULT '[]',
    wake_event_ids TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS idx_autonomy_runs_started ON autonomy_runs(started_at DESC);
