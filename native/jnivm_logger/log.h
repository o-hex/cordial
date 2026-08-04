// The logger libjnivm already knows how to use.
//
// `third_party/libjnivm/src/jnivm/internal/log.h` is written as
//
//     #ifdef HAVE_LOGGER
//     #include <log.h>
//     #define LOG(...) Log::debug(__VA_ARGS__)
//     #else
//     #define LOG(tag, format, ...) printf(...)
//     #endif
//
// and its CMakeLists defines `HAVE_LOGGER` and links whatever it finds when a
// target called `logger` exists. So this is the seam the library already
// offers, and **nothing in `third_party/` is patched to use it** — a vendored
// dependency with local edits is a dependency nobody dares update, and the JNI
// surface is the part of this project most likely to need updating.
//
// What it buys: libjnivm's `Constructed Unresolved symbol` line is the only
// notice Cordial gets that the engine asked for a Java class, method or field
// nobody has written. It used to go to stdout and nowhere else, interleaved
// with the engine's own narration. Now it also goes to
// `crate::unimplemented`, so the end-of-run report can say what the client
// asked for and did not get.

#pragma once

namespace Log {
// Same shape as `printf`, because that is what the macro expands to and every
// call site in libjnivm was written against it.
void debug(const char* tag, const char* format, ...)
    __attribute__((format(printf, 2, 3)));
}  // namespace Log
