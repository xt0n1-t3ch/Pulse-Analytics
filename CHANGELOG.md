# Changelog

All notable changes to **Pulse** are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning is [SemVer](https://semver.org/).

## [Unreleased]

### Fixed

- Show the current Claude plan after an upgrade or downgrade. Claude Code keeps the login-time `rateLimitTier` in `.credentials.json` but refreshes `oauthAccount` in `.claude.json`, so Pulse kept showing Max 5x after an upgrade to Max 20x. Pulse now prefers the account profile's recognized tier (`userRateLimitTier`, then `organizationRateLimitTier`) and falls back to the credentials file.
- Bill Claude Sonnet 5 at its permanent $2/$10 rates ($2.50 cache writes, $0.20 cache reads). Anthropic cancelled the scheduled September 1, 2026 increase, but Pulse switched to $3/$15 after that date. The "Intro Pricing" badge is removed.
- Bill Claude Fable/Mythos 5.1 cache reads at $0.25 per million. Pulse used version 5's $1 rate for every Fable/Mythos version.
- Point the Discord button, updater fallback links, install scripts, crate metadata and documentation at the repository's current name, `xt0n1-t3ch/Pulse-Analytics`. The updater still accepts release links under the former name.
- Store Claude session checkpoints under `PULSE_HOME/claude/session-checkpoints/` instead of inside Claude's own `~/.claude` folder. File names use a stable hash, so a Rust toolchain update no longer orphans them. At startup, Pulse removes checkpoints from another version, checkpoints for deleted transcripts and the old folder in `~/.claude`.
- Ignore the Claude usage cache when it belongs to another account. The cache now stores a hash of the account and organization IDs, never a token, so switching accounts no longer shows the previous account's limits for up to five minutes. The cache is written atomically because the daemon and the app can both update it.

### Documentation

- Record six-platform 1.9.3 and 1.9.4 publication and the installed Windows x64 upgrade evidence, including the Opus 5.5 cost recomputation.

## [1.9.4] - 2026-09-22

Claude token and cost accuracy fix. The database remains schema 7, Claude config schema 6 and Codex presence config schema 13 are unchanged, and the immutable shared core stays at 2.0.1. Rollback to 1.9.3 needs only its package. Session checkpoints from earlier builds are ignored, so the first poll after an upgrade re-reads active Claude transcripts once. Rows already stored in analytics history are rewritten only when Pulse re-reads a session that is still active. GitHub macOS packages have no Apple Developer ID signature or notarization.

### Fixed

- Count each Claude API response once. Claude Code writes one JSONL line per content block, and every line repeats the response's usage. Pulse added every line, which overstated tokens and costs, often by two to three times. Pulse now keys usage by `message.id` and keeps the latest value, because `output_tokens` can grow while a response streams.
- Discard Claude session checkpoints written by another Pulse build. Before this fix, a session that started before an upgrade kept the costs the older build had accumulated, so new model rates applied only to later turns.

## [1.9.3] - 2026-09-22

Claude Opus 5.5 release. The database remains schema 7, Claude config schema 6 and Codex presence config schema 13 are unchanged, and the immutable shared core stays at 2.0.1. Rollback to 1.9.2 needs only its package; historical session costs already stored are not repriced. GitHub macOS packages have no Apple Developer ID signature or notarization.

### Added

- Claude Opus 5.5 (`claude-opus-5-5`) support: official $4/$20 per million input/output, $5 5-minute cache writes and $0.20 cache reads (0.05x input), 1M GA context without a long-context surcharge, Fast mode at $8/$40, display names, Rich Presence labels, model routing and reports. Rates verified September 22, 2026 against Anthropic's pricing page.

### Fixed

- `claude-opus-5-5` no longer falls through to Opus 5's $5/$25 rates, which overstated Opus 5.5 estimates by 25% and cache reads by 150%. Opus 5 classification now requires exactly version 5.0.

### Documentation

- Add Opus 5.5 to the Claude model guide, price table, Fast list and implementation-gap table.
- Record six-platform 1.9.2 publication, downloaded-asset verification and installed Windows upgrade evidence, with remaining host-specific acceptance gaps.

## [1.9.2] - 2026-09-20

Patch correction for Command Code Rich Presence. The database remains schema 7, provider configuration is unchanged, and the immutable shared core stays at 2.0.1. Rollback to 1.9.1 needs only its package; rollback to 1.8.2 still requires the pre-schema-7 database backup. GitHub macOS packages have no Apple Developer ID signature or notarization.

### Fixed

- Keep Command Code visible on Discord while idle when it is the selected broadcaster and Rich Presence is enabled. Use the same application and artwork for CLI and Desktop. Clear presence when disabled or when another broadcaster is selected.
- Exclude historical session fields and session elapsed time from idle activity. Active sessions still publish their current data with existing privacy controls.
- Show Command Code's own idle label instead of "OpenCode is idle". Distinguish a published idle activity from a disconnected Discord client and remove the misleading publisher-ownership copy.

### Tests

- Cover idle, stale/completed sessions, disabled publication, provider labels and disconnected states. Verify the production idle publisher with real Discord acknowledgement and native UI toggle-off/toggle-on checks.

## [1.9.1] - 2026-09-20

Pulse 1.9.1 delivers the compatible 1.9 feature line: it adds Accounts, the Command Code provider and Codex cost coverage without removing existing behavior. The analytics database migrates additively to schema 7. Claude config schema 6 and Codex presence config schema 13 are unchanged. The shared `codex-presence-core` pin moves to 2.0.1 at `0caece60a71eb657d7b829e3b6c7c4896a0a536c` (canonical v1.11.2), which publishes per-event Codex cost accumulation and the reserved compositor cost space.

Rollback: Pulse 1.8.2 cannot open a schema 7 database. Reinstall the published 1.8.2 package and restore the database backup taken before the schema 7 migration. Account removal writes a tombstone; it does not sign out a provider agent and does not delete transcripts. GitHub macOS packages carry no Apple Developer ID signature or notarization.

### Added

- Add Accounts with independent provider connections, native allowances, credits, banked resets, per-account refresh state and protected profiles. Removing a link leaves agent logins and session history intact.
- Add one Command Code CLI/Desktop adapter across analytics, reports, Accounts and Discord, with official artwork and a shared identity.

### Fixed

- Accept canonical `Pulse-Analytics` release links after the GitHub repository rename, while retaining legacy links and rejecting unrelated repositories. Update the updater endpoint to the canonical repository.
- Preserve Codex known session-cost subtotals, cache-write telemetry and per-event model/speed pricing. Keep coverage and provenance in Pulse while showing currency-only amounts in Discord.
- Reserve enabled monetary values when long model labels fill Discord's line budget.
- Preserve native calendar-month usage and distinguish inactive OpenCode Go access from authenticated allowances.

### Security

- Update `rustls` to 0.23.45 and `rustls-webpki` to 0.103.15 for RUSTSEC-2026-0285, which affects TLS 1.3 handshake validation.

### Changed

- Upgrade Vitest and coverage to 5.0.0, align the Node.js/Vite baseline, and synchronize npm and Bun resolution. This changes contributor tooling, not the application runtime or data schema.
- Link the current downloads for all six targets and record publication, checksum, signature and native-runtime acceptance boundaries.
- Remove superseded release-preparation instructions and document the promoted core and schema-7 rollback boundary.
- Promote the shared `codex-presence-core` pin to 2.0.1 from the immutable upstream v1.11.2 release, replacing the 2.0.0 pin at canonical v1.10.3.

## [1.9.0] - 2026-09-19

Unpublished candidate. The annotated tag is retained unchanged. Publication was cancelled before any release assets became public because the updater rejected canonical links after the repository rename. Version 1.9.1 includes the candidate features and the correction.

## [1.8.2] - 2026-09-05

Patch release with no configuration or database schema change. Native cross-platform packages require the complete build, checksum and updater-signature gate. macOS DMGs and app archives are distributed through GitHub without Apple Developer ID signing or notarization; macOS may block first launch. This is not an App Store release. Known model-rate discrepancies remain documented; this release does not change runtime pricing.

### Fixed

- Pass the macOS executable before the architecture list to `lipo`, so successful DMG builds pass the native package check.

- Repair UTF-8 documentation text, include OpenCode in package and repository discovery metadata, and clarify local analytics, costs and network access.
- Use Node 24 artifact actions and the repository retention policy without deprecated-runtime or retention-limit warnings.

- Use host-native fixture paths in the OpenChamber process identity test so Linux and macOS execute the same assertions as Windows. Runtime process detection is unchanged.

- Resolve OpenCode provider and allowance icons from the tracked versioned artwork, so clean checkouts do not depend on an ignored local image.

### Security

- Patch the brace-expansion and nanoid transitive dependencies reported by npm audit.

### Changed

- Store Pulse data under `~/.pulse-analytics` with `PULSE_HOME`, independently of provider source roots. Copy legacy settings and consistent SQLite history without overwriting destination files or deleting originals; retain provider credentials and transcripts in place.
- Use English throughout maintained OpenCode documentation and clarify that prompts, transcripts and analytics telemetry are not uploaded.

- Consolidate current Claude and Codex model references, replace obsolete launch notes, organize documentation by purpose, and relocate accepted visual evidence out of the repository root.
- Remove machine-local agent residue and handoffs from the checkout while preserving recoverable copies and existing source/build contracts.

- Reorganize documentation into task-based guides, remove duplicated historical notes, and distinguish current Astra/GPT-5.6 API limits and prices from observed Codex capacity and bundled catalog gaps.

- Require native checks for all six desktop release targets, reuse one frontend artifact and make cross-platform verification the default manual workflow mode.
- Verify native macOS package architecture and publish GitHub DMG/app archives without Apple signing or notarization. Keep updater signatures mandatory and prevent Windows-only recovery releases from becoming latest.
- Restore the Pulse banner, move the current capture into Preview and document Claude, Codex and OpenCode separately with explicit platform availability.

- Restore a banner-led README with current Pulse imagery, release-backed badges, stable navigation, existing monochrome section icons and expandable usage guides.

- Update Svelte to 5.56.10, Testing Library user-event to 14.6.7 and the Tauri filesystem JavaScript plugin to 2.5.2 (#113, #114, #115).
- Update Tauri single-instance to 2.4.4, shell to 2.3.6, dialog to 2.7.3 and its filesystem dependency to 2.5.2 (#116, #117, #118).

## [1.8.1] - 2026-09-05

Pulse 1.8.1 includes the OpenCode and GPT-6 Astra integration, interface updates and Windows Efficiency mode introduced in 1.8.0. This patch also corrects non-Windows compilation. The highlights below describe the complete 1.8 update, not additional changes made after the 1.8.1 build.

### Fixed

- Make Windows Efficiency mode initialization an explicit no-op on other operating systems, so warning-denying Linux builds pass without suppressing lints. Windows EcoQoS behavior is unchanged.

### Included from 1.8.0

#### GPT-6 Astra support

- Recognize `gpt-6-astra` as GPT-6 Astra in session analytics and Discord presence through the shared model catalog.
- Preserve observed reasoning effort, including `ultra`, without presenting harness-specific labels as API settings.
- Track context limits and Standard/Fast pricing. Keep incomplete monetary coverage visible instead of claiming an exact bill.
- Keep subscription credits separate from token-based cost estimates. Unknown credit conversion remains unavailable.

#### Native OpenCode integration

- Read local OpenCode SQLite history without an agent plugin, including supported message-table schemas and configured database paths.
- Preserve model providers, original model IDs and mixed-model contributions. Recorded zero cost remains distinct from missing cost.
- Read OpenCode Go five-hour, weekly and monthly account windows from its authenticated usage API. Do not estimate quotas from session tokens.
- Remove completed sessions from live focus and Discord presence while preserving their history. With no recent session, show an idle state instead of an old model.
- Use the square OpenCode artwork in Pulse and Discord. Keep quota toggles, presets and field order consistent between preview and publication.

#### Frontend, design and responsive layouts

- Use one provider bar for Claude, Codex, OpenCode and combined analytics. Keep Discord's broadcast choice separate from combined history.
- Scope Home summaries and activity to the selected provider and seven-day window, with a separately labeled monthly projection.
- Fit account-limit cards to available width. Replace clipped, fixed-height session controls with content-sized layouts.
- Refine light/dark navigation, provider labels, full-width summary metrics and centered mobile headings.
- Use two-row mobile navigation, compact provider selection and collapsible Home quotas. Keep Sessions and Costs metrics in two columns on narrow screens.
- Preserve keyboard navigation, visible focus, selected states and explicit horizontal scrolling for wide tables.

#### Notification center

- Add individual and bulk read/unread controls.
- Confirm list clearing and retain the stored records. Persistent Undo restores the last confirmed clear and its read states.
- Keep the last loaded history when refresh fails. Do not claim an unconfirmed mutation succeeded.

#### Performance and Windows efficiency

- Import OpenCode history in bounded, incremental batches. Reuse calculated metadata for unchanged recent sessions and advance cursors only after persistence succeeds.
- Poll OpenCode Go usage through a background worker with a 60-second interval, timeout and no redirects.
- Request process-local Windows EcoQoS and Idle priority, with an environment opt-out and read-only verifier. Task Manager controls the leaf indicator; this is not a measured energy-savings claim.
- Add a portable Tauri build command that embeds the frontend and does not require a development server.

#### Data accuracy and compatibility

- Give a manual Codex plan selection priority over cached detection. Recognize Pro 5x, Pro 20x, Edu and explicit aliases.
- Read the connected Discord name and avatar from local READY identity. Do not fabricate missing banners or image badges.
- Display monetary values with two decimal places without reducing stored precision.
- Migrate analytics to schema 6 for OpenCode metadata. The shared core remains 2.0.0 at its existing immutable Git revision.

### Installation and recovery

Use the attached Windows x64 NSIS or MSI installer and verify it with `SHA256SUMS.txt`. This local release does not include a signed automatic-updater manifest. Back up configuration and the analytics database before upgrading; rollback must restore a compatible database backup with the older executable.

See the [OpenCode and Astra guide](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/blob/v1.8.1/docs/opencode-and-astra.md), [Windows Efficiency mode](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/blob/v1.8.1/docs/windows-efficiency.md), and [complete changes since 1.7.9](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/compare/v1.7.9...v1.8.1).

## [1.8.0] - 2026-09-05

### Added

- Native OpenCode SQLite collection across message-table schemas, arbitrary model providers and mixed-model sessions, with incremental imports and explicit cost provenance.
- OpenCode Go account windows for five-hour, weekly and monthly usage, sourced from the authenticated Go API without copying credentials.
- GPT-6 Astra identity, observed reasoning effort, context limits and Standard/Fast pricing with explicit incomplete monetary coverage.
- Windows Efficiency mode through process-local EcoQoS and Idle priority, with an environment opt-out and read-only policy verifier.
- Notification read/unread actions, bulk controls, confirmed soft clearing and persistent Undo for the last confirmed clear.
- A portable Tauri build command that embeds the frontend instead of depending on the development server.

### Changed

- One provider bar selects Claude, Codex, OpenCode or combined analytics; Discord exposes a separate broadcast selector.
- Home uses provider-scoped seven-day data, adaptive account-limit columns and content-sized active-session controls without clipped fixed-height rails.
- Light/dark navigation, centered mobile headings, readable provider identity, full-width summary metrics and accessible notification controls use the existing Pulse design system.
- OpenCode uses the supplied square logo throughout the app and Discord. The previous portal asset remains available for rollback.
- Analytics schema 6 adds OpenCode metadata. The shared core remains 2.0.0 at the existing immutable Git pin.

### Fixed

- Completed OpenCode sessions leave live focus and Discord immediately; history retains their actual values. A null completion field remains active, and old tool state cannot override a completed message.
- OpenCode quotas respect field toggles, presets and saved order through the shared preview/publisher compositor.
- Monetary display uses two decimal places without changing stored precision. Missing costs and unsupported credits remain unavailable.
- Manual Codex plan selection takes precedence over automatic caches. Protocol pro maps to Pro 20x and prolite to Pro 5x; Edu and explicit aliases are recognized.
- The connected Discord name and avatar come from local READY identity. Missing banners and image badges are not fabricated.
- Notification errors retain the last loaded history and do not claim that an unconfirmed mutation succeeded or failed to save.

## [1.7.9] - 2026-08-27

### Removed

- The Context view: it duplicated the Sessions surface whenever no session
  was live. Live-session context composition stays available through the
  session detail views.

### Changed

- Codex usage windows now use the canonical ChatGPT analytics naming: the
  primary label derives from the window duration ("5-hour limit", "Weekly
  limit"), model-scoped windows carry the model name as a muted secondary
  line, and raw tokens like "1w" are gone from primary labels.
- The Discord Live Preview card is re-skinned onto the app's theme tokens —
  light and dark both render it as a native Pulse panel (zero hardcoded hex
  in the preview styles, enforced by a source-level test), with the real
  local avatar, username, and banner when present.
- Costs de-nests to three top-level cards (ledger, charts, session details):
  the coverage footer folds into the ledger, chart sections separate by
  dividers instead of nested boxes, the ledger heading reads "Spend and
  coverage", and metric sub-lines stack without clipping.
- Dashboard focus stats keep their note lines under the value (no overlap
  into the neighboring figure), at-a-glance meta collapses to one line with
  coverage in a tooltip, and the Sessions search plus all-time facts share a
  single bordered toolbar.
- Repository discoverability: added root `llms.txt` (llmstxt.org format) and
  an SEO pass on the README (keyword-bearing opening, factual badge row,
  Why Pulse section, table of contents, single H1).

### Changed

- Reports bundles are cached in-process for 60 seconds per (window, project,
  provider) key, so revisiting the Reports view renders the stored analysis
  instantly instead of re-running every analyzer; first paint shows skeleton
  placeholders per card while the first bundle computes, and the header stays
  interactive throughout.
- Context no longer sits empty between sessions: with provider history it
  renders a "Latest session" panel with the recorded project, model, relative
  time, context window fill, and composition bar (falling back to the plain
  empty state when there is no history or the lookup fails).
- Costs reads as one ledger: a single "Last 30 days" window kicker, honest
  em-dash degradation when provider billing is unavailable (with one short
  explanation instead of a shouted word), compact provenance tags on every
  figure, sentence-case pane titles ("By token type / By model / By project"),
  and a semantic sessions table with right-aligned tabular numbers.
- The budget cockpit tells the real subscription story: when provider-billed
  readbacks are structurally absent, the hero figure shows the actual plan
  price parsed from the active plan ("Pro 20x subscription") with measured
  API-equivalent usage beside it, instead of an empty em-dash wall.
- By-model rows keep their full names on a wrapping two-line label (no
  mid-name ellipsis), and session-detail rows separate project from branch
  onto two lines with a dot-only status column carrying its meaning in
  tooltip and screen-reader text.
- The Context idle panel reads as a hero: the project name is the heading
  (no duplicated "Latest session" label), a full-width threshold-colored
  meter with big token figures, meta row for model/branch/time, and the
  recorded category composition — with an honest one-liner when no
  breakdown was recorded.
- Sessions replaces its raw native selects with the shared custom controls,
  keeps sort/range values verbatim, renders history as a real table with
  scoped headers and cost chips (exact / estimate / unavailable), and tightens
  the all-time footer into one fact line.
- Reports binds its subtitle to the active analysis window, gives exports
  pending states ("Copy report" / "Download HTML"), fixes the heading outline,
  and rewrites generic section subtitles to state their data window.
- Shell copy pass: "Needs attention", deduplicated accessible names on the
  Claude source card, shorter notification copy, calmer update-banner wording.

### Added

- The Discord live preview uses the signed-in local client's real identity:
  the account's global display name is parsed from the LevelDB profile record,
  the handle renders as an `@username` subtitle, and avatar, banner, and names
  refresh every 60 seconds instead of being captured once at startup.
- Dashboard sections state their data window (all time / month to date /
  30 days), the initial analytics load renders skeleton placeholders instead of
  reading as empty history, and the active-session tray plus focus panel form a
  complete tab/tabpanel pair.
- Settings reports a load failure for the window-close preference as a warning
  toast that keeps the default, and disables the export button with an
  "Exporting…" label while an export runs.
- Context exposes a clear empty state ("Nothing live yet") with a direct path
  to the Sessions view.
- The Discord live preview mirrors the real Rich Presence payload's small
  activity asset as a decorative circular chip on the artwork when the backend
  provides one.

### Fixed

- The Discord identity scanner now ranks candidate files from ALL installed
  variants (Stable/Canary/PTB/Flatpak/Snap) by modification time globally and,
  inside the newest file, prefers the richest, latest-written profile record
  (custom avatar > global name > banner) so the preview reflects the CURRENT
  local account instead of a stale one.
- A failed manual "check for updates" now says so ("Couldn't reach the update
  service.") instead of silently doing nothing, and a forced check that finds
  nothing confirms "You're already up to date."; the automatic startup probe
  stays silent either way.
- Discord identity extraction no longer aborts the whole scan when one LevelDB
  file is unreadable, falls back through Stable → Canary → PTB candidate
  directories until a valid record is found, parses the id-bearing object with
  string-aware boundaries and escape decoding so a neighboring account profile
  or quoted display names cannot corrupt the result, and caches the outcome
  (including "not found") for 60 seconds so view refreshes avoid rescanning.
- The Settings control rail now places all four controls in a dedicated grid
  column (previously four controls shared three tracks), and Appearance and
  Window close use the shared segmented control with correct group semantics
  instead of hand-rolled radiogroup markup.
- The Settings footer derives a friendly platform label (Windows/macOS/Linux)
  via userAgentData with a raw-value fallback instead of printing the
  deprecated `navigator.platform` string.
=======
## [1.7.8] - 2026-08-25

Pulse 1.7.8 publishes the complete patch after the immutable release gate caught
an architecture label missing from the Windows SBOM step before any v1.7.7
artifacts became public.

### Fixed

- Windows x64 and ARM64 SBOM filenames now receive their matrix platform label
  in the step that creates them, so the release assembler can verify and
  publish the complete cross-platform asset set.

## [1.7.7] - 2026-08-25

Pulse 1.7.7 was an unpublished release candidate that renewed the
time-bounded RustSec review contract that correctly stopped the first release
attempt before any artifacts were published.

### Security

- Re-reviewed the 19 accepted Tauri/GTK3 and build-time RustSec warnings against
  the current advisory database and extended their fail-closed review window to
  September 25, 2026. No new vulnerability exception was added.

## [1.7.6] - 2026-08-25

Pulse 1.7.6 is a patch release that makes update availability truthful on each
supported platform, completes native x64 and ARM64 release coverage, and
refines the operational workspace without changing public data contracts.

### Fixed

- The repository status row now links to Pulse's real local quality gate instead
  of a removed CI workflow, and the dashboard source strip uses the full
  operational width with a higher-contrast Codex signal color.
- Cross-platform releases now build and verify native Windows, macOS, and Linux
  artifacts for both x64 and ARM64, including architecture-specific updater
  targets, Windows SBOMs, checksums, and installer selection.
- Pulse now offers **Update** only after Tauri confirms that the current
  platform has a signed updater manifest. Missing or invalid updater metadata
  shows **Open release** immediately instead of failing after approval, and the
  verified update handle is reused for download and installation.
- Pulse's local Codex adapter recognizes `codex_work_desktop` sessions as the
  desktop app, matching the canonical Rich Presence classifier.

### Changed

- Refreshed Svelte and the frontend interaction test harness to compatible
  patch releases.
- Promoted the immutable Codex Presence Core pin to canonical v1.10.3 while
  retaining Pulse-owned desktop classification at the adapter boundary.

## [1.7.5] - 2026-08-25

Pulse 1.7.5 isolates analytics scope from Discord publishing, keeps Codex
quota windows faithful to fresh provider data, makes reset dismissals durable,
and ships a tighter, faster desktop workspace without changing public command
or database contracts.

### Fixed

- The frameless Windows title bar uses Tauri's native drag command again,
  preserves double-click maximize, and excludes navigation, notifications,
  theme, and window controls from the draggable surface.
- Analytics source changes no longer switch the provider published to Discord.
  The Discord view keeps its own Claude/Codex broadcaster selector while Home,
  Sessions, Context, Costs, and Reports retain their independent provider scope.
- Codex quota presentation now projects the provider's effective account scope.
  A newly reported weekly-only response removes an older or model-scoped 5-hour
  window from Pulse surfaces instead of presenting an invented `100% remaining`
  value; reported zero usage remains distinct from an absent window.
- Model-scoped quota windows now keep duration-specific identities, so a 5-hour
  and weekly window cannot alternately recreate the same reset notification on
  every poll. Dismissal remains durable across refreshes and restarts, and a
  one-time migration hides pre-fix collision rows without deleting the audit
  trail or suppressing later genuine reset events.
- A malformed Discord configuration now degrades only Discord settings and
  preview data. Core analytics remain live, same-provider last-good Discord
  data stays visible with an explicit warning, provider changes revoke that
  cache, and poisoned shared state fails the whole snapshot instead of
  publishing fabricated empty telemetry.
- Snapshot freshness uses the backend capture timestamp, including restored
  startup snapshots, rather than the time the frontend happened to receive it.
  Direct Discord preview reads now report configuration errors instead of
  silently composing a presence from defaults.
- Reports now normalizes and bounds stored prompt excerpts, keeps them out of
  the rendered DOM by default, and requires an explicit per-session reveal.

### Changed

- Refreshed the Svelte compiler and checker, user-interaction test harness, and
  Rust error-derive lockfile to their compatible patch releases.
- Refined the desktop analytics shell while preserving its ultra-dark canvas, clearer
  text hierarchy, a compact provider strip, a less crowded Discord field
  editor, and a bounded Reports loading state across desktop and narrow widths.
- Reflowed account limits into a content-driven horizontal ledger above the
  live workspace and widened the centered source selector, removing the empty
  full-height rail and unused chrome space without changing provider ownership.
- Split non-dashboard views into route-loaded production chunks. The measured
  entry JavaScript fell from 506.28 kB to 148.06 kB (167.01 kB to 51.67 kB
  gzip) while Home remains eagerly available.
- Added rendered axe-core coverage to the existing dark/light Playwright
  matrix, including the 390-pixel narrow shell across every primary view and
  geometry checks for centered source occupancy and content-driven allowances.
>>>>>>> origin/main

## [1.7.8] - 2026-08-25

Pulse 1.7.8 publishes the complete patch after the immutable release gate caught
an architecture label missing from the Windows SBOM step before any v1.7.7
artifacts became public.

### Fixed

- Windows x64 and ARM64 SBOM filenames now receive their matrix platform label
  in the step that creates them, so the release assembler can verify and
  publish the complete cross-platform asset set.

## [1.7.7] - 2026-08-25

Pulse 1.7.7 was an unpublished release candidate that renewed the
time-bounded RustSec review contract that correctly stopped the first release
attempt before any artifacts were published.

### Security

- Re-reviewed the 19 accepted Tauri/GTK3 and build-time RustSec warnings against
  the current advisory database and extended their fail-closed review window to
  September 25, 2026. No new vulnerability exception was added.

## [1.7.6] - 2026-08-25

Pulse 1.7.6 is a patch release that makes update availability truthful on each
supported platform, completes native x64 and ARM64 release coverage, and
refines the operational workspace without changing public data contracts.

### Fixed

- The repository status row now links to Pulse's real local quality gate instead
  of a removed CI workflow, and the dashboard source strip uses the full
  operational width with a higher-contrast Codex signal color.
- Cross-platform releases now build and verify native Windows, macOS, and Linux
  artifacts for both x64 and ARM64, including architecture-specific updater
  targets, Windows SBOMs, checksums, and installer selection.
- Pulse now offers **Update** only after Tauri confirms that the current
  platform has a signed updater manifest. Missing or invalid updater metadata
  shows **Open release** immediately instead of failing after approval, and the
  verified update handle is reused for download and installation.
- Pulse's local Codex adapter recognizes `codex_work_desktop` sessions as the
  desktop app, matching the canonical Rich Presence classifier.

### Changed

- Refreshed Svelte and the frontend interaction test harness to compatible
  patch releases.
- Promoted the immutable Codex Presence Core pin to canonical v1.10.3 while
  retaining Pulse-owned desktop classification at the adapter boundary.

## [1.7.5] - 2026-08-25

Pulse 1.7.5 isolates analytics scope from Discord publishing, keeps Codex
quota windows faithful to fresh provider data, makes reset dismissals durable,
and ships a tighter, faster desktop workspace without changing public command
or database contracts.

### Fixed

- The frameless Windows title bar uses Tauri's native drag command again,
  preserves double-click maximize, and excludes navigation, notifications,
  theme, and window controls from the draggable surface.
- Analytics source changes no longer switch the provider published to Discord.
  The Discord view keeps its own Claude/Codex broadcaster selector while Home,
  Sessions, Context, Costs, and Reports retain their independent provider scope.
- Codex quota presentation now projects the provider's effective account scope.
  A newly reported weekly-only response removes an older or model-scoped 5-hour
  window from Pulse surfaces instead of presenting an invented `100% remaining`
  value; reported zero usage remains distinct from an absent window.
- Model-scoped quota windows now keep duration-specific identities, so a 5-hour
  and weekly window cannot alternately recreate the same reset notification on
  every poll. Dismissal remains durable across refreshes and restarts, and a
  one-time migration hides pre-fix collision rows without deleting the audit
  trail or suppressing later genuine reset events.
- A malformed Discord configuration now degrades only Discord settings and
  preview data. Core analytics remain live, same-provider last-good Discord
  data stays visible with an explicit warning, provider changes revoke that
  cache, and poisoned shared state fails the whole snapshot instead of
  publishing fabricated empty telemetry.
- Snapshot freshness uses the backend capture timestamp, including restored
  startup snapshots, rather than the time the frontend happened to receive it.
  Direct Discord preview reads now report configuration errors instead of
  silently composing a presence from defaults.
- Reports now normalizes and bounds stored prompt excerpts, keeps them out of
  the rendered DOM by default, and requires an explicit per-session reveal.

### Changed

- Refreshed the Svelte compiler and checker, user-interaction test harness, and
  Rust error-derive lockfile to their compatible patch releases.
- Refined the desktop analytics shell while preserving its ultra-dark canvas, clearer
  text hierarchy, a compact provider strip, a less crowded Discord field
  editor, and a bounded Reports loading state across desktop and narrow widths.
- Reflowed account limits into a content-driven horizontal ledger above the
  live workspace and widened the centered source selector, removing the empty
  full-height rail and unused chrome space without changing provider ownership.
- Split non-dashboard views into route-loaded production chunks. The measured
  entry JavaScript fell from 506.28 kB to 148.06 kB (167.01 kB to 51.67 kB
  gzip) while Home remains eagerly available.
- Added rendered axe-core coverage to the existing dark/light Playwright
  matrix, including the 390-pixel narrow shell across every primary view and
  geometry checks for centered source occupancy and content-driven allowances.

## [1.7.2] - 2026-08-12

Pulse 1.7.2 makes local analytics incremental and provenance-correct, removes
blank route transitions, and promotes the published Codex Presence Core that
recognizes the Daybreak Blue and Red cyber models.

### Added

- Native `gpt-daybreak-blue-latest` / `GPT-5.6-Cyber-Blue` and
  `gpt-5.6-cyber` / `GPT-5.6-Cyber-Red` resolution, including aliases and all
  six observed reasoning-effort tiers without inventing Blue pricing.
- Versioned active-session checkpoints, bundled dashboard/cost snapshots, and
  reconciliation fixtures spanning raw JSONL, SQLite, command DTOs, and exports.

### Fixed

- Appended JSONL transcripts now read only their bounded anchor and new bytes;
  unchanged polls avoid historical body reads, and truncation, replacement,
  partial-line, and corrupt-checkpoint cases fall back to source safely.
- Dashboard, Costs, Reports, budgets, and exports distinguish provider-reported
  billed spend from documented API-equivalent estimates. Unknown values remain
  unavailable instead of becoming exact dollars or false budget overruns.
- Static memoized routes, stale-while-revalidate stores, request coalescing, and
  fingerprinted refreshes prevent blank transitions and redundant backend work.
- Local publication now selects only exact-version installers, validates a
  Windows SPDX SBOM, verifies downloaded hashes, and finalizes the release only
  after the draft asset inventory matches.

### Changed

- Pulse pins `codex-presence-core` 2.0.0 to the immutable Discord Rich Presence
  `v1.10.2` release at commit `a508507e0849fd5c9e09c7d1c55eebe2d199cfc0`.
- `@testing-library/user-event` is updated to 14.6.3.

## [1.7.1] - 2026-08-04

Pulse 1.7.1 corrects the Claude subscription route so a validly signed-in
account is no longer reported as signed out, resolves a design-QA punch-list
across the Dashboard, Sessions, Context, Costs, and Reports views, and hardens
the desktop lifecycle (single-instance, close-to-tray, and notifications).

### Added

- A "Window close" preference in Settings ("Minimize to tray" / "Quit",
  default minimize) that controls whether closing the window hides Pulse to the
  system tray or exits the app.

### Fixed

- Claude subscription access no longer collapses to "Sign in required" when
  valid, unexpired OAuth credentials are present but the current figures are
  served from Pulse's own short-lived usage cache. Cached usage written by a
  prior authenticated read is now trusted while the account's credentials
  remain valid: the route reports as authenticated and surfaces its cached
  quota windows instead of hiding the whole provider. A missing or expired
  credential still resolves to "Sign in required" as before.
- The Sessions history date-range window (Today / 7 / 30 / 90 / 365 days) now
  composes with the advanced From/To/Min $/Model filters instead of being
  dropped whenever an advanced filter was set.
- Reports "Download HTML" now saves via an in-browser blob download instead of
  the native save dialog, so a report can be saved to any location (previously
  a path outside the `$HOME`/`$DOWNLOAD`/`$DOCUMENT`/`$DESKTOP` scope failed).
- The Dashboard "Activity by hour" heatmap no longer overflows its panel; it now
  spans its own full-width row instead of a fixed minimum that could not shrink.
- The single-instance plugin is registered before all other plugins, and a
  launching Pulse GUI reaps any stray CLI / dev-bridge poller so only one
  process writes analytics and drives Discord presence in the background.
- The system-tray icon now restores the window on a single left-click as well as
  a double-click.
- Native notifications stay limited to genuine quota-reset edges. Provider
  health, quota thresholds, and Discord connectivity are polled every few
  seconds on signals that legitimately flap (a pending account read, a
  percentage hovering at a threshold, a reconnecting IPC), so alerting on each
  transition produced duplicate, incoherent toasts; those remain diagnostics
  only. Quota-reset copy is now human-readable (clean provider/window names and
  a friendly local reset time instead of internal ids and a raw timestamp), and
  a one-time cleanup dismisses any previously recorded spurious alerts.

### Changed

- The Dashboard "Live workspace" section header now uses the shared
  section-eyebrow treatment (matching "This session" and "At a glance") instead
  of a heading that rendered smaller than — and competed with — the session
  name beneath it.
- Context-window utilization bars use a single green / yellow / red semantic
  scale (healthy headroom → filling → near limit), shared by the per-session
  bars and the hero badge.
- The Costs "Subscription value ledger" heading and description were rewritten
  from generic copy to describe the API-equivalent value it presents; the data
  was already real and is unchanged.
- The Costs "Cost by project" chart was refined (fully rounded bars, tighter
  spacing, hover state, cleaner axes).

## [1.7.0] - 2026-08-01

Pulse 1.7.0 adds provider-neutral authenticated access routes, freshness-aware
quota presentation, and exact-cost provenance across the Pulse runtime.

### Added

- Canonical access snapshots that keep subscription and API routes separate,
  carry proof/provenance metadata, and fail closed for unproved API keys.
- A local-history capability on access routes, allowing stored Claude and Codex
  analytics to remain selectable without treating filesystem presence as
  authenticated provider proof.
- Freshness-gated quota output shared by Pulse previews and live Discord
  presence, including sparse provider windows and Extra Usage metadata.

### Changed

- Pulse now consumes the typed Codex Presence Core 2.0 usage contract and
  reports unavailable or partial costs without presenting fabricated exact
  totals.
- Costs now leads with a Subscription Value Ledger: provider-scoped token
  totals, token mix, daily trend, sessions, cache reuse, and explicit cost
  coverage remain visible when subscription spend is unavailable.
- Pulse carries the canonical Codex `rateLimitResetCredits` summary separately
  from spend `credits`, preserving the provider count and optional detail
  records without a duplicate parser in the Tauri host.

### Fixed

- Native quota notifications now require a fresh authenticated reset transition:
  Codex `remaining < 100 -> 100` or Claude `used > 0 -> 0`. Timestamp drift,
  first samples, stale/cache/unproved reads, thresholds, provider health, and
  Discord diagnostics stay silent; legacy timestamp-only reset rows are kept
  but dismissed once during migration.
- Claude subscription metrics no longer promote local cache data as an
  authenticated provider response when credentials are missing or invalid.
- Claude local history remains visible as an analytics source when its token is
  expired, while allowance, notification, plan, and Discord surfaces continue
  to require fresh provider proof.
- Billion-scale token totals render with a `B` suffix instead of four-digit
  millions.
- Pulse registers Tauri single-instance handling before the background poller,
  focusing the existing window on a second launch instead of duplicating the
  poller; the independent Discord publisher lease remains unchanged.

### Validated

- Rust workspace tests, Clippy with warnings denied, formatting, and route
  parity coverage pass on the release candidate.
- Live bridge and browser QA cover all seven routes at `1488 x 1058` and
  `390 x 844`, including Codex unavailable cost, Claude partial local history,
  zero page overflow, and an empty durable notification center.

## [1.6.5] - 2026-07-25

Pulse 1.6.5 makes the redesigned UI obey the same rule as the backend: one current owner per fact, with freshness and provider provenance visible at the point of use.

### Added

- Authenticated Codex account-quota reads through `account/rateLimits/read`, cached for 30 seconds and exposed through the existing provider-neutral usage snapshot.
- A responsive multi-instance selector that keeps every active session available while the Dashboard detail and Context Window fraction follow one selection.
- Exact Context Window fractions, remaining-token copy, live token composition, local-time hourly aggregation, and AM/PM heatmap labels.
- Regression coverage for Codex account-response parsing, stale quota rejection, coordinated release versions, active-only Context, multi-instance selection, Discord field wrapping, and heatmap summaries.

### Changed

- Dark surfaces are matte neutral black. View-local navy/gray sheens and duplicate kicker/description headers were removed; semantic and provider-scoped colors remain intact.
- Dashboard quota copy leads with available percentage, reset time, observed source, and freshness. Cost, token, model, project, and recent-session sections use a shared data-section hierarchy.
- Discord field controls stack at the content width where two columns would clip, while autosave and backend-generated preview behavior remain unchanged.
- The product, Tauri, frontend, lockfile, and release-contract versions move together to 1.6.5 without changing persisted configuration or database schemas.

### Fixed

- Pulse no longer combines a day-old global Codex percentage with newer model-scoped credits and presents the result as live. JSONL quota events older than 15 minutes are rejected as fallback data.
- Context no longer calls or renders the historical per-session utilization feed beside active windows, eliminating inactive and duplicated rows.
- Hourly analytics no longer group UTC timestamp substrings as local time, and the UI no longer abbreviates 5 PM as `5p`.
- The Dashboard's empty spend placeholder no longer occupies a live-session panel; provider-reported token categories render instead.

### Validated

- See `docs/evidence/visual-qa-v1.7.0.md` and `tests/index.md` for the final automated and live-browser proof matrix.

## [1.6.2] - 2026-07-24

Pulse 1.6.2 reports what it actually observed: the reasoning effort Claude Code recorded, the Rich Presence switches you last saved, and the handshake that produced your quota numbers.

### Added

- `PresenceConfig.presence_enabled` (schema v6) persists the master Rich Presence switch for Claude, so pausing presence survives a restart.
- `UsageManager::last_usage_origin()` records the auth scheme, endpoint host, and subscription tier observed on a successful usage read.
- `DiscordSettings.supports_credits` marks provider-unsupported fields so the UI can disable them instead of offering a switch that reverts.

### Changed

- Sessions, Context, and the shared stat tiles adopt one panel signature — a lit top edge, a directional sheen, and hover lift — driven by new `--panel-*`, `--meter-*`, and `--lift` tokens in `global.css` rather than per-view copies.
- Switching provider re-seeds the Discord switches from that provider's config instead of leaving the previous provider's cache on screen.
- The Claude presence config is written atomically (temp file, fsync, rename), matching the Codex writer; a truncating write let the 5s poller read torn JSON and fall back to defaults.

### Fixed

- Reasoning effort no longer reads `Medium` for every Claude session. Current Claude Code builds record the composer's selector as a top-level `effort` field on each `assistant` transcript line; Pulse now parses it, prefers the latest value, and ignores subagent overrides on the parent session.
- The usage footer no longer claims `api`. Pulse authenticates to the usage endpoint with an OAuth bearer token from `~/.claude/.credentials.json` and never with an API key, so the footer names the observed handshake and plan, or says the reading came from cache.
- `set_discord_enabled` persists for Claude, and startup seeds the cached flag from disk instead of hard-coding presence back on.
- A failed mirror write to the inactive provider's config no longer fails the whole save, which previously rolled the toggle back in the UI after it had already landed on disk.
- Toggling a field while a save is in flight re-publishes the store, so the checkbox cannot show a value the backend never received.
- `Context.svelte` no longer hardcodes a `JetBrains Mono` stack that was never loaded, and its meter tracks resolve through tokens.

## [1.6.1] - 2026-07-24

Pulse 1.6.1 corrects Claude Opus 5 pricing, reports true window totals instead of a capped page, and installs updates from inside the app.

### Added

- Claude Opus 5 support end to end: pricing, cache tiers, 1M GA context, fast-mode detection, display names, model routing, and Rich Presence.
- `get_cost_totals` aggregates an entire cost window, including per-model and per-project breakdowns, without the analyzer row cap.
- A signed in-app updater that downloads, verifies, and installs a published release without leaving Pulse.
- A read-only localhost bridge, compiled only in debug builds, so the Svelte frontend can be reviewed in a browser against the real backend.

### Changed

- Reports leads with a daily cost timeline that marks inflections on the curve; Cost Analysis becomes a budget cockpit with its own analytical shape.
- The Discord live preview and its header state resolve entirely through semantic tokens and stay legible in Dark and Light.

### Fixed

- `claude-opus-5` no longer falls through to legacy $15/$75 Opus pricing plus a long-context surcharge; it bills at the official $5/$25 rates.
- Cost Analysis and Reports no longer understate totals by summing only the newest page of sessions.
- Drop inflections report their real ratio instead of an inverted multiplier.
- Accent foregrounds on themed surfaces resolve through tokens instead of hardcoded colors.
- Both high-severity frontend dependency advisories are cleared.

## [1.6.0] - 2026-07-16

Pulse 1.6.0 unifies live analytics, Discord publication, and persistence around one semantic snapshot while promoting Codex parsing and presence composition to a canonical shared core.

### Added

- Versioned `get_app_snapshot()` hydration and `pulse://snapshot` change events shared by live UI, Discord, and persistence consumers.
- Semantic Codex quota scopes/windows and real account Credits, including global and model-specific surfaces without invented provider parity.
- A ten-field Rich Presence compositor with Credits, presets, per-field visibility, persistent order, compact/descriptive output, keyboard reordering, and backend-owned preview text.
- Release gates for product/core/config/database version synchronization, an immutable canonical core Git SHA, Windows SPDX SBOM validation, and schema migrations.

### Changed

- Pulse consumes the canonical UI-free `codex-presence-core` instead of maintaining a second telemetry/composition owner.
- Backend polling is consolidated around one content-fingerprinted snapshot; unchanged sessions no longer force recurring SQLite upserts.
- Dashboard quota cards adapt to the scopes actually present. Codex shows Credits while Claude retains Extra Usage when available.
- Usage reset timestamps render as absolute local dates and times, matching the Codex analytics presentation instead of a drifting relative countdown.
- Dark and Light themes now drive native Tauri chrome, previews, charts, heatmaps, overlays, focus states, and scrollbars through semantic tokens.
- Discord settings and the remaining views use responsive layouts down to 720×560, system fonts, reduced-motion behavior, and keyboard-accessible controls.
- Repository release governance now requires Conventional Commits, provider/runtime proof, Dark/Light viewport evidence, performance measurements, and an exact canonical core pin.

### Fixed

- A weekly-only 10,080-minute Codex quota no longer appears as a false five-hour limit.
- `âš¡ Fast` is derived from active-session evidence before fallbacks and is presented consistently in Pulse, preview, and Discord.
- Light mode no longer leaves the Discord preview and secondary UI surfaces on hardcoded dark colors.
- Parser tests no longer read the developer's real model cache.

### Performance

- Idle snapshot delivery is content-addressed, frontend recurring invokes are removed, and session persistence skips unchanged fingerprints.
- Historical provider/time queries gain a schema-5 index and controlled WAL maintenance.

### Validated

- Rust, Svelte, Vitest, bundle, migration, accessibility, and six-viewport visual gates passed.
- Real Windows Fast, weekly-only quota, Credits, presence persistence/publication, native Light mode, and 720x560 resize behavior were observed.
- Startup, idle CPU/memory, Tauri invokes, SQLite writes, and initial bundle size were measured with claim boundaries recorded where run order affected results.

## [1.5.3] — 2026-07-10

v1.5.3 is a Windows runtime-correctness patch for Pulse's existing Codex polling lane. No public Tauri command or persisted schema changed.

### Fixed

- The five-second Codex Git branch probes now use the shared Windows `CREATE_NO_WINDOW` launcher, preventing visible console flashes while Pulse refreshes live sessions.
- Pulse vendors Codex Discord Rich Presence v1.7.6 by immutable tag, commit, and per-file hashes, preserving Pulse-owned adapters outside the mirror and carrying canonical cross-file source contracts as uncompiled inputs.

### Validated

- A red-green vendoring regression rejects the previous v1.7.5 pin and any raw `Command::new("git")` polling path.
- Rust format, Clippy with warnings denied, workspace tests/build, Svelte check, frontend tests/build, RustSec audit, release contracts, Tauri packaging, and a repeated-poll Windows window trace.

## [1.5.2] — 2026-07-10

v1.5.2 is a compatibility, provenance, and presentation-correctness patch for GPT-5.6 and the existing Codex lane. No public Tauri command was removed.

### Added

- GPT-5.6 Sol, Terra, and Luna in the immutable Codex model catalog, including aliases, App labels, supported reasoning tiers, independent Standard/Fast capability, API pricing, Codex credit rates, cache policy, and sourced context metadata.
- Selectable `Codex App` / `ChatGPT App` desktop Rich Presence design, with separate Codex CLI and VS Code Extension surface labels.
- Complete Discord privacy controls for project, branch, model, activity, tokens, cost, limits, context, and systems in both Pulse and the vendored canonical presentation contract.
- SQLite schema v4 provenance fields for pricing completeness, cache savings, context source, and speed.

### Changed

- Pulse now vendors Codex Discord Rich Presence v1.7.5 by immutable tag, commit, and per-file hashes. Pulse-owned adapters remain outside the canonical mirror.
- Costs, cache savings, context windows, labels, reasoning effort, speed, and Discord lines consume the canonical model/presentation owners instead of frontend or Tauri fallback tables.
- Cache Health is available for Codex when its observed token telemetry supports it; Claude-only model-routing analysis remains capability-gated.
- Daily analytics are derived idempotently from `sessions`; the legacy `daily_stats` table remains untouched for rollback but is no longer an analytics source.
- Pulse branding, README, docs, report copy, and repository metadata now present Claude Code and Codex (ChatGPT App) as equal product lanes.

### Fixed

- Turning off Git branch now removes it from the live Codex Discord payload as well as the Pulse preview.
- GPT-5.6 model and reasoning display uses a clear separator, such as `GPT-5.6 Sol · Max`, while Fast remains an independent speed marker.
- Unknown Codex models no longer inherit GPT-5.1 pricing; unavailable and partial observations remain explicit.
- Context no longer invents fixed 10K/6K/3.3% inventory estimates or treats every installed skill as loaded telemetry.
- Reports capture one provider per generation and escape external text before producing offline HTML.

### Security

- Offline HTML reports include a restrictive Content Security Policy and encode transcript/database text at the rendering boundary.
- Release and vendoring gates verify immutable source hashes, version surfaces, tag provenance, and checksums.

### Validated

- Model, alias, effort, speed, context, pricing-completeness, privacy-toggle, migration, report-injection, frontend, and release-contract suites.
- Rust format, Clippy with warnings denied, workspace tests/build, Svelte check, frontend tests/build, dependency audit, and Tauri packaging.

## [1.5.1] — 2026-07-06

v1.5.1 is a Windows safety patch for the Codex runtime embedded in Pulse. No public API was removed.

### Fixed

- Mirrored Codex WSL session discovery is now explicit opt-in via `CODEX_PRESENCE_INCLUDE_WSL=1` or `CC_PRESENCE_INCLUDE_WSL=1`; by default Pulse does not invoke `wsl.exe` while polling Codex sessions.
- The WSL opt-in path now uses Pulse's hidden subprocess launcher, so enabled WSL scans do not create visible WSL/conhost windows from the desktop app.
- Codex source-sync scripts now preserve the WSL opt-in/no-window overlay, preventing a future sync from reintroducing the Windows console flood.

### Validated

- `cargo test --test codex_upstream_contract codex_wsl_probe_commands_use_hidden_windows_launcher`

## [1.5.0] — 2026-07-05

v1.5.0 brings the Codex Discord Rich Presence overhaul into Pulse. Codex App, Codex CLI, VS Code Extension, and OpenCode-hosted Codex sessions now share one cost/cache/context contract and keep the correct Discord identity while active or idle. No public API was removed.

### Added

- Codex GPT-5.4 and GPT-5.5 context metadata in Pulse: 400K visible OAuth cap, API-only 1,050,000-token metadata, 272K input threshold, 128K output reserve, and Fast cost multipliers of 2x / 2.5x.
- Codex Rich Presence preview asset beside the Claude preview in the README, using the same polished Discord card language.
- Desktop Codex App process detection for Pulse background polling so the live preview can distinguish a real desktop Codex surface from generic CLI idle.

### Changed

- Pulse now consumes the synced Codex runtime contract for model pricing, context windows, cache savings, surface branding, and idle display instead of reconstructing fallback values in the GUI layer.
- Codex Live Preview keeps the official `Codex App` identity and `Idling...` state when the desktop app is the last or current surface.
- Report generation and Codex cost summaries now share the same cache/context accounting used by the standalone Codex runtime.

### Fixed

- Codex App activity no longer collapses to the generic `Codex CLI / Codex VS Code Extension` Discord card immediately after a short idle gap.
- OAuth Codex context display no longer advertises the API-only 1M context path to typical ChatGPT/Codex users; Pulse caps the visible runtime at 400K while retaining API metadata separately.

### Validated

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo build --release`
- `cargo build -p pulse --release`
- `npm --prefix frontend run check`
- `npm --prefix frontend run build`

## [1.4.2] — 2026-07-01

v1.4.2 is the release-hygiene patch for the v1.4.1 lane. It keeps the published v1.4.1 tag immutable, then ships the final Discord Rich Presence contract fixes, Sonnet 5 copy corrections, docs cleanup, and dependency sweep as a new patch release. No public API was removed.

### Fixed

- Discord display preferences now cross the Tauri IPC boundary with the correct camelCase argument contract. Turning off Git branch persists to the Claude and Codex config files and removes the branch from both the real Discord broadcaster and the Pulse Live Preview. (#46)
- Pulse Live Preview now renders the backend Rich Presence payload instead of reconstructing Discord details/state in Svelte, so preview and live Discord use the same source of truth. (#46)
- Plain Claude thinking blocks no longer render as a fake workflow systems label. `ULTRACODE` is shown only for a live workflow record, and stale workflow records expire instead of keeping Discord stuck after the workflow ends. (#46)
- Codex systems copy uses `Tool active` for pending tool calls and avoids workflow wording for normal thinking activity. (#46)

### Changed

- README and docs now introduce Sonnet 5 with Anthropic's published introductory cache prices: 5-minute writes at $2.50 / MTok, 1-hour writes at $4.00 / MTok, and cache reads at $0.20 / MTok during the introductory window. (#46)
- The release notes template remains CHANGELOG-driven, so GitHub Releases use the curated section for the tag instead of stale generated wording. (#46)
- Local runtime proof artifacts under `target/evidence` were treated as scratch output and removed from the workspace cleanup path. (#46)

### Validated

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --jobs 1 -- -D warnings`
- `cargo test --workspace --jobs 1`
- `npm --prefix frontend run check`
- `npm --prefix frontend run test`
- `npm --prefix frontend run build`
- `cd src-tauri && cargo tauri build`

## [1.4.1] — 2026-06-30

v1.4.1 fixes a real, live-confirmed data-staleness bug in every "how full is my context right now" UI surface, and adds explicit aggregation-scope labels to the Dashboard and Costs cost totals. No public API was removed.

### Fixed

- Claude Discord Rich Presence uses the square `large` Developer Portal asset again instead of the letterboxed `claude-code` upload, so the activity card art fills the square slot instead of rendering as a tiny wide mascot inside black padding.
- Discord field toggles now apply through one shared contract for Claude and Codex. Turning off Git branch removes the branch from both the live broadcaster and the Pulse preview; the new Systems toggle exposes only safe generic signals such as `ULTRACODE`, `Tool active`, or `1 agent`.
- Claude and Codex activity labels suppress decorative/noisy shell commands such as `echo =====` banners and temp-path launchers. Live Claude transcript writes also reactivate stale `Idle` activity, so active sessions update back to `Thinking` instead of staying stuck.
- The Context Window header card, the "Per-session utilization" panel, and the Sessions/Dashboard "ctx-1m" badge all read a field (`max_turn_api_input`) that is a monotonically-increasing, never-resetting all-time peak across a session's entire lifetime -- including across compactions. A session that hit a high-water mark before an auto-compaction kept showing that historical peak, and the resulting false "Context is 100% full -- CRITICAL" recommendation, indefinitely after the compaction had actually emptied the context back out. Confirmed live: `get_context_breakdown` returned `used_tokens: 999486` for a real, currently-running session whose own JSONL transcript recorded a `compact_boundary` event with `compactMetadata.postTokens: 25500` 2.5 hours earlier. (#44)
- `src/session.rs` now detects `{"type":"system","subtype":"compact_boundary"}` events (which Claude Code writes on every compaction, with the authoritative post-compaction size in `compactMetadata.postTokens`) and tracks a new field, `current_context_tokens`, separately from `max_turn_api_input`. The new field resets to the real post-compaction size at each boundary and otherwise tracks the most recent turn's total -- the field every "current state" UI surface now reads. `max_turn_api_input` is untouched and keeps its correct, separate role: detecting whether a session has ever required the 1M context tier (a lifetime question that should never decrease). See [docs/context-tracking.md](docs/guides/context.md). (#44)

### Changed

- Dashboard's "Total Cost" KPI tile is now labeled "Total Cost (Live)" (it sums only currently-live sessions); the Costs view's "Total Spent" tile is now labeled "Total Spent (30d)" (it sums the persisted 30-day historical database). Both totals were already real and correctly computed -- they answer different, legitimate questions that weren't previously distinguished by their labels. (#44)

## [1.4.0] — 2026-06-30

v1.4.0 adds native Claude Sonnet 5 support, including a generic, date-driven introductory-pricing system that automatically reverts to standard pricing with no manual flag, and fixes a pre-existing 1M-context pricing bug discovered while building it. No public API was removed.

### Added

- Claude Sonnet 5 (`claude-sonnet-5`) native support: introductory pricing of $2.00 input / $10.00 output per MTok through August 31, 2026, then $3.00 / $15.00 standard — automatically, evaluated against the real clock on every poll cycle. (#43)
- A reusable introductory-pricing mechanism in `src/cost.rs`: clock-injected `cost::model_pricing_at(model_id, now)` (the real source of truth; `cost::model_pricing(model_id)` stays the existing real-clock entry point) and `cost::active_intro_pricing(model_id, now)`, which returns the active promo only while genuinely inside its window — `None` both for models with no promo and once a promo's window has closed. Adding the next time-boxed launch is a registry addition, not new branching logic. (#43)
- Sessions and Dashboard live-session cards show a new "Intro Pricing" badge sourced entirely from the backend (`SessionInfo.intro_pricing`) — exact discounted rate, human end date, and the rate it reverts to, with zero date math or hardcoded pricing in the frontend. (#43)
- `cost::has_inflated_tokenizer()` now also covers Sonnet 5 (Anthropic-confirmed new tokenizer, ~1.0-1.35x more tokens than Sonnet 4.6 for the same input, permanent and independent of the promo window) — the existing Sessions/Dashboard `⚠` marker now triggers for it too. (#43)
- [docs/sonnet-5.md](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/blob/9367949c496c832269505dfac4b3eee73310cdb8/docs/sonnet-5.md): official specs, the introductory-pricing mechanism, Anthropic's published cache multipliers, and the 1M-context bug fix below.

### Fixed

- `cost::is_ga_1m_context("claude-sonnet-5")` previously returned `false` — the generic Sonnet/Opus version parser expects a two-segment id like `"4-6"` and Sonnet 5's id has only one numeric segment (`"5"`), so it fell through and would have applied the beta long-context 2x/1.5x surcharge above 200K tokens. A dedicated `is_sonnet_5_class()` classifier (mirroring the existing `is_mythos_class()` pattern for Fable 5 / Mythos 5) now short-circuits `is_ga_1m_context`, `supports_1m_context`, `has_inflated_tokenizer`, and the pricing lookup, so all four agree that Sonnet 5 is GA at 1M context. (#43)

### Notes

- Anthropic publishes prompt-caching rates as multipliers of the input price: 5-minute writes at 1.25x, 1-hour writes at 2x, and cache hits at 0.10x. Pulse applies those official multipliers to Sonnet 5's introductory input rate and uses the 5-minute write rate for Claude Code JSONL because transcripts do not expose cache TTL. See [docs/sonnet-5.md](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/blob/9367949c496c832269505dfac4b3eee73310cdb8/docs/sonnet-5.md).
- This environment's `pulse` crate build (Tauri's full dependency tree) needs `cargo --jobs 2` under tight available memory, or the build can hit `STATUS_COMMIT_LIMIT_EXCEEDED` and cascade into unrelated-looking errors in transitive dependencies (`icu_properties`, `idna`). Documented in [tests/index.md](tests/index.md) — environment characteristic, not a code defect.

## [1.3.0] — 2026-06-16

v1.3.0 makes the Codex and Claude Rich Presence accurate again, makes the plan override actually stick, gives the Discord Live Preview the real Rich Presence artwork, and hardens the analytics core. No public API was removed.

### Added

- Discord Live Preview renders the real Rich Presence artwork — the Claude Code mascot and the Codex mark — bundled in-app and mapped by provider/surface, with a Fast-tier (⚡) indicator on the state line. The activity card now mirrors Discord's layout (large image, optional small badge, name, details, state, elapsed). (#32)
- Canonical Claude plan mapping module `cc_discord_presence::plan` (key, name, display name, badge, tolerant override parser), shared by the core library and the Tauri command layer. (#33)
- Coverage for the new behavior: plan round-trip/tolerance, bounded session-scan depth/entry limits, report-trace depth cap, canonical plan-key round-trip in Settings, and a credentials-refresh plan-detection test. (#31, #33)

### Changed

- Cost analysis reports **cost per 1M tokens** instead of per 1K, which rounded to `$0.00` at any realistic usage. (#34)
- Dashboard cache-health shows a neutral `—` instead of a red `F` when there is no token data yet. (#34)
- The duplicated Claude plan key/label/badge mapping is centralized; `config.rs` and the command layer delegate to the canonical module. (#33)
- Claude and Codex session-file walks and the report trace scan are now depth/entry/dir bounded so a pathological tree cannot walk unbounded. (#33)
- Previously-swallowed failures (Discord presence update, config/provider and display-pref saves, usage-cache write/remove, Codex plan-cache save) are logged via `tracing` instead of being discarded. (#33)

### Fixed

- Codex service tier (Fast mode) is read from `~/.codex/config.toml` `service_tier` (where current Codex versions persist it) with the legacy `default-service-tier` global-state key kept as a fallback, so Fast is detected and shown as `⚡ … · Fast`. (#30)
- The manual plan override persists to the config file, reaches the live Discord broadcast, and stays selected instead of snapping back to Auto-detect; the Settings select now uses a canonical plan-key contract. (#31)
- Claude plan auto-detect reads the credentials plan fields fresh from disk, so a plan upgrade (e.g. Max 5x → Max 20x) is reflected without restarting Pulse. (#31)

### Notes

- The live Discord broadcast still requires the Rich Presence images to be uploaded to each Discord application's Developer Portal (`codex-logo` / `codex-app` are not yet uploaded); the in-app Live Preview bundles its own art and is unaffected. Tracked in #36. See [docs/discord-assets.md](docs/guides/discord.md).

## [1.2.0] — 2026-06-10

v1.2.0 is a minor release for Anthropic's Fable/Mythos 5 launch, the Context Window view's stale one-session bias, and Pulse's first in-app release-awareness flow. The release adds new model economics, multi-session UI, and update-check UX without removing any public API.

### Added

- Claude Fable 5 and Claude Mythos 5 support: `claude-fable-5` / `claude-mythos-5`, $10 input / $50 output per MTok, $12.50 5-minute cache writes, $1 cache reads, 1M GA context, and 128K max output metadata.
- Rich Presence labels for the new family: `Fable 5 (1M)` and `Mythos 5 (1M)` render cleanly instead of falling through to raw parenthetical model IDs.
- `get_context_breakdowns(session_ids?: string[] | null)` Tauri command for returning context breakdowns across active sessions.
- Context Window active-session cards: every live session is visible at the top of the view, with utilization, activity, and click-to-select detail routing.
- Frontend API type `SessionContextBreakdown` and Vitest coverage for multi-session Context cards, active Discord preview selection, and Fable/Mythos session badges.
- Backend update-check command `check_app_update()` that compares the packaged Pulse version against the latest stable GitHub Release, plus `open_app_release_page()` with a Pulse-release URL allowlist.
- Global update popup with current/latest version, release title, release notes toggle, Later, Skip version, Open release, 6-hour polling, and a `?fakeUpdate=` development lane.
- Documentation page [docs/fable-5.md](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/blob/9367949c496c832269505dfac4b3eee73310cdb8/docs/fable-5.md) with official specs, pricing, context window, cache TTL, and validator notes.
- Documentation page [docs/update-checks.md](docs/guides/updates.md) covering the release-check flow and why v1.2.0 does not fake a signed auto-installer without updater metadata.
- Documentation page [docs/codex-rich-presence-upstream.md](docs/maintainers/codex-core.md) covering the Codex Rich Presence source-of-truth repo, sync scripts, and CI freshness gate.
- `src/codex/UPSTREAM.json`, `scripts/check-codex-rich-presence-upstream.*`, and `scripts/update-codex-rich-presence.*` so Pulse can prove and refresh its mirrored Codex Rich Presence core from `xt0n1-t3ch/Codex-Discord-Rich-Presence`.
- `tests/codex_upstream_contract.rs` to lock the Pulse-facing boundary around the upstream Codex presence modules.

### Changed

- `get_context_breakdown(session_id?)` is now a compatibility wrapper over `get_context_breakdowns`, keeping older callers stable while centralizing context logic.
- Context fallback selection now prefers active sessions before historical/idle rows; `get_sessions_context_usage()` prepends live active sessions, then dedupes historical results by session id.
- Discord preview chooses the first active session before falling back to historical sessions and shows the active-session count when multiple sessions are live.
- Usage-limit labels now use provider-accurate copy such as `5-hour window` instead of calling Anthropic's usage window a current session.
- Settings now exposes a manual **Check for updates** action that triggers the same global update popup.
- Frontend package metadata now matches the v1.2.0 root, Cargo, Tauri, and lockfile versions.
- Codex-specific Rich Presence code now flows from the standalone [Codex Discord Rich Presence](https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence) repository through a source-sync mirror plus a small Pulse compatibility overlay, avoiding private drift while keeping Pulse's Windows resources link-safe.

### Fixed

- Fable/Mythos pricing does not apply beta long-context surcharge across the full 1M window.
- Fable/Mythos sessions are not marked as Opus-tokenizer-inflated and are not treated as fast-capable until Anthropic documents those behaviors.
- Context Window no longer shows only one active session when multiple Claude/Codex sessions are live.
- Per-session Context Window utilization no longer reports impossible values above 100% from lifetime session token totals; history stores the last context snapshot and clamps stale rows.
- Windows session polling no longer launches `wsl.exe` by default. WSL transcript scanning is now explicit via `CC_PRESENCE_INCLUDE_WSL=1`, preventing broken WSL installs from throwing crash dialogs in the background.
- Mirrored Codex Rich Presence git-branch probes now use no-window process spawning on Windows.
- Removed stale Markdown issue templates now superseded by the YAML templates.

## [1.1.0] — 2026-05-28

### Added

- Claude Opus 4.8 support: pricing, 1M-context GA, and inflated-tokenizer detection.
- Fast mode: per-turn `usage.speed` detection, 2x speed-aware pricing, and a fast marker in Sessions, Discord presence, and the HTML report.
- OpenAI Codex GPT-5.5 pricing ($5 / $30 per MTok) and Codex Fast mode (`/fast`) cost: GPT-5.5 bills at 2.5x and GPT-5.4 at 2x the standard rate.
- Per-session Context Window: a session selector, per-session token usage, and tiered compaction hints.
- Single-scan Reports: `get_reports_bundle` loads every analyzer from one JSONL scan.
- Centralized test suite: Vitest unit, integration, and component plus Rust integration; see [tests/index.md](tests/index.md).
- Docs: [architecture.md](docs/architecture/index.md), [troubleshooting.md](docs/guides/troubleshooting.md), [opus-4-8.md](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/blob/9367949c496c832269505dfac4b3eee73310cdb8/docs/opus-4-8.md).

### Changed

- Reports and Insights no longer hangs: analyzer and export commands run off the UI thread via `spawn_blocking`, with per-file and file-count scan guards.
- HTML report rebuilt to render offline (no Google Fonts CDN), match the in-app dark theme, draw smooth bezier charts, and show a fast-vs-standard split.
- Repo conventions follow DLSSync: an artifact `.gitignore`, a root npm script hub, and CI running `clippy -D warnings`.

### Fixed

- Per-category cost breakdown reconciles with the speed-correct total; the statusline total stays authoritative.
- Sessions top-table no longer mutates reactive state during render.
- Codex unknown-model pricing keeps the real model id instead of relabeling it.

## [1.0.0] — 2026-04-18

### Added — Initial public release

- **Pulse desktop app** (Tauri 2 + Svelte 5) — native dashboard for Claude Code.
- **Analytics views** — Dashboard, Sessions, Context, Costs, Reports, Discord, Settings.
- **Cache health grading** — A–F letter grade with trend-weighted hit ratio.
- **Recommendations engine** — rule-based findings with "Copy Fix Prompt" for Claude Code.
- **Inflection detector** — ≥2× cost-per-session deviation alerts.
- **Model routing analyzer** — Opus/Sonnet/Haiku mix + savings estimate.
- **Tool-frequency, prompt-complexity, session-health** analyzers.
- **Discord Rich Presence** — five-tier reasoning (Low/Medium/High/Extra High/Max), multi-tier asset resolver, auto-reconnect.
- **Opus 4.7 support** — inflated-tokenizer detection, 1M context GA pricing.
- **Plan usage** — session/weekly/Sonnet/Extra-usage limits with sound alert on spikes.
- **Local-first** — SQLite at `~/.claude/pulse-analytics.db`, zero telemetry.
- **Tri-OS installers** — Windows (NSIS/MSI), macOS (DMG, arm64 + x64), Linux (deb/rpm/AppImage).

[1.5.3]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.5.3
[1.6.0]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/compare/v1.5.3...v1.6.0
[1.5.2]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.5.2
[1.5.1]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.5.1
[1.5.0]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.5.0
[1.4.2]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.4.2
[1.4.1]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.4.1
[1.4.0]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.4.0
[1.3.0]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.3.0
[1.2.0]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.2.0
[1.1.0]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.1.0
[1.0.0]: https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases/tag/v1.0.0
