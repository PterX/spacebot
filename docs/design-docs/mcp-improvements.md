# MCP Improvements

The MCP client shipped in [`mcp.md`](mcp.md) works: stdio and streamable HTTP transports, per-agent config under `[[defaults.mcp]]` / `[[agents.mcp]]`, worker tool registration, hot-reload reconciliation, retry with backoff, and a status API. What's missing is everything around it — the management API writes to a config key the loader ignores, the dashboard has zero MCP surface, remote servers that require OAuth can't be used at all, and a chatty server dumps its entire tool list into every worker's context.

This doc covers five improvements: fixing the config CRUD path, tool filtering, a dashboard panel, OAuth for remote servers, and connection health hardening.

## Current State

- `src/mcp.rs` — `McpManager` / `McpConnection`, lifecycle, retry, reconcile
- `src/tools/mcp.rs` — `McpToolAdapter`, tools namespaced `{server}_{tool}`, worker-only
- `src/api/mcp.rs` — global CRUD + status endpoints
- `src/api/agents.rs` — per-agent status + reconnect
- `src/cli/mcp.rs` — `spacebot mcp list|add|update|remove|reconnect|status`, wraps the API
- `interface/` — nothing except auto-generated types in `schema.d.ts`

### The bug (issue #221)

`src/api/mcp.rs` reads and writes a top-level `[[mcp_servers]]` array. The config loader only deserializes `[[defaults.mcp]]` and `[[agents.mcp]]` — top-level `mcp_servers` is silently dropped by serde, and `warn_unknown_config_keys` already warns about exactly this key. The test `top_level_mcp_servers_silently_ignored_by_serde` in `src/config.rs` documents the root cause.

Consequences today:

- `POST /api/mcp/servers` and `spacebot mcp add` write definitions that never load or connect
- `GET /api/mcp/servers` doesn't list servers configured correctly under `[[defaults.mcp]]`
- Only `GET /api/mcp/status`, `GET /api/agents/mcp`, and the reconnect endpoints are accurate, because they read live `McpManager` state instead of the config file

The CRUD handlers also write the config file and stop — they rely on the file watcher to pick up the change, instead of driving the same reload path the settings API uses.

## Decisions

- **CRUD targets `[[defaults.mcp]]`.** The API manages the global defaults list. Per-agent `[[agents.mcp]]` entries stay file-managed for now; they're visible read-only through the per-agent status endpoint. No migration or fallback read of top-level `mcp_servers` — the loader already warns on it, and it never worked.
- **Mutations apply synchronously.** After writing config, the handler drives the same reload + `reconcile()` path the settings API uses, then returns. The response reflects a config that is already being applied, not a file the watcher may or may not have seen yet.
- **Secrets never round-trip.** Config values keep `${VAR}` placeholders as written. Literal header and env values are redacted in GET responses; updates that omit them keep the existing values.
- **Tool filtering is config, not runtime state.** Include/exclude lists live on the server definition and apply at registration time. No per-tool toggling of a live connection.
- **OAuth tokens live in the agent database**, not in config.toml. Config declares `auth = "oauth"`; the tokens are runtime state with their own lifecycle.
- **Workers only, still.** None of this changes where MCP tools register. Channels keep delegating.

## Phase 1: Fix the Config CRUD Path

Retarget `src/api/mcp.rs` from top-level `mcp_servers` to `defaults.mcp` using the same `toml_edit` approach, preserving formatting and comments.

- `GET /api/mcp/servers` — read `[[defaults.mcp]]`, return full definitions (transport fields included, secrets redacted) merged with live state per server. Today it returns only name/transport/enabled/state, which isn't enough to populate an edit form.
- `POST` / `PUT` / `DELETE` — mutate `[[defaults.mcp]]`, validate with the same rules as `parse_mcp_server_config` (stdio requires `command`, http requires `url`) before writing, then apply the reload path and return the resulting connection state.
- The CLI inherits all of this for free since it wraps the API.

Update the `top_level_mcp_servers_silently_ignored_by_serde` test to cover the fixed behavior: API round-trip lands in `[[defaults.mcp]]` and the loader picks it up.

## Phase 2: Tool Filtering

A server like GitHub's exposes dozens of tools; a worker that needs three of them pays context for all of them. Add per-server filters:

```toml
[[defaults.mcp]]
name = "github"
transport = "http"
url = "https://api.githubcopilot.com/mcp/"
headers = { "Authorization" = "Bearer ${GITHUB_TOKEN}" }
tools = { include = ["search_*", "get_*"], exclude = ["get_release_assets"] }
```

- `McpServerConfig` gains `tools: Option<McpToolFilter>` with `include` / `exclude` glob lists. Matching runs against the server's original tool names, before namespacing/sanitization.
- If `include` is set, only matching tools register; `exclude` then removes from that set. Absent filter means everything, as today.
- Applied in `McpConnection::list_tools` so the filtered set is what `get_tools()`, `get_tool_names()`, the prompt injection, and the status endpoints all see — a filtered-out tool doesn't exist anywhere downstream.
- Filter changes are config changes, so hot-reload reconciliation picks them up. `reconcile()` must treat a filter change as "changed" (it currently compares full `McpServerConfig`, so this falls out of adding the field — verify with a test).
- Status responses gain `tool_count` and, per server, the filtered tool names, so the dashboard can show what a filter actually did.

## Phase 3: Dashboard Panel

The typed API client already has every endpoint from `schema.d.ts`; the frontend just never used them.

### Settings page: MCP Servers section

New section in `interface/src/routes/Settings.tsx` backed by the fixed CRUD API:

- Server list with live state badge (connected / connecting / failed / disconnected / disabled), transport, and tool count. Failed servers show the error string from `McpServerStatus`.
- Add/edit form that switches fields on transport — stdio: command, args, env; http: url, headers — plus enabled toggle and tool filter globs. Secret-shaped fields render redacted values as placeholders and only send when changed.
- Per-server actions: reconnect, disable/enable, remove.
- A hint that `${VAR}` interpolation is available for secrets, mirroring what the config file supports.

### Agent config page: MCP section

New entry in `SECTIONS` (`interface/src/components/agent-config/constants.ts`, `group: "config"`) showing the agent's resolved server list from `GET /api/agents/mcp` — which defaults it inherited, which per-agent entries override them, connection state, and a reconnect button. Read-only in this phase; editing per-agent entries stays in the config file.

## Phase 4: OAuth for Remote Servers

Static bearer headers cover self-hosted and API-key servers, but the hosted MCP servers people actually want (Sentry, Linear, Notion) require OAuth 2.1. The `rmcp` crate ships an `auth` feature implementing the MCP authorization spec (discovery, dynamic client registration, PKCE) — use it rather than hand-rolling.

### Config

```toml
[[defaults.mcp]]
name = "linear"
transport = "http"
url = "https://mcp.linear.app/mcp"
auth = "oauth"
```

`auth` is optional; absent means today's behavior (static headers). `auth = "oauth"` with `headers.Authorization` set is a config error.

### Flow

The dashboard is the browser, so the redirect flows through the API server:

1. Server with `auth = "oauth"` and no stored token connects into a new `AuthRequired` state (distinct from `Failed` — it's not an error, it's waiting on a human).
2. `POST /api/mcp/servers/{name}/auth` starts the flow: metadata discovery, client registration if needed, returns the authorization URL. The dashboard opens it.
3. `GET /api/mcp/oauth/callback` receives the redirect, exchanges the code (PKCE), persists tokens, and triggers reconnect for that server.
4. Refresh happens transparently inside the connection. A 401 on a tool call attempts one refresh-and-retry; if that fails, the server drops to `AuthRequired` and the tool call returns an error the LLM can see.

### Token storage

New table in the agent database keyed by server name: access token, refresh token, expiry, client registration, and the OAuth metadata that produced them. Deleting a server deletes its tokens. `AuthRequired` state and a "reauthenticate" action surface in both the settings panel and `spacebot mcp` (`spacebot mcp login <name>`).

## Phase 5: Connection Health

Two gaps in the current lifecycle:

- **Silent death.** A connected session that stops responding isn't noticed until a tool call fails. Add a keepalive: ping on an interval (default 180s), latch off per-connection if the server answers `-32601` method-not-found, fall back to `tools/list` in that case. A failed probe moves the connection into the existing retry path.
- **Infinite thrash.** `connect_with_retry` gives up after 12 attempts, but a server that connects and immediately drops resets the count each time — it can flap forever. Track whether a session was ever *proven* (survived one keepalive interval or served one successful tool call). Unproven sessions share one retry budget across drops; exhausting it parks the server in `Failed` with the last error until a manual reconnect or config change revives it.

Status responses gain `last_error` and `last_connected_at` so parked servers are diagnosable from the dashboard without log-diving. Both behaviors get metrics via the existing `mcp_*` telemetry registry.

## File Changes

| File | Change |
|------|--------|
| `src/api/mcp.rs` | retarget CRUD to `defaults.mcp`, full definitions + redaction, synchronous apply, auth endpoints |
| `src/config/types.rs`, `toml_schema.rs`, `load.rs` | `McpToolFilter`, `auth` field, validation |
| `src/mcp.rs` | filter application, keepalive, proven-session retry budget, `AuthRequired` state, token refresh |
| `src/api/server.rs` | route registration for auth endpoints |
| `src/cli/mcp.rs` | `login` subcommand, surface new status fields |
| `migrations/` | OAuth token table |
| `interface/src/routes/Settings.tsx` + new components | MCP servers section |
| `interface/src/components/agent-config/` | per-agent MCP section |

## Out of Scope

- MCP resources and prompts (still tools-only)
- Sampling and elicitation
- Spacebot as an MCP server
- Lazy connection / schema caching (connect-at-startup with background retry is fine at current fleet sizes)
- Per-tool trust tiers or approval gates
- A curated server catalog in the UI
- Editing `[[agents.mcp]]` from the dashboard (read-only visibility only)
