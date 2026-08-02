//! `ALooper_*` — the per-thread event loop the engine polls.
//!
//! `GameActivity`'s native side runs a loop that calls `ALooper_pollOnce` and
//! dispatches whatever comes back: input, lifecycle, and whatever file
//! descriptors the app registered. It is not optional and it cannot be faked —
//! a stub that returns immediately turns the engine's main loop into a busy spin,
//! and one that never returns hangs it.
//!
//! Android's implementation is epoll plus an eventfd for wakeups, which is
//! exactly what is available here.

use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// How many times the engine has polled. A game thread that is alive and waiting
/// for work shows up here; one that never started does not.
pub static POLLS: AtomicU64 = AtomicU64::new(0);

// Return values from android/looper.h.
pub const POLL_WAKE: c_int = -1;
pub const POLL_CALLBACK: c_int = -2;
pub const POLL_TIMEOUT: c_int = -3;
pub const POLL_ERROR: c_int = -4;

// Event bits.
const EVENT_INPUT: c_int = 1 << 0;
const EVENT_OUTPUT: c_int = 1 << 1;
const EVENT_ERROR: c_int = 1 << 2;
const EVENT_HANGUP: c_int = 1 << 3;

const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLL_CTL_ADD: c_int = 1;
const EPOLL_CTL_DEL: c_int = 2;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct EpollEvent {
    events: u32,
    data: u64,
}

extern "C" {
    fn epoll_create1(flags: c_int) -> c_int;
    fn epoll_ctl(epfd: c_int, op: c_int, fd: c_int, event: *mut EpollEvent) -> c_int;
    fn epoll_wait(epfd: c_int, events: *mut EpollEvent, maxevents: c_int, timeout: c_int)
        -> c_int;
    fn eventfd(initval: u32, flags: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
}

/// A registered descriptor. `ident` is what `pollOnce` reports for it; a
/// registration with a callback uses `POLL_CALLBACK` instead and Android runs
/// the callback itself.
struct Registration {
    fd: c_int,
    ident: c_int,
    callback: Option<extern "C" fn(c_int, c_int, *mut c_void) -> c_int>,
    data: *mut c_void,
}

pub struct Looper {
    epoll: c_int,
    /// Written to by `ALooper_wake`, so a blocked `pollOnce` returns promptly.
    wake: c_int,
    registrations: RefCell<Vec<Registration>>,
    refs: AtomicUsize,
}

thread_local! {
    /// Android's loopers are per-thread and `ALooper_forThread` returns the
    /// calling thread's, so the storage has to be thread-local too. Leaked on
    /// first use: the engine holds the pointer for the thread's lifetime and
    /// there is no point at which returning it would be safe.
    static LOOPER: RefCell<Option<&'static Looper>> = const { RefCell::new(None) };
}

impl Looper {
    fn new() -> Option<&'static Looper> {
        // SAFETY: plain syscall wrappers with no pointer arguments.
        let (epoll, wake) = unsafe { (epoll_create1(0), eventfd(0, 0)) };
        if epoll < 0 || wake < 0 {
            return None;
        }
        let looper = Box::leak(Box::new(Looper {
            epoll,
            wake,
            registrations: RefCell::new(Vec::new()),
            refs: AtomicUsize::new(1),
        }));

        let mut ev = EpollEvent {
            events: EPOLLIN,
            data: looper.wake as u64,
        };
        // SAFETY: `epoll` and `wake` are open descriptors; `ev` is live.
        unsafe { epoll_ctl(looper.epoll, EPOLL_CTL_ADD, looper.wake, &mut ev) };
        Some(looper)
    }

    fn for_thread() -> Option<&'static Looper> {
        LOOPER.with(|l| *l.borrow())
    }

    fn prepare() -> Option<&'static Looper> {
        LOOPER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Looper::new();
            }
            *slot
        })
    }
}

fn epoll_to_looper_events(events: u32) -> c_int {
    let mut out = 0;
    if events & EPOLLIN != 0 {
        out |= EVENT_INPUT;
    }
    if events & EPOLLOUT != 0 {
        out |= EVENT_OUTPUT;
    }
    if events & EPOLLERR != 0 {
        out |= EVENT_ERROR;
    }
    if events & EPOLLHUP != 0 {
        out |= EVENT_HANGUP;
    }
    out
}

/// Give the calling thread a looper.
///
/// Android's framework prepares one on the UI thread before any application
/// code runs, so `ALooper_forThread` never returns null there. AGDK relies on
/// that: `initializeNativeCode` calls `forThread` and bails out immediately if
/// it gets null, returning a zero handle with nothing logged.
///
/// Cordial has no framework doing this, so the thread that drives the Activity
/// has to prepare its own looper first. `forThread` itself stays faithful —
/// creating on demand there would paper over a real "this thread has no looper"
/// error somewhere else.
pub fn prepare_for_current_thread() -> bool {
    Looper::prepare().is_some()
}

/// Pump this thread's looper, as Android's UI thread does.
///
/// AGDK registers its command and input pipes on the looper belonging to the
/// thread that called `initializeNativeCode`, and expects that thread to keep
/// polling. Sleeping instead means the engine's own messages — including the one
/// that says the window is ready — are queued and never delivered, so it sits
/// with a surface it has not been told about and never draws.
///
/// `game_activity_handle`, when set, is also where host input joins this same
/// loop: every ~50ms iteration — the bounded timeout below — is a chance to
/// drain whatever mouse/keyboard events queued up on the active display
/// backend and deliver them through
/// `onTouchEventNative`/`onKeyDownNative`/`onKeyUpNative`, via
/// `android::pump_input_events`, which dispatches to whichever of `window`
/// (X11) or `wayland` is live — see `android::backend`. That function is
/// non-blocking by construction (see its own doc comment), so folding it into
/// this loop does not change this function's own timing behaviour — it is
/// still bounded by the same 50ms `epoll_wait` timeout either way. `None` (no
/// handle) is the case for callers that never bring AGDK up at all, e.g. the
/// app-bridge-only path driven by `CORDIAL_SKIP_AGDK`.
/// Set from the `SIGTERM`/`SIGINT` handler. The only thing a signal handler is
/// allowed to touch here, and the only thing it needs to: the pump notices
/// within one 50ms iteration and takes the same way out as a closed window.
static SIGNALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const SIGINT: c_int = 2;
const SIGTERM: c_int = 15;

extern "C" {
    fn signal(signum: c_int, handler: usize) -> usize;
    #[link_name = "_exit"]
    fn libc_exit(status: c_int) -> !;
}

/// Ask to shut down at the next iteration.
///
/// A second signal does not wait. Teardown drives five calls into the engine
/// and then pumps for half a second, and if any of that hangs — which is a real
/// possibility on a client that is already in trouble, and the reason somebody
/// is sending signals at all — a person pressing Ctrl-C twice expects the
/// process to go, not to be told politely that it is already going. `_exit` is
/// async-signal-safe; nothing else attempted here would be.
extern "C" fn on_terminating_signal(_sig: c_int) {
    if SIGNALLED.swap(true, Ordering::Relaxed) {
        // SAFETY: `_exit` is async-signal-safe by specification, and this is
        // the second signal — the polite path has already been tried.
        unsafe { libc_exit(1) };
    }
}

/// `SIGTERM` and `SIGINT` end the run the same way closing the window does.
///
/// `SIGTERM` is what a plain `kill` sends, what systemd sends, and what the
/// shell sends when it offers to close a client that is holding a profile. It
/// used to kill the process outright, which is survivable — the kernel drops
/// the `flock` on exit however the exit happens — but it also meant the engine
/// never got its shutdown sequence, and Roblox has storage open. Converging on
/// the same teardown as `--run` is what makes a terminated session flush what
/// a timed-out one flushes.
fn install_signal_handlers() {
    // SAFETY: `signal` with a plain `extern "C" fn(c_int)` handler is the
    // oldest interface in C; the handler below touches one atomic and, at
    // worst, `_exit`.
    unsafe {
        signal(SIGTERM, on_terminating_signal as *const () as usize);
        signal(SIGINT, on_terminating_signal as *const () as usize);
    }
}

/// Whether anything has asked this run to end early — a closed window or a
/// terminating signal. `--run` expiring is the third way and is the loop's own
/// condition, so that all three arrive at the same teardown.
fn asked_to_stop() -> Option<&'static str> {
    if SIGNALLED.load(Ordering::Relaxed) {
        return Some("a terminating signal");
    }
    // `CORDIAL_NO_CLOSE_EXIT=1` — the control. With it set, closing the window
    // leaves the process running exactly as it did before any of this existed,
    // so a run that ends can be shown to have ended *because* of the close and
    // not because the timer happened to be short. It is also the reason the
    // signal branch above is not behind the same switch: a control that also
    // disables `kill` is a trap.
    if !no_close_exit() && super::window_closed() {
        return Some("the window closing");
    }
    None
}

fn no_close_exit() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_NO_CLOSE_EXIT").is_some())
}

pub fn pump(duration: std::time::Duration, game_activity_handle: Option<i64>) {
    install_signal_handlers();
    // `--run 0` means no deadline at all: run until the window is closed or a
    // signal arrives. That is what a person playing a game wants — a session
    // should end when they end it, not when a number somebody picked runs out
    // — and it is only safe to offer now that closing the window is a way out.
    // A zero-length run was previously indistinguishable from an instant one,
    // which is not a use anything had.
    let deadline = (!duration.is_zero()).then(|| std::time::Instant::now() + duration);

    // Watch the display connection alongside the engine's own descriptors, so
    // a keypress or a click ends the wait immediately.
    //
    // Without this the loop drained input, then slept in `epoll_wait` for up to
    // 50 ms regardless of what the user did — so an event arriving just after a
    // drain waited out the whole timeout before anything saw it. That is up to
    // 50 ms of latency added to every input, on top of the frame the engine
    // then takes to act on it, and it is pure waiting rather than work.
    //
    // The 50 ms timeout stays, because it is what makes the loop notice
    // `deadline`; it is now the idle period rather than the input period.
    let watching = game_activity_handle.is_some()
        && super::connection_fd().is_some_and(watch_input_fd);

    // TEMPORARY INSTRUMENTATION -- not for commit.
    let instr = std::env::var_os("CORDIAL_INSTR").is_some();
    let start = std::time::Instant::now();
    let mut tick = start;
    let (mut p0, mut q0, mut i0) = (0u64, 0u64, 0u64);
    let mut iters: u64 = 0;
    // `CORDIAL_SCRIPT=60:fullscreen,90:windowed,120:motion-off` -- a timeline of
    // things a human would otherwise have to do by hand, so that one launch
    // covers what would otherwise be several. Fullscreen through
    // `gtk_window_fullscreen` and pointer motion through Cordial's own input
    // path are both allowed; nothing here goes near the compositor.
    let mut script: Vec<(f64, String)> = std::env::var("CORDIAL_SCRIPT")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.split_once(':'))
        .filter_map(|(t, a)| Some((t.trim().parse().ok()?, a.trim().to_string())))
        .collect();
    script.reverse();
    let mut motion = false;

    while deadline.is_none_or(|d| std::time::Instant::now() < d) {
        if let Some(why) = asked_to_stop() {
            println!("[android] ending the run: {why}");
            break;
        }
        iters += 1;
        if instr {
            let t = start.elapsed().as_secs_f64();
            while script.last().is_some_and(|(at, _)| t >= *at) {
                let (_, action) = script.pop().expect("just peeked");
                eprintln!("[instr] t={t:5.1}s script: {action}");
                match action.as_str() {
                    "fullscreen" => super::backend_set_fullscreen(true),
                    "windowed" => super::backend_set_fullscreen(false),
                    "motion-on" => motion = true,
                    "motion-off" => motion = false,
                    // The close button, without a button. This is how the
                    // close-to-exit path is tested: `close` at t=10 should end
                    // the process at t=10 whatever `--run` says, and with
                    // `CORDIAL_NO_CLOSE_EXIT=1` set it should not.
                    "close" => super::backend_close_window(),
                    other => eprintln!("[instr] unknown script action {other}"),
                }
            }
            if motion {
                // Wiggle the pointer inside the canvas through Cordial's own
                // input path. No compositor is involved, so nothing can reach
                // the developer's own session -- see docs/NEXT.md's rule.
                if let Some(handle) = game_activity_handle {
                    let (x, y) = (640.0 + 100.0 * (t as f32).sin(), 360.0 + 100.0 * (t as f32).cos());
                    let ms = (t * 1000.0) as i64;
                    super::input::deliver_touch(
                        handle,
                        super::input::ACTION_HOVER_MOVE,
                        x,
                        y,
                        0,
                        0,
                        ms,
                        0,
                    );
                    super::input::pass_mouse_move(x, y);
                }
            }
        }
        if instr && tick.elapsed() >= std::time::Duration::from_secs(1) {
            let dt = tick.elapsed().as_secs_f64();
            tick = std::time::Instant::now();
            let p = super::glcount::QUEUE_PRESENT.load(Ordering::Relaxed);
            let q = POLLS.load(Ordering::Relaxed);
            eprintln!(
                "[instr] t={:5.1}s presents/s={:6.1} looperpolls/s={:9.0} pumps/s={:6.0} {}",
                start.elapsed().as_secs_f64(),
                (p - p0) as f64 / dt,
                (q - q0) as f64 / dt,
                (iters - i0) as f64 / dt,
                super::backend_instr_geometry(),
            );
            p0 = p;
            q0 = q;
            i0 = iters;
        }
        if let Some(handle) = game_activity_handle {
            super::pump_input_events(handle);
        }
        looper_poll_once(
            if watching { 50 } else { 8 },
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        // The engine's cookie jar is memory-only, so somebody has to write it
        // down. Driven from here rather than from the engine's own `Set-Cookie`
        // callback because that callback arrives on the engine's HTTP thread
        // and reading the jar back from inside it would re-enter the engine on
        // its own thread. Cheap when nothing has changed: one relaxed load.
        crate::cookies::flush_if_dirty();
    }

    // Clean teardown, however the run ended — the timer expiring, the window
    // closing, or a terminating signal. The three converge here on purpose:
    // there is one shutdown sequence and three ways to reach it, rather than a
    // tidy path for the timer and an abrupt one for everything a person
    // actually does. Cordial previously just fell through
    // to `main`'s `_exit(0)` here, which is indistinguishable from the
    // process being killed mid-frame as far as the engine is concerned — it
    // never got a chance to flush the flag cache and telemetry it writes to
    // disk on the way through this chain.
    //
    // The last cookie flush goes *before* that descent, not after: the jar
    // lives in the engine, and after `terminateNativeCode` there is nothing
    // left to read it out of. Unconditional rather than dirty-gated, because
    // the engine only notifies on `Set-Cookie` and a session that was restored
    // at startup and never changed would otherwise not be written back.
    crate::cookies::flush("teardown");
    if let Some(handle) = game_activity_handle {
        teardown(handle);
    }
}

/// The ordered names driving `teardown`, pulled out as a constant so the
/// sequence itself — not just that *something* runs — is checkable by a test
/// without a live `GameActivity` handle. `onWindowFocusChangedNative` is not
/// in this list: it takes a `bool` the other four don't, so it is driven by
/// its own dedicated call (`game_activity::window_focus`) rather than
/// `game_activity::lifecycle`'s by-name lookup.
const TEARDOWN_LIFECYCLE_SEQUENCE: [&str; 4] =
    ["onPauseNative", "onSurfaceDestroyedNative", "onStopNative", "terminateNativeCode"];

/// Android's own shutdown order: `onWindowFocusChangedNative(false)` ->
/// `onPauseNative` -> `onSurfaceDestroyedNative` -> `onStopNative` ->
/// `terminateNativeCode`. Driven synchronously and back-to-back, the same way
/// `cordial_game_activity_start` drives the mirror-image bring-up sequence
/// with no pumping in between.
///
/// `terminateNativeCode` is not exported like `initializeNativeCode` — it is
/// one of the 24 natives AGDK registers dynamically during
/// `initializeNativeCode`, looked up by name exactly like
/// `onPauseNative`/`onStopNative`/`onSurfaceDestroyedNative` (see
/// `game_activity.cpp`'s own doc comment on `cordial_game_activity_lifecycle`
/// for how that was established — `nm -D` on the shipping `libroblox.so`
/// exports only `initializeNativeCode` by that naming scheme).
fn teardown(handle: i64) {
    use cordial_linker_sys::game_activity;

    fn step(name: &str, result: Result<Option<()>, String>) {
        match result {
            Ok(Some(())) => super::trace(format_args!("{name}")),
            // Not registered — a native that never resolved is worth a
            // trace line during teardown even with tracing off elsewhere,
            // since it is the difference between "the engine did not flush"
            // and "Cordial never asked it to".
            Ok(None) => eprintln!("[android] {name}: not registered"),
            Err(e) => eprintln!("[android] {name} failed: {e}"),
        }
    }

    step("onWindowFocusChangedNative(false)", game_activity::window_focus(handle, false));
    for name in TEARDOWN_LIFECYCLE_SEQUENCE {
        step(name, game_activity::lifecycle(handle, name));
    }

    // A brief grace period. The engine's flag-cache/telemetry writes this
    // chain triggers are not guaranteed to be finished by the time
    // `terminateNativeCode` returns to this thread — at least some of that
    // work is plausibly posted to another thread — and pumping a little
    // longer here is what separates a clean write from a log that just stops
    // mid-sentence when the process exits immediately after making these
    // calls. Bounded, not indefinite: teardown must not hang the process it
    // is trying to end cleanly.
    let grace = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < grace {
        looper_poll_once(50, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    }
}

/// Add a descriptor to the calling thread's looper so `pollOnce` returns as soon
/// as it is readable.
///
/// Returns false when there is no looper on this thread or the descriptor
/// cannot be registered, in which case the caller should fall back to polling
/// more often rather than assuming it will be woken.
fn watch_input_fd(fd: c_int) -> bool {
    let Some(l) = Looper::for_thread() else {
        return false;
    };
    let mut ev = EpollEvent { events: EPOLLIN, data: fd as u64 };
    // SAFETY: `l.epoll` is this looper's epoll descriptor and `ev` is live for
    // the call. Re-registering an already-watched fd fails harmlessly.
    let rc = unsafe { epoll_ctl(l.epoll, EPOLL_CTL_ADD, fd, &mut ev) };
    rc == 0
}

// ------------------------------------------------------------------- the API

extern "C" fn looper_prepare(_opts: c_int) -> *mut c_void {
    super::trace(format_args!("ALooper_prepare"));
    Looper::prepare().map_or(std::ptr::null_mut(), |l| l as *const Looper as *mut c_void)
}

extern "C" fn looper_for_thread() -> *mut c_void {
    super::trace(format_args!("ALooper_forThread"));
    Looper::for_thread().map_or(std::ptr::null_mut(), |l| l as *const Looper as *mut c_void)
}

fn as_looper(p: *mut c_void) -> Option<&'static Looper> {
    // SAFETY: every pointer handed out came from a leaked Box that is never
    // freed, so a non-null one is always live.
    (!p.is_null()).then(|| unsafe { &*(p as *const Looper) })
}

extern "C" fn looper_acquire(looper: *mut c_void) {
    if let Some(l) = as_looper(looper) {
        l.refs.fetch_add(1, Ordering::Relaxed);
    }
}

extern "C" fn looper_release(looper: *mut c_void) {
    // The count is tracked but never acted on: the looper is thread-local and
    // leaked, so dropping to zero would mean freeing something the thread may
    // still poll. Android's own loopers outlive their refcount reaching zero in
    // the same way.
    if let Some(l) = as_looper(looper) {
        l.refs.fetch_sub(1, Ordering::Relaxed);
    }
}

extern "C" fn looper_add_fd(
    looper: *mut c_void,
    fd: c_int,
    ident: c_int,
    events: c_int,
    callback: Option<extern "C" fn(c_int, c_int, *mut c_void) -> c_int>,
    data: *mut c_void,
) -> c_int {
    super::trace(format_args!("ALooper_addFd(fd={fd}, ident={ident})"));
    let Some(l) = as_looper(looper) else {
        return -1;
    };

    let mut epoll_events = 0;
    if events & EVENT_INPUT != 0 {
        epoll_events |= EPOLLIN;
    }
    if events & EVENT_OUTPUT != 0 {
        epoll_events |= EPOLLOUT;
    }

    let mut ev = EpollEvent {
        events: epoll_events,
        data: fd as u64,
    };
    // SAFETY: `l.epoll` is open, `fd` is the caller's, `ev` is live.
    if unsafe { epoll_ctl(l.epoll, EPOLL_CTL_ADD, fd, &mut ev) } < 0 {
        return -1;
    }

    l.registrations.borrow_mut().push(Registration {
        fd,
        // With a callback Android reports POLL_CALLBACK rather than the ident.
        ident: if callback.is_some() { POLL_CALLBACK } else { ident },
        callback,
        data,
    });
    1
}

extern "C" fn looper_remove_fd(looper: *mut c_void, fd: c_int) -> c_int {
    let Some(l) = as_looper(looper) else {
        return -1;
    };
    // SAFETY: EPOLL_CTL_DEL ignores the event argument.
    unsafe { epoll_ctl(l.epoll, EPOLL_CTL_DEL, fd, std::ptr::null_mut()) };
    let mut regs = l.registrations.borrow_mut();
    let before = regs.len();
    regs.retain(|r| r.fd != fd);
    if regs.len() < before {
        1
    } else {
        0
    }
}

extern "C" fn looper_wake(looper: *mut c_void) {
    let Some(l) = as_looper(looper) else {
        return;
    };
    let one: u64 = 1;
    // SAFETY: writing the eight bytes an eventfd requires to our own descriptor.
    unsafe { write(l.wake, &one as *const u64 as *const c_void, 8) };
}

extern "C" fn looper_poll_once(
    timeout_millis: c_int,
    out_fd: *mut c_int,
    out_events: *mut c_int,
    out_data: *mut *mut c_void,
) -> c_int {
    let Some(l) = Looper::for_thread() else {
        super::trace(format_args!("ALooper_pollOnce on a thread with no looper"));
        return POLL_ERROR;
    };
    POLLS.fetch_add(1, Ordering::Relaxed);

    let mut events = [EpollEvent { events: 0, data: 0 }; 16];
    // SAFETY: `events` is a live array of the length passed.
    let n = unsafe { epoll_wait(l.epoll, events.as_mut_ptr(), events.len() as c_int, timeout_millis) };
    if n < 0 {
        return POLL_ERROR;
    }
    if n == 0 {
        return POLL_TIMEOUT;
    }

    for ev in events.iter().take(n as usize) {
        let fd = ev.data as c_int;

        if fd == l.wake {
            let mut sink = 0u64;
            // SAFETY: draining the eight bytes written by looper_wake.
            unsafe { read(l.wake, &mut sink as *mut u64 as *mut c_void, 8) };
            return POLL_WAKE;
        }

        let (ident, callback, data) = {
            let regs = l.registrations.borrow();
            match regs.iter().find(|r| r.fd == fd) {
                Some(r) => (r.ident, r.callback, r.data),
                None => continue,
            }
        };
        let looper_events = epoll_to_looper_events(ev.events);

        if let Some(cb) = callback {
            // The registration is not borrowed across this call: a callback is
            // entitled to add or remove descriptors, and holding the borrow would
            // panic when it did.
            if cb(fd, looper_events, data) == 0 {
                looper_remove_fd(l as *const Looper as *mut c_void, fd);
            }
            return POLL_CALLBACK;
        }

        if !out_fd.is_null() {
            // SAFETY: caller-provided out-parameters, checked for null.
            unsafe { *out_fd = fd };
        }
        if !out_events.is_null() {
            // SAFETY: as above.
            unsafe { *out_events = looper_events };
        }
        if !out_data.is_null() {
            // SAFETY: as above.
            unsafe { *out_data = data };
        }
        return ident;
    }

    POLL_TIMEOUT
}

pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("ALooper_prepare", looper_prepare),
        f!("ALooper_forThread", looper_for_thread),
        f!("ALooper_acquire", looper_acquire),
        f!("ALooper_release", looper_release),
        f!("ALooper_addFd", looper_add_fd),
        f!("ALooper_removeFd", looper_remove_fd),
        f!("ALooper_pollOnce", looper_poll_once),
        f!("ALooper_wake", looper_wake),
    ]
}

#[cfg(test)]
mod tests {
    // Only the tests close a descriptor, so the binding lives with them rather
    // than in the module's extern block, where it read as dead code.
    extern "C" {
        fn close(fd: c_int) -> c_int;
    }

    use super::*;

    #[test]
    fn teardown_lifecycle_sequence_matches_androids_shutdown_order() {
        // Regression guard on the ordering itself, not just that `teardown`
        // calls something: onPause before onSurfaceDestroyed before onStop
        // before terminateNativeCode is the order the report specifies
        // Android actually uses, and a reorder here would be a real (if
        // subtle) behaviour change even though every step still ran.
        assert_eq!(
            TEARDOWN_LIFECYCLE_SEQUENCE,
            ["onPauseNative", "onSurfaceDestroyedNative", "onStopNative", "terminateNativeCode"]
        );
    }

    #[test]
    fn teardown_returns_within_its_grace_period_with_no_native_handle() {
        // A test process links no libroblox.so and starts no JavaVM, so every
        // `game_activity::*` call `teardown` makes fails immediately (see
        // `process_env`'s null-VM check) and this exercises only the bounded
        // grace-period loop. Regression guard: that loop must be bounded
        // rather than spin forever if the engine's natives never resolve.
        let looper = looper_prepare(0);
        assert!(!looper.is_null());
        let start = std::time::Instant::now();
        teardown(0);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "teardown took too long; its grace period must be bounded"
        );
    }

    #[test]
    fn poll_times_out_rather_than_spinning() {
        let looper = looper_prepare(0);
        assert!(!looper.is_null());
        let start = std::time::Instant::now();
        assert_eq!(looper_poll_once(50, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()), POLL_TIMEOUT);
        // A stub returning immediately would turn the engine's main loop into a
        // busy spin, which is the failure this implementation exists to avoid.
        assert!(start.elapsed().as_millis() >= 40, "pollOnce returned without waiting");
    }

    #[test]
    fn wake_interrupts_a_blocked_poll() {
        let looper = looper_prepare(0);
        looper_wake(looper);
        assert_eq!(
            looper_poll_once(1000, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()),
            POLL_WAKE
        );
    }

    #[test]
    fn a_readable_fd_is_reported_with_its_ident() {
        let looper = looper_prepare(0);
        // SAFETY: creating and writing to our own eventfd.
        let fd = unsafe { eventfd(1, 0) };
        assert!(fd >= 0);

        assert_eq!(looper_add_fd(looper, fd, 42, EVENT_INPUT, None, std::ptr::null_mut()), 1);

        let (mut out_fd, mut out_events) = (0, 0);
        let mut out_data = std::ptr::null_mut();
        let rc = looper_poll_once(500, &mut out_fd, &mut out_events, &mut out_data);
        assert_eq!(rc, 42, "pollOnce must report the ident the fd was registered with");
        assert_eq!(out_fd, fd);
        assert_ne!(out_events & EVENT_INPUT, 0);

        assert_eq!(looper_remove_fd(looper, fd), 1);
        // SAFETY: closing the descriptor this test opened.
        unsafe { close(fd) };
    }
}
