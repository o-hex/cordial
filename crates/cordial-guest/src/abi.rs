//! AAPCS64 arguments in, a SysV x86-64 call out.
//!
//! Design §3.1 proposed one fixed mapping -- x0..x5 to rdi..r9, x6 and x7 to
//! the stack, v0..v7 to xmm0..7 -- and a descriptor only for fix-ups. That
//! mapping is right for a call whose floating-point arguments all fit in
//! registers, and wrong once one overflows: both ABIs put overflowed
//! arguments on the stack in argument order, but they overflow *different*
//! arguments (SysV runs out of integer registers at six, AAPCS64 at eight),
//! so a ninth double passed before the seventh integer lands above it in
//! SysV and below it in AAPCS64. The M1 snprintf test is built to hit exactly
//! that. So every call is classified argument by argument from its type
//! list, and the fixed mapping falls out as the common case rather than
//! being assumed.
//!
//! Variadic arguments need no separate treatment on this side: AAPCS64 on
//! Linux (unlike Apple's) passes them exactly like named ones, and SysV's
//! only difference, `%al`, is set to 8 by the trampoline. What a variadic
//! thunk adds is the type list, which comes from the format string.

use crate::ffi::{cg_host_call_guarded, HostRet};
use crate::jit::{Call, Fault};

/// The type of one argument, as far as either calling convention cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    Ptr,
    F32,
    F64,
}

/// A return type. By-value aggregates and `long double` are absent on
/// purpose: a thunk that needs one is hand-written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ret {
    Void,
    Int(Ty),
    F32,
    F64,
}

impl Ty {
    pub fn is_fp(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }

    /// AAPCS64 leaves the bits above a narrow integer unspecified; SysV
    /// callers built by Clang extend to 32 and Clang-built callees rely on
    /// it. Extending here to 64 satisfies both readings.
    pub fn extend(self, v: u64) -> u64 {
        match self {
            Ty::I8 => v as i8 as i64 as u64,
            Ty::U8 => v as u8 as u64,
            Ty::I16 => v as i16 as i64 as u64,
            Ty::U16 => v as u16 as u64,
            Ty::I32 => v as i32 as i64 as u64,
            Ty::U32 => v as u32 as u64,
            _ => v,
        }
    }
}

/// Reads the AAPCS64 arguments described by `args` out of `call`, in
/// argument order: an integer as its 64-bit register or stack word, a float
/// as its bit pattern. Every stack argument on Linux AAPCS64 takes an 8-byte
/// slot, including an int or a float; Apple packs them, Linux does not.
pub fn collect(call: &Call, args: &[Ty]) -> Vec<u64> {
    let mut out = vec![0; args.len()];
    collect_into(call, args, &mut out);
    out
}

fn collect_into(call: &Call, args: &[Ty], out: &mut [u64]) {
    let (mut gi, mut gv, mut gs) = (0u32, 0u32, 0u64);
    for (&ty, o) in args.iter().zip(out) {
        *o = if ty.is_fp() {
            if gv < 8 {
                gv += 1;
                call.v(gv - 1)[0]
            } else {
                gs += 1;
                call.stack_word(gs - 1)
            }
        } else if gi < 8 {
            gi += 1;
            call.x(gi - 1)
        } else {
            gs += 1;
            call.stack_word(gs - 1)
        };
    }
}

/// Calls the SysV function `f` with `vals`, typed by `args`, and returns
/// every register a SysV return can use.
///
/// Guarded: a C++ exception thrown by the callee (libjnivm reports misuse
/// that way) is caught at the boundary and becomes `Fault::HostException`,
/// because unwinding it into the translator's frames or into Rust would end
/// in `std::terminate` with nothing said about where it came from.
///
/// # Safety
///
/// `f` must be a host function whose SysV signature `args` describes, and
/// each pointer among `vals` must be valid for what `f` does with it.
pub unsafe fn invoke(name: &str, f: *const std::ffi::c_void, args: &[Ty], vals: &[u64]) -> Result<HostRet, Fault> {
    debug_assert_eq!(args.len(), vals.len());
    let mut gpr = [0u64; 6];
    let mut xmm = [[0u64; 2]; 8];
    // At most as many stack words as arguments; on the stack below 32,
    // since this runs on every stub call and malloc was measurable in it.
    let mut small = [0u64; 32];
    let mut big = Vec::new();
    let buf: &mut [u64] = if args.len() <= small.len() {
        &mut small
    } else {
        big.resize(args.len(), 0);
        &mut big
    };
    let mut nstack = 0usize;
    let (mut ngpr, mut nxmm) = (0usize, 0usize);
    for (&ty, &v) in args.iter().zip(vals) {
        if ty.is_fp() {
            let bits = if ty == Ty::F32 { v & 0xffff_ffff } else { v };
            if nxmm < 8 {
                xmm[nxmm] = [bits, 0];
                nxmm += 1;
            } else {
                buf[nstack] = bits;
                nstack += 1;
            }
        } else {
            let v = ty.extend(v);
            if ngpr < 6 {
                gpr[ngpr] = v;
                ngpr += 1;
            } else {
                buf[nstack] = v;
                nstack += 1;
            }
        }
    }
    let mut out = HostRet::default();
    let mut err = [0u8; 256];
    // SAFETY: `f` is a host function whose SysV signature the caller has
    // described by `args`; the image built above is that signature's
    // register and stack assignment. That correspondence is the whole of the
    // contract and is what the M1 tests check against native calls.
    let rc = unsafe {
        cg_host_call_guarded(f, &gpr, &xmm, buf.as_ptr(), nstack as u64, &mut out,
                             err.as_mut_ptr().cast(), err.len())
    };
    if rc != 0 {
        let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
        return Err(Fault::HostException {
            function: name.to_owned(),
            what: String::from_utf8_lossy(&err[..end]).into_owned(),
        });
    }
    Ok(out)
}

/// Writes a SysV return into the guest's x0 or v0.
pub fn write_ret(call: &Call, ret: Ret, out: &HostRet) {
    match ret {
        Ret::Void => {}
        Ret::Int(t) => call.set_x(0, t.extend(out.rax)),
        Ret::F64 => call.set_v(0, [out.xmm0[0], 0]),
        Ret::F32 => call.set_v(0, [out.xmm0[0] & 0xffff_ffff, 0]),
    }
}

/// Builds the SysV register and stack image for a call whose AAPCS64
/// arguments are described by `args`, reading them out of `call`, and makes
/// the call. The return value is written back into x0 or v0.
pub(crate) fn call_host(call: &Call, f: *const std::ffi::c_void, args: &[Ty], ret: Ret) -> Result<(), Fault> {
    let mut small = [0u64; 16];
    let mut big = Vec::new();
    let vals: &mut [u64] = if args.len() <= small.len() {
        &mut small[..args.len()]
    } else {
        big.resize(args.len(), 0);
        &mut big
    };
    collect_into(call, args, vals);
    // SAFETY: the caller of call_host describes `f` by `args`; the values
    // are the guest's own arguments, whose pointers are host addresses.
    let out = unsafe { invoke(call.name(), f, args, vals) }?;
    write_ret(call, ret, &out);
    Ok(())
}

/// An AAPCS64 `va_list`, read from guest memory: `{__stack, __gr_top,
/// __vr_top, __gr_offs, __vr_offs}`, 32 bytes, against SysV's 24-byte
/// `__va_list_tag`. The host cannot forward one, so a thunk taking a
/// `va_list` walks it here by type, exactly as the callee's `va_arg` would,
/// and passes the values on explicitly.
pub struct VaList {
    stack: u64,
    gr_top: u64,
    vr_top: u64,
    gr_offs: i32,
    vr_offs: i32,
}

impl VaList {
    /// # Safety
    ///
    /// `p` must point at a live guest `va_list` (by AAPCS64 a `va_list`
    /// argument is passed as a pointer to the caller's copy).
    pub unsafe fn read(p: u64) -> VaList {
        let w = p as *const u64;
        // SAFETY: the caller's guarantee; identity mapping.
        unsafe {
            VaList {
                stack: w.read(),
                gr_top: w.add(1).read(),
                vr_top: w.add(2).read(),
                gr_offs: (p as *const i32).add(6).read(),
                vr_offs: (p as *const i32).add(7).read(),
            }
        }
    }

    /// The next argument of type `ty`, as `va_arg` would fetch it: from the
    /// register save area while its offset is negative, then from the stack.
    /// A float is returned as the bits of the double it was promoted to.
    pub fn next(&mut self, ty: Ty) -> u64 {
        if ty.is_fp() {
            if self.vr_offs < 0 && self.vr_offs + 16 <= 0 {
                let a = (self.vr_top as i64 + self.vr_offs as i64) as u64;
                self.vr_offs += 16;
                // SAFETY: inside the caller's vector register save area,
                // live for the call that passed the list (`read`'s contract).
                return unsafe { (a as *const u64).read() };
            }
            self.vr_offs = 0;
        } else {
            if self.gr_offs < 0 && self.gr_offs + 8 <= 0 {
                let a = (self.gr_top as i64 + self.gr_offs as i64) as u64;
                self.gr_offs += 8;
                // SAFETY: inside the caller's general register save area.
                return unsafe { (a as *const u64).read() };
            }
            self.gr_offs = 0;
        }
        // SAFETY: the caller's stack arguments, as `va_arg` would read them.
        let v = unsafe { (self.stack as *const u64).read() };
        self.stack += 8;
        v
    }
}

/// The types a printf-family format consumes, after the named arguments.
///
/// Refuses what it cannot classify rather than guessing: `%L` (the guest's
/// `long double` is a 128-bit quad in a q register, the host's is an 80-bit
/// x87 value in memory, and no register image converts one into the other)
/// and positional `%n$` arguments.
pub fn printf_types(fmt: &[u8]) -> Result<Vec<Ty>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            i += 1;
            continue;
        }
        i += 1;
        if fmt.get(i) == Some(&b'%') {
            i += 1;
            continue;
        }
        // Flags.
        while i < fmt.len() && b"-+ #0'I".contains(&fmt[i]) {
            i += 1;
        }
        // Width.
        if fmt.get(i) == Some(&b'*') {
            out.push(Ty::I32);
            i += 1;
        }
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            i += 1;
        }
        if fmt.get(i) == Some(&b'$') {
            return Err("positional printf arguments are not supported".into());
        }
        // Precision.
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            if fmt.get(i) == Some(&b'*') {
                out.push(Ty::I32);
                i += 1;
            }
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                i += 1;
            }
        }
        // Length.
        let mut wide = false;
        let mut long_double = false;
        while i < fmt.len() {
            match fmt[i] {
                b'h' => {}
                b'l' | b'j' | b'z' | b't' | b'q' => wide = true,
                b'L' => long_double = true,
                _ => break,
            }
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else {
            return Err("format ends inside a conversion".into());
        };
        i += 1;
        let ty = match conv {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'c' | b'C' => {
                if wide { Ty::I64 } else { Ty::I32 }
            }
            b's' | b'S' | b'p' | b'n' => Ty::Ptr,
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => {
                if long_double {
                    return Err("%L: long double differs between AAPCS64 and SysV".into());
                }
                Ty::F64
            }
            b'm' => continue,
            c => return Err(format!("unknown printf conversion %{}", c as char)),
        };
        out.push(ty);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printf_types_classifies_star_and_length() {
        use Ty::*;
        assert_eq!(printf_types(b"%*.*f %ld %s %%%c %hhd").unwrap(),
                   vec![I32, I32, F64, I64, Ptr, I32, I32]);
    }

    #[test]
    fn printf_types_refuses_long_double_and_positional() {
        assert!(printf_types(b"%Lf").is_err());
        assert!(printf_types(b"%1$d").is_err());
    }
}
