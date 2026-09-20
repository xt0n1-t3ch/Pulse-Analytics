[Documentation](../index.md) / Architecture

# Pulse architecture

Pulse combines four provider adapters, one analytics store and a native desktop interface. The Claude headless daemon is a separate executable; Pulse does not require that daemon to serve its GUI.

## Data flow

```text
Claude JSONL + statusline ── Claude adapter ──┐
Codex JSONL + core data ──── Codex adapter ───┼── snapshots ── SQLite analytics
OpenCode SQLite ─────────── OpenCode adapter ┘       │               │
                                                   ├── Discord     └── reports
                                                   └── Tauri IPC ── Svelte
Provider account probes ─── authenticated access and allowance snapshots
```

Session history, account proof and Discord broadcaster identity remain separate. A local transcript is not proof of a quota or account plan. An API context specification is not proof of a session's usable budget.

## Workspace owners

| Layer | Owner | Responsibility |
| --- | --- | --- |
| Claude library/daemon | [src](../../src/lib.rs) | JSONL/statusline, usage, cost, config and headless lifecycle |
| Codex adapter | [src/codex](../../src/codex/mod.rs) | Pulse-facing model, context and monetary adapters |
| Shared Codex core | [upstream contract](../maintainers/codex-core.md) | Pinned canonical telemetry and presence contracts |
| OpenCode ingestion | [src/opencode](../../src/opencode) | Read-only SQLite, sessions, contributions and metadata |
| Accounts | [account backend](../../src-tauri/src/accounts/mod.rs), [Accounts UI](../../frontend/src/views/Accounts.svelte) | Stable connections, account-isolated polling, protected credentials and removal tombstones |
| Command Code | [adapter](../../src/commandcode/mod.rs), [bridge](../../src-tauri/src/commandcode.rs) | One CLI/Desktop reader and publisher |
| Native integration | [commands](../../src-tauri/src/commands.rs), [OpenCode bridge](../../src-tauri/src/opencode.rs) | Polling, snapshots and IPC |
| Persistence | [storage](../../src/storage.rs), [db](../../src-tauri/src/db.rs) | Provider-neutral paths, safe legacy copies, analytics schema 7, WAL and notifications |
| Reports | [analyzers](../../src-tauri/src/analyzers), [report](../../src-tauri/src/report.rs) | Provider-supported analysis and exports |
| Presentation | [App](../../frontend/src/App.svelte), [API](../../frontend/src/lib/api.ts), [stores](../../frontend/src/lib/stores.ts) | Six routes and coherent backend state |

## Provider contracts

- Claude reads `~/.claude/projects/` and optional statusline data. Its statusline headline cost is authoritative when available. `CLAUDE_HOME` can change the root.
- Codex reads its own session and runtime metadata under `CODEX_HOME` (default `~/.codex`). Its local model inventory and observed context outrank bundled fallback values.
- OpenCode reads its local databases without mutating them. It preserves reported costs and unknowns, including mixed-model contributions. Completed sessions stay in history, not live presence.
- The native core pin, local model catalog and frontend are distinct owners. Do not activate residual `src/codex/app.rs` or duplicate parsers in Svelte.

Read the [Claude](../models/claude.md), [Codex](../models/codex.md) and [OpenCode](../guides/opencode.md) guides for provider-specific limits.

## Snapshot and UI boundary

The current routes are Home (`dashboard`), Sessions, Costs, Reports, Discord and Settings. Context commands/components remain available internally; Context is not a seventh primary route.

`get_app_snapshot` hydrates related frontend stores together. Sequence guards reject late responses. Transport failures preserve the last coherent snapshot with a disconnected state. Provider changes must not carry another provider's settings, allowances or preview forward.

A malformed Discord config can degrade Discord fields while analytics remain available. Freshness comes from the captured timestamp, not the time a cached snapshot was redisplayed. See [adaptive access](adaptive-access.md).

Provider selection scopes sessions and analytics. All providers combines analytics without replacing the selected broadcaster. The backend composes both Discord publication and its preview; the UI does not rebuild those lines.

## Native lifecycle and storage

The Tauri entry point registers single-instance behavior, native plugins, tray actions and IPC. The GUI poller currently runs every five seconds. Provider probes and parsing caches have independent lifetimes; read their owners before changing timing.

SQLite is shared within the process, uses WAL and retains durable notification state. Bulk dismissal preserves rows; Undo restores the saved batch and read states. A compatible backup is required for schema rollback. [Notifications](../guides/notifications.md), [release and recovery](../maintainers/releases.md).

Local Discord IPC owns connected identity. Missing banners remain absent. Native notifications, the tray and the in-app center share the unread lifecycle.

## Development and validation

The installed GUI uses Tauri IPC and an embedded frontend. The [debug development bridge](dev-bridge.md) exposes a restricted authenticated loopback interface for browser work. Fixtures stay in tests; a mock screenshot is not installed-runtime proof.

Use the [test map](../../tests/index.md), [UI design contract](ui.md), [model references](../index.md#models-and-analytics) and [platform acceptance](../maintainers/platforms.md). Build through Tauri's custom protocol; a raw Cargo GUI build can still reference the development URL.
