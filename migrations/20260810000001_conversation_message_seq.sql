-- A durable, monotonic, per-channel insertion order for conversation messages.
--
-- `created_at` defaults to CURRENT_TIMESTAMP, which SQLite resolves to whole
-- seconds, and message ids are random UUIDs. Ordering by `(created_at, id)` is
-- deterministic but NOT insertion order: `ConversationLogger` writes are
-- detached tasks, so a row inserted after a chronicle boundary was committed
-- could carry the same whole-second timestamp and a lexically smaller id, sort
-- behind that boundary, and never be selected by a later cut again.
--
-- `seq` is assigned at INSERT time from the channel's current maximum, so a row
-- written later always sorts later. Chronicle boundaries are seq values, which
-- makes "everything after the boundary" exact regardless of write timing.
ALTER TABLE conversation_messages ADD COLUMN seq INTEGER;

-- Backfill existing rows in their established order so historical boundaries
-- and expansions stay stable across the upgrade.
UPDATE conversation_messages
SET seq = (
    SELECT numbered.row_number
    FROM (
        SELECT id AS row_id,
               ROW_NUMBER() OVER (
                   PARTITION BY channel_id ORDER BY created_at, id
               ) AS row_number
        FROM conversation_messages
    ) AS numbered
    WHERE numbered.row_id = conversation_messages.id
)
WHERE seq IS NULL;

-- Two rows in one channel can never share a sequence. SQLite serializes
-- writers, so the read-max-and-increment inside a single INSERT is atomic;
-- this index makes a violation loud rather than silent.
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_channel_seq
    ON conversation_messages(channel_id, seq);

-- Chronicle coverage moves onto the same key. The message-id columns stay for
-- provenance; `covers_*_seq` is what selection and range queries use.
-- A start of 0 means "from the beginning of the channel".
ALTER TABLE channel_chronicle_checkpoints ADD COLUMN covers_from_seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channel_chronicle_checkpoints ADD COLUMN covers_to_seq INTEGER NOT NULL DEFAULT 0;

UPDATE channel_chronicle_checkpoints
SET covers_to_seq = COALESCE(
        (SELECT m.seq FROM conversation_messages m
         WHERE m.id = channel_chronicle_checkpoints.covers_to_message_id),
        0
    ),
    covers_from_seq = COALESCE(
        (SELECT m.seq FROM conversation_messages m
         WHERE m.id = channel_chronicle_checkpoints.covers_from_message_id),
        0
    );

-- Replaces the message-id boundary guard: two cuts in one channel can never
-- commit the same end position.
CREATE UNIQUE INDEX IF NOT EXISTS idx_chronicle_boundary_seq
    ON channel_chronicle_checkpoints(channel_id, level, covers_to_seq);
