// Copy out of Roblox, and the message the engine sends to ask for it.
//
// **The engine does not ask for `android.content.ClipboardManager`.**
// `docs/analysis/framework-classes.txt` lists `ClipboardManager`, `ClipData`,
// `ClipData$Item` and `ClipDescription`, and that is what made this look like a
// framework class to implement. It is not one, for the same reason
// `docs/analysis/webview-surface.md` §1 gives about `WebView`: that file is the
// dex's referenced-type table, so it says Roblox's *Java* code uses those
// classes, not that `libroblox.so` reaches for them. Cordial does not run
// Roblox's Java code — it stands in for it.
//
// What the engine actually does was read out of the shipping build rather than
// guessed:
//
//   * `readelf --dyn-syms` over `libroblox.so` exports no clipboard native at
//     all. Nothing matching `clip` or `paste`, out of 508 `Java_*` exports.
//   * The engine's string pool holds exactly one clipboard-shaped string,
//     `setClipboardText`, alongside `ExternalContentSharing`, `shareText`,
//     `shareUrl`, `shareImage` and `shareVideo`.
//   * `tools/dex_method.py` finds no method called `setClipboardText` anywhere
//     in the three dex files, but the dex *string* table holds
//     `ExternalContentSharing.setClipboardText` next to `ClipboardManager`,
//     `newPlainText`, `setPrimaryClip` and the error text
//     "setClipboardText received null content value for clipboard."
//
// A name that exists as a string on both sides and as a method on neither is a
// message-bus message id. So this is the same shape as the cookie jar and the
// deep link: the engine publishes, Roblox's Java subscribes, and Cordial has to
// be the subscriber. `native/cookies.cpp` and `native/deeplink.cpp` set out the
// same reasoning for their own surfaces.
//
// The four `share*` methods each have a native that hands out their message id
// (`JNIExternalContentSharingProtocol.getShareTextId` and friends).
// `setClipboardText` has no such getter, which is why the id is spelled out
// here rather than asked for.
//
// The other direction — pasting *into* Roblox — has no engine-side ask at all,
// and that is not an omission here. On Android a focused TextBox is edited by a
// real `android.widget.EditText` laid over the GL surface (see
// `CordialTextBoxInfo` in `android_classes.cpp`), so Android's own editor
// handles the paste and the engine only ever sees the resulting text arrive
// through `gametextinput`. Cordial's equivalent of that editor is
// `android::input`, so the paste path is `android::clipboard::paste_into_engine`
// and involves no JNI at all.
//
// **Nothing in this file prints a clipboard value.** The payload is a JSON
// document that came from whatever the user copied inside an experience; it is
// counted and handed on, never logged. The trace switch reports the message id
// and a byte count. `crates/cordial-runtime/src/android/clipboard.rs` carries
// the rest of that rule, and is the only place the text is looked at.

#include <jnivm.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <atomic>
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

/// Where a published payload is handed to the Rust side. Null until
/// `cordial_clipboard_set_sink` installs one, which is the state a run with
/// `CORDIAL_SKIP_CLIPBOARD=1` stays in: the classes are still registered and
/// the subscription is still made, so the control run differs in exactly
/// whether anything acts on a message rather than in whether the engine can
/// deliver one.
std::atomic<void (*)(const char*)> g_payload_sink{nullptr};

/// The callback object the engine holds. Parked here for the life of the
/// process on purpose — the bus stores the reference and calls back into it
/// from whichever thread published, long after the subscribing call returned
/// and every frame that might otherwise have owned it has gone. This is the
/// same lifetime problem `cookies.cpp` solves the same way.
std::shared_ptr<Object> g_callback;

/// The `Connection` the subscribe call handed back, kept for the same reason
/// and for one more: its `long` is the only handle by which
/// `Connection.isConnected` can be asked whether the subscription is still
/// live, which is how "Cordial subscribed" stops being an assumption.
std::shared_ptr<Object> g_connection;
std::atomic<long long> g_connection_ptr{0};

bool trace() {
    return getenv("CORDIAL_TRACE_CLIPBOARD") != nullptr;
}

} // namespace

/// `com.roblox.universalapp.messagebus.RawCallback`
///
/// The interface the bus calls back through, and the reason this is a hook
/// rather than something libjnivm can answer by itself. Its `run` has no
/// method id anywhere in the dex — Roblox's Java never calls it, only the
/// engine does, over JNI — so the descriptor `(Ljava/lang/String;)V` comes from
/// `MessageBus$a`, the adapter that wraps a JSON `Callback` into a
/// `RawCallback` and whose own `run` is declared exactly that way.
///
/// If the descriptor is wrong the failure is silent in the usual way: the hook
/// registers, the engine's `GetMethodID` misses it, and the jnivm log says
/// `Constructed Unresolved symbol ... Method=\`run\``. That line is the test.
class MessageBusRawCallback : public Object {
public:
    /// `run(Ljava/lang/String;)V`
    ///
    /// The argument is the raw JSON the engine published. It is measured and
    /// forwarded; it is never parsed here and never printed. Parsing belongs
    /// on the Rust side, where `serde_json` already lives and where the rule
    /// about printing names rather than values is written down and tested.
    static void run(ENV*, Object*, std::shared_ptr<String> payload) {
        auto* sink = g_payload_sink.load(std::memory_order_acquire);
        const std::string json =
            payload ? static_cast<const std::string&>(*payload) : std::string();
        if (trace()) {
            fprintf(stderr, "[clipboard] the engine published %zu bytes%s\n", json.size(),
                    sink ? "" : " and nothing is listening (CORDIAL_SKIP_CLIPBOARD)");
        }
        if (sink) {
            sink(json.c_str());
        }
    }

    static void Register(ENV* env) {
        const char* name = "com/roblox/universalapp/messagebus/RawCallback";
        env->GetClass<MessageBusRawCallback>(name);
        auto c = env->GetClass(name);
        c->HookInstanceFunction(env, "run", &MessageBusRawCallback::run);
    }
};

/// `com.roblox.universalapp.messagebus.Connection`
///
/// `doSubscribeRaw` returns one of these, and the engine builds it by calling
/// `<init>(J)` with the address of the subscription it just made. Registered
/// here for two reasons. Without a constructor the engine's `NewObject` gets an
/// unresolved stub and the `long` is dropped on the floor; and with one, that
/// `long` is exactly what `Connection.isConnected(J)` wants, which turns "the
/// subscribe call did not throw" into "the bus says this subscription is live".
class MessageBusConnection : public Object {
public:
    jlong ptr = 0;

    static std::shared_ptr<MessageBusConnection> init(ENV*, Class*, jlong p) {
        auto o = std::make_shared<MessageBusConnection>();
        o->ptr = p;
        return o;
    }

    static void Register(ENV* env) {
        const char* name = "com/roblox/universalapp/messagebus/Connection";
        env->GetClass<MessageBusConnection>(name);
        auto c = env->GetClass(name);
        c->Hook(env, "<init>", &MessageBusConnection::init);
    }
};

void register_clipboard_classes(jnivm::ENV* env) {
    MessageBusRawCallback::Register(env);
    MessageBusConnection::Register(env);
}

} // namespace cordial

extern "C" {

/// Install the sink published payloads are handed to, or clear it with null.
///
/// Separate from registration and from subscribing, so the control run differs
/// in one thing only: whether anything acts on a message. `cookies.cpp` splits
/// the same way and for the same reason — a behavioural difference must not be
/// confusable with the engine failing to resolve a callback.
void cordial_clipboard_set_sink(void (*sink)(const char*)) {
    cordial::g_payload_sink.store(sink, std::memory_order_release);
}

/// `MessageBus.doSubscribeRaw(String messageId, RawCallback cb, boolean) -> Connection`
///
/// The third argument's meaning is not established. It is passed `false`
/// because that is the answer that asks for nothing extra; a `true` whose
/// effect nobody here has measured would be a claim this file cannot support.
/// INFERRED that it selects some replay-or-not behaviour, from its position and
/// type alone.
///
/// The class is passed where a receiver would go, which is what
/// `deeplink.cpp`'s `publishRaw` caller already does for this same class and
/// what works there.
int cordial_clipboard_subscribe(void* fn, const char* message_id, char* err, size_t err_len) {
    using Call = jobject (*)(JNIEnv*, jobject, jstring, jobject, jboolean);
    auto* env = cordial::process_env();
    if (!fn || !env || !message_id) {
        snprintf(err, err_len, "no JavaVM, or doSubscribeRaw is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/universalapp/messagebus/MessageBus");
        auto cb = std::make_shared<cordial::MessageBusRawCallback>();
        auto id = std::make_shared<cordial::String>(std::string(message_id));
        // Parked before the call, not after: the bus may deliver a message
        // synchronously from inside this very call, and a callback owned only
        // by a local would already be a candidate for collection.
        cordial::g_callback = cb;
        jobject r = reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                               (jobject)cordial::to_jni(env, cls),
                                               (jstring)cordial::to_jni(env, id),
                                               (jobject)cordial::to_jni(env, cb),
                                               (jboolean) false);
        auto* conn = dynamic_cast<cordial::MessageBusConnection*>(
            reinterpret_cast<cordial::Object*>(r));
        if (conn) {
            cordial::g_connection_ptr.store(static_cast<long long>(conn->ptr),
                                            std::memory_order_release);
            // Hold the object as well as its address. The address is what
            // `isConnected` wants; the object is what keeps the engine's own
            // shared_ptr from being released by `Connection.finalize`.
            cordial::g_connection = conn->shared_from_this();
        } else {
            // Not fatal, and deliberately not reported as success either. A
            // subscribe that returned something other than a Connection has
            // still installed the callback as far as anything here can tell,
            // but nothing can then confirm it, so say so.
            cordial::g_connection_ptr.store(0, std::memory_order_release);
        }
        return 0;
    } catch (const std::exception& e) {
        cordial::g_callback.reset();
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        cordial::g_callback.reset();
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// The `long` inside the `Connection` the subscribe call returned, or 0 when
/// the engine handed back something this side could not read. Feeds
/// `Connection.isConnected(J)`; on its own it means only that a Connection came
/// back at all.
long long cordial_clipboard_connection_ptr() {
    return cordial::g_connection_ptr.load(std::memory_order_acquire);
}

/// `Connection.isConnected(J)` — a static native taking the subscription's own
/// address. Writes 1 or 0 into `*out_connected`.
int cordial_clipboard_is_connected(void* fn, long long ptr, int* out_connected, char* err,
                                   size_t err_len) {
    using Call = jboolean (*)(JNIEnv*, jobject, jlong);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or isConnected is not exported");
        return -1;
    }
    if (ptr == 0) {
        snprintf(err, err_len, "no Connection came back from doSubscribeRaw");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/universalapp/messagebus/Connection");
        jboolean r = reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                                (jobject)cordial::to_jni(env, cls),
                                                static_cast<jlong>(ptr));
        if (out_connected) {
            *out_connected = r ? 1 : 0;
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

} // extern "C"
