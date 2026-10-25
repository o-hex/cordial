//! The store view on Settings -> Roblox: which builds Cordial keeps, where each
//! came from, who uses it, and the way to remove one.
//!
//! [ADR-054](../../../docs/adr/ADR-054-cordial-owns-its-roblox-builds.md) makes
//! the store the only place a launch gets a build from, so the page that says
//! what Roblox is on this machine has to be a list of the store and not a path.
//! Each row says a fact read off the entry -- when it was downloaded, or that it
//! was imported from Sober on a date -- and nothing on the page says Cordial
//! depends on Sober. Imports are on the build row above this group.
//!
//! The pure functions here are what is worth pinning, and a `gtk::ListBox`
//! cannot be built without `gtk::init`, so they are separate from the widgets.

use std::cell::RefCell;
use std::rc::Rc;

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use cordial_shell::profile;
use cordial_update::store::{self, Entry};

use crate::roblox_versions::removal_blocked;

/// A profile and what it asked of the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePin {
    pub name: String,
    pub pinned: Option<String>,
}

/// Every profile and its pin, read off disk.
pub fn profile_pins() -> Vec<ProfilePin> {
    profile::list()
        .into_iter()
        .map(|name| {
            let pinned = profile::dir(&name).ok().and_then(|d| profile::pinned_version(&d));
            ProfilePin { name, pinned }
        })
        .collect()
}

/// The profiles that run `version`: pinned to it, or on Latest while it is the
/// newest build a launch may run. A profile pinned elsewhere does not use it.
pub fn used_by(version: &str, latest: Option<&str>, profiles: &[ProfilePin]) -> Vec<String> {
    profiles
        .iter()
        .filter(|p| match p.pinned.as_deref() {
            Some(pin) => pin == version,
            None => latest == Some(version),
        })
        .map(|p| p.name.clone())
        .collect()
}

/// "Used by default, alt", or the plain statement that nothing does.
pub fn used_line(users: &[String]) -> String {
    if users.is_empty() {
        "Not used by any profile".into()
    } else {
        format!("Used by {}", users.join(", "))
    }
}

/// The row's two lines: where it came from, then what it costs and who uses it.
///
/// A build kept without its archives, or one whose signer Cordial has not
/// established, says so, because it cannot be launched and a row that listed
/// it as healthy would be the stub that lies.
pub fn describe_kept(entry: &Entry, latest: Option<&str>, users: &[String], bytes: u64) -> String {
    let how = entry.provenance.map(|p| p.label()).unwrap_or_else(|| "Source not recorded".into());
    let mut facts = vec![profile::human_bytes(bytes), used_line(users)];
    if latest == Some(entry.version.as_str()) {
        facts.insert(0, "Latest".into());
    }
    if !entry.complete {
        facts.push("kept without its APK, so it cannot be run".into());
    } else if entry.signer.is_none() {
        facts.push("not checked yet; it is checked when it is first launched".into());
    }
    format!("{how}\n{}", facts.join(" · "))
}

/// The builds a clean-up would remove, for the row that offers it: the same
/// plan a filing runs, with this machine's pins and running clients.
pub fn unused_builds(entries: &[Entry], pins: &[String], running: &[String]) -> Vec<String> {
    store::gc_plan(entries, pins, running, store::SPARE)
}

/// What the clean-up row says.
pub fn cleanup_line(unused: &[String], entries: &[Entry]) -> String {
    if unused.is_empty() {
        return "Nothing to remove. The newest build, the one before it, pinned builds and running ones are kept."
            .into();
    }
    let freed: u64 = entries
        .iter()
        .filter(|e| unused.contains(&e.version))
        .map(|e| store::tree_bytes(&e.dir))
        .sum();
    format!("Removes {} and frees {}.", unused.join(", "), profile::human_bytes(freed))
}

/// The group, and the closure that redraws it from disk.
///
/// Rebuilt rather than patched: the store changes under this page whenever a
/// download, an import or an update lands, and a list assembled once is wrong by
/// the next one. The caller invokes the closure when any of those ends.
pub fn build_group(window: &gtk::Window) -> (adw::PreferencesGroup, Rc<dyn Fn()>) {
    let group = adw::PreferencesGroup::builder()
        .title("Builds Cordial keeps")
        .description(
            "Each profile runs the newest of these (Latest) or one it is pinned to on the Version \
             page. Cordial keeps the newest, the one before it, any a profile is pinned to and \
             any a client is running.",
        )
        .build();
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    let status = adw::ActionRow::builder().title("").build();
    status.set_visible(false);
    status.set_subtitle_lines(4);

    let redraw: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let refresh: Rc<dyn Fn()> = {
        let (group, rows, status, redraw) = (group.clone(), rows.clone(), status.clone(), redraw.clone());
        let window = window.clone();
        Rc::new(move || {
            for row in rows.borrow_mut().drain(..) {
                group.remove(&row);
            }
            group.remove(&status);
            let entries = store::list();
            let latest = store::latest(&entries).map(|e| e.version.clone());
            let profiles = profile_pins();
            let pinned_anywhere = profile::all_pinned_versions();
            let running: Vec<String> =
                entries.iter().filter(|e| store::is_in_use(&e.dir)).map(|e| e.version.clone()).collect();
            let root = store::root();

            if entries.is_empty() {
                let row = adw::ActionRow::builder()
                    .title("No builds yet")
                    .subtitle("Press Download Roblox above, or import one.")
                    .build();
                group.add(&row);
                rows.borrow_mut().push(row);
            }
            for entry in &entries {
                let users = used_by(&entry.version, latest.as_deref(), &profiles);
                let row = adw::ActionRow::builder()
                    .title(format!("Roblox {}", entry.version))
                    .subtitle(glib::markup_escape_text(&describe_kept(
                        entry,
                        latest.as_deref(),
                        &users,
                        store::tree_bytes(&entry.dir),
                    )))
                    .build();
                row.set_subtitle_lines(3);

                let remove = gtk::Button::from_icon_name("user-trash-symbolic");
                remove.set_valign(gtk::Align::Center);
                remove.add_css_class("flat");
                remove.set_tooltip_text(Some("Remove this build"));
                let blocked = removal_blocked(&entry.version, latest.as_deref(), &pinned_anywhere)
                    .or_else(|| running.contains(&entry.version).then_some("A client is running this build"));
                if let Some(why) = blocked {
                    remove.set_sensitive(false);
                    remove.set_tooltip_text(Some(why));
                }
                let (version, root, pins) = (entry.version.clone(), root.clone(), pinned_anywhere.clone());
                let (status, redraw) = (status.clone(), redraw.clone());
                remove.connect_clicked(move |_| match store::remove_in(&root, &version, &pins) {
                    Ok(()) => {
                        if let Some(redraw) = redraw.borrow().as_ref() {
                            let redraw = redraw.clone();
                            glib::idle_add_local_once(move || redraw());
                        }
                    }
                    Err(e) => {
                        status.set_title("The build was not removed");
                        status.set_subtitle(&glib::markup_escape_text(&e));
                        status.set_visible(true);
                    }
                });
                row.add_suffix(&remove);
                group.add(&row);
                rows.borrow_mut().push(row);
            }

            // The clean-up, only when there is something for it to do.
            let unused = unused_builds(&entries, &pinned_anywhere, &running);
            if !unused.is_empty() {
                let row = adw::ActionRow::builder()
                    .title("Remove builds nothing uses")
                    .subtitle(glib::markup_escape_text(&cleanup_line(&unused, &entries)))
                    .build();
                row.set_subtitle_lines(3);
                let clean = gtk::Button::with_label("Remove");
                clean.set_valign(gtk::Align::Center);
                clean.add_css_class("destructive-action");
                let (root, pins, redraw) = (root.clone(), pinned_anywhere.clone(), redraw.clone());
                clean.connect_clicked(move |_| {
                    store::gc_in(&root, &pins, store::SPARE);
                    if let Some(redraw) = redraw.borrow().as_ref() {
                        let redraw = redraw.clone();
                        glib::idle_add_local_once(move || redraw());
                    }
                });
                row.add_suffix(&clean);
                group.add(&row);
                rows.borrow_mut().push(row);
            }
            group.add(&status);
            let _ = &window;
        })
    };
    *redraw.borrow_mut() = Some(refresh.clone());
    refresh();
    (group, refresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(version: &str, complete: bool, signed: bool, source: Option<store::Source>) -> Entry {
        Entry {
            version: version.into(),
            dir: PathBuf::from("/x").join(version),
            loaded_by: None,
            bytes: 0,
            complete,
            content_hash: None,
            signer: signed.then(|| "ab".repeat(32)),
            provenance: source.map(|source| store::Provenance { source, at: Some(1_791_081_816) }),
            version_code: None,
        }
    }

    fn pin(name: &str, pinned: Option<&str>) -> ProfilePin {
        ProfilePin { name: name.into(), pinned: pinned.map(Into::into) }
    }

    /// A profile uses what it is pinned to, or Latest when it follows; never
    /// both, and never a build it is pinned away from.
    #[test]
    fn a_build_is_used_by_the_profiles_pinned_to_it_and_by_those_on_latest_only_if_it_is_latest() {
        let profiles = [pin("default", None), pin("alt", Some("2.736.0.1408")), pin("old", Some("2.700.0.1"))];
        assert_eq!(used_by("2.738.0.1397", Some("2.738.0.1397"), &profiles), ["default"]);
        assert_eq!(used_by("2.736.0.1408", Some("2.738.0.1397"), &profiles), ["alt"]);
        assert_eq!(used_by("2.700.0.1", Some("2.738.0.1397"), &profiles), ["old"]);
        assert!(used_by("2.730.0.1", Some("2.738.0.1397"), &profiles).is_empty());
        // No Latest at all (an empty or unusable store): followers use nothing.
        assert!(used_by("2.738.0.1397", None, &profiles).is_empty());
        assert_eq!(used_line(&["a".into(), "b".into()]), "Used by a, b");
        assert_eq!(used_line(&[]), "Not used by any profile");
    }

    #[test]
    fn a_row_says_where_the_build_came_from_and_never_that_cordial_depends_on_sober() {
        let sober = entry("2.737.0.1584", true, true, Some(store::Source::Sober));
        let line = describe_kept(&sober, Some("2.737.0.1584"), &["default".into()], 150_000_000);
        assert!(line.starts_with("Imported from Sober on 4 Oct 2026\n"), "{line}");
        assert!(line.contains("Latest") && line.contains("150.0 MB") && line.contains("Used by default"), "{line}");
        assert!(!line.contains("depend"), "{line}");

        let mirror = entry("2.738.0.1397", true, true, Some(store::Source::Mirror));
        assert!(describe_kept(&mirror, None, &[], 1).starts_with("Downloaded on 4 Oct 2026\n"));
        let file = entry("2.730.0.790", true, true, Some(store::Source::File));
        assert!(describe_kept(&file, None, &[], 1).starts_with("Imported from a file on"));
    }

    #[test]
    fn a_build_that_cannot_be_run_says_so_on_its_row() {
        let incomplete = entry("2.734.0.917", false, true, None);
        let line = describe_kept(&incomplete, None, &[], 1);
        assert!(line.starts_with("Source not recorded\n"), "{line}");
        assert!(line.contains("cannot be run"), "{line}");
        let unsigned = entry("2.735.0.1", true, false, Some(store::Source::Mirror));
        assert!(describe_kept(&unsigned, None, &[], 1).contains("checked when it is first launched"));
    }

    #[test]
    fn the_cleanup_offers_what_the_filing_would_collect_and_says_what_it_frees() {
        let entries: Vec<Entry> = ["2.742.0.9", "2.740.0.5", "2.738.0.1397", "2.700.0.1"]
            .iter()
            .map(|v| entry(v, true, true, None))
            .collect();
        let unused = unused_builds(&entries, &["2.700.0.1".into()], &[]);
        assert_eq!(unused, ["2.738.0.1397"]);
        assert!(cleanup_line(&unused, &entries).starts_with("Removes 2.738.0.1397 and frees "));
        assert!(unused_builds(&entries, &["2.700.0.1".into()], &["2.738.0.1397".into()]).is_empty());
        assert!(cleanup_line(&[], &entries).starts_with("Nothing to remove"));
    }
}
