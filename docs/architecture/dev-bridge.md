# Pulse browser development bridge

## Introduction

This reference documents the debug-only HTTP seam that lets the Vite browser
render Pulse from the same Rust state as the Tauri window. It is a development
transport, not a production API and not a fixture provider.

## Table of contents

- [Contract](#contract)
- [Running the standalone bridge](#running-the-standalone-bridge)
- [Proof and limits](#proof-and-limits)

## Contract

- Bind: `127.0.0.1` only; `PULSE_DEV_BRIDGE_PORT` selects the port (default `1421`).
- Request: authenticated `POST /invoke` with JSON
  `{ "command": "get_metrics", "args": { ... } }`.
- Authentication: `Authorization: Bearer <token>`, where the token is supplied
  through `PULSE_DEV_BRIDGE_TOKEN`. An unset token disables the listener.
- CORS: only `localhost` and `127.0.0.1` at the exact `PULSE_DEV_PORT` (default `1420`); wildcard origins are never emitted.
- Dispatch: an explicit allowlist calls `src-tauri/src/commands.rs` and the
  shared analytics database/live poller. Safe authenticated settings,
  notification, refresh, and history-confirmation controls use the same real
  commands; shell/open-url actions remain unavailable. Unknown commands return
  `404`; a real backend error returns `503`.
- Access snapshots include the SQLite-derived `local_history` capability for
  each discovered provider route. That field makes local analytics selectable;
  it does not create provider proof, quota windows, plan detection, or Discord
  authority.
- Cost totals retain provider-scoped token categories when monetary coverage is
  partial or unavailable. Consumers must continue to honor `cost_basis` and
  `priced_sessions`; the bridge never converts missing subscription spend to
  zero.

## Running the standalone bridge

For the normal browser development workflow, use the repository-owned launcher:

```powershell
cd frontend
bun run dev
```

It creates one random per-run token, exports it to the programmatic Vite
configuration, builds and starts the hidden Rust bridge, and waits for the
authenticated backend before binding the UI on `127.0.0.1:1420`. Opening the
local UI therefore either renders real backend state or fails closed; there is
no browser fixture fallback.

For low-level bridge debugging, set the same token in both processes, then run
the backend from the repository root:

```powershell
$env:PULSE_DEV_BRIDGE_TOKEN = "a-long-local-secret"
cargo run -p pulse --bin pulse-dev-bridge
```

The standalone process starts the real background poller before accepting
requests. It is debug-only and exits when the token is missing. The Tauri debug
binary can also start the listener through its normal `setup` path when the
same environment variable is present.

## Proof and limits

`src-tauri/src/dev_bridge.rs` tests the loopback address, token rejection,
strict origin allowlist, real command dispatch, and explicit unavailable/unknown
paths. Browser rendering proves the bridge seam only, not provider production
availability or Discord IPC. A live local-history count proves that Pulse can
read stored sessions for that provider; only a fresh authenticated route proves
provider allowances.

## Alternate ports and isolated real data

Set `PULSE_HOME` to a development profile initialized with a consistent SQLite backup. Keep provider source roots separate. Do not copy provider credentials into the analytics database.

```powershell
$env:PULSE_HOME = 'C:\pulse-dev-data'
$env:PULSE_DEV_PORT = '1430'
$env:PULSE_DEV_BRIDGE_PORT = '1431'
$env:PULSE_DISABLE_DISCORD = '1'
npm run dev
```

The launcher validates distinct integer ports, generates one shared token, starts Rust, checks readiness, then starts Vite. The proxy strips cookies and adds authentication even for clients that send `Expect: 100-continue`. Missing authentication and foreign origins remain rejected.

The six Accounts commands share the native Rust handlers: `discover_accounts`, `list_accounts`, `connect_account`, `cancel_account_connection`, `refresh_account` and `remove_account`. A provider connection is a state-changing operation, even though quota observations are read-only. The bridge does not expose purchases, plan changes or reset redemption.

For unpublished canonical core work, set `PULSE_CODEX_CORE_PATH` to the local `crates/codex-presence-core` folder. The launcher applies an explicit Cargo patch for that run. This is not a published promotion. Restore the immutable Git resolution in `Cargo.lock` before release verification.
