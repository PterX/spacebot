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

Time triggers: an interval or cron expression, plus one-shots. Everything the cron scheduler's trigger half already supports (`cron_expr`, `interval_secs`, `run_once`, `active_hours`, timezone).

### Webhook

An HTTP route bound to a Wake instead of to a fake inbound message. The request body becomes the wake event payload, rendered into the run context. Auth is per-wake bearer token, as the webhook adapter does today. This is the entry point for CI failures, issue trackers, payment events, uptime monitors, and anything else that can POST.

### Event

Typed internal system events, subscribed by filter:

- `task.approved` — start approved work immediately instead of waiting for the next ready-loop poll.
- `task.commented` — a user weighed in on a pending task; re-enrich. (This converts autonomy.md's selection rule 1 from a poll-time priority into an event.)
- `goal.created` / `goal.updated` — a goal with no tasks is the canonical "propose work" signal.
- `worker.completed` / `worker.failed` — completion routing beyond the parent channel.
- `agent.message` — peer delegation over links (already fires `fire_wake` today).
- `cortex.observation` — repeated failures, tripped circuit breakers, adapter outages: self-healing triage.
- `ingest.file_added` — the ingestion loop's poll, expressed as an event.

The event vocabulary is a closed, versioned enum. Wakes subscribe with a filter (`event = "task.approved"`, optionally narrowed by payload fields). Unknown event names are a config error at load time.

### Condition

Predicates with no event to hook, evaluated on the existing cortex tick (`CortexConfig.tick_interval_secs`):

- Idleness: no user activity across channels for N minutes — the overnight-enrichment window. This is the same predicate as autonomy.md's "quiet while active" suppression with the sign flipped; both read one shared activity clock.
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
CREATE TABLE wake_events (
    id           TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
    wake_id      TEXT NOT NULL,
    agent_id     TEXT NOT NULL,
    payload      TEXT DEFAULT '{}',   -- JSON: webhook body, event fields, condition snapshot
    fired_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    consumed_by  TEXT                 -- run id, set when a run consumes this event
);

CREATE INDEX wake_events_pending ON wake_events(agent_id, consumed_by, fired_at);
```

Events are persisted before the channel consumes them, so a crash between firing and running loses nothing. A run marks the events it consumed; `autonomy_complete` records their wake IDs, which is where run-history provenance comes from.

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
instructions = "Summarize overnight activity and what needs my attention today. Deliver to my DM."
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

Built-in wakes (`interval-survey`, `task-approved`) exist without configuration and can be tuned or disabled but not deleted. Validation at load: unknown event names, malformed schedules, unknown condition types, and `debounce_secs = 0` on webhook wakes are config errors.

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
| Repeated failures | Three consecutive failed firings disable the wake and emit a notification, mirroring the cron circuit breaker. |
| Creator unresolvable | Wake does not fire; event recorded as denied with reason. |
| Condition flapping | Rising-edge firing plus `rearm_secs` prevents oscillation. |

---

## Implementation Phases

**Phase 1 — Wake definitions and the event queue**
- `WakeDef`, `WakeTrigger`, config loading and validation
- `wake_events` table, persistence, consumed-by marking
- Built-in `interval-survey` wake replacing the bare autonomy interval
- Autonomy channel consumes pending events into run context; `autonomy_complete` records provenance

**Phase 2 — Event and schedule producers**
- Internal event bus taps: `task.approved`, `task.commented`, `goal.*`, `worker.*`
- Schedule producer for custom scheduled wakes
- `task-approved` built-in wake replacing the ready-loop poll path (poll retained as fallback)

**Phase 3 — Webhooks and conditions**
- Webhook routes bound to wakes; payload capture; per-wake auth
- Condition evaluation on the cortex tick: idleness, staleness
- Debounce, rearm, circuit breaker

**Phase 4 — Authority and surface**
- Creator re-resolution and capability ceilings per firing
- API endpoints, SSE events, panel wiring
- `wake_create` proposal tool

---

## Non-Goals

- **No free-form condition language.** Conditions are typed Rust implementations with declarative parameters.
- **No per-wake channels.** Wakes feed the single autonomy channel; cron keeps its existing isolated execution.
- **No wake-to-wake chaining.** A wake's run can create tasks and proposals, not fire other wakes.
- **No replacement of conversational triggers.** User messages are not wakes; channels behave as they do today.
