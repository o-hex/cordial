//! `memcpy`, `memmove`, `memset` and `memcmp` for small sizes, as guest code.
//!
//! In game these four are about 0.9 M of the engine's 1.85 M stub calls a
//! second, and nearly all of them are short: a struct, a key, a few vertices.
//! Through a stub each is a translator exit, a host dispatch and a glibc call
//! whose own work is a handful of loads and stores (M7 second pass,
//! docs/vr/dynarmic-design.md §9.9). So up to 128 bytes they run here, in
//! the stub page, the way `keys.rs` runs `pthread_getspecific`; past that the
//! same code branches to the host function's ordinary stub, where glibc's
//! vector loops are faster than anything the translator makes of a guest
//! loop. Nothing the engine executes is changed.
//!
//! The technique is the usual one for short copies: a load from each end of
//! the range, overlapping in the middle, so every size in a class takes the
//! same path. Every byte is loaded before any is stored, which makes the copy
//! overlap-safe at every size it handles and lets `memmove` be the same code.
//! `memcmp` returns -1, 0 or 1 from its word compare and the byte difference
//! from its byte loop; C asks only for the sign, and glibc returns either
//! depending on the path too.
//!
//! The arm64 below was written for this and assembled with clang
//! (`--target=aarch64-linux-android26`). It uses x3--x8, x16 and v0--v7,
//! all caller-saved in AAPCS64, as for any call to these.

use crate::jit::Runtime;

/// `memcpy` and `memmove`: x0 dst, x1 src, x2 n; x0 is returned unchanged.
///
/// ```text
///     add  x4, x1, x2          // srcend
///     add  x5, x0, x2          // dstend
///     cmp  x2, #16
///     b.hi 3f
///     cmp  x2, #8
///     b.lo 1f
///     ldr  x6, [x1]            // 8..16
///     ldr  x7, [x4, #-8]
///     str  x6, [x0]
///     str  x7, [x5, #-8]
///     ret
/// 1:  tbz  x2, #2, 2f          // 4..7
///     ldr  w6, [x1]
///     ldr  w7, [x4, #-4]
///     str  w6, [x0]
///     str  w7, [x5, #-4]
///     ret
/// 2:  cbz  x2, 9f              // 1..3: first, middle, last
///     lsr  x3, x2, #1
///     ldrb w6, [x1]
///     ldrb w7, [x4, #-1]
///     ldrb w8, [x1, x3]
///     strb w6, [x0]
///     strb w8, [x0, x3]
///     strb w7, [x5, #-1]
/// 9:  ret
/// 3:  cmp  x2, #32             // 17..32
///     b.hi 4f
///     ldr  q0, [x1]
///     ldr  q1, [x4, #-16]
///     str  q0, [x0]
///     str  q1, [x5, #-16]
///     ret
/// 4:  cmp  x2, #64             // 33..64
///     b.hi 5f
///     ldp  q0, q1, [x1]
///     ldp  q2, q3, [x4, #-32]
///     stp  q0, q1, [x0]
///     stp  q2, q3, [x5, #-32]
///     ret
/// 5:  cmp  x2, #128            // 65..128
///     b.hi 6f
///     ldp  q0, q1, [x1]
///     ldp  q2, q3, [x1, #32]
///     ldp  q4, q5, [x4, #-64]
///     ldp  q6, q7, [x4, #-32]
///     stp  q0, q1, [x0]
///     stp  q2, q3, [x0, #32]
///     stp  q4, q5, [x5, #-64]
///     stp  q6, q7, [x5, #-32]
///     ret
/// 6:  ldr  x16, 7f             // the host's, by its stub; x30 is still
///     br   x16                 // the caller's, so its `ret` returns there
///     nop
/// 7:  .quad host_stub
/// ```
const COPY: [u32; 56] = [
    0x8b020024, 0x8b020005, 0xf100405f, 0x540002e8, 0xf100205f, 0x540000c3, 0xf9400026, 0xf85f8087,
    0xf9000006, 0xf81f80a7, 0xd65f03c0, 0x361000c2, 0xb9400026, 0xb85fc087, 0xb9000006, 0xb81fc0a7,
    0xd65f03c0, 0xb4000102, 0xd341fc43, 0x39400026, 0x385ff087, 0x38636828, 0x39000006, 0x38236808,
    0x381ff0a7, 0xd65f03c0, 0xf100805f, 0x540000c8, 0x3dc00020, 0x3cdf0081, 0x3d800000, 0x3c9f00a1,
    0xd65f03c0, 0xf101005f, 0x540000c8, 0xad400420, 0xad7f0c82, 0xad000400, 0xad3f0ca2, 0xd65f03c0,
    0xf102005f, 0x54000148, 0xad400420, 0xad410c22, 0xad7e1484, 0xad7f1c86, 0xad000400, 0xad010c02,
    0xad3e14a4, 0xad3f1ca6, 0xd65f03c0, 0x58000070, 0xd61f0200, 0xd503201f, 0, 0,
];
const COPY_LITERAL: usize = 54;

/// `memset`: x0 dst, w1 the byte, x2 n; x0 is returned unchanged.
///
/// ```text
///     and  w1, w1, #0xff
///     mov  x3, #0x0101010101010101
///     mul  x3, x1, x3          // the byte in every lane
///     dup  v0.2d, x3
///     add  x5, x0, x2
///     cmp  x2, #16
///     b.hi 3f
///     cmp  x2, #8
///     b.lo 1f
///     str  x3, [x0]
///     str  x3, [x5, #-8]
///     ret
/// 1:  tbz  x2, #2, 2f
///     str  w3, [x0]
///     str  w3, [x5, #-4]
///     ret
/// 2:  cbz  x2, 9f
///     lsr  x4, x2, #1
///     strb w3, [x0]
///     strb w3, [x0, x4]
///     strb w3, [x5, #-1]
/// 9:  ret
/// 3:  cmp  x2, #32
///     b.hi 4f
///     str  q0, [x0]
///     str  q0, [x5, #-16]
///     ret
/// 4:  cmp  x2, #64
///     b.hi 5f
///     stp  q0, q0, [x0]
///     stp  q0, q0, [x5, #-32]
///     ret
/// 5:  cmp  x2, #128
///     b.hi 6f
///     stp  q0, q0, [x0]
///     stp  q0, q0, [x0, #32]
///     stp  q0, q0, [x5, #-64]
///     stp  q0, q0, [x5, #-32]
///     ret
/// 6:  ldr  x16, 7f
///     br   x16
///     nop
/// 7:  .quad host_stub
/// ```
const SET: [u32; 44] = [
    0x12001c21, 0xb200c3e3, 0x9b037c23, 0x4e080c60, 0x8b020005, 0xf100405f, 0x54000208, 0xf100205f,
    0x54000083, 0xf9000003, 0xf81f80a3, 0xd65f03c0, 0x36100082, 0xb9000003, 0xb81fc0a3, 0xd65f03c0,
    0xb40000a2, 0xd341fc44, 0x39000003, 0x38246803, 0x381ff0a3, 0xd65f03c0, 0xf100805f, 0x54000088,
    0x3d800000, 0x3c9f00a0, 0xd65f03c0, 0xf101005f, 0x54000088, 0xad000000, 0xad3f00a0, 0xd65f03c0,
    0xf102005f, 0x540000c8, 0xad000000, 0xad010000, 0xad3e00a0, 0xad3f00a0, 0xd65f03c0, 0x58000070,
    0xd61f0200, 0xd503201f, 0, 0,
];
const SET_LITERAL: usize = 42;

/// `memcmp`: x0 a, x1 b, x2 n.
///
/// ```text
///     cmp  x2, #128
///     b.hi 6f
///     cmp  x2, #8
///     b.lo 4f
/// 1:  ldr  x3, [x0], #8        // a word at a time
///     ldr  x4, [x1], #8
///     cmp  x3, x4
///     b.ne 3f
///     sub  x2, x2, #8
///     cmp  x2, #8
///     b.hs 1b
///     cbz  x2, 2f
///     sub  x2, x2, #8          // the last 8 bytes; the ones they share
///     ldr  x3, [x0, x2]        // with the previous word are equal
///     ldr  x4, [x1, x2]
///     cmp  x3, x4
///     b.ne 3f
/// 2:  mov  w0, #0
///     ret
/// 3:  rev  x3, x3              // the first differing byte is the most
///     rev  x4, x4              // significant one that differs
///     cmp  x3, x4
///     cset w0, ne
///     cneg w0, w0, lo
///     ret
/// 4:  cbz  x2, 2b              // 1..7, a byte at a time
/// 5:  ldrb w3, [x0], #1
///     ldrb w4, [x1], #1
///     subs w3, w3, w4
///     b.ne 8f
///     subs x2, x2, #1
///     b.ne 5b
///     mov  w0, #0
///     ret
/// 8:  mov  w0, w3
///     ret
/// 6:  ldr  x16, 7f
///     br   x16
/// 7:  .quad host_stub
/// ```
const CMP: [u32; 40] = [
    0xf102005f, 0x54000468, 0xf100205f, 0x540002c3, 0xf8408403, 0xf8408424, 0xeb04007f, 0x54000181,
    0xd1002042, 0xf100205f, 0x54ffff42, 0xb40000c2, 0xd1002042, 0xf8626803, 0xf8626824, 0xeb04007f,
    0x54000061, 0x52800000, 0xd65f03c0, 0xdac00c63, 0xdac00c84, 0xeb04007f, 0x1a9f07e0, 0x5a802400,
    0xd65f03c0, 0xb4ffff02, 0x38401403, 0x38401424, 0x6b040063, 0x540000a1, 0xf1000442, 0x54ffff61,
    0x52800000, 0xd65f03c0, 0x2a0303e0, 0xd65f03c0, 0x58000050, 0xd61f0200, 0, 0,
];
const CMP_LITERAL: usize = 38;

/// The names `string_function` answers.
pub const STRING_FUNCTIONS: [&str; 4] = ["memcpy", "memmove", "memset", "memcmp"];

impl Runtime {
    /// Writes the guest `name` into the stub page, falling back above 128
    /// bytes to `host_stub`, the address of the host function's ordinary
    /// stub, and returns its address. None for a name it does not implement.
    pub fn string_function(&self, name: &str, host_stub: u64) -> Option<u64> {
        let (code, at): (&[u32], usize) = match name {
            "memcpy" | "memmove" => (&COPY, COPY_LITERAL),
            "memset" => (&SET, SET_LITERAL),
            "memcmp" => (&CMP, CMP_LITERAL),
            _ => return None,
        };
        let mut words = code.to_vec();
        words[at] = host_stub as u32;
        words[at + 1] = (host_stub >> 32) as u32;
        Some(self.register_code(name, &words))
    }
}
