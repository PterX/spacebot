# Prompt Inspector

Every request Spacebot sends to a model is recorded, decomposed into named blocks, and openable from the UI. Any channel turn, branch, worker, compaction, chronicle, or cortex run can be inspected as the exact byte stream the model received, with a map of what each block was, where it came from, and what it cost.

The harness is the prompt. Today four of the five process types are black boxes.

---

## What Exists

Verified against the source and against the live daemon (`0.5.0 (jamiepine/worker-result-delivery@49f1516c)`) on 2026-08-14.

### The snapshot store

`src/agent/prompt_snapshot.rs` — a dedicated redb at `<data_dir>/prompt_snapshots.redb`, keyed `{channel_id}:{timestamp_ms}`, holding `{user_message, system_prompt, system_prompt_chars, history, history_length}`.

It works. Enabling capture on a channel and sending a message produced a snapshot with a 46,558-char system prompt and full serialized history. The plumbing is sound; the scope is the problem.

Its doc comment claims snapshots are "broken into named sections". They never have been — `system_prompt` is one undifferentiated `String`. That comment is the whole feature request in miniature.

### The capture toggle

`prompt_capture:{channel_id}` in the settings redb, read per turn in `Channel::maybe_capture_snapshot` (`src/agent/channel.rs:4720`), written by `POST /api/channels/prompt/capture`. Opt-in, per channel, off by default.

Keying on `channel_id` is a usability trap. Probing the live instance, a portal message sent with `session_id: "prompt-inspector-probe"` created a channel whose id is `prompt-inspector-probe` — not `portal:chat:main:prompt-inspector-probe` as the sibling channels' ids suggest. The first capture attempt silently recorded nothing because the toggle was set on an id that never existed. Nothing surfaces that mismatch.

### The live inspect endpoint

`GET /api/channels/prompt/inspect` re-implements prompt assembly by hand (`src/api/channels.rs:509`) — it re-renders identity, knowledge synthesis, skills, worker capabilities and system info itself rather than calling `Channel::build_system_prompt`, which is public and documented as existing "for fixture harnesses and context inspection". Two independent assemblies that must be kept in agreement by hand. It also returns `channel_not_active` for any channel not currently resident in `ApiState::channel_states`, which is most of them.

### The UI

`interface/src/components/PromptInspectModal.tsx` — an 80vw dialog, sidebar with Current/History, and one `<pre>` of raw text. Mounted only from `ChannelDetail.tsx` behind a single icon button. No block structure, no metrics beyond a char count, no way in from anywhere else.

### What is already conceptually right

Two artifacts already carry most of the vocabulary this feature needs, and the design should reuse them rather than invent parallel terms:

- `.agents/skills/prompt-review/SKILL.md` defines a six-layer composition model (identity → channel template → dynamic fragments → knowledge → working memory → runtime state). That is the block taxonomy.
- `docs/design-docs/prompt-stability.md` defines the stable/volatile split and the named-epoch vocabulary for accepted cache misses. That is the block stability class.

### Prior art for the storage shape

`worker_runs`/`branch_runs` (`migrations/20260213000003_process_runs.sql`) and `/api/agents/processes` already give every branch and worker a durable id, kind, `process_type`, `channel_id`, model and timestamps. Those ids are the join key a prompt record hangs off.

---

## What Is Missing

**Only channels are captured.** `maybe_capture_snapshot` has exactly one call site. Branches, workers, the compactor, chronicle checkpoints and rollups, cortex bulletin/synthesis/profile runs, and ingestion all send prompts nobody can see. The worker transcript in `conversation/worker_transcript.rs` records what the worker *did*, never what it was *told*.

**Tool definitions are invisible and uncounted.** They are a real slice of every request. `Channel::run_agent_turn` says so explicitly at the budget check: *"The estimate excludes serialized tool schemas — Rig assembles those inside the `ToolServer` at call time and does not expose them."* At the channel layer that is true. At the model layer it is not — `CompletionRequest::tools` is fully populated, which `build_tools` in `src/llm/anthropic/params.rs` reads to build the wire request. Capturing lower solves a problem capturing higher cannot.

**No output, no trigger, no linkage.** A snapshot records what went out and nothing about what came back, what caused the turn, or which message it belongs to.

**No blocks.** Which is the whole point.

### The cost of having no blocks

Segmenting the captured 46.5k-char prompt by markdown heading — a lossy approximation of the real structure — already shows what the inspector is for:

| Section | Chars | ~Tokens | Share |
| --- | --- | --- | --- |
| Available Skills | 17,222 | 4,305 | 37.0% |
| Memory Store | 5,906 | 1,476 | 12.7% |
| Active Projects | 4,816 | 1,204 | 10.3% |
| Worker Types | 2,375 | 593 | 5.1% |
| When To Stay Silent | 1,517 | 379 | 3.3% |

The skills block is over a third of every request on this instance and nothing in the product says so.

Heading-splitting is not good enough to ship, which is the central design constraint: `## Available Skills` spans 191 lines that are really one fragment render, `# HUMAN.md` is a top-level heading *inside* the identity block, and a template's own prose carries no heading at all. Block boundaries are a property of assembly, not of the rendered text. They have to be recorded where they exist, not recovered where they don't.

---

## Design

### Capture at the model chokepoint

Every LLM request in Spacebot passes through `SpacebotModel::completion` or `SpacebotModel::stream` (`src/llm/model.rs:582`, `:751`). Twelve construction sites cover every process type, each already labelling itself via `with_context(agent_id, process_type)`: `channel`, `branch`, `worker`, `compactor`, `cortex`, `chronicle`, `chronicle_rollup`, `ingestion`.

That is the capture point. It sees the complete `CompletionRequest` — preamble, chat history, tool definitions, sampling params — and it sees the response. One implementation covers everything, including processes not yet written.

What it does *not* see is block structure, because by then the preamble is one `String`. So the two halves are captured separately and joined by id:

- **The request record** is written at the model layer. Byte-exact, universal, includes the response.
- **The block map** is produced by `PromptEngine` at render time, where the named parts still exist, and handed to the model explicitly.

No ambient/task-local state. `SpacebotModel` gains a builder method alongside the existing ones, set at the same call sites that already build the prompt:

```rust
let model = SpacebotModel::make(&self.deps.llm_manager, model_name)
    .with_context(&*self.deps.agent_id, "channel")
    .with_debug(DebugContext {
        process_id: self.id.to_string(),
        trigger: Trigger::UserMessage { message_id },
        assembly: assembly.clone(),
    });
```

`DebugContext` is `Option` and cheap when capture is off. A process that never sets one still gets a record — with no block map, which is honest and still far better than nothing.

### Block segmentation that cannot drift

Blocks are recorded by instrumenting the render, not by parsing the result.

`PromptEngine::render` gains a variant that wraps every injected value in sentinel control characters before rendering, then splits the output on those sentinels to recover exact byte offsets and strips them. The invariant is checkable rather than trusted:

> the sentinel render with sentinels stripped is byte-identical to the plain render

That is one assertion in one test, and it holds for conditional sections, repeated variables and empty values alike. Regions *between* sentinels are the template's own literal prose, which become blocks in their own right — the channel template's operating contract and communication rules are a third of its non-fragment mass and deserve to be visible.

Each block records:

| Field | Meaning |
| --- | --- |
| `id` | Template variable or fragment path — `identity_context`, `fragments/skills_channel` |
| `layer` | The prompt-review six: identity, contract, capabilities, knowledge, working, runtime |
| `stability` | `static` / `epoch` / `volatile`, per `prompt-stability.md` |
| `source` | template literal, identity file, store render, cortex synthesis, live state |
| `range` | Byte offsets into the captured preamble |
| `chars`, `tokens` | Size and estimate |

On cache annotation, the honest answer differs from the intuition. There is no per-block cache status to report: `build_system_prompt` in `src/llm/anthropic/params.rs` sets one `cache_control` breakpoint on the entire preamble block and one on the last tool definition. The stable/volatile split from `prompt-stability.md` is designed but not implemented — the template is still one monolithic render. So the inspector renders **breakpoint markers at their real positions** and **stability class per block**, and does not colour blocks as "cached" when the request never distinguished them. Once the template split lands, the same markers move and start telling the truth about cache boundaries for free.

Token counts are estimates. There is no tokenizer in the dependency tree; `estimate_text_tokens` is `len/4` (`src/agent/compactor.rs:501`). The real total comes back from the provider in usage, so the inspector shows the measured total for the request and scales per-block estimates against it, labelled as estimates.

### Storage

The volume is real and the user has accepted it, so the shape should suit it: index in SQLite, payload on disk.

```
<data_dir>/prompts/<YYYY-MM-DD>/<request_id>.json
```

with a `prompt_requests` index table carrying `request_id`, `agent_id`, `process_kind`, `process_id`, `channel_id`, `message_id`, `model`, `provider`, `started_at`, `duration_ms`, `input_tokens`, `output_tokens`, `cached_tokens`, `status`, `path`.

The index joins to `channels`, `worker_runs`, `branch_runs` and messages, so "every request in this session" and "the prompt for this turn" are ordinary queries. The payload is a plain JSON file: greppable, diffable between two turns, deletable by date, and readable by an agent without going through the API at all.

A retention sweeper bounds the directory by age and total size, configurable, running on the existing maintenance schedule.

The redb snapshot store is replaced, not extended. It is channel-only, has no room for blocks or output, and keeping both is two sources of truth for one question.

### Setting

One global toggle — `prompt_debug_capture` in the settings store, surfaced in Settings — following the shape `worker_log_mode` already established. Per-channel opt-in goes away; it was the wrong granularity, and the id-matching failure above shows why asking the operator to name a channel correctly is a design flaw rather than a feature.

### Reference handles

Every record has a short id. The inspector's copy button yields a line that resolves without ceremony from a terminal or an agent session:

```
spacebot prompt show 7f3a2c1e
# ~/.spacebot/agents/main/prompts/2026-08-14/7f3a2c1e.json
```

Both forms in the clipboard: the command for humans, the path for agents that would rather just read the file.

### The record

```jsonc
{
  "request_id": "7f3a2c1e",
  "process": { "kind": "worker", "id": "bfd404b0-…", "type": "builtin",
               "channel_id": "telegram:8659410676" },
  "trigger":  { "kind": "spawn_worker", "message_id": 206, "parent": "channel:telegram:…" },
  "model":    { "name": "openai-chatgpt/gpt-5.6-sol", "provider": "openai-chatgpt", "max_turns": 50 },
  "system":   { "text": "…", "blocks": [ /* id, layer, stability, source, range, tokens */ ] },
  "tools":    [ { "name": "shell", "description": "…", "schema_chars": 812 } ],
  "messages": [ /* rig Messages, verbatim */ ],
  "cache_breakpoints": [ { "target": "system", "offset": 46558 },
                         { "target": "tools",  "index": 11 } ],
  "response": { "text": "…", "tool_calls": [], "stop_reason": "end_turn" },
  "usage":    { "input": 12904, "output": 118, "cached_read": 0, "cached_write": 12904,
                "cost_usd": 0.021, "duration_ms": 3184 }
}
```

---

## Interface

A modal, as decided — a dedicated screen is not earned yet, and the record format is screen-ready if it ever is.

```
┌───────────────────────────────────────────────────────────────────────┐
│ ● worker · builtin · gpt-5.6-sol          12,904 in / 118 out · $0.021 │
│ triggered by  spawn_worker ← channel telegram:8659410676 · msg #206    │
├────────┬────────────────────────────────┬─────────────────────────────┤
│  map   │  blocks                        │  raw                        │
│        │                                │                             │
│ ▓ 37%  │ ▌ identity_context             │  # Orion                    │
│ ▓      │   identity · static · file     │  You work for Jamie Pine…   │
│ ▒      │   4,102 ch · ~1,025 tok        │                             │
│ ▒ 13%  │                                │  ## Operating Contract      │
│ ░      │ ▌ fragments/skills_channel     │  You own every request…     │
│ ░      │   capabilities · epoch          │                             │
│ ▓      │   17,222 ch · ~4,305 tok  37%  │  ─── cache breakpoint ───   │
│ ░      │                                │                             │
│ ══════ │ ═══ tool definitions (11) ═══  │  ─── messages ───           │
│ ▪▪▪▪   │ ═══ messages (2) ═══           │  [user] claude-code […]     │
│        │                                │  [assistant] OK             │
└────────┴────────────────────────────────┴─────────────────────────────┘
                                              output ▸  OK
```

The left column is the minimap: one proportional bar per block over the full height, coloured by layer, with the viewport position tracked and click-to-scroll. Blocks are proportional divs, not a canvas — the whole point is that block extents are already known exactly.

Colour encodes **layer** (six values, stable across every process type so the eye learns them once). Stability is a texture/label on the block row, not a second colour axis. Tool definitions and message history are visually distinct bands with hard dividers, so where the assembled prompt ends and the session begins is unmissable.

Two views, sharing the header: **Request**, as above, and **Session** — the index of every request in a conversation, in order, typed by process kind, so a compaction that fired between two turns is visible next to them.

### Entry points

| Where | Control |
| --- | --- |
| Channel header | 3-dot `DropdownMenu` replacing the current icon → Inspect prompt, Session index |
| Message row | 3-dot on hover / right-click → Inspect prompt at this turn |
| `ProcessDetail` panel | Open inspector — the branch/worker gap |
| Settings | Prompt debug capture toggle, retention, disk usage |

Per-turn entry from a message row is the load-bearing one. Assembly changes between turns, so the question is never "what does this channel look like" but "what did it look like *then*", and the `message_id` in the index answers it directly.

`@spacedrive/primitives` already exports `DropdownMenu*`, `ContextMenu*` and `DialogRoot`, so no new primitives are needed.

---

## Phases

1. **Capture.** `DebugContext` on `SpacebotModel`; record written from `completion` and `stream`; file store plus `prompt_requests` index; global setting; retention sweeper. Every process type is captured from this phase on, block map or not.
2. **Blocks.** Sentinel instrumentation in `PromptEngine`, the byte-identity test, block classification by layer/stability/source, cache breakpoint positions from the Anthropic request builder.
3. **API and handles.** Request fetch, session index, per-message lookup; `spacebot prompt show` and `spacebot prompt diff`; copy-reference payload.
4. **Inspector.** The modal — map, blocks, raw, metadata, session view.
5. **Entry points.** Channel menu, message menu, `ProcessDetail` button, settings panel.
6. **Cleanups.** Delete the hand-rolled assembly in `inspect_prompt` and point live inspection at `Channel::build_system_prompt`; remove the redb snapshot store and its per-channel toggle.

Phases 1 and 2 are independently useful and phase 1 does not block on 2 — a record with no block map still opens four process types that are invisible today.

## Follow-on

`prompt-stability.md` phase 4 wants a quiet-turn byte-diff regression test and names the redb snapshot store as its source. This record replaces that source and improves on it: two consecutive requests can be diffed block by block, so a stability regression reports *which block moved* rather than that some byte did. Worth landing the two together.
