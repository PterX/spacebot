# Wakes

A Wake is a named condition under which the agent stirs without a user message, paired with instructions for what to do when it fires. Cron schedules, the autonomy interval, webhook deliveries, task approvals, and idle-time enrichment are all the same shape: a trigger, instructions, and a budget. Wakes give that shape one schema, one queue, one authority model, and one UI surface.

This doc defines the Wake model. For the channel that consumes wakes, see [`autonomy.md`](autonomy.md). For the authority model wakes inherit, see [`human-scoped-turn-authority.md`](human-scoped-turn-authority.md).

---

## Why This Exists

Spacebot already has four independent mechanisms that stir the agent without a user present:

- The cron scheduler (`src/cron/scheduler.rs`) — time triggers with instructions, isolated channels, delivery via `set_outcome`.
- The wake substrate (`src/agent/wake.rs`) — `WakeSender`/`fire_wake()`, an mpsc of agent IDs fired by cross-agent delegation and cron completion, consumed as ready-task pickup.
- The webhook adapter (`src/messaging/webhook.rs`) — external HTTP that currently masquerades as an inbound user message.
- The ingestion loop (`src/agent/ingestion.rs`) — a filesystem poll that triggers self-initiated LLM work.

Each grew its own trigger config, its own delivery semantics, and its own (usually absent) authority story. Every future trigger — task approved, goal created, CI failed, quiet hours reached — would otherwise grow a fifth and sixth mechanism. Wakes replace that trajectory with one concept.

---

## The Model

```text
producers                          queue                consumer
─────────                          ─────                ────────
schedule ticks     ─┐
webhook deliveries ─┤
internal events    ─┼──▶  wake events (persisted)  ──▶  autonomy channel run
condition checks   ─┘         coalesced,                 context includes:
                              debounced                  "woken by: X, Y"
```

A Wake firing does not spawn its own ad-hoc process. It enqueues a **wake event** — source, payload, instructions — and pulls the autonomy channel's next run forward. The channel wakes once, sees every event that accumulated since its last run, and acts with full survey context.

This gives three properties for free:

- **Storm safety.** The autonomy channel is single-flight. A webhook flood becomes one run with many payloads, not many runs. Per-wake debounce bounds queue growth before that.
- **Batching.** Events that arrive together are reasoned about together, in priority order, under one budget.
- **Provenance.** Every run records which wakes caused it. Run history answers "why did the agent act?" — not just "what did it do?".

The scheduled autonomy interval from [`autonomy.md`](autonomy.md) is not special machinery: it is the built-in default Wake (`trigger = schedule`, instructions = "survey and work"). Cron jobs keep their existing isolated-channel execution and delivery semantics; they are presented as Wakes in the UI and adopt the same authority rules, but their execution path is unchanged in this design.

---

## Wake Sources

### Schedule

Time triggers: an interval or cron expression, plus one-shots. Everything the cron scheduler's trigger half already supports (`cron_expr`, `interval_secs`, `run_once`, `active_hours`, timezone via `cron_timezone`).

The cron scheduler bisects cleanly: everything from cursor initialization through the claim (stale-cursor fast-forward with grace window, active-hours gating, skip-if-running, CAS `claim_and_advance`, `claim_run_once`) is generic trigger machinery; only the terminal action (spawn isolated channel, deliver outcome) is cron-specific. The schedule producer reuses that layer — a `ScheduleSpec` + cursor-store trait implemented by both `CronJob` and scheduled wakes, with the timer loop generic over its fire action ("insert a wake event and ring the doorbell" instead of "run a cron channel"). Scheduled wakes are not cron rows (`cron_jobs` requires `prompt` and `delivery_target` and carries no wake fields), and they are not a parallel scheduler (the CAS-claim protocol, restart anchoring, and timezone plumbing are already debugged once; see `cron-timezone-and-reliability.md`).

### Webhook

An HTTP endpoint bound to a Wake. The request body becomes the wake event payload, rendered into the run context. Auth is a per-wake bearer token.

Ingress lives on the **API server**, not the messaging webhook adapter: `POST /api/wakes/:id/fire` with the wake's token. The messaging adapter (`src/messaging/webhook.rs`) is a single-instance conversational surface whose only output is `InboundMessage` — binding wake routes to it would require a route registry, per-route auth, and a second output sink threaded through three construction sites. The API server already has auth, per-agent routing, a manual-wake endpoint (`src/api/agents.rs`), and the SSE stream; the manual test-fire endpoint and webhook ingress are the same endpoint. The webhook adapter stays what it is. Note that wake-triggered runs cannot reply to the HTTP caller; delivery, if any, goes through `delivery_target`.

This is the entry point for CI failures, issue trackers, payment events, uptime monitors, and anything else that can POST.

### Event

Typed internal system events, subscribed by filter:

- `task.approved` — start approved work immediately instead of waiting for the next ready-loop poll.
- `task.commented` — a user weighed in on a pending task; re-enrich. (This converts autonomy.md's selection rule 1 from a poll-time priority into an event.)
- `goal.created` / `goal.updated` — a goal with no tasks is the canonical "propose work" signal.
- `worker.completed` / `worker.failed` — completion routing beyond the parent channel.
- `agent.message` — peer delegation over links (already fires `fire_wake` today).
- `cortex.observation` — repeated failures, tripped circuit breakers, adapter outages: self-healing triage.
- `ingest.file_added` — the ingestion loop's poll, expressed as an event.

The event vocabulary is a closed, versioned enum with an `as_str()`/`parse()` pair (the `WorkingMemoryEventType`/`NotificationKind` pattern), which is what makes unknown event names a config error at load time. Wakes subscribe with a filter (`event = "task.approved"`, optionally narrowed by payload fields).

There is deliberately **no new broadcast bus**. The existing buses are all lossy (`ProcessEvent` at 256 slots per agent, `ApiEvent` at 512 instance-wide) and a queue that must survive a crash cannot ride one. Events are emitted as direct calls at the handful of mutation sites that already hand-roll multi-way fan-outs (ApiEvent + notification + working-memory event). Each such site collapses into one helper — e.g. `emit_task_transition(&TaskUpdateResult)` — that performs the existing fan-out plus the wake enqueue, which removes duplication rather than adding a fourth hand-written emission. `task.approved` specifically falls out of switching the approve endpoint to `update_with_status_transition`, whose returned `previous_status` (currently discarded) identifies the `pending_approval → ready` edge exactly.

### Condition

Predicates with no event to hook, evaluated on the existing cortex tick (`CortexConfig.tick_interval_secs`):

- Idleness: no user activity across channels for N minutes — the overnight-enrichment window. This is the same predicate as autonomy.md's "quiet while active" suppression with the sign flipped; both call one named function over `channels.last_activity_at` / `conversation_messages` (the query `render_channel_activity_map` already runs). That predicate must exclude cron and autonomy platforms: cron runs currently write `role='user'` rows and touch `last_activity_at` because the scheduler sets `source` to the delivery adapter, so a naive check would be reset by the agent's own scheduled work.
- Staleness: pending-approval tasks older than N hours (a wake whose output is a nudge to the human), memory-maintenance candidates piled past a threshold, a goal due date approaching with open tasks.

Conditions are declarative config fields, not a free-form expression language. Each condition type is implemented in Rust with typed parameters. A condition that holds continuously fires once on the rising edge, then re-arms only after the condition clears (or after `rearm_secs`, whichever is later).

---

## Schema

```rust
pub struct WakeDef {
    pub id: String,
    pub name: String,
    pub trigger: WakeTrigger,
    /// Jinja template key or inline instructions, rendered into the run
    /// context when this wake contributes to a run. Same convention as cron.
    pub instructions: String,
    /// Minimum seconds between firings. Events arriving inside the window
    /// coalesce into the pending wake event rather than being dropped.
    pub debounce_secs: u32,
    pub active_hours: Option<(u8, u8)>,
    /// Which autonomy levels this wake is eligible at. See "Level Gating".
    pub min_level: AutonomyLevel,
    pub enabled: bool,
    /// Typed delivery target ("discord:dm:123", "slack:work:C042") for
    /// notify-style wakes, parsed and validated like cron's delivery_target.
    /// Output is delivered via broadcast_proactive after the run; prose like
    /// "deliver to my DM" inside instructions is not a delivery mechanism.
    pub delivery_target: Option<BroadcastTarget>,
    /// Persisted, unlike cron's in-memory strike counter, so restarts do not
    /// reset a misfiring wake's progress toward the circuit breaker.
    pub consecutive_failures: u32,
    /// Human or system principal that created this wake. Authority is
    /// re-resolved from this principal at each firing.
    pub created_by: WakePrincipal,
    pub capabilities: CapabilitySet,
}

pub enum WakeTrigger {
    Schedule { expr: ScheduleExpr },
    Webhook { route_id: String },
    Event { event: SystemEvent, filter: Option<EventFilter> },
    Condition { condition: WakeCondition },
}
```

```sql
-- Per-agent database (migrations/), scoped by file like cron_jobs — no
-- agent_id column. Producers reading global tables (task mutations) route
-- to the owning agent's queue via the task's assigned_agent_id.
CREATE TABLE wake_events (
    id           TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
    wake_id      TEXT NOT NULL,
    dedupe_key   TEXT NOT NULL DEFAULT '',  -- coalescing identity within a wake
    payload      TEXT DEFAULT '{}',   -- JSON: webhook body, event fields, condition snapshot
    fired_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    delivery_count INTEGER NOT NULL DEFAULT 1,
    consumed_by  TEXT                 -- run id, set when a run consumes this event
);

CREATE INDEX wake_events_pending ON wake_events(consumed_by, fired_at);
-- Coalescing lives in SQL, not a timer: at most one pending event per
-- (wake, dedupe key), the same partial-unique-index pattern the
-- notifications store uses for duplicate suppression. A coalesced arrival
-- bumps delivery_count instead of inserting.
CREATE UNIQUE INDEX wake_events_coalesce ON wake_events(wake_id, dedupe_key)
    WHERE consumed_by IS NULL;
```

Events are persisted before the channel consumes them, so a crash between firing and running loses nothing. Enqueue-then-ring: after the insert, the producer fires the existing `WakeSender` doorbell (`src/agent/wake.rs`) — unbounded, payload-free, already threaded into both `AgentDeps` and `ApiState`, so API handlers can ring it. Durability comes from the table, liveness from the doorbell, and a missed ring degrades to the retained poll. `send_agent_message` already does exactly this persist-row-then-ring dance; wakes generalize it. Consumption marking uses the same CAS-guarded-UPDATE idiom as `claim_and_advance` and `claim_next_ready`. A run marks the events it consumed; `autonomy_complete` records their wake IDs, which is where run-history provenance comes from.

---

## Authority

Wakes are system principals. Per [`human-scoped-turn-authority.md`](human-scoped-turn-authority.md):

- A wake created by a Human is ceilinged by that Human's authority, re-resolved at each firing. Creation-time authority is a ceiling, not a durable grant; downgrading or blocking the creator narrows or stops their wakes at the next firing.
- A wake whose creator can no longer be resolved fails closed.
- Built-in wakes (the autonomy interval, `task.approved` pickup) run under an explicit configured system policy, not an implicit superuser.
- Webhook-triggered runs never acquire authority from payload content. The payload is data; the wake definition is the authority boundary.

---

## Level Gating

The autonomy dial governs what a wake may cause; wakes govern when the agent stirs. Each wake declares `min_level`:

| Level | Eligible wake work |
|---|---|
| `off` | Nothing fires. Events still persist for later. |
| `observe` | Summarize/annotate-only wakes: surveys, digests, working-memory notes. |
| `suggest` | Plus enrichment and proposal wakes: research, task creation, re-enrichment. |
| `act` | Plus execution wakes: `task.approved` pickup, self-healing actions. |

A wake below the current level does not fire its instructions, but its events still persist and appear in run context as observations once a run happens. Turning the dial up later means the agent knows what it slept through.

---

## Configuration

```toml
[[wakes]]
id = "morning-brief"
name = "Morning brief"
schedule = "0 8 * * *"
instructions = "Summarize overnight activity and what needs my attention today."
delivery_target = "discord:dm:128385659392"
min_level = "observe"

[[wakes]]
id = "ci-failed"
name = "CI failed on main"
webhook_route = "ci"
debounce_secs = 600
instructions = "A CI failure payload is attached. Investigate the failing job and propose a fix task with your findings."
min_level = "suggest"

[[wakes]]
id = "quiet-hours"
name = "Quiet-hours enrichment"
condition = { idle_minutes = 120 }
rearm_secs = 7200
instructions = "The humans are away. Use the time to research pending proposals."
min_level = "suggest"
```

Built-in wakes (`interval-survey`, `task-approved`) exist without configuration and can be tuned or disabled but not deleted. Validation at load: unknown event names, malformed schedules, unknown condition types, and `debounce_secs = 0` on webhook wakes are config errors. Validation mirrors `CronTool::create`'s checks (id charset/length, 5-field expression expand-and-parse, minimum interval, delivery-target adapter existence) rather than the config-seeding path, which validates nothing today.

**Ownership rule:** config is a seed, the database is the source of truth — the same relationship cron has. Wakes created or edited at runtime (API, `wake_create`) are user-owned rows; `[[wakes]]` entries are config-owned and reconciled by id on reload, never clobbering user-owned rows or resetting live cursors. Hot reload of `[[wakes]]` requires a `RuntimeConfig` field whose ArcSwap handle is held at the reload site — the named-adapter permissions gap exists because per-item handles were constructed where the watcher can't reach them; don't repeat that.

---

## API and UI

- `GET /api/agents/:id/wakes` — list wake definitions with last-fired times and recent event counts.
- `POST/PUT/DELETE` for custom wakes; built-ins accept `enabled` and tuning fields only.
- `POST /api/wakes/:id/fire` — manual test fire, recorded with API-client provenance.
- Wake events appear in the SSE stream (`wake_fired`, `wake_consumed`) for live panel updates.

The autonomy panel renders wakes as rows — trigger badge, name, last fired, enable toggle — and run history gains a "woken by" chip per run. The agent itself can propose new wakes through a `wake_create` tool gated behind approval, the same proposal flow as tasks.

---

## Failure Behavior

| Failure | Behavior |
|---|---|
| Webhook flood | Debounce coalesces into one pending event with a delivery count; queue depth is bounded per wake. |
| Wake fires while a run is active | Event persists; the running channel finishes; the next run consumes it. A `task.approved` event may shorten the wait by pulling the next run to immediately-after-completion. |
| Instructions render failure | Event is consumed with an error note in run history; the wake trips a counter. |
| Repeated failures | Three consecutive failed firings disable the wake and emit a notification. Same policy as the cron circuit breaker, but the counter is persisted — cron's lives in memory and resets on restart, which is a gap worth backporting. |
| Creator unresolvable | Wake does not fire; event recorded as denied with reason. |
| Condition flapping | Rising-edge firing plus `rearm_secs` prevents oscillation. |

---

## Implementation Phases

**Phase 1 — Wake definitions and the event queue**
- `ChannelKind { User, Cron, Autonomy }` on channel state, replacing the `cron_outcome.is_some()` and `starts_with("cron")` discriminators — precursor refactor, independently landable
- `WakeDef`, `WakeTrigger`, config loading and validation
- `wake_events` table (per-agent DB), persistence, CAS consumed-by marking, SQL coalescing
- Built-in `interval-survey` wake replacing the bare autonomy interval
- Autonomy channel consumes pending events into run context; `autonomy_complete` records provenance

**Phase 2 — Event and schedule producers**
- Typed `SystemEvent` enum; direct emission at task/goal/worker mutation sites via extracted fan-out helpers (each site already hand-rolls ApiEvent + notification + working-memory emission — the helper consolidates those and adds the wake enqueue)
- Approve endpoint switched to `update_with_status_transition` so the `pending_approval → ready` edge is observable
- Schedule producer sharing the cron trigger layer (`ScheduleSpec`, cursor-store trait, timer loop generic over its fire action)
- `task-approved` built-in wake replacing the ready-loop poll path (poll retained as fallback)

**Phase 3 — Webhooks and conditions**
- `POST /api/wakes/:id/fire` with per-wake tokens, serving both manual test-fires and webhook ingress
- Condition evaluation on the cortex tick: the shared idle predicate (cron/autonomy platforms excluded), staleness
- Debounce, rearm, persisted circuit breaker

**Phase 4 — Authority and surface**
- Creator re-resolution and capability ceilings per firing
- API endpoints, SSE events, panel wiring
- `wake_create` proposal tool

---

## Non-Goals

- **No free-form condition language.** Conditions are typed Rust implementations with declarative parameters.
- **No new broadcast bus.** The persisted table is the queue; the existing `WakeSender` mpsc is the doorbell; `ApiEvent::WakeFired/WakeConsumed` mirrors onto SSE for presentation only.
- **No changes to the messaging webhook adapter.** Wake ingress is an API-server concern.
- **No per-wake channels.** Wakes feed the single autonomy channel; cron keeps its existing isolated execution.
- **No wake-to-wake chaining.** A wake's run can create tasks and proposals, not fire other wakes.
- **No replacement of conversational triggers.** User messages are not wakes; channels behave as they do today.
