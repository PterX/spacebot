# Full CLI Coverage

Expand the CLI from a daemon controller into a complete control surface for the instance. Every resource the HTTP API exposes — agents, channels, tasks, cron, memories, wiki, projects, messaging, providers — becomes addressable from the terminal, using the daemon's API as the single backend.

## Problem

The clap tree covers lifecycle (`start`/`stop`/`restart`/`status`) plus three admin namespaces (`skill`, `auth`, `secrets`). Everything else lives exclusively behind the HTTP API and the web UI. There is no way to list agents, poke a cron job, approve a task, or search memories without opening a browser or hand-writing a curl call with a bearer token.

The pieces that do exist have grown ad hoc:

- All of it sits in `src/main.rs`, which is past 4,000 lines.
- `cmd_secrets` talks to the API through four near-identical `secrets_api_*` helpers and reads responses with untyped `body["field"].as_str()` chains, even though the typed request/response structs live in the same crate.
- Tables print to stderr, so piping output into `jq` or `awk` gets nothing.

## Solution

The CLI is an HTTP client of the running daemon. The Unix socket stays lifecycle-only (`stop`, `status` fast-path); every resource command goes through the existing API with the bearer token from config. One source of truth for behavior — the API handlers — and no SQLite contention between a CLI process and the daemon.

Target resolution, in order: `--url` flag, `SPACEBOT_URL` env var, `config.api` bind/port. Token: `--token`, `SPACEBOT_TOKEN`, `config.api.auth_token`. Local use needs zero flags; pointing the CLI at a remote instance is the same two variables the SDK already uses.

Commands are noun-verb, mirroring API resources: `spacebot agent list`, `spacebot task approve <id>`, `spacebot cron trigger <id>`. The existing `skill`/`auth`/`secrets` namespaces already have this shape and keep their semantics.

### Client and types

A single `ApiClient` in `src/cli/client.rs` replaces the `secrets_api_*` helpers: base URL, token, method helpers, and one error-mapping path that turns API error bodies into readable CLI errors. Handlers deserialize into the utoipa types from `src/api/*` directly — the CLI and server are the same crate, so there is no codegen and no drift.

When the daemon isn't running, every resource command detects it via the socket before attempting HTTP and exits with a pointer to `spacebot start`, rather than a connection-refused trace.

### Output

Human-readable tables by default, a global `--json` flag that prints the raw API response body. Data goes to stdout, status chatter to stderr — fixing the current inversion while the code moves.

### Module layout

```
src/cli/
  mod.rs        clap tree + dispatch
  client.rs     ApiClient (base URL, token, error mapping)
  output.rs     table rendering, --json plumbing
  agent.rs      one file per resource namespace
  task.rs
  cron.rs
  ...
```

`main.rs` shrinks to argument parsing, lifecycle commands, and the daemon `run()` path. One file per namespace, nothing deeper.

---

## Phase 1: Plumbing

Extract the CLI out of `main.rs` into `src/cli/`. Build `ApiClient` and the output layer. Port `skill`, `auth`, and `secrets` onto both — behavior-preserving, but responses become typed and tables move to stdout.

Decide the offline question here: `cmd_skill` currently mixes an HTTP path with offline fallback logic. The rule after this phase is that resource commands require a running daemon; the only sanctioned offline path is the secrets-store bootstrap that config loading already needs.

## Phase 2: Core resources

The daily-driver namespaces:

- `agent` — list, create, delete, overview, wake, identity get/set
- `channel` — list, status, messages, archive, cancel
- `task` — list, get, create, update, approve, execute, assign, delete
- `cron` — list, create, delete, toggle, trigger, executions
- `memory` — list, search

## Phase 3: Knowledge and workspace

- `wiki` — list, search, get, create, edit, history, restore
- `project` — list, get, create, scan, disk-usage, repo add/remove, worktree add/remove
- `ingest` — list, upload, delete

## Phase 4: Platform and ops

- `messaging` — status, toggle, instance add/remove
- `binding` — list, create, update, delete
- `provider` — list, set, test, delete
- `model` — list, refresh
- `mcp` — list, add, remove, reconnect, status
- `config` — settings get/set, raw config get/edit
- `notification` — list, read, dismiss
- `usage`, `activity`
- `update` — check, apply (wraps the settings update endpoints)

## Phase 5: Ergonomics

- `spacebot chat [--agent <id>]` — a portal REPL over portal send/history. Talk to an agent from the terminal without opening anything.
- `spacebot dashboard` — ensure the daemon is running (start it if not), print and open `http://localhost:<port>`.
- `spacebot desktop` — launch the installed desktop app, with a clear message when it isn't installed. Building the desktop app from source stays in `just`; the user CLI launches artifacts, it doesn't build them.
- Shell completions via `clap_complete` (`spacebot completions zsh`).

## Out of scope

Not every endpoint is a CLI verb. Avatar and attachment byte serving, link topology (graph data), cortex event streams, prompt snapshots, and the opencode proxy are UI plumbing — excluded deliberately rather than chasing endpoint parity. Anything excluded remains reachable with curl against the same API the CLI uses.
