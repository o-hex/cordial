//! Roblox's in-experience web window, and the protocol the engine drives it with.
//!
//! This is what opens when somebody taps account settings, buys Robux, or
//! follows any other link the client keeps inside itself rather than handing to
//! a browser. With nobody answering, those buttons do nothing at all — no error,
//! no window, no log line. That is the `broken_feature` shape from AGENTS.md and
//! it is the largest one Cordial has.
//!
//! ## The protocol is the engine's, and it describes itself
//!
//! `libroblox.so` exports twenty-three natives on
//! `com/roblox/protocols/webview/WebViewProtocol`. Between them they carry the
//! whole vocabulary: the protocol's name, the identifiers of the messages the
//! engine will send, and the JSON keys inside those messages.
//!
//! **Every one of them is a getter, and that is the point.** The message the
//! engine sends to open a window carries a URL under some key, and that key is
//! whatever `getUrlKey()` returns *in the build being run*. Hardcoding `"url"`
//! because that is what it is probably called produces something that works
//! until Roblox renames it, and then fails silently — the window opens with no
//! address and nobody can tell why. Reading the vocabulary out of the engine
//! cannot drift, because it is the same constant the engine matches against.
//!
//! The machinery to do that already existed. `JNILinkingProtocol` — deep links —
//! is the same shape and `native/deeplink.cpp` already has a generic "static,
//! zero-argument native returning String" shim, parameterised by class name. So
//! this module is a caller of existing code rather than new plumbing, which is
//! also why it is small.
//!
//! ## What is established and what is not
//!
//! Established, by reading the engine's exports and the dex: the class, the
//! twenty-three natives, their signatures, and — corrected here, because an
//! earlier version of this comment said the transport was untraced and that is
//! no longer true — the transport itself. `docs/analysis/webview-surface.md`
//! §"The transport, found in mocktail's bridge rather than by tracing" names it:
//! `com.roblox.universalapp.messagebus.MessageBus`, the same bus
//! `native/deeplink.cpp` already speaks for deep links. Receiving `openWindow`
//! is three calls — `getMessageId("WebView", "openWindow")` to compose the bus
//! id, `doSubscribeRaw(id, callback, false)` to subscribe, and the callback's
//! `run(String)` handed the raw JSON when the engine publishes. The callback and
//! `Connection` classes that shape needs are already registered, by
//! `native/clipboard.cpp`'s `register_clipboard_classes` — this module must not
//! register them a second time, because a second registration of the same class
//! name overwrites the first and silently breaks the clipboard subscription
//! that got there first.
//!
//! **The two blockers that used to stop here are both cleared.** The first was
//! that no call shape `cordial-linker-sys` exported fit `getMessageId`'s
//! descriptor, `(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;` — two
//! `String`s in, one back out. `cordial_linker_sys::game_activity::
//! call_static_two_strings_ret_string` now exists for exactly that shape. The
//! second was that `native/clipboard.cpp`'s subscribe machinery held one
//! callback and one `Connection` in module-level globals, so a second
//! subscriber would not add a subscription, it would silently overwrite
//! clipboard's. That file's `cordial_messagebus_subscribe` is now keyed by
//! message id, in a map, so [`arm`] below and clipboard's own `arm` in
//! `crate::android::clipboard` each get a `Subscription` of their own. Neither
//! of those changes is this module's to make — this module is the first
//! caller of both, not the place either was fixed.
//!
//! [`find_bus_natives`] and [`report_bus_natives`] stay as an early
//! diagnostic: they run from `load.rs` before `nativeRetryInit`, while there is
//! not yet an app bridge for anything to subscribe against. [`arm`] does the
//! real work — `getMessageId`, then `doSubscribeRaw`, then a first read of
//! `Connection.isConnected` — and is called later, once the AGDK handle
//! exists, the same distinction clipboard's own `arm` draws against
//! `looper::pump`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;

/// The class the whole protocol hangs off.
pub const CLASS: &str = "com/roblox/protocols/webview/WebViewProtocol";

/// The getters worth reading, by the native symbol that answers each.
///
/// Not every export is here: the two `getFFlag...` natives return `boolean`
/// rather than `String` and need a different call shape, and
/// `signalJavascriptCallback` takes arguments and is not a getter at all. Those
/// are left for when there is something to say with them, rather than called to
/// make a list look complete.
const STRING_GETTERS: &[(&str, &str)] = &[
    ("protocol", "getProtocolName"),
    // Message identifiers: what the engine will ask for.
    ("open-window", "getOpenWindowId"),
    ("close-window", "getCloseWindowId"),
    ("mutate-window", "getMutateWindowId"),
    ("handle-window-close", "getHandleWindowCloseId"),
    ("is-available", "getIsAvailableId"),
    // Payload keys: where to find things inside a message.
    ("key:url", "getUrlKey"),
    ("key:title", "getTitleKey"),
    ("key:window-type", "getWindowTypeKey"),
    ("key:search-params", "getSearchParamsKey"),
    ("key:search-type", "getSearchTypeKey"),
    ("key:is-visible", "getIsVisibleKey"),
    ("key:hide-header", "getHideHeaderKey"),
    ("key:back-button-visible", "getBackButtonVisibleKey"),
    ("key:show-domain-as-title", "getShowDomainAsTitleKey"),
    ("key:available", "getAvailableKey"),
];

/// A getter's answer, or why it could not be had.
pub struct Vocabulary {
    pub entries: Vec<(&'static str, String)>,
    pub missing: Vec<&'static str>,
}

/// Read the protocol's vocabulary out of the running engine.
///
/// `symbol` resolves a JNI native by name — the same lookup `load.rs` already
/// does for every other engine entry point. A getter that is not exported is
/// recorded rather than treated as fatal: Roblox adds and removes these between
/// builds, and one missing name is a fact worth printing, not a reason to
/// abandon the other fifteen.
pub fn read_vocabulary(mut symbol: impl FnMut(&str) -> Option<*mut c_void>) -> Vocabulary {
    let mut entries = Vec::new();
    let mut missing = Vec::new();
    for (label, getter) in STRING_GETTERS {
        let name = format!("Java_com_roblox_protocols_webview_WebViewProtocol_{getter}");
        match symbol(&name) {
            Some(f) => match cordial_linker_sys::game_activity::call_static_ret_string(f, CLASS) {
                Ok(v) => entries.push((*label, v)),
                // A getter that is exported and then fails is a different fact
                // from one that is absent, and conflating them would hide it.
                Err(e) => entries.push((*label, format!("<error: {e}>"))),
            },
            None => missing.push(*getter),
        }
    }
    Vocabulary { entries, missing }
}

/// Print what was read.
///
/// Verbose on purpose. This is the only record of what the protocol's names are
/// in a given build, and the next piece of work — receiving a message — cannot
/// start without it.
pub fn report(v: &Vocabulary) {
    if v.entries.is_empty() && v.missing.is_empty() {
        println!("  webview: WebViewProtocol is not exported by this build");
        return;
    }
    println!("  webview: protocol vocabulary, read from the engine");
    for (label, value) in &v.entries {
        println!("    {label:<26} {value}");
    }
    if !v.missing.is_empty() {
        println!("    not exported by this build: {}", v.missing.join(", "));
    }
}

/// The two `com/roblox/universalapp/messagebus/MessageBus` natives that
/// receiving `openWindow` needs, found or not, by the same symbol lookup
/// [`read_vocabulary`] already uses.
pub struct BusNatives {
    pub get_message_id: bool,
    pub do_subscribe_raw: bool,
}

/// Resolve `MessageBus.getMessageId` and `MessageBus.doSubscribeRaw`.
///
/// Finding a native and being able to call it are different facts —
/// `getMessageId` is found by every build that has ever been run against this
/// module, and it is still not called. See the module doc for why.
pub fn find_bus_natives(mut symbol: impl FnMut(&str) -> Option<*mut c_void>) -> BusNatives {
    BusNatives {
        get_message_id: symbol("Java_com_roblox_universalapp_messagebus_MessageBus_getMessageId")
            .is_some(),
        do_subscribe_raw: symbol(
            "Java_com_roblox_universalapp_messagebus_MessageBus_doSubscribeRaw",
        )
        .is_some(),
    }
}

/// Print what [`find_bus_natives`] found. This runs before `nativeRetryInit`,
/// while the app bridge that `doSubscribeRaw` needs to call into does not
/// exist yet — so finding both natives here is a fact worth logging, not
/// grounds to subscribe from this spot. [`arm`] is the call that actually
/// subscribes, made later once there is a bridge to subscribe against.
pub fn report_bus_natives(n: &BusNatives) {
    println!(
        "  webview: MessageBus.getMessageId exported: {}",
        n.get_message_id
    );
    println!(
        "  webview: MessageBus.doSubscribeRaw exported: {}",
        n.do_subscribe_raw
    );
    if n.get_message_id && n.do_subscribe_raw {
        println!("  webview: both natives are present; openWindow is subscribed later, from arm()");
    }
}

// -------------------------------------------------------------- subscribing
//
// `native/clipboard.cpp` carries the C++ half of this: `RawCallback` and
// `Connection` are classes it registers once for the whole process (never
// again here — see that file's header for the second-registration bug that
// already happened once), and `cordial_messagebus_subscribe` keeps one
// `Subscription` per message id in a map, so this subscription and
// clipboard's own live independently. Declared again here, rather than made
// `pub` in `crate::android::clipboard` and imported, because that file is off
// limits to edit for this change and an `extern "C"` block naming symbols the
// linker already provides is not a second definition of anything — both
// modules are just callers of the same three C++ functions.
unsafe extern "C" {
    fn cordial_messagebus_subscribe(
        f: *mut c_void,
        message_id: *const c_char,
        sink: Option<extern "C" fn(*const c_char)>,
        err: *mut c_char,
        n: usize,
    ) -> c_int;
    fn cordial_messagebus_connection_ptr(message_id: *const c_char) -> i64;
    fn cordial_messagebus_is_connected(
        f: *mut c_void,
        ptr: i64,
        out_connected: *mut c_int,
        err: *mut c_char,
        n: usize,
    ) -> c_int;
}

fn take_err(err: Vec<u8>) -> String {
    let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
    String::from_utf8_lossy(&err[..end]).into_owned()
}

static CONNECTION: AtomicI64 = AtomicI64::new(0);
static IS_CONNECTED_NATIVE: OnceLock<usize> = OnceLock::new();

/// The sink `cordial_messagebus_subscribe` calls when the engine publishes
/// `openWindow`.
///
/// Runs on whichever thread the engine published from, same as clipboard's
/// `on_payload`. Unlike clipboard, there is nothing here that needs the GTK
/// thread — this prints and returns, it does not touch a window or a
/// clipboard — so there is no pending slot to park into and no pump to drain
/// it.
///
/// **The JSON is measured, never read.** Scope for this change is receiving
/// the message, not acting on it — opening a window is `cordial-shell`'s, out
/// of reach here — and the payload may carry a URL with a session token in
/// it, so only its size is fit to print. `text_from_payload` in
/// `android::clipboard` is the pattern for a module that does need the
/// content: it exists, and this module deliberately does not call anything
/// like it yet.
extern "C" fn on_open_window(json: *const c_char) {
    if json.is_null() {
        println!("[webview] openWindow published with a null payload");
        return;
    }
    // SAFETY: the C side passes a NUL-terminated string that outlives the
    // call. Measured as raw bytes rather than decoded to `str`: a byte count
    // is all this prints, so there is no reason to reject a payload over a
    // UTF-8 question this module has no use for the answer to.
    let json = unsafe { CStr::from_ptr(json) };
    println!("[webview] openWindow message arrived: {} bytes", json.to_bytes().len());
}

/// Subscribe to the engine's `openWindow` message.
///
/// Three calls, each depending on the last having worked: `getMessageId`
/// composes the bus id the way the engine itself would look it up rather than
/// guessing at a constant that changes between builds (see the module doc on
/// [`read_vocabulary`]); `doSubscribeRaw` installs [`on_open_window`] through
/// the callback class `native/clipboard.cpp`'s `register_clipboard_classes`
/// already registered; `Connection.isConnected` is the only honest way to say
/// the subscription is live rather than merely attempted without throwing.
///
/// `symbol` is the same JNI-native lookup `load.rs` already threads through
/// [`read_vocabulary`] and [`find_bus_natives`]. Call this once the AGDK
/// handle exists and the app bridge has started — the same point
/// `android::clipboard::arm` is called from, for the same reason: the bus has
/// to exist before anything can subscribe to it, and this module cannot reach
/// `looper::pump` to add itself there, so `load.rs` calls this directly,
/// right before it hands control to that pump.
pub fn arm(mut symbol: impl FnMut(&str) -> Option<*mut c_void>) {
    let vocabulary = read_vocabulary(&mut symbol);
    let get = |label: &str| {
        vocabulary
            .entries
            .iter()
            .find(|(l, _)| *l == label)
            .map(|(_, v)| v.clone())
    };
    let (Some(protocol), Some(open_window_id)) = (get("protocol"), get("open-window")) else {
        println!(
            "  webview: cannot subscribe to openWindow without getProtocolName and \
             getOpenWindowId (see the vocabulary reported above)"
        );
        return;
    };

    let Some(get_message_id) =
        symbol("Java_com_roblox_universalapp_messagebus_MessageBus_getMessageId")
    else {
        println!("  webview: MessageBus.getMessageId is not exported; not subscribing");
        return;
    };
    let bus_id = match cordial_linker_sys::game_activity::call_static_two_strings_ret_string(
        get_message_id,
        "com/roblox/universalapp/messagebus/MessageBus",
        &protocol,
        &open_window_id,
    ) {
        Ok(id) => id,
        Err(e) => {
            println!("  webview: getMessageId({protocol:?}, {open_window_id:?}) failed: {e}");
            return;
        }
    };

    let Some(do_subscribe_raw) =
        symbol("Java_com_roblox_universalapp_messagebus_MessageBus_doSubscribeRaw")
    else {
        println!(
            "  webview: MessageBus.doSubscribeRaw is not exported; openWindow will do nothing"
        );
        return;
    };
    let Ok(id) = CString::new(bus_id.clone()) else {
        println!("  webview: the bus id getMessageId returned has a NUL in it: {bus_id:?}");
        return;
    };
    let mut err = vec![0u8; 512];
    // SAFETY: `do_subscribe_raw` resolved under its own name, so it is the
    // native `cordial_messagebus_subscribe` expects; every buffer outlives
    // the call.
    let rc = unsafe {
        cordial_messagebus_subscribe(
            do_subscribe_raw,
            id.as_ptr(),
            Some(on_open_window),
            err.as_mut_ptr() as *mut c_char,
            err.len(),
        )
    };
    if rc != 0 {
        println!("  webview: doSubscribeRaw failed: {}", take_err(err));
        return;
    }
    // SAFETY: reads a value the subscribe call above stored, keyed by the
    // same message id.
    let ptr = unsafe { cordial_messagebus_connection_ptr(id.as_ptr()) };
    CONNECTION.store(ptr, Ordering::Relaxed);
    if let Some(f) = symbol("Java_com_roblox_universalapp_messagebus_Connection_isConnected") {
        let _ = IS_CONNECTED_NATIVE.set(f as usize);
    }
    match connected() {
        Some(true) => println!(
            "  webview: subscribed to {bus_id} ({protocol}.{open_window_id}); the bus says it \
             is live"
        ),
        Some(false) => println!(
            "  webview: subscribed to {bus_id}, but the bus says the connection is not live"
        ),
        None => println!(
            "  webview: subscribed to {bus_id}; nothing here can confirm it (no Connection came \
             back)"
        ),
    }
}

/// What `Connection.isConnected` says about the subscription, or `None` when
/// there is nothing to ask about. Mirrors `android::clipboard::connected`.
pub fn connected() -> Option<bool> {
    let ptr = CONNECTION.load(Ordering::Relaxed);
    let f = *IS_CONNECTED_NATIVE.get()? as *mut c_void;
    if ptr == 0 {
        return None;
    }
    let mut out: c_int = -1;
    let mut err = vec![0u8; 256];
    // SAFETY: the native resolved under its own name; the buffers outlive the call.
    let rc = unsafe {
        cordial_messagebus_is_connected(
            f,
            ptr,
            &mut out as *mut c_int,
            err.as_mut_ptr() as *mut c_char,
            err.len(),
        )
    };
    (rc == 0).then_some(out != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The symbol names are built by concatenation, and a typo in the prefix
    /// would make every lookup miss at once and read as "the build does not
    /// export this" rather than as a bug here.
    #[test]
    fn getter_symbols_are_spelled_the_way_the_engine_exports_them() {
        let mut asked = Vec::new();
        let _ = read_vocabulary(|name| {
            asked.push(name.to_string());
            None
        });
        assert_eq!(asked.len(), STRING_GETTERS.len());
        assert!(asked.contains(
            &"Java_com_roblox_protocols_webview_WebViewProtocol_getUrlKey".to_string()
        ));
        assert!(asked.contains(
            &"Java_com_roblox_protocols_webview_WebViewProtocol_getOpenWindowId".to_string()
        ));
        assert!(asked.iter().all(|n| n
            .starts_with("Java_com_roblox_protocols_webview_WebViewProtocol_")));
    }

    /// An absent getter must be reported as absent rather than as an empty
    /// answer: "this build has no `getHideHeaderKey`" and "`getHideHeaderKey`
    /// returned an empty string" would need different work, and a caller that
    /// cannot tell them apart will do the wrong one.
    #[test]
    fn a_missing_getter_is_recorded_rather_than_silently_skipped() {
        let v = read_vocabulary(|_| None);
        assert!(v.entries.is_empty());
        assert_eq!(v.missing.len(), STRING_GETTERS.len());
    }

    /// Same spelling discipline as the vocabulary getters: a typo in either
    /// symbol name would read as "this build does not export MessageBus" and
    /// send the next agent looking at the wrong build rather than at this file.
    #[test]
    fn bus_native_symbols_are_spelled_the_way_the_engine_exports_them() {
        let mut asked = Vec::new();
        let _ = find_bus_natives(|name| {
            asked.push(name.to_string());
            None
        });
        assert_eq!(asked.len(), 2);
        assert!(asked.contains(
            &"Java_com_roblox_universalapp_messagebus_MessageBus_getMessageId".to_string()
        ));
        assert!(asked.contains(
            &"Java_com_roblox_universalapp_messagebus_MessageBus_doSubscribeRaw".to_string()
        ));
    }

    /// Neither native being found must read as neither found, not as a partial
    /// or default-true answer that would let `report_bus_natives` claim
    /// something it never checked.
    #[test]
    fn missing_bus_natives_are_reported_as_missing() {
        let n = find_bus_natives(|_| None);
        assert!(!n.get_message_id);
        assert!(!n.do_subscribe_raw);
    }

    /// `arm` must bail out before touching any of the `extern "C"` functions
    /// when the vocabulary getters are not exported — a build with none of
    /// this protocol must not crash trying to subscribe to it. Nothing here
    /// should ever reach `cordial_messagebus_subscribe`, so the only failure
    /// mode this rules out is a panic or a call into unresolved FFI.
    ///
    /// This is as far as `arm` can be exercised without a running engine:
    /// every path past this point calls into `cordial-linker-sys` with a
    /// function pointer the callee dereferences, and a test has no real JNI
    /// environment to hand it one that would not crash. The rest of `arm` —
    /// `getMessageId` composing a bus id, `doSubscribeRaw` installing the
    /// callback, `Connection.isConnected` reporting live — is established by
    /// running the client and reading what it printed, not by a unit test.
    #[test]
    fn arm_does_nothing_when_the_vocabulary_is_not_exported() {
        arm(|_| None);
        assert!(connected().is_none(), "a bare arm() with nothing exported must not connect");
    }
}
