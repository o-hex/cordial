#include "permissions_transport.h"

#include <cstdint>
#include <cstdlib>
#include <cstdio>
#include <stdexcept>
#include <vector>

namespace {

enum class EventType {
    Publish,
    Resolve,
};

struct Event {
    EventType type;
    jobject bus;
    jstring protocol;
    jstring method;
    jstring response;
    jint code;
    jstring telemetry;
    jstring correlation_id;
};

std::vector<Event> events;

void require(bool condition) {
    if (!condition) {
        std::fputs("permission transport check failed\n", stderr);
        std::abort();
    }
}

void publish(JNIEnv*, jobject bus, jstring protocol, jstring method, jstring response,
             jint code, jstring telemetry) {
    events.push_back({EventType::Publish, bus, protocol, method, response, code, telemetry, nullptr});
}

void resolve(JNIEnv*, jobject bus, jstring correlation_id, jstring response) {
    events.push_back(
        {EventType::Resolve, bus, nullptr, nullptr, response, 0, nullptr, correlation_id});
}

template <typename T>
T pointer(std::uintptr_t value) {
    return reinterpret_cast<T>(value);
}

cordial::permissions::ResponseDelivery delivery(const char* protocol_name, bool dual) {
    return {
        pointer<JNIEnv*>(0x100),
        pointer<jobject>(0x200),
        pointer<jstring>(0x300),
        pointer<jstring>(0x400),
        pointer<jstring>(0x500),
        pointer<jstring>(0x600),
        pointer<jstring>(0x700),
        publish,
        resolve,
        protocol_name,
        dual,
    };
}

void permissions_dual_response_publishes_before_resolving_the_same_body() {
    // Given an enabled PermissionsProtocol response with distinct routing values.
    events.clear();
    const auto input = delivery("PermissionsProtocol", true);

    // When the transport delivers the generated response.
    cordial::permissions::deliver_response(input);

    // Then protocol/method publication precedes correlation resolution and shares the body.
    require(events.size() == 2);
    require(events[0].type == EventType::Publish);
    require(events[0].bus == input.bus);
    require(events[0].protocol == input.protocol);
    require(events[0].method == input.method);
    require(events[0].response == input.response);
    require(events[0].code == 0);
    require(events[0].telemetry == input.telemetry);
    require(events[0].correlation_id == nullptr);
    require(events[1].type == EventType::Resolve);
    require(events[1].bus == input.bus);
    require(events[1].correlation_id == input.correlation_id);
    require(events[1].response == events[0].response);
    std::printf("ok: permissions_dual_response_publishes_before_resolving_the_same_body\n");
}

void disabled_dual_response_only_resolves_the_original_id() {
    // Given PermissionsProtocol with dual delivery disabled and no publish function.
    events.clear();
    auto input = delivery("PermissionsProtocol", false);
    input.publish = nullptr;

    // When the transport delivers the response.
    cordial::permissions::deliver_response(input);

    // Then legacy publication remains absent and the original async route is unchanged.
    require(events.size() == 1);
    require(events[0].type == EventType::Resolve);
    require(events[0].correlation_id == input.correlation_id);
    require(events[0].response == input.response);
    std::printf("ok: disabled_dual_response_only_resolves_the_original_id\n");
}

void other_protocols_ignore_the_permission_delivery_mode() {
    // Given another protocol while dual permission delivery is enabled.
    events.clear();
    const auto input = delivery("Linking", true);

    // When the transport delivers its response.
    cordial::permissions::deliver_response(input);

    // Then it uses only its correlation ID and never publishes a permission response.
    require(events.size() == 1);
    require(events[0].type == EventType::Resolve);
    require(events[0].correlation_id == input.correlation_id);
    require(events[0].response == input.response);
    std::printf("ok: other_protocols_ignore_the_permission_delivery_mode\n");
}

void requested_permission_dual_response_rejects_a_missing_publish_native() {
    // Given enabled PermissionsProtocol delivery without its required native.
    events.clear();
    auto input = delivery("PermissionsProtocol", true);
    input.publish = nullptr;

    // When delivery is attempted.
    bool rejected = false;
    try {
        cordial::permissions::deliver_response(input);
    } catch (const std::invalid_argument&) {
        rejected = true;
    }

    // Then it fails explicitly before either response route is called.
    require(rejected);
    require(events.empty());
    std::printf("ok: requested_permission_dual_response_rejects_a_missing_publish_native\n");
}

} // namespace

int main() {
    permissions_dual_response_publishes_before_resolving_the_same_body();
    disabled_dual_response_only_resolves_the_original_id();
    other_protocols_ignore_the_permission_delivery_mode();
    requested_permission_dual_response_rejects_a_missing_publish_native();
    std::printf("all permission transport checks passed\n");
    return 0;
}
