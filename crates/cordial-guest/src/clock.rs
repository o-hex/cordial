//! `clock_gettime(CLOCK_MONOTONIC)` as guest code.
//!
//! The engine calls `clock_gettime` about 0.26 M times a second in game
//! (M7 second pass, docs/vr/dynarmic-design.md §9.9), and as a stub each is
//! a translator exit and a host dispatch around a vDSO read that costs a
//! fraction of either. `mrs cntpct_el0` is the cheap way to the same clock:
//! dynarmic emits it as a direct call to `GetCNTPCT` (`native/shim.cpp`)
//! without leaving translated code, and that reads the host's
//! CLOCK_MONOTONIC at the 600 MHz dynarmic reports as CNTFRQ_EL0. This
//! scales it back to nanoseconds. Going through 600 MHz and back loses under
//! 2 ns, and both conversions round down, so the result never runs ahead of
//! the host clock and never goes backwards.
//!
//! Every other clock -- CLOCK_REALTIME above all, which has no counter
//! behind it -- branches to the host function's ordinary stub, as
//! `string.rs` does past 128 bytes. Nothing the engine executes is changed.
//!
//! ```text
//!     cmp  w0, #1                  // CLOCK_MONOTONIC
//!     b.ne 1f
//!     mrs  x2, cntpct_el0
//!     add  x2, x2, x2, lsl #2      // ticks * 5 / 3 = ns
//!     mov  x3, #3
//!     udiv x2, x2, x3
//!     mov  x3, #0xca00             // 1e9
//!     movk x3, #0x3b9a, lsl #16
//!     udiv x4, x2, x3              // tv_sec
//!     msub x5, x4, x3, x2          // tv_nsec
//!     stp  x4, x5, [x1]
//!     mov  w0, #0
//!     ret
//! 1:  ldr  x16, 2f                 // the host's, by its stub
//!     br   x16
//!     nop
//! 2:  .quad host_stub
//! ```

use crate::jit::Runtime;

const CLOCK_GETTIME: [u32; 18] = [
    0x7100041f, 0x54000181, 0xd53be022, 0x8b020842, 0xd2800063, 0x9ac30842, 0xd2994003, 0xf2a77343,
    0x9ac30844, 0x9b038885, 0xa9001424, 0x52800000, 0xd65f03c0, 0x58000070, 0xd61f0200, 0xd503201f, 0, 0,
];
const CLOCK_GETTIME_LITERAL: usize = 16;

impl Runtime {
    /// Writes the guest `clock_gettime` into the stub page, branching for
    /// any clock but CLOCK_MONOTONIC to `host_stub`, the host function's
    /// ordinary stub, and returns its address.
    pub fn clock_function(&self, host_stub: u64) -> u64 {
        let mut words = CLOCK_GETTIME;
        words[CLOCK_GETTIME_LITERAL] = host_stub as u32;
        words[CLOCK_GETTIME_LITERAL + 1] = (host_stub >> 32) as u32;
        self.register_code("clock_gettime", &words)
    }
}
