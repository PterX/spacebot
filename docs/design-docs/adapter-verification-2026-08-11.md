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
- Spoiler `||text||`: **verified** — converted to `<tg-spoiler>`.
- send_file typing: **verified** — MIME dispatch selects `send_photo`,
  `send_video`, `send_voice` for Ogg/Opus, `send_audio` for other audio,
  and `send_document` otherwise.

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
- `reply` `blocks` parameter: **verified** — the `reply` schema accepts raw
  Block Kit JSON and passes it to `OutboundResponse::RichMessage`.

### mattermost
- Markdown pass-through incl. tables: **verified** — zero transformation
  (`mattermost.rs:242–281`); only length-based splitting at 16,383,
  which could split a giant table across posts.
- send_file: **verified** — uniform multipart upload + attach.

### signal
- Markdown → native styles: **verified** — markdown converts to Signal
  `textStyle` / `textStyles` ranges using UTF-16 offsets.
- Tables mangled: **verified** (trivially — no rendering at all).

### portal
- Tables: **verified** (`react-markdown` + `remark-gfm`,
  `interface/src/components/Markdown.tsx`).
- Code highlighting: **failed** — no highlighter dependency; plain
  monospace `<pre><code>`. Fix optional (frontend); fragment says "code
  blocks render monospace" until then.
- Inline previews: images, audio, and video **verified**.

### twitch
- Cap and splitting: **verified** — 500 chars
  (`twitch.rs:111`), splits at newline/space boundaries into multiple
  `say` calls; never truncates.
- Plain text: **verified**, with a nuance the fragment must state: raw
  markdown syntax is sent verbatim (nothing strips `**` or backticks),
  so the model must not write markdown at all.

### webhook
- send_file: **verified** — poll responses include filename, MIME type, and
  base64 file data.
- Payload format: **verified** — raw JSON, `content` is the model's text
  unconverted and unannotated; `cards`/`blocks`/`interactive_elements`
  are silently dropped.

## Consequences for Appendix A

Ship as written (converter already honest): mattermost, twitch (add the
"don't write markdown syntax" line), discord (soften to attachment
wording), slack minus the `blocks` sentence, portal minus highlighting
and audio/video preview claims, webhook minus file delivery.

All converter fixes tracked by this verification pass are implemented.
