//! What to do when there is no Roblox build to launch.
//!
//! **This is the only copy of this advice.** It used to live in the justfile as
//! well, and it was deleted from there deliberately: two copies of the same
//! instructions drift, and the one the user reads is this one. If the detection
//! path in `install::sober_apk` ever changes, it changes here in the same edit.
//!
//! Not an error dialog, because this is not an error. A fresh install with no
//! Roblox build is the ordinary first-run state — Cordial ships no Roblox code
//! and never will — and it has a scripted way out, which is what an
//! `AdwStatusPage` is for and what a red alert is not.
//!
//! The button matters as much as the words. Without it, following the
//! instructions means installing Sober, waiting for a download, and then
//! discovering that Cordial has to be quit and started again before it will
//! look a second time. It re-runs exactly the check that failed.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

use crate::install;

/// The advice itself, kept as one constant so that it is quotable, testable,
/// and impossible to reword by halves.
const ADVICE: &str = "Cordial ships no Roblox build. The easiest way to get one is to install \
                      Sober and let it install Roblox for you:";

const COMMANDS: &str = "flatpak install flathub org.vinegarhq.Sober\n\
                        flatpak run org.vinegarhq.Sober    # let it finish downloading, then quit";

const ANY_APK: &str = "Any APK of the official Android x86-64 build works; that is simply the \
                       least fiddly way to obtain one.";

/// Show the instructions, transient for `parent`.
///
/// `retry` is called when the user presses the button, and returns whether a
/// build was found and started. `true` closes this window; `false` leaves it up
/// with the advice still on screen, because the user has not finished following
/// it and taking the instructions away mid-task helps nobody.
pub fn present(parent: &impl IsA<gtk::Window>, retry: impl Fn() -> bool + 'static) {
    let status = adw::StatusPage::builder()
        .icon_name("system-software-install-symbolic")
        .title("No Roblox build found")
        .description(ADVICE)
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.set_halign(gtk::Align::Center);

    // Monospace and selectable: these are commands, and the first thing anyone
    // does with a command on screen is try to copy it.
    let commands = gtk::Label::builder()
        .label(COMMANDS)
        .selectable(true)
        .wrap(true)
        .xalign(0.0)
        .css_classes(vec!["monospace".to_string(), "card".to_string()])
        .build();
    // A selectable label takes the initial focus and selects itself entirely,
    // so the window opens with two commands highlighted as if the user had
    // dragged across them. Copying still works; only the automatic focus goes.
    commands.set_focus_on_click(false);
    commands.set_can_focus(false);
    commands.set_margin_top(6);
    commands.set_margin_bottom(6);
    commands.set_margin_start(12);
    commands.set_margin_end(12);
    body.append(&commands);

    let footnote = gtk::Label::builder().label(ANY_APK).wrap(true).justify(gtk::Justification::Center).build();
    footnote.add_css_class("dim-label");
    body.append(&footnote);

    // Where Cordial looked, spelled out. A launcher that says "not found"
    // without saying where it searched leaves the user guessing at both the
    // path and whether it even tried.
    let looked = gtk::Label::builder()
        .label(format!("Cordial looked in {}", install::sober_apk().display()))
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .build();
    looked.add_css_class("dim-label");
    looked.add_css_class("caption");
    looked.set_focus_on_click(false);
    looked.set_can_focus(false);
    body.append(&looked);

    let button = gtk::Button::with_label("Check again and start Roblox");
    button.add_css_class("suggested-action");
    button.add_css_class("pill");
    button.set_halign(gtk::Align::Center);
    body.append(&button);

    status.set_child(Some(&body));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&status));

    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Set up Roblox")
        .default_width(600)
        // Tall enough that the button is on screen without scrolling. It was
        // not, at first: `AdwStatusPage` scrolls its own content, so the window
        // came up looking complete while the only control on it sat below the
        // fold — an instruction to press a button nobody could see.
        .default_height(720)
        .content(&toolbar)
        .build();

    let to_close = window.clone();
    button.connect_clicked(move |_| {
        if retry() {
            to_close.close();
        }
    });

    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_advice_names_sober_and_both_commands() {
        // This text is now the only place a user is told how to get a build.
        // Pinning it means a reword that drops half of it fails here rather
        // than in front of somebody who has nothing to launch.
        assert!(ADVICE.contains("Cordial ships no Roblox build"));
        assert!(COMMANDS.contains("flatpak install flathub org.vinegarhq.Sober"));
        assert!(COMMANDS.contains("flatpak run org.vinegarhq.Sober"));
        assert!(ANY_APK.contains("official Android x86-64 build"));
    }

    #[test]
    fn the_advice_and_the_detection_path_name_the_same_application() {
        // The instructions say to install Sober; the check has to be looking
        // where Sober puts it. These drifting apart is the failure this module
        // exists to prevent, so it is asserted rather than assumed.
        let looked = install::sober_apk();
        let looked = looked.to_string_lossy();
        assert!(looked.contains("org.vinegarhq.Sober"), "{looked}");
        assert!(looked.contains("com.roblox.client"), "{looked}");
    }
}
