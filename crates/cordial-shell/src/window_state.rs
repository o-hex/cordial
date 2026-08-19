//! Whether the shell's window was fullscreen, and what size it was, kept per
//! profile: `<profile_dir>/window.json`.
//!
//! ## Why per profile, not `shell.json`
//!
//! ADR-013 draws the line between the two config surfaces by what a setting
//! *is*, not by which directory it started in: appearance stays global because
//! it is application chrome, and everything that answers a question about one
//! account moves into the profile. A window preference argues for the profile
//! side of that line for the same reason the task that added this file was
//! given in the first place -- a fullscreen choice made while poking at a
//! throwaway profile must not silently follow the user onto the profile they
//! actually play on. That is exactly the grants-file argument ADR-013 makes at
//! length, applied to one more setting: an approval, or here a preference,
//! given in one profile answering a question about a different one.
//!
//! ## Placement, per ADR-013
//!
//! Same shape as `network.rs`'s own placement note: `window.json` lives beside
//! `flags.json`, `plugin-grants.json` and `network.json` inside the profile
//! directory rather than in `$XDG_CONFIG_HOME/cordial/shell.json`. There is no
//! legacy file to migrate -- this setting did not exist anywhere before this
//! change -- so absence simply means [`WindowState::default`], windowed at
//! whatever size `window.rs` already asked for before this file existed.
//!
//! ## Why there is no saved position
//!
//! `host_window.rs`'s own `smallest_monitor` doc already says it: "Wayland
//! does not let a client choose, or even learn in advance, which output its
//! toplevel will be mapped on -- that is the compositor's decision". The
//! corollary is the same for *placing* a mapped toplevel: there is no
//! `xdg_toplevel` request that moves a window to a coordinate, on any
//! compositor, by design -- ADR-011 is the decision that put Cordial on
//! Wayland at all, and ADR-011 is explicit that the X11-only capabilities it
//! gave up are not coming back. A `WindowState` with an `(x, y)` in it would be
//! two fields this file could write and nothing could ever read back, which is
//! worse than not having them: a settings file that appears to remember
//! something it cannot is the "stub that returns success" AGENTS.md rules out,
//! wearing a config file instead of a function. So this remembers size only.
//!
//! ## The escape hatch, and why this file restores fullscreen unconditionally
//!
//! The risk named in this feature's task is real and specific: a launch that
//! comes up fullscreen with a broken renderer, or a display that no longer
//! exists, traps the user behind chrome they cannot get to. That risk is about
//! the *engine's* window -- black canvas, no frame, nothing to click -- and
//! this file does not restore fullscreen there. It cannot: `window.rs` builds
//! the shell's own launcher window, which is a `GtkDrawingArea` that paints
//! nothing behind libadwaita's own background (see `HostWindow::with_canvas`),
//! never an engine surface. There is no renderer in this window to break.
//!
//! So `window.rs` restores this state unconditionally, before the window is
//! shown, and that is safe specifically because F11 (`win.fullscreen`, wired
//! in the same function, before `present()`) and the compositor's own
//! fullscreen keybinding are both live from the first frame this window ever
//! draws -- there is no gap in which a fullscreen chooser window exists with
//! no way out.
//!
//! **That reasoning does not carry over to the engine's own window, and
//! whoever wires a saved preference there next must re-derive it rather than
//! copy this comment.** `android/wayland.rs` builds that window directly
//! against `HostWindow::with_canvas` (confirmed by reading it, not touched by
//! this change -- it is out of this task's scope) with no `GtkApplication` and
//! therefore no accelerator group at all: `set_accels_for_action` in
//! `window.rs` only works because this window's `set_application` call gives
//! it one. `docs/NEXT.md` §1e already documents that window's fullscreen path
//! as fragile even when driven deliberately, by test instrumentation, with a
//! developer watching. Restoring a saved fullscreen state into a window with
//! no confirmed keyboard escape and a documented history of fullscreen bugs,
//! right as the engine itself might be the thing that is broken, is precisely
//! the trap this task's brief warns against. It needs its own escape hatch
//! before it earns this file's default.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One profile's remembered window state. `#[serde(default)]` on the struct
/// and `Option` on the size fields mean a file written before a field existed,
/// or one with only `fullscreen` in it, still loads -- same shape as
/// `ShellConfig` and `NetworkConfig` in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub fullscreen: bool,
    /// The windowed size to restore -- meaningless while `fullscreen` is
    /// true, but kept regardless of it, because a window has to have *some*
    /// default size to fall back to the moment it leaves fullscreen. `None`
    /// means "nothing saved yet"; `window.rs` falls back to its own built-in
    /// default in that case, the same 540x340 it used before this file
    /// existed.
    pub width: Option<i32>,
    pub height: Option<i32>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { fullscreen: false, width: None, height: None }
    }
}

impl WindowState {
    /// Discard a saved size that could not have come from a real window.
    ///
    /// `host_window.rs`'s `fit_within` floors a *requested* size against the
    /// monitor by `min`-ing it, and `i32::min` of a negative number and a
    /// positive one is the negative number -- so an out-of-range value here
    /// would not be floored by that clamp, it would slip straight past it.
    /// Nothing this module writes can produce such a value; this guards
    /// against a file this process did not write, the same posture
    /// `profile::is_valid_name` takes on a name typed into a text field.
    fn sanitised(self) -> Self {
        const MAX_SIDE: i32 = 16384; // Comfortably past any real display; a
                                      // floor against nonsense, not a policy.
        Self {
            fullscreen: self.fullscreen,
            width: self.width.filter(|w| (1..=MAX_SIDE).contains(w)),
            height: self.height.filter(|h| (1..=MAX_SIDE).contains(h)),
        }
    }
}

/// This profile's window-state file.
///
/// `CORDIAL_WINDOW_STATE` overrides it outright -- the same development-switch
/// shape as `CORDIAL_NETWORK`, `CORDIAL_FLAGS` and `CORDIAL_SHELL_CONFIG`, not
/// a supported per-profile arrangement, because it makes one file serve every
/// profile, which is the thing every one of those files exists to stop.
pub fn path_in(profile_dir: &Path) -> PathBuf {
    std::env::var_os("CORDIAL_WINDOW_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| profile_dir.join("window.json"))
}

/// Read a profile's window state, or the default if there is nothing to read.
///
/// Same default-on-anything-wrong shape as `shell_config::load` and
/// `network::load`: a missing file is the ordinary case -- every profile that
/// existed before this change has none -- and a malformed one is far more
/// likely to be an interrupted write than an attack, so both fall back to
/// [`WindowState::default`] rather than refusing to start. A launcher that
/// would not open because its own window-geometry file was corrupt would be a
/// far worse failure than the one this file exists to fix.
pub fn load(profile_dir: &Path) -> WindowState {
    let path = path_in(profile_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return WindowState::default();
    };
    match serde_json::from_str::<WindowState>(&text) {
        Ok(state) => state.sanitised(),
        Err(e) => {
            println!(
                "  window: {} is not usable ({e}); starting windowed at the default size",
                path.display()
            );
            WindowState::default()
        }
    }
}

/// Save a profile's window state.
///
/// `create_dir_all` rather than requiring the profile to already exist, the
/// same posture `network::save` takes: this is settings-file plumbing, not the
/// credential path `profile::acquire`'s `restrict_to_owner` exists for, and it
/// must not take that function's lock -- a fullscreen toggle on the launcher's
/// own window must work whether or not a client happens to be running against
/// the profile currently selected in the row above it, and `profile::acquire`
/// would refuse exactly when one is.
pub fn save(profile_dir: &Path, state: &WindowState) -> std::io::Result<()> {
    std::fs::create_dir_all(profile_dir)?;
    let text = serde_json::to_string_pretty(state).expect("WindowState always serialises");
    std::fs::write(path_in(profile_dir), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-window-state-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_profile_with_no_file_starts_windowed_at_no_saved_size() {
        let dir = scratch("absent");
        let state = load(&dir);
        assert!(!state.fullscreen);
        assert_eq!(state.width, None);
        assert_eq!(state.height, None);
    }

    #[test]
    fn a_malformed_file_falls_back_to_windowed_rather_than_refusing_to_start() {
        let dir = scratch("malformed");
        std::fs::write(path_in(&dir), "{not json").unwrap();
        assert!(!load(&dir).fullscreen);
    }

    #[test]
    fn a_saved_fullscreen_choice_round_trips() {
        let dir = scratch("fullscreen-roundtrip");
        save(&dir, &WindowState { fullscreen: true, width: None, height: None }).unwrap();
        assert!(load(&dir).fullscreen);
    }

    #[test]
    fn a_saved_size_round_trips() {
        let dir = scratch("size-roundtrip");
        save(&dir, &WindowState { fullscreen: false, width: Some(1024), height: Some(768) }).unwrap();
        let back = load(&dir);
        assert_eq!(back.width, Some(1024));
        assert_eq!(back.height, Some(768));
    }

    #[test]
    fn a_config_written_before_the_size_fields_existed_still_loads() {
        // Everyone who saved a window.json before `width`/`height` existed has
        // one holding only `fullscreen`. `#[serde(default)]` is what makes
        // this true; worth a test rather than a note, the same reasoning
        // `shell_config.rs`'s own version of this test gives.
        let dir = scratch("older-schema");
        std::fs::write(path_in(&dir), r#"{"fullscreen":true}"#).unwrap();
        let state = load(&dir);
        assert!(state.fullscreen);
        assert_eq!(state.width, None);
    }

    #[test]
    fn an_out_of_range_size_is_discarded_rather_than_handed_to_gtk() {
        // The reason `sanitised` exists, spelled out: `fit_within` clamps a
        // requested size by `min`-ing it against the monitor, and `i32::min`
        // of a negative number and a positive one is the negative number --
        // so a tampered or truncated file could otherwise hand GTK a size
        // that clamp does not defend against at all.
        let dir = scratch("out-of-range");
        std::fs::write(path_in(&dir), r#"{"fullscreen":false,"width":-1,"height":0}"#).unwrap();
        let state = load(&dir);
        assert_eq!(state.width, None, "a negative width must not reach the window");
        assert_eq!(state.height, None, "a zero height must not reach the window");

        std::fs::write(path_in(&dir), r#"{"fullscreen":false,"width":99999999,"height":600}"#).unwrap();
        let state = load(&dir);
        assert_eq!(state.width, None, "an implausibly large width must not reach the window");
        assert_eq!(state.height, Some(600), "a plausible sibling field must survive the check");
    }

    #[test]
    fn one_profiles_window_state_is_not_anothers() {
        let root = scratch("isolation");
        let testing = root.join("testing");
        let main = root.join("main");
        std::fs::create_dir_all(&testing).unwrap();
        std::fs::create_dir_all(&main).unwrap();

        // The exact scenario the task this file answers named: fullscreen set
        // while poking at a throwaway profile must not follow onto the one
        // somebody actually plays on.
        save(&testing, &WindowState { fullscreen: true, width: Some(3440), height: Some(1440) })
            .unwrap();
        assert!(load(&testing).fullscreen);
        assert!(!load(&main).fullscreen, "a neighbouring profile must not inherit this");
        assert_eq!(load(&main).width, None);
    }

    // No test sets `CORDIAL_WINDOW_STATE` itself: it is process-wide, cargo
    // runs this file's tests in parallel threads of one process, and every
    // other test above calls `load`/`save` without expecting a redirect --
    // `network.rs` makes the same choice for `CORDIAL_NETWORK`, for the same
    // reason. `profile.rs`'s own tests need a shared mutex to test their
    // process-wide override safely at all; this file has nothing that override
    // is worth that machinery for.
}
