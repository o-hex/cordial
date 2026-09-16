#include "aaudio_input_callback.h"

#include <algorithm>
#include <atomic>
#include <cassert>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <vector>

namespace {

using cordial::audio::InputCallbackDriver;

struct Source {
    std::vector<uint8_t> bytes;
    std::atomic<size_t> offset{0};
    uint32_t largest_read = UINT32_MAX;
    std::atomic<uint32_t> reads{0};
};

uint32_t read_source(void* opaque, void* dst, uint32_t size) {
    auto* source = static_cast<Source*>(opaque);
    source->reads.fetch_add(1);
    const size_t offset = source->offset.load();
    if (offset >= source->bytes.size()) return 0;
    const size_t available = source->bytes.size() - offset;
    const size_t count = std::min<size_t>({available, size, source->largest_read});
    std::memcpy(dst, source->bytes.data() + offset, count);
    source->offset.fetch_add(count);
    return static_cast<uint32_t>(count);
}

struct Result {
    std::mutex mutex;
    std::condition_variable changed;
    std::vector<uint8_t> bytes;
    uint32_t frames = 0;
    uint32_t calls = 0;
    uint32_t exits = 0;
    bool callback_stopped = false;
    bool continue_callback = true;
    bool cancel_in_callback = false;
    bool was_callback_thread = false;
    InputCallbackDriver* driver = nullptr;
};

bool receive(void* data, uint32_t frames, void* opaque) {
    auto* result = static_cast<Result*>(opaque);
    const size_t size = static_cast<size_t>(frames) * 2;
    {
        std::lock_guard<std::mutex> lock(result->mutex);
        result->bytes.assign(static_cast<uint8_t*>(data), static_cast<uint8_t*>(data) + size);
        result->frames = frames;
        ++result->calls;
        result->was_callback_thread = result->driver->is_callback_thread();
    }
    if (result->cancel_in_callback) result->driver->cancel();
    result->changed.notify_all();
    return result->continue_callback;
}

void exited(bool callback_stopped, void* opaque) {
    auto* result = static_cast<Result*>(opaque);
    {
        std::lock_guard<std::mutex> lock(result->mutex);
        ++result->exits;
        result->callback_stopped = callback_stopped;
    }
    result->changed.notify_all();
}

bool wait_for(Result& result, bool (*ready)(const Result&)) {
    std::unique_lock<std::mutex> lock(result.mutex);
    return result.changed.wait_for(lock, std::chrono::seconds(2), [&] { return ready(result); });
}

void fragmented_s16_bytes_reach_one_whole_frame_callback_unchanged() {
    constexpr uint32_t frames = 480;
    Source source;
    source.largest_read = 37;
    source.bytes.resize(frames * 2);
    for (size_t i = 0; i < source.bytes.size(); ++i) {
        source.bytes[i] = static_cast<uint8_t>((i * 29) & 0xff);
    }

    InputCallbackDriver driver;
    Result result;
    result.driver = &driver;
    result.continue_callback = false;
    assert(driver.start(&source, &read_source, 2, frames, &receive, &exited, &result));
    assert(wait_for(result, [](const Result& r) { return r.exits == 1; }));
    driver.join();

    assert(result.calls == 1);
    assert(result.frames == frames);
    assert(result.bytes == source.bytes);
    assert(result.callback_stopped);
    assert(result.was_callback_thread);
    assert(!driver.is_running());
    std::printf("ok: fragmented_s16_bytes_reach_one_whole_frame_callback_unchanged\n");
}

void callback_stop_delivers_once_then_exits() {
    Source source;
    source.bytes.assign(480 * 2 * 3, 0x5a);
    InputCallbackDriver driver;
    Result result;
    result.driver = &driver;
    result.continue_callback = false;
    assert(driver.start(&source, &read_source, 2, 480, &receive, &exited, &result));
    assert(wait_for(result, [](const Result& r) { return r.exits == 1; }));
    driver.join();

    assert(result.calls == 1);
    assert(source.offset.load() == 480 * 2);
    assert(result.callback_stopped);
    std::printf("ok: callback_stop_delivers_once_then_exits\n");
}

void idle_cancellation_wakes_without_delivering_partial_audio() {
    Source source;
    InputCallbackDriver driver;
    Result result;
    result.driver = &driver;
    assert(driver.start(&source, &read_source, 2, 480, &receive, &exited, &result));

    const auto read_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (source.reads.load() == 0 && std::chrono::steady_clock::now() < read_deadline) {
    }
    assert(source.reads.load() != 0);
    const auto before = std::chrono::steady_clock::now();
    driver.cancel();
    driver.join();
    const auto elapsed = std::chrono::steady_clock::now() - before;

    assert(elapsed < std::chrono::milliseconds(250));
    assert(result.calls == 0);
    assert(result.exits == 1);
    assert(!result.callback_stopped);
    assert(!driver.is_running());
    std::printf("ok: idle_cancellation_wakes_without_delivering_partial_audio\n");
}

void callback_thread_can_request_cancellation_without_self_joining() {
    Source source;
    source.bytes.assign(480 * 2, 0x37);
    InputCallbackDriver driver;
    Result result;
    result.driver = &driver;
    result.cancel_in_callback = true;
    assert(driver.start(&source, &read_source, 2, 480, &receive, &exited, &result));
    assert(wait_for(result, [](const Result& r) { return r.exits == 1; }));
    driver.join();

    assert(result.calls == 1);
    assert(result.was_callback_thread);
    assert(!result.callback_stopped);
    assert(!driver.is_running());
    std::printf("ok: callback_thread_can_request_cancellation_without_self_joining\n");
}

void a_cancelled_driver_can_start_again() {
    Source idle;
    InputCallbackDriver driver;
    Result first;
    first.driver = &driver;
    assert(driver.start(&idle, &read_source, 2, 480, &receive, &exited, &first));
    driver.cancel();
    driver.join();

    Source source;
    source.bytes.assign(480 * 2, 0x42);
    Result second;
    second.driver = &driver;
    second.continue_callback = false;
    assert(driver.start(&source, &read_source, 2, 480, &receive, &exited, &second));
    assert(wait_for(second, [](const Result& r) { return r.exits == 1; }));
    driver.join();

    assert(first.calls == 0 && first.exits == 1);
    assert(second.calls == 1 && second.exits == 1);
    assert(second.callback_stopped);
    std::printf("ok: a_cancelled_driver_can_start_again\n");
}

} // namespace

int main() {
    fragmented_s16_bytes_reach_one_whole_frame_callback_unchanged();
    callback_stop_delivers_once_then_exits();
    idle_cancellation_wakes_without_delivering_partial_audio();
    callback_thread_can_request_cancellation_without_self_joining();
    a_cancelled_driver_can_start_again();
    std::printf("all AAudio input callback checks passed\n");
    return 0;
}
