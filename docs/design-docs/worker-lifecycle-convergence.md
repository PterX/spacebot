# Worker Lifecycle Convergence

Workers are completing useful work and then being recorded as cancelled. Other workers finish after their autonomy run has already described them as incomplete. Branches cannot receive worker completions, so they poll persisted state and spawn replacement workers. These are ownership and state-transition failures, not one generic reliability problem.

This design gives every worker one authoritative lifecycle, makes terminal transitions monotonic, and defines how branches and autonomy runs hand work off and wait for it. A worker outcome is committed once. Completion, cancellation, timeout, and idle updates converge through the same state machine. Parent processes consume durable outcomes instead of racing worker futures through broadcast events.

Related designs:

- [worker-reliability.md](worker-reliability.md) defines persist-before-notify outcomes, progress-based liveness, and staged termination.
- [autonomous-action-audit.md](autonomous-action-audit.md) defines autonomy run attribution, child settling, and late arrivals.
- [autonomy-lifecycle.md](autonomy-lifecycle.md) moves task selection and execution into the autonomy lifecycle.
- [tool-nudging.md](tool-nudging.md) defines the worker terminal-outcome contract.

This document narrows those proposals around failures observed in the live instance on August 13, 2026. It specifies the shared lifecycle required to implement them safely.

## Observed failures

### Branch polling and worker fan-out

Branch `20bb0fa2-37f6-4adc-b4d4-7668370126c1` was asked to answer one source-grounded question. Its persisted transcript records:

- 40 tool calls before termination
- 25 `worker_inspect` calls
- 13 inspections of its first worker
- 4 workers spawned for substantially the same audit
- direct source inspection by the branch while those workers ran

The branch varied `worker_inspect.limit` from 1 to 50 even though the field is ignored when `worker_id` is present. This defeated argument-hash repetition detection. Each inspection returned `running` with no transcript, so the model spawned a narrower replacement while leaving the previous worker active.

The behavior followed the tool topology:

1. A branch can call `spawn_worker`.
2. The worker completion event targets the parent channel.
3. The spawning branch has no completion receiver or wait primitive.
4. `worker_inspect` is available to the branch.
5. `worker_inspect` reads only `worker_runs.transcript`, not the live transcript cache.
6. The spawn result says the worker "will report back when done" without naming the channel as the recipient.

The branch prompt tells the model to return after delegation, but no runtime transition enforces that instruction.

### Completed workers recorded as cancelled

Three workers from autonomy run `34ec9157-091a-4472-b334-819b2aab7dcb` were cancelled at `00:54:50Z`:

| Worker | Tool calls | State stored | Evidence before cancellation |
|---|---:|---|---|
| `9b4c9d54-195c-4490-b42b-75138b78faac` | 43 | `cancelled` | Called `set_status(kind="outcome")` and produced a full final report |
| `fe097bad-6c30-48d3-a52d-03e6a391c58b` | 31 | `cancelled` | Called `set_status(kind="outcome")` and produced a full final report |
| `fbc45655-b4bb-4339-ac60-716ca7285d35` | 16 | `idle` with a cancellation result | Called `set_status(kind="outcome")` and produced a full final report |

The autonomy channel had received a faster audit for task #7 and treated the longer audit as redundant. It cancelled the worker after the worker had already established its terminal outcome and final text. The cancellation path aborted the task and wrote a cancellation result before normal completion publication settled.

`set_status(kind="outcome")` currently sets an in-memory flag inside `SpacebotHook`. It permits the next text-only response to exit the Rig loop. It does not claim terminal ownership, update the worker registry, or prevent parent cancellation. The parent therefore still sees an active `JoinHandle` during the gap between the outcome tool call and `WorkerComplete` handling.

### Impossible terminal rows

Worker `fbc45655-b4bb-4339-ac60-716ca7285d35` was stored with:

```text
status = idle
completed_at = 2026-08-13T00:54:50Z
result = "Worker cancelled: Autonomy run is closing; avoid orphaning duplicate read-only audit."
```

`idle` and `completed_at` cannot both be true. The row is produced by unordered fire-and-forget writes:

```sql
UPDATE worker_runs SET status = 'idle' WHERE id = ?;
```

and:

```sql
UPDATE worker_runs
SET result = ?, status = 'cancelled', completed_at = CURRENT_TIMESTAMP
WHERE id = ? AND completed_at IS NULL;
```

A delayed idle update can overwrite the terminal status after cancellation commits. Similar races exist for `running` resume updates and terminal completion updates. The database does not enforce the worker state machine.

### Autonomy summary finalized before child completion

Autonomy run `dc115ade-d8ec-48cd-91dc-8cb9588711a2` finished at `00:13:45Z` with this durable claim:

> I resumed in-progress task #15, then cancelled the non-returning worker before run closure to avoid orphaned work. The task remains incomplete and should be retried with verification.

The run's active branch completed at `00:14:19Z`. The task-bound worker result became available immediately afterward and contained full verification that task #15 was complete. Task #15 was marked done at `00:14:45Z`.

The durable run summary became false 34 seconds after the run closed. The late result was not attached to the closed run, and the run had no settling phase in which to revise its conclusion.

### Transcript surfaces disagree

Worker transcripts have two representations:

1. `ApiState.live_process_transcripts` accumulates tool calls, results, output, and process text from live events. The API and UI use it while a process is running.
2. `worker_runs.transcript` stores a compressed durable transcript. Built-in workers write it at segment checkpoints, interactive idle boundaries, and handled terminal paths. Explicit cancellation also drains the live cache into this column.

`worker_inspect` reads only `worker_runs.transcript`. It does not query the live cache. A browser connected to SSE can therefore display live worker activity while a branch receives `running` and `No transcript available for this worker` for the same worker.

The UI distinction between "live" and "persisted" is valid. The inspection tool's claim that it returns a worker's full execution transcript is not valid for a running worker.

## Root causes

### Terminal ownership is implicit

Several actors can terminate a worker:

- the worker returning naturally
- the channel `cancel` tool
- the API cancel endpoint
- the cortex supervisor
- a wall-clock timeout
- process shutdown and next-boot reconciliation

Each path writes status, result, transcript, and events separately. There is no compare-and-swap transition that decides which actor owns terminal completion.

### Events are treated as state

`WorkerComplete` is both a notification and the mechanism by which channel state is retired. It is sent over a broadcast channel and accepted only while the channel still holds the worker handle. Event lag, channel exit, or cancellation can suppress the normal result even when the worker already completed its task.

### Parent and child lifecycles are not linked

Workers know `channel_id` but not their spawning branch or autonomy run. The system cannot answer:

- which branch delegated this worker
- which autonomy run owns this worker
- whether a run may close while this worker remains active
- whether a late result should amend a closed run

The model tries to infer ownership from active-worker lists and process descriptions. That is not sufficient for lifecycle control.

### Cancellation is destructive and immediate

`cancel_worker_with_reason` calls `JoinHandle::abort()`, snapshots the live transcript, writes a cancellation row, and emits a synthetic completion. It does not first check whether the worker has already claimed or produced a terminal outcome. It does not give the worker a chance to commit final text already generated inside its future.

### Prompt contracts conflict with runtime semantics

Branches are told that workers run independently and that the branch should return after spawning. They can still inspect and spawn indefinitely.

Autonomy is told to finish within a bounded run, not to call `autonomy_complete` while needed workers remain active, and not to leave orphaned work. There is no `finish_requested` or `settling` state. Cancelling children becomes the easiest way for the model to satisfy all three instructions.

## Invariants

The implementation must enforce these properties without model cooperation.

1. Every worker reaches exactly one durable terminal outcome.
2. Terminal status, result, transcript reference, completion time, and task linkage commit in one transaction.
3. A nonterminal update cannot overwrite a terminal row.
4. A worker that has claimed terminal completion cannot be reclassified as cancelled by a parent race.
5. Duplicate terminal events are idempotent.
6. Notification happens after the durable outcome commits.
7. Channel death or event loss cannot lose a worker outcome.
8. A branch either returns a conclusion or delegates. It does not supervise a worker.
9. An autonomy run writes its final summary after owned children settle or detach under an explicit policy.
10. Late child outcomes retain their owning run and are visible on that run.
11. Live and durable transcript APIs state which representation they return.
12. Cancellation preserves completed work and converges with concurrent completion.

## Worker state machine

Use one lifecycle field as the authority for transition control. Existing display statuses can be derived from it during migration.

```text
Created
  -> Running
  -> Completing
  -> Succeeded

Running
  -> WaitingForInput
  -> Cancelling
  -> TimingOut
  -> Completing
  -> Failed

WaitingForInput
  -> Running
  -> Cancelling
  -> Completing

Cancelling
  -> Cancelled
  -> Succeeded      completion won before cancellation committed
  -> Partial        cooperative cancellation preserved usable work

TimingOut
  -> TimedOut
  -> Succeeded      completion won before timeout committed
  -> Partial

Completing
  -> Succeeded
  -> Failed         final persistence failed irrecoverably
```

Terminal states are:

- `Succeeded`
- `Partial`
- `Cancelled`
- `TimedOut`
- `Blocked`
- `Failed`

No transition leaves a terminal state. `WaitingForInput` is nonterminal and valid only for interactive workers with no `completed_at`.

### Terminal claim

A successful `set_status(kind="outcome")` begins completion. It must atomically claim:

```text
Running | WaitingForInput -> Completing
```

The claim does not mark the worker successful before final text exists. It prevents cancellation from replacing an outcome that is already being finalized. A concurrent cancel request observes `Completing` and waits for the completion commit. If completion fails, the same convergence point chooses `Partial` or `Failed` with the preserved transcript and outcome summary.

`set_status(outcome)` remains a model-authored declaration, so it cannot by itself prove task correctness. It does prove that the worker is entering its terminal protocol. Cancellation after that point is coordination, not an immediate abort.

### Terminal transaction

All terminal paths call one operation:

```rust
complete_worker(
    worker_id,
    expected_lifecycle,
    outcome,
    result,
    transcript,
    usage,
) -> CompletionCommit
```

The operation:

1. Starts a database transaction.
2. Claims the lifecycle transition with a conditional update.
3. Writes the terminal status, structured outcome, result, transcript, usage, and `completed_at`.
4. Updates the bound task when applicable.
5. Commits.
6. Publishes a completion notification carrying the durable outcome version.

If another terminal path already committed, the operation returns the existing outcome. Callers do not overwrite it or publish a contradictory event.

### Conditional nonterminal writes

Every nonterminal status write carries an expected source state and excludes completed rows. For example:

```sql
UPDATE worker_runs
SET lifecycle = 'waiting_for_input'
WHERE id = ?
  AND lifecycle = 'running'
  AND completed_at IS NULL;
```

```sql
UPDATE worker_runs
SET lifecycle = 'running'
WHERE id = ?
  AND lifecycle = 'waiting_for_input'
  AND completed_at IS NULL;
```

Zero affected rows are a transition result, not a successful update. The caller reads the current lifecycle and handles terminal convergence explicitly.

## Cancellation

Cancellation becomes cooperative by default:

1. Attempt `Running | WaitingForInput -> Cancelling`.
2. If the worker is `Completing`, wait for its terminal commit.
3. Signal a cancellation token observed by the worker loop and active LLM/tool select.
4. Give the worker a bounded grace period to persist final or partial work.
5. Use `JoinHandle::abort()` only after the grace period.
6. Route both cooperative and forced paths through `complete_worker`.

Human-requested cancellation may use a short grace period. Autonomy cleanup and supervisor intervention should use the normal grace period. Neither path may overwrite `Succeeded`.

Cancellation reasons remain audit metadata. They do not replace a worker's result when the worker already completed. A late cancellation request against `Succeeded` returns `AlreadyCompleted` and the existing outcome.

## Branch delegation

A successful worker spawn is a terminal handoff for a normal branch.

```text
Branch Running
  -> returns conclusion -> Done
  -> spawn fails        -> Running
  -> spawn succeeds     -> Delegated -> Done
```

After spawn succeeds:

- record `origin_branch_id` on the worker
- stop the branch loop without another model turn
- return a deterministic branch conclusion naming the delegated task, not internal protocol details
- deliver the worker outcome to the channel
- let the channel retrigger from the durable outcome

`worker_inspect` is removed from default branch tools. It remains available to reflection passes and operator/cortex diagnostics for completed-worker analysis.

Legitimate parallel delegation uses one explicit bounded operation that accepts a batch of independent tasks, spawns them atomically up to the configured limit, and ends the branch. Incremental spawn-inspect-spawn supervision is not a branch capability.

Each branch gets an idempotency key for delegation. Retrying the same delegation returns the existing worker rather than creating another one. Exact task-string equality remains a secondary admission check, not the identity mechanism.

## Autonomy run settling

`autonomy_complete` becomes a finish request:

```text
Running -> FinishRequested -> Settling -> Completed
                              |          -> TimedOut
                              -> Detached
```

The run owns children through a system-stamped `run_id` on every worker and branch. `origin_branch_id` preserves the intermediate delegation chain.

When finish is requested:

1. Stop accepting new worker and branch spawns for the run.
2. Classify owned children.
3. Wait for non-interactive children already in `Running`, `Cancelling`, or `Completing`.
4. Do not wait for interactive workers. Detach them under cortex supervision.
5. Retrigger the autonomy model with all settled outcomes.
6. Compose and commit the final summary after settling.

The run has a bounded settle cap. Reaching it does not blindly cancel children. Each remaining child follows an explicit policy:

- continue detached under cortex supervision
- cooperatively cancel when the work is disposable or duplicated
- mark late and retain run attribution

Late outcomes append to the owning run's activity ledger. They do not rewrite the original summary silently, but the run detail shows the late result and its timing. A later chronicle can reconcile the summary with the late activity.

The autonomy model does not decide whether a child is still active by polling `worker_inspect`. The runtime supplies the settling state and wakes the run when it changes.

## Transcript contract

Live and durable transcript access remain separate because they have different consistency guarantees.

### Live transcript

The live cache is an event-derived view for active processes. It can contain:

- tool calls
- streaming tool output
- tool results
- model text emitted between calls

It is suitable for the UI and operator diagnostics. It is not the terminal record.

### Durable transcript

The durable transcript is committed at checkpoints and as part of terminal completion. The terminal transaction stores the final transcript version or a reference to it before notification.

### Inspection behavior

`worker_inspect` must do one of two explicit things for a running worker:

1. Return a labelled live snapshot from `ApiState.live_process_transcripts`, including its non-durable status.
2. Reject the request with `worker is still running; its durable transcript is not available yet`.

It must not return `No transcript available` when live activity exists. The tool schema must state that `limit` applies only to list mode and is ignored when `worker_id` is present.

The UI continues preferring SSE state while connected, the server live cache after refresh, and the durable transcript after terminal commit. On completion, it keeps the live view until the durable outcome query observes the committed transcript version. This removes the gap where completion clears the live cache before the durable transcript becomes readable.

## Durable ownership fields

Add system-stamped fields to `worker_runs`:

```text
lifecycle          TEXT NOT NULL
outcome_kind       TEXT
outcome_summary    TEXT
outcome_version    INTEGER NOT NULL DEFAULT 0
run_id             TEXT
origin_branch_id   TEXT
terminal_owner     TEXT
```

`terminal_owner` records which mechanism committed the outcome: `worker`, `cancel`, `timeout`, `supervisor`, `shutdown`, or `reconcile`. It is operational evidence, not model-authored content.

Add `run_id` and `origin_branch_id` to branch records where applicable. Existing rows remain null. Do not infer historical ownership from timestamps.

## Event semantics

`WorkerComplete` becomes a notification that a durable outcome version exists. It is not the outcome authority.

Consumers process notifications idempotently by `(worker_id, outcome_version)`. A channel that misses the event can reconcile from `worker_runs`. A channel that receives duplicates applies the outcome once. Removing a handle or releasing a concurrency slot is driven by the durable terminal lifecycle, not by which event arrived first.

The event may carry a summary for latency, but consumers verify or load the durable outcome before task transitions and user delivery.

## Circuit breakers and budgets

A total tool-call count is not lifecycle control. The global 20/40/80-call termination thresholds are removed. They killed productive branches and workers based only on work volume, discarded useful conclusions, and did not detect the polling pattern because arguments varied.

Targeted protections remain:

- identical consecutive call detection
- identical outcome detection
- bounded retry budgets for provider and context failures
- ping-pong detection
- output and context size limits

Budget exhaustion returns `Partial` with preserved work. It does not synthesize `Cancelled` or erase a terminal claim.

## Recovery and reconciliation

Startup and periodic reconciliation operate on lifecycle state:

- `Completing` with a committed terminal payload becomes its recorded terminal state.
- stale `Running` without an active registry entry becomes `Failed` or `Partial` with restart provenance.
- `WaitingForInput` is resumed only when the interactive backend is resumable.
- terminal rows with nonterminal display status are repaired from `outcome_kind` and `completed_at`.
- task state is reconciled from the bound worker's durable outcome.

Reconciliation never changes one terminal outcome into another. Conflicts are logged with both attempted owners and outcome versions.

## Verification

### Transition tests

- `Running -> Completing -> Succeeded`
- `Running -> Cancelling -> Cancelled`
- `WaitingForInput -> Running`
- nonterminal updates against terminal rows affect zero rows
- terminal transitions are idempotent
- illegal transitions return structured errors

### Race tests

- cancellation racing `set_status(outcome)` converges to the completion result
- cancellation racing final transcript persistence produces one terminal row
- timeout racing completion produces one terminal row
- delayed idle write cannot overwrite `Cancelled` or `Succeeded`
- duplicate `WorkerComplete` notifications deliver once
- completion before handle registration still reconciles correctly
- channel death after worker completion does not lose the result

### Branch tests

- successful spawn ends the branch without another LLM turn
- spawn failure leaves the branch active
- delegation retry returns the original worker
- default branches cannot call `worker_inspect`
- bounded batch fan-out creates only the requested workers
- worker completion retriggers the channel exactly once

### Autonomy tests

- `autonomy_complete` with active owned children enters settling
- final summary is written after child outcomes are available
- a child finishing during the settle race is included once
- settle-cap expiry detaches or cooperatively cancels according to policy
- late outcomes retain `run_id` and appear in run detail
- interactive children do not hold the run open
- the task #15 timeline cannot produce an "incomplete" final summary before its owned branch settles

### Transcript tests

- running process detail returns the server live cache after UI refresh
- completion does not clear live data before the durable transcript is readable
- cancellation preserves all completed tool calls and final text when present
- `worker_inspect` labels live snapshots or rejects running workers explicitly
- `limit` variations do not alter worker-id inspection behavior

## Delivery phases

### Phase 1: monotonic persistence

Add lifecycle and outcome fields, conditional nonterminal updates, one terminal transaction, and idempotent completion notifications. Fix the `idle + completed_at` corruption first.

### Phase 2: completion and cancellation convergence

Make `set_status(outcome)` claim `Completing`. Route cancel, timeout, supervisor, and normal return through the terminal transaction. Add cooperative cancellation and the abort backstop.

### Phase 3: branch handoff

End normal branches after successful delegation, remove live worker inspection from their toolset, add `origin_branch_id`, and make delegation retries idempotent. Add explicit bounded batch delegation if parallel handoff is required.

### Phase 4: autonomy attribution and settling

Stamp `run_id`, make `autonomy_complete` a finish request, wait for owned non-interactive children, apply explicit detach/cancel policy at the settle cap, and compose summaries after settling.

### Phase 5: transcript consistency

Define live and durable inspection behavior, bridge the completion handoff in the UI, and expose transcript version/readiness in process detail.

### Phase 6: reconciliation

Make channels and cortex recover durable outcomes after missed events, repair historical impossible states where evidence is unambiguous, and add operational metrics for transition conflicts and late outcomes.

## Non-goals

- Judging whether a model-authored outcome is factually correct. The lifecycle guarantees completion consistency, not task quality.
- Keeping every autonomy child inside the run indefinitely. The settle cap and detach policy remain bounded.
- Persisting hidden provider chain-of-thought. Transcripts contain provider-exposed text, tool activity, and results.
- Inferring `run_id` or branch ownership for historical rows.
- Replacing targeted retry and repetition guards. This removes count-based termination, not all loop protection.
