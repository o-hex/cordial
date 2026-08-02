// Checks that the buffer-queue fill path in `pipewire_backend.cpp` pads an
// underrun with silence rather than replaying stale buffer contents — see
// the comment on `testing::fill_pcm` in `pipewire_backend.h` for why that
// distinction matters: a periodic replay of old samples is exactly what
// turns a silent gap into an audible tone, and a real one reached a
// developer's speakers once already because a verification harness for this
// backend used a live, audible stream when a check like this one would have
// done the job with no PipeWire session, no hardware and no way to make a
// sound.
//
// Deliberately does not link against a live PipeWire connection: it only
// exercises `testing::fill_pcm()`, the pure copy-and-pad logic
// `Impl::process()` calls every cycle, so it runs anywhere this tree builds
// with `pipewire-devel` present, with no daemon, no session, and nothing
// audible possible.

#include "pipewire_backend.h"

#include <cassert>
#include <cstdio>
#include <cstring>

using cordial::audio::testing::fill_pcm;
using cordial::audio::testing::PendingBuffer;

namespace {

void a_full_queue_fills_the_buffer_exactly() {
    uint8_t source[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    std::deque<PendingBuffer> pending;
    pending.push_back({source, sizeof(source), 0, nullptr});

    uint8_t dst[8];
    std::memset(dst, 0xAA, sizeof(dst));
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 0);
    assert(std::memcmp(dst, source, sizeof(source)) == 0);
    assert(drained.size() == 1);
    assert(pending.empty());
    std::printf("ok: a_full_queue_fills_the_buffer_exactly\n");
}

void an_empty_queue_produces_silence_not_stale_bytes() {
    std::deque<PendingBuffer> pending;
    uint8_t dst[8];
    // A recognisable non-zero pattern first, matching what a reused
    // pw_stream buffer would actually hold left over from a previous cycle
    // — the bug this guards against is exactly this pattern surviving into
    // the output.
    std::memset(dst, 0xAA, sizeof(dst));

    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == sizeof(dst));
    for (uint8_t b : dst) {
        assert(b == 0);
    }
    assert(drained.empty());
    std::printf("ok: an_empty_queue_produces_silence_not_stale_bytes\n");
}

void a_short_queue_fills_what_it_has_and_pads_the_rest_with_silence() {
    uint8_t source[3] = {9, 9, 9};
    std::deque<PendingBuffer> pending;
    pending.push_back({source, sizeof(source), 0, nullptr});

    uint8_t dst[8];
    std::memset(dst, 0xAA, sizeof(dst));
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 5);
    assert(std::memcmp(dst, source, sizeof(source)) == 0);
    for (size_t i = 3; i < 8; ++i) {
        assert(dst[i] == 0);
    }
    assert(drained.size() == 1);
    std::printf("ok: a_short_queue_fills_what_it_has_and_pads_the_rest_with_silence\n");
}

void a_partially_consumed_buffer_is_not_dropped_early() {
    uint8_t source[4] = {1, 2, 3, 4};
    std::deque<PendingBuffer> pending;
    pending.push_back({source, sizeof(source), 0, reinterpret_cast<void*>(0x1234)});

    uint8_t dst[2];
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 0);
    assert(dst[0] == 1 && dst[1] == 2);
    // Not fully consumed yet: still queued, no drain callback due for it.
    assert(drained.empty());
    assert(pending.size() == 1);
    assert(pending.front().offset == 2);
    std::printf("ok: a_partially_consumed_buffer_is_not_dropped_early\n");
}

void multiple_buffers_are_drained_in_order_within_one_fill() {
    uint8_t a[2] = {1, 2};
    uint8_t b[2] = {3, 4};
    std::deque<PendingBuffer> pending;
    pending.push_back({a, sizeof(a), 0, reinterpret_cast<void*>(1)});
    pending.push_back({b, sizeof(b), 0, reinterpret_cast<void*>(2)});

    uint8_t dst[4];
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 0);
    uint8_t expect[4] = {1, 2, 3, 4};
    assert(std::memcmp(dst, expect, sizeof(expect)) == 0);
    assert(drained.size() == 2);
    assert(drained[0] == reinterpret_cast<void*>(1));
    assert(drained[1] == reinterpret_cast<void*>(2));
    assert(pending.empty());
    std::printf("ok: multiple_buffers_are_drained_in_order_within_one_fill\n");
}

// The privacy rule, checked the only way it can be checked without a session:
// by construction. A `CaptureStream` that has not been opened must hold no
// PipeWire resource, and the process-wide count of open capture streams must
// still be zero after one has been built and destroyed. This does not prove
// the live case — `pw-top` and the desktop's microphone indicator do that, and
// the report accompanying this change carries that evidence — but it does pin
// the invariant that the live case rests on, and it will fail loudly if
// someone later moves the `open()` call into the constructor.
void a_capture_stream_holds_nothing_until_it_is_opened() {
    assert(cordial::audio::active_capture_streams() == 0);
    {
        cordial::audio::CaptureStream capture;
        assert(!capture.is_open());
        assert(cordial::audio::active_capture_streams() == 0);

        // A read from a stream that was never opened returns no samples
        // rather than blocking for some that will never arrive, and must not
        // open anything in order to answer.
        uint8_t buf[64];
        std::memset(buf, 0xAA, sizeof(buf));
        assert(capture.read(buf, sizeof(buf)) == 0);
        assert(cordial::audio::active_capture_streams() == 0);
    }
    assert(cordial::audio::active_capture_streams() == 0);
    std::printf("ok: a_capture_stream_holds_nothing_until_it_is_opened\n");
}

} // namespace

int main() {
    a_full_queue_fills_the_buffer_exactly();
    an_empty_queue_produces_silence_not_stale_bytes();
    a_short_queue_fills_what_it_has_and_pads_the_rest_with_silence();
    a_partially_consumed_buffer_is_not_dropped_early();
    multiple_buffers_are_drained_in_order_within_one_fill();
    a_capture_stream_holds_nothing_until_it_is_opened();
    std::printf("all pipewire_backend fill_pcm checks passed\n");
    return 0;
}
