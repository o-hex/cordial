//! The one screen for "something went wrong": the diagnostics block, a way to
//! copy or save it, and the way to the issue tracker.
//!
//! **This used to be two screens showing the same text.** Settings had a
//! Report page with the block, a Copy button and an issue link; the About
//! dialog had its own Troubleshooting page with the block, Copy and Save. Both
//! read `diagnostics::report`, so they never disagreed about the machine, but a
//! reporter who found one did not know the other existed, and a page in
//! Settings that is not a setting was the wrong home for either.
//!
//! It is a dialog of its own rather than the About dialog's Troubleshooting
//! page because `AdwAboutDialog` has no way to open on a chosen page, and the
//! launcher's X11 notice has to land on exactly this screen. The About dialog
//! links here instead of carrying a second copy.

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

/// Where a report goes. One constant, because the row here and the About
/// dialog's link both name it and two copies drift.
pub const ISSUES_URL: &str = "https://github.com/luohoa97/cordial/issues/new/choose";

/// The file name the Save button offers. The same one the About dialog's own
/// Troubleshooting page offered, so anyone who knew it does not have to learn
/// a second.
const SAVE_NAME: &str = "cordial-diagnostics.txt";

/// Builds the report screen. Nothing is shown until it is presented.
pub fn build() -> adw::Dialog {
    let text = crate::diagnostics::report();

    let page = adw::PreferencesPage::new();

    let group = adw::PreferencesGroup::builder()
        .title("Diagnostics")
        // One line. The block says what it contains by containing it, and the
        // reasoning about what is deliberately absent lives in `diagnostics.rs`
        // next to the code that decides it.
        .description("Paste this into a GitHub issue.")
        .build();

    // Monospace and selectable: the columns only line up in a fixed-width font,
    // and somebody who wants one line rather than the block should be able to
    // take it without the button's all-or-nothing.
    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .cursor_visible(false)
        // **Wrapped, because `uname -a` is longer than any dialog.** Without it
        // the System line ran off the edge with no scrollbar, and somebody
        // checking what they were about to paste into a public issue could not
        // read the one line most likely to make them think twice. `WordChar`
        // rather than `Word`: a kernel version has no spaces to break at.
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    view.buffer().set_text(&text);
    let frame = gtk::Frame::new(None);
    frame.set_child(Some(&view));
    frame.set_margin_top(6);

    let copy = gtk::Button::with_label("Copy");
    copy.set_valign(gtk::Align::Center);
    let copied = text.clone();
    copy.connect_clicked(move |b| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&copied);
            // The label is the confirmation. A button that looks identical
            // after a press is one people press three times.
            b.set_label("Copied");
            let b = b.clone();
            glib::timeout_add_seconds_local_once(2, move || b.set_label("Copy"));
        }
    });
    let copy_row = adw::ActionRow::builder().title("Copy diagnostics").build();
    copy_row.add_suffix(&copy);
    group.add(&copy_row);

    // Kept from the About dialog's Troubleshooting page, which had it and the
    // Settings page did not: a report attached to an issue as a file survives
    // a paste box that mangles columns.
    let save = gtk::Button::with_label("Save…");
    save.set_valign(gtk::Align::Center);
    let saved = text.clone();
    save.connect_clicked(move |b| {
        let parent = b.root().and_downcast::<gtk::Window>();
        let text = saved.clone();
        gtk::FileDialog::builder()
            .title("Save diagnostics")
            .initial_name(SAVE_NAME)
            .build()
            .save(parent.as_ref(), gtk::gio::Cancellable::NONE, move |result| {
                // A dismissed picker is an error result too; only a real
                // failure to write is worth saying anything about.
                let Ok(file) = result else { return };
                if let Err(e) = file.replace_contents(
                    text.as_bytes(),
                    None,
                    false,
                    gtk::gio::FileCreateFlags::NONE,
                    gtk::gio::Cancellable::NONE,
                ) {
                    eprintln!("[cordial] could not save the diagnostics: {e}");
                }
            });
    });
    let save_row = adw::ActionRow::builder().title("Save to a file").build();
    save_row.add_suffix(&save);
    group.add(&save_row);
    group.add(&frame);
    page.add(&group);

    let where_group = adw::PreferencesGroup::builder().title("Bugs and feature requests").build();
    // A link row rather than prose with a URL in it: this is the last place
    // somebody is before they give up, and it should take one press to get from
    // here to the form.
    let issues = adw::ActionRow::builder()
        .title("Open an issue")
        .subtitle(ISSUES_URL.trim_start_matches("https://"))
        .activatable(true)
        .build();
    // `go-next-symbolic`, checked on disk rather than guessed: the first
    // attempt used `external-link-symbolic`, which is in no icon theme here and
    // rendered as the missing-image glyph.
    issues.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    // **`GtkUriLauncher`, not `cordial_plugins::urlopen`, and the difference is
    // focus.** Both reach `org.freedesktop.portal.OpenURI`, so both work inside
    // the Flatpak sandbox. But `urlopen` is the plugin path: a plugin has no
    // window, so it passes an empty parent handle and no activation token, and
    // GNOME's focus-stealing prevention answers by declining to raise the
    // browser. This row has a window to offer, and `UriLauncher::launch` hands
    // the portal what it needs to raise the browser properly.
    issues.connect_activated(move |row| {
        let parent = row.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(ISSUES_URL).launch(
            parent.as_ref(),
            gtk::gio::Cancellable::NONE,
            |result| {
                if let Err(e) = result {
                    eprintln!("[cordial] could not open the issue tracker: {e}");
                }
            },
        );
    });
    where_group.add(&issues);
    page.add(&where_group);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));

    adw::Dialog::builder()
        .title("Report a Problem")
        .content_width(520)
        .content_height(680)
        .child(&toolbar)
        .build()
}

/// Puts the report screen up over `parent`.
pub fn present(parent: &impl IsA<gtk::Widget>) {
    build().present(Some(parent));
}
