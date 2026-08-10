# Home Channel

The home channel is an instance's default outbound destination — the one conversation the agent can reach when no conversation is in scope. It is set once, from the chat that should receive it, and it is what makes autonomous outreach possible at all.

This doc defines the home channel and the delivery resolution that consumes it. For the trigger model that produces autonomous work, see [`wakes.md`](wakes.md). For the channel that runs it, see [`autonomy.md`](autonomy.md).

---

## Why This Exists

Spacebot has three delivery situations and only two of them are solved.

A user channel replies to whoever spoke — the target is the inbound message. A cron job delivers to the conversation it was created in: `default_delivery_target_for_conversation` (`src/tools.rs`) derives the target from the originating `conversation_id`. Both work because a conversation is in scope.

An autonomy run has neither. There is no inbound message, no originating conversation, and no `reply` tool — the tool is gated behind `allow_direct_reply`, and [`autonomy.md`](autonomy.md) lists replying to users under what the channel deliberately cannot do. The channel is structurally mute.

The gap is already visible in the schema. `WakeDef.delivery_target` is validated at config load (`src/wakes/config.rs` checks it parses as `adapter:target`), persisted, and round-tripped through the store (`src/wakes/defs.rs`) — and read by nothing. The field records the intent to deliver; what stopped it is that there is no sensible default behind it. A wake that wants to say something has nowhere to say it unless every wake is individually configured with a target, which is not a default anyone will set.

Every proactive behavior worth having — onboarding, digests, "I looked at that repo and found something" — needs one answer to "where does this go?".

---

## The Model

```text
autonomous send
      │
      ├── wake's delivery_target ──▶ send there
      ├── home channel ────────────▶ send there
      └── neither ─────────────────▶ record, don't send
```

An explicit `delivery_target` on the wake wins: a CI-failure wake can route to an engineering channel while everything else goes home. Otherwise the home channel receives it. If neither is set, the run records what it wanted to say as a memory and continues.

That third branch is load-bearing. An autonomous run must never fail because it had nothing to say something to, and it must never fall back to "the most recent channel we saw" — that is how an agent posts a private observation into a group. Unset means silent, and silence degrades into memory rather than into a guess.

---

## Addressing

The addressing layer already exists and does not need extending.

`parse_delivery_target` (`src/messaging/target.rs`) handles `adapter:target` and `adapter:instance:target`, including Signal's extra instance segment and named telegram/discord/slack instances. `resolve_broadcast_target(&ChannelInfo)` turns a live channel into a `BroadcastTarget { adapter, target }`, which `Display`s back into the same canonical string.

So setting the home channel is: resolve the current channel, store the string. Reading it is: parse the string, broadcast. One fully-qualified string covers every adapter.

**One home per instance, not one per adapter.** Per-adapter homes force the autonomy channel to choose which one to use on every send, and there is no principled answer to that question — the run has no adapter context to choose with. Goals and autonomy runs are instance-scoped; their outreach is too.

---

## Setting It

The intent is expressible in a sentence, so the primary path is a tool:

- **`set_home_channel`** — the model calls it when the user says "make this your home channel". Registered on user channels only, and it resolves the channel it was called from rather than taking a target argument.

The command is a second entry point to the same handler, not a second implementation:

```rust
CommandDef {
    name: "sethome",
    description: "set this chat as the home channel",
    category: CommandCategory::Session,
    aliases: &[],
    args: ArgSpec::None,
    handler: CommandHandler::Control(ControlAction::SetHome),
    access: CommandAccess::Authority,
    busy: BusyPolicy::Queue,
    availability: CommandAvailability::ALL,
}
```

A command earns its place here for one reason: a sentence can express the intent but cannot *discover* it. Platform slash menus — Discord application commands, the Telegram menu — are where a new user finds out the capability exists, and native registration already surfaces the registry there. That is the job conversation cannot do. Everything else about the command is a shortcut over the tool's handler.

There is no `/unsethome` and no separate status command. `/sethome` with no arguments sets the current channel; `/status` already reports binding state and gains the resolved home.

---

## Authority

`CommandAccess::Authority`, not `Everyone`. Access never widens who may talk to the agent, only who may change state — and this is state that redirects where the agent speaks. In a group chat, any sender the binding admits could otherwise point every autonomous message at a destination of their choosing. That is a redirection vector, not a configuration mistake.

The same reasoning as the wake authority model: the delivery target is a capability boundary, so it is set by principals the instance trusts, not by anyone who can reach the bot.

Setting a home the agent cannot actually broadcast to fails at set time, not at first send. Validation resolves the channel and checks the adapter exists, mirroring the adapter-existence check the wake config validation already performs.

---

## Storage

`SettingsStore` (`src/settings/store.rs`), with typed accessors alongside `worker_log_mode` and `prompt_capture_enabled`. It is instance-scoped, survives restart, and is mutable at runtime — a chat command must not require editing a config file or bouncing the daemon.

An optional `home_channel` key in config seeds an instance that ships pre-configured, following the ownership rule wakes and cron already use: config is a seed, the database is the source of truth, and a runtime change is never clobbered by a reload.

---

## First Run

A fresh instance has no home, which is exactly when the onboarding behavior most wants one — an agent with nothing to say hello to.

The first channel to complete a user turn becomes the home, recorded as implicit. An explicit set replaces it and marks it explicit; an implicit value never overwrites an explicit one. The agent surfaces the assignment once when it happens, so the destination is never a silent default the user discovers by receiving something unexpected.

---

## Level Gating

A home channel does not make the agent talk. The dial does.

| Level | Outbound behavior |
|---|---|
| `off` | Nothing fires, nothing sends. |
| `observe` | Never sends. Findings are recorded as memories and working-memory events. |
| `suggest` | May send to the resolved target. |
| `act` | May send to the resolved target. |

Mute-by-default survives the feature: an instance with a home set and the dial at `observe` accumulates memories and says nothing.

---

## Failure Behavior

| Failure | Behavior |
|---|---|
| No home set, no wake target | Run records the intended message as a memory and completes normally. Not an error. |
| Adapter unbound or offline at send | Send fails, run completes, content falls back to a memory. The run is not retried for delivery alone. |
| Bot removed from the home channel | Send fails as above; repeated failures clear the implicit home and notify on the next reachable surface. An explicit home is never silently cleared. |
| Target no longer parses after an adapter rename | Treated as unset. Resolution falls through to the record branch. |
| Same finding repeatedly worth sending | Every send is journaled into the autonomy transcript, so the next run sees what it already said and judges whether repeating is worth it. A content key backstops that against loops. An agent that repeats itself daily gets muted by its user. |

---

## Implementation Phases

**Phase 1 — Storage and resolution**
- `SettingsStore` accessors for the home target, explicit/implicit flag included
- `resolve_home_target()` helper implementing the three-branch order
- `WakeDef.delivery_target` finally consumed, with home as the fallback

**Phase 2 — Setting it**
- `set_home_channel` tool on user channels
- `ControlAction::SetHome` and the `/sethome` registry entry
- Authority gate and set-time validation
- `/status` reports the resolved home and whether it is explicit

**Phase 3 — First run and surface**
- Implicit home on first completed user turn, announced once
- Settings UI row showing the resolved target with a clear action

**Phase 4 — Autonomous outreach**
- Level gating on send
- Sends journaled into the autonomy transcript as the agent's own turn, surviving run compaction — see [`autonomy.md`](autonomy.md)
- Content-key dedupe as a loop backstop beneath that judgment
- Record-instead-of-send fallback wired to the memory path

---

## Non-Goals

- **No per-adapter homes.** One instance, one home. Fan-out to several destinations is a routing feature, not a default.
- **No content-based routing.** Wake-level `delivery_target` covers "this specific trigger goes elsewhere". Anything finer belongs to the wake definition, not to home resolution.
- **No `reply` tool on the autonomy channel.** Delivery to a configured target is not conversation. A user replying to an autonomous message is answered by the normal user channel for that conversation, which already has reply and full context.
- **No broadcast to every known channel.** There is no "announce" primitive here.
