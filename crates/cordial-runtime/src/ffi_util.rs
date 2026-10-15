//! Shared marshalling for the message-bus request/response shape.
//!
//! `linking::on_open_url`, `permissions::reply` and `webview::on_open_window`
//! each open-coded the same two steps by hand: check a pointer for null and
//! wrap it in a `CStr` inside one `unsafe` block on the way in, and (for the
//! two that answer synchronously) check a length and copy bytes back out
//! inside another `unsafe` block on the way out. Three copies of the same raw
//! pointer arithmetic are three chances for one of them to get the length
//! check wrong, and that is exactly backwards for the crate ADR-036 calls the
//! ABI edge: the two operations that actually touch a raw pointer should live
//! in one place each, so that everything downstream of them — `open_from_request`,
//! `answer`, `parse_open_window` — is ordinary safe Rust that never has to
//! reason about a pointer at all.
//!
//! Started on those three call sites and extended to `android/asset.rs`, whose
//! ten entry points now validate, convert through these helpers and call safe
//! inner functions; `bionic/signal.rs`'s `sigaction` translation went the same
//! way and has no pointer in its layout code at all. That is a few files of the
//! crate's ~350 `extern "C"` functions, not a rewrite of it -- the tracking
//! issue linked from ADR-036 says which remain.

use std::ffi::{c_char, c_void, CStr};

/// Borrow a request string handed in across the ABI edge.
///
/// Returns `None` for a null pointer. Every caller here already treats a null
/// request as "answer nothing" rather than a reason to panic — the message
/// bus is not expected to send one, but a defensive caller costs nothing to
/// plan for and turns a hypothetical null deref into an ordinary `None` match.
///
/// # Safety
/// `ptr`, if not null, must point to a NUL-terminated byte sequence valid for
/// reads for the duration of this call. Centralising the call does not remove
/// that contract — it means there is one place in the crate that states it,
/// checked once per call site, instead of one copy of the same comment at
/// every `extern "C" fn` that takes a request pointer.
pub(crate) unsafe fn borrow_request<'a>(ptr: *const c_char) -> Option<&'a CStr> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null per the check above; the NUL-termination and lifetime
    // half of the contract is this function's own, stated in its doc comment.
    Some(unsafe { CStr::from_ptr(ptr) })
}

/// Copy `bytes` (already NUL-terminated, as `CString::as_bytes_with_nul`
/// gives) into the caller's `out` buffer of `out_len` bytes.
///
/// Returns whether it fit. A null `out`, an `out_len` of zero, or a response
/// too large for the buffer are all "no room" rather than a partial write —
/// the caller already treats a `false` return as "answer nothing", which is
/// safer than a truncated response the far side has no way to detect.
pub(crate) fn write_response(bytes: &[u8], out: *mut c_char, out_len: usize) -> bool {
    if out.is_null() || out_len == 0 || bytes.len() > out_len {
        return false;
    }
    // SAFETY: `out` is non-null and `out_len` is at least `bytes.len()`,
    // checked immediately above; `bytes` is a Rust-owned slice the caller
    // holds no pointer into, so the two regions cannot overlap.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, bytes.len()) };
    true
}

// ------------------------------------------------------------ opaque handles
//
// The other shape that repeats across this crate's `extern "C"` surface: a
// native API that hands the engine an opaque pointer (`AAsset*`, `AAssetDir*`)
// which is really a `Box<T>` of ours, and takes it back to read, advance or
// free. Each function used to open-code the `*mut c_void as *const T` cast, its
// own null check and a different spelling of the SAFETY comment. The four
// operations below are the whole of it, so what remains in an entry point is
// "is this a handle of ours" and a call into ordinary Rust.

/// Move `value` to the heap and give the engine its address.
pub(crate) fn into_handle<T>(value: T) -> *mut c_void {
    Box::into_raw(Box::new(value)) as *mut c_void
}

/// Borrow the value behind a handle, or `None` for null.
///
/// # Safety
/// `handle`, if not null, must have come from [`into_handle::<T>`] with this
/// same `T` and not yet gone through [`drop_handle`], and nothing else may be
/// using it mutably for the length of the borrow.
pub(crate) unsafe fn handle_ref<'a, T>(handle: *mut c_void) -> Option<&'a T> {
    // SAFETY: the contract above is the caller's; `as_ref` supplies the null check.
    unsafe { (handle as *const T).as_ref() }
}

/// As [`handle_ref`], mutably.
///
/// # Safety
/// As [`handle_ref`], and no other borrow of the handle may be live.
pub(crate) unsafe fn handle_mut<'a, T>(handle: *mut c_void) -> Option<&'a mut T> {
    // SAFETY: as above.
    unsafe { (handle as *mut T).as_mut() }
}

/// Take a handle back and drop what it holds. Null is nothing to free.
///
/// # Safety
/// As [`handle_ref`], and the handle must not be used again afterwards: the
/// engine calls each `close` once, per the API's own contract.
pub(crate) unsafe fn drop_handle<T>(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: non-null, and from `into_handle::<T>` per the contract.
        drop(unsafe { Box::from_raw(handle as *mut T) });
    }
}

/// Write `value` through an out-parameter the caller may have passed as null.
/// Returns whether anything was written.
///
/// # Safety
/// `out`, if not null, must be valid for a write of one `T`.
pub(crate) unsafe fn store_out<T>(out: *mut T, value: T) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: non-null per the check above; validity is the caller's contract.
    unsafe { out.write(value) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn a_null_request_borrows_nothing() {
        // SAFETY: null is exactly the case under test; the function's own
        // null check runs before any dereference.
        assert!(unsafe { borrow_request(std::ptr::null()) }.is_none());
    }

    #[test]
    fn a_real_request_borrows_its_bytes() {
        let c = CString::new("hello").unwrap();
        // SAFETY: `c` outlives this call and is a valid NUL-terminated string.
        let borrowed = unsafe { borrow_request(c.as_ptr()) };
        assert_eq!(borrowed.map(CStr::to_bytes), Some(b"hello".as_slice()));
    }

    #[test]
    fn a_response_that_fits_is_copied() {
        let body = CString::new("ok").unwrap();
        let bytes = body.as_bytes_with_nul();
        let mut out = vec![0u8; bytes.len()];
        assert!(write_response(bytes, out.as_mut_ptr() as *mut c_char, out.len()));
        assert_eq!(out, bytes);
    }

    #[test]
    fn a_response_too_large_for_the_buffer_is_refused_not_truncated() {
        let body = CString::new("this will not fit").unwrap();
        let bytes = body.as_bytes_with_nul();
        let mut out = vec![0u8; 4];
        assert!(!write_response(bytes, out.as_mut_ptr() as *mut c_char, out.len()));
        // Refused, so untouched — a truncated write would be a response the
        // far side has no way to tell apart from a real, short answer.
        assert_eq!(out, vec![0u8; 4]);
    }

    #[test]
    fn no_room_is_no_room_however_it_is_spelled() {
        let bytes = b"x\0";
        assert!(!write_response(bytes, std::ptr::null_mut(), 8));
        let mut out = vec![0u8; 8];
        assert!(!write_response(bytes, out.as_mut_ptr() as *mut c_char, 0));
    }

    #[test]
    fn a_handle_round_trips_and_null_is_nothing() {
        let h = into_handle(String::from("asset"));
        // SAFETY: `h` is a live handle to a `String` and nothing else borrows it.
        unsafe {
            assert_eq!(handle_ref::<String>(h).map(String::as_str), Some("asset"));
            handle_mut::<String>(h).unwrap().push('!');
            assert_eq!(handle_ref::<String>(h).map(String::as_str), Some("asset!"));
            drop_handle::<String>(h);
            assert!(handle_ref::<String>(std::ptr::null_mut()).is_none());
            assert!(handle_mut::<String>(std::ptr::null_mut()).is_none());
            drop_handle::<String>(std::ptr::null_mut());
        }
    }

    #[test]
    fn dropping_a_handle_runs_the_destructor_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Counted;
        impl Drop for Counted {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        let h = into_handle(Counted);
        // SAFETY: a live handle to a `Counted`, freed exactly once.
        unsafe { drop_handle::<Counted>(h) };
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_out_parameter_may_be_null() {
        let mut v = 0i64;
        // SAFETY: `&mut v` is valid for a write; null is the case under test.
        unsafe {
            assert!(store_out(&mut v, 7));
            assert!(!store_out(std::ptr::null_mut::<i64>(), 9));
        }
        assert_eq!(v, 7);
    }
}
