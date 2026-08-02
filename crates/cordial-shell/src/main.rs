//! `cordial-shell` — the core shell binary.
//!
//! [ADR-002](../../../docs/adr/ADR-002-core-shell-and-ui-handoff.md) draws the
//! line this crate has to stay inside: core owns a window, the chooser that
//! paints at T1, and a minimal settings fallback narrow enough to disable a
//! broken plugin. Everything richer — real settings, themes, plugin-contributed
//! chooser entries, instance management — belongs to the UI plugin that takes
//! over at T3. This binary does not link the plugin host or the engine at all;
//! it is built standalone on purpose, so the window/chooser/settings shape can
//! be proven before either of those exist. See `window.rs` for the seam where
//! the engine's Wayland surface will eventually be embedded.
//!
//! [ADR-011](../../../docs/adr/ADR-011-wayland-and-libadwaita.md) is why this
//! is libadwaita rather than bare GTK: `AdwStyleManager` tracks
//! `org.freedesktop.appearance color-scheme` on its own, live, which is what
//! keeps the area behind the engine's canvas the desktop's actual background
//! colour instead of a flash of white while a resize catches up.

mod chooser;
mod flags_file;
mod install;
mod instructions;
mod launch;
mod profile_switcher;
mod settings;
mod shell_config;
mod window;

use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Must match `packaging/org.cordial.Cordial.desktop`'s file name and
/// `StartupWMClass`. GNOME Shell uses the application id to find the desktop
/// entry for window-to-launcher matching; let the two drift and the taskbar
/// icon and startup notification silently stop matching up rather than erroring.
const APP_ID: &str = "org.cordial.Cordial";

fn main() -> libadwaita::glib::ExitCode {
    let app = libadwaita::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        // Before anything can be launched, because the launcher points the
        // engine at a profile directory and the storage that has a login in it
        // is still at the pre-ADR-012 path. Skipped when there is nothing to
        // move, which is every run after the first.
        cordial_shell::profile::migrate_legacy_layout();

        let config_path = Rc::new(shell_config::path());
        let config = Rc::new(RefCell::new(shell_config::load(&config_path)));

        // Applied before the window exists so the very first paint already
        // matches whatever the user last chose in Appearance, rather than
        // flashing the libadwaita default and then correcting itself.
        config.borrow().appearance.apply();

        window::build(app, config, config_path);
    });
    app.run()
}
