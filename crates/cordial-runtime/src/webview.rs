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
//! **What this module still cannot do: call `getMessageId`.** Its descriptor is
//! `(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;` — two `String`s in,
//! one back out — and no call shape `cordial-linker-sys` exports fits that.
//! `call_static_string_ret_string` takes one `String` in and returns one;
//! `call_static_strings` takes up to three `String`s in but returns nothing,
//! because every native it was written for (`nativeSetFilesDirectory` and
//! friends) is `void`. Composing the bus id needs a wrapper that does not
//! exist, and this change is not permitted to add one to `cordial-linker-sys`
//! or to the C++ side that would back it. Writing a raw JNI call by hand here
//! instead would be exactly the machinery-rebuilding AGENTS.md warns is this
//! project's most expensive mistake, done in a new place rather than not done.
//! So [`find_bus_natives`] resolves and reports the two natives `openWindow`
//! needs without calling either — a diagnostic, same as [`read_vocabulary`]
//! below, and the blocker the next piece of work has to clear.

use std::ffi::c_void;

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

/// Print what [`find_bus_natives`] found, and say plainly why that is where
/// this stops rather than leaving the silence AGENTS.md warns is worse than an
/// honest "not yet".
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
        println!(
            "  webview: both natives are present, but composing the openWindow bus id needs a \
             two-String-argument, String-returning call, and cordial-linker-sys has no such \
             wrapper (see module doc) — not subscribing rather than guessing the id"
        );
    }
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
}
