//! The minimal settings fallback ADR-002 requires.
//!
//! Not the settings surface — the UI plugin owns that once it takes over at
//! T3. This exists purely for the recoverability argument ADR-002 makes: if
//! the UI plugin is broken (bad update, protocol mismatch after a Cordial
//! upgrade), the user still needs a way to see what is installed and turn
//! the offending one off, plus enough of Cordial's own appearance and
//! graphics preference to be usable, without a terminal.
//!
//! `AdwPreferencesWindow` with several `AdwPreferencesPage`s — Roblox, Updates,
//! Appearance, General, Plugins — each becomes its own tab/sidebar entry for
//! free; that is libadwaita's own page-switcher, not something built here.
//!
//! The Roblox page is newer than the argument above and is here for a different
//! reason: nothing in Cordial used to record where a Roblox build lived, so the
//! chooser had no target and the only way to run the client was a hand-typed
//! command line. That is configuration rather than a fallback, and it belongs
//! in the shell because the shell is what has to work when nothing else does.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use std::collections::BTreeSet;

use cordial_plugins::capability::Capability;
use cordial_plugins::enablement;
use cordial_plugins::grants;
use cordial_plugins::manifest::{self, Plugin};
use cordial_plugins::unpack;

use crate::install;
use crate::shell_config::{self, AppearanceScheme, ShellConfig};

/// What a plugin asks for, and what this profile has actually given it.
///
/// Both halves are read from disk every time the page is built. There used to be
/// a `DemoRegistry` here instead: three hand-written manifests, including one
/// claiming `assets.override` and one claiming `presence.set`, parsed against a
/// directory spelled `/nonexistent/demo-plugin-dir`. It rendered on a screen
/// whose entire job is telling somebody what has been granted what, on a machine
/// where `~/.local/share/cordial/plugins` is empty. ADR-003's default-deny means
/// nothing if the surface that displays it is fiction, and "this plugin may
/// change your assets and talk to Discord for you" is not a sentence to invent.
///
/// Three states, kept apart because somebody debugging a plugin that is doing
/// nothing has to be able to tell which one they are in.
///
/// *Enabled and granted something* is the one that runs. *Enabled and granted
/// nothing* also does not run, and collapsing it into "off" would hide the only
/// thing that would fix it — nobody has approved anything yet, which is where
/// ADR-003's default deny leaves every freshly installed plugin. *Disabled* does
/// not run whatever its grants say, and its grants are still shown, because they
/// are still there: turning something off does not cost the approvals.
pub fn capability_summary(
    plugin: &Plugin,
    granted: Option<&BTreeSet<Capability>>,
    enabled: bool,
) -> String {
    let names = |set: &BTreeSet<Capability>| -> String {
        set.iter().copied().map(Capability::name).collect::<Vec<_>>().join(", ")
    };
    let requests = if plugin.requested.is_empty() {
        "Requests no capabilities".to_string()
    } else {
        format!("Requests: {}", names(&plugin.requested))
    };
    let has_grants = granted.is_some_and(|g| !g.is_empty());
    let grants_line = if has_grants {
        format!("Granted: {}", names(granted.expect("has_grants implies Some")))
    } else {
        "Granted: nothing".to_string()
    };
    let state = match (enabled, has_grants) {
        (false, true) => "Off. Its grants are kept, so turning it back on costs no approvals",
        (false, false) => "Off",
        (true, true) => "On",
        (true, false) => "On, but nothing is approved yet, so it will not do anything",
    };
    format!("{requests}\n{grants_line}\n{state}")
}

/// "Appearance" — Cordial's own theme preference, independent of anything
/// else on the desktop. `AppearanceScheme::apply` is what makes this take
/// effect immediately; this function only wires the row to it and to disk.
fn build_appearance_page(config: Rc<RefCell<ShellConfig>>, config_path: Rc<PathBuf>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Appearance")
        .name("appearance")
        .icon_name("applications-graphics-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description(
            "This is Cordial's own preference, not the desktop's. System follows \
             org.freedesktop.appearance and updates live, and falls back to dark when \
             nothing answers for it; Light and Dark stay fixed regardless of what the \
             desktop is doing.",
        )
        .build();

    // Order has to match AppearanceScheme::index/from_index.
    let model = gtk::StringList::new(&["Light", "Dark", "System"]);
    let row = adw::ComboRow::builder()
        .title("Theme")
        .model(&model)
        .selected(config.borrow().appearance.index())
        .build();

    row.connect_selected_notify(move |row| {
        let scheme = AppearanceScheme::from_index(row.selected());
        scheme.apply();
        config.borrow_mut().appearance = scheme;
        if let Err(e) = crate::shell_config::save(&config_path, &config.borrow()) {
            eprintln!("shell: could not save {}: {e}", config_path.display());
        }
    });

    group.add(&row);
    page.add(&group);
    page
}

/// Save, and say so on stderr rather than swallowing the error.
///
/// A settings window that accepts a choice and silently fails to keep it is the
/// worst of the three possible behaviours, because the user finds out one
/// launch later and blames the launch.
pub fn persist(config: &Rc<RefCell<ShellConfig>>, path: &Rc<PathBuf>) {
    if let Err(e) = shell_config::save(path, &config.borrow()) {
        eprintln!("shell: could not save {}: {e}", path.display());
    }
}

/// One row showing a path Cordial is using, where it came from, and how to
/// change it.
///
/// The provenance line is not decoration. The build usually comes from another
/// application's private directory (Sober's), and a user who does not know that
/// has no way to understand why deleting Sober broke Cordial. ADR-002's rule
/// about not silently depending on something applies to a path exactly as much
/// as to a capability.
fn path_row(
    title: &str,
    effective: Option<(PathBuf, String)>,
    chosen: bool,
    on_choose: impl Fn() + 'static,
    on_clear: impl Fn() + 'static,
) -> adw::ActionRow {
    let subtitle = match &effective {
        Some((path, origin)) => format!("{}\n{origin}", path.display()),
        None => "Not found. Press Roblox in the launcher for how to get one.".to_string(),
    };
    let row = adw::ActionRow::builder().title(title).subtitle(subtitle).build();
    row.set_subtitle_lines(3);

    if chosen {
        let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
        clear.set_tooltip_text(Some("Forget this and look again on the next launch"));
        clear.set_valign(gtk::Align::Center);
        clear.add_css_class("flat");
        clear.connect_clicked(move |_| on_clear());
        row.add_suffix(&clear);
    }

    let choose = gtk::Button::with_label("Choose…");
    choose.set_valign(gtk::Align::Center);
    choose.connect_clicked(move |_| on_choose());
    row.add_suffix(&choose);
    row
}

/// "Roblox" — where the build is.
///
/// The whole reason `cordial-shell` could not launch anything: nothing
/// persisted where a Roblox build lived, so the only route to a running client
/// was a hand-typed command line. Both rows here are *overrides* — left empty,
/// which is the normal state, `install::locate` looks for a build every time,
/// because a remembered "yes it is installed" goes stale the moment somebody
/// moves it.
fn build_roblox_page(
    parent: &impl IsA<gtk::Window>,
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> adw::PreferencesPage {
    let page =
        adw::PreferencesPage::builder()
            .title("Roblox")
            .name("roblox")
            .icon_name("applications-games-symbolic")
            .build();

    let group = adw::PreferencesGroup::builder()
        .title("Roblox build")
        .description(
            "Cordial ships no Roblox code and never will, so it needs a build you already \
             have. Leave these empty and it finds one on its own; choose a file to pin it.",
        )
        .build();

    let apk = install::effective_apk(&config.borrow().roblox)
        .map(|(path, origin)| (path, origin.describe().to_string()));
    let apk_chosen = config.borrow().roblox.apk.is_some();

    let window = parent.as_ref().clone();
    let apk_row = {
        let config = config.clone();
        let config_path = config_path.clone();
        let window = window.clone();
        let cleared = config.clone();
        let cleared_path = config_path.clone();
        path_row(
            "APK",
            apk,
            apk_chosen,
            move || {
                let config = config.clone();
                let config_path = config_path.clone();
                choose_file(&window, "Choose the Roblox APK", false, move |path| {
                    config.borrow_mut().roblox.apk = Some(path);
                    persist(&config, &config_path);
                });
            },
            move || {
                cleared.borrow_mut().roblox.apk = None;
                persist(&cleared, &cleared_path);
            },
        )
    };
    group.add(&apk_row);

    // Shown separately from the APK because it usually is separate: on a split
    // build `libroblox.so` is inside `split_config.x86_64.apk`, not `base.apk`,
    // and Cordial extracts it into its own cache rather than writing into
    // whichever application's directory the APK came from.
    //
    // The extracted case carries `updater::cache_line`, which says whether the
    // engine still matches the APK above. That used to be a row in the
    // header-bar button's window; it belongs here, beside the directory it is
    // about, and it is the only place a Cordial that re-extracts 115 MB on every
    // launch says what it is comparing.
    let lib = config.borrow().roblox.lib_dir.clone().map(|p| (p, "Chosen in Settings".to_string())).or_else(
        || {
            let cache = install::engine_cache();
            if !cache.join(install::LIBRARY).is_file() {
                return None;
            }
            let apk = install::effective_apk(&config.borrow().roblox).map(|(path, _)| path);
            let line = crate::updater::cache_line(
                true,
                cordial_update::cache::stamp_of(&cache),
                apk.as_deref().is_some_and(|apk| cordial_update::cache::is_current(&cache, apk)),
            );
            Some((cache, line))
        },
    );
    let lib_chosen = config.borrow().roblox.lib_dir.is_some();

    let lib_row = {
        let config = config.clone();
        let config_path = config_path.clone();
        let window = window.clone();
        let cleared = config.clone();
        let cleared_path = config_path.clone();
        path_row(
            "Engine directory",
            lib,
            lib_chosen,
            move || {
                let config = config.clone();
                let config_path = config_path.clone();
                choose_file(&window, "Choose the directory holding libroblox.so", true, move |path| {
                    config.borrow_mut().roblox.lib_dir = Some(path);
                    persist(&config, &config_path);
                });
            },
            move || {
                cleared.borrow_mut().roblox.lib_dir = None;
                persist(&cleared, &cleared_path);
            },
        )
    };
    group.add(&lib_row);
    page.add(&group);

    // The update settings were a group on this page, on the argument that they
    // govern the build the two rows above point at. They are a page of their own
    // now, and the argument did not survive what it produced: a dropdown, two
    // switches, a conditional warning row and six lines of description sitting
    // on top of the two path rows this page exists for, so that "where is my
    // Roblox build" and "when does Cordial look for a new one" were answered by
    // one scroll. Two questions, two tabs, and libadwaita draws the tab.

    // The profile used to be a third row here, a text entry, and it is
    // deliberately not replaced by anything. It lives in the header bar now —
    // see `profile_switcher.rs` — because choosing one is a thing done on the
    // way to launching rather than a setting to go and find, and because two
    // controls writing `ShellConfig.profile` would be two things to keep
    // agreeing with each other.
    page
}

/// A file or folder picker, portal-backed under Flatpak.
///
/// `GtkFileDialog` rather than `GtkFileChooserNative`: the latter is deprecated
/// as of GTK 4.10, which is this project's floor anyway.
pub(crate) fn choose_file(
    parent: &gtk::Window,
    title: &str,
    directory: bool,
    on_chosen: impl Fn(PathBuf) + 'static,
) {
    let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
    let handle = move |result: Result<gtk::gio::File, glib::Error>| {
        // A dismissed dialog arrives here as an error, and it is the common
        // case rather than a fault — reporting it would mean an error message
        // every time somebody changes their mind.
        if let Ok(file) = result {
            if let Some(path) = file.path() {
                on_chosen(path);
            }
        }
    };
    if directory {
        dialog.select_folder(Some(parent), gtk::gio::Cancellable::NONE, handle);
    } else {
        dialog.open(Some(parent), gtk::gio::Cancellable::NONE, handle);
    }
}

/// "Performance" — the two host-side switches, which are not engine flags.
///
/// Separate from the Graphics group above because they are a different kind of
/// thing: the renderer preference is written into `flags.json` and read by the
/// engine, whereas these two are environment the shell sets on the client
/// process and neither of them is Roblox's business.
///
/// **The MangoHUD row is insensitive when MangoHUD is not installed, and says
/// so.** That is the whole reason this function is longer than it looks like it
/// should be. `MANGOHUD=1` with no layer installed is not an error — the client
/// starts, nothing appears, and nothing anywhere says why — so a switch offered
/// unconditionally would be a control that reports success and does not act.
/// The plugins page in this same window exists in its current form because that
/// exact defect shipped here twice.
fn build_performance_group(
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Performance")
        .description(
            "Applied to the client process at launch rather than written into Roblox's own \
             settings, so both take effect the next time you press Roblox.",
        )
        .build();

    let gamemode = adw::SwitchRow::builder()
        .title("Feral GameMode")
        .subtitle(
            "Asks gamemoded for the performance CPU governor, higher priority, the GPU's \
             performance profile and no screensaver while you play. Does nothing at all if \
             gamemoded is not installed; the client says which way it went.",
        )
        .active(config.borrow().gamemode)
        .build();
    gamemode.set_subtitle_lines(4);
    {
        let config = config.clone();
        let config_path = config_path.clone();
        gamemode.connect_active_notify(move |row| {
            config.borrow_mut().gamemode = row.is_active();
            persist(&config, &config_path);
        });
    }
    group.add(&gamemode);

    let layer = crate::launch::mangohud_layer();
    let mangohud = adw::SwitchRow::builder()
        .title("MangoHUD overlay")
        .subtitle(match &layer {
            Some(path) => format!(
                "Frame rate, frame times and CPU/GPU load, drawn over the game by MangoHUD's \
                 Vulkan layer.\n{}",
                path.display()
            ),
            // Named as the reason the switch is dead, with the fix. A row that
            // was simply greyed out would leave somebody toggling it and
            // wondering what they had done wrong.
            None => format!(
                "Not available: MangoHUD's Vulkan layer is not installed on this machine, so \
                 turning this on would do nothing.\n{}",
                crate::launch::mangohud_install_hint()
            ),
        })
        // A stale `true` in shell.json must not read as on when the layer has
        // since been uninstalled, because at launch it would not be on.
        .active(config.borrow().mangohud && layer.is_some())
        .sensitive(layer.is_some())
        .build();
    mangohud.set_subtitle_lines(4);
    {
        let config = config.clone();
        let config_path = config_path.clone();
        mangohud.connect_active_notify(move |row| {
            config.borrow_mut().mangohud = row.is_active();
            persist(&config, &config_path);
        });
    }
    group.add(&mangohud);

    group
}

/// "General" — ordinary client settings.
///
/// Resolution and monitor placement are genuine `cordial-run` options and are
/// still not offered here: `CORDIAL_MONITOR` and `CORDIAL_FULLSCREEN` are read
/// only by the X11 backend and do nothing on the Wayland one the launcher asks
/// for, so a row for either would be a control that changes nothing — which is
/// exactly what the Renderer row turned out to be until this change.
fn build_general_page(
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("General")
        .name("general")
        .icon_name("preferences-other-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Graphics")
        .description(
            "Which backend the client offers the engine. Needs a relaunch: the choice is \
             settled before Roblox loads its renderer.",
        )
        .build();

    // **Not backed by `FStringDebugGraphicsPreferredBackend` any more.** That
    // flag was measured on 2026-08-03 and changes nothing: deliberate rubbish,
    // five GLES spellings and the "confirmed" `"Vulkan"` all produced the same
    // `pack vulkan_mobile`, because Vulkan is what the engine takes regardless.
    // The row it backed looked like a setting and was one only by coincidence.
    //
    // What decides the backend is whether Cordial offers a Vulkan loader at all,
    // since `libroblox.so` links none and `dlopen`s it. So this writes
    // `shell.json` and `launch.rs` passes `CORDIAL_GRAPHICS`; the runtime's
    // `graphics` module has the full measurement.
    let model = gtk::StringList::new(&["Automatic", "Vulkan", "OpenGL ES"]);
    let selected = match config.borrow().graphics.as_str() {
        "vulkan" => 1,
        "gles" => 2,
        _ => 0,
    };
    let row = adw::ComboRow::builder()
        .title("Renderer")
        .subtitle(
            "Automatic lets Roblox take Vulkan and fall back to GLES3 itself, and is the only \
             setting a plugin may override. Vulkan keeps that fallback. OpenGL ES withholds \
             Vulkan, and has no fallback of its own.",
        )
        .model(&model)
        .selected(selected)
        .build();
    row.set_subtitle_lines(4);

    {
        let config = config.clone();
        let config_path = config_path.clone();
        row.connect_selected_notify(move |row| {
            let value = match row.selected() {
                1 => "vulkan",
                2 => "gles",
                _ => "automatic",
            };
            config.borrow_mut().graphics = value.to_string();
            persist(&config, &config_path);
        });
    }

    group.add(&row);
    page.add(&group);
    page.add(&build_performance_group(config, config_path));
    page
}

/// "Plugins" — what is installed, and what this profile has granted it.
///
/// Read from `manifest::discover` and from the selected profile's own grants
/// file, both at the moment the window is built. Grants stopped being global in
/// ADR-013 precisely because an approval given in a throwaway profile was
/// silently in force in the profile someone plays on, so a page that showed one
/// set for all profiles would be displaying the bug that change removed.
///
/// **The switch writes to disk, and is not the grants file.** It used to write
/// to an in-memory list nothing ever read, so pressing it changed nothing
/// anywhere — a control that reports success and does not act, which is the
/// shape AGENTS.md rules out for stubs and is no better in an interface. It now
/// goes through `cordial_plugins::enablement`, which keeps the answer in the
/// profile beside the grants and deliberately apart from them: disabling must
/// not revoke, or turning something back on would cost every approval already
/// given, and the cheaper response to that price is to leave a suspect plugin
/// enabled.
///
/// **It takes effect at the next launch, and the page says so.** Nothing here
/// can stop a plugin that is already running: the plugin host is not wired to a
/// running client at all yet, and the shell is a separate process from the
/// instance in any case. A toggle that looked immediate and was not is exactly
/// the small lie this project keeps writing ADRs about, so the wording is the
/// weaker claim.
/// A short, user-facing sentence for one capability — a checkbox needs prose,
/// and `cordial_plugins::capability::Capability`'s own doc comments are the
/// protocol's vocabulary, not this window's. Kept here rather than in that
/// crate for the same reason `capability_summary` is: this is a shell
/// concern, not a protocol one.
fn capability_description(cap: Capability) -> &'static str {
    match cap {
        Capability::FlagsRead => "Read which FastFlag overrides are in effect, and where each came from.",
        Capability::FlagsWrite => "Contribute FastFlag overrides that take effect at the next launch.",
        Capability::FlagsWriteDynamic => {
            "Change a DFFlag/DFInt/DFString while the client is running. Not available yet — \
             nothing in Cordial writes into the running engine."
        }
        Capability::Log => "Write log lines into Cordial's own output.",
        Capability::LifecycleRead => "Observe client lifecycle events — launch, ready, shutdown.",
        Capability::PresenceSet => {
            "Publish Discord Rich Presence. Cordial holds the connection to Discord; this only \
             sends what to show."
        }
        Capability::NotifySend => "Post a desktop notification.",
        Capability::UrlOpen => "Open an http(s) link in your browser.",
        Capability::AssetsOverride => {
            "Overlay files in place of Roblox's own assets — textures, sounds, fonts. Never \
             modifies the APK; removing the plugin restores the original."
        }
        Capability::SettingsRead => "Read the settings document Cordial keeps for it.",
        Capability::SettingsWrite => "Replace the settings document Cordial keeps for it.",
        Capability::EventsDeclare => "Register its own event types, for other plugins to hear.",
        Capability::EventsPublish => "Broadcast on an event type it has declared.",
        Capability::EventsSubscribe => "Receive events, including ones other plugins declared.",
    }
}

/// Recompute and set `expander`'s subtitle from what is actually on disk,
/// rather than from whatever a closure happened to capture when the row was
/// built. Both the enable switch and every capability switch below call this
/// after they write, so the summary line never lags behind the file it
/// describes — the same reasoning the single switch this replaced already
/// had, extended to cover more than one control changing the same plugin.
fn refresh_plugin_subtitle(expander: &adw::ExpanderRow, plugin: &Plugin, profile_dir: Option<&PathBuf>, enabled: bool) {
    let granted = profile_dir.map(|dir| grants::load(&grants::path_in(dir))).unwrap_or_default();
    expander.set_subtitle(&capability_summary(plugin, granted.get(&plugin.manifest.id), enabled));
}

/// One installed plugin's row: enable switch, one capability switch per
/// capability it requested, and a Remove button in the header. Factored out
/// of `build_plugins_page` so the exact same row — grants wiring, remove
/// wiring, everything — is what a freshly installed plugin gets too, rather
/// than a second, drifting copy of this logic living in the install button's
/// callback.
fn build_plugin_row(
    parent: &impl IsA<gtk::Window>,
    plugin: &Plugin,
    profile_dir: Option<&PathBuf>,
    root: &Path,
    group: &adw::PreferencesGroup,
) -> adw::ExpanderRow {
    let title = if plugin.manifest.name.is_empty() {
        plugin.manifest.id.clone()
    } else {
        plugin.manifest.name.clone()
    };
    let id = plugin.manifest.id.clone();
    let enabled = profile_dir.is_none_or(|dir| enablement::is_enabled(dir, &id));

    let expander = adw::ExpanderRow::builder()
        .title(title.clone())
        .subtitle(capability_summary(plugin, profile_dir.map(|dir| grants::load(&grants::path_in(dir))).unwrap_or_default().get(&id), enabled))
        .build();
    expander.set_subtitle_lines(4);

    // A freshly installed plugin has never been granted anything (ADR-003's
    // default deny), so this button is reachable — and needed — the moment
    // the row appears, not only after a page reload.
    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.set_valign(gtk::Align::Center);
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove this plugin"));
    expander.add_suffix(&remove);
    {
        let window = parent.as_ref().clone();
        let root = root.to_path_buf();
        let id = id.clone();
        let title = title.clone();
        let group = group.clone();
        let expander = expander.clone();
        remove.connect_clicked(move |_| {
            let dialog = adw::MessageDialog::builder()
                .transient_for(&window)
                .modal(true)
                .heading(format!("Remove {title}?"))
                .body(
                    "This deletes the plugin's installed files. Its saved settings and the \
                     capabilities you granted it in this profile are kept — reinstalling the \
                     same plugin later finds them again (ADR-013).",
                )
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("remove", "Remove");
            dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let root = root.clone();
            let id = id.clone();
            let group = group.clone();
            let expander = expander.clone();
            dialog.connect_response(None, move |dialog, response| {
                if response == "remove" {
                    match unpack::uninstall(&root, &id) {
                        Ok(()) => group.remove(&expander),
                        Err(e) => eprintln!("shell: could not remove plugin {id}: {e}"),
                    }
                }
                dialog.close();
            });
            dialog.present();
        });
    }

    // The switch that used to be the whole row is now one child row among
    // several, so switching a plugin off sits beside — and does not
    // disturb — the capability switches it does not revoke.
    let enable_row = adw::SwitchRow::builder().title("Enabled").active(enabled).build();
    {
        let plugin = plugin.clone();
        let dir = profile_dir.cloned();
        let id = id.clone();
        let expander = expander.clone();
        enable_row.connect_active_notify(move |row| {
            let on = row.is_active();
            if let Some(dir) = &dir {
                if let Err(e) = enablement::set_enabled(dir, &id, on) {
                    // Reported rather than swallowed, and the row is put back
                    // to what is actually on disk: a switch that stays where
                    // the user left it while the file says otherwise is a lie
                    // the user has no way to see.
                    eprintln!("shell: could not record that {id} is {}: {e}", if on { "on" } else { "off" });
                    row.set_active(!on);
                    return;
                }
            }
            refresh_plugin_subtitle(&expander, &plugin, dir.as_ref(), on);
        });
    }
    expander.add_row(&enable_row);

    // One switch per capability the plugin actually requested — offering
    // one for something it never asked for would be a control with nothing
    // to grant.
    if plugin.requested.is_empty() {
        expander.add_row(&adw::ActionRow::builder().title("Requests no capabilities").build());
    } else if let Some(dir) = profile_dir {
        let granted_here = grants::load(&grants::path_in(dir)).get(&id).cloned().unwrap_or_default();
        for &cap in &plugin.requested {
            let cap_row = adw::SwitchRow::builder()
                .title(cap.name())
                .subtitle(capability_description(cap))
                .active(granted_here.contains(&cap))
                .build();
            cap_row.set_subtitle_lines(2);
            expander.add_row(&cap_row);

            let dir = dir.clone();
            let id = id.clone();
            let plugin = plugin.clone();
            let expander = expander.clone();
            let enable_row = enable_row.clone();
            cap_row.connect_active_notify(move |row| {
                let on = row.is_active();
                if let Err(e) = grants::set(&grants::path_in(&dir), &id, cap, on) {
                    eprintln!("shell: could not record {id}'s {} grant: {e}", cap.name());
                    row.set_active(!on);
                    return;
                }
                refresh_plugin_subtitle(&expander, &plugin, Some(&dir), enable_row.is_active());
            });
        }
    } else {
        // No resolvable profile directory: there is nowhere a grant could be
        // written, so say that rather than offering switches that could not
        // persist anything a user did with them.
        expander.add_row(
            &adw::ActionRow::builder()
                .title("No profile to grant against")
                .subtitle("This profile's directory could not be resolved.")
                .build(),
        );
    }

    expander
}

/// "Install a plugin" — a `.tar.zst` picked from disk, unpacked the same
/// hardened way an index install would be (ADR-014), with no index involved.
///
/// This is the piece that used to be missing entirely: `cordial-plugins`
/// shipped a fully tested unpacker (`unpack::install_local`) that nothing in
/// the shell ever called, so "installing a plugin" meant unpacking a `tar`
/// archive by hand into `~/.local/share/cordial/plugins/` — which is not a
/// user-facing installer, it is the absence of one. Fetching from a remote
/// index — the marketplace half of ADR-014 — still needs a network fetcher
/// this change does not add; this is the half that was reachable without one.
fn build_install_group(
    parent: &impl IsA<gtk::Window>,
    root: &Path,
    profile_dir: Option<PathBuf>,
    installed_group: &adw::PreferencesGroup,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Install a plugin")
        .description(
            "An archive with plugin.json at its root — the format \
             `tar --zstd -cf name-1.0.0.tar.zst -C plugins/name .` produces. Unpacking refuses a \
             symlink, a path that escapes the plugin's own directory, and an oversized or \
             over-large archive, the same checks an index install goes through (ADR-014). \
             Installing adds the code only; it grants nothing on its own.",
        )
        .build();

    let row = adw::ActionRow::builder()
        .title("Choose an archive…")
        .subtitle("Nothing is installed until you pick a file.")
        .build();
    row.set_subtitle_lines(3);
    let button = gtk::Button::with_label("Choose file…");
    button.set_valign(gtk::Align::Center);
    row.add_suffix(&button);

    let window = parent.as_ref().clone();
    let root = root.to_path_buf();
    let installed_group = installed_group.clone();
    let row_for_status = row.clone();
    button.connect_clicked(move |_| {
        let picker_window = window.clone();
        let window = window.clone();
        let root = root.clone();
        let profile_dir = profile_dir.clone();
        let installed_group = installed_group.clone();
        let row = row_for_status.clone();
        choose_file(&picker_window, "Choose a plugin archive (.tar.zst)", false, move |path| {
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    row.set_subtitle(&format!("Could not read {}: {e}", path.display()));
                    return;
                }
            };
            match unpack::install_local(&bytes, &root) {
                Ok((plugin, _dir)) => {
                    let name = if plugin.manifest.name.is_empty() {
                        plugin.manifest.id.clone()
                    } else {
                        plugin.manifest.name.clone()
                    };
                    let requests = if plugin.requested.is_empty() {
                        "no capabilities".to_string()
                    } else {
                        plugin.requested.iter().map(|c| c.name()).collect::<Vec<_>>().join(", ")
                    };
                    row.set_subtitle(&format!(
                        "Installed {name} ({}). It requests {requests} and is granted nothing \
                         yet — approve what it may do below.",
                        plugin.manifest.id
                    ));
                    let new_row =
                        build_plugin_row(&window, &plugin, profile_dir.as_ref(), &root, &installed_group);
                    installed_group.add(&new_row);
                }
                Err(e) => row.set_subtitle(&format!("Could not install {}: {e}", path.display())),
            }
        });
    });

    group.add(&row);
    group
}

fn build_plugins_page(parent: &impl IsA<gtk::Window>, config: Rc<RefCell<ShellConfig>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Plugins")
        .name("plugins")
        .icon_name("application-x-addon-symbolic")
        .build();

    let root = manifest::plugin_root();
    let plugins = manifest::discover(&root);

    let profile_name = config.borrow().profile.clone();
    // A profile whose name will not resolve has no grants and no enablement
    // file to read, and the page still has to render. `None` here means every
    // row shows "granted nothing", which is what such a profile would in fact
    // give a plugin.
    let profile_dir = cordial_shell::profile::dir(&profile_name).ok();

    let group = adw::PreferencesGroup::builder()
        .title("Installed plugins")
        .description(format!(
            "Enablement and grants both belong to the profile rather than to the machine \
             (ADR-013); these are {profile_name}'s. Expand a plugin to approve or withdraw the \
             capabilities it has asked for — nothing here is granted until you switch it on. \
             Changes take effect the next time the client starts."
        ))
        .build();

    if plugins.is_empty() {
        group.add(
            &adw::ActionRow::builder()
                .title("No plugins installed")
                .subtitle(format!("Cordial looks in\n{}", root.display()))
                .build(),
        );
    } else {
        for plugin in &plugins {
            let row = build_plugin_row(parent, plugin, profile_dir.as_ref(), &root, &group);
            group.add(&row);
        }
    }

    page.add(&build_install_group(parent, &root, profile_dir, &group));
    page.add(&group);
    page
}

/// Builds the `AdwPreferencesWindow`: Appearance, General and Plugins, one
/// `AdwPreferencesPage` each.
pub fn build_preferences_window(
    parent: &impl IsA<gtk::Window>,
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .title("Settings")
        .search_enabled(false)
        // Taller than libadwaita's default, which was chosen for the Roblox
        // page's two rows and cut the Updates page off mid-switch: its warning
        // row appears exactly when both download switches are off, and a
        // warning below the fold is a warning nobody is shown. Only a default —
        // the window resizes, and GTK clamps this to the work area on a screen
        // that cannot hold it.
        .default_width(640)
        .default_height(720)
        .build();

    // First, because it is the one that has to be right before anything else
    // in this window matters: without a build there is nothing to launch, and
    // a renderer preference for a client that cannot start is decoration.
    window.add(&build_roblox_page(&window, config.clone(), config_path.clone()));
    // Second, next to the page about the build it is about.
    window.add(&crate::updater::build_update_page(config.clone(), config_path.clone()));
    window.add(&build_appearance_page(config.clone(), config_path.clone()));
    window.add(&build_general_page(config.clone(), config_path));
    window.add(&build_plugins_page(&window, config));

    window
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest built for this test and nowhere near the UI. The distinction
    /// matters here more than it usually would: a fixture that could reach the
    /// plugins page is how that page came to be listing software nobody had
    /// installed, requesting capabilities nobody had granted.
    fn fixture(json: &str) -> Plugin {
        manifest::parse(json, std::path::Path::new("/fixture")).expect("well-formed test manifest")
    }

    fn granted(names: &[&str]) -> BTreeSet<Capability> {
        names.iter().map(|n| Capability::parse(n).expect("a real capability name")).collect()
    }

    #[test]
    fn a_plugin_with_nothing_approved_is_not_described_as_off() {
        // The state the owner asked not to be collapsed into "off": installed,
        // switched on, and inert because ADR-003 starts everything at nothing.
        // A user in this state has to be told to approve something, not to
        // toggle a switch that is already where they want it.
        let plugin = fixture(r#"{"id":"p","name":"P","entry":"main.ts","capabilities":["flags.read"]}"#);
        let summary = capability_summary(&plugin, None, true);
        assert!(summary.contains("Granted: nothing"), "{summary}");
        assert!(summary.contains("nothing is approved yet"), "{summary}");
        assert!(!summary.contains("Off"), "an unapproved plugin must not read as switched off: {summary}");
    }

    #[test]
    fn a_disabled_plugin_still_shows_the_grants_it_keeps() {
        // Turning something off must not look like it revoked anything,
        // because it does not: that is the whole reason enablement is a
        // separate file from grants.
        let plugin = fixture(r#"{"id":"p","name":"P","entry":"main.ts","capabilities":["flags.read"]}"#);
        let summary = capability_summary(&plugin, Some(&granted(&["flags.read"])), false);
        assert!(summary.contains("Granted: flags.read"), "{summary}");
        assert!(summary.contains("costs no approvals"), "{summary}");
    }

    #[test]
    fn a_requested_capability_that_was_not_granted_is_not_shown_as_granted() {
        // The security-relevant half. Requested and granted are different
        // lists, and a summary that blurred them would say a plugin may do
        // something nobody approved.
        let plugin =
            fixture(r#"{"id":"p","name":"P","entry":"main.ts","capabilities":["flags.read","log"]}"#);
        let summary = capability_summary(&plugin, Some(&granted(&["log"])), true);
        assert!(summary.contains("Requests: flags.read, log"), "{summary}");
        assert!(summary.contains("Granted: log"), "{summary}");
        assert!(!summary.contains("Granted: flags.read"), "{summary}");
    }

    #[test]
    fn every_capability_has_a_description_a_user_could_read() {
        // `capability_description` is matched exhaustively over
        // `Capability`, so the compiler already refuses a variant added there
        // and missed here — this is the other half: that no arm quietly
        // returns an empty or placeholder string a real checkbox would show
        // blank.
        for cap in Capability::all() {
            let text = capability_description(*cap);
            assert!(!text.trim().is_empty(), "{cap} has no description");
            assert!(text.len() > 10, "{cap}'s description is too short to be real prose: {text:?}");
        }
    }
}
