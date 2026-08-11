-- Chronicle range-join provenance (execution-plan 1.7).
-- A memory that supersedes an earlier one records the checkpoint whose span
-- the superseding content came from. Nullable: ordinary memories have none.
ALTER TABLE memories ADD COLUMN supersedes_checkpoint_id TEXT;

CREATE INDEX IF NOT EXISTS idx_memories_supersedes
    ON memories(supersedes_checkpoint_id);
