//! Cordial's implementation of the Android NDK APIs in `libandroid.so`.
//!
//! Thirty-two functions across four groups, all currently stubbed except assets:
//!
//! | Group | Functions | State |
//! |---|---|---|
//! | `AAsset*` | 6 | implemented — see [`asset`] |
//! | `ANativeWindow_*` | 10 | implemented over a host window — see [`window`]/[`wayland`] |
//! | `ALooper_*` | 7 | implemented over epoll — see [`looper`] |
//! | `AConfiguration_*` | 9 | implemented — see [`config`] |
//!
//! The order is not arbitrary: assets gate everything, because the engine cannot
//! load a shader or a font without them. See docs/design/path-to-a-frame.md.

pub mod accessibility;
pub mod asset;
pub mod config;
pub mod gl;
pub mod glcount;
pub mod input;
pub mod looper;
pub mod system;
pub mod vulkan;
pub mod wayland;
pub mod window;

use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static TRACE: AtomicBool = AtomicBool::new(false);

/// Log every Android API call. AGDK's `initializeNativeCode` returns a bare 0 on
/// failure with nothing logged, so the only way to find where it stopped is to
/// watch which of these it reached.
pub fn set_trace(on: bool) {
    TRACE.store(on, Ordering::Relaxed);
}

pub(crate) fn trace(args: std::fmt::Arguments<'_>) {
    if TRACE.load(Ordering::Relaxed) {
        eprintln!("[android] {args}");
    }
}

// ------------------------------------------------------------ display backend
//
// ADR-011: Wayland is the target; X11 (via `window.rs`, over Xwayland where
// there is no real X server) stays in the tree as a diagnosable fallback while
// the Wayland path proves itself, not as a second supported configuration. See
// that ADR for why — the short version is that a resize on Xwayland cannot be
// made atomic no matter how this code is written, because the protocol has no
// way to express "the new content is ready"; `xdg_toplevel`'s configure/ack/
// commit sequence exists for exactly that.
//
// The choice is made once, from the environment, and then fixed for the life
// of the process — `open_window` and `overrides()` have to agree on it, and a
// value that could change after the window is open would let them disagree.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    X11,
    Wayland,
}

/// Which display backend this run uses. `WAYLAND_DISPLAY` set means a Wayland
/// compositor is available and is preferred; its absence means either a bare
/// Xorg session or an Xwayland-rootless host, and X11 is what both of those
/// actually offer a client. Reported unconditionally, the same way `window.rs`
/// always reports its window placement — "which backend Cordial picked" is
/// exactly the kind of fact a bug report needs and a trace flag would hide.
pub fn backend() -> Backend {
    static CHOSEN: OnceLock<Backend> = OnceLock::new();
    *CHOSEN.get_or_init(|| {
        // Opt-in, not automatic, until `wayland.rs` is real. `WAYLAND_DISPLAY`
        // is set on every modern desktop, so preferring Wayland on its presence
        // alone would make the client refuse to start for everyone the moment
        // the unimplemented backend merged. Once it works, this reverts to
        // preferring Wayland whenever a compositor is there — which is what
        // ADR-011 actually specifies.
        let b = if std::env::var_os("CORDIAL_WAYLAND").is_some()
            && std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            Backend::Wayland
        } else {
            Backend::X11
        };
        match b {
            Backend::Wayland => println!("[android] display backend: Wayland (CORDIAL_WAYLAND=1)"),
            Backend::X11 => println!(
                "[android] display backend: X11 (see ADR-011 — the fallback until the Wayland backend lands)"
            ),
        }
        b
    })
}

/// The open window, whichever backend it came from. Callers that only need
/// `geometry()` — which is all `load.rs` wants from the return value of
/// [`open_window`] — can use this directly; callers that need the backend's
/// own native handles (EGL, Vulkan) go through [`window::current`] or
/// [`wayland::current`] instead, because those handles have different shapes
/// per backend and forcing them into one enum would just move the `match` from
/// here to there.
pub enum WindowHandle {
    X11(&'static window::HostWindow),
    Wayland(&'static wayland::WaylandWindow),
}

impl WindowHandle {
    pub fn geometry(&self) -> (i32, i32, i32) {
        match self {
            WindowHandle::X11(w) => w.geometry(),
            WindowHandle::Wayland(w) => w.geometry(),
        }
    }
}

/// Open the host window on whichever backend [`backend()`] selected.
pub fn open_window(width: u32, height: u32, title: &str) -> Result<WindowHandle, String> {
    match backend() {
        Backend::Wayland => wayland::open(width, height, title).map(WindowHandle::Wayland),
        Backend::X11 => window::open(width, height, title).map(WindowHandle::X11),
    }
}

/// The active backend's input connection descriptor, for the looper to watch
/// alongside the engine's own fds. `None` before a window has been opened.
pub fn connection_fd() -> Option<c_int> {
    match backend() {
        Backend::Wayland => wayland::current().map(|w| w.connection_fd()),
        Backend::X11 => window::current().map(|w| w.connection_fd()),
    }
}

/// TEMPORARY INSTRUMENTATION -- not for commit.
pub fn backend_instr_geometry() -> String {
    match backend() {
        Backend::Wayland => wayland::instr_geometry(),
        Backend::X11 => "x11".to_string(),
    }
}

/// Drain and deliver whatever host input is queued, for the active backend.
pub fn pump_input_events(handle: i64) {
    match backend() {
        Backend::Wayland => wayland::pump_input_events(handle),
        Backend::X11 => window::pump_input_events(handle),
    }
}

/// Everything the Android layer implements so far.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    let mut v = asset::overrides();
    v.extend(config::overrides());
    v.extend(looper::overrides());
    // Only the selected backend's ANativeWindow_*/EGL overrides are
    // registered — registering both would mean whichever one is NOT hosting
    // the window still answers `ANativeWindow_getWidth` and the rest with
    // whatever its own (unopened) window's defaults are, which is a wrong
    // answer that looks like a right one.
    v.extend(match backend() {
        Backend::Wayland => wayland::overrides(),
        Backend::X11 => window::overrides(),
    });
    if std::env::var_os("CORDIAL_COUNT_GL").is_some() {
        v.extend(glcount::overrides());
    }
    v
}
