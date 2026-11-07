//! Folders where Discord clients keep their profile data: `Local Storage`
//! holds the signed-in user, `Cache` holds profile images such as the banner.
//!
//! Candidates are listed per platform in priority order. Folders that do not
//! exist are harmless; callers skip them when reading.

use std::path::PathBuf;

const LOCAL_STORAGE: &str = "Local Storage/leveldb";
const CACHE: &str = "Cache/Cache_Data";

/// Electron `userData` names of Discord stable, Canary and PTB.
const DISCORD_NAMES: [&str; 3] = ["discord", "discordcanary", "discordptb"];

/// Flatpak application id and the folder name that build uses under `config/`.
/// The Canary and PTB folder names are inferred from the Electron app name, so
/// the stable name is tried for them as well (not verified against a Flatpak
/// install).
const FLATPAK_DISCORD: [(&str, &str); 3] = [
    ("com.discordapp.Discord", "discord"),
    ("com.discordapp.DiscordCanary", "discordcanary"),
    ("com.discordapp.DiscordPTB", "discordptb"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// Base folders a lookup starts from; injected so tests need no environment.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    pub home: Option<PathBuf>,
    /// `dirs::config_dir()`: `XDG_CONFIG_HOME` or `~/.config` on Linux,
    /// `~/Library/Application Support` on macOS.
    pub config_dir: Option<PathBuf>,
    /// `%APPDATA%` (Windows).
    pub appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%` (Windows).
    pub local_appdata: Option<PathBuf>,
}

impl Locations {
    pub fn from_environment() -> Self {
        Self {
            home: dirs::home_dir(),
            config_dir: dirs::config_dir(),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
            local_appdata: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        }
    }

    /// Folders that contain `Local Storage`, `Cache` and the other profile data.
    pub fn client_roots(&self, os: Os) -> Vec<PathBuf> {
        match os {
            Os::Windows => self.windows_roots(),
            Os::MacOs => self.macos_roots(),
            Os::Linux => self.linux_roots(),
        }
    }

    /// Candidate `Local Storage/leveldb` folders, highest priority first.
    pub fn local_storage_dirs(&self, os: Os) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .client_roots(os)
            .into_iter()
            .map(|root| root.join(LOCAL_STORAGE))
            .collect();
        if os == Os::Windows
            && let Some(local) = &self.local_appdata
        {
            dirs.extend(
                ["Discord", "DiscordCanary", "DiscordPTB"]
                    .iter()
                    .map(|name| local.join(name).join(LOCAL_STORAGE)),
            );
        }
        dirs
    }

    /// Candidate `Cache/Cache_Data` folders, highest priority first.
    pub fn cache_dirs(&self, os: Os) -> Vec<PathBuf> {
        self.client_roots(os)
            .into_iter()
            .map(|root| root.join(CACHE))
            .collect()
    }

    fn windows_roots(&self) -> Vec<PathBuf> {
        let Some(appdata) = &self.appdata else {
            return Vec::new();
        };
        DISCORD_NAMES
            .iter()
            .map(|name| appdata.join(name))
            .collect()
    }

    fn macos_roots(&self) -> Vec<PathBuf> {
        let Some(support) = &self.config_dir else {
            return Vec::new();
        };
        let mut roots: Vec<PathBuf> = DISCORD_NAMES
            .iter()
            .chain(&["Discord", "Discord Canary", "Discord PTB"])
            .map(|name| support.join(name))
            .collect();
        roots.push(support.join("vesktop/sessionData"));
        roots.push(support.join("legcord"));
        roots
    }

    fn linux_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Some(config) = &self.config_dir {
            roots.extend(DISCORD_NAMES.iter().map(|name| config.join(name)));
        }
        if let Some(home) = &self.home {
            let flatpak = home.join(".var/app");
            for (id, name) in FLATPAK_DISCORD {
                roots.push(flatpak.join(id).join("config/discord"));
                if name != "discord" {
                    roots.push(flatpak.join(id).join("config").join(name));
                }
            }
            roots.extend(DISCORD_NAMES.iter().map(|name| {
                home.join("snap")
                    .join(name)
                    .join("current/.config")
                    .join(name)
            }));
        }
        if let Some(config) = &self.config_dir {
            roots.push(config.join("vesktop/sessionData"));
            roots.push(config.join("legcord"));
        }
        if let Some(home) = &self.home {
            let flatpak = home.join(".var/app");
            roots.push(flatpak.join("dev.vencord.Vesktop/config/vesktop/sessionData"));
            roots.push(flatpak.join("app.legcord.Legcord/config/legcord"));
        }
        roots
    }
}

/// `Local Storage/leveldb` folders of this machine's Discord clients.
pub fn local_storage_dirs() -> Vec<PathBuf> {
    Locations::from_environment().local_storage_dirs(Os::current())
}

/// `Cache/Cache_Data` folders of this machine's Discord clients.
pub fn cache_dirs() -> Vec<PathBuf> {
    Locations::from_environment().cache_dirs(Os::current())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn linux_locations() -> Locations {
        Locations {
            home: Some(PathBuf::from("/home/tony")),
            config_dir: Some(PathBuf::from("/home/tony/.config")),
            ..Locations::default()
        }
    }

    fn contains(dirs: &[PathBuf], expected: &str) -> bool {
        dirs.iter().any(|dir| dir == Path::new(expected))
    }

    #[test]
    fn linux_lists_native_flatpak_snap_vesktop_and_legcord_roots() {
        let roots = linux_locations().client_roots(Os::Linux);
        for expected in [
            "/home/tony/.config/discord",
            "/home/tony/.config/discordcanary",
            "/home/tony/.config/discordptb",
            "/home/tony/.var/app/com.discordapp.Discord/config/discord",
            "/home/tony/.var/app/com.discordapp.DiscordCanary/config/discordcanary",
            "/home/tony/.var/app/com.discordapp.DiscordPTB/config/discordptb",
            "/home/tony/snap/discord/current/.config/discord",
            "/home/tony/.config/vesktop/sessionData",
            "/home/tony/.var/app/dev.vencord.Vesktop/config/vesktop/sessionData",
            "/home/tony/.config/legcord",
        ] {
            assert!(contains(&roots, expected), "missing {expected}");
        }
    }

    #[test]
    fn linux_native_stable_client_has_the_highest_priority() {
        let roots = linux_locations().client_roots(Os::Linux);
        assert_eq!(roots[0], Path::new("/home/tony/.config/discord"));
    }

    #[test]
    fn linux_follows_xdg_config_home_but_keeps_flatpak_under_the_home_folder() {
        let locations = Locations {
            home: Some(PathBuf::from("/home/tony")),
            config_dir: Some(PathBuf::from("/mnt/state/xdg")),
            ..Locations::default()
        };
        let roots = locations.client_roots(Os::Linux);
        assert!(contains(&roots, "/mnt/state/xdg/discord"));
        assert!(contains(&roots, "/mnt/state/xdg/vesktop/sessionData"));
        assert!(!contains(&roots, "/home/tony/.config/discord"));
        assert!(contains(
            &roots,
            "/home/tony/.var/app/com.discordapp.Discord/config/discord"
        ));
    }

    #[test]
    fn linux_without_a_home_folder_lists_only_config_dir_roots() {
        let locations = Locations {
            config_dir: Some(PathBuf::from("/etc/xdg")),
            ..Locations::default()
        };
        let roots = locations.client_roots(Os::Linux);
        assert!(contains(&roots, "/etc/xdg/discord"));
        assert!(roots.iter().all(|root| root.starts_with("/etc/xdg")));
        assert!(Locations::default().client_roots(Os::Linux).is_empty());
    }

    #[test]
    fn local_storage_and_cache_use_the_profile_subfolders() {
        let locations = linux_locations();
        assert!(contains(
            &locations.local_storage_dirs(Os::Linux),
            "/home/tony/.config/discord/Local Storage/leveldb"
        ));
        assert!(contains(
            &locations.cache_dirs(Os::Linux),
            "/home/tony/.config/vesktop/sessionData/Cache/Cache_Data"
        ));
    }

    #[test]
    fn windows_keeps_appdata_variants_then_local_appdata_for_local_storage() {
        let locations = Locations {
            appdata: Some(PathBuf::from("/AppData/Roaming")),
            local_appdata: Some(PathBuf::from("/AppData/Local")),
            ..Locations::default()
        };
        let local_storage = locations.local_storage_dirs(Os::Windows);
        assert_eq!(
            local_storage,
            [
                "/AppData/Roaming/discord/Local Storage/leveldb",
                "/AppData/Roaming/discordcanary/Local Storage/leveldb",
                "/AppData/Roaming/discordptb/Local Storage/leveldb",
                "/AppData/Local/Discord/Local Storage/leveldb",
                "/AppData/Local/DiscordCanary/Local Storage/leveldb",
                "/AppData/Local/DiscordPTB/Local Storage/leveldb",
            ]
            .map(PathBuf::from)
        );
        // The banner cache lives in the roaming folder only.
        assert_eq!(
            locations.cache_dirs(Os::Windows),
            [
                "/AppData/Roaming/discord/Cache/Cache_Data",
                "/AppData/Roaming/discordcanary/Cache/Cache_Data",
                "/AppData/Roaming/discordptb/Cache/Cache_Data",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn windows_without_appdata_has_no_roaming_candidates() {
        assert!(Locations::default().client_roots(Os::Windows).is_empty());
    }

    #[test]
    fn macos_lists_application_support_variants_and_vesktop() {
        let locations = Locations {
            config_dir: Some(PathBuf::from("/Users/tony/Library/Application Support")),
            ..Locations::default()
        };
        let roots = locations.client_roots(Os::MacOs);
        for expected in [
            "/Users/tony/Library/Application Support/discord",
            "/Users/tony/Library/Application Support/Discord Canary",
            "/Users/tony/Library/Application Support/vesktop/sessionData",
        ] {
            assert!(contains(&roots, expected), "missing {expected}");
        }
    }
}
