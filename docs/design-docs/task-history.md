# Task Discussion and Revision History

A task's description is its specification, and until now it was the only place
anything about a task could live. Refinement happened in chat and evaporated;
an agent rewriting a description destroyed the version it replaced. Two records
fix both halves, with deliberately different semantics.

**`task_comments`** is the append-only conversation *around* the spec — what was
investigated, what was decided, what a worker found. Comments are never edited
or deleted; a record that can be rewritten is not a record.

**`task_revisions`** is the immutable version history *of* the spec. Every
material change appends a whole snapshot in the same transaction as the change
itself, so any past version reads back directly and restoring one appends a new
version rather than rewinding.

Related: [task-comments.md](task-comments.md) (the discussion design this
implements), [wiki.md](wiki.md) (the version/history/restore model this
mirrors), [task-dependencies.md](task-dependencies.md),
[execution-plan.md](execution-plan.md).

## Schema

```sql
CREATE TABLE task_comments (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    id          TEXT NOT NULL UNIQUE,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author_type TEXT NOT NULL,          -- user | agent | worker | system
    author_id   TEXT,
    body        TEXT NOT NULL,
    worker_id   TEXT,
    metadata    TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL
);

CREATE TABLE task_revisions (
    id            TEXT PRIMARY KEY,
    task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    revision      INTEGER NOT NULL,
    snapshot      TEXT NOT NULL,         -- JSON TaskRevisionSnapshot
    author_type   TEXT NOT NULL,
    author_id     TEXT,
    source        TEXT NOT NULL,
    edit_summary  TEXT,
    restored_from INTEGER,
    created_at    TEXT NOT NULL,
    UNIQUE (task_id, revision)
);

ALTER TABLE tasks ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
```

`seq` on comments breaks ties inside a millisecond, so chronological ordering
and cursor pagination stay stable. `UNIQUE (task_id, revision)` is what makes
numbering race-safe: a concurrent writer that computed the same next number
fails its insert rather than overwriting.

`tasks.revision` serves two jobs — the current version number, and the
optimistic-concurrency token a caller sends back as `expected_revision`.

## The versioned snapshot contract

`TaskRevisionSnapshot` captures every field whose change needs historical
reconstruction:

`title`, `description`, `status`, `priority`, `assigned_agent_id`, `subtasks`,
`metadata`, `goal_id`, `worker_type`, `project_id`, `repo_id`,
`worktree_mode`, `worktree_id`, `required_skills`, `depends_on`.

Excluded by explicit decision, because they describe a *run* rather than the
spec:

- `worker_id` — which worker is currently bound. Binding and unbinding a worker
  happens many times per task and says nothing about what the task asks for.
- `approved_at` / `approved_by` / `completed_at` — derived from status
  transitions, which are versioned. The author of the status revision already
  records who did it.
- `updated_at` — a clock, not a decision.
- `id`, `task_number`, `owner_agent_id`, `created_by`, `source_memory_id` —
  identity, fixed at creation.

Dependency edges are snapshotted as task *numbers*, not internal ids, and
sorted, so two snapshots of the same edge set compare equal.

## Revision semantics

- Creating a task commits **revision 1** with the initial snapshot in the same
  transaction as the task row. A task cannot exist without the version it
  started from.
- Every material edit appends **exactly one** revision, whatever the number of
  fields it touched.
- A no-op creates **nothing**: the store captures the snapshot before and after
  the write and compares them. Identical means no revision row and no
  `task_revised` event.
- Historical revisions are never updated or deleted by ordinary operations.
- **Restore N** performs a concurrency-checked material update and appends a
  new latest revision whose snapshot matches N, with `restored_from = N`.
  Revision N is untouched, numbering never rewinds, and no version in between
  is erased.

### Optimistic concurrency

A caller supplies `expected_revision`. When it no longer matches, the write
fails with a structured conflict carrying both the expected and the current
revision, so the caller can refresh and retry without another round trip. Over
HTTP that is a `409` with:

```json
{ "error": "...", "expected_revision": 4, "current_revision": 6 }
```

Omitting `expected_revision` means last-write-wins, which is right for a status
toggle and wrong for a description rewrite. The Portal and the tools supply it
on the paths that matter.

### Partial updates and clearing

`UpdateTaskInput` distinguishes "leave alone" from "set to null" through
`Patch<T> = Option<Option<T>>`: absent leaves the stored value, `Some(None)`
clears it. Over HTTP, omitting a key leaves it and sending `null` clears it.
Restore depends on this — a revision that had no project must be restorable
onto a task that has one.

`metadata` normally deep-merges. Restore sets `replace_metadata`, so a key
removed in the restored revision stays gone.

## One transactional mutation path

Every caller reaches storage through `TaskStore::apply_update` (or
`create_with_dependencies` for creation). The task row, its revision, and its
dependency edges commit together or not at all; a failure can never leave a
changed task without its revision, or a revision for a change that rolled back.

Migrated onto it: REST mutations, CLI mutations, Portal mutations, the
`task_update` and `task_create` tools, worker-permitted subtask/metadata
updates, status and approval transitions, assignment, and restore. Workers keep
their existing restriction — subtasks and metadata only, on the task they are
bound to — because restore and history are built *on* the update path, not
around it. For the same reason a restore cannot bypass status-transition rules,
dependency validation, or execution-plan validation.

`TaskStore::delete` removes comments, revisions, and dependency edges in the
same transaction rather than relying on a cascade that only fires with
`PRAGMA foreign_keys`.

## Author and source taxonomy

`author_type` is who: `user`, `agent`, `worker`, `system`.
`source` is which surface: `api`, `cli`, `portal`, `tool`, `worker`,
`restore`, `migration`, `system`.

Both are recorded per revision, and `author_type` per comment. The pair is what
makes history legible — "agent orion, via tool" reads differently from "user
jamie, via portal", and a `restore` source is never mistaken for an ordinary
edit that happened to reproduce an old version.

## Migration

`backfill_baseline_revisions` gives every task with `revision = 0` a baseline
revision 1 snapshotting it exactly as it stands, authored as `system` /
`migration`. It runs at startup, before the API serves.

It is idempotent twice over: the query selects only unversioned tasks, and
`UNIQUE (task_id, revision)` makes a retry after a partial run a no-op rather
than a duplicate. Verified against empty, fresh, and populated databases.

It does **not** reconstruct history that predates the feature, and the summary
it records says so. Descriptions overwritten before revisions existed are gone;
no synthetic history is inferred to cover that up.

## Surfaces

### API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/tasks/{n}/comments` | Thread, oldest first, cursor-paginated |
| `POST` | `/tasks/{n}/comments` | Append a comment |
| `GET` | `/tasks/{n}/revisions` | History summaries, newest first |
| `GET` | `/tasks/{n}/revisions/{r}` | One revision with its snapshot |
| `GET` | `/tasks/{n}/revisions/diff?from&to` | Field-aware diff; `to` defaults to current |
| `POST` | `/tasks/{n}/revisions/{r}/restore` | Restore, `expected_revision` required |

Every write endpoint accepts `author_type`, `author_id`, `source`, and
`edit_summary`; updates and restores also take `expected_revision`. Task reads
carry `revision`. Errors return a JSON body rather than a bare status.

### CLI

```
spacebot task comment <n> <body>            # append to the thread
spacebot task comments <n>                  # read the thread
spacebot task history <n>                   # revision list
spacebot task revision <n> <r>              # one revision, whole
spacebot task diff <n> <from> [to]          # what changed
spacebot task restore <n> <r> --summary "…" # restore, reads current revision first
spacebot task update <n> --summary "…" --expect <r>
```

All CLI writes record `source: cli`.

### Tools

- `add_task_comment` — branch/cortex on any task; worker restricted to the task
  it is bound to.
- `task_history` — `list`, `get`, `diff`, `restore` in one tool, because that is
  one train of thought. `restore` requires an `edit_summary` and sends the
  revision it just read as `expected_revision`.
- `task_update` — gains `edit_summary` and `expected_revision`, and returns the
  task's revision so the next edit can be written against it. A no-op says so in
  its message rather than claiming an update.

### Events

`task_commented` and `task_revised` are emitted **after commit**, and
`task_revised` only when a revision was actually written — a no-op update is
silent. Both carry the agent id so existing per-agent SSE routing applies, and
they ride the same stream and reconnect path as `task_updated`.

### Portal

The task detail pane gains a Discussion section (thread plus composer, with
system comments visually quiet and worker comments expanding their run output)
and a History section: revision list with author, source, summary and time; a
snapshot view; a field-aware diff against current; and restore behind a
confirmation that requires a reason. The restore carries the revision the user
was looking at, so a task that moved on mid-decision produces a conflict
message naming the new revision rather than a silent overwrite.

## Retention

Comments and revisions live in the instance database alongside tasks and the
wiki, and are covered by the same backup. Neither is pruned; both are deleted
only when their task is.

## Out of scope

- Recovering task history from before this feature.
- Editing or deleting comments. If it is ever added it needs explicit audit
  semantics, not a silent overwrite.
- Autonomy activity logging into comments — that is task #16, which can build on
  these APIs but is not a dependency.
