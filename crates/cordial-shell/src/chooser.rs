//! The chooser — ADR-002's T1 surface, painted before the plugin host is up.
//!
//! It has to be real `AdwPreferencesGroup`/`AdwActionRow` widgets rather than
//! hand-drawn boxes, for the same reason the rest of this crate is libadwaita:
//! this is the first thing anyone launching Cordial sees, and getting the
//! chrome (row hover states, focus rings, spacing against the header bar) to
//! merely *look* like libadwaita by hand would be more work than using it.
//!
//! **One entry, and it works.** There used to be two, and the second — Roblox
//! Studio — did nothing when pressed, because ADR-002 puts Studio out of scope
//! for this runtime. A row that looks live and is not is the interface version
//! of a stub returning success, so it is gone rather than disabled. The
//! `EntrySource` seam below stays: it is where plugin-contributed entries will
//! arrive over `cap:core.launcher.register` once there is a plugin host to ask,
//! and core still resolves and launches the target itself.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

/// One entry in the chooser.
///
/// The shape both sides will agree on once entries arrive from the plugin host
/// (ADR-002, "the plugin declares *what*, core decides *how*"). It does not
/// know or care where it came from.
pub struct Entry {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon_name: Option<String>,
}

/// Where the chooser gets its entries.
pub trait EntrySource {
    fn entries(&self) -> Vec<Entry>;
}

/// The one thing this shell knows how to start.
pub const ROBLOX: &str = "roblox";

/// Core's own entry: the Roblox client, launched by `launch::spawn`.
pub struct CordialSource;

impl EntrySource for CordialSource {
    fn entries(&self) -> Vec<Entry> {
        vec![Entry {
            id: ROBLOX.into(),
            title: "Roblox".into(),
            subtitle: Some("Start the client".into()),
            icon_name: Some("applications-games-symbolic".into()),
        }]
    }
}

/// Builds the chooser: an `AdwStatusPage` when there is nothing to launch,
/// otherwise an `AdwPreferencesGroup` of action rows, one per entry, clamped
/// to a readable width rather than stretched edge to edge.
///
/// `on_activate` is ADR-002's T2 — the user picking an entry. It is handed the
/// entry's id and nothing else; deciding what that id means, and refusing it if
/// it means nothing, is core's job and lives in `window.rs`.
pub fn build(source: &dyn EntrySource, on_activate: impl Fn(&str) + Clone + 'static) -> gtk::Widget {
    let entries = source.entries();

    // Reachable from any source that returns nothing, which is what a
    // plugin-contributed set will be before a plugin host exists. There was an
    // `EmptySource` here to exercise it, whose comment claimed it was covered by
    // tests; this file has no tests, so it was an unused stand-in with a comment
    // that lied about it, and both are gone.
    if entries.is_empty() {
        let status = adw::StatusPage::builder()
            .icon_name("applications-games-symbolic")
            .title("Nothing to launch yet")
            .description("Plugins that contribute launcher entries will appear here.")
            .vexpand(true)
            .build();
        return status.upcast();
    }

    let group = adw::PreferencesGroup::builder().title("Launch").build();

    for entry in entries {
        let row = adw::ActionRow::builder().title(entry.title).activatable(true).build();
        if let Some(subtitle) = &entry.subtitle {
            row.set_subtitle(subtitle);
        }
        if let Some(icon_name) = &entry.icon_name {
            let image = gtk::Image::from_icon_name(icon_name);
            image.set_icon_size(gtk::IconSize::Large);
            row.add_prefix(&image);
        }
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

        let id = entry.id.clone();
        let on_activate = on_activate.clone();
        row.connect_activated(move |_| on_activate(&id));

        group.add(&row);
    }

    let clamp = adw::Clamp::builder().maximum_size(480).child(&group).build();
    let scroller = gtk::ScrolledWindow::builder().child(&clamp).vexpand(true).build();
    scroller.upcast()
}
