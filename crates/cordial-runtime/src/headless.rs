//! `--headless`: run the client inside a nested compositor of its own, so a
//! test run never appears on the developer's screen or takes their focus.
//!
//! This exists because agents testing Cordial were opening windows on the
//! machine's owner's session while they were using it. A client that steals
//! focus mid-sentence is not a small annoyance when several runs happen an
//! hour, and the alternative everybody reaches for first -- "just put it on
//! another workspace" -- is not something a Wayland client may ask for.
//!
//! **The compositor has to be the parent process, which is why this re-execs.**
//! A Wayland client cannot create a compositor for itself after it has
//! connected to one, and by the time `main` has parsed its arguments GTK is
//! already talking to whatever `WAYLAND_DISPLAY` pointed at. So the only place
//! this can be done is before anything else happens: strip the flag, put `cage`
//! in front, and hand the process over.
//!
//! What was measured, on 2026-08-23, before any of this was written:
//!
//! - `cage --backend headless` works. `presents` climbed 796 -> 804 over eight
//!   seconds read through devctl, and `cordial_screenshot` returned a real
//!   frame of the sign-in page. About 1/s is the documented idle throttle with
//!   no input flowing, not a frame rate -- see AGENTS.md before quoting it.
//! - It only works with `be4b551`. Before that commit Cordial asked the seat
//!   for a pointer without checking whether it had one, and wlroots' headless
//!   backend advertises no capabilities at all, so the compositor killed the
//!   client on startup with `wl_seat.get_pointer called when no pointer
//!   capability has existed`.
//! - **Xvfb is not an alternative and no fallback to it should be written.**
//!   Mesa reports `No DRI3 support detected - required for presentation`,
//!   Vulkan's X11 WSI has no non-DRI3 path, and `presents` read a flat 0 twice
//!   twenty seconds apart. That is structural, not a missing Xorg option.
//! - `mutter --headless` is separately ruled out and has been since before
//!   this: it dies within a second of the engine presenting and takes the
//!   client with it. `examples/pointer_capture_probe.rs` records the control.

use std::ffi::OsString;

/// Set on the child so the re-exec happens once rather than forever.
///
/// A marker in the environment rather than an extra argument, because the
/// argument list is the thing being rewritten and a flag that has to survive
/// its own removal is a puzzle nobody needs.
pub const MARKER: &str = "CORDIAL_HEADLESS_CHILD";

/// The compositor. Chosen because it is the one that was actually measured
/// working here; `WLR_BACKENDS=headless` is a wlroots setting, so a different
/// wlroots-family compositor would very likely do, but "very likely" is not
/// what this file is for.
pub const COMPOSITOR: &str = "cage";

/// The argv for the nested run, or `None` if `--headless` was not asked for.
///
/// Pure so the rewriting can be tested without a compositor, which matters
/// because the failure mode being guarded against -- the flag silently not
/// taking effect -- looks exactly like success from inside the process.
pub fn nested_argv(argv: &[OsString]) -> Option<Vec<OsString>> {
    let wanted = argv.iter().skip(1).any(|a| a == "--headless");
    if !wanted {
        return None;
    }
    let exe = std::env::current_exe().ok().map(OsString::from).unwrap_or_else(|| argv[0].clone());

    let mut out: Vec<OsString> = vec![COMPOSITOR.into(), "--".into(), exe];
    // Every argument except the flag itself. `--headless` takes no value, so
    // there is nothing after it to drop with it.
    out.extend(argv.iter().skip(1).filter(|a| *a != "--headless").cloned());
    Some(out)
}

/// The environment the nested compositor needs, as (key, value-or-unset) pairs.
///
/// `WAYLAND_DISPLAY` and `DISPLAY` are cleared rather than left alone, and that
/// is the whole difference between a nested compositor and a second window on
/// the developer's screen: with either still set, cage nests into the running
/// session and renders there, which is precisely what `--headless` was asked
/// to avoid.
pub fn nested_env() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("WLR_BACKENDS", Some("headless")),
        // No libinput behind a headless backend, and asking for devices that
        // cannot exist logs an error on every run.
        ("WLR_LIBINPUT_NO_DEVICES", Some("1")),
        ("WAYLAND_DISPLAY", None),
        ("DISPLAY", None),
        (MARKER, Some("1")),
    ]
}

/// Whether this process is already the nested child.
pub fn is_child() -> bool {
    std::env::var_os(MARKER).is_some()
}

/// Replace this process with the same command under a nested compositor.
///
/// Returns only on failure. **It does not fall back to a visible window**, and
/// that is deliberate: somebody who passed `--headless` did so to avoid being
/// interrupted, and a run that quietly appears on their screen instead has
/// failed at the one thing it was asked to do while reporting success. A stub
/// that returns success it did not achieve is the shape AGENTS.md rules out,
/// and this is the same mistake wearing a window.
pub fn exec_nested() -> Result<std::convert::Infallible, String> {
    use std::os::unix::process::CommandExt;

    let argv: Vec<OsString> = std::env::args_os().collect();
    let nested = nested_argv(&argv).ok_or_else(|| "--headless was not requested".to_string())?;

    if which(COMPOSITOR).is_none() {
        return Err(format!(
            "--headless needs `{COMPOSITOR}`, which is not on PATH.\n\
             It is a nested Wayland compositor; install it with your package manager, or in \
             the toolbox this project builds in. Xvfb is not a substitute -- Mesa reports no \
             DRI3 support and nothing is ever presented -- and neither is `mutter --headless`, \
             which dies as soon as the engine starts presenting."
        ));
    }

    let mut cmd = std::process::Command::new(&nested[0]);
    cmd.args(&nested[1..]);
    for (k, v) in nested_env() {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    // Said before the handover, because after it this process is gone and any
    // explanation with it.
    eprintln!(
        "[headless] running under `{COMPOSITOR}` with WLR_BACKENDS=headless; no window will \
         appear on this session."
    );
    if std::env::var_os("CORDIAL_DEV_CONTROL").is_none() {
        // Not turned on automatically. A socket appearing because of a flag
        // about windows would be a surprise, and this project would rather say
        // the thing than do it quietly.
        eprintln!(
            "[headless] note: CORDIAL_DEV_CONTROL is not set, so there is no devctl socket and \
             nothing can see or drive this client -- headless, it has no window either. Set \
             CORDIAL_DEV_CONTROL=1 to attach the MCP."
        );
    }
    Err(format!("could not exec `{COMPOSITOR}`: {}", cmd.exec()))
}

/// First match for `name` on `PATH`.
fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    #[test]
    fn without_the_flag_nothing_is_rewritten() {
        assert!(nested_argv(&argv(&["cordial-run", "--run", "30"])).is_none());
    }

    #[test]
    fn the_flag_is_removed_and_every_other_argument_survives_in_order() {
        // Order matters more than it looks: `--profile` latches the active
        // profile directory on first use, and an argument list quietly
        // reordered here would be a session written to the wrong place.
        let out = nested_argv(&argv(&[
            "cordial-run",
            "--headless",
            "--profile",
            "agent",
            "--run",
            "30",
            "--game-activity",
        ]))
        .expect("the flag was present");
        let tail: Vec<_> = out.iter().skip(3).map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(tail, ["--profile", "agent", "--run", "30", "--game-activity"]);
        assert_eq!(out[0], OsString::from(COMPOSITOR));
        assert_eq!(out[1], OsString::from("--"));
    }

    #[test]
    fn the_flag_is_not_matched_inside_a_value() {
        // `--profile --headless` is a daft profile name and also a legal one,
        // and a filter written with `contains` would eat it.
        let out = nested_argv(&argv(&["cordial-run", "--headless", "--profile", "not--headless"]))
            .expect("the flag was present");
        let tail: Vec<_> = out.iter().skip(3).map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(tail, ["--profile", "not--headless"]);
    }

    #[test]
    fn the_real_display_is_cleared_rather_than_inherited() {
        // The one that decides whether this is a nested compositor or a second
        // window on somebody's desk.
        let env = nested_env();
        assert_eq!(env.iter().find(|(k, _)| *k == "WAYLAND_DISPLAY").unwrap().1, None);
        assert_eq!(env.iter().find(|(k, _)| *k == "DISPLAY").unwrap().1, None);
        assert_eq!(env.iter().find(|(k, _)| *k == "WLR_BACKENDS").unwrap().1, Some("headless"));
    }

    #[test]
    fn the_child_is_marked_so_the_re_exec_happens_once() {
        assert_eq!(nested_env().iter().find(|(k, _)| *k == MARKER).unwrap().1, Some("1"));
    }
}
