//! `pthread_key_*` as bionic implements them, with `pthread_getspecific`
//! and `pthread_setspecific` run as guest code.
//!
//! In game the engine calls `pthread_getspecific` about five million times a
//! second -- its `thread_local`s go through emutls, which has no other way to
//! find a thread's block -- and as a stub each call cost a translator exit,
//! a host dispatch and a glibc call (M7, docs/vr/dynarmic-design.md §9.8).
//! bionic's own implementation is a table lookup through TPIDR_EL0, so this
//! is that lookup, in arm64, in Cordial's stub page: no SVC, and nothing the
//! engine executes is changed.
//!
//! The data structures are bionic's (`libc/bionic/pthread_key.cpp`): a
//! process-wide map of `{seq, destructor}` per key, where bit 0 of `seq` says
//! the key is in use and every create or delete advances it; and per thread,
//! `{seq, data}` per key, where a value counts only while its `seq` matches
//! the map's. So a deleted and re-created key reads as null in every thread
//! without anyone visiting them. Keys are bionic's too, index | 1 << 31, and
//! there are `PTHREAD_KEYS_MAX` of them. The per-thread table sits in the
//! guest thread's TLS block, after bionic's slots, at `KEY_DATA_WORD`.
//!
//! Destructors run at guest thread exit, `key_clean_all`, as bionic's
//! `pthread_key_clean_all` runs them: up to four rounds, each value cleared
//! before its destructor is called.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::jit::{guest_call, with_thread_tls, Fault, Runtime};

/// bionic's `PTHREAD_KEYS_MAX`.
pub const KEYS: usize = 128;
/// Where a thread's `{seq, data}` pairs start in its TLS block, in words
/// from TPIDR_EL0. bionic's arm64 slots end at 9; 16 keeps each pair aligned.
pub(crate) const KEY_DATA_WORD: usize = 16;
/// The TLS block's length in words.
pub(crate) const TLS_WORDS: usize = KEY_DATA_WORD + 2 * KEYS;
const VALID: u32 = 1 << 31;
const EINVAL: i32 = 22;
const EAGAIN: i32 = 11;
/// bionic's `PTHREAD_DESTRUCTOR_ITERATIONS`.
const DESTRUCTOR_ROUNDS: usize = 4;

/// `pthread_getspecific`, assembled from this (clang,
/// `--target=aarch64-linux-android26`); the literal at word 18 is patched to
/// the key map's address. x9--x13 are caller-saved in AAPCS64, as for any
/// call.
///
/// ```text
///     eor  w9, w0, #0x80000000      // index; valid iff < 128 (KeyInValidRange)
///     cmp  w9, #128
///     b.hs 1f
///     ldr  x11, 3f                  // &key_map[index]
///     add  x11, x11, x9, lsl #4
///     ldr  x12, [x11]               // seq
///     mrs  x10, tpidr_el0           // &key_data[index]
///     add  x10, x10, #128
///     add  x10, x10, x9, lsl #4
///     tbz  x12, #0, 2f              // not in use
///     ldr  x13, [x10]
///     cmp  x12, x13
///     b.ne 2f                       // a deleted key's value
///     ldr  x0, [x10, #8]
///     ret
/// 2:  str  xzr, [x10, #8]           // cleared, as bionic does
/// 1:  mov  x0, #0
///     ret
/// 3:  .quad key_map
/// ```
const GETSPECIFIC: [u32; 20] = [
    0x52010009, 0x7102013f, 0x540001c2, 0x580001eb, 0x8b09116b, 0xf940016c, 0xd53bd04a, 0x9102014a,
    0x8b09114a, 0x360000cc, 0xf940014d, 0xeb0d019f, 0x54000061, 0xf9400540, 0xd65f03c0, 0xf900055f,
    0xd2800000, 0xd65f03c0, 0, 0,
];
const GETSPECIFIC_LITERAL: usize = 18;

/// `pthread_setspecific`, the same way; the literal is at word 16.
///
/// ```text
///     eor  w9, w0, #0x80000000
///     cmp  w9, #128
///     b.hs 1f
///     ldr  x11, 3f
///     add  x11, x11, x9, lsl #4
///     ldr  x12, [x11]               // seq
///     tbz  x12, #0, 1f              // not in use: EINVAL
///     mrs  x10, tpidr_el0
///     add  x10, x10, #128
///     add  x10, x10, x9, lsl #4
///     stp  x12, x1, [x10]           // {seq, data}
///     mov  w0, #0
///     ret
/// 1:  mov  w0, #22
///     ret
///     nop
/// 3:  .quad key_map
/// ```
const SETSPECIFIC: [u32; 18] = [
    0x52010009, 0x7102013f, 0x54000162, 0x580001ab, 0x8b09116b, 0xf940016c, 0x360000ec, 0xd53bd04a,
    0x9102014a, 0x8b09114a, 0xa900054c, 0x52800000, 0xd65f03c0, 0x528002c0, 0xd65f03c0, 0xd503201f, 0, 0,
];
const SETSPECIFIC_LITERAL: usize = 16;

/// bionic's `key_map`: `{seq, destructor}` per key.
pub(crate) struct KeyMap(Box<[AtomicU64]>);

impl KeyMap {
    pub(crate) fn new() -> KeyMap {
        KeyMap((0..2 * KEYS).map(|_| AtomicU64::new(0)).collect())
    }
    fn seq(&self, i: usize) -> &AtomicU64 {
        &self.0[2 * i]
    }
    fn dtor(&self, i: usize) -> &AtomicU64 {
        &self.0[2 * i + 1]
    }
    fn addr(&self) -> u64 {
        self.0.as_ptr() as u64
    }
}

fn with_literal<const N: usize>(code: [u32; N], at: usize, v: u64) -> [u32; N] {
    let mut c = code;
    c[at] = v as u32;
    c[at + 1] = (v >> 32) as u32;
    c
}

impl Runtime {
    /// Writes the guest `pthread_getspecific` and `pthread_setspecific` into
    /// the stub page and returns their addresses. The guest calls them like
    /// any import.
    pub fn key_functions(&self) -> (u64, u64) {
        let m = self.keys.addr();
        (self.register_code("pthread_getspecific", &with_literal(GETSPECIFIC, GETSPECIFIC_LITERAL, m)),
         self.register_code("pthread_setspecific", &with_literal(SETSPECIFIC, SETSPECIFIC_LITERAL, m)))
    }

    /// `pthread_key_create`: the key, or EAGAIN once all `KEYS` are in use.
    pub fn key_create(&self, dtor: u64) -> Result<u32, i32> {
        for i in 0..KEYS {
            let mut seq = self.keys.seq(i).load(Ordering::Relaxed);
            while seq & 1 == 0 {
                match self.keys.seq(i).compare_exchange_weak(seq, seq + 1, Ordering::SeqCst, Ordering::Relaxed) {
                    Ok(_) => {
                        self.keys.dtor(i).store(dtor, Ordering::SeqCst);
                        return Ok(i as u32 | VALID);
                    }
                    Err(s) => seq = s,
                }
            }
        }
        Err(EAGAIN)
    }

    /// `pthread_key_delete`: 0, or EINVAL for a key not in use. Destructors
    /// are not called, as POSIX requires.
    pub fn key_delete(&self, key: u32) -> i32 {
        let i = key ^ VALID;
        if i as usize >= KEYS {
            return EINVAL;
        }
        let s = self.keys.seq(i as usize);
        let seq = s.load(Ordering::Relaxed);
        if seq & 1 == 1 && s.compare_exchange(seq, seq + 1, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
            return 0;
        }
        EINVAL
    }

    /// This thread's value for `key`, as the guest's `pthread_getspecific`
    /// would read it, without clearing a stale one. For tests and reports.
    pub fn key_value(&self, key: u32) -> Option<u64> {
        let i = (key ^ VALID) as usize;
        if i >= KEYS {
            return None;
        }
        let seq = self.keys.seq(i).load(Ordering::Relaxed);
        with_thread_tls(self, |tls| {
            // SAFETY: TLS_WORDS words, this thread's own block.
            let (s, d) = unsafe {
                (tls.add(KEY_DATA_WORD + 2 * i).read_volatile(), tls.add(KEY_DATA_WORD + 2 * i + 1).read_volatile())
            };
            if seq & 1 == 1 && s == seq { d } else { 0 }
        })
    }
}

/// bionic's `pthread_key_clean_all`, for the calling thread as it leaves
/// the guest for good.
pub fn key_clean_all(rt: &Arc<Runtime>) -> Result<(), Fault> {
    for _ in 0..DESTRUCTOR_ROUNDS {
        let mut called = 0;
        for i in 0..KEYS {
            let seq = rt.keys.seq(i).load(Ordering::Relaxed);
            if seq & 1 == 0 {
                continue;
            }
            // SAFETY: this thread's own TLS block, TLS_WORDS long.
            let data = with_thread_tls(rt, |tls| unsafe {
                let s = tls.add(KEY_DATA_WORD + 2 * i);
                let d = s.add(1);
                (s.read_volatile() == seq).then(|| d.read_volatile()).unwrap_or(0)
            });
            if data.unwrap_or(0) == 0 {
                continue;
            }
            let dtor = rt.keys.dtor(i).load(Ordering::Acquire);
            if dtor == 0 || rt.keys.seq(i).load(Ordering::Relaxed) != seq {
                continue;
            }
            // SAFETY: as above; cleared before the call, as bionic does.
            with_thread_tls(rt, |tls| unsafe { tls.add(KEY_DATA_WORD + 2 * i + 1).write_volatile(0) });
            guest_call(rt, dtor, &[data.unwrap_or(0)], &[])?;
            called += 1;
        }
        if called == 0 {
            break;
        }
    }
    Ok(())
}
