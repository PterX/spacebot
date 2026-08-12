# External Sessions: chronicling work done in other AI tools

A person who uses spacebot almost certainly uses other AI tools too — coding
agents with CLIs, each keeping its own session logs on disk. Today that work is
invisible to the agent: the user finishes a session in one tool, then manually
briefs another. This doc designs a continuous pipeline that watches those
session stores, runs each finished session through the compactor model, and
lands the result in session chronicles — so the agent is aware of everything
the user has been doing, across every tool, without being told.

This supersedes the transcript-import half of
[import-tool.md](import-tool.md). That design moved a foreign corpus in
verbatim — sessions became channels, memories were batch-written, the daemon
stopped for the run. The migration story is now three existing mechanisms
instead of a staged CLI: session logs are chronicled through this pipeline
(the generic adapter covers any harness), identity and memory files travel
through the ingest route or plain file copies, and credentials re-enter by
hand as they always half-had to. Nothing here is verbatim and nothing here is
migration — the source stays where it is, and what crosses over is awareness,
not data.

## Why chronicles, not memories

Memories are the curated layer: refined, connected, janitored, decaying by
class. Thousands of imported sessions would poison it. Chronicles are the
right sink because they are already the "what happened, when" layer — cheap to
append, searchable by vector ([`src/memory/lance.rs`](../../src/memory/lance.rs)
`ChronicleEmbeddingTable`), expandable on demand by the agent
([`src/tools/chronicle.rs`](../../src/tools/chronicle.rs)), and regenerable
from SQLite if the embedding store is lost. External sessions extend the
chronicle concept from "what happened in this channel" to "what happened in
the user's working life."

Memories still get written — but by the agent, later, when a chronicle hit
surfaces something worth refining. The pipeline itself never writes memories.

## The model economics

This is the first workload where the swappable compactor model
(`RoutingConfig.compactor`, resolved via `ProcessType::Compactor` —
[`src/llm/routing.rs:84`](../../src/llm/routing.rs)) genuinely earns its
independence. Session summarization is high-volume, latency-insensitive, and
failure-tolerant: a bad summary can be regenerated at any time because the
source file still exists and every chronicle row records which `model` wrote
it. That is exactly the profile where a small — or local — model is the right
choice. The existing ingestion loop resolves `ProcessType::Branch`
([`src/agent/ingestion.rs:478`](../../src/agent/ingestion.rs)) because it
makes judgment calls about what becomes a memory; this pipeline makes no such
calls and routes to the compactor slot.

A note on the idle invariant: an idle instance still makes zero LLM calls.
The watcher polling a directory and finding nothing new costs nothing. LLM
work happens only when a session file appears or grows — which is user
activity, just activity that happened in another tool.

## Source adapters

A `SessionImporter` trait with three responsibilities: detect a source layout,
enumerate sessions with their mtimes, and normalize one session into a common
shape. Built-in adapters ship for tools we name the way we name messaging
platforms — Claude Code (`~/.claude/projects/*/*.jsonl`) and OpenCode first,
others as demand appears. Alongside them, a **generic adapter**: the user
points it at any directory of session files (JSONL, Markdown, plain text) and
labels it. The generic adapter makes no structural assumptions at all — each
file is a session, its text is the transcript. Any harness we don't ship an
adapter for is covered by configuration, and product surfaces stay neutral.

### Normalization is deterministic, and it is the privacy boundary

A raw coding-agent session is mostly tool output — file dumps, build logs,
diffs. Fed raw to a small model, 95% of the tokens buy a summary of noise.
Each adapter runs a deterministic pre-pass producing:

```rust
pub struct NormalizedSession {
    pub source: String,          // adapter instance name
    pub session_id: String,      // source-native id, or filename for generic
    pub path: PathBuf,           // the raw file — the re-chronicle pointer
    pub project: Option<String>, // cwd / project dir when the source records it
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub entries: Vec<NormalizedEntry>,
}

pub enum NormalizedEntry {
    User(String),
    Assistant(String),
    ToolUse { name: String, digest: String }, // one line: name + truncated args/result
}
```

Tool results are truncated to `tool_result_cap_chars` (default 300) in the
digest. User and assistant text pass through whole. The scrubber's
secret-pattern matching (the same patterns the import tool's scan reuses) runs
over the normalized text before anything reaches an LLM — session logs contain
env dumps and tokens in tool output, and truncation alone catching most of it
is not a property, it's an accident. Scrubbing here makes it a property.

Sources are read-only, always. Unlike the ingestion loop, nothing is ever
deleted or moved after processing — these are another tool's files.

## Storage

Two per-agent tables, following the checkpoint schema's conventions
([`migrations/20260809000004_session_chronicles.sql`](../../migrations/20260809000004_session_chronicles.sql)):

- `external_sessions` — the registry. One row per (source, session_id):
  `id` (TEXT PK, hash of source + session_id), `source`, `session_id`, `path`,
  `project`, `title`, `started_at`, `ended_at`, `entry_count`,
  `content_hash`, `bytes_processed`, `status`
  (`pending | chronicled | skipped_backfill | failed`), `model`,
  `chronicled_at`.
- `external_session_chronicles` — the summaries. Long sessions are chunked,
  so the checkpoint vocabulary carries over: `level` 0 rows are chunk
  summaries, the `level` 1 row is the session rollup. Columns: `id` (TEXT PK),
  `session_pk` (FK), `level`, `part`, `title`, `summary`, `covers_from_at`,
  `covers_to_at`, `token_estimate`, `model`, `created_at`; unique
  `(session_pk, level, part)`.

`content_hash` + `bytes_processed` make re-chronicling incremental: when a
session file grows (the user resumed it), only the appended portion is
summarized as a new part and the rollup is refreshed — the session updates its
chronicle instead of duplicating it.

## The pipeline

A per-agent background loop following the ingestion loop's shape exactly
(spawn in [`src/main.rs:3301`](../../src/main.rs) beside
`spawn_ingestion_loop`, and in
[`src/api/agents.rs:1153`](../../src/api/agents.rs) for dynamically created
agents; config hot-reloaded through an `ArcSwap` slot in `RuntimeConfig`).

Each pass:

1. **Scan.** Each enabled adapter enumerates sessions. New or grown files are
   upserted into the registry as `pending`.
2. **Idle gate.** A session is eligible only when its file's mtime is older
   than `idle_minutes` (default 15). Live sessions grow — chronicling one
   mid-flight produces a summary that is stale in an hour and a duplicate
   risk. The registry key makes the eventual re-chronicle cheap, but not
   racing the user's active session is still the right default.
3. **Normalize + scrub** (deterministic, above).
4. **Summarize.** Rendered entries are chunked by
   `estimate_text_tokens` ([`src/agent/compactor.rs:430`](../../src/agent/compactor.rs))
   to fit the compactor model comfortably; each chunk goes through a toolless
   single-turn agent mirroring `Chronicler::summarize`
   ([`src/agent/chronicle.rs:679`](../../src/agent/chronicle.rs)) — new static
   prompt `external_session_chronicle`, `TITLE:` first-line contract, same
   fallback to a mechanical title on parse failure. Multi-chunk sessions get
   a rollup pass over their part summaries.
5. **Commit + embed.** Rows written to SQLite, then embedded into LanceDB the
   same non-blocking way checkpoint embeddings work
   (`embed_chronicle_checkpoint`,
   [`src/memory/search.rs:101`](../../src/memory/search.rs)); embedding
   failures log and are repaired by the boot backfill.

Failures mark the registry row `failed` with the error; the loop retries with
backoff and moves on — one unparseable session never wedges the pipeline.

## Backfill

A user enabling this for the first time may have thousands of sessions. The
initial scan registers all of them but chronicles newest-first up to
`backfill_sessions` (default 50); the rest are marked `skipped_backfill` —
visible, counted, deliberately not processed. Going deeper is agent-initiated:
a `sessions` tool with `list | open | backfill` actions, where `backfill`
takes an optional source, date range, and limit, and works by flipping
registry rows back to `pending` for the loop to drain. The tool never does LLM
work inline — "chronicle my OpenCode sessions from July" is a sentence, the
loop is the machinery, and there is no UI knob.

`open` returns the stored summary plus provenance; the raw `path` is on the
registry row for the agent to read directly through normal file tools when a
summary isn't enough.

## Retrieval

Three surfaces, in order of importance:

1. **Recall.** `search_with_chronicle`
   ([`src/memory/search.rs:153`](../../src/memory/search.rs)) gains external
   hits. The Lance chronicle table adds `origin` (channel vs. adapter name)
   and `session_ref` columns — schema change handled by the table's existing
   drop-and-recreate-then-backfill posture, which is safe precisely because
   both checkpoint and session chronicles are regenerable from SQLite.
   `ChronicleHit` carries the new fields, and `memory_recall`'s formatting
   renders external hits with full provenance: source, session id, project,
   and the covered time range. The answer to "what do you know about my active
   projects?" should cite *which tool, which session, when*.
2. **Ambient digest.** A deterministic `external_activity` prompt fragment in
   the style of `render_chronicle_view`
   ([`src/agent/chronicle.rs:980`](../../src/agent/chronicle.rs)):
   recomputed from SQLite every turn, listing recent session titles across
   sources within the recent window, capped by its own small token budget
   (default 600). This is what makes the agent *feel* aware without being
   asked — the titles alone ("refactored prompt engine adapter registration",
   "debugged SSE stream truncation") are enough for it to ask the right
   follow-up question.
3. **Timeline.** The interface merges `external_sessions` with channel
   checkpoints into one chronological view. Valuable, but it is a view over
   data the first two surfaces already justify — it ships last.

## Config

House style for named multi-instance sources
([`src/config/toml_schema.rs`](../../src/config/toml_schema.rs) messaging
adapters), tuning under `[defaults.session_sync]` with per-agent override like
ingestion:

```toml
[defaults.session_sync]
enabled = false
poll_interval_secs = 60
idle_minutes = 15
backfill_sessions = 50
tool_result_cap_chars = 300
digest_token_budget = 600

[[session_sync.sources]]
name = "claude"
kind = "claude-code"        # claude-code | opencode | generic
# path defaults per kind; generic requires one
enabled = true

[[session_sync.sources]]
name = "lab-notes"
kind = "generic"
path = "~/some/other/harness/sessions"
```

Session sync is per-agent (which agent's chronicle store receives the
sessions), and in practice one agent — the main one — enables it. During
onboarding, detection is a natural moment: adapters can probe their default
paths and offer "import your existing AI-tool sessions" as a checkbox, which
gives a brand-new instance months of awareness before its first conversation.

## Hazards carried explicitly

- **Small-model summaries are load-bearing for retrieval.** A bad summary is
  a silent recall miss. Mitigations: the `model` column on every row, the raw
  `path` pointer, and re-chronicling as a first-class operation — upgrading
  the compactor model can be followed by re-running any date range.
- **Sessions contain secrets.** The scrub pass is stated policy, not
  best-effort; the generic adapter especially processes unknown content. The
  digest fragment renders titles only, never summary bodies.
- **Sessions about spacebot itself.** The user's other tools may hold
  sessions *about* this codebase or this agent. That is fine — they are
  summaries of work, not instructions — but the chronicle prompt should frame
  the transcript as third-party material to summarize, not conversation to
  continue.
- **mtime is not a completion signal.** Some tools touch files on read. The
  idle gate plus `content_hash` comparison means a touched-but-unchanged file
  costs a hash, never an LLM call.

## Phases

**Phase 1 — registry and normalization.** Migration for both tables, the
`SessionImporter` trait, the Claude Code and generic adapters with their
deterministic pre-passes and the scrub pass, the watcher loop with idle gate
and status transitions, config plumbing through all four layers
(toml_schema → types → load → runtime). No LLM calls yet: the registry fills,
statuses move, sessions normalize. Testable entirely with fixtures.

**Phase 2 — chronicling.** The `external_session_chronicle` prompt, chunked
summarization through the compactor slot, rollups, incremental re-chronicle on
growth, failure handling. The live test: point it at a real Claude Code
session directory and read what comes out.

**Phase 3 — retrieval.** Lance schema extension + backfill, `ChronicleHit`
provenance, `memory_recall` formatting, the `sessions` tool
(`list | open | backfill`), the `external_activity` digest fragment.

**Phase 4 — OpenCode adapter and onboarding.** Second built-in adapter,
default-path detection, the onboarding import prompt.

**Phase 5 — timeline UI and docs.** The merged chronological view in the
interface; user-facing docs under `(features)`, harness-neutral in the same
way the import tool's docs are: named adapters for tools we integrate with,
the generic adapter for everything else.

## Non-goals

- **No transcript import.** Messages never enter `conversation_messages`;
  sessions never become channels. That is the import tool's job, once, for an
  agent migrating in. This pipeline stores summaries and pointers.
- **No memory writes.** The pipeline writes chronicles only. Memories about
  external work are made by the agent through the normal recall → save path.
- **No bidirectional sync.** Spacebot's own sessions are not exported into
  other tools' stores.
- **No embedded-tool coupling.** The embedded OpenCode integration is
  untouched; its adapter reads the same on-disk sessions any external
  OpenCode instance produces.
- **No format archaeology.** Adapters normalize what the current format
  provides. A source that changes its layout gets an adapter update, not a
  compatibility layer.
