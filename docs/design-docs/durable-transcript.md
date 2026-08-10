# Durable Transcript

The channel transcript as an append-only, persisted artifact. Once a message has been sent to a provider as part of a request, its bytes and its position never change; the transcript grows by appending, shrinks only at declared epoch boundaries, and survives process restart byte-for-byte. A restarted channel replays the same request structure the running channel would have sent.

This doc defines the transcript invariant and the storage that backs it. For why byte identity pays (cache economics), see [`prompt-stability.md`](prompt-stability.md). For what a process is allowed to forget entirely, see [`dormancy.md`](dormancy.md).

---

## Why This Exists

Channel history today is an in-memory `Arc<RwLock<Vec<rig::message::Message>>>` (`src/agent/channel.rs`), and it mutates in ways that rewrite bytes the model has already seen:

- **The normal turn ending rewrites the turn.** When the `reply` tool fires, the loop ends in `PromptCancelled`, and `apply_history_after_turn` (`src/agent/channel_history.rs`) discards the turn's actual assistant tool-call and tool-result messages, pushing a synthesized clean pair in their place. The bytes replayed at turn N+1 are not the bytes the model produced at turn N.
- **The retrigger bridge pops and replaces.** A synthetic assistant message pushed at delegation time is later removed and substituted with the relay summary (`pop_retrigger_bridge_message`). A message that was part of a sent request disappears from the position it occupied.
- **Compaction rewrites the head, concurrently.** `run_compaction` (`src/agent/compactor.rs`) drains the oldest messages on a spawned worker, summarizes them, and does `insert(0, summary)` — the start of the prefix changes at an arbitrary point between turns, racing the turn loop for the write lock.
- **Restart discards the structure entirely.** History is not persisted. On restart the vector starts empty and the last `history_backfill_count` messages are loaded from the conversation log, serialized to JSON, and injected into the *system prompt* as `backfill_transcript` (`src/main.rs`, `prompts/en/channel.md.j2`). The same conversation becomes a structurally different request: a large system-prompt blob and a message array of one.

Each of these was locally reasonable. Together they mean the transcript is not an artifact — it is a mutable scratch buffer whose relationship to what the model actually saw degrades over time. That blocks three things: history-level prompt caching (a rewritten prefix is a cache miss by definition), byte-level restart recovery, and any future in which the process hosting a channel is disposable. The conversation log (`conversation_logger`) records what was said for humans; nothing records what was *sent* for replay.

---

## The Invariant

```text
transcript = epochs of append-only segments

epoch 0            epoch 1 (compaction)      epoch 2 (compaction)
──────────         ─────────────────────     ────────────────────
m0 m1 m2 … m40  →  [summary(m0..m25)]        [summary(…)]
                   m26 … m73             →   m60 … m112 …
                                             ▲
                              append-only within an epoch;
                              a new epoch is the only head rewrite
```

1. **Within an epoch, the transcript is append-only.** No pop, no replace, no insert at the head, no truncate except full rollback of an unsent turn.
2. **Sent bytes are canonical.** What entered a provider request is what the transcript stores. Post-hoc cleanup for human readability belongs to the conversation log, not the transcript.
3. **Epoch transitions are atomic and serialized with the turn loop.** A compaction produces the next epoch between turns, never during one.
4. **The transcript is durable.** Rows in SQLite, keyed `(channel_id, epoch, seq)`, written as messages are appended. Rehydration on restart reproduces the exact vector, and the system prompt carries no backfill blob for a resumed channel.

---

## Decisions

### Keep the real turn messages

The `PromptCancelled` synthesis exists to keep history tidy: a turn's tool spam collapses into a clean user/assistant pair. Under the invariant it has to go, and the trade is worth stating honestly. Keeping the real messages means zero cache invalidation at the tail — the cache written during the turn's inner loop is read back on the next turn. The cost is faster history growth: tool-call and tool-result blocks accumulate at cached-read rates until compaction. Cached carriage is roughly a tenth of list price; a per-turn tail rewrite forfeits the inner-loop cache every single turn. Growth is the cheaper problem, and it is the one we already have a mechanism for (compaction epochs).

The synthesis path survives in one narrow form: turns that produced no reply tool call and no durable side effects (hard error rollback) truncate to the pre-turn length exactly as today — rollback of unsent state is not a rewrite of sent state.

### Retrigger bridge appends

The bridge message stops being popped. The relay summary is appended as a new message; the bridge message stays where it was sent. Prompt guidance ("this is a bridge, a summary will follow") does the work the mutation used to do.

### Compaction becomes an epoch transition

The compactor stops racing the turn loop. It still runs summarization on a worker, but the swap — retire epoch N, write the summary as the head of epoch N+1, carry forward the uncompacted tail — is applied by the channel between turns, atomically, and recorded as a `compaction` epoch in the sense of [`prompt-stability.md`](prompt-stability.md). One deliberate full cache miss, logged with a reason, instead of an unpredictable head rewrite. `emergency_truncate` follows the same shape synchronously: it produces an epoch, not an in-place surgery.

### Restart rehydrates; backfill retires for resumed channels

The transcript table replaces the backfill path for any channel that has one: on restart, load epoch and messages, reconstruct the vector, and the first post-restart request is byte-identical to what the pre-restart process would have sent. The `backfill_transcript` template block remains only for genuinely new channels importing platform history they have never seen — its original purpose. The `restart` epoch in `prompt-stability.md` is then deleted from the accepted-miss table, which is the point of this doc.

### Everything that appends must persist

The quieter append sites — suppressed/observe-mode messages pushed without a turn, background results drained as assistant messages — already conform to append-only; they just also have to write through to the transcript table so rehydration doesn't lose them.

---

## Storage

```sql
CREATE TABLE channel_transcript (
    channel_id TEXT NOT NULL,
    epoch      INTEGER NOT NULL,
    seq        INTEGER NOT NULL,
    -- Serialized rig::message::Message, the exact object sent to providers.
    message    BLOB NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (channel_id, epoch, seq)
);

CREATE TABLE channel_transcript_epochs (
    channel_id TEXT NOT NULL,
    epoch      INTEGER NOT NULL,
    -- 'initial' | 'compaction' | 'emergency'
    reason     TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (channel_id, epoch)
);
```

Writes go through the existing per-channel write path (same discipline as the conversation logger), appended at the same points `apply_history_after_turn` mutates the in-memory vector — the vector and the table change together or not at all. Only the live epoch is loaded at rehydration; retired epochs are kept for provenance and debugging, and are prunable by age. The serialization format is versioned; a message that fails to deserialize after an upgrade forces a `compaction`-style epoch rather than a crash, so a format migration degrades to one cache miss.

The conversation log and `prompt_snapshot.rs` are unchanged. The three stores answer three different questions — what was said (log), what was sent (transcript), what one turn looked like end-to-end (snapshot) — and none can substitute for another.

---

## Phases

1. **Append-only mutations.** Keep real turn messages on `PromptCancelled`, convert the retrigger bridge to append, and route the quiet append sites through one helper so the invariant has a single enforcement point.
2. **Transcript table.** Schema, write-through from the append helper, migration in `migrations/global/`.
3. **Epoch compaction.** Serialize the swap with the turn loop, record epochs, retire in-place `insert(0)`.
4. **Rehydration.** Load on restart, retire backfill for resumed channels, delete the `restart` epoch from the accepted-miss table, and turn on the restart byte-diff test from [`prompt-stability.md`](prompt-stability.md).
