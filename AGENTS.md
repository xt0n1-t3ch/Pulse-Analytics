# AGENTS.md — Pulse repository guide

## Product and scope

Pulse is the desktop analytics GUI for Claude Code, Codex, OpenCode and Command Code. It uses Rust, Tauri 2, Svelte 5, TypeScript, Vite and SQLite. The `cc-discord-presence` binary is the Claude headless daemon; its CLI has `status`, `doctor` and `claude` commands. Do not describe that daemon as a Codex CLI.

This checkout targets Pulse 1.9.5 through the six-platform Release workflow. Read `package.json`, Cargo manifests, `scripts/release-contract.json` and `src/codex/UPSTREAM.json` for current version facts. Verify publication and installed-runtime status separately. Do not copy historical branch/version claims into current instructions, and do not describe a local development build or installation as a release.

- Origin: https://github.com/xt0n1-t3ch/Pulse-Analytics
- Default branch: `main`; inspect the current branch before editing.
- Start from `docs/index.md` and `tests/index.md`. The changelog owns release history; do not restore retired model notes or machine-specific handoffs to the documentation index.
- Never discard existing work. Do not commit, push, tag, dispatch, deploy or publish without Tony's explicit authorization for that effect.

## Ownership map

| Area | Owner | Boundary |
| --- | --- | --- |
| Claude daemon | `src/main.rs`, `src/cli.rs`, `src/app.rs` | Headless lifecycle and CLI |
| Claude sessions and costs | `src/session.rs`, `src/cost.rs`, `src/config.rs`, `src/usage.rs` | JSONL/statusline, model rules, local config and subscription probes |
| Codex model adapter | `src/codex/model.rs`, `src/codex/cost.rs`, `src/codex/model_catalog.json` | Alias, context, rate and completeness logic |
| Shared Codex core | `codex-presence-core`, pinned in Cargo and `src/codex/UPSTREAM.json` | Canonical telemetry/presence contracts; no duplicate parser in the frontend |
| OpenCode | `src/opencode/`, `src-tauri/src/opencode.rs` | Read-only local SQLite ingestion and snapshots |
| Accounts | `src-tauri/src/accounts/`, `frontend/src/views/Accounts.svelte` | Per-connection identity, protected profiles, native allowances and tombstones; never change agent logins |
| Command Code | `src/commandcode/`, `src-tauri/src/commandcode.rs` | Shared CLI/Desktop JSONL ingestion, reported costs and one Discord identity |
| GUI backend | `src-tauri/src/main.rs`, `src-tauri/src/commands.rs` | Native lifecycle, polling, IPC and provider presentation |
| Persistence and reports | `src/storage.rs`, `src-tauri/src/db.rs`, `src-tauri/src/report.rs`, `src-tauri/src/analyzers/` | Pulse-owned paths, safe migration, analytics, notifications and reports |
| Frontend | `frontend/src/App.svelte`, `frontend/src/lib/`, `frontend/src/components/`, `frontend/src/views/` | Render backend facts; do not invent quotas, prices or context |
| Discord identity | `src/discord_identity.rs`, `src/discord.rs`, Codex core compositor | Local IPC identity and shared publication/preview behavior |
| Packaging | `.github/workflows/release.yml`, `scripts/release-*.ps1`, `src-tauri/tauri.conf.json` | Native artifacts, signing and explicit release effects |

The current primary routes are Home (`dashboard`), Sessions, Costs, Reports, Discord, Accounts and Settings. Accounts is independent of the analytics and Discord provider selections. Context commands and components still exist; do not describe Context as a primary route. `src/codex/app.rs` is residual code, not an active integration entry point.

## Models, context and prices

Do not maintain a price table or fixed model list in this file. Use `docs/models/claude.md` and `docs/models/codex.md` for the dated provider references and `docs/guides/costs.md` for arithmetic. Claude-specific model notes are linked from the documentation index; `src/cost.rs` owns their implemented rules.

For model or pricing work:

1. Read the exact model ID, source date and owner. Separate API capabilities, local Codex inventory, session observations and bundled defaults.
2. Inspect the relevant fields in `CODEX_HOME/models_cache.json` and session telemetry without exposing credentials or private prompt text. Record client version and verification date.
3. Verify changing API facts against the official model card and pricing tier. Do not turn an older source date into a current verification date.
4. Distinguish raw context, usable context, maximum input, maximum output, current fill and compaction threshold. Never use an API maximum as proof of usable Codex capacity.
5. Preserve observed context first, then local inventory, then bundled fallback. Do not subtract a reserve twice or borrow another model's limits.
6. Keep model, effort and speed independent. A Codex `ultra` value is not automatically an API-supported effort. Subscription credits are not API dollars.
7. Treat missing cost as unavailable, not zero. `exact` describes component coverage under selected rates, not rate freshness or a confirmed provider bill.
8. Update the canonical catalog in Codex-Discord-Rich-Presence only within authorized scope, then synchronize the Pulse copy and run `scripts/check-model-catalog-parity.ps1`. Do not silently change the core pin or historical analytics.
9. Keep the model guide, cost guide, context guide and their linked snapshot consistent. Document a verified mismatch until the runtime owner is actually corrected.

The September 5, 2026 refresh identified GPT-5.6 pricing/context drift, an API-sized Astra fallback, the cancelled Sonnet 5 price increase and Fable/Mythos 5.1 cache-rate drift. See the guide's gap table before presenting runtime estimates as current. Documentation edits alone do not fix that runtime gap.

## Provider and data boundaries

| Source | Default root or path | Owner / override |
| --- | --- | --- |
| Claude transcripts | `~/.claude/projects/` | `src/config.rs`; `CLAUDE_HOME` |
| Claude statusline | `~/.claude/discord-presence-data.json` | Authoritative Claude headline cost/duration when present |
| Claude presence config | `~/.pulse-analytics/claude/discord-presence-config.json` | Schema 6 |
| Pulse analytics | `~/.pulse-analytics/pulse-analytics.db` | Schema 7 since 1.9.1; SQLite WAL, migrations and consistent backups |
| Codex sessions/inventory | `~/.codex/sessions/`, `~/.codex/models_cache.json` | `CODEX_HOME` |
| Codex presence config | `~/.pulse-analytics/codex/discord-presence-config.json` | Schema 13 |
| OpenCode integration config | `~/.pulse-analytics/pulse-opencode.json` | `PULSE_HOME`; database paths belong to OpenCode |

`src/storage.rs` owns Pulse paths under `PULSE_HOME`, defaulting to `~/.pulse-analytics`. Initialize storage before loading preferences or starting pollers. Migration copies and validates legacy files, retains originals and never overwrites destination data. Provider source roots and credentials remain separate. See `docs/guides/storage.md` for recovery and legacy instance-lock compatibility.

- Claude subscription, Codex subscription, Anthropic API, OpenAI API and OpenCode Go are separate sources. A configured credential is not authenticated proof.
- Allowance cards require provider proof. Local history may remain useful when authentication fails. Preserve freshness and unavailable states.
- OpenCode reads `opencode.db` and channel databases, respects `XDG_DATA_HOME`/`OPENCODE_DB`, and never inherits Claude/Codex quotas or prices.
- OpenCode Go uses `/zen/go/v1/usage` with the existing local key. Never derive account percentages from session token totals or copy keys into logs.
- All providers combines analytics but preserves the selected Discord broadcaster. Completed OpenCode sessions remain in history, not live presence.
- Claude raw `usage.input_tokens` excludes cache categories. Pulse's aggregate input includes them. Keep that distinction in formulas and tests.
- Claude current fill resets on valid compaction metadata; the historical peak does not. Codex/OpenCode cumulative usage is not current context fill.
- Windows WSL discovery is off by default; preserve the explicit `CC_PRESENCE_INCLUDE_WSL=1` opt-in and silent subprocess launchers.

## Discord, notifications and UI

Use the existing provider assets and `docs/guides/discord.md` for application IDs and resolver behavior. Local Discord IPC `READY` owns connected identity; cached local discovery is fallback. A missing banner must remain absent. A successful payload is not visual proof that an asset rendered.

The backend owns Discord field order and the preview compositor. Keep unavailable fields unavailable, omit stale live data, and preserve privacy controls. Reports expose only provider-supported recommendations and keep sensitive excerpts behind explicit reveal.

Notification records are durable in SQLite. Read/unread, individual dismissal, bulk clear and Undo share one lifecycle. `dismiss_all_notifications` is reversible; `restore_notifications` must restore the recorded read states. Do not replace clearing with row deletion. Native, tray and in-app state must agree.

Reuse the current monochrome design system, content-sized collections and accessible controls. Keep theme, focus, keyboard behavior, responsive layout and error states intact. Documentation uses descriptive headings, short tables, existing assets and dated evidence; release history belongs in CHANGELOG.md, not duplicate launch pages.

## Local development and proof

Run from the repository root:

```powershell
npm --prefix frontend ci
npm run dev
npm run verify
npm --prefix frontend run build
cargo test --locked --test release_scripts
```

`npm run dev` starts the authenticated loopback Rust bridge and Vite. Browser review uses real Rust commands through Vite's same-origin `/__pulse_api` proxy. Fixtures are test-only. The standalone debug bridge is `cargo run -p pulse --bin pulse-dev-bridge`; follow `docs/architecture/dev-bridge.md` for its token and proxy contract.

For native development, use `npm run dev:tauri`. For a portable embedded UI, use `npm run build:portable`; for installers, use `npm run build`. Do not promote a raw Cargo GUI release that still resolves the development URL. Validate the actual window with ports 1420 and 1421 stopped.

For daemon diagnostics, use `cargo run -p cc-discord-presence -- status` or `cargo run -p cc-discord-presence -- doctor`. Rust-only verification uses `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.

The GUI poll interval is currently 5 seconds in `src-tauri/src/commands.rs`. Read the owning constants and usage cache implementation before changing timing; daemon, session, quota and GUI caches are not interchangeable.

Before replacing a local installation, retain a consistent SQLite backup, config and binary. Confirm the exact executable/PID before stopping Pulse. Preserve other agents and applications. Schema-6 rollback requires the prior compatible database backup; keep the newer database separately.

## Commit and cross-platform release gate

Keep all edits local unless Tony explicitly authorizes the specific commit, push, tag, dispatch or publication. A request to prepare support is not a request to publish a release.

Before an authorized commit, run focused tests and record proof. Before an authorized push, run `npm run verify` and `npm --prefix frontend run build`. Changes to OS-specific code, dependencies, packaging, signing or releases must also pass `cargo test --locked --test release_scripts` (included in the workspace suite).

Use Conventional Commits and SemVer. Keep unreleased work under `Unreleased`. State compatibility, risk and rollback. Keep `docs/maintainers/releases.md`, `docs/maintainers/platforms.md` and `tests/index.md` aligned when these contracts change.

Do not reduce Pulse to one provider or one OS in README claims. Document Claude Code, Codex and OpenCode independently. Keep OS support, provider authentication and published downloads separate. Reuse the existing Pulse banner and monochrome icons; put application captures in Preview and label historical images.

## Releasing (cross-platform)

The normal path is `.github/workflows/release.yml`, manually dispatched against an authorized annotated tag. `publish_release=false` is the default: native Windows/macOS/Linux x64 and ARM64 checks plus unsigned verification artifacts, no publication. Never infer native runtime proof from Windows-only testing or static workflow checks.

Publication requires explicit authorization, `publish_release=true`, every target passing, required installers, six signed updater entries, checksum verification, Windows SPDX and macOS package architecture checks. GitHub macOS packages have no Apple Developer ID signature or notarization; disclose this in release notes. Build the frontend once and promote the same artifact to each native package. Keep the installed-window, provider, Discord, notification and upgrade evidence described in `docs/maintainers/platforms.md`.

Missing native hosts, updater signing secrets or runtime proof must be reported as gaps. Apple credentials are not a gate for the authorized unsigned GitHub DMGs. Never claim full platform support to hide a blocked release. Do not bypass Gatekeeper or substitute fixture output for native proof.

`scripts/release-local.ps1 -WindowsOnlyRecovery` is only for explicitly requested Windows x64 recovery. It must never become latest or satisfy the complete release gate. Published tags and assets are immutable; corrections need a new version. Workflows remain manual-only, with no automatic commit/push/PR triggers.


## Documentation acceptance

Write maintained public documentation in English. Preserve identifiers and quoted evidence.

For documentation-only work, verify local links and anchors, table arithmetic, source dates, code identifiers and rendered reading order. Refresh `docs/index.md` and `llms.txt` when navigation changes. Update `tests/index.md` when validators or test ownership change. Do not run a full binary build merely to claim that prose was tested.

For runtime model changes, test aliases, unknown models, observed/cache/fallback context, rate categories, speed, long-context coverage and provenance. A matching JSON hash does not prove current prices, native execution or a deployed consumer. Record those checks separately.

## Repository hygiene

- Keep `docs/index.md` as the documentation entry point. Provider references belong in `docs/models/`, user procedures in `docs/guides/`, implementation contracts in `docs/architecture/`, and release/integration policy in `docs/maintainers/`.
- Keep equivalent Claude and Codex model references. OpenCode metadata remains provider/model-scoped; do not copy Claude/Codex prices into it.
- Keep accepted visual proof under `assets/evidence/` with its dated receipt in `docs/evidence/`. Historical proof is not evidence for the current binary.
- Store task scratch, browser captures, local agent state and handoffs outside this checkout. Do not recreate `.claude`, `.claude-quality`, `.codex`, `.stealth-output`, `.zcode` or `.design-refs` here for temporary work. Preserve global user configuration.
- Root build/config manifests, `.git`, `.github`, source, tests and dependency/build caches have real consumers. Do not delete them for visual neatness.
- Before retiring a document, merge maintained content, repair references and retain a recoverable copy. Verify exact targets and hashes before and after moves. Preserve uncommitted changes and other worktrees.
