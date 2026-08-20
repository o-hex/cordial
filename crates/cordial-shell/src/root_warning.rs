//! What running as root costs, said once before it costs it.
//!
//! A FreeBSD user reached the Roblox home page, signed in, entered an
//! experience, and then the engine called `abort()`. The cause was three lines
//! above it in their log: no PipeWire session, so `opensles.cpp` reported
//! failure honestly, and the engine's answer to an honest audio failure is to
//! stop. They were running as root, which is why there was no session bus to
//! find a PipeWire daemon through.
//!
//! Root is not itself the fault, and this does not refuse it — on FreeBSD's
//! linuxulator there is no other user, so refusing would refuse the platform.
//! What it does is name the consequences before an hour is spent on them,
//! because each one presents as a different unrelated bug:
//!
//! - **No session bus**, so no keyring. Cookies fall back to a 0600 file, which
//!   `secrets.rs` already warns about, and anything able to read that file has
//!   the account.
//! - **No PipeWire**, so no audio — and the engine aborts at experience start
//!   rather than playing silence. That is the one that looks like a crash.
//! - No Feral GameMode, no accessibility bus.
//!
//! The dialog says what will happen and lets the user go ahead, because
//! sometimes root is the only option available and Cordial's job is to be
//! honest rather than obstructive.

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;

/// Whether this process is running as root.
pub fn running_as_root() -> bool {
    // SAFETY: `geteuid` takes no arguments, touches no memory and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// Ask before launching as root. `proceed` runs only if the user chooses to.
///
/// Nothing is remembered between launches on purpose. A warning about losing
/// your account to a readable file is not one to make dismissible forever on a
/// machine where it stays true.
pub fn confirm(window: &gtk::Window, proceed: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::builder()
        .heading("Running as root")
        .body(
            "Roblox will have no sound, and the engine stops when an experience \
             starts if it cannot open an audio device.\n\n\
             Your sign-in is also saved to a plain file instead of the keyring, \
             so anything that can read your files can take the account.\n\n\
             This happens because root usually has no desktop session for \
             PipeWire and the keyring to live in. If you can run Cordial as \
             an ordinary user, do that instead.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("proceed", "Launch Anyway");
    dialog.set_response_appearance("proceed", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |d, response| {
        d.close();
        if response == "proceed" {
            proceed();
        }
    });
    dialog.present(Some(window));
}
