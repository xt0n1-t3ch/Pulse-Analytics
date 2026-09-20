[Documentation](../index.md) / Storage

# Pulse storage and migration

Pulse stores its own data in `~/.pulse-analytics/`. Set `PULSE_HOME` to an absolute path to use another directory. On Windows, the default is `%USERPROFILE%\.pulse-analytics`. `CLAUDE_HOME` and `CODEX_HOME` change provider discovery, not Pulse storage. This behavior is the same in 1.8.2 and 1.9.0.

| Location inside the Pulse directory | Contents |
| --- | --- |
| `pulse-analytics.db` | History, budgets and notifications; SQLite WAL sidecars stay beside it |
| `pulse-provider.json`, `pulse-app-settings.json` | Selected provider and window preferences |
| `pulse-opencode.json` | OpenCode integration and presence settings |
| `pulse-last-app-snapshot.json` | Cached application snapshot |
| `claude/` | Presence settings, usage cache, daemon metrics and diagnostic log |
| `codex/` | Presence settings and plan cache |
| `legacy-migration-v1.json`, `migration.lock` | Migration receipt and coordination |

Provider credentials, transcripts, model inventories, OpenCode databases and the Claude statusline handoff stay in their existing directories. Legacy daemon instance locks also stay at their existing paths so old and new daemons cannot publish concurrently. Locks are coordination files, not analytics storage.

## First startup

1. Stop the previous Pulse installation before upgrading. Do not run old and new versions together.
2. Before polling or reading preferences, Pulse locks the migration directory.
3. Missing Pulse files are copied from the configured legacy Claude/Codex homes. Existing destination files are never replaced. JSON copies are parsed and byte-checked.
4. SQLite creates a consistent snapshot, including committed WAL transactions. Pulse checks it with `PRAGMA quick_check` before installing it without overwrite.
5. Pulse writes a receipt after success. Later starts do not import legacy data again.

Original files remain for rollback. A failed migration stops startup instead of silently creating empty history. Correct the reported path, permissions or corrupt source and retry. Partial migration resumes without overwriting destination files. The directory override uses the same migration; isolate provider homes too when testing.

## Recovery

Stop Pulse before restoring data. Retain the new directory first; use a consistent SQLite backup if its database is active. To run an older version, restore its matching binary and legacy settings/database backup. Legacy files do not contain sessions imported later into the new directory.

Do not delete the receipt to merge databases. Pulse does not automatically merge divergent databases. Keep both copies until recovery is verified.

## Privacy

Pulse does not upload prompts or session transcripts and sends no analytics telemetry. Local reports can contain sensitive details; inspect them before sharing. Provider quota checks, update checks and enabled Discord fields still use the network. Storage migration sends no data over the network.

## Account registry

Schema 7 adds `accounts` and `account_snapshots`. Migration retains session and notification history and creates a consistent pre-migration backup. Account IDs isolate cached usage; removal records a tombstone instead of logging out an agent. Secret references point to protected files outside SQLite.

For rollback to an older binary, use the compatible pre-v7 database backup and preserve the v7 database separately. Take that backup before you install 1.9.0 over an existing 1.8.2 installation.
