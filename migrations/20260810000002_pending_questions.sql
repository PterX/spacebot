-- Pending questions store for the ask tool.
--
-- When an agent calls the ask tool, the question + options are persisted here
-- so inbound interaction clicks can be correlated back to the original question.

CREATE TABLE IF NOT EXISTS pending_questions (
    question_id  TEXT PRIMARY KEY,
    agent_id     TEXT NOT NULL,
    channel_id   TEXT NOT NULL,
    question     TEXT NOT NULL,
    options      TEXT NOT NULL,   -- JSON array of AskOption
    multi_select INTEGER NOT NULL DEFAULT 0,
    message_ref  TEXT,            -- platform message id, for disabling buttons after answer
    created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    resolved_at  TIMESTAMP,
    answer       TEXT             -- JSON array of picked labels
);

-- Fast lookup by channel for pruning
CREATE INDEX IF NOT EXISTS idx_pending_questions_channel
    ON pending_questions(channel_id, created_at DESC);

-- Fast lookup for resolution via inbound interaction click
CREATE INDEX IF NOT EXISTS idx_pending_questions_resolved
    ON pending_questions(resolved_at);
