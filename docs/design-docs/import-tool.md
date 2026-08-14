# Import Tool: migrating an agent from another harness

> **Superseded by [external-sessions.md](external-sessions.md).** Session
> chronicling over a read-only source replaces the transcript import, and the
> remaining migration surface travels through existing mechanisms: identity
> and memory files via the ingest route or plain file copies, credentials by
> re-entry. Two pieces of this design remain independently valuable and may
> be lifted out on their own merit: the bulk memory writer (a deterministic
> batch path into SQLite + LanceDB) and the hazard analysis, the relevant
> half of which external sessions inherits as its scrub pass.

A mature agent is a corpus: identity, memories, skills, transcripts, scheduled
jobs, credentials. Today the only way to move one into spacebot is the memory
ingestion loop, which is LLM-mediated and lossy by design — nothing arrives
verbatim, results differ run to run, and everything that isn't a memory
(skills, cron, transcripts, identity) has no path at all. This doc designs a
deterministic importer. The reference source studied is a production Hermes
instance (126 skills, 233 sessions, 33.5k messages, ~25 days of continuous
operation); the tool itself is source-pluggable, and product surfaces name no
other harness — `spacebot import` detects the source layout and picks the
adapter.

## Principles

- **Deterministic, never LLM-mediated.** Every record maps by rule. Two runs
  produce identical results. The ingestion loop's failure mode — a model
  deciding per-chunk what deserves to survive — is the thing this tool
  replaces.
- **The source is read-only.** The importer never mutates or deletes source
  files. (The ingestion loop deletes successfully processed files; the
  importer inherits none of that.)
- **Idempotent.** Every imported record carries a content hash; re-running
  resumes and repairs rather than duplicates.
- **Offline.** The daemon is stopped during execute. The memory writer needs
  exclusive access to SQLite + LanceDB, and the config/secrets writes should
  not race the watcher.
- **Report, don't decide, on sensitive content.** Embedded secrets, private
  dossiers, and dangling references are surfaced in the scan report for the
  user to resolve — never silently copied or silently dropped.

## Source inventory (hermes home)

What a hermes instance holds, tiered by migration value. Sizes from the
reference instance.

| Tier | Data | Form |
|---|---|---|
| Critical | `SOUL.md` (persona), `USER.md` (user dossier), `memories/MEMORY.md` + `memories/USER.md` (§-delimited entries), `config.yaml`, `.env` credentials, `profiles/*/` (each a nested second agent) | ~40 KB of files |
| High | `skills/` (category dirs of SKILL.md + support files), `cron/jobs.json`, `scripts/` (cron job bodies), `webhook_subscriptions.json`, small state ledgers (e.g. `state/reviewed-prs.txt`) | ~57 MB |
| High, bulk | `state.db`: `sessions`, `messages`, `session_model_usage` | ~180 MB after excluding FTS shadow tables |
| Medium | cron output archives, `cron/executions.db`, `channel_directory.json`, `discord_threads.json` | ~2 MB |
| Skip | FTS indexes (rebuildable), caches, logs, snapshots/backups, lock/pid files, empty databases (`kanban.db`, `projects.db`, `response_store.db` in the reference), OAuth token stores (not portable — see hazards) | ~1.4 GB |

## Destination mapping

### Straight copies

| Source | Destination | Notes |
|---|---|---|
| `SOUL.md` | `agents/{id}/SOUL.md` | verbatim; `IDENTITY.md`/`ROLE.md` scaffold from the chosen preset |
| `USER.md` | `humans/{id}/HUMAN.md` | spacebot's human-graph file is the same concept |
| `skills/{category}/{name}/` | `agents/{id}/workspace/skills/` | format-compatible: frontmatter parser tolerates foreign fields, category layout matches the two-level scan. Deeper nesting flattens to `{category}-{sub}/{name}` with a report line. `skill_usage` rows seeded `created_by = 'installed'` — outside curation until adopted |
| `scripts/`, state ledgers | `agents/{id}/workspace/scripts/`, `workspace/state/` | paths inside cron prompts rewritten to match (see cron) |
| `profiles/{name}/` | a second `spacebot agent create`, same recipe recursively | each profile is a self-contained agent home |

### Transforms

| Source | Destination | Transform |
|---|---|---|
| `memories/*.md` § entries | `memories` + LanceDB | the bulk memory writer, below |
| `state.db` `sessions` | `channels` | one channel per (platform, chat_id, thread); `platform_meta` carries the source session ids |
| `state.db` `messages` | `conversation_messages` | active user/assistant rows by default (`--full` imports inactive too); tool rows dropped, tool names folded into `metadata`; timestamps preserved |
| `state.db` `session_model_usage` | `token_usage` | optional (`--usage`), analytics only |
| `cron/jobs.json` | `cron_jobs` | cron-expr jobs map to `cron_expr`, interval jobs to `interval_secs`; `deliver` targets map to `delivery_target` via the channel mapping; script paths rewritten to the imported `workspace/scripts/` |
| `.env`, `.env.handles` | secrets store | name mapping table per adapter (`TELEGRAM_BOT_TOKEN` and kin are already canonical); `auto_categorize` sorts System vs Tool; values enter via `SecretsStore::import_all` |
| `config.yaml` `mcp_servers` | `[[agents]]` `mcp` entries | direct field mapping; bearer-token env refs become `secret:NAME` |
| `config.yaml` platform config | `[messaging]` + `[[bindings]]` | enabled platforms with credentials present; allowlists map to channel permissions |
| `webhook_subscriptions.json` | webhook adapter config | prompt carried; embedded shared secrets flagged for rotation, never copied |
| `channel_directory.json`, thread lists | `channels` rows | pruned of entries with no messages (test stubs) |

Empty source databases import nothing and say so in the report.

## The bulk memory writer

The core new plumbing, and independently valuable beyond imports. Spacebot's
memory API is read-only; the only write path is the `memory_save` tool. The
importer adds a batch writer in `src/memory` that mirrors the tool's pipeline
exactly — SQLite insert, fastembed embedding into LanceDB, FTS refresh, with
the same compensating deletes on partial failure — minus the LLM and the
tool-call ceremony:

```rust
pub struct MemoryImport {
    pub content: String,
    pub memory_type: MemoryType,     // per-source mapping, default Fact
    pub importance: f32,             // explicit, never defaulted by decay class
    pub created_at: DateTime<Utc>,   // source timestamp when known
    pub source: String,              // "import:hermes:memories/USER.md"
}
```

§-delimited entries are already atomic memory-sized records: split, trim,
hash, write. Entries from the user-profile file get `memory_type = identity`
tilt where the adapter recognizes it, `fact` otherwise — mapping is rule-based
per adapter, never inferred by a model.

Janitor interactions, handled not hoped-for:

- The run sets explicit `importance` and real `created_at`; a batch stamped
  "now" with default importance would decay in lockstep and could be
  mass-pruned together.
- The memory janitor and near-duplicate merge are paused for the import run
  (offline execution makes this free) and the report lists any imported pairs
  above the merge similarity threshold so the user sees what the janitor will
  eventually consider merging.

The same writer backs a future `POST /agents/memories` batch endpoint and
gives the ingestion loop a verbatim mode; both are follow-ups, not part of
this tool.

## Staged CLI flow

```
spacebot import scan <path>              # read-only; writes import-report.md + manifest
spacebot import plan [--edit]            # show/adjust the manifest (target agent, inclusions)
spacebot import execute --agent <id>     # daemon stopped; deterministic, resumable
spacebot import verify                   # counts, embedding coverage, recall smoke test
```

- **scan** detects the source layout, inventories by tier, and emits the
  hazard list: embedded secrets found in content, dangling references
  (the reference instance's `USER.md` points at a `context/` directory that
  does not exist), coupled artifacts, unportable credentials.
- **plan** is an editable manifest — which agent receives the import, which
  tiers/items are in or out, channel mapping overrides. Defaults are the
  tables above.
- **execute** refuses to run with the daemon up, snapshots the target agent
  dir first (same tar.gz pattern as skill curation), then applies file copies
  and transforms in dependency order: identity → skills → secrets → config →
  channels → transcripts → memories → cron. Every write is recorded with its
  content hash in an import ledger (`data/import_ledger.db` in the target
  agent dir), which is what makes re-runs resumable.
- **verify** re-counts source vs. imported records, confirms every imported
  memory has an embedding row, runs an FTS query and a vector recall against
  known content, and validates each imported cron job parses and its script
  path exists.

## Hazards the design carries explicitly

- **Coupled artifacts move together.** Cron jobs reference `scripts/` by
  path and keep dedup ledgers in `state/`; the manifest groups a job with its
  script and ledger, and excluding one excludes the group with a warning.
- **Secrets embedded in content.** The scan flags secret-shaped strings in
  memories, prompts, and webhook definitions (reusing the scrubber's
  patterns). Webhook shared secrets are always regenerated on the target.
- **OAuth tokens don't port.** Provider auth (`auth.json`, Google tokens) is
  bound to the source install; the report lists which providers need re-auth
  on spacebot and which imported capabilities (e.g. a memory describing a
  Google integration) depend on them.
- **Transcript depth default is active-only.** Compacted-away rows and tool
  transcripts are retained in the source, which the importer never modifies;
  `--full` exists for completists.
- **The dossier is sensitive.** `USER.md`-class files carry health, financial,
  and relationship detail with stated disclosure rules. The scan report names
  them and requires their inclusion to be explicit in the manifest rather
  than bundled silently into a default.

## Config

None. The importer is a CLI flow with a manifest file; nothing about it
belongs in `config.toml`.

## Phases

**Phase 1 — bulk memory writer.** The batch writer in `src/memory` with
compensation, janitor pause, and explicit-metadata records; unit-tested
against the same invariants as `memory_save` (no orphans in either store).

**Phase 2 — scan and manifest.** Source adapter trait + hermes adapter
detection and inventory; report generation with the hazard list; manifest
format and plan editing.

**Phase 3 — execute.** File copies, secrets and config transforms, channel +
transcript import, cron transform with path rewriting; import ledger and
resumability; pre-run snapshot.

**Phase 4 — memories and verify.** § entry parsing, memory import through the
Phase 1 writer, the verify command, profile recursion (second source profile →
second agent).

**Phase 5 — docs.** User-facing migration guide, kept harness-neutral: layout
detection means the docs describe "importing an existing agent," not any
specific competitor.

## Non-goals

- **No live sync or incremental mirroring.** This is a migration, run a small
  number of times, not a bridge two harnesses run behind.
- **No reverse export.** Spacebot's backup export covers leaving; shaping it
  for a specific foreign harness is not our job.
- **No LLM passes.** Not for memory distillation, not for transcript
  summarization, not for skill rewriting. Anything worth condensing can be
  condensed by the agent after it has the verbatim corpus.
- **No OAuth token migration.** Re-auth is the correct cost.
- **No source mutation, ever** — including on success.
