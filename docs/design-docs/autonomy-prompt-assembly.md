# Autonomy Prompt Assembly

The autonomy channel is one persistent channel that wakes, orients itself,
works, records an outcome, and exits. Its prompt must distinguish standing
policy from the state of the current run. Today that distinction is inverted:
almost all autonomy policy and context are rendered into a synthetic first user
message, while the normal channel system prompt retains instructions for
talking to people.

This design gives `ChannelKind::Autonomy` a dedicated system-prompt assembly
path. It reuses the normal channel's durable context providers and chronicle
storage. It does not create a second autonomy runtime or a second chronicle
model. Each run begins with one tool-free model turn that writes a short wake
orientation before the normal tool-enabled loop starts. The portal represents
the start as a run divider rather than exposing the synthetic trigger.

## Current State

`Channel::build_system_prompt()` renders the normal `channel.md.j2` preamble
for every channel kind. An autonomy run then calls
`build_run_briefing()` in `src/agent/autonomy.rs`, which renders the entire
`prompts/en/autonomy_channel.md.j2` template. That output becomes the first
synthetic `InboundMessage` for channel id `autonomy`.

The template currently contains all of the following:

- Identity and no-human semantics.
- Wake events and recent run summaries.
- Task state, goals, and active workers.
- Autonomy level permissions and task approval rules.
- Empty-instance guidance.
- Memory guidance, task budget, wrap-up behavior, and the
  `autonomy_complete` contract.

This has five consequences.

1. Stable policy is stored repeatedly in the conversation history.
2. Run context is represented as user input even though no participant sent it.
3. The normal preamble tells an autonomy channel how to communicate with people,
   when it should stay silent, and how adapter delivery works.
4. Prompt inspection does not show autonomy context in the structured system
   prompt blocks used by normal channels.
5. The portal renders the full briefing as a system message even though it is
   runtime scaffolding, not part of the conversation.

The existing design already specifies the intended behavior. `autonomy.md`
states that the briefing belongs in the system prompt and that the transcript
holds the agent's output. The implementation has not reached that architecture.

## Prompt Layers

An autonomy run has three layers.

### Standing policy

Standing policy is rendered into the system prompt. It includes:

- Autonomy identity and the fact that no human is present.
- The autonomy level and allowed tool authority.
- The approval boundary: `pending_approval` tasks may be enriched but never
  executed; only `ready` tasks may execute at `act` level.
- Restrictions on user messaging, cron creation, identity changes, and config
  changes.
- Task budget and wrap-up behavior.
- Memory-saving guidance.
- The required `autonomy_complete` terminal contract.
- Empty-instance behavior.

This content changes only when configuration changes. It must not accumulate in
the channel transcript.

### Volatile context

Volatile context is also rendered into the system prompt, inside a clearly
labeled region rebuilt for every run:

- Wake events and payloads.
- Recent autonomy run summaries.
- Current task state.
- Active goals.
- Workers still running, including owner, lifecycle, last progress, and
  recovery state.
- Worker outcomes delivered since the prior autonomy run, including the bound
  task, terminal outcome, bounded result, and transcript reference.
- The autonomy channel's own chronicle view.
- The configured home channel's chronicle view.

These are database-backed observations. They are context, not instructions from
a participant. Wake instructions are data from an allowed wake definition and
remain explicitly labelled as such.

Worker continuity uses a durable outcome inbox, not channel history or an
in-memory status block. An autonomy channel may exit while its workers continue
under supervisor ownership. If one completes before the next wake, it is no
longer an active worker and the original channel cannot process its completion
event. The next run must therefore claim and render its unacknowledged terminal
outcome from durable state. This remains separate from the live completion
retrigger used while the current channel is still resident.

### Ephemeral trigger

The run uses two short, non-persisted synthetic messages. The first starts the
tool-free orientation turn:

```
Orient yourself for this autonomy run using the current system context. Respond
with a concise plain-text account of what woke you, what matters now, and what
you intend to do first. Do not claim that you have taken any action.
```

The second starts the normal tool-enabled loop after the orientation has been
recorded:

```
Proceed with the autonomy run using that orientation.
```

Their sole purpose is to advance the run between phases. Soft wrap-up and
hard-timeout notices remain ephemeral mid-run messages. These are one-run
control signals and must not become transcript history.

### Wake orientation

The first model call is a single tool-free turn. It receives the specialized
autonomy system prompt and the first ephemeral trigger, but no tool definitions.
Its output is a short assistant message visible in the autonomy transcript.

The orientation is operational text, not hidden chain-of-thought. It states the
wake cause, current priority, known constraints, and intended first action. It
must not contain private reasoning traces, tool-call syntax, or claims about
work that has not happened. For example:

```
Three task approval events triggered this wake. The newly approved migration
task is the highest-priority executable work, but an active worker may already
cover part of it. I will check ownership first, then execute it or record the
blocker.
```

Build the orientation agent separately from the channel's normal agent. It has
`max_turns = 1` and no `ToolServer` handle. Tools must not be temporarily removed
from a shared server because another path could register them before the request
is assembled. The absence of a tool server is the authority boundary for this
phase.

The orientation call does not use model-specific tool-use enforcement, malformed
tool-syntax recovery, worker outcome nudging, or the `autonomy_complete`
contract. It does count toward the run's wall-clock timeout and usage accounting,
but not the configured tool-enabled turn budget. Provider transport retries may
retry the same request. The runtime does not issue a second semantic orientation
prompt.

If the call fails or returns empty text, mark the run failed before any tools are
registered. A run cannot enter its action phase without a completed orientation.

## Specialized System Prompt

`Channel::build_system_prompt()` branches on `ChannelKind::Autonomy` before it
calls `PromptEngine::render_channel_prompt_with_links()`.

Normal user and cron channels keep their existing preamble composition. The
autonomy path calls a new `PromptEngine::render_autonomy_system_prompt(...)`.
It composes the reusable context layers that remain meaningful without a human:

- Identity context.
- Knowledge synthesis and memory store.
- Skills and worker capabilities.
- Project context.
- Working memory.
- Organization context.
- System status.
- Execution policy appropriate to direct autonomy work.
- Autonomy standing policy.
- Current autonomy run context.

It deliberately omits normal-channel sections that assume a human participant:

- Communication and reply wording.
- Silence and mention rules.
- Participant context.
- Adapter-specific delivery guidance.
- Available-channel prose intended for a conversational sender.
- User-channel authority wording that depends on the current sender.

The autonomy template must state its own bounded delivery rule. It has no
`reply` tool and does not speak to a human as part of a run. Any future allowed
outward action needs its own explicit policy and durable provenance.

## Chronicle Context

The autonomy channel already has a normal channel id, `autonomy`. Its own
chronicle is rendered through the existing
`agent::chronicle::render_chronicle_view()` call using that channel id. It has
the same checkpoint storage, rollups, truncation behavior, and token budget as
every other channel chronicle.

The autonomy prompt also receives a second chronicle block when the agent has
an explicit home channel.

### Autonomy chronicle

This block answers: what has autonomous work attempted, decided, completed, or
left blocked over time?

It is the normal chronicle for channel id `autonomy`. Terminal
`autonomy_complete` summaries are assistant transcript entries, so checkpoint
summaries naturally compact completed runs as the channel ages.

### Home channel chronicle

This block answers: what has the user recently asked for, decided, corrected,
or deprioritized?

Resolve the home target through `SettingsStore::home_channel()`. The setting is
explicit and instance-scoped. Map its canonical target to the corresponding
conversation id using the same target-routing normalization used by messaging.
Do not infer a home channel from recent activity.

Render a home chronicle only when all conditions hold:

- A home channel is configured.
- The target resolves to a known conversation id.
- It is not the `autonomy` channel itself.
- It has a non-empty chronicle view.

Otherwise omit the block. Log resolution failures with the agent id and target,
without failing the run.

The two blocks stay separate:

```
## Autonomy Chronicle
...

## Home Channel Chronicle
...
```

They serve different questions and must not be flattened into a cross-channel
transcript dump.

## Budgets

Chronicle views are bounded by `ChronicleConfig.context_token_budget`. Rendering
both views with the full channel budget would double the intended allowance.

The autonomy prompt has an explicit context allocation:

- Autonomy chronicle gets a bounded allocation.
- Home chronicle gets a separate bounded allocation.
- Recent run summaries use a newest-first character budget.
- Task state retains its existing per-description truncation and has a total
  block budget.
- Wake events, goals, active workers, working memory, and system status remain
  independently bounded.

The assembly order preserves the information most useful for a live decision:

1. Wake events.
2. Active workers and blocked/ready task state.
3. Home channel chronicle.
4. Autonomy chronicle.
5. Recent run summaries.

Each source reports omission rather than silently dropping context. Chronicle
views already collapse and remove oldest entries under their budget. Recent run
history needs the character-budgeted newest-first policy tracked by task `#7`.

The whole rendered system prompt remains subject to the existing request-size
estimate. Tool schemas remain outside that estimate because Rig assembles them
at call time.

## Transcript Rules

The autonomy transcript records the agent's output, not repeatedly rendered
system context.

- Neither run trigger is persisted.
- Wake briefings, task surveys, goals, and run-history windows are not
  persisted as messages.
- Soft and hard timeout notices are not persisted.
- A successful orientation writes exactly one assistant message before tools
  become available.
- The first durable terminal transition writes exactly one assistant summary to
  the autonomy transcript.
- External effects that the next run cannot re-derive are promoted into the
  terminal summary or a durable journal entry with explicit provenance.

The live in-memory sequence for a successful run is:

```
user      ephemeral orientation trigger
assistant durable wake orientation
user      ephemeral action trigger
assistant/tool activity
assistant durable terminal summary
```

On the next wake, history hydration restores assistant output only. The two
synthetic user messages are live role boundaries for their run and do not enter
the durable transcript or chronicle.

The terminal run row is the idempotency boundary. Only the caller that changes
an autonomy run from `running` to a terminal state may publish the transcript
summary. Completion retries, timeout races, and late worker arrival paths must
not append duplicate outcomes.

The orientation has a deterministic message id derived from the run id, for
example `autonomy-orientation:{run_id}`. Persist it with `INSERT OR IGNORE`
against the existing message primary key. A process retry reads and reuses the
existing orientation or fails the run, never appends another one.

## Portal Timeline

The autonomy channel displays the start of each run as a lifecycle divider:

```
──────── System wake 09:42 ────────
```

The divider is not a conversation message. Its durable source is the
`autonomy_runs` row, using the run id as the timeline item id and `started_at` as
its timestamp. The channel timeline API exposes it as a typed item such as
`autonomy_wake`; it must not synthesize the divider by inspecting message text.
The live event path emits the same typed item when `begin_run()` succeeds so the
portal does not wait for a history refresh.

The divider contains no briefing payload. Wake names and payloads remain in the
volatile system context, while run provenance remains queryable through the
run's `wake_event_ids`. A later expandable run inspector may resolve those ids,
but the channel timeline stays concise by default.

Existing autonomy system messages are historical briefing rows. The portal
renders those rows as wake dividers for compatibility and never renders their
content. New runs stop creating them. The history API should omit their raw
content from autonomy timeline responses so UI hiding does not leave the
briefing exposed over the network.

Autonomy timeline queries and live state must be scoped by both agent id and
channel id. The fixed `autonomy` conversation id is shared by agents and cannot
identify an agent's run on its own.

## Templates And APIs

Replace the overloaded `autonomy_channel.md.j2` with:

- `prompts/en/autonomy_system.md.j2` for standing policy and volatile context
  blocks.
- `prompts/en/autonomy_orientation.md.j2` for the tool-free orientation trigger.
- `prompts/en/autonomy_run.md.j2` for the action-phase trigger.

`PromptEngine` gains:

```rust
pub fn render_autonomy_system_prompt(
    &self,
    shared: AutonomySharedPromptContext,
    run: AutonomyRunPromptContext,
) -> Result<String>;

pub fn render_autonomy_orientation_trigger(&self) -> Result<String>;

pub fn render_autonomy_run_trigger(&self) -> Result<String>;
```

The context structs make ownership and budgets explicit. They should carry
already-rendered bounded blocks rather than giving templates database access.

`Channel::build_system_prompt()` owns shared context assembly. Autonomy-specific
run context is collected through a narrow helper in `agent::autonomy`, then
passed to the channel at construction or through a per-run state handle. Do not
make `Channel` query autonomy stores on behalf of user channels.

The run driver starts the orientation before `run_agent_turn()` registers direct
mode tools. The resulting assistant text is inserted into live history and
persisted through an orientation-specific logger. The action trigger then enters
the existing channel loop. Do not route orientation output through normal reply
handling because autonomy has no delivery target and plain text is otherwise
treated as an incomplete tool-enabled turn.

The timeline API gains a typed autonomy wake item assembled from
`autonomy_runs`. The corresponding process event and SSE payload carry the run
id, agent id, channel id, and start timestamp. They never carry the rendered
briefing.

## Migration

The change does not require a schema migration. Chronicle checkpoints, channel
history, home-channel settings, task state, run records, and the conversation
message primary key already provide the required storage and idempotency.

Existing autonomy system rows remain represented in the portal timeline as
historic wake dividers. Their briefing content is not returned or rendered.
They are not rehydrated as current briefing input. Future runs stop adding them.

Existing run summaries remain in `autonomy_runs` and in the autonomy transcript
where terminal publication already wrote them. The recent-runs context renderer
continues to read the store until the character-budgeted continuity policy
replaces it.

## Verification

### Prompt rendering

- User and cron channel preambles remain unchanged.
- Autonomy preambles omit communication, silence, participant, and adapter
  guidance.
- `observe`, `suggest`, and `act` each render the correct authority contract.
- Wake events, task state, goals, active workers, and recent runs appear in
  separate autonomy system-prompt blocks.
- Prompt inspection returns the specialized autonomy preamble.

### Chronicle selection

- The autonomy block uses the same rendered chronicle view as a normal channel
  with id `autonomy`.
- An explicit home channel adds a second bounded view.
- No configured home channel, an unresolved target, an empty view, and
  `home == autonomy` omit the second block.
- Each block remains inside its allocated budget and reports omitted entries.

### Lifecycle

- Starting an autonomy run persists no briefing message.
- The orientation request contains no tool definitions and permits exactly one
  plain-text model response.
- No branch, worker, memory, shell, file, browser, or completion tool can run
  before the orientation is recorded.
- A successful orientation persists once and appears between the wake divider
  and tool activity.
- Orientation retries and restart recovery do not duplicate its assistant row.
- An empty or failed orientation marks the run failed without registering
  tools.
- A successful completion, timeout, and failure each produce one terminal
  assistant transcript entry when they win the `running -> terminal`
  transition.
- Duplicate completion, timeout/completion races, and retries produce no
  duplicate outcome entry.
- A restart rebuilds both chronicle blocks from durable state.

### Portal timeline

- A new run appears as a typed `System wake {time}` divider in history and over
  the live event stream.
- The divider timestamp comes from `autonomy_runs.started_at`, not browser
  receipt time.
- New runs do not persist or transmit the rendered briefing as a message.
- Historical autonomy briefing rows render as dividers without exposing their
  content.
- Two agents can view their autonomy timelines without sharing runs or live
  events.

### Gates

Run focused prompt, chronicle, home-channel resolution, and autonomy lifecycle
tests. Then run `cargo fmt --all`, `cargo check --lib`, interface type checks
and `bun run build`, followed by `just preflight` and `just gate-pr`.
