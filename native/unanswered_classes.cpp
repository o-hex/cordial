// Classes the engine looks up and Cordial answered with nothing.
//
// A `CORDIAL_JNI_TRACE=ON` capture of a landing-page run shows the engine
// asking libjnivm for 39 distinct classes. Seven of them had no implementation
// anywhere in `native/`. Five are here; the two djinni local-storage ones
// (`ILocalStorageHandlerCore$CppProxy`, `com/snapchat/djinni/NativeObjectManager`)
// belong to the RbxStorage investigation and are deliberately not touched here.
//
// **Why registering a class matters even when nothing calls its methods.**
// libjnivm invents a class when `FindClass` misses, and an invented class
// invents its members too. That is not a harmless placeholder: it is what
// produced the crash in `docs/analysis/flag-init.md` §40, where djinni
// constructed a `java.lang.ref.WeakReference`, libjnivm's fabricated `<init>`
// returned null, and djinni asserted on the null and took the process down
// with `RBXCRASH`. So "the lookup resolves" and "the class behaves" are
// different states, and a resolved lookup against an invented class is the
// more dangerous of the two because it looks fine.
//
// **What is deliberately not hooked, and why that is the honest choice.**
// Only one method across all five is actually invoked on a landing-page run --
// `NetworkUtils.getPublicIPv4Addresseses`, measured, twice. Everything else is
// `FindClass`-only. Hooking a method nothing calls means inventing a return
// value with no observation to check it against, and AGENTS.md's rule is that
// a stub which reports success is worse than one which reports failure,
// because the engine proceeds on an answer that is not true and fails
// somewhere with no relationship to the cause.
//
// So the rule applied here is: hook a method only where Cordial has a truthful
// answer today. Everything else is left unhooked on purpose. An unhooked
// method on a *registered* class surfaces through libjnivm's own unresolved
// reporting and lands in Cordial's end-of-run table, which is exactly where a
// gap should be visible -- rather than being papered over with a plausible
// zero.

#include <jnivm.h>

#include <arpa/inet.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <sys/socket.h>

#include <cstdio>
#include <cstdlib>
#include <memory>
#include <string>

namespace cordial {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using jnivm::String;

namespace {

std::shared_ptr<String> S(const char* v) {
    return std::make_shared<String>(v ? v : "");
}

/// `com.roblox.engine.jni.util.NetworkUtils`
///
/// The one method in this file that is observed being called: twice on a
/// landing-page run, `Method=getPublicIPv4Addresseses` in the JNI trace. The
/// spelling of the name is the engine's, not a typo introduced here -- the dex
/// declares `getPublicIPv4Addresseses()Ljava/lang/String;` and libjnivm binds
/// by exact name, so correcting it to something that reads better would make
/// the hook silently never fire. That failure mode has cost this project five
/// separate bugs.
///
/// **What "public" means here, and why this returns local interface
/// addresses.** On Android this reads the device's own IPv4 addresses off its
/// network interfaces; it does not call out to a service to discover an
/// internet-facing address, and nothing here does either -- no packet leaves
/// the machine to answer this. `getifaddrs` is the direct equivalent of what
/// the platform does.
///
/// Loopback is excluded because an Android device does not report `127.0.0.1`
/// as one of its addresses and including it would be a different answer from
/// the one the engine expects, not a more complete one.
///
/// **This does hand the engine the machine's LAN address**, which is a real
/// privacy surface and worth stating plainly rather than burying. It is the
/// same value the official Android client reports from the same call, so this
/// is parity rather than new exposure, and answering falsely is not an option
/// this project takes -- but anyone uncomfortable with it should know the call
/// exists rather than discover it later.
class NetworkUtils : public Object {
public:
    static std::shared_ptr<String> getPublicIPv4Addresseses(ENV*, Class*) {
        struct ifaddrs* list = nullptr;
        if (getifaddrs(&list) != 0 || !list) {
            // Reporting an empty string here would say "this device has no
            // addresses", which is a claim about the network rather than
            // about the failure that actually happened. Say so in the log and
            // still return empty, because the signature has nowhere else to
            // put the distinction.
            fprintf(stderr, "[roblox] NetworkUtils: getifaddrs failed; reporting no addresses\n");
            if (list) freeifaddrs(list);
            return S("");
        }
        std::string out;
        for (struct ifaddrs* a = list; a; a = a->ifa_next) {
            if (!a->ifa_addr || a->ifa_addr->sa_family != AF_INET) {
                continue;
            }
            if (a->ifa_flags & IFF_LOOPBACK) {
                continue;
            }
            char buf[INET_ADDRSTRLEN] = {};
            auto* in4 = reinterpret_cast<struct sockaddr_in*>(a->ifa_addr);
            if (!inet_ntop(AF_INET, &in4->sin_addr, buf, sizeof(buf))) {
                continue;
            }
            if (!out.empty()) {
                out += ",";
            }
            out += buf;
        }
        freeifaddrs(list);
        return S(out.c_str());
    }

    static void Register(ENV* env) {
        env->GetClass<NetworkUtils>("com/roblox/engine/jni/util/NetworkUtils");
        auto c = env->GetClass("com/roblox/engine/jni/util/NetworkUtils");
        // **Static, and this was got wrong first time round.** The dex
        // declares `<init>()V` beside it, which reads like an instance class,
        // and it was registered with `HookInstanceFunction` on that basis. The
        // engine looks it up the other way, and libjnivm said so plainly on
        // the very first run:
        //
        //   Constructed Unresolved symbol, Class=`...NetworkUtils`,
        //   StaticMethod=`getPublicIPv4Addresseses`, Signature=`()Ljava/lang/String;`
        //
        // An instance hook against a static lookup binds nothing, silently, in
        // both directions -- the sixth instance of that failure in this
        // repository. It is only visible here because the class is registered:
        // an unregistered class would have had libjnivm invent the method and
        // return null with no complaint at all.
        c->Hook(env, "getPublicIPv4Addresseses", &NetworkUtils::getPublicIPv4Addresseses);
    }
};

/// `com.roblox.audio.AppRtcDeviceWrapper`
///
/// WebRTC's Android audio-device wrapper, used for voice chat routing. Cordial
/// has no Android audio routing at all: audio goes out through FMOD to
/// `native/opensles.cpp` and PipeWire, and there is no `AudioManager`, no
/// communication mode and no selectable device behind any of this.
///
/// `isValid()` is hooked and returns **false**, which is not a stub failing
/// safe by accident -- it is the honest answer. The dex declares it alongside
/// `getSelectedAudioDeviceAsInt`, `getSelectedAudioDeviceName`,
/// `wrapStartCommunication`, `wrapStopCommunication` and
/// `wrapSetCommunicationMute`, which is the shape of a wrapper that expects to
/// be asked whether it works before being driven. Answering false is the
/// pattern `native/opensles.cpp` already uses when it reports
/// `SL_RESULT_FEATURE_UNSUPPORTED` rather than handing back a dead engine
/// object.
///
/// The five driving methods are deliberately **not** hooked. Nothing has been
/// observed calling them, and a `wrapStartCommunication` that silently does
/// nothing would tell the engine communication started when no audio path
/// exists. If something does call one, it surfaces as unresolved, which is
/// where a gap belongs.
class AppRtcDeviceWrapper : public Object {
public:
    jboolean isValid(ENV*) { return JNI_FALSE; }

    static void Register(ENV* env) {
        env->GetClass<AppRtcDeviceWrapper>("com/roblox/audio/AppRtcDeviceWrapper");
        auto c = env->GetClass("com/roblox/audio/AppRtcDeviceWrapper");
        c->HookInstanceFunction(env, "isValid", &AppRtcDeviceWrapper::isValid);
    }
};

/// `com.roblox.engine.jni.memstorage.Connection`
///
/// Registered so the lookup resolves against a real class, and nothing more.
///
/// This is not a subsystem Cordial implements: every method on `MemStorage`
/// (`bind`, `fire`, `getItem`, `setItem`, `hasItem`, `removeItem`) is exported
/// by `libroblox.so` as a `Java_com_roblox_engine_jni_memstorage_*` native, so
/// the engine owns both sides and the app only holds the handle. `Connection`
/// is what `bind` returns, which is why the engine resolves the class -- it
/// needs it to construct the return value -- and `disconnect` and
/// `releaseConnection` are likewise engine natives.
///
/// `<init>(J)V` takes the native handle. It is not hooked: libjnivm's own
/// object construction is what the engine's native side expects to receive,
/// and fabricating a constructor that discards the handle is precisely the
/// §40 failure. Nothing is known to call it here in any case.
///
/// **In-memory, despite the shape of the name.** This is not the `rbx-storage`
/// disk cache and should not be confused with it while that investigation is
/// live -- see `docs/analysis/flag-init.md` §41.
class MemStorageConnection : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<MemStorageConnection>("com/roblox/engine/jni/memstorage/Connection");
    }
};

/// `org.webrtc.voiceengine.BuildInfo`
///
/// Registered, with **no getter hooked**, and that is a considered choice
/// rather than an unfinished one.
///
/// The class is nine device-identity getters -- brand, model, manufacturer,
/// device, product, build id, build release, build type, SDK version. Cordial
/// already has truthful answers for every one of them, but they live in
/// `native/init_params.cpp` behind `presenting_as_pc()` and the device-profile
/// switch, both file-static. Answering them here would mean a second copy of
/// "what device are we", and this codebase has already been bitten by exactly
/// that: `cordial_build_user_agent` exists specifically so the web view and
/// the engine cannot drift into two answers to that question.
///
/// So the follow-up is to expose the device profile from `init_params.cpp` the
/// way `S_pub` is exposed and hook these against it -- one source of truth,
/// nine getters. Not done here because that file is held by other work in
/// flight, and because nothing has been observed calling any of these: the
/// engine resolves the class and stops. A duplicated device identity shipped
/// today to satisfy calls that never come is a worse trade than a registered
/// class and a named follow-up.
class WebRtcBuildInfo : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<WebRtcBuildInfo>("org/webrtc/voiceengine/BuildInfo");
    }
};

/// `org.webrtc.voiceengine.WebRtcAudioTrack`
///
/// Registered only. This is WebRTC's playback path -- an `AudioTrack`, a
/// dedicated `AudioTrackThread`, an enum of start-error codes and two error
/// callback interfaces. Implementing it would mean standing up a second audio
/// output alongside the FMOD/OpenSL ES path that already works, driven by a
/// thread whose contract nothing here has observed.
///
/// Nothing is hooked. There is no honest partial version of an audio track:
/// one that accepts buffers and drops them would report playing audio that
/// nobody can hear, which is the failure mode `native/opensles.cpp`'s comment
/// exists to warn about. If voice chat ever reaches this class the unresolved
/// methods will say so by name, which is a better starting point for whoever
/// picks it up than a silent sink.
class WebRtcAudioTrack : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<WebRtcAudioTrack>("org/webrtc/voiceengine/WebRtcAudioTrack");
    }
};

} // namespace

/// Registered from `register_platform_classes`, which already runs at the
/// right point in `android_classes.cpp`'s bring-up.
///
/// **Every class above is its own C++ type, and that is load-bearing.**
/// libjnivm keys one class per C++ type on `typeid`, so registering two names
/// against the same type -- `GetClass<Object>(...)` in a loop being the way it
/// happens -- leaves the later name owning `typeid` and silently rewrites the
/// signatures of anything registered against the earlier one. That is the bug
/// found in `register_shared_preferences` and recorded in
/// `docs/analysis/flag-init.md` §40; it cost a crash that looked like a djinni
/// fault. Anything added here needs a distinct type for the same reason.
void register_unanswered_classes(ENV* env) {
    NetworkUtils::Register(env);
    AppRtcDeviceWrapper::Register(env);
    MemStorageConnection::Register(env);
    WebRtcBuildInfo::Register(env);
    WebRtcAudioTrack::Register(env);
}

} // namespace cordial
