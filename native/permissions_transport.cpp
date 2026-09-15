#include "permissions_transport.h"

#include <cstring>
#include <stdexcept>

namespace cordial::permissions {

bool uses_dual_response(bool requested, const char* protocol) {
    return requested && protocol && std::strcmp(protocol, "PermissionsProtocol") == 0;
}

void deliver_response(const ResponseDelivery& delivery) {
    if (!delivery.resolve) {
        throw std::invalid_argument("async response native is unavailable");
    }
    if (uses_dual_response(delivery.dual_response_requested, delivery.protocol_name)) {
        if (!delivery.publish) {
            throw std::invalid_argument("protocol response native is unavailable");
        }
        delivery.publish(delivery.env, delivery.bus, delivery.protocol, delivery.method,
                         delivery.response, 0, delivery.telemetry);
    }
    delivery.resolve(delivery.env, delivery.bus, delivery.correlation_id, delivery.response);
}

} // namespace cordial::permissions
