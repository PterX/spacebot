# Prompt & Reliability Audit — 2026-08-12

Findings from the live instance: the rendered telegram channel prompt (47,926
chars, pulled through `/api/channels/prompt/inspect`), today's logs, worker
failure transcripts, and the autonomy run table. Four problem areas, each with
evidence and a fix direction. The phases at the end govern the next commits.

Related: [system-prompt-rework.md](system-prompt-rework.md),
[production-worker-failures.md](production-worker-failures.md),
[worker-reliability.md](worker-reliability.md),
[autonomous-action-audit.md](autonomous-action-audit.md).

## 1. Workers still fail — showstopper

140 worker LLM failures on 2026-08-12, all between 06:57 and 07:07, all during
the TUI research session. They resolve into exactly two bugs, and both violate
the standing principle that a worker's terminal states are *success* or a
*graceful partial-result report* — never an error string relayed to the user.

### 1a. Worker compaction orphans tool results, then retry poisons itself

66 failures of the form:

> OpenAI ChatGPT Responses API error (400 Bad Request): No tool call found for
> function call output with call_id call_OmoWbiocGorPV2iLnNaZONPI.

The failure transcript for worker `039643a1` (persisted at
`data/logs/worker_039643a1*.log`) shows the mechanism directly:

```
[0] User:  [System: Earlier work has been summarized to free up context. 18 messages compacted.]
[1] User:  Tool Result (id: call_OmoWbiocGorPV2iLnNaZONPI)   ← the call_id from the 400
           Tool Result (id: call_zTuwqBw9ggHuoE4TDSOfxyOA)
           Tool Result (id: call_f9ZXYlKI5JXJ4NCNDLA4pJmg)
           Tool Result (id: call_uNjWTVxuoMmFh3s6B2sCqbvx)
[2] Assistant: <next tool calls — the pairing is broken from here on>
```

`maybe_compact_history` cut the assistant message that held the tool calls but
kept the user message holding their results. Every subsequent completion
request replays the orphaned results and is rejected. The retry loop then
resent the identical broken history at ~15 attempts/minute for four minutes —
the request is deterministically invalid, so no retry could ever succeed.

The retry path already knows how to dedup-strip stale tool results, but only
triggers that on context-overflow errors, not on this 400.

**Fix:**
- Compaction boundaries must treat a tool-call message and its result
  message(s) as one atomic unit. A cut that would separate them moves to the
  nearest safe boundary. This applies to `maybe_compact_history`,
  `force_compact_history`, and `precompact_forked_history`.
- Validate history invariants after every cut: any tool result whose call is
  no longer present gets folded into a plain-text note ("earlier tool output:
  …") rather than left as an orphaned protocol item.
- The tool-mismatch 400 joins context overflow as a *recoverable* error class:
  strip orphans and retry once, don't replay verbatim.

### 1b. Context overflow retries that cannot converge

74 failures: *"Your input exceeds the context window of this model."* The
research workers accumulated large tool outputs (full-file `nl -ba` dumps,
recursive `find` listings), overflow recovery force-compacted, and the result
still exceeded the window — then the loop retried an impossible request until
exhaustion. The user-visible outcome was a raw provider error in telegram
(seq 39, seq 41), twice, for a task that had already produced substantial
findings.

**Fix:**
- When overflow recovery exhausts its retries, the worker must return what it
  has: synthesize a partial report from the transcript (the persisted failure
  log proves the material exists) with an explicit "incomplete — ran out of
  context at step N" marker. Failure text alone is never a worker's output.
- Cap tool output at the source. `production-worker-failures.md` documented
  this a while ago and it remains undone: shell/file results need byte caps
  with head/tail elision before they enter history, not after they've blown
  the window.
- Force-compaction that doesn't shrink the estimate below the window on the
  first pass should escalate (drop oldest segments entirely) instead of
  re-running the same summarize step.

## 2. Solo channels get a group-chat prompt

The telegram DM prompt says, verbatim: *"Multiple users may be present. Each
message is prefixed with [username]."* Eleven lines earlier the identity
section says *"The person you work for is the only user you will ever have."*
Both are in every DM turn — a direct self-contradiction, and the model has to
guess which to believe.

The signal to fix this already exists and is dropped on the floor: the
telegram adapter records `telegram_chat_type = "private" | "group" |
"supergroup" | "channel"` in message metadata (`src/messaging/telegram.rs`).
Discord and Slack know DM-vs-guild equivalents. Prompt assembly never reads
any of it.

**Fix: a `ConversationMode` on the channel — `Solo` or `Group` — derived by
the adapter, not configured by hand.** Telegram private chats, Discord DMs,
Slack DMs, and portal chats are `Solo`; everything else `Group`. An explicit
per-channel override can exist in config for odd cases, but the default is
adapter-derived. Sections that branch on it:

| Section | Group (today's text) | Solo |
|---|---|---|
| Conversation Context | "Multiple users may be present…" | "This is a private conversation with {name}." |
| When To Stay Silent | ~1,500 chars of read-the-room guidance: banter, someone-else-answered, directed-at-another-human | A short paragraph: skip reactions/acks and media shared without a request; everything else in a DM is addressed to you |
| Authority | "Not everyone talking to you has the same authority…" | Collapses to the injected-content rule only — the one person present holds full authority |
| Participants | Roster with per-human anchors | Omitted entirely (the human's profile is already in Organization) |
| Message prefixes | `[username]` on every message | None |

The group text stays as-is for group channels — this is extraction, not
rewrite. Most of the ~2,500 chars this removes from DM prompts is text that
actively teaches wrong behavior there (a "when in doubt, skip" bias is
correct in a busy group and wrong in a DM where every message is addressed to
the agent).

## 3. The task system is dormant

An evening of multi-worker research produced zero tasks. The task board
currently surfaces in the channel prompt as one passive sentence ("Branch to
manage the task board") plus "Active Tasks — No active tasks" in the memory
store — nothing that would ever cause the model to reach for it. Meanwhile
the autonomy loop sat in its empty-instance deadlock (tasks: 0, goals: 0 →
"do not invent tasks" → tasks stay 0) and its investigations left no record
the next run could consult, which is half of why §4 happens at all.

**Direction — tasks as the spine of autonomous work:**
- Any autonomy run that starts a non-trivial investigation records it as a
  task first and closes it with an outcome comment. "Did we already look at
  VBX?" becomes a board query instead of a prompt-window guess. This is the
  durable fix for repeated investigations; §4's cap raise is the cheap one.
- Channel guidance grows the missing half: when work spans more than one
  exchange — a request parked for later, a multi-step job, anything Jamie
  says he wants done "at some point" — the model files a task. The board is
  shared state between conversation, autonomy, and workers, not an autonomy
  ledger.
- The prompt shows the board's live shape (open/in-progress counts and the
  top few titles) even when empty, so the tool stays visible. One line, not a
  dashboard.

## 4. Autonomy re-investigates because its memory is 75 minutes long

`run_history_count` defaults to **5** (`src/config/types.rs`). At the
configured 15-minute interval that is a 75-minute lookback. Observed today:

| Target | First run | Repeat | Gap |
|---|---|---|---|
| Discover.me | 01:30 | 05:08 | 3h38m |
| VBX | 03:52 | 05:23 | 1h31m |

Both repeats fell just outside the 5-run window; the prompt's own instruction
("Do not repeat work a recent run already did") was unfollowable because the
evidence had scrolled away.

**Fix: budget by characters, not run count.** Newest-first, include full
summaries until a `run_history_chars` budget (default 5,000) is spent, then
one-line truncated entries while they fit, hard cap 100 runs. A "no activity"
run consumes ~200 chars, so quiet nights keep a day-plus of memory; busy
periods still get the most recent runs in full. `run_history_count` is
replaced, not layered — one knob.

Worth stating: this is the mitigation. The reason a 75-minute memory *hurts*
is that investigations live nowhere else — §3 is the actual fix.

## 5. Rendered prompt audit

Read of the full 47,926-char live DM prompt. Section sizes measured; the
skills index (17.2k, 36%) is deliberate and excluded from criticism — the
category grouping reads well. The authored identity/disposition sections and
the shipped contract sections (Operating Contract, Execution, Communication)
are in good shape. The assembled context half is where quality drops.

### 5a. HUMAN.md renders twice, truncated differently each time

The same profile appears under Organization (`org_context.md.j2`, cut at a
section boundary, 2,256 of 4,000 chars) and again under Participants
(`src/memory/working.rs`, line-budget trim that cuts **mid-sentence**:
*"Designer-engineer — "rust, ai and nice"*). ~2,900 chars total, neither copy
complete. One home: Organization keeps the profile; Participants omits any
human whose profile already rendered — which in solo mode (§2) removes the
section entirely.

### 5b. Bookkeeping leaks into prose

`[56.0% — 2256/4000 chars]` renders inside the org context block. The model
doesn't need render-time accounting, and it reads like a debug build. Same
family: the Capabilities section renders its meta-instruction ("A capability
not listed here is not available.") and then *no capabilities*, because the
one capability line renders elsewhere. Empty section, keep the header out.

### 5c. The status block injects unsanitized delegate markdown

Two defects in `src/agent/status.rs` (Recently Completed):

- `- [{type}] {description}: {summary}` caps the summary at 500 chars but not
  the **description** — spawn task texts render at full length, and today's
  prompt carried two ~1,500-char task prompts as *list item labels*.
- Result summaries embed raw markdown: a worker result containing
  `## Checkout and revision` became a top-level section of the system prompt
  (1,829 chars between Recently Completed and Session Chronicle that no
  template put there). Delegate output must be demoted before injection —
  indent it, fence it, or strip heading markers.

Cap descriptions (~150 chars — the model can recover the rest from the
worker's own record) and neutralize headings in summaries.

### 5d. Internal plumbing leaks through Channel Activity

Channel previews render raw system rows: *"last: system: Branch completed:
Two additional high-confidence gaps…"* and — worse — *"last: **user**: Branch
completed: Branch failed: CompletionError: ProviderError…"*. A branch failure
attributed to a human. Previews should come from the last *conversational*
message, skipping system/process rows, and the `user`-attribution of injected
branch results is a labeling bug worth fixing at the source.

### 5e. Working memory event cap is too aggressive and double-prefixes

*"Worker completed: Worker completed: ## Checkout and state"* — the prefix is
applied twice. And *"(2 of 12 events shown)"* hides ten same-session events
with no route to them. Same shape as §4: raise to a char budget rather than a
count, and fix the prefix stutter.

### 5f. Available Channels is noise at DM scale

Ten entries: five `portal:chat` UUID variants, two internal test channels
(`orion-full`, `prompt-arch-snapshot-b`), and a `link:main:admin` that has
never carried a message. Collapse portal sessions to one line, hide
never-used link channels, and this section drops from 833 chars to ~200.

### 5g. Small repetitions and defects

- The no-flattery rule appears three times (Disposition, HUMAN.md §how-to-
  work-with-him — which itself renders twice per §5a).
- Delegation guidance overlaps across Execution, Worker Types, and the skills
  preamble; one pass to make each section own a distinct claim.
- Grammar in assembled text: "1 checkpoints", "1 messages since".
- A worker failure rendered verbatim in Recently Completed, provider request
  ID and help-center boilerplate included.
- Runs of 3–4 blank lines between conditional template blocks.
- Stale worktrees in Active Projects (`/private/tmp/voicebox-lang-798`,
  `.claude/worktrees/agent-*`) — data-driven, but a `still exists on disk`
  filter would keep the section honest.

## Phases

1. **Worker reliability (§1).** Atomic tool-pair boundaries in all three
   compaction paths, post-cut invariant validation, tool-mismatch 400 as a
   recoverable class, partial-result synthesis on overflow exhaustion, tool
   output caps. This is the showstopper; nothing else matters if delegated
   work dies.
2. **Status block hygiene (§5c, §5d, §5g).** Small, self-contained, and
   removes the ugliest artifacts from every future prompt.
3. **Solo conversation mode (§2 + §5a Participants).** Adapter-derived
   `ConversationMode`, section branching, contradiction removed.
4. **Autonomy run history budget (§4).** `run_history_chars`, fit-as-many,
   cap 100.
5. **Task adoption (§3).** Autonomy task-governance plus channel guidance.
   Also the exit from the empty-instance deadlock: with investigations
   recorded as tasks, the instance is no longer "empty" the moment the loop
   does anything.
6. **Prompt polish sweep (§5a, §5b, §5e, §5f).** Single-home HUMAN.md,
   bookkeeping markers out, working-memory char budget, channel list
   collapse, blank-line and pluralization cleanup.
