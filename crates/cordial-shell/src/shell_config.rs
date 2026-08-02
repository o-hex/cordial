//! Cordial's own shell preferences.
//!
//! Distinct from `flags_file.rs`, which speaks to the engine, and from
//! `cordial_plugins::grants`, which speaks to what a plugin may do. This is
//! the shell's own state, and for now that is exactly one thing: which
//! appearance the user asked Cordial itself to use.
//!
//! `$XDG_CONFIG_HOME/cordial/shell.json`, falling back to `$HOME/.config` —
//! the same layout `cordial_plugins::grants::path` and
//! `cordial_plugins::manifest::plugin_root` use — and the same
//! default-on-anything-wrong behaviour as `grants::load`: a missing or
//! malformed file means "use the defaults", not "refuse to start". Nobody
//! but this shell ever writes this file, so a malformed one is far likelier
//! to be an interrupted write than anything adversarial, and refusing to
//! start over that would be a worse failure than quietly falling back.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What appearance Cordial itself should use.
///
/// Not a desktop-wide setting — `AdwStyleManager::set_color_scheme` applies
/// this to this application only. `System` means *follow*
/// `org.freedesktop.appearance color-scheme`, live, the way ADR-011 already
/// relies on for the canvas background; it must never mean *write* that
/// setting. Cordial has no business changing the desktop's theme to satisfy
/// its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceScheme {
    Light,
    Dark,
    System,
}

impl AppearanceScheme {
    /// Order matches the `AdwComboRow` model in `settings.rs` — `index` and
    /// `from_index` are the seam between the two, kept as plain position
    /// rather than a second name-keyed lookup that could drift from the
    /// model's actual contents.
    pub fn index(self) -> u32 {
        match self {
            AppearanceScheme::Light => 0,
            AppearanceScheme::Dark => 1,
            AppearanceScheme::System => 2,
        }
    }

    pub fn from_index(index: u32) -> Self {
        match index {
            0 => AppearanceScheme::Light,
            1 => AppearanceScheme::Dark,
            _ => AppearanceScheme::System,
        }
    }

    /// Applies to this process only. `ColorScheme::Default` is what makes
    /// `System` live — libadwaita keeps tracking the portal itself once the
    /// override is lifted, nothing here has to poll or resubscribe.
    pub fn apply(self) {
        let scheme = match self {
            AppearanceScheme::Light => libadwaita::ColorScheme::ForceLight,
            AppearanceScheme::Dark => libadwaita::ColorScheme::ForceDark,
            AppearanceScheme::System => libadwaita::ColorScheme::Default,
        };
        libadwaita::StyleManager::default().set_color_scheme(scheme);
    }
}

impl Default for AppearanceScheme {
    fn default() -> Self {
        AppearanceScheme::System
    }
}

/// The profile a launch runs against when nobody has chosen otherwise.
///
/// ADR-012's migration lands the pre-existing storage at `profiles/default`, so
/// this name is not arbitrary — picking anything else would present as being
/// logged out.
pub const DEFAULT_PROFILE: &str = "default";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub appearance: AppearanceScheme,
    /// Where the Roblox build is, when the user has pinned it. Empty is the
    /// normal state and means "look every time" — see `install::locate`, which
    /// explains why a remembered answer is the wrong thing to store here.
    pub roblox: crate::install::RobloxInstall,
    /// Which profile an instance started from this shell runs. ADR-012: a
    /// profile is storage, an instance is a window, and one profile is held by
    /// at most one instance.
    pub profile: String,
    /// Ask Feral GameMode to raise the CPU governor, the process priority and
    /// the GPU's performance profile while the client runs, and to hold the
    /// screensaver off.
    ///
    /// Default on, which is what Sober does and what makes it worth having: a
    /// performance setting nobody finds is a performance setting nobody gets.
    /// It costs nothing on a machine without gamemoded — the request is a D-Bus
    /// call that fails, the client says so once and carries on — so there is no
    /// population this default hurts. `false` here becomes `CORDIAL_GAMEMODE=0`
    /// on the client, which is also the control for measuring what it does.
    pub gamemode: bool,
    /// Show MangoHUD's frame rate and frame time overlay over the client.
    ///
    /// Default off, unlike `gamemode`, and for a reason that is not timidity:
    /// this one is visible. It draws over the game whether or not the user
    /// wanted it there, so it has to be asked for. It is also the setting most
    /// likely to be switched on by somebody who has not got MangoHUD installed
    /// — see `launch::mangohud_layer`, which is what stops that being a silent
    /// no-op.
    pub mangohud: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            appearance: AppearanceScheme::default(),
            roblox: crate::install::RobloxInstall::default(),
            profile: DEFAULT_PROFILE.to_string(),
            gamemode: true,
            mangohud: false,
        }
    }
}

/// `CORDIAL_SHELL_CONFIG` overrides the path outright, the same override
/// pattern `cordial_plugins::grants::path` and `manifest::plugin_root` use —
/// useful for tests and for running more than one Cordial config side by
/// side without them fighting over the same file.
pub fn path() -> PathBuf {
    std::env::var_os("CORDIAL_SHELL_CONFIG").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(std::env::temp_dir)
            .join("cordial/shell.json")
    })
}

/// Load the config, or the defaults. A missing file is the ordinary case —
/// most people never open settings — and a malformed one is reported and
/// treated the same as missing, per the module docs above.
pub fn load(path: &Path) -> ShellConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return ShellConfig::default();
    };
    match serde_json::from_str(&text) {
        Ok(config) => config,
        Err(e) => {
            println!("  shell: {} is not usable ({e}); using defaults", path.display());
            ShellConfig::default()
        }
    }
}

pub fn save(path: &Path, config: &ShellConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config).expect("ShellConfig always serialises");
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("cordial-shell-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn a_missing_file_defaults_to_system() {
        let p = scratch("missing.json");
        let _ = std::fs::remove_file(&p);
        assert_eq!(load(&p).appearance, AppearanceScheme::System);
    }

    #[test]
    fn a_malformed_file_falls_back_to_defaults_rather_than_refusing_to_start() {
        let p = scratch("malformed.json");
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(load(&p).appearance, AppearanceScheme::System);
    }

    #[test]
    fn a_saved_choice_round_trips() {
        let p = scratch("roundtrip.json");
        save(&p, &ShellConfig { appearance: AppearanceScheme::Dark, ..Default::default() }).unwrap();
        assert_eq!(load(&p).appearance, AppearanceScheme::Dark);
    }

    #[test]
    fn a_config_written_before_the_roblox_fields_existed_still_loads() {
        // `#[serde(default)]` is what makes this true, and it is worth a test
        // rather than a note: everyone who has run this shell already has a
        // shell.json holding nothing but `appearance`, and a launcher that
        // refuses to start over a missing field it invented is a worse failure
        // than any of the ones it is meant to report.
        let p = scratch("older-schema.json");
        std::fs::write(&p, r#"{"appearance":"dark"}"#).unwrap();
        let config = load(&p);
        assert_eq!(config.appearance, AppearanceScheme::Dark);
        assert_eq!(config.profile, DEFAULT_PROFILE);
        assert_eq!(config.roblox, crate::install::RobloxInstall::default());
        // The performance fields are newer still, and the same argument
        // applies to them: everybody's shell.json predates them.
        assert!(config.gamemode, "an older config must still get GameMode's default");
        assert!(!config.mangohud);
    }

    #[test]
    fn the_performance_switches_round_trip() {
        // Both directions, because both defaults are worth being able to
        // reverse and a setting that only saves the value it already had would
        // pass a one-way test.
        let p = scratch("performance.json");
        save(&p, &ShellConfig { gamemode: false, mangohud: true, ..Default::default() }).unwrap();
        let back = load(&p);
        assert!(!back.gamemode);
        assert!(back.mangohud);
    }

    #[test]
    fn the_roblox_paths_round_trip() {
        let p = scratch("roblox.json");
        let mut config = ShellConfig::default();
        config.roblox.apk = Some(PathBuf::from("/somewhere/base.apk"));
        config.roblox.lib_dir = Some(PathBuf::from("/somewhere/lib/x86_64"));
        config.profile = "alt_account".into();
        save(&p, &config).unwrap();
        let back = load(&p);
        assert_eq!(back.roblox.apk, config.roblox.apk);
        assert_eq!(back.roblox.lib_dir, config.roblox.lib_dir);
        assert_eq!(back.profile, "alt_account");
    }

    #[test]
    fn index_and_from_index_agree_with_each_other() {
        for scheme in [AppearanceScheme::Light, AppearanceScheme::Dark, AppearanceScheme::System] {
            assert_eq!(AppearanceScheme::from_index(scheme.index()), scheme);
        }
    }
}
