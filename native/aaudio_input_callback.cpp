#include "aaudio_input_callback.h"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <limits>
#include <mutex>
#include <thread>
#include <vector>

namespace cordial::audio {
namespace {

thread_local const InputCallbackDriver* g_current_driver = nullptr;

} // namespace

struct InputCallbackDriver::Impl {
    void* source = nullptr;
    ReadCallback read = nullptr;
    DataCallback callback = nullptr;
    ExitCallback exited = nullptr;
    void* user = nullptr;

    std::vector<uint8_t> buffer;
    std::atomic<bool> cancelled{true};
    std::atomic<bool> running{false};
    std::mutex wait_mutex;
    std::condition_variable wake;
    std::thread worker;
};

InputCallbackDriver::InputCallbackDriver() : impl_(std::make_unique<Impl>()) {}

InputCallbackDriver::~InputCallbackDriver() {
    cancel();
    join();
}

bool InputCallbackDriver::start(void* source, ReadCallback read, uint32_t bytes_per_frame,
                                uint32_t frames_per_callback, DataCallback callback,
                                ExitCallback exited, void* user) {
    if (!impl_ || !source || !read || !callback || !exited || bytes_per_frame == 0 ||
        frames_per_callback == 0 ||
        frames_per_callback > std::numeric_limits<size_t>::max() / bytes_per_frame) {
        return false;
    }
    if (is_callback_thread() || impl_->running.load(std::memory_order_acquire)) return false;

    join();
    impl_->source = source;
    impl_->read = read;
    impl_->callback = callback;
    impl_->exited = exited;
    impl_->user = user;
    impl_->buffer.resize(static_cast<size_t>(bytes_per_frame) * frames_per_callback);
    impl_->cancelled.store(false, std::memory_order_release);
    impl_->running.store(true, std::memory_order_release);

    try {
        impl_->worker = std::thread([this, frames_per_callback] {
            g_current_driver = this;
            size_t filled = 0;
            bool callback_stopped = false;

            while (!impl_->cancelled.load(std::memory_order_acquire)) {
                const size_t remaining = impl_->buffer.size() - filled;
                const uint32_t request = static_cast<uint32_t>(
                    remaining > UINT32_MAX ? UINT32_MAX : remaining);
                const uint32_t got = impl_->read(
                    impl_->source, impl_->buffer.data() + filled, request);
                if (got > request) {
                    impl_->cancelled.store(true, std::memory_order_release);
                    break;
                }
                filled += got;

                if (filled == impl_->buffer.size()) {
                    if (!impl_->callback(impl_->buffer.data(), frames_per_callback, impl_->user)) {
                        callback_stopped = true;
                        break;
                    }
                    filled = 0;
                    continue;
                }

                if (got == 0) {
                    std::unique_lock<std::mutex> lock(impl_->wait_mutex);
                    impl_->wake.wait_for(lock, std::chrono::milliseconds(1), [this] {
                        return impl_->cancelled.load(std::memory_order_acquire);
                    });
                }
            }

            impl_->running.store(false, std::memory_order_release);
            impl_->exited(callback_stopped, impl_->user);
            g_current_driver = nullptr;
        });
    } catch (...) {
        impl_->cancelled.store(true, std::memory_order_release);
        impl_->running.store(false, std::memory_order_release);
        return false;
    }
    return true;
}

void InputCallbackDriver::cancel() {
    if (!impl_) return;
    impl_->cancelled.store(true, std::memory_order_release);
    impl_->wake.notify_all();
}

void InputCallbackDriver::join() {
    if (!impl_ || is_callback_thread()) return;
    if (impl_->worker.joinable()) impl_->worker.join();
}

bool InputCallbackDriver::is_callback_thread() const {
    return g_current_driver == this;
}

bool InputCallbackDriver::is_running() const {
    return impl_ && impl_->running.load(std::memory_order_acquire);
}

} // namespace cordial::audio
