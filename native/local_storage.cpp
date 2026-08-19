// `com.roblox.protocols.localstorageplatforminterface.generated.*`, and the
// screen-orientation call docs/analysis/flag-init.md §16 found missing.
//
// Two unrelated gaps share this file because both are small and both are new:
// the task that added them could only create one C++ file, and CMakeLists.txt
// lists sources by name rather than by glob (see the comment there on why —
// a `local_storage.cpp` that a build did not know to compile links with
// "undefined symbol" and looks like a mistake in the calling code instead).
//
// **What `localstorageplatforminterface` is, and what it is not.**
// `docs/analysis/flag-init.md` §12 already separates two things the dex and
// the engine both call "storage": `RbxStorage`, the content cache that gates
// on `LocalStorageManager.getAllocatableBytes` and that Cordial has never
// found a way to construct directly, and *this* -- `ILocalStorageHandlerCore`
// and `IPlatformLocalStorageHandler` -- which is a small per-user key/value
// store the engine asks the platform to hold on its behalf. The two are
// unrelated exports of the same class name prefix and nothing here bears on
// the `RbxStorage::init` question; §12 already established that nothing in
// the dex or the exports constructs `RbxStorage`, and adding this does not
// change that.
//
// **The direction of the call, worked out from `readelf`, not assumed.**
// `Java_..._ILocalStorageHandlerCore_setPlatformImpl` and the `nativeDestroy`/
// `native_*` family under `IPlatformLocalStorageHandler$CppProxy` are all
// *exported by* `libroblox.so` -- i.e. they are native methods the engine's
// own compiled code answers, the same shape as `LocalStorageManager`'s
// `initStorageManagerNativeV3` a few classes over. `setPlatformImpl` is the
// entry point: on real Android, application code builds a Java object
// implementing `IPlatformLocalStorageHandler` and hands it to this static, and
// the engine then calls straight back into that object's own methods --
// `getSecureValue`, `setCurrentUser` and the rest -- whenever it wants to read
// or write something. Cordial has no Java side to build that object, so this
// file is it: `PlatformLocalStorageHandler` below is a libjnivm `Object`
// subclass whose hooked methods answer exactly the calls the interface
// declares, in the same idiom `init_params.cpp`'s `AndroidActivity` and
// `Resources` already use for the classes the engine expects to call methods
// on rather than construct.
//
// The `$CppProxy` classes and their `native_*` exports are the reverse
// direction of the same djinni scaffolding -- a Java handle onto a *C++*
// implementation living inside the engine itself, used somewhere else in
// `libroblox.so` -- and are not this file's concern; nothing here constructs
// one or calls into it.
//
// **Reference, not source.** The shape of the call --
// `IPlatformLocalStorageHandler` built and handed to `setPlatformImpl` right
// after the asset manager is set up -- was confirmed against
// `mocktail/src/legacy/legacy_runtime.cc`'s `ConfigureLocalStorage`
// (`third_party/mocktail-webview/` is a different, unrelated vendor tree; the
// checkout this was read from lives outside this repository entirely, per the
// task that added this file). Mocktail is Apache-2.0; this is a GPL-3.0
// project, and nothing below is transcribed from it -- mocktail backs its
// handler with a generic `SharedPreferences` object and an XML file with no
// attempt at the secrecy the method names claim, which is a design this
// project has an explicit rule against repeating (see `secrets.rs`'s own
// header, and the note below). What is reused is the fact of the call and its
// argument shapes; the storage behind it is Cordial's own.
//
// **This handles secrets, named as such.** `getSecureValue`/`setSecureValue`
// and their `ForUser`/`ForCurrentUser` twins carry per-account credentials --
// the dex's own vocabulary, not a guess. AGENTS.md's rule and
// `crates/cordial-runtime/src/secrets.rs`'s own header apply here exactly as
// they do to the cookie jar and the identity mirror: nothing below prints a
// value or a user id at any verbosity. The user id is logged nowhere, not even
// at trace level; only key names and byte counts are, the same restraint
// `secrets.rs` documents for itself.
//
// **The store itself lives in Rust, not here, and not in `secrets.rs`.**
// `secrets.rs` already is this project's model for where a secret belongs --
// the desktop Secret Service first, a `0600` file second, told plainly either
// way -- and the right thing was to call it, not rebuild it. Its `Kind` enum
// only has two members, `Cookies` and `Identity`, each holding exactly one
// document per profile; a per-user, arbitrary-key store does not fit that
// shape, and the task that added this file left `secrets.rs` off limits to
// edit. So `crates/cordial-runtime/src/bin/load.rs` carries a second, small
// implementation of the same *reasoning* -- keyring first, honest fallback,
// nothing ever printed -- under its own schema, reachable from here only
// through the four `cordial_local_storage_*` externs below. That module's own
// header says why it is not simply a third `Kind`.
//
// **Never make a stub lie.** A value that was never stored, or that the
// keyring would not hand back, comes back to the engine as `null`, not `""`.
// An empty string here is indistinguishable from "the value is empty" and the
// engine has no way to tell the two apart; AGENTS.md's rule about
// `opensles.cpp` reporting failure honestly instead of a dead success applies
// exactly as much to a credential as it does to an audio engine.

#include <jnivm.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <vector>

namespace cordial {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using jnivm::String;

jnivm::ENV* process_env();

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

// Defined in `android_classes.cpp`, beside `NativeUserJavaInterface` --
// `init_params.cpp` already forward-declares the same four for
// `StartAppParams`, with the comment explaining why this project has one
// source for "who is signed in" rather than a header. Only the two needed
// here are repeated.
jlong identity_user_id();
bool identity_known();

// The Rust side of the secure store. Declared here rather than pulled from a
// generated header for the reason every cross-language boundary in this
// directory is: `cordial-linker-sys`, which owns the usual generated bindings
// for `native/`, was off limits to the task that added this file, so these
// four symbols are declared directly against what
// `crates/cordial-runtime/src/bin/load.rs` exports with `#[no_mangle]`.
//
// Return codes, shared by all four: `0` succeeded (a "not found" read is a
// success that found nothing, not a failure); anything negative is a reason
// to answer the engine honestly rather than to guess. `cordial_local_storage_get`
// additionally distinguishes "absent" (`1` not set) from "present" (`1` set,
// `*out_len` gives the length written) so the caller never has to infer one
// from the other.
extern "C" {
int cordial_local_storage_get(long long user_id, const char* key, char* out, size_t out_cap,
                               int* found, size_t* out_len);
int cordial_local_storage_set(long long user_id, const char* key, const char* value,
                              size_t value_len);
int cordial_local_storage_delete(long long user_id, const char* key);
int cordial_local_storage_delete_user(long long user_id);
} // extern "C"

// ---------------------------------------------------------------------------
// Which user "current" means.
//
// The interface has three families of method -- plain, `...ForUser(long)` and
// `...ForCurrentUser` -- plus `deleteCurrentUserValues()`, which is not even
// given a user id to work with. All four have to agree on one answer, kept
// here rather than re-derived per call.
// ---------------------------------------------------------------------------

namespace {
std::mutex g_users_mutex;
jlong g_current_user = 0;
bool g_current_user_set = false;
std::set<jlong> g_known_users;

/// Seeds from the signed-in identity the first time anything asks, so local
/// storage's notion of "current user" and `NativeUserJavaInterface`'s cannot
/// name two different accounts. Caller holds `g_users_mutex`.
jlong current_user_locked() {
    if (!g_current_user_set) {
        g_current_user = identity_known() ? identity_user_id() : 0;
        g_current_user_set = true;
        if (g_current_user != 0) {
            g_known_users.insert(g_current_user);
        }
    }
    return g_current_user;
}
} // namespace

/// `java.util.HashSet`, enough of it for `getUsers()`'s return value.
///
/// **What this is not**: a real enumeration of every user this profile has
/// ever stored a value for. Building that would mean listing every item the
/// desktop keyring holds for this profile and reading each one's attributes
/// back, which is a second, heavier D-Bus round trip this call has never been
/// measured to need. What it reports instead is honest on its own narrower
/// terms -- the users Cordial has actually seen this run, seeded from the
/// signed-in identity and grown by `setCurrentUser`/`setSecureValueForUser` --
/// which for the account-per-profile shape this project runs is the same set
/// either way. `size()`/`isEmpty()` only, matching `JavaMap`'s and
/// `JavaList`'s "enough of it" in `init_params.cpp`; no iterator, because
/// nothing here has needed one to observe.
class UserSet : public Object {
public:
    std::set<jlong> ids;

    jint size(ENV*) { return static_cast<jint>(ids.size()); }
    jboolean isEmpty(ENV*) { return ids.empty(); }

    static void Register(ENV* env) {
        env->GetClass<UserSet>("java/util/HashSet");
        auto c = env->GetClass("java/util/HashSet");
        c->HookInstanceFunction(env, "size", &UserSet::size);
        c->HookInstanceFunction(env, "isEmpty", &UserSet::isEmpty);
    }
};

/// `com.roblox.protocols.localstorageplatforminterface.generated.IPlatformLocalStorageHandler`
///
/// One instance, constructed once by `cordial_local_storage_set_platform_impl`
/// below and held by the engine for the life of the process -- the same
/// lifetime `cookies.cpp`'s `g_handler` documents for the same reason: the
/// engine calls back into this from its own thread, long after the call that
/// registered it returned, so nothing here may be a local.
class PlatformLocalStorageHandler : public Object {
public:
    jlong getCurrentUser(ENV*) {
        std::lock_guard<std::mutex> lock(g_users_mutex);
        return current_user_locked();
    }

    jboolean setCurrentUser(ENV*, jlong userId) {
        std::lock_guard<std::mutex> lock(g_users_mutex);
        g_current_user = userId;
        g_current_user_set = true;
        g_known_users.insert(userId);
        return JNI_TRUE;
    }

    std::shared_ptr<UserSet> getUsers(ENV* env) {
        std::lock_guard<std::mutex> lock(g_users_mutex);
        current_user_locked();
        auto set = std::make_shared<UserSet>();
        set->ids = g_known_users;
        to_jni(env, set);
        return set;
    }

    std::shared_ptr<String> getSecureValue(ENV* env, std::shared_ptr<String> key) {
        jlong user;
        {
            std::lock_guard<std::mutex> lock(g_users_mutex);
            user = current_user_locked();
        }
        return read(env, user, key);
    }
    std::shared_ptr<String> getSecureValueForCurrentUser(ENV* env, std::shared_ptr<String> key) {
        return getSecureValue(env, key);
    }
    std::shared_ptr<String> getSecureValueForUser(ENV* env, std::shared_ptr<String> key,
                                                  jlong userId) {
        return read(env, userId, key);
    }

    jboolean setSecureValue(ENV*, std::shared_ptr<String> key, std::shared_ptr<String> value) {
        jlong user;
        {
            std::lock_guard<std::mutex> lock(g_users_mutex);
            user = current_user_locked();
        }
        return write(user, key, value);
    }
    jboolean setSecureValueForCurrentUser(ENV* env, std::shared_ptr<String> key,
                                          std::shared_ptr<String> value) {
        return setSecureValue(env, key, value);
    }
    jboolean setSecureValueForUser(ENV*, std::shared_ptr<String> key,
                                   std::shared_ptr<String> value, jlong userId) {
        return write(userId, key, value);
    }

    jboolean deleteSecureValue(ENV*, std::shared_ptr<String> key) {
        if (!key) {
            return JNI_FALSE;
        }
        jlong user;
        {
            std::lock_guard<std::mutex> lock(g_users_mutex);
            user = current_user_locked();
        }
        std::string k(*key);
        int rc = cordial_local_storage_delete(static_cast<long long>(user), k.c_str());
        return rc == 0 ? JNI_TRUE : JNI_FALSE;
    }

    jboolean deleteUserValues(ENV*, jlong userId) {
        int rc = cordial_local_storage_delete_user(static_cast<long long>(userId));
        std::lock_guard<std::mutex> lock(g_users_mutex);
        g_known_users.erase(userId);
        return rc == 0 ? JNI_TRUE : JNI_FALSE;
    }

    jboolean deleteCurrentUserValues(ENV* env) {
        jlong user;
        {
            std::lock_guard<std::mutex> lock(g_users_mutex);
            user = current_user_locked();
        }
        return deleteUserValues(env, user);
    }

    static void Register(ENV* env) {
        const char* name =
            "com/roblox/protocols/localstorageplatforminterface/generated/"
            "IPlatformLocalStorageHandler";
        env->GetClass<PlatformLocalStorageHandler>(name);
        auto c = env->GetClass(name);
        c->HookInstanceFunction(env, "getCurrentUser", &PlatformLocalStorageHandler::getCurrentUser);
        c->HookInstanceFunction(env, "setCurrentUser", &PlatformLocalStorageHandler::setCurrentUser);
        c->HookInstanceFunction(env, "getUsers", &PlatformLocalStorageHandler::getUsers);
        c->HookInstanceFunction(env, "getSecureValue", &PlatformLocalStorageHandler::getSecureValue);
        c->HookInstanceFunction(env, "getSecureValueForCurrentUser",
                                &PlatformLocalStorageHandler::getSecureValueForCurrentUser);
        c->HookInstanceFunction(env, "getSecureValueForUser",
                                &PlatformLocalStorageHandler::getSecureValueForUser);
        c->HookInstanceFunction(env, "setSecureValue", &PlatformLocalStorageHandler::setSecureValue);
        c->HookInstanceFunction(env, "setSecureValueForCurrentUser",
                                &PlatformLocalStorageHandler::setSecureValueForCurrentUser);
        c->HookInstanceFunction(env, "setSecureValueForUser",
                                &PlatformLocalStorageHandler::setSecureValueForUser);
        c->HookInstanceFunction(env, "deleteSecureValue",
                                &PlatformLocalStorageHandler::deleteSecureValue);
        c->HookInstanceFunction(env, "deleteUserValues",
                                &PlatformLocalStorageHandler::deleteUserValues);
        c->HookInstanceFunction(env, "deleteCurrentUserValues",
                                &PlatformLocalStorageHandler::deleteCurrentUserValues);
    }

private:
    /// A single fixed-size buffer rather than the usual libjnivm two-call
    /// growth protocol: these are credentials and small platform tokens, not
    /// documents, and `secrets.rs`'s own bodies (a cookie jar, a whole
    /// identity record) are smaller than this in ordinary operation. A value
    /// that does not fit is refused rather than truncated -- a truncated
    /// credential is a corrupt one, and corrupt is a worse answer than absent.
    static constexpr size_t kMaxValue = 8192;

    static std::shared_ptr<String> read(ENV*, jlong userId, const std::shared_ptr<String>& key) {
        if (!key) {
            return nullptr;
        }
        std::string k(*key);
        std::vector<char> buf(kMaxValue);
        int found = 0;
        size_t len = 0;
        int rc = cordial_local_storage_get(static_cast<long long>(userId), k.c_str(), buf.data(),
                                           buf.size(), &found, &len);
        if (rc != 0 || !found) {
            // Absent, a locked keyring, a value too large for `kMaxValue`, or
            // any other reason the store could not answer -- all of them are
            // "nothing stored" to the engine. `crates/cordial-runtime/src/bin/
            // load.rs` is where the distinction is logged; nothing here lies
            // about which one happened by returning "" for any of them.
            return nullptr;
        }
        if (len > buf.size()) {
            len = buf.size();
        }
        return std::make_shared<String>(std::string(buf.data(), len));
    }

    static jboolean write(jlong userId, const std::shared_ptr<String>& key,
                          const std::shared_ptr<String>& value) {
        if (!key || !value) {
            return JNI_FALSE;
        }
        std::string k(*key);
        std::string v(*value);
        if (v.size() > kMaxValue) {
            return JNI_FALSE;
        }
        int rc = cordial_local_storage_set(static_cast<long long>(userId), k.c_str(), v.c_str(),
                                           v.size());
        return rc == 0 ? JNI_TRUE : JNI_FALSE;
    }
};

void register_local_storage_classes(ENV* env) {
    UserSet::Register(env);
    PlatformLocalStorageHandler::Register(env);
}

} // namespace cordial

extern "C" {

/// `ILocalStorageHandlerCore.setPlatformImpl(IPlatformLocalStorageHandler)
///   -> ILocalStorageHandlerCore`
///
/// A static native -- confirmed by mocktail's own call, which passes the
/// *class* object as the receiver rather than constructing a core instance
/// first (`ConfigureLocalStorage` in `legacy_runtime.cc`; see this file's
/// header for what "confirmed" means here). Cordial builds the one argument
/// the engine actually reads from and discards the `ILocalStorageHandlerCore`
/// this returns -- nothing here has found a reason to call anything on it.
///
/// **Measured to crash, and called from `load.rs` behind
/// `CORDIAL_LOCAL_STORAGE_SET_PLATFORM_IMPL` because of it.** The call
/// returns cleanly -- this function's own `rc == 0` -- but the engine's own
/// djinni glue starts throwing `djinni (djinni_support.cpp:529): weakRef`
/// immediately afterwards, repeatedly, and the process goes on to SIGSEGV a
/// few calls later inside libc's `_IO_fflush`, fault address `0x8`: the same
/// crash signature `docs/analysis/flag-init.md` §7.4/§15 records for an
/// unrelated native, which reads as heap corruption rather than a clean
/// null-check failure. A control run with only this one call skipped reached
/// `app ready: Landing` and exited 0; the identical run with only this call
/// re-enabled crashed under `lldb`. `IPlatformLocalStorageHandler` and
/// `ILocalStorageHandlerCore` are djinni-generated -- the `$CppProxy`
/// siblings in the dex are the giveaway -- and djinni's runtime is INFERRED
/// to need working weak global references to wrap a Java-side implementation,
/// which libjnivm is not established to provide; this is not confirmed by
/// reading `djinni_support.cpp`, which is engine code past what AGENTS.md
/// allows disassembling. Mocktail's own equivalent step defaults off in that
/// project too, which in hindsight was a warning this file's first version
/// read as merely a signal.
int cordial_local_storage_set_platform_impl(void* fn, char* err, size_t err_len) {
    using Call = jobject (*)(JNIEnv*, jclass, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or setPlatformImpl is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass(
            "com/roblox/protocols/localstorageplatforminterface/generated/ILocalStorageHandlerCore");
        auto handler = std::make_shared<cordial::PlatformLocalStorageHandler>();
        cordial::to_jni(env, handler);
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(),
                                   (jclass)cordial::to_jni(env, cls),
                                   (jobject)cordial::to_jni(env, handler));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// `NativeInputInterface.nativeUpdateScreenOrientation(I)V`
///
/// The descriptor is read from the dex with `dexproto.py`, not guessed --
/// `docs/analysis/flag-init.md` §16 records the class as the one call
/// mocktail makes between `initializeNativeCode` and the settings handshake
/// that Cordial did not. It is otherwise unrelated to the storage classes
/// above; both live here because the task that added them could only create
/// one new `.cpp` file (see the header).
///
/// The two orientation values are Android's own `Configuration.ORIENTATION_*`
/// constants -- `init_params.cpp`'s `Configuration` class already reports the
/// same two numbers through `getConfiguration().orientation`, derived from the
/// same width/height comparison. Duplicated here rather than shared through a
/// header, which is this directory's usual reason: one header for two
/// constants would be the only one in `native/`.
int cordial_update_screen_orientation(void* fn, int width, int height, char* err, size_t err_len) {
    constexpr jint kOrientationPortrait = 1;
    constexpr jint kOrientationLandscape = 2;
    using Call = void (*)(JNIEnv*, jclass, jint);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or nativeUpdateScreenOrientation is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeInputInterface");
        jint orientation = width >= height ? kOrientationLandscape : kOrientationPortrait;
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jclass)cordial::to_jni(env, cls), orientation);
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
