//! The minimal settings fallback ADR-002 requires.
//!
//! Not the settings surface — the UI plugin owns that once it takes over at
//! T3. This exists purely for the recoverability argument ADR-002 makes: if
//! the UI plugin is broken (bad update, protocol mismatch after a Cordial
//! upgrade), the user still needs a way to see what is installed and turn
//! the offending one off, plus enough of Cordial's own appearance and
//! graphics preference to be usable, without a terminal.
//!
//! `AdwPreferencesWindow` with several `AdwPreferencesPage`s — Roblox,
//! Appearance, General, Plugins — each becomes its own tab/sidebar entry for
//! free; that is libadwaita's own page-switcher, not something built here.
//!
//! The Roblox page is newer than the argument above and is here for a different
//! reason: nothing in Cordial used to record where a Roblox build lived, so the
//! chooser had no target and the only way to run the client was a hand-typed
//! command line. That is configuration rather than a fallback, and it belongs
//! in the shell because the shell is what has to work when nothing else does.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use cordial_plugins::capability::Capability;
use cordial_plugins::manifest::{self, Plugin};

use crate::flags_file::{self, RENDERER_BACKEND_FLAG};
use crate::install;
use crate::shell_config::{self, AppearanceScheme, ShellConfig};

/// A plugin as this settings surface cares about it.
///
/// `cordial_plugins::manifest::Plugin` is what a plugin *requests*; whether
/// it is currently enabled is a registry concern, not something the manifest
/// carries, so it lives here rather than being added to a crate we were told
/// not to touch.
pub struct PluginState {
    pub plugin: Plugin,
    pub enabled: bool,
}

/// Where the settings window gets its plugin list, and where a toggle lands.
///
/// The real implementation reads `manifest::discover(manifest::plugin_root())`
/// for the list and, for `set_enabled(_, false)`, most likely clears that
/// plugin's entry from `grants::path()` — a plugin holding no granted
/// capabilities is already inert, which sidesteps needing a second on/off
/// concept alongside the grants file. `DemoRegistry` below is a placeholder
/// standing in for that until the plugin host exists to ask.
pub trait PluginRegistry {
    fn list(&self) -> Vec<PluginState>;
    fn set_enabled(&self, id: &str, enabled: bool);
}

/// In-memory stand-in for the plugin registry.
///
/// The manifests are real `cordial_plugins::manifest::parse` output, not
/// hand-built structs, so this exercises the actual parser and capability
/// names rather than a shape that merely looks similar.
pub struct DemoRegistry {
    plugins: RefCell<Vec<PluginState>>,
}

impl DemoRegistry {
    pub fn installed() -> Rc<Self> {
        // Not a real plugin directory — nothing here is ever read from disk —
        // but manifest::parse still wants a `dir` to resolve `entry` against.
        let dir = std::path::PathBuf::from("/nonexistent/demo-plugin-dir");
        let plugin = |json: &str| -> Plugin {
            manifest::parse(json, &dir).expect("demo manifest is well-formed; covered by cordial-plugins' own tests")
        };
        let plugins = vec![
            PluginState {
                plugin: plugin(
                    r#"{"id":"presence-rpc","name":"Discord Presence","entry":"main.ts","capabilities":["lifecycle.read","presence.set"]}"#,
                ),
                enabled: true,
            },
            PluginState {
                plugin: plugin(
                    r#"{"id":"fps-tweaks","name":"FPS Tweaks","entry":"main.ts","capabilities":["flags.read","flags.write"]}"#,
                ),
                enabled: true,
            },
            PluginState {
                plugin: plugin(
                    r#"{"id":"asset-overlay-demo","name":"Asset Overlay Demo","entry":"main.ts","capabilities":["assets.override"]}"#,
                ),
                enabled: false,
            },
        ];
        Rc::new(Self { plugins: RefCell::new(plugins) })
    }
}

impl PluginRegistry for DemoRegistry {
    fn list(&self) -> Vec<PluginState> {
        self.plugins
            .borrow()
            .iter()
            .map(|s| PluginState { plugin: s.plugin.clone(), enabled: s.enabled })
            .collect()
    }

    fn set_enabled(&self, id: &str, enabled: bool) {
        if let Some(state) = self.plugins.borrow_mut().iter_mut().find(|s| s.plugin.manifest.id == id) {
            state.enabled = enabled;
        }
    }
}

fn capability_summary(plugin: &Plugin) -> String {
    if plugin.requested.is_empty() {
        return "Requests no capabilities".to_string();
    }
    let names: Vec<&str> = plugin.requested.iter().copied().map(Capability::name).collect();
    format!("Requests: {}", names.join(", "))
}

/// "Appearance" — Cordial's own theme preference, independent of anything
/// else on the desktop. `AppearanceScheme::apply` is what makes this take
/// effect immediately; this function only wires the row to it and to disk.
fn build_appearance_page(config: Rc<RefCell<ShellConfig>>, config_path: Rc<PathBuf>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Appearance")
        .icon_name("applications-graphics-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description(
            "This is Cordial's own preference, not the desktop's. System follows \
             org.freedesktop.appearance and updates live; Light and Dark stay fixed \
             regardless of what the desktop is doing.",
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
        adw::PreferencesPage::builder().title("Roblox").icon_name("applications-games-symbolic").build();

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
    let lib = config.borrow().roblox.lib_dir.clone().map(|p| (p, "Chosen in Settings".to_string())).or_else(
        || {
            let cache = install::engine_cache();
            cache
                .join(install::LIBRARY)
                .is_file()
                .then(|| (cache, "Extracted by Cordial from the APK".to_string()))
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
fn choose_file(
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

/// "General" — ordinary client settings. Only the renderer preference is
/// wired to anything real; see `flags_file.rs` for why resolution and
/// monitor placement are not offered here despite being genuine
/// `cordial-run` options in their own right.
fn build_general_page(flags_path: Rc<PathBuf>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title("General").icon_name("preferences-other-symbolic").build();

    let group = adw::PreferencesGroup::builder()
        .title("Graphics")
        .description(
            "Written to the flags.json cordial-runtime already treats as your own \
             overrides — always wins over a plugin's, and needs a relaunch: FString \
             flags are read once at startup.",
        )
        .build();

    let current = flags_file::read_string_flag(&flags_path, RENDERER_BACKEND_FLAG);
    let selected = if current.as_deref() == Some("Vulkan") { 1 } else { 0 };

    let model = gtk::StringList::new(&["Automatic", "Vulkan"]);
    let row = adw::ComboRow::builder()
        .title("Renderer")
        .subtitle("Automatic lets Roblox dlopen Vulkan and fall back to GLES2 itself")
        .model(&model)
        .selected(selected)
        .build();

    row.connect_selected_notify(move |row| {
        let value = if row.selected() == 1 { Some("Vulkan") } else { None };
        if let Err(e) = flags_file::set_string_flag(&flags_path, RENDERER_BACKEND_FLAG, value) {
            eprintln!("shell: could not update {}: {e}", flags_path.display());
        }
    });

    group.add(&row);
    page.add(&group);
    page
}

/// "Plugins" — the escape hatch ADR-002 actually asks for.
fn build_plugins_page(registry: Rc<dyn PluginRegistry>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Plugins")
        .icon_name("application-x-addon-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Installed plugins")
        .description("Turning a plugin off here is the escape hatch ADR-002 asks for: it stays installed, it just stops running.")
        .build();

    for state in registry.list() {
        let title = if state.plugin.manifest.name.is_empty() {
            state.plugin.manifest.id.clone()
        } else {
            state.plugin.manifest.name.clone()
        };

        let row = adw::SwitchRow::builder()
            .title(title)
            .subtitle(capability_summary(&state.plugin))
            .active(state.enabled)
            .build();

        let registry = registry.clone();
        let id = state.plugin.manifest.id.clone();
        row.connect_active_notify(move |row| {
            registry.set_enabled(&id, row.is_active());
        });

        group.add(&row);
    }

    page.add(&group);
    page
}

/// Builds the `AdwPreferencesWindow`: Appearance, General and Plugins, one
/// `AdwPreferencesPage` each.
pub fn build_preferences_window(
    parent: &impl IsA<gtk::Window>,
    registry: Rc<dyn PluginRegistry>,
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
    flags_path: Rc<PathBuf>,
) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .title("Settings")
        .search_enabled(false)
        .build();

    // First, because it is the one that has to be right before anything else
    // in this window matters: without a build there is nothing to launch, and
    // a renderer preference for a client that cannot start is decoration.
    window.add(&build_roblox_page(&window, config.clone(), config_path.clone()));
    window.add(&build_appearance_page(config, config_path));
    window.add(&build_general_page(flags_path));
    window.add(&build_plugins_page(registry));

    window
}
