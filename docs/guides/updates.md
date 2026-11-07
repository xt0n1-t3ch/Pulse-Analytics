# Pulse update checks

Pulse checks GitHub Releases from the Tauri backend and shows a small in-app popup when a newer stable release is available.

## Runtime flow

1. `check_app_update()` calls the GitHub latest-release API for `xt0n1-t3ch/Pulse-Analytics`.
2. The backend compares the release tag against `env!("CARGO_PKG_VERSION")`.
3. Drafts and prereleases are ignored.
4. `UpdateBanner.svelte` renders `New Update Available` with current version, latest version, release title, release notes, and actions.
5. Before rendering an install action, the banner asks Tauri's updater for a signed update that supports the current platform.
6. When that preflight succeeds, **Update** downloads and installs through the same verified handle, then the process plugin relaunches Pulse.
7. When updater metadata is missing, invalid, or unavailable for the platform, the banner offers **Open release** instead of an install action.
8. When the response has `managed_by_package_manager: true`, the install action is never offered. See [Package-manager installs](#package-manager-installs-and-appimage).

The popup checks at startup and then every 6 hours. Settings exposes a manual **Check for updates** action by dispatching `pulse:check-updates`.

## User controls

- **Later** hides the current popup until the next check.
- **Skip version** stores the latest version in `localStorage` and suppresses that release during automatic checks.
- **Update** appears only for a preflighted signed update and is the approval checkpoint for download, install, and relaunch.
- **Open release** appears when in-app installation is unavailable and opens the allowlisted GitHub release page.

## Signed updater boundary

Release discovery is informational; the Tauri updater configuration and signature verification remain authoritative for installation. The UI never treats a GitHub release response as install proof, never offers **Update** without a platform-specific signed manifest, and installs through the same handle that passed preflight. Relaunch occurs only after the signed updater reports success. Download, install, or relaunch failures stay in the popup as retryable errors.

## Package-manager installs and AppImage

On Linux, in-app updates are allowed only for an AppImage. The updater plugin treats any Linux install that is not a deb, rpm or AppImage as an AppImage and would overwrite the packaged binary, which bypasses pacman, apt and dnf.

| Install | In-app update | How to update |
| --- | --- | --- |
| AppImage started from the downloaded file | Yes | **Update** in the banner, then Pulse relaunches |
| Arch or CachyOS package from `packaging/aur/pulse-analytics-bin` | No | Bump `pkgver` and the checksums in the recipe, then `makepkg -si` |
| `.deb` or `.rpm` | No | Install the newer package with apt, dnf or zypper |
| Windows and macOS | Yes | **Update** in the banner |

Pulse decides this at startup from the environment. It counts as an AppImage when `APPIMAGE` is set and, if `APPDIR` is also set, the running executable is inside `APPDIR`; an `APPIMAGE` variable inherited from another AppImage does not count. For every other Linux install the backend sets `managed_by_package_manager` in the update check and does not register the updater plugin, so the banner offers **Open release** and the update check still reports the newer version.

## Validators

```bash
cargo nextest run -p pulse update_check
bun run test
bun run typecheck
```
