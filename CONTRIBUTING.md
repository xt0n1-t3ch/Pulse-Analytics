# Contributing to Pulse

Contributions to local-first analytics and Discord Rich Presence are welcome. Read the [documentation index](docs/index.md), [product context](PRODUCT.md), [design contract](DESIGN.md) and [Code of Conduct](CODE_OF_CONDUCT.md).

## Report a problem

Open an [issue](https://github.com/xt0n1-t3ch/Pulse-Analytics/issues) with the Pulse version, operating system and architecture, affected provider, reproduction steps, expected behavior and redacted diagnostics. Name Claude Code, Codex, OpenCode, Command Code or Orion App explicitly; they have independent data sources. Keep prompts, account identifiers, credentials and local reports private. Report vulnerabilities through the [security policy](SECURITY.md).

For features, explain the user problem before proposing a solution. Keep one concern per issue or pull request.

## Development

Install Rust 1.99.1, Bun 1.4.4, cargo-nextest and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). Linux development uses clang and mold; Windows uses rust-lld. Run from the repository root:

```bash
bun install
bun run dev
```

The browser launcher uses the authenticated [Rust bridge](docs/architecture/dev-bridge.md) and real backend state. Fixtures are test-only. Use an isolated `PULSE_HOME` and a consistent database backup for data changes; never modify provider source records or logins.

| Purpose | Command |
| --- | --- |
| Native development window | `bun run dev:tauri` |
| Type check | `bun run typecheck` |
| Lint | `bun run lint` |
| Format / check format | `bun run fmt` / `bun run fmt:check` |
| JavaScript tests | `bun run test` |
| Rust tests | `cargo nextest run --workspace` |
| Release tooling tests | `cargo nextest run -p xtask` |
| Full local gate | `cargo xtask check` (also `bun run verify`) |
| Portable build / installers | `bun run build:portable` / `bun run build` |
| Generate / check llms files | `cargo xtask llms` / `cargo xtask llms --check` |
| Changelog validation | `cargo xtask changelog check` |
| Extract release notes | `cargo xtask changelog notes <version>` |
| Codex catalog parity | `cargo xtask catalog-parity` |
| Check repository metadata | `cargo xtask meta --check` |

The [test map](tests/index.md) explains each proof boundary. Add the cheapest regression test that exercises changed behavior, with expected values from the source contract rather than the implementation under test. Mocked tests do not prove account authentication, Discord IPC or an installed WebView.

## Pull requests

1. Create a topic branch from `main` in your fork.
2. Keep the change focused and preserve existing work.
3. Update tests and documentation for changed contracts.
4. Run focused tests, then the local gate. Record commands and results.
5. Open against `main`, explaining compatibility, risk and rollback for data or native changes.

Workflows are manual-only. A push or pull request does not automatically run validation or authorize publication.

## Commits and versions

Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/): `feat:` for capabilities, `fix:` for corrections, `perf:` for performance, `docs:` for documentation, `refactor:` for internal restructuring and `test:` for coverage. Identify breaking changes with `!` and a `BREAKING CHANGE:` footer.

Pulse follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) and [Keep a Changelog 2.0.0](https://keepachangelog.com/en/2.0.0/). Write user-visible changes under `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md), using Added, Changed, Deprecated, Removed, Fixed or Security. [git-cliff](cliff.toml) drafts Unreleased text only; maintainers curate summaries and compatibility notes.

## Releases and security

Follow the [release procedure](docs/maintainers/releases.md) and [platform gate](docs/maintainers/platforms.md). Version owners, checksums, updater signatures and native runtime evidence must agree. Local builds are not releases; published tags and assets are immutable. Metadata application, commits, tags, dispatch and publication are explicit maintainer operations.

Keep private agent instructions and tooling state untracked. Do not contribute credentials, desktop settings, home-folder copies or private captures. Match existing source patterns, centralize shared facts and handle errors explicitly.

## License

Contributions are distributed under [Apache-2.0](LICENSE). Preserve copyright notices and [NOTICE](NOTICE) attribution.
