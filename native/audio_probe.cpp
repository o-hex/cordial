// Drives Cordial's audio backend the way Roblox drives it, so that "audio
// works" can be a measurement rather than a reading of the code.
//
// Everything about this backend that had been established before this file
// existed was established by reading it. `pipewire_backend_test.cpp` checks the
// one piece that can be checked with no session at all (the underrun fill), and
// that is deliberately all it checks. The remaining questions — does an OpenSL
// buffer enqueued by the caller reach the host's default sink, does the drain
// callback fire, does the device list match what the session actually has, and
// above all does the microphone close when recording stops — cannot be answered
// without a live PipeWire session, and were therefore not answered at all.
//
// This binary answers them. It links `opensles.cpp` and `pipewire_backend.cpp`
// and calls `slCreateEngine` through the same C entry point the engine links
// against, using the same Khronos vtable layouts, so what it exercises is the
// real path and not a friendlier one alongside it.
//
// Built out of tree on purpose, and never by `cargo build`:
//
//     clang++ -std=c++17 -DCORDIAL_HAVE_PIPEWIRE=1 \
//         -I/usr/include/pipewire-0.3 -I/usr/include/spa-0.2 \
//         native/opensles.cpp native/pipewire_backend.cpp native/audio_probe.cpp \
//         -ldl -lpthread -o /tmp/audio_probe
//
// It needs a running PipeWire session and it makes a real stream appear in it,
// which is exactly why it must not run as part of an ordinary build.
//
// ------------------------------------------------------------------------
// On making a noise
// ------------------------------------------------------------------------
//
// A verification harness for this backend once put a tone through a
// developer's speakers, which is why `pipewire_backend_test.cpp` refuses to
// touch a session at all. This one has to, so `play` defaults to an amplitude
// of 1/512 of full scale — about -54 dBFS, inaudible at any sane desktop
// volume. That is still hundreds of times the noise floor of a sink monitor,
// which is digitally exact zero when nothing is playing, so the measurement
// does not need the loudness the ear would. `--amplitude` can raise it; there
// is no reason to.

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

#include "pipewire_backend.h"

// ---------------------------------------------------------------- SL surface
//
// The subset of OpenSL ES 1.0.1 and its Android extension this probe calls,
// declared here rather than shared with `opensles.cpp`: that file keeps its
// object model in an anonymous namespace, and the point of this probe is to
// reach it only through the linkage Roblox has — the exported `slCreateEngine`
// and the exported `SL_IID_*` data symbols. A layout mistake here would show up
// as a crash or a rejected call, not as a silently different code path.

using SLuint8 = uint8_t;
using SLint16 = int16_t;
using SLint32 = int32_t;
using SLuint32 = uint32_t;
using SLboolean = SLuint32;
using SLchar = SLuint8;
using SLmillibel = SLint16;
using SLmillisecond = SLuint32;
using SLpermille = SLint16;
using SLresult = SLuint32;

constexpr SLboolean SL_BOOLEAN_FALSE = 0;
constexpr SLboolean SL_BOOLEAN_TRUE = 1;
constexpr SLresult SL_RESULT_SUCCESS = 0;

constexpr SLuint32 SL_DATALOCATOR_OUTPUTMIX = 0x00000004;
constexpr SLuint32 SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE = 0x800007BD;
constexpr SLuint32 SL_DATALOCATOR_IODEVICE = 0x00000003;
constexpr SLuint32 SL_IODEVICE_AUDIOINPUT = 0x00000001;
constexpr SLuint32 SL_DEFAULTDEVICEID_AUDIOINPUT = 0xFFFFFFFF;
constexpr SLuint32 SL_DATAFORMAT_PCM = 0x00000002;
constexpr SLuint32 SL_SPEAKER_FRONT_LEFT = 0x00000001;
constexpr SLuint32 SL_SPEAKER_FRONT_RIGHT = 0x00000002;
constexpr SLuint32 SL_SPEAKER_FRONT_CENTER = 0x00000004;
constexpr SLuint32 SL_BYTEORDER_LITTLEENDIAN = 0x00000002;

constexpr SLuint32 SL_PLAYSTATE_STOPPED = 0x00000001;
constexpr SLuint32 SL_PLAYSTATE_PAUSED = 0x00000002;
constexpr SLuint32 SL_PLAYSTATE_PLAYING = 0x00000003;

constexpr SLuint32 SL_RECORDSTATE_STOPPED = 0x00000001;
constexpr SLuint32 SL_RECORDSTATE_PAUSED = 0x00000002;
constexpr SLuint32 SL_RECORDSTATE_RECORDING = 0x00000003;

struct InterfaceID {
    uint32_t time_low;
    uint16_t time_mid;
    uint16_t time_hi_and_version;
    uint16_t clock_seq;
    uint8_t node[6];
};
using SLInterfaceID = InterfaceID*;

struct SLDataSource {
    void* pLocator;
    void* pFormat;
};
using SLDataSink = SLDataSource;

struct SLDataLocator_OutputMix {
    SLuint32 locatorType;
    void* outputMix;
};
struct SLDataLocator_AndroidSimpleBufferQueue {
    SLuint32 locatorType;
    SLuint32 numBuffers;
};
struct SLDataLocator_IODevice {
    SLuint32 locatorType;
    SLuint32 deviceType;
    SLuint32 deviceID;
    void* device;
};
struct SLDataFormat_PCM {
    SLuint32 formatType;
    SLuint32 numChannels;
    SLuint32 samplesPerSec;
    SLuint32 bitsPerSample;
    SLuint32 containerSize;
    SLuint32 channelMask;
    SLuint32 endianness;
};

struct SLObjectItf_;
using SLObjectItf = const SLObjectItf_* const*;
struct SLObjectItf_ {
    SLresult (*Realize)(SLObjectItf, SLboolean);
    SLresult (*Resume)(SLObjectItf, SLboolean);
    SLresult (*GetState)(SLObjectItf, SLuint32*);
    SLresult (*GetInterface)(SLObjectItf, SLInterfaceID, void*);
    SLresult (*RegisterCallback)(SLObjectItf, void*, void*);
    void (*AbortAsyncOperation)(SLObjectItf);
    void (*Destroy)(SLObjectItf);
    SLresult (*SetPriority)(SLObjectItf, SLint32, SLboolean);
    SLresult (*GetPriority)(SLObjectItf, SLint32*, SLboolean*);
    SLresult (*SetLossOfControlInterfaces)(SLObjectItf, SLint16, SLInterfaceID*, SLboolean);
};

struct SLPlayItf_;
using SLPlayItf = const SLPlayItf_* const*;
struct SLPlayItf_ {
    SLresult (*SetPlayState)(SLPlayItf, SLuint32);
    SLresult (*GetPlayState)(SLPlayItf, SLuint32*);
    SLresult (*GetDuration)(SLPlayItf, SLmillisecond*);
    SLresult (*GetPosition)(SLPlayItf, SLmillisecond*);
    SLresult (*RegisterCallback)(SLPlayItf, void*, void*);
    SLresult (*SetCallbackEventsMask)(SLPlayItf, SLuint32);
    SLresult (*GetCallbackEventsMask)(SLPlayItf, SLuint32*);
    SLresult (*SetMarkerPosition)(SLPlayItf, SLmillisecond);
    SLresult (*ClearMarkerPosition)(SLPlayItf);
    SLresult (*GetMarkerPosition)(SLPlayItf, SLmillisecond*);
    SLresult (*SetPositionUpdatePeriod)(SLPlayItf, SLmillisecond);
    SLresult (*GetPositionUpdatePeriod)(SLPlayItf, SLmillisecond*);
};

struct SLRecordItf_;
using SLRecordItf = const SLRecordItf_* const*;
struct SLRecordItf_ {
    SLresult (*SetRecordState)(SLRecordItf, SLuint32);
    SLresult (*GetRecordState)(SLRecordItf, SLuint32*);
    SLresult (*SetDurationLimit)(SLRecordItf, SLmillisecond);
    SLresult (*GetPosition)(SLRecordItf, SLmillisecond*);
    SLresult (*RegisterCallback)(SLRecordItf, void*, void*);
    SLresult (*SetCallbackEventsMask)(SLRecordItf, SLuint32);
    SLresult (*GetCallbackEventsMask)(SLRecordItf, SLuint32*);
    SLresult (*SetMarkerPosition)(SLRecordItf, SLmillisecond);
    SLresult (*ClearMarkerPosition)(SLRecordItf);
    SLresult (*GetMarkerPosition)(SLRecordItf, SLmillisecond*);
    SLresult (*SetPositionUpdatePeriod)(SLRecordItf, SLmillisecond);
    SLresult (*GetPositionUpdatePeriod)(SLRecordItf, SLmillisecond*);
};

struct SLAndroidSimpleBufferQueueItf_;
using SLAndroidSimpleBufferQueueItf = const SLAndroidSimpleBufferQueueItf_* const*;
struct SLAndroidSimpleBufferQueueState {
    SLuint32 count;
    SLuint32 index;
};
struct SLAndroidSimpleBufferQueueItf_ {
    SLresult (*Enqueue)(SLAndroidSimpleBufferQueueItf, const void*, SLuint32);
    SLresult (*Clear)(SLAndroidSimpleBufferQueueItf);
    SLresult (*GetState)(SLAndroidSimpleBufferQueueItf, SLAndroidSimpleBufferQueueState*);
    SLresult (*RegisterCallback)(SLAndroidSimpleBufferQueueItf,
                                  void (*)(SLAndroidSimpleBufferQueueItf, void*), void*);
};

struct SLVolumeItf_;
using SLVolumeItf = const SLVolumeItf_* const*;
struct SLVolumeItf_ {
    SLresult (*SetVolumeLevel)(SLVolumeItf, SLmillibel);
    SLresult (*GetVolumeLevel)(SLVolumeItf, SLmillibel*);
    SLresult (*GetMaxVolumeLevel)(SLVolumeItf, SLmillibel*);
    SLresult (*SetMute)(SLVolumeItf, SLboolean);
    SLresult (*GetMute)(SLVolumeItf, SLboolean*);
    SLresult (*EnableStereoPosition)(SLVolumeItf, SLboolean);
    SLresult (*IsEnabledStereoPosition)(SLVolumeItf, SLboolean*);
    SLresult (*SetStereoPosition)(SLVolumeItf, SLpermille);
    SLresult (*GetStereoPosition)(SLVolumeItf, SLpermille*);
};

struct SLEngineItf_;
using SLEngineItf = const SLEngineItf_* const*;
struct SLEngineItf_ {
    SLresult (*CreateLEDDevice)(SLEngineItf, SLObjectItf*, SLuint32, SLuint32, const SLInterfaceID*,
                                 const SLboolean*);
    SLresult (*CreateVibraDevice)(SLEngineItf, SLObjectItf*, SLuint32, SLuint32, const SLInterfaceID*,
                                   const SLboolean*);
    SLresult (*CreateAudioPlayer)(SLEngineItf, SLObjectItf*, SLDataSource*, SLDataSink*, SLuint32,
                                   const SLInterfaceID*, const SLboolean*);
    SLresult (*CreateAudioRecorder)(SLEngineItf, SLObjectItf*, SLDataSource*, SLDataSink*, SLuint32,
                                     const SLInterfaceID*, const SLboolean*);
    SLresult (*CreateMidiPlayer)(SLEngineItf, SLObjectItf*, SLDataSource*, SLDataSource*, SLDataSink*,
                                  SLDataSink*, SLDataSink*, SLuint32, const SLInterfaceID*,
                                  const SLboolean*);
    SLresult (*CreateListener)(SLEngineItf, SLObjectItf*, SLuint32, const SLInterfaceID*,
                                const SLboolean*);
    SLresult (*Create3DGroup)(SLEngineItf, SLObjectItf*, SLuint32, const SLInterfaceID*,
                               const SLboolean*);
    SLresult (*CreateOutputMix)(SLEngineItf, SLObjectItf*, SLuint32, const SLInterfaceID*,
                                 const SLboolean*);
    SLresult (*CreateMetadataExtractor)(SLEngineItf, SLObjectItf*, SLDataSource*, SLuint32,
                                         const SLInterfaceID*, const SLboolean*);
    SLresult (*CreateExtensionObject)(SLEngineItf, SLObjectItf*, void*, SLuint32, SLuint32,
                                       const SLInterfaceID*, const SLboolean*);
    SLresult (*QueryNumSupportedInterfaces)(SLEngineItf, SLuint32, SLuint32*);
    SLresult (*QuerySupportedInterfaces)(SLEngineItf, SLuint32, SLuint32, SLInterfaceID*);
    SLresult (*QueryNumSupportedExtensions)(SLEngineItf, SLuint32*);
    SLresult (*QuerySupportedExtension)(SLEngineItf, SLuint32, SLchar*, SLint16*);
    SLresult (*IsExtensionSupported)(SLEngineItf, const SLchar*, SLboolean*);
};

extern "C" {
uint32_t slCreateEngine(void** engine, uint32_t numOptions, const void* pEngineOptions,
                         uint32_t numInterfaces, const void* pInterfaceIds,
                         const void* pInterfaceRequired);
extern InterfaceID* SL_IID_ENGINE;
extern InterfaceID* SL_IID_PLAY;
extern InterfaceID* SL_IID_RECORD;
extern InterfaceID* SL_IID_VOLUME;
extern InterfaceID* SL_IID_ANDROIDSIMPLEBUFFERQUEUE;
extern InterfaceID* SL_IID_ANDROIDCONFIGURATION;
}

namespace {

void sleep_ms(int ms) { std::this_thread::sleep_for(std::chrono::milliseconds(ms)); }

/// Prints and flushes: every line this probe writes is a marker something
/// outside the process (a `pw-dump` sampler, a `pw-record` capture) is being
/// lined up against, so a line sitting in a buffer is a line that lies about
/// when the thing it describes happened.
void mark(const char* fmt, ...) __attribute__((format(printf, 1, 2)));
void mark(const char* fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    std::vfprintf(stdout, fmt, ap);
    va_end(ap);
    std::fputc('\n', stdout);
    std::fflush(stdout);
}

SLObjectItf g_engine_obj = nullptr;
SLEngineItf g_engine = nullptr;

bool create_engine() {
    void* obj = nullptr;
    SLInterfaceID ids[1] = {SL_IID_ENGINE};
    SLboolean req[1] = {SL_BOOLEAN_TRUE};
    SLresult r = slCreateEngine(&obj, 0, nullptr, 1, ids, req);
    mark("slCreateEngine -> %u", r);
    if (r != SL_RESULT_SUCCESS) return false;
    g_engine_obj = static_cast<SLObjectItf>(obj);
    r = (*g_engine_obj)->Realize(g_engine_obj, SL_BOOLEAN_FALSE);
    mark("engine Realize -> %u", r);
    if (r != SL_RESULT_SUCCESS) return false;
    r = (*g_engine_obj)->GetInterface(g_engine_obj, SL_IID_ENGINE, &g_engine);
    mark("engine GetInterface(SL_IID_ENGINE) -> %u", r);
    return r == SL_RESULT_SUCCESS && g_engine != nullptr;
}

// ------------------------------------------------------------------ devices

int cmd_devices() {
    auto devices = cordial::audio::enumerate_devices();
    mark("enumerate_devices returned %zu device(s)", devices.size());
    for (const auto& d : devices) {
        mark("  id=%u %s%s name='%s' description='%s' nick='%s' object.path='%s'", d.id,
             d.is_source ? "source" : "sink  ", d.is_default ? " DEFAULT" : "        ",
             d.node_name.c_str(), d.description.c_str(), d.nick.c_str(), d.object_path.c_str());
    }
    // The invariant the whole privacy rule rests on, checked on the live
    // session rather than by construction: listing microphones must not open
    // one. `pipewire_backend_test.cpp` pins the same property with no session
    // at all; this is the version that could actually catch a regression in
    // `enumerate_devices` itself.
    mark("active_capture_streams after enumeration = %u", cordial::audio::active_capture_streams());
    return devices.empty() ? 1 : 0;
}

// ------------------------------------------------------------------ playback

struct ToneFeeder {
    SLAndroidSimpleBufferQueueItf queue = nullptr;
    std::vector<std::vector<int16_t>> buffers;
    size_t next = 0;
    double phase = 0.0;
    double step = 0.0;
    double amplitude = 0.0;
    uint32_t channels = 2;
    std::atomic<uint64_t> drains{0};

    void fill(std::vector<int16_t>& buf) {
        for (size_t i = 0; i < buf.size(); i += channels) {
            auto s = static_cast<int16_t>(amplitude * 32767.0 * std::sin(phase));
            for (uint32_t c = 0; c < channels; ++c) buf[i + c] = s;
            phase += step;
            if (phase > 2 * M_PI) phase -= 2 * M_PI;
        }
    }

    /// Refills the buffer that just drained and re-enqueues it, which is the
    /// ordinary OpenSL pattern and the one worth exercising: it re-enters
    /// `Enqueue` from inside the drain callback, on PipeWire's own thread.
    void on_drained() {
        drains.fetch_add(1);
        auto& buf = buffers[next % buffers.size()];
        ++next;
        fill(buf);
        (*queue)->Enqueue(queue, buf.data(), static_cast<SLuint32>(buf.size() * sizeof(int16_t)));
    }
};

ToneFeeder g_feeder;

void buffer_drained(SLAndroidSimpleBufferQueueItf, void*) { g_feeder.on_drained(); }

int cmd_play(double seconds, double amplitude, double hz, bool silent) {
    if (!create_engine()) return 1;

    SLObjectItf mix = nullptr;
    SLresult r = (*g_engine)->CreateOutputMix(g_engine, &mix, 0, nullptr, nullptr);
    mark("CreateOutputMix -> %u", r);
    if (r != SL_RESULT_SUCCESS) return 1;
    r = (*mix)->Realize(mix, SL_BOOLEAN_FALSE);
    mark("output mix Realize -> %u", r);

    const SLuint32 rate = 48000;
    const SLuint32 channels = 2;

    SLDataLocator_AndroidSimpleBufferQueue src_loc{SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE, 4};
    SLDataFormat_PCM fmt{SL_DATAFORMAT_PCM,
                         channels,
                         rate * 1000, // milliHertz, as the spec has it
                         16,
                         16,
                         SL_SPEAKER_FRONT_LEFT | SL_SPEAKER_FRONT_RIGHT,
                         SL_BYTEORDER_LITTLEENDIAN};
    SLDataSource source{&src_loc, &fmt};
    SLDataLocator_OutputMix snk_loc{SL_DATALOCATOR_OUTPUTMIX, const_cast<void*>(static_cast<const void*>(mix))};
    SLDataSink sink{&snk_loc, nullptr};

    SLInterfaceID ids[3] = {SL_IID_ANDROIDSIMPLEBUFFERQUEUE, SL_IID_VOLUME, SL_IID_ANDROIDCONFIGURATION};
    SLboolean req[3] = {SL_BOOLEAN_TRUE, SL_BOOLEAN_TRUE, SL_BOOLEAN_FALSE};

    SLObjectItf player = nullptr;
    r = (*g_engine)->CreateAudioPlayer(g_engine, &player, &source, &sink, 3, ids, req);
    mark("CreateAudioPlayer -> %u", r);
    if (r != SL_RESULT_SUCCESS) return 1;
    r = (*player)->Realize(player, SL_BOOLEAN_FALSE);
    mark("player Realize -> %u", r);
    if (r != SL_RESULT_SUCCESS) return 1;

    SLPlayItf play = nullptr;
    r = (*player)->GetInterface(player, SL_IID_PLAY, &play);
    mark("player GetInterface(SL_IID_PLAY) -> %u", r);
    SLAndroidSimpleBufferQueueItf queue = nullptr;
    r = (*player)->GetInterface(player, SL_IID_ANDROIDSIMPLEBUFFERQUEUE, &queue);
    mark("player GetInterface(SL_IID_ANDROIDSIMPLEBUFFERQUEUE) -> %u", r);
    if (!play || !queue) return 1;

    g_feeder.queue = queue;
    g_feeder.channels = channels;
    g_feeder.amplitude = silent ? 0.0 : amplitude;
    g_feeder.step = 2 * M_PI * hz / rate;
    // 10 ms per buffer, four of them: the size WebRTC and most Android audio
    // paths use, and small enough that a stalled refill shows up as an underrun
    // rather than being papered over by a long buffer.
    const size_t frames = rate / 100;
    g_feeder.buffers.assign(4, std::vector<int16_t>(frames * channels, 0));

    (*queue)->RegisterCallback(queue, &buffer_drained, nullptr);
    for (auto& buf : g_feeder.buffers) {
        g_feeder.fill(buf);
        SLresult e = (*queue)->Enqueue(queue, buf.data(),
                                        static_cast<SLuint32>(buf.size() * sizeof(int16_t)));
        if (e != SL_RESULT_SUCCESS) mark("Enqueue -> %u", e);
    }
    g_feeder.next = g_feeder.buffers.size();

    mark("PLAY-BEGIN %s %.0f Hz amplitude %.5f", silent ? "(silence control)" : "(tone)", hz,
         g_feeder.amplitude);
    r = (*play)->SetPlayState(play, SL_PLAYSTATE_PLAYING);
    mark("SetPlayState(PLAYING) -> %u", r);

    sleep_ms(static_cast<int>(seconds * 1000));

    r = (*play)->SetPlayState(play, SL_PLAYSTATE_STOPPED);
    mark("PLAY-END SetPlayState(STOPPED) -> %u", r);
    mark("drain callbacks fired: %llu", static_cast<unsigned long long>(g_feeder.drains.load()));

    (*player)->Destroy(player);
    (*mix)->Destroy(mix);
    (*g_engine_obj)->Destroy(g_engine_obj);
    return g_feeder.drains.load() > 0 ? 0 : 1;
}

// ----------------------------------------------------------------- recording

/// Counts the buffers the recorder hands back and how loud they were. Peak
/// rather than a sample dump: the question this answers is "did real samples
/// arrive from a real microphone", and a room's noise floor answers it without
/// anything being recorded that anyone would want to keep.
struct RecordSink {
    SLAndroidSimpleBufferQueueItf queue = nullptr;
    std::vector<std::vector<int16_t>> buffers;
    size_t next = 0;
    std::atomic<uint64_t> filled{0};
    std::atomic<int> peak{0};

    void on_filled() {
        auto& buf = buffers[next % buffers.size()];
        ++next;
        int p = 0;
        for (int16_t s : buf) p = std::max(p, std::abs(static_cast<int>(s)));
        int prev = peak.load();
        while (p > prev && !peak.compare_exchange_weak(prev, p)) {}
        filled.fetch_add(1);
        (*queue)->Enqueue(queue, buf.data(), static_cast<SLuint32>(buf.size() * sizeof(int16_t)));
    }
};

RecordSink g_sink;

void buffer_filled(SLAndroidSimpleBufferQueueItf, void*) { g_sink.on_filled(); }

int cmd_record(double seconds) {
    if (!create_engine()) return 1;

    const SLuint32 rate = 48000;
    const SLuint32 channels = 1;

    SLDataLocator_IODevice src_loc{SL_DATALOCATOR_IODEVICE, SL_IODEVICE_AUDIOINPUT,
                                   SL_DEFAULTDEVICEID_AUDIOINPUT, nullptr};
    SLDataSource source{&src_loc, nullptr};
    SLDataLocator_AndroidSimpleBufferQueue snk_loc{SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE, 2};
    SLDataFormat_PCM fmt{SL_DATAFORMAT_PCM, channels, rate * 1000, 16, 16,
                         SL_SPEAKER_FRONT_CENTER,  SL_BYTEORDER_LITTLEENDIAN};
    SLDataSink sink{&snk_loc, &fmt};

    SLInterfaceID ids[2] = {SL_IID_ANDROIDSIMPLEBUFFERQUEUE, SL_IID_ANDROIDCONFIGURATION};
    SLboolean req[2] = {SL_BOOLEAN_TRUE, SL_BOOLEAN_FALSE};

    SLObjectItf recorder = nullptr;
    SLresult r = (*g_engine)->CreateAudioRecorder(g_engine, &recorder, &source, &sink, 2, ids, req);
    mark("CreateAudioRecorder -> %u  (capture streams open: %u)", r,
         cordial::audio::active_capture_streams());
    if (r != SL_RESULT_SUCCESS) {
        mark("MIC-NEVER-OPENED: no recorder object exists, so nothing can open the microphone");
        return 1;
    }

    r = (*recorder)->Realize(recorder, SL_BOOLEAN_FALSE);
    mark("RECORDER-REALIZED Realize -> %u  (capture streams open: %u)", r,
         cordial::audio::active_capture_streams());
    if (r != SL_RESULT_SUCCESS) return 1;

    SLRecordItf record = nullptr;
    r = (*recorder)->GetInterface(recorder, SL_IID_RECORD, &record);
    mark("recorder GetInterface(SL_IID_RECORD) -> %u", r);
    SLAndroidSimpleBufferQueueItf queue = nullptr;
    r = (*recorder)->GetInterface(recorder, SL_IID_ANDROIDSIMPLEBUFFERQUEUE, &queue);
    mark("recorder GetInterface(SL_IID_ANDROIDSIMPLEBUFFERQUEUE) -> %u", r);
    if (!record || !queue) return 1;

    // Realized but not started. This is the state a "muted but open" backend
    // would be indistinguishable from and the one an outside observer must see
    // nothing in: the pause here is long enough for a `pw-dump` sampler to
    // catch it.
    mark("IDLE-BEGIN realized, not recording (capture streams open: %u)",
         cordial::audio::active_capture_streams());
    sleep_ms(3000);
    mark("IDLE-END (capture streams open: %u)", cordial::audio::active_capture_streams());

    g_sink.queue = queue;
    g_sink.buffers.assign(2, std::vector<int16_t>(rate / 100 * channels, 0));
    (*queue)->RegisterCallback(queue, &buffer_filled, nullptr);
    for (auto& b : g_sink.buffers) {
        (*queue)->Enqueue(queue, b.data(), static_cast<SLuint32>(b.size() * sizeof(int16_t)));
    }
    g_sink.next = g_sink.buffers.size();

    r = (*record)->SetRecordState(record, SL_RECORDSTATE_RECORDING);
    mark("MIC-OPEN SetRecordState(RECORDING) -> %u  (capture streams open: %u)", r,
         cordial::audio::active_capture_streams());
    sleep_ms(static_cast<int>(seconds * 1000));

    SLuint32 state = 0;
    (*record)->GetRecordState(record, &state);
    mark("GetRecordState during recording -> %u (3 == RECORDING)", state);
    SLmillisecond pos = 0;
    (*record)->GetPosition(record, &pos);
    mark("GetPosition during recording -> %u ms", pos);

    // The case a "muted but open" backend would pass and this one must not:
    // pausing has to put the microphone out, not merely stop reading from it.
    r = (*record)->SetRecordState(record, SL_RECORDSTATE_PAUSED);
    mark("MIC-PAUSE SetRecordState(PAUSED) -> %u  (capture streams open: %u)", r,
         cordial::audio::active_capture_streams());
    sleep_ms(3000);
    mark("PAUSED-SETTLED (capture streams open: %u)", cordial::audio::active_capture_streams());

    r = (*record)->SetRecordState(record, SL_RECORDSTATE_RECORDING);
    mark("MIC-REOPEN SetRecordState(RECORDING) -> %u  (capture streams open: %u)", r,
         cordial::audio::active_capture_streams());
    sleep_ms(static_cast<int>(seconds * 1000));

    r = (*record)->SetRecordState(record, SL_RECORDSTATE_STOPPED);
    mark("MIC-CLOSE SetRecordState(STOPPED) -> %u  (capture streams open: %u)", r,
         cordial::audio::active_capture_streams());
    sleep_ms(3000);
    mark("STOPPED-SETTLED (capture streams open: %u)", cordial::audio::active_capture_streams());

    mark("buffers filled from the microphone: %llu, peak sample %d of 32767",
         static_cast<unsigned long long>(g_sink.filled.load()), g_sink.peak.load());

    (*recorder)->Destroy(recorder);
    mark("recorder destroyed (capture streams open: %u)", cordial::audio::active_capture_streams());
    (*g_engine_obj)->Destroy(g_engine_obj);
    return cordial::audio::active_capture_streams() == 0 ? 0 : 1;
}

// ------------------------------------------------ stopping from the callback
//
// The one recorder path that cannot be reached by driving the API politely
// from the outside, and the one that used to end the process. An OpenSL caller
// that wants N buffers stops the recorder from inside the buffer callback,
// which arrives on the pump thread — where a naive `stop` joins that thread to
// itself and `std::terminate` follows. Exercised here because "it does not
// crash" is not a property worth asserting from a reading of the code.

struct SelfStop {
    SLRecordItf record = nullptr;
    SLObjectItf recorder = nullptr;
    SLAndroidSimpleBufferQueueItf queue = nullptr;
    std::vector<std::vector<int16_t>> buffers;
    size_t next = 0;
    std::atomic<int> count{0};
    std::atomic<bool> done{false};
};

SelfStop g_selfstop;

void selfstop_filled(SLAndroidSimpleBufferQueueItf, void*) {
    int n = g_selfstop.count.fetch_add(1) + 1;
    if (n == 20) {
        mark("SELFSTOP calling SetRecordState(STOPPED) from inside the buffer callback");
        SLresult r = (*g_selfstop.record)->SetRecordState(g_selfstop.record, SL_RECORDSTATE_STOPPED);
        mark("SELFSTOP SetRecordState(STOPPED) returned %u  (capture streams open: %u)", r,
             cordial::audio::active_capture_streams());
        mark("SELFSTOP calling Destroy from inside the same callback");
        (*g_selfstop.recorder)->Destroy(g_selfstop.recorder);
        mark("SELFSTOP Destroy returned  (capture streams open: %u)",
             cordial::audio::active_capture_streams());
        g_selfstop.done.store(true);
        return;
    }
    if (n > 20) return; // recorder is gone; nothing to re-enqueue into
    auto& buf = g_selfstop.buffers[g_selfstop.next % g_selfstop.buffers.size()];
    ++g_selfstop.next;
    (*g_selfstop.queue)
        ->Enqueue(g_selfstop.queue, buf.data(), static_cast<SLuint32>(buf.size() * sizeof(int16_t)));
}

int cmd_record_selfstop() {
    if (!create_engine()) return 1;

    const SLuint32 rate = 48000;
    const SLuint32 channels = 1;
    SLDataLocator_IODevice src_loc{SL_DATALOCATOR_IODEVICE, SL_IODEVICE_AUDIOINPUT,
                                   SL_DEFAULTDEVICEID_AUDIOINPUT, nullptr};
    SLDataSource source{&src_loc, nullptr};
    SLDataLocator_AndroidSimpleBufferQueue snk_loc{SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE, 2};
    SLDataFormat_PCM fmt{SL_DATAFORMAT_PCM, channels, rate * 1000, 16, 16,
                         SL_SPEAKER_FRONT_CENTER,  SL_BYTEORDER_LITTLEENDIAN};
    SLDataSink sink{&snk_loc, &fmt};
    SLInterfaceID ids[1] = {SL_IID_ANDROIDSIMPLEBUFFERQUEUE};
    SLboolean req[1] = {SL_BOOLEAN_TRUE};

    SLObjectItf recorder = nullptr;
    SLresult r = (*g_engine)->CreateAudioRecorder(g_engine, &recorder, &source, &sink, 1, ids, req);
    if (r != SL_RESULT_SUCCESS) return 1;
    (*recorder)->Realize(recorder, SL_BOOLEAN_FALSE);

    SLRecordItf record = nullptr;
    (*recorder)->GetInterface(recorder, SL_IID_RECORD, &record);
    SLAndroidSimpleBufferQueueItf queue = nullptr;
    (*recorder)->GetInterface(recorder, SL_IID_ANDROIDSIMPLEBUFFERQUEUE, &queue);
    if (!record || !queue) return 1;

    g_selfstop.record = record;
    g_selfstop.recorder = recorder;
    g_selfstop.queue = queue;
    g_selfstop.buffers.assign(2, std::vector<int16_t>(rate / 100 * channels, 0));
    (*queue)->RegisterCallback(queue, &selfstop_filled, nullptr);
    for (auto& b : g_selfstop.buffers) {
        (*queue)->Enqueue(queue, b.data(), static_cast<SLuint32>(b.size() * sizeof(int16_t)));
    }
    g_selfstop.next = g_selfstop.buffers.size();

    (*record)->SetRecordState(record, SL_RECORDSTATE_RECORDING);
    mark("SELFSTOP recording; will stop and destroy from the 20th buffer callback");
    for (int i = 0; i < 100 && !g_selfstop.done.load(); ++i) sleep_ms(100);
    sleep_ms(1000); // let the detached pump unwind and free itself

    mark("SELFSTOP survived: %d buffers, capture streams open: %u", g_selfstop.count.load(),
         cordial::audio::active_capture_streams());
    (*g_engine_obj)->Destroy(g_engine_obj);
    return (g_selfstop.done.load() && cordial::audio::active_capture_streams() == 0) ? 0 : 1;
}

} // namespace

int main(int argc, char** argv) {
    const std::string cmd = argc > 1 ? argv[1] : "devices";
    double seconds = 3.0;
    double amplitude = 1.0 / 512.0; // see the header: about -54 dBFS
    double hz = 440.0;
    for (int i = 2; i < argc; ++i) {
        std::string a = argv[i];
        if (a == "--seconds" && i + 1 < argc) seconds = std::atof(argv[++i]);
        else if (a == "--amplitude" && i + 1 < argc) amplitude = std::atof(argv[++i]);
        else if (a == "--hz" && i + 1 < argc) hz = std::atof(argv[++i]);
    }

    if (cmd == "devices") return cmd_devices();
    if (cmd == "play") return cmd_play(seconds, amplitude, hz, false);
    if (cmd == "silence") return cmd_play(seconds, amplitude, hz, true);
    if (cmd == "record") return cmd_record(seconds);
    if (cmd == "record-selfstop") return cmd_record_selfstop();
    std::fprintf(stderr, "usage: audio_probe devices|play|silence|record|record-selfstop [--seconds N] "
                          "[--amplitude A] [--hz F]\n");
    return 2;
}
