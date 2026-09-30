//! The launcher's notice that the game will open on X11.
//!
//! ADR-024 keeps X11 supported and says plainly that it is the rougher
//! backend: Wayland is the primary one and gets the fixes first. Somebody on
//! X11 who hits something odd should hear that before they spend an evening on
//! it, and should have one press to get to the report screen.
//!
//! **Which backend is decided by the loader, not by this process.**
//! `cordial_runtime::android::backend()` picks Wayland when `WAYLAND_DISPLAY`
//! is set and `CORDIAL_X11` is not, and X11 otherwise; the launcher passes its
//! environment on to `cordial-run` unchanged. [`uses_x11`] is that same rule
//! over the same two variables. It is not "is this window's GDK display X11":
//! a session that exports `GDK_BACKEND=x11` on top of a Wayland compositor
//! runs the launcher through Xwayland and the game on Wayland, and telling
//! that user X11 support is buggy would be false.

use libadwaita as adw;
use libadwaita::prelude::*;

/// The action the banner's button and the primary menu share.
pub const REPORT_ACTION: &str = "win.report";

/// The message. Short: a banner is a strip above the launcher, not a page.
pub const TEXT: &str = "X11 support is buggy. If something goes wrong, try Wayland.";

/// The one control on the banner. It opens Cordial's own report screen, not a
/// web page: the diagnostics block is what makes the report worth reading.
pub const BUTTON: &str = "Report a Problem";

/// Whether the game will open on X11 for a session with these two variables.
///
/// Pure and taking presence rather than reading the environment, the same
/// split `diagnostics::roblox_in` makes: the environment is process-wide and
/// would interleave with every other test that touches it.
pub fn uses_x11(cordial_x11_set: bool, wayland_display_set: bool) -> bool {
    cordial_x11_set || !wayland_display_set
}

/// [`uses_x11`] for this process.
pub fn running_on_x11() -> bool {
    uses_x11(
        std::env::var_os("CORDIAL_X11").is_some(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    )
}

/// What the banner says, or nothing at all.
///
/// Split from the widget because an `AdwBanner` cannot be built without
/// `gtk::init`, and the decision is the part worth a test.
pub fn banner_text(x11: bool) -> Option<&'static str> {
    x11.then_some(TEXT)
}

/// The banner, revealed only when `x11` is true. It is always built and takes
/// no room while hidden, which keeps the launcher the same on Wayland.
pub fn build(x11: bool) -> adw::Banner {
    let banner = adw::Banner::new(banner_text(x11).unwrap_or(""));
    banner.set_button_label(Some(BUTTON));
    banner.set_action_name(Some(REPORT_ACTION));
    banner.set_revealed(x11);
    banner
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same rule `android::backend()` applies: a compositor wins unless
    /// X11 is forced, and no compositor means X11.
    #[test]
    fn x11_is_chosen_exactly_as_the_loader_chooses_it() {
        assert!(!uses_x11(false, true), "a Wayland session with nothing forced");
        assert!(uses_x11(true, true), "CORDIAL_X11 forces X11 over a compositor");
        assert!(uses_x11(false, false), "no compositor leaves X11");
        assert!(uses_x11(true, false));
    }

    #[test]
    fn the_notice_shows_under_x11_and_only_there() {
        assert_eq!(banner_text(true), Some(TEXT));
        assert_eq!(banner_text(false), None);
    }

    /// The button has to reach the same screen the menu does, so the action it
    /// names is the one `window.rs` registers. A rename that missed one side
    /// would leave a button that does nothing.
    #[test]
    fn the_button_names_the_report_action() {
        assert_eq!(REPORT_ACTION, "win.report");
        assert!(TEXT.len() < 80, "a banner is one line: {TEXT}");
    }
}
