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
//! This is a demonstration of the pattern on three call sites, not a rewrite
//! of the crate's ~350 `extern "C"` functions — see the tracking issue linked
//! from ADR-036 for the rest.

use std::ffi::{c_char, CStr};

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
}
