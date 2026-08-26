// Does the ALSA backend actually open, get pulled, and produce sound?
//
// **Standalone because waiting for the engine to open audio is not a test.**
// Measured on 2026-08-26: across six launches that reached a signed-in Home
// page, the engine opened an audio stream on exactly one of them. So a client
// run with no sound distinguishes nothing -- not a broken backend from a
// working one, and not either from a launch that simply never got that far.
// This asks the backend directly and takes two seconds.
//
// **`default` is not the same as hardware, and the difference matters here.**
// On a machine running PipeWire, alsa-lib's `default` is PipeWire's ALSA
// plugin -- so a pass proves the `snd_pcm_*` sequencing and the writer thread,
// and does not prove a real device is driven. For that, run it against
// `hw:0,0`, which means taking the card away from the session first. ADR-023
// records that asymmetry rather than letting a green result imply more than it
// shows.
//
// It plays a quiet 440 Hz tone rather than silence, so the answer is available
// by ear as well as by counter. `getXRunCount` reads zero on a perfect stream
// and on one underrunning every cycle -- `aaudio.cpp` says so, and says it is
// there so the counter does not become this project's fifth constant mistaken
// for a measurement. A frame count that climbs at the sample rate is not that.
//
// Build and run it by hand; it needs a live PulseAudio server, which
// `pipewire-pulse` also provides:
//
//     clang++ -std=c++20 -O1 -DCORDIAL_HAVE_ALSA=1 -I native \
//       native/alsa_backend_probe.cpp native/alsa_backend.cpp -ldl -lpthread -o /tmp/alsa-probe
#include "pipewire_backend.h"
#include <cmath>
#include <cstdio>
#include <thread>
#include <chrono>
#include <atomic>

static std::atomic<uint64_t> g_frames{0};
static double g_phase = 0.0;

static bool fill(void* dst, uint32_t frames, void*) {
    float* out = static_cast<float*>(dst);
    for (uint32_t i = 0; i < frames; ++i) {
        // A quiet 440 Hz tone, so "did it play" is answerable by ear as well as
        // by counter. -30 dBFS: audible, not startling.
        float s = static_cast<float>(std::sin(g_phase) * 0.03);
        g_phase += 2.0 * 3.14159265358979 * 440.0 / 48000.0;
        out[i * 2] = s;
        out[i * 2 + 1] = s;
    }
    g_frames.fetch_add(frames);
    return true;
}

int main() {
    std::printf("alsa_available(): %s\n", cordial::audio::alsa_available() ? "yes" : "NO");
    if (!cordial::audio::alsa_available()) return 1;
    auto stream = cordial::audio::make_alsa_stream();
    if (!stream) { std::printf("make_alsa_stream returned null\n"); return 1; }
    if (!stream->open(0, false, "Cordial (alsa probe)", nullptr, fill, nullptr)) {
        std::printf("open failed\n");
        return 1;
    }
    stream->set_running(true);
    std::printf("open ok: %u Hz, %u ch, %u bits, float=%d, burst %u\n",
                stream->rate_hz(), stream->channels(), stream->sample_bits(),
                (int)stream->sample_is_float(), stream->burst_frames());
    for (int i = 0; i < 5; ++i) {
        std::this_thread::sleep_for(std::chrono::milliseconds(400));
        std::printf("  t=%.1fs frames=%llu burst=%u silence_cycles=%llu\n",
                    (i + 1) * 0.4,
                    (unsigned long long)g_frames.load(), stream->burst_frames(),
                    (unsigned long long)stream->silence_cycles());
    }
    // set_running(false) must silence rather than tear down, and must be safe
    // to call from anywhere -- the rule the whole file is arranged around.
    stream->set_running(false);
    uint64_t before = g_frames.load();
    std::this_thread::sleep_for(std::chrono::milliseconds(400));
    std::printf("after set_running(false): engine frames %llu -> %llu (should not climb), "
                "silence_cycles %llu (should climb)\n",
                (unsigned long long)before, (unsigned long long)g_frames.load(),
                (unsigned long long)stream->silence_cycles());
    stream->close();
    std::printf("closed; is_open=%d\n", (int)stream->is_open());
    return g_frames.load() > 48000 ? 0 : 2;
}
