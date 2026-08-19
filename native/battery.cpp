// The host's battery, told to the engine.
//
// `NativeGLInterface.reportBatteryStateChanged(II)V` and
// `NativeGLInterface.reportBatteryStatus(Lcom/roblox/engine/jni/model/
// BatteryStatus;)V` are exported by every build this project has looked at and
// neither had ever been called. See `crates/cordial-runtime/src/battery.rs` for
// the sysfs read, the argument-meaning reasoning (`docs/traces/
// waydroid-roblox-startup.log.gz` settles what the two ints track together, not
// their exact numbering, which is `INFERRED` from Android's own public
// `BatteryManager` constants), and why every optional field below is left
// unset rather than guessed at.
//
// `BatteryStatus`'s fields were not guessed either. `tools/dex_method.py
// ~/.cache/cordial-dex/ --class .../BatteryStatus` shows only a no-arg
// `<init>()V` — a plain field-carrying object, the same shape as
// `DeviceStaticParams` in `android_classes.cpp` — so this project's own
// `tools/dex_fields.py` was written to answer the next question that tool
// leaves open: what the fields are named and typed. Declaration metadata only
// — the dex's `field_ids` table and each class's `encoded_field` list, the same
// category of information `dex_method.py` already reads for methods, not a
// decompilation of how anything is implemented. It found fifteen public
// instance fields, every one a boxed `java.lang.{Integer,Boolean,Long,Float}`
// or `String` rather than a primitive — nullable on purpose, on Roblox's side,
// which is exactly the shape a battery that cannot answer every question wants.
//
// Two nested enums confirm the reading of `status`/`plugged`/`health` as raw
// Android ints rather than Roblox-internal ones: `BatteryStatus$a`'s seven
// members (`COLD`, `DEAD`, `GOOD`, `OVERHEAT`, `OVER_VOLTAGE`, `UNKNOWN`,
// `UNSPECFIED_FAILURE` — that misspelling is Roblox's, preserved rather than
// corrected because it is what the dex says) are exactly Android's seven
// `BATTERY_HEALTH_*` values; `$b`'s six (`AC`, `DOCK`, `NOT_PLUGGED`,
// `UNKNOWN`, `USB`, `WIRELESS`) are `BATTERY_PLUGGED_*` plus "not plugged" and
// "unknown"; `$c`'s five (`CHARGING`, `DISCHARGING`, `FULL`, `NOT_CHARGING`,
// `UNKNOWN`) are `BATTERY_STATUS_*`. Each carries `androidValue`/`robloxValue`
// int fields, i.e. a lookup from Android's raw number to Roblox's own —
// confirming the outer object's plain-`Integer` fields are meant to receive
// Android's raw numbers, with Roblox doing its own translation on the far
// side, not Cordial's job to replicate.
//
// **What was not established, because no APK was available in the session that
// wrote this**: the actual live-run confirmation every other class in this
// file's neighbours got, of which fields the engine actually reads off a
// `reportBatteryStatus` argument. `DeviceStaticParams`'s own header describes
// how that confirmation normally happens — return a live object and let
// libjnivm's `Constructed Unresolved symbol` log name whatever field the
// engine reached for that was not yet hooked. All fifteen fields below are
// hooked pre-emptively, which is the honest inverse of that gap: it does not
// under-claim (nothing here uses fields the engine ignores as evidence they
// exist) but it also has not been watched happen. Confirming that, or finding
// out some of these fifteen are never read, is `tools/hook_descriptors.py`'s
// job for spelling and `CORDIAL_JNI_TRACE=1` in a real run for the rest.

#include <jnivm.h>

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>

namespace cordial {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using String = jnivm::String;

template <class T>
static auto to_jni(jnivm::ENV* env, const std::shared_ptr<T>& p) {
    return jnivm::JNITypes<std::shared_ptr<T>>::ToJNIType(env, p);
}

jnivm::ENV* process_env();

// ---------------------------------------------------------------- box types
//
// libjnivm has no built-in `java.lang.Integer`/`Boolean`/`Long`/`Float` —
// `BatteryStatus` is the first surface in this tree to need a boxed field
// rather than a primitive one or a `String`. Real application code unboxes
// through `intValue()`/`booleanValue()`/`longValue()`/`floatValue()`, so that
// is what is registered here; nothing assumes the engine pokes at these
// classes' private layout, which would be exactly the kind of ART-internals
// guess `docs/adr/` and AGENTS.md both warn away from.
//
// Kept minimal on purpose: no `valueOf`, no caching, no interning. Cordial is
// always the one constructing these — the engine only ever reads a
// `BatteryStatus` Cordial built — so there is no call path that needs a
// factory the engine itself invokes.

class JavaInteger : public Object {
public:
    jint value = 0;
    jint intValue(ENV*) { return value; }
    static void Register(ENV* env) {
        env->GetClass<JavaInteger>("java/lang/Integer");
        auto c = env->GetClass("java/lang/Integer");
        c->HookInstanceFunction(env, "intValue", &JavaInteger::intValue);
    }
};

class JavaBoolean : public Object {
public:
    jboolean value = false;
    jboolean booleanValue(ENV*) { return value; }
    static void Register(ENV* env) {
        env->GetClass<JavaBoolean>("java/lang/Boolean");
        auto c = env->GetClass("java/lang/Boolean");
        c->HookInstanceFunction(env, "booleanValue", &JavaBoolean::booleanValue);
    }
};

class JavaLong : public Object {
public:
    jlong value = 0;
    jlong longValue(ENV*) { return value; }
    static void Register(ENV* env) {
        env->GetClass<JavaLong>("java/lang/Long");
        auto c = env->GetClass("java/lang/Long");
        c->HookInstanceFunction(env, "longValue", &JavaLong::longValue);
    }
};

class JavaFloat : public Object {
public:
    jfloat value = 0;
    jfloat floatValue(ENV*) { return value; }
    static void Register(ENV* env) {
        env->GetClass<JavaFloat>("java/lang/Float");
        auto c = env->GetClass("java/lang/Float");
        c->HookInstanceFunction(env, "floatValue", &JavaFloat::floatValue);
    }
};

static std::shared_ptr<JavaInteger> box_int(jint v) {
    auto p = std::make_shared<JavaInteger>();
    p->value = v;
    return p;
}
static std::shared_ptr<JavaBoolean> box_bool(jboolean v) {
    auto p = std::make_shared<JavaBoolean>();
    p->value = v;
    return p;
}
static std::shared_ptr<JavaFloat> box_float(jfloat v) {
    auto p = std::make_shared<JavaFloat>();
    p->value = v;
    return p;
}

// ------------------------------------------------------------- BatteryStatus

/// `com.roblox.engine.jni.model.BatteryStatus` — see this file's header for
/// where the fifteen field names below came from and what is and is not
/// confirmed about them.
///
/// Every field defaults to null (`nullptr`), which is the honest state for
/// "sysfs on this machine did not answer this question" — a boxed type reads
/// as Java `null` from an unset `shared_ptr`, not a fabricated zero.
/// `battery_low` and `battery_saver_mode` are never set by anything in this
/// file: Cordial has no low-battery threshold or power-saver concept of its
/// own to report, and inventing one — Android's real low-battery event is a
/// system policy decision, not a sysfs fact — would be exactly the kind of
/// comfortable lie this project's `native/opensles.cpp` precedent argues
/// against. `energy_counter` is likewise never set: no sysfs node on any
/// machine this was written against maps to it by name, and guessing a
/// formula (current × voltage, say) under a field called "energy counter"
/// would misrepresent a derived number as a measured one.
class BatteryStatus : public Object {
public:
    std::shared_ptr<JavaBoolean> present;
    std::shared_ptr<JavaInteger> batteryPercentage;
    std::shared_ptr<JavaBoolean> batterySaverMode; // always null — see class doc
    std::shared_ptr<JavaInteger> chargeCounter;
    std::shared_ptr<JavaInteger> currentAverage;
    std::shared_ptr<JavaInteger> currentNow;
    std::shared_ptr<JavaLong> energyCounter;       // always null — see class doc
    std::shared_ptr<JavaInteger> health;
    std::shared_ptr<JavaInteger> plugged;
    std::shared_ptr<JavaInteger> power;
    std::shared_ptr<JavaBoolean> batteryLow;       // always null — see class doc
    std::shared_ptr<JavaInteger> status;
    std::shared_ptr<String> technology;
    std::shared_ptr<JavaFloat> temperature;
    std::shared_ptr<JavaInteger> voltage;

    // libjnivm rewrites an *instance* `<init>` lookup into a *static* one with
    // the return type folded into the signature — `ClientLocalFlags` in
    // `init_params.cpp` hit this first and its own comment explains it in
    // full. Same idiom here: a zero-argument static factory, matching the
    // dex's `<init>()V`.
    static std::shared_ptr<BatteryStatus> ctor(ENV*, Class*) {
        return std::make_shared<BatteryStatus>();
    }

    static void Register(ENV* env) {
        const char* name = "com/roblox/engine/jni/model/BatteryStatus";
        env->GetClass<BatteryStatus>(name);
        auto c = env->GetClass(name);
        c->Hook(env, "<init>", &BatteryStatus::ctor);
        c->HookInstance(env, "present", &BatteryStatus::present);
        c->HookInstance(env, "batteryPercentage", &BatteryStatus::batteryPercentage);
        c->HookInstance(env, "batterySaverMode", &BatteryStatus::batterySaverMode);
        c->HookInstance(env, "chargeCounter", &BatteryStatus::chargeCounter);
        c->HookInstance(env, "currentAverage", &BatteryStatus::currentAverage);
        c->HookInstance(env, "currentNow", &BatteryStatus::currentNow);
        c->HookInstance(env, "energyCounter", &BatteryStatus::energyCounter);
        c->HookInstance(env, "health", &BatteryStatus::health);
        c->HookInstance(env, "plugged", &BatteryStatus::plugged);
        c->HookInstance(env, "power", &BatteryStatus::power);
        c->HookInstance(env, "batteryLow", &BatteryStatus::batteryLow);
        c->HookInstance(env, "status", &BatteryStatus::status);
        c->HookInstance(env, "technology", &BatteryStatus::technology);
        c->HookInstance(env, "temperature", &BatteryStatus::temperature);
        c->HookInstance(env, "voltage", &BatteryStatus::voltage);
    }
};

/// Registers the box types and `BatteryStatus`. Called once, from
/// `android_classes.cpp`'s `cordial_register_android_classes` — see that
/// file's own list of `register_*_classes` calls, which this joins rather than
/// duplicates the pattern of.
void register_battery_classes(jnivm::ENV* env) {
    JavaInteger::Register(env);
    JavaBoolean::Register(env);
    JavaLong::Register(env);
    JavaFloat::Register(env);
    BatteryStatus::Register(env);
}

} // namespace cordial

// --------------------------------------------------------------- extern "C"
//
// The Rust-facing surface, matching the shape `init_params.cpp`'s
// `cordial_pass_current_refresh_rate` and friends already establish: resolve
// the exported native by symbol name on the Rust side, hand the function
// pointer in here, and this file does the `jnivm::ENV`/`jobject` plumbing Rust
// cannot name.

extern "C" {

/// `NativeGLInterface.reportBatteryStateChanged(II)V`.
///
/// `status` and `plugged` are Android's own `BatteryManager` raw values — see
/// this file's header and `crates/cordial-runtime/src/battery.rs` for where
/// that reading came from and what about it is `INFERRED`.
int cordial_report_battery_state_changed(void* fn, int status, int plugged, char* err,
                                          size_t err_len) {
    using Call = void (*)(JNIEnv*, jclass, jint, jint);
    auto* env = cordial::process_env();
    if (!fn || !env) {
        snprintf(err, err_len, "no JavaVM, or reportBatteryStateChanged is not exported");
        return -1;
    }
    try {
        auto cls = env->GetClass("com/roblox/engine/jni/NativeGLInterface");
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jclass)cordial::to_jni(env, cls),
                                   static_cast<jint>(status), static_cast<jint>(plugged));
        return 0;
    } catch (const std::exception& e) {
        snprintf(err, err_len, "%s", e.what());
        return -1;
    } catch (...) {
        snprintf(err, err_len, "non-standard C++ exception");
        return -1;
    }
}

/// Every field of `BatteryStatus`, as a flat C struct so the FFI boundary does
/// not need fifteen positional parameters of five different types. Each
/// nullable field carries its own `has_*` flag; when clear the corresponding
/// value is ignored and the Java field is left null, not zeroed — the whole
/// point being that Cordial only claims what sysfs actually answered. Mirrored
/// on the Rust side as a `#[repr(C)]` struct in `crates/cordial-linker-sys`.
///
/// `temperature_c` is a real Celsius float, not the tenths-of-a-degree integer
/// Android's own `EXTRA_TEMPERATURE` uses and `battery.rs` reads out of
/// sysfs's `temp` node — this class's field is declared `Ljava/lang/Float;`,
/// not `Integer`, which only makes sense as an already-converted value, so the
/// division happens on the Rust side before this struct is filled in.
struct CordialBatteryStatus {
    int32_t has_present;
    int32_t present;
    int32_t has_percentage;
    int32_t percentage;
    int32_t has_status;
    int32_t status;
    int32_t has_health;
    int32_t health;
    int32_t has_voltage_mv;
    int32_t voltage_mv;
    int32_t has_current_now_ua;
    int32_t current_now_ua;
    int32_t has_current_avg_ua;
    int32_t current_avg_ua;
    int32_t has_charge_counter_uah;
    int32_t charge_counter_uah;
    int32_t has_power_now_uw;
    int32_t power_now_uw;
    int32_t has_technology;
    const char* technology; // NUL-terminated; ignored unless has_technology
    int32_t has_temperature_c;
    float temperature_c;
    int32_t has_plugged;
    int32_t plugged;
};

/// `NativeGLInterface.reportBatteryStatus(Lcom/roblox/engine/jni/model/BatteryStatus;)V`.
int cordial_report_battery_status(void* fn, const CordialBatteryStatus* in, char* err,
                                   size_t err_len) {
    using Call = void (*)(JNIEnv*, jclass, jobject);
    auto* env = cordial::process_env();
    if (!fn || !env || !in) {
        snprintf(err, err_len, "no JavaVM, no reading, or reportBatteryStatus is not exported");
        return -1;
    }
    try {
        auto status = std::make_shared<cordial::BatteryStatus>();
        if (in->has_present) status->present = cordial::box_bool(in->present != 0);
        if (in->has_percentage) status->batteryPercentage = cordial::box_int(in->percentage);
        if (in->has_status) status->status = cordial::box_int(in->status);
        if (in->has_health) status->health = cordial::box_int(in->health);
        if (in->has_voltage_mv) status->voltage = cordial::box_int(in->voltage_mv);
        if (in->has_current_now_ua) status->currentNow = cordial::box_int(in->current_now_ua);
        if (in->has_current_avg_ua) status->currentAverage = cordial::box_int(in->current_avg_ua);
        if (in->has_charge_counter_uah)
            status->chargeCounter = cordial::box_int(in->charge_counter_uah);
        if (in->has_power_now_uw) status->power = cordial::box_int(in->power_now_uw);
        if (in->has_technology && in->technology)
            status->technology = std::make_shared<cordial::String>(std::string(in->technology));
        if (in->has_temperature_c) status->temperature = cordial::box_float(in->temperature_c);
        if (in->has_plugged) status->plugged = cordial::box_int(in->plugged);
        // batteryLow, batterySaverMode, energyCounter: never set. See the
        // class's own doc comment for why.

        auto cls = env->GetClass("com/roblox/engine/jni/NativeGLInterface");
        reinterpret_cast<Call>(fn)(env->GetJNIEnv(), (jclass)cordial::to_jni(env, cls),
                                   (jobject)cordial::to_jni(env, status));
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
