# The app bridge: what starts Roblox's Lua app shell

**Status:** investigation only, nothing modified. Written against Roblox for Android
2.732.1043 (`com.roblox.client`, versionCode 2814) — same build as
[`findings.md`](../findings.md) and [`framework-api-inventory.md`](framework-api-inventory.md).
Sources: `docs/analysis/jni-natives.tsv`, `tools/dex_method.py` against the shipping
dex files, `readelf`/`objdump` against `libroblox.so`, and the APK's
`AndroidManifest.xml` (parsed directly — see §6). `decompiled/` was not read, per
the task's instructions.

**Bottom line up front:** the call that flips the engine from idle to rendering the
Lua-based home shell is almost certainly
`Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeStartLuaAppDM`, a
no-argument native, reached after
`Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2InitWithParams` has
already been called once. Both are called from Java, in that order, by
`ActivityNativeMain.N2()` — and **`ActivityNativeMain`, not the AGDK
`MainGameActivity` Cordial has been driving, is the Activity the shipping app
actually launches into** (§6). That is the single most load-bearing finding in this
document and it changes what Phase 2 should target next.

---

## 1. Every JNI native touching the message bus / app bridge / call protocol

Full descriptors, cross-checked between `docs/analysis/jni-natives.tsv` (symbol
names) and `tools/dex_method.py` (proto). All exported from `libroblox.so`.

### 1.1 `com.roblox.engine.jni.NativeGLInterface` — the "V2" app/game bridge

This is the live API surface. It splits cleanly into two halves — **App** (the
Lua-driven home shell / universal app) and **Game** (a joined experience/place) —
which is the same App/Game split the rest of this document keeps finding at every
layer.

| Native | Signature | Address | Size |
|---|---|---|---|
| `nativeAppBridgeV2InitWithParams` | `(Lcom/roblox/engine/jni/autovalue/InitParams;)V` | `0x2185282` | 2337 |
| `nativeAppBridgeStartLuaAppDM` | `()V` | `0x21d201e` | 249 |
| `nativeAppBridgeV2StartAppWithParams` | `(Lcom/roblox/engine/jni/autovalue/StartAppParams;)V` | `0x228d249` | 491 |
| `nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams` | `(Landroid/view/Surface;Lcom/roblox/engine/jni/model/PlatformParams;)V` | — | — |
| `nativeAppBridgeV2SendAppEventOnAppReady` | `(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V` | `0x299c4ec` | 1421 |
| `nativeAppBridgeV2SendAppEventOnGameLoaded` | `(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V` | `0x299c055` | 1103 |
| `nativeAppBridgeV2PauseApp` | `()V` | — | — |
| `nativeAppBridgeV2DestroyApp` | `()V` | — | — |
| `nativeAppBridgeV2UserDidLogout` | `()V` | — | — |
| `nativeAppBridgeV2OnLowMemory` / `...ForRenderView` | `()V` / `(II)V` | — | — |
| `nativeAppBridgeV2StartGameWithParam` | `(Lcom/roblox/engine/jni/autovalue/StartGameParams;)I` | `0x299cac2` | 5244 |
| `nativeAppBridgeV2ResumeGameWithPlatformParams` | `(Landroid/view/Surface;Lcom/roblox/engine/jni/model/PlatformParams;Landroid/app/Activity;)V` | — | — |
| `nativeAppBridgeV2UpdateSurfaceGameWithPlatformParams` | same shape | — | — |
| `nativeAppBridgeV2PauseGame` / `nativeAppBridgeV2LeaveGame` | `()V` | — | — |
| `nativeBroadcastConnection` | `(ILjava/lang/String;Ljava/lang/String;)V` | `0x2999f9d` | 296 |
| `nativeBroadcastEventWithNamespace` | `(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V` | — | — |
| `nativeGameGlobalInit` | `()V` | `0x20da9ec` | 170 |
| `isColdStartDeeplinkToGame` | `()Z` | `0x21d2013` | 11 |

`isColdStartDeeplinkToGame` is an 11-byte trivial tail-call to an internal getter
(§4.4) — a flag query, not decision logic. The decision using it lives on the Java
side.

### 1.2 `com.roblox.engine.jni.NativeAppBridgeInterface` — an older, still-live "V1" path

| Native | Signature | Address | Size |
|---|---|---|---|
| `nativeAppBridgeAppStart` | `(Ljava/lang/String;Ljava/lang/String;ZLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)V` | `0x2164090` | 1674 |
| `setIsFirstInstall` | `(Z)V` | — | — |

The dex mangles this as
`nativeAppBridgeAppStart__Ljava_lang_String_2Ljava_lang_String_2ZLjava_lang_String_2Ljava_lang_String_2Ljava_lang_String_2`
— JNI's overload-disambiguation suffix, present because the un-suffixed short form
would otherwise collide (there is no overload actually present in this dex, but
Roblox's JNI header generator emits the long form unconditionally for methods with
primitive+object mixed argument lists in some versions).

This class is called from a *different* manager (`yk/l0`, `zk/a$a`/`zk/a$b`, §5.2)
than the V2 API, alongside `setIsFirstInstall`. It looks like the predecessor to
`NativeGLInterface`'s app-start path, kept alive for a first-install code path or
a slower rollout. Nothing in this investigation determined which of the two
(`nativeAppBridgeAppStart` vs `nativeAppBridgeV2StartAppWithParams`) actually wins
in the shipping app — see §7 open questions.

### 1.3 `com.roblox.universalapp.messagebus.MessageBus` / `Connection`

Generic pub/sub JSON-RPC bus. Every native here operates on raw strings
(JSON-encoded) or a `Connection` handle (a native `shared_ptr` wrapped in a jlong).

| Native | Signature |
|---|---|
| `subscribe` (addr `0x298b726`, 1546 bytes) | `(Lcom/roblox/universalapp/messagebus/RawSubscriptionContract;)Lcom/roblox/universalapp/messagebus/Connection;` |
| `doSubscribeRaw` | `(Ljava/lang/String;Lcom/roblox/universalapp/messagebus/RawCallback;Z)Lcom/roblox/universalapp/messagebus/Connection;` |
| `doSubscribeProtocolMethodRequestRaw` / `...ResponseRaw` | `(Ljava/lang/String;Ljava/lang/String;Lcom/roblox/universalapp/messagebus/RawCallback;Z)Lcom/roblox/universalapp/messagebus/Connection;` |
| `publishRaw` | `(Ljava/lang/String;Ljava/lang/String;)V` |
| `publishProtocolMethodRequestRaw` / `...ResponseRaw` | string-heavy variants |
| `makeRequest` / `makeRequestRaw` | request/response RPC |
| `setRequestHandler` / `...Async` / `...Raw` / `...AsyncRaw` | `(L...Contract;)V` |
| `clearRequestHandler` | `(Ljava/lang/String;Ljava/lang/String;)V` |
| `getLastRaw` / `getMessageId` | queries |
| `callResponseHandlerRaw` | `(Ljava/lang/String;Ljava/lang/String;)V` |
| `reportProtocolMethodRequestTelemetryData` / `...ResponseTelemetryData` | telemetry |
| `Connection.isConnected` (`0x298dbdf`, 54B) | `(J)Z` |
| `Connection.deleteSharedPtr` (`0x298da72`, 365B) | `(J)V` |

**Finding (§5.3): MessageBus is plumbing, not ignition.** Every caller found
(`WebViewProtocol`, `MediaPickerProtocol`, `RecentlyPlayedWidgetHandler`, `cn/f`,
`bn/b`) is a *feature* that talks to an already-running Lua app over this channel.
Nothing that starts the app shell calls into `MessageBus`. Treat it as the RPC pipe
the shell uses once alive, not the switch that turns it on.

### 1.4 `com.roblox.engine.jni.memstorage.Connection` / `MemStorage` — a second, unrelated "Connection"

Easy to conflate with 1.3 because of the shared class name, but this is a
different, smaller subsystem: a native key/value store (`MemStorage.bind` /
`.fire` / `.getItem` / `.setItem` / `.hasItem` / `.removeItem`) with its own
`Connection` handle type (`disconnect()`, `releaseConnection`). Used for
lightweight cross-component signaling — e.g. `com/roblox/client/a.X1()` (the
shared base class of both `ActivitySplash` and `ActivityNativeMain`, §6) fires
`MemStorage` events that other components (`hj/a`, `vh/c`, `bl/h`) bind to. Not
part of the app-bridge start sequence either.

### 1.5 `com.roblox.universalapp.call.JNICallProtocol`

All but one native are string-constant getters (`getCallIdKey`, `getPlaceIdKey`,
`getCalleeUserIdKey`, …) — JSON field-name accessors for Roblox's voice/video
calling feature, unrelated to app startup. The one active native,
`receiveCall(Ljava/lang/String;)V` (`0x2988039`, 194 bytes), delivers an incoming
call payload. Not part of the rendering-start path.

### 1.6 `com.roblox.universalapp.logging.JNILoggingProtocol`

`nativeGetTimestamp()J`, `nativeLogEvent(Ljava/lang/String;J[Ljava/lang/Object;)V`
(`0x20b623c`, 277 bytes), `nativeLogRobloxTelemetryEvent(Lcom/roblox/engine/jni/autovalue/RobloxTelemetryEvent;)V`.
Telemetry sink. The internal (non-exported) C++ function at `0x20c0ea5` that these
wrap is called *from inside* several app-bridge natives (§4.1) to log bridge
lifecycle events — logging is a side effect of the bridge, not a cause of
anything.

---

## 2. The Java-side surface, from the dex

Everything below came from `tools/dex_method.py` against
`apk/dex/{classes,classes2,classes3}.dex`. Cross-references (who calls what) came
from a purpose-built read-only bytecode scanner (§8) — the dex reader only lists
declared methods, it doesn't walk bytecode, so an xref pass was written for this
investigation on top of the same dex parsing approach.

### 2.1 Two Activities, two bridge idioms

| Class | Extends | Notes |
|---|---|---|
| `com.roblox.client.startup.MainGameActivity` | `com.google.androidgamesdk.GameActivity` (AGDK) | The class Cordial has been driving (`initializeNativeCode`, `nativeSetAssetPath`, `nativeAppBridgeSetInitParams`, `nativePreloadFlagOverrides`, `nativeRetryInit` — all on this class, per `findings.md` §8). `AndroidManifest.xml`: `exported="false"`, carries `<meta-data android:name="android.app.lib_name" value="roblox">` (the AGDK convention telling `GameActivity`'s base class which `.so` to load). |
| `com.roblox.client.ActivityNativeMain` | `com.roblox.client.a` (not AGDK) | Implements `com.roblox.engine.jni.NativeGLJavaInterface$OnAppShellReloadNeededListener` directly. **Owns the App Bridge lifecycle** (§4, §5). Not exported, no intent-filter — reached only by explicit `Intent`. |

Both implement a shared interface `vi/p` (single abstract method `E()V`). This is
**not**, on checking, implemented or used by the App Bridge manager classes
themselves — `vi/e` implements only `Lul/h$d;` and `vi/h0`/`vi/a` implement
`SurfaceHolder.Callback`-family interfaces, no `vi/p` among them (verified via
each class's `class_def_item` interfaces list). The one confirmed caller of
`vi/p.E()` is `vi/o0.j(Activity)` — a `ViewTreeObserver`-adjacent helper in the
same package as `vi/o` (§5.1's `DeviceParams`/`PlatformParams` builder), which
looks like a layout/orientation-change notifier back to whichever Activity hosts
it, not an app-bridge callback. Both Activities implementing `vi/p` is confirmed;
what it's *for* is not established beyond that one call site.

### 2.2 The autovalue param objects (Java builders that become the native structs)

```
InitParams        { baseURL, buildVariant, deviceParams, isPotato, isTablet,
                     isVrDevice, platformParams, userAgent, vrContext }
StartAppParams     { appStarterPlace, appStarterScript, appUserId, isUnder13,
                     membershipType, platformParams, selectedTheme, surface,
                     username, vrContext }
StartGameParams    { accessCode, callId, conversationId, deviceParams, eventId,
                     gameId, gameIdToExclude, gameJoinContext, isUnder13,
                     isoContext, joinAttemptId, joinAttemptOrigin,
                     joinRequestType, launchData, linkCode, placeId,
                     platformParams, referralPage, referredByPlayerId,
                     reservedServerAccessCode, surface, userId, username,
                     vrContext }
```

`StartAppParams` carrying its own `Surface` field (distinct from whatever surface
`GameActivity`'s `onSurfaceCreatedNative` would hand over) is worth flagging: the
Lua app shell's render surface is plumbed through this params object, not through
the AGDK four-call window lifecycle in `path-to-a-frame.md` §1. That lifecycle is
real and is what `MainGameActivity`/AGDK expects, but the App Bridge (§4/§5) has
its own, separate surface-plumbing convention built on plain `SurfaceHolder`.

### 2.3 `NativeGLJavaInterface` — the reverse direction (native calling into Java)

Not in `jni-natives.tsv` because it isn't `Java_*` exports — it's a plain Java
class whose methods the *native* side calls back into via cached `jmethodID`s, the
other half of the bridge:

```
getDeviceStaticParams() / setDeviceStaticParams(DeviceStaticParams)
onAppShellReloadNeeded()                  <- the engine telling Java to reload the shell
onAppBridgeNotification(String,String)
onDataModelNotificationCallback(String,String)
gameLoadedCallback(long)
gameDidLeave() / exitGameWithError(int)
getWebViewUserAgent() / getMobileAdvertisingId()
```

`onAppShellReloadNeeded` is a strong independent confirmation that "app shell" is
Roblox's own term for this Lua UI layer, matching `OnAppShellReloadNeededListener`
that `ActivityNativeMain` implements (§2.1).

---

## 3. The manifest: which Activity actually launches

`base.apk`'s `AndroidManifest.xml` is binary AXML; there is no `aapt`/`apktool`
here, so it was parsed directly (a ~150-line read-only parser, §8). Relevant
excerpt:

```xml
<activity android:name="com.roblox.client.startup.ActivitySplash"
          android:exported="true" android:launchMode="singleTop">
  <intent-filter>
    <category android:name="android.intent.category.LAUNCHER">
    <action android:name="android.intent.action.MAIN">
    <category android:name="android.intent.category.DEFAULT">

<activity android:name="com.roblox.client.ActivityNativeMain"
          android:launchMode="singleTask" .../>          <!-- no exported, no intent-filter -->

<activity android:name="com.roblox.client.startup.MainGameActivity"
          android:exported="false" android:launchMode="singleTask" ...>
  <meta-data android:name="android.app.lib_name" android:value="roblox">
```

`ActivitySplash` is the sole `MAIN`/`LAUNCHER` activity. Its `onCreate` flow
(`ActivitySplash.G()` → `w2(Z)`) builds a launch `Intent` and calls
`startActivity()` + `finish()`. The intent target comes from `rh/v.i(Context)`,
whose bytecode is:

```
invoke vk/e.a(Context) Intent          ; tried first — some override/experiment path
new-instance Intent
const-class  com.roblox.client.ActivityNativeMain
invoke Intent.<init>(Context, Class)
```

**The hardcoded, hard-linked target is `ActivityNativeMain`.** `vk/e.a` may be an
experiment/feature-flag override (its own body was not resolved in this pass — its
`class_data_item` wasn't found in the same dex as its call site, meaning its
definition lives in a different one of the three dex files than the ones searched
in that pass; not chased further, see §7), but the class literal actually compiled
into the `Intent` constructor call is `ActivityNativeMain`, not `MainGameActivity`.

**This means the shipping app's default path to the home shell is
`ActivitySplash → ActivityNativeMain`, not `MainGameActivity`.** `MainGameActivity`
exists, is `exported=false`, carries the AGDK `lib_name` meta-data, and everything
`findings.md` §8 describes about Cordial reaching `initializeNativeCode` on it is
real and correct — but it may be a secondary/experimental path in the real app
rather than the one that actually renders the Lua home shell today. This should be
checked against runtime evidence (e.g., instrumenting which Activity a real device
launches) before Cordial commits further engineering to the `MainGameActivity`
surface path.

---

## 4. Disassembly notes

All addresses are file offsets == virtual addresses (this `.so` loads at a fixed
internal layout typical of Android NDK builds; ASLR slides the whole mapping
uniformly, so these numbers are stable relative to each other and to symbol
lookups via `readelf -sW`). Two program `LOAD` segments matter for translating
addresses to file offsets when dumping raw bytes: `[0, 0x6992cc0)` is a straight
identity map, `[0x6996cc0, 0x6996cc0+0x4b87b0)` is offset by `-0x4000` from its
file position (`0x6992cc0` vs `0x6996cc0`).

### 4.1 Shared shape across every app-bridge native

`nativeAppBridgeV2StartAppWithParams`, `nativeAppBridgeStartLuaAppDM`, and
`nativeGameGlobalInit` all share an identical skeleton:

1. Save the stack-protector canary (`mov r14, [rip+#stack_chk_guard]`).
2. A verbosity-gated internal log/assert call (checks a level byte against `0x6`
   and a bitmask against `0xfc00` — reads like a `RBX_LOG`/glog-style severity
   gate — then conditionally calls an internal logger with a format string
   address, arg count, and level; objdump labels the target only by nearest
   preceding export, e.g. `nativeInitCrashpad+0x2a48`, which is **not** the real
   symbol — it's the nearest stripped-name anchor, a recurring source of noise
   when reading this binary).
3. The actual body (see below, varies per function).
4. Stack-protector check, then epilogue.
5. A cold landing-pad block after the `ret`, laid out by the compiler for C++
   exception unwinding (destructor calls for locals like the `std::string`-like
   objects built on the stack) — this is **not** part of normal control flow, and
   objdump prints it contiguously because it has no way to mark it as a
   cold/exceptional path. It is easy to mistake this for "the function calls X at
   the end" when X is actually a per-local destructor invoked only during
   unwinding.

`nativeAppBridgeV2StartAppWithParams` (`0x228d249`) additionally, in its real
body:

- Calls through the **`JNIEnv` function-table vtable** at offset `0xf8` (bytes) —
  index 31 in the 232-entry `JNINativeInterface` table — with the third JNI
  argument (the `StartAppParams` jobject) as the sole argument. Consistent with
  `env->GetObjectClass(startAppParams)` or a similar single-object JNI call used
  to introspect the params object before dispatch.
- Calls the internal telemetry logger (`0x20c0ea5`, the function
  `JNILoggingProtocol.nativeLogRobloxTelemetryEvent` ultimately wraps) **twice**,
  each with a different embedded string-literal argument — almost certainly two
  named telemetry events bracketing "start app" (something like
  "AppBridgeStartApp begin/end"; the literal bytes could not be read directly
  because their storage cells are zero in the file and filled by relocation at
  load time — see caveat below).
- Calls through the `JNIEnv` vtable again at offset `0x710` (index ≈226) — deep
  enough in the table that it's very likely a `CallObjectMethod`/`CallStaticObjectMethod`
  family entry, i.e. a callback **into Java** to pull a field or invoke a getter
  on the `StartAppParams` object (matching the AutoValue getter methods in §2.2).
- Builds a small (0x30-byte) heap-allocated struct from the results and passes it,
  with a stack-built string-like object, to a common internal function at
  `0x2996048` — **the same function every one of these entry points calls** with a
  similar `{ptr, 0}` calling convention. This function is the best remaining
  candidate for "the actual dispatch/queue-push that hands work to the Lua VM or
  its message loop," but its own body was not disassembled in this pass (see §7).

`nativeAppBridgeStartLuaAppDM` (`0x21d201e`) is much smaller (249 bytes: push/mov
rbp, one string literal built on the stack, one call to the same `0x2996048`
dispatcher, epilogue). Structurally it looks like a fire-and-forget "post a
zero-argument named message" call — consistent with it being a pure trigger with
no parameters of its own, which matches its Java signature (`()V`).

`nativeGameGlobalInit` (`0x20da9ec`, 170 bytes) is the smallest of the group: one
call to a *different* internal function (`0x20dae60`) with a constant string and
`edx=0`, then straight to epilogue. Whatever `0x20dae60` does, it's a single call
with a literal name — plausibly a "global systems init, once" latch (the name
suggests process-wide engine globals, distinct from the app/game-specific bridge
calls).

**Caveat on the string literals.** Several `lea rax, [rip+X]` instructions in
these functions compute an address whose file contents are all-zero bytes. That
is expected for a PIE `.so`: these cells sit in `.data.rel.ro`/`.bss`-adjacent
storage patched by `R_X86_64_RELATIVE` relocations at load time (or are
`__cxa_guard`-protected function-local statics initialized once at runtime), not
raw string bytes sitting directly in `.rodata`. Reading them requires either
applying relocations (not done here) or loading the library and reading live
memory (not done here — no execution was performed on `libroblox.so` beyond
`readelf`/`objdump`, per the task's disassembly-only scope). Take the "these are
telemetry event names" and "this looks like a message-post" claims above as
structural inference from calling convention and call-site shape, not confirmed
string contents.

`nativeAppBridgeV2InitWithParams` (`0x2185282`, 2337 bytes — the largest of the
group examined) was not fully disassembled; its size alone (an order of magnitude
bigger than `StartLuaAppDM`) is consistent with it doing real engine bring-up
(constructing whatever global app-bridge singleton state the later `Start*` calls
assume exists) rather than just posting a message.

### 4.2 `MainGameActivity.nativeAppBridgeSetInitParams` is a distinct, separate native

Disassembling its tail (`0x29bbcb2`–`0x29bc4a0`) shows it is *not* a thin wrapper
around `NativeGLInterface.nativeAppBridgeV2InitWithParams` — the tail is entirely
C++ exception-cleanup landing pads (destructor calls for several distinct local
objects), and none of the "calls" in that region resolve to the V2 init function
by anything more than nearest-symbol coincidence (objdump's `+0x3586a`-style
offsets from an unrelated export are the giveaway that the true target is an
unnamed stripped internal symbol, not the named export). Given `MainGameActivity`
and `ActivityNativeMain` are two independent Activity implementations (§2.1, §6),
the simplest reading is that each has its **own**, separately-compiled init
native, both of which presumably reach a shared engine-global init deeper in the
call graph — but that shared point was not located in this pass.

---

## 5. The call graph: what actually invokes the app-bridge natives

Found with a purpose-built dex bytecode xref scanner (§8) — the dex reader that
ships in `tools/` only enumerates declared methods, so this pass wrote a small,
separate, read-only tool that walks `class_data_item`/`code_item` structures and
extracts the method index operand out of every `invoke-*` instruction. Results
below were spot-checked against the addresses in §1/§4 and are consistent.

### 5.1 The App Bridge manager: `vi/e` (obfuscated name — "AppBridge" by role)

A singleton (`vi/e.i()` returns the instance) with, among others:

```
vi/e.j(vi/e$d)                       -> NativeGLInterface.nativeAppBridgeV2InitWithParams(InitParams)
vi/e.F(Surface)                      -> NativeGLInterface.nativeAppBridgeV2StartAppWithParams(StartAppParams)
vi/e.H(Surface,float,int,int)        -> NativeGLInterface.nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams(...)
vi/e.t(...)                          -> NativeGLInterface.nativeAppBridgeV2SendAppEventOnAppReady(...)
vi/e.z(...)                          -> NativeGLInterface.nativeAppBridgeV2SendAppEventOnGameLoaded(...)
vi/e.f()                             -> NativeGLInterface.nativeAppBridgeV2DestroyApp()
vi/e.G()                             -> NativeGLInterface.nativeAppBridgeV2PauseApp() (+ setTaskSchedulerBackgroundMode)
vi/e.E(Context)                      -> NativeGLInterface.nativeGameGlobalInit() + nativeUpdateAdapterInit()
```

`vi/e$d` (an inner builder-ish class with `a()` → `DeviceParams`, `b()` →
`PlatformParams`) is what gets turned into an `InitParams` before `vi/e.j` calls
the native. It's constructed by `vi/o.a(Context)` / `vi/o.b(Context, Activity)` —
`vi/o` reads screen/device metrics (the same territory `path-to-a-frame.md` §2
flags as needed for `Configuration`/`DeviceStaticParams`).

`vi/e.F(Surface)` only takes a `Surface` — it builds the rest of `StartAppParams`
from state already held by `vi/e` (username, theme, etc. presumably set earlier
via other setters not enumerated here) and calls
`nativeAppBridgeV2StartAppWithParams` internally.

A second manager, `vi/h0` (implements `SurfaceHolder.Callback`), owns the **Game**
half symmetrically: `vi/h0.C` → `nativeAppBridgeV2StartGameWithParam`, `.F` →
`PauseGame`, `.G` → `ResumeGameWithPlatformParams`/`UpdateSurfaceGameWithPlatformParams`,
`.t` → `LeaveGame`. `vi/h0` is constructed by `com.roblox.client.game.ExperienceSession`
— i.e. joining an actual game/place is a separate object lifecycle from the app
shell, consistent with the App/Game split in §1.1.

Neither `vi/e` nor `vi/h0` is called *directly* by class name from
`MainGameActivity` anywhere this pass found. That absence is not conclusive on its
own — an `invoke-interface` call through some interface both `vi/e` and a
`MainGameActivity`-owned object implement would not resolve back to `vi/e` by
class name in this bytecode scan, which only tracks the statically-declared
callee type in each `invoke-*` instruction, not runtime types — but no such
shared interface between `vi/e`/`vi/h0` and anything reachable from
`MainGameActivity` was found either (§2.1's correction above rules out `vi/p` as
that link). What *is* directly and unambiguously found calling `vi/e` methods by
exact class name is `ActivityNativeMain` and its helper classes (`vi/a`, `qi/a`,
`yk/l0`) — see §5.2.

### 5.2 The concrete call chain: `ActivitySplash` → `ActivityNativeMain.N2()`

`ActivityNativeMain.N2(FeatureState)`, called from `.h3()`, called from `.G()`
(an Activity lifecycle hook), executes — reading the raw invoke-instruction stream
in order, which for this method (81 code units, apparently straight-line: no
branch was found jumping over any of these calls in this pass) is:

```
1.  el/p.h(...)                                       -- log
2.  vi/e.i()                     -- get App Bridge singleton
3.  vi/e.E(Context)              -- NativeGLInterface.nativeGameGlobalInit() + nativeUpdateAdapterInit()
4.  vi/o.a(Context)              -- build vi/e$d (DeviceParams + PlatformParams)
5.  vi/e.i()
6.  vi/e.j(vi/e$d)               -- NativeGLInterface.nativeAppBridgeV2InitWithParams(InitParams)   <== APP BRIDGE INIT
7.  NativeGLInterface.isColdStartDeeplinkToGame()   -- query the flag
8.  tj/j.h() / xm/a.a() / ... / yk/l0.F(...)         -- feature-state / analytics bookkeeping
9.  rh/v.h().f().i()                                 -- game-session-related query
10. ActivityNativeMain.j3(FeatureState)
11. NativeGLInterface.nativeAppBridgeStartLuaAppDM()  <== START THE LUA APP SHELL, last call in the method
```

Step 11 is the **last invoke instruction in the method**. Steps 7–10 do not
visibly branch around step 11 in this straight-line read, but this pass did not
decode the `if`/`goto` opcodes themselves (only `invoke-*`), so a conditional path
elsewhere in the method that skips step 11 when `isColdStartDeeplinkToGame()` is
true cannot be ruled out from this evidence alone — see §7. What is certain: this
is the only place in the entire dex set where `nativeAppBridgeStartLuaAppDM` is
called, and it is called in the same method, after the same init call, that this
document's other evidence (App/Game split, `StartAppParams` vs `StartGameParams`,
`isColdStartDeeplinkToGame`'s name) all points at: *if this cold start is not a
deep link straight into a game, start the Lua home shell.*

`vi/a` (constructed by `qi/a.g()`, itself constructed and held by
`ActivityNativeMain`, per `ActivityNativeMain$l.a` → `vi/e.i()`/`vi/e.x(...)`/`vi/e.w(...)`
and `ActivityNativeMain.v2()` → `Lqi/a;`) is the `SurfaceHolder`-adjacent
controller that later calls `vi/e.F(Surface)` (§5.1) once a real `Surface` exists
(`vi/a.K2`, `.N0`) and `vi/e.H(Surface,...)` on `surfaceChanged` — i.e. the actual
`StartAppParams`-with-a-real-`Surface` call happens slightly after `N2()`'s
init+start-shell sequence, once the `SurfaceView` backing `ActivityNativeMain` is
ready. This matches Android's normal ordering (surface arrives after `onCreate`).

The legacy V1 path (`NativeAppBridgeInterface.nativeAppBridgeAppStart` +
`setIsFirstInstall`) is called from `yk/l0.O()` and `zk/a$a.d()`/`zk/a$b.d()` —
different call sites from the `vi/e` path above, not chained from `N2()`. Whether
V1 or V2 actually fires in a given install was not determined (§7).

### 5.3 MessageBus is downstream, not upstream

No caller of any `MessageBus` native traces back to `ActivitySplash`,
`ActivityNativeMain`, `vi/e`, or `vi/a`. Every caller found is a Lua-facing
"protocol" object (`WebViewProtocol`, `MediaPickerProtocol`,
`RecentlyPlayedWidgetHandler`, `cn/f`) instantiated well after the shell would
already be up. This supports treating `MessageBus`/`Connection` as the ongoing
RPC channel the Lua app uses once running, not part of the ignition sequence the
task asked about.

---

## 6. The manifest finding (repeated from §3 because it changes the plan)

`ActivitySplash` is the only exported, `LAUNCHER`-tagged Activity.
`ActivitySplash.w2(Z)` hard-codes `new Intent(context, ActivityNativeMain.class)`
(behind one `vk/e.a(Context)` call tried first, not resolved in this pass) and
calls `startActivity()` + `finish()`. `MainGameActivity` is present, has the AGDK
`lib_name` meta-data, is `exported=false`, and is a real, reachable, working
JNI/AGDK surface — Cordial's Phase-1 work against it (`findings.md` §8) is not
wrong — but nothing in the manifest or the bytecode found in this pass shows
`ActivitySplash` (or anything else) routing to it by default. If that holds up
under runtime verification, **the natives to make Cordial's next milestone
(a rendered frame of the actual Roblox home shell) are the `ActivityNativeMain` /
`vi/e` / `NativeGLInterface` ones in §5.2, not solely the `MainGameActivity` /
`GameActivity.initializeNativeCode` sequence `path-to-a-frame.md` currently
describes.** The two may not be mutually exclusive — `MainGameActivity` might be
what a joined *game* uses (§1.1's Game half, `StartGameParams`) once the app shell
already launched a game from inside `ActivityNativeMain` — but that composition
was not confirmed here.

---

## 7. What's inferred vs verified — and open questions

**Verified directly** (symbol table, dex declarations, manifest bytes, bytecode
xref):
- Every native signature in §1.
- The class/method inventories in §2.
- `ActivitySplash` is the sole launcher Activity, and its default `Intent` target
  class literal is `ActivityNativeMain` (§3, §6).
- `MainGameActivity extends GameActivity`, `ActivityNativeMain extends
  com.roblox.client.a` (not `GameActivity`) — from the dex `class_def` superclass
  field.
- The exact call chain `ActivityNativeMain.N2()` → `vi/e.j()` →
  `nativeAppBridgeV2InitWithParams`, then → `nativeAppBridgeStartLuaAppDM()`
  (§5.2), and that this is the *only* call site for the latter in the dex.
- `MessageBus` has no caller upstream of the app shell (§5.3).

**Inferred, not verified:**
- That step 11 in §5.2 always executes (vs. being skippable by a branch this
  pass's bytecode scan couldn't see) when `isColdStartDeeplinkToGame()` is true.
- The semantic meaning of the two telemetry-logger calls and the shared
  `0x2996048` dispatcher inside `nativeAppBridgeV2StartAppWithParams` /
  `nativeAppBridgeStartLuaAppDM` (§4.1) — structurally consistent with "post a
  named message," but the string contents themselves are relocation-filled and
  were not read.
- Whether `NativeAppBridgeInterface.nativeAppBridgeAppStart` (V1, §1.2) or
  `NativeGLInterface`'s V2 API is what actually fires for a given
  install/experiment bucket — both exist, both have live callers, and this pass
  did not determine which (or whether both, in sequence) run.
- What `vk/e.a(Context)` (§3, tried before the hardcoded `ActivityNativeMain`
  intent) can return — its `class_data_item` wasn't resolved in this pass because
  its definition apparently lives in a different one of the three dex files than
  its call site; if it can build an `Intent` targeting `MainGameActivity` under
  some experiment, that would change §6's conclusion from "default path" to
  "one of (at least) two live paths gated by a flag."
- Whether `vi/a`/`vi/e` are reached from `MainGameActivity` at all, by any path.
  `vi/p` was checked and ruled out as the link (§2.1). No other shared interface
  or field between `MainGameActivity` and `vi/e`/`vi/h0`/`vi/a` was found, but
  this bytecode scanner cannot fully rule out an indirect path through a runtime
  type it can't see statically.

**Not established at all:**
- Whether any of this actually produces an OpenGL call once run for real —
  everything above is static analysis of the APK; no code was executed.

---

## 8. Tooling used (read-only, not committed)

Two small scratch scripts were written for this investigation, both pure-Python,
no dependencies, read-only:

- **A dex bytecode xref scanner.** `tools/dex_method.py` (existing, in-repo) only
  enumerates `method_ids` — declared signatures, no bytecode. To answer "who
  calls this," this investigation extended the same struct-level dex parsing
  (string pool, type ids, proto ids, plus `class_def_item` → `class_data_item` →
  `code_item`) and scanned every method's raw instruction stream for
  `invoke-{virtual,super,direct,static,interface}` and their `/range` forms
  (opcodes `0x6e`–`0x72`, `0x74`–`0x78`), pulling the method-index operand out of
  the fixed second code-unit both formats share. **Caveat:** it advances one code
  unit at a time rather than by true instruction length (no full opcode-length
  table was implemented), so it can in principle mis-parse a multi-unit
  instruction's tail as a spurious opcode. In practice every hit reported in §5
  resolved to a real, sensible method name/class — the false-positive risk this
  shortcut carries is a stray extra edge in the graph, not a fabricated one, and
  none of the load-bearing findings above rest on an isolated, uncorroborated hit.
- **A minimal binary `AndroidManifest.xml` (AXML) parser.** No `aapt`/`apktool`
  was available. AXML is a documented, simple chunk format (string pool +
  `RES_XML_START_ELEMENT`/`END_ELEMENT` chunks); a ~150-line parser was enough to
  recover the element tree with resolved attribute names/values in `base.apk`
  (§3, §6). It implements only what was needed (string pool, element
  start/end, attributes) — not styles, not the resource map's symbolic names for
  non-string attributes (framework attribute IDs beyond the ones printed
  resolved by their string-pool-backed names, which was all that was needed
  here).

Both scripts are throwaway investigation aids in the session scratch directory,
not added to the repository — this task was investigation-only and no code was
modified, per the instructions it was given.

---

## 9. A second, JS-side bridge: `Roblox.Hybrid.Game.launchGame` (issue #40/#34)

Everything above is the *native* app bridge, reached from Java/JNI once a game
is already running inside the engine. There is a second, unrelated contract
one layer up, in the web page roblox.com serves into Cordial's own in-app
`WebView` for the Servers-list popup — found by watching `window.Roblox` from
the page's own JavaScript rather than by reading the dex, because this
contract is the page's, not the engine's.

**The call.** A game's Servers list renders each server as a card with a
"Join" button. Pressing it calls, verbatim:

```text
Roblox.Hybrid.Game.launchGame(payload, callback)
```

`payload` arrives as either a JSON string or a plain JS object — both were
observed on real clicks, and the field is genuinely one or the other, not
always pre-stringified as an early capture through a `JSON.stringify`-based
logger appeared to show. Its fields, all observed on real signed-in clicks
against two different games:

```text
{
  "requestType": "RequestGameJob",
  "placeId": "<numeric place id>",
  "instanceId": "<UUID of one specific running server>",
  "isPlayTogetherGame": false,
  "browserTrackerId": "<numeric>",
  "joinAttemptId": "<UUID>",
  "joinAttemptOrigin": "publicServerListJoin"
}
```

`callback` is a real JavaScript closure the page defines (observed as
something in the shape of `function(){n.resolve(t)}`), never a plain
serialisable value — confirming this is genuine page-side JS with its own
promise/resolve machinery, not a native stub answering to a fixed shape.

**Nothing native ever received this call**, on any build before issue
#40/#34's fix: `Roblox.Hybrid` is the page's *own* namespace object,
assigned by the page's own bootstrap, and grepping this whole codebase
confirms nothing in Cordial ever created or seeded `window.Roblox` in any
form. There was no native layer underneath the call for it to reach, which
is the entire reason pressing Join did nothing.

**`StartGameParams` (§2.2) was not needed for this, and was deliberately not
built.** The fix (`crates/cordial-shell/src/webview.rs`,
`crates/cordial-runtime/src/webview.rs`,
`crates/cordial-runtime/src/deeplink.rs::publish_hybrid_game_launch`) wraps
`launchGame` in the page's own JS, forwards the same payload through the
`executeRoblox` bridge this project already had end-to-end, and republishes
it as an extended `Linking.detectURL` message — the same publish
`docs/analysis/deep-links.md` §6 already measured producing a real
`Game.launch`. That path takes a `placeId` and a handful of named query
fields; it does not take an AutoValue-built native struct, and building one
would have meant guessing at a native calling convention this session had no
call site for. `requestType` and `isPlayTogetherGame` are dropped on
delivery: neither has a field anywhere in `StartGameParams`'s own list (§2.2)
or in the engine's own link grammar for either to have gone into if a native
struct had been built instead.

Verified 8/8 across two rounds (5/5 with diagnostics attached, 3/3 again with
them removed) on public servers from the Servers list, each landing in the
exact `instanceId` requested per the engine's own join log. Private servers
remain untested — none was reachable from the signed-in test account without
a purchase.
