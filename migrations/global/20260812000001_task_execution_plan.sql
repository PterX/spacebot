-- Execution plan fields on tasks: how and where an approved task runs.
-- All nullable — a task without a plan falls back to its project's defaults,
-- and a task with neither leaves the decision to the executing turn.
ALTER TABLE tasks ADD COLUMN worker_type TEXT;
ALTER TABLE tasks ADD COLUMN project_id TEXT;
ALTER TABLE tasks ADD COLUMN repo_id TEXT;
ALTER TABLE tasks ADD COLUMN worktree_mode TEXT;
ALTER TABLE tasks ADD COLUMN worktree_id TEXT;
ALTER TABLE tasks ADD COLUMN required_skills TEXT NOT NULL DEFAULT '[]';
