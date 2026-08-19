// bionic's pre-API-23 `FILE __sF[3]`, translated onto the host's real streams.
//
// `crates/cordial-runtime/src/bionic/mod.rs` supplies zeroed storage for `__sF`
// so that the symbol resolves and glibc's exit-time walk of its stream list does
// not report an invalid handle. Its comment is explicit that this is only half
// the job: anything that actually writes through `&__sF[k]` needs every
// FILE*-taking function wrapped to remap the pointer onto the host's stream, and
// says that is "only worth building once something is observed using it".
//
// Something is observed using it. Three unrelated engine calls fault at address
// `0x8` inside `libc.so.6`'s `_IO_fflush` --
// `nativePostClientSettingsLoadedInitialization3`, the late-settings ordering
// mocktail itself uses, and `ILocalStorageHandlerCore.setPlatformImpl` -- which
// is what a zeroed `FILE` reaches when glibc dereferences a pointer field a few
// bytes in. See docs/analysis/flag-init.md §18 and §18.1.
//
// ## Why C++ and not Rust
//
// `fprintf` is variadic. Rust cannot define a variadic `extern "C"` function, and
// AGENTS.md records that Cordial's one previous attempt to wrap variadics
// unsafely -- `CORDIAL_TRACE=1` -- aborts the engine outright. Forwarding through
// `va_list` is ordinary C and is safe here, so the wrappers live on this side.
//
// ## What this deliberately does not do
//
// It does not emulate a `FILE`. The three legacy slots are mapped onto `stdin`,
// `stdout` and `stderr` and everything else is passed through untouched. A
// pointer that is not one of the three is not ours to interpret, and guessing at
// one would turn a visible crash into a silent wrong answer.

#include <cstdarg>
#include <cstddef>
#include <cstdio>
#include <cstdint>

extern "C" {

/// The base of the Rust-side `LEGACY_SF` array, and the stride between its
/// entries. Both come from Rust rather than being restated here: the size is
/// `sizeof(struct __sFILE)` in pre-M bionic on LP64, and two copies of that
/// constant would eventually disagree.
extern const unsigned char* cordial_legacy_sf_base(void);
extern size_t cordial_legacy_sf_stride(void);

/// Map a `FILE*` onto the host's real stream when it points into `__sF`.
///
/// Returns the argument unchanged for every other pointer. The comparison is on
/// the address falling inside the array, not on equality with a slot, because a
/// caller that computed `&__sF[1]` and a caller that took `stdout` as a macro
/// arrive at the same place by different arithmetic and both must be caught.
static FILE* translate(FILE* f) {
    const unsigned char* base = cordial_legacy_sf_base();
    if (!f || !base) {
        return f;
    }
    const auto addr = reinterpret_cast<uintptr_t>(f);
    const auto lo = reinterpret_cast<uintptr_t>(base);
    const size_t stride = cordial_legacy_sf_stride();
    if (addr < lo || addr >= lo + stride * 3) {
        return f;
    }
    switch ((addr - lo) / stride) {
        case 0: return stdin;
        case 1: return stdout;
        default: return stderr;
    }
}

// Only the FILE-taking entry points `libroblox.so` actually imports. Checked
// with `readelf --dyn-syms`; wrapping more would be surface with no caller, and
// each of these shadows the host symbol for the engine only because Cordial's
// symbol table resolves the engine's imports before the host's.

int cordial_legacy_fflush(FILE* f) { return fflush(translate(f)); }
int cordial_legacy_fclose(FILE* f) { return fclose(translate(f)); }
int cordial_legacy_fseek(FILE* f, long off, int whence) { return fseek(translate(f), off, whence); }
long cordial_legacy_ftell(FILE* f) { return ftell(translate(f)); }
int cordial_legacy_fputs(const char* s, FILE* f) { return fputs(s, translate(f)); }
int cordial_legacy_setvbuf(FILE* f, char* buf, int mode, size_t size) {
    return setvbuf(translate(f), buf, mode, size);
}
size_t cordial_legacy_fread(void* p, size_t sz, size_t n, FILE* f) {
    return fread(p, sz, n, translate(f));
}
size_t cordial_legacy_fwrite(const void* p, size_t sz, size_t n, FILE* f) {
    return fwrite(p, sz, n, translate(f));
}

/// The variadic pair, forwarded through `va_list`, which is the whole reason
/// this file is C++.
int cordial_legacy_vfprintf(FILE* f, const char* fmt, va_list ap) {
    return vfprintf(translate(f), fmt, ap);
}

int cordial_legacy_fprintf(FILE* f, const char* fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    const int r = vfprintf(translate(f), fmt, ap);
    va_end(ap);
    return r;
}

} // extern "C"
