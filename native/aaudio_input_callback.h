// Cancellable bridge from a pull-style capture source to an AAudio input
// callback. This runs on its own non-realtime thread: PipeWire's process
// callback only fills CaptureStream's ring, and this consumer may wait for a
// complete AAudio burst without ever blocking PipeWire.

#pragma once

#include <cstdint>
#include <memory>

namespace cordial::audio {

class InputCallbackDriver {
public:
    using ReadCallback = uint32_t (*)(void* source, void* dst, uint32_t size);
    using DataCallback = bool (*)(void* data, uint32_t frames, void* user);
    using ExitCallback = void (*)(bool callback_stopped, void* user);

    InputCallbackDriver();
    ~InputCallbackDriver();

    InputCallbackDriver(const InputCallbackDriver&) = delete;
    InputCallbackDriver& operator=(const InputCallbackDriver&) = delete;

    bool start(void* source, ReadCallback read, uint32_t bytes_per_frame,
               uint32_t frames_per_callback, DataCallback callback,
               ExitCallback exited, void* user);

    /// Requests cancellation and wakes an idle worker. Safe from inside the
    /// data callback because it never joins.
    void cancel();

    /// Waits for the worker after cancellation or callback STOP. Must not be
    /// called from inside the data callback; `is_callback_thread` lets callers
    /// preserve that rule when their lifecycle API is re-entered there.
    void join();

    bool is_callback_thread() const;
    bool is_running() const;

private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace cordial::audio
