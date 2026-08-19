// extern "C" surface over mcpelauncher-linker's C++ API.
//
// The linker's own interface takes std::unordered_map<std::string, void*>, which
// Rust cannot construct. Everything here is a translation of that, and nothing
// else — no policy, no state. Policy lives in Rust.

#include <mcpelauncher/linker.h>

#include <chrono>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <unordered_map>

extern "C" {

void cordial_linker_init() {
    linker::init();
}

// names/addrs are parallel arrays of length n. Returns a library handle, or null.
void* cordial_linker_load_library(const char* name, const char* const* names,
                                  void* const* addrs, size_t n) {
    std::unordered_map<std::string, void*> symbols;
    symbols.reserve(n);
    for (size_t i = 0; i < n; i++) {
        symbols.emplace(names[i], addrs[i]);
    }
    return linker::load_library(name, symbols);
}

void cordial_linker_update_ld_library_path(const char* path) {
    linker::update_LD_LIBRARY_PATH(path);
}

void* cordial_linker_dlopen(const char* filename, int flags) {
    // `CORDIAL_TRACE_DLOPEN=1` reports every request and how long it took.
    //
    // Roblox reaches several subsystems this way rather than through DT_NEEDED
    // — Vulkan is the known one — so this is the only place that shows which
    // optional backends it actually looks for, and whether a miss fails
    // promptly. FMOD's Android output prefers AAudio and falls back to OpenSL
    // ES; if that fallback depends on `dlopen("libaaudio.so")` failing fast,
    // a slow or hanging miss would be a real bug rather than a cosmetic one.
    static const bool trace = getenv("CORDIAL_TRACE_DLOPEN") != nullptr;
    if (!trace) {
        return linker::dlopen(filename, flags);
    }
    auto start = std::chrono::steady_clock::now();
    void* h = linker::dlopen(filename, flags);
    auto us = std::chrono::duration_cast<std::chrono::microseconds>(
                  std::chrono::steady_clock::now() - start)
                  .count();
    fprintf(stderr, "[cordial] dlopen(%s) -> %s in %lldus\n", filename ? filename : "(null)",
            h ? "ok" : "NULL", (long long)us);
    return h;
}

// EXPERIMENTAL, cordial-agent-defer: cordial-agent-defer's split-phase
// dlopen. Declared here rather than added to mcpelauncher's own public
// header (`public_include/mcpelauncher/linker.h`) because that header is
// shared surface and this pair exists only to test whether deferring
// libroblox.so's ELF constructors past Cordial's own directory setup is
// coherent — see docs/analysis/flag-init.md §26 and patches/README.md for
// the patch this corresponds to once (if) it earns a permanent home.
extern "C" void mcpelauncher_defer_next_ctors(int defer);
extern "C" void mcpelauncher_run_deferred_ctors(void* handle);

void cordial_linker_defer_next_ctors(int defer) {
    mcpelauncher_defer_next_ctors(defer);
}

void cordial_linker_run_deferred_ctors(void* handle) {
    mcpelauncher_run_deferred_ctors(handle);
}

void* cordial_linker_dlsym(void* handle, const char* symbol) {
    return linker::dlsym(handle, symbol);
}

const char* cordial_linker_dlerror() {
    return linker::dlerror();
}

size_t cordial_linker_get_library_base(void* handle) {
    return linker::get_library_base(handle);
}

void cordial_linker_get_library_code_region(void* handle, size_t* base, size_t* size) {
    size_t b = 0, s = 0;
    linker::get_library_code_region(handle, b, s);
    *base = b;
    *size = s;
}

} // extern "C"
