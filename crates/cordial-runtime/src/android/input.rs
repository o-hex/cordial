//! Display-server-independent input plumbing, shared by [`super::window`] (X11)
//! and [`super::wayland`].
//!
//! X11 delivers keysyms directly. Wayland delivers raw evdev keycodes plus an
//! xkb keymap the client has to interpret itself — see ADR-011. Everything
//! *below* that difference is identical: both backends end up with a keysym, a
//! button number, or committed text, and from there the two paths converge.
//! This module is that convergence point. It used to live inside `window.rs`,
//! written for the only backend that existed; the text-entry state machine in
//! particular (`TextField`, the caret arithmetic, the reseed-on-focus-change
//! logic) took real iteration to get right — see the tests below — and a
//! second display backend is exactly the situation duplicating it would have
//! caused a second, silently-diverging copy of the same bugs to be fixed twice.
//!
//! What stays behind in each backend is the part that is genuinely
//! display-specific: opening a connection, reading its events, and turning
//! them into the keysym/button/text vocabulary this module speaks.

use std::ffi::{c_ulong, c_void};
use std::sync::{Mutex, OnceLock};

// --------------------------------------------------------- Android vocabulary
//
// `android.view.MotionEvent`/`KeyEvent` constants both backends synthesise
// events against, via `deliver_touch`/`deliver_key` below.

pub const BUTTON_PRIMARY: i32 = 1;
pub const BUTTON_SECONDARY: i32 = 2;
pub const BUTTON_TERTIARY: i32 = 4;

/// `nativePassMouseButton`'s own button index, which is not Android's bitmask.
///
/// Only one value here is established: 0 is the left button, because that is
/// what Cordial has always sent and clicking Roblox's interface works. The
/// other two are `INFERRED` from Roblox's own `Enum.UserInputType`, where
/// `MouseButton1`/`2`/`3` are left/right/middle in that order — a zero-based
/// index whose 0 is the left button is that enum minus one. The dex declares
/// the parameter as a bare `I` and strips parameter names, so nothing readable
/// settles it; a human with a mouse does, in one click.
///
/// Getting this wrong is not silent: the right button would act as some other
/// button rather than doing nothing.
pub fn roblox_mouse_button(android_button: i32) -> i32 {
    match android_button {
        BUTTON_SECONDARY => 1,
        BUTTON_TERTIARY => 2,
        _ => 0,
    }
}
pub const ACTION_DOWN: i32 = 0;
pub const ACTION_UP: i32 = 1;
pub const ACTION_MOVE: i32 = 2;
pub const ACTION_HOVER_MOVE: i32 = 7;
pub const ACTION_BUTTON_PRESS: i32 = 11;
pub const ACTION_BUTTON_RELEASE: i32 = 12;
/// `ACTION_SCROLL`. Not delivered by [`deliver_touch`] — a scroll carries no
/// button state and no gesture start, so it has its own call; see
/// [`deliver_scroll`].
pub const ACTION_SCROLL: i32 = 8;

// `android.view.KeyEvent.META_*`.
pub const META_SHIFT_ON: i32 = 1;
pub const META_ALT_ON: i32 = 2;
pub const META_CTRL_ON: i32 = 0x1000;
pub const META_CAPS_LOCK_ON: i32 = 0x100000;

/// A pragmatic subset of keysyms mapped to `android.view.KeyEvent.KEYCODE_*`.
///
/// The values are X11's `keysymdef.h` numbering, but that numbering is not an
/// X11 peculiarity — it is the shared keysym space `xkbcommon` also uses (the
/// "xkb" in the name is literally "X Keyboard extension"), which is what makes
/// this table usable from both backends rather than needing a second one keyed
/// on evdev codes. Covers what a desktop text field and basic UI navigation
/// need — letters, digits, common punctuation, arrows, and the usual control
/// keys. Anything outside this set is dropped rather than guessed at.
pub fn keysym_to_android(keysym: c_ulong) -> Option<i32> {
    let k = keysym as u32;
    Some(match k {
        0x30..=0x39 => 7 + (k - 0x30) as i32,  // 0..9 -> AKEYCODE_0..9
        0x61..=0x7a => 29 + (k - 0x61) as i32, // a..z -> AKEYCODE_A..Z
        0x41..=0x5a => 29 + (k - 0x41) as i32, // A..Z (shifted) -> the same keycodes
        0x0020 => 62,                          // space
        0xff0d | 0xff8d => 66,                 // Return, KP_Enter
        0xff08 => 67,                          // BackSpace
        0xff09 => 61,                          // Tab
        0xff1b => 111,                         // Escape
        0xff51 => 21,                          // Left
        0xff52 => 19,                          // Up
        0xff53 => 22,                          // Right
        0xff54 => 20,                          // Down
        0xffe1 => 59,                          // Shift_L
        0xffe2 => 60,                          // Shift_R
        0xffe3 => 113,                         // Control_L
        0xffe4 => 114,                         // Control_R
        0xffe9 => 57,                          // Alt_L
        0xffea => 58,                          // Alt_R
        0xffe5 => 115,                         // Caps_Lock
        0xffff => 112,                         // Delete (forward delete)
        0xff50 => 122,                         // Home
        0xff57 => 123,                         // End
        0xff55 => 92,                          // Page_Up
        0xff56 => 93,                          // Page_Down
        0xff63 => 124,                         // Insert
        0x002c => 55,                          // comma
        0x002e => 56,                          // period
        0x002f => 76,                          // slash
        0x003b => 74,                          // semicolon
        0x0027 => 75,                          // apostrophe
        0x0060 => 68,                          // grave
        0x002d => 69,                          // minus
        0x003d => 70,                          // equal
        0x005b => 71,                          // bracketleft
        0x005d => 72,                          // bracketright
        0x005c => 73,                          // backslash
        _ => return None,
    })
}

/// Say that a native the input path wanted is not there — at the first drop,
/// and then at each power of ten.
///
/// "Not there" covers both ways it happens: an AGDK native that
/// `initializeNativeCode` has not put in the natives table, and a
/// `NativeInputInterface`/`NativeGLInterface` export the loader could not
/// resolve. Both end the same way, with an input event going nowhere.
///
/// `Ok(None)` used to be silent everywhere in this file, on the grounds that a
/// call arriving before `initializeNativeCode` has finished is a normal startup
/// race. The cost of that silence was measured the hard way: a session run with
/// `CORDIAL_ANDROID_TRACE=1`, pressing keys in an experience, printed no
/// `onKeyDownNative` line at all — and "the trace said nothing" and "the engine
/// never received the key" were the same observation, with no way to tell them
/// apart without changing the code first.
///
/// Not once, and not per event. Once is indistinguishable from a startup race;
/// per event would bury the log under one line per keystroke. At decade
/// boundaries a race prints a single line and a native that never registers
/// keeps coming back, which is the distinction that was missing.
///
/// Deliberately not behind `CORDIAL_ANDROID_TRACE`: input being dropped on the
/// floor is not tracing, and the one run where it mattered had the flag on and
/// still learned nothing.
pub(crate) fn report_unregistered(name: &'static str) {
    crate::unimplemented::record(crate::unimplemented::Kind::NativeNotRegistered, name);
    static DROPPED: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());
    let n = {
        let mut seen = DROPPED.lock().unwrap_or_else(|e| e.into_inner());
        match seen.iter_mut().find(|(k, _)| *k == name) {
            Some((_, count)) => {
                *count += 1;
                *count
            }
            None => {
                seen.push((name, 1));
                1
            }
        }
    };
    let decade = {
        let mut d = 1u64;
        while d < n {
            d = d.saturating_mul(10);
        }
        d == n
    };
    if decade {
        eprintln!(
            "[android] {name} is not registered in the natives table (or was not \
             resolved at load); {n} input event(s) dropped so far, reported at \
             each power of ten. A single line early in startup is the normal \
             race against initializeNativeCode; a line that keeps returning is not."
        );
    }
}

/// Deliver one AGDK touch event, the same `MotionEvent` synthesis both
/// backends drive their pointer input through.
#[allow(clippy::too_many_arguments)]
pub fn deliver_touch(
    handle: i64,
    action: i32,
    x: f32,
    y: f32,
    button_state: i32,
    action_button: i32,
    event_time_ms: i64,
    down_time_ms: i64,
) {
    if no_agdk_touch() {
        return;
    }
    match cordial_linker_sys::game_activity::touch(
        handle,
        action,
        x,
        y,
        button_state,
        action_button,
        event_time_ms,
        down_time_ms,
    ) {
        Ok(Some(consumed)) => {
            super::trace(format_args!("onTouchEventNative(action={action}) -> {consumed}"))
        }
        Ok(None) => report_unregistered("onTouchEventNative"),
        Err(e) => super::trace(format_args!("onTouchEventNative(action={action}) failed: {e}")),
    }
}

/// Deliver one AGDK wheel event as `ACTION_SCROLL` with the scroll axes filled.
///
/// Private, and reached only through [`wheel`], so that the sign and scale
/// policy cannot be applied to one of the two wheel paths and not the other.
fn deliver_scroll(handle: i64, x: f32, y: f32, hscroll: f32, vscroll: f32, event_time_ms: i64) {
    if no_agdk_touch() {
        return;
    }
    match cordial_linker_sys::game_activity::scroll(handle, x, y, hscroll, vscroll, event_time_ms) {
        Ok(Some(consumed)) => super::trace(format_args!(
            "onTouchEventNative(ACTION_SCROLL h={hscroll} v={vscroll}) -> {consumed}"
        )),
        Ok(None) => report_unregistered("onTouchEventNative"),
        Err(e) => super::trace(format_args!("onTouchEventNative(ACTION_SCROLL) failed: {e}")),
    }
}

/// Deliver one AGDK key event, the `KeyEvent` synthesis both backends drive.
pub fn deliver_key(
    handle: i64,
    down: bool,
    key_code: i32,
    scan_code: i32,
    meta_state: i32,
    repeat_count: i32,
    unicode_char: i32,
    event_time_ms: i64,
    down_time_ms: i64,
) {
    if no_agdk_key() {
        return;
    }
    match cordial_linker_sys::game_activity::key(
        handle,
        down,
        key_code,
        scan_code,
        meta_state,
        repeat_count,
        unicode_char,
        event_time_ms,
        down_time_ms,
    ) {
        Ok(Some(consumed)) => {
            super::trace(format_args!("onKey{}Native(code={key_code}) -> {consumed}",
                if down { "Down" } else { "Up" }))
        }
        Ok(None) => report_unregistered(if down { "onKeyDownNative" } else { "onKeyUpNative" }),
        Err(e) => super::trace(format_args!(
            "onKey{}Native(code={key_code}) failed: {e}",
            if down { "Down" } else { "Up" }
        )),
    }
}

pub fn deliver_surface_redraw(handle: i64) {
    match cordial_linker_sys::game_activity::surface_redraw_needed(handle) {
        Ok(Some(())) => super::trace(format_args!("onSurfaceRedrawNeededNative")),
        Ok(None) => report_unregistered("onSurfaceRedrawNeededNative"),
        Err(e) => super::trace(format_args!("onSurfaceRedrawNeededNative failed: {e}")),
    }
}

// ------------------------------------------------------------ native passthrough
//
// The two `NativeInputInterface` natives Roblox's interface actually reads.
//
// Resolved once by the loader and stored here, because the input drain runs on
// the looper thread and has no access to the loaded library. Null until set, in
// which case only the AGDK path is driven — which is what shipped before, and
// which the interface ignores.
static PASS_MOUSE_MOVE: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static PASS_MOUSE_BUTTON: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static PASS_MOUSE_WHEEL: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static PASS_KEY_EVENT: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static PASS_TEXT: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// `syncTextboxTextAndCursorPosition2`. Separate from `PASS_TEXT` because it is
/// a different call at a different moment, not an alternative spelling of one.
static SYNC_TEXTBOX: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// `updateKeyboardSize`, the acknowledgement that an editor is up.
static UPDATE_KEYBOARD_SIZE: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// `nativeGetMainWindowIsMouseLockedCenter`. See
/// [`engine_wants_pointer_lock`].
static GET_MOUSE_LOCKED_CENTER: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// Focus generation the keyboard state was last reported for.
static KEYBOARD_REPORTED: Mutex<Option<u32>> = Mutex::new(None);

#[allow(clippy::too_many_arguments)]
pub fn set_input_natives(
    mouse_move: *mut c_void,
    mouse_button: *mut c_void,
    mouse_wheel: *mut c_void,
    key_event: *mut c_void,
    pass_text: *mut c_void,
    sync_textbox: *mut c_void,
    update_keyboard_size: *mut c_void,
) {
    PASS_MOUSE_MOVE.store(mouse_move, std::sync::atomic::Ordering::Relaxed);
    PASS_MOUSE_BUTTON.store(mouse_button, std::sync::atomic::Ordering::Relaxed);
    PASS_MOUSE_WHEEL.store(mouse_wheel, std::sync::atomic::Ordering::Relaxed);
    PASS_KEY_EVENT.store(key_event, std::sync::atomic::Ordering::Relaxed);
    PASS_TEXT.store(pass_text, std::sync::atomic::Ordering::Relaxed);
    SYNC_TEXTBOX.store(sync_textbox, std::sync::atomic::Ordering::Relaxed);
    UPDATE_KEYBOARD_SIZE.store(update_keyboard_size, std::sync::atomic::Ordering::Relaxed);
}

/// `NativeInputInterface.nativeGetMainWindowIsMouseLockedCenter()Z`, resolved
/// separately from [`set_input_natives`] because it is the only one of these
/// that Cordial *reads* rather than writes.
pub fn set_mouse_lock_native(native: *mut c_void) {
    GET_MOUSE_LOCKED_CENTER.store(native, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the engine currently wants the pointer locked to the centre of the
/// window — first person, or anything else that turns the camera with the mouse
/// rather than with a cursor.
///
/// `None` means Cordial does not know, and that is a third answer rather than a
/// polite `false`: the native may not be exported by this build, may not be
/// resolvable yet during startup, or may have failed. A caller that treats
/// "unknown" as "no lock wanted" is making the stub lie in the sense AGENTS.md
/// means it; here the caller keeps its own drag-driven lock instead, which is
/// honest about resting on something other than the engine's word.
///
/// **The direction of this call is the hypothesis worth being explicit about.**
/// The native is a getter on `NativeInputInterface` and had never been called by
/// Cordial, so nothing about a running session distinguishes "the platform is
/// supposed to poll it" from "the engine calls something else and this is dead".
/// Android has no pointer to lock, so the real client on real Android may never
/// have a true answer to give — and a false that never changes is exactly what
/// a dead getter and an idle one both look like. `CORDIAL_TRACE_MOUSE=1` prints
/// every transition so that a session in first person settles it; until one
/// has, the *engine-driven* half of pointer capture is `INFERRED` and the
/// drag-driven half is not.
///
/// Called through `call_static_bare_bool`, the same `(JNIEnv*, jclass)` shape
/// every other `NativeInputInterface` native here is called with — see
/// `native/game_activity.cpp`'s `cordial_input_mouse_move`, which passes the
/// class object in exactly this position.
pub fn engine_wants_pointer_lock() -> Option<bool> {
    let f = GET_MOUSE_LOCKED_CENTER.load(std::sync::atomic::Ordering::Relaxed);
    if f.is_null() {
        return None;
    }
    // A native that throws once will throw every pump, and 20 identical lines a
    // second buries whatever the session was actually about. One line, then
    // never again — and never again also means never *called* again, because
    // the failure was in the call itself.
    static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if FAILED.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    match cordial_linker_sys::game_activity::call_static_bare_bool(
        f,
        "com/roblox/engine/jni/NativeInputInterface",
    ) {
        Ok(v) => {
            if trace_mouse() {
                static LAST: Mutex<Option<bool>> = Mutex::new(None);
                let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
                if *last != Some(v) {
                    *last = Some(v);
                    eprintln!("[cordial] nativeGetMainWindowIsMouseLockedCenter() -> {v}");
                }
            }
            Some(v)
        }
        Err(e) => {
            FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "[cordial] nativeGetMainWindowIsMouseLockedCenter() failed ({e}); \
                 not asking again this session. Pointer capture now depends on the \
                 mouse button alone."
            );
            None
        }
    }
}

/// Tell the engine whether an editor is up, when that has changed.
///
/// This closes the handshake `showKeyboard` opens. It runs from the input pump
/// rather than from inside `showKeyboard` itself because on Android the reply
/// comes from the UI thread after the IME has actually appeared, not
/// synchronously from within the request — and calling back into the engine
/// from inside its own call is a re-entry this has no reason to risk.
pub fn report_keyboard_state(current_geometry: (i32, i32)) {
    // `CORDIAL_NO_KEYBOARD_REPORT=1` suppresses this entirely, and by default it
    // is suppressed — see `keyboard_report_enabled` for the measurement.
    if !keyboard_report_enabled() {
        return;
    }
    let f = UPDATE_KEYBOARD_SIZE.load(std::sync::atomic::Ordering::Relaxed);
    if f.is_null() {
        return;
    }
    let generation = cordial_linker_sys::game_activity::textbox_generation();
    {
        let mut seen = KEYBOARD_REPORTED.lock().unwrap_or_else(|e| e.into_inner());
        if *seen == Some(generation) {
            return;
        }
        *seen = Some(generation);
    }
    // The dex declares this as `updateKeyboardSize(Z, I, I, I, I)`, and the
    // real-Android capture pins the argument order and the resting value. Its
    // Java-side layout callback logs, twice, at surface bring-up:
    //
    //     rbx.glview.layout: onUpdateKeyboardSize() v:false x:0 y:999 w:2491 h:0
    //
    // So it is (visible, x, y, width, height), and the keyboard-hidden baseline
    // the real client reports is *not* an empty rectangle: it is visible=false
    // with the box still pinned to the bottom edge of the UI space, full width,
    // zero height. Cordial's rectangle was already that shape; only the boolean
    // was wrong. It used to send `visible=true` with a zero height, which claims
    // a soft keyboard is on screen and simultaneously that it covers nothing,
    // and that was measured to bounce focus continuously.
    //
    // A desktop genuinely has no soft keyboard, so the resting baseline is the
    // truthful report here as well as the observed one, and it does not become
    // less true when a box takes focus. Do not "fix" this by zeroing x/y/w
    // as well: an all-zero rectangle is a third value nothing has ever been
    // seen to send. `INFERRED` only in that the capture shows the app's own
    // Java callback rather than the JNI call it feeds; the shape is observed,
    // the 1:1 with the native call is not.
    let (w, h) = current_geometry;
    let r = cordial_linker_sys::game_activity::update_keyboard_size(f, false, 0, h, w, 0);
    if trace_text() {
        eprintln!("[cordial] updateKeyboardSize(visible=false, x=0, y={h}, w={w}, h=0) -> {r:?}");
    }
}

/// `NativeGLInterface.nativePassKeyEvent(Z down, I keyCode, I modifiers, Z isRepeat)`.
///
/// Traced, because it had never once been observed. Every keyboard
/// investigation here has read `onKeyDownNative` lines, and this is the *other*
/// path a keystroke takes — the one the interface actually reads, by the same
/// argument that made `nativePassMouseButton` rather than `onTouchEventNative`
/// the thing that moves the UI. A path with no instrumentation cannot be ruled
/// in or out, and both were being ruled on from the same silence.
///
/// `key_code` is an `android.view.KeyEvent.KEYCODE_*`, produced by
/// [`keysym_to_android`]. It is emphatically *not* an evdev code, and the trace
/// prints it so that a run can say which of the two the engine is being handed
/// rather than a reader having to trust the call chain. The two numbering
/// schemes agree at exactly one letter — evdev `KEY_D` and `AKEYCODE_D` are
/// both 32 — so "only D works" is the signature of a raw evdev code reaching
/// something that wanted an Android one, and one traced keystroke settles it.
/// `NativeInputInterface.nativePassKeyEvent(Z down, I code, I modifiers, Z repeat)`.
///
/// **`code` is a Linux evdev code, not an Android keycode**, and getting that
/// backwards cost days. The symptom was that exactly one key worked — `D` — and
/// that holding Alt made the character *jump*.
///
/// Both fall out of the same arithmetic. `AKEYCODE_D` is 32 and `KEY_D` is 32,
/// so `D` worked by pure collision and hid the problem. `AKEYCODE_ALT_LEFT` is
/// 57 and `KEY_SPACE` is 57, so Alt read as Space, and Space is jump. The rest
/// simply landed on codes with no meaning: `W` went as `AKEYCODE_W` 51, which is
/// `KEY_COMMA`; `A` as 29, which is `KEY_LEFTCTRL`; `S` as 47, which is `KEY_V`.
///
/// Four theories were measured and disproved before this, all of them assuming a
/// number was wrong somewhere in a translation table. The number was fine. It
/// was the *vocabulary* — every one of them took for granted that this native
/// wanted what AGDK's `onKeyDownNative` wants, and it does not. Note the
/// signature has no scan-code slot at all, which is the tell: a native that
/// takes one code and no scan code is taking the platform's own.
///
/// `CORDIAL_KEY_ANDROID_CODES=1` restores the old behaviour as a control.
pub fn pass_key_event(down: bool, evdev_code: i32, modifiers: i32) {
    if no_pass_key() {
        return;
    }
    track_key_held(down, evdev_code);
    let key_code = evdev_code;
    let f = PASS_KEY_EVENT.load(std::sync::atomic::Ordering::Relaxed);
    if f.is_null() {
        report_unregistered("nativePassKeyEvent");
        return;
    }
    let r = cordial_linker_sys::game_activity::pass_key_event(f, down, key_code, modifiers, false);
    super::trace(format_args!(
        "nativePassKeyEvent(down={down}, keyCode={key_code}, modifiers={modifiers:#x}) -> {r:?}"
    ));
}

/// Which evdev codes are currently held, tracked here rather than in
/// `window.rs`/`wayland.rs` because both backends already funnel every key
/// transition through this one function. A `Vec` rather than a `HashSet`: the
/// realistic size is single digits (nobody holds more than a few keys at
/// once), so a linear scan is cheaper than hashing and `Vec::new()` is a
/// `const fn`, which a `HashSet` field on a static would complicate for no
/// measured benefit.
///
/// This exists for [`idle_keepalive`] — see that function for what it is
/// tracking held keys *for*.
static KEYS_HELD: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn track_key_held(down: bool, evdev_code: i32) {
    let mut held = KEYS_HELD.lock().unwrap_or_else(|e| e.into_inner());
    if down {
        if !held.contains(&evdev_code) {
            held.push(evdev_code);
        }
    } else {
        held.retain(|&c| c != evdev_code);
    }
}

/// Send a zero-delta `nativePassMouseMove` while a key is held, so the engine's
/// own idle throttle does not mistake "walking in a straight line without
/// touching the mouse" for nobody playing.
///
/// **What this is answering.** `docs/NEXT.md` §1d established that presents
/// collapse from ~60/s to exactly 1.0/s about thirteen seconds into an idle
/// app shell, and that driving `pass_mouse_move` continuously holds it at
/// 50-60/s indefinitely — but that measurement drove mouse movement, camera
/// look and held keys all together, so it could not say which one the engine
/// was actually watching. Measured in isolation (`CORDIAL_SCRIPT=key-on`,
/// `touch-on`, `look-on`, `ping-on` against a real `libroblox.so`, landing
/// page, no account): a single held key produces exactly one down event under
/// Wayland (`keyboard_repeat_info` in `wayland.rs` is a documented no-op, and
/// nothing here reintroduces repeat), and that one event does not stop the
/// collapse — it lands at the same ~15s mark as no input at all, twice.
/// Redriving `deliver_key`/`pass_key_event` on every tick, simulating what a
/// repeat timer would send, does not stop it either. Redriving
/// `deliver_touch`'s AGDK touch queue every tick does not stop it. Only
/// `pass_mouse_move` — `NativeInputInterface.nativePassMouseMove`, the "V2"
/// interface call, not AGDK's `onTouchEventNative` — keeps it away, and it
/// does so with the delta held at exactly zero: a fixed position resent every
/// tick holds presents at a flat 60.0/s for the whole run, no less reliably
/// than a moving one, and collapses within about a second of stopping. So the
/// engine is watching this one call landing, not the camera actually turning.
///
/// **Why a real position matters.** The dex declares this native as an
/// absolute position plus a delta, and `MOUSE_LAST` is the last position a
/// genuine pointer event reported — reusing it, with a (0, 0) delta, tells the
/// engine truthfully where the pointer already is and that it has not moved,
/// which is honest in both halves. Inventing a position (window centre, say)
/// would tell the engine the pointer jumped there, and Roblox's UI does hit
/// testing against the reported absolute position — a jump risks nudging
/// whatever the real cursor happens to be hovering. If no genuine pointer
/// event has ever landed there is nothing honest to resend, so this does
/// nothing rather than guess.
///
/// Called from [`super::looper::pump`] every tick a key is held; harmless to
/// call when the interface native is not registered yet, since
/// [`pass_mouse_move_delta`] already falls through to
/// [`report_unregistered`]'s deduplicated logging rather than spamming.
/// When Cordial stops sending [`idle_keepalive`], from `CORDIAL_THROTTLE`.
///
/// The shell's "Slow the game down in the background" row sets this; see
/// `cordial_shell::shell_config::ThrottleWhen`, which holds the reasoning for
/// why `Visible` is the default rather than `Unfocused`. Parsed once, because
/// the launch settles it and nothing changes it mid-run.
///
/// **This governs the keepalive only.** `onWindowFocusChangedNative` is driven
/// on every genuine transition whatever this says — the engine is told the
/// truth about focus and this decides what Cordial does about it. Anything
/// unrecognised, including the variable being absent, is `Visible`: an old
/// shell launching a new client, or a client started by hand, gets the default
/// rather than a refusal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThrottleWhen {
    Visible,
    Unfocused,
    Off,
}

pub fn throttle_policy() -> ThrottleWhen {
    static POLICY: std::sync::OnceLock<ThrottleWhen> = std::sync::OnceLock::new();
    *POLICY.get_or_init(|| match std::env::var("CORDIAL_THROTTLE").as_deref() {
        Ok("unfocused") => ThrottleWhen::Unfocused,
        Ok("off") => ThrottleWhen::Off,
        _ => ThrottleWhen::Visible,
    })
}

/// Whether the keepalive should run this tick.
///
/// `focused` and `visible` are `None` when the backend does not track them —
/// X11 tracks neither — and `None` keeps the keepalive running. That is
/// deliberate and is the whole reason these are three-valued: throttling a
/// window because nothing was watching it would be the same class of bug as
/// never throttling at all, arriving from the other side.
///
/// Pure, and separate from [`idle_keepalive`], so the policy table is testable
/// without a window, a compositor or a loaded engine.
pub fn keepalive_wanted(policy: ThrottleWhen, focused: Option<bool>, visible: Option<bool>) -> bool {
    match policy {
        ThrottleWhen::Off => true,
        ThrottleWhen::Unfocused => focused != Some(false),
        // Not `visible != Some(false)` alone. A minimised window reports both
        // unfocused and not visible, but a compositor that reports neither
        // `SUSPENDED` nor `MINIMIZED` for a window it has hidden would leave
        // this setting doing nothing at all; losing focus is the weaker
        // signal and is not sufficient on its own, so it is not consulted
        // here. If that compositor turns up, this is where the second
        // condition goes.
        ThrottleWhen::Visible => visible != Some(false),
    }
}

pub fn idle_keepalive() {
    let any_held = !KEYS_HELD.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
    if !any_held {
        return;
    }
    if let Some((x, y)) = *MOUSE_LAST.lock().unwrap_or_else(|e| e.into_inner()) {
        pass_mouse_move_delta(x, y, 0.0, 0.0);
    }
}

pub fn pass_text(which: i64, text: &str, cursor: i32) {
    // The per-keystroke sync first: this is the call that actually fills the
    // field. `nativePassText` is driven alongside it for the same reason both
    // mouse paths are — the interface declares both and the cost of driving
    // one that turns out to be a no-op is nothing.
    let sync = SYNC_TEXTBOX.load(std::sync::atomic::Ordering::Relaxed);
    if !sync.is_null() {
        if let Err(e) = cordial_linker_sys::game_activity::sync_textbox(sync, text, cursor) {
            if trace_text() {
                eprintln!("[cordial] syncTextbox failed: {e}");
            }
        }
    }
    // `nativePassText` is deliberately NOT driven per keystroke.
    //
    // The two calls are not alternatives. `syncTextboxTextAndCursorPosition2`
    // takes no box handle and updates whichever box has focus — that is the
    // per-keystroke update. `nativePassText` takes the handle `showKeyboard`
    // issued and is the *finish* call: on Android it is the soft keyboard
    // delivering its final text and dismissing itself.
    //
    // Driving both on every character meant typing one letter and then hanging
    // up, which is precisely what the trace showed — the character landed and
    // the box immediately lost focus:
    //
    //     textbox focused ... current=0 bytes
    //     key down "g" focus=Some(140515299098752)
    //     text -> "g" caret=1
    //     textbox blurred
    //     ... textbox focused ... current=1 bytes   <- the "g" was accepted
    //
    // So the field really was receiving the text; every keystroke also ended
    // the editing session, which is why it needed re-clicking per character and
    // why no caret ever persisted.
    //
    // `CORDIAL_PASS_TEXT_ON_KEY=1` restores the old behaviour for anyone
    // testing this claim rather than taking it on trust.
    let f = if std::env::var_os("CORDIAL_PASS_TEXT_ON_KEY").is_some() {
        PASS_TEXT.load(std::sync::atomic::Ordering::Relaxed)
    } else {
        std::ptr::null_mut()
    };
    if !f.is_null() {
        // `nativePassText(long, String, boolean, int)`. The boolean's meaning is
        // not declared anywhere Cordial can read, so it stays a knob until a run
        // settles it: `CORDIAL_PASSTEXT_FLAG=1` sends true.
        let flag = std::env::var_os("CORDIAL_PASSTEXT_FLAG").is_some();
        if let Err(e) = cordial_linker_sys::game_activity::pass_text(f, which, text, flag, cursor) {
            if trace_text() {
                eprintln!("[cordial] passText failed: {e}");
            }
        }
    }
    if trace_text() {
        // The size, not the text. See `trace_text_contents` — this line used to
        // print a password in full on every keystroke of it.
        eprintln!(
            "[cordial] text -> {} caret={cursor} sync={} passText={}",
            redacted(text),
            !sync.is_null(),
            !f.is_null()
        );
    }
}

/// Where the pointer was the last time one was reported, so the next report
/// can carry how far it moved. `None` means "no previous position to subtract"
/// — see [`reset_mouse_delta`].
static MOUSE_LAST: Mutex<Option<(f32, f32)>> = Mutex::new(None);

/// Forget the last reported pointer position, so the next move reports a zero
/// delta rather than the distance from wherever the pointer was before.
///
/// Called when the pointer enters or leaves the canvas. Without it, a pointer
/// that left at one edge and came back at the other would report the whole
/// width of the window as a single movement — and a delta is what turns the
/// camera, so that is not a cosmetic error but a view that snaps round.
pub fn reset_mouse_delta() {
    *MOUSE_LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Split a new absolute position into itself and the movement since the last
/// one. Separate from [`pass_mouse_move`] so the arithmetic — specifically the
/// first-event case — is testable without a loaded engine.
fn mouse_delta(x: f32, y: f32) -> (f32, f32) {
    let mut last = MOUSE_LAST.lock().unwrap_or_else(|e| e.into_inner());
    let d = match *last {
        Some((px, py)) => (x - px, y - py),
        None => (0.0, 0.0),
    };
    *last = Some((x, y));
    d
}

/// `nativePassMouseMove(F x, F y, F dx, F dy)`.
///
/// The last two arguments used to be sent as constant zeros, which is the
/// likeliest reason the mouse would not turn the camera: an absolute position
/// says where the cursor is, and a camera is rotated by how far it *moved*. The
/// dex declares `(FFFF)V` and strips parameter names, so "the last two are the
/// delta" is `INFERRED` — but it is the shape this file already assumed when it
/// hardcoded zeros, and a real delta is strictly closer to the truth than a
/// value that says the pointer never moves.
pub fn pass_mouse_move(x: f32, y: f32) {
    let (dx, dy) = mouse_delta(x, y);
    pass_mouse_move_delta(x, y, dx, dy);
}

/// As [`pass_mouse_move`], but with the movement supplied rather than derived
/// from the previous position.
///
/// This is what a captured pointer needs. Under `zwp_pointer_constraints_v1`
/// the cursor stops moving on purpose, so there is no new absolute position to
/// subtract a previous one from — the movement arrives on its own, through
/// `zwp_relative_pointer_v1`, and the absolute pair stays wherever the lock
/// caught it. Subtracting two identical positions would report that the mouse
/// had not moved, which is the same "constant zeros" bug the delta arguments
/// were added to fix, arriving by a different route.
///
/// `MOUSE_LAST` is deliberately left alone: it tracks where the *cursor* is,
/// and while the pointer is locked the cursor is not going anywhere.
pub fn pass_mouse_move_delta(x: f32, y: f32, dx: f32, dy: f32) {
    let f = PASS_MOUSE_MOVE.load(std::sync::atomic::Ordering::Relaxed);
    if f.is_null() {
        report_unregistered("nativePassMouseMove");
        return;
    }
    let r = cordial_linker_sys::game_activity::pass_mouse_move(f, x, y, dx, dy);
    if trace_mouse() {
        eprintln!("[cordial] nativePassMouseMove(x={x}, y={y}, dx={dx}, dy={dy}) -> {r:?}");
    }
}

/// `nativePassMouseButton(F x, F y, Z down, I button)`.
///
/// `android_button` is the `MotionEvent.BUTTON_*` bit the backend decoded;
/// [`roblox_mouse_button`] turns it into this interface's own index.
///
/// Only the primary button used to be delivered here at all, and always as
/// index 0. Roblox turns the camera on a right-button drag, so a client that
/// never reports a right button cannot turn its camera with the mouse however
/// well the rest of the path works.
pub fn pass_mouse_button(x: f32, y: f32, down: bool, android_button: i32) {
    let f = PASS_MOUSE_BUTTON.load(std::sync::atomic::Ordering::Relaxed);
    if f.is_null() {
        report_unregistered("nativePassMouseButton");
        return;
    }
    let button = roblox_mouse_button(android_button);
    let r = cordial_linker_sys::game_activity::pass_mouse_button(f, x, y, down, button);
    if trace_mouse() {
        eprintln!(
            "[cordial] nativePassMouseButton(x={x}, y={y}, down={down}, \
             android={android_button}, roblox={button}) -> {r:?}"
        );
    }
}

/// One wheel movement, through both input paths, in detents.
///
/// This is what both backends call, and it is the whole of the scroll wheel:
/// `nativePassMouseWheel` is a real export that Cordial had never called once,
/// which is why scrolling did nothing anywhere in the client. X11 dropped
/// buttons 4-7 on the floor and Wayland's `wl_pointer.axis` handler was an
/// empty function.
///
/// **The unit is the detent** — one notch of a mouse wheel is 1.0 — because
/// that is the one unit both backends can produce honestly. X11 gives it for
/// free (button 4 *is* one notch); Wayland reports a distance instead and
/// `wayland.rs` converts. `MotionEvent.AXIS_VSCROLL` is documented in the same
/// unit, and Roblox's own `MouseWheel` input reports ±1 per notch, so it is
/// also the likeliest thing the third float wants.
///
/// Sign: positive is away from the user, and positive horizontal is to the
/// right, which is what Android documents for the two scroll axes. Whether
/// Roblox agrees is `INFERRED` — nothing readable declares it, and it is one
/// scroll for a human to settle. `CORDIAL_WHEEL_SCALE` is that experiment
/// without a rebuild: it multiplies both axes, so `-1` inverts and `3` makes
/// each notch scroll three.
///
/// `nativePassMouseWheel` takes one float, not two, so horizontal scroll
/// reaches only the AGDK path. `nativePassMousePan(FFFF)` is the plausible
/// home for it and is not driven here, because "plausible" is how this file
/// acquires bugs that take a session to find.
pub fn wheel(handle: i64, x: f32, y: f32, hscroll: f32, vscroll: f32, event_time_ms: i64) {
    let scale = wheel_scale();
    let (h, v) = (hscroll * scale, vscroll * scale);
    if handle != 0 {
        deliver_scroll(handle, x, y, h, v, event_time_ms);
    }
    let f = PASS_MOUSE_WHEEL.load(std::sync::atomic::Ordering::Relaxed);
    if f.is_null() {
        report_unregistered("nativePassMouseWheel");
    }
    let passed = (!f.is_null()).then(|| cordial_linker_sys::game_activity::pass_mouse_wheel(f, x, y, v));
    if trace_wheel() {
        // The arguments as the engine receives them, and what the call
        // answered. Which of the two paths ran matters as much as the numbers:
        // "the wheel does nothing" has two quite different causes, and this
        // line tells them apart without a debugger.
        eprintln!(
            "[cordial] nativePassMouseWheel(x={x}, y={y}, delta={v}) -> {passed:?}; \
             AGDK ACTION_SCROLL h={h} v={v} handle={handle}"
        );
    }
}

/// `CORDIAL_WHEEL_SCALE=<f>`, applied to both scroll axes. Negative inverts.
///
/// A knob rather than a constant because neither the sign nor the size of a
/// notch is declared anywhere Cordial can read, and both are a single scroll
/// for a human to check. Rejected values fall back to 1.0 loudly: a silently
/// ignored scale reads as "the wheel still does not work".
fn wheel_scale() -> f32 {
    static SCALE: OnceLock<f32> = OnceLock::new();
    *SCALE.get_or_init(|| {
        let Some(v) = std::env::var_os("CORDIAL_WHEEL_SCALE") else {
            return 1.0;
        };
        match v.to_string_lossy().trim().parse::<f32>() {
            Ok(f) if f.is_finite() && f != 0.0 => f,
            _ => {
                eprintln!(
                    "[cordial] CORDIAL_WHEEL_SCALE={} is not a non-zero number; using 1.0",
                    v.to_string_lossy()
                );
                1.0
            }
        }
    })
}

/// `CORDIAL_TRACE_WHEEL=1`. Its own switch for the same reason
/// `CORDIAL_TRACE_TEXT` has one: the question is what Cordial *sent*, and the
/// general trace is documented as ABI-unsafe and aborts the engine.
/// `CORDIAL_TRACE_MOUSE=1` — every pointer call Cordial makes into the engine.
///
/// Added because hovering a game card shows a Play button on Sober and does not
/// here, and nothing could say whether the hover events were arriving at all.
/// `nativePassMouseMove` had never been traced, so "the engine ignores hover"
/// and "the engine is never told about hover" looked identical from outside —
/// the same ambiguity that made the keyboard take days, where four theories all
/// assumed a delivery problem and the answer was an interpretation one.
///
/// Note the argument meaning of `nativePassMouseMove(FFFF)` is INFERRED as
/// `(x, y, dx, dy)`. Four floats and no tell to disambiguate them, unlike
/// `nativePassKeyEvent`, whose missing scan-code slot said what vocabulary it
/// wanted once anyone read it. If hover events arrive and still do nothing, that
/// inference is the first thing to doubt.
pub fn trace_mouse() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_TRACE_MOUSE").is_some())
}

pub fn trace_wheel() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_TRACE_WHEEL").is_some())
}

/// `CORDIAL_TRACE_TEXT=1`. Text entry is the one path where the interesting
/// question is what the host *saw*, not what the engine did, so it gets its own
/// switch rather than riding on the general trace — which is documented as
/// ABI-unsafe and aborts the engine.
/// `CORDIAL_NO_AGDK_TOUCH=1` — deliver pointer input only through Roblox's own
/// `NativeInputInterface`, not also through AGDK's `onTouchEventNative`.
///
/// Both paths are real and the engine consumes both, so one physical click
/// arrives twice. Kept as a control: it was the first suspect for text focus
/// bouncing and was measured *not* to be the cause, and that result is worth
/// being able to reproduce.
fn no_agdk_touch() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_NO_AGDK_TOUCH").is_some())
}

/// Which of the two key paths carries a keystroke. **AGDK's is off by default**,
/// and `CORDIAL_AGDK_KEY=1` puts it back.
///
/// ## Settled by measurement, 2026-08-19
///
/// Every key press used to go to the engine twice, and the comment here used to
/// say that nobody had ever observed one path in isolation. Both have now been
/// run alone against a real session:
///
/// | configuration | result |
/// |---|---|
/// | `nativePassKeyEvent` alone | **works** |
/// | AGDK `onKeyDownNative` alone | **nothing works at all** |
/// | both, focused at startup | works |
/// | both, not focused at startup | only `SPACE` arrives |
///
/// So the engine reads `NativeInputInterface.nativePassKeyEvent` and does not
/// read AGDK's key queue — one of the four numbering schemes this file agonises
/// over was never being consulted. And sending both is not merely redundant: the
/// last row is the bug it caused, where a client started without keyboard focus
/// (a scripted launch, `tools/join-run.sh`) lost everything but the one key that
/// happened to survive.
///
/// That also retires the old note above about `D` being the only key that moved
/// the character and Alt causing a jump. Two deliveries of one press, interpreted
/// differently, was exactly the variable nobody had removed, and removing it is
/// the fix rather than another mapping table.
///
/// AGDK delivery is kept behind the flag rather than deleted because it is the
/// standard Android path and a future engine build may start reading it. It is
/// not carrying anything today.
///
/// **Every key press is delivered to the engine twice**, through AGDK's
/// `onKeyDownNative` and through `NativeInputInterface.nativePassKeyEvent`, and
/// until now there was no way to run either alone. The touch path has had that
/// control since the focus-bounce investigation; the key path never did, so
/// nobody has ever observed one in isolation.
///
/// That matters because the symptom does not look like a mapping error. Both
/// natives are registered, both are called, both receive the correct Android
/// keycodes, and both report the engine consumed them — measured — and yet only
/// `D` moves the character, and Ctrl+Alt makes it *jump*. Jump is `SPACE`, and
/// no encoding of Ctrl or Alt is anywhere near `SPACE` in any of the four
/// numbering schemes in play here, so this is not an off-by-N. Two deliveries
/// of one press, interpreted differently, is the variable nobody has removed.
///
fn no_agdk_key() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    // Inverted: the default is now *not* to send. `CORDIAL_NO_AGDK_KEY` is still
    // honoured so anything scripted against it keeps working.
    *ON.get_or_init(|| std::env::var_os("CORDIAL_AGDK_KEY").is_none())
}

fn no_pass_key() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_NO_PASS_KEY").is_some())
}

/// Whether to acknowledge the keyboard to the engine at all. **Off by default.**
///
/// `updateKeyboardSize(visible=true)` was added to close the text-entry
/// handshake and instead destroys focus. Measured, in trace order:
///
/// ```text
/// textbox focused handle=139759059370112
/// updateKeyboardSize(visible=true)
/// textbox blurred
/// ```
///
/// Focus bounces continuously while it is driven, and a bouncing focus resets
/// the edit buffer between keystrokes because the reseed is generation-driven —
/// which is what made the field appear to clear as you typed. With it
/// suppressed, focus is stable, confirmed by control in the same session.
///
/// It is off rather than deleted because the engine plainly wants *something*
/// to acknowledge a keyboard; the fault is in the arguments or the moment, not
/// in the call existing. The arguments have since been corrected against the
/// real-Android capture — see `report_keyboard_state`, which now sends the
/// baseline that capture actually shows. `CORDIAL_KEYBOARD_REPORT=1` turns it
/// on, which is also the control for testing it. See `docs/NEXT.md` §1.
///
/// **The reason it is still off has changed.** It used to be that the corrected
/// form had never been driven through a live typing session. It has now
/// (2026-08-03, X11, `CORDIAL_SCRIPT` clicking a login field and typing into
/// it): the corrected report does *not* bounce focus, and it does not change
/// anything else either — the engine draws a focused box's text neither with it
/// nor without it, pixel-identical at every step of the same scripted sequence.
/// So it stays off for want of a reason to turn it on rather than for fear of
/// what it did, and turning it on is not a fix for text entry. Do not spend
/// another session on that hypothesis; §1 has the screenshots.
fn keyboard_report_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_KEYBOARD_REPORT").is_some())
}

pub fn trace_text() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_TRACE_TEXT").is_some())
}

/// `CORDIAL_TRACE_TEXT_SHOW_PASSWORDS=1` — print what was typed, not just how
/// much of it.
///
/// **The name is the documentation.** The first field anyone debugging this
/// reaches for is Roblox's password box, and `CORDIAL_TRACE_TEXT=1` used to put
/// its contents on the terminal a character at a time and then again in full on
/// every keystroke. Once Ctrl+V is bound it would also print whatever was on
/// the clipboard, which is routinely a password out of a manager and routinely
/// not even this user's. Two other places in this tree logged secrets the same
/// way in the same week — the shell's banner printed a live auth ticket, and
/// `deeplink::describe` printed a whole payload under the words "values not
/// shown" — and both were fixed to names and byte counts.
///
/// Byte counts and caret positions answer every question this switch exists
/// for. The bug it was written for is that characters do not *paint*, which a
/// length answers as well as the text does; where the text itself genuinely
/// matters — a mangled multi-byte character, say — this switch exists and says
/// out loud what turning it on means.
fn trace_text_contents() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_TRACE_TEXT_SHOW_PASSWORDS").is_some())
}

/// How to describe a piece of text in a trace line: its size, or — only when
/// [`trace_text_contents`] is on — the text itself.
pub fn redacted(text: &str) -> String {
    if trace_text_contents() {
        format!("{text:?}")
    } else {
        format!("<{} bytes, {} chars>", text.len(), text.chars().count())
    }
}

/// Whether this key press is "paste".
///
/// Shared by both backends so the shortcut cannot end up meaning one thing on
/// X11 and another on Wayland, and so the one subtlety in it is written down
/// once: **Ctrl+Shift+V is not this**. That is "paste without formatting" in
/// most of the desktop and it is also what a terminal uses for plain paste; a
/// text field that treated the two as the same would be wrong in the case where
/// the difference matters. Alt+V is not this either — that is a menu mnemonic.
///
/// `keysym` rather than an evdev code, because paste lives on whichever
/// physical key the layout calls `v`, which is the whole point of a layout.
/// Both `v` and `V` are accepted: Caps Lock does not turn paste off.
pub fn is_paste_shortcut(keysym: c_ulong, meta: i32) -> bool {
    let ctrl_only = meta & META_CTRL_ON != 0 && meta & (META_SHIFT_ON | META_ALT_ON) == 0;
    ctrl_only && (keysym == 'v' as c_ulong || keysym == 'V' as c_ulong)
}

// ------------------------------------------------------------------ text entry

static TEXT_BUFFER: Mutex<TextField> = Mutex::new(TextField::new());

/// The editing state Cordial keeps on behalf of the engine.
///
/// Android delegates text editing to the IME, and with a hardware keyboard the
/// IME is still in the loop — it receives the key events and commits finished
/// text through the InputConnection. Cordial is that IME here, so it owns the
/// caret as well as the contents. Sending the whole string with the caret
/// pinned to the end is what made typing feel broken: every keystroke dragged
/// the caret back, so arrows and clicking into the middle of a field could not
/// work by construction.
///
/// The caret is counted in `char`s, not bytes, because that is what the engine
/// is told and what a person means by "third character".
///
/// This state is display-server independent by construction: it is driven by
/// committed text and caret movements (`Edit`), which is exactly the vocabulary
/// `zwp_text_input_v3` hands over on Wayland and `XLookupString` approximates
/// on X11. Neither backend needs its own copy.
struct TextField {
    text: String,
    caret: usize,
}

impl TextField {
    const fn new() -> Self {
        TextField { text: String::new(), caret: 0 }
    }

    /// Byte offset of the caret, for slicing.
    fn byte_offset(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.caret)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    fn seed(&mut self, text: String) {
        self.caret = text.chars().count();
        self.text = text;
    }

    /// As [`Self::seed`], but with the caret placed explicitly rather than at
    /// the end — for seeding from `InputConnection.setState`, which reports a
    /// real selection, unlike `showKeyboard`'s byte array which carries no
    /// caret at all. Clamped into range: a stale or out-of-sync `selectionEnd`
    /// from the engine must not panic the char-boundary arithmetic elsewhere
    /// in this struct.
    fn seed_with_caret(&mut self, text: String, caret_chars: i32) {
        let len = text.chars().count();
        self.caret = caret_chars.max(0) as usize;
        if self.caret > len {
            self.caret = len;
        }
        self.text = text;
    }

    fn insert(&mut self, s: &str) {
        let at = self.byte_offset();
        self.text.insert_str(at, s);
        self.caret += s.chars().count();
    }

    /// Delete the character before the caret. False when there is nothing to
    /// delete, so the caller can avoid sending an unchanged state.
    fn backspace(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        self.caret -= 1;
        let at = self.byte_offset();
        self.text.remove(at);
        true
    }

    /// Delete the character at the caret — the `Delete` key, as distinct from
    /// backspace. Without it, correcting a typo means deleting everything after
    /// it too.
    fn delete(&mut self) -> bool {
        if self.caret >= self.len_chars() {
            return false;
        }
        let at = self.byte_offset();
        self.text.remove(at);
        true
    }

    /// Move the caret. Returns whether it moved, so a Left at position zero
    /// does not resend identical state.
    fn move_caret(&mut self, to: Caret) -> bool {
        let before = self.caret;
        self.caret = match to {
            Caret::Left => self.caret.saturating_sub(1),
            Caret::Right => (self.caret + 1).min(self.len_chars()),
            Caret::Home => 0,
            Caret::End => self.len_chars(),
        };
        self.caret != before
    }

    /// `zwp_text_input_v3.delete_surrounding_text`: remove `before` bytes
    /// immediately before the caret and `after` bytes immediately after it.
    ///
    /// The protocol counts in bytes, not characters — deliberately so an IME
    /// never has to know the client's internal representation — but this
    /// buffer is a `String`, so a byte count that does not land on a UTF-8
    /// character boundary would panic on `remove`/slicing rather than
    /// misbehave quietly. Both cuts are clamped to the nearest valid boundary
    /// at or before the requested byte offset, which only ever deletes less
    /// than asked, never more and never a partial codepoint.
    fn delete_surrounding(&mut self, before: usize, after: usize) -> bool {
        let caret_byte = self.byte_offset();

        let start = if before == 0 {
            caret_byte
        } else {
            let want = caret_byte.saturating_sub(before);
            // Walk forward from `want` to the next real boundary rather than
            // backward from `caret_byte`, so a `want` that already landed
            // exactly on a boundary is left alone rather than over-deleting
            // one extra character.
            (want..=caret_byte)
                .find(|&i| self.text.is_char_boundary(i))
                .unwrap_or(caret_byte)
        };
        let end = if after == 0 {
            caret_byte
        } else {
            let want = (caret_byte + after).min(self.text.len());
            (caret_byte..=want)
                .rev()
                .find(|&i| self.text.is_char_boundary(i))
                .unwrap_or(caret_byte)
        };
        if start == end {
            return false;
        }

        let removed_chars_before_caret = self.text[start..caret_byte].chars().count();
        self.text.replace_range(start..end, "");
        self.caret = self.caret.saturating_sub(removed_chars_before_caret);
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Caret {
    Left,
    Right,
    Home,
    End,
}

/// The focus generation `TEXT_BUFFER` was last seeded for. `showKeyboard`
/// bumps the engine-side counter on every focus change; when this falls behind
/// it, the buffer belongs to a box that no longer has focus and is reseeded
/// from whatever the engine says the newly focused box contains.
///
/// Without this, moving from the username field to the password field carries
/// the username into it, and the first keystroke in a pre-filled field appends
/// rather than continues.
static TEXT_GENERATION: Mutex<Option<u32>> = Mutex::new(None);

/// What a key press means to the focused field.
pub enum Edit<'a> {
    Insert(&'a str),
    Backspace,
    Delete,
    Move(Caret),
    /// `zwp_text_input_v3.delete_surrounding_text` — byte counts, not chars.
    /// See [`TextField::delete_surrounding`] for why that distinction is
    /// handled inside the buffer rather than by the caller pre-converting.
    DeleteSurrounding { before_bytes: usize, after_bytes: usize },
}

/// Reseed the buffer when focus has moved since it was last filled, shared by
/// [`edit_text_buffer`] and [`text_buffer_snapshot`] so the two cannot drift
/// into different reseed conditions.
///
/// The *trigger* is still `textbox_generation()` — `showKeyboard`'s
/// focus-change counter, proven in practice (see `docs/NEXT.md` §1's account
/// of the bouncing-focus bug this generation check exists to survive). What
/// changed is the *content* reseeded: `InputConnection.setState` is the
/// engine's own outbound report of what a field contains, and — once at least
/// one has actually arrived — is preferred over `showKeyboard`'s byte array,
/// which is a one-shot snapshot taken only at the moment focus changed and
/// carries no caret at all (`seed_with_caret` uses `setState`'s
/// `selectionEnd`; `showKeyboard`'s path still defaults to the end of the
/// text, via `seed`, as it always has).
///
/// Deliberately *not* done: reseeding on every `ime_state_generation()`
/// change, i.e. treating each `setState` as a live overwrite regardless of
/// focus. `setState` is also how the engine would echo back a state Cordial
/// itself just pushed via `pass_text`/`sync_textbox`, and reseeding on that
/// echo — mid-keystroke, not at a focus boundary — is exactly the shape of
/// feedback loop that produced the focus-bounce bug `keyboard_report_enabled`
/// documents. Restricting the new source to the existing, already-safe reseed
/// boundary avoids reopening that without the interactive test needed to
/// confirm a live-overwrite version does not regress it.
fn reseed_if_needed(buf: &mut TextField) {
    let generation = cordial_linker_sys::game_activity::textbox_generation();
    let mut seen = TEXT_GENERATION.lock().unwrap_or_else(|e| e.into_inner());
    if *seen != Some(generation) {
        if cordial_linker_sys::game_activity::ime_state_generation() > 0 {
            let text = cordial_linker_sys::game_activity::ime_state_text();
            let (_, selection_end) = cordial_linker_sys::game_activity::ime_state_selection();
            buf.seed_with_caret(text, selection_end);
        } else {
            // No `setState` has landed yet this session — nothing has told
            // Cordial anything through the new path, and treating that as
            // "the field is empty" would wrongly blank a pre-filled box that
            // `showKeyboard`'s snapshot still has correctly.
            buf.seed(cordial_linker_sys::game_activity::textbox_text());
        }
        *seen = Some(generation);
    }
}

/// Apply one edit to the focused field.
///
/// Returns the contents and caret to send, or `None` when nothing changed —
/// resending identical state on every arrow key at the end of a field makes the
/// engine redraw for no reason.
pub fn edit_text_buffer(edit: Edit<'_>) -> Option<(String, i32)> {
    let mut buf = TEXT_BUFFER.lock().unwrap_or_else(|e| e.into_inner());
    reseed_if_needed(&mut buf);

    let changed = match edit {
        Edit::Insert(s) => {
            // Control characters are not text. A field receives what a person
            // typed, not every key they pressed.
            if s.is_empty() || s.chars().any(|c| c.is_control()) {
                false
            } else {
                buf.insert(s);
                true
            }
        }
        Edit::Backspace => buf.backspace(),
        Edit::Delete => buf.delete(),
        Edit::Move(to) => buf.move_caret(to),
        Edit::DeleteSurrounding { before_bytes, after_bytes } => {
            buf.delete_surrounding(before_bytes, after_bytes)
        }
    };

    changed.then(|| (buf.text.clone(), buf.caret as i32))
}

/// The focused field's contents and caret, reseeding first exactly as
/// [`edit_text_buffer`] does, but without requiring an edit to apply.
///
/// The Wayland IME bridge needs this to splice a not-yet-committed preedit
/// string into the caret position for display — that is not an edit to the
/// committed buffer (see `wayland.rs`'s module doc on why preedit is tracked
/// separately), so it cannot go through `edit_text_buffer`, which only ever
/// reports state when something actually changed.
pub fn text_buffer_snapshot() -> (String, i32) {
    let mut buf = TEXT_BUFFER.lock().unwrap_or_else(|e| e.into_inner());
    reseed_if_needed(&mut buf);
    (buf.text.clone(), buf.caret as i32)
}

// ------------------------------------------------------------ scripted input
//
// A click and a keystroke Cordial delivers to itself, for the experiments the
// text path cannot otherwise have.
//
// The rule against synthesising input (AGENTS.md, `docs/NEXT.md`'s "how to work
// on this") is about the *compositor*: `XTestFake*`, `ydotool`,
// `wlr-virtual-keyboard` and the RemoteDesktop portal all land on whatever has
// focus, which is the developer's session. Nothing here goes near one. Cordial
// is the client, so these call the same natives the backends' own
// `dispatch_button`/`dispatch_key` call, with the same arguments, one layer
// below the display server. The X11 keycode-to-keysym and the xkb translations
// are the only thing they do not exercise, and those are established (see
// `pass_key_event` on the evdev/AKEYCODE vocabulary that cost days).
//
// This exists because the last open question about text entry — does the engine
// draw a focused box's own text — takes a keystroke to answer, and every
// previous attempt stalled exactly there.

/// The evdev code for an ASCII character, for [`script_type`]'s
/// `nativePassKeyEvent` argument.
///
/// A separate table from [`keysym_to_android`] because the two want different
/// vocabularies and conflating them is the bug documented at length on
/// [`pass_key_event`]: the native takes the *platform's* code, and on Linux
/// that is evdev's. Deliberately small — a scripted run types identifiers and
/// digits, and a character with no entry is dropped rather than guessed at, so
/// a missing one shows up as a missing character rather than as some other key.
fn ascii_to_evdev(c: char) -> Option<i32> {
    const LETTERS: [i32; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
        45, 21, 44,
    ];
    Some(match c {
        'a'..='z' => LETTERS[c as usize - 'a' as usize],
        '1'..='9' => 2 + (c as i32 - '1' as i32),
        '0' => 11,
        ' ' => 57,
        _ => return None,
    })
}

/// One left click at a canvas position, as [`super::window::HostWindow`]'s
/// `dispatch_button` delivers one: `ACTION_DOWN`/`ACTION_BUTTON_PRESS`, then
/// the release pair, plus `nativePassMouseButton` on each half.
pub fn script_click(handle: i64, x: f32, y: f32, now_ms: i64) {
    // A hover first. Roblox's interface highlights on hover and hit-tests the
    // press against where it believes the pointer is, and a press with no
    // preceding motion is a shape a real mouse never produces.
    deliver_touch(handle, ACTION_HOVER_MOVE, x, y, 0, 0, now_ms, 0);
    pass_mouse_move(x, y);

    deliver_touch(handle, ACTION_DOWN, x, y, BUTTON_PRIMARY, 0, now_ms, now_ms);
    deliver_touch(handle, ACTION_BUTTON_PRESS, x, y, BUTTON_PRIMARY, BUTTON_PRIMARY, now_ms, now_ms);
    pass_mouse_button(x, y, true, BUTTON_PRIMARY);

    let up = now_ms + 40;
    deliver_touch(handle, ACTION_BUTTON_RELEASE, x, y, 0, BUTTON_PRIMARY, up, now_ms);
    deliver_touch(handle, ACTION_UP, x, y, 0, 0, up, now_ms);
    pass_mouse_button(x, y, false, BUTTON_PRIMARY);
}

/// Type a string into whatever box the engine says has focus, one character at
/// a time, down and up, through every path a real keystroke takes.
///
/// Returns how many characters were delivered to a focused box. Zero with a
/// non-empty argument means no box had focus, which is a result rather than a
/// failure — sending text with no focused box means sending it to handle 0 and
/// the engine drops it in silence.
pub fn script_type(handle: i64, text: &str, now_ms: i64) -> usize {
    let mut delivered = 0;
    for (i, c) in text.chars().enumerate() {
        let keysym = c as c_ulong;
        let evdev = ascii_to_evdev(c);
        let t = now_ms + i as i64 * 60;
        if let (Some(keycode), Some(evdev)) = (keysym_to_android(keysym), evdev) {
            deliver_key(handle, true, keycode, evdev + 8, 0, 0, c as i32, t, t);
            pass_key_event(true, evdev, 0);
        }
        let Some(which) = cordial_linker_sys::game_activity::focused_textbox() else {
            if trace_text() {
                eprintln!("[cordial] script type: no focused textbox");
            }
            continue;
        };
        let mut buf = [0u8; 4];
        if let Some((contents, caret)) = edit_text_buffer(Edit::Insert(c.encode_utf8(&mut buf))) {
            let _ = cordial_linker_sys::game_activity::text_input(handle, &contents, caret, caret);
            pass_text(which, &contents, caret);
            delivered += 1;
        }
        if let (Some(keycode), Some(evdev)) = (keysym_to_android(keysym), evdev) {
            deliver_key(handle, false, keycode, evdev + 8, 0, 0, c as i32, t + 30, t);
            pass_key_event(false, evdev, 0);
        }
    }
    delivered
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole policy table, because the interesting cases are the ones
    /// nobody thinks about: a backend that does not know, and a window that is
    /// unfocused but plainly visible on the other monitor.
    #[test]
    fn the_keepalive_stops_only_where_the_setting_says_it_should() {
        use ThrottleWhen::*;
        // Off never throttles, whatever the window is doing.
        for f in [None, Some(true), Some(false)] {
            for v in [None, Some(true), Some(false)] {
                assert!(keepalive_wanted(Off, f, v), "Off must keep going: {f:?} {v:?}");
            }
        }
        // Unfocused throttles on lost focus and ignores visibility.
        assert!(!keepalive_wanted(Unfocused, Some(false), Some(true)));
        assert!(keepalive_wanted(Unfocused, Some(true), Some(false)));
        // Visible is the default, and the case it exists for: a window on the
        // second monitor, not focused, still being watched.
        assert!(keepalive_wanted(Visible, Some(false), Some(true)));
        assert!(!keepalive_wanted(Visible, Some(false), Some(false)));
        assert!(!keepalive_wanted(Visible, Some(true), Some(false)));
        // "Not known" is not "not visible". X11 tracks neither and must keep
        // the behaviour it has always had rather than throttling itself.
        assert!(keepalive_wanted(Visible, None, None));
        assert!(keepalive_wanted(Unfocused, None, None));
    }

    #[test]
    fn paste_is_ctrl_v_and_not_its_neighbours() {
        assert!(is_paste_shortcut('v' as c_ulong, META_CTRL_ON));
        assert!(is_paste_shortcut('V' as c_ulong, META_CTRL_ON | META_CAPS_LOCK_ON));
        // Ctrl+Shift+V is "paste without formatting" everywhere else, and a
        // terminal's plain paste. Not the same key.
        assert!(!is_paste_shortcut('v' as c_ulong, META_CTRL_ON | META_SHIFT_ON));
        assert!(!is_paste_shortcut('v' as c_ulong, META_ALT_ON | META_CTRL_ON));
        assert!(!is_paste_shortcut('v' as c_ulong, 0));
        assert!(!is_paste_shortcut('c' as c_ulong, META_CTRL_ON));
    }

    #[test]
    fn a_trace_line_does_not_carry_the_text_by_default() {
        // The switch is read from the environment once per process, so this
        // asserts the default shape rather than flipping it: a password must
        // not be reconstructible from what this returns.
        let line = redacted("hunter2");
        assert!(!line.contains("hunter2"), "trace line leaked the text: {line}");
        assert!(line.contains('7'), "trace line should still carry the size: {line}");
    }

    #[test]
    fn evdev_codes_are_the_platforms_not_androids() {
        // The one collision that hid the vocabulary bug for days: `d` is 32 in
        // both numbering schemes. Every other letter must differ from its
        // AKEYCODE, or this table has been filled in from the wrong one.
        assert_eq!(ascii_to_evdev('d'), Some(32));
        assert_eq!(ascii_to_evdev('a'), Some(30));
        assert_eq!(keysym_to_android('a' as c_ulong), Some(29));
        assert_eq!(ascii_to_evdev('w'), Some(17));
        assert_eq!(ascii_to_evdev(' '), Some(57));
        assert_eq!(ascii_to_evdev('@'), None);
    }

    #[test]
    fn a_caret_edits_where_it_is_not_at_the_end() {
        // Every keystroke used to send the whole string with the caret pinned to
        // the end, which meant arrows and clicking into the middle of a field
        // could not work however the engine behaved. This is the regression that
        // made typing feel broken rather than absent.
        let mut f = TextField::new();
        f.seed("hello".into());
        assert_eq!(f.caret, 5);
        assert!(f.move_caret(Caret::Home));
        assert_eq!(f.caret, 0);
        f.insert("say ");
        assert_eq!(f.text, "say hello");
        assert_eq!(f.caret, 4);
    }

    #[test]
    fn backspace_and_delete_are_not_the_same_key() {
        // Backspace removes before the caret, Delete at it. Treating Delete as
        // backspace loses the character on the wrong side of the cursor, which
        // is the sort of bug people describe as "it eats my text".
        let mut f = TextField::new();
        f.seed("abc".into());
        f.move_caret(Caret::Home);
        assert!(!f.backspace()); // nothing before the caret
        assert!(f.delete());
        assert_eq!(f.text, "bc");
        assert_eq!(f.caret, 0);
        f.move_caret(Caret::End);
        assert!(f.backspace());
        assert_eq!(f.text, "b");
    }

    #[test]
    fn the_caret_is_counted_in_characters_not_bytes() {
        // The engine is told a character offset. Counting bytes puts the caret
        // mid-codepoint for any non-ASCII input and slices a String there, which
        // panics rather than misbehaving quietly.
        let mut f = TextField::new();
        f.seed("héllo".into());
        assert_eq!(f.caret, 5);
        f.move_caret(Caret::Home);
        f.move_caret(Caret::Right);
        f.move_caret(Caret::Right);
        assert_eq!(f.caret, 2);
        f.insert("X");
        assert_eq!(f.text, "héXllo");
    }

    #[test]
    fn a_caret_move_that_goes_nowhere_reports_no_change() {
        // Left at position zero must not resend identical state; the engine
        // would redraw the field on every held arrow key for nothing.
        let mut f = TextField::new();
        f.seed("ab".into());
        f.move_caret(Caret::Home);
        assert!(!f.move_caret(Caret::Left));
        assert!(f.move_caret(Caret::Right));
        f.move_caret(Caret::End);
        assert!(!f.move_caret(Caret::Right));
    }

    #[test]
    fn delete_surrounding_counts_bytes_not_chars() {
        // "café" is 4 chars but 5 bytes (é is 2 bytes in UTF-8). An IME asking
        // to delete 2 bytes before the caret means "delete é", not "delete fé"
        // — treating the count as chars would delete one codepoint too many.
        let mut f = TextField::new();
        f.seed("café".into());
        assert_eq!(f.caret, 4);
        assert!(f.delete_surrounding(2, 0));
        assert_eq!(f.text, "caf");
        assert_eq!(f.caret, 3);
    }

    #[test]
    fn delete_surrounding_deletes_both_sides_of_the_caret() {
        // set_surrounding_text/delete_surrounding_text lets an IME correct
        // text on either side of where composition is happening, not only
        // backspace-style before the caret.
        let mut f = TextField::new();
        f.seed("hello world".into());
        f.move_caret(Caret::Home);
        for _ in 0..6 {
            f.move_caret(Caret::Right);
        }
        assert_eq!(f.caret, 6); // caret sits just before "world"
        assert!(f.delete_surrounding(6, 2));
        assert_eq!(f.text, "rld");
        assert_eq!(f.caret, 0);
    }

    #[test]
    fn delete_surrounding_clamps_to_a_char_boundary_rather_than_panicking() {
        // A byte count that lands mid-codepoint must not slice the string
        // there — this is the case the doc comment on `delete_surrounding`
        // calls out explicitly, so it gets its own test rather than trusting
        // the boundary-walk to be exercised incidentally.
        let mut f = TextField::new();
        f.seed("café".into()); // caret at 4 chars = byte 5 (é is 2 bytes)
        // Asking for 1 byte lands between é's two bytes, mid-codepoint. The
        // buffer clamps down to the nearest boundary at or after that point
        // — which is the caret itself here — rather than either panicking or
        // deleting more than the 1 byte actually requested. Nothing to
        // delete is therefore the correct, safe answer, not a bug.
        assert!(!f.delete_surrounding(1, 0));
        assert_eq!(f.text, "café");
    }

    #[test]
    fn every_mouse_button_maps_to_its_own_index() {
        // The bug this replaces was not a wrong index, it was no index at all:
        // only the primary button was ever delivered, and always as 0. A test
        // that the three are distinct is what stops a future edit collapsing
        // them back into one.
        assert_eq!(roblox_mouse_button(BUTTON_PRIMARY), 0);
        assert_eq!(roblox_mouse_button(BUTTON_SECONDARY), 1);
        assert_eq!(roblox_mouse_button(BUTTON_TERTIARY), 2);
    }

    #[test]
    fn the_first_move_after_the_pointer_arrives_has_no_delta() {
        // One test rather than several, because `mouse_delta` reads and writes
        // a process-wide last-position and Rust runs tests in parallel threads
        // — two tests sharing it would race and fail intermittently, which is
        // worse than no test.
        //
        // The case that matters is the first one. A pointer that re-enters the
        // canvas at the far side must not report the width of the window as a
        // single movement: a delta is what turns the camera, so that would
        // snap the view round rather than merely be slightly wrong.
        reset_mouse_delta();
        assert_eq!(mouse_delta(100.0, 50.0), (0.0, 0.0));
        assert_eq!(mouse_delta(103.0, 47.0), (3.0, -3.0));
        assert_eq!(mouse_delta(103.0, 47.0), (0.0, 0.0));
        reset_mouse_delta();
        assert_eq!(mouse_delta(900.0, 47.0), (0.0, 0.0));
        reset_mouse_delta();
    }

    #[test]
    fn a_reported_snapshot_does_not_require_a_change_to_reflect_state() {
        // `text_buffer_snapshot` exists precisely because `edit_text_buffer`
        // only reports when something changed; the preedit splice needs the
        // current state unconditionally, including when nothing has been
        // typed into this field yet.
        let mut f = TextField::new();
        f.seed("draft".into());
        assert_eq!((f.text.clone(), f.caret as i32), ("draft".to_string(), 5));
    }
}
