[Pulse](../README.md) / Documentation

# ![](../assets/icons/map.svg) Pulse documentation

Local analytics and Discord Rich Presence for **Claude Code, Codex, OpenCode and Command Code**. Find a task, understand its data, then follow the relevant implementation or release contract.

**Published: Pulse v1.9.0** · **Previous release: v1.8.2** · **Docs refreshed September 19, 2026**

[Start here](#start-here) · [Models and analytics](#models-and-analytics) · [Build and maintain](#build-and-maintain) · [Version](#version)

## Start here

| I want to… | Read |
| --- | --- |
| Install Pulse or check my operating system | [Install](../README.md#install) · [Platform support](maintainers/platforms.md) |
| Connect Claude, Codex or OpenCode | [Use Pulse](../README.md#use-pulse) · [Plans and access](guides/plans.md) |
| Manage multiple provider accounts | [Accounts](guides/accounts.md) |
| Connect Command Code CLI and Desktop | [Command Code](guides/command-code.md) |
| Configure OpenCode sessions and Go limits | [OpenCode and Astra](guides/opencode.md) |
| Choose what Discord shows | [Discord controls](../README.md#discord-controls) · [Assets and application IDs](guides/discord.md) |
| Understand notifications or updates | [Notifications](guides/notifications.md) · [Update checks](guides/updates.md) |
| Diagnose missing data | [Troubleshooting](guides/troubleshooting.md) |
| Locate or recover local data | [Storage and migration](guides/storage.md) |

## Models and analytics

| Guide | What it answers |
| --- | --- |
| [Claude models & context](models/claude.md) | Fable/Mythos 5.1, Opus 5, Sonnet 5 and Haiku: API limits, Claude Code windows, prices and implementation gaps |
| [Codex models & context](models/codex.md) | Astra, Sol, Terra and Luna: local Codex capacity, usable budget, API limits, effort and current pricing |
| [Context tracking](guides/context.md) | Current fill, session totals, compaction and source precedence |
| [Cost calculation](guides/costs.md) | Provider-specific token math, price provenance and unknown values |
| [Analysis and recommendations](architecture/analyzers.md) | Cache health, session analysis and supported recommendations |

> **Model reference is not runtime proof.** The Claude and Codex guides separate today's official limits from the local inventory and the bundled catalog. They also list known pricing and context gaps in the implementation.

## Build and maintain

| Area | Reference |
| --- | --- |
| Architecture | [Component map](architecture/index.md) · [Provider access](architecture/adaptive-access.md) |
| Local development | [Contributing](../CONTRIBUTING.md) · [Authenticated development bridge](architecture/dev-bridge.md) |
| UI and platform behavior | [Design system](architecture/ui.md) · [Windows Efficiency mode](maintainers/windows-efficiency.md) |
| Codex integration | [Shared core and vendoring](maintainers/codex-core.md) · [Model catalog](models/codex.md) |
| Quality and security | [Test map](../tests/index.md) · [Dependency audit](maintainers/dependencies.md) |
| Releases | [Release procedure](maintainers/releases.md) · [Six-platform acceptance](maintainers/platforms.md) |

The immutable v1.9.0 release includes all six platform/architecture targets, checksums and signed updater payloads. Native package checks passed; installed-GUI acceptance is recorded separately per operating system. See [current platform status](maintainers/platforms.md#current-release-status).

Version **v1.9.0** adds Accounts, the Command Code provider, native allowances and promoted Codex per-event cost. It is published through the six-platform Release workflow with signed updater artifacts. GitHub macOS packages carry no Apple Developer ID signature or notarization.

## Version

- Latest published app: **v1.9.0**
- Version owners in this checkout: **v1.9.0**
- Shared core: **2.0.1**, immutable v1.11.2 pin
- Schema: **Claude config v6 / Codex config v13 / Pulse analytics DB v6 in 1.8.2; v7 in 1.9.0**
- Version owners: [release contract](../scripts/release-contract.json) and [upstream manifest](../src/codex/UPSTREAM.json).
- Windows WSL discovery is opt-in through `CC_PRESENCE_INCLUDE_WSL=1`.

## History and project links

[Changelog](../CHANGELOG.md) · [License](../LICENSE) · [Report an issue](https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/issues)
