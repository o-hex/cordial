//! What to do when there is no Roblox build to launch.
//!
//! Not an error dialog, because this is not an error. A fresh install with no
//! Roblox build is the ordinary first-run state -- Cordial ships no Roblox code
//! and never will -- and it has a one-press way out, which is what an
//! `AdwStatusPage` is for and what a red alert is not.
//!
//! ## One button, and it is not "install Sober"
//!
//! This screen used to be instructions: two `flatpak` commands, a note that any
//! APK works, and the path Cordial had searched. That was the honest screen
//! while Cordial could not fetch a build -- the only way forward genuinely was
//! to go and run another program first, and a launcher that says "not found"
//! without saying what to do about it leaves the user guessing.
//!
//! It is the wrong screen now. **Cordial owns getting the build**, so the way
//! forward is a button, and putting another project's install commands in front
//! of somebody who has just opened this one is asking them to do work Cordial
//! is about to do anyway.
//!
//! None of that is a reason to stop crediting Sober, and the README still does,
//! at length. A first-run screen and an acknowledgement are different things:
//! one is a way out of a dead end, the other is saying who the work is owed to.
//! Confusing them is how the acknowledgement ended up as an instruction.
//!
//! ## It does not download on its own
//!
//! An empty first run could fetch the build without being asked, and it must
//! not. This is a few hundred megabytes, `metered.rs` exists precisely because
//! somebody may be paying for it by the megabyte, and a program that opens and
//! immediately spends that is one people learn not to open on the train. One
//! press is the whole difference and it costs nothing.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

use crate::download_progress::Meter;
use crate::updater;

/// What the screen says above the button.
///
/// Two sentences: what is missing, and that pressing the button fixes it.
const ADVICE: &str =
    "Cordial ships no Roblox build. Press the button and it will download and verify one.";

/// Said once, small, under the button. Where the build comes from is on the
/// Updates page in Settings for anybody who wants it; this is the sentence that
/// stops "download" reading as "trust whatever arrives".
const CHECKED: &str = "Installs only if Roblox's signing certificate signed it.";

/// The one button.
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

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_halign(gtk::Align::Center);

    let fetch = gtk::Button::with_label(FETCH);
    fetch.add_css_class("suggested-action");
    fetch.add_css_class("pill");
    fetch.set_halign(gtk::Align::Center);
    body.append(&fetch);

    let checked = gtk::Label::builder()
        .label(CHECKED)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    checked.add_css_class("dim-label");
    checked.add_css_class("caption");
    body.append(&checked);

    // The bar replaces the button in place rather than opening in front of it.
    // See `download_progress`: a second window for one action leaves two things
    // to dismiss and says nothing this space could not.
    let meter = Meter::new();
    meter.widget().set_width_request(320);
    body.append(meter.widget());

    status.set_child(Some(&body));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&status));

    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Set up Roblox")
        .default_width(560)
        .default_height(520)
        .content(&toolbar)
        .build();

    let to_close = window.clone();
    let retry = std::rc::Rc::new(retry);
    fetch.connect_clicked(move |b| {
        b.set_visible(false);
        checked.set_visible(false);
        meter.start();

        let b = b.clone();
        let checked = checked.clone();
        let meter = meter.clone();
        let failed_meter = meter.clone();
        let to_close = to_close.clone();
        let retry = retry.clone();
        updater::on_worker_reporting(
            |report| {
                cordial_update::provider::obtain_and_install(None, &mut |p| report(p))
                    .map(|(got, _)| got.version.name)
                    .map_err(|e| e.to_string())
            },
            move |step| meter.step(&step),
            move |outcome| match outcome {
                Ok(version) => {
                    failed_meter.finish(&version);
                    // **Both go at once.** The build is installed and the
                    // launcher can now find it, so leaving this window up for
                    // the user to dismiss is asking them to acknowledge a
                    // finished job. `retry` starts the client; if it somehow
                    // cannot, the window stays and the bar keeps saying what
                    // happened rather than vanishing into nothing.
                    if retry() {
                        to_close.close();
                    }
                }
                Err(why) => {
                    failed_meter.failed(&why);
                    b.set_visible(true);
                    checked.set_visible(true);
                }
            },
        );
    });

    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **This test used to pin two `flatpak` commands and a note about any APK
    /// working.** It was the right test while this screen was instructions:
    /// the text was the only place a user was told how to get a build, and a
    /// reword that dropped half of it had to fail here rather than in front of
    /// somebody with nothing to launch.
    ///
    /// The screen is a button now, so what has to be pinned is different: the
    /// promise the button makes, and the sentence that keeps "download" from
    /// reading as "trust whatever arrives".
    #[test]
    fn the_screen_promises_a_download_and_says_what_is_checked() {
        assert!(ADVICE.contains("Cordial ships no Roblox build"));
        assert!(ADVICE.contains("download"));
        assert!(CHECKED.contains("signing certificate"));
        assert_eq!(FETCH, "Download Roblox");
    }

    /// Nothing on this screen sends the user to another program.
    ///
    /// Cordial owns getting the build, so an instruction to go and install
    /// something else first is work it is about to do anyway. Sober is still
    /// credited at length in the README; a first-run screen and an
    /// acknowledgement are different things, and this is the one that is a way
    /// out of a dead end.
    #[test]
    fn the_screen_does_not_send_anybody_to_another_program() {
        for text in [ADVICE, CHECKED, FETCH] {
            assert!(!text.contains("Sober"), "{text}");
            assert!(!text.contains("flatpak"), "{text}");
        }
    }
}
