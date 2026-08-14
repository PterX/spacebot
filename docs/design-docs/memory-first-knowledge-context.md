# Memory-First Knowledge Context

Remove read-time knowledge synthesis and render the memory store directly
into the channel prompt. Curation moves to write time, where the process
doing it has conversational context. This supersedes the earlier
knowledge-synthesis continuity design: that design diagnosed the synthesis
as stateless and prescribed edit-in-place; this one removes the
transformation layer entirely, which removes the failure class rather than
managing it. Task breakdown and ordering across both tracks:
[`execution-plan.md`](execution-plan.md).

---

## Why

The synthesis path (`generate_knowledge_synthesis`, `agent/cortex.rs:2914`)
runs an LLM over five memory searches and active tasks and replaces the
Knowledge Context wholesale. The economics are the forcing function:
interval synthesis burns tokens on an instance that is doing nothing — there
is a GitHub issue from a user who exhausted an entire subscription by
leaving an idle instance running. The target invariant is blunt: **an idle
instance makes zero LLM calls** unless autonomy is enabled. The structural
problems compound it:

- **Stateless regeneration.** The previous synthesis is not an input; stable
  facts get re-derived every run and can silently drop between versions.
- **Deduplication by superstition.** It is ordered never to restate layers
  (identity, working memory) it has never been shown.
- **Unanchored ranking.** "Most actionable" with no view of active work.
- **Permanently volatile bytes.** LLM output differs run to run even when
  nothing changed — the block can never be cache-stable. A direct render is
  a deterministic function of the store: byte-identical between writes, so
  "memory write" becomes a clean epoch event under
  [`prompt-stability.md`](prompt-stability.md).
- **Provenance destroyed.** Synthesis blends entries into prose; the render
  keeps each entry's type, date, and identity, so the prompt block, the UI,
  and the store show the same rows.

What synthesis genuinely provided — compression when the store outgrows the
budget — is replaced by write-time consolidation (below), which is where the
compression belongs: performed by the process with the most context, at the
moment it is trying to save.

## Degradation models — why chronicles stay out of the store

Memories and chronicles decay on different axes, and a store's decay model
is its identity:

- **Memories degrade with volume.** The store fills; budget pressure forces
  merge, supersede, tighten. Age is irrelevant — an old fact that is still
  true is healthy.
- **Chronicles degrade with time.** The timeline only extends; recent spans
  stay detailed, old spans roll up into coarser summaries. Volume pressure
  is meaningless, dedup does not apply, and forgetting is forbidden —
  append-only is the invariant.

Merging them would subject episodes to consolidation machinery (or require
special-casing them out of every code path). They stay separate stores with
separate lifecycles, unified at the query surface: recall tools and the UI
search both and label results; the memory→checkpoint range join
([`system-prompt-rework.md`](system-prompt-rework.md) Related work) is the
bridge. Semantic memory consolidates; episodic memory fades; one search box
sees both.

## Design

### 1. The rendered block

The Knowledge Context slot becomes a direct render of the store:

```
## Memory Store
Scope: global · <participants> · <projects>

### Decisions — 4 of 17
- <entry> (<date>)
### Preferences — 2 of 9
### Goals
### Observations — 3 of 38
### Active Tasks        ← rendered from the task store directly
### Project: <name> — 3 of 21   ← scoped, only for projects in scope
### About <participant> — 2 of 54 ← scoped, only for humans present
```

No fill percentage anywhere: the render is a **view** over a store that is
supposed to exceed it, and a view's fill is ~100% by construction — the
renderer always fills its budget by selection. (A utilization header is
honest only where store = render, i.e. authored documents like `HUMAN.md`;
a reference harness whose store is the rendered file reports fill
meaningfully, and that is exactly why the number does not transfer here.)
The honest signals for a view are **what was loaded** — the scope line —
and **how deep each partition goes** — the `shown of total` counts, which
tell the model when branch recall into the full store is worth it. A
section whose entries all fit shows no count.

Importance-ranked within per-type budgets, deterministic ordering
(importance desc, then updated_at, then id) so the bytes are stable between
writes. The block's overall budget is user-configurable with a sensible,
decently high default — operators with tokens to spend can raise it; the
per-type split scales within it. The budget divides across whatever
partitions are loaded: five participants in a channel means five thinner
`About` slices, which is correct view behavior — the store holds everyone
in full, the render shows each person's highest-importance entries, and
recall reaches the rest.

Because the block is epoch-stable (memory writes only), its position
relative to the cache breakpoint is a measurable trade-off rather than a
given: below the last breakpoint it is rebilled every turn despite rarely
changing; a breakpoint layered after it (breakpoints cascade — a memory
write would re-prefill only the durable-context→memory segment) would cache
it at the cost of one of the four breakpoint slots
[`prompt-stability.md`](prompt-stability.md) currently allocates elsewhere.
Measure write frequency against turn frequency on a live instance before
spending the slot. Entries carry their dates. No LLM call anywhere in the render path.
Active tasks — previously delivered via the synthesis gather — render
directly from the task store with their statuses, also deterministic.
Active Tasks always renders, even when the board is empty — task-awareness
is a standing signal, and an empty board is itself information. Scoped
sections are the opposite: they appear only when their human or project is
in scope.

### 2. Scoped loading

The render loads by scope:

- **Global** — always rendered, importance-ranked, kept small by
  consolidation pressure.
- **Participants** — an `### About <name>` subsection per resolved human
  present in the channel (resolution below). The split rule: **authored =
  timeless contract, learned = dated state.** For org humans, `HUMAN.md`
  shrinks to its core — identity, how to work with the person, standing
  rules — and everything dated or volatile (work snapshots, health,
  finances, current plans) lives here as scoped memories, supersedable and
  individually forgettable, instead of being re-edited into a growing
  profile document.
- **Projects** — a `### Project: <name>` subsection per project in scope.
  Projects are first-class entities in the project store, so project
  scoping is a nullable `project_id` column set by the persistence branch
  at save.

#### Humans: provisioned and ambient

Org-graph Humans are *provisioned* — a config entry, a folder, an authored
`HUMAN.md`, permissions, links. Most people the agent meets are *ambient* —
group-chat participants, outsiders — and they deserve memory, not config.
Hundreds of speakers in one channel must never spawn filesystem nodes. Two
tiers, one mechanism:

- **A new memory type: `human`.** A human memory is the anchor record for a
  person — profile notes as content, dated like any memory. Other memories
  about that person link to the anchor through the existing associations
  table rather than carrying a foreign key. Org humans get an anchor too,
  bound to their org id, so the `### About` render is a single mechanism
  for both tiers; org humans additionally carry their authored profile in
  org context.
- **Identity mapping.** A small indexed table maps platform identities
  (`telegram:…`, `discord:…`, `slack:…`) to anchor memories — one person's
  identities across platforms unify under one anchor. Exact-match
  queryable; platform ids never live as free text inside memory content.
- **In-turn resolution, session-cached.** When a participant's message
  arrives, their platform id is resolved against the mapping during prompt
  build — an indexed exact-match lookup, synchronous and effectively free.
  Ids already resolved this session are skipped. An unknown id with no
  anchor loads nothing and costs nothing. No LLM and no vector search
  anywhere in resolution.
- **Reflection owns the write side.** The reflection step asks "did we talk
  to anyone new worth remembering?" — it creates anchors selectively
  (drive-by speakers do not earn one), updates existing ones, and merges
  anchors when it learns two platform identities are the same person
  (supersede one anchor, repoint its identities and associations).
- **Promotion.** Making an ambient person an org Human binds their existing
  anchor to the new org node — the accumulated memories carry over.

Project scope resolution is deterministic — three cheap, auditable signals,
no LLM anywhere: (a) projects pinned to the channel via channel settings,
(b) a project name or alias appearing in the recent message window,
(c) recent project activity in this channel (worktree created, worker
spawned in its root). Scope has **hysteresis**: a signal scopes a project
in, and it stays in scope until a decay period passes with no signal
(default a few hours) — never silently falling out because a mention slid
past a window edge. Scope state is stored, so entering and leaving scope
are explicit, logged events. A participant joining or a project entering
scope is an epoch event: the partition swaps and the block re-renders — the
same named-epoch semantics as every other section, and below the cache
breakpoint, so a swap costs one re-render, never the prefix above.

Scoping is also how a raw render scales past its budget: the store can grow
far beyond any single render because the render is a view — consolidation
bounds each partition's size, scoping bounds how many partitions load.

Over-scoping is the failure mode: a general fact tagged to project A
disappears whenever A is out of scope. The branch scopes only when the
memory is clearly entity-specific, defaults to global, and the consolidation
pass may rescope in either direction.

#### The loaded set: initialized programmatically, refined by branches

Scope rules and importance ranking are reflexes — cheap, deterministic, and
conversationally blind. The layer above them is a **loaded set**: a stored,
versioned list of the memory ids currently in a channel's view. The render
is a pure function of (store, loaded set), so it stays deterministic and
LLM-free; set edits are logged epoch events like scope changes.

- **Initialization** (new session): the programmatic baseline — scoped
  partitions for participants and in-scope projects, seeded by hard vector
  and importance queries against those entities, plus top global
  importance. No conversation exists yet, so the entities are the query.
- **Refinement**: branches see the channel's current loaded set in their
  context. A branch that recalls or saves may, alongside its normal
  response, adjust the set — load the entries it just found relevant,
  release ones that no longer fit the conversation. This is the only
  LLM-driven selection in the system, and it is in-band by construction:
  branches run only when conversation activity spawned them, so curation
  sharpens with use and costs nothing at idle. Selection quality scales
  with activity instead of with a timer.
- **Guardrails**: edits stay within the render budget; the programmatic
  baseline is a floor — branches add to it and reorder above it, never
  evict below it; each loaded entry records its loader (`scope`,
  `importance`, `branch`), so "why is this memory loaded" is queryable.
  Branch-loaded entries do not decay: they stay until a branch releases
  them or a later edit displaces them within the budget. The curator that
  loaded an entry is the curator that unloads it — no timer competes with
  its judgment.

### 3. Write-time consolidation — the load-bearing piece

The organized store is the *output of write pressure*, not of tidiness —
but the pressure is keyed to **store hygiene, not the render budget**. A
view-based render normally shows less than the partition holds, so
"partition exceeds render budget" is the healthy steady state, never a
trigger. Consolidation triggers on two store-side signals instead:

- a per-partition **store cap** set well above any render budget (the point
  where a partition has accumulated enough entries that its
  top-of-importance slice is probably carrying duplicates), and
- **near-duplicate detection at save** — `find_similar` over the existing
  embeddings flags a new entry that closely matches existing ones, and the
  response asks the branch for one atomic consolidation batch: merge
  near-duplicates, supersede dated facts, tighten wording.

Corrections supersede rather than overwrite: the replacement entry carries
the date, and the association graph links it to what it replaced.

**The save itself never fails.** A save that looked like it happened but
did not is the worst failure a memory system can have, and our
consolidation loop is not in-band with the conversation — the persistence
branch can error or be cancelled mid-retry. So both triggers are advisory,
never blocking: the write always lands, the partition is flagged with
consolidation debt, and the render is unaffected — it is a selection
either way, and debt only means the selection is probably carrying
redundancy until the batch runs. Consolidation retries are bounded;
unresolved debt surfaces in the UI and status rather than looping.
Consolidation is serialized per partition — one consolidator at a time;
concurrent saves append and add to the debt rather than racing the batch.

Do not ship the raw render without this. Without write pressure it degrades
into an unranked pile of near-duplicates, which is the failure people
misattribute to "raw memories don't work."

### 4. Cortex: writer, not renderer

Idle behaviors, observation memories, and task elevation stay — the cortex
commits memories like any other process. Retired: `generate_knowledge_synthesis`,
`KNOWLEDGE_SYNTHESIS_SECTIONS`, the `knowledge_synthesis_version` plumbing,
the `memory_bulletin` sync, and both synthesis prompt templates
(`cortex_knowledge_synthesis.md.j2`, `fragments/system/cortex_synthesis.md.j2`).

### 5. Working memory: raw today, one digest per day

Working memory's older spans are currently condensed by cortex intraday
synthesis — the same interval-LLM pattern this design removes, and it would
die silently with the cortex teardown anyway. Replacement:

- **Today** renders as raw events: programmatic, deduplicated
  (`Agent started ×7`), no LLM.
- **Past days** render as daily digests written once by a **built-in cron
  job** at the day boundary (instance timezone), derived from that day's
  chronicle checkpoints across channels, memories written, and system
  events. At most one LLM run per day — and on a day with no activity the
  cron writes "No activity" programmatically, no LLM run at all.
  Write-once, immutable, dated.
- The digest is episodic, so it lives on the chronicle spine: an
  instance-level, channel-less daily checkpoint. It embeds like any
  checkpoint, appears in the timeline, and degrades with time (weekly
  rollups later) — never subject to volume machinery. Working memory
  renders raw today plus the last N daily digests. One invariant guard:
  channel-less digests sit outside the per-channel contiguous-coverage
  invariant, so the memory→checkpoint range join must explicitly exclude
  null-channel checkpoints — a memory resolves to its channel's covering
  checkpoint, never to a digest.
- It runs as an ordinary cron job, visible in the cron list with an
  inspectable, adjustable schedule — the scheduler we already ship, not a
  bespoke cortex loop.

Retired with it: `cortex_intraday_synthesis.md.j2` and the intraday
synthesis path.

The resulting invariant for the whole knowledge layer: **the model never
reads LLM output that was not written once, dated, and stored.** Chronicle
cuts and the daily digest are the only LLM writes in the path; every render
is deterministic.

## Phases

1. Deterministic render function: typed sections, per-type budgets,
   scope line and shown-of-total counts, stable ordering, active tasks,
   dates. Swappable into
   the knowledge-context slot.
2. Budget rejection and the atomic consolidation batch in the memory tools;
   persistence branch prompt gains the over-budget path. Gates the swap —
   phase 1 does not replace the live block until this exists.
3. Human anchors and project scoping: the `human` memory type, the identity
   mapping table, in-turn resolution with the session cache, anchor
   creation/merge in reflection, the `project_id` column, deterministic
   project-scope resolution (channel pins, name match, activity signals),
   scoped rendering with partition swaps as epoch events. Then the loaded
   set: per-channel stored state, programmatic initialization, the branch
   tool for set edits, loader provenance, baseline floor.
4. Remove the synthesis path and templates; cortex loops continue as
   writers.
5. Daily digest cron: instance-level daily checkpoint on the chronicle
   spine, derived from chronicles, memories, and events; working memory
   renders raw today plus the last N digests; retire intraday synthesis.
6. UI: store sizes, consolidation debt, and consolidation events visible;
   the prompt block is a labeled selection of the same rows the store view
   shows; digests appear in the chronicle timeline.

## Acceptance

- The rendered block is byte-identical across turns with no store writes,
  no scope events, and no loaded-set edits (all three are stored, logged
  events — never a side effect of a sliding window or an implicit
  recomputation).
- No LLM call in the render path.
- A still-true memory leaves the block only by explicit supersede or forget
  — holds by construction; a fixture asserts it anyway.
- Saves never fail; consolidation triggers (store cap, near-duplicate
  match) flag debt and request an atomic batch; the batch path succeeds
  atomically; shown-of-total counts in the render match the store.
- Participant and project sections appear only for correlated humans
  present and projects in scope; the same store, participants, and project
  signals always produce the same block (scope resolution is deterministic
  and logged).
- At most one digest LLM run per day; everything LLM-written that the model
  reads is write-once, dated, and stored (chronicle cuts and daily digests
  are the only such writes).
- An idle instance makes zero LLM calls: no messages, no autonomy → no
  synthesis, no digest (empty days short-circuit programmatically), no
  background token spend of any kind.
- Participant resolution is an indexed exact-match lookup — no LLM, no
  vector search, session-cached; an unknown platform id loads nothing.
