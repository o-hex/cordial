// The host side of the OpenSL ES translation: a PCM playback stream backed by
// PipeWire, reached without linking against it.
//
// This header carries no PipeWire types, deliberately. `opensles.cpp` builds
// the OpenSL object model against this instead of `<pipewire/pipewire.h>`
// directly, so the object model compiles the same way whether or not this
// tree was configured with pipewire-devel present — see the top of
// `pipewire_backend.cpp` for how that split is enforced.

#include <cstddef>
#include <cstdint>
#include <deque>
#include <string>
#include <vector>

namespace cordial::audio {

/// True once a PipeWire session has been confirmed reachable: the library
/// loaded, `pw_init` ran, and a core connection completed a round trip. The
/// first call does the work and every later call returns the cached result,
/// so this is cheap to call from `slCreateEngine` on every attempt.
///
/// False covers three different failures on purpose — headers absent at
/// build time, `libpipewire-0.3.so.0` absent at run time, and a reachable
/// library with no session behind it (`PIPEWIRE_RUNTIME_DIR` unset, daemon
/// not running) — because the caller's response is the same for all three:
/// report failure rather than hand back an engine with no audio behind it.
bool pipewire_available();

/// One PipeWire node that carries audio, as the registry describes it.
///
/// Everything here is copied verbatim out of the node's global properties
/// rather than interpreted: the caller that wants an
/// `android.media.AudioDeviceInfo.TYPE_*` out of it is `audio_classes.cpp`,
/// and Android's vocabulary belongs in the file that speaks Android. What
/// this file is responsible for is that each string is what PipeWire
/// actually said, so that a wrong guess upstream is a wrong guess about
/// honest data rather than a guess compounded on a guess.
struct DeviceInfo {
    /// The PipeWire global id, used unchanged as `AudioDeviceInfo.getId()`.
    /// Android only requires that ids be unique among the devices reported
    /// at the same moment and stable while a device stays connected, which
    /// is exactly what a PipeWire global id already is — so there is no
    /// mapping table here to drift out of step with the session.
    uint32_t id = 0;
    /// `node.name`: the stable routing target (`alsa_output.pci-...`), which
    /// is what a stream sets `PW_KEY_TARGET_OBJECT` to. Not shown to a user.
    std::string node_name;
    /// `node.description`: what the user's own volume control calls this
    /// device, and therefore what `getProductName()` must return if the
    /// device picker is to be recognisable.
    std::string description;
    /// `node.nick`, when set — the short form ("Speaker", "HDMI 1").
    std::string nick;
    /// `object.path` ("alsa:acp:sofhdadsp:3:playback", "bluez5:..."), whose
    /// prefix names the backing API. The most reliable single signal for the
    /// Android device type, because it comes from the SPA plugin that
    /// created the node rather than from a name someone can rename.
    std::string object_path;
    /// `media.class` was `Audio/Source` rather than `Audio/Sink`; i.e. this
    /// device records. Maps straight onto `AudioDeviceInfo.isSource()`.
    bool is_source = false;
    /// This node is the one the `default` metadata's `default.audio.sink` /
    /// `default.audio.source` key names — the device the user's desktop
    /// sends audio to when nothing has asked for anything else.
    bool is_default = false;
};

/// Every audio sink and source the session currently has, defaults marked.
///
/// **This opens no stream of any kind.** Listing a microphone is not using
/// one, and the whole privacy gate in `audio_classes.cpp` rests on that
/// distinction holding here: this walks the registry, reads properties, and
/// disconnects the registry listener again. A capture stream exists only
/// after `CaptureStream::open`, which nothing on the enumeration path calls.
///
/// Returns empty when no session is reachable, which the caller must report
/// as "no devices" rather than substituting a plausible-looking one.
std::vector<DeviceInfo> enumerate_devices();

/// How many `CaptureStream`s currently hold an open `pw_stream`.
///
/// Exists so the privacy requirement can be *checked* rather than asserted:
/// `audio_classes.cpp` logs this when the engine stops recording, and the
/// test binary reads it to prove enumeration left it at zero. `pw-top` and
/// the desktop's own microphone indicator are the independent checks; this
/// is the one Cordial can make about itself.
uint32_t active_capture_streams();

/// One playback stream, backed by one `pw_stream`. `opensles.cpp` owns one of
/// these per realized `AudioPlayer` object.
///
/// The enqueue/drain contract mirrors `SLAndroidSimpleBufferQueueItf`
/// directly rather than adding a layer of translation: `enqueue` takes
/// ownership of nothing and copies nothing — the caller's buffer must stay
/// valid and unmodified until the drain callback fires for it, exactly as
/// the Android buffer queue documents. That symmetry is what makes the
/// threading tractable: PipeWire's realtime side drains our internal queue
/// under a single mutex and calls back out to the caller with the mutex
/// already released, so a callback that re-enters `enqueue` (the ordinary
/// OpenSL pattern — queue the next buffer as soon as the last one drains)
/// does not deadlock against itself.
class PlaybackStream {
public:
    PlaybackStream();
    ~PlaybackStream();

    PlaybackStream(const PlaybackStream&) = delete;
    PlaybackStream& operator=(const PlaybackStream&) = delete;

    /// Creates and connects the underlying `pw_stream`, negotiating exactly
    /// the format described (no resampling surprises: what Roblox put in
    /// `SLDataFormat_PCM` is what reaches PipeWire's converter). `rate` is in
    /// Hz — `opensles.cpp` divides the spec's milliHertz value by 1000 before
    /// calling this, which is where that conversion belongs, not here.
    /// `container_bits` and `big_endian` pick the exact `spa_audio_format`;
    /// combinations this backend does not know how to describe return false
    /// rather than guessing at a nearby one.
    bool open(uint32_t rate_hz, uint32_t channels, uint32_t bits_per_sample,
              uint32_t container_bits, bool big_endian, uint32_t max_pending_buffers);

    void close();

    /// Queues `size` bytes of interleaved PCM starting at `data`. Never
    /// blocks: it either appends to the pending list under a short-held lock
    /// or, if the list is already at the player's declared buffer count,
    /// returns false immediately for the caller to report as
    /// `SL_RESULT_BUFFER_INSUFFICIENT`.
    bool enqueue(const void* data, uint32_t size, void* buffer_context);

    /// Discards every pending buffer without invoking the drain callback for
    /// any of them — matching `SLAndroidSimpleBufferQueueItf::Clear`, which
    /// is a discard, not a fast-forward.
    void clear();

    /// Called from PipeWire's own thread the moment a previously enqueued
    /// buffer has been fully copied out (i.e. "consumed", in the buffer
    /// queue's vocabulary). Registering a new callback replaces the old one;
    /// there is exactly one, matching
    /// `SLAndroidSimpleBufferQueueItf::RegisterCallback`.
    using DrainCallback = void (*)(void* buffer_context, void* user);
    void set_drain_callback(DrainCallback cb, void* user);

    void set_active(bool active);
    void set_volume_linear(float linear);
    void set_mute(bool mute);

    struct QueueState {
        uint32_t count; ///< buffers currently pending (mirrors `SLAndroidSimpleBufferQueueState.count`)
        uint32_t index; ///< buffers enqueued since the last `open`/`clear` (mirrors `.index`)
    };
    QueueState state() const;

private:
    struct Impl;
    Impl* impl_;
};

/// One capture stream, backed by one `pw_stream` in the input direction.
///
/// The lifetime rule is the point of this class and is not negotiable: an
/// instance holds no PipeWire resource at all until `open()`, and holds none
/// again the moment `close()` returns. There is no paused state and no muted
/// state, because neither of those puts the desktop's microphone indicator
/// out and neither of those stops another application seeing Cordial holding
/// the capture device. A recording that has stopped must be indistinguishable
/// from a Cordial that never recorded, and the only way to be
/// indistinguishable is to actually not be there.
///
/// That is why the read side is a ring buffer owned by this object rather
/// than a borrowed-pointer arrangement like `PlaybackStream`'s: the caller
/// (`android.media.AudioRecord.read`) polls on its own thread whenever it
/// likes, including not at all, and none of that may keep the stream alive a
/// moment longer than the engine asked for.
class CaptureStream {
public:
    CaptureStream();
    ~CaptureStream();

    CaptureStream(const CaptureStream&) = delete;
    CaptureStream& operator=(const CaptureStream&) = delete;

    /// Creates and connects a `pw_stream` capturing S16 PCM at `rate_hz` and
    /// `channels`, from `target_node_name` if it is non-empty and from
    /// whatever PipeWire calls the default source otherwise.
    ///
    /// **This is the only function in Cordial that opens the microphone.**
    /// Every caller of it must be on a path the engine explicitly asked to
    /// record on; see `audio_classes.cpp`, where the only call sites are
    /// `AudioRecord.startRecording` and `WebRtcAudioRecord.startRecording`.
    bool open(uint32_t rate_hz, uint32_t channels, const std::string& target_node_name);

    /// Destroys the underlying `pw_stream` and drops every buffered sample.
    /// Idempotent, so the double-stop that a `stop()` followed by a
    /// `release()` produces costs nothing and is not an error.
    void close();

    bool is_open() const;

    /// Copies up to `size` bytes of captured interleaved S16 PCM into `dst`,
    /// returning how many bytes were written. Returns 0 rather than blocking
    /// when the stream is closed or nothing has arrived yet: a recorder that
    /// blocks forever on a device that will never produce samples is the
    /// failure mode this whole file exists to avoid.
    uint32_t read(void* dst, uint32_t size);

private:
    struct Impl;
    Impl* impl_;
};

/// Exposed only so `pipewire_backend_test.cpp` can check the underrun
/// behaviour without a live PipeWire session, hardware, or any risk of
/// producing sound: a periodic replay of stale buffer contents is exactly
/// what turns a silent gap into an audible tone, so the "pad with zeroed
/// silence rather than whatever the buffer held last cycle" rule is worth
/// checking on its own, not only by ear.
namespace testing {

struct PendingBuffer {
    const uint8_t* data;
    uint32_t size;
    uint32_t offset;
    void* context;
};

/// Copies from the front of `pending` into `dst[0, want)`, popping buffers
/// as they empty (and recording their `context` in `drained_contexts`, in
/// order) and zero-filling any shortfall. Returns the number of trailing
/// bytes that had to be silence-padded — zero on a clean fill. This is
/// exactly what `PlaybackStream::Impl::process()` does each PipeWire
/// cycle, factored out because that method also touches real `pw_buffer`
/// and `pw_stream` objects that only exist with a live stream connected.
uint32_t fill_pcm(std::deque<PendingBuffer>& pending, uint8_t* dst, uint32_t want,
                   std::vector<void*>& drained_contexts);

} // namespace testing

} // namespace cordial::audio
