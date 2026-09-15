//! Optional logging wrappers around the libc calls that reveal what Roblox is
//! trying to do.
//!
//! There is no `strace` in the target environment and the process has no frame
//! pointers, so an unwound backtrace past the innermost frame is guesswork.
//! Cordial owns the symbol table, though, which means any import can be
//! intercepted. That is a better instrument than a debugger here: it says what
//! was called, with what, in order.
//!
//! Enabled with `CORDIAL_TRACE=1`. Off by default, and **not merely because it
//! is loud**: `open64`, `openat`, `prctl` and `syscall` are variadic, and these
//! wrappers declare them with fixed arity. That is not ABI-safe — the callee
//! reads `al` for the vector-register count and walks the register save area
//! differently — and turning this on has been observed to make Roblox abort
//! where it otherwise runs. Treat any behaviour seen under it as suspect until
//! reproduced without it.
//!
//! `CORDIAL_ANDROID_TRACE=1` is the safe counterpart: the Android API has no
//! variadic entry points, so wrapping it changes nothing.

use std::ffi::{c_char, c_int, c_long, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

fn log(args: std::fmt::Arguments<'_>) {
    if ENABLED.load(Ordering::Relaxed) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        // The thread id is not decoration. Roblox runs this work on twenty-odd
        // threads, and a single interleaved sequence invites reading two
        // unrelated calls as cause and effect.
        let tid = unsafe { libc_gettid() };
        eprintln!("[{n:>6}] tid={tid} {args}");
    }
}

extern "C" {
    #[link_name = "gettid"]
    fn libc_gettid() -> c_int;
}

/// SAFETY: `p` is either null or a NUL-terminated C string.
unsafe fn s(p: *const c_char) -> String {
    if p.is_null() {
        "(null)".into()
    } else {
        // SAFETY: non-null per the check above; NUL-termination is this
        // function's own contract, stated above.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

extern "C" {
    fn open64(path: *const c_char, flags: c_int, ...) -> c_int;
    fn openat(dirfd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn access(path: *const c_char, mode: c_int) -> c_int;
    fn getenv(name: *const c_char) -> *mut c_char;
    fn sysconf(name: c_int) -> c_long;
    fn getauxval(kind: c_ulong) -> c_ulong;
    fn prctl(option: c_int, ...) -> c_int;
    fn syscall(num: c_long, ...) -> c_long;
    fn abort() -> !;
}

#[allow(non_camel_case_types)]
type c_ulong = u64;

// ------------------------------------------------------------------- wrappers

extern "C" fn t_open64(path: *const c_char, flags: c_int, mode: u32) -> c_int {
    // SAFETY: forwarding the caller's own arguments.
    let fd = unsafe { open64(path, flags, mode) };
    log(format_args!("open64({:?}, {flags:#x}) = {fd}", unsafe { s(path) }));
    fd
}

extern "C" fn t_openat(dirfd: c_int, path: *const c_char, flags: c_int, mode: u32) -> c_int {
    // SAFETY: forwarding the caller's own arguments.
    let fd = unsafe { openat(dirfd, path, flags, mode) };
    log(format_args!("openat({dirfd}, {:?}, {flags:#x}) = {fd}", unsafe { s(path) }));
    fd
}

extern "C" fn t_access(path: *const c_char, mode: c_int) -> c_int {
    // SAFETY: forwarding the caller's own arguments.
    let rc = unsafe { access(path, mode) };
    log(format_args!("access({:?}, {mode}) = {rc}", unsafe { s(path) }));
    rc
}

extern "C" fn t_getenv(name: *const c_char) -> *mut c_char {
    // SAFETY: forwarding the caller's own arguments.
    let v = unsafe { getenv(name) };
    log(format_args!(
        "getenv({:?}) = {:?}",
        unsafe { s(name) },
        unsafe { s(v) }
    ));
    v
}

extern "C" fn t_sysconf(name: c_int) -> c_long {
    // SAFETY: forwarding the caller's own arguments.
    let v = unsafe { sysconf(name) };
    log(format_args!("sysconf({name}) = {v}"));
    v
}

extern "C" fn t_getauxval(kind: c_ulong) -> c_ulong {
    // SAFETY: forwarding the caller's own arguments.
    let v = unsafe { getauxval(kind) };
    log(format_args!("getauxval({kind}) = {v:#x}"));
    v
}

extern "C" fn t_prctl(option: c_int, a: u64, b: u64, c: u64, d: u64) -> c_int {
    // SAFETY: forwarding the caller's own arguments.
    let rc = unsafe { prctl(option, a, b, c, d) };
    log(format_args!("prctl({option}, {a:#x}, ...) = {rc}"));
    rc
}

extern "C" fn t_syscall(num: c_long, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> c_long {
    // SAFETY: forwarding the caller's own arguments.
    let rc = unsafe { syscall(num, a, b, c, d, e, f) };
    log(format_args!("syscall({num}, {a:#x}, {b:#x}) = {rc}"));
    rc
}

/// The one wrapper that is worth having even without `--trace`: it turns "the
/// process died" into "the process chose to die, here".
extern "C" fn t_abort() -> ! {
    eprintln!("\n*** Roblox called abort() ***");
    crate::stubs::report();
    // SAFETY: abort is noreturn and takes no arguments.
    unsafe { abort() }
}

extern "C" fn t_stack_chk_fail() -> ! {
    eprintln!("\n*** stack protector tripped inside Roblox ***");
    eprintln!("    A callee wrote past its frame. If Cordial provided that callee,");
    eprintln!("    its signature or a struct layout is wrong — see bionic::pthread.");
    crate::stubs::report();
    // SAFETY: as above.
    unsafe { abort() }
}

extern "C" fn t_android_log_assert(
    cond: *const c_char,
    tag: *const c_char,
    fmt: *const c_char,
) -> ! {
    // SAFETY: liblog's contract is NUL-terminated strings or null.
    unsafe {
        eprintln!("\n*** Roblox log-assert ***");
        eprintln!("    tag:       {}", s(tag));
        eprintln!("    condition: {}", s(cond));
        eprintln!("    message:   {}", s(fmt));
    }
    crate::stubs::report();
    // SAFETY: as above.
    unsafe { abort() }
}

/// Wrappers installed unconditionally, because a silent abort costs more time
/// than the wrapper costs anything.
pub fn always_on() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("abort", t_abort),
        f!("__stack_chk_fail", t_stack_chk_fail),
        f!("__android_log_assert", t_android_log_assert),
    ]
}

/// Wrappers installed only with `--trace`.
pub fn verbose() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("open64", t_open64),
        f!("openat", t_openat),
        f!("access", t_access),
        f!("getenv", t_getenv),
        f!("sysconf", t_sysconf),
        f!("getauxval", t_getauxval),
        f!("prctl", t_prctl),
        f!("syscall", t_syscall),
    ]
}
