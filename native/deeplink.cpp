// `roblox-player://` and `roblox://` — the calls a URL from a browser click has
// to reach, and what is known about each.
//
// The question this file exists to answer: when a deep link arrives, what does
// the engine actually ask for? **Nothing.** The engine never asks for an
// `Intent`, a `Uri`, or a URL of any kind — a full launch with
// `--dump-classes` and the jnivm log open shows no such request, and the
// Waydroid capture of the real client shows every `Intent` line belonging to
// Google Play services rather than to Roblox's process. On Android the URL goes
// the other way: Roblox's own Java receives it and calls *inward*.
//
// So the surface here is the set of inward calls, and Cordial is the Java side
// in this architecture — the same reasoning `cookies.cpp` sets out for
// `nativeSetMultipleCookies`. Three of them matter:
//
//     JNIBaseUrlProtocol.maybeHandleColdStartProtocolLaunch(String) -> boolean
//     JNIWebLoginProtocol.maybeHandleColdStartProtocolLaunch(String) -> boolean
//     MessageBus.publishRaw(String messageId, String json)
//
// The first two are asked first and answer honestly: measured, both return
// false for `roblox://experiences/start?placeId=…`, so they are the base-URL
// and web-login special cases rather than the join path. The third is the join
// path — publishing the URL on `Linking.detectURL` produces a `Game.launch`
// message carrying the place from the link. `docs/analysis/deep-links.md` has
// the runs.
//
// `JNILinkingProtocol.nativeReportReceived(String, String[, boolean])` is also
// exported and was tried: it returns cleanly and changes nothing observable —
// not `Game.launch`, not `isColdStartDeeplinkToGame()`. It is not called from
// here, because its second argument would have to be invented and a call with
// made-up arguments is a claim this file cannot support.
//
// **A URL that arrives here came from a browser click and is attacker-shaped.**
// It is validated in `cordial_runtime::deeplink` before it reaches this file —
// scheme, length, and character set — and it is never used to build a path, a
// command line, or a format string. Here it is one `String` argument handed to
// one native.

#include <jnivm.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>

namespace cordial {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using jnivm::String;

jnivm::ENV* process_env();

template <class T>
static auto to_jni(jnivm::ENV* env, const std::shared_ptr<T>& p) {
    return jnivm::JNITypes<std::shared_ptr<T>>::ToJNIType(env, p);
}

} // namespace cordial

extern "C" {

/// A static, zero-argument native returning `String`.
///
/// `JNILinkingProtocol` is almost entirely getters of this shape — `getUrlKey`,
/// `getOpenURLId`, `getPendingURLId` and a dozen more. They return the message
/// names and JSON field names of the engine's own linking protocol, so calling
/// them is how the protocol's vocabulary is read out of a running engine
/// instead of guessed at from symbol names. Diagnostic; changes nothing.
int cordial_deeplink_protocol_string(void* fn, const char* class_name, char* out, size_t out_len,
                                     char* err, size_t err_len) {
    using Call = jstring (*)(JNIEnv*, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env || !class_name) {
        snprintf(err, err_len, "no JavaVM, or the native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass(class_name);
        jstring r = reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                               (jobject)cordial::to_jni(env, cls));
        const auto* s = reinterpret_cast<cordial::String*>(r);
        const std::string value = s ? static_cast<const std::string&>(*s) : std::string();
        if (value.size() >= out_len) {
            snprintf(err, err_len, "the answer is %zu bytes and the buffer is %zu", value.size(),
                     out_len);
            return -1;
        }
        memcpy(out, value.data(), value.size());
        out[value.size()] = '\0';
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// A static native taking one `String` and returning `boolean` —
/// `maybeHandleColdStartProtocolLaunch` on both `JNIBaseUrlProtocol` and
/// `JNIWebLoginProtocol`.
///
/// The boolean is the whole point. "Maybe handle" means the engine inspects the
/// URL and says whether it claimed it, so a caller can tell a link that was
/// consumed from one that fell through — the distinction between a deep link
/// that worked and one that silently did nothing.
int cordial_deeplink_cold_start(void* fn, const char* class_name, const char* url, int* out_handled,
                                char* err, size_t err_len) {
    using Call = jboolean (*)(JNIEnv*, jobject, jstring);
    auto* env = cordial::process_env();
    if (!fn || !env || !class_name) {
        snprintf(err, err_len, "no JavaVM, or the native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass(class_name);
        auto arg = std::make_shared<cordial::String>(std::string(url ? url : ""));
        jboolean r = reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                               (jobject)cordial::to_jni(env, cls),
                                               (jstring)cordial::to_jni(env, arg));
        if (out_handled) {
            *out_handled = r ? 1 : 0;
        }
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// A static native taking one `Context` — `JNIBaseUrlProtocol.init` and
/// `JNIWebLoginProtocol.init`.
///
/// The `Context` is a bare object. Cordial has no `android.content.Context` and
/// never has; every other place the engine is handed one (`initializeNativeCode`,
/// `initStorageManagerNativeV3`) passes the same empty stand-in, and libjnivm
/// answers whatever is asked of it with an unresolved-symbol stub rather than
/// crashing. If the engine ever reads something real off it, the jnivm log says
/// so by name — which is the point of driving this rather than skipping it.
int cordial_deeplink_protocol_init(void* fn, const char* class_name, char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env || !class_name) {
        snprintf(err, err_len, "no JavaVM, or the native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass(class_name);
        auto context = std::make_shared<jnivm::Object>();
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                   (jobject)cordial::to_jni(env, cls),
                                   (jobject)cordial::to_jni(env, context));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// A static native taking one `String` and returning one —
/// `MessageBus.getLastRaw(String)`.
///
/// This is the only way anything on Cordial's side can *see* the engine's
/// message bus without implementing a `RawCallback` class for it to call back
/// into: it hands back the last raw payload published on a message id. It is
/// how "did the publish land" and "did the app shell answer" stop being
/// assumptions.
int cordial_deeplink_string_ret_string(void* fn, const char* class_name, const char* arg, char* out,
                                       size_t out_len, char* err, size_t err_len) {
    using Call = jstring (*)(JNIEnv*, jobject, jstring);
    auto* env = cordial::process_env();
    if (!fn || !env || !class_name) {
        snprintf(err, err_len, "no JavaVM, or the native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass(class_name);
        auto a = std::make_shared<cordial::String>(std::string(arg ? arg : ""));
        jstring r = reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                               (jobject)cordial::to_jni(env, cls),
                                               (jstring)cordial::to_jni(env, a));
        const auto* s = reinterpret_cast<cordial::String*>(r);
        const std::string value = s ? static_cast<const std::string&>(*s) : std::string();
        if (value.size() >= out_len) {
            snprintf(err, err_len, "the answer is %zu bytes and the buffer is %zu", value.size(),
                     out_len);
            return -1;
        }
        memcpy(out, value.data(), value.size());
        out[value.size()] = '\0';
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

} // extern "C"
