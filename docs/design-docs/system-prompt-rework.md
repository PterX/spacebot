# System Prompt Rework

Implementation plan for the target defined in
[`system-prompt-architecture.md`](system-prompt-architecture.md) (content,
ordering, section ownership) and [`prompt-stability.md`](prompt-stability.md)
(caching invariant). Those docs say what the prompt should be; this one lists
every change required to get there, with code locations, grouped into phases
that can land independently. [`execution-plan.md`](execution-plan.md) carves
these phases and the memory-first work into ordered, parallelizable tasks.

A fully instantiated reference render exists (held privately with the
captures): 36.5k chars against the 49.1k the same operator config renders
today, while carrying more — six skills the current loader drops, full skill
descriptions, and an authority section the current prompt lacks.

---

## Phase 0 — Behavioral fixtures

Freeze fixtures against the **current** prompt before changing any prose.
Every later phase must not regress them. A prompt rewrite without a
behavioral baseline is an aesthetic opinion.

A fixture is: a canned conversation (system prompt rendered from a pinned
config + message history), one live model call, and an assertion on the
**first tool choice and its key arguments** — not on prose. Routing is what
the rewrite risks, and routing is observable in the first call. Each fixture
runs N samples (default 5) against the models the routing config actually
uses, with a pass threshold (default 4/5) — sampling because model outputs
vary, thresholds because a single flake must not block a phase. Byte-level
prompt assertions are a separate, cheaper suite (they gate caching, not
behavior).

- Standard-mode fixtures: delegation routing (branch vs worker vs direct
  reply), silence policy, acknowledgment behavior, result relay, memory
  intent handoff, skill suggestion.
- **Direct-mode fixtures**: the mode that keeps execution tools when the
  "never do X yourself" prohibitions are deleted. Does it still delegate
  long-running work? Does it still branch for memory? The prohibitions may
  not die before these exist.
- Capability-consistency test harness: for every mode and config combination,
  the capabilities the rendered prompt advertises must equal the registered
  tool set. This test exists before the template rewrite so the rewrite can
  be asserted against it.

## Phase 1 — Skill system

Independent of everything else and the largest byte win. Loader changes live
in `skills.rs`; the rendering lives in the **three fragment templates** —
`fragments/skills_channel.md.j2`, `skills_branch.md.j2`,
`skills_worker.md.j2` — and the flattening must land in all three, or branch
and worker prompts keep paying the XML tax.

1. **Flatten the index rendering.** Each skill currently renders as four
   indented XML lines (`<skill>`, `<name>…</name>`, `<description>…</description>`,
   `</skill>`) — measured at 107 chars of markup per skill, 22.6k to carry
   9.7k of names and descriptions on a 121-skill catalog, 54% of the whole
   prompt. Render `- name: description` under the existing category grouping
   instead. ~9.8k recovered, no information lost. The worker variant carries
   a `suggested="true"` attribute — the flat format keeps it as a marker on
   the line (e.g. `- name (suggested): description`).
2. **Recurse nested categories.** `load_skills_from_dir` descends one level
   (`skills.rs:550–602` — the doc comment even states the limitation);
   skills at `{category}/{subcategory}/{name}` are silently dropped (a
   126-skill tree renders 120). Recurse and render the subcategory as its own
   grouping (`category/subcategory` header).
3. **Read category descriptions from `DESCRIPTION.md`.**
   `load_category_description` reads only `index.md` (`skills.rs:607–618`);
   `DESCRIPTION.md` is the ecosystem convention and portable skill trees ship
   it. Accept both, prefer `DESCRIPTION.md`. Imported trees currently load
   categories with empty descriptions.
4. **Fix description truncation and close the import enforcement gap.**
   `DESCRIPTION_BUDGET` (80) truncates at a char boundary with an ellipsis
   (`skills.rs:387–397`) but mid-word; raise to 160 and cut at a word
   boundary. The budget is enforced at agent create/edit
   (`validate_skill_content`, `tools/skill_manage.rs:213`) but **not** on the
   import paths — `install_skill.rs` never calls it and `load_skills_from_dir`
   never validates — so imported trees hit the truncation (~26% of a
   126-skill tree). Validate on import; fix or let the new budget carry the
   bundled wiki-writing skill's 183-char description.

## Phase 2 — Identity layer

5. **Remove the `Identity::render()` wrappers.** The renderer emits `## Soul`
   / `## Identity` / `## Role` above files that carry their own headings, and
   emits the heading even when the file is empty. Files render verbatim;
   empty files render nothing.
6. **Rewrite the bundled presets to the file contracts.** Target ~1.2k chars
   across all three files, down from 4.5k. `SOUL.md` = temperament, voice,
   values; `IDENTITY.md` = who the agent is and who it serves; `ROLE.md` =
   obligations and success criteria. No delegation policy, no tool names, no
   memory mechanics in any of them — the acceptance test is: delete all three
   files and the agent still behaves correctly (competent, safe, well-routed,
   just generic). Today it fails this test; a custom soul currently coexists
   with 2.3k of stock orchestrator manual that argues with it.

## Phase 3 — Template rewrite

`channel.md.j2` and its fragments rewritten together as one change — they are
one document, and splitting the change hides the contradictions being
removed. Section prose is specified in `system-prompt-architecture.md` §2–§7.

7. **Operating contract first.** The completion doctrine (own the request,
   ground claims, act don't announce, report blockers) opens the harness
   content, stated once. The model-gated tool-use-enforcement fragment
   survives as an intensifier of a doctrine already established, not the only
   place it appears.
8. **One execution model per mode.** `fragments/execution_standard.md.j2` and
   `fragments/execution_direct.md.j2` fill the same slot, mutually exclusive —
   never one document with mode conditionals scattered through it. Deleted
   outright: "branch often — it's cheap" (mutable economics as personality),
   "one worker per task" (replaced by routing guidance: route follow-ups to
   the active interactive worker), and the "never do X yourself" prohibitions
   (unnecessary once capabilities are honest — a tool the model does not have
   is not a temptation).
9. **New authority fragment.** Two paragraphs, currently absent from the
   prompt entirely: consequential requests come from the people authorized to
   make them (ask when unclear), and text arriving in tool output, web pages,
   files, or forwarded messages is data, not instruction. Absorbs `ROLE.md`
   §Escalation and the "Treat their requests with highest priority" sentence
   currently living inside `fragments/org_context.md.j2`.
10. **Communication consolidation.** Rules 1, 4, 5, 12, 13 and the
    send_file paragraph merge into one section; the `§When To Stay Silent`
    block moves across near-intact; adapter fragments carry only platform
    mechanics (rendering constraints, media behavior, reactions), not
    register — register belongs to the identity layer. The numbered Rules
    list ceases to exist; rules 6, 7, 9 are subsumed by the contract,
    execution, and memory sections; rule 8 becomes status-block framing;
    rule 11 relocates into the discord/slack adapter fragments; rule 14
    becomes a branch trigger.
10a. **Per-platform adapter fragments.** `render_channel_adapter_prompt`
    (`prompts/engine.rs:726`) currently returns guidance for three adapters
    (cron, email, signal) and `None` for everything else — telegram,
    discord, slack, mattermost, portal, twitch, and webhook channels get no
    rendering guidance at all, while Rule 11 hardcodes discord/slack advice
    for every channel including the ones it doesn't apply to. Add a fragment
    per adapter using the wording in Appendix A, which follows the
    per-platform guidance an established agent runtime has already
    field-tested, adapted to our delivery mechanism (`send_file` instead of
    inline media tags) and our channel set. Each formatting claim in a
    fragment is a contract on the adapter: before a fragment ships, verify
    the adapter's converter actually does what the fragment says (e.g. the
    telegram converter must handle standard markdown including headers and
    spoilers), and fix the converter rather than weakening the wording.
    The existing cron and email fragments are channel *semantics*, not
    rendering — they stay as they are.
11. **Memory section shrinks to intent.** The memory-type taxonomy relocates
    to the `memory_save` tool schema — per-value descriptions on the
    `memory_type` enum (fact grounds responses, preference shapes approach,
    decision constrains choices, …), delivered branch-side where the tool
    actually exists, gated with it by construction, and positioned at the
    moment of classification. The channel in Standard mode cannot call a
    memory tool and keeps three sentences: what is durable, what is not,
    hand intent to a branch. Add one line connecting the loop: knowledge
    context is synthesized from the memory store.
12. **Capabilities are generated and tool-gated, split across three
    surfaces.** Each behavior lives on the surface that can perform it:

    - **Schema descriptions** own per-tool mechanics: parameters, recovery
      paths, per-value enum semantics. Enrich freely — tool definitions are
      serialized into the request ahead of the system prompt and sit inside
      the cache prefix, so they are stable bytes on the cheapest surface.
      Rig registers them conditionally out of the box, which makes their
      guidance tool-gated with zero template logic.
    - **§7 prose** owns proactive triggers — anything that must be active
      when the tool is *not* under consideration ("when the user states a
      preference, hand it to a branch"). A schema only gets attention once
      its tool is already a candidate; proactivity cannot live there.
    - **§3** owns cross-tool arbitration (branch vs worker vs reply) — the
      one thing no single tool's schema can carry, because each schema
      speaks only for itself.

    A one-sentence deliberate overlap of a core rule across schema and
    prose is allowed; whole paragraphs are not. Concrete moves this
    enables: `§Cron` is deleted outright (`cron_description` already covers
    and exceeds it); `§Task Board` shrinks to one sentence, with lifecycle
    detail moving to `task_create`/`task_update` descriptions (the tools
    are branch-side — the channel template currently documents tools the
    channel does not have); the sandbox posture line moves to the
    `spawn_worker` description. The Phase 0 consistency test asserts
    advertised == registered for every mode.

## Phase 4 — Context tier

13. **Evidence framing.** The durable/volatile context region opens with one
    line declaring everything below it world-state, not instruction. Needed
    because our context blocks are authored by other processes (cortex,
    builders) yet currently render with the same voice and authority as
    harness policy — the reason a stale observation can act like a standing
    prohibition.
14. **Budgets everywhere, reported inline.** `HUMAN.md` renders into
    `org_context.md.j2` uncapped — a 9k profile is 9k of prompt every turn;
    cap `entry.description` (default 4,000 chars) and render the budget into
    the block header. The budget is deliberately tight because of the
    authored/learned split in
    [`memory-first-knowledge-context.md`](memory-first-knowledge-context.md):
    `HUMAN.md` carries only the timeless core (identity, how to work with
    the person, standing rules), while dated and volatile facts live as
    scoped memories — so a profile pressing its cap is a signal to convert
    entries to memories, not to raise the cap. The cap never mid-content
    truncates: an operator-authored document may contain standing rules in
    its tail, the one content class that must not be silently dropped. An
    over-budget profile renders in full with the utilization header
    reporting >100% loudly; past a hard ceiling (2× the cap) it truncates
    only at a section boundary. The conversion actor is reflection: it
    proposes which entries to move to scoped memories, and the operator
    approves — the harness never rewrites an authored file on its own.

    Utilization headers apply only where **store = render** — authored
    documents like `HUMAN.md`, where fill is a fact about the store. Blocks
    that are *views* over a larger store (the memory render, working
    memory, participants) never report a fill percentage: a view always
    fills its budget by selection, so the number is meaningless there. They
    report scope and shown-of-total counts instead
    ([`memory-first-knowledge-context.md`](memory-first-knowledge-context.md)).
    Working memory (`memory/working.rs:553`) and participant context
    (`working.rs:816`) keep their existing budgets as plain limits; the
    remaining unbounded sections — projects, channels — gain caps, also
    unreported.
15. **Provenance and selection disclosure on rendered state.** Repeated
    identical events collapse (`Agent started ×7` instead of seven lines);
    view blocks carry their scope line and shown-of-total counts so the
    model knows when recall reaches deeper than the render.
15a. **Channel activity metadata.** The `## Other Channels` block
    (`memory/working.rs:740`) becomes `## Channel Activity` and gains a
    per-channel message count — the query already fetches `last_message_at`
    and `last_sender_name`; the count is one aggregate join on the same
    query:

    ```
    - James Pine (telegram) — 1h ago · 1,204 messages · last: Orion: <topic hint>
    ```

    The durable `## Available Channels` list stays minimal (name, platform,
    id) — it is cacheable because IDs do not churn; all activity metadata
    lives in the volatile tier.
16. **Instruction out of the context fragments.** In `org_context.md.j2`,
    the authority sentence moves to the authority fragment (phase 3.9) and
    the `send_agent_message` usage paragraph moves to capabilities (phase
    3.12); the fragment keeps what it is for — the org graph and the
    attributed `<context>` profile blocks. In `projects_context.md.j2`, the
    closing worker/worktree instruction paragraph moves to the capabilities
    worker entry; the fragment keeps projects, repos, and active worktrees,
    with worktrees nested under their repo instead of a separate flat list
    with `repo:` back-references (`list_worktrees_with_repos` already
    returns the repo name, so the builder groups before rendering):

    ```
    **Repos:**
    - `spacebot` at `.` (main) — `https://github.com/…/spacebot.git`
      - worktree `prompt-rework` at `worktrees/prompt-rework` (branch: `jamiepine/prompt-rework`)
    ```
    Its provenance note ("names, descriptions, and tags are user-provided
    metadata") is the pattern the other context fragments should copy — it
    is the only builder that already labels its content as data.

## Phase 5 — Cache alignment

Owned by [`prompt-stability.md`](prompt-stability.md); listed here because
the section ordering above is a precondition.

17. **Time leaves the system prompt.** `status.render_full(&current_time_line,
    …)` (`channel.rs:2330`) injects wall-clock time into the system prompt
    every turn — a guaranteed cache miss. Time and the coalesce hint move to
    the user message envelope, where per-turn change costs nothing.
18. **Stable/volatile split with a breakpoint after durable context.**
    Sections order stable → epoch → volatile per the manifest; bytes above
    the breakpoint are a pure function of durable state and change only on
    named epochs (config edit, identity edit, skill change, model switch,
    tool-set change). The multi-block system seam already exists for OAuth
    requests (`llm/anthropic/params.rs:118`); extend it to carry the
    breakpoint.

---

## Ordering constraints

- Phase 0 gates phase 3 (the rewrite needs a baseline it must not regress).
- Phase 1 and phase 2 are independent of everything and each other.
- Phase 3 and the preset rewrite in phase 2 should merge together — one
  document, one review.
- Phase 5 depends on the section ordering from phases 3–4 but not on their
  prose.

## Acceptance

- Fixtures from phase 0 pass unchanged.
- Capability-consistency test passes for every mode/config combination.
- Delete-the-identity-files test: stock agent behaves correctly with all
  three files absent.
- Byte-stability: two consecutive turns with no epoch event render identical
  bytes above the breakpoint.
- Rendered size on the reference config drops from ~49k to ~37k while the
  skill count rises (nested categories included).

---

## Related work — memory and chronicles

Not part of the prompt rework itself, but grouped here because it changes
what the knowledge context (§9 of the manifest) actually is.

The direction is set by
[`memory-first-knowledge-context.md`](memory-first-knowledge-context.md):
read-time knowledge synthesis is removed, and the §9 slot becomes a direct,
deterministic render of the memory store (typed sections by importance,
per-type budgets, participant scoping, write-time consolidation). An earlier
plan to feed session chronicles into the synthesis gather is superseded with
the synthesis itself — chronicles reach the model through the chronicle
window and recall tools, and reach knowledge through the links below.

1. **Memory → chronicle links.** Memories already carry `channel_id`
   (stamped at save, `tools/memory_save.rs:252`) and `created_at`; chronicle
   coverage is contiguous and non-overlapping per channel. So every memory
   resolves to exactly one checkpoint by range join —
   `channel_id = ? AND covers_from_at <= created_at <= covers_to_at` — with
   no schema change. Resolve lazily at read time (a fresh memory sits in
   the unsummarized tail until the next cut; by recall time its checkpoint
   exists). Surface it in two places:
   - `memory_search` results gain a session pointer (checkpoint title +
     seq), so a branch that recalls a fact can immediately open the session
     it came from and expand to raw transcript through the chronicle tool.
     Recall stops dead-ending at the fact and becomes a path back to its
     context.
   - Supersede-with-provenance: when a memory is corrected, the replacement
     can cite the session where it changed, instead of the old fact just
     vanishing.
   Add an explicit `checkpoint_id` column only if the range join proves hot —
   `idx_chronicle_window` already covers most of the lookup.
2. **Vectorize chronicle checkpoints.** Same engine as memory, separate
   corpus: a `chronicle_embeddings` LanceDB table next to `memory_embeddings`
   (`memory/lance.rs`), sharing the connection, embedding model
   (all-MiniLM-L6-v2, 384 dims), HNSW+FTS setup, and the
   regenerate-from-SQLite recovery story. Checkpoints are the ideal
   embedding corpus — LLM-authored paragraph summaries, append-only, so each
   is embedded once at commit and never invalidated. Embed level-0 rows
   only; rollups are derivative and exist for prompt-window compression, not
   retrieval.

   Separate table because the two corpora have different degradation
   models, and a store's degradation model is its identity: memories
   degrade with **volume** (budget pressure forces merge, supersede,
   forget; age is irrelevant to a still-true fact), chronicles degrade with
   **time** (the timeline only extends; resolution decays with distance via
   rollups; dedup does not apply and forgetting is forbidden — append-only
   is the invariant). Rows in a shared store would either be mangled by the
   volume machinery or special-cased out of every code path. Secondary
   reasons hold too: uncalibrated cosine scores across facts vs.
   narratives, disjoint filter dimensions, opposite delete semantics.

   Unify at the query surface instead: branch-side recall and the UI search
   both tables and return labeled results — memories and sessions — with
   the memory→checkpoint range join (item 1) as the bridge. This closes the
   semantic gap in session navigation: every current path is temporal;
   nothing answers "when did we discuss X" by meaning unless a memory
   happened to be written. One embed per checkpoint cut plus a one-time
   backfill.

---

## Appendix A — adapter fragment wording

One fragment per messaging adapter, rendered into the communication section's
adapter slot. Register (message length, tone, when to go long) is not here —
that belongs to the identity layer. These carry only what the platform itself
imposes.

### telegram

> You are on a text messaging communication platform, Telegram. Standard
> Markdown is automatically converted to Telegram formatting. Supported:
> **bold**, *italic*, ~~strikethrough~~, ||spoiler||, `inline code`,
> ```code blocks```, [links](url), and ## headers. Prefer bullet lists and
> labeled key:value pairs for structured data. You can send media files
> natively: deliver them with `send_file`. Images (.png, .jpg, .webp) appear
> as photos, audio (.ogg) sends as voice bubbles, and videos (.mp4) play
> inline. A well-placed reaction lands better than reacting to everything.

### discord

> You are in a Discord server or group chat communicating with your user.
> You can send media files natively: deliver them with `send_file` — images
> (.png, .jpg, .webp) are sent as photo attachments, audio as file
> attachments. Prefer rich responses when output is structured or multi-part
> (task outcomes, summaries, comparisons, checklists, plans) — use `reply`
> with `cards` instead of plain text walls when it improves clarity.

### slack

> You are in a Slack workspace communicating with your user. You can send
> media files natively: deliver them with `send_file` — images (.png, .jpg,
> .webp) are uploaded as photo attachments, audio as file attachments.
> Prefer rich responses when output is structured or multi-part — use
> `reply` with `blocks` instead of plain text walls when it improves
> clarity.

### mattermost

> You are in a Mattermost workspace communicating with your user.
> Mattermost renders standard Markdown — headings, bold, italic, code
> blocks, and tables all work. You can send media files natively: deliver
> them with `send_file` — images (.jpg, .png, .webp) are uploaded as photo
> attachments, audio and video as file attachments.

### signal

Prepended to the existing cross-channel-targets fragment:

> You are on a text messaging communication platform, Signal. Standard
> markdown (**bold**, *italic*, ~~strike~~, # headers, `code`,
> ```code blocks```) is auto-converted to Signal's native rich formatting —
> write in markdown, and use bullet lists ('- item') freely (they render as
> • bullets). Tables are NOT supported — prefer bullet lists or labeled
> key:value pairs. You can send media files natively with `send_file`:
> images (.png, .jpg, .webp) appear as photos, audio as attachments, and
> other files arrive as downloadable documents.

### portal

> You are in the portal, a browser-based chat interface. Full Markdown
> rendering is supported — headings, bold, italic, code blocks, and tables
> render natively. Deliver files with `send_file`; images, audio, and video
> render as rich previews. Do not paste local filesystem paths as the
> handoff — deliver the file.

### twitch

> You are in a Twitch chat. Plain text only — no markdown, no formatting;
> messages are capped around 500 characters, so be brief and direct. This
> is fast-moving public chat: one thought per message, and most messages
> are addressed to the streamer or other chatters rather than you.

### webhook

> You are responding through a webhook. The rendering layer is unknown —
> assume plain text. No markdown formatting (no asterisks, bullets,
> headers, code fences). Treat this like a conversation, not a document.
> Keep responses brief and natural.

### Claims to verify before each fragment ships

- telegram: converter handles standard markdown → Telegram entities,
  including headers and spoilers; `send_file` maps .ogg to voice bubbles.
- discord/slack: `send_file` attachment behavior per file type; `cards` /
  `blocks` names match the reply tool's actual parameters.
- mattermost: table rendering confirmed; `send_file` upload types.
- portal: markdown feature set of the web renderer (tables, code
  highlighting); which preview types the portal actually renders inline.
- twitch: exact message cap; whether the adapter splits long messages.
- webhook: whether `send_file` has any delivery path (if not, the fragment
  must say to state plain file paths instead).
