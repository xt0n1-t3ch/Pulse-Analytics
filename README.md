<!-- Landing page: help coding-tool users inspect Pulse, install it and find the relevant guide. -->
<div align="center">

<h1>
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="assets/pulse-logo-dual-light.png">
    <img src="assets/pulse-logo-dual-dark.png" alt="Pulse" width="560">
  </picture>
</h1>
<h2>Your coding activity. One clear view.</h2>
<p>Local-first AI coding analytics and Discord Rich Presence for<br>Claude Code, OpenAI Codex and OpenCode.</p>

<p>
  <a href="https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/latest"><img src="https://img.shields.io/github/v/release/xt0n1-t3ch/Pulse-Analytics?style=flat-square&amp;label=release&amp;color=171717&amp;labelColor=303030" alt="Latest GitHub release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-171717?style=flat-square&amp;labelColor=303030" alt="License: Apache-2.0"></a>
  <a href="src-tauri/Cargo.toml"><img src="https://img.shields.io/badge/built_with-Tauri_2-171717?style=flat-square&amp;labelColor=303030" alt="Built with Tauri 2"></a>
</p>

<p><a href="#install"><img src="assets/icons/download.svg" alt="" width="20" height="20" align="center"> Download</a> &middot; <a href="#preview"><img src="assets/icons/image.svg" alt="" width="20" height="20" align="center"> Preview</a> &middot; <a href="#whats-new"><img src="assets/icons/sparkles.svg" alt="" width="20" height="20" align="center"> What's new</a> &middot; <a href="#use-pulse"><img src="assets/icons/terminal.svg" alt="" width="20" height="20" align="center"> Use Pulse</a> &middot; <a href="docs/index.md"><img src="assets/icons/map.svg" alt="" width="20" height="20" align="center"> Documentation</a></p>


</div>

| Live activity | Provider limits | Discord presence |
| --- | --- | --- |
| See your active project, model and session context. | Read account usage without mixing providers or session tokens. | Choose which application and fields your profile shares. |

Pulse is an open-source desktop dashboard for **Claude Code analytics, OpenAI Codex usage and OpenCode sessions**. Track tokens, API-equivalent costs, prompt-cache efficiency and account limits, then share selected live details through Discord Rich Presence.

Live activity, session history and account limits stay separate. Missing prices remain unavailable, not zero. Your session analytics stay on your machine.

<h2 id="preview"><img src="assets/icons/image.svg" alt="" width="28" height="28" align="center">&nbsp; Preview</h2>

See provider limits and active sessions in the current Home view. Select the screenshot to inspect the original capture.

<p align="center">
<a href="assets/evidence/pulse-home-v1.8.2-2026-09-07.png"><img src="assets/evidence/pulse-home-v1.8.2-2026-09-07.png" alt="Pulse 1.8.2 Home in dark mode: separate Claude, Codex and OpenCode account limits above two active coding sessions" width="960"></a>
<br><sub>Provider limits and live sessions. Pulse 1.8.2 on Windows.</sub>
</p>

<details>
<summary>Explore earlier Discord previews</summary>

These repository captures show earlier Claude and Codex presence layouts. They are historical examples, not screenshots of the current Pulse interface.

<p align="center">
  <a href="assets/screenshots/discord-rich-presence.png"><img src="assets/screenshots/discord-rich-presence.png" alt="Historical Claude Code Discord Rich Presence example" width="360"></a>
  <a href="assets/screenshots/codex-discord-rich-presence.png"><img src="assets/screenshots/codex-discord-rich-presence.png" alt="Historical Codex Discord Rich Presence example" width="360"></a>
</p>

</details>

<a id="whats-new-in-v180"></a>
<a id="whats-new-in-v181"></a>

<h2 id="whats-new"><img src="assets/icons/sparkles.svg" alt="" width="28" height="28" align="center">&nbsp; What's new</h2>

Pulse v1.9.5 is available for Windows, macOS and Linux, on both x64 and ARM64.

<h3 id="new-in-v195">New in v1.9.5</h3>

- **Your current Claude plan.** After you upgrade or downgrade, Pulse shows the new plan, for example Max 20x instead of Max 5x. It now reads the plan Claude Code keeps with your account profile, which Claude Code updates, rather than the one saved when you last logged in.
- **Correct prices.** Sonnet 5 stays at $2/$10, because Anthropic cancelled the planned increase. Fable 5.1 and Mythos 5.1 cache reads cost $0.25 per million instead of $1.
- **Fix your saved history.** In **Settings > Claude history correction**, Pulse checks saved sessions against their transcripts and shows what would change. After you confirm, it backs up the database and corrects the sessions that earlier versions overcounted.
- **Cleaner data.** Pulse keeps its session checkpoints in its own folder and removes outdated ones. Usage figures from one Claude account are never shown for another, and the first screen after an update no longer shows the previous version's numbers.

See the [1.9.5 changelog](CHANGELOG.md#195---2026-09-22).

<h3 id="new-in-v194">New in v1.9.4</h3>

Claude session costs are now accurate. Claude Code writes a separate transcript line for each part of a response, and every line repeats the same token usage. Earlier versions added each line, so tokens and costs could come out two or three times too high. Pulse now counts each response once.

Pulse also re-reads your active Claude sessions once after an update. That way, new prices apply to the whole session, not only to messages sent after the update. Costs saved for sessions that already ended are not rewritten. See the [1.9.4 changelog](CHANGELOG.md#194---2026-09-22).

<h3 id="new-in-v193">New in v1.9.3</h3>

Pulse now supports Claude Opus 5.5. Sessions use Anthropic's published rates: $4 per million input tokens, $20 per million output tokens and $0.20 per million cache reads. Fast mode costs $8/$40, and the full 1M context window has no long-context surcharge. Earlier versions priced Opus 5.5 like Opus 5, so estimates came out too high. Pulse and Discord now show the model as "Opus 5.5 (1M)".

Your database, settings and Discord configuration carry over from 1.9.2 unchanged. See the [1.9.3 changelog](CHANGELOG.md#193---2026-09-22).

<h3 id="new-in-v192">New in v1.9.2</h3>

Command Code stays visible in Discord while it is idle. See the [1.9.2 changelog](CHANGELOG.md#192---2026-09-20).

<h3 id="highlights-of-v19">Highlights of the 1.9 line</h3>

- Link several accounts per provider in [Accounts](docs/guides/accounts.md) and see each account's allowances, credits and banked resets. Removing a link never signs your coding client out.
- Track [Command Code](docs/guides/command-code.md) sessions from the CLI and the desktop app together, with the costs Command Code reports and one Discord identity.
- See Codex session costs add up event by event. Discord shows the known amount, and Pulse shows the full cost coverage.
- Track OpenCode sessions from its local database, including sessions that mix models and the costs the provider reports.
- Check OpenCode Go limits for the five-hour, weekly and monthly windows, separately from other providers.
- Recognize GPT-6 Astra sessions with their context and Standard/Fast pricing. When a price is missing, Pulse shows that instead of guessing.
- Customize OpenCode's Discord presence: put the model first, keep your own field order and show costs to two decimals. Finished sessions stop broadcasting.
- Review the last seven days of activity per provider on the dashboard, in a dark or light theme.
- Mark notifications as read or unread, and undo a clear even after you restart the app.
- Keep your data in `~/.pulse-analytics`, or choose another folder with `PULSE_HOME`. Data from older versions is copied over and never overwritten.
- On Windows, Pulse lowers its background priority with Efficiency mode and EcoQoS.

The [changelog](CHANGELOG.md) has the full release history.

<h2 id="install"><img src="assets/icons/download.svg" alt="" width="28" height="28" align="center">&nbsp; Install</h2>

Choose an installer for your operating system and architecture from [GitHub Releases](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/latest).

| Platform | v1.9.5 downloads | Installer formats |
| --- | --- | --- |
| Windows | [x64](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/download/v1.9.5/pulse-windows-x64-Pulse_1.9.5_x64-setup.exe) · [ARM64](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/download/v1.9.5/pulse-windows-arm64-Pulse_1.9.5_arm64-setup.exe) | `.exe` (NSIS), `.msi` |
| macOS | [Intel](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/download/v1.9.5/pulse-macos-x64-Pulse_1.9.5_x64.dmg) · [Apple Silicon](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/download/v1.9.5/pulse-macos-arm64-Pulse_1.9.5_aarch64.dmg) | `.dmg`, app archive |
| Linux | [x64](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/download/v1.9.5/pulse-linux-x64-Pulse_1.9.5_amd64.AppImage) · [ARM64](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/download/v1.9.5/pulse-linux-arm64-Pulse_1.9.5_aarch64.AppImage) | `.deb`, `.rpm`, `.AppImage` |

macOS GitHub packages are not Apple-signed or notarized. macOS may block first launch. Updater signatures are separate from Apple signing.

The immutable [v1.9.5 release](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/tag/v1.9.5) contains all six targets. The links above select NSIS, DMG or AppImage; MSI, DEB and RPM alternatives are on the release page. Native build/test checks passed; installed-GUI acceptance is recorded per operating system. See [platform verification](docs/maintainers/platforms.md).

Every complete release must include `SHA256SUMS.txt`, Windows software bills of materials and `latest.json` with signed updater payloads for all six targets. Do not install an asset for a different architecture to work around a missing download.

<details>
<summary>Windows installation</summary>

Download the matching `.exe` or `.msi`, compare its SHA-256 hash with `SHA256SUMS.txt`, then run the installer. Pulse uses Microsoft Edge WebView2. Windows Efficiency mode is specific to Windows.

Asset names include the platform and architecture, such as `pulse-windows-x64-Pulse_1.8.2_x64-setup.exe`. Each Windows architecture has its own SPDX file.

</details>

<details>
<summary>macOS installation</summary>

Download the Intel or Apple Silicon `.dmg`, verify its checksum and drag Pulse to Applications. The configured minimum is macOS 11.0.

These GitHub packages have no Apple Developer ID signature or notarization. Keep system security controls enabled; the native build checks do not establish Gatekeeper acceptance.

</details>

<details>
<summary>Linux installation</summary>

Verify the checksum and install the package for your distribution and architecture. Debian-based systems use `.deb`; RPM-based systems use `.rpm`. AppImage is the standalone packaging option, not a promise of compatibility with every distribution.

The release builds use Ubuntu 22.04. GTK 3, WebKitGTK 4.1 and an AppIndicator-compatible desktop are required for the corresponding native features. Tray behavior depends on the desktop environment.

</details>

See [platform support and verification](docs/maintainers/platforms.md) for the release gate, runtime checks and known limits. Manual-only releases without `latest.json` cannot supply in-app updates.

<h2 id="use-pulse"><img src="assets/icons/terminal.svg" alt="" width="28" height="28" align="center">&nbsp; Use Pulse</h2>

Start with your existing coding tools:

1. Open Claude Code, Codex or OpenCode.
2. Launch Pulse.
3. Select a provider, or choose **All providers** for combined analytics.
4. Open **Discord** to choose what your profile shares.

| View | Purpose |
| --- | --- |
| Home | Active sessions, provider limits, cache ratio and the last seven days of activity |
| Sessions | Live sessions and searchable, filtered history |
| Costs | Provider-billed spend and API-equivalent value, with their provenance |
| Reports | Provider-supported analysis and exports |
| Discord | Broadcast application, presets, field order and the backend-owned preview |
| Settings | Provider, plan display, appearance, window behavior and data controls |

Choosing a provider changes its workspace context. **All providers** combines analytics without changing the last broadcast application. The **Broadcast from** control in Discord makes publication ownership explicit.

A completed OpenCode session stays in history but leaves the live focus and Discord publisher. **Idle** does not borrow the last model, token count or monetary value. Unknown cost is unavailable; a provider-reported zero is displayed as `$0.00`.

<details>
<summary id="claude-code">Claude Code</summary>

Pulse reads your local Claude Code transcripts and optional statusline data. Review active sessions, token usage, prompt-cache efficiency and cost estimates in one workspace.

Claude subscription limits and Anthropic API access are separate sources. Account limits appear only after a successful provider check. Session tokens do not stand in for your remaining allowance.

The default Claude source root is `~/.claude`; `CLAUDE_HOME` can override it. Subscription checks currently read `.credentials.json` from that root. Keychain-only credentials on macOS are not supported by this reader; local session analytics remain available.

See [plan detection](docs/guides/plans.md), [cost calculations](docs/guides/costs.md) and [troubleshooting](docs/guides/troubleshooting.md).

</details>

<details>
<summary id="codex">Codex</summary>

Pulse reads local Codex sessions and runtime metadata. Inspect the model, reasoning effort, context usage and recorded activity without combining them with Claude or OpenCode metrics.

Codex subscription limits and OpenAI API access remain separate. Pulse preserves provider-reported limit windows and marks unavailable costs or limits instead of inventing values.

The Codex integration uses `CODEX_HOME`, or `~/.codex` by default. **All providers** combines analytics without changing the application selected for Discord broadcasting.

See the [Codex model catalog](docs/models/codex.md), [context tracking](docs/guides/context.md) and [core integration](docs/maintainers/codex-core.md).

</details>

<details>
<summary id="opencode-and-go">OpenCode and Go</summary>

Pulse reads `opencode.db` and channel databases from OpenCode's local data directory. It supports both message-table schemas and respects `XDG_DATA_HOME` and `OPENCODE_DB`. Additional databases can be set in `~/.pulse-analytics/pulse-opencode.json`.

When an existing OpenCode Go credential is available, Pulse requests its account usage from the Go API. Pulse does not copy that credential into its own settings or logs. The monthly window uses the provider's reset date rather than an invented fixed duration.

Desktop, CLI and OpenChamber can share this local store. Unknown client attribution remains OpenCode; a managed OpenChamber label requires matching process identity. Remote stores on other machines are outside this local integration. Read the [OpenCode and Astra guide](docs/guides/opencode.md) for the source and absence rules.

</details>

<details>
<summary id="discord-controls">Discord controls</summary>

Open Discord, choose a broadcast application in Pulse, and enable Rich Presence. Use **Minimal**, **Standard** or **Full**, then adjust individual fields. OpenCode defaults to model, activity, project and branch before its numeric fields. Field order controls priority within each Discord line.

Unavailable fields stay disabled. Go quotas identify Go explicitly and appear only while their data is fresh and the field is enabled. The profile preview uses the same Rust compositor as the publisher. The connected user's name and avatar come from local Discord IPC; an unavailable banner is not fabricated.

</details>

<details>
<summary id="notifications">Notifications</summary>

The bell opens the local notification history. You can mark one item or all items as read/unread, dismiss an item, or clear the list. **Clear all** requires confirmation and preserves the records. **Undo** restores the last confirmed clear and its original read states. The last Undo receipt survives an app restart on the same client.

If Pulse cannot refresh the history, it labels the retained snapshot and disables bulk changes until the connection recovers.

</details>

<details>
<summary id="windows-efficiency-mode">Windows Efficiency mode</summary>

Pulse requests EcoQoS and Idle process priority at startup. Windows owns the Task Manager leaf indicator; Pulse does not draw a substitute. Other operating systems keep their normal scheduling behavior.

Set this environment variable before launch to opt out:

```powershell
$env:PULSE_EFFICIENCY_MODE = "0"
```

See [Windows Efficiency mode](docs/maintainers/windows-efficiency.md) for behavior and the read-only verification command.

</details>

<h2 id="your-data-and-network-access"><img src="assets/icons/shield.svg" alt="" width="28" height="28" align="center">&nbsp; Your data and network access</h2>

Pulse stores its own data in `~/.pulse-analytics/`, or under `PULSE_HOME`. Provider credentials and source records stay in their original directories. The first startup copies legacy history and settings without deleting or overwriting them. See [storage and recovery](docs/guides/storage.md).

Network use is limited to the configured provider quota checks, release/update checks and the Discord presence fields you enable. Project, branch and activity controls determine what the presence exposes. Keep private prompts, credentials and local reports out of public issues.

On Windows, WSL session discovery is off by default. Enable it only when you need those transcript roots:

```powershell
$env:CC_PRESENCE_INCLUDE_WSL = "1"
```

<h2 id="common-questions"><img src="assets/icons/info.svg" alt="" width="28" height="28" align="center">&nbsp; Common questions</h2>

### Is Pulse a Claude Code cost tracker or a Codex dashboard?

Both, plus OpenCode. Each provider has its own data source, context rules and cost provenance. The combined view does not merge account allowances or treat subscription credits as API spend.

### Does Pulse need a separate account or API key?

Local session history does not need a Pulse account. Provider quota cards use existing supported credentials and require a successful provider check. Missing credentials do not erase local history. See [plans and access](docs/guides/plans.md).

### Does Pulse upload my prompts?

No. Pulse does not upload your prompts or session transcripts and sends no analytics telemetry. Your session analytics remain local.

Enabled provider quota checks, update checks and Discord presence use the network without uploading transcripts. See [your data and network access](#your-data-and-network-access).

### Are the cost estimates my actual bill?

Not always. Pulse separates provider-reported cost from API-equivalent estimates. Model prices can change, and documented runtime pricing gaps remain. See [cost provenance](docs/guides/costs.md) before using estimates for a budget.

<h2 id="build-from-source"><img src="assets/icons/terminal.svg" alt="" width="28" height="28" align="center">&nbsp; Build from source</h2>

Install Rust, Node.js 22.12+ (22.x), 24.x, or 26+, the Tauri CLI and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). Then run:

```powershell
git clone https://github.com/xt0n1-t3ch/Pulse-Analytics.git
cd Pulse-Analytics
npm --prefix frontend ci
npm run verify
npm run build:portable
```

`build:portable` compiles the frontend and runs `cargo tauri build --no-bundle`. The Tauri CLI enables `tauri/custom-protocol`, so the executable contains its UI. Do not promote a raw Cargo GUI build that still points at the development URL.

Use `npm run build` for installers. Use `npm run dev` for the authenticated, loopback-only browser development bridge. The development bridge is not required by the installed application.

<h2 id="compatibility-and-recovery"><img src="assets/icons/info.svg" alt="" width="28" height="28" align="center">&nbsp; Compatibility and recovery</h2>

Pulse v1.8.2 uses Claude config schema 6, Codex config schema 13 and analytics schema 6. The analytics migration adds OpenCode metadata without discarding previous sessions.

Pulse v1.9.5 keeps Claude config schema 6 and Codex config schema 13, and migrates analytics to schema 7 by adding Accounts and Command Code data. Existing sessions are preserved. Pulse v1.8.2 cannot open analytics schema 7: back up the database before you install 1.9.5, and keep the schema-7 database separately if you return to 1.8.2.

Back up the executable, configuration and database through SQLite Backup before replacing an installation. Pulse 1.7.9 cannot open analytics schema 6: rollback requires the schema-5 backup, while the newer database should be preserved separately.

Pulse consumes `codex-presence-core` 2.0.1 through an immutable Git pin recorded in [UPSTREAM.json](src/codex/UPSTREAM.json), promoted from the upstream v1.11.2 release. The standalone [Codex Discord Rich Presence](https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence) runtime has its own releases and terminal interface.

See the [dependency audit scope](docs/maintainers/dependencies.md) for inherited Tauri advisories and platform boundaries.

<h2 id="contribute-and-report-problems"><img src="assets/icons/git-pull-request.svg" alt="" width="28" height="28" align="center">&nbsp; Contribute and report problems</h2>

Read [CONTRIBUTING.md](CONTRIBUTING.md), the [test map](tests/index.md), [release procedure](docs/maintainers/releases.md) and [Code of Conduct](CODE_OF_CONDUCT.md). Report security issues privately through [GitHub Security Advisories](https://github.com/xt0n1-t3ch/Pulse-Analytics/security/advisories/new).

<h2 id="license"><img src="assets/icons/scale.svg" alt="" width="28" height="28" align="center">&nbsp; License</h2>

[Apache-2.0](LICENSE), copyright 2026 xt0n1-t3ch. Redistributed and derivative versions must preserve the license, copyright notice and [NOTICE](NOTICE) attribution.
