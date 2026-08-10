# Dormancy

The agent survives its process. The test is one sentence: **at any quiet moment the process can be killed, and a fresh process, given only the durable state, continues as the same agent** — same conversations, same obligations, same pending work, same prompt-cache economics. A dormant agent is a directory (configuration plus its SQLite databases), not a running machine, and it costs what storage costs.

This doc defines what dormancy requires and the runtime split that makes it deployable. It depends on three invariants established elsewhere: durable triggers ([`wakes.md`](wakes.md)), a durable byte-stable transcript ([`durable-transcript.md`](durable-transcript.md)), and deterministic prompt rendering ([`prompt-stability.md`](prompt-stability.md)).

---

## Why This Exists

A spacebot agent is a resident process today, and most of what makes it *that agent* — its identity, transcript, tasks, memory, schedules — is already in SQLite. The gap between "resident process" and "unit of state" is an enumerable list of things that live only in memory, plus one genuine architectural constraint (messaging connections). Closing that gap buys three unrelated-looking things with one mechanism:

- **Idle economics.** An agent that is mostly asleep — which is most agents, most of the time — should cost disk, not a machine. A fleet of deployed agents is viable exactly to the degree that a sleeping one is free.
- **Operational freedom.** Deploys, host migration, and crash recovery stop being events the agent experiences. A process is a vehicle; getting out of one and into another loses nothing, including the provider prompt cache.
- **Custody.** The dormant form is a portable artifact the operator owns: copy the directory, move it to another host, back it up, park it for a year. The agent's continuity is not coupled to any process, machine, or hosted service. This is a property to state positively in product terms; it falls out of the architecture rather than being a feature bolted on.

The wakes design already crossed the conceptual line: once every reason-to-act is a persisted wake event with provenance, *which process consumes the queue* is an implementation detail. Dormancy is the follow-through.

---

## State Audit

Everything a running spacebot holds in memory, sorted by what dormancy requires of it. The invariant: every entry is either **persisted**, **reconstructible** from durable state, or an **accepted loss** with a stated blast radius. Nothing is load-bearing and unaccounted for.

| State | Where it lives | Disposition |
| --- | --- | --- |
| Channel history | `Arc<RwLock<Vec<Message>>>` per channel | Persisted — the transcript table ([`durable-transcript.md`](durable-transcript.md)) |
| Tasks, checkpoints, assignments | tasks store (SQLite) | Already persisted |
| Cron cursors | SQLite, CAS claims | Already persisted; restart anchoring exists ([`cron-timezone-and-reliability.md`](cron-timezone-and-reliability.md)) |
| Wake queue, debounce windows, condition re-arm state | wake stores | Persisted per [`wakes.md`](wakes.md) — a queue that must survive a crash cannot ride a lossy bus |
| Working memory buffers | in-memory, per channel | Persisted — the event rows are already durable; the rendered view is reconstructible |
| Cortex in-memory history | cortex process | Accepted loss today ([`cortex-history.md`](cortex-history.md)); becomes persisted or explicitly bounded as part of this work |
| Memory bulletin / knowledge synthesis | `ArcSwap` slots | Reconstructible — recomputed by the cortex on next tick; blast radius is one stale render |
| In-flight turn | the running future | Accepted loss at shutdown: the turn completes before exit (drain), or rolls back to the pre-turn transcript exactly as a hard error does today |
| Prompt snapshots | redb, debug-gated | Already persisted; diagnostic only |
| Rate-limit windows, connection backoff | adapter/process memory | Accepted loss — worst case is one over-eager reconnect |

The audit is the deliverable of the first phase: each row becomes either a pointer to existing persistence, a change, or a documented acceptance. Anything discovered outside this table joins it.

---

## The Split: Ingress and Brain

The one thing that genuinely cannot be stateless is a socket. Messaging adapters hold live connections (Discord gateway, Slack socket mode); a webhook can wake a dead process, but a websocket cannot exist without a resident one. So the runtime splits along that line:

```text
ingress (resident, tiny)              brain (materializable)
────────────────────────              ──────────────────────
platform connections                  channels · turns · workers
inbound → durable queue               cortex · compaction
outbound delivery                     wake consumption
liveness/presence                     everything with an LLM in it
        │                                     ▲
        └── enqueue + doorbell ───────────────┘
              (same path wakes
               already defines)
```

- **Ingress** holds connections and translates: inbound platform events become durable rows (inbound messages and wake events — the same enqueue-and-doorbell path [`wakes.md`](wakes.md) defines), and outbound messages are delivered on behalf of the brain. It contains no agent logic and no model calls; its footprint is a connection holder's. It is also optional: deployments whose only surfaces are the API server and webhooks have no resident requirement at all.
- **Brain** is the agent: it boots, rehydrates from durable state, drains the queue, runs turns and background work, and — in the deployment shapes that want it — exits when idle.

This is one binary with roles, not two products. The default deployment runs both roles in one resident process exactly as today, and nothing about a self-hosted single-agent install changes. The split is a boundary inside the code (adapters talk to the queue, not to channels) that deployment shapes can then exploit.

## Deployment Shapes

| Shape | Ingress | Brain | Idle cost |
| --- | --- | --- | --- |
| Resident (default) | in-process | in-process, always on | one process |
| Suspended | in-process | frozen by supervisor, woken on traffic | pages on disk |
| Materialized | separate small process | started on demand, exits when idle | ingress only |

The suspended shape needs nothing from spacebot beyond clean signal handling — a supervisor that freezes and thaws the process (or the VM under it) preserves memory, and the wake path already tolerates delivery latency. The materialized shape is the full expression: the brain's lifecycle is boot → rehydrate → drain → work → idle-exit, and *any* external supervisor — a socket-activated unit, a container autoscaler, a control plane that starts the brain when the queue is non-empty — can own the start decision. Spacebot deliberately does not ship a supervisor; it ships a process that is safe to start and stop, and lets the environment be opinionated.

Rehydration cost is the constraint that makes [`prompt-stability.md`](prompt-stability.md) and [`durable-transcript.md`](durable-transcript.md) prerequisites rather than siblings: a brain that reboots into a byte-identical request keeps its provider prompt cache across materializations (within the cache TTL), so waking is cheap in tokens, not just in milliseconds. Without those invariants, every wake pays a full-context cache write and dormancy's economics invert.

## Lifecycle

- **Boot.** Open stores, rehydrate transcripts for channels with queued work (lazily — a channel rehydrates when addressed, not all at once), register with ingress if separate.
- **Drain.** Consume the wake queue and inbound messages in the order and coalescing [`wakes.md`](wakes.md) defines. Provenance rows already record why the brain woke.
- **Idle-exit.** A single policy decides quiescence: no queued work, no in-flight turns or workers, no wake due within a configured horizon. On the decision: flush, checkpoint WALs, exit 0. The policy is conservative by construction — a wrong "stay up" costs a process-hour; a wrong "exit" costs nothing if boot is correct, which is the invariant the audit protects.
- **Shutdown on signal.** Same path as idle-exit with a deadline: finish or roll back the in-flight turn, flush, exit. This replaces "the process died and we hope" with "the process left and it doesn't matter."

---

## Phases

1. **State audit.** Land the table above against the code, close the unaccounted rows (cortex history is the known one), and make clean shutdown provably lossless — kill-at-quiet-moment becomes a test, not a hope.
2. **Queue boundary.** Route adapter inbound through the durable queue unconditionally, so the in-process default already exercises the ingress/brain seam.
3. **Idle-exit lifecycle.** The quiescence policy, drain-on-signal, and lazy rehydration. At this point the suspended and materialized shapes are deployment choices, not code changes.
4. **Ingress role.** The standalone connection-holder process for materialized deployments that need resident platform connections.
