# Unresolved Java surface — what the engine asks for and doesn't get

**Method:** one real run, captured in full, of

```
cd <repo>
LIBDIR=<scratchpad>/apk/native/lib/x86_64
APK=<scratchpad>/apk/base.apk
CORDIAL_STUB_QUIET=1 CORDIAL_LOG_LEVEL=v timeout 90 ./target/release/cordial-load \
    --lib-dir "$LIBDIR" --apk "$APK" --host-libc --game-activity --run 10
```

plus a second identical run with `CORDIAL_COUNT_GL=1` (no `CORDIAL_LOG_LEVEL=v`) to get thread
states and a graphics-call tally at the end of the 10s window. Both logs are in the scratchpad,
not the repo (343 and ~330 lines respectively). Class descriptors were cross-checked against the
shipping dex with `tools/dex_method.py`. `native/android_classes.cpp`, `native/game_activity.cpp`
and `native/init_params.cpp` were read to see what Cordial already implements; nothing in them was
changed. `sober-oss-reference/decompiled/` was not read.

## 1. The run, phase by phase

`crates/cordial-runtime/src/bin/load.rs` drives a fixed sequence once `JNI_OnLoad` returns:
`initializeNativeCode` → `JNIAAssetManagerSetup.initNative` → `LocalStorageManager
.initStorageManagerNativeV3` → `MainGameActivity.nativeAppBridgeSetInitParams` →
`MainGameActivity.nativeRetryInit` → `GameActivity.{onStartNative,onResumeNative,
onSurfaceCreatedNative,onSurfaceChangedNative}` → then the process just pumps its looper for the
`--run` duration (10s here).

**Every single "Constructed Unresolved symbol" line in this run happens before "surface handed to
the engine" is printed** — i.e. before the looper pump starts. Confirmed by line position in the
raw log (all unresolved-symbol lines are above line 331 of 343; "surface handed to the engine" is
line 338). Once the looper starts pumping:

```
D/GameActivity  ************** mainWorkCallback *********
D/GameActivity  ************** mainWorkCallback *********
```
— printed twice, then nothing for the remaining ~10 seconds. The second run, with
`CORDIAL_COUNT_GL=1`, measured this idle window directly:

```
  threads:
    Main               state=R  wchan=0
    RBX Worker A       state=S  wchan=futex_do_wait
    Main               state=S  wchan=ep_poll
    Main               state=S  wchan=futex_do_wait
    HttpClient         state=S  wchan=poll_schedule_timeout.constprop.0
    HttpClient         state=S  wchan=futex_do_wait
  looper polls: 207

  graphics calls Roblox made:
    eglCreateWindowSurface   0
    eglMakeCurrent           0
    eglSwapBuffers           0
    glClear                  0
    glDrawElements           0
    glDrawArrays             0
    glCompileShader          0
    glTexImage2D             0
```

So: zero GL calls (verified, not inferred), a real `HttpClient` thread exists but is parked in a
**scheduled-timer wait** (`poll_schedule_timeout`), not blocked on `connect()`/`recv()` — consistent
with a network attempt that already ran to completion (success or failure) rather than one that is
currently in flight. `RBX Worker A` is idle on a futex. `looper polls: 207` over 10s shows the main
thread is alive and cycling, just with nothing to do.

**This means the stall does not manifest as a new stub being hit.** `=== no stubs were called ===`
is printed at the very end of both runs — none of the 49 lower-level (libc/libmediandk/
libjnigraphics) stubs fired during the whole run. The stall is entirely upstream of that: something
during the startup phase (before the looper even starts) silently swallowed a signal the engine
needed, and the engine is now idling on purpose, not crashing or spinning.

## 2. Full inventory, deduplicated, grouped by class

"Constructed Unresolved symbol" = libjnivm had no hook for this exact class/method-or-field/
signature and returned a stub. Startup-path column: **yes** = observed before "surface handed to
the engine"; every row below is yes, per §1. What differs is whether the *call* actually fired
(engine executed code that reads/invokes it) versus the symbol was merely pre-registered (e.g. AGDK
caching every method ID it might need at `JNI_OnLoad` time, whether or not it is ever invoked this
session).

### 2a. `com/roblox/client/startup/MainGameActivity` / `NativeHelper` — the bootstrap/flags gap

Not in the 22-class list in `observed-java-surface.md` — these are reached via `GetObjectClass` +
`GetMethodID` (reflection-style access), not `FindClass`+`Hook`, and they are **not implemented
anywhere in `native/*.cpp`** at all (no `MainGameActivity`, `NativeHelper`, or `Context`/`Activity`
class is registered in `android_classes.cpp` or `game_activity.cpp`).

| Method | Signature | Called? | What a correct implementation needs |
|---|---|---|---|
| `getResources` | `()Landroid/content/res/Resources;` | called, receiver null | A real `Resources`/`DisplayMetrics` object with `density` — Cordial has none |
| `getNativeHelper` (MainGameActivity) | `()Lcom/roblox/client/startup/NativeHelper;` | called, receiver null | — |
| `bootstrapTheApp` (MainGameActivity) | `()V` | called, receiver null | **Java application logic**, not a getter — see below |
| `gameActivity_onFlagsFailed` (NativeHelper) | `()V` | called, receiver = `Invalid` (via `GetObjectClass(null)`) | Real notification that FastFlags loading failed; see below |

Dex confirms `NativeHelper.gameActivity_onFlagsFailed()V` has a sibling,
`gameActivity_onFlagsLoaded(Ljava/nio/ByteBuffer;)V` — this is a real success/failure pair for a
flags fetch, not a guess. `MainGameActivity` also exports a *native* method
`nativePreloadFlagOverrides(Ljava/lang/String;)V` (Java calls into native with the fetched flag
string) and `bootstrapTheApp()`/`getNativeHelper()` are themselves ordinary (obfuscated) Java
methods on `MainGameActivity`, confirmed with `tools/dex_method.py --class
com/roblox/client/startup/MainGameActivity`.

**`bootstrapTheApp()` is Kotlin/Java application logic, not a platform-service accessor.** It is the
method whose real bytecode would: talk to Roblox's FastFlags service (network), and — on success —
call native's `nativePreloadFlagOverrides` or — on failure — call `NativeHelper
.gameActivity_onFlagsFailed()`. Cordial has no JVM, so this method literally cannot execute; there
is no "correct one-line stub" for it the way there is for `getFilesDir()`. Implementing this
properly means Cordial's own C++ code has to *replace* this piece of Roblox's own Java startup flow
— fetch/skip flags and then explicitly drive whatever native entry point the real Java bootstrap
would have called next (`nativePreloadFlagOverrides`, or directly proceed with defaults).

**A separate, unexplained detail sits underneath this:** `getResources`, `getNativeHelper`,
`bootstrapTheApp`, and (further down, same call chain) `baseURL`, `userAgent`, `platformParams`,
`isTablet`, `isVrDevice`, `assetFolderPath`, `dpiScale`, `isTouchDevice` are *all* reached through
`GetMethodID`/`GetField` with **`GetObjectClass` returning `Invalid` because the `jobject` argument
itself was null** — confirmed by reading `third_party/libjnivm/src/jnivm/vm.cpp:241`:
`jo ? … : env->FindClass("Invalid")`. The last group (`baseURL` … `isTouchDevice`) are fields
Cordial's own `native/init_params.cpp` *does* implement, correctly hooked with both `HookInstance`
and `HookInstanceGetterFunction` on the `InitParams`/`PlatformParams` objects it constructs and
passes to `nativeAppBridgeSetInitParams`. Yet at the point these accessors are invoked, the receiver
is a null reference, not Cordial's `InitParams` instance — so the engine appears to be re-reading
these values from some *other*, currently-null, stored reference rather than the object Cordial
handed it. This was **verified** (the null-jobject code path in libjnivm) but **not diagnosed**
(which stored reference, and why it's null, would need tracing into Roblox's own compiled
`initializeNativeCode`/`nativeAppBridgeSetInitParams`, which is out of scope here). Flagging it
because no getter-level fix to `InitParams`/`PlatformParams` will matter until this receiver is
understood.

### 2b. `com/roblox/client/LocalStorageManager`

| Method | Signature | Called? | Correct return |
|---|---|---|---|
| `getAllocatableBytes` | `()J` | yes | Real free bytes on the files directory (`statvfs` on the path `native/android_classes.cpp`'s `files_dir()` already computes), not `0` |

Class exists (`FindClass` succeeds, presumably auto-vivified) but nothing hooks this one method.
Returning `0` reads as "no space available" to the engine, which is a plausible independent reason
for it to refuse to cache/download anything even if flags succeeded.

### 2c. `com/roblox/engine/jni/NativeGLJavaInterface` — extra callbacks, pre-registered, not yet fired

**Correction, and one of these turned out not to be idle.** `onDataModelNotificationCallback`,
`onAppBridgeNotification` and `getWebViewUserAgent` are now hooked in
`native/android_classes.cpp`; they are the engine's whole web-view surface on the transport this
build uses, and `docs/analysis/webview-surface.md` is the map. The claim below that none of these
shows a call this run was true of the run it describes and is **wrong in general**:
`onDataModelNotificationCallback` fires three times before the landing page on every run — measured
on three consecutive 40-second runs, `APP_READY` with `PlatformAccountRouter`, `Startup` and
`Landing`. It only looked idle because nothing was hooked to notice.

The rest of the table stands.

`native/android_classes.cpp` implements this class already (see `observed-java-surface.md`), but
Roblox's `JNI_OnLoad` looks up *every* static native-facing method on it up front, and these are not
hooked:

| Method | Signature |
|---|---|
| `gameLoadedCallback` | `(J)V` |
| `onDataModelNotificationCallback` | `(Ljava/lang/String;Ljava/lang/String;)V` — hooked since |
| `onLuaTextBoxChangedCallback` | `(Ljava/lang/String;)V` |
| `onLuaTextBoxPropertyChangedCallback` | `()V` |
| `onAppBridgeNotification` | `(Ljava/lang/String;Ljava/lang/String;)V` — hooked since |
| `onExtendedAnalyticsRecvCallback` | `([BI)V` |
| `saveImageToAlbum` | `(Ljava/lang/String;)V` |
| `onVrSessionStateUpdate` | `(I)V` |
| `getWebViewUserAgent` | `()V` — hooked since |
| `getMobileAdvertisingId` | `()V` |
| `promptNativePurchaseWithPaymentSessionId` | `(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)V` (and a 2-arg overload) |

None of these show a "Call"/"Invoked" line in this run — only "Constructed" during the upfront
method-ID caching. **Not on the critical path today**, because nothing downstream of the
flags/bootstrap gap (§2a) has run yet. They matter the moment that gap closes: `gameLoadedCallback`
in particular is the signal that a game/experience finished loading, and following the project's own
documented pattern ("Roblox stops at the first null it gets, so unimplemented classes silently halt
progress") the first of these actually invoked will become the next silent wall.

### 2d. `com/roblox/engine/jni/util/NetworkUtils`

| Method | Signature | Called? |
|---|---|---|
| `getPublicIPv4Addresseses` | `()Ljava/lang/String;` | `FindClass` only; never called this run |

Already listed in the 22-class dump in `observed-java-surface.md`. Not yet exercised, but it's a
plausible next thing read once bootstrap proceeds (device/network fingerprinting ahead of a join
attempt).

### 2e. `java/lang/Class.getClassLoader` — logged as unresolved, but appears benign

`Constructed Unresolved symbol, Class=`java/lang/Class`, Method=`getClassLoader``, called twice.
Both times it is immediately followed by `FindClass java/lang/ClassLoader` and a successful
`loadClass`/`findClass` (which **are** implemented, in `native/game_activity.cpp`'s `ClassLoader`
class). The observed behaviour is that whatever object the unresolved `getClassLoader()` stub
returns resolves to a usable `ClassLoader` afterwards — so this specific unresolved symbol does not
appear to have blocked anything in this run, only logged noise. Flagging it so a future run doesn't
waste time chasing it as a suspect; not claiming to understand the exact libjnivm mechanism that
makes it work anyway.

### 2f. AGDK / GameActivity window-inset and lifecycle extras — partially resolved since

**`setImeEditorInfoFields(III)V` and `setWindowFlags(II)V` are implemented** (as no-op
`HookInstanceFunction`s on `GameActivity` in `native/game_activity.cpp`) and confirmed resolving
cleanly in a later run — see `docs/NEXT.md` §1's "AGDK's `InputConnection`" subsection. The other
three below were not touched and remain accurate as "not yet exercised":

| Class | Members |
|---|---|
| `com/google/androidgamesdk/GameActivity` | `finish()V`, `getWindowInsets(I)Landroidx/core/graphics/Insets;`, `getWaterfallInsets()Landroidx/core/graphics/Insets;` |
| `androidx/core/graphics/Insets` | fields `left`, `right`, `top`, `bottom` (all `I`) |
| `androidx/core/view/WindowInsetsCompat$Type` | static `captionBar/displayCutout/ime/mandatorySystemGestures/navigationBars/statusBars/systemBars/systemGestures/tappableElement`, all `()I` |

Would matter for window-inset/IME layout once a frame is actually being drawn. Not currently
blocking anything.

### 2g. `android/view/MotionEvent`, `android/view/KeyEvent` — input accessors, not yet exercised

Pre-registered because `GameActivity`'s native touch/key handlers declare these parameter types;
never invoked because `cordial-load` injects no synthetic touch or key events in this run.

| Class | Methods |
|---|---|
| `android/view/MotionEvent` | `getDeviceId, getSource, getAction, getEventTime, getDownTime, getFlags, getMetaState, getActionButton, getButtonState, getClassification, getEdgeFlags, getHistorySize, getHistoricalEventTime(I)J, getPointerCount, getPointerId(I)I, getToolType(I)I, getRawX(I)F, getRawY(I)F, getXPrecision, getYPrecision, getAxisValue(II)F, getHistoricalAxisValue(III)F` |
| `android/view/KeyEvent` | `getDeviceId, getSource, getAction, getEventTime, getDownTime, getFlags, getMetaState, getModifiers, getRepeatCount, getKeyCode, getScanCode, getUnicodeChar` |

Not currently blocking anything — will matter the moment real input needs to reach the engine.

### 2h. `android/content/res/Configuration`, `android/os/LocaleList`, `java/util/Locale` — actually invoked, likely benign

Unlike 2f/2g, these **were** invoked — `native/game_activity.cpp`'s `Configuration` class is
registered empty ("the native side reads configuration through `AConfiguration_*` rather than this
object's fields"), and AGDK's own `initializeNativeCode` reads every field off the `Configuration`
object passed to it immediately, before Cordial's own real screen/config values are relevant to it:

| Class | Field/Method | Signature |
|---|---|---|
| `android/content/res/Configuration` | `colorMode, densityDpi, fontScale(F), fontWeightAdjustment, hardKeyboardHidden, keyboard, keyboardHidden, mcc, mnc, navigation, navigationHidden, orientation, screenHeightDp, screenLayout, screenWidthDp, smallestScreenWidthDp, touchscreen, uiMode` | mostly `I`, `fontScale` is `F` |
| `android/content/res/Configuration` | `getLocales` | `()Landroid/os/LocaleList;` |
| `android/os/LocaleList` | `size`, `get` | `()I`, `(I)Ljava/util/Locale;` |
| `java/util/Locale` | `getLanguage, getScript, getCountry, getVariant` | all `()Ljava/lang/String;` |

All return 0/""/empty-list defaults today. Likely low-impact: Cordial's own native config path
(`android::config::set_screen`, per the loader) is the one the comment in `game_activity.cpp`
documents as authoritative; this Java-side object is probably read once by AGDK's own C++ for a
value it caches, and 0-valued defaults have not visibly broken anything downstream in this run.
Not verified further — flagged as low-priority rather than cleared.

### 2i. `com/google/androidgamesdk/gametextinput/{State,InputConnection}` — implemented since

**Correction: this was found to matter, and is now implemented**, not merely something that would
matter "once exercised" — a later run's jnivm log showed the engine reaching for these on `Constructed
Unresolved symbol` during ordinary bring-up, not only inside a hypothetical on-screen-keyboard path.
See `docs/NEXT.md` §1's "AGDK's `InputConnection`" subsection for the full account: `InputConnection`
is now a real class in `native/game_activity.cpp`, constructed once and registered with the engine via
`GameActivity.setInputConnectionNative`, with `setState`/`setSoftKeyboardActive`/`restartInput`
implemented as real instance-method hooks rather than left unresolved. `State` was already implemented
here as `TextInputState` (same file) before this was written.

| Class | Member | Signature |
|---|---|---|
| `InputConnection` | `setState`, `setSoftKeyboardActive`, `restartInput` | `(Lcom/google/androidgamesdk/gametextinput/State;)V`, `(ZI)V`, `()V` |
| `State` | `text, selectionStart, selectionEnd, composingRegionStart, composingRegionEnd` | `Ljava/lang/String;`, `I` x4 |

Resolving cleanly is confirmed; whether `setState` actually changes what appears in a typed field is
not yet — see `docs/NEXT.md` §1 for exactly what is and is not verified.

## 3. Top 5 most likely to be blocking progress

1. **`MainGameActivity.bootstrapTheApp()` / `NativeHelper.gameActivity_onFlagsFailed()` — the
   FastFlags bootstrap gap.** Highest confidence. `bootstrapTheApp()` runs, synchronously followed
   by `gameActivity_onFlagsFailed()` — a real success/failure notification pair confirmed by the
   sibling `gameActivity_onFlagsLoaded(ByteBuffer)` in the dex — and this happens *before* the surface
   is handed to the engine. Neither `MainGameActivity` nor `NativeHelper` is implemented anywhere in
   `native/*.cpp`. Corroborating evidence: a live `HttpClient` thread exists at the end of the run but
   sits in a scheduled-timer wait rather than an active connect/read, which is what you'd expect right
   after one attempt has already resolved (failed) and nothing has queued a retry. This is Java
   application logic, not a getter — there is no cheap stub for it.

2. **The null-receiver chain underneath it** (`getResources`, `getNativeHelper`, `bootstrapTheApp`,
   `baseURL`, `userAgent`, `platformParams`, `isTablet`, `isVrDevice`, `assetFolderPath`, `dpiScale`,
   `isTouchDevice`, all via `GetObjectClass(null)`). Priority because it undermines confidence in
   fix #1 in a specific way: `native/init_params.cpp` already implements `baseURL`/`userAgent`/
   `platformParams`/`isTablet`/`isVrDevice`/`assetFolderPath`/`dpiScale`/`isTouchDevice` correctly as
   hooked fields *and* AutoValue-style getters, yet the engine is reading them through a different,
   currently-null, reference at this point in the trace. Until it's understood which reference that
   is and why it's null, implementing `NativeHelper` in isolation may not be sufficient — the object
   identity problem could recur for whatever object `NativeHelper` itself needs.

3. **`LocalStorageManager.getAllocatableBytes()J`** returning `0`. Priority because it is cheap,
   mechanical, and independently sufficient to explain "no network/no assets": an engine that
   believes it has zero bytes of allocatable storage has a legitimate reason to refuse to proceed,
   regardless of how #1/#2 resolve. `native/android_classes.cpp` already computes a real
   `files_dir()` — the same directory a `statvfs` call would need.

4. **`NativeGLJavaInterface`'s uncalled callbacks** (`gameLoadedCallback`, `onDataModelNotificationCallback`,
   `onAppBridgeNotification`, etc.). Priority is speculative-but-cheap-to-pre-empt: these are the
   next class in the same file that already implements `NativeGLJavaInterface`, they are pre-cached
   and ready to be hit the moment #1–#3 unblock forward progress, and per the project's own working
   method ("it stops at the first null it gets, so unimplemented classes silently halt progress")
   the next stall after this one is very likely to be one of these, not a new class.

5. **`NetworkUtils.getPublicIPv4Addresseses()`**. Lower confidence — it's never actually called in
   this run — but it's already surfaced in the 22-class dump from `--dump-classes`, it's the kind of
   device/network fingerprinting call that plausibly precedes a join attempt, and it's cheap to
   implement (read the host's own interface addresses) relative to its potential to be the next wall.

## 4. Verified vs inferred — explicit

**Verified directly, this session:**
- The full deduplicated list in §2, extracted by grep from a real run's raw output.
- All unresolved-symbol lines occur before "surface handed to the engine" (by line position in the
  log).
- Zero GL calls during the 10s idle window (`CORDIAL_COUNT_GL=1` tally).
- Thread states during the idle window, via `/proc/self/task` (`HttpClient` parked in
  `poll_schedule_timeout`, not blocked on socket I/O; `RBX Worker A` on a futex).
- `gameActivity_onFlagsFailed`/`gameActivity_onFlagsLoaded` are a real pair, and `bootstrapTheApp`/
  `getNativeHelper` are real `MainGameActivity` methods — via `tools/dex_method.py` against the
  shipping dex.
- `GetObjectClass(null jobject)` returns `FindClass("Invalid")` in libjnivm — read directly from
  `third_party/libjnivm/src/jnivm/vm.cpp:241`.
- What `native/android_classes.cpp`, `native/game_activity.cpp`, `native/init_params.cpp` currently
  implement (used to determine which unresolved symbols already have a partial implementation
  elsewhere vs. none at all).

**Inferred, not proven:**
- That `gameActivity_onFlagsFailed` is specifically what halts forward progress, as opposed to some
  other event in the same short window.
- That the `HttpClient` thread's parked state is causally downstream of the flags failure, rather
  than an unrelated idle keep-alive thread that happens to also be idle.
- That the null-receiver chain (§2a, item 2) shares a root cause with the flags failure, rather than
  being a separate, coincidentally-adjacent bug.

**Not established:**
- What `NativeHelper.gameActivity_onFlagsFailed()`'s real (Kotlin) implementation does — would
  require reading decompiled Roblox code, which this task was told not to do.
- Whether the flags fetch failed at the network layer (DNS, TLS, sandboxing) or never actually
  reached a socket call — `CORDIAL_ANDROID_TRACE` doesn't cover libc socket calls, so this wasn't
  directly observed in this session, only inferred from the `HttpClient` thread's state.
- Whether fixing `LocalStorageManager.getAllocatableBytes` or the `NativeGLJavaInterface` extras
  would change observable behavior at all before #1/#2 are addressed — they're prioritized on
  structural grounds (cheap, plausible, next-in-line), not because they were shown to matter in this
  run.
