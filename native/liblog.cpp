// Android's liblog, implemented against the host's stderr.
//
// This is the single most valuable thing in the runtime layer to get working
// early. Roblox narrates its own startup through it, so every later failure
// comes with the client's own account of what it was doing — which is otherwise
// unavailable, since nothing else in the process can say anything.
//
// It is also the source of `onJoin`, `onLeave` and `onLogLine` in the plugin
// event schema: those are parsed from exactly this stream.
//
// Written in C++ rather than Rust because three of the six entry points are
// variadic, and forwarding a C variadic to a real `vsnprintf` is the one thing
// C does better here.

#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstddef>

extern "C" {

// android/log.h priorities.
enum {
    ANDROID_LOG_UNKNOWN = 0,
    ANDROID_LOG_DEFAULT,
    ANDROID_LOG_VERBOSE,
    ANDROID_LOG_DEBUG,
    ANDROID_LOG_INFO,
    ANDROID_LOG_WARN,
    ANDROID_LOG_ERROR,
    ANDROID_LOG_FATAL,
    ANDROID_LOG_SILENT,
};

} // extern "C"

namespace {

char priority_letter(int prio) {
    switch (prio) {
        case ANDROID_LOG_VERBOSE: return 'V';
        case ANDROID_LOG_DEBUG:   return 'D';
        case ANDROID_LOG_INFO:    return 'I';
        case ANDROID_LOG_WARN:    return 'W';
        case ANDROID_LOG_ERROR:   return 'E';
        case ANDROID_LOG_FATAL:   return 'F';
        case ANDROID_LOG_SILENT:  return 'S';
        default:                  return '?';
    }
}

/// Minimum priority actually printed.
///
/// **The default was INFO, and that cost a whole investigation.** The comment
/// here used to say Roblox is extremely chatty at VERBOSE and DEBUG and that
/// INFO "keeps the useful half". The chattiness claim is wrong for DEBUG:
/// measured on a landing-page run, INFO prints 872 lines and DEBUG prints 960
/// — eighty-seven more, not a flood. What those eighty-seven contained was
/// `nativeInitializeNativeFlags: Registered Flag Provider ID from Java: 0` and
/// `flagCount = 139`, the two lines that tell you the flag provider registered
/// exactly as the real client's Waydroid capture shows it registering.
///
/// Because they were hidden, their absence was read as Cordial failing to
/// register a flag provider at all, and `docs/analysis/flag-init.md` §41 had to
/// warn in writing against building on that absence before someone did. This
/// project has mistaken an absence for evidence nine times; a logger whose
/// default silently drops a whole severity is a machine for producing the tenth.
///
/// VERBOSE is still off by default, and that one is genuinely chatty. Anything
/// hidden by default should be cheap to reveal and loudly documented, which is
/// what `CORDIAL_LOG_LEVEL` is for.
int minimum_priority() {
    static const int level = [] {
        const char* v = getenv("CORDIAL_LOG_LEVEL");
        if (!v) return (int)ANDROID_LOG_DEBUG;
        switch (v[0]) {
            case 'v': case 'V': return (int)ANDROID_LOG_VERBOSE;
            case 'd': case 'D': return (int)ANDROID_LOG_DEBUG;
            case 'i': case 'I': return (int)ANDROID_LOG_INFO;
            case 'w': case 'W': return (int)ANDROID_LOG_WARN;
            case 'e': case 'E': return (int)ANDROID_LOG_ERROR;
            default:            return (int)ANDROID_LOG_INFO;
        }
    }();
    return level;
}

void emit(int prio, const char* tag, const char* text) {
    if (prio < minimum_priority()) {
        return;
    }
    fprintf(stderr, "%c/%-24s %s\n", priority_letter(prio), tag ? tag : "(no tag)",
            text ? text : "(null)");
}

int __android_log_write(int prio, const char* tag, const char* text) {
    emit(prio, tag, text);
    return 1;
}

int __android_log_buf_write(int /*bufID*/, int prio, const char* tag, const char* text) {
    // Android routes to main/radio/system/crash buffers. One stream is fine here;
    // nothing downstream distinguishes them.
    emit(prio, tag, text);
    return 1;
}

int __android_log_vprint(int prio, const char* tag, const char* fmt, va_list ap) {
    char buf[4096];
    vsnprintf(buf, sizeof(buf), fmt, ap);
    emit(prio, tag, buf);
    return 1;
}

int __android_log_print(int prio, const char* tag, const char* fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int rc = __android_log_vprint(prio, tag, fmt, ap);
    va_end(ap);
    return rc;
}

/// bionic's fatal-assertion path. It does not return, and it must not: as a stub
/// returning 0 a failed assertion continues with state its author already
/// declared invalid, and the eventual crash points somewhere unrelated.
void __android_log_assert(const char* cond, const char* tag, const char* fmt, ...) {
    char buf[4096];
    if (fmt) {
        va_list ap;
        va_start(ap, fmt);
        vsnprintf(buf, sizeof(buf), fmt, ap);
        va_end(ap);
    } else {
        snprintf(buf, sizeof(buf), "assertion failed: %s", cond ? cond : "(unknown)");
    }
    fprintf(stderr, "\n*** Roblox fatal assertion ***\n");
    fprintf(stderr, "    tag:       %s\n", tag ? tag : "(no tag)");
    fprintf(stderr, "    condition: %s\n", cond ? cond : "(none)");
    fprintf(stderr, "    message:   %s\n", buf);
    abort();
}

/// Where bionic stashes a message for the crash reporter to pick up. There is no
/// crash reporter here, so printing it is strictly more useful.
void android_set_abort_message(const char* msg) {
    fprintf(stderr, "[abort-message] %s\n", msg ? msg : "(null)");
}

} // namespace

/// Table of everything above, for the Rust symbol table to install.
///
/// The functions have internal linkage on purpose. Cordial links the AOSP bionic
/// linker into the same binary and it defines `__android_log_write` and
/// `android_set_abort_message` for its own logging; exporting a second copy is a
/// duplicate-symbol error. Roblox never resolves these by name through the host
/// linker anyway — they reach it through this table and Cordial's virtual
/// `liblog.so`, which is the only path that matters.
extern "C" struct CordialSymbol {
    const char* name;
    void* addr;
};

extern "C" const CordialSymbol* cordial_liblog_symbols(size_t* count) {
    static const CordialSymbol table[] = {
        {"__android_log_write", (void*)&__android_log_write},
        {"__android_log_buf_write", (void*)&__android_log_buf_write},
        {"__android_log_print", (void*)&__android_log_print},
        {"__android_log_vprint", (void*)&__android_log_vprint},
        {"__android_log_assert", (void*)&__android_log_assert},
        {"android_set_abort_message", (void*)&android_set_abort_message},
    };
    *count = sizeof(table) / sizeof(table[0]);
    return table;
}
