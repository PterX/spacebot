# Task Comments

A task's description is its spec; there is nowhere to put the conversation
*around* the spec. Refinement happens in chat and evaporates, or lands as
description rewrites that erase how the plan got there. A task that takes
weeks of back-and-forth before it's ready needs a durable thread, and the
agent needs to participate in that thread on its own schedule — seeing new
comments in its autonomy runs, and being summonable *now* when tagged.

The interaction this is designed around: Jamie stacks several comments on a
task over an evening — thoughts, corrections, links — none of which demand a
response. The last one tags the agent. The agent wakes, reads the whole
unseen span as one briefing, researches, edits the task (description, plan,
subtasks, dependencies), and replies in the thread. Comments without a tag
cost nothing; the tag is the doorbell.

Related: [prompt-audit-2026-08-12.md](prompt-audit-2026-08-12.md) §3,
[task-dependencies.md](task-dependencies.md), [autonomy.md](autonomy.md),
[wakes.md](wakes.md), [worker-briefing.md](worker-briefing.md).

## What already exists (source-grounded)

- **`SystemEvent::TaskCommented` is already a wake event variant**
  (`src/wakes/events.rs`) — the wakes system anticipated this feature; no
  producer emits it yet.
- **The wake doorbell makes "now" real.** `emit_system_event` enqueues
  durable rows per subscribed wake def and rings `fire_wake`, which nudges
  the cortex ahead of its tick. A tagged comment reaches a running agent in
  seconds, not at the next interval.
- **`worker_runs` has no task linkage.** Its `task` column is the
  description text; `tasks.worker_id` binds only the *current* worker. A
  task's execution history is unrecoverable today.
- Tasks already carry execution plans and dependency edges; comments are the
  third leg — the deliberation that produces those two.

## Schema

```sql
CREATE TABLE task_comments (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author_kind TEXT NOT NULL,          -- 'human' | 'agent' | 'worker' | 'system'
    author_id TEXT NOT NULL,
    body TEXT NOT NULL,
    mentions TEXT NOT NULL DEFAULT '[]', -- resolved agent ids tagged in this comment
    worker_run_id TEXT,                  -- set when the comment reports a run
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Per-agent read cursor: everything after this is "new" to that agent.
CREATE TABLE task_comment_cursors (
    agent_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    seen_through TEXT NOT NULL,          -- created_at watermark
    PRIMARY KEY (agent_id, task_id)
);

ALTER TABLE worker_runs ADD COLUMN task_number INTEGER;
```

Comments are **append-only**. No edits, no deletes except the cascade — the
thread is a record of how the plan evolved, and records that can be rewritten
aren't records. Descriptions stay mutable (that's the living spec); the
thread is why it changed.

Chronological pagination, stable order `(created_at, id)`.

### System comments make the thread the timeline

Lifecycle events land as terse `author_kind = 'system'` comments: status
transitions ("approved by jamie", "ready → in_progress"), worker runs
("worker started: <first line of task>" with `worker_run_id` set, "worker
completed/failed"), dependency changes. One line each.

This is what makes "weeks of back and forth" legible later: the thread
interleaves human thoughts, agent replies, and execution attempts in one
sequence, without a separate timeline-merging API. `worker_runs.task_number`
additionally supports a direct runs-for-task query (the UI's runs view and
`worker_inspect` from a thread), but the narrative lives in the thread.

## Mentions and waking

A comment may tag agents — `@orion` in the body, resolved against the agent
registry at write time into the `mentions` column (unresolvable tags stay
plain text; the API also accepts an explicit `mentions` array for
programmatic callers).

**Untagged comments are silent.** They write the row, emit the SSE event for
live UI, and advance nothing else. This is the point: stacking thoughts must
be free.

**A tagged comment emits `SystemEvent::TaskCommented`** with payload
`{task_number, comment_id}` through the standard producer
(`emit_system_event`), which rings the doorbell. A built-in wake definition
subscribed to `task.commented` is seeded per agent (`insert_if_absent`, like
other defaults), gated at `min_level: observe` — being summoned to read and
reply is safe at every autonomy level; what the run may *do* about it stays
governed by the level as usual.

The wake carries instructions to the run: read the task's unseen comments,
do what they ask (research, plan edits), and reply in the thread.

## The autonomy loop sees threads

Two surfacing paths, both bounded:

- **Woken by a tag**: the briefing's Woken By section names the task; the
  task state render includes the full unseen span for that task (char-capped,
  oldest first, "N earlier comments not shown" when clipped).
- **Interval runs**: every rendered task line gains an unseen-comment marker
  — `[3 new comments]` — and tasks with unseen comments render their unseen
  span below the task line, budgeted the same way run history is. A quiet
  board costs nothing; a chatty one surfaces exactly the deltas.

The cursor advances to the rendered watermark **when the run completes**
(`autonomy_complete` or fallback) — a run that died mid-flight sees the same
comments again next time. Channel-side reads (a branch answering "what's new
on #7") advance the cursor the same way on branch completion.

The reply loop closes with an `add_task_comment` tool:

- **Branch and autonomy toolsets**: full form — body plus optional mentions.
- **Worker toolset**: body only, `author_kind = 'worker'`, restricted to the
  task the worker is bound to (same scoping as `task_update`'s worker mode).
  Workers report findings into the thread they were spawned from.

An agent reply is board state, not a message to Jamie — no hard-rule
conflict with "do not message users." A low-severity notification (existing
notifications surface) makes replies visible without being a ping.

## The refinement loop, end to end

1. Jamie comments three times on #13 over an evening. Rows written, UI
   updates, nothing wakes.
2. Fourth comment: "…so let's scope it to notes/ first. @orion work this up."
   Mention resolves → `task.commented` wake → doorbell.
3. The run's briefing carries the task and all four comments. The agent
   researches (workers per the task's execution plan — their runs land in
   the thread as system comments), rewrites the description, adjusts
   `depends_on`, and replies with what it changed and what it recommends.
4. Cursor advances on run completion. Jamie reads the reply in the thread
   whenever he looks — the notification is there, but nothing interrupted
   him.
5. Repeat for weeks if needed. Approval happens when the thread converges;
   the executing worker inherits the distilled description *plus* the
   bounded tail of the thread in its briefing, so decisions made in
   comment #12 don't get relitigated by the worker.

## Surfaces

- **API**: `GET/POST /tasks/{number}/comments` (paginated), comment rows on
  the SSE stream (`TaskCommentAdded`), unseen counts on the task response
  (per the requesting agent's cursor is meaningless for the UI — the UI
  shows the thread itself; unseen tracking is agent-side only).
- **Tools**: `add_task_comment` (branch/autonomy full, worker restricted).
- **UI**: thread on the task detail below the Execution Plan section —
  composer with `@` autocomplete, author-typed rendering (human / agent /
  worker / system distinguished), system comments visually quiet, worker-run
  comments linking to the run inspector.
- **Worker briefing**: the executing worker's context includes the thread
  tail under a char budget, newest-last, so refinement history transfers
  without dumping weeks of discussion.

## Failure modes

- **Mention of an unknown agent** — stays plain text, no wake; the UI
  autocomplete makes this rare. No silent partial-wake.
- **Wake fires while a run is active** — the event is durable; the
  single-flight run guard already serializes runs, and the pending event
  makes the next run due immediately.
- **Comment storm** (worker system-comments on a retry loop) — system
  comments for worker lifecycle are per-run, not per-attempt, and the
  briefing render is char-budgeted; the thread absorbs noise without the
  prompt doing so.
- **Cursor loss** — cursors are advisory; losing one re-surfaces old
  comments, which is annoying and safe.

## Relationship to main

A parallel implementation of durable task comments is in flight against
`origin/main` (autonomy vertical slice: `task_comments` store,
`add_task_comment`, briefing integration). This design is deliberately
schema-compatible with that shape — immutable rows, author kind/id, worker
linkage, bounded briefing inclusion. What this doc adds on top: mention →
wake-now via the existing `TaskCommented` event, per-agent read cursors, the
system-comment timeline, and `worker_runs.task_number`. Whichever lands
first, converge on one schema at merge rather than carrying two.

## Phases

1. **Store.** `task_comments` + cursors + `worker_runs.task_number`, system
   comments from status transitions and worker lifecycle, comment writes
   from the existing spawn/pickup paths.
2. **Tools and API.** `add_task_comment` across the three toolsets, REST +
   SSE, mention resolution.
3. **Wake integration.** Producer emit on tagged comments, seeded
   `task.commented` wake def, briefing render of unseen spans (tagged and
   interval paths), cursor advance on run completion.
4. **UI.** Thread + composer + run links on task detail.
5. **Worker briefing.** Bounded thread tail in the executing worker's
   context.
