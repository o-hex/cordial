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
pub mod clipboard;
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

/// TEMPORARY INSTRUMENTATION -- not for commit.
pub fn backend_set_fullscreen(on: bool) {
    match backend() {
        Backend::Wayland => wayland::instr_set_fullscreen(on),
        // X11 too, and only because a Wayland surface cannot be photographed
        // on this desktop — see `window::HostWindow::set_fullscreen`. A
        // fullscreen transition that can be seen on one backend is worth more
        // than one that can only be counted on the other.
        Backend::X11 => {
            if let Some(w) = window::current() {
                w.set_fullscreen(on);
            }
        }
    }
}

/// Close the window as the close button would, from a scripted run. See
/// [`wayland::instr_close_window`] for why this is a fair test of the real
/// close path and not a shortcut past it.
pub fn backend_close_window() {
    if let Backend::Wayland = backend() {
        wayland::instr_close_window();
    }
}

/// Whether the user has closed the engine's window, for the active backend.
///
/// The X11 backend answers `false` unconditionally rather than growing a second
/// implementation of this: ADR-011 makes X11 the diagnostic fallback, and a
/// closed window there still ends the way it always has, on `--run`.
pub fn window_closed() -> bool {
    match backend() {
        Backend::Wayland => wayland::window_closed(),
        Backend::X11 => false,
    }
}

/// Whether the engine's window currently has focus, for the active backend.
///
/// `None` means "this backend does not know", which is not the same answer as
/// `Some(false)` and must not be collapsed into one: the caller drives
/// `onWindowFocusChangedNative` off this, and telling the engine it has lost
/// focus because nothing was watching would throttle a window the user is
/// looking at. X11 answers `None` for that reason — `window.rs` selects no
/// `FocusChangeMask` and ADR-011 makes X11 the diagnostic fallback, so it keeps
/// the behaviour it has always had rather than growing a second implementation
/// of this.
/// **A window that is not visible does not have focus, whatever the compositor
/// says about activation.**
///
/// Measured over 15 runs: in two of them mutter reported `FOCUSED | SUSPENDED`
/// -- covered, not being drawn, still activated -- for 20 s and 7 s. Throughout,
/// this returned `true`, no `onWindowFocusChangedNative(false)` was sent, and
/// the engine's looper thread spun at 8.5-10 M `ALooper_pollOnce` calls a second
/// for 105% of a core. That is the whole of the "idle at 100% while unfocused"
/// report, and it is the one part of that spin which is ours.
///
/// Folding visibility in is *more* faithful to Android rather than less: an
/// activity that is not visible is stopped and does not hold window focus, so
/// an engine written against that lifecycle is being told the truth by this and
/// was being told something Android would never say by the version before it.
///
/// `None` from either signal still means "not known" and is not collapsed into
/// `Some(false)` -- telling the engine it lost focus because nothing was
/// watching would throttle a window the user is looking at. Only a definite
/// `Some(false)` from visibility overrides a definite `Some(true)` from focus.
/// X11 answers `None` for both and keeps the behaviour it has always had.
pub fn backend_focused() -> Option<bool> {
    let focused = match backend() {
        Backend::Wayland => wayland::focused(),
        Backend::X11 => None,
    };
    match (focused, backend_visible()) {
        (Some(true), Some(false)) => Some(false),
        (f, _) => f,
    }
}

/// Whether the engine's window is visible, for the active backend.
///
/// `None` means "not known", on the same footing and for the same reason as
/// [`backend_focused`]. X11 answers `None`: `window.rs` tracks no visibility
/// and a `_NET_WM_STATE_HIDDEN` reader would be a second implementation of
/// something ADR-011 makes the fallback.
pub fn backend_visible() -> Option<bool> {
    match backend() {
        Backend::Wayland => wayland::visible(),
        Backend::X11 => None,
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
