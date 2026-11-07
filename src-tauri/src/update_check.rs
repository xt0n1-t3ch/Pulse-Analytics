//! Update check: asks the GitHub releases API for the latest Pulse release and classifies it for
//! the current install channel.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use cc_discord_presence::http;
use chrono::Utc;
use serde::{Deserialize, Serialize};

const REPO_RELEASES_URL: &str = "https://github.com/xt0n1-t3ch/Pulse-Analytics/releases";
const LEGACY_RELEASES_URL: &str =
    "https://github.com/xt0n1-t3ch/Pulse-Claude-Code-Analytics/releases";
const LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/xt0n1-t3ch/Pulse-Analytics/releases/latest";
const USER_AGENT: &str = "Pulse-Analytics";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppUpdateAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
    pub content_type: String,
    /// Platform this asset installs on, derived from the file name
    /// ("windows", "macos", "linux") or `None` when it is not a recognizable
    /// installer. Lets the UI offer one obvious download instead of a raw
    /// asset dump.
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub release_name: Option<String>,
    pub release_notes: Option<String>,
    pub release_url: String,
    pub published_at: Option<String>,
    pub checked_at: String,
    pub assets: Vec<AppUpdateAsset>,
    /// Semver jump between the running build and the release, so the UI can
    /// signal how significant the update is without re-parsing versions.
    pub severity: UpdateSeverity,
    /// True when a package manager owns this install (pacman, apt, dnf), so
    /// Pulse must not replace its own files. The UI offers the release page
    /// instead of an in-app install.
    pub managed_by_package_manager: bool,
}

/// How an install receives updates.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    /// Pulse downloads and installs signed releases itself.
    InApp,
    /// The system package manager updates Pulse.
    PackageManager,
}

/// Decides the update channel from the OS, the `APPIMAGE` and `APPDIR`
/// variables and the running executable.
///
/// `tauri-plugin-updater` treats a Linux install that is not a deb, rpm or
/// AppImage as an AppImage and overwrites the running binary, which would
/// bypass pacman, apt and dnf. In-app updates therefore stay on only for a real
/// AppImage. An inherited `APPIMAGE` (Pulse started from another AppImage) must
/// not count: the AppImage runtime also sets `APPDIR` to the mount that holds
/// the executable, so an executable outside it is not that AppImage.
pub fn update_channel(
    os: &str,
    appimage: Option<&OsStr>,
    appdir: Option<&OsStr>,
    exe: Option<&Path>,
) -> UpdateChannel {
    if os != "linux" {
        return UpdateChannel::InApp;
    }
    if appimage.is_none_or(OsStr::is_empty) {
        return UpdateChannel::PackageManager;
    }
    match appdir.filter(|dir| !dir.is_empty()) {
        None => UpdateChannel::InApp,
        Some(dir) if exe.is_some_and(|exe| exe.starts_with(dir)) => UpdateChannel::InApp,
        Some(_) => UpdateChannel::PackageManager,
    }
}

/// The update channel of the running process.
pub fn current_update_channel() -> UpdateChannel {
    update_channel(
        std::env::consts::OS,
        std::env::var_os("APPIMAGE").as_deref(),
        std::env::var_os("APPDIR").as_deref(),
        std::env::current_exe().ok().as_deref(),
    )
}

/// How large a version jump a release represents.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSeverity {
    /// No newer release, or the running build is ahead.
    None,
    /// Patch bump: 1.6.0 -> 1.6.1.
    Patch,
    /// Minor bump: 1.6.x -> 1.7.0.
    Minor,
    /// Major bump: 1.x -> 2.0.0.
    Major,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    published_at: Option<String>,
    prerelease: bool,
    draft: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    content_type: Option<String>,
}

#[tauri::command]
pub async fn check_app_update() -> Result<AppUpdateInfo, String> {
    tauri::async_runtime::spawn_blocking(check_latest_release)
        .await
        .map_err(|err| format!("update check task failed: {err}"))?
}

/// The update channel without a network call, so the UI knows it offline.
#[tauri::command]
pub fn get_update_channel() -> UpdateChannel {
    current_update_channel()
}

#[tauri::command]
pub fn open_app_release_page(url: Option<String>) -> Result<(), String> {
    let target = url.unwrap_or_else(|| REPO_RELEASES_URL.to_string());
    if !is_allowed_release_url(&target) {
        return Err("release URL is outside the Pulse GitHub releases page".to_string());
    }
    open_url_with_os(&target)
}

fn check_latest_release() -> Result<AppUpdateInfo, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let response = http::agent(Duration::from_secs(8), http::DEFAULT_REDIRECTS)
        .get(LATEST_RELEASE_API_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|err| format!("GitHub release lookup failed: {err}"))?;

    let release: GithubRelease = response
        .into_body()
        .read_json()
        .map_err(|err| format!("GitHub release response was invalid: {err}"))?;

    Ok(update_info_from_release(
        &current_version,
        release,
        current_update_channel() == UpdateChannel::PackageManager,
    ))
}

fn update_info_from_release(
    current_version: &str,
    release: GithubRelease,
    managed_by_package_manager: bool,
) -> AppUpdateInfo {
    let latest_version = normalize_version(&release.tag_name);
    let update_available = !release.draft
        && !release.prerelease
        && compare_versions(&latest_version, current_version).is_gt();

    let severity = if update_available {
        severity_between(current_version, &latest_version)
    } else {
        UpdateSeverity::None
    };

    AppUpdateInfo {
        current_version: current_version.to_string(),
        latest_version: Some(latest_version),
        update_available,
        release_name: release.name,
        release_notes: release.body,
        release_url: release.html_url,
        published_at: release.published_at,
        checked_at: Utc::now().to_rfc3339(),
        assets: release
            .assets
            .into_iter()
            .map(|asset| AppUpdateAsset {
                platform: platform_for_asset(&asset.name),
                name: asset.name,
                download_url: asset.browser_download_url,
                size: asset.size,
                content_type: asset.content_type.unwrap_or_default(),
            })
            .collect(),
        severity,
        managed_by_package_manager,
    }
}

/// Classifies the jump from `current` to `latest`. Only called once an update
/// is known to be newer, so the equal case collapses to `Patch`.
fn severity_between(current: &str, latest: &str) -> UpdateSeverity {
    let cur = parse_version_core(current);
    let new = parse_version_core(latest);
    if new[0] > cur[0] {
        UpdateSeverity::Major
    } else if new[1] > cur[1] {
        UpdateSeverity::Minor
    } else {
        UpdateSeverity::Patch
    }
}

/// Maps a release asset file name to the platform it installs on. Extension
/// based, because that is what the bundle targets in `tauri.conf.json`
/// actually produce (nsis/msi, dmg/app, deb/rpm/appimage).
fn platform_for_asset(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let platform = if lower.ends_with(".exe") || lower.ends_with(".msi") {
        "windows"
    } else if lower.ends_with(".dmg") || lower.ends_with(".app.tar.gz") {
        "macos"
    } else if lower.ends_with(".deb") || lower.ends_with(".rpm") || lower.ends_with(".appimage") {
        "linux"
    } else {
        return None;
    };
    Some(platform.to_string())
}

fn is_allowed_release_url(url: &str) -> bool {
    [REPO_RELEASES_URL, LEGACY_RELEASES_URL]
        .iter()
        .any(|base| url == *base || url.starts_with(&format!("{base}/")))
}

fn open_url_with_os(url: &str) -> Result<(), String> {
    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };

    result
        .map(|_| ())
        .map_err(|err| format!("failed to open release page: {err}"))
}

fn normalize_version(version: &str) -> String {
    version.trim().trim_start_matches('v').to_string()
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left = parse_version_core(left);
    let right = parse_version_core(right);
    left.cmp(&right)
}

fn parse_version_core(version: &str) -> [u64; 3] {
    let core = normalize_version(version);
    let core = core.split_once('-').map_or(core.as_str(), |(head, _)| head);
    let mut parts = [0_u64; 3];
    for (idx, part) in core.split('.').take(3).enumerate() {
        parts[idx] = part.parse::<u64>().unwrap_or(0);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag_name: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag_name.to_string(),
            name: Some(format!("Release {tag_name}")),
            body: Some("Release notes".to_string()),
            html_url: format!("{REPO_RELEASES_URL}/tag/{tag_name}"),
            published_at: Some("2026-06-10T00:00:00Z".to_string()),
            prerelease: false,
            draft: false,
            assets: vec![GithubAsset {
                name: "Pulse_1.3.0_x64-setup.exe".to_string(),
                browser_download_url: format!("{REPO_RELEASES_URL}/download/{tag_name}/Pulse.exe"),
                size: 42,
                content_type: Some("application/octet-stream".to_string()),
            }],
        }
    }

    #[test]
    fn compares_semver_tags_without_v_prefix() {
        assert!(compare_versions("v1.3.0", "1.2.9").is_gt());
        assert!(compare_versions("1.2.0", "v1.2.0").is_eq());
        assert!(compare_versions("1.1.9", "1.2.0").is_lt());
    }

    #[test]
    fn release_newer_than_current_marks_update_available() {
        let info = update_info_from_release("1.2.0", release("v1.3.0"), false);
        assert!(info.update_available);
        assert_eq!(info.current_version, "1.2.0");
        assert_eq!(info.latest_version.as_deref(), Some("1.3.0"));
        assert_eq!(info.assets.len(), 1);
    }

    #[test]
    fn same_or_prerelease_tags_do_not_mark_update_available() {
        let same = update_info_from_release("1.2.0", release("v1.2.0"), false);
        assert!(!same.update_available);

        let mut prerelease = release("v1.3.0-beta.1");
        prerelease.prerelease = true;
        let info = update_info_from_release("1.2.0", prerelease, false);
        assert!(!info.update_available);
    }

    #[test]
    fn release_url_allowlist_blocks_external_links() {
        assert!(is_allowed_release_url(REPO_RELEASES_URL));
        assert!(is_allowed_release_url(&format!(
            "{REPO_RELEASES_URL}/tag/v1.3.0"
        )));
        assert!(!is_allowed_release_url(
            "https://example.com/releases/tag/v1.3.0"
        ));
    }

    #[test]
    fn release_url_allowlist_accepts_renamed_repository_without_broadening_scope() {
        for base in [REPO_RELEASES_URL, LEGACY_RELEASES_URL] {
            assert!(is_allowed_release_url(base));
            assert!(is_allowed_release_url(&format!("{base}/tag/v1.9.1")));
            assert!(is_allowed_release_url(&format!(
                "{base}/download/v1.9.1/pulse.exe"
            )));
            assert!(!is_allowed_release_url(&format!("{base}-other/tag/v1.9.1")));
        }
        assert!(!is_allowed_release_url(
            "https://github.com/another-owner/Pulse-Analytics/releases/tag/v1.9.1"
        ));
        assert!(!is_allowed_release_url(
            "https://github.com.evil.test/xt0n1-t3ch/Pulse-Analytics/releases/tag/v1.9.1"
        ));
        assert!(!is_allowed_release_url(
            "http://github.com/xt0n1-t3ch/Pulse-Analytics/releases/tag/v1.9.1"
        ));
        let info = update_info_from_release("1.8.2", release("v1.9.1"), false);
        assert!(is_allowed_release_url(&info.release_url));
    }

    #[test]
    fn severity_reflects_the_semver_jump() {
        assert_eq!(severity_between("1.6.0", "1.6.1"), UpdateSeverity::Patch);
        assert_eq!(severity_between("1.6.1", "1.7.0"), UpdateSeverity::Minor);
        assert_eq!(severity_between("1.6.1", "2.0.0"), UpdateSeverity::Major);
        // A major bump outranks a lower minor.
        assert_eq!(severity_between("1.9.0", "2.0.0"), UpdateSeverity::Major);
    }

    #[test]
    fn severity_is_none_when_no_update_is_available() {
        let same = update_info_from_release("1.2.0", release("v1.2.0"), false);
        assert_eq!(same.severity, UpdateSeverity::None);
    }

    #[test]
    fn severity_is_reported_for_an_available_update() {
        let info = update_info_from_release("1.2.0", release("v1.3.0"), false);
        assert_eq!(info.severity, UpdateSeverity::Minor);
    }

    #[test]
    fn assets_are_tagged_with_their_install_platform() {
        assert_eq!(
            platform_for_asset("Pulse_1.6.1_x64-setup.exe").as_deref(),
            Some("windows")
        );
        assert_eq!(
            platform_for_asset("Pulse_1.6.1_x64_en-US.msi").as_deref(),
            Some("windows")
        );
        assert_eq!(
            platform_for_asset("Pulse_1.6.1_aarch64.dmg").as_deref(),
            Some("macos")
        );
        assert_eq!(
            platform_for_asset("pulse_1.6.1_amd64.deb").as_deref(),
            Some("linux")
        );
        assert_eq!(
            platform_for_asset("pulse_1.6.1_amd64.AppImage").as_deref(),
            Some("linux")
        );
        // Checksums and signatures are not installers.
        assert_eq!(platform_for_asset("checksums.txt"), None);
        assert_eq!(platform_for_asset("Pulse.exe.sig"), None);
    }

    fn os(value: &str) -> Option<&OsStr> {
        Some(OsStr::new(value))
    }

    #[test]
    fn windows_and_macos_always_update_in_app() {
        for platform in ["windows", "macos"] {
            assert_eq!(
                update_channel(platform, None, None, Some(Path::new("/usr/bin/pulse"))),
                UpdateChannel::InApp
            );
        }
    }

    #[test]
    fn linux_without_appimage_is_managed_by_the_package_manager() {
        let exe = Some(Path::new("/usr/bin/pulse"));
        assert_eq!(
            update_channel("linux", None, None, exe),
            UpdateChannel::PackageManager
        );
        assert_eq!(
            update_channel("linux", os(""), None, exe),
            UpdateChannel::PackageManager
        );
    }

    #[test]
    fn linux_appimage_updates_in_app() {
        let appimage = os("/home/tony/Apps/Pulse.AppImage");
        assert_eq!(
            update_channel("linux", appimage, None, None),
            UpdateChannel::InApp
        );
        assert_eq!(
            update_channel(
                "linux",
                appimage,
                os("/tmp/.mount_PulseAbc123"),
                Some(Path::new("/tmp/.mount_PulseAbc123/usr/bin/pulse")),
            ),
            UpdateChannel::InApp
        );
    }

    #[test]
    fn inherited_appimage_variable_does_not_enable_in_app_updates() {
        assert_eq!(
            update_channel(
                "linux",
                os("/home/tony/Apps/Other.AppImage"),
                os("/tmp/.mount_OtherXyz"),
                Some(Path::new("/usr/bin/pulse")),
            ),
            UpdateChannel::PackageManager
        );
        // The mount name is a prefix of a sibling folder, not an ancestor.
        assert_eq!(
            update_channel(
                "linux",
                os("/home/tony/Apps/Pulse.AppImage"),
                os("/tmp/.mount_Pulse"),
                Some(Path::new("/tmp/.mount_Pulse2/usr/bin/pulse")),
            ),
            UpdateChannel::PackageManager
        );
        assert_eq!(
            update_channel(
                "linux",
                os("/home/tony/Apps/Pulse.AppImage"),
                os("/tmp/.mount_Pulse"),
                None,
            ),
            UpdateChannel::PackageManager
        );
    }

    #[test]
    fn release_info_reports_package_manager_ownership() {
        let managed = update_info_from_release("1.2.0", release("v1.3.0"), true);
        assert!(managed.managed_by_package_manager);
        assert!(managed.update_available);
        let unmanaged = update_info_from_release("1.2.0", release("v1.3.0"), false);
        assert!(!unmanaged.managed_by_package_manager);
    }

    #[test]
    fn update_channel_serializes_as_the_frontend_strings() {
        assert_eq!(
            serde_json::to_value(UpdateChannel::PackageManager).unwrap(),
            "package_manager"
        );
        assert_eq!(
            serde_json::to_value(UpdateChannel::InApp).unwrap(),
            "in_app"
        );
    }
}
