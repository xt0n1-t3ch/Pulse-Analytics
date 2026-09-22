# Pulse test suite

Centralized tests for the whole app. Frontend logic/integration/component-render runs on Vitest
5 (happy-dom); the daemon + analytics core run on `cargo test`. The Vitest config lives in
`frontend/` but the spec tree is rooted here at `tests/`; run both layers before shipping.

## Run

Vitest 5 requires Node.js `^22.12.0 || ^24.0.0 || >=26.0.0` and Vite 6.4 or later. Keep the npm and Bun lockfiles synchronized.

| Layer | Command | What it covers |
|:---|:---|:---|
| Frontend | `bun run --cwd frontend test` | Vitest unit + integration + component render (`tests/unit`, `tests/integration`, `tests/components`) |
| Frontend (watch) | `bun run --cwd frontend test:watch` | Same, watch mode |
| Release contracts | `cargo test --locked --test release_scripts` | Six native runner targets, publication opt-in, macOS architecture checks and updater signing gates, unsigned verification isolation, required public packages and Windows-only recovery exclusion from latest |
| Public documentation | `npm --prefix frontend run test -- tests/unit/documentation-contract.test.ts` | Valid UTF-8, mojibake prevention, local documentation links, provider metadata and the tracked Claude guide |
| Backend | `cargo test --workspace` | Both Rust crates (`cc-discord-presence` daemon/core + `pulse` Tauri host): inline `#[cfg(test)]` modules + the `tests/*.rs` / `src-tauri/tests/*.rs` integration tests |
| Browser bridge | `cargo test -p pulse --lib dev_bridge` | Loopback-only POST contract, Bearer-token auth, strict local CORS, real command dispatch, safe-control argument guards, and explicit unavailable/unknown behavior |
| Real browser development runtime | `bun run --cwd frontend dev` | Starts Vite plus the authenticated Rust bridge with one generated shared token; `1420` fails closed instead of rendering fixtures when the backend is unavailable |
| Browser Playwright | `bunx --cwd frontend playwright test` | Reuses the authenticated local bridge outside CI, validates proofed-source selection and exact metrics, then verifies the `390px` page width and viewport-bound notification panel |
| Types | `bun run --cwd frontend check` | svelte-check, 0 errors / 0 warnings |
| Codex mirror integrity | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-codex-rich-presence-upstream.ps1` | Verifies the pinned annotated tag, exact commit, inventory, adapters, and every vendored target hash in `src/codex/UPSTREAM.json`; the scheduled drift job tracks later upstream releases separately |
| Windows polling consoles | `cargo test --locked --test release_scripts vendored_windows_polling_commands_use_silent_launcher -- --exact` | Rejects the old canonical pin and any five-second Git probe that bypasses the shared `CREATE_NO_WINDOW` launcher |
| Provider/metrics fixture contract | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/e2e/test-pulse-contract.ps1` | Parses 11 sanitized provider/Discord fixtures plus true-zero/unavailable metrics vectors; checks version/source-contract and cost exactness alignment |
| Pulse E2E harness dry-run | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/e2e/run-pulse.ps1 -Mode Browser -DryRun` | Stages temporary homes/DB/provider config and checks the Browser launch prerequisites without starting a server |
| Native Tauri CDP + Playwright E2E | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/e2e/run-pulse.ps1 -Mode Tauri -Fixture no_data -RunPlaywright` | Starts owned Vite on `1420` before the repo-owned debug binary; native Tauri alone owns IPC and polling. It isolates WebView2 user data, runs `chromium.connectOverCDP`, validates the no-data Home contract, and cleans only exact owned processes. |

Vitest config: [frontend/vitest.config.ts](../frontend/vitest.config.ts) (alias `@` → `frontend/src`,
`fs.allow` widened to the repo root so the root-level spec tree resolves). Tauri IPC, the event/app/
window/dialog/fs plugins, and Chart.js are all mocked in [tests/setup.ts](setup.ts) so store/view
modules import cleanly outside a WebView; a WAAPI `Element.prototype.animate` stub plus a `matchMedia`
shim let Svelte transitions and media queries run headless. The fake `__TAURI_INTERNALS__.invoke`
routes known list commands, including `get_context_breakdowns`, to `[]`, scalars to stub values, and `get_reports_bundle` to an empty bundle.

## E2E boundary: Vitest, Playwright, Tauri CDP, and Discord IPC

The layers intentionally prove different seams; one green layer is not a
substitute for another:

- **Vitest** (`npm --prefix frontend run test`) owns unit, integration, and
  component rendering against the mocked Tauri bridge. It does not prove a
  real WebView, provider authentication, or Discord IPC.
- **Playwright** (`node tests/visual/verify-responsive.mjs`) owns the browser
  visual matrix against a running URL (`PULSE_VISUAL_URL`, default
  `http://127.0.0.1:1420`). The fixture runner's Browser mode only starts Vite
  and probes `/`; it does not silently claim Playwright or Tauri coverage.
- **Tauri CDP** is available through
  `scripts/e2e/run-pulse.ps1 -Mode Tauri -RunPlaywright` only after a
  repo-owned debug binary exists. The runner starts `bun run dev` from the
  repo-owned `frontend/` directory first because the debug binary's
  `devUrl` is `http://localhost:1420`; it verifies the bridge/Vite listeners
  are descendants of that launcher before starting Pulse. It then uses a fresh WebView2
  remote-debugging port plus an isolated temporary user-data folder (the
  installed Pulse profile is never reused), probes `/json/version`, runs the
  Playwright spec with `chromium.connectOverCDP`, and fails clearly when the
  binary or endpoint is absent; no backend endpoint is guessed.
- **Raw Discord IPC** is available through `-Mode Discord` only with
  `PULSE_DISCORD_LIVE=1`, an already-running Discord desktop process, and a
  proof JSON path. The proof must contain `SET_ACTIVITY` with an activity and a
  clear-activity event. The runner never restarts Discord and does not treat a
  frontend preview as raw IPC evidence.

The provider vectors and expected DTO are documented in
[adaptive-access.md](../docs/architecture/adaptive-access.md). They are
synthetic contract inputs, not live provider snapshots.

| File | Seam | Coverage |
|:---|:---|:---|
| [workspace.spec.ts](../frontend/tests/e2e/browser/workspace.spec.ts) | Browser E2E | proofed provider sources remain separate, unproved API lanes stay hidden, exact metrics remain exact, Home occupies over 96% of the reference viewport, the zero-proof Home contract releases the allowance rail while keeping live work visible, and all primary views are checked at `390px` without page-level overflow |
| [pulse.spec.ts](../frontend/tests/e2e/tauri/pulse.spec.ts) | Tauri CDP E2E | repo-owned WebView renders the current Provider limits/Live workspace Home shell through the real Tauri surface |
| [production.spec.ts](../frontend/tests/e2e/tauri/production.spec.ts) | Installed production E2E | installed executable serves the embedded UI instead of the development URL, reports the expected product version, answers real native IPC, renders Accounts, and previews currency-only Codex subtotals. It runs against the installed application with Vite and the development bridge stopped |

The zero-proof rail contract is enforced by the focused Vitest counterpart
`bun run --cwd frontend test -- tests/components/Dashboard.test.ts` and the
browser proof
`bun run --cwd frontend test:e2e -- --grep "Home releases the allowance rail"`.
Home keeps live session telemetry visible while releasing the allowance column
until an authenticated provider route supplies quota proof.

## Frontend — unit (`tests/unit/`)

| File | Module under test | Coverage |
|:---|:---|:---|
| [utils.test.ts](unit/utils.test.ts) | `lib/utils` | `fmtTokens` (B/M/K/unit tiers), `fmtCost` ($ two-decimal), `fmtDuration` (s/m/h+m), `fmtTps` (/s→K/s), `fmtPct` (whole-percent rounding), `usageColor` (normal/warning/danger), `classifyActivity` (thinking/editing/reading/running/waiting/idle), `fmtClock` (HH:MM passthrough + em-dash fallback on null·undefined) |
| [access.test.ts](unit/access.test.ts) | `lib/access` | provider/subscription vs API labels, authenticated-route vs local-history display projections, dynamic provider-native windows, no numeric output for stale or unavailable usage, stable source-id to provider-scope resolution, and no API-to-subscription aliasing |
| [provider.test.ts](unit/provider.test.ts) | `lib/provider` | optimistic latest-intent provider selection, serialized persistence, stale bootstrap rejection, and provider revision publication only for the surviving selection |

## Frontend — integration (`tests/integration/`)

| File | Surface | Coverage |
|:---|:---|:---|
| [tauri-mock.test.ts](integration/tauri-mock.test.ts) | `lib/api` over the mocked Tauri IPC | list commands resolve to `[]`, mapped scalar (`get_active_provider`→`"claude"`), unmapped command resolves to `undefined` without throwing |
| [poll-flow.test.ts](integration/poll-flow.test.ts) | `stores.poll()` → global stores → `Dashboard.svelte` | one `poll()` pass calls each loader exactly once and hydrates `health`/`metrics`/`sessions`/`rateLimits`/`planInfo` + derived `activeSessions`; provider changes synchronously clear proof-bound state and reject prior-provider responses in flight; Dashboard then renders the provider/work shell and two live session cards end to end |
| [phase5-flow.test.ts](integration/phase5-flow.test.ts) | `Reports.svelte` data flow | Reports renders the bundle through a single `getReportsBundle` call |

## Frontend — component render (`tests/components/`)

DOM-render tests for every view/component via `@testing-library/svelte` on happy-dom. Tauri is
satisfied through the injected internals in [setup.ts](setup.ts); the Chart.js view (`Costs`) swaps in
the [fixtures/ChartStub.svelte](fixtures/ChartStub.svelte) stub so canvas-bound charts render headless.

| File | Component | Coverage |
|:---|:---|:---|
| [DesignSystem.test.ts](components/DesignSystem.test.ts) | shared UI contract | canonical neutral dark surfaces, monochrome shell accent, semantic status tokens, Codex-blue provider identity, labeled navigation, coherent primary-view headers/states, flat metric strips, and removal of the Sessions decorative glyph |
| [PulseMark.test.ts](components/PulseMark.test.ts) | `PulseMark` | svg sized to the `size` prop, P-glyph-only when `showPulse` is false (1 path), P glyph + pulse line when true (2 paths) |
| [SessionCard.test.ts](components/SessionCard.test.ts) | `SessionCard` | fast badge present/absent on the `fast` flag, inflated-tokenizer marker shown for opus 4.7+ and Sonnet 5 (sourced from the backend `has_inflated_tokenizer` flag, not a local regex) and omitted for 4.6, Opus 4.8 model display name, Fable/Mythos badges without tokenizer warnings, Sonnet 5 "Intro Pricing" badge presence/absence driven by `session.intro_pricing` |
| [Dashboard.test.ts](components/Dashboard.test.ts) | `Dashboard` (view) | selectable multi-instance focus, exact Context Window fraction, unique session-status rail, flat four-KPI strip, account quota + remaining percentage, and reconciled cost/model data |
| [AccessSourceBar.test.ts](components/AccessSourceBar.test.ts) | `AccessSourceBar` | authenticated subscription/API lanes, local-history-only provider selection, hidden unproved/no-history providers, and separation between analytics scope and the active Discord provider |
| [WorkspaceSurfaces.test.ts](components/WorkspaceSurfaces.test.ts) | `AllowanceRail` + `DataSourceInspector` | honest empty states plus selected-provider allowances without fabricated windows |
| [NotificationCenter.test.ts](components/NotificationCenter.test.ts) | `NotificationCenter` | durable notification loading, unread count, quota-action routing, mark-all-read refresh, last-good preservation on transport failure, stale-response rejection, persistence-failure navigation, and native tray-open events |
| [Sessions.test.ts](components/Sessions.test.ts) | `Sessions` (view) | flat KPI strip labels, live session rows + "2 active", history table loaded from the api layer |
| [Costs.test.ts](components/Costs.test.ts) | `Costs` (view) | Subscription Value Ledger for unavailable money, exact/partial coverage boundaries, token mix/trend, budget cockpit for known spend, Cost-by-Type reconciliation, window-aggregate KPIs, project refetch, and live-snapshot refresh |
| [Heatmap.test.ts](components/Heatmap.test.ts) | `Heatmap` | 24 local-hour cells, total/coverage/peak summaries, proper AM/PM labels, and accessible volume context |
| [VersionContract.test.ts](components/VersionContract.test.ts) | release owners | v1.9.4 synchronization across Cargo, Tauri, frontend, both lock surfaces of `frontend/package-lock.json`, the release contract and the changelog, plus the immutable canonical core release and commit |
| [UpdateBanner.test.ts](components/UpdateBanner.test.ts) | `UpdateBanner` | automatic update popup, Later/Skip/Open release actions, skipped-version behavior, fake dev update, one explicit Update action followed by signed install and automatic relaunch, retryable failures |
| [Reports.test.ts](components/Reports.test.ts) | `Reports` (view) | coherent analysis header/copy, sections populated from a single bundle call, reload feedback, and cost timeline totals/peaks |
| [Discord.test.ts](components/Discord.test.ts) | `Discord` (view) | coherent Broadcast header, live-preview backend payload, provider capability gates, autosave saving/saved lifecycle, rollback on failed persistence, field reorder/toggles, and theme-aware preview |
| [Settings.test.ts](components/Settings.test.ts) | `Settings` (view) | coherent Application header, identity masthead + config controls, db size + session total, plan override rollback, latest-provider-wins race handling, two-step clear-history confirm, and theme toggle |

## Backend — Rust (`cargo test --workspace`)

Two workspace crates. `cc-discord-presence` (repo root) is the daemon + analytics core;
`pulse` (`src-tauri/`) is the Tauri host that depends on it. `--workspace` runs both crates' inline
`#[cfg(test)]` modules plus the cargo integration tests below. All green.

### Integration tests (`tests/*.rs`, `src-tauri/tests/*.rs`)

| File | Crate | Coverage |
|:---|:---|:---|
| [daemon_e2e.rs](daemon_e2e.rs) | `cc-discord-presence` | end-to-end daemon pipeline over temp JSONL fixtures: Claude session collect accumulates speed-aware per-category cost and reconciles categories to the headline total; tracks last-turn speed/effort/service-tier and builds presence lines (project, "Opus 4.8 (1M)", fast âš¡ marker, effort label); Codex session parses meta/turn-context/token-count, resolves effort/window/totals, and builds presence state (model display, "(Extra High)", fast marker, "Pro 20x ($200/month)") |
| [codex_upstream_contract.rs](codex_upstream_contract.rs) | `cc-discord-presence` | Pulse-facing contract for the mirrored Codex Rich Presence modules: config, cost, display labels, telemetry limits, active-session selection, OpenCode process compatibility, and Windows WSL opt-in/no-window subprocess safety |
| [codex_account_usage.rs](codex_account_usage.rs) | `cc-discord-presence` | canonical `account/rateLimits/read` parsing, coherent global quota + spend Credits, structured `rateLimitResetCredits` preservation, exact used/remaining arithmetic, omission of unreported model scopes, and rejection of empty sparse responses |
| `src/codex/account_usage.rs` unit tests | `cc-discord-presence` | Windows Codex CLI selection prefers the user-local unpackaged binary over an inaccessible AppX path while retaining the desktop GUI rejection guard |
| [access_runtime.rs](../src-tauri/tests/access_runtime.rs) | `pulse` | provider-neutral access routes, local-history capability without authentication promotion, dynamic sparse windows, provider-plan/proof separation, freshness gating, fail-closed mismatched/unknown usage sources, and separate structured Codex reset-credit DTOs |
| [notifications.rs](../src-tauri/tests/notifications.rs) | `pulse` | genuine Codex/Claude reset transitions, timestamp-drift silence, legacy timestamp-only row dismissal migration, provider-health silent baselines, quota deduplication, durable unread/read state, and dismissal filtering |
| [startup_order.rs](../src-tauri/tests/startup_order.rs) | `pulse` | Tauri single-instance registration remains ahead of setup and background-poller startup, so a rejected second launch cannot create another notification/analytics producer |
| [reports_e2e.rs](../src-tauri/tests/reports_e2e.rs) | `pulse` | `build_reports_bundle_from_roots` aggregates fixture traces (user/assistant/tool/mcp/compaction counts, cache health); regression guard that the JSONL tree is scanned exactly once per bundle (no double/8x scan); oversized JSONL over `MAX_JSONL_BYTES` is skipped while small files still trace |
| [report_html.rs](../src-tauri/tests/report_html.rs) | `pulse` | `generate_html_report` / `generate_markdown_report`: writes a sample HTML for `/browser` visual review; offline-safe (no Google Fonts / gstatic / `@import` / `http://`, https only in w3.org+github namespaces); well-formed doctype + single `<html>`, inline `<style>`, inline SVG charts (token-composition aria-label); brand kicker + KPI strip + all eight analyzer section anchors + Speed Split; offline system/monospace font stacks; markdown is non-empty GFM with every section heading + the speed-split table header |

### Inline `#[cfg(test)]` modules

Per-crate, per-module unit tests compiled with each crate. Representative coverage:

| Area | Module | Coverage |
|:---|:---|:---|
| cost / pricing | `src/cost.rs` | per-tier pricing, Fable/Mythos rates, Opus 5.5 version-specific rates (0.05x cache read, $8/$40 Fast) kept separate from Opus 5, Sonnet 5 introductory/standard pricing across the clock-injected cutoff boundary, digit-boundary-safe Sonnet 5 id classification, cache math, speed-aware totals, 1M-context surcharge, GA no-surcharge table, fast-capable model table |
| presence lines | `src/discord.rs` | Claude presence details/state/tooltip composition across model/effort/speed/marker permutations, including Fable 5 (1M) and Mythos 5 (1M) labels |
| session collect | `src/session.rs` | JSONL parse, token/cost accumulation counted once per `message.id` at its latest usage (repeated content-block lines, streamed output growth, interleaved responses), build-stamped session checkpoints that another Pulse build never restores, reasoning-effort + speed + service-tier extraction, git-branch + parse caching; compaction-boundary detection resets `current_context_tokens` to the real post-compaction size while `max_turn_api_input` (the 1M-tier lifetime peak) survives unchanged, including the missing-`compactMetadata`, compaction-as-first-line, and zero-compaction edge cases |
| metrics / usage | `src/metrics.rs`, `src/usage.rs` | aggregate metrics rollups, plan/usage-window derivation |
| config | `src/config.rs`, `src/codex/config.rs` | presence/pricing config defaults + round-trip + Windows WSL opt-in flag parsing |
| util / process | `src/util.rs`, `src/process_guard.rs` | path/format helpers, single-instance process guard |
| codex core | `src/codex/{session.rs,session/parser.rs,cost.rs,discord.rs,util.rs,process.rs}` | Codex JSONL parse, cost, presence lines, helpers |
| codex telemetry | `src/codex/telemetry/{plan.rs,service_tier.rs,limits.rs}` | provider-scoped exact plan tiers (no Codex Max or generic Pro), service-tier resolution, rate-limit window parsing |
| Windows test harness | `src-tauri/build.rs`, `src-tauri/src/lib.rs` | embeds the shared Common-Controls v6 + Tauri `asInvoker`/`uiAccess=false` manifest; Cargo's current Windows lib harness cannot consume a build-script test-only link flag, so packaged RT_MANIFEST readback remains a release gate |
| db | `src-tauri/src/db.rs` | SQLite historical-session insert/query/round-trip + context snapshot storage clamped to the model window; Codex JSONL collect → upsert → 7d/month forecast integration; inclusive now-6d/exclusive now-8d window boundary; retryable write fingerprints; exact provider isolation plus explicit `all` aggregation; provider history inventory independent of cost/authentication; stored exact/partial/unavailable/provider-billed cost provenance round-trip; Summary, daily, project, hourly, model, and Reports timeline aggregates exclude unavailable raw estimates, average only priced sessions, and expose aggregate coverage |
| analyzers | `src-tauri/src/analyzers/{session_trace.rs,cache_health.rs,model_routing.rs,prompt_complexity.rs,inflection.rs}` | trace scan + scan-pass counting, cache-health grading, provenance-aware model-routing split and inflection detection that cannot promote unavailable raw estimates, prompt-complexity scoring |
| commands | `src-tauri/src/commands.rs` | access snapshots enriched with local-history inventory; reports-bundle assembly from roots; provenance-aware cost totals that retain token categories even when money is unavailable; daily timeline points that exclude unavailable raw estimates and report partial coverage; Codex weekly-only routes are never duplicated into the legacy 5h slot when a model-scoped weekly window is also present; `SessionInfo.intro_pricing`/`has_inflated_tokenizer` wiring for Claude sessions (real-clock, matched against a fresh `cost::active_intro_pricing` call so the test never goes stale across the real cutoff date) and confirmed absent for Codex sessions; `SessionInfo.context_used_tokens` and `build_claude_context_breakdown`'s `used_tokens` reflect current fill (`current_context_tokens`) rather than the historical peak (`max_turn_api_input`), while `context_window_tokens`/the 1M-vs-200K decision still correctly keys off the peak |
| update checks | `src-tauri/src/update_check.rs` | SemVer tag comparison, newer-release detection, prerelease/draft suppression, release URL allowlist |
| browser bridge | `src-tauri/src/dev_bridge.rs` | Debug-only `127.0.0.1:1421` POST transport, Bearer auth, Vite-origin allowlist, bounded request/response transport, validated safe-control arguments, real-command dispatch, and standalone poller seam |

## v1.4.1 targeted validators

Run these before cutting the context-tracking-fix release:

```bash
cargo test --workspace --jobs 2 compact_boundary
cargo test --workspace --jobs 2 current_context_tokens
cargo test -p pulse --lib --jobs 2 reflects_current_fill
npm --prefix frontend run test -- tests/components/Dashboard.test.ts tests/components/Costs.test.ts
npm --prefix frontend run check
```

Live re-proof (not part of the automated suite -- run manually before release): rebuild, relaunch
with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`, and re-issue the
exact original repro (`window.__TAURI_INTERNALS__.invoke("get_context_breakdown", {sessionId})`
via CDP `Runtime.evaluate`) against the real session that first surfaced the bug; confirm
`used_tokens` is no longer pinned at the stale historical peak.

## v1.4.0 targeted validators

Run these before cutting the Sonnet 5 release:

```bash
cargo test --workspace --jobs 2 sonnet_5
cargo test -p pulse --lib --jobs 2 intro_pricing
npm --prefix frontend run test -- tests/unit/utils.test.ts tests/components/SessionCard.test.ts
npm --prefix frontend run check
```

Full pre-ship gate (see below) applies as usual. Note: this environment compiles the `pulse`
crate's full dependency tree (Tauri + the `icu_properties`/`idna` chain it pulls in) under
tight available memory — pass `--jobs 2` to `cargo` invocations that touch the `pulse` package
or the build can hit `STATUS_COMMIT_LIMIT_EXCEEDED` and cascade into unrelated-looking
compile errors in transitive dependencies. This is an environment/parallelism characteristic,
not a code defect — see the Sonnet 5 handoff for the diagnosis.

## v1.2.0 targeted validators

Run these before cutting the Fable/Mythos + multi-session Context release:

```bash
cargo test --workspace fable mythos
cargo test --workspace presence
npm --prefix frontend run test -- tests/integration/phase5-flow.test.ts tests/components/Discord.test.ts tests/components/SessionCard.test.ts
cargo test -p pulse update_check --lib
cargo test --workspace --test codex_upstream_contract
cargo test --workspace session_used_tokens_uses_context_snapshot_not_lifetime_total
cargo test --workspace wsl_roots_are_explicit_opt_in
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-codex-rich-presence-upstream.ps1
npm --prefix frontend run test -- tests/components/UpdateBanner.test.ts
npm --prefix frontend run check
```

Full pre-ship remains:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-codex-rich-presence-upstream.ps1
npm --prefix frontend run test
npm --prefix frontend run build
```

## Project cross-reference

| Doc | Where | Purpose |
|:---|:---|:---|
| [README.md](../README.md) | repo root | Install, feature overview, daemon + GUI quick start |
| [CHANGELOG.md](../CHANGELOG.md) | repo root | Release history (Claude config schema v6, Codex config schema v13, DB schema v6 in 1.8.2 and v7 in 1.9.1) |
| [docs/index.md](../docs/index.md) | `docs/` | Documentation hub: architecture, Discord assets, reasoning-effort variants, analyzers, cost calculation |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | repo root | Contribution + local-dev workflow |

## OpenCode and Astra

- `src/opencode/store/tests.rs`: both message schemas, model families, mixed models, zero/unknown costs, pagination and locked/incompatible SQLite.
- `src/opencode/process.rs`: managed PID, parent and executable identity; stale records are rejected.
- `src/opencode/presence.rs`: privacy, OpenCode identity and no invented costs.
- `src-tauri/src/db.rs::opencode_tests`: persisted metadata and history without borrowed prices.
- `tests/components/AccessSourceBar.test.ts` and `Discord.test.ts`: independent filters, native application and unavailable capabilities.
- `scripts/check-model-catalog-parity.ps1`: byte equality with the canonical catalog.
- Browser accessibility checks need keyboard, screenshots and real Discord visual proof as well as axe-core.

`every_plan_override_survives_reload_and_matches_the_discord_payload` covers manual precedence, reload, invalid values and return to Auto-detect. `account_plan_is_independent_from_quota_windows` separates identity from quotas.

`opencode_go::tests` checks three windows and actual monthly reset dates. `discord_identity::tests` validates READY and excludes credentials from the DTO. OpenCode regressions cover freshness, Go attribution, quota toggle persistence, presets, compositor order and the opencode-v2 logo.

## Provider-neutral storage

`tests/storage_migration.rs` runs the real daemon startup with isolated roots. It checks first-start migration, config writes, restart behavior and rejection of relative `PULSE_HOME` paths before writing state.

`src/storage.rs::tests` checks consistent migration from an active WAL database, schema retention, settings copies, credential exclusion, repeat startup, existing destination preservation, corrupt JSON recovery and corrupt database rejection. GUI command tests isolate `PULSE_HOME` as well as both provider roots. The native/browser E2E launcher also isolates `PULSE_HOME`.

## Windows efficiency and release validation

`src-tauri/src/update_check.rs` checks canonical and legacy GitHub release URLs. Its repository-rename regression rejects unrelated repositories, lookalike domains and non-HTTPS links while preserving valid release-page and installer links.

`src/power.rs` tests option parsing and the Windows ABI layout. `scripts/check-windows-efficiency.ps1` reads the running process policy. Release checks validate versions, PE architecture, software bills of materials and checksums.

Notification regressions cover per-item/bulk read state, soft clear and Undo, command routing and confirmation. OpenCode lifecycle tests distinguish null completion from completed responses and reject stale tool-state overrides.

## Accounts, Command Code and cost coverage

- `src-tauri/src/accounts/`: protected profile references, unlink tombstones, callback origin/state validation, zero/optional usage and monthly allocation independent of purchased/free credits.
- `tests/components/Accounts.test.ts`: independent selection, two-account separation, reconnection, removal confirmation, stale and unavailable data, Pro tiers, reset counts and cycle-versus-expiry labels.
- `tests/components/WorkspaceSurfaces.test.ts`: Command Code artwork and calendar-month allowances.
- `src/commandcode/`: shared CLI/Desktop identity, message/session deduplication, active-versus-completed history, BYOK and privacy.
- `tests/commandcode_live.rs`: ignored real-history proof and separately opted-in local Discord diagnostic acknowledgement. These tests never run in the ordinary workspace suite.
- `frontend/tests/e2e/tauri/accounts.spec.ts`: real account observations through native IPC, no browser-proxy calls and the native folder-picker affordance. Requires an isolated native run and `PULSE_REAL_ACCOUNT_PROOF_DIR`.
- `src/codex/session/costing.rs`: repeated events, cache writes, model/speed changes and per-request long-context pricing. Canonical equivalents live in the owner repository.

The canonical core's reserved-cost compositor ships in the pinned `codex-presence-core` 2.0.1 from upstream v1.11.2, so the ordinary pinned build covers it and no Cargo override is required. Browser, native IPC, provider API, published asset inventory and Discord acknowledgement are separate proof layers. Discord visual rendering and native macOS/Linux account sign-in remain host-specific acceptance checks.

A version bump must update `Cargo.toml`, `src-tauri/Cargo.toml`, both `Cargo.lock` workspace packages, `package.json`, `frontend/package.json`, both version fields in `frontend/package-lock.json`, `src-tauri/tauri.conf.json`, `scripts/release-contract.json`, `src/codex/UPSTREAM.json` and `tests/components/VersionContract.test.ts`. `frontend/bun.lock` records no product version, so a bump does not change it; do not regenerate that lockfile to synchronize a version.

[production.spec.ts](../frontend/tests/e2e/tauri/production.spec.ts) proves the installed application, not a release. It requires a completed local installation, stopped ports 1420 and 1421, and the real user profile paths. A development installer that is built with a scratch Tauri configuration and `createUpdaterArtifacts=false` produces no updater artifact and no signature, so it cannot stand in for a published installer. Record the previous installed executable and its SHA-256 before replacement, so rollback stays possible.

`src/commandcode/presence.rs` covers enabled idle payloads, completed/stale session exclusion, disabled clearing and absent idle session timers. Its ignored `local_discord_acknowledges_idle_from_the_production_publisher` test exercises the production publisher against real Discord and requires `PULSE_COMMANDCODE_IDLE_PROOF`; stop other Pulse publishers first. `tests/components/Discord.test.ts` checks provider-specific idle labels, published idle status and disconnected behavior.
