-- Session chronicles: append-only interval checkpoints over a channel's
-- conversation log. Each checkpoint summarizes exactly the span since the
-- previous one; coverage is contiguous and non-overlapping.
CREATE TABLE IF NOT EXISTS channel_chronicle_checkpoints (
    id                     TEXT PRIMARY KEY,
    channel_id             TEXT NOT NULL,
    -- Monotonic per channel, allocated inside the commit transaction.
    seq                    INTEGER NOT NULL,
    -- 0 for interval checkpoints, 1+ for rollups over lower levels.
    level                  INTEGER NOT NULL DEFAULT 0,
    -- 'interval' | 'bootstrap' | 'pressure' | 'emergency' | 'rollup'
    kind                   TEXT NOT NULL,
    title                  TEXT NOT NULL,
    summary                TEXT NOT NULL,
    -- Coverage over conversation_messages ordered by (created_at, id).
    covers_from_at         TEXT NOT NULL,
    covers_to_at           TEXT NOT NULL,
    covers_from_message_id TEXT,
    covers_to_message_id   TEXT,
    message_count          INTEGER NOT NULL DEFAULT 0,
    token_estimate         INTEGER NOT NULL DEFAULT 0,
    -- Set on covered rows when a rollup absorbs them. The rows themselves stay.
    rolled_up_into         TEXT REFERENCES channel_chronicle_checkpoints(id),
    -- Set on rollup rows: the inclusive checkpoint sequence range covered.
    rolls_up_from_seq      INTEGER,
    rolls_up_to_seq        INTEGER,
    model                  TEXT,
    created_at             TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_chronicle_seq
    ON channel_chronicle_checkpoints(channel_id, seq);

-- Overlap guard: two concurrent cuts cannot commit the same end boundary.
CREATE UNIQUE INDEX IF NOT EXISTS idx_chronicle_boundary
    ON channel_chronicle_checkpoints(channel_id, level, covers_to_message_id)
    WHERE covers_to_message_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_chronicle_window
    ON channel_chronicle_checkpoints(channel_id, level, covers_to_at);

CREATE INDEX IF NOT EXISTS idx_chronicle_rollup
    ON channel_chronicle_checkpoints(rolled_up_into);
