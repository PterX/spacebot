# Prompt Stability

Byte-stable prompt prefixes. A turn's request must share the longest possible byte-identical prefix with the previous turn's request in the same channel, so provider prompt caches actually hit. Every byte above the last cache breakpoint must be a pure function of durable state — bytes change when facts change, never because the clock moved.

This doc defines the stability invariant and the rendering changes that establish it. For the transcript-side invariant (history that never rewrites sent bytes), see [`durable-transcript.md`](durable-transcript.md). For the accounting that measures it, see [`token-usage-tracking.md`](token-usage-tracking.md).

---

## Why This Exists

Provider prompt caching bills cached input at roughly a tenth of list price, and a long-lived channel re-sends its entire past on every turn. Spacebot already has cache machinery — `src/llm/anthropic/cache.rs` resolves a retention tier and `build_anthropic_request` (`src/llm/anthropic/params.rs`) sets `cache_control` breakpoints on the system preamble and the last tool definition — but the bytes above those breakpoints never repeat:

- The status block renders `Time: {line}` from `current_time_line()` (`src/agent/channel_prompt.rs`) at second resolution, inside the single system text block. Both system breakpoints are invalidated on every turn, unconditionally. The realistic cache hit rate on the system prompt today is zero.
- Conversation history carries no `cache_control` at all — `convert_messages_to_anthropic` (`src/llm/model.rs`) attaches none, so on a channel with substantial history the entire message array is re-billed as uncached input every turn. This is the dominant token cost of a busy channel.
- The channel activity map and participant context render relative ages (`format_time_ago`: "5m ago", "2h ago") recomputed from the current time, so those sections churn even when nothing happened.
- Tool registration is order-sensitive by construction (rig's `ToolServer` keeps insertion order; channel tools are added and removed around each turn in `src/tools.rs`), so conditional registrations reshuffle the tools array and silently invalidate the tools breakpoint.

None of this is a provider problem. It is a rendering discipline problem, and it compounds: implicit prefix caching on OpenAI-compatible providers depends on the same byte stability without any explicit breakpoint to inspect, so instability silently forfeits the discount everywhere at once.

---

## The Invariant

```text
stable prefix                      volatile suffix
─────────────                      ───────────────
identity · adapter · skills        working memory
worker capabilities · channels     channel activity map
org / link / project context       participant context
tool definitions                   status block · time
        ▲                          coalesce hint
        │                                  ▲
        └── changes only on epoch          └── may change every turn,
            (config, skills, model)            sits below the last
                                               cache breakpoint
```

Two rules, checked in CI rather than remembered:

1. **Above the last breakpoint, bytes are a pure function of durable configuration.** Identity, skills, capabilities, org context. These change when an operator or the agent changes them — a deliberate, logged event — never as a side effect of rendering.
2. **Anything derived from `Utc::now()` renders below the last breakpoint or inside the current user message.** The clock is the canonical volatile input; nothing above the line may observe it.

A deliberate full miss is an **epoch**: a named event (config change, skill edit, model switch, compaction, restart) after which the prefix is expected to differ. Epochs are logged with a reason. A prefix diff outside an epoch is a bug.

---

## Rendering Changes

### Template split

`prompts/en/channel.md.j2` is one monolithic render with ~18 optional sections, volatile and stable interleaved. It splits into a stable region and a volatile region along the table above. The engine (`PromptEngine::render_channel_prompt_with_links`, `src/prompts/engine.rs`) renders both regions and returns them separately; the provider layer places the breakpoint between them instead of decorating one undifferentiated block. `maybe_append_tool_use_enforcement` appends to the volatile region only.

Sections that move to the volatile region: working memory, channel activity map, participant context, memory bulletin, knowledge synthesis, conversation context, status block, coalesce hint, backfill transcript. The bulletin and synthesis blocks are fact-driven (the cortex publishes them when it has something new), but they publish at arbitrary times from a background process, which makes them volatile from the cache's point of view — they live below the line.

### Time quarantine

- The `Time:` line leaves the system prompt entirely and renders into the current user message envelope. History messages already bake absolute timestamps at insert time (`format_user_message` in `src/agent/channel_history.rs`), so the model keeps full temporal grounding; the only casualty is a clock line that was stale by mid-turn anyway.
- `format_time_ago` relative ages in the activity map and participant context are replaced with absolute timestamps computed once at event time. Relative phrasing is a presentation nicety that costs a full re-render of those sections on every turn; absolute timestamps are stable bytes the model reads equally well.
- Process timestamps in the status block (`started_at.format("%H:%M:%S")`) are already absolute; they stay, and the status block is volatile-region regardless.

### Tool order pinning

Tool definitions are part of the cached prefix (the tools breakpoint precedes the system block in request order). Registration becomes deterministic: a fixed ordering (registration category, then name) applied when the request is built, not inherited from insertion order. Conditional tools — `allow_direct_reply`, delegation-mode variants, optional cron and messaging tools — still appear and disappear, but only when their governing configuration changes, which is an epoch. MCP tool-list changes are likewise epochs, observed at reconnect.

### History breakpoints

`convert_messages_to_anthropic` gains rolling breakpoints: `cache_control` on the final message block and the block a fixed distance behind it. Consecutive turns then read the shared prefix from cache and write only the tail. This is the standard rolling-window pattern; the reason it has not been worth doing until now is that the mutations described in [`durable-transcript.md`](durable-transcript.md) rewrite sent bytes, and a rewritten prefix makes history breakpoints pointless. The two docs land together: this one makes stability cheap to keep, that one makes it true.

OpenAI-compatible providers need no request changes — implicit prefix caching picks up the same byte stability automatically. The provider matrix work is Anthropic-only.

### Cache retention configuration

`CacheRetention` is currently resolved from the `PI_CACHE_RETENTION` env var alone. It becomes a real config field with the env var as an override, following the existing config precedence conventions. Long retention emits a 1h TTL on `api.anthropic.com` as today.

---

## Epochs

The accepted-miss vocabulary, exhaustively:

| Epoch | Trigger site | Expected diff |
| --- | --- | --- |
| `config` | agent/channel settings change | stable region |
| `skills` | skill add/edit/retire | stable region |
| `tools` | conditional or MCP tool set change | tools block |
| `model` | model or provider switch | whole request |
| `compaction` | transcript head swap ([`durable-transcript.md`](durable-transcript.md)) | history head |
| `restart` | process restart, until transcript rehydration lands | whole request |

Each epoch increments a per-channel counter recorded alongside usage rows, so a cache-miss spike is attributable to a named event or flagged as a regression.

---

## Measurement

The telemetry already exists. `src/llm/usage.rs` normalizes `cache_read_input_tokens` / `cache_creation_input_tokens` (and the OpenAI-compatible equivalents) into `cache_read_tokens` / `cache_write_tokens` and flushes per-turn rows to SQLite; `src/llm/pricing.rs` prices cached reads and writes separately. The shipped metric is per-channel cache hit rate — cached read tokens over total input tokens — surfaced next to the existing spend numbers.

The regression test uses `src/agent/prompt_snapshot.rs`, which already captures per-turn `{system_prompt, history}` behind `prompt_capture_enabled`:

- **Quiet-turn byte diff.** Two consecutive turns in a channel with no intervening activity must produce byte-identical stable regions and tool arrays. Any diff is printed and fails the test.
- **Restart byte diff.** Once transcript rehydration exists, a captured turn replayed after restart must produce a byte-identical request. Until then this test documents the `restart` epoch instead of asserting identity.

---

## Phases

1. **Time quarantine and template split.** Move the clock and the relative-age strings; split `channel.md.j2` and thread the two-region render through the engine and the Anthropic request builder. This alone takes the system-prompt hit rate from zero to near-total on quiet turns.
2. **Tool order pinning and retention config.** Deterministic tool ordering at request build; `CacheRetention` into config.
3. **History breakpoints.** Rolling `cache_control` in `convert_messages_to_anthropic`, landed with the append-only invariant from [`durable-transcript.md`](durable-transcript.md).
4. **Epoch counter and CI.** Epoch logging on the trigger sites, the hit-rate query, and the quiet-turn byte-diff test over prompt snapshots.
