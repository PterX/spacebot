-- Durable branch observability metadata and transcripts.
ALTER TABLE branch_runs ADD COLUMN input TEXT NOT NULL DEFAULT '';
ALTER TABLE branch_runs ADD COLUMN status TEXT NOT NULL DEFAULT 'running';
ALTER TABLE branch_runs ADD COLUMN transcript BLOB;
ALTER TABLE branch_runs ADD COLUMN tool_calls INTEGER NOT NULL DEFAULT 0;
ALTER TABLE branch_runs ADD COLUMN profile TEXT NOT NULL DEFAULT 'default';
ALTER TABLE branch_runs ADD COLUMN model TEXT;
ALTER TABLE branch_runs ADD COLUMN max_turns INTEGER;

UPDATE branch_runs
SET input = description,
    status = CASE
        WHEN completed_at IS NULL THEN 'running'
        ELSE 'done'
    END;

CREATE INDEX idx_branch_runs_status ON branch_runs(status, started_at);
