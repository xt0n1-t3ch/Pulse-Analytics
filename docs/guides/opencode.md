[Documentation](../index.md) / OpenCode and Astra

# ![](../../assets/icons/terminal.svg) Use OpenCode and GPT-6 Astra in Pulse

This guide explains sessions, account limits and Discord publication in Pulse 1.8.2 and 1.9.0. OpenCode reads local data. The shared model catalog and core pin have separate contracts.

## Select sessions and publication

One bar selects Claude, Codex, OpenCode or All providers. Selecting an application selects its context and history. All providers combines analytics without changing the selected Discord broadcaster. Account limits retain their own provider and authentication.

OpenCode Desktop, CLI and OpenChamber share the OpenCode identity in Discord. SQLite ingestion needs no agent plugin. Pulse keeps the surface generic when attribution is uncertain. OpenChamber attribution requires matching PID, parent, executable and session, with no competing backend.

The six views share the Pulse design system. Quotas fit the available width. Mobile navigation uses two rows and a compact provider selector. Home can collapse quotas. Sessions and Costs use two-column metrics. Tables support horizontal scrolling and keyboard access. Controls retain visible focus and selected states. Changing views resets reading position.

The preview shows the two lines computed by Rust. Text wraps inside the card. An unavailable small image is not replaced with the Pulse logo.

## Configure OpenCode

Pulse discovers `opencode.db` and `opencode-*.db` channel files. It respects `XDG_DATA_HOME` and `OPENCODE_DB`. Relative database paths resolve inside the OpenCode data directory. `:memory:` is ignored because it is not shared storage.

Pulse settings live in `~/.pulse-analytics/pulse-opencode.json`, or under `PULSE_HOME`. Provider credentials and configuration remain unchanged. See [storage and migration](storage.md). `database_paths` accepts additional SQLite files; duplicate paths are normalized. `enabled`, `privacy_enabled`, `client_id` and `layout` control publication.

The default Application ID is `1545590419763761303`; the image key is `opencode-v2`. Remote databases and OpenChamber servers on other machines are outside this local integration.

## Interpret the data

The reader opens SQLite read-only and supports `message` and `session_message` tables. It imports batches of 64 sessions with a stable cursor and revisits recent sessions. The cursor advances only after persistence succeeds. Unchanged sessions reuse computed metadata.

Pulse retains provider, original model ID, display name, variant, source database and per-model contributions. Completed sessions leave live presence but remain in history. The latest response identifies the used model; session selection is the fallback before the first response. Multi-model sessions show Mixed models with a breakdown.

A reported zero cost differs from unknown cost. OpenCode-reported value is not a confirmed provider bill. Pulse does not invent prices, quotas or context capacity. Context comes from message data or provider/model metadata, not cumulative tokens. Claude-specific recommendations remain disabled for OpenCode.

Diagnostics appear in Settings. Prompts and command arguments never enter the Discord payload. `privacy_enabled` omits project and branch; other fields follow their settings.

## Verify Astra and images

`gpt-6-astra` displays as GPT-6 Astra. The [model catalog](../models/codex.md) owns verified context, effort, prices and sources.

> Astra's API exposes 1,050,000 total tokens, up to 922,000 input and 128,000 output. The local Codex 0.153.3 inventory checked on September 5, 2026 exposes 272,000 raw and 258,400 usable tokens. These are different capacities, not guarantees for every account.

`ultra` is an observed harness value, not a published API effort level. OpenCode does not inherit Astra prices or limits. Read the [bundled catalog gaps](../models/codex.md#bundled-catalog-gaps).

Codex-Discord-Rich-Presence owns the canonical catalog. `scripts/check-model-catalog-parity.ps1` checks byte equality with Pulse. Pulse 1.9.0 pins core 2.0.1 from the upstream v1.11.2 release; catalog edits must land upstream first and then synchronize here.

`assets/branding/opencode.provenance.json` records the supplied image, checksums and previous source for rollback. The preview and Developer Portal use the same PNG. A valid payload or portal key does not replace visual verification in Discord.

## Update and recover

Back up the executable, settings and a consistent SQLite snapshot before upgrading. Do not copy an active database without its transactions. Schema 6 adds `opencode_json` and retains a pre-migration backup. The storage migration also retains legacy files; see [recovery](storage.md#recovery).

Stop only the PID whose executable matches the intended Pulse installation. Verify the new binary's SHA-256, startup, history and `PRAGMA quick_check`. Preserve Discord and other agents. For rollback, retain the new database and restore the previous binary, settings and consistent backup. Later imports remain in the retained new database, not the restored copy.

## Plan detection and override

Manual Settings overrides precede telemetry, memory and cache. Auto-detect reads `account/read` and `account/rateLimits/read` in the same authenticated process. Plan identity does not create quota windows.

Pulse recognizes Free, Go, Plus, Pro 5x, Pro 20x, Business, Enterprise and Edu. Protocol `pro` means Pro 20x; `prolite` means Pro 5x. Explicit `pro_5x` and `pro_20x` aliases are accepted. Claude Max is not a Codex plan. Unknown signals do not replace valid identity.

Regression tests reload each override and compare Settings with Discord. Auto-detect returns to account signals, not the last override. Invalid overrides fail without changing settings.

## OpenCode Go limits

The adapter queries `https://opencode.ai/zen/go/v1/usage` with the existing `opencode-go` key. It does not copy the key to Pulse or logs. Queries run every 60 seconds with a timeout and no redirects. Only valid responses enable the five-hour, weekly and monthly windows.

The API supplies percentages and reset dates. Pulse does not assume a 30-day month. These limits belong to Go, not every OpenCode provider or model. Usage quotas controls all three windows in Discord and the preview. Presets and field order share one compositor. Text includes Go and used to identify the account. Stale or incomplete limits are not published.

Without a recent session, OpenCode clears Discord activity and the preview shows Waiting for OpenCode. The default layout starts with model, activity, project and branch. Home filters summary, history and activity by provider and the last seven days. Monthly projection keeps This month. A recent selected session's model replaces the historical model.

## Local Discord profile

Local IPC `READY` owns user identity, display name and avatar. Reading identity needs no credentials and publishes no activity. Banners require a client value or a local image reference for the same user. Missing banners remain absent. The recorded local check returned no banner from `READY` or `GET_USER`.

## Notifications and efficiency

The center supports individual and bulk read/unread actions and clearing with confirmation. Clearing retains records. Undo restores the last confirmed batch and its read states. Transport failures retain the previous list and do not claim a saved mutation.

[Windows Efficiency mode](../maintainers/windows-efficiency.md) explains EcoQoS, reduced priority and the opt-out.
