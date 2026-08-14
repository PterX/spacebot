-- Monotonic worker lifecycle, durable outcomes, transcript versions, and run ownership.
ALTER TABLE worker_runs ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'created';
ALTER TABLE worker_runs ADD COLUMN outcome_kind TEXT;
ALTER TABLE worker_runs ADD COLUMN outcome_summary TEXT;
ALTER TABLE worker_runs ADD COLUMN outcome_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE worker_runs ADD COLUMN transcript_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE worker_runs ADD COLUMN run_id TEXT;
ALTER TABLE worker_runs ADD COLUMN origin_branch_id TEXT;
ALTER TABLE worker_runs ADD COLUMN terminal_owner TEXT;

ALTER TABLE branch_runs ADD COLUMN run_id TEXT;
ALTER TABLE branch_runs ADD COLUMN origin_branch_id TEXT;

UPDATE worker_runs
SET lifecycle = CASE
        WHEN status = 'done' THEN 'succeeded'
        WHEN status = 'cancelled' THEN 'cancelled'
        WHEN status = 'failed' THEN 'failed'
        WHEN completed_at IS NOT NULL THEN 'failed'
        WHEN status = 'idle' THEN 'waiting_for_input'
        ELSE 'running'
    END,
    outcome_kind = CASE
        WHEN status = 'done' THEN 'succeeded'
        WHEN status = 'cancelled' THEN 'cancelled'
        WHEN status = 'failed' OR completed_at IS NOT NULL THEN 'failed'
        ELSE NULL
    END,
    outcome_summary = CASE
        WHEN status IN ('done', 'cancelled', 'failed') OR completed_at IS NOT NULL THEN result
        ELSE NULL
    END,
    outcome_version = CASE
        WHEN status IN ('done', 'cancelled', 'failed') OR completed_at IS NOT NULL THEN 1
        ELSE 0
    END,
    transcript_version = CASE WHEN transcript IS NULL THEN 0 ELSE 1 END;

-- A completion timestamp is unambiguous terminal evidence even when a delayed
-- nonterminal display write produced an impossible status.
UPDATE worker_runs
SET status = CASE lifecycle
        WHEN 'succeeded' THEN 'done'
        WHEN 'partial' THEN 'done'
        WHEN 'cancelled' THEN 'cancelled'
        WHEN 'timed_out' THEN 'failed'
        WHEN 'blocked' THEN 'failed'
        WHEN 'failed' THEN 'failed'
        WHEN 'waiting_for_input' THEN 'idle'
        ELSE 'running'
    END;

CREATE INDEX idx_worker_runs_lifecycle ON worker_runs(lifecycle, started_at);
CREATE INDEX idx_worker_runs_outcome_version ON worker_runs(outcome_version) WHERE outcome_version > 0;
CREATE INDEX idx_worker_runs_run ON worker_runs(run_id, started_at) WHERE run_id IS NOT NULL;
CREATE INDEX idx_worker_runs_origin_branch ON worker_runs(origin_branch_id, started_at)
    WHERE origin_branch_id IS NOT NULL;
CREATE INDEX idx_branch_runs_run ON branch_runs(run_id, started_at) WHERE run_id IS NOT NULL;
CREATE INDEX idx_branch_runs_origin_branch ON branch_runs(origin_branch_id, started_at)
    WHERE origin_branch_id IS NOT NULL;
