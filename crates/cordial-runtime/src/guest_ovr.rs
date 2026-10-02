//! The arm64 guest's `libovrplatformloader.so`: Meta's Platform SDK, answered
//! as a host where Meta platform services do not exist (docs/vr/dynarmic-design.md
//! §3.2).
//!
//! The Quest engine reaches the SDK once `initMaquettesSDK` runs: it starts
//! `ovr_PlatformInitializeAndroidAsynchronous` and pumps `ovr_PopMessage`.
//! Every call here fails, and fails the way the SDK's public reference says a
//! failure arrives: a request function returns a request id and puts a message
//! of its own `ovrMessageType` on the queue with `ovr_Message_IsError` true and
//! an `ovrError` saying the services are absent. `ovr_PopMessage` returns null
//! when the queue is empty, and `ovr_GetLoggedInUserID` returns 0, "if there is
//! none". Nothing here ever reports a successful entitlement check, user proof,
//! integrity token or purchase: those requests are queued as errors like every
//! other, and a payload accessor on an error message returns null, so there is
//! no success shape for the engine to read even by accident.
//!
//! The message type values are the public `OVR_MessageType.h` ones. The error
//! code is Cordial's own: the SDK documents `ovr_Error_GetCode` but no fixed
//! code for "no platform", so a value is chosen that no real server response
//! would produce and said so here rather than borrowed from a meaning it lacks.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::{c_char, CStr, CString};
use std::sync::Mutex;

use cordial_guest::Handler;

const MSG_PLATFORM_INITIALIZE_ANDROID_ASYNCHRONOUS: u32 = 0x1AD3_07B4;
const MSG_ENTITLEMENT_GET_IS_VIEWER_ENTITLED: u32 = 0x186B_58B1;
const MSG_USER_GET_USER_PROOF: u32 = 0x2281_0483;
const MSG_IAP_GET_VIEWER_PURCHASES: u32 = 0x3A0F_8419;
const MSG_IAP_LAUNCH_CHECKOUT_FLOW: u32 = 0x3F9B_0D0D;
const MSG_ABUSE_REPORT_REPORT_REQUEST_HANDLED: u32 = 0x4B8E_FC86;
const MSG_USER_AGE_CATEGORY_REPORT: u32 = 0x2E4D_D8D6;
const MSG_DEVICE_APPLICATION_INTEGRITY_GET_INTEGRITY_TOKEN: u32 = 0x3271_ABDA;

/// Cordial's code for "there is no Meta platform here"; see the module comment.
pub const ERROR_NO_PLATFORM: i32 = -1;
const ERROR_TEXT: &str = "Meta platform services are not available on this host (Cordial)";

struct Message {
    ty: u32,
    /// What `ovr_Message_GetRequestID` would return; this build does not
    /// import it, so only the tests read it.
    #[cfg_attr(not(test), allow(dead_code))]
    request: u64,
    error: ErrorRec,
}

struct ErrorRec {
    code: i32,
    text: CString,
}

struct Queue {
    next_request: u64,
    pending: VecDeque<usize>,
    /// Every message handed out and not yet freed, so a stray handle is
    /// refused instead of dereferenced.
    live: BTreeSet<usize>,
    errors: BTreeSet<usize>,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    next_request: 1,
    pending: VecDeque::new(),
    live: BTreeSet::new(),
    errors: BTreeSet::new(),
});

/// The queue is process-wide, so tests that look at it take turns.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

static SEEN: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

fn once(name: &'static str, detail: &str) {
    if SEEN.lock().unwrap().insert(name) {
        eprintln!("[ovr] {name} called{detail}; answered as a host without Meta platform services");
    }
}

/// Queues a failed result of type `ty` and returns its request id.
fn fail_request(ty: u32) -> u64 {
    let mut q = QUEUE.lock().unwrap();
    let request = q.next_request;
    q.next_request += 1;
    let m = Box::new(Message {
        ty,
        request,
        error: ErrorRec { code: ERROR_NO_PLATFORM, text: CString::new(ERROR_TEXT).unwrap() },
    });
    let err = &m.error as *const ErrorRec as usize;
    let p = Box::into_raw(m) as usize;
    q.pending.push_back(p);
    q.live.insert(p);
    q.errors.insert(err);
    request
}

fn pop() -> u64 {
    QUEUE.lock().unwrap().pending.pop_front().unwrap_or(0) as u64
}

/// The message behind a live handle, with the queue locked.
fn with_message<R>(h: u64, f: impl FnOnce(&Message) -> R) -> Option<R> {
    let q = QUEUE.lock().unwrap();
    if !q.live.contains(&(h as usize)) {
        return None;
    }
    // SAFETY: a live handle is a Box<Message> this module leaked and has not freed.
    Some(f(unsafe { &*(h as *const Message) }))
}

fn free(h: u64) {
    let mut q = QUEUE.lock().unwrap();
    if q.live.remove(&(h as usize)) {
        q.pending.retain(|&p| p != h as usize);
        // SAFETY: as in `with_message`; removed from `live` first, so freed once.
        let m = unsafe { Box::from_raw(h as *mut Message) };
        q.errors.remove(&(&m.error as *const ErrorRec as usize));
    }
}

fn with_error<R>(h: u64, f: impl FnOnce(&ErrorRec) -> R) -> Option<R> {
    let q = QUEUE.lock().unwrap();
    if !q.errors.contains(&(h as usize)) {
        return None;
    }
    // SAFETY: an entry in `errors` is inside a live message.
    Some(f(unsafe { &*(h as *const ErrorRec) }))
}

/// The guest's `ovr_*` import `name`, or `None` for one this module does not
/// know, which then stops by name as before.
pub fn handler(name: &str) -> Option<Handler> {
    // A request function: fails through the queue with its own message type.
    let request = |label: &'static str, ty: u32| -> Handler {
        Box::new(move |c| {
            once(label, "");
            c.set_x(0, fail_request(ty));
            Ok(())
        })
    };
    // A payload accessor: nothing but errors is ever queued, so there is never
    // a payload, and null is what the SDK returns for a message without one.
    let null = |label: &'static str| -> Handler {
        Box::new(move |c| {
            once(label, "");
            c.set_x(0, 0);
            Ok(())
        })
    };
    let h: Handler = match name {
        "ovr_PlatformInitializeAndroidAsynchronous" => Box::new(|c| {
            // (const char* appId, jobject activity, JNIEnv*) -> ovrRequest
            let app = if c.x(0) == 0 {
                "null".to_string()
            } else {
                // SAFETY: the guest's C string; identity mapping.
                unsafe { CStr::from_ptr(c.x(0) as *const c_char) }.to_string_lossy().into_owned()
            };
            once("ovr_PlatformInitializeAndroidAsynchronous", &format!(" (appId {app})"));
            c.set_x(0, fail_request(MSG_PLATFORM_INITIALIZE_ANDROID_ASYNCHRONOUS));
            Ok(())
        }),
        "ovr_Entitlement_GetIsViewerEntitled" =>
            request("ovr_Entitlement_GetIsViewerEntitled", MSG_ENTITLEMENT_GET_IS_VIEWER_ENTITLED),
        "ovr_User_GetUserProof" => request("ovr_User_GetUserProof", MSG_USER_GET_USER_PROOF),
        "ovr_IAP_GetViewerPurchases" => request("ovr_IAP_GetViewerPurchases", MSG_IAP_GET_VIEWER_PURCHASES),
        "ovr_IAP_LaunchCheckoutFlow" => request("ovr_IAP_LaunchCheckoutFlow", MSG_IAP_LAUNCH_CHECKOUT_FLOW),
        "ovr_AbuseReport_ReportRequestHandled" =>
            request("ovr_AbuseReport_ReportRequestHandled", MSG_ABUSE_REPORT_REPORT_REQUEST_HANDLED),
        "ovr_UserAgeCategory_Report" => request("ovr_UserAgeCategory_Report", MSG_USER_AGE_CATEGORY_REPORT),
        "ovr_DeviceApplicationIntegrity_GetIntegrityToken" => request(
            "ovr_DeviceApplicationIntegrity_GetIntegrityToken",
            MSG_DEVICE_APPLICATION_INTEGRITY_GET_INTEGRITY_TOKEN,
        ),
        "ovr_PopMessage" => Box::new(|c| {
            once("ovr_PopMessage", "");
            c.set_x(0, pop());
            Ok(())
        }),
        "ovr_FreeMessage" => Box::new(|c| {
            once("ovr_FreeMessage", "");
            free(c.x(0));
            Ok(())
        }),
        "ovr_Message_GetType" => Box::new(|c| {
            once("ovr_Message_GetType", "");
            c.set_x(0, with_message(c.x(0), |m| m.ty).unwrap_or(0) as u64);
            Ok(())
        }),
        "ovr_Message_IsError" => Box::new(|c| {
            once("ovr_Message_IsError", "");
            // Every message this module queues is an error.
            c.set_x(0, with_message(c.x(0), |_| 1).unwrap_or(0));
            Ok(())
        }),
        "ovr_Message_GetError" => Box::new(|c| {
            once("ovr_Message_GetError", "");
            c.set_x(0, with_message(c.x(0), |m| &m.error as *const ErrorRec as u64).unwrap_or(0));
            Ok(())
        }),
        "ovr_Error_GetMessage" => Box::new(|c| {
            once("ovr_Error_GetMessage", "");
            c.set_x(0, with_error(c.x(0), |e| e.text.as_ptr() as u64).unwrap_or(0));
            Ok(())
        }),
        "ovr_Error_GetCode" => Box::new(|c| {
            once("ovr_Error_GetCode", "");
            c.set_x(0, with_error(c.x(0), |e| e.code).unwrap_or(0) as u32 as u64);
            Ok(())
        }),
        "ovr_GetLoggedInUserID" => null("ovr_GetLoggedInUserID"),
        "ovr_Message_GetString" => null("ovr_Message_GetString"),
        "ovr_Message_GetPurchase" => null("ovr_Message_GetPurchase"),
        "ovr_Message_GetPurchaseArray" => null("ovr_Message_GetPurchaseArray"),
        "ovr_Message_GetUserProof" => null("ovr_Message_GetUserProof"),
        "ovr_Purchase_GetSKU" => null("ovr_Purchase_GetSKU"),
        "ovr_UserProof_GetNonce" => null("ovr_UserProof_GetNonce"),
        "ovr_PurchaseArray_GetSize" => null("ovr_PurchaseArray_GetSize"),
        "ovr_PurchaseArray_GetElement" => null("ovr_PurchaseArray_GetElement"),
        _ => return None,
    };
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_fails_through_the_queue_and_none_succeeds() {
        let _turn = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let r = fail_request(MSG_ENTITLEMENT_GET_IS_VIEWER_ENTITLED);
        assert_ne!(r, 0);
        let m = pop();
        assert_ne!(m, 0);
        assert_eq!(with_message(m, |m| (m.ty, m.request)), Some((MSG_ENTITLEMENT_GET_IS_VIEWER_ENTITLED, r)));
        let e = with_message(m, |m| &m.error as *const ErrorRec as u64).unwrap();
        assert_eq!(with_error(e, |e| e.code), Some(ERROR_NO_PLATFORM));
        free(m);
        assert_eq!(with_message(m, |_| ()), None, "a freed handle is refused");
        assert_eq!(with_error(e, |_| ()), None);
        assert_eq!(pop(), 0, "an empty queue pops null");
    }
}
