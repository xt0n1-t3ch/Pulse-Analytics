[Pulse](../README.md) / Tests

# Pulse test map

Run every local check from the repository root:

```bash
cargo xtask check
```

Install dependencies with `bun install` first. The workspace uses Rust 1.99.1, Bun 1.4.4 and cargo-nextest. Results below are **pending** for the integrated changes. The maintainer records dated counts after validation; this map does not claim native runtime acceptance.

[Local gates](#local-gates) · [Frontend](#frontend) · [Backend](#backend) · [Release and documentation](#release-and-documentation) · [Runtime evidence](#runtime-evidence)

## Local gates

| What it proves | Command | Result |
| --- | --- | --- |
| Complete local quality gate | `cargo xtask check` (also `bun run verify`) | pending |
| Svelte and TypeScript type correctness (bun check and svelte-check) | `bun run typecheck` | pending |
| JavaScript lint rules (oxlint) | `bun run lint` | pending |
| JavaScript and Rust formatting | `bun run fmt:check` | pending |
| Rust warnings and lint rules | `cargo clippy --workspace --all-targets -- -D warnings` | pending |
| All workspace Rust tests | `cargo nextest run --workspace` | pending |
| All JavaScript tests under the browser condition | `bun run test` | pending |

## Frontend

Bun tests cover logic, mocked IPC and Svelte component rendering using happy-dom. [Bun setup](bun-setup.ts) registers the DOM and Svelte compilation; [shared setup](setup.ts) supplies test seams. These tests do not authenticate providers or prove a native WebView.

| What it proves | Command | Result |
| --- | --- | --- |
| Formatting helpers, access proof/freshness, provider selection and documentation contracts in `tests/unit/` | `bun run test` | pending |
| Poll/store hydration, provider race rejection, mocked IPC and reports flow in `tests/integration/` | `bun run test` | pending |
| Home, Sessions, Costs, Reports, Discord, Accounts and Settings behavior in `tests/components/` | `bun run test` | pending |
| Notifications retain last-good state, confirm clears and restore original read states | `bun run test` | pending |
| Unknown costs, native quota windows and independent broadcaster selection remain explicit | `bun run test` | pending |
| Coordinated version surfaces and immutable core pin | `bun run test` | pending |

## Backend

The workspace contains the `cc-discord-presence` core/Claude daemon, the `pulse` Tauri host and `xtask` tooling. Unit modules stay beside their owners; integration tests live here and in `src-tauri/tests/`.

| What it proves | Command | Result |
| --- | --- | --- |
| Claude response deduplication, current context after compaction, model aliases, TTL cache pricing and per-request tiers in `src/session/tests.rs` and `src/cost/tests.rs` | `cargo nextest run --workspace` | pending |
| Codex observed/inventory/fallback context, cost coverage, speed, quota windows and canonical core contracts | `cargo nextest run --workspace` | pending |
| OpenCode SQLite schemas, mixed models, Go windows, completion rules and reported zero versus unavailable cost | `cargo nextest run --workspace` | pending |
| Command Code shared CLI/Desktop collection and identity | `cargo nextest run --workspace` | pending |
| Orion current index/JSONL/gzip/telemetry, legacy fallback, last-record-wins and subagent roll-up in `src/orion/` | `cargo nextest run --workspace` | pending |
| Purpose-folder layout, flat-layout relocation, legacy import and `~/.claude` cleanup, preserved destination data and credential exclusion in [storage_migration.rs](storage_migration.rs) and `src/storage/migrate.rs` | `cargo nextest run --workspace` | pending |
| Zero footprint: no side files next to other programs' databases (`src/foreign_sqlite.rs`), in-process process list and Git branch (`src/process_list.rs`, `src/git_head.rs`), read-only Claude logins (`src/usage.rs`) and Codex reads only on Refresh (`src-tauri/src/commands/codex_probe_gate_tests.rs`) | `cargo nextest run --workspace` | pending |
| Provider-neutral access, durable notifications, single-instance startup and webview security in `src-tauri/tests/` | `cargo nextest run --workspace` | pending |
| Offline report escaping, cost provenance and bounded single-scan analysis in [reports_e2e.rs](../src-tauri/tests/reports_e2e.rs) and [report_html.rs](../src-tauri/tests/report_html.rs) | `cargo nextest run --workspace` | pending |
| Authenticated loopback transport, strict origins, real-command dispatch and safe-control arguments in `src-tauri/src/dev_bridge.rs` | `cargo nextest run --workspace` | pending |
| Component-boundary workspace matching and platform process ownership | `cargo nextest run --workspace` | pending |

## Release and documentation

| What it proves | Command | Result |
| --- | --- | --- |
| Rust release tooling and asset contracts, including publication opt-in and platform boundaries | `cargo nextest run -p xtask` | pending |
| Keep a Changelog structure and version references | `cargo xtask changelog check` | pending |
| Curated release section extraction | `cargo xtask changelog notes <version>` | pending |
| Generated llms files match [docs/llms.toml](../docs/llms.toml) and linked source documents | `cargo xtask llms --check` | pending |
| Repository description and topics match declared metadata | `cargo xtask meta --check` | pending |
| Pulse and canonical Codex catalogs are byte-identical and required entries agree | `cargo xtask catalog-parity` | pending |

The release workflow is manual-only. A local successful gate does not publish anything. Read the [release procedure](../docs/maintainers/releases.md), [platform acceptance](../docs/maintainers/platforms.md) and [dependency audit](../docs/maintainers/dependencies.md) before promoting artifacts.

## Runtime evidence

| What it proves | Command | Result |
| --- | --- | --- |
| Browser UI uses real Rust state through the authenticated proxy, not fixtures | `bun run dev` | pending |
| Development native window responds through real Tauri IPC | `bun run dev:tauri` | pending |
| Portable build embeds the frontend; inspect the actual window with development ports stopped | `bun run build:portable` | pending |
| Native installers build for the current OS; inspect installation, upgrade and rollback separately | `bun run build` | pending |
| Claude daemon status and diagnostics | `cargo run -p cc-discord-presence -- status` / `cargo run -p cc-discord-presence -- doctor` | pending |

Browser layout and keyboard review, native installation, provider authentication, Discord IPC acknowledgement and Discord visual asset rendering are separate manual evidence. Review light/dark themes, focus, narrow layouts, loading and error states. Never substitute fixtures for installed-runtime or account proof. Stop only processes whose executable and PID belong to the current test.

Ignored live-history and Discord tests require explicit opt-in and are not part of the ordinary suite. Do not run them against private profiles or publish their output without review. The removed browser E2E harness is not a supported validation command.
