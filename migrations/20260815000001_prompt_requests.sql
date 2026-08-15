-- Index of captured LLM requests. The payload (system prompt, block map,
-- tool definitions, messages, response) lives on disk at `path`; this table
-- exists so a session's requests can be listed, filtered and joined to the
-- channel, message or process they belong to.

CREATE TABLE IF NOT EXISTS prompt_requests (
    request_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    -- channel, branch, worker, compactor, cortex, chronicle, ingestion.
    process_kind TEXT NOT NULL,
    -- Branch/worker uuid, or the channel id for a channel turn.
    process_id TEXT,
    -- Narrower label where one kind has variants (builtin, opencode, ...).
    process_type TEXT,
    channel_id TEXT,
    -- Conversation message that triggered the turn, when there was one.
    message_id TEXT,
    trigger TEXT,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    started_at TIMESTAMP NOT NULL,
    duration_ms INTEGER,
    system_chars INTEGER NOT NULL DEFAULT 0,
    history_length INTEGER NOT NULL DEFAULT 0,
    tool_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cached_tokens INTEGER,
    -- ok | error
    status TEXT NOT NULL DEFAULT 'ok',
    -- Path to the JSON payload, relative to the agent data directory.
    path TEXT NOT NULL
);

CREATE INDEX idx_prompt_requests_started ON prompt_requests(started_at DESC);
CREATE INDEX idx_prompt_requests_channel ON prompt_requests(channel_id, started_at DESC);
CREATE INDEX idx_prompt_requests_process ON prompt_requests(process_id, started_at DESC);
CREATE INDEX idx_prompt_requests_message ON prompt_requests(message_id);
