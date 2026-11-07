[Documentation](../index.md) / Storage

# Pulse storage and migration

Pulse writes only inside `~/.pulse-analytics/`. Set `PULSE_HOME` to an absolute path to use another directory; on Windows the default is `%USERPROFILE%\.pulse-analytics`. `CLAUDE_HOME` and `CODEX_HOME` change where Pulse looks for sessions, not where it stores anything.

| Folder | Contents |
| --- | --- |
| `config/` | `app.json` (window), `provider.json` (selected provider), `claude-presence.json` and `codex-presence.json` (Discord), `opencode.json`, `orion.json`, `commandcode.json` (integrations) |
| `data/` | `pulse-analytics.db`: history, budgets, accounts and notifications. SQLite keeps its `-wal` and `-shm` files beside it while Pulse runs |
| `accounts/` | Private profiles of Pulse-managed account connections |
| `cache/` | `app-snapshot.json` (first frame of the next launch, saved at most once a minute), `claude-usage.json`, `codex-plan.json`, `claude-checkpoints/` (transcript parse checkpoints) and `codex-probe/` (the isolated `CODEX_HOME` of a Codex quota read). Safe to delete while Pulse is closed |
| `state/` | Lock files and the one-time migration receipt |
| `logs/` | `daemon.log`, capped at 1 MiB plus one `.old` copy |
| `webview/` | The embedded browser profile (local storage and HTTP cache). On macOS the system keeps it in its own WebKit store |

## What Pulse never touches

- Agent folders are read, never written: `~/.claude`, `~/.codex`, OpenCode's data folder, `~/.orion` and Command Code's folder. Pulse does not install hooks, statusline scripts or settings in them, and never rewrites an agent's login. Only a profile that Pulse itself created under `accounts/` gets refreshed tokens.
- Another program's SQLite database is opened read-only. When its owner is closed, Pulse opens it as immutable, so no `-wal` or `-shm` file appears next to it.
- No program runs on a timer. Process detection, the Git branch and instance checks run in-process. A Codex quota read starts `codex app-server` only when you select **Refresh** under **Provider limits** or refresh a Codex account, and its files stay in `cache/codex-probe/`. Opening a release page or connecting an account also runs a program, on that click only.

## First startup

1. Stop the previous Pulse installation before upgrading. Do not run old and new versions together.
2. Before polling or reading preferences, Pulse locks `state/migration.lock`.
3. Files from the flat 1.9 layout (`pulse-provider.json`, `claude/`, `codex/` and the others) move into the folders above. The analytics database is checkpointed first, so one move carries every committed transaction. A newer file is never replaced; its older duplicate is removed. Obsolete lock and metrics files are deleted and the empty `claude/` and `codex/` folders go away.
4. Releases before 1.9 kept Pulse files in `~/.claude`. Pulse copies their settings and history, checks the database copy with `PRAGMA quick_check`, then removes those Pulse files from `~/.claude`. Claude's own files are never touched. The Codex presence config is copied but stays in `~/.codex`, because Codex Discord Rich Presence uses it too.
5. Pulse writes a receipt to `state/`. Later starts do not import legacy data again.

A failed migration stops startup instead of silently creating empty history. Correct the reported path, permissions or corrupt source and retry; a partial migration resumes without overwriting anything.

## Recovery

Stop Pulse before restoring data. Retain the new directory first; use a consistent SQLite backup if its database is active. To run an older version, restore its matching binary and legacy settings/database backup. Legacy files do not contain sessions imported later into the new directory.

Do not delete the receipt to merge databases. Pulse does not automatically merge divergent databases. Keep both copies until recovery is verified.

### Correct saved Claude history

Earlier versions counted parts of some Claude responses more than once and priced Sonnet 5 and Fable 5.1 cache reads at outdated rates. Pulse corrects a session while it is active, but saved sessions that already ended keep their old totals until you correct them:

1. Open **Settings** and select **Check history** under **Claude history correction**. Pulse re-reads your transcripts and shows how many sessions would change, with token and cost totals before and after. Nothing is written.
2. Select **Correct sessions…**, then confirm. Pulse writes `data/pulse-analytics.db.pre-history-repair-<UTC time>.bak` with `VACUUM INTO`, checks it, and updates the sessions in one transaction.
3. Sessions whose transcript is no longer on this computer stay unchanged. Sessions priced by Claude's statusline keep their cost and receive corrected token counts only.

To undo the correction, stop Pulse and restore the backup file as `data/pulse-analytics.db`.

## Privacy

Pulse does not upload prompts or session transcripts and sends no analytics telemetry. Local reports can contain sensitive details; inspect them before sharing. Provider quota checks, update checks and enabled Discord fields still use the network. Storage migration sends no data over the network.

## Account registry

Schema 7 adds `accounts` and `account_snapshots`. Migration retains session and notification history and creates a consistent pre-migration backup. Account IDs isolate cached usage; removal records a tombstone instead of logging out an agent. Secret references point to protected files outside SQLite.

For rollback to an older binary, use the compatible pre-v7 database backup and preserve the v7 database separately. Take that backup before you install 1.9.0 over an existing 1.8.2 installation.
