// PulseAudio, as a second host backend behind ADR-023's seam.
//
// **Written second, and after PipeWire rather than instead of it**, because
// PipeWire is what every current user's session runs and a native PulseAudio
// path only helps somebody whose does not: a machine running the real
// `pulseaudio` daemon, or one where `pipewire-pulse` is absent. Sober, the
// reference implementation this project studies, links PulseAudio -- which is a
// data point about what the people already running Roblox on Linux have.
//
// ## The contract this has to meet, and where a port like this goes wrong
//
// `OutputStream` is a *pull* interface: the caller hands over a `FillCallback`
// and expects to be asked for frames. PulseAudio's threaded mainloop is the
// closest thing to that of the three backends ADR-023 schedules -- its write
// callback says "there is room for `nbytes`", which is one `pa_stream_begin_
// write` away from "fill this". ALSA, when it comes, will have to own a writer
// thread instead.
//
// Two things the engine does make this less free than it looks, both measured
// and both recorded in `docs/analysis/aaudio-contract.md`:
//
//   * **The engine constrains nothing and reads everything back.** It looks up
//     no `AAudioStreamBuilder_setSampleRate` and no `_setChannelCount`; it
//     opens a stream and then asks what it got. So `rate_hz`, `channels` and
//     `burst_frames` must be what the server actually gave, never what was
//     asked for.
//   * **FMOD sizes itself from `getFramesPerBurst`**, nine times over. A burst
//     figure that is a guess rather than the server's real fragment size is a
//     buffer nine times the wrong size.
//
// ## The rules that are not negotiable
//
// `pipewire_backend.h` states them for the interface and they are inherited
// whole. Restated here because this file is where somebody will get them wrong:
//
//   1. **The fill path may not lock, allocate, free, log or make a syscall.**
//      This backend calls the engine's callback from PulseAudio's own mainloop
//      thread, inside `pa_threaded_mainloop`'s lock, which is exactly where
//      PipeWire's `process()` runs relative to its loop lock.
//   2. **`set_running(false)` writes silence rather than corking**, so it is
//      safe to call from inside the fill callback -- which
//      `AAudioStream_requestStop` is documented to be reached from. Corking
//      takes the mainloop lock and would deadlock against a callback that
//      already holds it. This is the same reason `CallbackStream::set_running`
//      does not call `pw_stream_set_active`.
//   3. **Never hold a mutex of this file's across a `pa_*` call.** That is the
//      shape of the AB-BA deadlock `c7215eb` fixed on the OpenSL path, and the
//      reason the AAudio path was written to have no mutex at all. This file
//      keeps that property: every field the mainloop thread touches is either
//      set before the stream connects or is an atomic.
//
// ## Linked at run time, never at build time
//
// `libpulse.so.0` is `dlsym`'d, exactly as `pipewire_backend.cpp` does for
// PipeWire, so a machine without it degrades to "this backend is unavailable"
// rather than to a client that will not start. The headers are needed to
// compile and are optional: without them this file compiles to a backend that
// reports itself unavailable, which is the same honest shape the PipeWire file
// takes.

#include "pipewire_backend.h"

#include <atomic>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>

#ifdef CORDIAL_HAVE_PULSE

#include <dlfcn.h>
#include <pulse/pulseaudio.h>

namespace cordial::audio {
namespace {

/// Everything this file calls in `libpulse`, resolved once.
///
/// A table rather than a link line for the reason at the top of the file, and
/// spelled out rather than macro-generated so that a missing symbol names
/// itself in the log instead of failing as a null call.
struct Pulse {
    pa_threaded_mainloop* (*mainloop_new)();
    void (*mainloop_free)(pa_threaded_mainloop*);
    int (*mainloop_start)(pa_threaded_mainloop*);
    void (*mainloop_stop)(pa_threaded_mainloop*);
    void (*mainloop_lock)(pa_threaded_mainloop*);
    void (*mainloop_unlock)(pa_threaded_mainloop*);
    void (*mainloop_wait)(pa_threaded_mainloop*);
    void (*mainloop_signal)(pa_threaded_mainloop*, int);
    pa_mainloop_api* (*mainloop_get_api)(pa_threaded_mainloop*);

    pa_context* (*context_new)(pa_mainloop_api*, const char*);
    void (*context_unref)(pa_context*);
    int (*context_connect)(pa_context*, const char*, pa_context_flags_t, const pa_spawn_api*);
    void (*context_disconnect)(pa_context*);
    pa_context_state_t (*context_get_state)(pa_context*);
    void (*context_set_state_callback)(pa_context*, pa_context_notify_cb_t, void*);

    pa_stream* (*stream_new)(pa_context*, const char*, const pa_sample_spec*, const pa_channel_map*);
    void (*stream_unref)(pa_stream*);
    int (*stream_connect_playback)(pa_stream*, const char*, const pa_buffer_attr*,
                                   pa_stream_flags_t, const pa_cvolume*, pa_stream*);
    int (*stream_disconnect)(pa_stream*);
    pa_stream_state_t (*stream_get_state)(pa_stream*);
    void (*stream_set_state_callback)(pa_stream*, pa_stream_notify_cb_t, void*);
    void (*stream_set_write_callback)(pa_stream*, pa_stream_request_cb_t, void*);
    int (*stream_begin_write)(pa_stream*, void**, size_t*);
    int (*stream_cancel_write)(pa_stream*);
    int (*stream_write)(pa_stream*, const void*, size_t, pa_free_cb_t, int64_t, pa_seek_mode_t);
    const pa_sample_spec* (*stream_get_sample_spec)(pa_stream*);
    const pa_buffer_attr* (*stream_get_buffer_attr)(pa_stream*);
};

const Pulse* pulse() {
    static const Pulse* table = [] () -> const Pulse* {
        void* lib = ::dlopen("libpulse.so.0", RTLD_LAZY | RTLD_LOCAL);
        if (!lib) {
            std::fprintf(stderr,
                "I/Cordial-Pulse           libpulse.so.0 is not on this machine; the PulseAudio "
                "backend is unavailable.\n");
            return nullptr;
        }
        auto* t = new Pulse{};
        bool ok = true;
        auto need = [&] (auto& slot, const char* name) {
            void* sym = ::dlsym(lib, name);
            if (!sym) {
                std::fprintf(stderr,
                    "E/Cordial-Pulse           libpulse.so.0 has no %s; refusing to use a "
                    "half-resolved backend.\n", name);
                ok = false;
                return;
            }
            slot = reinterpret_cast<std::remove_reference_t<decltype(slot)>>(sym);
        };
#define NEED(field, name) need(t->field, name)
        NEED(mainloop_new, "pa_threaded_mainloop_new");
        NEED(mainloop_free, "pa_threaded_mainloop_free");
        NEED(mainloop_start, "pa_threaded_mainloop_start");
        NEED(mainloop_stop, "pa_threaded_mainloop_stop");
        NEED(mainloop_lock, "pa_threaded_mainloop_lock");
        NEED(mainloop_unlock, "pa_threaded_mainloop_unlock");
        NEED(mainloop_wait, "pa_threaded_mainloop_wait");
        NEED(mainloop_signal, "pa_threaded_mainloop_signal");
        NEED(mainloop_get_api, "pa_threaded_mainloop_get_api");
        NEED(context_new, "pa_context_new");
        NEED(context_unref, "pa_context_unref");
        NEED(context_connect, "pa_context_connect");
        NEED(context_disconnect, "pa_context_disconnect");
        NEED(context_get_state, "pa_context_get_state");
        NEED(context_set_state_callback, "pa_context_set_state_callback");
        NEED(stream_new, "pa_stream_new");
        NEED(stream_unref, "pa_stream_unref");
        NEED(stream_connect_playback, "pa_stream_connect_playback");
        NEED(stream_disconnect, "pa_stream_disconnect");
        NEED(stream_get_state, "pa_stream_get_state");
        NEED(stream_set_state_callback, "pa_stream_set_state_callback");
        NEED(stream_set_write_callback, "pa_stream_set_write_callback");
        NEED(stream_begin_write, "pa_stream_begin_write");
        NEED(stream_cancel_write, "pa_stream_cancel_write");
        NEED(stream_write, "pa_stream_write");
        NEED(stream_get_sample_spec, "pa_stream_get_sample_spec");
        NEED(stream_get_buffer_attr, "pa_stream_get_buffer_attr");
#undef NEED
        if (!ok) { delete t; return nullptr; }
        return t;
    }();
    return table;
}

} // namespace

namespace {

/// One playback stream on a PulseAudio server.
///
/// The lifetime rule is `CallbackStream`'s: nothing is held until `open()` and
/// nothing is held once `close()` returns, so a stream that has stopped is
/// indistinguishable from one that never existed.
class PulseStream final : public OutputStream {
public:
    PulseStream() = default;
    ~PulseStream() override { close(); }

    bool open(uint32_t sample_bits, bool is_float, const char* node_description,
              const char* target_node_name, FillCallback cb, void* user) override;
    void close() override;
    bool is_open() const override { return stream_ != nullptr; }

    void set_running(bool running) override { running_.store(running, std::memory_order_relaxed); }
    bool is_running() const override { return running_.load(std::memory_order_relaxed); }

    uint32_t rate_hz() const override { return rate_; }
    uint32_t channels() const override { return channels_; }
    uint32_t sample_bits() const override { return 32; }
    bool sample_is_float() const override { return true; }
    uint32_t burst_frames() const override { return burst_.load(std::memory_order_relaxed); }
    uint64_t silence_cycles() const override { return silence_.load(std::memory_order_relaxed); }

private:
    static void on_context_state(pa_context*, void* self);
    static void on_stream_state(pa_stream*, void* self);
    static void on_write(pa_stream*, size_t bytes, void* self);

    pa_threaded_mainloop* loop_ = nullptr;
    pa_context* context_ = nullptr;
    pa_stream* stream_ = nullptr;

    FillCallback fill_ = nullptr;
    void* user_ = nullptr;
    uint32_t rate_ = 0;
    uint32_t channels_ = 0;
    /// Set once, before the stream connects, so the callback thread reads it
    /// without synchronising. Everything else it touches is atomic.
    uint32_t frame_bytes_ = 0;

    std::atomic<uint32_t> burst_{0};
    std::atomic<uint64_t> silence_{0};
    std::atomic<bool> running_{false};
};

void PulseStream::on_context_state(pa_context*, void* self) {
    auto* s = static_cast<PulseStream*>(self);
    pulse()->mainloop_signal(s->loop_, 0);
}

void PulseStream::on_stream_state(pa_stream*, void* self) {
    auto* s = static_cast<PulseStream*>(self);
    pulse()->mainloop_signal(s->loop_, 0);
}

/// PulseAudio saying "there is room for `bytes`". The pull side of the seam.
///
/// **Runs on the mainloop thread with its lock held**, which is the same
/// position `process()` occupies relative to PipeWire's loop lock -- so the
/// realtime rule at the top of this file applies to everything reachable from
/// here, including the engine's own callback.
void PulseStream::on_write(pa_stream* stream, size_t bytes, void* self) {
    auto* s = static_cast<PulseStream*>(self);
    const Pulse* pa = pulse();
    if (!pa || s->frame_bytes_ == 0) return;

    // The server's own idea of how much it wants, which is the only truthful
    // answer to `getFramesPerBurst` -- and FMOD multiplies it by nine to size
    // itself, so a guess here is a buffer nine times the wrong size.
    s->burst_.store(static_cast<uint32_t>(bytes / s->frame_bytes_), std::memory_order_relaxed);

    while (bytes > 0) {
        void* dst = nullptr;
        size_t chunk = bytes;
        if (pa->stream_begin_write(stream, &dst, &chunk) < 0 || !dst || chunk == 0) return;
        const uint32_t frames = static_cast<uint32_t>(chunk / s->frame_bytes_);
        if (frames == 0) {
            pa->stream_cancel_write(stream);
            return;
        }
        // **Silence rather than a cork when stopped**, so `set_running` stays
        // callable from inside this callback; see rule 2 at the top.
        bool keep = true;
        if (s->running_.load(std::memory_order_relaxed) && s->fill_) {
            keep = s->fill_(dst, frames, s->user_);
        } else {
            std::memset(dst, 0, static_cast<size_t>(frames) * s->frame_bytes_);
            s->silence_.fetch_add(1, std::memory_order_relaxed);
        }
        pa->stream_write(stream, dst, static_cast<size_t>(frames) * s->frame_bytes_, nullptr, 0,
                         PA_SEEK_RELATIVE);
        if (!keep) {
            // The engine asked to stop being pulled. Silence from here rather
            // than a disconnect, for the same reason `set_running` does.
            s->running_.store(false, std::memory_order_relaxed);
            return;
        }
        bytes -= static_cast<size_t>(frames) * s->frame_bytes_;
    }
}

bool PulseStream::open(uint32_t, bool, const char* node_description,
                       const char* target_node_name, FillCallback cb, void* user) {
    const Pulse* pa = pulse();
    if (!pa) return false;
    close();

    fill_ = cb;
    user_ = user;

    // **PulseAudio needs a concrete sample spec and PipeWire does not, and the
    // difference is worth being honest about.** `CallbackStream::open` offers a
    // format and lets the graph choose rate and channels, so what it reports
    // back is measured. Here the server converts to whatever the sink wants and
    // `pa_stream_get_sample_spec` returns what was *asked for*, so this is a
    // choice rather than a measurement. 48 kHz stereo float is what every
    // PipeWire negotiation on this path has produced, so it is the choice least
    // likely to introduce a conversion that was not there before.
    pa_sample_spec spec{};
    spec.format = PA_SAMPLE_FLOAT32LE;
    spec.rate = 48000;
    spec.channels = 2;
    rate_ = spec.rate;
    channels_ = spec.channels;
    frame_bytes_ = sizeof(float) * spec.channels;

    loop_ = pa->mainloop_new();
    if (!loop_) return false;
    context_ = pa->context_new(pa->mainloop_get_api(loop_),
                               node_description ? node_description : "Cordial");
    if (!context_) { close(); return false; }

    pa->context_set_state_callback(context_, &PulseStream::on_context_state, this);
    pa->mainloop_lock(loop_);
    if (pa->mainloop_start(loop_) < 0) {
        pa->mainloop_unlock(loop_);
        close();
        return false;
    }
    if (pa->context_connect(context_, nullptr, PA_CONTEXT_NOAUTOSPAWN, nullptr) < 0) {
        pa->mainloop_unlock(loop_);
        close();
        return false;
    }
    for (;;) {
        pa_context_state_t st = pa->context_get_state(context_);
        if (st == PA_CONTEXT_READY) break;
        if (st == PA_CONTEXT_FAILED || st == PA_CONTEXT_TERMINATED) {
            pa->mainloop_unlock(loop_);
            std::fprintf(stderr,
                "I/Cordial-Pulse           no PulseAudio server answered; this backend is "
                "unavailable for this run.\n");
            close();
            return false;
        }
        pa->mainloop_wait(loop_);
    }

    stream_ = pa->stream_new(context_, "Roblox", &spec, nullptr);
    if (!stream_) { pa->mainloop_unlock(loop_); close(); return false; }
    pa->stream_set_state_callback(stream_, &PulseStream::on_stream_state, this);
    pa->stream_set_write_callback(stream_, &PulseStream::on_write, this);

    // `nullptr` for the sink means "follow the server's default", and — like
    // PipeWire's empty target — it keeps following it when the default changes.
    // That property is the reason `CORDIAL_AUDIO_SINK` means what it does, and
    // it is the one property ADR-023 records as surviving into this backend and
    // no further: ALSA resolves `default` once and never moves a live stream.
    const char* sink = (target_node_name && target_node_name[0]) ? target_node_name : nullptr;
    const auto flags = static_cast<pa_stream_flags_t>(PA_STREAM_START_CORKED == 0 ? 0 : 0);
    if (pa->stream_connect_playback(stream_, sink, nullptr, flags, nullptr, nullptr) < 0) {
        pa->mainloop_unlock(loop_);
        close();
        return false;
    }
    for (;;) {
        pa_stream_state_t st = pa->stream_get_state(stream_);
        if (st == PA_STREAM_READY) break;
        if (st == PA_STREAM_FAILED || st == PA_STREAM_TERMINATED) {
            pa->mainloop_unlock(loop_);
            close();
            return false;
        }
        pa->mainloop_wait(loop_);
    }

    // Seed the burst from the server's own attributes so the first
    // `getFramesPerBurst` is a real number rather than zero; `on_write`
    // replaces it with what was actually asked for on every cycle.
    if (const pa_buffer_attr* attr = pa->stream_get_buffer_attr(stream_)) {
        if (attr->minreq != static_cast<uint32_t>(-1) && frame_bytes_ != 0) {
            burst_.store(attr->minreq / frame_bytes_, std::memory_order_relaxed);
        }
    }
    pa->mainloop_unlock(loop_);

    std::fprintf(stderr,
        "I/Cordial-Pulse           opened %u Hz, %u channel(s), PCM_FLOAT, %u frames per burst, "
        "on %s.\n", rate_, channels_, burst_.load(std::memory_order_relaxed),
        sink ? sink : "the server's default sink");
    return true;
}

void PulseStream::close() {
    const Pulse* pa = pulse();
    if (!pa) { stream_ = nullptr; context_ = nullptr; loop_ = nullptr; return; }
    running_.store(false, std::memory_order_relaxed);
    if (loop_) pa->mainloop_stop(loop_);
    if (stream_) {
        pa->stream_disconnect(stream_);
        pa->stream_unref(stream_);
        stream_ = nullptr;
    }
    if (context_) {
        pa->context_disconnect(context_);
        pa->context_unref(context_);
        context_ = nullptr;
    }
    if (loop_) {
        pa->mainloop_free(loop_);
        loop_ = nullptr;
    }
    fill_ = nullptr;
    user_ = nullptr;
}

} // namespace

bool pulse_available() {
    // **The probe has to be as trustworthy as PipeWire's, and ADR-023 says
    // why:** `supportsAAudio()` is a one-way door. Once FMOD has been told
    // AAudio exists it will not fall back, so a backend that answers "yes" and
    // then fails to open leaves the whole session silent with no second chance.
    // Connecting a context and tearing it down is the cheapest thing that
    // actually proves a server is there, and it is what this returns.
    static const bool ok = [] {
        const Pulse* pa = pulse();
        if (!pa) return false;
        pa_threaded_mainloop* loop = pa->mainloop_new();
        if (!loop) return false;
        bool ready = false;
        pa_context* ctx = pa->context_new(pa->mainloop_get_api(loop), "Cordial (probe)");
        if (ctx) {
            struct Probe { pa_threaded_mainloop* loop; } probe{loop};
            pa->context_set_state_callback(ctx, [] (pa_context*, void* p) {
                pulse()->mainloop_signal(static_cast<Probe*>(p)->loop, 0);
            }, &probe);
            pa->mainloop_lock(loop);
            if (pa->mainloop_start(loop) >= 0 &&
                pa->context_connect(ctx, nullptr, PA_CONTEXT_NOAUTOSPAWN, nullptr) >= 0) {
                for (;;) {
                    pa_context_state_t st = pa->context_get_state(ctx);
                    if (st == PA_CONTEXT_READY) { ready = true; break; }
                    if (st == PA_CONTEXT_FAILED || st == PA_CONTEXT_TERMINATED) break;
                    pa->mainloop_wait(loop);
                }
            }
            pa->mainloop_unlock(loop);
            pa->mainloop_stop(loop);
            pa->context_disconnect(ctx);
            pa->context_unref(ctx);
        }
        pa->mainloop_free(loop);
        return ready;
    }();
    return ok;
}

std::unique_ptr<OutputStream> make_pulse_stream() { return std::make_unique<PulseStream>(); }

} // namespace cordial::audio

#else // !CORDIAL_HAVE_PULSE

namespace cordial::audio {

// Built without pulseaudio-libs-devel. Honest rather than absent: the selector
// asks `pulse_available()` and gets a truthful no, so a run that asked for this
// backend is told it did not get one instead of going silent.
bool pulse_available() { return false; }
std::unique_ptr<OutputStream> make_pulse_stream() { return nullptr; }

} // namespace cordial::audio

#endif // CORDIAL_HAVE_PULSE
