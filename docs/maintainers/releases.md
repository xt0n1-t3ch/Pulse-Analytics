# Release contract

Pulse releases use annotated tags, exact commits and immutable assets. The normal release path is the manual six-platform Release workflow. The local Windows script is a limited recovery path, not the cross-platform release process.

## Version surfaces

`scripts/release-contract.json` owns the product, core, configuration and database contract. Tags must agree with Cargo, npm, lockfiles, Tauri, README, the docs index, changelog and `src/codex/UPSTREAM.json`.

Pulse 1.9.0 consumes `codex-presence-core` 2.0.1 through the full Git revision recorded in `src/codex/UPSTREAM.json`, promoted from the upstream v1.11.2 release at commit `0caece60a71eb657d7b829e3b6c7c4896a0a536c`. Path dependencies and mismatched pins fail the release contract. The published v1.8.2 release used core 2.0.0 from upstream v1.10.3.

## Commit and pull request checks

Before an authorized commit, run the focused tests for the change. Before an authorized push or release, run `npm run verify` and `npm --prefix frontend run build`.

Changes to OS-specific code, dependencies, installers, signing or release tooling also require the platform contract tests:

```powershell
cargo test --locked --test release_scripts
```

These tests run in the regular workspace suite. They check native runner coverage, publication opt-in, required packages, updater signatures and the Windows-only recovery boundary. They do not replace native runtime tests.

Keep changes under `Unreleased` until a release is requested. Record compatibility and release impact. Keep `AGENTS.md`, this guide, the [platform support guide](platforms.md) and the [test map](../../tests/index.md) aligned. Do not claim a platform passed if no native evidence exists.

There is no automatic push, pull request, tag or scheduled CI. `release.yml` and `upstream-freshness.yml` remain manual-only.

## Cross-platform verification

After the release tag is explicitly authorized and pushed, dispatch Release with that tag and `publish_release=false`. This is the default. It verifies the exact annotated tag, builds the frontend once, runs Clippy/tests natively on all six targets and retains unsigned verification packages without publishing.

The installed GUI must also pass the [native runtime checklist](platforms.md#native-runtime-acceptance). Complete release claims require evidence, not only a configured matrix.

## Publish a complete release

Only after explicit publication authorization:

1. Bump every version surface and prepare the reviewed changelog section.
2. Run local verification. Record completed native runtime checks and any unavailable hosts; do not infer installed-runtime acceptance from a compiled package.
3. Create and push the authorized annotated tag from the reviewed commit.
4. Configure the required updater signing secret. GitHub macOS packages do not require Apple credentials.
5. Dispatch Release with the tag and `publish_release=true`.
6. Require all six native jobs, the complete package set, six updater entries and checksums.
7. Verify the draft's downloaded bytes and GitHub digests before making it public and immutable.

The workflow blocks publication if any required target, updater signature or checksum check fails. macOS packages are distributed without Apple Developer ID signing or notarization; state this limit in the release notes. It never updates an existing immutable release. A correction requires a new patch version.

## Windows-only recovery

Use this path only when a Windows x64-only recovery release is explicitly requested:

```powershell
pwsh -File scripts/release-local.ps1 -WindowsOnlyRecovery
```

`-SkipBuild` can reuse exact-version installers; `-Draft` retains the verified draft. The script requires a Windows x64 host and an explicit recovery flag before remote operations. It publishes four Windows assets without an updater manifest and always sets `--latest=false`.

A recovery release does not satisfy the six-platform contract. The already-published v1.8.1 Windows-only release remains unchanged.

## Local portable validation

Run `npm run build:portable` to embed the frontend through Tauri's custom protocol. A raw Cargo GUI build can still point at the development URL. Validate the actual application window with development listeners stopped, not only the process or Discord connection.

A local installation is a development check, not a release step. If the host has no updater signing key, build the local installer with a scratch Tauri configuration that sets `createUpdaterArtifacts=false`, and keep the checked-in `src-tauri/tauri.conf.json` unchanged. The resulting package has no updater artifact and no signature, so it cannot satisfy the publication gate. Record the previous executable path, file version and SHA-256 before replacement.

## Storage upgrade acceptance

For 1.8.2, verify the provider-neutral storage migration before promotion. Keep legacy files, check a WAL database with FTS5, confirm history and preferences in the real consumer, and verify that test runs do not write to the user data directory. Follow [storage and recovery](../guides/storage.md).

## Unreleased Accounts and Command Code

Accounts and the fourth provider are compatible feature additions. Their additive SQLite migration is schema 7. Every version owner in this checkout now reads 1.9.0, which is the prepared minor version. This is a source-tree preparation, not a replacement for the published 1.8.2 release. Keep the pre-v7 backup for rollback.

New cost-compositor behavior currently uses an explicit local canonical-core override during validation. A release must first promote an authorized immutable core containing that behavior. Do not publish the local path override or reuse an existing release tag.

### Remaining gates for a 1.9.0 release

A local Windows build or installation does not satisfy any of these gates:

1. Promote the canonical compositor work in Codex-Discord-Rich-Presence to an authorized immutable release, then update the core version, `rev`, `canonical_release` and `canonical_commit` in `src/codex/UPSTREAM.json`, `scripts/release-contract.json` and both Cargo manifests. Until then the pinned build does not contain the new reserved-cost compositor.
2. Move the reviewed `Unreleased` entries into a `## [1.9.0] - <date>` changelog section. `scripts/check-release-contract.ps1` requires that section; with the current preparation it stops at `CHANGELOG.md has no section for 1.9.0`, which is the expected state while the work stays under `Unreleased`. All other version surfaces and documentation surfaces already pass that script.
3. Obtain explicit authorization for the commit, the push, and the annotated `v1.9.0` tag on the reviewed commit, which must be reachable from `origin/main`.
4. Dispatch Release with `publish_release=false` and pass all six native targets.
5. Complete the [native runtime checklist](platforms.md#native-runtime-acceptance) on real hosts, including the macOS and Linux account flows that remain unverified.
6. Configure the updater signing secret, then dispatch with `publish_release=true` and verify six updater entries, the required installers, checksums and the Windows SPDX files.
