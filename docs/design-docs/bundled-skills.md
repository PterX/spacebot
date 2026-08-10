# Bundled Skills: the curated first-party catalog

Builds on `skill-lifecycle.md`, which covers how skills load, mutate, and get
curated. This doc covers what ships in the box: which skills spacebot bundles,
the rule that decides membership, how bundled skills are distributed and
updated, and how skills declare the secrets they depend on.

## Why bundle at all

The skills.sh registry solves distribution, not discovery. A fresh install
staring at a few thousand registry entries has no signal for which ten matter,
and most of the corpus is low quality. A curated catalog is the harness authors
predicting the common use cases and shipping a vetted, maintained answer for
each — the same reason a distro ships coreutils instead of a package search
box. Users who don't want a bundled skill delete it once; the sync mechanism
records the choice and never brings it back.

The catalog also completes the credential integration story. The documented
pattern is "add a credential, install a skill." With bundled skills declaring
their secrets (see below), a fresh install renders as a visible menu: here is
what I can do, here is the one key each capability needs.

## The inclusion rule

Reading a skill costs a tool turn plus its body in context, paid every time
the activity comes up. That fixes the test for membership:

**Bundle a skill when reading it shifts the session into a mode — the activity
is infrequent, the craft content is dense, and the knowledge is not already in
the system prompt. Reject it when it documents a routine primitive.**

Task tracking, worker delegation, cron management, and messaging are routine
primitives: the agent does them constantly, the prompt fragments already carry
the guidance, and a skill would add a read tax to every occurrence while
teaching nothing the fragments don't. Wiki writing is a mode: it happens
occasionally, the craft (page types, linking discipline, tone) is too large to
carry in every prompt, and reading the skill once at the start of a wiki
session measurably changes output quality. Every candidate below passed this
test; the skip list is mostly candidates that failed it.

The same test applies to future additions. A proposed bundled skill should
answer: how often does this come up, and what does the skill know that the
system prompt doesn't?

## The catalog

Twenty skills in five groups plus the two craft skills. Descriptions shown are
the real index entries and fit the 80-char budget.

### Craft (currently the builtin tier)

| Skill | Description | Secrets |
|---|---|---|
| `wiki-writing` | Language, structure, and judgment for creating or editing wiki pages | — |
| `skill-authoring` | What makes a durable skill; when to patch, create, or write nothing | — |

`wiki-writing` already ships. `skill-authoring` is new and is the second
legitimate own-tool skill: reflection fires at most hourly (a mode, not a
primitive), and it is the one place where bad output compounds — a badly
authored skill degrades every future session that reads it. Its content is the
prompt-policy ladder from `skill-lifecycle.md` §4 expanded with worked
examples: task-class naming, the negative-capture list, patch-over-create,
support-dir usage, description budget discipline.

### Documents

| Skill | Description | Secrets |
|---|---|---|
| `docx` | Create and edit Word documents with styles, tables, and tracked changes | — |
| `xlsx` | Build spreadsheets with formulas, charts, and formatting via openpyxl | — |
| `pptx` | Build presentation decks with layouts, themes, and speaker notes | — |
| `pdf` | Create, merge, split, and fill PDF forms | — |
| `ocr` | Extract text and structure from scanned documents and images | — |

The strongest case for bundling: script-heavy, impossible to wing reliably
from model knowledge, and "make me a deck and send it here" is core
chat-assistant territory. Adapt from `anthropics/skills`, the canonical
upstream — track it rather than fork-and-drift. Output lands via `send_file`.

### Development

| Skill | Description | Secrets |
|---|---|---|
| `github-pr-workflow` | Branch, commit, open PRs, track CI, and merge with gh | `GH_TOKEN` |
| `github-code-review` | Review diffs and leave inline PR comments with gh | `GH_TOKEN` |
| `claude-code` | Delegate coding tasks to the Claude Code CLI from a worker | `ANTHROPIC_API_KEY` |
| `systematic-debugging` | Four-phase root-cause method: reproduce, isolate, fix, verify | — |

`claude-code` is the orchestrator posture made explicit: a worker driving a
coding CLI inside a project worktree, with spacebot handling the conversation,
scheduling, and reporting. `systematic-debugging` is pure methodology — the
cheapest kind of skill to maintain and the clearest mode shift.

### Research

| Skill | Description | Secrets |
|---|---|---|
| `deep-research` | Multi-source research with grounded citations and source verification | — |
| `arxiv` | Search, download, and summarize papers by topic, author, or ID | — |
| `monitoring-digest` | Recurring watch on a topic or site, delivered as a cited digest | — |

`monitoring-digest` is a recipe, not a primitive manual: it composes `cron`,
`web_search`, and `reply` into a repeatable pattern (dedup against last run,
digest format, when to stay silent). Recipes pass the inclusion rule even when
their ingredients are primitives, because the composition is the craft.

### Chat and media

| Skill | Description | Secrets |
|---|---|---|
| `gif-search` | Find and send GIFs via the Tenor API | `TENOR_API_KEY` |
| `youtube-content` | Pull video transcripts and turn them into summaries or posts | — |
| `media-processing` | Convert, trim, resize, and compose media with ffmpeg and imagemagick | — |
| `diagrams` | Draw architecture and flow diagrams, rendered to images | — |

### Integrations

| Skill | Description | Secrets |
|---|---|---|
| `google-workspace` | Read and manage Gmail, Calendar, and Drive | Google OAuth credentials |
| `notion` | Read, create, and update Notion pages and databases | `NOTION_API_KEY` |
| `maps` | Geocode, find places, and compute routes via OpenStreetMap | — |

Keyless skills work out of the box. Keyed skills cost one index line while
unconfigured and light up when the secret appears — that is the "add a
credential" pattern with the discovery problem solved.

## Not bundled, and why

Ready answers for "why doesn't spacebot ship X":

- **Desktop-session skills** (Apple Notes/Reminders/iMessage, screen control,
  local note vaults): they assume a logged-in desktop the process is sitting
  on. Spacebot instances are typically headless servers; platform gating can't
  express "has a user session." Registry material for the exception cases.
- **Email clients over IMAP/SMTP CLIs**: spacebot has a native email adapter
  and `email_search`; a client skill would fight the platform layer.
- **Local inference / model-training stacks** (vLLM, llama.cpp, fine-tuning):
  real audiences, wrong default. Registry.
- **Diffusion pipelines** (ComfyUI and kin): heavy infrastructure assumptions.
  Registry.
- **Smart-home and social-posting CLIs**: too niche for the default index.
  Registry.

The long tail belongs to skills.sh plus `install_skill`. Bundling is for what
most instances will actually use.

## The index is grouped by category

Skill discovery already scans `skills/{category}/{name}/SKILL.md`, but the
category dies there — the `Skill` struct doesn't carry it, and all three index
fragments render a flat name+description list. That flat list is fine at five
skills and structurally hostile at a hundred: a model scanning for relevance
does materially better when the index is organized by domain first, entry
second, and a mature instance accumulates enough self-authored skills that
flat scanning degrades exactly when the corpus becomes most valuable.

Changes:

1. **Category on the index entry**, derived from the directory path at load
   (top-level skills get `general`). No frontmatter field — the filesystem is
   already the taxonomy, and installers/creators choose placement by path.
2. **Grouped rendering** in all three fragments: category line, then its
   skills, categories and names sorted. Same information, hierarchical shape.
3. **Category descriptions.** A category directory may carry an `index.md`
   with a one-line `description` in frontmatter, rendered on the category
   line. The bundled catalog ships one per category; user categories work
   without them.
4. **A firmer load directive.** The current fragment preamble suggests
   scanning for relevant skills. It should instruct: scan before acting, read
   any skill that is even partially relevant, and prefer reading an
   unnecessary skill over missing an established procedure — the skill defines
   how the task is done here, even when the task looks familiar. The index
   only pays for itself if reading it reliably converts to `read_skill` calls;
   a polite preamble undersells the corpus.

The bundled catalog lands pre-categorized (`craft/`, `documents/`,
`development/`, `research/`, `chat-media/`, `integrations/`), so a fresh
instance starts with a structured index and self-authored skills grow into
the same shape instead of piling into a flat root.

## Secrets integration

The secret store already does the hard half: `auto_categorize` defaults
unknown names to `Tool`, and `tool_env_vars` injects Tool secrets into every
worker subprocess. A skill's script that reads `TENOR_API_KEY` from the
environment works the moment the user sets the secret. What's missing is
declaration and surfacing.

### Frontmatter

New optional field on `SkillFrontmatter`:

```yaml
secrets:
  - name: TENOR_API_KEY
    purpose: Tenor API for GIF search
    setup_url: https://developers.google.com/tenor/guides/quickstart
```

`name` is required, `SCREAMING_SNAKE` enforced; `purpose` and `setup_url`
optional. Foreign skills without the field declare nothing and behave as
today. Skills from ecosystems that carry an advisory env-var list in prose
keep working — this field is the wired version.

### Index annotation

At prompt render, declared names are checked against
`tool_secret_names(agent_id)` — existence only, values never touched. Missing
secrets annotate the index line:

```
- gif-search: Find and send GIFs via the Tenor API (needs TENOR_API_KEY)
```

Soft gate, deliberately. A hidden skill can't be offered; an annotated one
lets the agent say "I could do this if you add a key," which is the discovery
moment the credential pattern exists for. Platform gating stays hard —
a skill for the wrong OS is noise, a skill missing a key is a suggestion.

### Surfacing

- `install_skill` returns declared-but-unset secrets in its result, so the
  installing agent relays setup steps in the same turn.
- `read_skill` includes secret status alongside `linked_files`.
- `SkillInspector` lists declared secrets with set/unset state, linking to the
  existing secret entry flow. `spacebot skill info` prints the same.
- The interface's bundled-skills view groups the catalog and shows which
  capabilities are one key away from lighting up.

### Guardrail

Lint — at `skill_manage` create/edit, install, and seed time — rejects any
declared secret whose name matches the `system_secret_registry`, exact or
instance-pattern (`DISCORD_*_BOT_TOKEN` and kin). The category system already
guarantees System secrets never inject into workers, but installed skills are
third-party content: a skill requesting a bot token or LLM key is confused or
hostile, and either way it fails loudly at install rather than silently at
runtime.

## Distribution: seeding replaces the embed

The builtin tier is a compile-time `include_str!` of a single `SKILL.md` with
a synthetic `builtin://` path. That cannot carry `scripts/` or `references/`,
and `{baseDir}` has nothing to resolve to — a dead end for the document
skills, which are mostly scripts. Bundled skills instead seed to disk:

1. The catalog lives in the repo under `skills/bundled/{category}/{name}/`
   (full support dirs) and is embedded in the binary as a directory tree
   (`rust-embed` or equivalent) — no install-time network dependency.
2. On startup, the seeder materializes the catalog into
   `{instance_dir}/skills/`, guided by a manifest at
   `{instance_dir}/skills/.bundled_manifest.json` recording a content hash
   per seeded file.
3. Sync rules, per skill:
   - not on disk and not pruned → seed it, record hashes.
   - on disk, hashes match the manifest → safe to update in place when the
     binary ships a newer version.
   - on disk, hashes differ → the user edited it; skip updates, mark
     diverged. Their copy wins until they restore.
   - user-deleted → recorded in the manifest's pruned list; never re-seeded.
     `spacebot skill restore <name>` clears the entry and re-seeds.
   - dropped from the catalog in a newer binary → removed if unmodified,
     left in place if diverged.
4. The `Builtin` source tier is retired. `wiki-writing` moves into the seeded
   catalog; precedence collapses to Instance < Workspace with bundled skills
   living at instance level like any installed skill.

Seeded skills are ordinary instance-level skills, which the existing rails
already handle: agent-origin writes cannot touch instance-level skills, so no
reflection pass ever mutates the catalog; workspace copies override by name
for per-agent customization; pin/archive/adopt behave uniformly. On first
sight, the usage-table seeding described in `skill-lifecycle.md` §2 consults
the manifest and tags catalog skills `created_by = 'bundled'` — outside
curator jurisdiction, like `'installed'`, and distinguishable in the UI.

Deleting a bundled skill is the disable mechanism. It is one action, it
persists across upgrades via the prune list, and restore is symmetric. No
separate disabled-set config.

## Config

```toml
[skills.bundled]
enabled = true      # false skips seeding entirely; already-seeded skills remain
```

One flag. Everything else is expressed through the existing skill lifecycle
(delete to disable, restore to re-enable, workspace copy to customize).

## Phases

**Phase 1 — index shape.** Category on the index entry; grouped rendering in
the three fragments; `index.md` category descriptions; the firmer load
directive. Small, self-contained, and improves existing installs before any
catalog work.

**Phase 2 — secrets wiring.** `secrets` frontmatter field; lint against the
system secret registry; index annotation at prompt render; secret status in
`install_skill` results, `read_skill`, `skill info`, and `SkillInspector`.
Ships independently — installed registry skills benefit before the catalog
exists.

**Phase 3 — seeding.** Embedded catalog payload, manifest, sync rules, prune
list, `skill restore`; retire the `Builtin` tier and migrate `wiki-writing`;
`created_by = 'bundled'` provenance; `[skills.bundled]` config.

**Phase 4 — craft and documents.** Author `skill-authoring`; adapt the five
document skills from `anthropics/skills`; validate script execution through
the sandboxed shell on both platforms.

**Phase 5 — the working catalog.** Development, research, and chat/media
groups. Each skill lands with its secrets declared and a smoke test that
exercises the underlying CLI path.

**Phase 6 — integrations and surfaces.** Keyed integration skills; bundled
grouping and secret status in the interface; user docs for the catalog and
the disable/restore/customize flows.

## Non-goals

- **No automatic installation of third-party CLI dependencies.** Skills
  document their install steps; execution stays inside the existing sandboxed
  shell. Prerequisite checking beyond secrets can come later if it earns its
  keep.
- **No out-of-band catalog updates.** The catalog versions with the binary.
  A skill fix ships like a code fix.
- **No own-tool skills beyond the two craft skills.** The inclusion rule is
  the standing policy; primitives stay in prompt fragments.
- **No bundled skill referencing other agent harnesses or consumers by name.**
  Catalog content is platform documentation and stays vendor-neutral.
- **No secret values in skill land, ever.** Skills declare names; the store
  holds values; rendering checks existence only. Skill bodies and support
  files never contain credentials, and lint rejects obvious violations.
