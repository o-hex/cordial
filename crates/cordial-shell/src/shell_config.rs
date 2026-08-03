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
            AppearanceScheme::System => system_scheme(portal_colour_scheme()),
        };
        libadwaita::StyleManager::default().set_color_scheme(scheme);
    }
}

impl Default for AppearanceScheme {
    fn default() -> Self {
        AppearanceScheme::System
    }
}

/// How long to wait for the settings portal before deciding nobody is there.
///
/// ADR-002 budgets the shell's first paint in milliseconds and this call sits in
/// front of it, so the number is a compromise rather than a safety margin: on a
/// session with a portal the reply comes back in about a millisecond and this is
/// never reached, and on one without it the bus refuses immediately rather than
/// hanging. What it actually bounds is the case in between — a portal that is
/// starting, or wedged — where half a second of default theming is a better
/// outcome than a launcher that appears to have failed to start.
const PORTAL_TIMEOUT_MS: i32 = 500;

/// What `System` has to resolve to right now, given what the desktop said.
///
/// **A preference, not a correctness fix, and the branch looks wrong until you
/// know why.** `ColorScheme::Default` is the value that means "follow the
/// desktop", and it is what `System` should be whenever the desktop can be
/// asked — someone on a light desktop must still get light, and `Default` is
/// also what keeps the window tracking a change made while it is open, which is
/// worth more than any startup decision taken once.
///
/// The single source libadwaita consults for that is the settings portal's
/// `org.freedesktop.appearance color-scheme`. When nothing answers it, there is
/// no preference to follow — but `Default` does not mean "unknown", it renders
/// light, so an unreachable portal presents as a deliberate light theme. That is
/// how the owner's launcher kept appearing in light on a `prefer-dark` desktop:
/// a process without the session bus in its environment has no portal to ask,
/// and light is what falls out. A game launcher guessing light when it has not
/// been told is the worse guess, so the unknown case is dark.
///
/// The cost is stated rather than hidden: if the portal is unreachable *and*
/// libadwaita's non-sandboxed GSettings fallback would have found a genuine
/// light preference, this overrides it. That is accepted — the owner asked for
/// dark as the answer to "we do not know", and the portal is what defines
/// knowing here.
fn system_scheme(portal: Option<u32>) -> libadwaita::ColorScheme {
    match portal {
        Some(_) => libadwaita::ColorScheme::Default,
        None => libadwaita::ColorScheme::ForceDark,
    }
}

/// The desktop's `org.freedesktop.appearance color-scheme`, asked for directly.
///
/// Only the *presence* of an answer is used — see [`system_scheme`] — so this
/// deliberately does not interpret the value it returns. `ReadOne` first because
/// it is what current portals implement, then `Read`, which older ones offer and
/// which boxes the value one variant deeper; either shape is unwrapped by
/// following `v` down until something that is not a variant comes out.
///
/// Through `gio` rather than `zbus`. `zbus` is a dependency of `cordial-runtime`
/// and not of this crate, and the shell is already holding a GDBus connection
/// through GTK — adding an async runtime to this crate to ask one question the
/// toolkit can already ask would be the larger change.
fn portal_colour_scheme() -> Option<u32> {
    use libadwaita::gtk::gio;
    use libadwaita::gtk::glib::prelude::*;

    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let arguments = ("org.freedesktop.appearance", "color-scheme").to_variant();

    for method in ["ReadOne", "Read"] {
        let Ok(reply) = bus.call_sync(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            method,
            Some(&arguments),
            None,
            gio::DBusCallFlags::NONE,
            PORTAL_TIMEOUT_MS,
            gio::Cancellable::NONE,
        ) else {
            continue;
        };
        let mut value = reply.child_value(0);
        while let Some(inner) = value.as_variant() {
            value = inner;
        }
        if let Some(scheme) = value.get::<u32>() {
            return Some(scheme);
        }
    }
    None
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
    /// What Cordial does about a new Roblox build without being asked, and over
    /// which connections it may fetch one.
    ///
    /// Two fields rather than one `UpdateSettings`, because they are two rows in
    /// two places: the dropdown and the pair of switches sit in the same group
    /// but nothing else in this file nests, and a settings document is read by
    /// people as often as by serde. `updater::update_settings` puts them back
    /// together for `cordial_update::settings::UpdateSettings::plan`, which is
    /// the only thing that wants them as a pair.
    ///
    /// Neither governs anything today and the settings page says so: Roblox
    /// publishes no Android build to download, so there is nothing for the plan
    /// to act on. They are stored anyway, because the choice is the user's to
    /// make before the day it matters rather than after it.
    pub automatic_updates: cordial_update::settings::Automatic,
    pub download_on: cordial_update::settings::DownloadOn,
    /// Show MangoHUD's frame rate and frame time overlay over the client.
    ///
    /// Default off, unlike `gamemode`, and for a reason that is not timidity:
    /// this one is visible. It draws over the game whether or not the user
    /// wanted it there, so it has to be asked for. It is also the setting most
    /// likely to be switched on by somebody who has not got MangoHUD installed
    /// — see `launch::mangohud_layer`, which is what stops that being a silent
    /// no-op.
    /// Which graphics backend the client offers the engine.
    ///
    /// Stored as the same lowercase words `cordial_runtime::graphics::Backend`
    /// parses, and passed to the client as `CORDIAL_GRAPHICS` rather than
    /// written to a file: the backend has to be settled before the engine's
    /// first `dlopen`, which is long before anything opens a profile.
    ///
    /// `"automatic"` is the default and is not merely "Vulkan by another name" —
    /// it is the absence of a user opinion, which is what lets a plugin have
    /// one. See `graphics::resolve`.
    pub graphics: String,
    pub mangohud: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            appearance: AppearanceScheme::default(),
            roblox: crate::install::RobloxInstall::default(),
            profile: DEFAULT_PROFILE.to_string(),
            automatic_updates: cordial_update::settings::Automatic::default(),
            download_on: cordial_update::settings::DownloadOn::default(),
            gamemode: true,
            graphics: "automatic".to_string(),
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
    fn the_update_settings_round_trip() {
        // Same shape as `a_saved_choice_round_trips`, for the same reason: a
        // control that accepts a choice and does not keep it is worse than one
        // that refuses, because the user finds out a launch later.
        use cordial_update::settings::{Automatic, DownloadOn};
        let p = scratch("updates.json");
        save(
            &p,
            &ShellConfig {
                automatic_updates: Automatic::Manual,
                download_on: DownloadOn { metered: true },
                ..Default::default()
            },
        )
        .unwrap();
        let back = load(&p);
        assert_eq!(back.automatic_updates, Automatic::Manual);
        assert!(back.download_on.metered);
    }

    #[test]
    fn a_config_written_before_the_update_settings_existed_gets_their_defaults() {
        // Everybody's shell.json predates these three controls, and a launcher
        // that refuses to start over a field it has just invented is a worse
        // failure than any it exists to report.
        use cordial_update::settings::Automatic;
        let p = scratch("pre-updates.json");
        std::fs::write(&p, r#"{"appearance":"dark","profile":"default"}"#).unwrap();
        let config = load(&p);
        assert_eq!(config.automatic_updates, Automatic::Background);
        assert!(!config.download_on.metered, "a data allowance is not the default to spend");
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
    fn an_unanswered_portal_is_dark_rather_than_light() {
        // The owner's report, in one assertion: their launcher kept opening in
        // light on a `prefer-dark` desktop, because `ColorScheme::Default`
        // renders light when nothing told it otherwise and a process without
        // the session bus has nothing to ask. Unknown is dark now.
        assert_eq!(system_scheme(None), libadwaita::ColorScheme::ForceDark);
    }

    #[test]
    fn a_desktop_that_answers_is_still_followed_live() {
        // The half that must not be lost in fixing the other one. `Default` is
        // the only value that keeps tracking a change made while the window is
        // open, and forcing dark on an answering desktop would take light away
        // from somebody who chose it. The value itself is not inspected on
        // purpose, so every answer maps the same way.
        for reported in [0, 1, 2] {
            assert_eq!(system_scheme(Some(reported)), libadwaita::ColorScheme::Default);
        }
    }

    #[test]
    fn index_and_from_index_agree_with_each_other() {
        for scheme in [AppearanceScheme::Light, AppearanceScheme::Dark, AppearanceScheme::System] {
            assert_eq!(AppearanceScheme::from_index(scheme.index()), scheme);
        }
    }
}
