//! Two Cordial clients, on two different profiles, running at once.
//!
//! ADR-012 already stops two clients writing *one* profile's storage —
//! that is a corruption risk and `profile::acquire`'s `flock` refuses it
//! outright, unconditionally, every time. Multi-instancing is the other
//! shape: two clients on two *different* profiles, which the lock has no
//! opinion about and which Roblox's own enforcement does. Two sessions from
//! one machine is a pattern their systems watch for, and getting it wrong
//! is not a Cordial bug to fix — it is the user's account.
//!
//! So this warns rather than refuses, on the same footing as `root_warning`:
//! naming a consequence before it is paid is honest, and Cordial has no way
//! to tell a deliberate second profile from one Roblox would flag either, so
//! refusing outright would refuse a use nobody here can rule out as legitimate.
//!
//! **This one is remembered, and `root_warning` is not, and that is a real
//! difference rather than an inconsistency to fix.** `root_warning::confirm`
//! says so plainly: "a warning about losing your account to a readable file
//! is not one to make dismissible forever on a machine where it stays true."
//! Root's hazards are also silent and technical — no PipeWire, no keyring —
//! and they cost a fresh hour of confused debugging each time somebody meets
//! them anew, because the failure they cause (an `abort()` at experience
//! start) does not look like the cause. This warning's hazard is neither: it
//! is a plain sentence about what Roblox might do, and once read and
//! accepted it does not become less true or less understood on the tenth
//! launch. Multi-instancing is also, for some of the people who want it, an
//! ordinary and repeated way of using this client — a second profile run
//! alongside the first most sessions — and a dialog that reappeared on every
//! one of those launches would not keep being read; it would be reflex-
//! clicked past, which is a warning working worse than one shown once and
//! actually taken in. That trade does not hold for root, where every launch
//! is a fresh chance to have forgotten what running as root costs.
//!
//! Stored in `shell_config::ShellConfig::multi_instance_warning_seen` —
//! `$XDG_CONFIG_HOME/cordial/shell.json` — rather than beside a profile the
//! way `plugin-consent-seen.json` is. That file's per-profile home is right
//! for plugin consent, which is a decision about *that* account, and wrong
//! here: this is a decision about running several profiles at once, so a
//! flag that reset for every new profile would fire again at exactly the
//! moment the user is doing the thing it is meant to catch — creating a
//! second one. `shell.json` is the piece of state this shell already keeps
//! above every profile, which is where a decision about profiles-in-plural
//! belongs.
//!
//! Detection lives in `profile::other_profile_is_running`, not here: knowing
//! whether a lock is actually held, as opposed to merely having a `.lock`
//! file left over from a process that exited months ago, is a fact about the
//! lock, and `profile.rs` is the one place that already knows how to ask the
//! kernel that question rather than the filesystem.

use crate::shell_config::ShellConfig;
use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Ask before launching a second profile alongside one already running.
/// `proceed` runs only if the user accepts, and acceptance is written to
/// `shell.json` before it runs so the dialog does not return on the next
/// launch.
pub fn confirm(window: &gtk::Window, config: &Rc<RefCell<ShellConfig>>, proceed: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::builder()
        .heading("Warning")
        .body(
            "Multi-instancing can get you banned on Roblox. It is not a supported feature \
             and may lead to account ban waves.\n\n\
             Maintainers and contributors are not responsible if your account gets banned.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("proceed", "Accept");
    dialog.set_response_appearance("proceed", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let config = config.clone();
    dialog.connect_response(None, move |d, response| {
        d.close();
        if response == "proceed" {
            mark_seen(&config);
            proceed();
        }
    });
    dialog.present(Some(window));
}

/// Record acceptance so the warning does not show again. Best-effort: a
/// write that fails leaves the in-memory flag set for the rest of this
/// process (so this launch and this session do not re-ask), and only a
/// failure to persist would bring the dialog back on a later run — reported
/// rather than escalated, on the same footing as every other `shell.json`
/// write in this crate.
fn mark_seen(config: &Rc<RefCell<ShellConfig>>) {
    config.borrow_mut().multi_instance_warning_seen = true;
    let path = crate::shell_config::path();
    if let Err(e) = crate::shell_config::save(&path, &config.borrow()) {
        println!(
            "  multi_instance_warning: could not save {} ({e}); the warning may show again",
            path.display()
        );
    }
}
