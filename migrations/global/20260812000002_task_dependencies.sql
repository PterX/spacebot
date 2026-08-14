-- Dependency edges between tasks. `gate` blocks the dependent until the
-- dependency is done; `stack` blocks only until the dependency's branch
-- exists (worktree provisioned) — see docs/design-docs/task-dependencies.md.
CREATE TABLE task_dependencies (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind TEXT NOT NULL DEFAULT 'gate',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (task_id, depends_on_task_id)
);

CREATE INDEX idx_task_dependencies_depends_on ON task_dependencies(depends_on_task_id);
