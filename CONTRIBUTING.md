# Contributing to Pulse

Pulse is an open-source project under the Apache License 2.0. Contributions of any size are welcome, from typo fixes to new analyzers. Thank you for helping make it better.

## Code of conduct

Be respectful, assume good intent, keep discussions on-topic. Personal attacks, harassment, and discriminatory language are not tolerated. See the full [Code of Conduct](CODE_OF_CONDUCT.md). Maintainers reserve the right to remove off-topic or abusive content from issues, pull requests, and discussions.

## Reporting bugs

Open an issue at [github.com/xt0n1-t3ch/Pulse-Analytics/issues](https://github.com/xt0n1-t3ch/Pulse-Analytics/issues) and include:

- Pulse version (visible in Settings).
- Operating system and version.
- Which provider you use (Claude Code, Codex, or both).
- Steps to reproduce.
- Expected vs actual behavior.
- Relevant log lines (run with `RUST_LOG=debug` — see [docs/troubleshooting.md](docs/guides/troubleshooting.md)).

Reproduction steps are the single biggest factor in turnaround time. The issue templates guide you through the fields.

## Suggesting features

Open an issue with the feature template. Describe the user problem first, the proposed solution second. Mock-ups or screenshots help. Keep one feature per issue.

## Pull requests

1. Fork the repository and create a topic branch off `main` (e.g. `feat/linux-tray`).
2. Keep the change focused. One PR, one concern.
3. Run the quality gates locally (see below). CI runs the same gates and must pass before review.
4. Open the PR against `main`. Link the relevant issue and fill in the PR template.
5. Address review feedback by pushing additional commits to the same branch; the maintainer squashes on merge.

## Commit conventions

Conventional Commits are enforced and feed the changelog and release notes ([cliff.toml](cliff.toml)):

| Prefix | Use for |
|---|---|
| `feat:` | A new user-visible capability. |
| `fix:` | A bug fix. |
| `perf:` | A measurable performance improvement. |
| `refactor:` | A code change that does not alter behavior. |
| `docs:` | Documentation only. |
| `test:` | Tests only. |
| `chore:` | Build, tooling, or housekeeping. |

Keep the subject line under 72 characters, lowercase, imperative. Use the body for context, not a restatement of the diff.

## Coding standards

- Match the existing patterns of the file you are editing.
- Centralize constants, paths, and shared types rather than duplicating across crates.
- Prefer explicit error handling over `unwrap()` outside of tests.
- Reserve comments for external constraints (license headers, upstream-bug workarounds with links, lint suppressions). Names and types carry the rest.
- Rust passes `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
- TypeScript / Svelte passes `svelte-check` with zero errors and zero warnings.

## Dev setup

Prerequisites: Rust (stable, pinned by [rust-toolchain.toml](rust-toolchain.toml)), Node.js 22.12+ (22.x), 24.x, or 26+, and the platform Tauri dependencies — see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```bash
# frontend dev server
cd frontend && npm install && npm run dev

# in another terminal: the Tauri app (connects to the dev server)
cd src-tauri && cargo tauri dev
```

## Testing

Add or update tests alongside any behavior change. Tests are centralized: frontend specs live under `tests/{unit,integration,components}` (Vitest, config in [frontend/vitest.config.ts](frontend/vitest.config.ts)); Rust integration tests live under `tests/` and `src-tauri/tests/`; Rust unit tests stay inline as `#[cfg(test)]`. See [tests/index.md](tests/index.md).

The full validator chain — all must pass with zero warnings:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix frontend run check
npm --prefix frontend run test
npm --prefix frontend run build
```

## Release process

Releases are tag-driven. Maintainers bump `version` together in `Cargo.toml`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `frontend/package.json`, `frontend/package-lock.json`, the root `package.json`, and `scripts/release-contract.json`; add a `CHANGELOG.md` section; commit `chore(release): vX.Y.Z`; then push an annotated `vX.Y.Z` tag only after explicit promotion approval.

The canonical `codex-presence-core` dependency must be a Git dependency pinned with a full 40-character `rev`, never a release-time `path` dependency or moving tag/branch. Its version and SHA must match the canonical manifest exactly. The Release workflow rejects drift, validates config and database migrations, builds Windows/macOS/Linux bundles, validates the Windows SPDX SBOM, generates SHA-256 checksums, composes curated release notes, and publishes an immutable release.

Before promotion, attach Dark/Light native screenshots, real Discord proof where affected, schema-migration results, and measured performance. Local development stops before tag, push, PR, or release creation until the maintainer approves.

Pre-release tags (`vX.Y.Z-rc.N`) follow the same flow and are marked as pre-release.

## License

By submitting a contribution, you agree that it will be licensed under the project's Apache License 2.0. See [`LICENSE`](LICENSE) and the attribution policy in [`NOTICE`](NOTICE).
