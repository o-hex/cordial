//! What the launcher shows when the client stops on its own.
//!
//! ## Why a status page and not a red alert
//!
//! [`crate::instructions`] argues the opposite way for the case it covers, and
//! the argument is worth repeating rather than assumed: a fresh install with no
//! Roblox build "is not an error", it "has a scripted way out, which is what an
//! `AdwStatusPage` is for and what a red alert is not".
//!
//! A crash is an error, so half of that reasoning does not carry. The other
//! half does, and more strongly: this needs *room*. An `AdwMessageDialog` has a
//! heading and a paragraph, and what has to fit here is a heading, an exit
//! status, a couple of hundred lines of the client's own output, and a control
//! to copy them. Poured into a dialog body that becomes an unreadable wall with
//! no way to fold it away, which is precisely what the thing this replaces did
//! with the command line it printed.
//!
//! Everything on the page is libadwaita's own: `AdwStatusPage` for the frame,
//! `AdwExpanderRow` in an `AdwPreferencesGroup` for the disclosure. The first
//! version used a plain `GtkExpander` and it was spotted immediately -- a GTK3
//! triangle-and-label sitting in a GNOME 4x page. Nothing here should be a raw
//! GTK widget where libadwaita has an equivalent.
//!
//! So: a status page for the room, and the seriousness carried by the error
//! icon, the wording, and the fact that this window is the only thing on screen
//! rather than a toast that fades. The detail is behind an expander because the
//! first question is "did it crash", not "why" -- and somebody who does want
//! why should not have to reproduce it in a terminal to get it, which is what
//! the old alert asked of them.
//!
//! ## What used to happen
//!
//! `window.rs` raised one alert, from one timer, three seconds after launch. It
//! said "Roblox stopped as soon as it started", then "Running this in a
//! terminal will show what it printed", and quoted the command line. A crash
//! ten minutes in produced nothing at all: the timer had long since fired, and
//! the window simply disappeared. The output the user was being sent to a
//! terminal to find had been on the launcher's own stdout the whole time,
//! inherited from a process it started.
//!
//! ## The output on this page is not written anywhere new
//!
//! `launch::pump` echoes the client's streams to the launcher's own stdout and
//! stderr, exactly as inheriting them did, and keeps a redacted copy in memory
//! for this window. Nothing is added to a log file, and `[cookies]` and
//! `[identity]` lines are replaced before they reach the buffer -- see
//! `launch::redact` for why that happens at capture rather than at the copy
//! button.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

/// Whether an exit means something went wrong.
///
/// **Only a failing status counts.** The three ordinary ways a session ends --
/// the user closing Roblox's window, `SIGTERM`/`SIGINT`, and `--run` expiring
/// -- all leave `cordial-run` exiting zero, because it has one shutdown path
/// and three entry points into it (see `launch::DEFAULT_RUN_SECONDS`). A signal
/// death gives no code at all and `success()` is false for it, which is the
/// case this page most needs to catch: a `SIGSEGV` in the engine is the crash
/// nobody currently gets told about.
///
/// Kept as a function of the status alone, so it is testable without a process.
pub fn is_crash(status: &std::process::ExitStatus) -> bool {
    !status.success()
}

/// How the exit is described in one line, without making the user read it as a
/// number.
///
/// `ExitStatus`'s own `Display` already spells out both shapes -- "exit status:
/// 1" and "signal: 11 (SIGSEGV)" -- so this adds the sentence around it rather
/// than reimplementing the formatting and getting the signal names wrong.
pub fn describe(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("Roblox stopped on its own, with exit code {code}."),
        None => format!("Roblox was stopped by the system ({status})."),
    }
}

/// Show the crash, transient for `parent`.
///
/// `output` is what the client last printed, already redacted; `command_line`
/// is what was run. Both go in the expander, in that order, because the output
/// is what somebody is looking for and the command is context for it.
pub fn present(
    parent: &impl IsA<gtk::Window>,
    status: &std::process::ExitStatus,
    command_line: &str,
    output: &str,
) {
    let status_page = adw::StatusPage::builder()
        .icon_name("dialog-error-symbolic")
        .title("Roblox stopped unexpectedly")
        .description(describe(status))
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_halign(gtk::Align::Fill);

    let detail = if output.trim().is_empty() {
        // Said rather than shown as an empty box. A client killed before it
        // wrote a line is a different situation from one that printed a panic,
        // and an empty expander looks like the page failing to load its own
        // content.
        format!("Roblox printed nothing before it stopped.\n\nIt was started with:\n{command_line}")
    } else {
        format!("{output}\n\nIt was started with:\n{command_line}")
    };

    // **`AdwExpanderRow`, not a bare `GtkExpander`.** The first version of this
    // page used the plain GTK widget -- a small triangle and a text label -- and
    // it read as exactly what it was: GTK3-era chrome sitting inside an
    // otherwise clean `AdwStatusPage`. libadwaita's own row gets the rounded
    // card, the row height, the animated chevron and the typography that every
    // other expandable thing in a GNOME application has, and none of that is
    // worth reimplementing badly.
    let label = gtk::Label::builder()
        .label(&detail)
        .selectable(true)
        .wrap(false)
        .xalign(0.0)
        .yalign(0.0)
        .css_classes(vec!["monospace".to_string()])
        .build();
    // Monospace and not wrapped, so a path or a symbol name stays one line and
    // the scroller deals with the width. A wrapped log is soup.
    //
    // Selectable but not focusable, which `instructions.rs` already paid for
    // once: a selectable label takes the initial focus and selects itself
    // entirely, so the window would open with the whole log highlighted as if
    // somebody had dragged across it.
    label.set_focus_on_click(false);
    label.set_can_focus(false);
    label.set_margin_top(8);
    label.set_margin_bottom(8);
    label.set_margin_start(12);
    label.set_margin_end(12);

    let scroller = gtk::ScrolledWindow::builder()
        .min_content_height(260)
        .max_content_height(320)
        .propagate_natural_height(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&label)
        .build();
    scroller.set_height_request(260);

    let expander = adw::ExpanderRow::builder()
        .title("Details")
        .subtitle("What Roblox printed before it stopped")
        .build();
    expander.add_row(&scroller);

    // The group is what supplies the card. An `AdwExpanderRow` outside a
    // `AdwPreferencesGroup`/`GtkListBox` is an unstyled row floating on the
    // page, which is the same mistake one layer up.
    let group = adw::PreferencesGroup::new();
    group.add(&expander);
    body.append(&group);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_halign(gtk::Align::Center);

    // **Copies exactly what the expander shows**, which is the whole reason
    // `launch::redact` runs at capture. A button that silently copied more than
    // was on screen would put a signed-in username into somebody's paste
    // without them ever having seen it.
    let copy = gtk::Button::with_label("Copy details");
    copy.add_css_class("pill");
    {
        let detail = detail.clone();
        let copy_for_label = copy.clone();
        copy.connect_clicked(move |button| {
            button.clipboard().set_text(&detail);
            // Feedback, because a clipboard write is invisible and the second
            // press of a button that appeared to do nothing is how people end
            // up with the wrong thing pasted.
            copy_for_label.set_label("Copied");
        });
    }
    buttons.append(&copy);

    let close = gtk::Button::with_label("Close");
    close.add_css_class("pill");
    close.add_css_class("suggested-action");
    buttons.append(&close);
    body.append(&buttons);

    status_page.set_child(Some(&body));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&status_page));

    let window = adw::Window::builder()
        .transient_for(parent.as_ref())
        .modal(true)
        .title("Roblox stopped")
        .default_width(680)
        // Tall enough that the expander can open without the buttons falling
        // below the fold -- `instructions.rs` records paying for exactly that
        // mistake with a button nobody could see.
        .default_height(560)
        .content(&toolbar)
        .build();

    let to_close = window.clone();
    close.connect_clicked(move |_| to_close.close());
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn a_clean_exit_is_not_a_crash() {
        // The requirement that keeps this page out of the way of ordinary use:
        // closing Roblox, or `--run` expiring, must not raise an error window.
        assert!(!is_crash(&ExitStatus::from_raw(0)));
    }

    #[test]
    fn a_failing_exit_code_is_a_crash() {
        // `wait`-style encoding: the low byte is the signal, the next is the
        // code. 1 << 8 is "exited with 1".
        let status = ExitStatus::from_raw(1 << 8);
        assert!(is_crash(&status));
        assert!(describe(&status).contains("exit code 1"), "{}", describe(&status));
    }

    #[test]
    fn a_signal_death_is_a_crash_and_is_not_described_as_an_exit_code() {
        // The case the old three-second alert could never catch: an engine
        // `SIGSEGV` twenty minutes into a session. There is no exit code here,
        // and saying "exit code 0" -- which `code()` returning `None` would
        // become under a careless `unwrap_or_default` -- would describe a crash
        // as a clean shutdown.
        let status = ExitStatus::from_raw(11); // SIGSEGV, no core flag
        assert!(is_crash(&status));
        let line = describe(&status);
        assert!(!line.contains("exit code"), "{line}");
        assert!(line.contains("stopped by the system"), "{line}");
    }
}
