//! The Version page: which Roblox build this profile runs, and which builds
//! Cordial is keeping.
//!
//! ADR-033's picker. The store and the pin already existed and a pin could only
//! be set by writing a file by hand, which is not a rollback anybody a Roblox
//! update has just broken is going to find.
//!
//! One list, one row per build, each carrying its own actions: a tick on the one
//! this profile uses, play to use it and launch now, and remove. It was first
//! built as a radio group with a second "Kept builds" group repeating every
//! version beside a Remove button, which put the same build in two places and
//! the two things anybody does with it on different parts of the page.
//!
//! The page offers what the store holds and nothing else. Listing older versions
//! from the mirror is ADR-033's open question and is deliberately not answered
//! here: an index nobody publishes as an interface is a new way for Settings to
//! be slow or wrong, and a list of what is on disk is honest and works offline.

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use cordial_shell::profile;
use cordial_update::store::{self, Entry};

use crate::install;
use crate::shell_config::ShellConfig;

/// Said above the choice, not after a failure, because the failure it pre-empts
/// looks like Cordial breaking: Roblox refuses clients older than a minimum it
/// sets server-side, whenever it chooses, and every pin ends there eventually.
const SERVER_MINIMUM: &str = "Roblox stops accepting old versions whenever it chooses. \
     A pinned build that starts refusing to join games has been retired by Roblox, not broken by Cordial.";

/// What a row says about one build, beneath its version number.
///
/// `loaded_by` is compared against this Cordial because that is the claim the
/// record supports: the engine got through `dlopen` with this shim. A different
/// Cordial having loaded it says less, and no record at all is not a sign of
/// trouble -- every build kept before the record existed has none -- so it is
/// worded as unknown rather than as a warning.
pub fn describe(entry: &Entry, current: Option<&str>, this_cordial: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if current == Some(entry.version.as_str()) {
        parts.push("Current build".into());
    }
    match entry.loaded_by.as_deref() {
        Some(v) if v == this_cordial => parts.push("Loaded by this Cordial".into()),
        Some(v) => parts.push(format!("Last loaded by Cordial {v}")),
        None => parts.push("Not loaded by Cordial yet".into()),
    }
    if !entry.complete {
        parts.push("kept without its APK, so it cannot be chosen".into());
    }
    parts.push(profile::human_bytes(entry.bytes));
    parts.join(" · ")
}

/// Why a remove button is greyed out, or `None` when it is not.
pub fn removal_blocked(version: &str, current: Option<&str>, pinned_anywhere: &[String]) -> Option<&'static str> {
    if current == Some(version) {
        Some("The current build cannot be removed")
    } else if pinned_anywhere.iter().any(|p| p == version) {
        Some("A profile is pinned to this build")
    } else {
        None
    }
}

/// The row's subtitle, led by the fact that matters most when it is true.
fn subtitle(active: bool, detail: &str) -> String {
    if active {
        format!("Active — used on launch · {detail}")
    } else {
        detail.to_string()
    }
}

struct View {
    page: adw::PreferencesPage,
    config: Rc<RefCell<ShellConfig>>,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
}

pub fn build_version_page(config: Rc<RefCell<ShellConfig>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Version")
        // The name `open_on_start` addresses it by: `settings=version`.
        .name("version")
        // Checked on disk: `symbolic/actions/` in Adwaita.
        .icon_name("document-open-recent-symbolic")
        .build();
    let view = Rc::new(View { page: page.clone(), config, groups: RefCell::new(Vec::new()) });
    populate(&view);
    page
}

/// Later, not now: every handler that changes the store runs inside a signal
/// on a widget this would remove.
fn repopulate_soon(view: &Weak<View>) {
    let view = view.clone();
    glib::idle_add_local_once(move || {
        if let Some(view) = view.upgrade() {
            populate(&view);
        }
    });
}

/// The tick on the active row, and an invisible one of the same size on every
/// other row so the titles line up in a column rather than jumping sideways.
fn tick(active: bool) -> gtk::Image {
    let image = gtk::Image::from_icon_name("object-select-symbolic");
    image.set_opacity(if active { 1.0 } else { 0.0 });
    image
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_valign(gtk::Align::Center);
    button.add_css_class("flat");
    button.set_tooltip_text(Some(tooltip));
    button
}

/// Rebuilt from disk every time, rather than patched: the store changes under
/// this page whenever a launch or an update keys a build, and a list assembled
/// once is wrong by the next one.
fn populate(view: &Rc<View>) {
    for group in view.groups.borrow_mut().drain(..) {
        view.page.remove(&group);
    }
    let weak = Rc::downgrade(view);

    let profile_name = view.config.borrow().profile.clone();
    let profile_dir = profile::dir(&profile_name);
    let pinned = profile_dir.as_ref().ok().and_then(|d| profile::pinned_version(d));
    let root = store::root();
    let live = install::engine_cache();
    let current = store::current_in(&root, &live);
    let entries = store::list();
    let pinned_anywhere = profile::all_pinned_versions();
    let this_cordial = env!("CARGO_PKG_VERSION");

    let list = adw::PreferencesGroup::builder()
        .title(format!("Roblox version for {profile_name}"))
        .description(SERVER_MINIMUM)
        .build();

    let profile_dir = match profile_dir {
        Ok(dir) => dir,
        Err(e) => {
            let row = adw::ActionRow::builder().title("This profile cannot hold a pin").subtitle(e).build();
            list.add(&row);
            add_group(view, list);
            return;
        }
    };

    // Shown where the choice is, rather than only by the launch refusing: a
    // pin that cannot be honoured stops this profile launching at all.
    let status = adw::ActionRow::builder().title("").build();
    status.set_visible(false);
    status.set_subtitle_lines(4);

    let follow = adw::ActionRow::builder()
        .title("Follow the current build")
        .subtitle(subtitle(
            pinned.is_none(),
            &match &current {
                Some(v) => format!("Roblox {v} now, and newer builds as they are installed"),
                None => "Whichever build Cordial has, and newer builds as they are installed".into(),
            },
        ))
        .activatable(true)
        .build();
    follow.set_subtitle_lines(2);
    follow.add_prefix(&tick(pinned.is_none()));
    let play = icon_button("media-playback-start-symbolic", "Follow the current build and launch");
    follow.add_suffix(&play);
    list.add(&follow);
    connect_use(&follow, &play, &profile_dir, None, &status, &weak);

    if let Some(pin) = pinned.as_deref().filter(|p| !entries.iter().any(|e| e.version == *p)) {
        let row = adw::ActionRow::builder()
            .title(format!("Roblox {pin}"))
            .subtitle("Pinned, but Cordial no longer has this build. This profile will not launch until you choose another.")
            .build();
        row.set_subtitle_lines(3);
        row.add_prefix(&tick(true));
        list.add(&row);
    }

    for entry in &entries {
        let active = pinned.as_deref() == Some(entry.version.as_str());
        let row = adw::ActionRow::builder()
            .title(format!("Roblox {}", entry.version))
            .subtitle(subtitle(active, &describe(entry, current.as_deref(), this_cordial)))
            // Not offered rather than offered and refused: an engine without
            // its own APK would run against another version's assets.
            .activatable(entry.complete)
            .build();
        row.set_subtitle_lines(2);
        row.add_prefix(&tick(active));

        let play = icon_button("media-playback-start-symbolic", "Use this build and launch");
        play.set_sensitive(entry.complete);
        row.add_suffix(&play);

        let remove = icon_button("user-trash-symbolic", "Remove this build");
        if let Some(why) = removal_blocked(&entry.version, current.as_deref(), &pinned_anywhere) {
            remove.set_sensitive(false);
            remove.set_tooltip_text(Some(why));
        }
        let version = entry.version.clone();
        let (root, live, pinned_anywhere) = (root.clone(), live.clone(), pinned_anywhere.clone());
        let (status_for_remove, weak_for_remove) = (status.clone(), weak.clone());
        remove.connect_clicked(move |_| match store::remove_in(&root, &live, &version, &pinned_anywhere) {
            Ok(()) => repopulate_soon(&weak_for_remove),
            Err(e) => show_status(&status_for_remove, "The build was not removed", &e),
        });
        row.add_suffix(&remove);

        list.add(&row);
        if entry.complete {
            connect_use(&row, &play, &profile_dir, Some(entry.version.clone()), &status, &weak);
        }
    }
    list.add(&status);

    if entries.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No builds kept yet")
            .subtitle("Cordial keeps a build here once it knows which version it is, which happens the next time Roblox launches or updates.")
            .build();
        row.set_subtitle_lines(3);
        list.add(&row);
    }
    add_group(view, list);

    if !entries.is_empty() {
        let note = adw::PreferencesGroup::builder()
            .description(format!(
                "Cordial keeps the newest {} and any build a profile is pinned to. Removing one does not touch APKs that came from Sober.",
                store::KEEP
            ))
            .build();
        add_group(view, note);
    }
}

fn add_group(view: &Rc<View>, group: adw::PreferencesGroup) {
    view.page.add(&group);
    view.groups.borrow_mut().push(group);
}

fn show_status(status: &adw::ActionRow, title: &str, detail: &str) {
    status.set_title(title);
    status.set_subtitle(detail);
    status.set_visible(true);
}

/// Clicking the row makes it this profile's build; play does that and then
/// launches.
///
/// Play writes the pin rather than overriding one launch. ADR-033 left a
/// per-launch override out on purpose -- cheap to add, impossible to take away
/// -- so pressing play on an old build leaves the profile on it, and the tick
/// moving to that row is what says so.
fn connect_use(
    row: &adw::ActionRow,
    play: &gtk::Button,
    profile_dir: &Path,
    version: Option<String>,
    status: &adw::ActionRow,
    view: &Weak<View>,
) {
    let choose = {
        let (profile_dir, version, status, view) =
            (profile_dir.to_path_buf(), version.clone(), status.clone(), view.clone());
        move || -> bool {
            match profile::set_pinned_version(&profile_dir, version.as_deref()) {
                Ok(()) => {
                    repopulate_soon(&view);
                    true
                }
                Err(e) => {
                    show_status(&status, "The choice was not saved", &e);
                    false
                }
            }
        }
    };
    let choose = Rc::new(choose);

    let on_row = choose.clone();
    row.connect_activated(move |_| {
        on_row();
    });

    play.connect_clicked(move |button| {
        // Taken before the dialog closes: once it has, this button is no
        // longer inside the launcher window and `win.launch` cannot be found
        // from it.
        let window = button.root().and_downcast::<gtk::Window>();
        if !choose() {
            return;
        }
        if let Some(dialog) = button.ancestor(adw::Dialog::static_type()).and_downcast::<adw::Dialog>() {
            dialog.close();
        }
        if let Some(window) = window {
            let _ = window.activate_action("win.launch", None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(version: &str, loaded_by: Option<&str>, complete: bool) -> Entry {
        Entry {
            version: version.into(),
            dir: PathBuf::from("/nonexistent"),
            loaded_by: loaded_by.map(Into::into),
            bytes: 118_732_400,
            complete,
        }
    }

    #[test]
    fn a_build_nothing_has_loaded_is_unknown_rather_than_broken() {
        let line = describe(&entry("2.738.0.1397", None, true), None, "0.14.0");
        assert_eq!(line, "Not loaded by Cordial yet · 118.7 MB");
    }

    #[test]
    fn the_current_build_loaded_by_this_cordial_says_both() {
        let line = describe(&entry("2.738.0.1397", Some("0.14.0"), true), Some("2.738.0.1397"), "0.14.0");
        assert_eq!(line, "Current build · Loaded by this Cordial · 118.7 MB");
    }

    #[test]
    fn an_engine_without_its_apk_says_why_it_cannot_be_chosen() {
        let line = describe(&entry("2.734.0.917", Some("0.13.0"), false), None, "0.14.0");
        assert!(line.contains("Last loaded by Cordial 0.13.0"), "{line}");
        assert!(line.contains("cannot be chosen"), "{line}");
    }

    #[test]
    fn remove_is_withheld_from_the_current_build_and_a_pinned_one_only() {
        let pins = vec!["2.0".to_string()];
        assert!(removal_blocked("3.0", Some("3.0"), &pins).is_some());
        assert!(removal_blocked("2.0", Some("3.0"), &pins).is_some());
        assert_eq!(removal_blocked("1.0", Some("3.0"), &pins), None);
    }

    #[test]
    fn the_active_row_leads_with_being_active() {
        assert_eq!(subtitle(true, "118.7 MB"), "Active — used on launch · 118.7 MB");
        assert_eq!(subtitle(false, "118.7 MB"), "118.7 MB");
    }
}
