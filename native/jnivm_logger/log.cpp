#include "log.h"

#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>

// `crate::unimplemented`'s C entry point. The codes match `Kind::from_code`.
extern "C" void cordial_unimplemented_record(unsigned int kind, const char* detail);

namespace {

constexpr unsigned int KIND_JNI = 0;

// The exact string libjnivm formats when it invents a stub for something it was
// asked for and does not have. Both `field.cpp` and `method.cpp` use it, which
// is why matching the message is enough and no per-call-site hook is needed.
//
// Matching on a message is ordinarily a poor idea and it is the right trade
// here: the alternative is editing two files in `third_party/` and owning that
// diff forever. If libjnivm ever rewords this, the report goes quiet rather
// than wrong — and `the_marker_is_the_one_libjnivm_actually_prints` in
// `unresolved_marker.rs` is the test that notices.
constexpr const char* MARKER = "Constructed Unresolved symbol";

// libjnivm is not quiet, and `JNIVM_ENABLE_TRACE` makes it very loud. Passing
// everything through keeps the behaviour the `#else` branch had, so turning the
// logger on is not also a behaviour change nobody asked for.
bool quiet() {
    static const bool on = std::getenv("CORDIAL_JNI_QUIET") != nullptr;
    return on;
}

}  // namespace

namespace Log {

void debug(const char* tag, const char* format, ...) {
    char line[1024];

    va_list args;
    va_start(args, format);
    const int written = vsnprintf(line, sizeof(line), format, args);
    va_end(args);

    if (written < 0) {
        return;
    }

    if (!quiet()) {
        printf("[%s]: %s\n", tag ? tag : "JNIVM", line);
    }

    // The whole reason this file exists. Everything else here is preserving
    // what the default macro already did.
    if (strstr(line, MARKER) != nullptr) {
        cordial_unimplemented_record(KIND_JNI, line);
    }
}

}  // namespace Log
