[Documentation](../index.md) / Guides / Orion App

# Use Orion App with Pulse

The Orion App adapter reads Orion's local session store. Orion desktop and the Orion CLI write the same store, so both appear as one provider named **Orion App**, with one Discord application, one artwork and one set of preferences.

## Session source

Pulse opens `~/.orion/cli/db/db.sqlite` read-only. When that file is missing, Pulse uses the legacy `~/.zcode/cli/db/db.sqlite` link. `PULSE_ORION_HOME` overrides the data root; `data_roots` in `~/.pulse-analytics/pulse-orion.json` selects explicit roots. Pulse never writes to the Orion database.

| Orion data | Pulse use |
| --- | --- |
| `session` rows with `task_type = 'interactive'` and no parent | One Pulse session each |
| `subagent_child` sessions | Rolled into the parent's tokens and cost; counted as subagents |
| Assistant `message.data.tokens` | Input, output, reasoning, cache read and cache write totals |
| Latest `runtime/model_selection` entry | Current model and reasoning level |
| Running `tool` part in the last two minutes | Current activity and sanitized file name |
| `~/.orion/v2/provider_config.json` model rules | Context window per provider and model |

Orion's `tokens.input` already includes cache read and cache write. Pulse subtracts both once to get uncached input. The current context fill is the latest completed assistant message's `tokens.total`, not the cumulative session total. When Orion has no context rule for a model, Pulse uses its bundled Claude or Codex facts; otherwise context stays unreported.

## Cost

Orion stores `cost: 0` on every message. That value is not a price, so Pulse ignores it. Pulse estimates an API-equivalent value only for models with a known public rate: Claude models use `src/cost.rs`, and models in the Codex catalog use `src/codex/cost.rs`. A session that mixes priced and unpriced models shows a partial subtotal. A session with no priced model shows cost as unavailable, never as zero. Subscription usage through OpenCodex is not an API bill.

Orion reports no account quota or credits to Pulse. The Quotas and Credits fields stay unavailable for this provider.

## Discord identity and artwork

| Field | Value |
| --- | --- |
| Application name | Orion App |
| Application ID | `1553775289274994708` |
| Large asset key | `orion` |
| Published asset ID | `1553775834148380782` |
| Local asset | `frontend/src/assets/rp/orion.png` |

The artwork is the Orion App icon, `packages/desktop/build/icons/1024x1024.png` in the Orion repository, at 1024×1024.

With Orion App selected and Rich Presence enabled, Pulse publishes an active session with model, activity, project, branch, tokens, cost, context and subagent count, subject to the field switches and privacy mode. Without an active session, Pulse publishes a plain Idle presence without historical fields or a session timer.

## Verify the integration

Unit tests cover the root and subagent roll-up, cache arithmetic, live activity, context rules, a missing database and a read-only database file. The ignored `local_orion_database_probe` test reads the real Orion store without writing to it. The ignored `local_discord_acknowledges_orion_presence` test publishes one diagnostic activity through local Discord IPC and clears it.

On 2026-09-27, the probe read the active Orion session in 118 ms on the first poll and 9 ms on the next. Local Discord acknowledged `SET_ACTIVITY` for application `1553775289274994708` and resolved the large image to asset `1553775834148380782`. An acknowledged asset ID is not a screenshot of Discord rendering it.
