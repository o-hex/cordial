// The Java side of Cordial's framework layer.
//
// Roblox's native code calls out to Java for everything the platform is supposed
// to answer. libjnivm hands it stub classes by default, which return null — and
// Roblox notices:
//
//     W/JNIMain  DeviceStaticParams is null.
//
// Each class implemented here replaces one of those nulls. The method surface is
// not guessed: `--dump-classes` records exactly what Roblox reached for, and
// because it only reaches further once it gets a non-null answer, implementing
// one class reveals the next. See docs/analysis/observed-java-surface.md.

#include <jnivm.h>

#include <cstdio>
#include <sys/stat.h>
#include <atomic>
#include <chrono>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <memory>

// ------------------------------------------------------- focused text box
//
// Which text box the engine currently has focus in, learned from
// `showKeyboard` and needed by `nativePassText`. See the comment on
// `NativeGLJavaInterface::showKeyboard` for why this has to be plumbed through
// rather than assumed.
//
// Written from the engine's thread inside `showKeyboard`, read from the input
// thread when a key is dispatched, so it is guarded rather than plain.
namespace {
std::atomic<long long> g_textbox_handle{0};
std::mutex g_textbox_mutex;
std::string g_textbox_text;
/// Bumped on every focus change so the input side can tell "same box, keep
/// editing" from "new box, reseed the buffer" without comparing handles — a
/// handle can be reused after a box is destroyed.
std::atomic<unsigned> g_textbox_generation{0};
} // namespace

extern "C" void cordial_textbox_focused(long long handle, const char* text) {
    if (getenv("CORDIAL_TRACE_TEXT")) {
        fprintf(stderr, "[cordial] textbox focused handle=%lld current=%zu bytes\n",
                handle, text ? strlen(text) : 0);
    }
    {
        std::lock_guard<std::mutex> lock(g_textbox_mutex);
        g_textbox_text = text ? text : "";
    }
    g_textbox_handle.store(handle, std::memory_order_release);
    g_textbox_generation.fetch_add(1, std::memory_order_acq_rel);
}

extern "C" void cordial_textbox_blurred() {
    if (getenv("CORDIAL_TRACE_TEXT")) {
        fprintf(stderr, "[cordial] textbox blurred\n");
    }
    g_textbox_handle.store(0, std::memory_order_release);
    g_textbox_generation.fetch_add(1, std::memory_order_acq_rel);
}

/// The focused box's handle, or 0 when nothing is focused. A 0 here is why
/// text must not be sent at all, rather than sent to handle 0.
extern "C" long long cordial_textbox_handle() {
    return g_textbox_handle.load(std::memory_order_acquire);
}

extern "C" unsigned cordial_textbox_generation() {
    return g_textbox_generation.load(std::memory_order_acquire);
}

/// Copy the focused box's current contents into `buf`. Returns the number of
/// bytes written, not counting the NUL.
extern "C" int cordial_textbox_text(char* buf, int n) {
    if (!buf || n <= 0) return 0;
    std::lock_guard<std::mutex> lock(g_textbox_mutex);
    int len = static_cast<int>(g_textbox_text.size());
    if (len > n - 1) len = n - 1;
    memcpy(buf, g_textbox_text.data(), static_cast<size_t>(len));
    buf[len] = '\0';
    return len;
}

namespace cordial {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using String = jnivm::String;

/// libjnivm's `String` derives both `Object` and `std::string`, so a Java string
/// is simply constructed — there is no VM-side allocator to go through.
inline std::shared_ptr<String> str(const char* v) {
    return std::make_shared<String>(std::string(v ? v : ""));
}

/// `java.lang.String.getBytes(String charsetName)`
///
/// libjnivm provides `String` but not this method, and the engine reaches it on
/// the text-entry path: it reads AGDK's `gametextinput/State.text` field and
/// then converts that string to bytes before consuming it. Observed as the last
/// thing to happen on every keystroke before nothing happened:
///
///     Invoked Field Getter Class=`com/google/androidgamesdk/gametextinput/State` Field=`text`
///     Found symbol, Class=`java/lang/String`, Method=`getBytes`, Signature=`(Ljava/lang/String;)[B`
///     Call Unknown Member Function Class=`java/lang/String` Method=`getBytes`
///
/// "Call Unknown Member Function" is the engine asking and getting nothing, so
/// the text it had just read was converted to no bytes at all. Every keystroke
/// reached AGDK correctly and died here.
///
/// The charset argument is accepted and ignored. libjnivm's `String` derives
/// `std::string` and holds UTF-8 already, and the only charset Android's text
/// input path asks for is UTF-8; honouring a request for anything else would
/// mean carrying an encoder for a case that does not arise. If a non-UTF-8
/// charset is ever observed here, that is worth a real conversion rather than a
/// silent wrong answer.
class StringBridge {
public:
    static std::shared_ptr<jnivm::Array<jbyte>> getBytes(ENV*, Object* self,
                                                         std::shared_ptr<String>) {
        auto* str = dynamic_cast<String*>(self);
        const std::string text = str ? static_cast<const std::string&>(*str) : std::string();
        auto out = std::make_shared<jnivm::Array<jbyte>>(static_cast<jsize>(text.size()));
        if (!text.empty()) {
            memcpy(out->getArray(), text.data(), text.size());
        }
        return out;
    }

    static void Register(ENV* env) {
        auto c = env->GetClass("java/lang/String");
        c->HookInstanceFunction(env, "getBytes", &StringBridge::getBytes);
    }
};

/// `com.roblox.engine.jni.model.DeviceStaticParams`
///
/// The device description Roblox reads once at startup and then believes for the
/// rest of the session — form factor, screen metrics, identifiers. This is where
/// spec §4.2's "Roblox thinks you're mobile" is actually decided; the
/// `__system_property_get` values in `bionic` cover the native side, but this
/// object is what the engine consults.
///
/// Fields are added as Roblox is observed reading them. It stops at the first
/// null, so the surface only becomes visible one answer at a time.
class DeviceStaticParams : public Object {
public:
    // Field names and types were not guessed. Returning a live object instead of
    // null made Roblox read them, and libjnivm named each one as it did.
    std::shared_ptr<String> osVersion;
    std::shared_ptr<String> deviceName;
    std::shared_ptr<String> appVersion;
    std::shared_ptr<String> manufacturer;
    std::shared_ptr<String> deviceSku;
    std::shared_ptr<String> appBuildVariant;
    std::shared_ptr<String> socModel;
    jboolean cpu64Bit = true;

    static std::shared_ptr<DeviceStaticParams> Create() {
        auto p = std::make_shared<DeviceStaticParams>();
        // Desktop values, deliberately. Roblox reads this once and believes it for
        // the session, so it is the single most load-bearing place to be honest
        // about what Cordial is. Claiming to be a particular phone would invite
        // device-specific workarounds that do not apply here.
        p->osVersion       = str("15");
        p->deviceName      = str("Cordial");
        p->manufacturer    = str("Cordial");
        p->deviceSku       = str("cordial");
        p->socModel        = str("cordial");
        p->appBuildVariant = str("release");
        // Left as the client's own version until Cordial reads it from the APK
        // manifest; a wrong value here shows up in telemetry and support threads.
        p->appVersion      = str("");
        p->cpu64Bit        = true;
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<DeviceStaticParams>("com/roblox/engine/jni/model/DeviceStaticParams");
        auto c = env->GetClass("com/roblox/engine/jni/model/DeviceStaticParams");
        c->HookInstance(env, "osVersion", &DeviceStaticParams::osVersion);
        c->HookInstance(env, "deviceName", &DeviceStaticParams::deviceName);
        c->HookInstance(env, "appVersion", &DeviceStaticParams::appVersion);
        c->HookInstance(env, "manufacturer", &DeviceStaticParams::manufacturer);
        c->HookInstance(env, "deviceSku", &DeviceStaticParams::deviceSku);
        c->HookInstance(env, "appBuildVariant", &DeviceStaticParams::appBuildVariant);
        c->HookInstance(env, "socModel", &DeviceStaticParams::socModel);
        c->HookInstance(env, "cpu64Bit", &DeviceStaticParams::cpu64Bit);
    }
};

/// `com.roblox.engine.jni.model.NativeTextBoxInfo`
///
/// The text-box state the on-screen keyboard would have edited. Desktop input
/// never needs it, but it has to exist as a type for the `showKeyboard`
/// descriptor to match.
class NativeTextBoxInfo : public Object {
public:
    // The engine constructs one of these and hands it to `showKeyboard`. With no
    // constructor registered, libjnivm logged
    //
    //     Call Unknown Static Function ... Method=`<init>`
    //
    // and produced nothing, so the engine passed a null down its own call. An
    // unresolved JNI call is not free: it can leave a pending exception that
    // every later JNI call on the thread then trips over, which is a plausible
    // way for text entry to be wedged well after the call that caused it.
    //
    // The descriptor is (FFFFFZIIIIIIZZ) — five floats for the box's rectangle
    // and font metrics, then flags and counts. The fields are not interpreted
    // here: Cordial has no on-screen keyboard to lay out, so what matters is
    // that a real object exists and the call resolves.
    static std::shared_ptr<NativeTextBoxInfo> init(
        ENV*, Class*, jfloat, jfloat, jfloat, jfloat, jfloat, jboolean, jint, jint,
        jint, jint, jint, jint, jboolean, jboolean) {
        return std::make_shared<NativeTextBoxInfo>();
    }

    static void Register(ENV* env) {
        env->GetClass<NativeTextBoxInfo>("com/roblox/engine/jni/model/NativeTextBoxInfo");
        auto c = env->GetClass("com/roblox/engine/jni/model/NativeTextBoxInfo");
        c->Hook(env, "<init>", &NativeTextBoxInfo::init);
    }
};

/// `com.roblox.engine.jni.NativeGLJavaInterface`
///
/// The engine's main line back into Java: device parameters, keyboard, screen
/// orientation, purchase prompts, overlays, and the leave/exit notifications that
/// become `onLeave` in the plugin event schema.
class NativeGLJavaInterface : public Object {
public:
    static std::shared_ptr<DeviceStaticParams> getDeviceStaticParams(ENV*, Class*) {
        // Returning a live object rather than null is the whole point: Roblox
        // logs and gives up on null, and never reaches the code that would tell
        // us which fields it wants.
        return DeviceStaticParams::Create();
    }

    // The on-screen keyboard, and the half of text entry that runs engine->host.
    //
    // These were no-ops on the reasoning that a desktop has no soft keyboard, so
    // host key events could reach the engine through the input path alone. That
    // was wrong, and it is why the login form's boxes stayed empty: the `jlong`
    // is the *handle of the text box being edited*, and it is the only place the
    // host is ever told which box has focus. Android's IME keeps it and passes it
    // straight back as the first argument of `nativePassText`. Cordial threw it
    // away and then sent text for handle 0, so every keystroke arrived addressed
    // to a box that was not the focused one.
    //
    // The `byte[]` is the box's current contents. Editing has to start from that
    // rather than from an empty buffer, otherwise the first keystroke in a
    // pre-filled or re-focused field wipes it.
    //
    // Signature is (JZ[BLcom/roblox/engine/jni/model/NativeTextBoxInfo;)V. libjnivm
    // matches hooks on the descriptor derived from the C++ types, so `byte[]` has
    // to be an Array<jbyte> and the last argument the real class — Object would
    // produce Ljava/lang/Object; and silently never match.
    static void showKeyboard(ENV*, Class*, jlong handle, jboolean,
                             std::shared_ptr<jnivm::Array<jbyte>> current,
                             std::shared_ptr<NativeTextBoxInfo>) {
        std::string text;
        if (current) {
            for (jsize i = 0; i < current->getSize(); i++) {
                jbyte b = (*current)[i];
                if (b == 0) break;
                text.push_back(static_cast<char>(b));
            }
        }
        cordial_textbox_focused(handle, text.c_str());
    }
    static void hideKeyboard(ENV*, Class*) { cordial_textbox_blurred(); }

    // In-app purchases go through Google Play Billing, which does not exist here.
    // Silently doing nothing is the honest behaviour: the alternative is
    // pretending a purchase flow started and leaving the engine waiting for a
    // result that never arrives.
    static void promptNativePurchase(ENV*, Class*, jlong, std::shared_ptr<String>,
                                     std::shared_ptr<String>) {}
    static void promptNativePurchaseShort(ENV*, Class*, jlong, std::shared_ptr<String>) {}
    static void promptNativePurchaseWithPayload(ENV*, Class*, jlong, std::shared_ptr<String>,
                                                std::shared_ptr<String>) {}

    static void exitGameWithError(ENV*, Class*, jint code) {
        fprintf(stderr, "[roblox] exitGameWithError(%d)\n", code);
    }
    static void gameDidLeave(ENV*, Class*) {
        // This is `onLeave` in the plugin event schema (spec §9a), and where
        // "close when you leave an experience" hangs off. Both need core.
        fprintf(stderr, "[roblox] gameDidLeave\n");
    }
    static void onAppShellReloadNeeded(ENV*, Class*) {}

    // The engine notifies Java when the focused Lua text box changes, and when
    // one of its properties does. Observed as unresolved during sign-in:
    //
    //     Constructed Unresolved symbol ... `onLuaTextBoxChangedCallback`, `(Ljava/lang/String;)V`
    //     Constructed Unresolved symbol ... `onLuaTextBoxPropertyChangedCallback`, `()V`
    //
    // Nothing on a desktop needs to react to either — there is no IME view to
    // reposition — so these are no-ops. They exist because *resolving* is the
    // point: an unresolved call is a different thing from a call that did
    // nothing, and only the second one is safe.
    static void onLuaTextBoxChangedCallback(ENV*, Class*, std::shared_ptr<String>) {}
    static void onLuaTextBoxPropertyChangedCallback(ENV*, Class*) {}
    static void listenToMotionEvents(ENV*, Class*, std::shared_ptr<String>) {}
    static void screenOrientationChanged(ENV*, Class*, jint) {}
    static void openNativeOverlay(ENV*, Class*, std::shared_ptr<String>,
                                  std::shared_ptr<String>) {}

    static void Register(ENV* env) {
        env->GetClass<NativeGLJavaInterface>("com/roblox/engine/jni/NativeGLJavaInterface");
        auto c = env->GetClass("com/roblox/engine/jni/NativeGLJavaInterface");
        c->Hook(env, "getDeviceStaticParams", &NativeGLJavaInterface::getDeviceStaticParams);
        c->Hook(env, "showKeyboard", &NativeGLJavaInterface::showKeyboard);
        c->Hook(env, "hideKeyboard", &NativeGLJavaInterface::hideKeyboard);
        c->Hook(env, "promptNativePurchase", &NativeGLJavaInterface::promptNativePurchase);
        c->Hook(env, "promptNativePurchaseWithPayload",
                &NativeGLJavaInterface::promptNativePurchaseWithPayload);
        c->Hook(env, "exitGameWithError", &NativeGLJavaInterface::exitGameWithError);
        c->Hook(env, "gameDidLeave", &NativeGLJavaInterface::gameDidLeave);
        c->Hook(env, "onAppShellReloadNeeded", &NativeGLJavaInterface::onAppShellReloadNeeded);
        c->Hook(env, "onLuaTextBoxChangedCallback",
                &NativeGLJavaInterface::onLuaTextBoxChangedCallback);
        c->Hook(env, "onLuaTextBoxPropertyChangedCallback",
                &NativeGLJavaInterface::onLuaTextBoxPropertyChangedCallback);
        c->Hook(env, "listenToMotionEvents", &NativeGLJavaInterface::listenToMotionEvents);
        c->Hook(env, "screenOrientationChanged",
                &NativeGLJavaInterface::screenOrientationChanged);
        c->Hook(env, "openNativeOverlay", &NativeGLJavaInterface::openNativeOverlay);
    }
};

/// `com.roblox.engine.jni.locale.NativeLocaleJavaInterface`
///
/// Roblox distinguishes three locales: the system's, the one the account is set
/// to, and the one the current experience is running in. Only the first is
/// Cordial's to answer; the other two are account and session state it does not
/// have, so they mirror the system locale until auth exists.
class NativeLocaleJavaInterface : public Object {
public:
    static std::shared_ptr<String> systemLocale() {
        // Android wants a BCP-47-ish tag. POSIX gives "en_AU.UTF-8"; take the
        // language and region and drop the encoding.
        const char* raw = getenv("LC_ALL");
        if (!raw || !*raw) raw = getenv("LC_MESSAGES");
        if (!raw || !*raw) raw = getenv("LANG");
        if (!raw || !*raw) return str("en_us");

        std::string tag(raw);
        if (auto dot = tag.find('.'); dot != std::string::npos) {
            tag.resize(dot);
        }
        if (tag.empty() || tag == "C" || tag == "POSIX") {
            return str("en_us");
        }
        for (auto& ch : tag) {
            ch = static_cast<char>(tolower(static_cast<unsigned char>(ch)));
        }
        return str(tag.c_str());
    }

    static std::shared_ptr<String> getLocale(ENV*, Class*) { return systemLocale(); }
    static std::shared_ptr<String> getRobloxLocale(ENV*, Class*) { return systemLocale(); }
    static std::shared_ptr<String> getGameLocale(ENV*, Class*) { return systemLocale(); }

    static void Register(ENV* env) {
        env->GetClass<NativeLocaleJavaInterface>(
            "com/roblox/engine/jni/locale/NativeLocaleJavaInterface");
        auto c = env->GetClass("com/roblox/engine/jni/locale/NativeLocaleJavaInterface");
        c->Hook(env, "getLocale", &NativeLocaleJavaInterface::getLocale);
        c->Hook(env, "getRobloxLocale", &NativeLocaleJavaInterface::getRobloxLocale);
        c->Hook(env, "getGameLocale", &NativeLocaleJavaInterface::getGameLocale);
    }
};

/// `com.roblox.engine.jni.user.NativeUserJavaInterface`
///
/// Who is signed in. Nobody is: Cordial has no auth yet, and the honest answer to
/// every one of these is the signed-out value rather than a plausible-looking
/// fake. A fabricated user id would flow straight into telemetry and analytics as
/// if it were real.
class NativeUserJavaInterface : public Object {
public:
    static jlong getUserId(ENV*, Class*) { return 0; }
    static jboolean getIsUnder13(ENV*, Class*) {
        // This mirrors account state; it is not the age gate. Roblox enforces age
        // restrictions server-side from the account itself, so a real under-13
        // account is restricted whether or not the client says so here.
        //
        // Defaulting to under-13 therefore protects nobody and degrades the
        // client for the majority of players, who are teens and adults. Once auth
        // exists this comes from the signed-in account and stops being a default
        // at all.
        return false;
    }
    static jint getMembershipType(ENV*, Class*) { return 0; }
    static jboolean getHasRobloxSubscription(ENV*, Class*) { return false; }
    static std::shared_ptr<String> getUsername(ENV*, Class*) { return str(""); }
    static std::shared_ptr<String> getDisplayName(ENV*, Class*) { return str(""); }
    static std::shared_ptr<String> getAlternateName(ENV*, Class*) { return str(""); }
    static std::shared_ptr<String> getPlatformName(ENV*, Class*) {
        // Roblox's own name for the platform family, not Cordial's. The engine
        // branches on this for input handling and store behaviour, and it only
        // knows the values its own builds ship with.
        return str("Android");
    }
    static std::shared_ptr<String> getTheme(ENV*, Class*) { return str("Dark"); }

    static void Register(ENV* env) {
        env->GetClass<NativeUserJavaInterface>("com/roblox/engine/jni/user/NativeUserJavaInterface");
        auto c = env->GetClass("com/roblox/engine/jni/user/NativeUserJavaInterface");
        c->Hook(env, "getUserId", &NativeUserJavaInterface::getUserId);
        c->Hook(env, "getIsUnder13", &NativeUserJavaInterface::getIsUnder13);
        c->Hook(env, "getUsername", &NativeUserJavaInterface::getUsername);
        c->Hook(env, "getDisplayName", &NativeUserJavaInterface::getDisplayName);
        c->Hook(env, "getAlternateName", &NativeUserJavaInterface::getAlternateName);
        c->Hook(env, "getPlatformName", &NativeUserJavaInterface::getPlatformName);
        c->Hook(env, "getMembershipType", &NativeUserJavaInterface::getMembershipType);
        c->Hook(env, "getHasRobloxSubscription", &NativeUserJavaInterface::getHasRobloxSubscription);
        c->Hook(env, "getTheme", &NativeUserJavaInterface::getTheme);
    }
};

/// `com.roblox.universalapp.logging.LoggingProtocol`
class LoggingProtocol : public Object {
public:
    /// Milliseconds since the process started. Roblox timestamps its own log
    /// lines against this, so a constant would make every duration zero.
    static jlong getProcessTimestamp(ENV*, Class*) {
        static const auto start = std::chrono::steady_clock::now();
        auto now = std::chrono::steady_clock::now();
        return std::chrono::duration_cast<std::chrono::milliseconds>(now - start).count();
    }

    static void Register(ENV* env) {
        env->GetClass<LoggingProtocol>("com/roblox/universalapp/logging/LoggingProtocol");
        auto c = env->GetClass("com/roblox/universalapp/logging/LoggingProtocol");
        c->Hook(env, "getProcessTimestamp", &LoggingProtocol::getProcessTimestamp);
    }
};

/// The directory Roblox may write to.
///
/// On Android this is the app's private storage. Here it is per-instance, which
/// is the mechanism the multi-account design rests on: two instances that never
/// share a files directory can never share a session.
/// See docs/design/instances-and-launch.md §4.
const char* files_dir() {
    static const std::string dir = [] {
        if (const char* override = getenv("CORDIAL_FILES_DIR")) {
            return std::string(override);
        }
        std::string base;
        if (const char* xdg = getenv("XDG_DATA_HOME")) {
            base = xdg;
        } else if (const char* home = getenv("HOME")) {
            base = std::string(home) + "/.local/share";
        } else {
            base = "/tmp";
        }
        auto path = base + "/cordial/instances/default/data";
        // Roblox assumes the directory exists; on Android the platform made it.
        std::string acc;
        for (size_t i = 1; i <= path.size(); i++) {
            if (i == path.size() || path[i] == '/') {
                acc = path.substr(0, i);
                mkdir(acc.c_str(), 0700);
            }
        }
        return path;
    }();
    return dir.c_str();
}

/// `com.roblox.engine.jni.reporter.SessionReporterJavaInterface`
///
/// Crash and session telemetry. The reporting entry points are deliberately
/// inert — Cordial is not going to forward a user's session data to an analytics
/// endpoint on their behalf — but the getters have to answer, because the engine
/// uses `getFilesDir` for real storage and not merely for reports.
class SessionReporterJavaInterface : public Object {
public:
    static std::shared_ptr<String> getFilesDir(ENV*, Class*) { return str(files_dir()); }
    static std::shared_ptr<String> getAppVersion(ENV*, Class*) { return str(""); }
    static std::shared_ptr<String> getLastLoggedInUser(ENV*, Class*) { return str(""); }
    static std::shared_ptr<String> getLastLoggedInUserId(ENV*, Class*) { return str(""); }

    static void sendSessionReport(ENV*, Class*, std::shared_ptr<String>, std::shared_ptr<String>) {
        // Inert on purpose. See the class comment.
    }
    static void setEventTrackingGoogleAnalytics(ENV*, Class*, std::shared_ptr<String>,
                                                std::shared_ptr<String>,
                                                std::shared_ptr<String>, jlong) {
        // Likewise.
    }

    static void Register(ENV* env) {
        env->GetClass<SessionReporterJavaInterface>(
            "com/roblox/engine/jni/reporter/SessionReporterJavaInterface");
        auto c = env->GetClass("com/roblox/engine/jni/reporter/SessionReporterJavaInterface");
        c->Hook(env, "getFilesDir", &SessionReporterJavaInterface::getFilesDir);
        c->Hook(env, "getAppVersion", &SessionReporterJavaInterface::getAppVersion);
        c->Hook(env, "getLastLoggedInUser", &SessionReporterJavaInterface::getLastLoggedInUser);
        c->Hook(env, "getLastLoggedInUserId", &SessionReporterJavaInterface::getLastLoggedInUserId);
        c->Hook(env, "sendSessionReport", &SessionReporterJavaInterface::sendSessionReport);
        c->Hook(env, "setEventTrackingGoogleAnalytics",
                &SessionReporterJavaInterface::setEventTrackingGoogleAnalytics);
    }
};

/// `com.roblox.engine.jni.video.VideoCodecCapability`
class VideoCodecCapability : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<VideoCodecCapability>("com/roblox/engine/jni/video/VideoCodecCapability");
    }
};

/// `com.roblox.engine.jni.video.MediaCodecInfoUtils`
///
/// Hardware video codecs, which on Android come from MediaCodec. Cordial has no
/// MediaCodec: `libmediandk` is entirely stubbed. Reporting none is correct
/// rather than merely convenient — claiming a codec Cordial cannot decode would
/// fail later, inside video playback, with no way back to this decision.
class MediaCodecInfoUtils : public Object {
public:
    static std::shared_ptr<jnivm::Array<VideoCodecCapability>> getVideoCodecs(ENV*, Class*) {
        return std::make_shared<jnivm::Array<VideoCodecCapability>>(0);
    }
    static jboolean hevcHardwareEncodingSupported(ENV*, Class*, jint, jint, jint) {
        return false;
    }

    static void Register(ENV* env) {
        env->GetClass<MediaCodecInfoUtils>("com/roblox/engine/jni/video/MediaCodecInfoUtils");
        auto c = env->GetClass("com/roblox/engine/jni/video/MediaCodecInfoUtils");
        c->Hook(env, "getVideoCodecs", &MediaCodecInfoUtils::getVideoCodecs);
        c->Hook(env, "hevcHardwareEncodingSupported",
                &MediaCodecInfoUtils::hevcHardwareEncodingSupported);
    }
};

} // namespace cordial

extern "C" void cordial_register_android_classes(void* env_ptr) {
    auto* env = static_cast<jnivm::ENV*>(env_ptr);
    if (!env) {
        return;
    }
    cordial::StringBridge::Register(env);
    cordial::DeviceStaticParams::Register(env);
    cordial::NativeTextBoxInfo::Register(env);
    cordial::NativeGLJavaInterface::Register(env);
    cordial::NativeLocaleJavaInterface::Register(env);
    cordial::NativeUserJavaInterface::Register(env);
    cordial::LoggingProtocol::Register(env);
    cordial::SessionReporterJavaInterface::Register(env);
    cordial::VideoCodecCapability::Register(env);
    cordial::MediaCodecInfoUtils::Register(env);
    if (getenv("CORDIAL_JNI_TRACE")) {
        fprintf(stderr, "[classes] Cordial's Java side registered\n");
    }
}
