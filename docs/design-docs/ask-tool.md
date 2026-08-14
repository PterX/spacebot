# Ask Tool: Questions with Selectable Answers

Agents frequently need a decision from the user: which environment, which of three approaches, proceed or not. Today the only way to do that is prose ("reply 1 or 2"), or hand-rolling `reply` with `interactive_elements` — which renders buttons on Discord only, and when the user clicks one, the agent receives a bare `[interaction: custom_id → label]` breadcrumb with no link back to the question that was asked.

This doc adds an `ask` tool: a first-class question primitive with selectable options, rendered natively on every channel that supports buttons, with a numbered-text fallback everywhere else, and answer correlation so the model sees "James answered *Which environment?* → staging" instead of an interaction breadcrumb.

## Current state

The transport is mostly built; the semantics are missing.

**Outbound.** `ReplyArgs` already accepts `interactive_elements` (buttons, select menus) and `poll` (`src/tools/reply.rs:97-118`), carried by `OutboundResponse::RichMessage` (`src/lib.rs:729`). The generic types — `InteractiveElements`, `Button`, `SelectMenu`, `SelectOption` — live at `src/lib.rs:1002-1046`.

**Inbound.** `MessageContent::Interaction` (`src/lib.rs:611-623`) carries `action_id`, `values`, and `label` back from a click.

**Per-channel reality:**

| Channel | Outbound buttons | Inbound clicks |
|---|---|---|
| Discord | Full: ActionRows, embeds, native polls (`src/messaging/discord.rs:1088-1245`) | `interaction_create` → `Interaction` (`discord.rs:678-780`) |
| Slack | Broken: adapter forwards `blocks` only, and `reply.rs:485` hardcodes `blocks: vec![]`, so buttons are silently dropped | `block_actions` → `Interaction` (`slack.rs:666-688`) |
| Telegram | None: no `InlineKeyboardMarkup`/`reply_markup` anywhere; native polls only | None: no `callback_query` handler |
| Portal (web UI) | None: `forward_sse_event` (`src/main.rs:148-159`) collapses `RichMessage` to text before the SSE bus; rich payloads aren't persisted in conversation history | None: `PortalSendRequest` is text + attachments only (`src/api/portal.rs:20-30`) |
| Mattermost | No `RichMessage` arm at all | None |
| Twitch / Signal / Email / Webhook | Text fallback only | None |

**Agent side.** Interactions are flattened to their `Display` form before the LLM sees them (`src/agent/channel.rs:2120-2131`) — `[interaction: {action_id} → {label}]`. Nothing correlates a click to the question it answers, and nothing tracks that a question is pending.

**Prior design.** `docs/design-docs/agent-factory.md` Phase 4 ("Structured Message Types") specs a `StructuredKind::Buttons` for the portal, unimplemented. This doc subsumes that variant: the ask tool is the question primitive, and the portal rendering work below is the same work Phase 4 needs. `SelectCards`/`Progress`/`Summary` remain factory-scoped and out of scope here.

## Design decision: non-blocking

Two possible models:

1. **Blocking** — the tool call parks until the user answers (or a timeout fires), and the answer arrives as the tool result. Great mid-task UX: the agent continues with everything in context.
2. **Non-blocking** — the tool sends the question and returns; the turn ends; the answer arrives later as a new inbound message, enriched with the question context.

We go non-blocking. Spacebot turns are cheap and the click already arrives as an inbound message on Discord/Slack. Blocking would hold a channel turn open for potentially hours, interacts badly with message coalescing and turn cancellation, and a process restart would orphan the parked tool call. A pending question in a store survives restart for free — the click is just a message that arrives whenever it arrives.

The tradeoff is that the model must re-orient when the answer comes in. Enrichment (below) closes most of that gap: the answer message carries the original question text, so the model doesn't need the asking turn in recent context to interpret it.

One consequence worth stating: there is no "Other (type your answer)" option, because the chat input is always available. A typed reply remains a normal inbound message: it does not resolve or correlate a pending question automatically. The model can interpret it from the transcript, but only a matching interaction resolves the stored question.

## The tool

```rust
/// Arguments for ask tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskArgs {
    /// The question to ask.
    pub question: String,
    /// Selectable answers. 2 to 10 options.
    pub options: Vec<AskOption>,
    /// Allow picking more than one option. Renders as a select menu
    /// with multi-select where supported.
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskOption {
    /// Short label shown on the button (keep under ~40 chars).
    pub label: String,
    /// Optional longer description, shown where the platform supports it
    /// (select menu descriptions, portal UI) and in the text fallback.
    #[serde(default)]
    pub description: Option<String>,
}
```

Output returns `question_id` and a reminder that the answer arrives as a future message. The tool description (new `prompts/en/tools/ask_description.md.j2`) instructs the model to end its turn after asking rather than speculating about the answer.

`ask` desugars into the existing `RichMessage` path — it builds `InteractiveElements` from the options and sends through `RoutedSender` like `reply` does. No new `OutboundResponse` variant, so no adapter `match` arms or `broadcast_variant_name` changes.

Rendering rules:

- ≤ 5 options and no `multi_select` → buttons, one `custom_id` per option.
- \> 5 options or `multi_select` → select menu (Discord allows 25 options; multi-select uses `min_values`/`max_values`).
- Every channel gets the question plus a numbered option list in the message text — this *is* the message on text-only channels, and on button channels it means a typed "2" is still interpretable from transcript context.

`custom_id` scheme: `ask:{question_id}:{idx}`, where `question_id` is a short random id. Discord caps `custom_id` at 100 chars and Telegram caps `callback_data` at 64 bytes, so option values never go in the id — the index is resolved against the stored question.

## Pending question store

A sqlite table, same shape philosophy as `NotificationStore` (`src/notifications.rs`):

```sql
CREATE TABLE pending_questions (
    question_id  TEXT PRIMARY KEY,
    agent_id     TEXT NOT NULL,
    channel_id   TEXT NOT NULL,
    question     TEXT NOT NULL,
    options      TEXT NOT NULL,   -- JSON array of AskOption
    message_ref  TEXT,            -- platform message id, for disabling buttons after answer
    created_at   INTEGER NOT NULL,
    resolved_at  INTEGER,
    answer       TEXT             -- JSON array of picked labels
);
```

Written when `ask` sends, resolved when an answer lands. Restart-safe by construction. Resolved and aged-out rows are pruned on write (questions older than a TTL, default 7 days, are treated as expired on lookup).

## Answer correlation

At the flattening point in `src/agent/channel.rs:2120-2131`, an `Interaction` whose `action_id` parses as `ask:{question_id}:{idx}`:

1. Looks up the pending question. Miss or already-resolved → render as today's breadcrumb with an `(expired)` note, so late clicks don't false-resolve anything.
2. Hit → atomically mark resolved. Only the interaction that wins that update renders for the LLM as:

   ```
   {sender} answered "{question}": {label}
   ```

3. Best-effort, tell the adapter to disable/strip the buttons on the original message (edit components on Discord, `editMessageReplyMarkup` on Telegram, block replacement on Slack) so stale questions don't invite double-answers. A second click on an already-resolved question gets the expired breadcrumb, which the model can acknowledge or ignore.

Interactions that don't match the `ask:` prefix keep the current breadcrumb behavior — `reply`'s raw `interactive_elements` stay available for free-form use.

## Phases

### Phase 1: Core tool and correlation

- `src/tools/ask.rs` — `AskArgs`/`AskOption`, tool impl building `RichMessage`, store write.
- `src/questions.rs` (or module under `src/agent/`) — pending question store.
- Registration in `src/tools.rs` (`add_channel_tools` and `add_direct_mode_tools` both), module exports, `prompts/en/tools/ask_description.md.j2`, template registration in `src/prompts/text.rs`.
- Enrichment at the `channel.rs` flattening point.
- Numbered text fallback verified on a text-only channel (Signal or email).

Discord works end to end at the close of this phase with no adapter changes.

### Phase 2: Telegram

- `InlineKeyboardMarkup` rendering in the `RichMessage` arm of `src/messaging/telegram.rs` — full option text in the message body, short labels on the buttons (Telegram truncates button labels aggressively on mobile), `callback_data` = the `custom_id`.
- `callback_query` branch in the update loop producing `MessageContent::Interaction`, plus `answerCallbackQuery` ack so the client spinner clears.
- Strip the keyboard via `editMessageReplyMarkup` once answered.

### Phase 3: Slack

- Synthesize Block Kit `actions` blocks from `interactive_elements` in the Slack adapter's `RichMessage` arm (adapter-side synthesis, not in `reply.rs` — the tool stays platform-agnostic and `blocks: vec![]` stops mattering).
- Inbound `block_actions` already produces `Interaction`; verify the `action_id` round trip and replace blocks on resolution.

### Phase 4: Portal

- `ApiEvent::OutboundMessage` gains an optional `interactive` field (the `InteractiveElements` JSON plus `question_id` when the message is an ask); `forward_sse_event` stops collapsing it.
- Persist the payload: optional structured column on `TimelineItem::Message` (`src/conversation/history.rs`) written by `ConversationLogger` at the `reply`/`ask` log site, including resolution state — so questions re-render correctly on reload, answered ones disabled with the pick shown.
- `PortalSendRequest` gains an interaction variant so a portal click arrives as `MessageContent::Interaction` like Discord and Slack, hitting the same enrichment path. No new endpoint.
- `interface/`: a button-group component rendered as a sibling of `MessageBubble` in `PortalTimeline.tsx` (the `@spacedrive/ai` bubble has no children slot); click posts the interaction, disables the group, and shows the selected state.

### Phase 5: Parity and docs

- Run the `messaging-adapter-parity` checklist: every adapter either renders options natively or degrades to the numbered text fallback predictably; Mattermost needs at least a `RichMessage` text arm.
- Capability matrix entry in `docs/content/docs/(messaging)/` for interactive questions.
- Tests: store resolution and expiry, enrichment rendering (hit, miss, double-click), Telegram callback parsing, portal round trip.

## Open questions

1. **Multi-select scope.** Native multi-select exists on Discord select menus and the portal; Telegram has no multi-select inline keyboard. Ship `multi_select` in Phase 1 with select-menu channels only, or hold it until someone asks?
2. **Group-chat answering.** Should anyone in a group channel be able to answer, or only the person the agent was talking to? Proposal: anyone, and the enrichment names who answered — the model can decide whether that's authoritative.
3. **Cron and broadcast asks.** `cron` deliveries support cards today. Letting scheduled deliveries ask questions ("standup: pick your status") falls out almost for free but multiplies pending-question volume per channel. Defer?
4. **Reminder nudges.** A pending question older than some threshold could surface in the agent's briefing so it can decide to re-ask or proceed on judgment. Cheap to add once the store exists; not needed for v1.
