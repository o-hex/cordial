// Bridging Roblox's Android accessibility surface to a Rust-side mirror that
// `crates/cordial-runtime/src/android/accessibility.rs` turns into AT-SPI —
// Linux's screen-reader protocol, the OS-level equivalent of TalkBack.
//
// Same shape as every other class in this directory: Roblox's native engine
// calls out to Java for a platform service, libjnivm hands it a stub by
// default, and this file answers instead of leaving the call unresolved.
// What is different here is *why* the surface exists in the first place —
// nothing upstream of this file asked Cordial to build it; it exists because
// `docs/analysis/framework-classes.txt` shows the shipping dex references
// `android/view/accessibility/AccessibilityManager`,
// `AccessibilityNodeInfo` (and its `AccessibilityAction`/`CollectionInfo`/
// `CollectionItemInfo` nested classes), `AccessibilityEvent` and
// `AccessibilityNodeProvider` — so the engine was built with TalkBack support
// compiled in. Whether it actually *reaches* for any of this once
// `AccessibilityManager.isEnabled()` says yes has not been observed in this
// change: no Roblox APK was available in the environment this was written in
// (see the accompanying report / docs/NEXT.md). Every class below is a
// faithful implementation of the *public, AOSP-documented* contract for its
// class — verified against `/usr/include/at-spi-2.0/atspi/atspi-constants.h`
// and a live AT-SPI bus on the AT-SPI side (see accessibility.rs), and
// against this project's own established descriptor-matching lessons on the
// JNI side — but whether Roblox's engine calls any of it beyond the
// `isEnabled` gate is INFERRED, not observed, and is exactly what
// `--dump-classes` / `CORDIAL_JNI_TRACE=1` against a real run would settle.
//
// One structural finding worth recording here rather than only in NEXT.md:
// real Android's accessibility tree is *pull*, not *push* — TalkBack asks an
// app's `AccessibilityNodeProvider` for nodes on demand; the platform never
// receives a pre-built tree. A provider is Java/Kotlin code the *app*
// subclasses, and per this project's own established finding on
// `MainGameActivity.bootstrapTheApp()`, Java/Kotlin application logic cannot
// execute under Cordial at all — there is no JVM. If Roblox's Android build
// implements its accessibility bridge that way (plausible: it is the
// documented, idiomatic mechanism for a single-View/SurfaceView app), no
// amount of hooking `AccessibilityNodeInfo` here reaches it, for the same
// reason hooking getters alone never reached FastFlags bootstrap. What *is*
// plausible, and is what this file is written to catch, is Roblox's engine
// building nodes directly over JNI the way it does everything else in
// `android_classes.cpp` — a native-to-Java push, no app-side subclass
// involved. Only a live run distinguishes the two; this file assumes the
// second and is honest scaffolding either way.
//
// ---------------------------------------------------------------------------
// MEASURED 2026-08-21, and the answer is neither: Roblox exposes no
// accessibility tree on Android at all, so nothing below is ever reached.
//
// The premise above -- "the engine was built with TalkBack support compiled
// in", inferred from the dex referencing these classes -- does not hold. A
// referenced class is not a used class, which is the same error this project
// made reading `framework-classes.txt` as a request log. Four independent
// checks against the shipping APK and a live run, all negative:
//
//   - `readelf --dyn-syms libroblox.so` has 517 `Java_*` exports across 20
//     engine interfaces (flags, GL, input, settings, video, audio, storage,
//     purchase, reporting). **None mention accessibility.** There is no native
//     entry point for a provider shim to call, so the pull model has nothing
//     to pull even if a JVM existed.
//   - No class anywhere in `classes{,2,3}.dex` is named `*Accessib*` under
//     `com/roblox/`, and none implements a provider. The only hits are one
//     `Landroid/view/accessibility/AccessibilityNodeProvider;` type reference
//     and the method name `getAccessibilityNodeProvider` -- AndroidX
//     boilerplate, not an implementation.
//   - The dex contains no `com/roblox/**` `View` or `Surface` subclass, which
//     is what a virtual-descendant provider would have to hang off.
//   - A 40 s run with `CORDIAL_ACCESSIBILITY=1`, reaching the Home screen,
//     with the AT-SPI bridge genuinely attached (`connected to the AT-SPI bus
//     as :1.2069`, so the `isEnabled` gate below answered true honestly) made
//     **zero** calls into this file: no `obtain`, no `setBoundsInScreen`, no
//     `setContentDescription`, no `sendAccessibilityEvent`.
//
// So this file is not blocked on the JVM, and it is not blocked on the gate.
// It is unreachable because the producer does not exist. It is kept rather
// than deleted for two reasons: it is a faithful implementation of the public
// AOSP contract, so it costs nothing and answers correctly if a future Roblox
// build ever does populate a tree; and the AT-SPI half in `accessibility.rs`
// is independently useful for exposing *Cordial's own* interface, which is
// GTK and already has a real accessibility tree.
//
// What this closes: a semantic UI-element route for any test harness or
// automation. There is no tree to read, so element-level access would require
// engine introspection, which ADR-001 and ADR-003 put permanently out of
// scope. A development control surface has to work in coordinates and pixels.
// ---------------------------------------------------------------------------

#include <jnivm.h>

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

namespace cordial {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using String = jnivm::String;

template <class T>
static auto to_jni(jnivm::ENV* env, const std::shared_ptr<T>& p) {
    return jnivm::JNITypes<std::shared_ptr<T>>::ToJNIType(env, p);
}

namespace {
std::shared_ptr<String> S(const char* v) {
    return std::make_shared<String>(std::string(v ? v : ""));
}
} // namespace

// ------------------------------------------------------------- the gate
//
// `AccessibilityManager.isEnabled()` is the single most load-bearing call in
// this file: real Android apps skip building an accessibility tree entirely
// when it answers false, on the (correct) assumption that nobody is listening
// and the work would be wasted. Cordial has no system AccessibilityManagerService
// to poll for "is a real service running" — the nearest honest equivalent on
// Linux is "did Cordial's own AT-SPI bridge manage to attach to the
// accessibility bus", which `accessibility.rs` decides once, early, off the
// engine's hot path, and reports here through a plain atomic rather than
// blocking a JNI call on a D-Bus round-trip.
//
// `CORDIAL_ACCESSIBILITY=0`/`=1` forces the answer either way, the same
// override-by-environment-variable idiom `CORDIAL_DPI_SCALE` and
// `CORDIAL_INPUT_TOUCH` already use elsewhere in this tree — useful for
// exercising the engine's accessible path without an AT-SPI client attached,
// or for ruling the whole feature out as a variable while debugging something
// unrelated.
std::atomic<int> g_a11y_bridge_connected{0};
} // namespace cordial

extern "C" void cordial_accessibility_set_bridge_connected(int connected) {
    cordial::g_a11y_bridge_connected.store(connected ? 1 : 0, std::memory_order_release);
}

namespace cordial {
namespace {
bool accessibility_enabled() {
    if (const char* e = getenv("CORDIAL_ACCESSIBILITY")) {
        if (*e) {
            return *e != '0';
        }
    }
    return g_a11y_bridge_connected.load(std::memory_order_acquire) != 0;
}
} // namespace

// ------------------------------------------------------- the node mirror
//
// Real `AccessibilityNodeInfo` has no object-graph child API — `addChild`
// takes a `View`/virtual-descendant-id pair, not another `AccessibilityNodeInfo`,
// because the platform is meant to *pull* children through a provider rather
// than have the app hand over a finished tree (see the file comment above).
// So this mirror does not attempt to reconstruct a hierarchy: it is a flat
// registry of whatever nodes get built and populated, each independently
// addressable. `accessibility.rs` exposes them as flat children of one
// "Cordial" application object. That is a real limitation, not a placeholder
// — recovering the actual parent/child structure needs to know how Roblox's
// engine actually calls this surface, which is exactly the unresolved
// question the file comment above describes.
//
// Keyed by an id assigned at `obtain()` time rather than by pointer, so a
// node's identity on the Rust side survives being passed back and forth
// across the JNI boundary as a `shared_ptr` (whose address is not stable) and
// stays a plain, `Copy`-friendly `u32` for the FFI surface below.
struct NodeState {
    unsigned id = 0;
    std::string class_name;
    std::string text;
    std::string content_description;
    int left = 0, top = 0, right = 0, bottom = 0;
    // Bit layout is this file's own, not Android's or AT-SPI's — translated
    // into AT-SPI `StateType` bits on the Rust side, where the real ordinals
    // (verified against a live AT-SPI provider; see accessibility.rs's own
    // comment) live.
    unsigned state = 0;
    // `AccessibilityAction` ids `addAction` recorded, the legacy
    // `AccessibilityNodeInfo.ACTION_*` integers (see `AccessibilityAction`,
    // below, for why those specific values were chosen).
    std::vector<int> actions;
};

enum NodeStateBit : unsigned {
    kCheckable = 1u << 0,
    kChecked = 1u << 1,
    kClickable = 1u << 2,
    kEnabled = 1u << 3,
    kFocusable = 1u << 4,
    kFocused = 1u << 5,
    kLongClickable = 1u << 6,
    kPassword = 1u << 7,
    kScrollable = 1u << 8,
    kSelected = 1u << 9,
    kVisibleToUser = 1u << 10,
};

namespace {
std::mutex g_registry_mutex;
std::unordered_map<unsigned, NodeState> g_registry;
std::atomic<unsigned> g_next_id{1};
/// Bumped whenever any node is added, changed or recycled, so
/// `accessibility.rs` can poll-and-diff cheaply instead of copying the whole
/// registry every tick — the same pattern `g_textbox_generation` and
/// `g_ime_state_generation` already use for the same reason.
std::atomic<unsigned> g_registry_generation{0};

NodeState& locked_node(unsigned id) {
    // Callers hold `g_registry_mutex`; `operator[]` default-constructs on
    // first touch, which is what every setter below relies on to work
    // regardless of call order.
    return g_registry[id];
}

/// A pending event for `AccessibilityManager.sendAccessibilityEvent`, kept
/// separate from the node registry: an event is a point-in-time announcement
/// (Android's `TYPE_*` constants; see `AccessibilityEvent` below), not
/// standing state, so it is drained once rather than diffed.
struct PendingEvent {
    int event_type = 0;
    std::string class_name;
    std::string text;
};
std::mutex g_event_mutex;
std::vector<PendingEvent> g_event_queue;
constexpr size_t kMaxQueuedEvents = 256; // bounded: a stuck consumer must not leak memory forever.
} // namespace

// ---------------------------------------------------------------- Rect
//
// `android.graphics.Rect`: four public fields, no behaviour Cordial needs.
// Both directions of `AccessibilityNodeInfo.{set,get}BoundsInScreen` have the
// *caller* (Roblox's native code) construct the `Rect` via JNI, so the
// constructor has to resolve, not just field access — see
// `NativeFlagsInitResult`'s doc comment in `init_params.cpp` for why an
// instance `<init>` needs the static-factory idiom under libjnivm, confirmed
// there against a live run, applied the same way here on inference from that
// same finding rather than a fresh observation.
class Rect : public Object {
public:
    jint left = 0, top = 0, right = 0, bottom = 0;

    static std::shared_ptr<Rect> ctor0(ENV* env, Class*) {
        auto p = std::make_shared<Rect>();
        to_jni(env, p);
        return p;
    }
    static std::shared_ptr<Rect> ctor4(ENV* env, Class*, jint l, jint t, jint r, jint b) {
        auto p = std::make_shared<Rect>();
        p->left = l;
        p->top = t;
        p->right = r;
        p->bottom = b;
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<Rect>("android/graphics/Rect");
        auto c = env->GetClass("android/graphics/Rect");
        c->Hook(env, "<init>", &Rect::ctor0);
        c->Hook(env, "<init>", &Rect::ctor4);
        c->HookInstance(env, "left", &Rect::left);
        c->HookInstance(env, "top", &Rect::top);
        c->HookInstance(env, "right", &Rect::right);
        c->HookInstance(env, "bottom", &Rect::bottom);
    }
};

// --------------------------------------------------------- CharSequence
//
// Every text-bearing setter on `AccessibilityNodeInfo`
// (`setText`/`setContentDescription`, and plausibly `setClassName`/
// `setPackageName` — the AOSP declaration for those two is less certain
// without the shipping dex to check against, see the file comment) takes
// `CharSequence`, not `String`, in the public SDK. libjnivm matches a hook by
// the descriptor derived from its C++ parameter type, so a hook typed
// `shared_ptr<String>` would derive `Ljava/lang/String;` and never match a
// call compiled against the `CharSequence`-typed overload — silently, the
// same failure shape `showKeyboard`'s `Array<jbyte>` needed originally. This
// class exists only to give those setters a parameter type that derives the
// right descriptor; the object actually handed across the JNI boundary at
// runtime is still a real `jnivm::String`, recovered inside each setter with
// `dynamic_pointer_cast` — valid because both derive from the same
// polymorphic `Object` root, even though neither derives from the other in
// this file's C++ hierarchy the way `String` genuinely implements
// `CharSequence` in Java's.
class CharSequence : public Object {
public:
    static void Register(ENV* env) { env->GetClass<CharSequence>("java/lang/CharSequence"); }
};

static std::string char_sequence_to_std_string(const std::shared_ptr<CharSequence>& cs) {
    if (auto s = std::dynamic_pointer_cast<String>(cs)) {
        return *s;
    }
    return std::string();
}

// ----------------------------------------------- AccessibilityAction
//
// `AccessibilityNodeInfo.AccessibilityAction`. The modern (API 21+) object
// form wraps an id and a label; the ids for the *standard* actions are the
// same bitmask integers the legacy `AccessibilityNodeInfo.ACTION_*` int
// constants have always used, kept for backward compatibility — this is a
// well-known, long-stable part of the public platform API, not read from
// this build's dex (no dex was available; see the file comment), so it is
// asserted here as public-API knowledge rather than as something observed
// from Roblox specifically.
class AccessibilityAction : public Object {
public:
    jint id = 0;
    std::shared_ptr<CharSequence> label;

    jint getId(ENV*) { return id; }
    std::shared_ptr<CharSequence> getLabel(ENV*) { return label; }

    static std::shared_ptr<AccessibilityAction> ctor(ENV* env, Class*, jint action_id,
                                                      std::shared_ptr<CharSequence> lbl) {
        auto p = std::make_shared<AccessibilityAction>();
        p->id = action_id;
        p->label = std::move(lbl);
        to_jni(env, p);
        return p;
    }

    static std::shared_ptr<AccessibilityAction> standard(ENV* env, jint action_id) {
        auto p = std::make_shared<AccessibilityAction>();
        p->id = action_id;
        to_jni(env, p);
        return p;
    }

    static void Register(ENV* env) {
        env->GetClass<AccessibilityAction>(
            "android/view/accessibility/AccessibilityNodeInfo$AccessibilityAction");
        auto c =
            env->GetClass("android/view/accessibility/AccessibilityNodeInfo$AccessibilityAction");
        c->Hook(env, "<init>", &AccessibilityAction::ctor);
        c->HookInstanceFunction(env, "getId", &AccessibilityAction::getId);
        c->HookInstanceFunction(env, "getLabel", &AccessibilityAction::getLabel);

        // The standard actions Roblox's UI is most likely to need for a
        // pointer/keyboard-navigable menu system: activate, focus, select and
        // the two scroll directions. Not exhaustive — extend as a live run
        // shows others resolving unresolved, per this file's own working
        // method (see android_classes.cpp's header comment).
#define STD_ACTION(field, value)                                                                 \
    c->HookGetterFunction(env, field, [](ENV* e, Class*) {                                       \
        static std::shared_ptr<AccessibilityAction> a = AccessibilityAction::standard(e, value);  \
        return a;                                                                                 \
    })
        STD_ACTION("ACTION_FOCUS", 0x00000001);
        STD_ACTION("ACTION_CLEAR_FOCUS", 0x00000002);
        STD_ACTION("ACTION_SELECT", 0x00000004);
        STD_ACTION("ACTION_CLEAR_SELECTION", 0x00000008);
        STD_ACTION("ACTION_CLICK", 0x00000010);
        STD_ACTION("ACTION_LONG_CLICK", 0x00000020);
        STD_ACTION("ACTION_ACCESSIBILITY_FOCUS", 0x00000040);
        STD_ACTION("ACTION_CLEAR_ACCESSIBILITY_FOCUS", 0x00000080);
        STD_ACTION("ACTION_SCROLL_FORWARD", 0x00001000);
        STD_ACTION("ACTION_SCROLL_BACKWARD", 0x00002000);
#undef STD_ACTION
    }
};

// ------------------------------------------------------ AccessibilityNodeInfo
class AccessibilityNodeInfo : public Object {
public:
    unsigned node_id = 0;

    static std::shared_ptr<AccessibilityNodeInfo> obtain(ENV* env, Class*) {
        auto p = std::make_shared<AccessibilityNodeInfo>();
        p->node_id = g_next_id.fetch_add(1, std::memory_order_relaxed);
        {
            std::lock_guard<std::mutex> lock(g_registry_mutex);
            locked_node(p->node_id).id = p->node_id;
        }
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
        to_jni(env, p);
        return p;
    }
    static std::shared_ptr<AccessibilityNodeInfo> obtain_copy(
        ENV* env, Class*, std::shared_ptr<AccessibilityNodeInfo> other) {
        auto p = std::make_shared<AccessibilityNodeInfo>();
        p->node_id = g_next_id.fetch_add(1, std::memory_order_relaxed);
        {
            std::lock_guard<std::mutex> lock(g_registry_mutex);
            NodeState copy;
            if (other) {
                auto it = g_registry.find(other->node_id);
                if (it != g_registry.end()) {
                    copy = it->second;
                }
            }
            copy.id = p->node_id;
            g_registry[p->node_id] = std::move(copy);
        }
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
        to_jni(env, p);
        return p;
    }

    void recycle(ENV*) {
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        g_registry.erase(node_id);
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }

    void setText(ENV*, std::shared_ptr<CharSequence> v) {
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        locked_node(node_id).text = char_sequence_to_std_string(v);
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }
    void setContentDescription(ENV*, std::shared_ptr<CharSequence> v) {
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        locked_node(node_id).content_description = char_sequence_to_std_string(v);
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }
    // Both overloads registered — see the file comment on `CharSequence` for
    // why the declared descriptor for `className`/`packageName` is uncertain
    // without the shipping dex to check.
    void setClassNameCs(ENV*, std::shared_ptr<CharSequence> v) {
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        locked_node(node_id).class_name = char_sequence_to_std_string(v);
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }
    void setClassNameStr(ENV*, std::shared_ptr<String> v) {
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        locked_node(node_id).class_name = v ? std::string(*v) : std::string();
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }
    // `packageName` is accepted and dropped: nothing on the AT-SPI side reads
    // it (there is no equivalent concept once the node is flattened into
    // Cordial's own application), but the call still has to resolve rather
    // than land on an unresolved-symbol stub.
    void setPackageNameCs(ENV*, std::shared_ptr<CharSequence>) {}
    void setPackageNameStr(ENV*, std::shared_ptr<String>) {}
    void setViewIdResourceName(ENV*, std::shared_ptr<String>) {}

    void setBoundsInScreen(ENV*, std::shared_ptr<Rect> r) {
        if (!r) return;
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        auto& n = locked_node(node_id);
        n.left = r->left;
        n.top = r->top;
        n.right = r->right;
        n.bottom = r->bottom;
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }
    void getBoundsInScreen(ENV*, std::shared_ptr<Rect> out) {
        if (!out) return;
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        auto& n = locked_node(node_id);
        out->left = n.left;
        out->top = n.top;
        out->right = n.right;
        out->bottom = n.bottom;
    }

#define BOOL_STATE_SETTER(name, bit)                                                             \
    void name(ENV*, jboolean v) {                                                                \
        std::lock_guard<std::mutex> lock(g_registry_mutex);                                      \
        auto& n = locked_node(node_id);                                                          \
        if (v) n.state |= (bit); else n.state &= ~(bit);                                          \
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);                           \
    }
    BOOL_STATE_SETTER(setCheckable, kCheckable)
    BOOL_STATE_SETTER(setChecked, kChecked)
    BOOL_STATE_SETTER(setClickable, kClickable)
    BOOL_STATE_SETTER(setEnabled, kEnabled)
    BOOL_STATE_SETTER(setFocusable, kFocusable)
    BOOL_STATE_SETTER(setFocused, kFocused)
    BOOL_STATE_SETTER(setLongClickable, kLongClickable)
    BOOL_STATE_SETTER(setPassword, kPassword)
    BOOL_STATE_SETTER(setScrollable, kScrollable)
    BOOL_STATE_SETTER(setSelected, kSelected)
    BOOL_STATE_SETTER(setVisibleToUser, kVisibleToUser)
#undef BOOL_STATE_SETTER

    // The legacy integer form. Kept alongside the `AccessibilityAction`
    // object form below because a compile-time constant like
    // `AccessibilityNodeInfo.ACTION_CLICK` is inlined by javac into the
    // caller — there is no field lookup for this file to intercept even in
    // principle, so the *only* way to observe which standard actions a node
    // carries is to accept the plain int here.
    void addActionInt(ENV*, jint action) {
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        locked_node(node_id).actions.push_back(action);
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }
    void addActionObj(ENV*, std::shared_ptr<AccessibilityAction> a) {
        if (!a) return;
        std::lock_guard<std::mutex> lock(g_registry_mutex);
        locked_node(node_id).actions.push_back(a->id);
        g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    }

    static void Register(ENV* env) {
        env->GetClass<AccessibilityNodeInfo>("android/view/accessibility/AccessibilityNodeInfo");
        auto c = env->GetClass("android/view/accessibility/AccessibilityNodeInfo");
        c->Hook(env, "obtain", &AccessibilityNodeInfo::obtain);
        c->Hook(env, "obtain", &AccessibilityNodeInfo::obtain_copy);
        c->HookInstanceFunction(env, "recycle", &AccessibilityNodeInfo::recycle);
        c->HookInstanceFunction(env, "setText", &AccessibilityNodeInfo::setText);
        c->HookInstanceFunction(env, "setContentDescription",
                                &AccessibilityNodeInfo::setContentDescription);
        c->HookInstanceFunction(env, "setClassName", &AccessibilityNodeInfo::setClassNameCs);
        c->HookInstanceFunction(env, "setClassName", &AccessibilityNodeInfo::setClassNameStr);
        c->HookInstanceFunction(env, "setPackageName", &AccessibilityNodeInfo::setPackageNameCs);
        c->HookInstanceFunction(env, "setPackageName", &AccessibilityNodeInfo::setPackageNameStr);
        c->HookInstanceFunction(env, "setViewIdResourceName",
                                &AccessibilityNodeInfo::setViewIdResourceName);
        c->HookInstanceFunction(env, "setBoundsInScreen",
                                &AccessibilityNodeInfo::setBoundsInScreen);
        c->HookInstanceFunction(env, "getBoundsInScreen",
                                &AccessibilityNodeInfo::getBoundsInScreen);
        c->HookInstanceFunction(env, "setCheckable", &AccessibilityNodeInfo::setCheckable);
        c->HookInstanceFunction(env, "setChecked", &AccessibilityNodeInfo::setChecked);
        c->HookInstanceFunction(env, "setClickable", &AccessibilityNodeInfo::setClickable);
        c->HookInstanceFunction(env, "setEnabled", &AccessibilityNodeInfo::setEnabled);
        c->HookInstanceFunction(env, "setFocusable", &AccessibilityNodeInfo::setFocusable);
        c->HookInstanceFunction(env, "setFocused", &AccessibilityNodeInfo::setFocused);
        c->HookInstanceFunction(env, "setLongClickable",
                                &AccessibilityNodeInfo::setLongClickable);
        c->HookInstanceFunction(env, "setPassword", &AccessibilityNodeInfo::setPassword);
        c->HookInstanceFunction(env, "setScrollable", &AccessibilityNodeInfo::setScrollable);
        c->HookInstanceFunction(env, "setSelected", &AccessibilityNodeInfo::setSelected);
        c->HookInstanceFunction(env, "setVisibleToUser",
                                &AccessibilityNodeInfo::setVisibleToUser);
        c->HookInstanceFunction(env, "addAction", &AccessibilityNodeInfo::addActionInt);
        c->HookInstanceFunction(env, "addAction", &AccessibilityNodeInfo::addActionObj);
    }
};

// -------------------------------------------------------- AccessibilityEvent
//
// Real `AccessibilityEvent` carries its source via `setSource(View, int)` —
// another View-shaped call this runtime cannot answer meaningfully (see the
// file comment). What is implemented is the part that does not need a View:
// the event's own type and text, which a caller can set directly on the
// event object before handing it to
// `AccessibilityManager.sendAccessibilityEvent`. That is enough to carry a
// live announcement (focus moved, a live region changed) even though it
// cannot carry a full node reference.
class AccessibilityEvent : public Object {
public:
    jint event_type = 0;
    std::string class_name;
    std::string text;

    static std::shared_ptr<AccessibilityEvent> obtain0(ENV* env, Class*) {
        auto p = std::make_shared<AccessibilityEvent>();
        to_jni(env, p);
        return p;
    }
    static std::shared_ptr<AccessibilityEvent> obtain1(ENV* env, Class*, jint type) {
        auto p = std::make_shared<AccessibilityEvent>();
        p->event_type = type;
        to_jni(env, p);
        return p;
    }
    void setEventType(ENV*, jint type) { event_type = type; }
    jint getEventType(ENV*) { return event_type; }
    void setClassName(ENV*, std::shared_ptr<CharSequence> v) {
        class_name = char_sequence_to_std_string(v);
    }
    void setContentDescription(ENV*, std::shared_ptr<CharSequence> v) {
        text = char_sequence_to_std_string(v);
    }

    static void Register(ENV* env) {
        env->GetClass<AccessibilityEvent>("android/view/accessibility/AccessibilityEvent");
        auto c = env->GetClass("android/view/accessibility/AccessibilityEvent");
        c->Hook(env, "obtain", &AccessibilityEvent::obtain0);
        c->Hook(env, "obtain", &AccessibilityEvent::obtain1);
        c->HookInstanceFunction(env, "setEventType", &AccessibilityEvent::setEventType);
        c->HookInstanceFunction(env, "getEventType", &AccessibilityEvent::getEventType);
        c->HookInstanceFunction(env, "setClassName", &AccessibilityEvent::setClassName);
        c->HookInstanceFunction(env, "setContentDescription",
                                &AccessibilityEvent::setContentDescription);
    }
};

// ----------------------------------------------------- AccessibilityManager
class AccessibilityManagerListener : public Object {
public:
    static void Register(ENV* env) {
        env->GetClass<AccessibilityManagerListener>(
            "android/view/accessibility/AccessibilityManager$TouchExplorationStateChangeListener");
    }
};

class AccessibilityManager : public Object {
public:
    static std::shared_ptr<AccessibilityManager> getInstance(ENV* env, Class*,
                                                              std::shared_ptr<Object>) {
        // One instance for the process, matching every other platform
        // singleton in this tree (`shared_activity`, `shared_input_connection`
        // in game_activity.cpp) — real `AccessibilityManager.getInstance`
        // returns the same object for a given `Context` too, and an engine
        // that compares identity across calls should see that here as well.
        static std::shared_ptr<AccessibilityManager> instance;
        if (!instance) {
            instance = std::make_shared<AccessibilityManager>();
            to_jni(env, instance);
        }
        return instance;
    }

    jboolean isEnabled(ENV*) { return accessibility_enabled(); }

    // Desktop mouse-and-keyboard input is not a touchscreen, so there is
    // nothing here for "explore by touch" to mean — see the task brief's own
    // note that this needed deciding explicitly, not left to default to
    // whatever `isEnabled()` returns.
    jboolean isTouchExplorationEnabled(ENV*) { return false; }

    // Accepted and stored nowhere: Cordial's own answer to both of these
    // never changes mid-session (the AT-SPI bridge either connected at
    // startup or it did not), so there is nothing to notify a listener about
    // later. Resolving the call is the point — see the same reasoning on
    // `onLuaTextBoxChangedCallback` in `android_classes.cpp`.
    void addTouchExplorationStateChangeListener(
        ENV*, std::shared_ptr<AccessibilityManagerListener>) {}
    void removeTouchExplorationStateChangeListener(
        ENV*, std::shared_ptr<AccessibilityManagerListener>) {}

    void sendAccessibilityEvent(ENV*, std::shared_ptr<AccessibilityEvent> ev) {
        if (!ev) return;
        std::lock_guard<std::mutex> lock(g_event_mutex);
        if (g_event_queue.size() >= kMaxQueuedEvents) {
            // A consumer that stopped draining is a bug worth being loud
            // about once, not a slow leak — drop the oldest rather than grow
            // without bound.
            g_event_queue.erase(g_event_queue.begin());
        }
        g_event_queue.push_back(PendingEvent{ev->event_type, ev->class_name, ev->text});
    }

    static void Register(ENV* env) {
        env->GetClass<AccessibilityManager>("android/view/accessibility/AccessibilityManager");
        auto c = env->GetClass("android/view/accessibility/AccessibilityManager");
        c->Hook(env, "getInstance", &AccessibilityManager::getInstance);
        c->HookInstanceFunction(env, "isEnabled", &AccessibilityManager::isEnabled);
        c->HookInstanceFunction(env, "isTouchExplorationEnabled",
                                &AccessibilityManager::isTouchExplorationEnabled);
        c->HookInstanceFunction(env, "addTouchExplorationStateChangeListener",
                                &AccessibilityManager::addTouchExplorationStateChangeListener);
        c->HookInstanceFunction(env, "removeTouchExplorationStateChangeListener",
                                &AccessibilityManager::removeTouchExplorationStateChangeListener);
        c->HookInstanceFunction(env, "sendAccessibilityEvent",
                                &AccessibilityManager::sendAccessibilityEvent);
    }
};

void register_accessibility_classes(ENV* env) {
    Rect::Register(env);
    CharSequence::Register(env);
    AccessibilityAction::Register(env);
    AccessibilityNodeInfo::Register(env);
    AccessibilityEvent::Register(env);
    AccessibilityManagerListener::Register(env);
    AccessibilityManager::Register(env);
}

} // namespace cordial

// -------------------------------------------------------------- Rust FFI
//
// Bounded, buffer-copy style throughout, matching `cordial_textbox_text`'s
// convention elsewhere in this directory: fixed-size C structs and
// length-prefixed string copies rather than anything that hands ownership of
// a C++ container across the boundary.

extern "C" {

struct CordialA11yNode {
    unsigned id;
    char class_name[128];
    char text[256];
    char content_description[256];
    int left, top, right, bottom;
    unsigned state;
    // Fixed-size rather than a second indirection: 16 standard actions is
    // generous for anything a menu/button/list-item node would carry, and a
    // fixed array keeps this struct's layout `#[repr(C)]`-simple on the Rust
    // side. Overflow is silently truncated, not an error — losing a rarely
    // used action off a node is a far smaller problem than a partially
    // written struct.
    int actions[16];
    unsigned action_count;
};

static void fill_node(const cordial::NodeState& n, CordialA11yNode* out) {
    out->id = n.id;
    std::snprintf(out->class_name, sizeof(out->class_name), "%s", n.class_name.c_str());
    std::snprintf(out->text, sizeof(out->text), "%s", n.text.c_str());
    std::snprintf(out->content_description, sizeof(out->content_description), "%s",
                  n.content_description.c_str());
    out->left = n.left;
    out->top = n.top;
    out->right = n.right;
    out->bottom = n.bottom;
    out->state = n.state;
    size_t k = std::min(n.actions.size(), sizeof(out->actions) / sizeof(out->actions[0]));
    for (size_t i = 0; i < k; ++i) {
        out->actions[i] = n.actions[i];
    }
    out->action_count = static_cast<unsigned>(k);
}

/// Copy up to `max` live nodes into `out`. Returns the number written — not
/// the total live count, which callers get from
/// `cordial_accessibility_node_count` if they need to size the buffer first.
size_t cordial_accessibility_snapshot(CordialA11yNode* out, size_t max) {
    if (!out || max == 0) return 0;
    std::lock_guard<std::mutex> lock(cordial::g_registry_mutex);
    size_t n = 0;
    for (const auto& kv : cordial::g_registry) {
        if (n >= max) break;
        fill_node(kv.second, &out[n]);
        ++n;
    }
    return n;
}

size_t cordial_accessibility_node_count() {
    std::lock_guard<std::mutex> lock(cordial::g_registry_mutex);
    return cordial::g_registry.size();
}

unsigned cordial_accessibility_generation() {
    return cordial::g_registry_generation.load(std::memory_order_acquire);
}

/// Dequeue one pending `sendAccessibilityEvent` call. Returns 1 with
/// `*event_type` set and the buffers filled if one was pending, 0 if the
/// queue was empty — callers should drain in a loop until this returns 0
/// rather than assume one call means one event, since the engine can call
/// `sendAccessibilityEvent` faster than a poll loop drains it.
/// Inject one node directly into the registry, bypassing JNI entirely.
///
/// Test-only plumbing, not a second way for Roblox to reach this file: the
/// AT-SPI bridge on the Rust side has no way to tell a node populated this
/// way from one a live engine populated through
/// `AccessibilityNodeInfo`'s real hooks above, so this exists purely so the
/// bridge itself — the AT-SPI-facing half — can be exercised and observed
/// with `busctl`/`accerciser` without a Roblox APK, which this change was
/// written without access to (see the file's header comment and the
/// accompanying report). Returns the assigned node id.
unsigned cordial_accessibility_test_seed_node(const char* class_name, const char* text,
                                              const char* content_description, int left, int top,
                                              int right, int bottom, unsigned state) {
    unsigned id = cordial::g_next_id.fetch_add(1, std::memory_order_relaxed);
    {
        std::lock_guard<std::mutex> lock(cordial::g_registry_mutex);
        auto& n = cordial::locked_node(id);
        n.id = id;
        n.class_name = class_name ? class_name : "";
        n.text = text ? text : "";
        n.content_description = content_description ? content_description : "";
        n.left = left;
        n.top = top;
        n.right = right;
        n.bottom = bottom;
        n.state = state;
    }
    cordial::g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
    return id;
}

/// Drop every node, seeded or real. Test-only, same reasoning as
/// `cordial_accessibility_test_seed_node`.
void cordial_accessibility_test_clear() {
    std::lock_guard<std::mutex> lock(cordial::g_registry_mutex);
    cordial::g_registry.clear();
    cordial::g_registry_generation.fetch_add(1, std::memory_order_acq_rel);
}

int cordial_accessibility_next_event(int* event_type, char* class_name_buf, int cn_len,
                                     char* text_buf, int text_len) {
    std::lock_guard<std::mutex> lock(cordial::g_event_mutex);
    if (cordial::g_event_queue.empty()) return 0;
    cordial::PendingEvent ev = cordial::g_event_queue.front();
    cordial::g_event_queue.erase(cordial::g_event_queue.begin());
    if (event_type) *event_type = ev.event_type;
    if (class_name_buf && cn_len > 0) {
        std::snprintf(class_name_buf, static_cast<size_t>(cn_len), "%s", ev.class_name.c_str());
    }
    if (text_buf && text_len > 0) {
        std::snprintf(text_buf, static_cast<size_t>(text_len), "%s", ev.text.c_str());
    }
    return 1;
}

} // extern "C"
