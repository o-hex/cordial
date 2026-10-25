// A C ABI over dynarmic's A64 Jit, for crates/cordial-guest.
//
// dynarmic's interface is C++ classes with virtual callbacks, which Rust
// cannot implement directly. This file is the whole of the translation, and
// it deliberately makes no decisions: every callback that is not plain memory
// access goes straight back to Rust, which is where the stub table, the
// dispatcher and the honest-failure policy live.
//
// Memory is identity-mapped (docs/vr/dynarmic-design.md §2): a guest address
// is a host address. The Jit reaches memory through fastmem at base 0 with 64
// address bits, so the callbacks below run only for the paths fastmem does
// not take -- exclusive accesses without fastmem_exclusive_access, and the
// recompile-after-fault path.

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <exception>
#include <memory>
#include <optional>

#include <dynarmic/interface/A64/a64.h>
#include <dynarmic/interface/A64/config.h>
#include <dynarmic/interface/exclusive_monitor.h>

using Dynarmic::A64::VAddr;
using Dynarmic::A64::Vector;

extern "C" {

struct cg_jit;

struct cg_callbacks {
    void* user;
    void (*svc)(void* user, cg_jit* jit, uint32_t imm);
    void (*exception)(void* user, cg_jit* jit, uint64_t pc, uint32_t kind);
    void (*fallback)(void* user, cg_jit* jit, uint64_t pc, uint64_t count);
    void (*icache)(void* user, cg_jit* jit, uint32_t op, uint64_t va);
};

struct cg_config {
    cg_callbacks callbacks;
    uint64_t* tpidr_el0;
    const uint64_t* tpidrro_el0;
    void* monitor;
    uint64_t processor_id;
    uint8_t fastmem_exclusive;
    uint8_t ignore_global_monitor;
    uint64_t code_cache_size;
    // dynarmic OptimizationFlag bits among the Unsafe_* FP ones, or 0.
    uint32_t unsafe_fp;
};

}  // extern "C"

namespace {

template<typename T>
T load(VAddr a) {
    return *reinterpret_cast<volatile const T*>(a);
}

template<typename T>
void store(VAddr a, T v) {
    *reinterpret_cast<volatile T*>(a) = v;
}

template<typename T>
bool exclusive_store(VAddr a, T value, T expected) {
    // The global monitor's lock only serialises Jits against each other. A
    // host thread storing to the same word does not take it, so the store
    // itself has to be a real compare-and-swap for STXR's failure to mean
    // what it means on hardware.
    return __atomic_compare_exchange_n(reinterpret_cast<T*>(a), &expected, value, false,
                                       __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
}

// The last guest addresses the translator fetched code from on this thread.
// dynarmic reports an internal failure by assertion, which names neither the
// guest address nor the instruction; this is how a report can say which
// guest block was being translated when it happened.
thread_local uint64_t t_fetch_ring[64];
// Guest instructions fetched for translation, process-wide. dynarmic reads
// code only when it translates a block, so this counts the translation
// work, not the instructions executed.
std::atomic<uint64_t> g_fetches{0};
thread_local unsigned t_fetch_next;

void report_fetches() {
    std::fprintf(stderr, "cordial-guest: the translator's last code fetches on this thread, oldest first:\n");
    for (unsigned i = 0; i < 64; ++i) {
        const uint64_t a = t_fetch_ring[(t_fetch_next + i) % 64];
        if (a) {
            std::fprintf(stderr, "  %#llx: %08x\n", (unsigned long long)a,
                         *reinterpret_cast<const uint32_t*>(a));
        }
    }
}

[[noreturn]] void on_terminate() {
    report_fetches();
    std::abort();
}

struct Callbacks final : Dynarmic::A64::UserCallbacks {
    cg_callbacks cb{};
    cg_jit* self = nullptr;

    std::optional<std::uint32_t> MemoryReadCode(VAddr a) override {
        t_fetch_ring[t_fetch_next++ % 64] = a;
        g_fetches.fetch_add(1, std::memory_order_relaxed);
        return load<uint32_t>(a);
    }
    uint8_t MemoryRead8(VAddr a) override { return load<uint8_t>(a); }
    uint16_t MemoryRead16(VAddr a) override { return load<uint16_t>(a); }
    uint32_t MemoryRead32(VAddr a) override { return load<uint32_t>(a); }
    uint64_t MemoryRead64(VAddr a) override { return load<uint64_t>(a); }
    Vector MemoryRead128(VAddr a) override { return {load<uint64_t>(a), load<uint64_t>(a + 8)}; }
    void MemoryWrite8(VAddr a, uint8_t v) override { store(a, v); }
    void MemoryWrite16(VAddr a, uint16_t v) override { store(a, v); }
    void MemoryWrite32(VAddr a, uint32_t v) override { store(a, v); }
    void MemoryWrite64(VAddr a, uint64_t v) override { store(a, v); }
    void MemoryWrite128(VAddr a, Vector v) override {
        store(a, v[0]);
        store(a + 8, v[1]);
    }
    bool MemoryWriteExclusive8(VAddr a, uint8_t v, uint8_t e) override { return exclusive_store(a, v, e); }
    bool MemoryWriteExclusive16(VAddr a, uint16_t v, uint16_t e) override { return exclusive_store(a, v, e); }
    bool MemoryWriteExclusive32(VAddr a, uint32_t v, uint32_t e) override { return exclusive_store(a, v, e); }
    bool MemoryWriteExclusive64(VAddr a, uint64_t v, uint64_t e) override { return exclusive_store(a, v, e); }
    bool MemoryWriteExclusive128(VAddr a, Vector v, Vector e) override {
        unsigned __int128 value = (static_cast<unsigned __int128>(v[1]) << 64) | v[0];
        unsigned __int128 expected = (static_cast<unsigned __int128>(e[1]) << 64) | e[0];
        return __sync_bool_compare_and_swap(reinterpret_cast<unsigned __int128*>(a), expected, value);
    }

    // Host callees run under whatever MXCSR dynarmic derived from the guest's
    // FPCR, because CallSVC does not switch it back. That is left alone on
    // purpose: on Android the libc the engine calls runs under the engine's
    // own FPCR, and a rounding mode the guest set is one it expects its
    // callees to honour.
    void CallSVC(uint32_t imm) override { cb.svc(cb.user, self, imm); }
    void ExceptionRaised(VAddr pc, Dynarmic::A64::Exception e) override {
        cb.exception(cb.user, self, pc, static_cast<uint32_t>(e));
    }
    void InterpreterFallback(VAddr pc, size_t n) override { cb.fallback(cb.user, self, pc, n); }
    // IC IVAU and IC IALLU[IS]: the guest saying it changed code. The
    // translator raises these whatever the config says; the base class
    // ignores them.
    void InstructionCacheOperationRaised(Dynarmic::A64::InstructionCacheOperation op, VAddr va) override {
        cb.icache(cb.user, self, static_cast<uint32_t>(op), va);
    }

    // Cycle counting is off, so these are never asked; answering "plenty
    // left" keeps a future caller that turns it on from halting every block.
    void AddTicks(uint64_t) override {}
    uint64_t GetTicksRemaining() override { return UINT64_MAX; }
    uint64_t GetCNTPCT() override {
        timespec ts;
        clock_gettime(CLOCK_MONOTONIC, &ts);
        return static_cast<uint64_t>(ts.tv_sec) * 600000000ull + static_cast<uint64_t>(ts.tv_nsec) * 3 / 5;
    }
};

}  // namespace

extern "C" {

struct cg_jit {
    Callbacks callbacks;
    std::unique_ptr<Dynarmic::A64::Jit> jit;
};

// dynarmic reports internal failures by throwing or asserting. Neither may
// unwind into Rust, so anything that escapes stops the process here, named.
#define CG_GUARD(expr)                                                   \
    try {                                                                \
        expr;                                                            \
    } catch (const std::exception& e) {                                  \
        std::fprintf(stderr, "cordial-guest: dynarmic threw: %s\n", e.what()); \
        std::abort();                                                    \
    } catch (...) {                                                      \
        std::fprintf(stderr, "cordial-guest: dynarmic threw\n");         \
        std::abort();                                                    \
    }

void* cg_monitor_new(uint64_t processors) noexcept {
    return new Dynarmic::ExclusiveMonitor(processors);
}

void cg_monitor_free(void* m) noexcept {
    delete static_cast<Dynarmic::ExclusiveMonitor*>(m);
}

cg_jit* cg_jit_new(const cg_config* c) noexcept {
    static const bool once = (std::set_terminate(on_terminate), true);
    (void)once;
    auto* j = new cg_jit;
    j->callbacks.cb = c->callbacks;
    j->callbacks.self = j;

    Dynarmic::A64::UserConfig conf;
    conf.callbacks = &j->callbacks;
    conf.processor_id = c->processor_id;
    conf.global_monitor = static_cast<Dynarmic::ExclusiveMonitor*>(c->monitor);
    conf.tpidr_el0 = c->tpidr_el0;
    conf.tpidrro_el0 = c->tpidrro_el0;
    // Identity mapping: fastmem at base 0 over the full 64 bits, so the
    // emitted access is [r13 + vaddr] with r13 = 0 and no range check.
    conf.fastmem_pointer = 0;
    conf.fastmem_address_space_bits = 64;
    conf.silently_mirror_fastmem = false;
    conf.fastmem_exclusive_access = c->fastmem_exclusive != 0;
    if (c->ignore_global_monitor) {
        conf.unsafe_optimizations = true;
        conf.optimizations |= Dynarmic::OptimizationFlag::Unsafe_IgnoreGlobalMonitor;
    }
    if (c->unsafe_fp) {
        conf.unsafe_optimizations = true;
        conf.optimizations |= static_cast<Dynarmic::OptimizationFlag>(c->unsafe_fp);
    }
    conf.enable_cycle_counting = false;
    conf.wall_clock_cntpct = true;
    if (c->code_cache_size)
        conf.code_cache_size = c->code_cache_size;
    CG_GUARD(j->jit = std::make_unique<Dynarmic::A64::Jit>(conf));
    return j;
}

void cg_jit_free(cg_jit* j) noexcept { delete j; }

uint32_t cg_jit_run(cg_jit* j) noexcept {
    Dynarmic::HaltReason hr{};
    CG_GUARD(hr = j->jit->Run());
    return static_cast<uint32_t>(hr);
}

void cg_jit_halt(cg_jit* j, uint32_t reason) noexcept {
    j->jit->HaltExecution(static_cast<Dynarmic::HaltReason>(reason));
}

void cg_jit_clear_halt(cg_jit* j, uint32_t reason) noexcept {
    j->jit->ClearHalt(static_cast<Dynarmic::HaltReason>(reason));
}

uint64_t cg_jit_get_x(const cg_jit* j, uint32_t i) noexcept { return j->jit->GetRegister(i); }
void cg_jit_set_x(cg_jit* j, uint32_t i, uint64_t v) noexcept { j->jit->SetRegister(i, v); }
uint64_t cg_jit_get_sp(const cg_jit* j) noexcept { return j->jit->GetSP(); }
void cg_jit_set_sp(cg_jit* j, uint64_t v) noexcept { j->jit->SetSP(v); }
uint64_t cg_jit_get_pc(const cg_jit* j) noexcept { return j->jit->GetPC(); }
void cg_jit_set_pc(cg_jit* j, uint64_t v) noexcept { j->jit->SetPC(v); }

void cg_jit_get_v(const cg_jit* j, uint32_t i, uint64_t out[2]) noexcept {
    const Vector v = j->jit->GetVector(i);
    out[0] = v[0];
    out[1] = v[1];
}

void cg_jit_set_v(cg_jit* j, uint32_t i, const uint64_t in[2]) noexcept {
    j->jit->SetVector(i, Vector{in[0], in[1]});
}

uint32_t cg_jit_get_fpcr(const cg_jit* j) noexcept { return j->jit->GetFpcr(); }
void cg_jit_set_fpcr(cg_jit* j, uint32_t v) noexcept { j->jit->SetFpcr(v); }
uint32_t cg_jit_get_pstate(const cg_jit* j) noexcept { return j->jit->GetPstate(); }
void cg_jit_set_pstate(cg_jit* j, uint32_t v) noexcept { j->jit->SetPstate(v); }

void cg_jit_invalidate(cg_jit* j, uint64_t start, uint64_t len) noexcept {
    CG_GUARD(j->jit->InvalidateCacheRange(start, len));
}

void cg_jit_clear_exclusive(cg_jit* j) noexcept { j->jit->ClearExclusiveState(); }

uint64_t cg_translated_instructions() noexcept { return g_fetches.load(std::memory_order_relaxed); }

// Every guest-to-host call goes through here. A C++ exception from the
// callee -- libjnivm reports misuse by throwing -- would otherwise unwind
// into the translator's emitted code, which has no unwind tables, and end in
// std::terminate with no word about which call threw. cg_host_call carries
// CFI, so the unwinder reaches this frame through it.
uint64_t cg_host_call(const void* f, const uint64_t gpr[6], const uint64_t xmm[8][2],
                      const uint64_t* stack, uint64_t nstack, void* out);

// The guest's long double is IEEE binary128, the host's x87 extended. The
// two share an exponent range, so widening the host's to binary128 is exact;
// what is lost is the 49 mantissa bits the host never had. Clang lowers the
// conversion through compiler-rt's __extendxftf2.
void cg_strtold_quad(const char* s, char** end, uint64_t out[2]) noexcept {
    const __float128 q = static_cast<__float128>(std::strtold(s, end));
    static_assert(sizeof(q) == 16);
    std::memcpy(out, &q, 16);
}

int cg_host_call_guarded(const void* f, const uint64_t gpr[6], const uint64_t xmm[8][2],
                         const uint64_t* stack, uint64_t nstack, void* out,
                         char* err, size_t err_len) noexcept {
    try {
        cg_host_call(f, gpr, xmm, stack, nstack, out);
        return 0;
    } catch (const std::exception& e) {
        std::snprintf(err, err_len, "%s", e.what());
        return 1;
    } catch (...) {
        std::snprintf(err, err_len, "a C++ exception not derived from std::exception");
        return 2;
    }
}

}  // extern "C"
