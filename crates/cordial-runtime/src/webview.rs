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
//! twenty-three natives, their signatures, and that Cordial answers none of it.
//!
//! **Not established: how the engine delivers a message once the protocol is
//! initialised.** `WebViewProtocol`'s non-native methods are obfuscated down to
//! single letters — `a`, `b`, `c` through `k` — and take `org.json.JSONObject`,
//! so there is a JSON message bus in between whose shape has not been traced.
//! Until that is known there is no honest way to write the receiving half, and
//! guessing at it would produce exactly the stub that reports success and does
//! nothing which AGENTS.md forbids.
//!
//! So this module does the half that can be done truthfully: it reads the
//! vocabulary and reports it. That is a diagnostic, it changes no engine state
//! beyond registering the app as the provider, and the log it prints is the
//! input the next piece of work needs.

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
}
