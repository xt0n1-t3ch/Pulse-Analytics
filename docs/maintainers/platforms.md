# Platform support and verification

Pulse targets Windows, macOS and Linux on x64 and ARM64. A configured target is not proof of a tested application or a published download.

## Current release status

On September 20, 2026, the immutable [v1.9.2 release](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/tag/v1.9.2) published all six targets from commit `163be68387ed1a431132dfad7acb3c15032eb26b`. The [release run](https://github.com/xt0n1-t3ch/Pulse-Analytics/actions/runs/35523134676) passed preflight, all six native jobs and publication. The earlier v1.9.0 candidate was not published; its tag remains unchanged.

The release contains 24 assets, including 23 checksum entries, two Windows SPDX files and six signed updater payloads in `latest.json`. Downloaded bytes match both the checksum manifest and GitHub asset digests. All six updater signatures verify against the bundled public key; modified payload controls are rejected. The following packages are published:

| Platform | Native runner | Rust target | Required public packages |
| --- | --- | --- | --- |
| Windows x64 | windows-2022 | x86_64-pc-windows-msvc | NSIS, MSI, updater signature, SPDX |
| Windows ARM64 | windows-11-arm | aarch64-pc-windows-msvc | NSIS, MSI, updater signature, SPDX |
| macOS Intel | macos-15-intel | x86_64-apple-darwin | DMG, app archive, updater signature |
| macOS Apple Silicon | macos-latest | aarch64-apple-darwin | DMG, app archive, updater signature |
| Linux x64 | ubuntu-22.04 | x86_64-unknown-linux-gnu | DEB, RPM, AppImage, updater signature |
| Linux ARM64 | ubuntu-22.04-arm | aarch64-unknown-linux-gnu | DEB, RPM, AppImage, updater signature |

The workflow verifies the Rust host target before executing tests. It builds the frontend once and uses that artifact in every native bundle. Native jobs run warning-denying Clippy and workspace tests before packaging.

These checks prove native tests and packaging, not the complete installed-GUI checklist below. Real-data migration and preference persistence were also checked through the Windows Rust bridge and frontend. Full installed-GUI acceptance on all six targets remains unverified.

During the previous 1.8.2 release, GitHub emitted an upstream runner notice that `windows-11-arm` will switch its default Visual Studio image on September 21, 2026. It did not fail the build. Recheck the runner toolchain for subsequent releases; do not change published 1.8.2 assets.

## Verify without publishing

After an explicitly authorized annotated tag exists on the remote, dispatch Release with `publish_release=false`, the default. It runs the native matrix and retains verification artifacts under the repository retention policy. It does not create a GitHub release, and its unsigned packages are not updater candidates.

Local regression checks run with `cargo test --locked --test release_scripts`. They are also part of `npm run verify`. These fixture and workflow checks do not execute Linux or macOS binaries on Windows.

## Public release gate

Publishing requires an explicit dispatch with `publish_release=true`, all six successful jobs, complete artifacts, checksums and six signed updater entries. Missing updater signatures fail assembly. Apple credentials are not required for GitHub packages.

Configure `TAURI_SIGNING_PRIVATE_KEY` without printing its value; add `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if the key has a password.

The macOS artifacts are GitHub downloads, not App Store submissions. They have no Apple Developer ID signature or notarization. The workflow checks the packaged executable architecture. macOS may block first launch. Do not claim Gatekeeper acceptance or notarization. Updater signatures prove the update payload, not an Apple-approved publisher. Windows downloads may also trigger SmartScreen when publisher signing or reputation is unavailable.

Only the complete release can become latest. `scripts/release-local.ps1 -WindowsOnlyRecovery` is an explicitly limited Windows x64 recovery path. It always uses `--latest=false` and cannot repair an immutable release.

## Native runtime acceptance

Before claiming support for a release, retain evidence from the installed package on each target:

1. Install the architecture-matched package and launch it with development ports 1420 and 1421 stopped.
2. Check the embedded UI, version, light/dark themes, resize and all primary views.
3. Check single-instance behavior, close-to-tray, restore, notifications and persisted settings.
4. Test Claude, Codex and OpenCode with available local sessions. Separate live activity, history and account limits; verify unavailable sources remain unavailable.
5. Connect Discord and compare published fields with the Pulse preview. Verify idle and completed sessions do not retain stale live data.
6. Test an upgrade from the previous supported version, database migration, rollback backup and the signed updater path.

Record the OS version, architecture, tag commit, installer SHA-256, result and known limits. A missing host or credential is a gap, not a pass. Do not enable fixture-backed behavior in production to obtain a screenshot.

## Known platform differences

- Windows uses WebView2. EcoQoS and Efficiency mode apply only to Windows.
- macOS has a configured minimum of 11.0. The current Claude subscription reader reads `.credentials.json`; it does not read Keychain-only credentials. Local transcript analytics do not require that quota credential.
- Linux uses GTK 3 and WebKitGTK 4.1. Tray availability depends on AppIndicator support and the desktop environment. AppImage does not guarantee compatibility with every Linux distribution.
- Provider sign-in, quota access and cost coverage are separate from operating-system support. An installed client or configured credential is not proof of authenticated quota access.

## Sources

- [GitHub-hosted runner labels](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [Tauri macOS signing and notarization](https://v2.tauri.app/distribute/sign/macos/)
- [Release workflow](../../.github/workflows/release.yml), [release contract](../../scripts/release-contract.json), [Tauri configuration](../../src-tauri/tauri.conf.json)

## Account and Command Code acceptance in 1.9.2

The Windows development build rendered real Accounts data through native Tauri IPC with the browser proxy unused. The native folder-picker affordance was visible. An isolated Command Code key connection passed authentication, same-ID reconnection, protected-profile access checks and removal without changing the source credential.

The shared reader accepted real CLI and Desktop history without duplicate IDs. Local Discord acknowledged the requested Command Code name and published asset ID, then acknowledged the diagnostic clear. The 1.9.2 production publisher also received a real acknowledgement for idle activity without a session timestamp. The user confirmed its name and artwork in the actual Discord client. Those observations came from development validation. Installed-package evidence follows below.

v1.9.2 is published through the manual six-platform Release workflow. Publication requires native Clippy and tests on each of the six runners, the complete installer set per platform, both Windows SPDX documents, `SHA256SUMS.txt`, and `latest.json` with a signed updater payload for `windows-x86_64`, `windows-aarch64`, `darwin-aarch64`, `darwin-x86_64`, `linux-x86_64` and `linux-aarch64`. The workflow refuses to publish when any of those checks fails, and it never rewrites an existing immutable release.

Native job success is build and test evidence, not installed-application evidence. Windows x64 was upgraded from the existing 1.9.1 installation using the downloaded v1.9.2 NSIS installer, after a consistent SQLite and configuration backup. The installed executable matches the payload extracted from that signed-updater installer; Windows uninstall registration reports 1.9.2.

- Installer SHA-256: `d61ba33ff0b80545be2517c416b0c32ff102cccc9ae30d98665e0cf0186b55ce`.
- Installed executable SHA-256: `e4fb26b27a6c080e4b158d91006995203cc6cc872f63e63c94c502eb4786a102`.
- Native test: embedded `tauri.localhost` UI, real Tauri IPC, every primary route, zero development-proxy requests and zero page errors.
- Upgrade: SQLite integrity check passed at schema 7; all 1708 pre-upgrade session IDs remained present. The prior schema-6 backup and executable are retained locally for rollback.
- Real account snapshots and Command Code five-hour, weekly and monthly windows were checked through the installed consumer. The normal app was reopened without the temporary debugging listener after acceptance.
- The installed 1.9.2 consumer passed the Command Code idle label, keyboard toggle-off/Disabled and toggle-on/Connected checks. Its updater reported the canonical 1.9.2 release with 24 assets and no pending update. Source credential fingerprints remained unchanged.

Installed macOS and Linux runtime and fresh browser OAuth completion for every provider remain open acceptance gaps. Command Code idle rendering was confirmed in the real Discord client during local validation, and the production publisher received the correct application and asset acknowledgement. GitHub macOS packages have no Apple Developer ID signature or notarization.

A local development installation may use a scratch Tauri configuration with `createUpdaterArtifacts=false` when no updater signing key exists on the host. That path produces no updater artifact and no signature, so it cannot satisfy the publication gate, and it leaves the checked-in `src-tauri/tauri.conf.json` unchanged. Before you replace an installed executable, record its path, file version and SHA-256, and keep the previous package for rollback.
