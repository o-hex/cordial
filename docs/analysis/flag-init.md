# Why `nativeInitializeNativeFlags` reports `onFlagsFailed` and segfaults

**Status:** investigation only; nothing modified except this file, per instructions.
Disassembly against the same `libroblox.so` (Roblox for Android 2.732.1043) used by
`findings.md`, [`app-bridge.md`](app-bridge.md) and [`render-gate.md`](render-gate.md),
using `objdump`/`nm`/`readelf` plus two small scratch scripts (a direct-`call`-site
scanner and a rip-relative-`lea`-site scanner, same technique `render-gate.md` §2
used for `eglCreateWindowSurface`). Cross-checked against `native/init_params.cpp`
(read, not modified) and reproduced once against the built `cordial-load` binary
without rebuilding it, since `native/*.cpp` is being edited concurrently.

**Bottom line up front:** `nativeInitializeNativeFlags` itself is not the function
that calls `gameActivity_onFlagsFailed`. Its own disassembled body — verified in
full — constructs the `NativeFlagsInitResult` exactly as documented, iterates the
input `String[]`, and returns normally, exception-safe, no matter what is in that
array. The **only** call to `gameActivity_onFlagsFailed` anywhere in the 116 MB
binary lives in a small, separate helper reached through a completely different,
**indirectly-invoked** function that has zero direct callers in `.text` — the same
shape `render-gate.md` §2 already found for `eglCreateWindowSurface`'s trigger, and
for the same reason: something calls it through a function pointer this pass cannot
resolve statically. What *is* pinned down precisely is the gating condition on that
call, and a live, currently-wrong piece of Cordial's own code that feeds
`nativeInitializeNativeFlags` a single "flag name" that is actually Roblox's entire
22,318-flag settings document.

---

## 1. `nativeInitializeNativeFlags` builds its result exactly as documented — verified

`Java_com_roblox_client_flags_FlagJniInterface_nativeInitializeNativeFlags` sits at
file offset `0x215a0b3` in `.dynsym`, 1625 bytes up to the next export
(`nativeRegisterFFlag` at `0x215a70c`). The dex confirms the descriptor:

```
com/roblox/client/flags/FlagJniInterface.nativeInitializeNativeFlags([Ljava/lang/String;)Lcom/roblox/client/flags/NativeFlagsInitResult;
```

Every `call *0xNN(%rax)` in this function was decoded against the exact
`JNINativeInterface` layout Cordial's own libjnivm ships
(`third_party/libjnivm/include/jni.h`), counted field-by-field rather than assumed
from memory — the header has an extra `ToReflectedField` slot that a memorized
table would miss, and getting one offset wrong cascades into every later one:

| Offset | Index | Function | Confirms |
|---|---|---|---|
| `0x30` | 6 | `FindClass` | class `com/roblox/client/flags/NativeFlagsInitResult` (read from `.rodata` at `0x34b965`) |
| `0x108` | 33 | `GetMethodID` | `"<init>"` `"(I)V"` (at `0x445690`/`0x46267b`), then `"addBoolean"` `"(Ljava/lang/String;ZZ)V"` (at `0x4628cc`/`0x507da6`) |
| `0x558` | 171 | `GetArrayLength` | called on the JNI argument itself — **it is a `jarray`, not a `jstring`** |
| `0x568` | 173 | `GetObjectArrayElement` | one call per loop iteration, index `i` |
| `0x720` | 228 | `ExceptionCheck` | after each `addBoolean` call |
| `0x88` | 17 | `ExceptionClear` | called, not `FatalError` — a pending exception here is **swallowed**, not fatal |
| `0xb8` | 23 | `DeleteLocalRef` | cleans up the array element every iteration |

Control flow, verified instruction-by-instruction:

1. `FindClass("com/roblox/client/flags/NativeFlagsInitResult")`.
2. `GetMethodID(class, "<init>", "(I)V")` and `GetMethodID(class, "addBoolean",
   "(Ljava/lang/String;ZZ)V")` — matching `native/init_params.cpp`'s own
   `NativeFlagsInitResult::ctor(ENV*, jint)` and
   `addBoolean(ENV*, shared_ptr<String>, jboolean, jboolean)` hooks exactly, both
   name and descriptor. **Cordial's Java-side stub for this class is shaped
   correctly.**
3. Calls a local helper at `0x215a308` with a *global* pointer (`0x77d9298`, an
   internal engine singleton, unrelated to the JNI argument) to obtain an `int` —
   logged as `"nativeInitializeNativeFlags: flagCount = %d"` (string at `0x34b993`,
   tag `"rbx.JNIRobloxSettings"` at `0x4d0b6b`). This value becomes the constructor's
   sole `int` argument.
4. `NewObject(class, ctor, thatInt)` via a direct-call wrapper at `0x20b7658` — the
   `int` passed here is a **provider ID**, not a count of the array (see §3).
5. `GetArrayLength(env, arg3)` — **confirms the third JNI parameter is read as an
   array**, matching the dex descriptor.
6. Loop `i = 0 .. length-1`:
   - `GetObjectArrayElement(arg3, i)` → one array element (a `jstring`, per the
     descriptor).
   - A local helper (`0x20b6351`) extracts it into a small string wrapper (SSO
     inline buffer or heap-allocated, decided by a size check).
   - A local helper (`0x215a5ce`) looks that name up in an internal table (via a
     shared hash-table `find` at `0x215a48a` — the same low-level `find` also used,
     unrelated to flags, by another function this pass ran into at `0x68fab46`;
     `objdump`'s nearest-export label for it, `...FMOD_OutputAAudioHeadphonesChanged`,
     is nearest-symbol noise per `app-bridge.md` §4.1's caveat, not a real
     relationship).
   - Logs either `"nativeInitializeNativeFlags: ... %d: %s = %s"` (found, `0x3d6727`)
     or `"nativeInitializeNativeFlags: ... %d: %s not found."` (not found,
     `0x40eed1`) — **this is the log line the reproduction run below actually
     produced.**
   - `addBoolean(result, name, found, wasOverridden)`.
   - `ExceptionCheck` → if true, `ExceptionClear` (swallowed, execution continues).
   - `DeleteLocalRef` on the element, next iteration.
7. Stack-canary check, return the constructed `NativeFlagsInitResult`.

**No path through this function — including all three of its own internal, non-exported
helpers, which were disassembled in full — references the strings `"NativeHelper"` or
`"gameActivity_onFlagsFailed"`, and none of them can throw past the per-element
`ExceptionClear`.** Whatever ends up in the array, this function returns a validly
constructed result object. It is not, itself, capable of calling
`gameActivity_onFlagsFailed`, and nothing in it looks capable of segfaulting on its
own account either (SSO/heap string handling is bounds-checked; the hash lookup is a
generic `find`; the JNI calls are all standard, guarded ones).

---

## 2. The live bug: Cordial is passing the entire ClientSettings document as a single flag *name*

`native/init_params.cpp`'s `cordial_init_flags` (the C++ side of `cordial_init_flags`,
called from `crates/cordial-runtime/src/bin/load.rs`) contains this, **as of this
session, unedited by this investigation**:

```cpp
// The array is a list of flag *names to cache*, not a settings document.
//
// This was wrong for several iterations: passing Roblox's ClientSettings
// JSON here made the engine call addBoolean with the entire document as a
// single flag name, which is exactly what the trace showed. ...
// An empty list is therefore correct: cache nothing up front. ...
const bool have = settings_json && *settings_json;
auto arr = std::make_shared<jnivm::Array<jnivm::String>>(have ? 1 : 0);
if (have) {
    arr->Set(0, cordial::S_pub(settings_json));
}
```

The comment states the array should always be empty. **The code directly below it
does not do that** — when `--client-settings` points at a real file (as the
reproduction instructions in this task do), `have` is true and the entire file
contents become element 0 of a one-element `String[]`. This is a live discrepancy
between documented intent and actual behaviour, not a hypothesis: the reproduction
run below shows it happening.

### Reproduced

Ran the exact command given, against the already-built binary (did **not** run
`cargo build --release` first, to avoid racing the concurrent edits to
`native/*.cpp`):

```
CORDIAL_STUB_QUIET=1 timeout 100 ./target/release/cordial-load --lib-dir "$LIBDIR" \
  --apk "$APK" --client-settings /tmp/clientsettings.json --host-libc --game-activity --run 8
```

Result: **exit 139 (SIGSEGV)**, `timeout: the monitored command dumped core`. The
captured output includes:

```
[roblox] flags FAILED — the engine could not load its flag set
I/rbx.JNIRobloxSettings    nativeInitializeNativeFlags: ... 0: {"applicationSettings":{"DFFlagConsumePlatformNameOverAlternateName":"False", ... [4123 bytes, cuts off mid-JSON, no closing brace] ...
```

That second line is exactly step 6's "found"/"not found" log (§1), firing for array
index 0, whose "name" is the raw JSON text — confirming, at runtime, that the array
Cordial builds today is the one-element, whole-document array the comment says is
wrong. The line is truncated at roughly 4 KB, consistent with `__android_log_print`'s
own bounded formatting buffer (not a crash by itself — the line ends cleanly with a
newline).

**Caveat on ordering:** the `onFlagsFailed` print (via `fprintf(stderr, …)`, which
Cordial's `NativeHelper::onFlagsFailed` hook uses unbuffered) appears *before* the
`__android_log_print`-based line in the captured file, even though §1's disassembly
shows the per-element log is produced from inside `nativeInitializeNativeFlags`,
which — per §1 — cannot itself call `onFlagsFailed`. The most likely explanation is
buffering: the two log paths go through different sinks with different flush
timing, so file order does not necessarily reflect wall-clock order. Treat the
printed order as inconclusive; both facts (the call happened, the truncated-JSON
log line happened) are independently confirmed, their relative order is not.

**No usable backtrace could be obtained in this sandbox.** There is no `gdb`
installed, and `coredumpctl` (`core_pattern` is piped to `systemd-coredump`) lists
no core for this crash — its storage is not retaining dumps here (other processes'
crashes show the same `COREFILE missing`). This is worth recording so a future
session does not repeat the attempt: getting a real backtrace requires either
installing a debugger, or Cordial installing its own `SIGSEGV` handler (the
terminate-handler precedent in `native/jni_shim.cpp`, per `findings.md` §8.2, is the
right shape for this).

---

## 3. Where `gameActivity_onFlagsFailed` is actually called from — verified, with one open edge

A whole-binary scan (every rip-relative `lea` in `.text`, ~80.6 MB, checked against
its computed target — the same method `render-gate.md` §2 used for
`eglCreateWindowSurface`) for the string `"gameActivity_onFlagsFailed"` (the only
occurrence of that string in the file, at `0x40f096`) found **exactly one** site
referencing it: `0x29c931e`.

That address is inside a small, non-exported helper starting at `0x29c92eb`. Its
entire body:

```
GetMethodID(cachedClass, "gameActivity_onFlagsFailed", "()V")   ; class ref cached at 0x6eba788
CallVoidMethod(cachedInstance, thatMethodID)                     ; instance cached at 0x6eba780
```

— i.e. this is precisely the JNI call that reaches Cordial's
`NativeHelper::onFlagsFailed` hook (`native/init_params.cpp:279`). A sibling copy
seven bytes later (`0x29c937d`) is structurally identical but calls
`"gameActivity_onEngineInitialized"` instead (`0x462ada`) — both share the
`FLog::JNINativeHelper] FATAL: Java exception occurred in JNI call.` diagnostic
string (`0x40f0d3`), consistent with a generic "call this no-arg `NativeHelper`
callback, log FATAL if it throws" template instantiated per callback name.

**This `onFlagsFailed` helper has exactly one direct caller in the whole
binary**: `0x29c553c`. That call site is gated by:

```
this = <some object>                     ; entered with this in %rbx
if (this->[0x10] == null) goto skip;      // 0x29c54f4
handle = getter(this->[0x10]);            // 0x2932f40 — looks like weak_ptr::lock()
if (handle == null) goto skip;            // 0x29c550d
target = this->[0x8];
if (target != null) {
    target->[0x10] = 11;                    // marks a state/result field FAILED
    obj = target->[0x40]->[0x10]->[0x10];   // three more pointer dereferences
    report_onFlagsFailed(obj);              // 0x29c92eb — the call at 0x29c553c
}
// unconditionally: release `handle` (refcount decrement, virtual dtor if it hits 0)
```

(`skip` reaches the same release/cleanup path with no report — i.e. those two
null-checks are safety guards around an otherwise-unconditional report, not a
separate success/failure branch; whatever decided "this is the failure case"
happened earlier, outside what this pass could see.)

**The containing function (starts at `0x29c52cc`) has zero direct callers anywhere
in `.text`.** Exactly like `render-gate.md` §2's GPU-tier device-cache function, it
is reached only through an indirect call (a function pointer, `std::function`, or
virtual dispatch) that a direct-`call`-site scan cannot follow. This is the
honest edge of what this pass could establish: **what invokes this check, and
under what real-world condition `this->[0x8]` ends up non-null with state
worth marking `11`, is not determined here.**

What can be said with confidence: `this`, `this->[0x10]`, and `this->[0x8]` are
**not** Java objects and are **not** anything Cordial's JNI layer
(`NativeFlagsInitResult`, `NativeHelper`, etc.) constructs or touches — they are
private, internal C++ engine state, invisible to JNI entirely. Nothing in
`native/init_params.cpp` initialises or influences this object graph one way or
the other. If the crash is in the three-deep dereference chain
(`target->[0x40]->[0x10]->[0x10]`) immediately preceding the report call, or in
whatever indirectly invokes `0x29c52cc` at all, it would be because that internal
state was never populated the way the real Android app populates it before this
code path runs — consistent with, but not proven by, the reproduction in §2. This
is inference, not a verified fact; nailing it down needs a live debugger (§2's
caveat) or an instrumented breakpoint/wrapper at `0x29c52cc`'s entry, similar in
spirit to the libc-call wrapper technique `findings.md` §8.1 already uses.

---

## 4. `nativeRegisterJavaFlagProvider` — verified unreachable from Java in this build

`Java_com_roblox_client_flags_FlagJniInterface_nativeRegisterJavaFlagProvider` is
exported at `0x29aba7a` (57 bytes, ending well before the next export,
`MemStorage.setItem`). Checked against **all three** shipping dex files with
`tools/dex_method.py`, both restricted to the `FlagJniInterface` class and with an
unrestricted name search across every class:

```
$ python3 tools/dex_method.py apk/dex/ --class com/roblox/client/flags/FlagJniInterface
# lists 7 methods: nativeGetFFlag, nativeGetFInt, nativeGetFString,
# nativeInitializeNativeFlags, nativeRegisterFFlag, nativeRegisterFInt,
# nativeRegisterFString — no nativeRegisterJavaFlagProvider

$ python3 tools/dex_method.py apk/dex/ nativeRegisterJavaFlagProvider
no match
```

**No Java class in this shipping build declares this native method at all.** It
cannot be reached through the normal JNI static-linkage path (the
`Java_com_roblox_..._methodName` symbol convention only matters if some
`class.native(...)` declaration in the dex causes the JVM to look it up), and
nothing else in the dex calls it either. Its own body confirms it does not expect
to be called through the normal path: it takes the standard `(JNIEnv*, jclass, ...)`
JNI convention but **ignores every incoming argument** and simply:

```
GetOrRegisterProviderId(&globalProviderRegistry)   ; call 0x215a308 — the SAME
                                                     ; helper nativeInitializeNativeFlags's
                                                     ; own preamble calls (§1 step 3)
__android_log_print(INFO, "rbx.JNIRobloxSettings",
    "nativeRegisterJavaFlagProvider: Registered external flag provider from Java with ID: %d",
    result)
return result
```

The log format string (`0x4b41e6`) confirms the semantics: the shared counter at
`0x215a308` is a **provider-ID generator**, and this native is just an alternate
(unused, in this build) entry point into the same registration the constructor
step in §1 already performs inline. **Answer to the task's question: no, the flag
pipeline does not require calling it first — the real shipping app does not call
it either, and `nativeInitializeNativeFlags` already performs the equivalent
registration itself.** This rules it out as the missing piece.

---

## 5. What's verified vs inferred

**Verified** (disassembly, cross-checked against Cordial's own `jni.h` and
`native/init_params.cpp`, and against a live reproduction):
- `nativeInitializeNativeFlags`'s full body, all three of its private helpers,
  builds `NativeFlagsInitResult` exactly as Cordial's Java-side stub expects
  (§1), takes a `String[]` of flag *names* (not a document), and cannot itself
  call `onFlagsFailed` or crash on the input it's given.
- `native/init_params.cpp`'s `cordial_init_flags` currently packs the entire
  settings document as element 0 of that array when `--client-settings` is
  supplied, contradicting its own comment (§2) — reproduced live.
- The sole reference to `"gameActivity_onFlagsFailed"` in the binary, its sole
  caller, and that caller's gating logic (§3).
- `nativeRegisterJavaFlagProvider` is unreachable from Java in this dex set and
  is not a prerequisite (§4).
- The reproduction: exit 139/SIGSEGV, `onFlagsFailed` fires, the truncated
  whole-document-as-flag-name log line fires (§2).

**Inferred, not verified:**
- That the segfault is specifically the `target->[0x40]->[0x10]->[0x10]` chain in
  §3, or specifically triggered by whatever indirectly invokes `0x29c52cc`. No
  backtrace was obtainable in this sandbox (§2) to confirm the exact faulting
  instruction.
- Why `this->[0x8]` (§3) ends up non-null/marked-11 at all in Cordial's run — this
  requires knowing what real Android's flags subsystem normally does to populate
  it, which is internal engine state with no JNI-visible surface.

**Not established:**
- Whether fixing §2 (passing an empty array, as the code's own comment says it
  should) changes the outcome. Given §3's trigger looks independent of the
  array's contents — it's gated on unrelated internal engine state, not on
  anything `nativeInitializeNativeFlags` touches — there is no evidence either
  way that this fixes `onFlagsFailed`, only that it removes a confirmed,
  currently-live bug (a multi-megabyte string being hashed, logged, and searched
  for as if it were a flag name).

---

## 6. Recommendation

1. **Fix the discrepancy in `native/init_params.cpp`'s `cordial_init_flags`
   (§2) to match its own comment** — always construct a zero-length array,
   regardless of whether `settings_json` is non-empty. This is confirmed-wrong
   today and reproducibly wastes a multi-KB log line and a hash lookup on a
   string that can never be a real flag name. Low risk, because §1 shows an
   empty array makes the loop in step 6 simply not execute — the function still
   returns a validly constructed (if empty) `NativeFlagsInitResult`.
2. **Do not add a call to `nativeRegisterJavaFlagProvider`** — §4 shows it is
   unreachable in the real app and redundant with what
   `nativeInitializeNativeFlags` already does internally.
3. **Fix #1 is very unlikely to be sufficient on its own.** §3's trigger for
   `onFlagsFailed` is gated on internal engine object state
   (`this->[0x10]`, `this->[0x8]`) that has nothing to do with the JNI array
   argument — it is reached through an indirect call this pass could not
   resolve. Expect `onFlagsFailed` (and possibly the segfault) to persist after
   fix #1, unless it happens to be timing-sensitive in a way that an empty,
   fast-returning array changes.
4. **The highest-leverage next step is a working debugger or core-dump
   pipeline in this environment** — there is no `gdb` here and
   `systemd-coredump` is not retaining dumps (§2). Without a backtrace, closing
   §3's open edge (what calls `0x29c52cc`, and what exactly is null/dangling at
   the crash) requires either an external debugger, or an instrumented
   breakpoint/wrapper at `0x29c52cc`'s entry — printing `this`, `this->[0x10]`,
   and `this->[0x8]` at runtime, the same wrapper-based instrumentation
   technique `findings.md` §8.1 already used successfully for libc calls.

---

## 7. Follow-up session: `lldb` is available now, and it found a second, real bug

`lldb` (`/home/linuxbrew/.linuxbrew/bin/lldb`) turned out to be present in this
environment (§6's blocker assumed it was not). With `settings set
target.disable-aslr true`, `libroblox.so` loads at a fixed address every run
(`0x7fffefec0000` under the task's exact repro invocation), which makes raw
breakpoint addresses reproducible run to run. This section only reports what
was confirmed by breaking and inspecting live state — per this project's own
hard-won lesson, static disassembly alone has repeatedly produced wrong
conclusions here (see §3 below for a concrete instance of exactly that).

### 7.1 The real, fixed bug: `NativeFlagsInitResult`'s constructor was never reachable

`native/init_params.cpp` registered `NativeFlagsInitResult`'s constructor with:

```cpp
c->HookInstanceFunction(env, "<init>", &NativeFlagsInitResult::ctor);
```

This looks right, and §1's disassembly (`GetMethodID(class, "<init>", "(I)V")`
then `NewObject`) looks like it should call it. **It never did.** The live JNI
trace (`JNI_TRACE` build of `third_party/libjnivm`) showed, on every run before
this fix:

```
[JNIVM]: Constructed Unresolved symbol, Class=`NativeFlagsInitResult`,
    StaticMethod=`<init>`, Signature=`(I)Lcom/roblox/client/flags/NativeFlagsInitResult;`
[JNIVM]: Call Unknown Static Function Class=`NativeFlagsInitResult` Method=`<init>` ...
```

i.e. libjnivm was looking for a **static** method literally named `<init>`
whose signature has the **return type folded in**, not the instance
constructor Cordial registered. The cause is in libjnivm itself
(`third_party/libjnivm/src/jnivm/internal/method.cpp:13-24`,
`jnivm::GetMethodID`):

```cpp
// Rewrite init to Static external function
if(!isStatic && sname == "<init>") {
    // strips everything after ')', appends "L<nativeprefix>;"
    return GetMethodID<true, ReturnNull, AllowNative, trace>(env, cl, str0, ssig.data());
}
```

Every *instance* `GetMethodID(cls, "<init>", sig)` call is unconditionally
rewritten into a **static** lookup with signature `sig-up-to-')'` +
`"L" + nativeprefix + ";"`. So `GetMethodID(class, "<init>", "(I)V")` actually
resolves against `("<init>", "(I)Lcom/roblox/client/flags/NativeFlagsInitResult;")`,
static. `HookInstanceFunction` can never register a match for that lookup —
it registers an *instance* method with the *original* signature. The engine
got back an auto-synthesized unresolved-symbol stub (which `defaultVal<jobject>`
makes return null), called it, and treated the null/degenerate result as a
reason to report `onFlagsFailed`.

This is not specific to `NativeFlagsInitResult` — it is true of *every*
`<init>` this codebase registers via a real `NewObject`/`GetMethodID` path from
the engine's side. It happened not to matter elsewhere because every other
class in `native/init_params.cpp` is constructed by *Cordial's own C++ code*
calling a `Create()` factory directly (never through JNI dispatch), so the
libjnivm rewrite was never exercised for them. `NativeFlagsInitResult` is
the one class the *engine itself* constructs via `NewObject`, which is exactly
why this only showed up here.

**Fix applied:** register the constructor the same way this file's own
`Create()` factories are shaped — as a plain **static** function taking
`(ENV*, Class*, jint)` and returning `std::shared_ptr<NativeFlagsInitResult>`,
via `c->Hook(env, "<init>", &NativeFlagsInitResult::ctor)` (not
`HookInstanceFunction`). `Class::Hook` auto-detects "static" from the
parameter types (second parameter `Class*`, not `Object*`/`jobject`), and its
derived signature is exactly `"(I)L<nativeprefix>;"` — matching libjnivm's
rewritten lookup. Confirmed live: the trace now reads
`Found symbol ... StaticMethod=\`<init>\`` and
`Call Static Function ... Method=\`<init>\`` (not "Unresolved"/"Unknown") —
the constructor genuinely runs now, `NativeFlagsInitResult` is built with a
real backing `JavaMap`, and its return value is a valid, non-null object
reaching the caller. **`gameActivity_onFlagsFailed` still fires afterward
(see §7.3) — this fix was necessary but not sufficient**, exactly as §6.3
warned.

Also fixed in the same pass: §2's confirmed live bug (the whole ClientSettings
document being packed as a single array element) — `cordial_init_flags` now
always builds a zero-length array, matching its own comment, regardless of
whether `--client-settings` is set.

### 7.2 `com.roblox.engine.jni.model.ClientLocalFlags` implemented; `readLocalFlags()` called

A second investigation thread (a parallel review of the render/network path)
found that `NativeGLInterface.readLocalFlags()` — `()Lcom/roblox/engine/jni/
model/ClientLocalFlags;`, exported at `Java_com_roblox_engine_jni_
NativeGLInterface_readLocalFlags` — is the engine's *offline* counterpart to
fetching `ClientSettings` over the network: it reads whatever bundled/cached
flag defaults the engine has and hands them back via the same `new` +
repeated `add(name, value)` idiom `nativeInitializeNativeFlags` uses for its
own result object. Nothing in the shipping dex calls it on the
`ActivityNativeMain` chain Cordial drives (dex xref: its only caller is a
different startup path), so it was entirely dead code here, and its Java
counterpart class was completely unimplemented.

Implemented `ClientLocalFlags` (dex-verified shape: `<init>()V`,
`add(String,String)V`, `getAll()Lorg/json/JSONObject;`, `isEmpty()Z`,
`size()I`) plus a minimal `org.json.JSONObject` stub, using the same
static-factory `<init>` registration §7.1 established is required. Wired a
`cordial_read_local_flags` bridge and call it right after
`nativeInitializeNativeFlags`. **Result: it runs cleanly (no crash, no
unresolved-symbol noise) but calls `add()` zero times** — this build has no
bundled local flag defaults on disk, so the engine constructs an empty
`ClientLocalFlags` and returns. `onFlagsFailed` is unaffected.

### 7.3 The real trigger: `onFlagsFailed` fires from an unrelated background thread, confirmed by breakpoint

§3's static disassembly identified a single, specific address
(`libroblox+0x29c92eb`) as "the helper that calls `gameActivity_
onFlagsFailed`", reached from one caller at `libroblox+0x29c553c`. **Both
addresses were placed as raw hardware breakpoints and neither one was ever
hit before the process crashed** — even though, in the same run without those
breakpoints, `onFlagsFailed` demonstrably fired (Cordial's own hook printed
`[roblox] flags FAILED`). This is exactly the kind of static-analysis error
the top of this file warns about: the string-reference scan found *a*
call site for the string `"gameActivity_onFlagsFailed"`, but not necessarily
*the* one actually exercised at runtime by this code path.

Breaking instead on Cordial's own hook —
`cordial::NativeHelper::onFlagsFailed` (a real symbol in `cordial-load`,
`nm`-verified, so no address guessing needed) — hits reliably, and its
backtrace is unambiguous:

```
frame #0  cordial::NativeHelper::onFlagsFailed
frame #1  jnivm::Wrap<...>::InstanceInvoke
frame #2  jnivm::MDispatchBase2<void>::CallMethod
frame #3  jnivm::MDispatchBase<void,jobject*>::CallMethod(..., va_list)
frame #4  libroblox+0x68a6fe3
frame #5  libroblox+0x29c8fff
frame #6  libroblox+0x29c9349
frame #7  libroblox+0x29c5541      <- return addr; matches §3's claimed
                                       call site (0x29c553c) + 5 bytes exactly
frame #8-10  libroblox (+0x1f9b850, +0x1f9b728, +0x1f9b5a7)
frame #11 libc.so.6`start_thread + 921
frame #12 libc.so.6`__clone3 + 44
```

**This call happens on a separate `pthread`, spawned via `start_thread`/
`__clone3` — not on the thread that runs Cordial's sequential bring-up code
(`nativeInitializeNativeFlags`, `readLocalFlags`, `nativeInitClientSettings`,
etc. all run on the "calling" thread; this backtrace contains none of those
frames).** §3's outer-caller address (frame #7, `0x29c553c`) is confirmed
correct — the return address matches exactly — but §3's claim that the
`onFlagsFailed`-reporting helper itself lives at `0x29c92eb` is off by one
level of the call chain (frame #6 is closer to that address; the actual
`GetMethodID`+`CallVoidMethod` pair is one level deeper, around
`0x29c8fxx`/frame #5). More importantly: **this whole chain runs
independently of, and after, whatever Cordial's own thread has done.**

This was confirmed empirically, not just from one backtrace: `onFlagsFailed`
fires with the *identical* character (same log line, same async-thread
backtrace shape) across every combination tried in this session —
`nativeInitializeNativeFlags` called with an empty array (correct) or the
old buggy whole-document array; `readLocalFlags` called or not; and
`nativeInitClientSettings` (§7.4) called with a real `ClientSettings`
document, with all-empty arguments, or not called at all. **Nothing this
session found how to influence changes whether or when `onFlagsFailed`
fires.** That is consistent with §3's original conclusion — the trigger is
gated on internal engine state this pass could not identify the origin of —
now confirmed live rather than inferred from disassembly alone.

### 7.4 `nativeInitClientSettings` / `nativePostClientSettingsLoadedInitialization3` — wired, with one new hazard found

Per the architecture Roblox ships: these `NativeGLInterface` natives are not
the engine asking Cordial for settings over JNI — they are the interface a
**host app** uses to hand the engine settings *it* already fetched. Cordial
is the host app here, so calling these directly (with real data, no forged
HTTP responses) is the legitimate interface, not a workaround. Dex-verified
descriptors:

```
nativeInitClientSettings(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I
nativeInitClientSettingsSigned(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I
nativeInitClientSettingsCached(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)I
nativePostClientSettingsLoadedInitialization3(Ljava/util/List;)V
```

Implemented `cordial_init_client_settings` (unsigned variant only — deliberately
not touching `...Signed`, since forging a signature would misrepresent real
account/server state) and called it with the real `--client-settings` document
in the middle argument, empty strings for the other two (their exact roles
were not determined — see below). **It returns `1` — but that value is
*not* a validation result**: it returns exactly `1` for every combination
tried, including all three arguments empty. That is strong evidence the
`int` is a synchronous request handle/"accepted for async processing" code,
not a success/failure flag — consistent with §7.3's finding that the actual
accept/reject decision happens later, on a different thread.

**A new, real hazard found and *not* shipped enabled:**
`nativePostClientSettingsLoadedInitialization3(List)`, called with an empty
`java.util.ArrayList` (a new minimal `JavaList` stub, same static-factory
`<init>` pattern), **crashes synchronously, immediately, on the calling
thread** — verified live under `lldb`: `SIGSEGV`, fault address `0x8`, inside
`libc.so.6\`_IO_fflush`, called from inside the engine's own implementation
of this native. This is a *worse* regression than the pre-existing
asynchronous crash (§3's `libroblox+0x2ccd937`, `SingleSurfaceAppImpl`'s null
`JNIEnv`) — it happens earlier, synchronously, and on Cordial's own thread
instead of an engine-internal one. **The call is implemented (`cordial_
post_client_settings_loaded` / `game_activity::post_client_settings_loaded`)
but gated behind `CORDIAL_TRY_POST_CLIENT_SETTINGS=1` and not run by
default** — an empty list is evidently not what this native expects, and
guessing further was not attempted this session (time-boxed). Whatever real
list contents it wants remain undetermined.

With `nativePostClientSettingsLoadedInitialization3` disabled, the crash
reverts to the original, unrelated `0x2ccd937` (confirmed via the same
breakpoint-on-`onFlagsFailed`-and-`continue` technique) — i.e. §7.4's changes,
as shipped, introduce no new crash.

### 7.5 `nativePreloadFlagOverrides` — a second real bug found, format still undetermined

`--flag-overrides <f>` was **parsed but never wired to any native call at
all** before this session — `opt.flag_overrides` existed as a CLI field with
no corresponding call anywhere in `native/init_params.cpp` or `load.rs`. That
fully explains §3's original "no extra logging" result: nothing was ever
invoked. A `cordial_preload_flag_overrides` bridge (dex-verified descriptor
`MainGameActivity.nativePreloadFlagOverrides(Ljava/lang/String;)V`, an
*instance* native — second JNI argument is an Activity object, following
`cordial_set_init_params`'s precedent of a bare placeholder `jnivm::Object`)
is now wired and called with the file's raw contents.

A second bug was caught in the same pass, while fixing the first: an initial
version of the wiring re-read `opt.flag_overrides` **as if it were a file
path** — but the option parser (`--flag-overrides` in `load.rs`'s argument
loop) already reads the file at parse time and stores its *contents*, not
its path. That re-read silently failed and passed an **empty string**
through. Fixed to use the stored content directly; confirmed by checking the
transmitted byte count matches the source file's size exactly.

**With the call now genuinely delivering real bytes, still no observable
effect was found**: tried a flat `{"FLogChannelName":"7", ...}` map (the
shape suggested by the FLog-channel hypothesis in §3/crash-trace.md) — no new
log lines, no change to the flags verdict or crash. **The correct JSON shape
for this native remains undetermined.** What is now known for certain: the
call itself is reachable and does not throw or crash with a small flat JSON
object as input. Candidates not yet tried: the doubly-wrapped
`{"applicationSettings":{...}}` shape the real `clientsettings.roblox.com`
response uses (same shape as `/tmp/clientsettings.json`, which is ~1.2MB —
worth trying whole, since "preload" suggests it may want the same document
`nativeInitClientSettings` takes); a JSON array of flag names (mirroring
`nativeInitializeNativeFlags`'s actual argument shape); or no JSON at all
(a newline-separated list, as this file's own `--client-settings` help text
describes for a *different*, unrelated option, which may be a hint about the
project's own earlier assumptions rather than the engine's real expectation).

### 7.6 Summary: what changed, what didn't

**Fixed, verified live:**
- `NativeFlagsInitResult`'s constructor now actually runs (§7.1) — this was a
  real, confirmed bug (unreachable native constructor), not a hypothesis.
- `cordial_init_flags` no longer packs the whole ClientSettings document as a
  single array element (§7.1, closing §2/§6.1).
- `readLocalFlags()` / `ClientLocalFlags` implemented and called (§7.2) —
  runs cleanly, contributes nothing (no bundled local defaults in this build).
- `nativeInitClientSettings` implemented and called with the real
  `ClientSettings` document (§7.4) — runs cleanly, returns `1` (an accept
  code, not validation) regardless of payload.
- `--flag-overrides` is now actually wired to `nativePreloadFlagOverrides`
  and delivers real bytes (§7.5) — it was previously a dead CLI option.
- A new synchronous crash (`nativePostClientSettingsLoadedInitialization3`
  with an empty `ArrayList`) was found and **not** shipped enabled (§7.4).

**Not fixed — honest negative result:** `gameActivity_onFlagsFailed` still
fires. It is confirmed, by breakpoint (not inference), to run on a separate
pthread whose creation and decision-making this session could not trace back
to any Cordial-controlled input — every synchronous call this session added
or removed left its behaviour identical. The condition that "chooses failure
over success" is internal engine state on that background thread, still not
identified. The highest-leverage next step is the same as §6.4 named:
tracing what spawns that pthread (breakpoint on `pthread_create`, or on the
`start_thread` return addresses seen in §7.3's backtrace, `libroblox+
0x1f9b5a7`/`+0x1f9b728`/`+0x1f9b850`, to find where that thread's *own*
entry point is, not just its later call stack).

---

## §8. Client settings are not what the flags verdict depends on

Cordial now fetches Roblox's real client-settings document and the engine accepts
it: `nativeInitClientSettings` returns `0`, and
`nativePostClientSettingsLoadedInitialization3` then succeeds. (That call used to
crash synchronously in `_IO_fflush`; the crash was a *consequence* of the
settings not being accepted, not a bad `List`, and it went away on its own once
`nativeInitClientSettings` started returning `0`. It is now unconditional.)

The verdict is still `onFlagsFailed`.

Three orderings were tried, each strictly earlier than the last:

| when the settings are delivered | result |
|---|---|
| after the flag calls (original) | `flags FAILED` before the call |
| before the flag calls | `flags FAILED` still first |
| **before `initializeNativeCode`** — `-> 0` | `flags FAILED` unchanged |

The third is the decisive one. The settings are parsed and accepted before the
engine's own bring-up even starts, and the verdict does not move. So this is not
a race that a better ordering can win, and **the flags verdict does not depend on
the client-settings document.**

That is worth stating plainly because two independent investigations converged on
client settings as the likely root cause of `onFlagsFailed`, and the reasoning
was good: flags *are* client settings, the document really was missing, and the
fetch really was never happening. It was still wrong. The work it produced is
real — the CA bundle, the asset folder, the fetch, the call contract — but none
of it is the answer to this question.

What remains unexplained: `onFlagsFailed` arrives on a background thread, early,
with a full and valid flag set already installed. Whatever it is testing, it is
not "do I have flags".

## §9. Where the verdict is actually made

A breakpoint on Cordial's own `NativeHelper::onFlagsFailed` gives the real call
chain (offsets into `libroblox.so`, ASLR disabled, base `0x7fffefec0000`):

```
onFlagsFailed  (Cordial)
  <- libjnivm CallMethod
  <- 0x29c8fff   JNI varargs wrapper
  <- 0x29c9349   inside a small reporter function starting at 0x29c92eb
  <- 0x29c5541
  <- 0x20db850 / 0x20db728 / 0x20db5a7  = nativeGameGlobalInit
  <- start_thread
```

Two facts worth having:

**There are two separate reporter functions, not one branch.** The string
`gameActivity_onFlagsLoaded` is referenced from exactly one place, `0x29c9182`,
inside a function beginning at `0x29c9120`; `gameActivity_onFlagsFailed` from
exactly one place, `0x29c931e`, inside a *different* function beginning at
`0x29c92eb` (`push %rbp` prologue). So neither reporter chooses anything — the
choice is made by whoever calls one of them.

**The call site is already committed.** At `0x29c553c` the failed reporter is
called unconditionally:

```
29c5519:  mov  0x8(%rbx),%rax     ; an object hanging off the caller's state
29c5527:  je   29c5541            ; skipped entirely if that is null
29c5529:  movl $0xb,0x10(%rax)    ; write 11 into it -- a status, before reporting
29c5530:  mov  0x40(%rax),%rax
29c5538:  mov  0x10(%rax),%rdi
29c553c:  call 29c92eb            ; report FAILED
```

The `movl $0xb` is the useful part: **11 is written into `+0x10` of that object
immediately before the report.** That is a status code being recorded, and it is
a far better instrumentation target than the report itself — a watchpoint on
that word catches the moment the verdict is decided, with the deciding frame
still on the stack. Whatever picks `0xb` is upstream of `0x29c5541` and is the
actual question.

Stopping the static walk here deliberately. This is the point at which previous
investigations on this binary started inferring, and eight consecutive inferences
have been wrong. The next step is a watchpoint on that status word, not more
disassembly.

## §10. Answered — and not the blocker

**Breakpoints inside `libroblox.so` do not work, and never have.** Cordial
`mmap`s the library with its own bionic loader, so the system dynamic linker
never registers it: lldb's `image list` never lists it, and
`breakpoint set --address`/`--shlib` stay permanently `unresolved, hit count 0`.
The only technique that works is writing `0xCC` into the target address with
`memory write`, then on the trap rewinding `$pc` and restoring the original byte.

This is worth knowing before anything else here: **any earlier claim in this repo
of "I set a breakpoint in the engine" is suspect unless it used that method.**
Breakpoints in Cordial's *own* code (`onFlagsFailed`, the Rust driver) resolve
normally, and crash-stop backtraces are genuine — only breakpoints inside the
mapped engine silently never fire.

**What writes the failure status.** RTTI on `%rbx` at `0x29c5529` — not on
`%rax`, whose first qword is a self-pointer rather than a vtable — gives:

```
std::__ndk1::__function::__func<
    RBX::NativeDataModelManager::getFlagsFromEngine()::$_0, ... void()>
```

So the write comes from the completion lambda of
**`RBX::NativeDataModelManager::getFlagsFromEngine()`**, not from generic flag
glue. A sibling type exists for `initEngine()`'s lambda.

**The watchpoint.** `+0x10` holds `2`, and is written exactly once for the whole
run — to `11` (`0xb`) — from that lambda. Nothing else ever touches it, and no
success value is ever written.

**The success path exists but is never reached.** The success reporter has
exactly one static caller in the entire 115 MB binary: `0x29c346d`, inside the
real body of `nativePreloadFlagOverrides`, immediately after a *conditional*
write of status `3` guarded by a byte at `[r15+0x288]`. The reporter itself is
straight-line and unguarded, so if it were reached it would call
`gameActivity_onFlagsLoaded`. Across five runs `onFlagsLoaded` is never resolved
or called. So the success path is gated by something upstream inside
`nativePreloadFlagOverrides` that this harness never satisfies — which is
interesting, because `nativePreloadFlagOverrides` is a function Cordial *does*
call, and whose payload format is still unknown.

**Priority, stated plainly.** A parallel investigation confirmed that the flags
verdict does *not* gate rendering (see `render-gate.md`): the crash address moves
between paths while the verdict stays constant. Both results hold at once — the
flag load really does fail, and that failure really does not block the frame. So
this is a genuine defect worth fixing for correctness, but it is **not** the
render blocker, and it should not be worked before the thread-race deadlock.

## §11. 2026-08-07: two unresolved symbols on the path, both answered, verdict unmoved

Run with `tools/hook_descriptors.py`, a JNI-traced startup, and the engine's own
`FLog` file, which this document had not been using.

### The engine log exists, and it says the flags load

`<profile>/data/files/appData/logs/*.log`. Earlier work looked for it in the
wrong place and concluded none was written; that was wrong. Cordial's `FlagCache`
compresses 1273559 bytes to 366594 and writes them, channel `production`,
tombstone valid. Sober's writes 362169 on the same document. So `onFlagsFailed`
is misleading in the way §7 suspected and now on the engine's own evidence rather
than on a return code.

### Three symbols that could never bind

* `NativeHelper.gameActivity_onFlagsLoaded` — registered `(Ljava/lang/Object;)V`,
  dex declares `(Ljava/nio/ByteBuffer;)V`. **Every prior observation in this
  document that `onFlagsLoaded` "is never resolved or called" was made with this
  bug in place** and cannot distinguish "the engine did not call it" from "it
  could not have been called". §9's reasoning about the success path rests on
  that observation and should be re-taken, not trusted.
* `GameActivity.bootstrapTheApp()V` — the dex declares it on the subclass
  `MainGameActivity`; the engine looks it up on the base
  `com/google/androidgamesdk/GameActivity` and libjnivm walks no superclass
  chain. It was `Constructed Unresolved symbol` on every run this project has
  ever made.
* `java/util/List.size()I` and `get(I)Ljava/lang/Object;` — `JavaList` registered
  on `java/util/ArrayList` only, and the engine asks the interface. The list
  handed to `nativePostClientSettingsLoadedInitialization3` was therefore an
  object whose every method was a stub.

All three are fixed. The sweep is down to one, `getWaterfallInsets`.

### None of them is the cause

`CORDIAL_TRACE_PATHS=1` over a `--run 10` startup: zero paths containing
`rbx-storage`, byte-identical in that respect to the `CORDIAL_NO_BOOTSTRAP=1`
control taken minutes later. `onFlagsFailed` still fires. Delivering in Sober's
order — settings, then post, then flag names, read off its log at 3.700s and
3.796s — did not change it.

### Where the gap now is

Sober's log, immediately after `nativePostClientSettingsLoadedInitialization3`
at 3.796751s: `ClientRunInfo` git hash, base url and channel; then
`AppPlatformQoSEmergencyHandler was instanced`; then the `Mimalloc` block; then
`RbxStorage::init [INIT] user: flagLoaded` at 3.820885s.

Cordial's log contains **none** of those, zero occurrences each, on a run where
the `AndroidGLView` channel is demonstrably open because another line on it
appears. Comparing distinct log channels over the same ten seconds, Sober reaches
ten and Cordial seven; absent are `AppPlatformQoSEmergency`, `KeyRing`,
`Mimalloc`, `NetworkClient`, `RbxStorage` and `RbxTransportDummyClient`.
`NetworkClient` is expected on a startup-only run and `Mimalloc` is explained by
Cordial not linking it. The other four are not explained.

So: Cordial's call to `nativePostClientSettingsLoadedInitialization3` returns
without the engine's own body of it having run. The two symbols it needs on the
way in are now answered and the body still does not run. The one difference left
that this session can point at is the argument — an empty `List` whose `size()`
now truthfully returns 0, where before it returned whatever an unresolved stub
returns. What that list should contain is still unknown, and recovering it means
reading code units rather than declarations, which is out of scope. **That is
the open question, stated as narrowly as the evidence allows.**

### One assertion in the tree corrected

`init_params.cpp` says the real client passes 139 flag names to
`nativeInitializeNativeFlags`, from a Waydroid capture. The capture is not in
doubt, but Sober — which works — logs `flagCount = 0`. Passing 139 is not a
requirement.

### §11.1 The empty `ArrayList` is correct, and 7.4 can be closed

7.4 recorded the list passed to `nativePostClientSettingsLoadedInitialization3`
as unresolved, and §11 above named it as the last difference worth pointing at.
Both are now settled, and the lead dies.

The erased descriptor is `(Ljava/util/List;)V`, which is all the method_ids table
holds and is why two sessions treated the contents as a guess. The dex also
carries the generic signature, in `dalvik.annotation.Signature`:

    com/roblox/engine/jni/NativeGLInterface.nativePostClientSettingsLoadedInitialization3
      (Ljava/util/List<Lcom/roblox/engine/jni/model/ApplicationExitInfoCpp;>;)V

`tools/dex_signature.py` prints it. Two independent confirmations: the identical
signature is on `nativeSetAppPreviousExitReasons`, and a traced startup shows the
engine doing `FindClass com/roblox/engine/jni/model/ApplicationExitInfoCpp`
immediately after this call.

`ApplicationExitInfoCpp` declares three constructors — `(IIJLjava/lang/String;)V`,
`(IJLjava/lang/String;)V`, and a nine-argument form carrying two more strings, two
longs and an int. That is Android's `ApplicationExitInfo`: reason, importance,
timestamp, description, and on the long form process name and more.

**Who populates it:** the app, from
`ActivityManager.getHistoricalProcessExitReasons()`. **What it means empty:** the
previous run recorded no abnormal exit. So Cordial's empty `ArrayList` is not a
placeholder standing in for something unknown — it is the correct value, and
filling it would be telling the engine about crashes that did not happen.

What that leaves: every input to this native is now correct and every symbol on
the way in resolves, and the engine's body of it still does not run — none of the
seven log lines Sober emits from it appear. The remaining gap is inside that
native and this session cannot name it.

### §11.2 `nativeSetAppPreviousExitReasons` — tried, inert, not shipped

It is exported (`0x295cc6c`) and carries the identical
`(Ljava/util/List<Lcom/roblox/engine/jni/model/ApplicationExitInfoCpp;>;)V`, and
Cordial had never called it, which made it the obvious next candidate on this
handshake. Called with the same empty list, before the settings call, on a
`--run 10` startup: it returns cleanly and changes nothing. Zero `RbxStorage`,
zero `ClientRunInfo`, zero `AppPlatformQoS`, zero `[FLog::AndroidGLView] native*`
lines, and zero paths containing `rbx-storage`.

The code is **not** in the tree. A call that reports success and produces no
engine-log line is exactly what this project forbids adding, so it is recorded
here instead of left behind a flag for somebody to find and switch on.

Worth noting for whoever picks this up: the export addresses put
`nativeSetAppPreviousExitReasons` (`0x295cc6c`) next to
`nativeInitClientSettingsSigned` (`0x295c421`), `...Cached` (`0x295c731`) and
`...CachedCompressed` (`0x295c9ad`), while the two natives Cordial actually calls
sit far away at `0x20b6981` and `0x20f2f6d`. That is an observation about layout
and nothing more, but the newer cluster is the one Cordial has never reached.

### §11.3 The strongest form of the remaining question

`[FLog::AndroidGLView] nativeInitClientSettings` appears in Sober's log and never
in Cordial's, and this is not a verbosity difference. Cordial's log does contain
`[FLog::AndroidGLView] rbx.datamodel: setTaskSchedulerBackgroundMode()` at
severity 6 — the same channel at the same level as Sober's line. The log opens at
1.806s and Cordial's third call to that native happens after the Vulkan device
lines at 3.4s, well inside the window.

So: the same channel, the same level, an open log, a call that returns 0, and no
line. Every symbol on the way in resolves and every argument is correct. The
engine's own body of `nativeInitClientSettings` does not appear to execute, and
naming what stops it is where the next session starts.

### §11.4 Retraction: the native's body does run. Four more leads, all dead

**§11.3 above is wrong and is retracted.** It concluded that the engine's own
body of `nativeInitClientSettings` "does not appear to execute", on the strength
of `[FLog::AndroidGLView] nativeInitClientSettings` never appearing. The
reasoning did not survive its own control.

Cordial made that call three times per run, the first before the log file opens
at 1.806s, so the missing line could have been an artifact of ordering. Gating
the pre-`initializeNativeCode` call off and re-running settles it: `FlagCache`
still fires nine times and still writes the document. So the body runs, well
inside the logged window, consumes the 1273559-byte document, and does not emit
its own line. Why it does not is unknown; `FLogAndroidGLView` is absent from the
settings document, so both clients run the compiled-in default and verbosity is
not the difference either. Neither half of §11.3 stands.

Four further leads, each dead, each recorded so it is not re-run:

* **`FFlagStartRbxStorageInitRighAfterFlags=False`.** The premise of this whole
  line is that the store constructs off the flags-loaded event because that flag
  is True. Overriding it to False, which should route construction back to the
  direct call Cordial already makes, applies cleanly (`1 override(s) applied`)
  and produces no `RbxStorage` line and no `rbx-storage` path. Storage does not
  construct on the other path either.
* **`nativeSetAppPreviousExitReasons`** — §11.2, inert.
* **`nativeRegisterJavaFlagProvider`** — exported by the engine and never called
  by Cordial, which made it look like the missing registration step. It is **not
  declared anywhere in the dex**: no Java class in this APK has a counterpart for
  it, so it is not on any path this build takes. A tempting name is not a lead.
* **The natives Cordial does not call.** The engine exports 91 on
  `NativeGLInterface`, `MainGameActivity` and `FlagJniInterface` together;
  `load.rs` calls 42. The 49 it does not are lifecycle (`PauseApp`, `LeaveGame`),
  purchase, VR, text-box and the three unused client-settings variants. Nothing
  in that list is a plausible prerequisite for the settings handshake.

**The one divergence left that is not explained.** Sober logs
`nativeInitializeNativeFlags: Registered Flag Provider ID from Java: 0`.
No Cordial run in this session logs it, and in its place Cordial emits the same
`JNIRobloxSettings nativeInitializeNativeFlags:` prefix with an empty message,
while every other line on that tag formats correctly (`... 0: <name> not
found.`). Whether that empty line is the provider-registration message arriving
without its value, or Cordial's `__android_log_print` shim dropping a format it
does not handle, is **not established** — and the distinction matters, because
one is an engine-state difference and the other is a logging bug in Cordial.
That is the next thing to settle, and it is one instrumented run away.

### §11.5 Retraction: the flag-provider divergence was Cordial's own log filter

§11.4 closed by naming one unexplained divergence — Sober logs
`nativeInitializeNativeFlags: Registered Flag Provider ID from Java: 0` and no
Cordial run does. **That is wrong and is retracted.** It also described an "empty
message" line in its place, which never existed; that was an artifact of the
`grep -o` pattern used to look for it, not of any run.

Sober's line is at `debug:` priority. `native/liblog.cpp`'s `minimum_priority()`
defaults to `ANDROID_LOG_INFO` and drops everything below it, so Cordial had been
discarding the line before it reached a terminal. `CORDIAL_LOG_LEVEL=d` on the
same build:

    Registered Flag Provider ID from Java: 0
    flagCount = 139.
    Registered Flag Provider ID from Java: 1
    flagCount = 139.

Cordial registers a flag provider, exactly as Sober does.

**The general warning, which is worth more than the retraction.** Sober's log
file contains `debug:` lines; Cordial's stderr, at its default level, does not.
Any comparison between the two that concludes something from an *absence* on the
Cordial side is invalid unless it was taken at `CORDIAL_LOG_LEVEL=d`. Two
conclusions in this session were drawn that way and one of them survived only
because it was tested.

This does not touch the `RbxStorage`, `ClientRunInfo` or `AppPlatformQoS`
absences in §11 and §11.4. Those were read from the engine's own `FLog` file in
`<profile>/data/files/appData/logs`, which the engine writes directly and which
`liblog.cpp` does not filter.

One real difference does fall out of the debug run: Cordial registers **three**
flag providers per launch, IDs 0, 1 and 2, because it calls
`nativeInitializeNativeFlags` three times — the early call, `bootstrapTheApp`,
and the original post-init block. Sober registers one. Whether repeated
registration is harmless is not established.

### §11.6 One delivery, not three. Measured, and still not the fix

The debug run in §11.5 showed Cordial registering flag providers 0, 1 and 2 on a
single launch where Sober registers 0 and stops. Three deliveries: the pre-init
call, `bootstrapTheApp`, and the original post-init block. A fourth would have
followed, because the engine calls `bootstrapTheApp` **twice** per launch — two
`Call Member Function ... bootstrapTheApp ()V` in the trace.

Now one. The pre-init call runs only under `CORDIAL_NO_BOOTSTRAP=1`, the
post-init block is skipped when the bootstrap delivered, and `run_bootstrap`
swaps a flag so the engine's second call is a no-op. `Registered Flag Provider
ID from Java: 0`, once, matching Sober exactly.

It is not the fix. Same run: zero `RbxStorage` in the engine log, zero
`rbx-storage` paths, `onFlagsFailed` still reported.

One thing did move, and it is recorded because it is the only quantity in this
whole session that responded to anything: `onFlagsFailed` is reported **twice**
with the single delivery and **four times** in the `CORDIAL_NO_BOOTSTRAP=1`
control taken minutes later. So the verdict is reported once per delivery
attempt, not once per launch. That is consistent with the verdict being a
property of each attempt rather than a latched startup state, which is new, and
nobody should read more into it than that.

### §11.7 The ordering, read file-to-file: Cordial starts the app bridge first

§11 and §11.4 compared Cordial's `FLog` file against Sober's *captured stdout*,
which after §11.5 is not a comparison anyone should trust. Sober writes its own
`FLog` file, `appData/logs/<version>_<ts>_Player_<id>_last.log`, the same sink in
the same format from the same build. Redone file-to-file, the earlier reading
survives: Sober's file carries all seven lines (`AndroidGLView`
`nativeInitClientSettings` ×1, `ClientRunInfo` ×3, `RbxStorage` ×2,
`AppPlatformQoSEmergency` ×1, `Mimalloc` ×43) and Cordial's carries none.

What the file-to-file view adds is the sequence, and it is not what this document
has assumed.

| | Sober | Cordial |
|---|---|---|
| log opens | 1.652 `RobloxChannel has been set to production` | 1.781 |
| | *engine silent for 2.05 s* | |
| `nativeInitClientSettings` | 3.700 | never logged |
| `nativePostClientSettingsLoadedInitialization3` | 3.796 | never logged |
| `RbxStorage::init [INIT] user: flagLoaded` | 3.820 | never |
| `nativeAppBridgeV2Init` | **3.901** | **1.781 — the first line in the file** |
| `initializeWithAppStarter` | 3.906 | 1.781 |
| `InitializedLuaApp` | — | 3.102 |

**Sober brings the app bridge up 200 ms after the content store. Cordial brings
it up first, and it is the very first thing the engine logs.** Sober's engine
does nothing at all between 1.652 s and 3.700 s: it is waiting for the host
application to hand it settings, and only starts once it has them. Cordial's
engine is already into `nativeAppBridgeV2Init`, `initializeWithAppStarter` and
`InitializedLuaApp` while that handshake is still going on.

Channels Sober reaches and Cordial never does, whole file against whole file:
`AppPlatformQoSEmergency`, `KeyRing`, `Mimalloc`, `RbxStorage`, plus
`AssetProvider`, `NetworkClient`, `RbxTransportDummyClient`,
`RbxTransportRnaExpConnection` and `TrackerAnimationStreamSourceTrace`, which are
join-time and expected absent from a startup-only run. `Mimalloc` is explained by
Cordial not linking it. `AppPlatformQoSEmergency`, `KeyRing` and `RbxStorage` all
sit inside the window Cordial skips past.

**This is the named divergence, and it is an ordering one, not a missing
symbol.** Every symbol on the path now resolves and every argument is now known
correct; what differs is when Cordial does the handshake relative to starting the
engine's application layer. The next experiment is to deliver settings after
`nativeAppBridgeV2InitWithParams` rather than inside `initializeNativeCode`, and
watch for `RbxStorage::init`. That is a reordering of `load.rs`, it is not
attempted here, and it should be done with the `--run 8` startup and a control.

### §11.8 The reordering §11.7 proposed was run. It crashes

`CORDIAL_LATE_SETTINGS=1` moves the whole handshake out of
`initializeNativeCode` and into Sober's position, after
`nativeAppBridgeV2InitWithParams`. Two runs, both `--run 8`:

| | SIGSEGV | reached the app bridge |
|---|---|---|
| `CORDIAL_LATE_SETTINGS=1` | 2 of 2 | no |
| default (`bootstrapTheApp` delivers) | 0 of 2 | yes |
| `CORDIAL_NO_BOOTSTRAP=1` | 0 of 1 | yes |

The engine dies before the bridge is reached, so the late delivery never runs at
all. **Cordial cannot adopt Sober's ordering by moving the call.** Sober's engine
can afford to sit idle from 1.652 s to 3.700 s because the Kotlin activity
lifecycle is what will eventually hand it settings; Cordial drives the natives
directly and by that point has advanced past the state in which they can arrive.

So §11.7's ordering observation stands as a description and dies as a fix. The
difference is real and it is not something a reordering of `load.rs` can close.
Whatever Sober's engine is waiting for during those two seconds, Cordial never
enters that state — and identifying *that* state, rather than any further symbol,
is what the next session should chase.

The gate is left in `load.rs` with the crash recorded in the comment beside it,
on the same reasoning 7.4 kept `CORDIAL_TRY_POST_CLIENT_SETTINGS`: the experiment
has a result and the next person should not have to rebuild it to find out.

### §11.9 There is no state. Those two seconds are the application's, not the engine's

§11.8 ended by saying Sober's engine "spends two seconds in a state Cordial never
enters" and that identifying that state was the next thing to chase. There is no
such state, and that closing sentence is retracted.

The two log lines either side of the gap carry the same thread id:

    1.652055,b25302c0,6 [FLog::Output] RobloxChannel has been set to production
    3.700769,b25302c0,6 [FLog::AndroidGLView] nativeInitClientSettings

Same thread, and the engine emits nothing at all in between. So it did not block
inside a call — it **returned to the host application**, and the application
called back in two seconds later. The only entry in the whole window is Sober's
own launcher, not the engine:

    info: state: Applying remote app settings override: FStringRenderTextureBudgetByRam=""

Sober's lifecycle log accounts for the time in its own terms: `fs_init` 1132 ms,
`devices_init` 1716 ms, `gamemode_init` 1409 ms, `app_core` 669 ms,
`check_security` 465 ms, `runtime_handler` 716 ms. That is a launcher doing
launcher work between two engine calls.

Cordial's equivalent stretch is short because Cordial's work is short: the
settings document is already on disk under a six-hour cache, there is no security
check, no gamemode integration and no device enumeration of that weight. The
compressed timeline in §11.7's table is Cordial being faster, not Cordial
skipping a phase. Nothing in the gap is a prerequisite the engine is waiting on,
which is also why §11.8's reordering could not work and should not have been
expected to.

One detail worth carrying, unrelated to the verdict: Sober applies its **own**
`app_settings` manifest on top of Roblox's document —
`{"app_settings":{"FStringRenderTextureBudgetByRam":""}, ...}` — so the two
clients are not running byte-identical flag sets even when they fetch the same
document. Cordial applies no such overlay. That has not been tested against the
verdict and is recorded only so nobody assumes the flag sets match exactly.

### §11.10 Sober's `app_settings` overlay — applied, no effect

§11.9 recorded that Sober applies its own `app_settings` manifest on top of
Roblox's document, so the two clients do not run identical flag sets. Tested,
because "untested" is not a place to leave something that was raised as a
difference.

Sober's manifest carries exactly one entry: `FStringRenderTextureBudgetByRam=""`.
Given to Cordial through `CORDIAL_FLAGS`, `--run 10` startup, against a control
run taken immediately afterwards on the same build:

| | overrides applied | `RbxStorage` | `onFlagsFailed` |
|---|---|---|---|
| with Sober's overlay | 1 | 0 | 2 |
| control, no overlay | 0 | 0 | 2 |

Identical. The overlay is not the difference, and the flag-set discrepancy noted
in §11.9 can be closed: it is one empty `FString` about a render texture budget
and it has nothing to do with the flags verdict or the content store.
