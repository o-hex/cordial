//! "Play in VR", under the Roblox button.
//!
//! A second, quieter control rather than a split button or a menu: the main
//! button keeps its one job and its one look, and the VR entry has something
//! the main button never needs, which is a sentence underneath saying what is
//! missing. A split button's arrow would hide that sentence behind a click, and
//! an insensitive menu item cannot explain itself at all -- the rule
//! `settings::detail` states for rows, applied here: an unavailable control
//! keeps its reason on screen.
//!
//! Absent rather than greyed on a host that cannot run it (anything but
//! x86-64), because nothing the user can do would change that answer.

use std::cell::RefCell;
use std::rc::Rc;

use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use crate::shell_config::ShellConfig;
use cordial_shell::vr::Readiness;

pub struct VrEntry {
    pub widget: gtk::Widget,
    /// Re-reads the store, the runtime and WiVRn's server. Called when
    /// Settings closes and when the window comes back to the front, which are
    /// the two moments any of them is likely to have changed.
    pub refresh: Rc<dyn Fn()>,
}

pub fn build(config: Rc<RefCell<ShellConfig>>, on_play: impl Fn() + 'static) -> Option<VrEntry> {
    if !cordial_shell::vr::HOST_SUPPORTED {
        return None;
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_halign(gtk::Align::Center);
    content.append(&gtk::Image::from_icon_name("view-dual-symbolic"));
    content.append(&gtk::Label::new(Some("Play in VR")));
    let button = gtk::Button::builder()
        .child(&content)
        .css_classes(["pill"])
        .build();
    button.connect_clicked(move |_| on_play());

    let caption = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(44)
        .css_classes(["dim-label", "caption"])
        .build();
    let setup = gtk::Button::builder()
        .label("Set Up VR…")
        .halign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    setup.connect_clicked(|b| {
        let _ = b.activate_action("win.settings", Some(&"vr".to_variant()));
    });

    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(&button);
    column.append(&caption);
    column.append(&setup);

    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        // Without a Quest build nothing else can make the entry usable, so a
        // launcher that has never been set up for VR stops here: no scan for
        // OpenXR runtimes and no `flatpak info` subprocess on the GTK thread,
        // at start-up or on each return to the front.
        if cordial_update::quest::current().is_none() {
            button.set_sensitive(false);
            caption.set_label(cordial_shell::vr::NO_QUEST_BUILD);
            return;
        }
        let readiness = Readiness::gather(config.borrow().vr_openxr_runtime.as_deref());
        let missing = readiness.missing();
        button.set_sensitive(missing.is_empty());
        caption.set_label(&match missing.first() {
            Some(first) => first.clone(),
            None => readiness.summary(),
        });
    });
    refresh();
    Some(VrEntry {
        widget: column.upcast(),
        refresh,
    })
}

/// Keep `entry` current while the window is in use: on every return to the
/// front, which is when a headset may have been plugged in or WiVRn started.
pub fn follow_window(window: &impl IsA<gtk::Window>, refresh: Rc<dyn Fn()>) {
    window.connect_is_active_notify(move |w| {
        if w.is_active() {
            let refresh = refresh.clone();
            // Off the notify itself: gathering asks `/proc` and the
            // filesystem, and a property notification is not the place for it.
            glib::idle_add_local_once(move || refresh());
        }
    });
}
