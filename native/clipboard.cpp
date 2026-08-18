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
//
// This file also carries the message-bus *subscribe* machinery, and that part
// is no longer clipboard-specific. It used to be: one `g_payload_sink`, one
// `g_connection`, one `g_connection_ptr`, all module-level globals, because
// clipboard was the only subscriber anybody had written. `docs/analysis/
// webview-surface.md`'s "What is left, precisely" names the bug that shape
// was about to cause: the web window needs its own subscription to
// `openWindow`, and a second subscriber going through those same three
// globals would not add a subscription, it would silently overwrite
// clipboard's, exactly the way registering a class twice under this same
// engine already did once. `cordial_messagebus_subscribe` below is keyed by
// message id — a callback and a `Connection` per id, in a map, so clipboard's
// subscription and any other module's live independently. Nothing about
// clipboard's own behaviour changes; it is now one caller of a shared
// mechanism rather than the owner of it.

#include <jnivm.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <atomic>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>

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

/// One message id's subscription. The callback the engine holds a reference
/// to, and the `Connection` `doSubscribeRaw` handed back, kept alive for the
/// same reason clipboard's original single-slot globals kept theirs — the bus
/// calls back into the callback from whichever thread published, long after
/// the subscribing call returned, and `Connection.isConnected` needs the
/// address inside the `Connection` for as long as anyone might ask about it.
struct Subscription {
    std::shared_ptr<Object> callback;
    std::shared_ptr<Object> connection;
    std::atomic<long long> connection_ptr{0};
};

/// One entry per message id ever passed to `cordial_messagebus_subscribe`.
///
/// A map rather than a single slot: see the file comment for the bug a single
/// slot causes as soon as a second subscriber exists. Guarded by a mutex
/// because subscribing is not obviously single-threaded from this file's own
/// vantage point, even though in practice every subscribe call so far has
/// come from the looper thread. The payload-delivery path (`run`, below)
/// never touches this map or the mutex — a `Subscription` is heap-allocated
/// once and its address is stable for the rest of the process, so a callback
/// that already has its own `Subscription*` needs neither.
std::mutex g_subscriptions_mutex;
std::unordered_map<std::string, std::unique_ptr<Subscription>> g_subscriptions;

/// The `Subscription` for `message_id`, creating an empty one on first ask.
///
/// Returns a reference into heap-allocated storage that outlives the lock,
/// which is safe only because the value is a `unique_ptr`: `unordered_map`
/// insertion can move other *slots* around on rehash, but never the object a
/// `unique_ptr` in some slot points at, so the `Subscription&` returned here
/// stays valid even after a later call inserts a different id and triggers a
/// rehash.
Subscription& subscription_for(const std::string& message_id) {
    std::lock_guard<std::mutex> lock(g_subscriptions_mutex);
    auto& slot = g_subscriptions[message_id];
    if (!slot) {
        slot = std::make_unique<Subscription>();
    }
    return *slot;
}

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
///
/// One instance of this class exists per subscription, not per class — that
/// is what makes the map above unnecessary on the delivery path. `sink` and
/// `message_id` are set once, on the instance `cordial_messagebus_subscribe`
/// creates, before that instance is ever handed to `doSubscribeRaw`; `run` is
/// hooked once for the class and reads them back off whichever instance the
/// bus calls it on, which is `self` below because
/// `HookInstanceFunction` binds `self` to the receiver for a static method
/// whose second parameter (after `ENV*`) is `Object*` — `MessageBusConnection`
/// relies on the same binding for its own `<init>`.
class MessageBusRawCallback : public Object {
public:
    void (*sink)(const char*) = nullptr;
    std::string message_id;

    /// `run(Ljava/lang/String;)V`
    ///
    /// The argument is the raw JSON the engine published. It is measured and
    /// forwarded; it is never parsed here and never printed. Parsing belongs
    /// on the Rust side, where `serde_json` already lives and where the rule
    /// about printing names rather than values is written down and tested.
    static void run(ENV*, Object* self, std::shared_ptr<String> payload) {
        auto* cb = dynamic_cast<MessageBusRawCallback*>(self);
        const std::string json =
            payload ? static_cast<const std::string&>(*payload) : std::string();
        if (trace()) {
            fprintf(stderr, "[messagebus] %s published %zu bytes%s\n",
                    (cb && !cb->message_id.empty()) ? cb->message_id.c_str() : "(unknown id)",
                    json.size(), (cb && cb->sink) ? "" : " and nothing is listening");
        }
        if (cb && cb->sink) {
            cb->sink(json.c_str());
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

/// Registers `RawCallback` and `Connection` once for the whole process.
///
/// Called from `android_classes.cpp`'s `JNI_OnLoad` path. Must not be called a
/// second time from anywhere else — a second `GetClass` under the same name
/// overwrites the first registration, which is the exact mistake this file's
/// header describes happening once already. Every subscriber, clipboard
/// included, shares this one registration and gets its own `Subscription`
/// through `cordial_messagebus_subscribe` instead.
void register_clipboard_classes(jnivm::ENV* env) {
    MessageBusRawCallback::Register(env);
    MessageBusConnection::Register(env);
}

} // namespace cordial

extern "C" {

/// `MessageBus.doSubscribeRaw(String messageId, RawCallback cb, boolean) -> Connection`
///
/// One callback object and one `Connection` are created and kept alive per
/// `message_id`, looked up through `cordial::subscription_for`, rather than in
/// the single set of globals this file used to hold. `sink` may be null: that
/// is the control case a run with `CORDIAL_SKIP_CLIPBOARD=1` still exercises —
/// registration and the subscribe call both still happen, and only whether
/// anything acts on what comes back differs. Installed on the callback object
/// before the subscribing call, not after: the bus may deliver a message
/// synchronously from inside `doSubscribeRaw`, and a callback whose sink was
/// set only after the call returned would silently drop that first delivery.
///
/// The third `doSubscribeRaw` argument is passed `false`. Its meaning is not
/// established; `false` is the answer that asks for nothing extra, and a
/// `true` whose effect nobody here has measured would be a claim this file
/// cannot support. INFERRED that it selects some replay-or-not behaviour, from
/// its position and type alone — unchanged from clipboard's original
/// reasoning, because nothing about the generalisation touches this call.
///
/// The class is passed where a receiver would go, which is what
/// `deeplink.cpp`'s `publishRaw` caller already does for this same class and
/// what works there.
int cordial_messagebus_subscribe(void* fn, const char* message_id, void (*sink)(const char*),
                                 char* err, size_t err_len) {
    using Call = jobject (*)(JNIEnv*, jobject, jstring, jobject, jboolean);
    auto* env = cordial::process_env();
    if (!fn || !env || !message_id) {
        snprintf(err, err_len, "no JavaVM, or doSubscribeRaw is not exported");
        return -1;
    }
    const std::string id(message_id);
    auto& sub = cordial::subscription_for(id);
    try {
        auto cls = env->GetClass("com/roblox/universalapp/messagebus/MessageBus");
        auto cb = std::make_shared<cordial::MessageBusRawCallback>();
        cb->sink = sink;
        cb->message_id = id;
        auto jid = std::make_shared<cordial::String>(id);
        // Parked before the call, not after: the bus may deliver a message
        // synchronously from inside this very call, and a callback owned only
        // by a local would already be a candidate for collection.
        sub.callback = cb;
        jobject r = reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                               (jobject)cordial::to_jni(env, cls),
                                               (jstring)cordial::to_jni(env, jid),
                                               (jobject)cordial::to_jni(env, cb),
                                               (jboolean) false);
        auto* conn = dynamic_cast<cordial::MessageBusConnection*>(
            reinterpret_cast<cordial::Object*>(r));
        if (conn) {
            sub.connection_ptr.store(static_cast<long long>(conn->ptr),
                                     std::memory_order_release);
            // Hold the object as well as its address. The address is what
            // `isConnected` wants; the object is what keeps the engine's own
            // shared_ptr from being released by `Connection.finalize`.
            sub.connection = conn->shared_from_this();
        } else {
            // Not fatal, and deliberately not reported as success either. A
            // subscribe that returned something other than a Connection has
            // still installed the callback as far as anything here can tell,
            // but nothing can then confirm it, so say so.
            sub.connection_ptr.store(0, std::memory_order_release);
            sub.connection.reset();
        }
        return 0;
    } catch (const std::exception& e) {
        sub.callback.reset();
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        sub.callback.reset();
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// The `long` inside the `Connection` a prior `cordial_messagebus_subscribe`
/// for `message_id` returned, or 0 when that id was never subscribed or the
/// engine handed back something this side could not read as a `Connection`.
/// Feeds `cordial_messagebus_is_connected`; on its own it means only that a
/// `Connection` came back at all.
long long cordial_messagebus_connection_ptr(const char* message_id) {
    if (!message_id) {
        return 0;
    }
    std::lock_guard<std::mutex> lock(cordial::g_subscriptions_mutex);
    auto it = cordial::g_subscriptions.find(message_id);
    if (it == cordial::g_subscriptions.end() || !it->second) {
        return 0;
    }
    return it->second->connection_ptr.load(std::memory_order_acquire);
}

/// `Connection.isConnected(J)` — a static native taking the subscription's own
/// address. Writes 1 or 0 into `*out_connected`. Unchanged in shape from
/// clipboard's original version: the address alone is what the native wants,
/// so this does not need to know which message id it came from.
int cordial_messagebus_is_connected(void* fn, long long ptr, int* out_connected, char* err,
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
