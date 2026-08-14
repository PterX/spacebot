# Cortex and Autonomy Audit

Autonomy was added after the Cortex and now overlaps with it. Both systems can
select work, construct workers, own detached execution, retry failures, and
change task state. The overlap produces divergent execution semantics and
makes worker lifecycle convergence harder than it needs to be.

Autonomy owns autonomous decisions and task execution. Cortex observes,
maintains, and reconciles durable state. This is the removal boundary for the
legacy Cortex orchestration path.

Related designs:

- [autonomy-lifecycle.md](autonomy-lifecycle.md) defines the Cortex succession
  plan, deliberation, and task readiness.
- [autonomous-action-audit.md](autonomous-action-audit.md) defines run
  attribution and child settling.
- [worker-lifecycle-convergence.md](worker-lifecycle-convergence.md) defines
  durable worker outcomes and terminal-state convergence.
- [coding-worker-backends.md](coding-worker-backends.md) defines the shared
  task execution path and backend boundary.

## Finding

`src/agent/cortex.rs` retains a ready-task pickup loop that claims tasks and
constructs builtin workers directly. The path duplicates the channel worker
setup and does not use `SpawnWorkerTool::resolve_task_plan`.

It therefore bypasses execution-plan behavior that manual task execution uses:

- selected worker type
- project defaults
- worktree policy and directory selection
- required skill validation and injection
- task-bound worker metadata

A task can run differently based on whether a user, a branch, or the legacy
Cortex pickup loop started it. Adding worker backends or more lifecycle policy
to this path would make that divergence permanent.

The same area contains detached-worker completion, retry, cancellation, and
task-transition logic. It overlaps with channel dispatch and the worker
lifecycle persistence layer. The current lifecycle convergence work must make
these producers and consumers correct, but it must not deepen Cortex-specific
orchestration.

## Ownership

| Component | Owns | Does not own |
|---|---|---|
| Autonomy run | Deliberation, task selection, autonomous task ownership, run settling, and final run summary | Worker implementation details or terminal persistence |
| Task execution service | Approval, execution-plan resolution, workspace preparation, worker admission, and task binding | Autonomous prioritization or provider-specific loops |
| Worker supervisor | Worker attempts, cancellation, timeout, durable outcomes, recovery, and notifications | Task selection or autonomy summary generation |
| Cortex | Memory bulletin generation, memory maintenance, observation, reconciliation, and diagnostics | Ready-task pickup, worker construction, retries, or task completion policy |

`run_id` and `origin_branch_id` belong to the task execution and worker
supervision path. They are stamped from the initiating autonomy run or branch,
not inferred by Cortex from a channel or timestamp.

## Current Paths

### Legacy Cortex execution

`pickup_one_ready_task` in `src/agent/cortex.rs` performs all of the following:

1. Claims a ready task.
2. Renders a builtin worker prompt.
3. Creates directories and a `Worker` directly.
4. Registers detached cancellation state.
5. Starts and persists a worker run.
6. Classifies outcomes and chooses task transitions.
7. Requeues selected failures.

This is a second task executor. Its worker prompt assembly alone duplicates
the setup in `src/agent/channel_dispatch.rs`.

### Supported execution

`SpawnWorkerTool::resolve_task_plan` resolves a task into worker type,
project, worktree, directory, and required skills before dispatch. Channel and
branch spawns use this route. It is the existing base for the shared task
execution service described in `coding-worker-backends.md`.

### Lifecycle convergence compatibility

Until the Cortex execution path is removed, it remains a lifecycle producer
and consumer. It must:

- create a durable worker row before starting the future
- use the shared terminal commit operation
- consume durable outcome versions idempotently
- preserve `run_id` when one is supplied
- avoid overwriting a completion with cancellation, timeout, or retry state

These are compatibility requirements. They are not a reason to add new
Cortex-specific state machines or retry abstractions.

## Removal Plan

## Recorded Implementation State

The lifecycle-convergence agent was stopped after its nested implementation
worker expanded the scope and then crashed. The following state records the
worklist as it stood at interruption. It is a scope record, not evidence that
unfinished items are safe to merge.

| Workstream | State at interruption |
|---|---|
| Inventory current worker, branch, autonomy, transcript, and migration state | Complete |
| Implement durable monotonic worker lifecycle and terminal transaction | Complete |
| Converge worker completion, cancellation, timeout, and idle races | In progress |
| Implement terminal branch delegation and idempotent spawn ownership | Not started |
| Implement autonomy run attribution, finish request, and child settling | Not started |
| Align live and durable transcript APIs, tools, and UI handoff | Not started |
| Implement durable outcome reconciliation and recovery | Not started |
| Add targeted transition, race, branch, autonomy, and transcript tests | Not started |
| Run targeted checks and repository delivery gates | Not started |

Resume only the lifecycle-convergence slice next: finish convergence, add its
transition/race/idempotency tests, and verify it. Branch delegation, autonomy
settling, transcript handoff, and reconciliation remain separate changes.

### Phase 1: Finish lifecycle convergence

Complete the active worker lifecycle work across channel, OpenCode, resume,
Cortex, and detached-worker paths. Use the shared persistence API for terminal
outcomes. Keep changes to `cortex.rs` limited to adapting existing producers
and consumers.

Required outcome: any worker started through the legacy path has the same
monotonic durable lifecycle as a channel worker.

### Phase 2: Extract task execution

Introduce the shared task execution entry point from
`coding-worker-backends.md`. It receives an approved task and initiating
context, resolves the execution plan once, creates the durable worker record,
and dispatches the selected backend.

Manual spawns, branch delegation, autonomy execution, wakes, and scheduled
execution call this entry point. The entry point returns worker identity and
durable ownership metadata, not a Cortex-specific control object.

Required outcome: no caller constructs a builtin worker from a task row.

### Phase 3: Move ready-task pickup

Move ready-task selection from the Cortex tick into the autonomy run. The run
deliberates, selects bounded work, and calls the shared task execution entry
point. Each worker receives the active `run_id`.

Autonomy settling owns the decision to wait, detach, or cooperatively cancel
children. Completed and late outcomes amend the run activity ledger through
their durable ownership fields.

Required outcome: a ready task never begins outside an autonomy run unless an
explicit external trigger invokes the shared executor with its own owner.

### Phase 4: Remove Cortex orchestration

Delete the following from the Cortex path after callers have moved:

- ready-task polling and claiming
- direct `Worker::new` construction
- detached worker registry ownership for task pickup
- outcome-to-task retry and requeue policy
- task-worker prompt assembly
- Cortex-only worker completion routing

Keep Cortex memory and maintenance work. Retain reconciliation only where it
operates on durable rows without choosing or starting work.

### Phase 5: Delete deprecated Cortex chat

Cortex chat and `create_cortex_chat_tool_server` are already deprecated. Do
not port new execution or worker tools into that topology. Move any remaining
operator capability to the channel settings or operator diagnostics path, then
delete the endpoints, factory, prompts, and compatibility configuration.

## Guardrails

- Do not add a new worker backend through `cortex.rs`.
- Do not add Cortex-only task-plan resolution, worktree handling, retries, or
  cancellation policy.
- Do not let Cortex infer ownership from active workers or channel IDs.
- Do not keep a legacy fallback that silently starts a builtin worker when the
  selected backend cannot run.
- Do not treat a detached worker as Cortex-owned when its durable `run_id` or
  initiating context says otherwise.

## Verification

- A task with an OpenCode worker type and a created worktree resolves the same
  worker type and directory for manual and autonomous execution.
- Required skills are validated and injected through every task execution
  trigger.
- An autonomy-owned worker persists its `run_id` before execution starts.
- A terminal worker outcome is committed once when cancellation, timeout, and
  natural completion race.
- A task cannot be claimed by the removed Cortex pickup loop.
- Cortex reconciliation can repair or report durable lifecycle state without
  spawning a worker or changing task priority.
- No `Worker::new` call in `cortex.rs` remains after Phase 4.
