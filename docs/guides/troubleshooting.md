# Troubleshooting and diagnostics

Pulse has two processes: the **`cc-discord-presence` daemon** (headless, pushes
Discord Rich Presence and writes metrics) and the **Pulse GUI** (the Tauri
analytics window). Both read the same data sources under `~/.claude`. Most
problems are a missing or stale source file, the wrong Discord app id, or
Discord not running. Start with `cc-discord-presence doctor`.

## First step: run the doctor

```
cc-discord-presence doctor
```

It prints the resolved paths and a pass/warn line for each dependency:

```
cc-discord-presence doctor
config_path: C:\Users\you\.pulse-analytics\config\claude-presence.json
statusline_path: C:\Users\you\.claude\discord-presence-data.json
credentials_path: C:\Users\you\.claude\.credentials.json
projects_paths: C:\Users\you\.claude\projects  (+ any WSL distro paths)
[OK] Discovered 1 accessible projects root(s).
[OK] Discord client id configured.
[INFO] Statusline data file not found (will fall back to JSONL parsing).
[OK] Anthropic credentials file exists.
[OK] claude command available.
[OK] git command available.
[OK] Plan: Max ($200/mo)
Doctor: healthy
```

`[WARN]` lines are real problems; `[INFO]` lines are graceful fallbacks, not
failures. Exit code is `0` when healthy, `1` when any `[WARN]` fired.

`cc-discord-presence status` prints the current snapshot and the live Discord
connection status string (`Connected`, `Reconnecting in Ns...`,
`Missing CC_DISCORD_CLIENT_ID`, `Discord error: ...`).

## Logs and `RUST_LOG`

The daemon logs to **stdout/stderr** via `tracing`. There is no rolling log
file — run the daemon from a terminal to see its output, and raise the level
with `RUST_LOG`. The default filter is `info`:

```
RUST_LOG=debug cc-discord-presence
```

Scope it to the crate to cut noise:

```
RUST_LOG=cc_discord_presence=debug
```

`debug` is where the useful detail lives: usage-API rate-limit backoff, token
refresh attempts, credential read/parse failures, and the extra-usage toggle
HTTP results.

**The Pulse GUI does not write a log file** and does not install a `tracing`
subscriber, so its `tracing` lines are discarded unless you launch the GUI
binary from a console with `RUST_LOG` set. Reproduce GUI-side issues that way.
If the Discord config cannot be read while the rest of the snapshot is healthy,
Home shows **Discord settings degraded** and keeps only the same provider's
last-good Discord view. Changing provider clears that retained view. The status
chip's tooltip carries the current parse/read error; it is an in-app diagnostic,
not a persistent log claim.

**Explorer-launch diagnostic log.** When the daemon binary is started by
double-clicking it in Explorer (not from a terminal), it appends a few
diagnostic lines to:

```
~/.pulse-analytics/logs/daemon.log
```

This only captures launch/exit/panic context for the "window closes
immediately" case. It is best-effort and silent on write failure; it is not the
general application log.

## Where the data comes from

Provider sources resolve under `CLAUDE_HOME` or `CODEX_HOME`. Pulse-owned data
uses `PULSE_HOME`, defaulting to `~/.pulse-analytics`. See [storage and recovery](storage.md).
The GUI refreshes every 5 seconds; the daemon interval is configurable.

| Source | Path | Role |
| --- | --- | --- |
| Claude Code session transcripts | `~/.claude/projects/**/*.jsonl` (+ WSL distro paths on Windows) | Authoritative per-turn tokens, cost, model, activity, reasoning effort, fast-mode speed |
| Statusline handoff | `~/.claude/discord-presence-data.json` | Claude Code's own billed `total_cost_usd` + token counts; wins the headline cost when present |
| Anthropic OAuth credentials | `~/.claude/.credentials.json` | Bearer token for the usage API (5h / 7d / extra-usage limits) |
| Usage cache | `~/.pulse-analytics/cache/claude-usage.json` | 5-min cache of the usage-API response (shared rate limit with Claude Code) |
| Config | `~/.pulse-analytics/config/claude-presence.json` | Discord client id, privacy toggles, asset keys, plan |
| Analytics database | `~/.pulse-analytics/data/pulse-analytics.db` | The Pulse GUI's SQLite store (WAL); historical sessions + daily stats |

Two precedence rules matter when numbers look off:

- **Statusline cost wins.** When `discord-presence-data.json` is fresh
  (modified within 60s) it supplies the authoritative `total_cost`; the JSONL
  per-category cost split is scaled to reconcile with it. With no statusline,
  cost is the JSONL estimate computed from per-token pricing.
- **Sessions must be recent.** A `*.jsonl` is only shown while it is inside the
  staleness / sticky window (`CC_PRESENCE_STALE_SECONDS`, default 90;
  `CC_PRESENCE_ACTIVE_STICKY_SECONDS`, default 300). Old transcripts are
  ignored by design.

## Common failures and fixes

### Presence not showing in Discord

In order of likelihood:

- **Discord desktop is not running.** Rich Presence rides the Discord IPC
  socket, which only exists when the desktop app is open. The browser client
  does **not** expose it. `status` shows `Reconnecting in Ns...` and the daemon
  retries with exponential backoff (5s → up to 60s). Open Discord; it
  reconnects on the next attempt.
- **Activity status is hidden.** Discord → Settings → Activity Privacy →
  "Display current activity as a status message" must be on.
- **Wrong / unconfigured app id.** Presence is published under a Discord
  application id. Default is the project's app (`1466664856261230716`). Override
  with `CC_DISCORD_CLIENT_ID` (env wins) or `discord_client_id` in the config.
  If the id points at a Discord app you do not control, the activity may publish
  but the **images won't** — image keys are resolved against that specific app's
  uploaded assets.
- **Logo / activity icons missing, text shows.** The large image and the
  per-activity small icons (`large`, `thinking`, `reading`, `editing`,
  `running`, `waiting`, `idle`) are **asset keys** that must be uploaded to the
  Developer Portal for the configured app. Discord silently drops plain
  `https://` image URLs on many client versions; the resolver wraps them as
  `mp:external/...` but that is best-effort. Upload the assets — see
  [discord-assets.md](discord.md). Config migrations move legacy defaults
  from the old GitHub-raw URL and the letterboxed `claude-code` portal asset to
  the square Developer Portal asset.
- **Presence is stuck or one-line.** The daemon dedups identical payloads and
  rate-limits publishes to once per 2s, with a 30s heartbeat re-send to keep the
  IPC alive. A frozen presence usually means the underlying session went stale
  (see the staleness window above) — it falls back to the idle
  "Waiting for session" presence.

### "Waiting for session" / no project, but Claude Code is running

The daemon found no fresh JSONL or statusline data:

- **No transcript yet.** A brand-new Claude Code session writes its first
  assistant turn before it has a model id; until then the session is skipped.
  Send one message.
- **Transcript is stale.** Last activity is older than the staleness window.
  Raise `CC_PRESENCE_STALE_SECONDS` if your sessions idle a lot.
- **`projects/` not found.** `doctor` prints `[WARN] No discovered Claude Code
  sessions directory is currently accessible.` Confirm `~/.claude/projects`
  exists, or set `$CLAUDE_HOME` to the correct root. On Windows the daemon also
  probes WSL distro paths (`\\wsl.localhost\<distro>\home\<user>\.claude\projects`).
- **Empty / malformed JSONL.** Blank lines are skipped; lines that fail to parse
  as JSON are skipped individually (one bad line does not poison the file). A
  file with no parseable assistant message and no model id yields no snapshot.
  The parser is incremental (it seeks from a saved cursor); if a file is
  truncated or rewritten shorter, the cursor resets and it re-reads from the
  start.

### Cost is missing or looks like a rough estimate

- **No statusline file** (`[INFO] Statusline data file not found`). Cost then
  falls back to the **JSONL estimate** from per-token pricing in
  `cost.rs` — close, but it is not Claude Code's own billed figure. To get the
  authoritative number, configure Claude Code's statusline to write
  `~/.claude/discord-presence-data.json`.
- **Statusline present but cost still off.** The file must be **fresh** (modified
  within 60s) and carry a valid 36-char UUID `session_id`; otherwise it is
  ignored and JSONL takes over. A `display_name` of `Test` or a `<synthetic>`
  model id is rejected.
- **5h / 7d limits or extra-usage missing.** Those come from the usage API,
  which needs `~/.credentials.json`. `[INFO] Credentials file not found` means
  no limits. Watch `RUST_LOG=debug` for `no credentials`, `token expired`,
  `auth failed — re-login to claude.ai`, or `refreshing in Ns` (a 429 backoff —
  the endpoint shares Claude Code's rate limit and is cached 5 min).

### Fast mode not detected (no ⚡, cost not doubled)

Fast mode is read **only** from the per-turn `usage.speed` field in the JSONL
(`"fast"` → Fast, anything else/absent → Standard). It is **not** inferred from
settings. Two conditions must both hold for the ⚡ marker and the 2× billing:

1. The assistant turn's `usage.speed` is `"fast"`, **and**
2. the model is fast-capable (Opus 4.8+; `is_fast_capable`).

If a model that is not fast-capable reports `speed: "fast"`, the flag is ignored
and cost is unchanged — by design. If you expect fast mode and don't see it,
confirm the latest assistant turn in the transcript actually carries
`usage.speed: "fast"`; the statusline file does not carry speed, so a
statusline-only session always shows Standard until JSONL merges in.

### Reasoning effort shows the wrong level

Current Claude Code builds record the composer's effort selector in the
transcript: every `type: "assistant"` line carries a **top-level** `effort`
field (`low` | `medium` | `high` | `xhigh` | `max`), a sibling of `type` and
`message`. That is Pulse's authoritative source, and the latest main-chain value
wins, so changing the selector mid-session is reflected on the next turn.

Two deliberate exclusions:

- **Sidechain lines are ignored.** A subagent can run at its own effort and its
  turns are interleaved into the parent transcript, so honouring them would let
  a subagent define the tier you see for the whole session.
- **Legacy reminders never override the field.** The older
  `<reasoning_effort>` / "reasoning effort level: X" system-reminder scrape is
  kept only for transcripts written before the field existed, and is skipped
  once a real value has been observed.

Pulse still does **not** infer effort from the presence of thinking blocks —
that produced a confidently-wrong "High".

If a session reports the default `Medium`, check that its transcript actually
contains the field:

```bash
grep -o '"effort":"[a-z]*"' ~/.claude/projects/<encoded-project>/<session>.jsonl | sort | uniq -c
```

No matches means the build writing that transcript predates the field; Pulse
then falls back to `effortLevel` in `~/.claude/settings.json`, if present.

### SQLite database locked

`pulse-analytics.db` is the GUI's store, opened in WAL mode behind a single
in-process mutex. `database is locked` / `SQLITE_BUSY` shows up when:

- **Two Pulse GUI instances** are open against the same `PULSE_HOME`. The daemon
  uses a single-instance lock (`~/.pulse-analytics/state/daemon.lock`) and takes
  over a stale instance, but two GUI windows can still contend. Close the extra
  one.
- **An external reader holds a write lock** (a DB browser with the file open in
  read/write, a backup tool, antivirus scanning the `-wal`). Close it.
- **Stale WAL sidecars after a hard kill.** If the process was killed, the
  `pulse-analytics.db-wal` / `-shm` files can linger. Close all readers and let
  WAL checkpoint on next open; if it stays wedged, deleting the `-wal` and
  `-shm` files (with every reader closed) is safe — the main `.db` is intact.

The DB is rebuildable: it is derived from the JSONL transcripts, so deleting
`pulse-analytics.db` (GUI closed) just re-imports history on next launch. Back
it up first if you want the historical daily-stats rollups.

### GPU / "none" hardware acceleration in the GUI

The Pulse window is a Tauri WebView (WebView2 on Windows). On machines with no
usable GPU, with a stale/blocked GPU driver, or under remote-desktop/VM
sessions, WebView2 falls back to software rendering — charts render slower but
correctly. This is a WebView2 fallback, not a Pulse bug. If the window is blank
or fails to create, confirm the **WebView2 Runtime** is installed (Pulse needs
it), update the GPU driver, or force software rendering. Nothing in the GUI
requires the GPU; the daemon is headless and never touches it.

### Linux: closing the window quits Pulse instead of hiding it

Close-to-tray needs `libayatana-appindicator3.so.1` or `libappindicator3.so.1`. When neither loads, Pulse skips the tray and quits on close, and logs one warning (`no appindicator library found`). Install the library and start Pulse again: `libayatana-appindicator` on Arch and CachyOS, `libayatana-appindicator3-1` on Debian and Ubuntu, `libayatana-appindicator-gtk3` on Fedora. GNOME also needs an AppIndicator shell extension to show the icon. Your close-to-tray setting is not changed.

### Linux: blank window on NVIDIA

WebKitGTK's DMA-BUF renderer can leave the window empty with the NVIDIA driver. Pulse sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` at startup when the variable is unset and `/proc/driver/nvidia/version` exists. If the window is still blank, start Pulse from a terminal to read the WebKit messages, and try `WEBKIT_DISABLE_COMPOSITING_MODE=1 pulse`. A value you export, including `WEBKIT_DISABLE_DMABUF_RENDERER=0`, is respected.

### Linux: Wayland

Pulse uses its own title bar. If dragging, resizing or snapping misbehaves on Wayland, run with `GDK_BACKEND=x11 pulse` to compare, and report which compositor shows the problem. The tray icon needs a StatusNotifier host, which KDE Plasma provides.

### Linux: no update button

Arch, `.deb` and `.rpm` installs are updated by the package manager, so the banner offers **Open release** only. An AppImage updates in place. See [updates.md](updates.md#package-manager-installs-and-appimage).

### Linux: Discord banner or user missing

Pulse reads Discord's profile data from `$XDG_CONFIG_HOME` (default `~/.config`) for Discord, Canary and PTB, from Flatpak (`~/.var/app/com.discordapp.Discord/config/discord`), Snap, Vesktop (`vesktop/sessionData`) and Legcord. Start the client once so those folders exist. The live connection itself does not depend on them.

## Reset checklist

When state looks corrupt, with **all Pulse processes closed**:

1. Back up `~/.pulse-analytics/data/pulse-analytics.db` if you want the history.
2. Delete the `cache/` folder and, to rebuild history from the transcripts,
   `data/pulse-analytics.db` with its `-wal` and `-shm` files. Both rebuild.
3. Keep `config/` (your settings). Claude Code's `.credentials.json` is never
   touched by Pulse.
4. Relaunch and run `cc-discord-presence doctor`.

## Related docs

- [index.md](../index.md) — documentation map
- [discord-assets.md](discord.md) — uploading the RP logo + activity icons
- [cost-calculation.md](costs.md) — pricing, cache math, fast mode, 1M surcharge
- [opus-4-8.md](../models/claude.md) — fast mode and Opus 4.8 specifics
