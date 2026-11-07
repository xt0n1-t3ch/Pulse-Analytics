# Release contract

Pulse releases use annotated tags, exact commits and immutable assets. The normal release path is the manual six-platform Release workflow. The local Windows script is a limited recovery path, not the cross-platform release process.

## Version surfaces

`scripts/release-contract.json` owns the product, core, configuration and database contract. Tags must agree with the Cargo manifests, `Cargo.lock`, both `package.json` files, Tauri, README, the docs index, the changelog and `src/codex/UPSTREAM.json`. `cargo xtask release-contract` checks all of them against the tag.

Pulse 2.0.0 consumes `codex-presence-core` 2.0.1 through the full Git revision recorded in `src/codex/UPSTREAM.json`, promoted from the upstream v1.11.3 release at commit `77d5dd637d33f6876a63cc8760fdc0a1a86ba4f8`. Path dependencies and mismatched pins fail the release contract. The published v1.8.2 release used core 2.0.0 from upstream v1.10.3.

## Commit and pull request checks

Before an authorized commit, run the focused tests for the change. Before an authorized push or release, run the full gate and the frontend build:

```sh
cargo xtask check
bun run build:frontend
```

`cargo xtask check` runs `cargo fmt --check`, the Bun format check, oxlint, the type check, Clippy with `-D warnings` for the host and for the Windows target (through `cargo xwin` on Linux and macOS), the Bun tests, `cargo nextest run --workspace`, the changelog check, `llms --check`, the checkout release contract, the vendored-core check, the RustSec audit (needs `cargo-audit` 0.22.2 and network) and the model catalog parity check, and prints one line per step.

Changes to OS-specific code, dependencies, installers, signing or release tooling also require the release-tool tests:

```sh
cargo nextest run -p xtask
```

These tests run in the regular workspace suite on every OS, with no PowerShell. They check native runner coverage, publication opt-in, required packages, updater signatures, the vendoring contract, the release contract and the Windows-only recovery boundary. They do not replace native runtime tests.

Keep changes under `Unreleased` until a release is requested. Record compatibility and release impact. Keep `AGENTS.md`, this guide, the [platform support guide](platforms.md) and the [test map](../../tests/index.md) aligned. Do not claim a platform passed if no native evidence exists.

There is no automatic push, pull request, tag or scheduled CI. `release.yml` and `upstream-freshness.yml` remain manual-only.

## Release commands

Every release step that is more than git, Bun or Cargo is a `cargo xtask` command. The workflows and the tests call the same code.

| Command | Purpose |
| --- | --- |
| `cargo xtask release-contract --tag <tag> --expected-commit <sha>` | Strict SemVer tag, annotated and reachable from `origin/main`, every version surface, core pin, schema versions |
| `cargo xtask upstream check` | Verifies `src/codex/UPSTREAM.json` against Cargo and the vendoring contract |
| `cargo xtask upstream update --tag <tag> --commit <sha>` | Re-vendors the Codex mirror from an annotated upstream tag |
| `cargo xtask upstream freshness` | Opens, updates or closes the drift issue when the core pin trails upstream `main` (needs `gh`) |
| `cargo xtask audit` | `cargo audit --deny warnings` with the reviewed exceptions in `scripts/rustsec-accepted-warnings.json` |
| `cargo xtask platform-assets` | Collects one platform's installers under `pulse-<platform>-` names |
| `cargo xtask release-assets` | Assembles the 22 package files, `latest.json` with six signed updater entries, and `SHA256SUMS.txt` |
| `cargo xtask sbom ci`, `sbom new`, `sbom check` | Windows SPDX 2.3 SBOM generation and validation |
| `cargo xtask bundle-args`, `verify-native` | Tauri bundle arguments and the native Clippy and test gate of a build job |
| `cargo xtask changelog check` | Keep a Changelog 2.0.0 format, SemVer order, compare links and tags |
| `cargo xtask changelog notes <version>` | The release body: that version's changelog section, verbatim |
| `cargo xtask llms [--check]` | Writes or checks `llms.txt` and `llms-full.txt` from `docs/llms.toml` |
| `cargo xtask meta --check` / `--apply` | Compares or applies the GitHub description, homepage and topics in `.github/repository.toml` |
| `cargo xtask catalog-parity` | Byte parity with the canonical Codex model catalog and the Astra, Sol and Terra contracts |
| `cargo xtask windows-efficiency --pid <pid>` | Reads EcoQoS and priority state of a running process (Windows only) |

`cargo xtask check` also runs the release preflight's file checks on every call: the checkout part of `release-contract` (version owners agree, the core pin is an immutable Git revision, followed through `[workspace.dependencies]`, and once the changelog has a section for the version, README and `docs/index.md` name it) and `upstream check`. A tag cut from a checkout that passes the gate does not stop at preflight on GitHub.

Release notes use the title `Pulse <version>` and the changelog section as the body, with no H1 or H2, asset list, integrity section or compare link. Only a section longer than 150 lines gets the Release assets, Integrity and Full changelog sections appended. `meta --apply` edits the public repository page, so run it on purpose, like a push. The catalog parity command compares with the sibling `Codex-Discord-Rich-Presence` folder, or the folder named by `--upstream-root`; when the default folder is absent it skips only the byte comparison and says so.

## Cross-platform verification

After the release tag is explicitly authorized and pushed, dispatch Release with that tag and `publish_release=false`. This is the default. It verifies the exact annotated tag, builds the frontend once, runs Clippy/tests natively on all six targets and retains unsigned verification packages without publishing.

The installed GUI must also pass the [native runtime checklist](platforms.md#native-runtime-acceptance). Complete release claims require evidence, not only a configured matrix.

## Publish a complete release

Only after explicit publication authorization:

1. Bump every version surface and prepare the reviewed changelog section.
2. Run `cargo xtask check`. Record completed native runtime checks and any unavailable hosts; do not infer installed-runtime acceptance from a compiled package.
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

This is the only PowerShell release script left; it calls `cargo xtask` for every check and asset step.

`-SkipBuild` can reuse exact-version installers; `-Draft` retains the verified draft. The script requires a Windows x64 host and an explicit recovery flag before remote operations. It publishes four Windows assets without an updater manifest and always sets `--latest=false`.

A recovery release does not satisfy the six-platform contract. The already-published v1.8.1 Windows-only release remains unchanged.

## Local portable validation

Run `bun run build:portable` to embed the frontend through Tauri's custom protocol. A raw Cargo GUI build can still point at the development URL. Validate the actual application window with development listeners stopped, not only the process or Discord connection.

A local installation is a development check, not a release step. If the host has no updater signing key, build the local installer with a scratch Tauri configuration that sets `createUpdaterArtifacts=false`, and keep the checked-in `src-tauri/tauri.conf.json` unchanged. The resulting package has no updater artifact and no signature, so it cannot satisfy the publication gate. Record the previous executable path, file version and SHA-256 before replacement.

## Storage upgrade acceptance

For 1.8.2, verify the provider-neutral storage migration before promotion. Keep legacy files, check a WAL database with FTS5, confirm history and preferences in the real consumer, and verify that test runs do not write to the user data directory. Follow [storage and recovery](../guides/storage.md).

## Accounts and Command Code upgrade

Accounts and the fourth provider are compatible additions in 1.9.1. Their additive SQLite migration is schema 7. Before replacing an installed package, close that exact Pulse instance and take a consistent SQLite backup, including committed WAL data. Retain its configuration and executable. Pulse 1.8.2 cannot open the newer database: rollback requires the pre-v7 backup and the previous package together.

The published canonical v1.11.2 core contains per-event Codex cost accumulation and reserved monetary-field space. Pulse pins core 2.0.1 at the immutable revision listed above. The release must use that pin, not the local source override used during earlier development.

Keep package publication and installed-runtime acceptance separate. Record the downloaded installer hash, installed executable hash, embedded UI origin, database migration and real native provider results. See the [platform acceptance record](platforms.md#account-and-command-code-acceptance-in-192) for completed checks and remaining gaps. Never move an existing published tag to add later evidence or documentation.
