# Autonomous Action Audit: attributing what the agent does on its own

When the agent acts without a human in the loop, three questions must have
durable answers: what did it do, which run did it belong to, and why did it
reach outside. Today none of them reliably do. A message can arrive on the
user's phone with no ledger row anywhere connecting it to the run that caused
it — not because anything failed, but because the run had already declared
itself finished while its own work was still in flight.

This doc fixes the attribution chain end to end. It is scoped to autonomous
operation: the autonomy loop, its spawned workers and branches, and the
outward-facing actions they produce.

## The incident this is derived from

Reconstructed from the live instance, all times UTC:

| Time | Event |
|---|---|
| 02:16:04 | autonomy run `a310eed2` starts; wake prompt injected into `autonomy` |
| 02:17:30 | the model calls `autonomy_complete` — run marked completed, summary written about a different project |
| 02:17:33 | the worker that run spawned *starts* |
| 02:19:13 | worker finishes: 27 tool calls, a 4,528-char repository survey |
| 02:19:40 | the survey is sent to the user's Telegram |

The run's stored summary and its `actions` array describe none of this. The
outbound message at 02:19:40 falls inside no run window at all. The only
durable trace of the entire episode is one `worker_runs` row, whose
`channel_id` records *where* it ran but not *which tick* it belonged to.

Three independent defects produced that outcome, and each needs its own fix.

## Defect 1: a run ends when the model says so, not when its work is done

`autonomy_complete` ([`src/tools/autonomy_complete.rs`](../../src/tools/autonomy_complete.rs))
is a model-invoked terminal contract; `run_autonomy_channel`
([`src/agent/autonomy.rs:397`](../../src/agent/autonomy.rs)) only synthesizes
an outcome when `handle.completed()` is false. Nothing checks whether the run
still has outstanding children. A model that spawns a fire-and-forget worker
and then calls `autonomy_complete` closes the ledger on work that has not
started yet — which is exactly what happened, three seconds before the worker
began.

**Fix, in two parts.**

*Attribution.* Add `run_id TEXT` to `worker_runs` and `branch_runs`, populated
from the spawning process context, indexed for run lookup. `channel_id =
'autonomy'` tells you a delegate ran under autonomy; it cannot tell you which
of the day's 96 ticks owned it. This column is what makes every later fix
possible, and it is stamped by the system — never supplied by the model.

*Settling.* `autonomy_complete` becomes a request to finish rather than an
unconditional close. When outstanding children exist, the run stays open and
the model is told to wait for them; if it has nothing left to do it simply
yields. A run closes when its children settle, when it hits
`run_settle_cap_secs`, or when the existing timeout fires — whichever comes
first, so a wedged worker can never hold a run past the next scheduled tick.
Children that land after the cap still carry `run_id` and append to the closed
run's `actions` as late arrivals rather than disappearing.

The ordering constraint that matters: **the summary is written after settling,
never before.** A summary composed while delegates are in flight is guaranteed
to misrepresent the run, and it is the summary that feeds working memory and,
downstream, everything the agent later believes about its own day.

## Defect 2: the autonomy channel is write-only

`conversation_messages` for `channel_id = 'autonomy'` contains wake
injections and nothing else — 158 rows, all of them the same injected prompt.
No assistant turns, no delegate-result injections, no record of the decision
to message the user.

Every audit question about autonomous behaviour is therefore unanswerable
after the fact, and the omission compounds: the chronicler reads conversation
history ([`src/agent/chronicle.rs`](../../src/agent/chronicle.rs)), so the
agent's own self-directed work is the one category of work it can never
recall later. Autonomy is the only channel that thinks out loud into a void.

**Fix.** Persist autonomy turns through the same path every other channel
uses: assistant turns and delegate-result injections both. The autonomy
channel's contract with the model is unchanged — nothing it says is
*delivered* — but "not delivered" and "not recorded" are different
properties, and conflating them is what erased this incident. Chronicling
follows for free once the rows exist, which is what gives the agent recall
over its own autonomous history.

Volume is the obvious objection and it is bounded: runs are already capped in
frequency and duration, and the existing compaction and chronicle machinery
applies unchanged.

## Defect 3: outward actions carry no reason

`SendMessageArgs` ([`src/tools/send_message_to_another_channel.rs:66`](../../src/tools/send_message_to_another_channel.rs))
is `{ target, message }`. Nothing records why the agent crossed from an
internal context into a user-visible one, so an unexplained message is
indistinguishable from a malfunctioning one — which is precisely how the
incident was experienced.

**Fix.** A required `why` argument, one sentence, stated in the tool
description as the audit trail for autonomous action. Required
unconditionally, not "required when autonomous": a conditional audit field is
skipped exactly when it matters most, and a one-line reason on a routine
conversational send costs nothing.

`why` is paired with, not a substitute for, system-stamped provenance. They
are different kinds of fact:

- **`why` is intent.** Only the model knows it. It cannot be derived.
- **Provenance is observation.** The originating run, process type, and
  process id are stamped at the tool boundary from context the system already
  holds.

Never ask the model to report *that* it was acting autonomously. It supplies
the reason; the system supplies the circumstances. A model-reported
provenance field is forgeable by omission, by hallucination, or by honest
confusion about which context it is running in — and this run demonstrated
the model misjudging its own lifecycle state.

Both land in the outbound message's `metadata` and as an entry in the owning
run's `actions`. The `why` never appears in the delivered message body.

**Scope.** Implement on cross-channel send now. Shape the audit fields as a
shared convention rather than fields private to one tool, so `send_agent_message`
and the rest of the blast-radius surface adopt it without reinvention. Do not
build a framework ahead of the second caller.

## Defect 4: none of it is visible

Attribution that never reaches the user is bookkeeping. The autonomy run
detail view ([`src/api/autonomy.rs`](../../src/api/autonomy.rs) backs it)
gains its delegates — resolved through `run_id`, late arrivals included — and
its outward actions, each showing target, time, and `why`. A message the user
did not expect should be traceable to its cause in one click.

The failure here was never that the agent sent a message. It was that the
message could not be accounted for.

## Defect 5: sends are shaped by the origin, not the destination

A cross-channel send renders under the origin channel's adapter guidance; the
autonomy channel carries no adapter fragment at all. The only
destination-shaping instruction in scope was a single line in the agent's
identity block, and the result was a 3,000-character report delivered to a
chat surface.

**Fix.** Route cross-channel sends through the destination adapter's
formatting, the same shaping a normal reply to that channel receives. Lowest
priority of the five: a well-attributed message in the wrong shape is a far
smaller problem than an unattributed one.

## Phases

**Phase 1 — attribution.** `run_id` on `worker_runs` and `branch_runs`,
stamped from process context; backfill left null. Run settling in
`run_autonomy_channel`, `autonomy_complete` becoming a finish request,
`run_settle_cap_secs` config, late-arrival appends. Summary composed after
settling. Tests: a run with an outstanding fire-and-forget child does not
close; a child exceeding the cap appends instead of vanishing; the cap never
pushes a run past the next tick.

**Phase 2 — persistence.** Autonomy assistant turns and delegate-result
injections written through the normal conversation path; chronicling verified
over an autonomy channel.

**Phase 3 — the audit field.** Required `why` on cross-channel send, tool
description and prompt guidance, system-stamped provenance at the tool
boundary, persisted to message metadata and run actions.

**Phase 4 — surfacing.** Run detail shows delegates and outward actions with
their reasons.

**Phase 5 — destination formatting.** Cross-channel sends adopt the target
adapter's shaping.

## Non-goals

- **No blocking on interactive workers.** Settling waits on fire-and-forget
  delegates only; an interactive worker is steered by a human and has no
  business holding a scheduled run open.
- **No approval gate.** This makes autonomous action accountable, not
  permissioned. The blast-radius rule already governs what requires
  confirmation, and nothing here changes it.
- **No `why` in delivered content.** It is audit metadata. A message that
  needs to explain itself to its reader should say so in its own words.
- **No retroactive attribution.** Existing rows keep a null `run_id`; the
  ledger starts being trustworthy from the migration forward rather than
  being guessed backward from timestamps.
