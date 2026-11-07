[Pulse](../README.md) / Documentation

# Pulse documentation

Local-first analytics and Discord Rich Presence for Claude Code, OpenAI Codex, OpenCode, Command Code and Orion App. Start with a task, then follow its data and implementation owners.

**State on 2026-11-07:** the checkout version is 2.0.0 and shared Codex core is 2.0.1. Planned changes stay under [Unreleased](../CHANGELOG.md#unreleased); documentation is not proof of publication or installed native behavior. Analytics uses schema 7, Claude configuration schema 6 and Codex configuration schema 13.

[Start here](#start-here) · [Models and analytics](#models-and-analytics) · [How it works](#how-it-works) · [Maintain it](#maintain-it) · [Rules](#rules)

## Start here

| I want to... | Read |
| --- | --- |
| Install Pulse on Windows, macOS or Linux | [Installation](../README.md#installation) · [Platform support](maintainers/platforms.md) |
| Use existing provider sessions | [Supported providers](../README.md#supported-providers) · [Plans and access](guides/plans.md) |
| Manage multiple accounts without changing agent logins | [Accounts](guides/accounts.md) |
| Read Command Code CLI and Desktop sessions | [Command Code](guides/command-code.md) |
| Read Orion App's current or legacy store | [Orion App](guides/orion.md) |
| Configure OpenCode sessions and Go limits | [OpenCode](guides/opencode.md) |
| Choose what Discord shows | [Discord assets and controls](guides/discord.md) |
| Understand notifications or updates | [Notifications](guides/notifications.md) · [Updates](guides/updates.md) |
| Diagnose missing data | [Troubleshooting](guides/troubleshooting.md) |
| Locate, back up or recover local data | [Storage and migration](guides/storage.md) |

## Models and analytics

| I want to... | Read |
| --- | --- |
| Check Claude model identity, context and rates | [Claude reference](models/claude.md) |
| Separate Codex capacity from API limits | [Codex reference](models/codex.md) · [Dated context snapshot](models/data/codex-context-snapshot.json) |
| Understand current context fill and compaction | [Context tracking](guides/context.md) |
| Understand token arithmetic and cost provenance | [Costs](guides/costs.md) |
| Understand provider-supported recommendations | [Analyzers](architecture/analyzers.md) |

Official API specifications, observed client context and bundled fallback values are different sources. A missing price or context value remains unavailable, not zero. An API-equivalent estimate is not a provider invoice.

## How it works

| I want to... | Read |
| --- | --- |
| Find a component or data owner | [Architecture](architecture/index.md) |
| Understand provider proof and freshness | [Adaptive access](architecture/adaptive-access.md) |
| Develop in a browser with real Rust state | [Authenticated development bridge](architecture/dev-bridge.md) |
| Understand UI tokens and interaction rules | [UI contract](architecture/ui.md) · [Design system](../DESIGN.md) |
| Understand product scope and audience | [Product context](../PRODUCT.md) |
| Review accepted historical visual evidence | [Preview receipt](evidence/preview-v1.8.2-2026-09-07.md) · [Historical visual QA](evidence/visual-qa-v1.7.0.md) |

## Maintain it

| I want to... | Read |
| --- | --- |
| Contribute and run the local gate | [Contributing](../CONTRIBUTING.md) · [Test map](../tests/index.md) |
| Maintain the shared Codex boundary | [Core integration](maintainers/codex-core.md) · [Upstream manifest](../src/codex/UPSTREAM.json) |
| Review dependencies and security exceptions | [Dependency audit](maintainers/dependencies.md) |
| Prepare a release without publishing by accident | [Release procedure](maintainers/releases.md) · [Platform acceptance](maintainers/platforms.md) |
| Understand Windows scheduling behavior | [Windows Efficiency mode](maintainers/windows-efficiency.md) |
| Check version and schema history | [Changelog](../CHANGELOG.md) · [Release contract](../scripts/release-contract.json) |

## Rules

| I want to... | Read |
| --- | --- |
| Report a vulnerability privately | [Security policy](../SECURITY.md) |
| Understand community expectations | [Code of Conduct](../CODE_OF_CONDUCT.md) |
| Reuse or cite Pulse | [License](../LICENSE) · [Attribution](../NOTICE) · [Citation](../CITATION.cff) |
| Read the generated documentation map | [llms.txt](../llms.txt) · [Generator source](llms.toml) |

GitHub macOS packages are not Apple Developer ID signed or notarized. Build checks, provider authentication, Discord acknowledgement, visual rendering and installed-window acceptance are separate evidence. Windows WSL discovery remains opt-in with `CC_PRESENCE_INCLUDE_WSL=1`.
