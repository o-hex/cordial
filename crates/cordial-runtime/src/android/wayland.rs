//! The native Wayland backend.
//!
//! [ADR-011](../../../../docs/adr/ADR-011-wayland-and-libadwaita.md) makes
//! Wayland the target and X11 the diagnostic fallback. `window.rs` dlopens
//! libX11 rather than linking it, so the loader and asset tests still run
//! with no display at all; this file follows the same rule for
//! `libwayland-client.so.0`, `libwayland-egl.so.1` and `libxkbcommon.so.0`.
//!
//! ## One hand-written protocol table is left, and that is one too many
//!
//! `wl_proxy_marshal_flags` needs a `wl_interface` table to know how many
//! arguments a request takes and of what type, and the core protocol's tables
//! are compiled into `libwayland-client.so` and exported as data symbols —
//! `dlsym` reaches them the same way it reaches `XOpenDisplay` in `window.rs`.
//!
//! This file used to hand-write two protocols that nothing exports, because no
//! `wayland-protocols` XML was available to generate from. `xdg_shell` is now
//! gone: GTK owns the `xdg_toplevel`, and the subsurface role this file uses
//! instead is core protocol. `text-input-unstable-v3` is still hand-written,
//! and is the one interface below whose event count is this file's assertion
//! rather than the library's — see the note on its live bug further down, and
//! notice that the failure mode it shows is precisely the failure mode a
//! hand-written table has.
//!
//! **Do not add another.** A signature that is wrong by one argument makes
//! `wl_proxy_marshal_flags` read the wrong number or type of variadic
//! arguments and corrupts the wire, and this file's comments record a crash of
//! exactly that family. If a further protocol is genuinely needed
//! here, generate it — or take it from GTK, which is in the process now.
//!
//! ## The engine's surface is a subsurface, not a toplevel
//!
//! This file used to give the engine an `xdg_toplevel` of its own, which made
//! the engine's canvas *the whole window*: no titlebar, no client-side
//! decorations, and nowhere to put anything of Cordial's beside the canvas.
//! ADR-011 already said the window is GTK4 + libadwaita and that the shell's
//! window and this one "are the same window"; a bare toplevel here was the
//! part that had not caught up.
//!
//! So GTK owns the toplevel now — [`cordial_shell::host_window`], the same
//! definition the shell binary uses — and the engine's `wl_surface` is a
//! `wl_subsurface` of it, positioned over the window's content area.
//! Consequences worth knowing before changing anything here:
//!
//! GTK's `wl_display` is the *only* connection in the process. Wayland object
//! ids are scoped to the connection that made them, so a subsurface cannot
//! parent to a surface on another one; `open` takes GDK's display rather than
//! calling `wl_display_connect`, and Mesa is handed the same pointer (see
//! `egl_get_display`, whose comment on the second-connection hazard now
//! applies to GDK's connection rather than to one of this file's own).
//!
//! `wl_subsurface.set_desync` is not optional. A subsurface starts
//! *synchronised*, meaning its commits do not take effect until the parent
//! commits — the engine would present frames that appear only when GTK
//! happened to repaint, which for a static window is never.
//!
//! `set_position` is the mirror image: it *is* latched on the parent's commit,
//! so moving the canvas needs GTK to repaint afterwards
//! (`HostWindow::queue_commit`).
//!
//! Input is filtered by surface. Cordial's `wl_pointer` is a second pointer
//! object on the same seat as GDK's, so it sees `enter` for the header bar
//! too; forwarding those to the engine would have it react to clicks on the
//! window controls. `pointer_enter` records which surface the pointer is on
//! and nothing is delivered unless it is the engine's.
//!
//! ## `zwp_text_input_v3` had a version-2 event table written to version 1
//!
//! **Correction to what this comment used to say.** It recorded `interface
//! 'zwp_text_input_v3' has no event 8` as a live bug and explained it as
//! "event 8 exists in `zwp_text_input_v2`", a different protocol. That is
//! wrong, and the real explanation is entirely inside this file.
//!
//! `zwp_text_input_v3` **version 2** — which is what GNOME 50's mutter
//! advertises, and what the `bind` below has always asked for — adds three
//! events to the six version 1 has: `action` (6), `language` (7) and
//! `preedit_hint` (8). Event 8 is `preedit_hint`. The table here declared six.
//! An object's version on the wire is inherited from the object that created
//! it, and nothing about passing a smaller number to `wl_proxy_marshal_flags`
//! changes what the compositor believes it may send — so binding the manager
//! at 2 and describing the child at 1 asks for events this file then cannot
//! receive.
//!
//! Measured on the wire, `WAYLAND_DEBUG=1`, before the fix:
//!
//! ```text
//! wl_registry#107.global(26, "zwp_text_input_manager_v3", 2)
//!  -> wl_registry#107.bind(26, "zwp_text_input_manager_v3", 2, new id [unknown]#74)
//!  -> zwp_text_input_manager_v3#74.get_text_input(new id [unknown]#71, wl_seat#103)
//! zwp_text_input_v3#71.enter(wl_surface#47)
//! ```
//!
//! Note the last line: the compositor starts talking to this object as soon as
//! the toplevel takes keyboard focus, with no `enable` sent and no field
//! clicked. There is no window in a session where it is dormant.
//!
//! The failure that follows is *not* a protocol error and this is worth being
//! precise about, because a wrong errno sends the next person somewhere else.
//! Reproduced standalone against this same compositor, by binding `wl_seat` at
//! version 8 behind a deliberately one-event table:
//!
//! ```text
//! interface 'wl_seat' has no event 1
//! roundtrip=-1  wl_display_get_error=11 (Resource temporarily unavailable)
//! ```
//!
//! libwayland refuses the event, puts the *whole display* into a permanent
//! error state, and leaves `errno` at whatever it was — 11, not 71. Every
//! client on the connection stops, which is the freeze. A `wl_display.error`
//! sent by the compositor is the other thing, gives 71 (`EPROTO`), and prints
//! `<interface>#<id>: error <code>: <reason>` first.
//!
//! The rule this file already applied to `wl_pointer`/`wl_keyboard` is the fix,
//! plus its converse: declare the complete event set for the version bound, and
//! never bind above the version whose table is written here. `bind` now takes
//! its version from `TEXT_INPUT_MANAGER_INTERFACE.version` so the two cannot
//! drift apart when wayland-protocols ships a version 3.
//!
//! One thing established earlier and still true, from `WAYLAND_DEBUG=1`:
//! bringing GTK into the process does not add a second text-input object.
//! There is exactly one `get_text_input` on the connection and it is this
//! file's, because GDK creates its own only when a GTK text widget takes
//! focus, and this window has none. That stops being true the moment anything
//! focusable-and-editable is added to the window — two `zwp_text_input_v3`
//! objects on one seat from one client would be a new and much harder bug, and
//! whoever adds an editor widget here has to resolve which of the two speaks
//! for Cordial before doing it.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

// ------------------------------------------------------------- wire layout
//
// `struct wl_interface`, from `wayland-util.h`. Only the type is needed now,
// as the parameter `wl_proxy_marshal_flags` takes and the type of the tables
// `dlsym` hands back; nothing here declares one. Its layout is part of
// libwayland-client's stable ABI — every language binding for Wayland depends
// on it not changing.

#[repr(C)]
struct WlInterface {
    name: *const c_char,
    version: c_int,
    method_count: c_int,
    methods: *const WlMessage,
    event_count: c_int,
    events: *const WlMessage,
}
// SAFETY: as `WlMessage` below.
unsafe impl Sync for WlInterface {}


// ------------------------------------------------- hand-written wire layout
//
// `struct wl_message`/the null `types` array, from `wayland-util.h`. Needed
// only by the two `zwp_text_input` tables below — every other interface this
// file uses comes out of `libwayland-client.so` itself. `wl_proxy_marshal_
// flags` gets a *new* object's interface from the explicit `interface`
// parameter at the call site rather than from `types[]`, which is read only
// for `WAYLAND_DEBUG` printing and for auto-creating a proxy for an incoming
// `new_id` event argument — `zwp_text_input_v3` has no such event, so the
// null fill costs a `(nil)` in debug output and nothing else.

#[repr(C)]
struct WlMessage {
    name: *const c_char,
    signature: *const c_char,
    types: *const *const WlInterface,
}
// SAFETY: every field is a pointer either to a `'static` C string literal or
// to another `'static` table defined in this file; nothing here is mutated
// after the enclosing `static` is initialised.
unsafe impl Sync for WlMessage {}

#[repr(C)]
struct NullTypes([*const WlInterface; 8]);
// SAFETY: all-null, never mutated.
unsafe impl Sync for NullTypes {}
static NO_TYPES: NullTypes = NullTypes([std::ptr::null(); 8]);

// ------------------------------------------------- zwp_text_input_manager_v3

static TEXT_INPUT_MANAGER_METHODS: [WlMessage; 2] = [
    WlMessage { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage {
        name: c"get_text_input".as_ptr(),
        signature: c"no".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
];
static TEXT_INPUT_MANAGER_INTERFACE: WlInterface = WlInterface {
    name: c"zwp_text_input_manager_v3".as_ptr(),
    // Version 2. The manager's own request set is unchanged from 1 — the bump
    // is on `zwp_text_input_v3` — but binding higher is *not* free, and this
    // number is the one that decides what the compositor may send on the child
    // object: a `zwp_text_input_v3` created by a v2 manager is a v2 object
    // however small a version this file passes to `wl_proxy_marshal_flags`.
    // `TEXT_INPUT_EVENTS` below therefore has to be complete for this number,
    // and `bind` reads it from here rather than repeating the literal, so the
    // table and the request cannot drift. `bind` still clamps to what the
    // compositor advertised, so a v1-only compositor works unchanged.
    version: 2,
    method_count: 2,
    methods: TEXT_INPUT_MANAGER_METHODS.as_ptr(),
    event_count: 0,
    events: std::ptr::null(),
};

const TEXT_INPUT_MANAGER_GET_TEXT_INPUT: u32 = 1;

// ------------------------------------------------------------ zwp_text_input_v3

static TEXT_INPUT_METHODS: [WlMessage; 11] = [
    WlMessage { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage { name: c"enable".as_ptr(), signature: c"".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage { name: c"disable".as_ptr(), signature: c"".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage {
        name: c"set_surrounding_text".as_ptr(),
        signature: c"sii".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage {
        name: c"set_text_change_cause".as_ptr(),
        signature: c"u".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage {
        name: c"set_content_type".as_ptr(),
        signature: c"uu".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage {
        name: c"set_cursor_rectangle".as_ptr(),
        signature: c"iiii".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage { name: c"commit".as_ptr(), signature: c"".as_ptr(), types: NO_TYPES.0.as_ptr() },
    // Version 2's three additions. None of them is sent by this file, and
    // `set_available_actions` in particular must not be sent casually — an
    // array containing `none`, or the same action twice, is the interface's own
    // `invalid_action` protocol error and would kill the connection. They are
    // declared so that `method_count` matches the version bound, in the same
    // spirit as the event table below: a table that describes a different
    // protocol version from the one on the wire is the bug this whole section
    // is about.
    WlMessage {
        name: c"set_available_actions".as_ptr(),
        signature: c"2a".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage { name: c"show_input_panel".as_ptr(), signature: c"2".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage { name: c"hide_input_panel".as_ptr(), signature: c"2".as_ptr(), types: NO_TYPES.0.as_ptr() },
];
static TEXT_INPUT_EVENTS: [WlMessage; 9] = [
    WlMessage { name: c"enter".as_ptr(), signature: c"o".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage { name: c"leave".as_ptr(), signature: c"o".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage {
        name: c"preedit_string".as_ptr(),
        signature: c"?sii".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage {
        name: c"commit_string".as_ptr(),
        signature: c"?s".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage {
        name: c"delete_surrounding_text".as_ptr(),
        signature: c"uu".as_ptr(),
        types: NO_TYPES.0.as_ptr(),
    },
    WlMessage { name: c"done".as_ptr(), signature: c"u".as_ptr(), types: NO_TYPES.0.as_ptr() },
    // Version 2, and where the recorded `has no event 8` came from. The
    // leading `2` in each
    // signature is the `since` version, exactly as `wayland-scanner` emits it
    // (`wl_seat`'s own `name` event reads `2s` in the host library); the
    // demarshaller skips it, so it is documentation that cannot go stale.
    WlMessage { name: c"action".as_ptr(), signature: c"2uu".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage { name: c"language".as_ptr(), signature: c"2s".as_ptr(), types: NO_TYPES.0.as_ptr() },
    WlMessage { name: c"preedit_hint".as_ptr(), signature: c"2uuu".as_ptr(), types: NO_TYPES.0.as_ptr() },
];
static TEXT_INPUT_INTERFACE: WlInterface = WlInterface {
    name: c"zwp_text_input_v3".as_ptr(),
    // Must equal the manager's, because that is what the compositor gives this
    // object — see the module doc.
    version: 2,
    method_count: 11,
    methods: TEXT_INPUT_METHODS.as_ptr(),
    event_count: 9,
    events: TEXT_INPUT_EVENTS.as_ptr(),
};

const TEXT_INPUT_ENABLE: u32 = 1;
const TEXT_INPUT_DISABLE: u32 = 2;
const TEXT_INPUT_SET_SURROUNDING_TEXT: u32 = 3;
const TEXT_INPUT_SET_CONTENT_TYPE: u32 = 5;
const TEXT_INPUT_SET_CURSOR_RECTANGLE: u32 = 6;
const TEXT_INPUT_COMMIT: u32 = 7;


// ----------------------------------------------------- subsurface opcodes
//
// `wl_subcompositor`/`wl_subsurface` are core protocol, so their
// `wl_interface` tables come out of `libwayland-client.so` itself like every
// other interface this file touches — see `WlClient::load`. Only the opcode
// numbers, fixed by `wayland.xml`, need naming.
const WL_SUBCOMPOSITOR_GET_SUBSURFACE: u32 = 1;
const WL_SUBSURFACE_SET_POSITION: u32 = 1;
const WL_SUBSURFACE_SET_DESYNC: u32 = 5;

// wl_compositor/wl_display/wl_registry/wl_seat/wl_pointer/wl_surface opcodes.
// Their `wl_interface` tables come from the library itself (dlsym'd below),
// so only the opcode numbers — fixed by `wayland.xml`, the core protocol —
// need naming here.
const WL_DISPLAY_GET_REGISTRY: u32 = 1;
const WL_REGISTRY_BIND: u32 = 0;
const WL_COMPOSITOR_CREATE_SURFACE: u32 = 0;
const WL_SURFACE_COMMIT: u32 = 6;
const WL_SEAT_GET_POINTER: u32 = 0;
const WL_SEAT_GET_KEYBOARD: u32 = 1;
const WL_POINTER_SET_CURSOR: u32 = 0;

// --------------------------------------------------------------- dlopen'd API

/// `wl_proxy_marshal_flags`'s C signature is variadic — the fixed prefix is
/// typed, and each call site below supplies however many trailing arguments
/// that message's signature actually needs. This is the same function
/// `wayland-scanner`'s generated inline wrappers call; there is no separate
/// "send a request" primitive underneath it.
type ProxyMarshalFlags = unsafe extern "C" fn(
    *mut c_void,
    u32,
    *const WlInterface,
    u32,
    u32,
    ...
) -> *mut c_void;

struct WlClient {
    get_fd: unsafe extern "C" fn(*mut c_void) -> c_int,
    flush: unsafe extern "C" fn(*mut c_void) -> c_int,
    dispatch_pending: unsafe extern "C" fn(*mut c_void) -> c_int,
    prepare_read: unsafe extern "C" fn(*mut c_void) -> c_int,
    read_events: unsafe extern "C" fn(*mut c_void) -> c_int,
    cancel_read: unsafe extern "C" fn(*mut c_void) -> c_int,
    roundtrip: unsafe extern "C" fn(*mut c_void) -> c_int,
    marshal_flags: ProxyMarshalFlags,
    add_listener: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int,
    /// What a proxy's version *actually* is, rather than what the call site
    /// that made it guessed. A child object inherits its parent's version, so
    /// this is the only honest source for the number `wl_proxy_marshal_flags`
    /// should be given when creating one — see the text-input section of the
    /// module doc for what a guess cost here.
    get_version: unsafe extern "C" fn(*mut c_void) -> u32,
    /// Set once the connection is unusable. Non-zero means every later request
    /// is discarded and every dispatch fails, so a run that reaches this is
    /// over whatever it does next; `pump` reports it rather than letting the
    /// process die with only GDK's `Error %d ... dispatching to Wayland
    /// display` to go on, which names neither the object nor the reason.
    get_error: unsafe extern "C" fn(*mut c_void) -> c_int,
    get_protocol_error:
        unsafe extern "C" fn(*mut c_void, *mut *const WlInterface, *mut u32) -> u32,

    registry_interface: *const WlInterface,
    compositor_interface: *const WlInterface,
    subcompositor_interface: *const WlInterface,
    subsurface_interface: *const WlInterface,
    surface_interface: *const WlInterface,
    seat_interface: *const WlInterface,
    pointer_interface: *const WlInterface,
    keyboard_interface: *const WlInterface,
}
// SAFETY: every field is either a function pointer (inherently `Send + Sync`
// — it is a code address, not aliased state) or a pointer into a host shared
// library that is dlopen'd once and never closed, exactly like `Xlib` in
// `window.rs`.
unsafe impl Send for WlClient {}
unsafe impl Sync for WlClient {}

struct WlEgl {
    create: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *mut c_void,
    resize: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int),
}
unsafe impl Send for WlEgl {}
unsafe impl Sync for WlEgl {}

struct Xkb {
    context_new: unsafe extern "C" fn(u32) -> *mut c_void,
    context_unref: unsafe extern "C" fn(*mut c_void),
    keymap_new_from_string: unsafe extern "C" fn(*mut c_void, *const c_char, u32, u32) -> *mut c_void,
    keymap_unref: unsafe extern "C" fn(*mut c_void),
    keymap_mod_get_index: unsafe extern "C" fn(*mut c_void, *const c_char) -> u32,
    state_new: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    state_unref: unsafe extern "C" fn(*mut c_void),
    // Deliberately no `xkb_state_update_key`: `wl_keyboard.modifiers` already
    // carries the compositor's own authoritative depressed/latched/locked
    // mask for every key event, applied via `state_update_mask` below.
    // Re-deriving it per keystroke with `state_update_key` would be redundant
    // for ordinary keys and actively wrong for modifier keys themselves,
    // double-applying a toggle the server already accounted for.
    state_update_mask: unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32, u32, u32) -> u32,
    state_key_get_one_sym: unsafe extern "C" fn(*mut c_void, u32) -> u32,
    state_key_get_utf8: unsafe extern "C" fn(*mut c_void, u32, *mut c_char, usize) -> c_int,
    state_mod_index_is_active: unsafe extern "C" fn(*mut c_void, u32, u32) -> c_int,
}
unsafe impl Send for Xkb {}
unsafe impl Sync for Xkb {}

const RTLD_NOW: c_int = 2;

extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn close(fd: c_int) -> c_int;
}

impl WlClient {
    fn load() -> Result<Self, String> {
        // SAFETY: a literal soname; the handle is never closed, matching
        // every other host library this runtime dlopen's.
        let lib = unsafe { dlopen(c"libwayland-client.so.0".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("libwayland-client.so.0 is not available".into());
        }
        macro_rules! sym {
            ($name:literal) => {{
                let name = CString::new($name).unwrap();
                // SAFETY: the handle is open and the name is one of
                // libwayland-client's documented exports (function or data
                // symbol), so the transmuted type is the one that name has.
                let p = unsafe { dlsym(lib, name.as_ptr()) };
                if p.is_null() {
                    return Err(format!("libwayland-client has no {}", $name));
                }
                unsafe { std::mem::transmute(p) }
            }};
        }
        Ok(WlClient {
            // Deliberately no `wl_display_connect`: the one connection in this
            // process is GTK's, and opening a second would give the engine a
            // surface whose buffers can never be attached to the window's. See
            // the module doc.
            get_fd: sym!("wl_display_get_fd"),
            flush: sym!("wl_display_flush"),
            dispatch_pending: sym!("wl_display_dispatch_pending"),
            prepare_read: sym!("wl_display_prepare_read"),
            read_events: sym!("wl_display_read_events"),
            cancel_read: sym!("wl_display_cancel_read"),
            roundtrip: sym!("wl_display_roundtrip"),
            marshal_flags: sym!("wl_proxy_marshal_flags"),
            add_listener: sym!("wl_proxy_add_listener"),
            get_version: sym!("wl_proxy_get_version"),
            get_error: sym!("wl_display_get_error"),
            get_protocol_error: sym!("wl_display_get_protocol_error"),
            registry_interface: sym!("wl_registry_interface"),
            compositor_interface: sym!("wl_compositor_interface"),
            subcompositor_interface: sym!("wl_subcompositor_interface"),
            subsurface_interface: sym!("wl_subsurface_interface"),
            surface_interface: sym!("wl_surface_interface"),
            seat_interface: sym!("wl_seat_interface"),
            pointer_interface: sym!("wl_pointer_interface"),
            keyboard_interface: sym!("wl_keyboard_interface"),
        })
    }
}

impl WlEgl {
    fn load() -> Result<Self, String> {
        // SAFETY: as `WlClient::load`.
        let lib = unsafe { dlopen(c"libwayland-egl.so.1".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("libwayland-egl.so.1 is not available".into());
        }
        macro_rules! sym {
            ($name:literal) => {{
                let name = CString::new($name).unwrap();
                let p = unsafe { dlsym(lib, name.as_ptr()) };
                if p.is_null() {
                    return Err(format!("libwayland-egl has no {}", $name));
                }
                unsafe { std::mem::transmute(p) }
            }};
        }
        Ok(WlEgl { create: sym!("wl_egl_window_create"), resize: sym!("wl_egl_window_resize") })
    }
}

/// `XKB_CONTEXT_NO_FLAGS`.
const XKB_CONTEXT_NO_FLAGS: u32 = 0;
/// `XKB_KEYMAP_FORMAT_TEXT_V1` — the only format `wl_keyboard.keymap` sends.
const XKB_KEYMAP_FORMAT_TEXT_V1: u32 = 1;
/// `XKB_STATE_MODS_EFFECTIVE` — "is this modifier affecting keysym
/// translation right now", as opposed to merely depressed/latched/locked.
const XKB_STATE_MODS_EFFECTIVE: u32 = 1 << 3;

impl Xkb {
    fn load() -> Result<Self, String> {
        // SAFETY: as `WlClient::load`.
        let lib = unsafe { dlopen(c"libxkbcommon.so.0".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("libxkbcommon.so.0 is not available".into());
        }
        macro_rules! sym {
            ($name:literal) => {{
                let name = CString::new($name).unwrap();
                let p = unsafe { dlsym(lib, name.as_ptr()) };
                if p.is_null() {
                    return Err(format!("libxkbcommon has no {}", $name));
                }
                unsafe { std::mem::transmute(p) }
            }};
        }
        Ok(Xkb {
            context_new: sym!("xkb_context_new"),
            context_unref: sym!("xkb_context_unref"),
            keymap_new_from_string: sym!("xkb_keymap_new_from_string"),
            keymap_unref: sym!("xkb_keymap_unref"),
            keymap_mod_get_index: sym!("xkb_keymap_mod_get_index"),
            state_new: sym!("xkb_state_new"),
            state_unref: sym!("xkb_state_unref"),
            state_update_mask: sym!("xkb_state_update_mask"),
            state_key_get_one_sym: sym!("xkb_state_key_get_one_sym"),
            state_key_get_utf8: sym!("xkb_state_key_get_utf8"),
            state_mod_index_is_active: sym!("xkb_state_mod_index_is_active"),
        })
    }
}

// ------------------------------------------------------------------ listeners
//
// `wl_proxy_add_listener` takes a pointer to an array of function pointers,
// one per event opcode in that interface's table, plus an opaque userdata
// pointer handed back on every call. A `#[repr(C)]` struct of function
// pointers in opcode order *is* that array — no different from how
// `wayland-scanner`'s generated `wl_xxx_listener` structs are defined, just
// written by hand. Function pointers are `Send + Sync` unconditionally (they
// are code addresses, not aliased data), so unlike `WlInterface` above these
// need no manual `unsafe impl`.

#[repr(C)]
struct RegistryListener {
    global: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *const c_char, u32),
    global_remove: unsafe extern "C" fn(*mut c_void, *mut c_void, u32),
}

// `wl_pointer_interface`/`wl_keyboard_interface` (below) are `dlsym`'d from
// the host's real `libwayland-client.so`, not hand-written like the
// `text-input` tables above — so their `event_count` is whatever the *host's*
// library version really declares, not whatever this file happens to have a
// listener field for. `dispatch_event` indexes the listener array
// `wl_proxy_add_listener` was given by the wire opcode with no bounds check of
// its own, so every one of wl_seat's core-protocol interfaces needs its
// *complete, current* event set declared here regardless of which `wl_seat`
// version this file requests.
//
// The crash this prevents was measured, on a listener struct that is no longer
// here to point at: the `xdg_toplevel` one, back when this file owned the
// toplevel. It was two fields long against an interface Mutter sent a fifth
// event on, and `wl_closure_invoke` jumped to address `0xe0` — a small garbage
// address, which is what reading past the end of a listener array looks like.
// The lesson outlived the code: "the compositor will not send events past the
// version I bound" did not hold on GNOME Shell. `PointerListener` was previously missing
// `frame`/`axis_source`/`axis_stop`/`axis_discrete`/`axis_value120`/
// `axis_relative_direction` (added in `wl_pointer` v5, v5, v5, v5, v8, v9);
// `KeyboardListener` below was missing `repeat_info` (`wl_keyboard` v4). Every
// new field here is a genuine no-op — none of scroll-wheel batching, event
// framing, or key-repeat timing is implemented — but the slot has to exist.
#[repr(C)]
struct PointerListener {
    enter: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void, i32, i32),
    leave: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void),
    motion: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, i32, i32),
    button: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, u32, u32),
    axis: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, i32),
    frame: unsafe extern "C" fn(*mut c_void, *mut c_void),
    axis_source: unsafe extern "C" fn(*mut c_void, *mut c_void, u32),
    axis_stop: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32),
    axis_discrete: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, i32),
    axis_value120: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, i32),
    axis_relative_direction: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32),
}

/// `struct wl_array` (`wayland-util.h`): `{ size_t size; size_t alloc; void
/// *data; }`. Only the layout matters here — `wl_keyboard.enter`'s pressed-key
/// array is received and ignored, since it does not change what Cordial does
/// with a key.
#[repr(C)]
struct WlArray {
    size: usize,
    alloc: usize,
    data: *mut c_void,
}

#[repr(C)]
struct KeyboardListener {
    keymap: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, c_int, u32),
    enter: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void, *const WlArray),
    leave: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void),
    key: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, u32, u32),
    modifiers: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, u32, u32, u32),
    repeat_info: unsafe extern "C" fn(*mut c_void, *mut c_void, i32, i32),
}

/// Nine slots, not six. The last three are `zwp_text_input_v3` version 2's
/// `action`/`language`/`preedit_hint`, and they are here for the same reason
/// `PointerListener` carries slots for scroll events nothing implements: the
/// compositor sends by opcode, `dispatch_event` indexes this array by that
/// opcode with no bounds check of its own, and the version the compositor
/// thinks this object has is the manager's, not the number this file passes
/// around. Leaving them out is what produced `interface 'zwp_text_input_v3'
/// has no event 8` — see the module doc for the measurement.
#[repr(C)]
struct TextInputListener {
    enter: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
    leave: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
    preedit_string: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char, i32, i32),
    commit_string: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char),
    delete_surrounding_text: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32),
    done: unsafe extern "C" fn(*mut c_void, *mut c_void, u32),
    action: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32),
    language: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char),
    preedit_hint: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, u32),
}

// -------------------------------------------------------------------- state

struct Geometry {
    width: i32,
    height: i32,
    format: i32,
}

/// One accumulated `zwp_text_input_v3` double-buffer group — see the module
/// doc's "double-buffered" paragraph. `None` means "this event type did not
/// arrive since the last `done`", which is different from arriving with an
/// empty/null payload — that distinction is exactly what an
/// `Option<Option<_>>` expresses and a plain default-empty value would lose.
#[derive(Default)]
struct PendingImeGroup {
    /// Outer: did `commit_string` arrive this group. Inner: its (nullable)
    /// text.
    commit: Option<Option<String>>,
    /// Outer: did `preedit_string` arrive this group. Inner: its (nullable)
    /// text plus the cursor range *within that preedit text*, in bytes.
    preedit: Option<Option<(String, i32, i32)>>,
    delete: Option<(u32, u32)>,
}

struct ImeState {
    /// The composing string currently shown, spliced into the committed text
    /// at the caret — `None` when nothing is being composed. Kept apart from
    /// `input::edit_text_buffer`'s buffer on purpose: see the module doc.
    preedit: Option<(String, i32, i32)>,
    pending: PendingImeGroup,
    /// Whether `enable()` has been sent for the currently focused box, so
    /// enable/disable only fire on an actual focus transition rather than on
    /// every input-pump tick.
    enabled: bool,
    /// The last `textbox_generation()` this was synchronised against.
    synced_generation: Option<u32>,
    /// Whether the input method has produced any text for the current focus
    /// session — set by the first `commit_string` or `preedit_string` to
    /// arrive, cleared on `leave`.
    ///
    /// This is what stops the two paths inserting the same character twice.
    /// Both `wl_keyboard` and `zwp_text_input_v3` can deliver text, and which
    /// one actually does depends entirely on the user's setup: with no input
    /// source configured — `org.gnome.desktop.input-sources sources` empty,
    /// which is the default on a fresh GNOME — the compositor answers `enable`
    /// with nothing but `done`, and every character arrives through
    /// `wl_keyboard`. Configure an engine such as ibus typing-booster and the
    /// same keystrokes arrive as preedit and commits instead, with the
    /// compositor free to also forward the raw key.
    ///
    /// So neither path can be the only one, and neither can be trusted to be
    /// silent. The keyboard path inserts text only while this is false; once
    /// an input method speaks for this session it owns the text and the
    /// keyboard is left to arrows, Enter and shortcuts.
    ime_producing: bool,
}
struct XkbState {
    xkb: Xkb,
    context: *mut c_void,
    keymap: *mut c_void,
    state: *mut c_void,
    shift_idx: u32,
    ctrl_idx: u32,
    alt_idx: u32,
    caps_idx: u32,
}
// SAFETY: only ever touched from the input-pump thread (see the module doc
// on `pump_input_events` never running concurrently with itself); the
// pointers are opaque libxkbcommon handles this runtime owns exclusively.
unsafe impl Send for XkbState {}
unsafe impl Sync for XkbState {}

/// The GTK window the engine's surface hangs under.
///
/// GTK objects are `Rc`-refcounted, not atomically, so touching this from two
/// threads corrupts a refcount rather than failing a lock. Everything that
/// reaches it — `open`, `pump`, the geometry sync — runs on the thread that
/// called `open`, which is the same thread `looper::pump` runs on, which is
/// the thread Android calls the UI thread. Nothing else may touch it, and
/// that is the whole justification for the `unsafe impl` below.
struct HostWindowCell(cordial_shell::host_window::HostWindow);
// SAFETY: see above — main-thread-only by construction, and only reachable
// through `&WaylandWindow`, whose other users (Mesa's EGL/Vulkan paths, from
// the engine's render thread) never call the methods that go through here.
unsafe impl Send for HostWindowCell {}
unsafe impl Sync for HostWindowCell {}

pub struct WaylandWindow {
    wl: WlClient,
    egl: Option<WlEgl>,
    display: *mut c_void,
    host: HostWindowCell,
    // Kept named and typed even though only `surface`/`subsurface` are read
    // again after construction — the rest are still owned proxies for the
    // life of this one-window-per-process runtime (the same scope
    // `window.rs`'s `HostWindow` has), and naming them documents the object
    // graph a future teardown or diagnostic would need, rather than letting
    // it go unrecorded because nothing currently reads it back.
    #[allow(dead_code)]
    compositor: *mut c_void,
    #[allow(dead_code)]
    subcompositor: *mut c_void,
    surface: *mut c_void,
    subsurface: *mut c_void,
    /// GTK's own toplevel `wl_surface` — the subsurface's parent, and the
    /// surface `wl_keyboard`/`wl_pointer` report focus against for everything
    /// that is not the canvas.
    parent_surface: *mut c_void,
    #[allow(dead_code)]
    seat: *mut c_void,
    #[allow(dead_code)]
    pointer: *mut c_void,
    #[allow(dead_code)]
    keyboard: *mut c_void,
    text_input: *mut c_void,
    conn_fd: c_int,

    buffers: Mutex<Geometry>,
    /// Where the canvas currently sits inside the parent surface. Compared
    /// against the content widget's allocation every pump so a resize or a
    /// header-bar height change moves the subsurface exactly once, rather than
    /// re-sending `set_position` on every tick.
    placed_at: Mutex<(i32, i32)>,
    egl_window: Mutex<*mut c_void>,

    xkb: Mutex<Option<XkbState>>,
    pointer_pos: Mutex<(f32, f32)>,
    pointer_buttons: AtomicI32,
    down_time_ms: AtomicI64,
    clock: std::time::Instant,

    ime: Mutex<ImeState>,

    /// The `GameActivity` handle `pump_input_events` was last called with.
    /// AGDK callbacks (`surface_resized` in particular) need this, but they
    /// run from inside `wl_display_dispatch_pending`, invoked from listener
    /// callbacks that have no handle parameter of their own — the protocol's
    /// event signatures are fixed, not something this file can extend. `0` is
    /// "no handle observed yet", which is never a real `GameActivity` handle.
    active_handle: AtomicI64,
}
// SAFETY: every raw pointer field is either a `libwayland-client` proxy (only
// ever touched from the single input-pump thread, matching the file-level
// "must never block" constraint `window.rs` documents for X11) or a host
// library handle from a library this runtime never closes.
unsafe impl Send for WaylandWindow {}
unsafe impl Sync for WaylandWindow {}

static WINDOW: OnceLock<WaylandWindow> = OnceLock::new();

/// TEMPORARY INSTRUMENTATION -- not for commit. `CORDIAL_INSTR=1`.
fn instr_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_INSTR").is_some())
}

// -------------------------------------------------------------- construction

/// Collected while walking `wl_registry`'s globals. A plain struct on the
/// stack rather than anything in `WaylandWindow`, because at this point in
/// `open()` there is no window yet to attach it to — this is the same
/// chicken-and-egg `window.rs` does not have to solve (X11 resource ids are
/// valid the moment they are allocated; Wayland globals have to be *told
/// about* before they can be bound).
#[derive(Default)]
struct Globals {
    compositor: Option<(u32, u32)>,
    subcompositor: Option<(u32, u32)>,
    seat: Option<(u32, u32)>,
    text_input_manager: Option<(u32, u32)>,
}

unsafe extern "C" fn registry_global(
    data: *mut c_void,
    _registry: *mut c_void,
    name: u32,
    interface: *const c_char,
    version: u32,
) {
    let globals = &mut *(data as *mut Globals);
    // SAFETY: `wl_registry.global`'s `interface` argument is a NUL-terminated
    // string per the protocol.
    let iface = CStr::from_ptr(interface).to_string_lossy();
    match iface.as_ref() {
        "wl_compositor" => globals.compositor = Some((name, version)),
        "wl_subcompositor" => globals.subcompositor = Some((name, version)),
        "wl_seat" => globals.seat = Some((name, version)),
        "zwp_text_input_manager_v3" => globals.text_input_manager = Some((name, version)),
        _ => {}
    }
}

unsafe extern "C" fn registry_global_remove(_data: *mut c_void, _registry: *mut c_void, _name: u32) {
    // A global disappearing mid-session (compositor restart, seat unplug) is
    // not handled — the same scope Cordial's X11 backend keeps: one window,
    // fixed for the process's life. Noting rather than silently ignoring, in
    // case a future bug report starts here.
    super::trace(format_args!("wayland: a global was removed; not handled"));
}

static REGISTRY_LISTENER: RegistryListener =
    RegistryListener { global: registry_global, global_remove: registry_global_remove };

pub fn open(width: u32, height: u32, title: &str) -> Result<&'static WaylandWindow, String> {
    if let Some(w) = WINDOW.get() {
        return Ok(w);
    }
    let wl = WlClient::load()?;

    // ---- the window itself. GTK opens the connection, owns the
    // `xdg_toplevel`, draws the header bar and answers configure/ack; this
    // file's job starts at the content area and stops there. See the module
    // doc for why the engine's surface cannot live on a connection of its own.
    cordial_shell::host_window::init_wayland()?;
    let host = cordial_shell::host_window::HostWindow::with_canvas(title, width as i32, height as i32);
    host.present();
    host.wait_until_mapped(std::time::Duration::from_secs(5))?;

    let display = host.wl_display().ok_or_else(|| {
        "GTK's display is not a Wayland display (GDK_BACKEND?)".to_string()
    })?;
    let parent_surface =
        host.wl_surface().ok_or_else(|| "GTK's window has no wl_surface".to_string())?;
    let (cx, cy, cw, ch) =
        host.content_rect().ok_or_else(|| "GTK's window has no content allocation".to_string())?;

    // ---- registry: walk every global, remembering the ones this backend
    // needs, then roundtrip so the walk is known to be complete before
    // anything tries to bind from it.
    //
    // This runs on GDK's connection and its default queue, so the roundtrip
    // also dispatches whatever GTK had waiting. That is fine — it is the same
    // thread GTK's main loop runs on — but it is why nothing here may assume
    // it is the only code touching the connection.
    let registry = unsafe {
        (wl.marshal_flags)(
            display,
            WL_DISPLAY_GET_REGISTRY,
            wl.registry_interface,
            1,
            0,
            std::ptr::null_mut::<c_void>(),
        )
    };
    if registry.is_null() {
        return Err("wl_display_get_registry failed".into());
    }
    // Leaked on purpose, and this is not tidiness that got away.
    //
    // The registry proxy is never destroyed, so its listener stays registered
    // for the process's whole life, and `wl_registry.global` fires again
    // whenever the compositor adds one — a monitor hotplug, a seat appearing.
    // A `&mut` to a local here would have `open()`'s stack frame under it,
    // long since returned and reused, and `registry_global` writes through
    // that pointer. That is the *exact* shape of §1a's bug 2, where a
    // stack-local `XdgSurfaceListener` stayed registered and the first
    // configure that arrived afterwards jumped through reused stack bytes; the
    // only difference is that this one writes rather than calls, so it would
    // corrupt something instead of crashing where it happened. Not observed to
    // fire — a global has to arrive that this file matches on — which is why
    // it is worth fixing now rather than after it does.
    let globals: &'static mut Globals = Box::leak(Box::new(Globals::default()));
    unsafe {
        (wl.add_listener)(
            registry,
            &REGISTRY_LISTENER as *const RegistryListener as *const c_void,
            globals as *mut Globals as *mut c_void,
        );
        if (wl.roundtrip)(display) < 0 {
            return Err("wl_display_roundtrip failed while enumerating globals".into());
        }
    }

    let Some((compositor_name, compositor_ver)) = globals.compositor else {
        return Err("compositor advertises no wl_compositor".into());
    };
    let Some((subcompositor_name, subcompositor_ver)) = globals.subcompositor else {
        // Every desktop compositor has one — it is core protocol, not an
        // extension — but saying so plainly beats a null-pointer failure three
        // calls later if one ever does not.
        return Err("compositor advertises no wl_subcompositor; the engine's surface cannot be embedded".into());
    };
    let Some((seat_name, seat_ver)) = globals.seat else {
        return Err("compositor advertises no wl_seat".into());
    };
    // The text-input manager is the whole point of choosing Wayland (see the
    // module doc), but its absence should not make the window fail to open —
    // a compositor with no `zwp_text_input_v3` support still renders and
    // takes mouse/keyboard input correctly through everything else here, and
    // failing outright would make "no IME protocol" look like "no Wayland at
    // all". Text entry simply will not compose through an IME on such a
    // compositor, which is reported once, not hidden.
    let text_input_manager_global = globals.text_input_manager;
    if text_input_manager_global.is_none() {
        eprintln!(
            "[android] wayland: compositor advertises no zwp_text_input_manager_v3; \
             text entry will not have IME composition (see ADR-011)"
        );
    }

    let bind = |name: u32, want_version: u32, target_ver: u32, interface: &WlInterface, iface_name: &str| unsafe {
        let version = want_version.min(target_ver);
        let iface_c = CString::new(iface_name).unwrap();
        (wl.marshal_flags)(
            registry,
            WL_REGISTRY_BIND,
            interface as *const WlInterface,
            version,
            0,
            name,
            iface_c.as_ptr(),
            version,
            std::ptr::null_mut::<c_void>(),
        )
    };

    // SAFETY (this whole block): every proxy bound below comes from a global
    // this same roundtrip just confirmed exists, at a version clamped to
    // what the compositor actually advertised. Binding a global GTK has also
    // bound is ordinary — a global may be bound any number of times, and the
    // resulting objects are independent.
    let compositor = bind(compositor_name, 1, compositor_ver, unsafe { &*wl.compositor_interface }, "wl_compositor");
    if compositor.is_null() {
        return Err("binding wl_compositor failed".into());
    }
    let subcompositor = bind(
        subcompositor_name,
        1,
        subcompositor_ver,
        unsafe { &*wl.subcompositor_interface },
        "wl_subcompositor",
    );
    if subcompositor.is_null() {
        return Err("binding wl_subcompositor failed".into());
    }
    let seat = bind(seat_name, 1, seat_ver, unsafe { &*wl.seat_interface }, "wl_seat");
    if seat.is_null() {
        return Err("binding wl_seat failed".into());
    }

    // The version asked for comes from the table, not from a literal repeated
    // here. Those two numbers being allowed to disagree is the whole of the
    // freeze recorded in the module doc: this call said 2, `TEXT_INPUT_EVENTS`
    // described 1, and the compositor sent event 8 to a six-slot listener.
    let text_input_manager = text_input_manager_global.and_then(|(name, ver)| {
        let want = TEXT_INPUT_MANAGER_INTERFACE.version as u32;
        let m = bind(name, want, ver, &TEXT_INPUT_MANAGER_INTERFACE, "zwp_text_input_manager_v3");
        (!m.is_null()).then_some(m)
    });

    // ---- the canvas: a plain `wl_surface` given the subsurface role against
    // GTK's toplevel. No `xdg_surface`, no configure handshake, no ack — a
    // subsurface has no size of its own to negotiate, it is whatever its
    // buffer is, wherever its parent says.
    let surface = unsafe {
        (wl.marshal_flags)(
            compositor,
            WL_COMPOSITOR_CREATE_SURFACE,
            wl.surface_interface,
            1,
            0,
            std::ptr::null_mut::<c_void>(),
        )
    };
    if surface.is_null() {
        return Err("wl_compositor.create_surface failed".into());
    }
    let subsurface = unsafe {
        (wl.marshal_flags)(
            subcompositor,
            WL_SUBCOMPOSITOR_GET_SUBSURFACE,
            wl.subsurface_interface,
            1,
            0,
            std::ptr::null_mut::<c_void>(),
            surface,
            parent_surface,
        )
    };
    if subsurface.is_null() {
        return Err("wl_subcompositor.get_subsurface failed".into());
    }
    unsafe {
        // Desync is what makes the engine's own commits take effect when the
        // engine makes them. A subsurface is created *synchronised*, meaning
        // every commit waits for the parent's — and GTK commits only when it
        // repaints, which for a window nobody is touching is never. Leave this
        // out and the canvas shows one frame per accidental GTK redraw.
        (wl.marshal_flags)(subsurface, WL_SUBSURFACE_SET_DESYNC, std::ptr::null(), 1, 0);
        (wl.marshal_flags)(subsurface, WL_SUBSURFACE_SET_POSITION, std::ptr::null(), 1, 0, cx, cy);
        (wl.marshal_flags)(surface, WL_SURFACE_COMMIT, std::ptr::null(), 1, 0);
    }
    // `set_position` is latched on the *parent's* next commit, so the canvas
    // sits at (0,0) — under the header bar — until GTK draws again.
    host.queue_commit();
    host.pump();

    println!("[android] wayland: canvas {cw}x{ch} at ({cx},{cy}) as a subsurface of the libadwaita window");

    // ---- wl_seat: pointer + keyboard. Cordial takes input from the seat
    // directly rather than through GTK, because what the engine wants is
    // Android `MotionEvent`/`KeyEvent` shapes and GTK's controllers would only
    // be a second translation of the same evdev data. GDK has its own pointer
    // and keyboard on this seat; both clients see every event, and
    // `pointer_enter` below is what keeps this one from acting on the ones
    // aimed at the header bar.
    let pointer = unsafe {
        (wl.marshal_flags)(seat, WL_SEAT_GET_POINTER, wl.pointer_interface, 1, 0, std::ptr::null_mut::<c_void>())
    };
    let keyboard = unsafe {
        (wl.marshal_flags)(seat, WL_SEAT_GET_KEYBOARD, wl.keyboard_interface, 1, 0, std::ptr::null_mut::<c_void>())
    };

    // ---- text-input-v3: created against the seat, listener wired once the
    // window exists (below), since its handlers use `current()`.
    let text_input = text_input_manager.and_then(|mgr| {
        // The child's version is the manager's, read back rather than assumed:
        // this used to pass a literal 1 while the manager was bound at 2, which
        // made every version check on this side answer for a protocol the
        // compositor was not speaking.
        // SAFETY: `mgr`/`seat` are live proxies bound above.
        let version = unsafe { (wl.get_version)(mgr) };
        let ti = unsafe {
            (wl.marshal_flags)(
                mgr,
                TEXT_INPUT_MANAGER_GET_TEXT_INPUT,
                &TEXT_INPUT_INTERFACE,
                version,
                0,
                std::ptr::null_mut::<c_void>(),
                seat,
            )
        };
        (!ti.is_null()).then_some(ti)
    });

    let conn_fd = unsafe { (wl.get_fd)(display) };

    let egl = match WlEgl::load() {
        Ok(e) => Some(e),
        Err(e) => {
            // Not fatal here — the caller only needs a window and Vulkan does
            // not go through `wl_egl_window` at all (see `vulkan.rs`). A
            // GLES-only host without `libwayland-egl.so.1` (unlikely, but not
            // impossible on a minimal install) still gets a working Vulkan
            // path this way.
            eprintln!("[android] wayland: {e}; GLES window surfaces will not be available");
            None
        }
    };

    let host = WaylandWindow {
        wl,
        egl,
        display,
        host: HostWindowCell(host),
        compositor,
        subcompositor,
        surface,
        subsurface,
        parent_surface,
        seat,
        pointer,
        keyboard,
        text_input: text_input.unwrap_or(std::ptr::null_mut()),
        conn_fd,
        buffers: Mutex::new(Geometry { width: cw, height: ch, format: super::window::WINDOW_FORMAT_RGBA_8888 }),
        placed_at: Mutex::new((cx, cy)),
        egl_window: Mutex::new(std::ptr::null_mut()),
        xkb: Mutex::new(None),
        pointer_pos: Mutex::new((0.0, 0.0)),
        pointer_buttons: AtomicI32::new(0),
        down_time_ms: AtomicI64::new(0),
        clock: std::time::Instant::now(),
        ime: Mutex::new(ImeState {
            preedit: None,
            pending: PendingImeGroup::default(),
            enabled: false,
            synced_generation: None,
            ime_producing: false,
        }),
        active_handle: AtomicI64::new(0),
    };
    let host = WINDOW.get_or_init(|| host);

    // Listeners that dereference `current()` can only be installed now.
    unsafe {
        if !pointer.is_null() {
            (host.wl.add_listener)(pointer, &POINTER_LISTENER as *const PointerListener as *const c_void, std::ptr::null_mut());
            // The cursor is hidden from `pointer_enter`, not here — see
            // `hide_pointer`. Sending it at setup was wrong twice over: there
            // is no valid serial to send yet, and the request has to be
            // repeated on every enter regardless.
        }
        if !keyboard.is_null() {
            (host.wl.add_listener)(keyboard, &KEYBOARD_LISTENER as *const KeyboardListener as *const c_void, std::ptr::null_mut());
        }
        if !host.text_input.is_null() {
            (host.wl.add_listener)(
                host.text_input,
                &TEXT_INPUT_LISTENER as *const TextInputListener as *const c_void,
                std::ptr::null_mut(),
            );
        }
        (host.wl.flush)(display);
    }

    Ok(host)
}

impl WaylandWindow {
    /// A resize GTK has already settled.
    ///
    /// The configure/ack handshake ADR-011 chose Wayland *for* still happens;
    /// it just happens in GTK now, which is the point of handing it the
    /// toplevel. By the time the content widget's allocation changes, the
    /// compositor and GTK have agreed on the new size, so this is the
    /// downstream half only: resize the EGL window if one exists, and tell the
    /// engine. Mirrors `window.rs::dispatch_configure`.
    fn apply_resize(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        let format = {
            let mut g = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
            if g.width == width && g.height == height {
                return;
            }
            g.width = width;
            g.height = height;
            g.format
        };
        let egl_win = *self.egl_window.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(egl), false) = (&self.egl, egl_win.is_null()) {
            // SAFETY: `egl_win` was created by `wl_egl_window_create` and is
            // still live (never destroyed for the process's lifetime, same
            // as everything else in this single-window runtime).
            unsafe { (egl.resize)(egl_win, width, height, 0, 0) };
        }
        let handle = self.active_handle.load(Ordering::Relaxed);
        if instr_on() {
            eprintln!("[instr] surface_resized -> {width}x{height}");
        }
        if handle != 0 {
            if let Err(e) = cordial_linker_sys::game_activity::surface_resized(handle, format, width, height) {
                super::trace(format_args!("wayland: surface resize failed: {e}"));
            }
        }
        if instr_on() {
            eprintln!("[instr] surface_resized {width}x{height} returned");
        }
    }

    /// Keep the canvas over the window's content area.
    ///
    /// GTK owns the layout, so this reads the content widget's allocation
    /// rather than tracking a configure sequence of its own — the header bar's
    /// height, the window's CSD inset and every resize all arrive through the
    /// same one number pair. Called every pump; both halves are no-ops unless
    /// something actually moved, because `set_position` costs a parent repaint
    /// and `apply_resize` costs the engine a surface-changed callback.
    fn sync_canvas_geometry(&self) {
        let Some((x, y, w, h)) = self.host.0.content_rect() else { return };
        let moved = {
            let mut placed = self.placed_at.lock().unwrap_or_else(|e| e.into_inner());
            let moved = *placed != (x, y);
            *placed = (x, y);
            moved
        };
        if moved {
            INSTR_SET_POSITIONS.fetch_add(1, Ordering::Relaxed);
            INSTR_QUEUE_COMMITS.fetch_add(1, Ordering::Relaxed);
            if instr_on() {
                eprintln!("[instr] set_position({x}, {y}) size={w}x{h}");
            }
            // SAFETY: `self.subsurface` is a live proxy for the process's
            // lifetime and `set_position`'s signature is "ii".
            unsafe {
                (self.wl.marshal_flags)(
                    self.subsurface,
                    WL_SUBSURFACE_SET_POSITION,
                    std::ptr::null(),
                    1,
                    0,
                    x,
                    y,
                );
            }
            // Latched on the parent's commit, not ours — see `HostWindow::
            // queue_commit`.
            self.host.0.queue_commit();
        }
        // `onSurfaceRedrawNeededNative` was tried here, on both halves of this
        // function, and **did not help** — see docs/NEXT.md §1e. The reasoning
        // was that `window.rs` drives that native from the final X11 `Expose`
        // and this backend drives it from nowhere, so an idle engine has nothing
        // telling it the canvas moved. The measurement says the engine already
        // repaints on `surface_resized` by itself: over the idle fullscreen
        // cycle of two otherwise identical 240-second runs, presents totalled
        // ~75 without the call and ~74 with it. Left out rather than kept as a
        // plausible-sounding no-op.
        self.apply_resize(w, h);
    }

    pub fn geometry(&self) -> (i32, i32, i32) {
        let g = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        (g.width, g.height, g.format)
    }

    pub fn connection_fd(&self) -> c_int {
        self.conn_fd
    }

    /// The pointer `eglGetPlatformDisplay`/`eglCreateWindowSurface` need —
    /// see `egl_get_display`/`egl_create_window_surface` in [`overrides`] for
    /// why the engine's own plain `eglGetDisplay(EGL_DEFAULT_DISPLAY)` call
    /// cannot be left to Mesa's own auto-connect.
    pub fn wl_display(&self) -> *mut c_void {
        self.display
    }

    pub fn wl_surface(&self) -> *mut c_void {
        self.surface
    }

    /// The `wl_egl_window*` EGL surfaces are created against, creating it on
    /// first use (the engine may call `eglCreateWindowSurface` more than once
    /// across its lifetime in principle; `wl_egl_window_create` must only run
    /// once per `wl_surface`).
    fn egl_window(&self) -> Option<*mut c_void> {
        let mut slot = self.egl_window.lock().unwrap_or_else(|e| e.into_inner());
        if !slot.is_null() {
            return Some(*slot);
        }
        let egl = self.egl.as_ref()?;
        let (w, h, _) = self.geometry();
        // SAFETY: `self.surface` is a live `wl_surface` proxy on this
        // connection; `wl_egl_window_create` is documented to accept it
        // directly.
        let win = unsafe { (egl.create)(self.surface, w, h) };
        if win.is_null() {
            return None;
        }
        *slot = win;
        Some(win)
    }
}

pub fn current() -> Option<&'static WaylandWindow> {
    WINDOW.get()
}

// ------------------------------------------------------------------ pointer
//
// `wl_fixed_t` is a 24.8 fixed-point number; `/256.0` is the exact inverse of
// how the compositor encoded it, per `wayland-util.h`'s own
// `wl_fixed_to_double`.
fn fixed_to_f32(v: i32) -> f32 {
    v as f32 / 256.0
}

/// Linux `input-event-codes.h` `BTN_*` values, which is what
/// `wl_pointer.button` reports — X11 numbers buttons 1/2/3 in click order,
/// Wayland reports the evdev code directly. `window.rs`'s equivalent table
/// has the fuller explanation of the primary/secondary/tertiary mismatch this
/// mirrors.
fn linux_button_to_android(button: u32) -> Option<i32> {
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;
    const BTN_MIDDLE: u32 = 0x112;
    match button {
        BTN_LEFT => Some(super::input::BUTTON_PRIMARY),
        BTN_RIGHT => Some(super::input::BUTTON_SECONDARY),
        BTN_MIDDLE => Some(super::input::BUTTON_TERTIARY),
        _ => None,
    }
}

impl WaylandWindow {
    fn now_ms(&self) -> i64 {
        self.clock.elapsed().as_millis() as i64
    }

    fn dispatch_pointer_motion(&self, x: f32, y: f32) {
        *self.pointer_pos.lock().unwrap_or_else(|e| e.into_inner()) = (x, y);
        let handle = self.active_handle.load(Ordering::Relaxed);
        let buttons = self.pointer_buttons.load(Ordering::Relaxed);
        let down_time = self.down_time_ms.load(Ordering::Relaxed);
        let now = self.now_ms();
        let action =
            if buttons != 0 { super::input::ACTION_MOVE } else { super::input::ACTION_HOVER_MOVE };
        if handle != 0 {
            super::input::deliver_touch(handle, action, x, y, buttons, 0, now, down_time);
        }
        super::input::pass_mouse_move(x, y);
    }

    fn dispatch_pointer_button(&self, android_button: i32, press: bool) {
        let (x, y) = *self.pointer_pos.lock().unwrap_or_else(|e| e.into_inner());
        let handle = self.active_handle.load(Ordering::Relaxed);
        let now = self.now_ms();

        if press {
            let before = self.pointer_buttons.fetch_or(android_button, Ordering::Relaxed);
            if before == 0 {
                self.down_time_ms.store(now, Ordering::Relaxed);
            }
            let buttons = self.pointer_buttons.load(Ordering::Relaxed);
            let down_time = self.down_time_ms.load(Ordering::Relaxed);
            if handle != 0 {
                super::input::deliver_touch(handle, super::input::ACTION_DOWN, x, y, buttons, 0, now, down_time);
                super::input::deliver_touch(
                    handle, super::input::ACTION_BUTTON_PRESS, x, y, buttons, android_button, now, down_time,
                );
            }
        } else {
            self.pointer_buttons.fetch_and(!android_button, Ordering::Relaxed);
            let buttons = self.pointer_buttons.load(Ordering::Relaxed);
            let down_time = self.down_time_ms.load(Ordering::Relaxed);
            if handle != 0 {
                super::input::deliver_touch(
                    handle, super::input::ACTION_BUTTON_RELEASE, x, y, buttons, android_button, now, down_time,
                );
                super::input::deliver_touch(handle, super::input::ACTION_UP, x, y, buttons, 0, now, down_time);
            }
        }

        // Every button, not only the primary one. The gate that used to stand
        // here dropped right and middle before they reached Roblox's own input
        // path, and a right-button drag is how a mouse turns the camera.
        super::input::pass_mouse_button(x, y, press, android_button);
    }

    /// One `wl_pointer.axis` event, converted to detents and handed to the
    /// shared wheel path.
    fn dispatch_pointer_axis(&self, axis: u32, value: f32) {
        let Some((hscroll, vscroll)) = axis_to_notches(axis, value) else {
            return;
        };
        let (x, y) = *self.pointer_pos.lock().unwrap_or_else(|e| e.into_inner());
        let handle = self.active_handle.load(Ordering::Relaxed);
        super::input::wheel(handle, x, y, hscroll, vscroll, self.now_ms());
    }
}

/// `wl_pointer.axis`'s (axis, distance) as `(hscroll, vscroll)` in detents with
/// Android's signs, or `None` for an axis this does not know.
///
/// Wayland's positive is down and to the right; Android's two scroll axes are
/// positive *up* and to the right, so the vertical one is negated and the
/// horizontal one is not. Getting that backwards is the "scrolling goes the
/// wrong way" report, which is why the sign lives in its own tested function
/// rather than inline in an event handler no test can reach.
fn axis_to_notches(axis: u32, value: f32) -> Option<(f32, f32)> {
    const AXIS_VERTICAL_SCROLL: u32 = 0;
    const AXIS_HORIZONTAL_SCROLL: u32 = 1;
    let notches = value / WHEEL_AXIS_STEP;
    match axis {
        AXIS_VERTICAL_SCROLL => Some((0.0, -notches)),
        AXIS_HORIZONTAL_SCROLL => Some((notches, 0.0)),
        _ => None,
    }
}

/// How much `wl_pointer.axis` reports for one detent of a mouse wheel.
///
/// `INFERRED`, and the one number on the Wayland scroll path that is not read
/// off the wire. `wl_pointer.axis` carries a distance in surface coordinates;
/// the events that carry a *count* — `axis_discrete` (version 5) and
/// `axis_value120` (version 8) — never arrive here, because `wl_seat` is bound
/// at version 1 (see the `bind` call) and a child object's version is its
/// parent's. Raising that would make this exact, and would also change what
/// `wl_keyboard` sends, which is a separate change with its own testing.
///
/// 10.0 is what mutter and Weston both use as their axis step for a discrete
/// wheel click. A compositor that disagrees makes every notch scroll by the
/// wrong amount but still in the right direction, and `CORDIAL_WHEEL_SCALE`
/// corrects it without a rebuild.
const WHEEL_AXIS_STEP: f32 = 10.0;

/// Whether the pointer is currently over the engine's canvas rather than over
/// the rest of the window.
///
/// Cordial's `wl_pointer` is a second pointer object on the seat GDK also has
/// one on, so the compositor delivers *every* enter, motion and button to both
/// — including the ones aimed at the header bar and the window controls. The
/// engine used to own the whole toplevel and there was nothing else for a
/// click to mean; now there is. Without this the engine reacts to a click on
/// the close button, and the cursor vanishes over the titlebar because
/// `hide_pointer` fired for it.
static POINTER_ON_CANVAS: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn pointer_enter(
    _data: *mut c_void,
    pointer: *mut c_void,
    serial: u32,
    surface: *mut c_void,
    x: i32,
    y: i32,
) {
    let Some(w) = current() else { return };
    let ours = std::ptr::eq(surface, w.surface);
    POINTER_ON_CANVAS.store(ours, Ordering::Release);
    if !ours {
        return;
    }
    // Arriving somewhere is not moving there. `pass_mouse_move` reports how far
    // the pointer travelled, and the distance from wherever it was when it last
    // left the canvas is not a movement the user made.
    super::input::reset_mouse_delta();
    w.hide_pointer(pointer, serial);
    // Subsurface coordinates are relative to the subsurface, so these are
    // already canvas-local — no offset for the header bar has to be
    // subtracted anywhere, which is the main practical reason to let the
    // compositor do this rather than translating window coordinates by hand.
    w.dispatch_pointer_motion(fixed_to_f32(x), fixed_to_f32(y));
}
unsafe extern "C" fn pointer_leave(_data: *mut c_void, _pointer: *mut c_void, _serial: u32, _surface: *mut c_void) {
    POINTER_ON_CANVAS.store(false, Ordering::Release);
    super::input::reset_mouse_delta();
}
unsafe extern "C" fn pointer_motion(_data: *mut c_void, _pointer: *mut c_void, _time: u32, x: i32, y: i32) {
    if !POINTER_ON_CANVAS.load(Ordering::Acquire) {
        return;
    }
    if let Some(w) = current() {
        w.dispatch_pointer_motion(fixed_to_f32(x), fixed_to_f32(y));
    }
}
unsafe extern "C" fn pointer_button(
    _data: *mut c_void,
    _pointer: *mut c_void,
    _serial: u32,
    _time: u32,
    button: u32,
    state: u32,
) {
    if !POINTER_ON_CANVAS.load(Ordering::Acquire) {
        return;
    }
    let Some(w) = current() else { return };
    let Some(android_button) = linux_button_to_android(button) else { return };
    w.dispatch_pointer_button(android_button, state == 1);
}
/// The scroll wheel. Filtered by surface like every other pointer event: the
/// header bar is GTK's, and a scroll over it is not the engine's to see.
unsafe extern "C" fn pointer_axis(_data: *mut c_void, _pointer: *mut c_void, _time: u32, axis: u32, value: i32) {
    if !POINTER_ON_CANVAS.load(Ordering::Acquire) {
        return;
    }
    if let Some(w) = current() {
        w.dispatch_pointer_axis(axis, fixed_to_f32(value));
    }
}
// `frame`/`axis_source`/`axis_stop`/`axis_discrete`/`axis_value120`/
// `axis_relative_direction` — see `PointerListener`'s own comment for why
// these slots must exist. They stay empty even now that scroll works: none of
// them is delivered to a version 1 `wl_pointer`, so an implementation here
// could never be tested and would be a claim rather than a result.
unsafe extern "C" fn pointer_frame(_data: *mut c_void, _pointer: *mut c_void) {}
unsafe extern "C" fn pointer_axis_source(_data: *mut c_void, _pointer: *mut c_void, _axis_source: u32) {}
unsafe extern "C" fn pointer_axis_stop(_data: *mut c_void, _pointer: *mut c_void, _time: u32, _axis: u32) {}
unsafe extern "C" fn pointer_axis_discrete(_data: *mut c_void, _pointer: *mut c_void, _axis: u32, _discrete: i32) {}
unsafe extern "C" fn pointer_axis_value120(_data: *mut c_void, _pointer: *mut c_void, _axis: u32, _value120: i32) {}
unsafe extern "C" fn pointer_axis_relative_direction(
    _data: *mut c_void,
    _pointer: *mut c_void,
    _axis: u32,
    _direction: u32,
) {
}

static POINTER_LISTENER: PointerListener = PointerListener {
    enter: pointer_enter,
    leave: pointer_leave,
    motion: pointer_motion,
    button: pointer_button,
    axis: pointer_axis,
    frame: pointer_frame,
    axis_source: pointer_axis_source,
    axis_stop: pointer_axis_stop,
    axis_discrete: pointer_axis_discrete,
    axis_value120: pointer_axis_value120,
    axis_relative_direction: pointer_axis_relative_direction,
};

// ----------------------------------------------------------------- keyboard

const MAP_FAILED: *mut c_void = -1isize as *mut c_void;

unsafe extern "C" fn keyboard_keymap(_data: *mut c_void, _kb: *mut c_void, format: u32, fd: c_int, size: u32) {
    let Some(w) = current() else {
        // SAFETY: the fd is Cordial's own now, per the protocol's fd-passing
        // contract, regardless of whether there is anywhere to put the
        // keymap it describes.
        unsafe { close(fd) };
        return;
    };
    if format != XKB_KEYMAP_FORMAT_TEXT_V1 {
        unsafe { close(fd) };
        return;
    }
    // SAFETY: `fd` was just received via `wl_keyboard.keymap`'s documented fd
    // argument, still open and exclusively Cordial's; `size` is the
    // compositor's own claim about its length, mapped read-only/private per
    // the protocol's stated contract for this event.
    let map = unsafe { mmap(std::ptr::null_mut(), size as usize, 1 /* PROT_READ */, 2 /* MAP_PRIVATE */, fd, 0) };
    if map == MAP_FAILED {
        unsafe { close(fd) };
        return;
    }

    let xkb = match Xkb::load() {
        Ok(x) => x,
        Err(e) => {
            super::trace(format_args!("wayland: {e}"));
            unsafe {
                munmap(map, size as usize);
                close(fd);
            }
            return;
        }
    };
    // SAFETY: `map` points at `size` bytes of the compositor-supplied keymap
    // text, which `wl_keyboard.keymap` documents as NUL-terminated.
    let context = unsafe { (xkb.context_new)(XKB_CONTEXT_NO_FLAGS) };
    let keymap = if context.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe {
            (xkb.keymap_new_from_string)(context, map as *const c_char, XKB_KEYMAP_FORMAT_TEXT_V1, 0)
        }
    };
    unsafe {
        munmap(map, size as usize);
        close(fd);
    }
    if keymap.is_null() {
        super::trace(format_args!("wayland: xkb_keymap_new_from_string failed"));
        if !context.is_null() {
            unsafe { (xkb.context_unref)(context) };
        }
        return;
    }
    let state = unsafe { (xkb.state_new)(keymap) };
    if state.is_null() {
        unsafe {
            (xkb.keymap_unref)(keymap);
            (xkb.context_unref)(context);
        }
        return;
    }

    let mod_index = |name: &CStr| unsafe { (xkb.keymap_mod_get_index)(keymap, name.as_ptr()) };
    let new = XkbState {
        shift_idx: mod_index(c"Shift"),
        ctrl_idx: mod_index(c"Control"),
        alt_idx: mod_index(c"Mod1"),
        caps_idx: mod_index(c"Lock"),
        xkb,
        context,
        keymap,
        state,
    };

    let mut slot = w.xkb.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = slot.take() {
        // A keymap change mid-session (layout switch) — release the old
        // one rather than leaking it.
        unsafe {
            (old.xkb.state_unref)(old.state);
            (old.xkb.keymap_unref)(old.keymap);
            (old.xkb.context_unref)(old.context);
        }
    }
    *slot = Some(new);
}

unsafe extern "C" fn keyboard_enter(
    _data: *mut c_void,
    _kb: *mut c_void,
    _serial: u32,
    surface: *mut c_void,
    _keys: *const WlArray,
) {
    // Keyboard focus lands on the *toplevel*, never on the subsurface — a
    // subsurface has no keyboard focus of its own in the protocol — so the
    // surface named here is GTK's, not the canvas. Checking it anyway rather
    // than accepting any surface, because this client now owns more than one
    // window's worth of surfaces (GTK's dialogs, its cursor surfaces) and
    // "some surface of ours has focus" is not the same claim as "the window
    // the engine is in has focus".
    let Some(w) = current() else { return };
    if std::ptr::eq(surface, w.parent_surface) {
        KEYBOARD_FOCUSED.store(true, Ordering::Release);
    }
}
/// Whether the compositor currently gives this surface keyboard focus.
///
/// `wl_keyboard.leave` was an empty stub, so Cordial kept processing every key
/// the seat delivered even after focus moved to another window — a `Ctrl+C`
/// typed into a terminal appeared in Cordial's own trace. That is a real
/// privacy problem and not merely a correctness one: a game client has no
/// business seeing keystrokes aimed at other applications, whatever it does
/// with them afterwards.
///
/// Wayland is not at fault here; the compositor sends `leave` precisely so a
/// client knows to stop. Cordial simply was not listening.
static KEYBOARD_FOCUSED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn keyboard_leave(_data: *mut c_void, _kb: *mut c_void, _serial: u32, _surface: *mut c_void) {
    KEYBOARD_FOCUSED.store(false, Ordering::Release);
}

unsafe extern "C" fn keyboard_key(_data: *mut c_void, _kb: *mut c_void, _serial: u32, _time: u32, key: u32, state: u32) {
    // Not ours to see. The seat can still deliver events around a focus
    // change; `KEYBOARD_FOCUSED` is what makes that harmless.
    if !KEYBOARD_FOCUSED.load(Ordering::Acquire) {
        return;
    }
    if let Some(w) = current() {
        w.dispatch_key(key, state == 1);
    }
}

unsafe extern "C" fn keyboard_modifiers(
    _data: *mut c_void,
    _kb: *mut c_void,
    _serial: u32,
    depressed: u32,
    latched: u32,
    locked: u32,
    group: u32,
) {
    let Some(w) = current() else { return };
    let guard = w.xkb.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(xk) = guard.as_ref() {
        // SAFETY: `xk.state` is a live `xkb_state` this same struct owns.
        unsafe { (xk.xkb.state_update_mask)(xk.state, depressed, latched, locked, 0, 0, group) };
    }
}

// `repeat_info` — see `KeyboardListener`'s own comment on `PointerListener`
// for why this slot must exist; key-repeat cadence is not implemented, this
// file relies on whatever repeat the host's own input layer already applies
// before events reach `wl_keyboard.key`.
unsafe extern "C" fn keyboard_repeat_info(_data: *mut c_void, _kb: *mut c_void, _rate: i32, _delay: i32) {}

static KEYBOARD_LISTENER: KeyboardListener = KeyboardListener {
    keymap: keyboard_keymap,
    enter: keyboard_enter,
    leave: keyboard_leave,
    key: keyboard_key,
    modifiers: keyboard_modifiers,
    repeat_info: keyboard_repeat_info,
};

impl WaylandWindow {
    /// A physical key event from `wl_keyboard`. Composition (dead keys, CJK,
    /// autocorrect) never reaches here: a compositor with an active input
    /// method routes the keys it wants to compose with directly to that IME
    /// instead of delivering them to this client's `wl_keyboard` at all —
    /// that routing is the compositor's job, not something this file
    /// arbitrates — so a key that *does* arrive here is, by construction,
    /// one the IME either does not exist or chose not to consume. Treating
    /// it exactly like `window.rs`'s X11 `dispatch_key` — direct
    /// keysym-driven insert/backspace/delete/move — is therefore correct
    /// rather than a fallback that risks double-entry against
    /// `zwp_text_input_v3`'s own `commit_string`.
    fn dispatch_key(&self, evdev_key: u32, down: bool) {
        // `xkb_keycode_t` is evdev's own code offset by 8 — XKB reserves the
        // low 8 for historical X11 reasons every xkbcommon consumer has to
        // replicate.
        let xkbcode = evdev_key + 8;

        let (keysym, unicode, text_len, text_buf, meta) = {
            let guard = self.xkb.lock().unwrap_or_else(|e| e.into_inner());
            let Some(xk) = guard.as_ref() else { return };
            // SAFETY: `xk.state` is live for as long as `guard` is held.
            let keysym = unsafe { (xk.xkb.state_key_get_one_sym)(xk.state, xkbcode) } as std::ffi::c_ulong;
            let mut text_buf = [0u8; 8];
            let n = unsafe {
                (xk.xkb.state_key_get_utf8)(xk.state, xkbcode, text_buf.as_mut_ptr() as *mut c_char, text_buf.len())
            };
            let is_active = |idx: u32| -> bool {
                idx != 0xffff_ffff
                    // SAFETY: as above.
                    && unsafe { (xk.xkb.state_mod_index_is_active)(xk.state, idx, XKB_STATE_MODS_EFFECTIVE) } == 1
            };
            let mut meta = 0;
            if is_active(xk.shift_idx) {
                meta |= super::input::META_SHIFT_ON;
            }
            if is_active(xk.ctrl_idx) {
                meta |= super::input::META_CTRL_ON;
            }
            if is_active(xk.alt_idx) {
                meta |= super::input::META_ALT_ON;
            }
            if is_active(xk.caps_idx) {
                meta |= super::input::META_CAPS_LOCK_ON;
            }
            let unicode = if n > 0 { text_buf[0] as i32 } else { 0 };
            (keysym, unicode, n.max(0) as usize, text_buf, meta)
        };

        let handle = self.active_handle.load(Ordering::Relaxed);
        let now = self.now_ms();

        if super::input::trace_text() {
            eprintln!(
                "[cordial] wayland key {} keysym={keysym:#x} text={:?} keycode={:?} focus={:?}",
                if down { "down" } else { "up" },
                std::str::from_utf8(&text_buf[..text_len]).unwrap_or(""),
                super::input::keysym_to_android(keysym),
                cordial_linker_sys::game_activity::focused_textbox(),
            );
        }

        if let Some(keycode) = super::input::keysym_to_android(keysym) {
            if handle != 0 {
                super::input::deliver_key(handle, down, keycode, evdev_key as i32, meta, 0, unicode, now, now);
            }
            super::input::pass_key_event(down, keycode, meta);
        } else {
            super::trace(format_args!("wayland: unmapped keysym {keysym:#x}"));
        }

        if !down {
            return;
        }
        let Some(which) = cordial_linker_sys::game_activity::focused_textbox() else { return };

        // `CORDIAL_NO_TEXT_BUFFER=1` sends key events only and never text.
        //
        // Cordial keeping a shadow copy of a field Roblox owns is a design
        // error, not a feature: it is why an empty group cleared the box, why
        // characters land at the end of the string regardless of where the
        // caret actually is, and why the caret position is this side's guess
        // rather than the engine's fact. Editing a text field is the input
        // method's job on Android and the engine's job on desktop; it is not
        // the host shim's job in either case.
        //
        // The open question is whether Roblox's engine edits its own TextBox
        // from `nativePassKeyEvent` alone, as it does on desktop, in which case
        // the buffer can be deleted outright rather than repaired. This switch
        // is how that gets answered by running rather than by argument.
        if std::env::var_os("CORDIAL_NO_TEXT_BUFFER").is_some() {
            return;
        }

        // If an input method is producing text for this session, it owns the
        // text and the keyboard must not also insert it — otherwise every
        // character an engine commits arrives twice. Editing keys still go
        // through: an IME consumes the characters it composes, not the arrows.
        let ime_owns_text = {
            let ime = self.ime.lock().unwrap_or_else(|e| e.into_inner());
            ime.ime_producing
        };

        let typed = std::str::from_utf8(&text_buf[..text_len]).unwrap_or("");
        // Same keysym set as `window.rs`'s X11 path — see its comment for why
        // these six are handled as edits rather than as text, and why an
        // unmapped keysym still falls through to `Edit::Insert` instead of
        // being dropped.
        let edit = match keysym {
            0xff08 => super::input::Edit::Backspace, // XK_BackSpace
            0xffff => super::input::Edit::Delete,    // XK_Delete
            0xff51 => super::input::Edit::Move(super::input::Caret::Left),
            0xff53 => super::input::Edit::Move(super::input::Caret::Right),
            0xff50 => super::input::Edit::Move(super::input::Caret::Home),
            0xff57 => super::input::Edit::Move(super::input::Caret::End),
            _ if ime_owns_text => return,
            _ => super::input::Edit::Insert(typed),
        };
        if let Some((contents, caret)) = super::input::edit_text_buffer(edit) {
            if handle != 0 {
                let _ = cordial_linker_sys::game_activity::text_input(handle, &contents, caret, caret);
            }
            self.send_current_text(which);
        }
    }
}

// -------------------------------------------------------------------- IME
//
// See the module doc's "double-buffered" and "preedit and committed text"
// paragraphs before changing anything below.

/// Splice a composing string into committed text at the caret, and report
/// where the caret should now appear to be. A pure function so its several
/// cases (no preedit; preedit with a mid-string cursor; preedit replacing the
/// prior one entirely) are unit-testable without any Wayland state at all —
/// see the tests at the bottom of this file.
fn splice_preedit(committed: &str, committed_caret_chars: i32, preedit: Option<&(String, i32, i32)>) -> (String, i32) {
    let Some((preedit_text, cursor_begin, _cursor_end)) = preedit else {
        return (committed.to_string(), committed_caret_chars);
    };
    let caret_byte = committed
        .char_indices()
        .nth(committed_caret_chars.max(0) as usize)
        .map(|(i, _)| i)
        .unwrap_or(committed.len());

    let mut spliced = String::with_capacity(committed.len() + preedit_text.len());
    spliced.push_str(&committed[..caret_byte]);
    spliced.push_str(preedit_text);
    spliced.push_str(&committed[caret_byte..]);

    // `cursor_begin` is a byte offset *within the preedit text*, per the
    // protocol; -1 means "the IME expresses no cursor position", which is
    // treated as "at the end of the composing text" — a reasonable default
    // and never worse than pinning it to the start.
    let want = if *cursor_begin < 0 { preedit_text.len() } else { (*cursor_begin as usize).min(preedit_text.len()) };
    let boundary = (0..=want).rev().find(|&i| preedit_text.is_char_boundary(i)).unwrap_or(0);
    let preedit_chars_before_cursor = preedit_text[..boundary].chars().count() as i32;

    (spliced, committed_caret_chars + preedit_chars_before_cursor)
}

impl WaylandWindow {
    /// Forward committed-text-with-preedit-spliced-at-the-caret to the
    /// engine — the one place both the hardware-key path and the IME `done`
    /// path funnel through, so they cannot disagree about how a live preedit
    /// is displayed.
    fn send_current_text(&self, which: i64) {
        let (committed, caret) = super::input::text_buffer_snapshot();
        let preedit = self.ime.lock().unwrap_or_else(|e| e.into_inner()).preedit.clone();
        let (text, caret) = splice_preedit(&committed, caret, preedit.as_ref());
        super::input::pass_text(which, &text, caret);
    }

    /// Drive `enable()`/`disable()` off the same focus signal `input.rs`
    /// already tracks (`focused_textbox`/`textbox_generation`), rather than
    /// this file inventing a second notion of "which box is focused". Cheap
    /// to call every pump tick: an atomic load and a comparison unless focus
    /// actually changed.
    fn sync_ime_focus(&self) {
        if self.text_input.is_null() {
            return;
        }
        let generation = cordial_linker_sys::game_activity::textbox_generation();
        let (was_enabled, just_focused, just_blurred) = {
            let mut ime = self.ime.lock().unwrap_or_else(|e| e.into_inner());
            if ime.synced_generation == Some(generation) {
                return;
            }
            ime.synced_generation = Some(generation);
            let now_focused = cordial_linker_sys::game_activity::focused_textbox().is_some();
            let was_enabled = ime.enabled;
            ime.enabled = now_focused;
            if now_focused != was_enabled {
                ime.preedit = None;
                ime.pending = PendingImeGroup::default();
            }
            (was_enabled, now_focused && !was_enabled, !now_focused && was_enabled)
        };
        let _ = was_enabled;
        if just_focused {
            self.ime_enable();
        } else if just_blurred {
            self.ime_disable();
        }
    }

    fn ime_enable(&self) {
        // SAFETY: `self.text_input` is non-null — checked by the only caller,
        // `sync_ime_focus` — and every signature below matches
        // `TEXT_INPUT_METHODS`'s table exactly.
        unsafe {
            (self.wl.marshal_flags)(self.text_input, TEXT_INPUT_ENABLE, std::ptr::null(), 1, 0);
            // hint=0 (none), purpose=0 (normal) — Roblox's own login form
            // does not expose which of its fields is the password field
            // anywhere this backend can read, so no field is marked
            // password-purpose. The practical cost is a candidate window
            // that may show what was composed for a password field, exactly
            // as it would for any other; there is no channel to do better
            // without engine-side support this file does not have.
            (self.wl.marshal_flags)(self.text_input, TEXT_INPUT_SET_CONTENT_TYPE, std::ptr::null(), 1, 0, 0u32, 0u32);
        }
        self.send_surrounding_text();
        self.send_cursor_rectangle();
        unsafe { (self.wl.marshal_flags)(self.text_input, TEXT_INPUT_COMMIT, std::ptr::null(), 1, 0) };
    }

    fn ime_disable(&self) {
        unsafe {
            (self.wl.marshal_flags)(self.text_input, TEXT_INPUT_DISABLE, std::ptr::null(), 1, 0);
            (self.wl.marshal_flags)(self.text_input, TEXT_INPUT_COMMIT, std::ptr::null(), 1, 0);
        }
    }

    /// Tell the IME what the field currently contains, so a predictive
    /// engine's corrections are made against real context rather than
    /// nothing. Sent once per focus gain rather than after every keystroke —
    /// an IME already knows what it itself just committed or deleted, so
    /// re-announcing state after every `done` this file *caused* would only
    /// add commit/serial churn without new information.
    fn send_surrounding_text(&self) {
        let (text, caret_chars) = super::input::text_buffer_snapshot();
        let caret_byte =
            text.char_indices().nth(caret_chars.max(0) as usize).map(|(i, _)| i).unwrap_or(text.len()) as i32;
        // The protocol caps surrounding text at 4000 bytes; Roblox's login
        // fields are nowhere near that, so no truncation is implemented.
        let Ok(cstr) = CString::new(text) else { return };
        unsafe {
            (self.wl.marshal_flags)(
                self.text_input,
                TEXT_INPUT_SET_SURROUNDING_TEXT,
                std::ptr::null(),
                1,
                0,
                cstr.as_ptr(),
                caret_byte,
                caret_byte,
            );
        }
    }

    /// Best-effort candidate-window placement.
    ///
    /// The reverse `showKeyboard` contract `input.rs` answers hands over a
    /// box's handle and contents, not its on-screen bounds (see
    /// `docs/NEXT.md` §1) — there is no engine API this backend can reach
    /// that reports where a text field is drawn. The last pointer position is
    /// used instead: it is where the user just clicked to focus the field,
    /// which is inside or very close to it in practice. That is a stand-in
    /// for real field geometry, not a claim of pixel accuracy.
    ///
    /// The offset is not decoration. `set_cursor_rectangle` is in the
    /// coordinate space of the surface the text input is *entered* on, and
    /// that is GTK's toplevel — a subsurface never takes keyboard focus, so it
    /// is never the entered surface. The pointer position this reads is
    /// canvas-local. Sending it unadjusted would put the candidate window a
    /// header bar and a drop shadow away from where the user is typing.
    fn send_cursor_rectangle(&self) {
        let (x, y) = *self.pointer_pos.lock().unwrap_or_else(|e| e.into_inner());
        let (ox, oy) = *self.placed_at.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            (self.wl.marshal_flags)(
                self.text_input,
                TEXT_INPUT_SET_CURSOR_RECTANGLE,
                std::ptr::null(),
                1,
                0,
                x as i32 + ox,
                y as i32 + oy,
                100i32,
                24i32,
            );
        }
    }

    /// Apply one accumulated `zwp_text_input_v3` double-buffer group. Only
    /// `done` calls this — every other text-input event above only records
    /// into `ImeState::pending`, never touches the committed buffer or the
    /// composing string directly. See the module doc.
    fn apply_ime_done(&self) {
        let Some(which) = cordial_linker_sys::game_activity::focused_textbox() else {
            // Nothing focused to apply this to — a group that arrived after a
            // blur raced `disable()`. Drop it rather than editing a buffer
            // whose focus generation has already moved on.
            let mut ime = self.ime.lock().unwrap_or_else(|e| e.into_inner());
            ime.pending = PendingImeGroup::default();
            return;
        };

        let (delete, commit, preedit_update) = {
            let mut ime = self.ime.lock().unwrap_or_else(|e| e.into_inner());
            let g = std::mem::take(&mut ime.pending);
            (g.delete, g.commit, g.preedit)
        };

        // A `done` carrying nothing is an acknowledgement, not an edit. The
        // compositor sends one in reply to `enable()`, and with no input method
        // configured it sends *only* these — the trace of a working client
        // shows `done(2)` through `done(13)` with no commit_string between
        // them.
        //
        // Treating that as an empty edit is destructive, because what gets
        // pushed to the engine is this side's whole idea of the field's
        // contents, which at focus time is nothing:
        //
        //     textbox focused handle=139983126597760 current=0 bytes
        //     text -> "" caret=0                  <- the field is cleared here
        //     textbox blurred                     <- and the engine drops focus
        //
        // Every keystroke after that logged `focus=None`, because there was no
        // longer a focused box to type into. So: an empty group changes
        // nothing and must not reach the engine at all.
        if delete.is_none() && commit.is_none() && preedit_update.is_none() {
            return;
        }

        // Applied in protocol order: delete relative to the cursor as it
        // stood before this group, then the commit is inserted at the
        // (now-current) cursor, then the new preedit — which may be "no
        // preedit" if the IME sent a null/empty one, a real event per the
        // module doc — replaces whatever was composing before.
        if let Some((before, after)) = delete {
            let _ = super::input::edit_text_buffer(super::input::Edit::DeleteSurrounding {
                before_bytes: before as usize,
                after_bytes: after as usize,
            });
        }
        if let Some(text) = commit {
            let text = text.unwrap_or_default();
            if !text.is_empty() {
                let _ = super::input::edit_text_buffer(super::input::Edit::Insert(&text));
            }
        }
        if let Some(new_preedit) = preedit_update {
            let mut ime = self.ime.lock().unwrap_or_else(|e| e.into_inner());
            ime.preedit = new_preedit;
        }

        let handle = self.active_handle.load(Ordering::Relaxed);
        if handle != 0 {
            let (committed, caret) = super::input::text_buffer_snapshot();
            let _ = cordial_linker_sys::game_activity::text_input(handle, &committed, caret, caret);
        }
        self.send_current_text(which);
    }
}

unsafe extern "C" fn ti_enter(_data: *mut c_void, _ti: *mut c_void, _surface: *mut c_void) {}
unsafe extern "C" fn ti_leave(_data: *mut c_void, _ti: *mut c_void, _surface: *mut c_void) {
    // Focus left this surface: whatever the input method was doing no longer
    // applies, so the keyboard path takes the text back until an input method
    // speaks again.
    if let Some(w) = current() {
        let mut ime = w.ime.lock().unwrap_or_else(|e| e.into_inner());
        ime.ime_producing = false;
        ime.preedit = None;
    }
}

unsafe extern "C" fn ti_preedit_string(
    _data: *mut c_void,
    _ti: *mut c_void,
    text: *const c_char,
    cursor_begin: i32,
    cursor_end: i32,
) {
    let Some(w) = current() else { return };
    // SAFETY: `text` is `zwp_text_input_v3.preedit_string`'s documented
    // nullable, NUL-terminated argument.
    let text = (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned());
    let mut ime = w.ime.lock().unwrap_or_else(|e| e.into_inner());
    // An input method has spoken for this session, so the keyboard path must
    // stop inserting text — see `ImeState::ime_producing`.
    ime.ime_producing = true;
    // A new preedit_string replaces the previous one entirely — this
    // assignment, not an append, is that rule.
    ime.pending.preedit = Some(text.map(|t| (t, cursor_begin, cursor_end)));
}

unsafe extern "C" fn ti_commit_string(_data: *mut c_void, _ti: *mut c_void, text: *const c_char) {
    let Some(w) = current() else { return };
    // SAFETY: as `ti_preedit_string`.
    let text = (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned());
    let mut ime = w.ime.lock().unwrap_or_else(|e| e.into_inner());
    ime.pending.commit = Some(text);
}

unsafe extern "C" fn ti_delete_surrounding_text(_data: *mut c_void, _ti: *mut c_void, before: u32, after: u32) {
    let Some(w) = current() else { return };
    let mut ime = w.ime.lock().unwrap_or_else(|e| e.into_inner());
    ime.pending.delete = Some((before, after));
}

unsafe extern "C" fn ti_done(_data: *mut c_void, _ti: *mut c_void, _serial: u32) {
    if let Some(w) = current() {
        w.apply_ime_done();
    }
}

/// `action` (version 2): the input method reports that the user activated the
/// field — the on-screen keyboard's Go/Search key. Nothing is done with it
/// because Cordial never sends `set_available_actions`, so the protocol says
/// no action is available and this cannot legitimately arrive; the slot exists
/// so that a compositor which sends it anyway is ignored rather than fatal.
/// If Roblox's `returnKeyType` is ever wired through (docs/NEXT.md §1), this is
/// where Enter-from-the-IME belongs.
unsafe extern "C" fn ti_action(_data: *mut c_void, _ti: *mut c_void, _action: u32, _serial: u32) {}
/// `language` (version 2): a BCP 47 tag for whatever the input method is
/// currently composing in, sent on creation and on every change. Roblox has no
/// call that takes it, so it is accepted and dropped.
unsafe extern "C" fn ti_language(_data: *mut c_void, _ti: *mut c_void, _language: *const c_char) {}
/// `preedit_hint` (version 2): how a range of the composing string should be
/// styled — underline, selection, spelling error. Cordial does not draw the
/// preedit itself (it splices it into the string the engine renders, see
/// `splice_preedit`), so there is nothing here to style. **This is event 8**,
/// the one whose absence produced the freeze recorded in the module doc.
unsafe extern "C" fn ti_preedit_hint(_data: *mut c_void, _ti: *mut c_void, _start: u32, _end: u32, _hint: u32) {}

static TEXT_INPUT_LISTENER: TextInputListener = TextInputListener {
    enter: ti_enter,
    leave: ti_leave,
    preedit_string: ti_preedit_string,
    commit_string: ti_commit_string,
    delete_surrounding_text: ti_delete_surrounding_text,
    done: ti_done,
    action: ti_action,
    language: ti_language,
    preedit_hint: ti_preedit_hint,
};


// --------------------------------------------------------- pointer cursor

impl WaylandWindow {
    /// Hide the host cursor over the canvas.
    ///
    /// `wl_pointer.set_cursor` takes the serial of the `wl_pointer.enter` it is
    /// answering, and a compositor ignores the request with any other value —
    /// which is why doing this once at setup with a serial of `0`, as this
    /// previously did, silently did nothing and left two cursors on screen.
    ///
    /// It also has to be repeated on *every* enter: the cursor a client sets
    /// applies to that enter only, and reverts as soon as the pointer leaves
    /// and returns. A null surface is the protocol's way of saying "no cursor
    /// at all", which is what Roblox wants because it draws its own.
    ///
    /// Only for enters onto the engine's own surface. Calling this for the
    /// header bar would take the pointer away from the window controls, which
    /// GTK is drawing hover states for and the user is trying to click.
    ///
    /// `CORDIAL_SHOW_CURSOR=1` restores the host cursor for debugging input.
    fn hide_pointer(&self, pointer: *mut c_void, serial: u32) {
        if pointer.is_null() || std::env::var_os("CORDIAL_SHOW_CURSOR").is_some() {
            return;
        }
        // SAFETY: `pointer` is the live `wl_pointer` this event arrived on, and
        // the argument list matches `set_cursor`'s `uoii` signature.
        unsafe {
            (self.wl.marshal_flags)(
                pointer,
                WL_POINTER_SET_CURSOR,
                std::ptr::null(),
                1,
                0,
                serial,
                std::ptr::null_mut::<c_void>(),
                0i32,
                0i32,
            );
        }
    }

}

// -------------------------------------------------------------------- pump
//
// Mirrors `window.rs`'s X11 pump: must never block, since it runs inside
// `looper::pump`'s bounded timeout loop on the thread that also owns the
// engine's message pump.

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 0x001;

// Same `*mut c_void` signature reasoning as `window.rs`'s own `poll` extern:
// two `extern "C" fn poll` declarations with different signatures anywhere in
// the crate trip `clashing_extern_declarations`, since both ultimately bind
// the one process-wide C symbol bionic's emulated libc also declares.
extern "C" {
    fn poll(fds: *mut c_void, nfds: u64, timeout_ms: c_int) -> c_int;
}

// TEMPORARY INSTRUMENTATION -- not for commit. See the session notes.
static INSTR_SET_POSITIONS: AtomicI64 = AtomicI64::new(0);
static INSTR_QUEUE_COMMITS: AtomicI64 = AtomicI64::new(0);

pub fn instr_geometry() -> String {
    let Some(w) = current() else { return "no-wayland-window".into() };
    format!(
        "rect={:?} placed={:?} setpos={} qcommit={}",
        w.host.0.content_rect(),
        *w.placed_at.lock().unwrap_or_else(|e| e.into_inner()),
        INSTR_SET_POSITIONS.load(Ordering::Relaxed),
        INSTR_QUEUE_COMMITS.load(Ordering::Relaxed),
    )
}

/// Drive fullscreen without a click, from `looper::pump`'s scripted timeline.
/// `gtk_window_fullscreen` is a request to the compositor made by this client
/// about its own window, so it exercises the same configure path a dragged
/// edge does without going anywhere near the developer's session.
pub fn instr_set_fullscreen(on: bool) {
    if let Some(w) = current() {
        w.host.0.set_fullscreen(on);
    }
}

impl WaylandWindow {
    fn pump(&self, handle: i64) {
        self.active_handle.store(handle, Ordering::Relaxed);
        let (gw, gh, _) = self.geometry();
        super::input::report_keyboard_state((gw, gh));

        // GTK's main loop does not get a thread of its own. It is iterated
        // here, on the thread that ran `gtk_init`, from inside the engine's
        // own message pump — which is what makes the header bar's buttons,
        // hover states and the compositor's resize handshake work at all,
        // since nothing else in this process ever runs a `GMainLoop`.
        //
        // The cost is that GTK's responsiveness is bounded by how often
        // `looper::pump` comes round, currently every 50ms or immediately on
        // any traffic on the display connection — and the display connection
        // is exactly where a click on the header bar arrives, so the idle case
        // is the only one that waits.
        self.host.0.pump();
        self.sync_canvas_geometry();
        self.sync_ime_focus();

        // The documented thread-safe idiom for a `wl_display` connection that
        // more than one thread touches — and more than one does: Mesa's own
        // Wayland EGL winsys reads and writes this exact connection from
        // whichever thread calls `eglSwapBuffers`/creates buffers, since
        // `egl_get_display` (below) hands it *this* display rather than
        // letting it open a second, unrelated one. `prepare_read` reserves
        // the right to be the next reader; if something else already holds
        // that reservation, back off to dispatching whatever is already
        // queued rather than contending for the socket.
        //
        // GDK is now a third party to the same connection — it owns it — and
        // uses this same idiom from its own `GSource`. That is why this stays
        // exactly as it was rather than being replaced by "let GTK do the
        // reading": the reservation is what makes two readers safe, and
        // `self.host.0.pump()` above having just run means the usual outcome
        // here is a `prepare_read` that succeeds with nothing left to read.
        //
        // SAFETY: `self.display` is live for the process's lifetime.
        if unsafe { (self.wl.prepare_read)(self.display) } != 0 {
            unsafe { (self.wl.dispatch_pending)(self.display) };
            return;
        }
        unsafe { (self.wl.flush)(self.display) };

        let mut pfd = PollFd { fd: self.conn_fd, events: POLLIN, revents: 0 };
        // SAFETY: `pfd` is a live value for the call; a 0ms timeout makes
        // this a pure non-blocking check, exactly as in `window.rs`.
        let ready = unsafe { poll(&mut pfd as *mut PollFd as *mut c_void, 1, 0) };
        if ready > 0 {
            // SAFETY: `prepare_read` above succeeded, so this is the read it
            // reserved.
            unsafe { (self.wl.read_events)(self.display) };
        } else {
            // SAFETY: as above — cancels the reservation instead of using it.
            unsafe { (self.wl.cancel_read)(self.display) };
        }
        unsafe { (self.wl.dispatch_pending)(self.display) };
        self.report_display_error();
    }

    /// Say what killed the connection, once, in Cordial's own words.
    ///
    /// A session was lost to nothing but this, on a signed-in home page:
    ///
    /// ```text
    /// Gdk-Message: 14:10:43.968: Error 71 (Protocol error) dispatching to Wayland display.
    /// ```
    ///
    /// GDK prints that from `_gdk_wayland_display_queue_events` and then calls
    /// `_exit(1)`, so it is the last line there is. It names an errno and
    /// nothing else — not the interface, not the object, not the reason —
    /// and 71 is `EPROTO`, which means the *compositor* rejected something
    /// this client sent.
    ///
    /// The description of *what* it rejected is recovered elsewhere, by
    /// [`cordial_shell::host_window`]'s GDK-domain log handler; read that
    /// function's comment first, because it is the one that actually works.
    /// This is the second net, and a poor one: whichever side pulls the error
    /// off the socket dispatches it, and when GDK does, it exits before this
    /// ever runs. Measured with a deliberate bad `bind`, GDK won 3 times out of
    /// 3 — so **the absence of this line means nothing**. What it adds when it
    /// does win is the interface and protocol error code in Cordial's own
    /// voice, and coverage of the non-`EPROTO` case, where libwayland itself
    /// gave up on an event and GDK's line would report a meaningless errno.
    ///
    /// Deliberately not a panic. The display is already unusable and every
    /// later request is discarded, so there is nothing to salvage; the point is
    /// only that the next person gets the object and the code.
    fn report_display_error(&self) {
        // SAFETY: `self.display` is live for the process's lifetime, and both
        // calls are pure reads of state libwayland already recorded.
        let err = unsafe { (self.wl.get_error)(self.display) };
        if err == 0 {
            return;
        }
        if DISPLAY_ERROR_REPORTED.swap(true, Ordering::Relaxed) {
            return;
        }
        let mut interface: *const WlInterface = std::ptr::null();
        let mut id: u32 = 0;
        let code = unsafe { (self.wl.get_protocol_error)(self.display, &mut interface, &mut id) };
        // 71 is EPROTO: the compositor sent `wl_display.error`, and then
        // `interface`/`id`/`code` are populated. Any other errno means
        // libwayland itself gave up — a malformed or unknown event, most
        // likely an opcode past the end of one of the hand-written tables at
        // the top of this file — and they are not.
        let name = if interface.is_null() {
            "(none)".to_string()
        } else {
            // SAFETY: a `wl_interface` libwayland owns; `name` is a static C
            // string in whichever table declared it.
            unsafe { CStr::from_ptr((*interface).name) }.to_string_lossy().into_owned()
        };
        eprintln!(
            "[android] wayland: the display connection is dead (errno {err}); \
             compositor error on {name}#{id}, code {code}. \
             The compositor's own description of it is on stderr just above this line."
        );
    }
}

/// So that a dead connection is described once rather than on every pump tick
/// for however many ticks happen before the process goes.
static DISPLAY_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);

pub fn pump_input_events(handle: i64) {
    if let Some(w) = current() {
        w.pump(handle);
    }
}

// ------------------------------------------------------------- ANativeWindow_*
//
// Identical shape to `window.rs`'s X11 implementation — see its comments for
// the reasoning behind each of these; nothing here differs except which
// backend's singleton is read.

fn handle_ptr() -> *mut c_void {
    WINDOW.get().map_or(std::ptr::null_mut(), |w| w as *const WaylandWindow as *mut c_void)
}

fn as_window(p: *mut c_void) -> Option<&'static WaylandWindow> {
    (!p.is_null()).then(|| WINDOW.get()).flatten()
}

extern "C" fn native_window_from_surface(_env: *mut c_void, _surface: *mut c_void) -> *mut c_void {
    let w = handle_ptr();
    super::trace(format_args!("wayland: ANativeWindow_fromSurface -> {w:?}"));
    w
}
extern "C" fn native_window_acquire(_window: *mut c_void) {}
extern "C" fn native_window_release(_window: *mut c_void) {}
extern "C" fn native_window_get_width(window: *mut c_void) -> i32 {
    as_window(window).map_or(0, |w| w.geometry().0)
}
extern "C" fn native_window_get_height(window: *mut c_void) -> i32 {
    as_window(window).map_or(0, |w| w.geometry().1)
}
extern "C" fn native_window_get_format(window: *mut c_void) -> i32 {
    as_window(window).map_or(0, |w| w.geometry().2)
}
extern "C" fn native_window_set_buffers_geometry(window: *mut c_void, width: i32, height: i32, format: i32) -> i32 {
    let Some(w) = as_window(window) else { return -22 }; // -EINVAL
    let mut g = w.buffers.lock().unwrap_or_else(|e| e.into_inner());
    if width > 0 {
        g.width = width;
    }
    if height > 0 {
        g.height = height;
    }
    if format > 0 {
        g.format = format;
    }
    0
}
extern "C" fn native_window_lock(_window: *mut c_void, _buffer: *mut c_void, _dirty: *mut c_void) -> i32 {
    -38 // -ENOSYS — Roblox renders through GLES/Vulkan, never this path.
}
extern "C" fn native_window_unlock_and_post(_window: *mut c_void) -> i32 {
    -38
}

/// `eglCreateWindowSurface`, with the native window substituted for a real
/// `wl_egl_window*` — the Wayland equivalent of `window.rs`'s XID
/// substitution; see that function's doc for why the engine's own argument
/// is discarded rather than translated (there is exactly one window).
extern "C" fn egl_create_window_surface(
    dpy: *mut c_void,
    config: *mut c_void,
    _native_window: *mut c_void,
    attribs: *mut c_void,
) -> *mut c_void {
    crate::android::glcount::CREATE_WINDOW_SURFACE.fetch_add(1, Ordering::Relaxed);
    let name = c"eglCreateWindowSurface";
    // SAFETY: RTLD_DEFAULT; libEGL is in the global scope by the time the
    // engine reaches this call.
    let f = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
    if f.is_null() {
        return std::ptr::null_mut();
    }
    type Fn_ = extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    // SAFETY: resolved from the host for exactly this name.
    let f: Fn_ = unsafe { std::mem::transmute(f) };
    let Some(win) = current().and_then(|w| w.egl_window()) else {
        super::trace(format_args!("wayland: eglCreateWindowSurface asked for a wl_egl_window that could not be created"));
        return std::ptr::null_mut();
    };
    f(dpy, config, win, attribs)
}

/// `EGL_PLATFORM_WAYLAND_KHR`, from `EGL/eglext.h`.
const EGL_PLATFORM_WAYLAND_KHR: u32 = 0x31D8;

/// `eglGetDisplay`, redirected to `eglGetPlatformDisplay`/`...EXT` with
/// Cordial's own `wl_display` connection.
///
/// Roblox calls the plain, platform-agnostic `eglGetDisplay(EGL_DEFAULT_DISPLAY)`
/// — Android has no concept of Wayland, so there was never a reason for it to
/// call anything else. Left uninterposed, Mesa's own platform auto-detection
/// sees `$WAYLAND_DISPLAY` and calls `wl_display_connect(NULL)` *itself*,
/// opening a **second, independent connection** to the compositor. That is
/// silently wrong rather than loudly broken: Wayland object ids are scoped to
/// the connection that created them, so the `wl_buffer`s Mesa allocates on
/// its own connection could never be attached to the `wl_surface` this file
/// created on a different one. X11 has no equivalent hazard — resource ids
/// there are valid across any connection to the same server — which is
/// exactly the kind of Wayland-specific sharp edge ADR-011 means when it
/// calls the new backend "substantially more code... not... a second
/// supported configuration [for X11] worth keeping". Forcing the same
/// connection here is not an optimisation; without it, buffer attachment
/// would fail with a protocol error the first time the engine actually swaps.
extern "C" fn egl_get_display(native_display: *mut c_void) -> *mut c_void {
    let plain_get_display = || {
        let name = c"eglGetDisplay";
        // SAFETY: RTLD_DEFAULT; libEGL is in the global scope by this point.
        let f = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
        if f.is_null() {
            return std::ptr::null_mut();
        }
        type Fn_ = extern "C" fn(*mut c_void) -> *mut c_void;
        // SAFETY: resolved from the host for exactly this name.
        let f: Fn_ = unsafe { std::mem::transmute(f) };
        f(native_display)
    };

    let Some(w) = current() else {
        // No window yet to bind to — behave exactly as the unpatched call
        // would have.
        return plain_get_display();
    };

    for name in [c"eglGetPlatformDisplay", c"eglGetPlatformDisplayEXT"] {
        // SAFETY: as above.
        let f = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
        if f.is_null() {
            continue;
        }
        type Fn_ = extern "C" fn(u32, *mut c_void, *const c_void) -> *mut c_void;
        // SAFETY: resolved from the host for exactly this name.
        let f: Fn_ = unsafe { std::mem::transmute(f) };
        let d = f(EGL_PLATFORM_WAYLAND_KHR, w.wl_display(), std::ptr::null());
        if !d.is_null() {
            return d;
        }
    }
    super::trace(format_args!(
        "wayland: neither eglGetPlatformDisplay nor ...EXT resolved; falling back to eglGetDisplay, \
         buffer attachment will likely fail on swap"
    ));
    plain_get_display()
}

/// `eglSwapInterval`, forced to 0 — the same override the X11 backend applies,
/// for a worse reason.
///
/// With a non-zero interval Mesa's Wayland EGL will not return from
/// `eglSwapBuffers` until the compositor delivers a `wl_surface.frame`
/// callback. On X11 the equivalent wait was for a vblank source the host could
/// not supply and cost frame rate; here it costs everything, because the
/// callback is delivered on a Wayland event queue and a render thread blocked
/// inside `eglSwapBuffers` is not dispatching one. The first frame never
/// returns, no buffer is ever attached to the surface, and the compositor shows
/// a window with nothing in it — present in the dock and in alt-tab, blank on
/// screen, which is exactly what this looked like.
///
/// Forcing the interval Mesa actually receives to 0 makes `eglSwapBuffers`
/// return as soon as the frame is submitted. The engine still paces itself
/// through its own `RenderJob` timing, so this removes a broken throttle rather
/// than handing it a runaway framerate.
extern "C" fn egl_swap_interval(dpy: *mut c_void, _interval: c_int) -> u32 {
    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    let name = std::ffi::CString::new("eglSwapInterval").unwrap_or_default();
    // SAFETY: RTLD_DEFAULT; libEGL is in the global scope by the time the
    // engine reaches this call.
    let f = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
    if f.is_null() {
        return 0;
    }
    type Fn_ = extern "C" fn(*mut c_void, c_int) -> u32;
    // SAFETY: resolved from the host for exactly this name.
    let f: Fn_ = unsafe { std::mem::transmute(f) };
    f(dpy, 0)
}

pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("ANativeWindow_fromSurface", native_window_from_surface),
        f!("ANativeWindow_acquire", native_window_acquire),
        f!("ANativeWindow_release", native_window_release),
        f!("ANativeWindow_getWidth", native_window_get_width),
        f!("ANativeWindow_getHeight", native_window_get_height),
        f!("ANativeWindow_getFormat", native_window_get_format),
        f!("ANativeWindow_setBuffersGeometry", native_window_set_buffers_geometry),
        f!("ANativeWindow_lock", native_window_lock),
        f!("ANativeWindow_unlockAndPost", native_window_unlock_and_post),
        f!("eglCreateWindowSurface", egl_create_window_surface),
        f!("eglSwapInterval", egl_swap_interval),
        f!("eglGetDisplay", egl_get_display),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `app_id_matches_the_desktop_entry` test that lived here moved to
    // `cordial_shell::host_window`, which is what sets the app_id now that GTK
    // owns the toplevel. Same test, same ADR-009 reason; it has to sit beside
    // the constant that actually reaches the wire or it pins nothing.

    /// The listener array `wl_proxy_add_listener` is handed must have one
    /// function pointer per event the interface declares, and the interface
    /// must declare every event of the version bound. Getting that wrong is
    /// how `interface 'zwp_text_input_v3' has no event 8` happened: the table
    /// described version 1 while `bind` asked for version 2, and the fatal
    /// gap was three events wide.
    ///
    /// Only the hand-written interface can be checked here. `wl_pointer` and
    /// `wl_keyboard` come from the host's own `libwayland-client.so`, so their
    /// `event_count` is not a constant this crate can see at compile time —
    /// which is exactly why the module doc says the listeners for those must
    /// carry the *complete current* event set rather than the one matching
    /// some version this file picked.
    #[test]
    fn the_text_input_listener_has_a_slot_for_every_event_declared() {
        assert_eq!(TEXT_INPUT_EVENTS.len() as c_int, TEXT_INPUT_INTERFACE.event_count);
        assert_eq!(TEXT_INPUT_METHODS.len() as c_int, TEXT_INPUT_INTERFACE.method_count);
        assert_eq!(
            std::mem::size_of::<TextInputListener>(),
            TEXT_INPUT_EVENTS.len() * std::mem::size_of::<*const c_void>(),
            "TextInputListener must be exactly one function pointer per declared event"
        );
    }

    #[test]
    fn scrolling_down_reports_a_negative_vertical_notch() {
        // Wayland's positive is down; Android's AXIS_VSCROLL positive is away
        // from the user. A sign error here is not subtle to a person — the page
        // goes the wrong way — but it is invisible to a build, so it is pinned.
        let (h, v) = axis_to_notches(0, WHEEL_AXIS_STEP).expect("vertical axis is known");
        assert_eq!((h, v), (0.0, -1.0));
        let (h, v) = axis_to_notches(0, -WHEEL_AXIS_STEP).expect("vertical axis is known");
        assert_eq!((h, v), (0.0, 1.0));
        // Horizontal keeps Wayland's sign, because both call right positive.
        let (h, v) = axis_to_notches(1, WHEEL_AXIS_STEP).expect("horizontal axis is known");
        assert_eq!((h, v), (1.0, 0.0));
        // A third axis is not a thing `wl_pointer` has; inventing a meaning for
        // one would scroll on an event that said nothing about scrolling.
        assert!(axis_to_notches(2, WHEEL_AXIS_STEP).is_none());
    }

    /// A `zwp_text_input_v3` is created by the manager and therefore *is*
    /// whatever version the manager was bound at, whatever number this file
    /// passes to `wl_proxy_marshal_flags`. These two drifting apart is the
    /// whole bug, so they are pinned together rather than left as a comment.
    #[test]
    fn the_text_input_and_its_manager_declare_the_same_version() {
        assert_eq!(TEXT_INPUT_INTERFACE.version, TEXT_INPUT_MANAGER_INTERFACE.version);
        // Version 2 is what GNOME 50's mutter advertises, measured on the wire:
        // `wl_registry#107.global(26, "zwp_text_input_manager_v3", 2)`. Raising
        // this means adding the new version's events to the table above first.
        assert_eq!(TEXT_INPUT_INTERFACE.version, 2);
    }

    #[test]
    fn no_preedit_leaves_committed_text_untouched() {
        let (text, caret) = splice_preedit("hello", 5, None);
        assert_eq!(text, "hello");
        assert_eq!(caret, 5);
    }

    #[test]
    fn preedit_is_spliced_at_the_caret_not_appended() {
        // The caret is mid-string (after "he"), not at the end — a splice
        // that always appended at the end would be indistinguishable from a
        // commit in this test and would miss the actual bug class this
        // guards: composing in the middle of existing text.
        let (text, caret) = splice_preedit("hello", 2, Some(&("XX".to_string(), 2, 2)));
        assert_eq!(text, "heXXllo");
        // Two committed chars before the caret, plus both preedit chars
        // (cursor_begin=2 is the end of a 2-char preedit).
        assert_eq!(caret, 4);
    }

    #[test]
    fn preedit_cursor_can_land_inside_the_composing_text() {
        // A predictive engine can put its cursor partway through what it is
        // suggesting, not only at the end — e.g. showing "ing" appended to a
        // stem with the cursor still after the stem.
        let (text, caret) = splice_preedit("run", 3, Some(&("ning".to_string(), 0, 0)));
        assert_eq!(text, "running");
        // cursor_begin=0 means the preedit's own cursor is at its start, so
        // the displayed caret stays at the committed caret (3), not
        // advanced into "ning".
        assert_eq!(caret, 3);
    }

    #[test]
    fn preedit_replaces_rather_than_appends_to_the_previous_one() {
        // "A new preedit_string replaces the previous one entirely" — the
        // module doc's own rule. This is really documenting `ImeState`'s
        // assignment (`ime.pending.preedit = Some(...)`, not a push/append),
        // but `splice_preedit` only ever sees the current value, so an
        // out-of-date caller passing a stale preedit is exactly the bug this
        // would catch if `apply_ime_done` ever accumulated instead of
        // replacing.
        let after_first = splice_preedit("x", 1, Some(&("a".to_string(), 1, 1)));
        assert_eq!(after_first.0, "xa");
        let after_second = splice_preedit("x", 1, Some(&("ab".to_string(), 2, 2)));
        assert_eq!(after_second.0, "xab");
    }

    #[test]
    fn an_empty_preedit_still_splices_as_a_real_value() {
        // "An empty `preedit_string` clears composition; that is a real
        // event, not a no-op" — `Some(("", ..))` must behave identically to
        // `None` for display purposes (nothing to splice in), which this
        // checks explicitly rather than trusting an empty string to fall out
        // of the general case correctly.
        let (text, caret) = splice_preedit("hi", 2, Some(&(String::new(), 0, 0)));
        assert_eq!(text, "hi");
        assert_eq!(caret, 2);
    }

    #[test]
    fn preedit_splicing_counts_the_committed_caret_in_characters() {
        // The committed side of the splice takes a char index (that is what
        // `text_buffer_snapshot` reports), so a multi-byte character before
        // the caret must not shift where the preedit lands.
        let (text, caret) = splice_preedit("héllo", 2, Some(&("X".to_string(), 0, 0)));
        assert_eq!(text, "héXllo");
        assert_eq!(caret, 2);
    }
}
