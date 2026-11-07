[Documentation](../index.md) / Guides / Orion App

# Use Orion App with Pulse

The Orion App adapter reads Orion's local session store. Orion desktop and the Orion CLI write the same store, so both appear as one provider named **Orion App**, with one Discord application, one artwork and one set of preferences.

Pulse reads only. It opens every Orion SQLite file read-only, never writes or locks an Orion file, and never opens `providers.json`, `v2/credentials.json` or any other credential file.

## Where Orion keeps its data

Orion has used one layout on every operating system since its 2026-10-17 session-storage change: conversations live in one JSONL file per session, listed by a small index database, and per-request timings live in a separate database. Orion no longer writes conversations to SQL tables.

| System | Default data root |
| --- | --- |
| Linux | `~/.orion` |
| macOS | `~/.orion` |
| Windows | `%USERPROFILE%\.orion` |

Pulse picks the data root in this order and uses the first match:

1. `PULSE_ORION_HOME`, used as given.
2. `ORION_DATA_BASE_DIR`, when it is an absolute path: `<ORION_DATA_BASE_DIR>/.orion`. Orion applies the same rule and never falls back from it, so Pulse does not either.
3. `~/.orion`, when it holds a session index or a pre-cutover database.
4. The legacy `~/.zcode` link, when it holds one of those and `~/.orion` does not.
5. `~/.orion`, so the diagnostic names the expected place.

`data_roots` in `~/.pulse-analytics/config/orion.json` replaces the default with explicit roots. The path rules come from Orion's source (`orion-app-process/src/data_root.rs`). Only a Linux store has been compared against Pulse's reader; the macOS and Windows locations follow the same code and are not yet verified on those systems.

## What Pulse reads

| File under the data root | Pulse use |
| --- | --- |
| `projects/index.sqlite` | Session list: root sessions are rows with no parent, `task_type = 'interactive'` and no archive time. Subagents are rows whose `parent_id` points at the session. Freshness is `last_write_ms`, because `time_updated` lags by up to 30 seconds |
| `projects/<workspace>/<session>.jsonl` (`.jsonl.gz` after 7 days) | Conversation records of a session, and of each subagent under `<session>/subagents/` |
| `telemetry/usage.sqlite` | Per-request timings and helper requests, kept by Orion for 30 days |
| `cli/db/db.sqlite` (also under `~/.zcode`) | Pre-cutover sessions, read only while its `session` table still has rows and only when no `projects/index.sqlite` exists |

Transcripts are streamed line by line, never loaded whole. Pulse remembers how far it read each live transcript and parses only the appended bytes on the next 5 second poll; a file that shrinks or turns into gzip is read again from the start. Large tool outputs that Orion moved to `tool-results/` are not needed and are not read. Finished sessions keep no parsed state in memory.

| Orion record | Pulse use |
| --- | --- |
| Assistant `message` records | Input, output, reasoning, cache read and cache write tokens. Orion writes a streamed message several times and the first write has 0 tokens, so only the last line of each message id counts. Each message is priced as one request |
| `runtime/model_selection` entry | Current model, provider and reasoning level |
| `runtime/context_usage` entry | Real context window, current fill, auto-compact threshold and prompt cache hit rate |
| Running `tool` part written in the last two minutes | Current activity and sanitized file name |
| Subagent transcripts | Rolled into the parent's tokens and cost. Live cards and Discord count only subagents still working (latest request streaming or calling tools, written in the last two minutes); history keeps the total started |

Orion's `tokens.input` already includes cache read and cache write. Pulse subtracts both once to get uncached input. The current context fill is Orion's own `usedTokens`; without that entry Pulse falls back to the latest completed request's prompt size and to its bundled Claude or Codex window.

## Usage telemetry

`telemetry/usage.sqlite` adds what transcripts do not hold. Pulse reads it per root session, including its subagents:

| Field | Meaning |
| --- | --- |
| Time to first token | Median over completed main and subagent requests |
| Output speed | Output tokens divided by generation time over the same requests |
| Helper requests | Count, tokens and API-equivalent cost of requests that are neither `main_turn` nor `subagent`: title, compaction, prediction and prompt enhancement. They never appear as transcript messages, so they are reported beside the transcript totals, not added to them |
| Subagents by agent | Requests, tokens and API-equivalent cost per agent type (for example `orion-Explore`), most expensive first |

Orion deletes rows older than 30 days. Transcripts stay the source of token totals and history. For a session that began before the retention window, helper requests and the per-agent breakdown are unavailable, because sums over the surviving rows would be too low. A session with no telemetry rows at all, or no telemetry database, has every one of these fields unavailable. Unavailable is shown as unavailable, never as zero. A cost is a lower bound when some request uses a model with no known rate.

## What stays unavailable

| Item | Reason |
| --- | --- |
| Billed cost | Orion stores `cost: 0` on every message and records no price. Pulse calculates an API-equivalent estimate instead |
| Account quota and credits | Orion keeps them in `providers.json`, next to credential-type values. Pulse does not open that file, so the Quotas and Credits fields stay off |
| Cost of an unknown model family | Models such as `deepseek/...` have no rate card in Pulse and stay unpriced |
| Telemetry for sessions older than 30 days | Orion removed the rows |

## Cost

Orion stores no price, so Pulse calculates the session cost the way it does for Claude Code: exact token counts times published per-model rates. Claude models use `src/cost.rs`, and models in the Codex catalog use `src/codex/cost.rs`. Orion lists Codex-routed models by bare id or display name (`gpt-6.1-terra`, `6.1-Terra`, `GPT-6.1-Terra`, each with an optional `-Fast` suffix); all of them resolve to the catalog entry. A subscription or provider plan behind Orion is not an API bill, so the value is a calculation, not an invoice.

Pulse shows what it shows for a Claude Code session: a **Session cost** headline, the input, output, cache write and cache read costs, value per hour, output speed, and the same cost in session history, the Costs view, forecasts, reports and the Discord cost field. A session that mixes priced and unpriced models shows a **Known subtotal** marked partial. A session with no priced model shows cost as unavailable, never as zero. The [cost guide](costs.md#orion-app) has the arithmetic, the basis table and how stored rows are re-imported.

## Discord identity and artwork

| Field | Value |
| --- | --- |
| Application name | Orion App |
| Application ID | `1553775289274994708` |
| Large asset key | `orion` |
| Published asset ID | `1553775834148380782` |
| Local asset | `frontend/src/assets/rp/orion.png` |

The artwork is the Orion App icon, `packages/desktop/build/icons/1024x1024.png` in the Orion repository, at 1024×1024.

With Orion App selected and Rich Presence enabled, Pulse publishes an active session with model, activity, project, branch, tokens, cost, context and the number of subagents still working, subject to the field switches and privacy mode. Without an active session, Pulse publishes **Orion App** / **Idling...** without historical fields. Its timer counts from the last Orion activity, so Discord reads as idle time, and the Pulse preview spells it out as "Idling for 12m".

## Verify the integration

Unit tests build small Orion stores in temporary folders with Orion's real column and record names. They cover the data-root order, transcript paths, last-line-wins messages, incremental and gzip transcripts, the root and subagent roll-up, Orion's context entry, running subagents and tools, freshness from `last_write_ms`, usage telemetry (median time to first token, output speed, helper split, per-agent breakdown), unavailable telemetry past the retention window, the legacy layout, read-only access and GPT-6.1 Terra names and prices.

```bash
cargo nextest run -p cc-discord-presence orion
cargo nextest run -p cc-discord-presence --run-ignored ignored-only local_orion_store_probe
```

The ignored `local_orion_store_probe` test reads the real Orion store of the machine without writing to it and prints counts, timings and numbers, never message text. The ignored `local_orion_database_probe` test does the same for a pre-cutover database. The ignored `local_discord_acknowledges_orion_presence` test publishes one diagnostic activity through local Discord IPC and clears it.

On 2026-09-27, local Discord acknowledged `SET_ACTIVITY` for application `1553775289274994708` and resolved the large image to asset `1553775834148380782`. An acknowledged asset ID is not a screenshot of Discord rendering it.
