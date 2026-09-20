[Documentation](../index.md) / Guides / Command Code

# Use Command Code with Pulse

The Command Code adapter reads CLI and Desktop sessions through one Rust collector. Both clients use the visible name **Command Code**, the same Discord application, artwork and preferences.

## Session sources

Pulse reads `.commandcode/projects/**/*.jsonl`. `PULSE_COMMANDCODE_HOME` overrides the default profile; `profile_paths` in `pulse-commandcode.json` selects explicit profiles. Desktop's `Command Code/thread-index.db` adds titles and branch metadata. Pulse opens that index read-only.

The collector excludes checkpoints and hook audit logs. It deduplicates message IDs and session IDs before storage. Completed sessions remain in history. A recent session with active work supplies session details. Starting with 1.9.2, when Command Code is selected and Rich Presence is enabled, Pulse keeps a plain Idle presence visible without an active session. Idle never reuses historical project, model, cost, tokens, context or session elapsed time. Turning off Rich Presence or selecting another broadcaster clears it.

Token categories and `costUsd` come from Command Code telemetry. Missing monetary values stay unavailable. Pulse retains known subtotals when some messages lack cost. It does not borrow Claude or Codex pricing. Context appears only when observed; cumulative session tokens do not become context fill.

Configured custom provider prefixes mark BYOK sessions. These sessions do not publish Command Code subscription quotas or credits as their own allowance.

## Account usage

The account reader uses the authenticated Command Code endpoints:

- `/alpha/whoami`
- `/alpha/billing/credits`
- `/alpha/billing/subscriptions`

Five-hour and weekly meters use the response's native `used` and `cap`. The monthly meter subtracts remaining monthly credits from the authenticated plan allocation. Explicit API allocations take priority over bundled plan data. An unstarted window with `resetAt: 0` does not display the Unix epoch.

The individual plan allocation fallback matches Command Code CLI 1.39.0's `getPlanInfo` catalog, checked on 2026-09-20. Current Go, GOAT, Pro, Max and Ultra allocations were cross-checked against [Command Code pricing](https://commandcode.ai/pricing). Legacy `individual-pro` remains distinct from `individual-pro-v1`. API-only, team and unknown plans require an explicit allocation response; Pulse does not guess it from price, tokens or model discounts.

## Discord identity and artwork

| Field | Value |
| --- | --- |
| Application name | Command Code |
| Application ID | `1551026507806281829` |
| Large asset key | `commandcode` |
| Published asset ID | `1551028194629521428` |
| Local asset | `frontend/src/assets/rp/commandcode.png` |

The unmodified image comes from the [official Command Code artwork](https://raw.githubusercontent.com/CommandCodeAI/command-code/refs/heads/main/.github/commandcode/symbols/spaced-bg-black-symbol-commandcode.png), linked by the [brand page](https://commandcode.ai/brand). Its SHA-256 is `5D1BF3183D6CACC975D12F9F3043D313DCD374FF5F104CF3187718DE946297AF`.

The public asset inventory and local Discord `SET_ACTIVITY` acknowledgement resolved that published asset ID on 2026-09-20. The diagnostic activity was cleared. An acknowledged asset ID is not a screenshot of Discord rendering it.

## Verify the integration

Unit tests cover deduplication, history/activity separation, BYOK and privacy. The opt-in `tests/commandcode_live.rs` reads actual CLI/Desktop history without editing it. Its separate Discord test publishes a labelled diagnostic activity and requires an explicit proof-output path. Stop another Pulse publisher before that test and restore it afterward.

The host proof read six Desktop sessions and ten CLI sessions without duplicate session IDs. Native Tauri Accounts also rendered real usage through IPC without the browser proxy. These results do not prove every Command Code client release or a new live Desktop conversation.

The 1.9.2 idle correction was checked locally through native Tauri IPC, real Discord acknowledgement from the production publisher and a user-confirmed Discord screenshot. Command Code used its own name and published artwork while idle. The installed release remains a separate delivery check.
