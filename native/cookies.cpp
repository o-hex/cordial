// The engine's cookie jar, and the Java side that was supposed to persist it.
//
// The bug this file exists for: sign in to Roblox, quit, restart against the
// same profile, and you are back on the landing page. Reported twice, and
// confirmed on a single profile, so it is not the `flock` in `profile.rs`
// handing out a different directory.
//
// The measurement that explains it. A complete `CORDIAL_TRACE_PATHS=1`
// inventory of every non-system file the engine opens contains no cookie jar
// and no credential store of any kind, and `grep -rl ROBLOSECURITY` over a
// real profile tree finds nothing. **The engine never writes its cookies to
// disk.** It holds them in memory for the life of the process and expects the
// Java side of the app to persist them, which the Waydroid capture shows
// happening:
//
//     OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler with Cookies
//     from URL https://apis.roblox.com/browser-tracker-api/device/initialize
//
// Cordial has no Java side, so nothing was persisting anything. This is worth
// stating precisely because it rules out the fix everybody reaches for first:
// no shutdown path can flush a file that is never written, and the graceful
// teardown descent in `looper.rs` — which does exist and does work — was
// controlled against by alternating killed and graceful runs over two passes.
// No file is created or updated at shutdown that a killed run does not also
// produce. Teardown was never the missing piece.
//
// So Cordial has to be the Java side. Three natives, all read out of the
// shipping APK's own dex declarations and confirmed as real exports in
// `libroblox.so` (`readelf --dyn-syms`):
//
//     JNICookieProtocol.updateOnSetCookieHandler(JNICookieProtocol$OnSetCookieHandler)
//       -> handler.onSetCookie(String[] cookies, String url)
//     NativeSettingsInterface.nativeGetCookiesForDomain(String) -> String
//     NativeSettingsInterface.nativeSetMultipleCookies(String domain, String cookies)
//
// The first is how the engine tells us its jar changed, the second is how we
// read it back out, the third is how we put it back on the next launch.
//
// `JNICookieManager_{getCookie,setCookie,setCookiesFromDisk,convertCookiesToNetscape}`
// is also exported and looks like exactly the API this wants. It is dead.
// Plain `strings` over all three dex files finds zero occurrences of the class
// name, so nothing on Android has called it in this build — a stale export, of
// the kind an unstripped native library keeps long after proguard has removed
// the Java caller. Building against it would be building against nothing.
//
// **Nothing in this file logs, prints or traces a cookie value, at any
// verbosity, behind any flag.** `onSetCookie`'s first argument is the live
// session; it is passed straight through to the engine's own jar and never
// read here. Only the *host* of the second argument leaves this file, and it
// is extracted before anything else touches the string, because a Roblox URL's
// query can itself carry a one-time authentication ticket — the same reason
// the path tracer elides query strings.

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

namespace {

/// Where an observed host is handed to the Rust store. Null until
/// `cordial_cookies_set_host_sink` installs one, which is the state a run with
/// cookie persistence switched off stays in — the class is still registered so
/// the engine's callback resolves, but nothing is recorded.
void (*g_host_sink)(const char*) = nullptr;

/// The handler object the engine holds. Kept alive here for the life of the
/// process on purpose: the engine stores the reference and calls back into it
/// from its own HTTP thread whenever a response carries `Set-Cookie`, which is
/// long after the registering call has returned and every local frame that
/// might otherwise have owned it has gone.
std::shared_ptr<Object> g_handler;

/// The host out of a URL, and nothing else.
///
/// Deliberately not a URL parser. Everything after the authority is dropped
/// without being examined, because a Roblox URL's query string can carry a
/// one-time authentication ticket and this value's whole purpose is to be
/// recorded somewhere Cordial can see it later.
std::string host_of(const std::string& url) {
    auto start = url.find("://");
    start = (start == std::string::npos) ? 0 : start + 3;
    auto end = url.find_first_of("/?#", start);
    std::string host = url.substr(start, end == std::string::npos ? end : end - start);
    // Strip credentials and port, both of which would make this a different
    // key for the same jar.
    auto at = host.rfind('@');
    if (at != std::string::npos) {
        host = host.substr(at + 1);
    }
    auto colon = host.rfind(':');
    if (colon != std::string::npos && host.find(']') == std::string::npos) {
        host = host.substr(0, colon);
    }
    return host;
}

} // namespace

/// `com.roblox.universalapp.cookie.JNICookieProtocol$OnSetCookieHandler`
///
/// The interface the engine calls back through. The `$` is not decoration —
/// the dex declares this as a nested interface of `JNICookieProtocol`, and
/// libjnivm matches on the exact binary name, so registering the top-level
/// spelling would leave the engine holding an object whose method never
/// resolves. That failure is silent and looks precisely like cookies simply
/// not arriving.
class OnSetCookieHandler : public Object {
public:
    /// `onSetCookie([Ljava/lang/String;Ljava/lang/String;)V`
    ///
    /// The cookies argument is the live session and is deliberately never
    /// read. The engine already holds these in its own jar — this callback is
    /// a notification that the jar changed, not the only copy of it — so the
    /// useful thing to take is which host changed, and to read the jar back
    /// properly through `nativeGetCookiesForDomain` at a moment of our own
    /// choosing rather than reassembling it from `Set-Cookie` headers here.
    ///
    /// Reading it back rather than parsing it here also avoids calling into
    /// the engine from inside the engine's own callback, on its own HTTP
    /// thread.
    static void onSetCookie(ENV*, Object*, std::shared_ptr<jnivm::Array<String>>,
                            std::shared_ptr<String> url) {
        if (!g_host_sink || !url) {
            return;
        }
        const std::string host = host_of(static_cast<const std::string&>(*url));
        if (!host.empty()) {
            g_host_sink(host.c_str());
        }
    }

    static void Register(ENV* env) {
        const char* name = "com/roblox/universalapp/cookie/JNICookieProtocol$OnSetCookieHandler";
        env->GetClass<OnSetCookieHandler>(name);
        auto c = env->GetClass(name);
        // `HookInstanceFunction`, matching every other instance method in this
        // directory; `HookInstance` is what the accessibility classes use for
        // *fields*, and it ORs the deduced kind with `Instance` rather than
        // forcing it.
        //
        // **This registration is not confirmed to be callable, and the feature
        // does not depend on it.** `--dump-classes` reports `onSetCookie`
        // twice, once with a receiver and once without, under either helper —
        // so the dump does not settle which descriptor the engine would resolve.
        // Settling it needs a real `Set-Cookie` to arrive, and no response in a
        // logged-out Cordial run carries one: over repeated runs the sink was
        // called zero times while the engine's own log showed it reaching the
        // network and collecting the documented 401s. The capture's cookie
        // traffic comes from requests Roblox's *Java* code issues, which
        // Cordial does not run.
        //
        // So this stays INFERRED, and the session is saved by reading the jar
        // back on a timer and at teardown instead. If this callback does fire
        // it makes those saves promptly rather than on the next tick; if it
        // never fires, nothing is lost.
        c->HookInstanceFunction(env, "onSetCookie", &OnSetCookieHandler::onSetCookie);
    }
};

void register_cookie_classes(jnivm::ENV* env) {
    OnSetCookieHandler::Register(env);
}

} // namespace cordial

extern "C" {

/// Install the sink observed hosts are reported to, or clear it with null.
///
/// Separate from registration so that the control run — same binary, cookie
/// persistence switched off — differs in exactly one thing: whether anything
/// is listening. The class stays registered either way, so a difference in
/// behaviour cannot be confused with the engine failing to resolve the
/// callback.
void cordial_cookies_set_host_sink(void (*sink)(const char*)) {
    cordial::g_host_sink = sink;
}

/// `JNICookieProtocol.updateOnSetCookieHandler(OnSetCookieHandler)`.
///
/// Hands the engine an object to call when its jar changes. Verified firing
/// four times in the Waydroid capture, on a *logged-out* cold start — the
/// device and tracking cookies exercise the identical plumbing an auth cookie
/// does, which is what makes this testable without an account.
int cordial_cookies_register_handler(void* fn, char* err, size_t err_len) {
    using Call = void (*)(JNIEnv*, jobject, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or updateOnSetCookieHandler is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/universalapp/cookie/JNICookieProtocol");
        auto handler = std::make_shared<cordial::OnSetCookieHandler>();
        // Park it before the call, not after: the engine may call back
        // synchronously from inside this very call, and a handler owned only
        // by a local would already be a candidate for collection.
        cordial::g_handler = handler;
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                   (jobject)cordial::to_jni(env, cls),
                                   (jobject)cordial::to_jni(env, handler));
        return 0;
    } catch (const std::exception& e) {
        cordial::g_handler.reset();
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        cordial::g_handler.reset();
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// A static native taking one `String` and returning one —
/// `nativeGetCookiesForDomain` and `nativeGetCookiesInNetscapeFormat`.
///
/// Writes into a caller-supplied buffer and reports the length it wanted in
/// `needed`. **Truncation is an error, not a short answer.** A cookie jar cut
/// off mid-value would still parse as a list of cookies, and handing that back
/// to the engine on the next launch is the "half-token that parses" failure —
/// it presents as an invalid session rather than as a Cordial bug, which is
/// the worst place for it to present.
int cordial_cookies_get_for_domain(void* fn, const char* class_name, const char* domain,
                                   char* out, size_t out_len, size_t* needed,
                                   char* err, size_t err_len) {
    using Call = jstring (*)(JNIEnv*, jobject, jstring);
    auto* env = cordial::process_env();
    if (!fn || !env || !class_name) {
        snprintf(err, err_len, "no JavaVM, or the cookie native is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass(class_name);
        auto arg = std::make_shared<cordial::String>(std::string(domain ? domain : ""));
        jstring r = reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                               (jobject)cordial::to_jni(env, cls),
                                               (jstring)cordial::to_jni(env, arg));
        const auto* s = reinterpret_cast<cordial::String*>(r);
        const std::string value = s ? static_cast<const std::string&>(*s) : std::string();
        if (needed) {
            *needed = value.size();
        }
        if (value.size() >= out_len) {
            // The size is not a secret and naming it is the only way anyone
            // diagnoses this; the value it describes is never printed.
            snprintf(err, err_len, "the jar for this domain is %zu bytes and the buffer is %zu",
                     value.size(), out_len);
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
