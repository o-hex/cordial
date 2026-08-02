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
//!
//! **One entry is a button; several are a list.** The single entry used to be
//! drawn the same way a list of them would be — an `AdwPreferencesGroup` titled
//! "Launch", holding one `AdwActionRow` subtitled "Start the client" with a
//! chevron after it. Three pieces of chrome around one verb, in a window whose
//! entire job is that verb, and the result read as a settings page rather than
//! a launcher. GNOME's HIG has a control for the one thing a window exists to
//! do and it is a `suggested-action` button, so that is what a lone entry
//! becomes. The list is still what more than one produces, because at that
//! point the user is choosing between them rather than confirming the only
//! option, and a row per entry is how that choice is shown.

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
            // No second line. It said "Start the client", which is what the
            // control it sits on already says and what the window is for. The
            // field stays because a plugin contributing two entries may need it
            // to tell them apart; core's own entry does not.
            subtitle: None,
            icon_name: Some("applications-games-symbolic".into()),
        }]
    }
}

/// Builds the chooser: an `AdwStatusPage` when there is nothing to launch, a
/// single `suggested-action` button when there is one thing, and an
/// `AdwPreferencesGroup` of action rows when there is a choice to make.
///
/// The caller decides where this sits and how wide it is — see `window.rs`,
/// which clamps it and the profile row together so the two line up.
///
/// `on_activate` is ADR-002's T2 — the user picking an entry. It is handed the
/// entry's id and nothing else; deciding what that id means, and refusing it if
/// it means nothing, is core's job and lives in `window.rs`.
pub fn build(source: &dyn EntrySource, on_activate: impl Fn(&str) + Clone + 'static) -> gtk::Widget {
    let mut entries = source.entries();

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

    if entries.len() == 1 {
        return button(entries.remove(0), on_activate);
    }

    // No group title. "Launch" over a list of things to launch is a header
    // restating the only thing the window does, and the rows below it already
    // name themselves.
    let group = adw::PreferencesGroup::new();

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

    group.upcast()
}

/// The lone entry, as the button the window exists for.
///
/// The entry's own icon goes inside the button rather than above it as an
/// ornament, so nothing here invents a picture: what is drawn is exactly what
/// the entry declared, and an entry with no icon simply gets a label. The
/// subtitle, if a plugin set one, becomes the tooltip — dropping it outright
/// would lose the only thing a contributed entry has to explain itself with,
/// and putting it under the label would rebuild the two-line row this replaced.
fn button(entry: Entry, on_activate: impl Fn(&str) + 'static) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_halign(gtk::Align::Center);
    if let Some(icon_name) = &entry.icon_name {
        content.append(&gtk::Image::from_icon_name(icon_name));
    }
    content.append(&gtk::Label::new(Some(&entry.title)));

    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("suggested-action");
    button.add_css_class("pill");
    if let Some(subtitle) = &entry.subtitle {
        button.set_tooltip_text(Some(subtitle));
    }

    let id = entry.id;
    button.connect_clicked(move |_| on_activate(&id));
    button.upcast()
}
