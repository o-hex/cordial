//! `PermissionsProtocol` -- the engine asking the host whether it may use the
//! microphone.
//!
//! Voice could not ask for the mic. On Android the engine does not call
//! `checkSelfPermission` for this (that is answered, and granted, in
//! `android_classes.cpp`, and it changed nothing); it sends a message-bus
//! request on protocol `PermissionsProtocol` and waits for Roblox's Java to
//! answer it. Cordial replaces that Java, bound nothing, and the request went
//! unanswered. `RBX::PermissionsProtocolCore::requestPermissions` and
//! `hasPermissions` in the engine's symbol table, and the three method names
//! next to the protocol name in its string pool, are the evidence for the shape.
//!
//! The vocabulary is read from string tables, not from any implementation:
//! the engine carries `PermissionsProtocol`, `PermissionsRequest`,
//! `HasPermissions`, `SupportsPermissions`, `MICROPHONE_ACCESS`, `permissions`
//! and `status`; the dex carries the same names plus `AUTHORIZED`, `DENIED`
//! and `missingPermissions`. **How those keys are arranged in the request and
//! the response is INFERRED** -- `{"permissions": [...]}` in, and
//! `{"status": ..., "missingPermissions": [...]}` out. Every request prints the
//! permission names it carried and the answer given, so a wrong guess shows up
//! beside the engine's own `PermissionsProtocolCore: Invalid response
//! received.` rather than as silence.
//!
//! Granting the permission does not open the microphone. The rule in
//! `native/audio_classes.cpp` stands: no capture stream exists until Roblox
//! actually starts recording.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

extern "C" {
    fn cordial_messagebus_set_request_handler_async(
        set_fn: *mut c_void,
        respond_fn: *mut c_void,
        protocol: *const c_char,
        method: *const c_char,
        sink: extern "C" fn(*const c_char, *mut c_char, usize) -> c_int,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
}

const PROTOCOL: &str = "PermissionsProtocol";

/// The only permission Cordial can truthfully grant. Anything else -- camera,
/// contacts, local network -- has no implementation behind it, so saying yes
/// would be a stub that lies.
const MICROPHONE: &str = "MICROPHONE_ACCESS";

extern "C" fn on_request(request: *const c_char, out: *mut c_char, out_len: usize) -> c_int {
    reply("PermissionsRequest", request, out, out_len)
}

extern "C" fn on_has(request: *const c_char, out: *mut c_char, out_len: usize) -> c_int {
    reply("HasPermissions", request, out, out_len)
}

extern "C" fn on_supports(request: *const c_char, out: *mut c_char, out_len: usize) -> c_int {
    reply("SupportsPermissions", request, out, out_len)
}

fn reply(method: &str, request: *const c_char, out: *mut c_char, out_len: usize) -> c_int {
    if request.is_null() || out.is_null() || out_len == 0 {
        return 0;
    }
    // SAFETY: the bus hands over a NUL-terminated string it owns for the call.
    let raw = unsafe { CStr::from_ptr(request) }.to_string_lossy().into_owned();
    let body = answer(method, &raw);
    let Ok(c) = CString::new(body) else { return 0 };
    let bytes = c.as_bytes_with_nul();
    if bytes.len() > out_len {
        return 0;
    }
    // SAFETY: length checked against the caller's buffer immediately above.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, bytes.len()) };
    1
}

/// Build the response for one request, and say what was asked and answered.
///
/// Printing is safe here in a way it is not for `linking`: a permissions
/// request carries permission names and nothing a user typed.
fn answer(method: &str, raw: &str) -> String {
    let value = serde_json::from_str::<serde_json::Value>(raw).unwrap_or(serde_json::Value::Null);
    let asked: Vec<String> = value
        .get("permissions")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    if asked.is_empty() {
        let keys: Vec<&str> = value
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        println!("  permissions: {method} carried no permission list (keys {keys:?})");
    }
    let missing: Vec<&str> = asked.iter().map(String::as_str).filter(|p| *p != MICROPHONE).collect();
    let status = if missing.is_empty() { "AUTHORIZED" } else { "DENIED" };
    println!("  permissions: {method} {asked:?} -> {status}");
    serde_json::json!({ "status": status, "missingPermissions": missing }).to_string()
}

/// Bind the three methods. `symbol` resolves a name in the loaded engine.
///
/// Not fatal on failure, like `linking::arm`: a client without voice is still
/// worth launching.
pub fn arm(symbol: impl Fn(&str) -> Option<*mut c_void>) {
    let set = symbol("Java_com_roblox_universalapp_messagebus_MessageBus_setRequestHandlerAsyncRaw");
    let respond = symbol("Java_com_roblox_universalapp_messagebus_MessageBus_callResponseHandlerRaw");
    let (Some(set), Some(respond)) = (set, respond) else {
        println!("  permissions: async request natives are not exported; the microphone cannot be granted");
        return;
    };
    let protocol = CString::new(PROTOCOL).expect("literal");
    let methods: [(&str, extern "C" fn(*const c_char, *mut c_char, usize) -> c_int); 3] =
        [("PermissionsRequest", on_request), ("HasPermissions", on_has), ("SupportsPermissions", on_supports)];
    for (method, sink) in methods {
        let m = CString::new(method).expect("literal");
        let mut err = vec![0u8; 512];
        // SAFETY: both strings outlive the call; `err` is only written into.
        let rc = unsafe {
            cordial_messagebus_set_request_handler_async(
                set,
                respond,
                protocol.as_ptr(),
                m.as_ptr(),
                sink,
                err.as_mut_ptr() as *mut c_char,
                err.len(),
            )
        };
        if rc == 0 {
            println!("  permissions: {PROTOCOL}.{method} handler bound");
        } else {
            let msg = String::from_utf8_lossy(&err);
            println!("  permissions: could not bind {PROTOCOL}.{method}: {}", msg.trim_end_matches('\0'));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(json: &str) -> (String, Vec<String>) {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let missing = v["missingPermissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect();
        (v["status"].as_str().unwrap().to_owned(), missing)
    }

    #[test]
    fn the_microphone_is_granted() {
        let (s, missing) = status(&answer("PermissionsRequest", r#"{"permissions":["MICROPHONE_ACCESS"]}"#));
        assert_eq!(s, "AUTHORIZED");
        assert!(missing.is_empty());
    }

    #[test]
    fn a_permission_with_nothing_behind_it_is_not_granted() {
        let (s, missing) =
            status(&answer("PermissionsRequest", r#"{"permissions":["MICROPHONE_ACCESS","CAMERA_ACCESS"]}"#));
        assert_eq!(s, "DENIED");
        assert_eq!(missing, vec!["CAMERA_ACCESS"]);
    }

    #[test]
    fn a_malformed_request_still_gets_a_well_formed_answer() {
        let (s, _) = status(&answer("HasPermissions", "not json"));
        assert_eq!(s, "AUTHORIZED");
    }
}
