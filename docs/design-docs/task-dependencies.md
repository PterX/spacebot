# Task Dependencies and Stacked PRs

Tasks can carry an execution plan — where and how work runs — but not an
ordering. Nothing can express "not until #7 lands," so the board can't hold a
pipeline, and the two autonomous consumers of readiness (the cortex ready-task
loop and act-level autonomy runs) treat every ready task as immediately
runnable.

Meanwhile GitHub shipped native stacked pull requests (public preview
2026-07-30): ordered PR chains where each layer targets the branch below,
merging a layer server-side rebases and retargets everything above it, and the
whole surface is explicitly agent-addressable — a `gh-stack` CLI extension
with machine-readable exit codes, REST endpoints to list/create/extend/
dissolve stacks, a published agent skill (`gh skill install github/gh-stack`),
and a `stack` object on `pull_request` webhooks.

These are one feature seen from two sides. **A dependency edge is the board
primitive; a stack is what that edge compiles to when both tasks are coding
work in the same repo.** This doc designs the edge, the readiness semantics,
and the compilation.

Related: [prompt-audit-2026-08-12.md](prompt-audit-2026-08-12.md) §3 (tasks as
the spine of autonomous work), [autonomy.md](autonomy.md),
[wakes.md](wakes.md), [goals.md](goals.md).

## Current state (source-grounded)

- `tasks` has no dependency representation. `goal_id` groups without ordering;
  subtasks are a checklist inside one task.
- `TaskStore::claim_next_ready` selects by `status = 'ready'` ordered by
  priority then task number — readiness is a status, not a computation.
- Execution plans (worker type, project, repo, `worktree_mode`,
  `required_skills`) landed in `2a08e098`. `worktree_mode: create` provisions
  a `task-{N}` worktree on branch `task/{N}` via
  `projects::provision_worktree`, which threads down to
  `git::create_worktree(.., start_point)` — the `start_point` is currently
  always `None`, so every task branches from trunk HEAD. That parameter is
  the entire git-side mechanic of a stack, already plumbed and waiting.

## Two dependency kinds

The design has to hold a distinction or it collapses into the wrong feature:

| | `gate` | `stack` |
|---|---|---|
| Meaning | B needs A's *outcome* | B builds on A's *code* |
| B may start | when A is done | as soon as A's branch exists |
| B may finish | on its own terms | B's PR can't merge before A's (GitHub enforces bottom-up) |
| Applies to | any two tasks | same project + repo, both `worktree_mode: create` |

A stack is not a blocking dependency — it's a *base* dependency, and its
whole purpose is concurrency: A and B run in parallel in their own worktrees
while the merge order stays enforced at the PR layer. Collapsing stacks into
gates would serialize exactly the work stacking exists to parallelize.

The kind is stored explicitly, not derived. Two same-repo coding tasks can
still legitimately need a `gate` (B needs to know what A *found*, not just
its diff), so intent has to be declared — consistent with the execution-plan
philosophy that approval means knowing how work will run.

## Schema

```sql
CREATE TABLE task_dependencies (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind TEXT NOT NULL DEFAULT 'gate',   -- 'gate' | 'stack'
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (task_id, depends_on_task_id)
);
```

Edges form a DAG. Cycle rejection happens at insert (walk the transitive
closure of `depends_on_task_id`; SQLite recursive CTE, bounded by board
size). Self-edges rejected trivially. Deleting a task cascades its edges —
dependents become unblocked rather than orphaned, and the store logs which
tasks lost a dependency so the event is visible in working memory.

`stack` edges carry an extra invariant, validated at creation and re-checked
at spawn: both tasks resolve to the same `project_id` and `repo_id` with
`worktree_mode: create`. A stack edge whose plans have drifted apart fails
the spawn with an actionable error rather than silently degrading to a gate.

A task may have multiple `gate` edges but at most one `stack` edge —
stacks are linear chains (matching GitHub's model), and the one-parent rule
is what makes "which branch do I base on" unambiguous.

## Readiness

Readiness becomes a computation over status plus edges. `claim_next_ready`
gains one clause:

```sql
AND NOT EXISTS (
    SELECT 1 FROM task_dependencies d
    JOIN tasks dep ON dep.id = d.depends_on_task_id
    WHERE d.task_id = tasks.id
      AND CASE d.kind
            WHEN 'gate'  THEN dep.status != 'done'
            -- A stack parent unblocks once its branch exists (worktree
            -- provisioned) or it has finished entirely.
            WHEN 'stack' THEN dep.worktree_id IS NULL AND dep.status != 'done'
          END
)
```

The same predicate backs a `blocked` computed field on the task API response
(with the blocking task numbers), so the UI and the autonomy briefing render
"blocked by #2" without recomputing edges client-side.

Approval is unchanged: a blocked task can be approved to `ready` — approval
is authorization, blocking is scheduling. The two autonomous consumers pick
it up only when the edges clear.

## Stack compilation

When a `stack`-child task spawns (`spawn_worker` with `task_number`):

1. **Base resolution.** The parent's branch is `task/{parent_number}`. The
   child's worktree provisions with `start_point = task/{parent_number}`
   instead of trunk — one new parameter through `provision_worktree` into
   `git::create_worktree`, which already accepts it.
2. **Worker briefing.** The task prompt states the base: the worker is on
   `task/{child}` branched from `task/{parent}`, its PR must target
   `task/{parent}`'s branch, and the diff it owns is only its own layer.
3. **Stack registration.** After the child's PR exists, `gh stack link`
   assembles the chain on GitHub from the existing branches/PRs — built for
   exactly this case (branches managed by external tooling). Registration is
   the worker's final step, taught by skill rather than hard-coded, because
   PR creation itself is worker-side.
4. **Required skill.** Stacked tasks get `github-stacked-prs` appended to
   their effective `required_skills` — a thin skill wrapping GitHub's own
   agent skill: `gh stack link / sync / rebase`, the exit-code table
   (3 = rebase conflict, 8 = stack locked), `--auto` for non-interactive
   submit, and the rule that merge order is bottom-up.

Concurrency needs no new machinery: parent and child run in separate
worktrees, and the OpenCode server pool's per-directory claim already keeps
workers from colliding.

### Rebase flow

When the parent branch moves after the child branched, GitHub cascades the
PR-side rebase server-side. The child's *worktree* is what goes stale: the
recovery is `gh stack rebase` in the child worktree, and exit code 3
(conflict) is a genuinely human-worthy event — surfaced as a task state
change rather than silently retried.

### Merge flow and the webhook loop

`pull_request` webhook payloads now include a `stack` object. The wakes
system subscribes; on a merge event the wake handler:

- marks the corresponding task `done` when its PR merges,
- which (for `gate` dependents) flips readiness, and the existing ready-task
  loop picks up the next task in the pipeline with no polling.

This closes the loop the audit doc asked for: the board stops being a
snapshot Jamie maintains and becomes state that external reality updates.

## Surfaces

- **Task tools**: `task_create`/`task_update` gain `depends_on`
  (`[{task: N, kind: "gate" | "stack"}]`, kind defaulting to `gate`).
  Validation: existence, cycle check, stack invariants.
- **API**: dependency edges on the task response plus the computed `blocked`
  / `blocked_by` fields; add/remove endpoints.
- **UI**: the Execution Plan section renders "based on #2" for a stack edge
  and "blocked by #3, #4" for unmet gates; the task list shows a blocked
  state on approved-but-blocked tasks so they don't read as stalled.
- **Autonomy briefing**: `render_task_line` appends `[blocked by #2]` /
  `[stacks on #2]` so runs don't re-derive ordering from prose.

## Failure modes

- **Parent reopened after child branched** — the edge stays satisfied
  (branch exists); the PR layer keeps merge order honest. No board-side
  un-blocking gymnastics.
- **Dependency deleted** — cascade removes the edge, dependents unblock,
  event logged. A stacked child whose parent branch vanished fails its next
  spawn with the git error, which is the honest signal.
- **Plans drift under a stack edge** — spawn-time re-validation catches it.
- **Preview-API drift** — the stack registration and webhook layers touch
  preview surface (`gh stack link`, webhook `stack` object) and are isolated
  in phase 3–4; edges and readiness (phases 1–2) depend on nothing external
  and are valuable standalone.

## Phases

1. **Edges and readiness.** Migration, store (insert with cycle check,
   edge queries, `blocked_by` computation), `claim_next_ready` clause, task
   tool + API surface, autonomy briefing render. The board can hold a
   pipeline.
2. **UI.** Blocked states in list and detail, dependency rendering in the
   Execution Plan section.
3. **Stack compilation.** `start_point` through `provision_worktree`,
   spawn-time base resolution and validation, worker briefing, the
   `github-stacked-prs` skill, `gh stack link` as the worker's closing step.
4. **Webhook loop.** `pull_request` stack events into wakes; merge →
   task `done` → dependents unblock.
5. **Conveniences.** Rebase-conflict surfacing as task events, land-the-chain
   from the task UI via the async merge API, stack visualization.
