#pragma once

#include <jni.h>

namespace cordial::permissions {

using PublishProtocolMethodResponseRaw =
    void (*)(JNIEnv*, jobject, jstring, jstring, jstring, jint, jstring);
using CallResponseHandlerRaw = void (*)(JNIEnv*, jobject, jstring, jstring);

struct ResponseDelivery {
    JNIEnv* env;
    jobject bus;
    jstring protocol;
    jstring method;
    jstring response;
    jstring telemetry;
    jstring correlation_id;
    PublishProtocolMethodResponseRaw publish;
    CallResponseHandlerRaw resolve;
    const char* protocol_name;
    bool dual_response_requested;
};

bool uses_dual_response(bool requested, const char* protocol);
void deliver_response(const ResponseDelivery& delivery);

} // namespace cordial::permissions
