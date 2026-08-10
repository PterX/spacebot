# Session Chronicles

An append-only chronicle of a channel's past, cut at intervals instead of rewritten under pressure. Each checkpoint summarizes only the span since the previous one, is durable, and is never re-summarized from raw. The active context carries a bounded window of that chronicle plus recent raw turns, and the agent can walk back through it — list the checkpoints, open one, expand a range into raw transcript.

This is a second compaction mode, selected by config, sitting alongside the rolling compactor. For the transcript-side invariant this eventually composes with, see [`durable-transcript.md`](durable-transcript.md). For where the chronicle view is allowed to render, see [`prompt-stability.md`](prompt-stability.md).

[Reflection notices](#reflection-notices) ride the same spine: the memory persistence branch reports what it learned and why in plain English, those notes are durable and append-only like checkpoints, and each checkpoint carries the ones committed inside its span.

---

## Why This Exists

Compaction today is a single rolling summary held in memory. `Compactor::check_and_compact` (`src/agent/compactor.rs`) runs after every turn from four sites in `src/agent/channel.rs` (1944, 2052, 2396, 2583), divides `estimate_history_tokens(&history)` by the context window, and compares against `CompactionConfig` (`src/config/types.rs:757`, defaults 0.80 / 0.85 / 0.95). Above the background threshold it spawns `run_compaction`, which drains the oldest 30% (or 50% when aggressive) out of the shared `Arc<RwLock<Vec<Message>>>`, renders them as a transcript, runs a toolless one-turn agent on `prompts/en/compactor.md.j2`, and does `hist.insert(0, "[Compaction Summary]: …")`. Emergency truncation drops the oldest half synchronously with no LLM.

Four consequences, all of which get worse the longer a session runs:

- **Each rewrite compresses an ever-larger past.** The `[Compaction Summary]` head is itself part of the transcript handed to the next compaction (`render_messages_as_transcript` has no special case for it), so summary N+1 is a summary of summary N plus new turns. Detail decays geometrically and there is no floor. `prompts/en/compactor.md.j2` even instructs the model to discard "repeated information already covered in earlier summaries" — the loss is by design, because the alternative under one rolling summary is unbounded growth.
- **Context stays full until it is nearly full.** Nothing happens below 80%. The channel spends most of its life carrying raw turns it no longer needs, then does one large cut under pressure.
- **Nothing survives restart.** The summary lives only in the in-memory vector. On restart `main.rs:1311-1337` loads the last `history_backfill_count` rows from `conversation_messages` and injects them into the *system prompt* as `backfill_transcript` (`prompts/en/channel.md.j2:231`). Every summary ever produced is gone. `migrations/20260211000004_compaction.sql` is a tombstone — `compaction_summaries` and `conversation_archives` existed once and were dropped as redundant.
- **There is no navigable structure.** A channel that has run for three weeks has one paragraph about week one and no way to ask what happened on the Tuesday. `channel_recall` (`src/tools/channel_recall.rs`) can pull raw messages by timestamp, but it is branch-only and it has no map — the agent would have to guess at a window.

The compaction path also races the turn loop: `run_compaction` drains under the lock, releases it for the duration of an LLM call, then re-acquires it to `insert(0, …)`. `apply_history_after_turn` (`src/agent/channel_history.rs:47`) explicitly extends rather than replaces the guard to survive this. Under `prompt-stability.md`'s accounting, every one of those head inserts is a full history cache miss.

Note the pre-flight fork trimming in the same file — `precompact_forked_history`, used by branches and forked workers before their first call — is a different mechanism with different guarantees, and this design does not touch it.

---

## The Invariant

```text
conversation_messages, ordered by (created_at, id)
├──────────────┼──────────────┼──────────────┼─────────────────────┤
     cp #1          cp #2          cp #3        unsummarized tail
     ▲              ▲              ▲            ▲
     └── each covers exactly the span since the previous one:
         contiguous, non-overlapping, no gaps

     └──────── rollup L1 ───────┘
         level-0 rows kept, marked rolled_up_into

active context = system prompt { header + rollups + recent checkpoints }
               + in-memory history { raw turns since the last checkpoint }
```

1. **Coverage is total, contiguous, and non-overlapping.** Checkpoint N's start boundary *is* checkpoint N-1's end boundary, read in the same transaction that writes N. There is no span of the durable log that is covered twice, and none that is covered zero times once chronicling has started.
2. **A checkpoint summarizes raw messages only.** Prior summaries may be supplied to the summarizer as narrative context, but the output describes only the new interval. No checkpoint is ever regenerated from another checkpoint's text.
3. **Checkpoints are append-only and immutable.** Rollups add rows; they never delete or rewrite the rows they cover. A level-0 checkpoint's summary text, boundaries, and sequence are fixed at commit.
4. **Commit is idempotent under retry and restart.** Boundaries are derived inside the transaction and constrained by unique indexes, so a duplicate or late commit is rejected rather than written. A checkpoint that fails to commit leaves the span unsummarized — the next cut picks it up.
5. **The chronicle never blocks a turn.** Cut selection takes a read lock, the LLM call holds none, and the in-memory trim is a bounded write-lock section guarded by a generation check.

### Ordering key

`conversation_messages.created_at` defaults to `CURRENT_TIMESTAMP`, which SQLite resolves to whole seconds, and `id` is a random UUID. Neither is individually a total order. The chronicle uses `(created_at, id)` as *the* ordering key everywhere — cut selection, boundary comparison, and expansion all use the same `ORDER BY created_at, id`. Within a one-second burst the resulting order may not be true arrival order, but it is deterministic and identical across every consumer, which is what contiguity requires. Messages are never reordered after insert, so a boundary stays valid. Adding a monotonic sequence column was considered and rejected: the log's writers are fire-and-forget `tokio::spawn` tasks (`ConversationLogger`), so a per-channel counter would have to be serialized across them for no gain over a deterministic composite key, and implicit `rowid` is not VACUUM-stable on a `TEXT PRIMARY KEY` table.

---

## Decisions

### Coverage is anchored to the durable log, not the in-memory vector

The in-memory `Vec<Message>` is empty on restart and is mutated by the turn loop in ways the chronicle does not control. Boundaries anchored to it would not survive a restart and could not be expanded back into anything. So a checkpoint's identity is a range over `conversation_messages`, which is durable, append-only in practice, and already the source `channel_recall` and the portal timeline read from.

The in-memory trim is then a *derived* action, not the definition of the checkpoint. After a checkpoint commits, the channel drops the corresponding prefix of its live history. If that trim is skipped — because the generation changed, or the process restarted mid-flight — the checkpoint is still correct and the next context assembly still uses it. Durable state and live state can disagree temporarily without corrupting either.

### The chronicle view renders into the system prompt, not into history

Today's `insert(0, summary)` is a head rewrite: it changes the first bytes of the message array, which is a guaranteed full history cache miss and a mutation of already-sent bytes — exactly what [`durable-transcript.md`](durable-transcript.md) is trying to eliminate.

The chronicle head goes into the channel system prompt instead, as a new `session_chronicle` block in `prompts/en/channel.md.j2`, in the **volatile region** defined by [`prompt-stability.md`](prompt-stability.md) — the same region that already holds working memory, the memory bulletin, and `backfill_transcript`. It is recomputed from durable state on each turn, so restart reproduces it exactly with no rehydration machinery.

To be precise about the cache: trimming the live history is still a head mutation, and still invalidates the history prefix once per checkpoint. What the split buys is that the summary text never enters the message array at all, so a checkpoint costs one trim rather than a trim plus a growing synthetic head message that changes content every time. In chronicle mode the channel's history mutations reduce to append plus tail-preserving drain, which is closer to the append-only invariant than the current path, and the chronicle itself needs no rehydration because it is derived, not stored in the transcript.

### Interval, not pressure — with pressure kept as a floor

A cut fires when **either** `messages_since_last_checkpoint >= interval_messages` (default 40) **or** `tokens_since_last_checkpoint >= interval_tokens` (default 12% of the context window). Message count gives predictable narrative granularity in conversational channels; token growth catches the tool-heavy turn that consumes a third of the window in four messages. Either alone leaves an obvious hole.

The existing thresholds stay as a safety net rather than the primary trigger. Crossing `background_threshold` forces a cut immediately regardless of interval (`kind = 'pressure'`), and `emergency_threshold` still performs the synchronous no-LLM truncation — but records a `kind = 'emergency'` checkpoint marking the discarded span, so a gap in the chronicle is visible rather than silent. That is the one case where a checkpoint's summary does not describe its contents, and it says so.

### Rollups add a level; they never replace a row

When the level-0 count exceeds `rollup_threshold` (default 12), the oldest `rollup_batch` (default 8) are rolled into one level-1 row whose message range is the union of theirs. The level-0 rows stay, with `rolled_up_into` set to the rollup's id. Same mechanism at level 2 and above. Nothing is a summary-of-summary blob: every rollup names exactly which checkpoints it covers (`rolls_up_from_seq`/`rolls_up_to_seq`), each covered checkpoint names its rollup, and both directions are queryable and renderable in the UI. A rollup is a *view* over provenance that remains intact underneath it.

Rollup input is the constituent checkpoints' summaries, which is the one legitimate summary-of-summary — it is bounded (one level of recursion per rollup generation), explicit, and reversible by reading the level-0 rows.

### The expansion tool is scope- and capability-bound at construction

The chronicle tool takes its channel scope and its capability from its constructor, never from tool arguments:

```rust
pub struct ChronicleTool {
    store: ChronicleStore,
    conversation_logger: ConversationLogger,
    channel_id: ChannelId,
    capability: ChronicleCapability, // Metadata | Expand
}
```

Channels get `Metadata` — `list` and `open`, which return bounded, already-summarized text. Branches get `Expand` as well, which pulls raw transcript for a checkpoint's range through `ConversationLogger::load_channel_transcript` under the same 100-message cap `channel_recall` uses. This keeps the existing "don't dump raw results into channel context" rule intact while giving the channel the map it needs to decide a branch is worth spawning.

Because scope and capability are constructor-injected, a future `TurnAuthority` can hand a channel a narrower tool without any change to the tool's argument surface or to the prompt — the filtering point already exists. Cross-channel recall stays `channel_recall`'s job; this tool never reads another channel.

### Coexistence: `mode` on the existing compaction config, defaulting to rolling

```toml
[agent.compaction]
mode = "rolling"   # or "chronicle"
background_threshold = 0.80
```

`CompactionMode` is a `#[serde(rename_all = "snake_case")]` enum with `#[derive(Default)] Rolling`. `TomlCompactionConfig` gains `mode: Option<CompactionMode>`, so every existing config file deserializes unchanged and every existing threshold key works in both modes.

The default stays `Rolling`. Chronicle mode is the better architecture and this doc argues for it, but rolling is the path with production hours on it, and defaults should follow evidence rather than the persuasiveness of a design doc. The default flips when chronicle mode has soak time on real long-running channels, as a separate, deliberate change.

Cron and autonomy channels (`ChannelKind::Cron`, `ChannelKind::Autonomy`, `src/agent/channel.rs:342`) are short-lived by construction — they self-exit once work settles. They stay on rolling compaction regardless of mode; chronicling a channel that will not outlive its run is pure cost.

### Entering chronicle mode with legacy history

A channel switching to chronicle mode with no checkpoints gets a **bootstrap** checkpoint before its first interval cut. Its range is `[earliest logged message, now]`, and its summary is, in order of preference: the live `[Compaction Summary]` head if the rolling compactor left one in memory; otherwise an LLM summary of the last `history_backfill_count` messages; otherwise a placeholder stating that history prior to this point was not chronicled.

This makes coverage total from the first cut and stops the first real checkpoint from trying to summarize three weeks in one call. Legacy channels stay readable either way — `conversation_messages` is untouched by any of this, and a channel switched back to rolling mode simply stops consulting the checkpoint table while the rows remain.

---

## Storage

```sql
CREATE TABLE channel_chronicle_checkpoints (
    id                     TEXT PRIMARY KEY,
    channel_id             TEXT NOT NULL,
    seq                    INTEGER NOT NULL,
    level                  INTEGER NOT NULL DEFAULT 0,
    -- 'interval' | 'bootstrap' | 'pressure' | 'emergency' | 'rollup'
    kind                   TEXT NOT NULL,
    title                  TEXT NOT NULL,
    summary                TEXT NOT NULL,
    covers_from_at         TIMESTAMP NOT NULL,
    covers_to_at           TIMESTAMP NOT NULL,
    covers_from_message_id TEXT,
    covers_to_message_id   TEXT,
    message_count          INTEGER NOT NULL,
    token_estimate         INTEGER NOT NULL,
    -- Set on level-0 rows once a rollup covers them; the rows themselves stay.
    rolled_up_into         TEXT REFERENCES channel_chronicle_checkpoints(id),
    -- Set on level>0 rows: which checkpoint sequences this rollup covers.
    rolls_up_from_seq      INTEGER,
    rolls_up_to_seq        INTEGER,
    model                  TEXT,
    created_at             TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX idx_chronicle_seq ON channel_chronicle_checkpoints(channel_id, seq);
-- The overlap guard: two concurrent cuts cannot commit the same end boundary.
CREATE UNIQUE INDEX idx_chronicle_boundary
    ON channel_chronicle_checkpoints(channel_id, level, covers_to_message_id)
    WHERE covers_to_message_id IS NOT NULL;
CREATE INDEX idx_chronicle_window
    ON channel_chronicle_checkpoints(channel_id, level, covers_to_at);
```

New migration in `migrations/`, per the immutability rule in `AGENTS.md`. Store lives in `src/conversation/chronicle.rs` next to `history.rs`, following `ConversationLogger`'s shape — except commits are awaited, not fire-and-forget, because the boundary read and the insert must be one transaction.

### Commit protocol

```
BEGIN IMMEDIATE
  last  := SELECT * FROM checkpoints WHERE channel_id=? AND level=0 ORDER BY seq DESC LIMIT 1
  -- Reject a stale cut: the tail moved while the LLM was running.
  IF last.covers_to_message_id != cut.covers_from_message_id THEN ROLLBACK; report Superseded
  seq   := COALESCE(MAX(seq),0)+1
  INSERT ...
COMMIT
```

Both unique indexes are belt to the transaction's braces: the boundary check makes a stale commit a clean rejection, and the indexes make a racing one an error rather than a duplicate. A rejected cut is logged and dropped — its span stays unsummarized and the next cut, which reads the current boundary, covers it. No retry loop is needed for correctness.

### In-memory generation guard

`ChronicleState` holds a `generation: AtomicU64`, bumped on every structural head mutation of the live history (checkpoint trim, emergency truncation). A cut records the generation and the cut index when it snapshots; the post-commit trim re-acquires the write lock, re-reads the generation, and applies `history.drain(..cut_index)` only if it is unchanged. Otherwise the trim is skipped with a log line. This is the fix for the specific hazard the current compactor has: `run_compaction` drains *before* the LLM call and re-inserts *after*, so anything appended in between lands behind a summary that does not describe it.

---

## Context Assembly

Rendered per turn from durable state, into the volatile region of the system prompt:

```
render_chronicle_view(channel_id, now, budget_tokens):
  header  := session age, first/last activity, total logged messages,
             checkpoint count by level, coverage end, raw turns since it
  recent  := level-0 checkpoints with covers_to_at >= now - recent_window,
             capped at max_recent, oldest first
  older   := highest-level rollups covering everything before `recent`,
             plus any level-0 checkpoints older than the window that no
             rollup covers yet, oldest first
  body    := older ++ recent
  while estimate_text_tokens(header ++ body) > budget_tokens and body.len() > 1:
      collapse the oldest `body` entry to its title + range line
  return header ++ body
```

Defaults: `recent_window` 24h, `max_recent` 8, `context_token_budget` 2000 — the same order of magnitude as `WorkingMemoryConfig::context_token_budget` (1500). The header never collapses; it is what tells the agent that more exists and that the chronicle tool can reach it. Degradation is graceful and monotone: the oldest entries lose their bodies first, and a title plus a date range is still a usable index entry.

The live history in chronicle mode holds only messages after the last checkpoint's boundary, so steady-state context is roughly `interval_tokens` of raw turns plus a bounded chronicle view — substantially below the 80% the rolling mode idles at, continuously, rather than sawtoothing between 80% and 50%.

---

## Surfaces

**Tool.** `chronicle`, `src/tools/chronicle.rs`, actions `list` / `open` / `expand`. Registered into the branch tool server (`create_branch_tool_server`) with `Expand`, and into the channel tool server (`add_channel_tools`) with `Metadata`. Prompt text in `prompts/en/tools/chronicle.md`, following the existing `crate::prompts::text::get("tools/…")` convention.

**API and timeline.** `TimelineItem` (`src/conversation/history.rs:264`) gains a `Checkpoint` variant, and `load_channel_timeline`'s `UNION ALL` gains a fourth arm over the checkpoint table. It flows to the portal unchanged: `GET /channels/messages` → OpenAPI → `interface/src/api/schema.d.ts` via `just typegen` → the `TimelineEntry` switch in `ChannelDetail.tsx:424` and `PortalTimeline.tsx`. Checkpoints render as their own entry kind — neither user nor assistant — showing kind, covered range, message count, summary, and a rollup badge where `rolled_up_into` is set, with the body expandable.

**Live.** `ProcessEvent::ChronicleCheckpoint` on commit, bridged to `ApiEvent::ChronicleCheckpoint` (`src/api/state.rs:363`) and emitted as SSE `chronicle_checkpoint` (`src/api/system.rs:151`), pushed into the timeline by `useChannelLiveState`. Existing SSE behaviour is untouched — this is one more event type on the same stream, and a client that ignores it sees the checkpoint on the next history load.

---

## Reflection Notices

The memory persistence branch (`spawn_memory_persistence_branch`, `src/agent/channel_dispatch.rs:219`) is the one process that changes the agent itself. It writes memories, and when the reflection signal is set it also patches skills under agent-origin rails. It completes silently — `src/agent/channel.rs:3551` drops the result deliberately — so the only trace a user ever sees is the ephemeral status label `persisting memories and reflecting on skills...` and a `BranchRun` row in the portal timeline. The agent rewrites its own standing procedures and says nothing about it.

The tempting fix is to reconstruct a summary after the branch exits: walk its successful `memory_save` and `skill_manage` results and join their messages. That yields `Patched SKILL.md in skill 'git-worktree-maintenance' (1 replacement).` — the what, and only ever the what, because a tool receipt acknowledges a write; it does not know why the write happened. The reason lives in the branch's context, in the correction the user made three turns ago and the worker transcript where the retry pattern showed up, and that context is gone the moment the branch exits. Post-hoc reconstruction cannot recover an intent that was never recorded.

So the notice is authored by the branch as part of its terminal contract, not derived from it afterwards.

### The contract carries the note

`memory_persistence_complete` (`src/tools/memory_persistence_complete.rs`) already ends every pass with a validated self-report: `saved_memory_ids` must match, byte for byte, the IDs recorded by successful `memory_save` calls in the same run. Notes extend that contract rather than adding a second reporting channel.

```rust
pub struct ReflectionNote {
    /// Remembered | Revised | SkillPatched | SkillCreated | ReferenceAdded
    kind: NoteKind,
    /// One sentence, plain English, addressed to the user: what is now true
    /// that was not true before this pass.
    what: String,
    /// One sentence: what happened in this session that caused the write.
    why: String,
    /// Memory IDs and skill names this note accounts for.
    refs: Vec<NoteRef>,
}
```

### Receipts make a note falsifiable

Every `refs` entry must resolve to a receipt recorded during the same run, or the terminal call is rejected the way a fabricated `saved_memory_ids` is today. Memory receipts already exist — `MemorySaveTool` records into `MemoryPersistenceContractState`. Skill writes have none: `SkillManageTool` (`src/tools/skill_manage.rs:41`) never touches the contract state, so nothing in the system can currently confirm that a claimed skill change happened. It gains an optional contract handle on the same pattern as `memory_save`, recording `(action, skill name, file path)` per successful write.

The check runs in both directions. A note whose refs have no receipt is a fabrication and fails the call. A receipt with no note covering it is a silent self-modification and fails it too — the pass does not get to change the agent without saying so. `no_memories` with a non-empty note list is likewise rejected, matching the existing outcome rules.

Salience is derived, never model-supplied, so the branch cannot promote its own work: a note is `Prominent` when its kind is a skill write, or when the same pass extracted a `user_correction` event; `Routine` otherwise.

### Storage

```sql
CREATE TABLE channel_reflection_notes (
    id           TEXT PRIMARY KEY,
    channel_id   TEXT NOT NULL,
    branch_id    TEXT NOT NULL,
    -- 'remembered' | 'revised' | 'skill_patched' | 'skill_created' | 'reference_added'
    kind         TEXT NOT NULL,
    what         TEXT NOT NULL,
    why          TEXT NOT NULL,
    -- Receipt-validated memory IDs and skill names, as JSON.
    refs         TEXT NOT NULL,
    -- 'prominent' | 'routine', derived at commit.
    salience     TEXT NOT NULL,
    created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_reflection_notes_channel
    ON channel_reflection_notes(channel_id, created_at);
```

Append-only and immutable, like checkpoints, and ordered on the same `(created_at, id)` key — which is what lets a note fall inside exactly one checkpoint span with no extra machinery.

### Folding into the chronicle

Notes are the second thing worth remembering about a span. The checkpoint summarizer takes the notes committed inside its range as an input alongside the raw transcript, and the checkpoint gains a *what changed about me* section listing them verbatim — they are already one sentence each and already plain English, so they are quoted, not re-summarized. Rollups carry the same section, concatenated from their constituents under the rollup's budget.

The chronicle view header gains one line: `4 lessons since Aug 2`. That line is what makes a chat notice optional rather than load-bearing — the record is in the chronicle whether or not anything was said at the time, and it survives restart, which a chat message the user scrolled past does not.

### Delivery

Two tiers, because the durable record and the interruption have different thresholds.

**Always durable.** Commit → `TimelineItem::ReflectionNote` (a fifth `UNION ALL` arm in `load_channel_timeline`) → `ProcessEvent::ReflectionNote` → `ApiEvent::ReflectionNote` → SSE `reflection_note`. The portal renders it as its own entry kind: `what` prominent, `why` muted beneath it, refs as chips that open the memory or the skill. Nothing is gated here; every note lands.

**Chat, gated.** Controlled by `notice` on the reflection config, default `salient`, which sends only `Prominent` notes. Group channels default to off entirely — announcing "you'd rather I ask before force-pushing" to twelve people is noise about a private preference — and opt in with `notice_in_groups`. Per-channel override rides `resolved_settings` the same way `persistence_enabled` does (`src/agent/channel.rs:4053`). Before sending, a note whose normalized `what` matches one from the channel's last 20 notes is suppressed, so a lesson the agent keeps re-learning is not announced each time.

The chat notice goes out through `send_routed` as an ordinary outbound text and is **never inserted into channel history**. It is not a turn, and a head insert is the mutation [`prompt-stability.md`](prompt-stability.md) exists to eliminate — the same reasoning that puts the chronicle head in the system prompt.

One hazard specific to this path: `send_routed` targets `current_inbound`, which may be absent or stale by the time an async persistence branch finishes, and falls back to `InboundMessage::empty()` with a warning. The routing target is therefore captured at spawn time, alongside the branch id, in the map that today is `memory_persistence_branches: HashSet<BranchId>` — the same capture `BranchStarted` already does for `reply_to_message_id`. A note whose target cannot be resolved is still committed and still reaches the timeline; only the chat send is skipped.

### Wording

Rules live in a `## Reporting what you learned` section of `prompts/en/memory_persistence.md.j2`: address the user directly, no tool names, no IDs, no file paths in the `what`; the `why` cites something that happened in this session and never restates the `what`; one note per lesson, not one per tool call. And the load-bearing one — if you cannot write the `why`, the write was probably not worth making.

| Rejected | Written |
|---|---|
| `Memory updated` | You keep worktrees under `~/orca/workspaces`, one per branch. |
| `Patched SKILL.md in skill 'git-worktree-maintenance' (1 replacement).` | Skill `git-worktree-maintenance`: confirm a PR is merged before removing its worktree. |
| Why: this is important to remember. | Why: a cleanup run deleted a worktree whose PR was still open — the old steps only checked local branch state. |
| Why: because you asked me to patch the skill. | Why: you stopped me twice today, both times just before a force-push to a shared branch. |

Rendered, single note:

```
💾 Learned: you want PR descriptions to skip the testing section when the diff
   has no tests — you cut that section from #628 and told me not to add it back.
```

Rendered, a pass that produced two:

```
💾 Learned 2 things
• Skill `git-worktree-maintenance`: confirm a PR is merged before removing its worktree.
  ↳ A cleanup run deleted a worktree whose PR was still open — the old steps only
    checked local branch state, which reads "gone" for a branch that was never merged.
• Published-PR cleanup now handles drafts explicitly.
  ↳ Two drafts got swept as stale this week; the procedure didn't separate
    "draft" from "abandoned".
```

The `no_memories` outcome never speaks in chat. Its `reason` is already required by the contract and renders as a dim timeline row — a background process reporting that it did nothing is pure noise.

### Config

```toml
[skills.reflection]
enabled = true
min_tool_iterations = 10
cooldown_secs = 3600
notice = "salient"        # "off" | "salient" | "all"
notice_in_groups = false
```

`ReflectionConfig` (`src/config/types.rs:810`) and `TomlReflectionConfig` gain both keys as `Option`, so existing configs deserialize unchanged.

---

## Testing

Unit, against the store and the assembly functions:

- Boundary contiguity across a sequence of cuts; no gap, no overlap, under the `(created_at, id)` ordering including a same-second burst.
- Stale-cut rejection: commit with a `covers_from` that no longer matches the tail is refused and leaves the table unchanged.
- Idempotency: the same cut committed twice yields one row.
- Rollup preserves provenance: covered rows survive, `rolled_up_into` and `rolls_up_*_seq` agree in both directions, and coverage of the rollup equals the union of its children.
- Context selection: budget pressure collapses oldest-first, never drops the header, and is monotone in the budget.
- Generation guard: a trim whose generation moved is skipped and leaves history intact.
- Bootstrap seeding from each of the three sources, and coverage totality afterwards.
- `CompactionConfig` deserializes from a `mode`-less TOML to `Rolling` with thresholds intact.

Reflection notices, against the contract and the delivery gate:

- A note referencing a memory ID or skill name with no receipt from this run is rejected.
- A receipt with no note covering it is rejected; `no_memories` with notes attached is rejected.
- Salience derivation: skill writes and passes carrying a `user_correction` event come out `Prominent`, everything else `Routine`.
- The delivery gate — `off` sends nothing, `salient` sends only `Prominent`, groups stay silent unless `notice_in_groups`, and a repeated `what` is suppressed against the last 20 notes.
- A note whose routing target cannot be resolved still commits and still reaches the timeline.
- `ReflectionConfig` deserializes from a TOML without `notice`/`notice_in_groups`.

Integration:

- Restart recovery — checkpoints commit, process drops the in-memory history, and the reassembled view matches what was rendered before.
- Tool authorization surface — a `Metadata` tool refuses `expand`, and every action is confined to its constructed channel scope.
- Timeline contract — a committed checkpoint appears in `load_channel_timeline` in chronological position with the right shape.
- Notes committed inside a checkpoint's span appear in that checkpoint's *what changed about me* section and in no other's.
- A chat notice is delivered without appearing anywhere in the channel's history.

---

## Phases

1. **Durable spine.** Migration, `ChronicleStore`, commit protocol with both unique indexes, boundary and idempotency tests. Nothing reads it yet.
2. **Lifecycle.** `CompactionMode` config, cut triggers, the summarizer prompt (`prompts/en/chronicle_checkpoint.md.j2`), the generation-guarded trim, bootstrap seeding, and the `pressure`/`emergency` paths. Chronicle mode is selectable and correct; the view is not yet assembled.
3. **Context assembly.** `session_chronicle` block in `channel.md.j2` (volatile region), the selection and budgeting algorithm, and history trimming to the checkpoint boundary. This is the phase that actually frees context.
4. **Inspection.** The `chronicle` tool with both capabilities, prompt text, registration in the branch and channel tool servers.
5. **Visibility.** `TimelineItem::Checkpoint`, the timeline union arm, typegen, the `ProcessEvent`/`ApiEvent`/SSE path, and the portal renderer.
6. **Rollups.** Level-1+ generation, `rolled_up_into` backfill on commit, rollup rendering in the view and the UI, and the rollup badge in the timeline.
7. **Reflection contract.** Skill write receipts in `MemoryPersistenceContractState`, the `notes` field on `memory_persistence_complete` with both-directions validation, derived salience, the `channel_reflection_notes` migration and store, and the prompt section. Notes commit; nothing surfaces them yet.
8. **Reflection delivery.** `TimelineItem::ReflectionNote` and its union arm, the `ProcessEvent`/`ApiEvent`/SSE path, the portal renderer, the spawn-time routing target, the `notice` gate with group and dedupe rules, and folding notes into the checkpoint summarizer.

Phases 1-5 are the end-to-end vertical slice: durable schema, lifecycle, assembly, inspection, and visible UI. Phase 6 is additive — until it lands, the view carries the full level-0 list under its token budget, which degrades by collapsing oldest entries to titles rather than by losing them.

### Status

Phases 1-5 are implemented and on by config (`compaction.mode = "chronicle"`); the default stays `rolling`. Phases 6-8 are not built. Two consequences of 6 being absent are worth stating plainly rather than discovering later:

- **No rollups exist yet**, so `level` is always 0 and `rolled_up_into` is always null. The schema, the `list_before_seq` selection, the `Rollup` label in both renderers, and the "rolled up" badge are all in place and inert; they light up when phase 6 writes level-1 rows. Until then a very long session's older checkpoints degrade by collapsing to titles under the token budget, which keeps them navigable but not summarized in-context.
- **`max_older` bounds the older list.** Checkpoints beyond it are absent from the prompt view entirely — reachable through the `chronicle` tool, which reads the table directly, but not rendered. That is the gap rollups close.

Phases 7-8 are independent of 1-6 and of each other in the same way: phase 7 stands alone against today's rolling compactor, since the notes table and the contract have no dependency on checkpoints, and only the summarizer fold in phase 8 requires the chronicle to exist.
