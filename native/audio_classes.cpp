// Roblox's Android audio surface, answered from PipeWire.
//
// This is the Java half of audio. The native half — `libOpenSLES.so`, which
// FMOD uses for game and UI sound — lives in `opensles.cpp` and reaches
// PipeWire through the same backend; nothing here duplicates it. What is here
// are the two things that are *not* OpenSL ES and are easy to assume are:
//
// 1. **Device enumeration is Java.** Roblox asks
//    `AudioManager.getDevices(int)` for an `AudioDeviceInfo[]` and reads seven
//    getters off each element. `SLOutputMixItf::GetDestinationOutputDeviceIDs`
//    exists in `opensles.cpp` and reports zero devices, which is correct — the
//    OpenSL routing surface genuinely is not how Android enumerates audio
//    hardware, and it is not where a device picker gets its list.
//
// 2. **The microphone is not OpenSL ES either.** Roblox records through
//    `org.webrtc.voiceengine.WebRtcAudioRecord`, which wraps
//    `android.media.AudioRecord`. `SLEngineItf::CreateAudioRecorder` in
//    `opensles.cpp` reports `SL_RESULT_FEATURE_UNSUPPORTED` and continues to,
//    and that is not a gap left by this file: it is a path Roblox's Android
//    build does not record through, so implementing it would add a second way
//    to open the microphone with nothing asking for one. One door is easier to
//    keep shut than two.
//
// Every method below was taken from the shipping dex with `dexproto.py`, not
// from memory of the Android SDK. The seven `AudioDeviceInfo` getters, the
// `getDevices(I)[Landroid/media/AudioDeviceInfo;` shape, the
// `AudioRecord.<init>(IIIII)` argument count and
// `AudioRecord.read(Ljava/nio/ByteBuffer;I)I` rather than the `byte[]`
// overload are all declared surface, and anything not declared there is not
// implemented here.
//
// -------------------------------------------------------------------------
// THE MICROPHONE RULE, which the rest of this file is arranged around
// -------------------------------------------------------------------------
//
// No PipeWire capture stream may exist while Roblox is not recording.
//
// Not paused, not muted, not connected-but-inactive — absent. A capture stream
// in any of those states still keeps the desktop's microphone indicator lit
// and still shows every other application that Cordial is holding the capture
// device, which is the harm; whether samples are flowing is not the part a
// user can see. `CaptureStream::close()` therefore destroys the `pw_stream`
// rather than deactivating it, and the only two places in Cordial that call
// `CaptureStream::open()` are `AudioRecord::startRecording` and
// `WebRtcAudioRecord::startRecording` below.
//
// Two consequences worth stating because they are the kind of thing a later
// change makes by accident:
//
// * **Enumeration must not open anything.** Listing a microphone is not using
//   one. `enumerate_devices()` walks the PipeWire registry and reads
//   properties; it never constructs a `CaptureStream`. If a device picker ever
//   starts lighting the microphone indicator, that invariant is what broke.
//
// * **Failure closes.** Every path below that cannot honour the rule reports
//   failure instead of opening the microphone anyway — including the case
//   where recording starts successfully but the engine's own callback cannot
//   be reached, where the stream is closed again immediately rather than left
//   open delivering samples to nobody. A microphone held open for a recording
//   that is not happening is exactly the state this file exists to prevent.

#include <jni.h>
#include <jnivm.h>

#include <atomic>
#include <cstdio>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

#include "pipewire_backend.h"

namespace cordial {
namespace {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using String = jnivm::String;

inline std::shared_ptr<String> str(const std::string& v) {
    return std::make_shared<String>(v);
}

/// `getProductName()` is declared `()Ljava/lang/CharSequence;` in the dex, not
/// `()Ljava/lang/String;`, and libjnivm derives the descriptor it matches a
/// hook by from the hook's own C++ types. A `getProductName` returning
/// `shared_ptr<String>` therefore registers as `()Ljava/lang/String;` and
/// never matches the call Roblox compiled — which is not a build error and not
/// a run-time error either. It is a device list where every entry has a null
/// name, which is exactly what the first run of this file produced:
///
///     [0] getId=220 getType=9 isSource=false getProductName="(null)"
///
/// Deriving from `jnivm::String` rather than from `Object` is what makes the
/// returned object still be a real Java string to everything downstream —
/// `GetStringUTFChars` and friends recover it by `dynamic_cast`, and a
/// `CharSequence` that was not one would be a null name again by a longer
/// route. This is the same trick `accessibility.cpp` uses for the parameter
/// direction, in the one direction that file did not need.
class CharSequence : public String {
public:
    explicit CharSequence(const std::string& v) : String(v) {}
    static void Register(ENV* env) { env->GetClass<CharSequence>("java/lang/CharSequence"); }
};

inline std::shared_ptr<CharSequence> chars(const std::string& v) {
    return std::make_shared<CharSequence>(v);
}

// ------------------------------------------------- android.media constants
//
// From AOSP's public `android.media` API — the documented constant values
// every Android application is compiled against, not anything read out of
// Roblox. Written out rather than included because there is no Android SDK in
// this tree to include them from.

constexpr jint GET_DEVICES_INPUTS = 0x0001;
constexpr jint GET_DEVICES_OUTPUTS = 0x0002;

constexpr jint TYPE_UNKNOWN = 0;
constexpr jint TYPE_BUILTIN_SPEAKER = 2;
constexpr jint TYPE_WIRED_HEADSET = 3;
constexpr jint TYPE_WIRED_HEADPHONES = 4;
constexpr jint TYPE_BLUETOOTH_SCO = 7;
constexpr jint TYPE_BLUETOOTH_A2DP = 8;
constexpr jint TYPE_HDMI = 9;
constexpr jint TYPE_USB_DEVICE = 11;
constexpr jint TYPE_BUILTIN_MIC = 15;
constexpr jint TYPE_USB_HEADSET = 22;

constexpr jint ENCODING_PCM_16BIT = 2;

constexpr jint AUDIO_RECORD_STATE_UNINITIALIZED = 0;
constexpr jint AUDIO_RECORD_STATE_INITIALIZED = 1;
constexpr jint RECORDSTATE_STOPPED = 1;
constexpr jint RECORDSTATE_RECORDING = 3;

/// Android's own default and the rate WebRTC's voice engine asks for. Used
/// wherever a rate has to be reported before a stream exists to ask.
constexpr jint DEFAULT_SAMPLE_RATE = 48000;

bool contains(const std::string& haystack, const char* needle) {
    return haystack.find(needle) != std::string::npos;
}

/// Lowercased copy, so the name matching below does not have to guess at
/// PipeWire's capitalisation (`HiFi__HDMI1__sink`, `hdmi-stereo`, `Speaker`).
std::string lower(const std::string& s) {
    std::string out = s;
    for (char& c : out) {
        if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
    }
    return out;
}

/// The one genuinely interpretive step in this file: PipeWire describes a node
/// by how it is plugged in, Android describes it by what it is, and the two
/// vocabularies do not line up exactly.
///
/// The rule applied here is to answer from the most authoritative signal
/// available and, where none of them settles it, to answer `TYPE_UNKNOWN`
/// rather than the most plausible-sounding constant. `TYPE_BUILTIN_SPEAKER` is
/// a specific claim about hardware; guessing it for anything unrecognised
/// would be wrong roughly as often as it was right, and a device picker
/// showing a confidently wrong icon is harder to distrust than one showing an
/// honestly generic one.
///
/// The signals, most trustworthy first:
///
/// * `object.path`'s prefix names the SPA plugin that created the node —
///   `bluez5:` cannot be anything but Bluetooth, and unlike a name it is not
///   something a user can rename.
/// * `node.name` follows PipeWire's stable device-naming convention, in which
///   `.usb-` and `.pci-` are the bus and the `HiFi__<profile>__sink` tail is
///   the ALSA UCM profile. `hdmi`, `speaker`, `headphone`, `headset` and `mic`
///   appearing there are conventions, not guarantees, which is why they rank
///   below `object.path`.
/// * Direction, which decides between the paired Bluetooth constants: A2DP is
///   the playback profile and SCO the (bidirectional) headset one, so a
///   Bluetooth *source* is SCO by construction.
jint android_device_type(const audio::DeviceInfo& d) {
    const std::string path = lower(d.object_path);
    const std::string name = lower(d.node_name);
    const std::string nick = lower(d.nick);

    if (path.rfind("bluez5:", 0) == 0 || contains(name, "bluez_")) {
        return d.is_source ? TYPE_BLUETOOTH_SCO : TYPE_BLUETOOTH_A2DP;
    }
    if (contains(name, "hdmi") || contains(nick, "hdmi")) {
        // Android has no "DisplayPort"; HDMI is the constant it routes
        // display audio through, and a DP output is the same path here.
        return TYPE_HDMI;
    }
    if (contains(name, ".usb-") || contains(path, "usb")) {
        if (contains(name, "headset") || contains(nick, "headset")) return TYPE_USB_HEADSET;
        return TYPE_USB_DEVICE;
    }
    if (contains(name, "headset") || contains(nick, "headset")) return TYPE_WIRED_HEADSET;
    if (contains(name, "headphone") || contains(nick, "headphone")) return TYPE_WIRED_HEADPHONES;
    if (d.is_source && (contains(name, "mic") || contains(nick, "mic"))) return TYPE_BUILTIN_MIC;
    if (!d.is_source && (contains(name, "speaker") || contains(nick, "speaker"))) {
        return TYPE_BUILTIN_SPEAKER;
    }
    // A line input, a virtual sink, a card whose UCM profile is named
    // something this does not recognise. Android documents TYPE_UNKNOWN for
    // exactly this and Roblox is entitled to see it rather than a flattering
    // guess.
    return TYPE_UNKNOWN;
}

// ------------------------------------------------ android.media.AudioDeviceInfo

/// `android.media.AudioDeviceInfo`
///
/// A snapshot, not a live view. Android's own `AudioDeviceInfo` is immutable
/// and describes the device as it was when `getDevices` returned it, so
/// copying PipeWire's answer into the object at construction is the faithful
/// translation as well as the simple one: a getter that re-queried the session
/// could report a different device than the one the caller thinks it is
/// holding.
class AudioDeviceInfo : public Object {
public:
    jint id = 0;
    jint type = TYPE_UNKNOWN;
    bool source = false;
    std::string productName;

    /// PipeWire converts on the graph edge, so a device's *native* rate is not
    /// the set of rates a stream may ask for. These are the rates Cordial will
    /// actually accept from Roblox and honour, which is what the getter is
    /// asked to describe.
    static std::shared_ptr<jnivm::Array<jint>> rates(ENV*, Object*) {
        static const jint kRates[] = {8000, 11025, 16000, 22050, 32000, 44100, 48000};
        auto out = std::make_shared<jnivm::Array<jint>>(
            static_cast<jsize>(sizeof(kRates) / sizeof(kRates[0])));
        std::memcpy(out->getArray(), kRates, sizeof(kRates));
        return out;
    }

    static std::shared_ptr<jnivm::Array<jint>> channels(ENV*, Object*) {
        static const jint kCounts[] = {1, 2};
        auto out = std::make_shared<jnivm::Array<jint>>(2);
        std::memcpy(out->getArray(), kCounts, sizeof(kCounts));
        return out;
    }

    /// One entry, and deliberately only one. `CaptureStream` negotiates
    /// `SPA_AUDIO_FORMAT_S16` and nothing else, and `PlaybackStream` reports
    /// failure for a layout it cannot describe rather than reinterpreting the
    /// bytes; advertising a float or 8-bit encoding here would be a promise
    /// made in this file that the backend breaks in another.
    static std::shared_ptr<jnivm::Array<jint>> encodings(ENV*, Object*) {
        auto out = std::make_shared<jnivm::Array<jint>>(1);
        out->getArray()[0] = ENCODING_PCM_16BIT;
        return out;
    }

    static jint getId(ENV*, Object* self) { return self_of(self)->id; }
    static jint getType(ENV*, Object* self) { return self_of(self)->type; }
    static jboolean isSource(ENV*, Object* self) { return self_of(self)->source; }

    /// PipeWire's `node.description` verbatim — "Raptor Lake-P/U/H cAVS
    /// Speaker", the same string the user's own volume control shows. The
    /// point of a device picker is that the user recognises the entry, so
    /// anything invented here (a tidied-up name, a Cordial-branded one) makes
    /// the list worse however much better it reads.
    static std::shared_ptr<CharSequence> getProductName(ENV*, Object* self) {
        return chars(self_of(self)->productName);
    }

    /// The same answer under the `()Ljava/lang/String;` descriptor. Registered
    /// alongside the declared `CharSequence` one for the reason
    /// `accessibility.cpp` registers both of its `setClassName` overloads: it
    /// costs one line, and the failure it prevents is silent.
    static std::shared_ptr<String> getProductNameStr(ENV*, Object* self) {
        return str(self_of(self)->productName);
    }

    static void Register(ENV* env) {
        env->GetClass<AudioDeviceInfo>("android/media/AudioDeviceInfo");
        auto c = env->GetClass("android/media/AudioDeviceInfo");
        c->HookInstanceFunction(env, "getId", &AudioDeviceInfo::getId);
        c->HookInstanceFunction(env, "getType", &AudioDeviceInfo::getType);
        c->HookInstanceFunction(env, "isSource", &AudioDeviceInfo::isSource);
        c->HookInstanceFunction(env, "getProductName", &AudioDeviceInfo::getProductName);
        c->HookInstanceFunction(env, "getProductName", &AudioDeviceInfo::getProductNameStr);
        c->HookInstanceFunction(env, "getSampleRates", &AudioDeviceInfo::rates);
        c->HookInstanceFunction(env, "getChannelCounts", &AudioDeviceInfo::channels);
        c->HookInstanceFunction(env, "getEncodings", &AudioDeviceInfo::encodings);
    }

private:
    static AudioDeviceInfo* self_of(Object* o) {
        static AudioDeviceInfo fallback;
        auto* d = dynamic_cast<AudioDeviceInfo*>(o);
        return d ? d : &fallback;
    }
};

std::shared_ptr<AudioDeviceInfo> make_device_info(const audio::DeviceInfo& d) {
    auto info = std::make_shared<AudioDeviceInfo>();
    info->id = static_cast<jint>(d.id);
    info->source = d.is_source;
    info->type = android_device_type(d);
    info->productName = !d.description.empty() ? d.description
                        : !d.nick.empty()      ? d.nick
                                                : d.node_name;
    return info;
}

/// The PipeWire node name of the current default source, or empty if the
/// session names none. Read at the moment recording starts rather than cached,
/// so that switching the desktop's default microphone between two Roblox voice
/// sessions is picked up without restarting the client.
std::string default_source_node_name() {
    for (const audio::DeviceInfo& d : audio::enumerate_devices()) {
        if (d.is_source && d.is_default) return d.node_name;
    }
    return {};
}

// ---------------------------------------------------- android.media.AudioManager

/// `android.media.AudioManager`
///
/// Only the methods the dex declares, and among those only the ones that have
/// a truthful answer on this host. Everything to do with Bluetooth SCO
/// (`startBluetoothSco`, `setBluetoothScoOn`, `isBluetoothScoOn`) is left
/// unimplemented rather than answered `false`: SCO is a telephony routing mode
/// Android's audio flinger owns, PipeWire has no equivalent to report on, and
/// a confident `false` would be a claim about hardware state rather than an
/// admission of not knowing.
class AudioManager : public Object {
public:
    /// `getDevices(int flags)` — the device list, and the reason this class
    /// exists.
    ///
    /// The default device is placed first within its direction. Android's
    /// `AudioDeviceInfo` has no `isDefault()` and the dex does not declare
    /// one, so ordering is the only channel available for saying which device
    /// the host would pick — and it is the channel Android's own
    /// implementation uses, so a caller that takes element zero gets the
    /// answer it would get on a phone.
    static std::shared_ptr<jnivm::Array<AudioDeviceInfo>> getDevices(ENV*, Object*, jint flags) {
        const bool want_inputs = (flags & GET_DEVICES_INPUTS) != 0;
        const bool want_outputs = (flags & GET_DEVICES_OUTPUTS) != 0;

        std::vector<std::shared_ptr<AudioDeviceInfo>> chosen;
        std::vector<audio::DeviceInfo> devices = audio::enumerate_devices();
        // Two passes so defaults lead, without sorting the vector and
        // disturbing the order PipeWire reported the rest in.
        for (bool defaults_pass : {true, false}) {
            for (const audio::DeviceInfo& d : devices) {
                if (d.is_default != defaults_pass) continue;
                if (d.is_source && !want_inputs) continue;
                if (!d.is_source && !want_outputs) continue;
                chosen.push_back(make_device_info(d));
            }
        }

        auto out = std::make_shared<jnivm::Array<AudioDeviceInfo>>(static_cast<jsize>(chosen.size()));
        for (size_t i = 0; i < chosen.size(); ++i) {
            (*out)[static_cast<jint>(i)] = chosen[i];
        }
        return out;
    }

    /// `getCommunicationDevice()` — Android's name for "the device voice
    /// audio is going to and coming from right now". PipeWire's default
    /// source is the honest answer: it is what a capture stream that names no
    /// target will be connected to, which is precisely what
    /// `CaptureStream::open` does when Roblox starts recording.
    ///
    /// Null when the session names no default, rather than the first source
    /// that happens to exist. A caller that gets null knows it does not know;
    /// a caller handed an arbitrary device does not.
    static std::shared_ptr<AudioDeviceInfo> getCommunicationDevice(ENV*, Object*) {
        for (const audio::DeviceInfo& d : audio::enumerate_devices()) {
            if (d.is_source && d.is_default) return make_device_info(d);
        }
        return nullptr;
    }

    static jboolean isMicrophoneMute(ENV*, Object*) {
        // Cordial has no global microphone mute of its own, and reporting the
        // *host's* mute state would be reporting on a control Roblox cannot
        // then operate through `setMicrophoneMute`. False is the truthful
        // answer to "is Cordial muting the microphone", which is the question
        // this object is being asked.
        return false;
    }

    static jboolean isVolumeFixed(ENV*, Object*) {
        // The host's mixer is adjustable, per stream, and `SLVolumeItf`
        // already routes Roblox's own volume changes to it.
        return false;
    }

    static jboolean isSpeakerphoneOn(ENV*, Object*) {
        // A desktop has no earpiece to be the alternative to a speakerphone,
        // so audio is always going to what Android would call the speaker.
        return true;
    }

    static jboolean isMusicActive(ENV*, Object*) {
        // Would require watching every other client's streams in the session
        // to answer, which is a great deal of machinery for a hint Roblox uses
        // to duck its own audio. False is the conservative answer: it makes
        // Roblox play normally rather than quietly for a reason the user
        // cannot see.
        return false;
    }

    static void Register(ENV* env) {
        env->GetClass<AudioManager>("android/media/AudioManager");
        auto c = env->GetClass("android/media/AudioManager");
        c->HookInstanceFunction(env, "getDevices", &AudioManager::getDevices);
        c->HookInstanceFunction(env, "getCommunicationDevice", &AudioManager::getCommunicationDevice);
        c->HookInstanceFunction(env, "isMicrophoneMute", &AudioManager::isMicrophoneMute);
        c->HookInstanceFunction(env, "isVolumeFixed", &AudioManager::isVolumeFixed);
        c->HookInstanceFunction(env, "isSpeakerphoneOn", &AudioManager::isSpeakerphoneOn);
        c->HookInstanceFunction(env, "isMusicActive", &AudioManager::isMusicActive);
    }
};

// ----------------------------------------------------- android.media.AudioRecord

/// `android.media.AudioRecord`
///
/// The gate itself. Constructing one of these opens nothing; only
/// `startRecording()` does, and `stop()` and `release()` both close.
///
/// `getMinBufferSize` is declared static in the dex and is answered without
/// any object existing, which is worth noting because it is the one method
/// here that a caller reaches before deciding whether to record at all —
/// another reason it must not touch the microphone.
class AudioRecord : public Object {
public:
    audio::CaptureStream capture;
    jint sampleRate = DEFAULT_SAMPLE_RATE;
    jint channelCount = 1;
    jint bufferSizeBytes = 0;
    std::atomic<jint> recordState{RECORDSTATE_STOPPED};
    std::string targetNode;

    /// `AudioRecord(int audioSource, int sampleRateInHz, int channelConfig,
    ///              int audioFormat, int bufferSizeInBytes)`
    ///
    /// `channelConfig` arrives as `CHANNEL_IN_MONO` (0x10) or
    /// `CHANNEL_IN_STEREO` (0x0c) rather than a count, which is the one place
    /// this constructor's arguments are not what their names suggest.
    static std::shared_ptr<AudioRecord> ctor(ENV*, Class*, jint /*audioSource*/, jint sampleRateInHz,
                                              jint channelConfig, jint audioFormat,
                                              jint bufferSizeInBytes) {
        auto r = std::make_shared<AudioRecord>();
        r->sampleRate = sampleRateInHz > 0 ? sampleRateInHz : DEFAULT_SAMPLE_RATE;
        r->channelCount = channelConfig == 0x0c ? 2 : 1;
        r->bufferSizeBytes = bufferSizeInBytes > 0 ? bufferSizeInBytes : 4096;
        if (audioFormat != ENCODING_PCM_16BIT) {
            // Reported now, at construction, rather than by producing silence
            // later. `getState()` staying UNINITIALIZED is how AudioRecord
            // says this, and every caller checks it before recording.
            std::fprintf(stderr,
                "E/Cordial-Audio           AudioRecord asked for encoding %d; only "
                "ENCODING_PCM_16BIT (2) is supported, so this recorder reports "
                "STATE_UNINITIALIZED and will not open the microphone.\n", audioFormat);
            r->sampleRate = 0; // makes getState() report UNINITIALIZED, below
        }
        return r;
    }

    static jint getMinBufferSize(ENV*, Class*, jint sampleRateInHz, jint channelConfig,
                                  jint /*audioFormat*/) {
        const jint channels = channelConfig == 0x0c ? 2 : 1;
        const jint rate = sampleRateInHz > 0 ? sampleRateInHz : DEFAULT_SAMPLE_RATE;
        // 20 ms of S16 frames, matching the callback period WebRTC's voice
        // engine uses. Android returns a hardware-derived figure here; there
        // is no hardware period to derive one from when PipeWire owns the
        // graph, so this is the smallest buffer the rest of this file is
        // actually prepared to keep fed.
        return channels * 2 * rate / 50;
    }

    static jint getState(ENV*, Object* self) {
        auto* r = as(self);
        return (r && r->sampleRate > 0) ? AUDIO_RECORD_STATE_INITIALIZED
                                         : AUDIO_RECORD_STATE_UNINITIALIZED;
    }
    static jint getRecordingState(ENV*, Object* self) {
        auto* r = as(self);
        return r ? r->recordState.load() : RECORDSTATE_STOPPED;
    }
    static jint getSampleRate(ENV*, Object* self) {
        auto* r = as(self);
        return r ? r->sampleRate : 0;
    }
    static jint getChannelCount(ENV*, Object* self) {
        auto* r = as(self);
        return r ? r->channelCount : 0;
    }
    static jint getAudioFormat(ENV*, Object*) { return ENCODING_PCM_16BIT; }
    static jint getBufferSizeInFrames(ENV*, Object* self) {
        auto* r = as(self);
        if (!r || r->channelCount == 0) return 0;
        return r->bufferSizeBytes / (r->channelCount * 2);
    }
    static jint getAudioSessionId(ENV*, Object*) {
        // Android uses this to tie effects (AEC, NS) to a recording session.
        // There are no such effects here to tie anything to, and zero is the
        // documented "no session" value rather than an id that would imply one.
        return 0;
    }

    /// **One of exactly two places in Cordial that open the microphone.**
    static void startRecording(ENV*, Object* self) {
        auto* r = as(self);
        if (!r || r->sampleRate <= 0) return;
        if (r->capture.is_open()) return;
        r->targetNode = default_source_node_name();
        if (!r->capture.open(static_cast<uint32_t>(r->sampleRate),
                             static_cast<uint32_t>(r->channelCount), r->targetNode)) {
            // Stays STOPPED. A recorder that reports RECORDING with no stream
            // behind it is the "stub that lies" this project keeps finding the
            // cost of: the caller would sit reading zero bytes forever with
            // nothing to explain why.
            std::fprintf(stderr,
                "E/Cordial-Audio           AudioRecord.startRecording could not open a "
                "capture stream; staying stopped rather than reporting a recording that "
                "is not happening.\n");
            return;
        }
        r->recordState.store(RECORDSTATE_RECORDING);
    }

    static void stop(ENV*, Object* self) {
        auto* r = as(self);
        if (!r) return;
        r->recordState.store(RECORDSTATE_STOPPED);
        r->capture.close();
    }

    static void release(ENV*, Object* self) {
        // Android permits release() without a preceding stop(), and a
        // released recorder must not be holding the device either.
        // CaptureStream::close() is idempotent, so the ordinary
        // stop()-then-release() pair costs nothing extra.
        stop(nullptr, self);
    }

    /// `read(ByteBuffer audioBuffer, int sizeInBytes)`
    static jint read(ENV*, Object* self, std::shared_ptr<jnivm::ByteBuffer> buffer,
                      jint sizeInBytes) {
        auto* r = as(self);
        if (!r || !buffer || sizeInBytes <= 0) return 0;
        void* dst = buffer->buffer;
        if (!dst) return 0;
        const jlong capacity = buffer->capacity;
        const uint32_t want =
            static_cast<uint32_t>(sizeInBytes < capacity ? sizeInBytes : capacity);
        return static_cast<jint>(r->capture.read(dst, want));
    }

    static void Register(ENV* env) {
        env->GetClass<AudioRecord>("android/media/AudioRecord");
        auto c = env->GetClass("android/media/AudioRecord");
        c->Hook(env, "<init>", &AudioRecord::ctor);
        c->Hook(env, "getMinBufferSize", &AudioRecord::getMinBufferSize);
        c->HookInstanceFunction(env, "getState", &AudioRecord::getState);
        c->HookInstanceFunction(env, "getRecordingState", &AudioRecord::getRecordingState);
        c->HookInstanceFunction(env, "getSampleRate", &AudioRecord::getSampleRate);
        c->HookInstanceFunction(env, "getChannelCount", &AudioRecord::getChannelCount);
        c->HookInstanceFunction(env, "getAudioFormat", &AudioRecord::getAudioFormat);
        c->HookInstanceFunction(env, "getBufferSizeInFrames", &AudioRecord::getBufferSizeInFrames);
        c->HookInstanceFunction(env, "getAudioSessionId", &AudioRecord::getAudioSessionId);
        c->HookInstanceFunction(env, "startRecording", &AudioRecord::startRecording);
        c->HookInstanceFunction(env, "stop", &AudioRecord::stop);
        c->HookInstanceFunction(env, "release", &AudioRecord::release);
        c->HookInstanceFunction(env, "read", &AudioRecord::read);
    }

private:
    static AudioRecord* as(Object* o) { return dynamic_cast<AudioRecord*>(o); }
};

// ------------------------------------- org.webrtc.voiceengine.WebRtcAudioManager

/// `org.webrtc.voiceengine.WebRtcAudioManager`
///
/// WebRTC's voice engine asks Java for the platform's audio parameters once,
/// at construction, and then builds its whole audio device module around the
/// answers. On real Android this class is Kotlin/Java running in the app; that
/// cannot execute under Cordial (there is no JVM — the same established
/// finding that governs `MainGameActivity.bootstrapTheApp`), so the class is
/// implemented here instead and answers from what Cordial can actually do.
///
/// **`init()` reports failure, deliberately, and that is the current state of
/// voice chat.** The parameter getters below are honest, but the uplink is
/// only half the path: WebRTC also needs `WebRtcAudioTrack` for the downlink,
/// which is not implemented, and an audio device module that has been told
/// initialisation succeeded will proceed to open a microphone in order to
/// send audio into a session it cannot play the other side of. Reporting
/// failure here keeps the gap where someone can find it and — the part that
/// matters for this file — means the microphone is never opened for a voice
/// session that cannot work. See the report accompanying this change.
class WebRtcAudioManager : public Object {
public:
    static jboolean init(ENV*, Object*) {
        static bool said = false;
        if (!said) {
            said = true;
            std::fprintf(stderr,
                "I/Cordial-Audio           WebRtcAudioManager.init reports failure: the "
                "voice-chat downlink (WebRtcAudioTrack) is not implemented, and starting an "
                "audio device module without it would open the microphone for a session "
                "with no playback. Voice chat is unavailable; the microphone stays shut.\n");
        }
        return false;
    }

    static void dispose(ENV*, Object*) {}

    static jint getNativeOutputSampleRate(ENV*, Object*) { return DEFAULT_SAMPLE_RATE; }
    static jint getSampleRateForApiLevel(ENV*, Object*) { return DEFAULT_SAMPLE_RATE; }

    // Mono in both directions: `CaptureStream` is opened mono for voice, and
    // claiming stereo would have WebRTC size its buffers for frames that never
    // arrive.
    static jboolean getStereoInput(ENV*, Object*) { return false; }
    static jboolean getStereoOutput(ENV*, Object*) { return false; }

    /// 10 ms at 48 kHz, WebRTC's own frame size. Not a hardware figure — there
    /// is no fixed hardware period to report when PipeWire negotiates the
    /// graph's quantum dynamically — but it is the period this backend is
    /// built to deliver, which is the honest thing for the caller to size
    /// itself against.
    static jint getLowLatencyInputFramesPerBuffer(ENV*, Object*) { return DEFAULT_SAMPLE_RATE / 100; }
    static jint getLowLatencyOutputFramesPerBuffer(ENV*, Object*) { return DEFAULT_SAMPLE_RATE / 100; }
    static jint getMinInputFrameSize(ENV*, Object*, jint sampleRate, jint channels) {
        return (sampleRate > 0 ? sampleRate : DEFAULT_SAMPLE_RATE) / 100 *
               (channels > 0 ? channels : 1);
    }
    static jint getMinOutputFrameSize(ENV*, Object*, jint sampleRate, jint channels) {
        return getMinInputFrameSize(nullptr, nullptr, sampleRate, channels);
    }

    // Android's hardware audio effects. There are none here — PipeWire's echo
    // canceller is a separate module the user opts into, not something Cordial
    // can claim on the device's behalf — and saying otherwise would have
    // WebRTC switch off its own software AEC in favour of one that does not
    // exist, which is audible as echo rather than as an error.
    static jboolean isAcousticEchoCancelerSupported(ENV*, Object*) { return false; }
    static jboolean isNoiseSuppressorSupported(ENV*, Object*) { return false; }
    static jboolean isLowLatencyInputSupported(ENV*, Object*) { return false; }
    static jboolean isLowLatencyOutputSupported(ENV*, Object*) { return false; }
    static jboolean isProAudioSupported(ENV*, Object*) { return false; }
    static jboolean isAAudioSupported(ENV*, Object*) { return false; }
    static jboolean hasEarpiece(ENV*, Object*) { return false; }
    static jboolean isCommunicationModeEnabled(ENV*, Object*) { return false; }
    static jboolean isDeviceBlacklistedForOpenSLESUsage(ENV*, Object*) { return false; }

    static void Register(ENV* env) {
        env->GetClass<WebRtcAudioManager>("org/webrtc/voiceengine/WebRtcAudioManager");
        auto c = env->GetClass("org/webrtc/voiceengine/WebRtcAudioManager");
        c->HookInstanceFunction(env, "init", &WebRtcAudioManager::init);
        c->HookInstanceFunction(env, "dispose", &WebRtcAudioManager::dispose);
        c->HookInstanceFunction(env, "getNativeOutputSampleRate",
                                &WebRtcAudioManager::getNativeOutputSampleRate);
        c->HookInstanceFunction(env, "getSampleRateForApiLevel",
                                &WebRtcAudioManager::getSampleRateForApiLevel);
        c->HookInstanceFunction(env, "getStereoInput", &WebRtcAudioManager::getStereoInput);
        c->HookInstanceFunction(env, "getStereoOutput", &WebRtcAudioManager::getStereoOutput);
        c->HookInstanceFunction(env, "getLowLatencyInputFramesPerBuffer",
                                &WebRtcAudioManager::getLowLatencyInputFramesPerBuffer);
        c->HookInstanceFunction(env, "getLowLatencyOutputFramesPerBuffer",
                                &WebRtcAudioManager::getLowLatencyOutputFramesPerBuffer);
        c->HookInstanceFunction(env, "getMinInputFrameSize",
                                &WebRtcAudioManager::getMinInputFrameSize);
        c->HookInstanceFunction(env, "getMinOutputFrameSize",
                                &WebRtcAudioManager::getMinOutputFrameSize);
        c->HookInstanceFunction(env, "isAcousticEchoCancelerSupported",
                                &WebRtcAudioManager::isAcousticEchoCancelerSupported);
        c->HookInstanceFunction(env, "isNoiseSuppressorSupported",
                                &WebRtcAudioManager::isNoiseSuppressorSupported);
        c->HookInstanceFunction(env, "isLowLatencyInputSupported",
                                &WebRtcAudioManager::isLowLatencyInputSupported);
        c->HookInstanceFunction(env, "isLowLatencyOutputSupported",
                                &WebRtcAudioManager::isLowLatencyOutputSupported);
        c->HookInstanceFunction(env, "isProAudioSupported", &WebRtcAudioManager::isProAudioSupported);
        c->HookInstanceFunction(env, "isAAudioSupported", &WebRtcAudioManager::isAAudioSupported);
        c->HookInstanceFunction(env, "hasEarpiece", &WebRtcAudioManager::hasEarpiece);
        c->HookInstanceFunction(env, "isCommunicationModeEnabled",
                                &WebRtcAudioManager::isCommunicationModeEnabled);
        c->HookInstanceFunction(env, "isDeviceBlacklistedForOpenSLESUsage",
                                &WebRtcAudioManager::isDeviceBlacklistedForOpenSLESUsage);
    }
};

// -------------------------------------- org.webrtc.voiceengine.WebRtcAudioRecord

/// `org.webrtc.voiceengine.WebRtcAudioRecord`
///
/// Roblox's actual microphone path, and therefore where the rule at the top of
/// this file has to hold in practice rather than in principle.
///
/// `initRecording` reports failure. That is not a placeholder: reaching this
/// class at all means `WebRtcAudioManager.init()` above already declined, so
/// an engine arriving here has ignored that answer, and the one thing this
/// file must not do is open the microphone for it anyway. Refusing keeps the
/// promise; the alternative — opening a capture stream and delivering samples
/// through `nativeDataIsRecorded` — is real work that belongs with a working
/// downlink, not ahead of it.
///
/// `stopRecording` still closes, and `setMicrophoneMute` is still recorded,
/// because both are cheap and both are the sort of thing that must already be
/// right on the day the rest is filled in.
class WebRtcAudioRecord : public Object {
public:
    audio::CaptureStream capture;
    std::atomic<bool> microphoneMuted{false};

    static jint initRecording(ENV*, Object*, jint sampleRate, jint channels) {
        std::fprintf(stderr,
            "I/Cordial-Audio           WebRtcAudioRecord.initRecording(%d Hz, %d ch) "
            "reports failure: voice chat has no downlink (see WebRtcAudioManager.init), so "
            "the microphone is not opened. No capture stream was created.\n",
            sampleRate, channels);
        // Negative is WebRTC's own "initialisation failed" for this method; a
        // zero frame count would be read as a successful init that produced an
        // empty buffer, which is the ambiguity a stub should never introduce.
        return -1;
    }

    static jboolean startRecording(ENV*, Object*) {
        // Unreachable if initRecording is honoured. Reported rather than
        // silently ignored, because an engine that got here has ignored a
        // failure and that is worth seeing in a log.
        std::fprintf(stderr,
            "W/Cordial-Audio           WebRtcAudioRecord.startRecording called after "
            "initRecording reported failure; refusing, and the microphone stays shut.\n");
        return false;
    }

    static jboolean stopRecording(ENV*, Object* self) {
        if (auto* r = dynamic_cast<WebRtcAudioRecord*>(self)) r->capture.close();
        return true;
    }

    static void setMicrophoneMute(ENV*, Object* self, jboolean mute) {
        if (auto* r = dynamic_cast<WebRtcAudioRecord*>(self)) r->microphoneMuted.store(mute);
    }

    // Both report that the hardware effect is unavailable, matching
    // WebRtcAudioManager above. WebRTC falls back to its own software AEC/NS,
    // which is the outcome that actually works here.
    static jboolean enableBuiltInAEC(ENV*, Object*, jboolean) { return false; }
    static jboolean enableBuiltInNS(ENV*, Object*, jboolean) { return false; }

    static jint getDefaultAudioSource(ENV*, Class*) {
        return 7; // MediaRecorder.AudioSource.VOICE_COMMUNICATION
    }

    static void Register(ENV* env) {
        env->GetClass<WebRtcAudioRecord>("org/webrtc/voiceengine/WebRtcAudioRecord");
        auto c = env->GetClass("org/webrtc/voiceengine/WebRtcAudioRecord");
        c->HookInstanceFunction(env, "initRecording", &WebRtcAudioRecord::initRecording);
        c->HookInstanceFunction(env, "startRecording", &WebRtcAudioRecord::startRecording);
        c->HookInstanceFunction(env, "stopRecording", &WebRtcAudioRecord::stopRecording);
        c->HookInstanceFunction(env, "setMicrophoneMute", &WebRtcAudioRecord::setMicrophoneMute);
        c->HookInstanceFunction(env, "enableBuiltInAEC", &WebRtcAudioRecord::enableBuiltInAEC);
        c->HookInstanceFunction(env, "enableBuiltInNS", &WebRtcAudioRecord::enableBuiltInNS);
        c->Hook(env, "getDefaultAudioSource", &WebRtcAudioRecord::getDefaultAudioSource);
    }
};

} // namespace

void register_audio_classes(jnivm::ENV* env) {
    CharSequence::Register(env);
    AudioDeviceInfo::Register(env);
    AudioManager::Register(env);
    AudioRecord::Register(env);
    WebRtcAudioManager::Register(env);
    WebRtcAudioRecord::Register(env);
}

} // namespace cordial
