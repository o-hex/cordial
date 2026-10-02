// The generic guest-to-host call, docs/vr/dynarmic-design.md §3.1.
//
// It knows nothing of AAPCS64. The Rust side has already decided, per
// function, which guest register or stack slot feeds which SysV slot; this
// only loads a SysV call from that image and captures every register a SysV
// return can use. Keeping the classification out of assembly is what lets a
// variadic thunk and a plain one share it: they differ in how the image is
// built, not in how it is called.
//
//   uint64_t cg_host_call(fn, const uint64_t gpr[6], const uint8_t xmm[8][16],
//                         const uint64_t* stack, uint64_t nstack,
//                         struct { uint64_t rax, rdx; uint8_t xmm0[16], xmm1[16]; } *out)

    .text
    .globl cg_host_call
    .type cg_host_call, @function
cg_host_call:
    .cfi_startproc
    push %rbp
    .cfi_def_cfa_offset 16
    .cfi_offset %rbp, -16
    mov %rsp, %rbp
    .cfi_def_cfa_register %rbp
    push %rbx
    push %r12
    .cfi_offset %rbx, -24
    .cfi_offset %r12, -32
    mov %r9, %rbx
    mov %rdi, %r12

    // Stack arguments, rounded to 16 so the callee sees an aligned frame.
    lea 15(,%r8,8), %rax
    and $-16, %rax
    sub %rax, %rsp
    xor %r10, %r10
1:  cmp %r8, %r10
    jae 2f
    mov (%rcx,%r10,8), %r11
    mov %r11, (%rsp,%r10,8)
    inc %r10
    jmp 1b
2:
    movdqu 0(%rdx), %xmm0
    movdqu 16(%rdx), %xmm1
    movdqu 32(%rdx), %xmm2
    movdqu 48(%rdx), %xmm3
    movdqu 64(%rdx), %xmm4
    movdqu 80(%rdx), %xmm5
    movdqu 96(%rdx), %xmm6
    movdqu 112(%rdx), %xmm7

    mov %rsi, %r11
    mov 0(%r11), %rdi
    mov 8(%r11), %rsi
    mov 16(%r11), %rdx
    mov 24(%r11), %rcx
    mov 32(%r11), %r8
    mov 40(%r11), %r9
    // %al bounds the vector registers a variadic callee must spill. Eight is
    // always a correct upper bound and costs a callee a few stores.
    mov $8, %eax
    call *%r12

    mov %rax, 0(%rbx)
    mov %rdx, 8(%rbx)
    movdqu %xmm0, 16(%rbx)
    movdqu %xmm1, 32(%rbx)

    lea -16(%rbp), %rsp
    pop %r12
    pop %rbx
    pop %rbp
    .cfi_def_cfa %rsp, 8
    ret
    .cfi_endproc
    .size cg_host_call, .-cg_host_call

// The host-to-guest entry, for a guest function the host is going to call
// as though it were its own: a JNI native the engine registered, which
// libjnivm calls with a SysV frame. Each such function is a 16-byte slot
// in a page built once by src/entry.rs,
//
//     mov $index, %r10d ; jmp *common(%rip)
//
// and every slot lands here with its index in %r10 and the caller's SysV
// arguments untouched. This saves them into an image the Rust side reads by
// the function's own type list -- the mirror of cg_host_call, and for the
// same reason: the classification stays out of assembly.
//
//   struct entry_regs { uint64_t gpr[6]; uint8_t xmm[8][16]; uint64_t stack;
//                       uint64_t rax; uint8_t xmm0[16]; }   // 208 bytes
    .globl cg_host_entry_common
    .type cg_host_entry_common, @function
cg_host_entry_common:
    .cfi_startproc
    push %rbp
    .cfi_def_cfa_offset 16
    .cfi_offset %rbp, -16
    mov %rsp, %rbp
    .cfi_def_cfa_register %rbp
    sub $208, %rsp
    mov %rdi, 0(%rsp)
    mov %rsi, 8(%rsp)
    mov %rdx, 16(%rsp)
    mov %rcx, 24(%rsp)
    mov %r8, 32(%rsp)
    mov %r9, 40(%rsp)
    movdqu %xmm0, 48(%rsp)
    movdqu %xmm1, 64(%rsp)
    movdqu %xmm2, 80(%rsp)
    movdqu %xmm3, 96(%rsp)
    movdqu %xmm4, 112(%rsp)
    movdqu %xmm5, 128(%rsp)
    movdqu %xmm6, 144(%rsp)
    movdqu %xmm7, 160(%rsp)
    // The first stack argument, above the return address and saved %rbp.
    lea 16(%rbp), %rax
    mov %rax, 176(%rsp)
    mov %r10, %rdi
    mov %rsp, %rsi
    call cg_host_entry_dispatch
    mov 184(%rsp), %rax
    movdqu 192(%rsp), %xmm0
    leave
    .cfi_def_cfa %rsp, 8
    ret
    .cfi_endproc
    .size cg_host_entry_common, .-cg_host_entry_common

    .section .note.GNU-stack,"",@progbits
