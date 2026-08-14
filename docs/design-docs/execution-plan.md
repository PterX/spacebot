# Execution Plan — prompt rework + memory-first knowledge context

Task breakdown for [`system-prompt-rework.md`](system-prompt-rework.md) and
[`memory-first-knowledge-context.md`](memory-first-knowledge-context.md).
Those docs own the design; this one carves the work into PR-sized tasks with
dependencies, ordered into waves. Everything in the same wave can run in
parallel.

There are only two hard dependency chains in the whole plan:

1. **fixtures → template rewrite → breakpoint** — the rewrite may not start
   until the behavioral baseline runs green against the current prompt, and
   the cache breakpoint needs the rewrite's section ordering.
2. **render function + consolidation → synthesis removal** — the direct
   memory render does not replace the live knowledge context until write-time
   consolidation exists.

Everything else lands independently.

---

## Wave 1 — no dependencies, fully parallel

### 1.1 Skill system (rework Phase 1, one PR)

All four items together — one code area (`skills.rs` + the three
`fragments/skills_*.md.j2` templates), mechanical, the largest byte win:

- Flatten index rendering to `- name: description` in all three templates;
  worker variant keeps the suggested marker inline (`- name (suggested): …`).
- Recurse nested categories in `load_skills_from_dir` (`skills.rs:550–602`);
  render `category/subcategory` groupings.
- Read category descriptions from `DESCRIPTION.md`, fall back to `index.md`
  (`load_category_description`, `skills.rs:607–618`).
- Raise `DESCRIPTION_BUDGET` to 160, cut at word boundaries
  (`skills.rs:387–397`), and validate on the import paths
  (`install_skill.rs`, `load_skills_from_dir`) — not just create/edit.

Clears the three skill entries in `TODO`.

### 1.2 Behavioral fixtures (rework Phase 0)

The critical-path item — start first. The harness: canned conversation from
a pinned config, one live call, assertion on first tool choice + key
arguments, N=5 samples, 4/5 threshold. Then the fixture sets:

- Standard-mode: delegation routing, silence, acknowledgment, result relay,
  memory intent handoff, skill suggestion.
- Direct-mode: still delegates long-running work, still branches for memory.
  The "never do X yourself" prohibitions may not be deleted before these
  exist.
- Capability-consistency test: advertised capabilities == registered tool
  set, every mode/config combination.
- Byte-level prompt assertions as a separate cheap suite (gates caching,
  not behavior).

All fixtures must run green against the **current** prompt before Wave 2
begins.

### 1.3 Identity wrapper removal (rework Phase 2, item 5 only)

`Identity::render()` stops emitting `## Soul` / `## Identity` / `## Role`
wrappers and headings for empty files. Files render verbatim; empty files
render nothing. The bundled preset rewrite (item 6) is deliberately *not*
here — it merges with the template rewrite (2.1) for one review.

### 1.4 Time eviction (rework Phase 5, item 17)

Move `current_time_line` out of `status.render_full` (`channel.rs:2330`)
into the user message envelope, with the coalesce hint. Independent of the
rewrite, and it removes the one guaranteed per-turn cache miss — the
highest-value small PR in the plan. Ship early.

### 1.5 Adapter converter verification (rework Appendix A gates)

Investigation only, one agent per adapter: verify each formatting claim
against the converter before its fragment ships (telegram markdown →
entities incl. headers/spoilers, `send_file` type mapping per platform,
`cards`/`blocks` parameter names, twitch message cap and splitting, whether
webhook has a `send_file` path). Output: a verified/failed checklist per
adapter. Converter fixes get filed as their own small PRs — fix the
converter, never weaken the fragment wording.

### 1.6 Memory render function (memory-first Phase 1)

The deterministic render: typed sections, per-type budgets, scope line,
shown-of-total counts, stable ordering (importance desc, updated_at, id),
active tasks from the task store, dates. Built behind a flag, swappable
into the knowledge-context slot but **not swapped** — pure new code, no
conflicts with the prompt track.

### 1.7 Chronicle work (rework Related work, items 1–2)

- Memory → checkpoint range join surfaced in `memory_search` results
  (checkpoint title + seq) and supersede-with-provenance.
- `chronicle_embeddings` LanceDB table beside `memory_embeddings`
  (`memory/lance.rs`): shared connection, model, HNSW+FTS, level-0 rows
  only, one-time backfill, unified labeled results at the query surface.

Independent of everything else in both tracks.

### 1.8 Tool schema enrichment (the additive half of rework 3.12)

- `memory_save`: per-value descriptions on the `memory_type` enum — the
  taxonomy moves here from the channel template.
- `task_create` / `task_update`: lifecycle detail.
- `spawn_worker`: sandbox posture line.

Additive and prompt-neutral, so it lands now; the deletions it enables
(§Cron, §Task Board shrink, §Memory System taxonomy) happen in the
template rewrite.

---

## Wave 2 — gated on Wave 1

### 2.1 Template rewrite (rework Phase 3 + Phase 2 item 6, one PR)

Gated on 1.2 green. `channel.md.j2` and its fragments rewritten together,
plus the bundled preset rewrite — one document, one review:

- Operating contract first; execution model per mode (mutually exclusive
  fragments); the deletions ("branch often", "one worker per task", the
  prohibitions).
- New authority fragment; absorbs `ROLE.md` §Escalation and the org-context
  authority sentence.
- Communication consolidation; the numbered Rules list ceases to exist.
- Memory section shrinks to intent + the loop-closing line.
- Capability deletions enabled by 1.8; §7 keeps proactive triggers and
  cross-tool arbitration only.
- Adapter fragments from Appendix A — only those whose converter claims
  passed 1.5; stragglers follow as individual PRs when their converter is
  fixed.
- Bundled presets rewritten to the file contracts (~1.2k across three
  files); delete-the-identity-files test passes.

Asserted against the 1.2 fixtures and the reference render.

### 2.2 Context tier (rework Phase 4, individually landable PRs)

Good fan-out material — none depend on each other:

- Evidence framing line over the durable/volatile region.
- `HUMAN.md` cap (default 4,000) with inline utilization header; loud
  >100%, section-boundary truncation only past 2×. (The actual profile
  conversion to scoped memories waits for Wave 3 — an over-budget profile
  rendering loud is the correct interim state.)
- Caps on projects and channels sections, unreported.
- Event collapsing (`Agent started ×7`) and scope + shown-of-total counts
  on view blocks.
- `## Other Channels` → `## Channel Activity` with message counts
  (`memory/working.rs:740`).
- Instruction extraction from `org_context.md.j2` /
  `projects_context.md.j2`; worktrees nested under their repo.

### 2.3 Write-time consolidation (memory-first Phase 2)

Per-partition store caps, `find_similar` near-duplicate detection at save,
the atomic consolidation batch in the memory tools, consolidation debt
(advisory, never blocking — the save always lands), bounded retries,
per-partition serialization. Persistence branch prompt gains the
over-budget path. **Gates the swap** — 1.6 does not replace the live block
until this exists.

---

## Wave 3 — memory core + breakpoint

### 3.1 Humans, scoping, loaded set (memory-first Phase 3, three PRs)

- **3.1a — Human anchors**: the `human` memory type, the
  `human_identities` mapping table, in-turn exact-match resolution with
  session cache, anchor creation/merge in reflection, promotion path for
  ambient → org humans.
- **3.1b — Project scoping**: nullable `project_id` column, deterministic
  scope resolution (channel pins, name match, activity signals),
  hysteresis with stored scope state, scope changes as logged epoch
  events.
- **3.1c — Loaded set**: per-channel stored, versioned set; programmatic
  initialization (entity-seeded vector + importance queries); branch tool
  for set edits; loader provenance (`scope` / `importance` / `branch`);
  baseline floor; no decay. Depends on 3.1a/3.1b — it curates their
  partitions.

### 3.2 Cache breakpoint (rework Phase 5, item 18)

Extend the multi-block system seam (`llm/anthropic/params.rs:118`) to carry
the breakpoint after durable context; sections ordered stable → epoch →
volatile per the manifest. Needs the section ordering from 2.1/2.2
settled, not their prose. Byte-stability acceptance: two consecutive
no-epoch turns render identical bytes above the breakpoint. Whether the
memory block earns its own layered breakpoint is measured on a live
instance (write frequency vs turn frequency), not decided here.

---

## Wave 4 — the swap and teardown

### 4.1 Synthesis removal (memory-first Phase 4)

Flip the flag from 1.6. Retire `generate_knowledge_synthesis`,
`KNOWLEDGE_SYNTHESIS_SECTIONS`, the `knowledge_synthesis_version` plumbing,
the `memory_bulletin` sync, and both synthesis templates. Gated on 1.6 +
2.3; sequence after 3.1 so the first live render is the scoped one rather
than a global-only interim.

### 4.2 Daily digest cron (memory-first Phase 5)

Built-in cron at the day boundary: instance-level, channel-less checkpoint
on the chronicle spine, derived from that day's checkpoints, memories, and
events. Empty days short-circuit programmatically — no LLM run. Range join
from 1.7 excludes null-channel checkpoints. Working memory renders raw
today + last N digests. Retire `cortex_intraday_synthesis.md.j2` and the
intraday path.

### 4.3 UI + profile conversion (memory-first Phase 6)

Store sizes, consolidation debt, and loaded-set provenance visible; the
prompt block is a labeled selection of the same rows the store view shows;
digests in the chronicle timeline. Then the live `HUMAN.md` conversion:
reflection proposes which entries become scoped memories, the operator
approves — the harness never rewrites an authored file on its own.

---

## Dependency summary

```
1.2 fixtures ──────────────► 2.1 template rewrite ──► 3.2 breakpoint
1.5 adapter verification ──► 2.1 (per-fragment gate)
1.8 schema enrichment ─────► 2.1 (enables deletions)
1.6 render function ───┬───► 4.1 synthesis removal
2.3 consolidation ─────┘
3.1a/b ──► 3.1c loaded set ──► 4.1 (sequencing preference, not a hard gate)
1.7 chronicle range join ──► 4.2 digest (null-channel exclusion)
```

Everything not named in a chain is independent. Wave 1 is eight parallel
work streams; the two rules worth enforcing are the two gates: no template
rewrite before fixtures run green on the current prompt, and no render
swap without consolidation.
