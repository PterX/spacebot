-- Wake definitions: named triggers paired with instructions for the autonomy
-- run that consumes their events. Built-in rows are seeded in code, config
-- rows are reconciled from [[agents.X.wakes]] on load, and user rows come
-- from the API. Scoped to the agent by database file, like cron_jobs and
-- wake_events.

CREATE TABLE IF NOT EXISTS wake_defs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    trigger_kind TEXT NOT NULL,
    trigger_spec TEXT NOT NULL DEFAULT '{}',
    instructions TEXT NOT NULL,
    min_level TEXT NOT NULL DEFAULT 'observe',
    enabled INTEGER NOT NULL DEFAULT 1,
    builtin INTEGER NOT NULL DEFAULT 0,
    config_owned INTEGER NOT NULL DEFAULT 0,
    delivery_target TEXT,
    webhook_token TEXT,
    active_hours_start INTEGER,
    active_hours_end INTEGER,
    next_run_at TEXT,
    last_fired_at TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL DEFAULT 'system',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_wake_defs_enabled_kind ON wake_defs(enabled, trigger_kind);
