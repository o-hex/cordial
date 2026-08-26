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
//!
//! ## The other button
//!
//! Since [ADR-025](../../../docs/adr/ADR-025-fetching-from-a-third-party-mirror.md)
//! Cordial can fetch the build itself, so there is a second button that does
//! it. The Sober advice stays because it is still the better route for anyone
//! who would rather go through Google Play on their own account, and because
//! somebody who already has Sober is finished before they press anything.
//!
//! **Where the download comes from is not written on this screen**, and that
//! was a deliberate trim rather than an omission. A caption explaining a
//! button before anybody has pressed it is clutter on the one screen a new
//! user meets, and the provenance is not hidden: the README says it, the
//! Updates page in Settings says it, and the line under the button says it
//! while the download is running. What the screen owes a first-time user is a
//! way forward, not a disclosure they cannot act on yet.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

use crate::install;
use crate::updater;

/// The advice itself, kept as one constant so that it is quotable, testable,
/// and impossible to reword by halves.
const ADVICE: &str = "Cordial ships no Roblox build. The easiest way to get one is to install \
                      Sober and let it install Roblox for you:";

const COMMANDS: &str = "flatpak install flathub org.vinegarhq.Sober\n\
                        flatpak run org.vinegarhq.Sober    # let it finish downloading, then quit";

const ANY_APK: &str = "Any APK of the official Android x86-64 build works; that is simply the \
                       least fiddly way to obtain one.";

/// What the download button says before it is pressed.
const FETCH: &str = "Download Roblox";

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

    let fetch = gtk::Button::with_label(FETCH);
    fetch.add_css_class("suggested-action");
    fetch.add_css_class("pill");
    fetch.set_halign(gtk::Align::Center);
    body.append(&fetch);

    // One line under the button, and it is the progress line once the button is
    // pressed. It starts empty rather than carrying a paragraph nobody asked
    // for: the screen already says what Cordial does, and a caption explaining
    // the button before anyone has pressed it is the clutter this screen was
    // trimmed of.
    let fetch_note = gtk::Label::builder()
        .label("")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .visible(false)
        .build();
    fetch_note.add_css_class("dim-label");
    fetch_note.add_css_class("caption");
    body.append(&fetch_note);

    let button = gtk::Button::with_label("Check again and start Roblox");
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

    let retry = std::rc::Rc::new(retry);

    let to_close = window.clone();
    let on_retry = retry.clone();
    button.connect_clicked(move |_| {
        if on_retry() {
            to_close.close();
        }
    });

    // The fetch runs off the main thread and reports as it goes. **A button
    // that goes dead for four minutes reads as a crash**, and a 229 MB
    // download takes about that, so the note underneath becomes the progress
    // line rather than leaving the user watching a frozen window.
    let to_close = window.clone();
    let on_retry = retry;
    let note = fetch_note.clone();
    fetch.connect_clicked(move |b| {
        b.set_sensitive(false);
        b.set_label("Working...");
        note.set_visible(true);
        note.remove_css_class("error");
        note.set_label("Looking for a build...");

        let b = b.clone();
        let note = note.clone();
        let failed_note = note.clone();
        let to_close = to_close.clone();
        let on_retry = on_retry.clone();
        updater::on_worker_reporting(
            |report| {
                cordial_update::provider::obtain_and_install(None, &mut |p| report(describe(&p)))
                    .map(|(got, _)| format!("{} from {}", got.version.name, got.provider))
                    .map_err(|e| e.to_string())
            },
            move |line| note.set_label(&line),
            move |outcome| match outcome {
                Ok(what) => {
                    b.set_label(&format!("Installed {what}"));
                    // The window closes only if the build actually starts, the
                    // same rule the other button follows: an install that
                    // cannot be launched is not a finished task.
                    if on_retry() {
                        to_close.close();
                    }
                }
                Err(why) => {
                    // **The message says what could not be reached, verbatim.**
                    // "Download failed" is the message this project has spent
                    // whole afternoons on: a mirror being down and a machine
                    // having no network look identical from in here, and only
                    // one of them is the user's to fix.
                    b.set_sensitive(true);
                    b.set_label(FETCH);
                    failed_note.set_label(&why);
                    failed_note.add_css_class("error");
                }
            },
        );
    });

    window.present();
}

/// One line of progress, in words rather than in the enum's shape.
fn describe(p: &cordial_update::provider::Progress) -> String {
    use cordial_update::provider::Progress;
    match p {
        Progress::Asking { provider } => format!("Asking {provider}..."),
        // Bytes rather than a percentage when the server did not say how many
        // there are, because a bar that invents a total is worse than no bar.
        Progress::Fetching { done, total: Some(t), .. } => {
            format!("Downloading: {} of {} MB", done / 1_048_576, t / 1_048_576)
        }
        Progress::Fetching { done, .. } => format!("Downloading: {} MB", done / 1_048_576),
        Progress::Verifying { file } => format!("Checking {file} is signed by Roblox..."),
    }
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
