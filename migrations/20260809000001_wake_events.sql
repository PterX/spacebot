-- Persisted wake-event queue. Wake producers (schedules, webhooks, internal
-- events, conditions) insert rows here before ringing the wake doorbell; the
-- autonomy channel consumes pending rows into its run context. Scoped to the
-- agent by database file, like cron_jobs.

CREATE TABLE IF NOT EXISTS wake_events (
    id TEXT PRIMARY KEY,
    wake_id TEXT NOT NULL,
    dedupe_key TEXT NOT NULL DEFAULT '',
    payload TEXT NOT NULL DEFAULT '{}',
    fired_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    delivery_count INTEGER NOT NULL DEFAULT 1,
    consumed_by TEXT
);

CREATE INDEX IF NOT EXISTS idx_wake_events_pending ON wake_events(consumed_by, fired_at);

-- Coalescing: at most one pending event per (wake, dedupe key). An arrival
-- that hits this index bumps delivery_count instead of inserting a new row.
CREATE UNIQUE INDEX IF NOT EXISTS idx_wake_events_coalesce
    ON wake_events(wake_id, dedupe_key) WHERE consumed_by IS NULL;
