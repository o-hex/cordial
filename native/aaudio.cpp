// AAudio, backed by PipeWire — Android's callback-driven audio output, which
// is the one FMOD prefers on any modern Android and the one Roblox therefore
// asks for first.
//
// Why this exists rather than leaving well alone. Audio currently reaches
// PipeWire through FMOD's *Java* fallback, `org/fmod/AudioDevice`, over JNI
// and jnivm: the engine hands `write([BI)V` a `jbyteArray` per block and
// `native/audio_classes.cpp` copies it into `PlaybackStream`'s pending queue.
// That layer is where the freeze fixed in `c7215eb` lived — an AB-BA deadlock
// between `AudioDevice::close`, holding its own `std::mutex` while waiting on
// PipeWire's thread-loop lock, and PipeWire's own thread, holding that lock
// while waiting on the same mutex inside `AudioDevice::drained`. It took two
// days to find because the symptom was a client that looked wedged in its
// renderer.
//
// AAudio's data callback is the same shape as PipeWire's `process()`: "here
// is a buffer, fill it, you are on a realtime thread". Bridging them deletes
// the queue, the copy, the JNI hop and the mutex rather than making any of
// them safe, so that class of bug becomes structurally impossible instead of
// avoided by discipline. `CallbackStream` in `pipewire_backend.cpp` is the
// half of it that speaks PipeWire; this file is the half that speaks Android.
//
// **Once FMOD has been told AAudio exists, there is no way back.** This is
// the most consequential thing measured while writing this file, and it is
// the reverse of what the plan assumed. A control run
// (`CORDIAL_AUDIO=aaudio-refuse`) answered `supportsAAudio()` true and then
// reported `AAUDIO_ERROR_UNAVAILABLE` from every `openStream`. FMOD tried
// twice and then abandoned audio altogether: no `AudioDevice.init`, no
// `slCreateEngine`, no PipeWire node of any kind for the rest of the run, and
// a place that loaded and played in silence. The "AAudio, then OpenSL ES,
// then Java" chain does not exist on this build once the first link has been
// claimed. That is why `supportsAAudio()` checks `pipewire_available()`
// before saying yes, and why this whole path stays behind a switch that is
// off by default.
//
// **What the engine actually asks for is measured, not assumed.**
// `docs/analysis/aaudio-contract.md` lists the 25 `AAudio*` names present in
// `libroblox.so` 2.734.0.917, and the absences shaped this file more than the
// presences did:
//
//   * There is no `AAudioStream_write`, only `_read`. Playback is
//     callback-driven and nothing else, so there is no write path here to go
//     stale.
//   * There is no `setSampleRate` and no `setChannelCount`. The engine opens
//     a stream and then reads `getSampleRate`/`getChannelCount`/`getFormat`
//     back off it. So `CallbackStream::open` constrains neither, PipeWire
//     picks whatever the session is already running at, and these getters
//     report that. Nothing resamples at either end.
//   * There is no `setDeviceId`, so nothing on this path can select an
//     output. Roblox's own device picker is fed by `AudioManager.getDevices`
//     in `audio_classes.cpp` and is a separate job.
//
// Every type, constant and prototype below is transcribed from AOSP's public
// `media/libaaudio/include/aaudio/AAudio.h` (Apache-2.0,
// android.googlesource.com), the same provenance and the same reason as the
// Khronos and AOSP headers `native/opensles.cpp` names at its own top: an
// ABI recalled from memory is an ABI that silently plays noise. The header is
// not vendored — only the handful of values this file uses are written out,
// so the tree gains no new build-time dependency.
//
// **Input is refused, on purpose.** `AAudioStreamBuilder_setInputPreset` and
// `AAudioStream_read` are both in the engine's list, so an AAudio recorder is
// a path this build has. It is not implemented here and `openStream` reports
// `AAUDIO_ERROR_UNIMPLEMENTED` for `AAUDIO_DIRECTION_INPUT`, because the
// privacy rule the audio code is written around — a microphone that exists
// for exactly as long as the engine asked to record, checkable from outside
// with `pw-dump` — needs the same care `CaptureStream` was given and does not
// get it for free from a class designed to be pulled forever. An honest
// refusal leaves that gap where somebody can find it; see AGENTS.md on stubs
// that lie.

#include "aaudio.h"
#include "pipewire_backend.h"

#include <pthread.h>

#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>

namespace {

// ------------------------------------------------------------ the AAudio ABI
//
// From AOSP's AAudio.h. The enumerators that matter are spelled out with
// their values rather than left to sequence position, because a wrong number
// here is not a compile error anywhere — it is audio that is quietly the
// wrong format, or a result code the engine reads as success.

using aaudio_result_t = int32_t;
using aaudio_stream_state_t = int32_t;
using aaudio_direction_t = int32_t;
using aaudio_format_t = int32_t;
using aaudio_data_callback_result_t = int32_t;
using aaudio_performance_mode_t = int32_t;
using aaudio_usage_t = int32_t;
using aaudio_input_preset_t = int32_t;

constexpr aaudio_result_t AAUDIO_OK = 0;
constexpr aaudio_result_t AAUDIO_ERROR_ILLEGAL_ARGUMENT = -898;
constexpr aaudio_result_t AAUDIO_ERROR_INVALID_STATE = -895;
constexpr aaudio_result_t AAUDIO_ERROR_UNIMPLEMENTED = -890;
constexpr aaudio_result_t AAUDIO_ERROR_UNAVAILABLE = -889;
constexpr aaudio_result_t AAUDIO_ERROR_NO_MEMORY = -887;
constexpr aaudio_result_t AAUDIO_ERROR_NULL = -886;

constexpr aaudio_direction_t AAUDIO_DIRECTION_OUTPUT = 0;
constexpr aaudio_direction_t AAUDIO_DIRECTION_INPUT = 1;

constexpr aaudio_format_t AAUDIO_FORMAT_INVALID = -1;
constexpr aaudio_format_t AAUDIO_FORMAT_UNSPECIFIED = 0;
constexpr aaudio_format_t AAUDIO_FORMAT_PCM_I16 = 1;
constexpr aaudio_format_t AAUDIO_FORMAT_PCM_FLOAT = 2;
constexpr aaudio_format_t AAUDIO_FORMAT_PCM_I24_PACKED = 3;
constexpr aaudio_format_t AAUDIO_FORMAT_PCM_I32 = 4;

constexpr aaudio_stream_state_t AAUDIO_STREAM_STATE_OPEN = 2;
constexpr aaudio_stream_state_t AAUDIO_STREAM_STATE_STARTED = 4;
constexpr aaudio_stream_state_t AAUDIO_STREAM_STATE_PAUSED = 6;
constexpr aaudio_stream_state_t AAUDIO_STREAM_STATE_STOPPED = 10;
constexpr aaudio_stream_state_t AAUDIO_STREAM_STATE_CLOSED = 12;

constexpr aaudio_data_callback_result_t AAUDIO_CALLBACK_RESULT_CONTINUE = 0;

struct AAudioStreamBuilder;
struct AAudioStream;

using AAudioStream_dataCallback = aaudio_data_callback_result_t (*)(AAudioStream* stream,
                                                                     void* userData,
                                                                     void* audioData,
                                                                     int32_t numFrames);
using AAudioStream_errorCallback = void (*)(AAudioStream* stream, void* userData,
                                             aaudio_result_t error);

// ------------------------------------------------------------ backend choice

/// What `CORDIAL_AUDIO` selected, decided once and logged once.
///
/// The variable did not exist before this file. It was described in
/// conversation as the intended design, then tried on a live run where
/// nothing read it — which looks exactly like a feature that did nothing, and
/// is why the selection is announced at startup rather than left to be
/// inferred from whether sound comes out.
enum class Backend {
    /// FMOD's Java `org.fmod.AudioDevice` path. The default, and it stays the
    /// default until AAudio has been measured against it — the same rule
    /// `Performance::Balanced` follows in `crates/cordial-runtime/src/flags.rs`.
    Java,
    /// AAudio over PipeWire: this file, wired all the way through.
    AAudio,
    /// AAudio resolvable, every `openStream` honestly refused. **This is the
    /// control**, and it is the reason the switch has three values rather
    /// than two: it answers "does FMOD prefer AAudio once it can see it, and
    /// what does it do when a stream will not open" in the same build as the
    /// working path, so the difference between them is one environment
    /// variable rather than two binaries.
    AAudioRefuse,
};

Backend parse_backend(const char* value) {
    if (!value || value[0] == '\0') return Backend::Java;
    if (std::strcmp(value, "aaudio") == 0) return Backend::AAudio;
    if (std::strcmp(value, "aaudio-refuse") == 0) return Backend::AAudioRefuse;
    if (std::strcmp(value, "java") == 0) return Backend::Java;
    std::fprintf(stderr,
        "W/Cordial-AAudio          CORDIAL_AUDIO=%s is not a backend I know "
        "(java, aaudio, aaudio-refuse); using java.\n", value);
    return Backend::Java;
}

Backend selected_backend() {
    static const Backend backend = [] {
        Backend b = parse_backend(std::getenv("CORDIAL_AUDIO"));
        const char* name = b == Backend::AAudio          ? "aaudio"
                            : b == Backend::AAudioRefuse ? "aaudio-refuse"
                                                          : "java";
        std::fprintf(stderr,
            "I/Cordial-AAudio          audio backend: %s (CORDIAL_AUDIO=%s). %s\n", name,
            std::getenv("CORDIAL_AUDIO") ? std::getenv("CORDIAL_AUDIO") : "unset",
            b == Backend::Java
                ? "libaaudio.so is not registered and org.fmod.FMOD.supportsAAudio() reports "
                  "false, so FMOD takes its Java AudioDevice path."
                : b == Backend::AAudio
                      ? "libaaudio.so is registered and supportsAAudio() reports true; streams "
                        "open against PipeWire."
                      : "CONTROL RUN: libaaudio.so is registered and supportsAAudio() reports "
                        "true, but every openStream is refused with AAUDIO_ERROR_UNAVAILABLE.");
        return b;
    }();
    return backend;
}

// ------------------------------------------------------------------ objects

struct Builder {
    aaudio_direction_t direction = AAUDIO_DIRECTION_OUTPUT;
    aaudio_format_t format = AAUDIO_FORMAT_UNSPECIFIED;
    int32_t buffer_capacity = 0;
    aaudio_performance_mode_t performance_mode = 0;
    aaudio_usage_t usage = 0;
    aaudio_input_preset_t input_preset = 0;
    AAudioStream_dataCallback data_callback = nullptr;
    void* data_user = nullptr;
    AAudioStream_errorCallback error_callback = nullptr;
    void* error_user = nullptr;
};

struct Stream {
    cordial::audio::CallbackStream pw;

    AAudioStream_dataCallback data_callback = nullptr;
    void* data_user = nullptr;
    AAudioStream_errorCallback error_callback = nullptr;
    void* error_user = nullptr;

    aaudio_format_t format = AAUDIO_FORMAT_UNSPECIFIED;
    std::atomic<aaudio_stream_state_t> state{AAUDIO_STREAM_STATE_OPEN};

    /// The thread PipeWire last ran the fill callback on, recorded so that a
    /// `close` reaching this file from inside that callback can be refused
    /// instead of deadlocking on the loop lock. AAudio's own documentation
    /// forbids it ("another thread should be used to stop and close the
    /// stream"), and a well-behaved FMOD will never do it — but a hang is a
    /// far worse way to find out than an error code and a line of log.
    std::atomic<unsigned long> callback_thread{0};

    /// Reported by `AAudioStream_getXRunCount`. See the comment there: this
    /// counts what Cordial can honestly see, which is not what Android counts.
    std::atomic<int32_t> xruns{0};

    // CORDIAL_TRACE_AUDIO=1 bookkeeping only. Touched from the fill callback
    // and nowhere else, so plain members rather than atomics: AAudio's own
    // contract is that the data callback is never entered from two threads at
    // once, and PipeWire honours it here by running `process` on one loop.
    uint64_t trace_cycles = 0;
    uint64_t trace_frames = 0;
    float trace_peak = 0.0f;
    std::chrono::steady_clock::time_point trace_last{};
};

/// Translates one `aaudio_format_t` into the bits/float pair
/// `CallbackStream::open` speaks. Returns false for anything this bridge
/// cannot carry, which the caller reports as a refused open.
bool format_to_bits(aaudio_format_t format, uint32_t& bits, bool& is_float) {
    switch (format) {
    case AAUDIO_FORMAT_UNSPECIFIED:
        // No preference: `CallbackStream::open` reads zero bits as "offer
        // PipeWire's own float", which converts nothing anywhere.
        bits = 0;
        is_float = false;
        return true;
    case AAUDIO_FORMAT_PCM_I16: bits = 16; is_float = false; return true;
    case AAUDIO_FORMAT_PCM_FLOAT: bits = 32; is_float = true; return true;
    case AAUDIO_FORMAT_PCM_I24_PACKED: bits = 24; is_float = false; return true;
    case AAUDIO_FORMAT_PCM_I32: bits = 32; is_float = false; return true;
    default: return false;
    }
}

aaudio_format_t bits_to_format(uint32_t bits, bool is_float) {
    if (is_float) return bits == 32 ? AAUDIO_FORMAT_PCM_FLOAT : AAUDIO_FORMAT_INVALID;
    switch (bits) {
    case 16: return AAUDIO_FORMAT_PCM_I16;
    case 24: return AAUDIO_FORMAT_PCM_I24_PACKED;
    case 32: return AAUDIO_FORMAT_PCM_I32;
    default: return AAUDIO_FORMAT_INVALID;
    }
}

const char* format_name(aaudio_format_t f) {
    switch (f) {
    case AAUDIO_FORMAT_UNSPECIFIED: return "UNSPECIFIED";
    case AAUDIO_FORMAT_PCM_I16: return "PCM_I16";
    case AAUDIO_FORMAT_PCM_FLOAT: return "PCM_FLOAT";
    case AAUDIO_FORMAT_PCM_I24_PACKED: return "PCM_I24_PACKED";
    case AAUDIO_FORMAT_PCM_I32: return "PCM_I32";
    default: return "?";
    }
}

/// `CORDIAL_TRACE_AUDIO=1` only. Off, this is one relaxed bool load per cycle.
bool trace_audio_enabled() {
    static const bool enabled = std::getenv("CORDIAL_TRACE_AUDIO") != nullptr;
    return enabled;
}

/// Largest absolute sample in a freshly filled buffer, as a fraction of full
/// scale.
///
/// This exists because "a game played sound" is otherwise not something this
/// code can honestly claim. A connected PipeWire node proves a stream was
/// opened; `pw-top` proves it is being driven; neither distinguishes sound from a
/// perfectly punctual river of zeroes, which is exactly what a bridge that
/// negotiated the wrong format or handed the engine the wrong frame count
/// would produce. A peak is the cheapest reading that tells those apart.
float buffer_peak(const void* data, uint32_t frames, uint32_t channels, uint32_t bits,
                  bool is_float) {
    const size_t samples = static_cast<size_t>(frames) * channels;
    float peak = 0.0f;
    if (is_float && bits == 32) {
        const auto* p = static_cast<const float*>(data);
        for (size_t i = 0; i < samples; ++i) {
            float v = p[i] < 0.0f ? -p[i] : p[i];
            if (v > peak) peak = v;
        }
    } else if (!is_float && bits == 16) {
        const auto* p = static_cast<const int16_t*>(data);
        for (size_t i = 0; i < samples; ++i) {
            int v = p[i] < 0 ? -static_cast<int>(p[i]) : p[i];
            if (static_cast<float>(v) > peak) peak = static_cast<float>(v);
        }
        peak /= 32768.0f;
    } else if (!is_float && bits == 32) {
        const auto* p = static_cast<const int32_t*>(data);
        for (size_t i = 0; i < samples; ++i) {
            int64_t v = p[i] < 0 ? -static_cast<int64_t>(p[i]) : p[i];
            float f = static_cast<float>(v);
            if (f > peak) peak = f;
        }
        peak /= 2147483648.0f;
    }
    return peak;
}

/// PipeWire's pull, turned into AAudio's push-to-the-app.
///
/// **Realtime.** With the trace off this is two relaxed atomic stores and the
/// engine's own callback: no lock, no allocation, no log. With
/// `CORDIAL_TRACE_AUDIO=1` it also scans the buffer for a peak and prints a
/// line a second, which breaks that rule deliberately and only when asked —
/// the same bargain `PlaybackStream::process` already strikes with the same
/// variable, and the reason it is a switch rather than a default.
bool fill_from_engine(void* dst, uint32_t frames, void* user) {
    auto* s = static_cast<Stream*>(user);
    s->callback_thread.store(static_cast<unsigned long>(pthread_self()),
                             std::memory_order_relaxed);
    auto cb = s->data_callback;
    if (!cb) return false;
    const bool cont = cb(reinterpret_cast<AAudioStream*>(s), s->data_user, dst,
                          static_cast<int32_t>(frames)) == AAUDIO_CALLBACK_RESULT_CONTINUE;

    if (trace_audio_enabled()) {
        ++s->trace_cycles;
        s->trace_frames += frames;
        float peak = buffer_peak(dst, frames, s->pw.channels(), s->pw.sample_bits(),
                                  s->pw.sample_is_float());
        if (peak > s->trace_peak) s->trace_peak = peak;
        auto now = std::chrono::steady_clock::now();
        if (now - s->trace_last >= std::chrono::seconds(1)) {
            s->trace_last = now;
            std::fprintf(stderr,
                "D/Cordial-AAudio          audio trace: %llu callback(s), %llu frame(s), peak "
                "%.4f of full scale since the last line\n",
                static_cast<unsigned long long>(s->trace_cycles),
                static_cast<unsigned long long>(s->trace_frames), s->trace_peak);
            s->trace_peak = 0.0f;
        }
    }
    return cont;
}

bool on_callback_thread(Stream* s) {
    unsigned long t = s->callback_thread.load(std::memory_order_relaxed);
    return t != 0 && t == static_cast<unsigned long>(pthread_self());
}

} // namespace

// --------------------------------------------------------------- entry points
//
// Exported through `cordial_aaudio_symbols` below rather than as real ELF
// exports: nothing on the host links `libaaudio.so`, the guest reaches it
// through the bionic linker's virtual-library table, and keeping these
// internal means the host's own dynamic symbol table gains no `AAudio*` names
// that could shadow anything.

extern "C" {

static aaudio_result_t AAudio_createStreamBuilder(AAudioStreamBuilder** builder) {
    if (!builder) return AAUDIO_ERROR_NULL;
    auto* b = new (std::nothrow) Builder();
    if (!b) return AAUDIO_ERROR_NO_MEMORY;
    *builder = reinterpret_cast<AAudioStreamBuilder*>(b);
    return AAUDIO_OK;
}

static aaudio_result_t AAudioStreamBuilder_delete(AAudioStreamBuilder* builder) {
    if (!builder) return AAUDIO_ERROR_NULL;
    delete reinterpret_cast<Builder*>(builder);
    return AAUDIO_OK;
}

static void AAudioStreamBuilder_setDirection(AAudioStreamBuilder* builder,
                                              aaudio_direction_t direction) {
    if (builder) reinterpret_cast<Builder*>(builder)->direction = direction;
}

static void AAudioStreamBuilder_setFormat(AAudioStreamBuilder* builder, aaudio_format_t format) {
    if (builder) reinterpret_cast<Builder*>(builder)->format = format;
}

static void AAudioStreamBuilder_setBufferCapacityInFrames(AAudioStreamBuilder* builder,
                                                           int32_t numFrames) {
    if (builder) reinterpret_cast<Builder*>(builder)->buffer_capacity = numFrames;
}

static void AAudioStreamBuilder_setPerformanceMode(AAudioStreamBuilder* builder,
                                                    aaudio_performance_mode_t mode) {
    if (builder) reinterpret_cast<Builder*>(builder)->performance_mode = mode;
}

static void AAudioStreamBuilder_setUsage(AAudioStreamBuilder* builder, aaudio_usage_t usage) {
    if (builder) reinterpret_cast<Builder*>(builder)->usage = usage;
}

static void AAudioStreamBuilder_setInputPreset(AAudioStreamBuilder* builder,
                                                aaudio_input_preset_t preset) {
    if (builder) reinterpret_cast<Builder*>(builder)->input_preset = preset;
}

static void AAudioStreamBuilder_setDataCallback(AAudioStreamBuilder* builder,
                                                 AAudioStream_dataCallback callback,
                                                 void* userData) {
    if (!builder) return;
    auto* b = reinterpret_cast<Builder*>(builder);
    b->data_callback = callback;
    b->data_user = userData;
}

static void AAudioStreamBuilder_setErrorCallback(AAudioStreamBuilder* builder,
                                                  AAudioStream_errorCallback callback,
                                                  void* userData) {
    if (!builder) return;
    auto* b = reinterpret_cast<Builder*>(builder);
    b->error_callback = callback;
    b->error_user = userData;
}

/// The one call that can honestly fail, and the one worth narrating.
///
/// Everything the engine configured is logged here whatever the outcome,
/// because this is the only place the *requested* configuration is visible —
/// the getters below report the negotiated one, and the difference between
/// them is the whole answer to "what does this build actually want".
static aaudio_result_t AAudioStreamBuilder_openStream(AAudioStreamBuilder* builder,
                                                       AAudioStream** streamOut) {
    if (!builder || !streamOut) return AAUDIO_ERROR_NULL;
    auto* b = reinterpret_cast<Builder*>(builder);
    *streamOut = nullptr;

    std::fprintf(stderr,
        "I/Cordial-AAudio          openStream requested: direction=%s format=%s "
        "bufferCapacity=%d performanceMode=%d usage=%d inputPreset=%d dataCallback=%s "
        "errorCallback=%s\n",
        b->direction == AAUDIO_DIRECTION_INPUT ? "INPUT" : "OUTPUT", format_name(b->format),
        b->buffer_capacity, b->performance_mode, b->usage, b->input_preset,
        b->data_callback ? "yes" : "no", b->error_callback ? "yes" : "no");

    if (b->direction == AAUDIO_DIRECTION_INPUT) {
        // Measured on 2026-08-22: FMOD asks for one input stream, gets this,
        // and the *output* stream it already had keeps playing normally for
        // the rest of the run. So refusing capture costs capture and nothing
        // else — which is not the same as saying recording finds another
        // route, and that half is untested. `INFERRED` either way; nothing
        // here records, and `WebRtcAudioRecord` in `audio_classes.cpp` is
        // refused as well.
        std::fprintf(stderr,
            "W/Cordial-AAudio          refusing an input stream: AAudio capture is not "
            "implemented here (see the file header on why an honest refusal beats a "
            "microphone with no lifetime rule). Playback is unaffected.\n");
        return AAUDIO_ERROR_UNIMPLEMENTED;
    }

    if (selected_backend() == Backend::AAudioRefuse) {
        std::fprintf(stderr,
            "I/Cordial-AAudio          CONTROL RUN (CORDIAL_AUDIO=aaudio-refuse): reporting "
            "AAUDIO_ERROR_UNAVAILABLE so that what FMOD does next can be read off the log.\n");
        return AAUDIO_ERROR_UNAVAILABLE;
    }

    uint32_t bits = 0;
    bool is_float = false;
    if (!format_to_bits(b->format, bits, is_float)) {
        std::fprintf(stderr,
            "E/Cordial-AAudio          no PipeWire equivalent for aaudio_format_t %d; "
            "refusing rather than substituting a nearby format.\n", b->format);
        return AAUDIO_ERROR_ILLEGAL_ARGUMENT;
    }

    if (!cordial::audio::pipewire_available()) {
        // Should be unreachable: `org.fmod.FMOD.supportsAAudio()` in
        // `audio_classes.cpp` asks the same question first and answers false
        // when there is no session, precisely so that FMOD never gets here.
        // Kept because a refusal at this point is not recoverable — the
        // control run on 2026-08-22 showed FMOD abandoning audio outright
        // rather than falling back — so the one place that could still
        // produce it should say what it cost.
        std::fprintf(stderr,
            "E/Cordial-AAudio          no PipeWire session reachable, and FMOD has already "
            "committed to AAudio; there will be no audio this run and no fallback. This "
            "means supportsAAudio() said yes and the session went away between then and "
            "now.\n");
        return AAUDIO_ERROR_UNAVAILABLE;
    }

    // A stream with no data callback is not a mistake and must not be
    // refused.
    //
    // **Measured**, on a signed-in run into place 1818 on 2026-08-22. FMOD
    // opens four output streams in this order, and only the last is the one
    // that plays:
    //
    //     bufferCapacity=0     dataCallback=no   errorCallback=no    -> closed at once
    //     bufferCapacity=0     dataCallback=yes  errorCallback=yes   -> closed at once
    //     bufferCapacity=9216  dataCallback=yes  errorCallback=yes   -> kept, and pulled
    //     direction=INPUT      dataCallback=no   errorCallback=no    -> refused, below
    //
    // The first two are *probes*. With no `AAudioStreamBuilder_setSampleRate`
    // to ask with, the only way to learn what the device runs at is to open a
    // stream and read `getSampleRate`/`getFramesPerBurst` off it — and the
    // 9216 in the third line is FMOD sizing itself from what the second one
    // reported (nine times the 1024-frame burst PipeWire gave it).
    //
    // The first revision of this file refused the callback-less probe, on the
    // reasoning that such a stream could never be fed. True, and beside the
    // point: nothing is meant to feed it. So it opens here like any other, and
    // `CallbackStream` simply never has anything to pull — `fill_from_engine`
    // returns false on a null callback and every cycle is silence.
    auto* s = new (std::nothrow) Stream();
    if (!s) return AAUDIO_ERROR_NO_MEMORY;
    s->data_callback = b->data_callback;
    s->data_user = b->data_user;
    s->error_callback = b->error_callback;
    s->error_user = b->error_user;

    if (!s->pw.open(bits, is_float, "Cordial (Roblox via AAudio)", &fill_from_engine, s)) {
        delete s;
        return AAUDIO_ERROR_UNAVAILABLE;
    }

    s->format = bits_to_format(s->pw.sample_bits(), s->pw.sample_is_float());
    if (s->format == AAUDIO_FORMAT_INVALID) {
        std::fprintf(stderr,
            "E/Cordial-AAudio          PipeWire negotiated a %u-bit %s format with no "
            "aaudio_format_t to describe it; refusing rather than reporting a format the "
            "engine would lay its mixer out against wrongly.\n",
            s->pw.sample_bits(), s->pw.sample_is_float() ? "float" : "integer");
        delete s;
        return AAUDIO_ERROR_UNAVAILABLE;
    }

    std::fprintf(stderr,
        "I/Cordial-AAudio          openStream ok: PipeWire negotiated %u Hz, %u channel(s), "
        "%s, %u frames per burst. Requested format was %s; rate and channel count were left "
        "for PipeWire to choose, which is why nothing resamples.\n",
        s->pw.rate_hz(), s->pw.channels(), format_name(s->format), s->pw.burst_frames(),
        format_name(b->format));

    *streamOut = reinterpret_cast<AAudioStream*>(s);
    return AAUDIO_OK;
}

static aaudio_result_t AAudioStream_close(AAudioStream* stream) {
    if (!stream) return AAUDIO_ERROR_NULL;
    auto* s = reinterpret_cast<Stream*>(stream);
    if (on_callback_thread(s)) {
        // Would take PipeWire's loop lock from the loop thread itself, which
        // is a plain mutex and would hang the graph. AAudio documents that
        // this is not allowed; refusing keeps the bug visible.
        std::fprintf(stderr,
            "E/Cordial-AAudio          AAudioStream_close called from inside the data "
            "callback; refusing (AAudio requires another thread for this) rather than "
            "deadlocking PipeWire's loop.\n");
        return AAUDIO_ERROR_INVALID_STATE;
    }
    s->pw.close();
    s->state.store(AAUDIO_STREAM_STATE_CLOSED, std::memory_order_relaxed);
    std::fprintf(stderr,
        "I/Cordial-AAudio          stream closed after %llu silence-filled cycle(s).\n",
        static_cast<unsigned long long>(s->pw.silence_cycles()));
    delete s;
    return AAUDIO_OK;
}

// requestStart/Pause/Stop flip one relaxed atomic and touch nothing else.
//
// They deliberately do *not* call `pw_stream_set_active`, which would need
// PipeWire's loop lock — and AAudio documents that an error callback may stop
// the stream, which arrives on the loop thread, where taking that lock hangs.
// The cost is a PipeWire node that stays connected and writes silence while
// paused; the benefit is that the state machine cannot deadlock from anywhere
// it might be called, which is the entire point of moving off the JNI path.
static aaudio_result_t AAudioStream_requestStart(AAudioStream* stream) {
    if (!stream) return AAUDIO_ERROR_NULL;
    auto* s = reinterpret_cast<Stream*>(stream);
    if (!s->pw.is_open()) return AAUDIO_ERROR_INVALID_STATE;
    s->pw.set_running(true);
    s->state.store(AAUDIO_STREAM_STATE_STARTED, std::memory_order_relaxed);
    return AAUDIO_OK;
}

static aaudio_result_t AAudioStream_requestPause(AAudioStream* stream) {
    if (!stream) return AAUDIO_ERROR_NULL;
    auto* s = reinterpret_cast<Stream*>(stream);
    s->pw.set_running(false);
    s->state.store(AAUDIO_STREAM_STATE_PAUSED, std::memory_order_relaxed);
    return AAUDIO_OK;
}

static aaudio_result_t AAudioStream_requestStop(AAudioStream* stream) {
    if (!stream) return AAUDIO_ERROR_NULL;
    auto* s = reinterpret_cast<Stream*>(stream);
    s->pw.set_running(false);
    s->state.store(AAUDIO_STREAM_STATE_STOPPED, std::memory_order_relaxed);
    return AAUDIO_OK;
}

static aaudio_stream_state_t AAudioStream_getState(AAudioStream* stream) {
    if (!stream) return AAUDIO_STREAM_STATE_CLOSED;
    return reinterpret_cast<Stream*>(stream)->state.load(std::memory_order_relaxed);
}

static int32_t AAudioStream_getSampleRate(AAudioStream* stream) {
    if (!stream) return 0;
    return static_cast<int32_t>(reinterpret_cast<Stream*>(stream)->pw.rate_hz());
}

static int32_t AAudioStream_getChannelCount(AAudioStream* stream) {
    if (!stream) return 0;
    return static_cast<int32_t>(reinterpret_cast<Stream*>(stream)->pw.channels());
}

static aaudio_format_t AAudioStream_getFormat(AAudioStream* stream) {
    if (!stream) return AAUDIO_FORMAT_INVALID;
    return reinterpret_cast<Stream*>(stream)->format;
}

/// The graph quantum PipeWire last asked for, measured rather than declared.
static int32_t AAudioStream_getFramesPerBurst(AAudioStream* stream) {
    if (!stream) return 0;
    return static_cast<int32_t>(reinterpret_cast<Stream*>(stream)->pw.burst_frames());
}

// Capacity and size are both the burst, and that is the truth rather than a
// shortcut: there is no buffer between the engine and PipeWire on this path.
// PipeWire pulls one quantum and the engine fills it in the same call, so
// there is nothing that could hold more, and reporting a larger number would
// invite FMOD to believe it had latency headroom it does not have.
static int32_t AAudioStream_getBufferCapacityInFrames(AAudioStream* stream) {
    return AAudioStream_getFramesPerBurst(stream);
}

static int32_t AAudioStream_getBufferSizeInFrames(AAudioStream* stream) {
    return AAudioStream_getFramesPerBurst(stream);
}

/// Returns the size actually in force, which AAudio documents as the correct
/// answer when the request is clamped. Here it is always the burst, for the
/// reason above.
static aaudio_result_t AAudioStream_setBufferSizeInFrames(AAudioStream* stream,
                                                           int32_t numFrames) {
    if (!stream) return AAUDIO_ERROR_NULL;
    auto* s = reinterpret_cast<Stream*>(stream);
    int32_t actual = static_cast<int32_t>(s->pw.burst_frames());
    if (numFrames != actual) {
        std::fprintf(stderr,
            "I/Cordial-AAudio          setBufferSizeInFrames(%d) -> %d: the buffer is "
            "PipeWire's quantum and there is nothing between it and the engine to resize.\n",
            numFrames, actual);
    }
    return actual;
}

/// **This is not Android's xrun count, and saying so is the point.**
///
/// On Android this is AudioFlinger's tally of times the client missed a
/// deadline. Here the engine's callback is invoked *synchronously* from
/// PipeWire's own cycle, so a late callback is a late PipeWire cycle: the
/// glitch is real and audible, but it is counted on the server side, and
/// nothing in `pw_time` or the stream API hands it back to the client. What
/// this reports is the only underrun Cordial can see for itself — cycles that
/// had to be filled with silence while the stream was running, which in a
/// healthy run is zero and stays zero.
///
/// So a flat zero here is **not** evidence of a clean run. The instrument for
/// that is `pw-top`'s ERR column against Cordial's node, which counts the
/// server's own xruns and is comparable across this backend and the Java one.
/// This project has four "fixes" on record that were scored with an
/// instrument that turned out to be constant across every run; this comment
/// is here so that this counter does not become the fifth.
static int32_t AAudioStream_getXRunCount(AAudioStream* stream) {
    if (!stream) return 0;
    auto* s = reinterpret_cast<Stream*>(stream);
    uint64_t silence = s->pw.silence_cycles();
    return static_cast<int32_t>(silence > INT32_MAX ? INT32_MAX : silence);
}

/// Unreachable while `openStream` refuses every input direction, and an
/// honest error rather than a zero-filled buffer if that ever changes without
/// this changing with it. Returning 0 frames "successfully" would give the
/// engine silence it could not distinguish from a quiet room.
static aaudio_result_t AAudioStream_read(AAudioStream* stream, void* buffer, int32_t numFrames,
                                          int64_t timeoutNanoseconds) {
    (void)stream;
    (void)buffer;
    (void)numFrames;
    (void)timeoutNanoseconds;
    static bool warned = false;
    if (!warned) {
        warned = true;
        std::fprintf(stderr,
            "E/Cordial-AAudio          AAudioStream_read on a stream this bridge only opens "
            "for output; reporting AAUDIO_ERROR_UNIMPLEMENTED.\n");
    }
    return AAUDIO_ERROR_UNIMPLEMENTED;
}

// ------------------------------------------------------------- symbol table

struct CordialAAudioSymbol {
    const char* name;
    void* address;
};

static const CordialAAudioSymbol kSymbols[] = {
    {"AAudio_createStreamBuilder", reinterpret_cast<void*>(&AAudio_createStreamBuilder)},
    {"AAudioStreamBuilder_delete", reinterpret_cast<void*>(&AAudioStreamBuilder_delete)},
    {"AAudioStreamBuilder_openStream", reinterpret_cast<void*>(&AAudioStreamBuilder_openStream)},
    {"AAudioStreamBuilder_setBufferCapacityInFrames",
     reinterpret_cast<void*>(&AAudioStreamBuilder_setBufferCapacityInFrames)},
    {"AAudioStreamBuilder_setDataCallback",
     reinterpret_cast<void*>(&AAudioStreamBuilder_setDataCallback)},
    {"AAudioStreamBuilder_setDirection", reinterpret_cast<void*>(&AAudioStreamBuilder_setDirection)},
    {"AAudioStreamBuilder_setErrorCallback",
     reinterpret_cast<void*>(&AAudioStreamBuilder_setErrorCallback)},
    {"AAudioStreamBuilder_setFormat", reinterpret_cast<void*>(&AAudioStreamBuilder_setFormat)},
    {"AAudioStreamBuilder_setInputPreset",
     reinterpret_cast<void*>(&AAudioStreamBuilder_setInputPreset)},
    {"AAudioStreamBuilder_setPerformanceMode",
     reinterpret_cast<void*>(&AAudioStreamBuilder_setPerformanceMode)},
    {"AAudioStreamBuilder_setUsage", reinterpret_cast<void*>(&AAudioStreamBuilder_setUsage)},
    {"AAudioStream_close", reinterpret_cast<void*>(&AAudioStream_close)},
    {"AAudioStream_getBufferCapacityInFrames",
     reinterpret_cast<void*>(&AAudioStream_getBufferCapacityInFrames)},
    {"AAudioStream_getBufferSizeInFrames",
     reinterpret_cast<void*>(&AAudioStream_getBufferSizeInFrames)},
    {"AAudioStream_getChannelCount", reinterpret_cast<void*>(&AAudioStream_getChannelCount)},
    {"AAudioStream_getFormat", reinterpret_cast<void*>(&AAudioStream_getFormat)},
    {"AAudioStream_getFramesPerBurst", reinterpret_cast<void*>(&AAudioStream_getFramesPerBurst)},
    {"AAudioStream_getSampleRate", reinterpret_cast<void*>(&AAudioStream_getSampleRate)},
    {"AAudioStream_getState", reinterpret_cast<void*>(&AAudioStream_getState)},
    {"AAudioStream_getXRunCount", reinterpret_cast<void*>(&AAudioStream_getXRunCount)},
    {"AAudioStream_read", reinterpret_cast<void*>(&AAudioStream_read)},
    {"AAudioStream_requestPause", reinterpret_cast<void*>(&AAudioStream_requestPause)},
    {"AAudioStream_requestStart", reinterpret_cast<void*>(&AAudioStream_requestStart)},
    {"AAudioStream_requestStop", reinterpret_cast<void*>(&AAudioStream_requestStop)},
    {"AAudioStream_setBufferSizeInFrames",
     reinterpret_cast<void*>(&AAudioStream_setBufferSizeInFrames)},
};

const CordialAAudioSymbol* cordial_aaudio_symbols(size_t* count) {
    if (count) *count = sizeof(kSymbols) / sizeof(kSymbols[0]);
    return kSymbols;
}

int cordial_audio_backend_is_aaudio(void) {
    return selected_backend() == Backend::Java ? 0 : 1;
}

void cordial_audio_backend_announce(void) { (void)selected_backend(); }

} // extern "C"
