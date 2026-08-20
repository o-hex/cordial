// `android.app.ActivityThread` and `android.app.Application`.
//
// Both were checked against the dex before anything here was written, not
// assumed: `tools/dex_method.py ~/.cache/cordial-dex/ --class
// android/app/ActivityThread` lists exactly two members --
// `getApplication()Landroid/app/Application;` (instance) and
// `currentActivityThread()Landroid/app/ActivityThread;` (static) -- and both
// are in `docs/analysis/unanswered-jni-observed.tsv`, which is not a class
// dump but a capture of one real `CORDIAL_JNI_TRACE=ON` run that joined a game
// and played until the 304 disconnect. So unlike most of the classes a fork's
// wishlist names, this pair is not a guess: the engine's own native code
// reaches for them on a real session, and today gets nothing back.
//
// **Why the engine asks `ActivityThread` for a `Context` instead of using one
// it was handed.** This is the standard trick native Android SDKs use to reach
// an application `Context` from C++ code that was never given one through the
// ordinary `Activity`/`Application` lifecycle -- `ActivityThread
// .currentActivityThread().getApplication()` is a well-known, JNI-only path
// that needs no cooperation from the app's own Java bootstrap. That fits this
// codebase's running finding about the engine's Java surface: `bootstrapTheApp`
// and everything Java-bootstrap-shaped never runs here because Cordial has no
// JVM (`docs/analysis/unresolved-java.md` §2a), while calls the engine's own
// *native* code places directly through JNI -- like this one -- are exactly
// the calls Cordial can answer, because nothing upstream of them needed to be
// real Java bytecode in the first place.
//
// **What was deliberately not built alongside these two.** `PackageManager`,
// `PackageInfo`, `android/content/pm/Signature` and `SigningInfo` are real
// classes in this dex (`getPackageManager`, `getPackageInfo`, `getLongVersionCode`,
// `getSigningCertificateHistory`, `Signature.toByteArray` all resolve against
// it), and `Context.getPackageManager()` would be the natural next call after
// `getApplication()`. They are not implemented here. Two reasons, not one:
// first, neither appears anywhere in the one full real-session trace this
// project has (`docs/analysis/unanswered-jni-observed.tsv`), so unlike
// `ActivityThread` there is no observation backing the claim that the engine's
// native code ever reaches for them -- only that it plausibly could. Second,
// and separately, the signature-verification tail of that chain
// (`SigningInfo.getSigningCertificateHistory` -> `Signature.toByteArray`) is
// exactly the territory AGENTS.md's hard rule covers: "never make a stub lie".
//
// **The second reason as originally written is now wrong, and is corrected
// here rather than left to mislead.** It said Cordial "has no genuine APK
// signing certificate to hand back for this build". It has one: the APK the
// user already supplies carries the real certificate in its v2 and v3 signing
// blocks — `O=Roblox Corporation, OU=Mobile`, self-signed, valid 2014 to 2039,
// DER SHA-256 `44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477`,
// identical in both blocks. Extracting it needs no key material and no
// network: parse the End of Central Directory, walk back to the `APK Sig Block
// 42` footer, and read the first signer's first certificate. So if this chain
// were ever implemented there would be a truthful answer available, and
// nothing here would have to invent one.
//
// **What has not changed is the reason that actually decides it**: nothing has
// ever been observed asking. Building an unobserved chain is speculative work
// whichever way it would answer, and this project has paid for speculative
// work before. If it is ever built, the certificate must be read out of the
// supplied APK at the time of the call — never hardcoded from the constant
// above, which is a record of what was measured on one build and not a value
// to assert about another.
//
// Note also which side of the line this sits on. Answering a platform call
// truthfully is Cordial's job; deciding whether the client is tampered with is
// Roblox's, and AGENTS.md puts client-side integrity flags permanently out of
// scope. Cordial hands over the facts and forms no opinion about them.
//
// Rather than build it unobserved and half-finished, this is left unimplemented and reported so;
// see the task's own final report for the class-by-class account. `BatteryStatus`
// and `RobloxTelemetryEvent` were checked the same way and are absent for a
// different reason -- see the report, not this file, for why.
//
// **`HardwareInfo`, `ApkSignature`, `EcdsaPublicKey`, `AndroidSurfaceShimHandle`,
// `NativeInputStream`, `NativeOutputStream` and `MainActivity` are not in this
// file because they are not in this dex.** `tools/dex_method.py`'s `--class`
// match is a substring search over every class name the dex declares, and none
// of these seven strings appears anywhere in it, under any name. Nothing here
// answers them because there is nothing in this build that asks.

#include <jnivm.h>

#include <memory>
#include <mutex>
#include <string>

namespace cordial {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using jnivm::String;

// Every other file in this directory carries its own copy of this rather than
// sharing a header -- see the comment on `to_jni` in `init_params.cpp` for
// what it does that a raw `jnivm::JNITypes<...>::ToJNIType` call would skip
// silently (the class pointer, and keeping the object alive in the local
// frame). Kept identical here on purpose.
template <class T>
static auto to_jni(jnivm::ENV* env, const std::shared_ptr<T>& p) {
    return jnivm::JNITypes<std::shared_ptr<T>>::ToJNIType(env, p);
}

namespace {
std::shared_ptr<String> S(const char* v) {
    return std::make_shared<String>(std::string(v ? v : ""));
}
} // namespace

/// `android.app.Application`.
///
/// A real Android `Application.getProcessName()` answers the process's actual
/// name, which for an unforked, single-process app is its package name --
/// `com.roblox.client`, taken from the package every class in this dex lives
/// under (`docs/traces/README.md`'s own launch command names it directly:
/// `am start -n com.roblox.client/.startup.ActivitySplash`). That is an
/// observed fact about this APK, not a guess standing in for one.
///
/// **Registered as its own class, separately from `Context`/`Activity`, on
/// purpose.** libjnivm's `GetMethodID` walks a class's `baseclasses` function
/// when a lookup misses (`third_party/libjnivm/src/jnivm/internal/method.cpp`),
/// but that function is only populated for C++ types that declare a real
/// inheritance relationship through `Class::GetBaseClasses`; two unrelated
/// `jnivm::Object` registrations under different Java class names, which is
/// what `Context`, `Activity` and this `Application` all are, share nothing
/// and the walk finds nothing. `android_classes.cpp`'s own
/// `register_shared_preferences` already hooks `getSharedPreferences` on both
/// `android/content/Context` and `android/app/Activity` for exactly this
/// reason ("the engine resolved the method against Context but calls it on
/// whatever object it is holding"); `getApplication()` below hands the engine
/// a third kind of object capable of being asked the same question, so it is
/// added to that same hook rather than left to fail silently the way this
/// project's four previous descriptor-mismatch bugs did.
class Application : public Object {
public:
    std::shared_ptr<String> getProcessName(ENV*) { return S("com.roblox.client"); }

    static std::shared_ptr<Application> Create(ENV* env) {
        auto p = std::make_shared<Application>();
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<Application>("android/app/Application");
        auto c = env->GetClass("android/app/Application");
        c->HookInstanceFunction(env, "getProcessName", &Application::getProcessName);
    }
};

/// One instance for the process's lifetime. Real Android hands back the same
/// `Application` object to every caller; nothing here has been observed to
/// depend on that, but there is no reason to hand out a different object each
/// time and a real reason not to (object-identity comparisons, were the engine
/// ever to make one, would otherwise fail for no visible reason).
static std::shared_ptr<Application> g_application;
static std::mutex g_application_mutex;

static std::shared_ptr<Application> application_singleton(ENV* env) {
    std::lock_guard<std::mutex> lock(g_application_mutex);
    if (!g_application) {
        g_application = Application::Create(env);
    }
    return g_application;
}

/// `android.app.ActivityThread`.
///
/// Exists only to be asked for the `Application` above; nothing here has
/// observed the engine construct or hold on to the `ActivityThread` object
/// itself; asked for once and discarded is the pattern the trace shows
/// (`currentActivityThread` immediately followed by `getApplication` on
/// whatever it returned).
class ActivityThread : public Object {
public:
    std::shared_ptr<Application> getApplication(ENV* env) { return application_singleton(env); }

    static std::shared_ptr<ActivityThread> currentActivityThread(ENV* env, Class*) {
        auto p = std::make_shared<ActivityThread>();
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<ActivityThread>("android/app/ActivityThread");
        auto c = env->GetClass("android/app/ActivityThread");
        c->HookInstanceFunction(env, "getApplication", &ActivityThread::getApplication);
        c->Hook(env, "currentActivityThread", &ActivityThread::currentActivityThread);
    }
};

void register_platform_classes(ENV* env) {
    Application::Register(env);
    ActivityThread::Register(env);
}

} // namespace cordial
