# Adapter fragment verification — 2026-08-11

Results of the Appendix A claims-to-verify pass
([`system-prompt-rework.md`](system-prompt-rework.md), task 1.5 in
[`execution-plan.md`](execution-plan.md)). Each adapter fragment is a
contract on its converter; a FAILED claim means the converter needs a fix
before that fragment line ships — the fragment wording does not get
weakened to match a broken converter unless the fix is genuinely out of
scope.

Adapter inventory: `discord`, `email`, `mattermost`, `portal`, `signal`,
`slack`, `telegram`, `twitch`, `webhook` (`src/messaging.rs`). `cron` is a
prompt-only fragment, not an adapter; there is no `api_server` module —
the HTTP channel is `webhook`. `render_channel_adapter_prompt`
(`src/prompts/engine.rs:726–744`) returns fragments for `email`, `cron`,
`signal` only; all seven others get `None`, so the new fragments are
net-new.

## Verdicts

### telegram
- Markdown → entities: **verified** for bold, italic, strikethrough,
  inline code, code blocks, links, and headers (headers render as bold —
  `markdown_to_telegram_html`, `src/messaging/telegram.rs:1113–1190`).
- Spoiler `||text||`: **failed** — not implemented anywhere; passes
  through as literal pipes. Converter fix: map `||…||` → `<tg-spoiler>`.
- send_file typing: **failed** — `respond()` (`telegram.rs:359–438`)
  branches only on `audio/*` → `send_audio` (player UI, not a voice
  bubble); everything else, including images and .mp4, goes out as
  `send_document`. No `send_photo`, `send_video`, or `send_voice` calls
  exist. Converter fix: mime-typed dispatch to
  `send_photo`/`send_video`/`send_voice` (.ogg/opus).

### discord
- `reply` `cards` parameter: **verified** — literally named `cards`
  (`src/tools/reply.rs:106–109`, schema at `:286–345`).
- send_file typing: uniform path — `mime_type` is discarded
  (`discord.rs:348`), one `CreateAttachment` path for every type.
  Acceptable as-is: Discord's client renders images/audio inline from
  the attachment itself, so the fragment can say "sent as attachments;
  images and audio render inline" without a converter change.

### slack
- send_file: **verified** — uniform v2 external-upload flow, mimetype
  passed through, client renders previews.
- `reply` `blocks` parameter: **failed** — `OutboundResponse::RichMessage`
  carries a `blocks` field (`src/lib.rs:815–818`) and the adapter consumes
  it (`slack.rs:1029–1055`), but no tool schema exposes it:
  `reply.rs:485` and `ask.rs:312` hardcode `blocks: vec![]`. The agent
  has no way to produce Block Kit today. Fix: add `blocks` to the reply
  tool schema (plumbing exists end-to-end), or the fragment ships
  without the rich-response sentence.

### mattermost
- Markdown pass-through incl. tables: **verified** — zero transformation
  (`mattermost.rs:242–281`); only length-based splitting at 16,383,
  which could split a giant table across posts.
- send_file: **verified** — uniform multipart upload + attach.

### signal
- Markdown → native styles: **failed** — the adapter sends raw text with
  no conversion of any kind (`signal.rs:408–412`; module doc states no
  rich formatting). Asterisks and underscores arrive literally. Fix:
  markdown → signal textStyle ranges (bold/italic/strike/mono) in the
  adapter, or the fragment must instruct plain text, no markdown.
- Tables mangled: **verified** (trivially — no rendering at all).

### portal
- Tables: **verified** (`react-markdown` + `remark-gfm`,
  `interface/src/components/Markdown.tsx`).
- Code highlighting: **failed** — no highlighter dependency; plain
  monospace `<pre><code>`. Fix optional (frontend); fragment says "code
  blocks render monospace" until then.
- Inline previews: images **verified**; audio/video **failed** — no
  `<audio>`/`<video>` elements anywhere in the frontend
  (`PortalTimeline.tsx:66–169`); non-image files are download links.
  Fix: add audio/video elements for those mime types, or fragment claims
  images only.

### twitch
- Cap and splitting: **verified** — 500 chars
  (`twitch.rs:111`), splits at newline/space boundaries into multiple
  `say` calls; never truncates.
- Plain text: **verified**, with a nuance the fragment must state: raw
  markdown syntax is sent verbatim (nothing strips `**` or backticks),
  so the model must not write markdown at all.

### webhook
- send_file path: **exists but drops the payload** — the poll response
  records `{type:"file", filename, caption}` with no bytes and no
  retrieval route (`webhook.rs:62–76,171–178`). Fix: either expose file
  content (download URL or base64 field) or the fragment must say files
  cannot be delivered on this channel — stating a path is worse than
  admitting there isn't one.
- Payload format: **verified** — raw JSON, `content` is the model's text
  unconverted and unannotated; `cards`/`blocks`/`interactive_elements`
  are silently dropped.

## Consequences for Appendix A

Ship as written (converter already honest): mattermost, twitch (add the
"don't write markdown syntax" line), discord (soften to attachment
wording), slack minus the `blocks` sentence, portal minus highlighting
and audio/video preview claims, webhook minus file delivery.

Blocked on converter fixes, each its own PR:

1. telegram send_file mime dispatch (`send_photo` / `send_video` /
   `send_voice`) + spoiler entity support.
2. slack `blocks` exposure in the reply tool schema.
3. signal markdown → textStyle ranges (largest; decide whether it earns
   the effort or the fragment teaches plain text instead).
4. webhook file delivery (or an explicit no-files contract).
5. portal audio/video inline elements (frontend, optional).
