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

### §11.11 A join, after all of the above: 304 at 60.6 s, unchanged

Everything from §11 onwards was startup-only. One instrumented join on the test
profile, after all of it, to answer whether any of it moved the disconnect:

    RESULT postfix server=128.116.50.33 alive=60.6s reason=304 (connections: 1)

Squarely inside the 60.1–60.9 s band recorded across twelve-plus earlier runs.
**Nothing in §11 is a fix for the 304**, which was never claimed but is now
measured rather than assumed. The client is healthy at the moment it is dropped:
`Connection lost: AckTimeout 0, IsOutgoingDataWaiting 1`.

In the same run: `bootstrapTheApp` delivered once, `onFlagsFailed` twice,
`FlagCache` wrote, and `KeyRing` logged two parsed configs — so `KeyRing`, listed
in §11.7 as a channel Cordial never reaches, is simply join-time and is reached
normally. That entry in §11.7's list is wrong and is corrected here.

The store is still not constructed, and for the first time the engine says so in
its own words rather than by absence:

    8.486503 Error [DFLog::CaptureStorage] RbxStorage is not initialized,
                                           cannot access storage interface

Twice, at 8.49 s, on the join path. There is no `rbx-storage.db` under the
profile. So the picture from §11 holds — the store never initialises — and there
is now a named consumer that wanted it and was refused, which is a better handle
than an absent log line.

A caution on how that was nearly misread: `grep -c RbxStorage` on this log
returns 2, and the obvious reading is that the store initialised on a join when
it does not on a startup. It is the opposite; both matches are the error above.
Count then read, in that order.

## §12. What is blocking `RbxStorage::init`: nothing is. It is never asked for

Chased from the `CaptureStorage` error in §11.11. The conclusion is that the
question "what is blocking storage init" has no answer because storage init is
not blocked.

**There is no way to initialise it directly.** `RbxStorage` is engine-internal.
The engine exports `LocalStorageManager_initStorageManagerNative`, `...V3`, the
`memstorage` family and the `localstorageplatforminterface` family — all of which
are *LocalStorage*, a different thing that Cordial already has working
(`appData/LocalStorage/*.json` is populated). Nothing exported, and nothing in the
dex, constructs `RbxStorage`. There is no handle to pull.

**Its only trigger is the flags-loaded event.** `FFlagStartRbxStorageInitRighAfterFlags
= True` in the live set, and Sober's own log names the trigger in the line
itself: `RbxStorage::init [INIT] user: flagLoaded`.

**The routing flags were tried and do nothing.** `FFlagRbxStorageUseStdThread`,
`RunInitInStdThreadLatch`, `BackgroundThread` and `SynchronizeInit2` all False,
plus `StartRbxStorageInitRighAfterFlags` False — seven overrides applied, zero
`RbxStorage` lines, zero `rbx-storage` paths.

**And the overrides are real, which had never been established.** Static flags
were assumed to land and were not tested. `FLogGraphics=0` takes
`[FLog::Graphics]` from **30 lines to 0** on the same build. Static and dynamic
overrides both reach the engine, so every flag experiment in §11 and here is a
genuine negative rather than a no-op. The "settings arrive too late for static
flags" theory is dead.

**The engine says nothing about it at any verbosity.** All 134 `FLog*`/`DFLog*`
keys in the document set to 7: the engine log goes from ~220 lines to **6247**,
reaching 31 distinct channels, and contains zero lines matching `RbxStorage`,
`InitBlocked`, `flagLoaded` or the verdict. `FFlagRbxStorageReportInitBlocked` is
True in the live set and never fires. Storage is not failing to initialise; it is
not being asked to.

### §12.1 `NativeFlagsInitResult` — three methods implemented and never registered

Found while chasing the above, fixed, and **not** the cause. The dex declares
five members; Cordial registered two:

    <init>                   (I)V                        registered
    addBoolean               (Ljava/lang/String;ZZ)V     registered
    getNativeFlagProviderId  ()I                         written, not registered
    getBooleanCachedMap      ()Ljava/util/Map;           written, not registered
    resolveFlagValue         (Ljava/lang/String;)Z       written, not registered

All three had working C++ bodies sitting in the class. `Register()` never hooked
them, so the object answered its constructor and `addBoolean` and returned an
unresolved stub to every question about what it had stored. **This is a third
variant of the silent-hook bug**, after the wrong descriptor and the wrong class:
an implemented method that is never registered. It does not show up in a grep for
the method name, which is why it survived the audit in 2ca7811 that was looking
for precisely this, and `tools/hook_descriptors.py` cannot see it either because
it only checks hooks that exist.

Registered now. It changes nothing, and the trace says why: across a whole
startup the engine calls `<init>` once and `addBoolean` **138 times** and never
looks up any of the three. It builds the result object, fills it, and never reads
it back.

### §12.2 Where that leaves the verdict

The flag handshake completes on Cordial's side. 138 of 139 names accepted, the
flag cache written, static and dynamic overrides demonstrably in effect. The
engine then reports `onFlagsFailed` over JNI without having asked Cordial a single
further question — no unresolved lookup, no failed call, no log line at any
verbosity. Every symbol on the path resolves and every argument is correct.

So the verdict is not a reaction to anything Cordial does or fails to do at the
JNI boundary. That is a much narrower statement than this document has been able
to make before, and it rules out the entire class of fix it has been pursuing
since §7.

## §13. mocktail does not take the 304, and the difference is visible

The experiment that had been deferred for two days. mocktail 1.0.3, Flatpak,
signed in and joined to place 17625359962 — the same place Cordial dies in.

    Joining game ... place 17625359962 at 10.60.0.203     84.048s
    Connection accepted from 128.116.51.33|61655          84.102s
    last engine timestamp                                333.512s
    Disconnect / Peer Disconnected / Connection lost            0

**249 seconds connected and still running**, against Cordial's 60.6. So the 304
is not something every third-party client gets, not a property of the engine on
Linux, and not unavoidable. A working comparison now exists.

### The startup difference, counted on the same place and the same day

| | mocktail | Cordial |
|---|---|---|
| `onFlagsFailed` | **0** | 2 |
| `RbxStorage::init` | **[INIT] 0.164s, [DONE] 1.037s** | never (only "not initialized" errors) |
| `ClientRunInfo` | **3** | 0 |
| `AppPlatformQoS` | **1** | 0 |
| 304 | **no** | at 60.6s |

This is the chain ad985a8 proposed, now with the other side of it. The client
that raises the flags-loaded event builds its content store and keeps its
connection; the client that reports `onFlagsFailed` does neither.

mocktail's order, which Cordial reproduces up to the third line and then stops:

    0.139  nativeInitClientSettings
    0.155  [FlagCache] Deferring flag cache write to post TTI
    0.158  nativePostClientSettingsLoadedInitialization3
    0.158  [ClientRunInfo] RobloxGitHash: 9141bfb7...
    0.158  [ClientRunInfo] The base url is https://www.roblox.com/
    0.158  [ClientRunInfo] The channel is production
    0.159  AppPlatformQoSEmergencyHandler was instanced
    0.164  RbxStorage::init [INIT] user: flagLoaded

**`ClientRunInfo` is the first thing Cordial does not reach.** It is the engine
stating its own run identity — git hash, base url, channel — immediately after
the post-settings call, and six milliseconds before the store is built. Cordial
makes both calls, gets 0 and "ok" from them, and produces none of these lines.

### Two things mocktail does that Cordial does not do at all

Neither is established as the cause. Both are recorded because they are
differences between a client that works and one that does not, which is a much
better position than this document has been in.

**It presents as a PC, not a phone.** `src/runtime/device_profile.cc` reports
`device profile=pc-windows-11 class=pc model="Windows 11 PC"`. Cordial tells the
engine it is an Android tablet — 6d8c280 built a User-Agent saying exactly that,
on the reasoning that the real Android client sends it. mocktail's choice is the
opposite one and mocktail is the client that survives.

**It bootstraps a tracker identity before the engine starts.**
`src/services/browser_tracker_service.cc` calls
`https://apis.roblox.com/browser-tracker-api/device/initialize?suggestedBrowserTrackerId=`
and keeps the `RBXEventTrackerV2` cookie it returns. Cordial has no equivalent
and no such cookie.

This was already half-written down here and never connected to anything:
`docs/analysis/webview-surface.md` records that `libroblox.so` carries the string
`BrowserTrackerIdRequest: No RBXEventTrackerV2 in cookie.`, and
`docs/design/sign-in.md` has the same endpoint in an Android capture. So the
engine has a code path that notices this cookie missing, and Cordial has never
supplied it.

**Not evidence, and worth stating so nobody quotes it as such:** that log string
did **not** appear in Cordial's join log, and the only BrowserTracker line in
mocktail's log is mocktail's own launcher rather than the engine. Neither engine
said anything about a tracker cookie at default verbosity. The string's presence
in the binary shows the check exists; it does not show it fired.

### What to do next, in order

1. **Raise the log level and rerun the Cordial join.** §12 established that flag
   overrides reach the engine, so the channels around `ClientRunInfo`,
   `BrowserTrackerIdRequest` and `AppPlatformQoS` can be turned up. Find out
   whether the engine says anything about the tracker cookie when it is missing.
2. **Supply `RBXEventTrackerV2`.** Cordial owns the cookie jar already
   (`crates/cordial-runtime/src/cookies.rs`) and the endpoint is a documented,
   ordinary HTTPS request the real client makes. Nothing here forges or replays
   anything.
3. **Try the PC device profile.** Cheap, reversible, and the client that works
   uses it.

Take them one at a time with a control each. Three changes at once against a
failure that takes sixty seconds to reproduce would tell us nothing.

### §13.1 The tracker cookie comes through the WebView, not from an API call

Tried, and it does not work the way §13 assumed.

`browser-tracker-api/device/initialize` refuses a plain request. `GET` answers
**404**, which is a route that does not exist. `POST` with mocktail's own headers
— `Accept: application/json`, `Content-Type: application/json`, body `{}` —
answers **500** with an empty body, unchanged whether the User-Agent says
`Mocktail/0.1` or `Cordial/0.5`. So the method is right and something else about
the request is not.

`docs/design/sign-in.md` already recorded why, from a logged-out Android capture:

    Flushed WebViewCookieHandler with Cookies from URL
      https://apis.roblox.com/browser-tracker-api/device/initialize
    OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler with Cookies from URL
      https://apis.roblox.com/browser-tracker-api/device/initialize

**`WebViewCookieHandler`.** The real client obtains this cookie inside its web
view, during the logged-out sign-in flow, with a browser's full context behind
the request. It is not a standalone API call, and mocktail's succeeds because
mocktail signs in through its web view first.

That joins two gaps that were being tracked separately:

    no WebView -> no sign-in in a browser context -> no RBXEventTrackerV2
                                                  -> no device identity

Which raises the WebView from "account settings and Robux do not work" to a
possible prerequisite for the session surviving at all. **Still `INFERRED` —
nothing here shows the missing cookie causes the 304.** But the ordering is now
the right way round to test: build the web window, sign in through it, and see
whether the cookie arrives on its own.

`crates/cordial-runtime/src/browser_tracker.rs` is written and tested and
**deliberately not called from anywhere**. Wiring in a request that returns 500
would put a line in the log saying Cordial had done something it had not.

## §14. The 304 no longer reproduces, and the device profile is not why

Four join runs on 2026-08-18, place 17625359962, profile CordialTest, 90 seconds
each:

| run | identity | result |
|---|---|---|
| pc | `CORDIAL_DEVICE_PROFILE=pc-windows-11` | connected 1.77s, last log 90.70s, **0 disconnects** |
| control-1 | default android-tablet | still connected at exit |
| control-2 | default android-tablet | still connected at exit |
| control-3 | default android-tablet | still connected at exit |

Every one survived. Against twelve-plus runs that died at 60.1–60.9s with reason
304, the disconnect is gone.

**The control is the point.** The PC identity run came first and looked like the
answer; three runs with the identity left at its Android default did exactly the
same thing. So `CORDIAL_DEVICE_PROFILE` is not what changed, and crediting it
would have been the easiest wrong conclusion available today.

### What actually changed is not established

Two candidates, and nothing here separates them:

**Roblox shipped a new engine.** Every 304 was measured on 2.730.0.790. The APK
updated mid-session and all four of these runs are 2.734.0.917. A server-side
disconnect that stops happening across a client update is exactly what a
server-side change looks like from here.

**Or one of the day's changes.** `hypotf` (which the new build needs to load at
all), the texture-manager default, the webview subscription, the MessageBus
generalisation, the refresh-rate calls, the plugin host work.

The engine update is the stronger candidate on timing alone, and it is also the
one Cordial cannot take credit for. **Nothing in this document should be read as
"the 304 was fixed".** It is not reproducing, on this build, on this day, on four
runs.

### What did not change

    RbxStorage::init=0   ClientRunInfo=0   onFlagsFailed=2   webview-open=0

Identical in all four. So the chain §13 established is untouched: the engine
still reports `onFlagsFailed`, still never builds its content store, still never
prints `ClientRunInfo`. Whatever stopped the disconnect did not fix that, and
§13's correlation between the flags verdict and the 304 is weakened by exactly
this — mocktail had the storage *and* stayed connected; Cordial now stays
connected *without* it.

The honest reading is that the two were never as tightly coupled as the
correlation suggested, and that the storage chain is worth pursuing on its own
merits — a client with no content store re-fetches every asset, which is a real
cost whether or not it ever caused a disconnect.

### Before this is quoted anywhere

Run it again on a different day, and if the 304 returns, the engine update was a
reprieve rather than a fix. `tools/join-run.sh` exists so that is one command.

## §15. Two answers read out of mocktail's source, one of which kills a theory

### The empty `ArrayList` was never the problem

`flag-init.md` §7.4 has recorded since it was written that Cordial passes
`nativePostClientSettingsLoadedInitialization3` an empty `java.util.ArrayList`
and that this is unresolved — the implication being that a real list is what the
engine wants and that the empty one is why nothing follows.

mocktail's `BuildApplicationExitInfoList`, which is what it passes to that same
native:

    jobject BuildApplicationExitInfoList(JNIEnv* env) {
      jobject list = NewObject(env, "java/util/ArrayList");
      if (!list) list = NewObject(env, "java/util/List");
      return list;
    }

**An empty `ArrayList`.** The same thing Cordial passes. And mocktail reaches
`RbxStorage::init [INIT] user: flagLoaded` 6 ms later.

So the argument is not the difference, and §7.4's open question is closed as a
dead end rather than left to be re-investigated. The list's real element type is
`ApplicationExitInfo` — Android's historical process exit reasons — and the
engine evidently does not require any.

### `channelPlatformName` was carrying the wrong string

Cordial passed `AndroidApp` to
`NativeSettingsInterface.nativeOverrideChannelPlatformName`. mocktail passes
`GoogleAndroidApp`. Counted rather than argued, with exact whole-string matches:

| literal | in `libroblox.so` | in the dex |
|---|---|---|
| `AndroidApp` | 3 | 0 |
| `GoogleAndroidApp` | 0 | 2 |

Two strings doing two jobs. `AndroidApp` is the application name in the settings
URL — `v2/settings/application/AndroidApp` serves the real document and the other
spellings return HTTP 400, which is established by experiment and still true.
`GoogleAndroidApp` is what the *application* calls its channel platform, lives in
the dex where the Java side is, and is what that native wants. The two were
conflated and this call had the other's value.

Corrected. **It changes nothing measurable**, which is recorded here so nobody
tries it again expecting more:

    RESULT gplat alive=still connected reason=none
      RbxStorage::init=0 ClientRunInfo=0 onFlagsFailed=2 webview-open=0

Identical to the four runs in §14. A correctness fix with a null result is still
worth having — the value is now the one the dex declares rather than one that
appears nowhere as a channel platform name — but it is not the storage fix.

### Where that leaves the storage question

Both of the concrete differences visible in mocktail's startup path have now been
tried, and neither moves `onFlagsFailed`. The verdict is still reached inside
`initializeNativeCode`, before the settings calls, exactly as §12 measured. What
mocktail does differently to reach `flagLoaded` is still not identified, and it
is not either of these.

## §16. mocktail's startup path, read call by call: the gap is not a missing call

The question was why mocktail has no storage gap. Its whole pre-settings path
has now been read, and the answer is not in the shape this investigation assumed.

**Between `initializeNativeCode` and the settings call, mocktail makes five
calls.** Cordial makes four of them:

| call | Cordial |
|---|---|
| `nativeSetAssetPath` | yes |
| NativeSettings directories | yes |
| `JNIBaseUrlProtocol.init` | yes |
| `nativeGameGlobalInit` | yes |
| `nativeUpdateScreenOrientation` | **no** |

One real gap, and it is a screen-orientation notification rather than anything
that plausibly gates a content store. Worth closing on its own; not this.

**And Cordial calls far more of the engine than mocktail does.** Counting the
`Java_*` symbols each names: mocktail 17, Cordial 54. So the storage gap cannot
be explained by Cordial failing to make a call mocktail makes — the set is the
other way round.

That inverts the hypothesis worth testing next: not a missing call but an
**extra** one, something Cordial does that provokes the verdict. The obvious
candidate was Cordial's early `nativeInitClientSettings`, made before
`initializeNativeCode`, which mocktail does not do — added when the first
`flags FAILED` was seen arriving before any settings call.

**That candidate is dead too.** The call is gated behind `CORDIAL_NO_BOOTSTRAP`
and does not fire in a default run at all; a default run's log contains no
`early client settings` line. So it is not a difference between the two clients
in normal operation, and an earlier reading of that line in this document came
from a traced run with that variable set.

### What has been ruled out, so nobody re-runs it

- the empty `ArrayList` to `nativePostClientSettingsLoadedInitialization3`
  (§15 — mocktail passes an empty one too)
- `channelPlatformName` (§15 — corrected to the dex's value, no effect)
- the device identity (§14 — three controls)
- every flag routing `RbxStorage`'s construction (§12)
- the settings document, in four variants (§7, §11)
- a missing call in mocktail's pre-settings path (this section)
- Cordial's early settings call (this section — it does not run)

**The honest state: the difference is still unidentified.** It is not any of the
things that were visible from the outside, and reading mocktail's source has
removed candidates rather than supplied the answer. That is progress of the
cheaper kind, and it is worth having written down before somebody spends another
day re-testing the same seven things.

## §17. The premise behind Cordial's bootstrap was wrong, and §11.8's crash is the real lead

mocktail's pre-`initializeNativeCode` stretch has now been read — the last part of
its startup nobody had looked at. It does not contain the answer §16 was hoping
for. It contains something more useful: evidence that a theory this project built
on is not true.

### `bootstrapTheApp` does not have to deliver anything

mocktail's implementation, `src/jnivm/jnivm.cc:1702`, in full:

    if (std::strcmp(name, "bootstrapTheApp") == 0) {
      SetBooleanFieldRaw(obj, "bootstrapStarted", JNI_TRUE);
      ... log ...
      return;
    }

**One boolean. Nothing else.** And mocktail reaches
`RbxStorage::init [INIT] user: flagLoaded` regardless.

Cordial's `run_bootstrap` exists on the theory that an unresolved
`bootstrapTheApp` causes an immediate `onFlagsFailed`. §7 established that for the
*unresolved-symbol* case, which is real. What is now clear is that the converse
does not follow: a resolved-but-empty `bootstrapTheApp` does not reproduce the
failure for mocktail, so "deliver enough through bootstrap" was never the shape of
the fix.

### Which puts §11.8's abandoned crash back at the front

mocktail delivers client settings **after `initializeNativeCode` returns**,
sequentially, the ordinary way. Cordial tried exactly that — `CORDIAL_LATE_SETTINGS=1`
— and **crashed twice out of two**, which is why it delivers early instead. §11.8
records the crash and says plainly it was never root-caused.

So the open question is not "what does mocktail do before `initializeNativeCode`".
It is **why does Cordial segfault doing what mocktail does after it**. The client
that works runs through the ordering that kills ours, and that crash was set aside
rather than understood.

**And the crash is worth re-testing before anything else.** It was recorded on
2.730.0.790. The engine is now 2.734.0.917 — a build that also needed `hypotf`
before it would load at all. A crash on an engine two versions old is not evidence
about this one.

    CORDIAL_LATE_SETTINGS=1 tools/join-run.sh late

That is the next experiment, and it is one command.

### Also untried, cheaper, lower odds

Cordial's `Configuration` object is registered and populated with nothing
(`native/game_activity.cpp:128`). It is handed to the same `initializeNativeCode`
call whose next line decides the verdict. mocktail's `CreateAndroidConfiguration`
fills fifteen fields — orientation, touchscreen, keyboard, densityDpi, screen
dimensions, layout, uiMode, colorMode, mcc/mnc, navigation, fontWeightAdjustment.

Never examined; not among §16's seven. **Plausibility honestly moderate-to-low** —
AGDK usually derives its internal `AConfiguration` from the `AssetManager` rather
than reading this object's fields, so it may be inert. Cheap to try, and if it
changes nothing, populate only `orientation` and `touchscreen` rather than
restoring a whole struct of guesses.

### Closed

BrowserTracker is non-fatal in mocktail too — `src/main.cc:711` logs the failure
and continues. It structurally cannot be a hard gate on either side, which
confirms §13.1's own caution and settles that it should stay unwired while the
endpoint returns 500.

### Recorded as better, so nobody "fixes" it

Cordial's `AssetManager`, `Configuration` and window-insets classes are
deliberately stateless with a documented reason each. mocktail populates its Java
objects with synthetic hardware descriptions it cannot verify — `mcc`/`mnc` zero,
`colorMode` zero, `navigation` one, guesses throughout. Keep the minimalism unless
the experiment above proves a specific field load-bearing.

## §18. `__sF` is unfinished, and the signature matches a crash already on record

A comparison of Cordial's bionic/glibc boundary against mocktail's found three
real ABI defects. One of them may be a crash this document has been carrying
since §7.4.

### The three, all verified against the engine's own imports

**`__sF` — the legacy `stdin`/`stdout`/`stderr` array.** `bionic/mod.rs:97`
supplies a zeroed three-element array and its own comment already says this stops
a load-time crash and "does not make the legacy streams work". What it does not
say is what happens next. The engine imports **ten** FILE-taking stdio
functions — `fflush`, `fwrite`, `fread`, `fclose`, `fprintf`, `fputs`, `fseek`,
`ftell`, `setvbuf`, `vfprintf` — and every one is unoverridden passthrough to
host glibc. So a `FILE*` the engine computes as `&__sF[1]` reaches glibc's
`fwrite` or `fflush` and is dereferenced as a real `FILE` against **zeroed
memory**. That is not "no output"; it is a fault at a small offset.

**`mallinfo` — a 40-byte struct filled into an 80-byte expectation.** bionic's
`struct mallinfo` is ten `size_t`; this host's glibc is ten `int`, confirmed by
compiling `sizeof(struct mallinfo)` here and getting **40**. `mallinfo` is
imported by the engine and passes straight through, so the callee writes 40 bytes
of int-strided fields and the caller reads them at 8-byte strides. Every field
after the first is misaligned, and the upper half is never written at all.

**`__cxa_thread_atexit_impl` — a stub that reports success.** Imported by the
engine, no override, so it falls to the generated stub and returns 0. Every
bionic-compiled `thread_local` with a non-trivial destructor is registered
nowhere and never torn down. This is precisely the shape AGENTS.md singles out
`__assert2` for: an answer that is not true, and a failure that surfaces
somewhere unrelated.

### The connection worth testing first

§7.4 records `nativePostClientSettingsLoadedInitialization3` crashing
**synchronously, under lldb, with `SIGSEGV` at fault address `0x8`, inside
`libc.so.6` `_IO_fflush`**.

A zeroed `FILE` handed to `_IO_fflush`, faulting on a pointer field a few bytes
in, produces exactly that. `fflush` is one of the ten the engine imports.

**This is a signature match, not a demonstration.** Nothing here has reproduced
the crash with the `__sF` gap closed and watched it go away, and that is the only
thing that would settle it. But it is a specific, mechanical account of a crash
that has been described as unexplained for weeks, and it costs one experiment.

It matters beyond §7.4 because §17 identified §11.8's `CORDIAL_LATE_SETTINGS`
crash — Cordial segfaulting while doing what mocktail does successfully — as the
strongest remaining lead on the flags verdict. **If both crashes are this bug,
the ordering mocktail uses becomes available to Cordial**, and the thing §17 said
to investigate becomes the thing to fix.

### Order of work

1. Translate the legacy streams. mocktail's `bionic_stdio_runtime.cc` checks each
   FILE-taking entry point and redirects the three `__sF` slots to real
   `stdin`/`stdout`/`stderr`. Roughly 150–200 lines of the same shape here, and
   only for the entry points the engine actually imports.
2. Re-run `nativePostClientSettingsLoadedInitialization3` and see whether the
   `_IO_fflush` crash is gone. That is the test of the whole theory.
3. Then `CORDIAL_LATE_SETTINGS=1`, which §17 wants re-tested on 2.734.0.917
   anyway.
4. `mallinfo` and `__cxa_thread_atexit_impl` independently — each is small,
   neither depends on the above.

### And one thing not to change

`native/netdb_compat.cpp` translates bionic's `AI_*` bits before calling host
`getaddrinfo`, because bionic's `AI_DEFAULT` is `0x600` and handing that to glibc
returns `EAI_BADFLAGS` and fails every lookup. mocktail's `HostHints()` copies
`ai_flags` **unmodified** — the same latent bug, not yet triggered by whatever
its callers pass. Cordial is right here and mocktail is not; nobody should
simplify that file to match theirs.

### §18.1 The same crash, now from a third unrelated call

§18 proposed that `__sF`'s zeroed `FILE` array explains §7.4's `SIGSEGV` at
fault address `0x8` inside `libc.so.6` `_IO_fflush`, and called it a signature
match rather than a demonstration.

Wiring `ILocalStorageHandlerCore.setPlatformImpl` produced **the same crash
again** — same function, same fault address — from a call that has nothing to do
with client settings. Controlled both ways on this machine:

    gate off  exit 0    reaches app ready: Landing
    gate on   exit 139  SIGSEGV

That is three independent paths now: `nativePostClientSettingsLoadedInitialization3`
(§7.4), `CORDIAL_LATE_SETTINGS` (§11.8), and `setPlatformImpl`. Three unrelated
natives cannot share a fault address by coincidence, and the `__sF` gap predicts
exactly this shape for any engine path that touches a legacy stream.

**It is still not demonstrated.** The demonstration is closing the gap and
watching all three stop, and nobody has done that. But §18's order of work now
has three reproducers to test against instead of one, and the cheapest of them is
a one-variable environment flip rather than a join.

One thing seen alongside and *not* explained: with the gate on, the engine's own
djinni glue throws `djinni_support.cpp:529: weakRef` a dozen times before the
segfault. `IPlatformLocalStorageHandler` is djinni-generated — the `$CppProxy`
siblings give it away — so djinni plausibly wants working weak global references
that libjnivm does not provide. **`INFERRED` from the exception name and timing.**
Confirming it would mean reading the engine's own implementation, which is the
line AGENTS.md draws, so it stays inferred. Whether the weak refs and the
`_IO_fflush` fault are one problem or two is open.

## §19. The streams are translated, and the engine finally says what is wrong

`native/legacy_stdio.cpp` maps the three `__sF` slots onto the host's real
`stdin`/`stdout`/`stderr` and routes the ten FILE-taking functions the engine
imports through the translation. In C++ rather than Rust because `fprintf` is
variadic, Rust cannot define a variadic `extern "C"`, and AGENTS.md records that
this project's one previous attempt at wrapping variadics unsafely aborts the
engine.

### All three reproducers changed, and none of them segfaults

| reproducer | before | after |
|---|---|---|
| `setPlatformImpl` | 139, `SIGSEGV` in `_IO_fflush` | 133, `SIGTRAP` |
| `CORDIAL_LATE_SETTINGS` | crashed 2/2, never root-caused | 133, `SIGTRAP` |

§18 predicted the fault and §18.1 found a third instance of it. The prediction
holds: with the legacy streams translated, **no reproducer produces a `SIGSEGV`
at `_IO_fflush` any more**. What each produces instead is a named error, which is
the whole point.

### And the named error is the answer this document has been looking for

`CORDIAL_LATE_SETTINGS=1` now ends:

    RBXCRASH: FatalRuntimeError
      (Can't initialize the TaskScheduler before flags have been loaded)

**The engine has been saying this all along and it was arriving as a memory
fault.** For weeks this was "Cordial crashes when it uses mocktail's ordering,
cause unknown" — §11.8 set it aside on exactly that basis. It is not unknown. The
TaskScheduler is initialised before flags are loaded, and the engine treats that
as fatal.

That reframes the whole storage question. §12 established the flags verdict is
reached inside `initializeNativeCode` before any settings call, and could not say
why. This says why: something on that path brings the TaskScheduler up first, and
the engine's flag machinery will not run behind it.

`setPlatformImpl` ends differently — thirteen
`djinni (djinni_support.cpp:529): weakRef` exceptions and then
`RBXCRASH: JNI: Crashing due to unhandled Java exception`. A separate problem,
now visible as one: djinni wants working weak global references and libjnivm does
not provide them. Still `INFERRED` as to cause, but it is a reported Java
exception rather than a corrupted heap, and it can be worked on.

### What this cost and what it bought

Ten wrapper functions and an address exported from Rust so the constant is not
restated. No behaviour change on the default path — a pointer that is not one of
the three legacy slots is passed through untouched, because a pointer this code
does not recognise is not its to interpret.

What it bought is that two failures which presented as heap corruption now
present as sentences. AGENTS.md's rule is that a stub which reports success is
worse than one that reports failure; the same holds a level down. A crash that
names its cause is worth more than one that does not, and this project spent
weeks on one that did not.

### Next

The TaskScheduler line is the thread to pull. It is the first statement from the
engine itself about *why* flags are not loaded, as opposed to *that* they are
not, and everything in §§12–17 was working without it.

### §19.1 What the TaskScheduler line does and does not explain

**Superseded in part by §22.** The conclusion below that "the gate is
satisfied on the path Cordial actually uses" is wrong: it reads the absence of
the fatal error as the gate being passed, when it is the gate never being
reached. Kept because the narrowing it does to the late-settings ordering holds.

Chased, and the scope is narrower than §19 implied. Worth pinning before anyone
builds on it.

**In a working default run the TaskScheduler is fine.** The only mention in the
engine log of a 90-second join is

    [FLog::AndroidGLView] rbx.datamodel: setTaskSchedulerBackgroundMode()
                          enable:false context:ASMA.start

which is background mode, not initialisation, and no error accompanies it. So the
gate is satisfied on the path Cordial actually uses.

`Can't initialize the TaskScheduler before flags have been loaded` therefore
explains **why the late-settings ordering cannot work**, and nothing more. It is
not an account of `onFlagsFailed`, which still fires twice on the default path
where the scheduler comes up cleanly.

That is still worth having. §17 named §11.8's crash as the strongest remaining
lead precisely because mocktail succeeds with an ordering that killed Cordial,
and the answer turns out to be that the ordering is unavailable rather than
mysterious: something on Cordial's `initializeNativeCode` path brings the
scheduler up, and the engine will not load flags behind it. Adopting mocktail's
ordering would mean also deferring whatever does that, which is a different and
larger change than reordering two calls.

**So the lead is narrowed, not closed.** `onFlagsFailed` on the default path
remains unexplained after eight eliminated candidates, and the honest position is
that §19's framing — "the answer this document has been looking for" — was one
step ahead of the evidence. The engine named the cause of a crash, not the cause
of the verdict.

## §20. A 255-second leg of real gameplay, and the texture flag confirmed

Reported from actual play rather than a harness: Doors, to room 15, stopped
because the player got bored. Numbers from that session's engine log,
`2.734.0.917_20260819T041725Z`:

    connections                     3   (Doors teleports lobby -> run)
    last engine timestamp     381.3s
    last Connection accepted  126.1s
    disconnect events               1

The single disconnect is at 23.4s and is `connectMode: Disconnect ASAP`,
`AckTimeout 0, IsOutgoingDataWaiting 0` — a client-initiated teleport out of the
lobby, not a server drop. **The final leg ran from 126.1s to 381.3s: 255 seconds
with no disconnect at all.**

Against a 60.6s death that reproduced twelve-plus times, and against §14's four
90-second harness runs, this is the first long session and it is four times the
window those covered. The 304 is not merely failing to reproduce inside 90
seconds; it does not reproduce across a real play session either.

That does not change §14's conclusion about *why*. Roblox shipped 2.734.0.917
mid-session and every 304 was measured on 2.730.0.790; the engine update remains
the stronger candidate and Cordial still cannot claim the fix. What this adds is
that the reprieve is not an artefact of short runs.

### And the texture flag is doing something

    14.930483 [FLog::Graphics] Using TM1
    14.930499 [FLog::Graphics] Warning: Using TexturePackGenerator.

**TM1 is TextureManager 1** — the legacy path. `FStringGraphicsTextureManager2DenyPattern2
= ".*"`, shipped as a built-in default, denies every pattern in TextureManager2,
and the engine has fallen back exactly as predicted. That entry was marked
`INFERRED` on the grounds that the flag's absence from Roblox's document and its
effect on mocktail were established while the mechanism was not.

The mechanism is now observed. The engine says which texture manager it chose,
and it chose the one the flag leaves available. Whether the resulting textures
look better is still a judgement nobody has made side by side — but "the flag
reaches the engine and changes which manager runs" is no longer inferred.

## §21. Settings before `initializeNativeCode`, with the bootstrap intact: no change

The early `nativeInitClientSettings` call was added because the first
`flags FAILED` was seen arriving before settings had been delivered at all — the
answer arriving after the question it was meant to inform. It was then wired
behind `CORDIAL_NO_BOOTSTRAP`, so "settings early" and "no bootstrap" have only
ever been true together and the useful half was never tested alone.

`CORDIAL_EARLY_SETTINGS=1` decouples them. Controlled on the same build:

    baseline                   early=0  onFlagsFailed=2  RbxStorage=0  ClientRunInfo=0
    CORDIAL_EARLY_SETTINGS=1   early=1  onFlagsFailed=2  RbxStorage=0  ClientRunInfo=0

The call fires and nothing moves. **Candidate ten, eliminated.**

That one mattered more than the others because §12 measured the verdict being
reached inside `initializeNativeCode` before any settings call, which made "the
engine wants its flags already present when that runs" the obvious reading. It is
wrong: the flags can be present and the verdict is the same.

The switch stays, off by default, because it is now the only way to vary that
ordering without also changing the bootstrap and it will be wanted again.

### The eliminated list, in one place

Ten now. The empty `ArrayList`; `channelPlatformName`; the device identity; every
flag routing `RbxStorage`'s construction; four settings-document variants; a
missing call in mocktail's pre-settings path; Cordial's early settings call as
originally wired; the `Configuration` object being empty; delivering settings
before `initializeNativeCode` with the bootstrap intact; and — from §17 —
`bootstrapTheApp` needing to deliver anything, which mocktail disproves by
reaching `flagLoaded` with a one-line no-op.

### What is left that has not been tried

Two things, and both are larger than a flag.

**The `_IO_fflush` crashes are fixed but their consequences are not explored.**
§19 turned three segfaults into named errors, and one of them said
`Can't initialize the TaskScheduler before flags have been loaded`. §19.1 pinned
that to the late-settings ordering only. Nobody has yet asked what *does* bring
the scheduler up on the default path, or whether the flags machinery runs behind
it there too, quietly, without the fatal error.

**Loud logging is exhausted as a technique.** *(Retracted in §22: the sweep
drew its 135 names from a 30-line list of channels seen on Android, out of 724 in
the binary, and setting a channel in `flags.json` is shown there to silence it.
`FLog::NativeDM` was never in the list and has been printing the answer all
along.)* 135 channels at maximum produce
5961 lines and not one about the verdict — before or after the stdio fix, which
was the last hope that the engine had been trying to tell us and could not. It
had not. Whatever decides this does not log.

## §22. The engine has been naming the state every run, on a channel nobody read

§21 closed with "loud logging is exhausted as a technique". That was wrong, and
the way it was wrong is worth stating before the finding itself.

The 135-channel sweep drew its channel names from `docs/traces/flog-channels.txt`,
which is 30 lines — the channels that happened to appear in the Waydroid capture.
`libroblox.so` defines **724**. `FLog::NativeDM` is not in the 30, was never
enabled, never grepped for, and has been printing twelve lines in every run this
project has ever made.

### What it says

From a plain `--run 12` with no overrides at all:

    [FLog::NativeDM] nativeActivity_onStart:
    [FLog::NativeDM] nativeActivity_onResume:
    [FLog::NativeDM] dataModelBindings_onGameLoaded: placeId = 0.
    [FLog::NativeDM] nativeActivity_onSurfaceChanged: state:11.
    [FLog::NativeDM] nativeActivity_onSurfaceChanged: ... Flags-Not-Received. Return.
    [FLog::NativeDM] nativeActivity_onSurfaceChanged: state:11.
    [FLog::NativeDM] nativeActivity_onSurfaceChanged: ... Flags-Not-Received. Return.
    [FLog::NativeDM] nativeActivity_onKillSurface: state:11.
    [FLog::NativeDM] nativeActivity_onKillSurface: ... Flags-Not-Received. Return.
    [FLog::NativeDM] nativeActivity_onStop:
    [FLog::NativeDM] nativeActivity_onDestroyed:
    [FLog::NativeDM] nativeActivity_onDestroyed: ... Flags-Not-Received. Return.

`NativeDM` is `RBX::NativeDataModelManager` — the class `init_params.cpp` already
names as the writer of `onFlagsFailed`, from `getFlagsFromEngine_`'s completion
lambda. **`Flags-Not-Received` is its own word for the state Cordial is stuck
in**, and every lifecycle callback the engine delivers turns round and returns on
it. That is the mechanism by which nothing downstream happens: not a missing
call, a latched state.

Absent from the log, and they are the rest of that class's vocabulary:
`[Constructor]`, `initialize: state:{}. areFlagsLoaded:{}.`, `getFlagsFromEngine_:`,
`continueAfterFlagsLoaded_:`, `initEngine_:`.

### §19.1 was wrong about the TaskScheduler and this is the retraction

§19.1 concluded "in a working default run the TaskScheduler is fine … the gate is
satisfied on the path Cordial actually uses", reasoning from the absence of the
fatal error. Absence of the error is not evidence the gate was passed; it is
equally what never reaching the gate looks like, and that is the case here.
`RbxStorage::init` and `ClientRunInfo` are absent from every Cordial run, and both
sit downstream of it.

The one scheduler line Cordial does produce —
`setTaskSchedulerBackgroundMode() enable:false context:ASMA.start` — lands at
**0.480s**, while the same line on real Android lands at 0.417s *after*
`RbxStorage::init`. It is background mode rather than initialisation either way,
so it is not itself the fatal path, but it is not the clean bill of health §19.1
read it as.

### The signature theory, killed by a control before it cost anything

Cordial's flag cache write logs `Wrote signatureSize: 0`, and the engine exports
`nativeInitClientSettingsSigned(String, String, String, String)I` alongside the
plain three-argument form Cordial calls. That is a tidy story and it is false:
**Sober logs `Wrote signatureSize: 0` as well**, on the run at
`2.734.0.917_20260819T003213Z`, and Sober reaches `flagLoaded`. Nothing is gated
on a signature Cordial is failing to supply.

### The flags themselves are fine, and this is now measured rather than argued

Same run, no overrides:

    [FlagCache] writeFlagCache: Compressing flag cache data (input size: 1270529 bytes)
    [FlagCache] writeFlagCache: Compression complete. Output size: 328040 bytes, ratio: 3.87x
    [FlagCache] writeFlagCache: Successfully wrote 328045 bytes
    [FLog::TombstoneCache] Tombstone 1, expiry time 360, holdout false, channel 'production', written

1.27 MB of flag data parsed, zstd-compressed and persisted, against Sober's
1301322 → 333489 on the same day. The data loads. The *event* does not fire.
`init_params.cpp`'s comment — "the flag data did load" — is confirmed, and the
question is narrowed to the notification rather than the payload.

### Overriding an `FLog` channel can silence it

Set out here because it invalidates the sweep §21 rested on, and because it will
mislead the next person the same way:

| `flags.json` | `NativeDM` lines |
|---|---|
| absent | 12 |
| `{"DFLogNativeDM": 7}` | 12 (wrong prefix, no effect) |
| `{"FLogNativeDM": 1}` | **0** |
| `{"FLogNativeDM": "100"}` | **0** |
| `{"FLogGraphics": 7}` | 12 — an unrelated override changes nothing |

Controls on both sides: no-override runs before and after read 12. So naming a
channel in `flags.json` set it to *quiet* at every value tried, including 100,
while `FLogAppShellReporter: 7` took that channel 0 → 14 on the same mechanism.
Whatever the semantics are, **"set 135 channels to maximum" is not verified to
have raised anything and may have lowered some of them.** §21's conclusion that
loud logging is exhausted does not follow from that sweep.

### mocktail does not solve this. It patches the byte

The comparison this project has run against mocktail for weeks assumed mocktail
reaches the flags-loaded state legitimately and Cordial fails to. It does not.
`src/legacy/legacy_runtime.cc:13354` (Apache-2.0, read directly):

```cpp
bool ForceNativeFlagsLoadedForTaskScheduler(const char* reason) {
  ...
  auto* flag = reinterpret_cast<unsigned char*>(
      g_libroblox_base + kRobloxNativeFlagsLoadedByteOffset);
  ...
  if (!EnsureWritablePage(flag)) { ... }
  const unsigned int old_value = *flag;
  *flag = 1;
```

`mprotect` the page, write `1` to a fixed offset inside `libroblox.so`, and the
gate opens. It is on by default —
`SetEnvDefault("MOCKTAIL_PATCH_NATIVE_FLAGS_LOADED", "1")` at line 2818 — and it
is not the only one; `MOCKTAIL_PATCH_STAGE6_START_LUA_DM_FORCE_SAME_THREAD` and
`ForceStage6DataModelPatcherForceLocalFlag` sit beside it. The function is named
for our fatal error.

**Cordial cannot do this and will not.** In-process memory patching is exactly
what [ADR-001](../adr/ADR-001-in-process-hooking.md) and
[ADR-003](../adr/ADR-003-plugin-isolation.md) make *absent* rather than disabled,
so that no fork can extract the primitive. That decision stands; this is the
first time it has had a visible cost, and the cost is that the one comparable
implementation's answer is off the table.

What that changes: every "mocktail gets further, find the call we are missing"
inference in §§13–17 was chasing a difference that does not exist at the call
level. mocktail is not further along this path. It is past it by force.

### Sober, however, reaches `flagLoaded` and how is not established

Sober's log for 2026-08-19, at the same landmarks:

    3.001397 [FLog::AndroidGLView] nativeInitClientSettings
    3.064486 [DFLog::FlagCache] Deferring flag cache write to post TTI
    3.067240 [FLog::AndroidGLView] nativePostClientSettingsLoadedInitialization3
    3.067323 [FLog::ClientRunInfo] RobloxGitHash / base url / channel
    3.072664 [DFLog::AppPlatformQoSEmergency] instanced
    3.075039 [DFLog::Mimalloc] ...
    3.082697 [FLog::TombstoneCache] Tombstone 1 ... read from file
    3.091522 [DFLog::RbxStorage] RbxStorage::init [INIT] user: flagLoaded
    3.093661 [FLog::JNIAppBridge] nativeAppBridgeAppStart:

Cordial has none of the block between `nativePostClientSettingsLoadedInitialization3`
and `RbxStorage::init`, and its `nativeAppBridgeV2Init` is the **first** line in
its log at 0.228s where Sober's app bridge starts at 3.093s, after the storage is
up. Sober also *reads* an existing tombstone where Cordial writes a fresh one.

Whether Sober patches memory to get there is **not established** — the reference
tree here (`~/Projects/sober-oss-reference`) contains only `libbadcpu`, and its
`decompiled/` directory is off-limits under AGENTS.md. So "Sober does it
legitimately" is *not* a claim this section makes. What is observed is only that
Sober reaches the state and Cordial does not.

### Where this leaves it

Not fixed. What is now established rather than guessed:

* The stuck state has a name, `Flags-Not-Received`, and it is latched — the
  engine re-checks and re-returns on every lifecycle callback.
* The flag data is not the problem: 1.27 MB loads, compresses and persists.
* No signature is required; the control kills that.
* `getFlagsFromEngine_`'s completion chooses failure with all of that in place,
  and `initialize:` / `continueAfterFlagsLoaded_` never log.
* The only known implementation past the gate forces it with a memory write
  Cordial has permanently ruled out.

The next experiment is the one this section could not run: get
`getFlagsFromEngine_:` and `initialize: state:{}. areFlagsLoaded:{}.` to print.
They are the two lines that would say what the engine thinks the state is at the
moment it decides, and the channel is already open by default — it is the
verbosity of those particular lines that is not. Raising it through `flags.json`
demonstrably does the opposite, so that route needs understanding first.

### §22.1 Four more eliminations, and the shape they make

All measured on the same build with controls, after §22.

**Fourteen: the engine's own compressed flag cache, handed back.** The engine
writes `flag_cache.dat` — 365074 bytes here — and exports three settings natives
besides the plain three-string form Cordial has always used. Cordial had never
handed one back, so every launch looked cold to the engine with the cache sitting
on disk beside it. `nativeInitClientSettingsCachedCompressed([B, String, String,
String, long, boolean)I` now takes it:

    365074 bytes, [||],                     when 1787121646510, flag true  -> 3
    365074 bytes, [||],                     when 1787121696442, flag false -> 2
    365074 bytes, [||],                     when 0,             flag true  -> 3
    365074 bytes, [AndroidApp|production|], when 1787121709184, flag true  -> 3
    365074 bytes, [production||],           when 1787121722001, flag true  -> 3

The **boolean** is the only argument that changes the result; the three strings
and the timestamp are ignored. 2 and 3 are result codes this project has not seen
before — the plain form gives 0 for a good document and 1 for a bad one — so the
engine is reading the cache and rejecting it, probably over the five-byte
signature/compression header the write log describes.

Not pursued, and the reason matters: **the plain path already returns 0.** Making
a second path also return 0 would be a fourteenth way of establishing "the
settings were accepted", which is the one thing never in doubt. The wrapper stays
because it is correct and someone will want it, behind `CORDIAL_CACHED_SETTINGS`.

**Fifteen: not calling `nativeGameGlobalInit` at all.** §9's captured stack
reaches the failure reporter through it, and §22's ordering test only moved it.
`CORDIAL_NO_GLOBAL_INIT=1` skips the pair outright. The run segfaults later,
exactly as the original comment predicted — `StartLuaAppDM` crashes on a null
`JNIEnv` the globals init was supposed to store — but it gets far enough to
answer the question:

    onFlagsFailed=2   RbxStorage=0   Flags-Not-Received=4

**The verdict fires twice with that call never made.** So it is not produced
there. §9's stack was one of two occurrences and removing that path removes
neither.

### What the shape of fifteen eliminations says

Every input the app-facing interface accepts has now been varied, and none of
them moves the verdict:

* the document — four variants, and its absence
* when it arrives — six seconds early, mid-`initializeNativeCode`, and after
* which native takes it — plain, and the compressed-cache form
* preloaded overrides — three shapes, all accepted with no `ParseFailure`
* the flag-name list, the flag provider, the `Configuration`, the `ArrayList`
* call ordering — bootstrap, globals early, globals late, globals absent
* the callbacks the engine can reach — 7 answered, then 19, descriptors verified

That is the whole surface. The verdict is decided inside the engine at a point
§9 pinned precisely — `movl $0xb` written unconditionally at `0x29c5529`, status
11, the same 11 `NativeDM` then reports as `state:11` forever — and whatever
picks `0xb` is upstream of anything the app hands in.

**This is now a policy question rather than an engineering one.** The only
implementation known to be past this gate writes the byte: mocktail's
`ForceNativeFlagsLoadedForTaskScheduler`, one of **98** patch/force/install
functions in `legacy_runtime.cc`, with 116 `PatchCode` call sites and 77
`EnsureWritablePage` calls beside it. Memory patching is not incidental to
mocktail, it is its method. Its git history was squashed on a GitLab migration,
so there is no record of a legitimate route being tried and failing — only that
the one real JNI candidate, `nativeInitializeNativeFlags`, defaults to **off**
there while the patch runs unconditionally straight afterwards.

Sober reaches `flagLoaded` and remains the existence proof that the state is
reachable. **How it does so is not established** and cannot be from here: the
reference tree holds only `libbadcpu`, and its `decompiled/` directory is
off-limits. "Sober does it legitimately" is not a claim this document makes.

So the honest position: Cordial cannot reach this state through the interface
Roblox exposes to its host application, and the alternative on the table is the
one [ADR-001](../adr/ADR-001-in-process-hooking.md) and
[ADR-003](../adr/ADR-003-plugin-isolation.md) make *absent* rather than disabled,
so that a fork cannot extract the primitive. That decision was taken when no
feature depended on it. One does now. Reversing it is a change to Cordial's
security posture, in a public GPL repository with a fork already layering exploit
functionality on it, and it belongs to the project owner rather than to whoever
is next to run out of eliminations.

### §22.2 Sober's mechanism cannot be settled from here, and why

§22.1 left the central question as: is the flags-loaded state reachable at all
through the host-application interface, or does every implementation that gets
there force it? mocktail forces it. Sober reaches `flagLoaded` and is the only
existence proof that the state is reachable by something.

The test designed for this was to compare Sober's in-memory `libroblox.so`
executable text against the file on disk. Executable pages of a PIE carry no
relocations on x86-64, so a faithful loader leaves `.text` byte-identical; a
non-zero difference count is code patching. `tools/`-adjacent scratch script,
`ptrace_scope` is 0 on this machine, and the method is sound.

**It cannot be run.** Sober is a Flatpak and its engine runs inside the
sandbox's PID namespace, so no host `/proc/<pid>/maps` contains the mapping —
a scan of every readable process for an executable mapping over 50 MB finds
Chrome, WebKitGTK and two LLVM copies, and nothing of Sober's. `flatpak enter`
would cross the namespace but `setns` needs `CAP_SYS_ADMIN`, so it exits without
output as an ordinary user. The mapping is also anonymous rather than named,
exactly as Cordial's own loader leaves it, so it could not have been found by
name either.

Recorded rather than quietly dropped, because the next person will have the same
idea. Settling it needs root, or a build of Sober's loader, or Sober's source —
and the reference tree here holds only `libbadcpu`, with its `decompiled/`
directory off-limits under AGENTS.md.

### One inference not made

`CORDIAL_LATE_SETTINGS=1` still ends the way §19 recorded, `SIGTRAP` at 0.231 s
with `RBXCRASH: FatalRuntimeError (Can't initialize the TaskScheduler before
flags have been loaded)`. The default path produces no such error.

It is tempting to read that as the gate being satisfied on the default path, and
therefore as evidence that `Flag::areFlagsLoaded()` is already true there while
only `NativeDataModelManager` is uninformed. **That is precisely the reasoning
§19.1 got wrong and §22 retracted**: absence of the error is equally what never
attempting the initialisation looks like, and the default path shows no sign of
attempting it — no `RbxStorage::init`, no `ClientRunInfo`. Cordial's own
`setTaskSchedulerBackgroundMode` call is background mode, not initialisation, and
does not bear on it.

So what is observed is only this: the late ordering *attempts* the scheduler
initialisation and dies on the gate, and the default ordering does not attempt
it. Which of `Flag::areFlagsLoaded()` and NativeDM's `Flags-Not-Received` is
false on the default path is **not established**, and the two are not known to be
the same bit.

### §22.3 Correction (2026-08-19): the silencing was a value-shape mismatch, not the mechanism

§22's "Overriding an `FLog` channel can silence it" table used one shape for
every value tried — a bare number, `1`, `7` or `100` — and concluded that
naming a channel in `flags.json` sets it to quiet at every value including the
largest tried. That conclusion is wrong as stated, and the sweep it invalidated
(§21) is not thereby exonerated either — this only settles the mechanism.

Roblox's own cached settings document (`~/.cache/cordial/clientsettings.json`)
already answers what §22 did not check: `FLog`/`DFLog` values there are not one
shape. Most are a bare verbosity number (`FLogNetwork = "7"`,
`DFLogHttpTraceError = "12"`), but a real minority are a severity name with an
optional sub-level (`FLogAudio = "Info"`, `FLogWebRTC = "Error"`,
`DFLogWebSocketTraceError = "Warning,6"`, `DFLogRakNetConnectTrace_PlaceFilter =
"Verbose,9"`). Which shape a given channel's C++ declaration wants is not
visible from the settings document or from `flags.rs`, and `flags.rs` never
tries to guess it — see its `read_layer` doc comment, extended alongside this
correction.

Repeating §22's `FLogNativeDM` case with a severity name instead of a number,
each figure the mean of two runs, `--run 20`, own profile, engine's own `FLog`
file read directly (not stderr):

| `flags.json` | `[FLog::NativeDM]` lines | `[FLog::AppShellReporter]` lines |
|---|---|---|
| absent | 29, 29 (repeat) | 0, 0 (repeat) |
| `{"FLogNativeDM": "9"}` (bare number, as §22 used) | 0 | — |
| `{"FLogNativeDM": "100"}` (bare number, as §22 used) | 0 | — |
| `{"FLogNativeDM": "Debug", ...}` (severity name) | 29 | 14 |
| `{"FLogNativeDM": "Verbose", ...}` (severity name) | 30, 30 (repeat) | 16, 14 (repeat) |

A bare number silences `NativeDM` on every value tried, exactly as §22 found.
A severity name does not — it leaves the channel at or above its unset count,
and the *same* override raises `AppShellReporter`, which is silent by default,
from 0 to 14–16 lines, matching what a bare `"7"`/`"9"` already did to that
channel in §22 (0 → 14). So the direction of the effect (raise vs silence) is
not a property of the mechanism or of the number chosen — it is a property of
whether the value's shape matches what that specific channel's declaration
expects. Wrong shape reads as "override present, channel now silent"; right
shape raises it, on two independent channels, repeated.

**What this means for `flags.rs` and `client_settings.rs`: nothing was wrong.**
Both convert a JSON value to a plain string and hand it through unchanged —
`"7"` stays `"7"`, `"Verbose"` stays `"Verbose"` — which is exactly the
behaviour a heterogeneous, string-typed settings document requires. No code
change was needed to "make it work"; using the right value shape was
sufficient, demonstrated above. A doc comment on `read_layer` now says so, so
the next person who reruns §22's experiment with a bare number does not
independently re-arrive at "the override mechanism is broken".

**The `FlagJniInterface.nativeGetFInt` cross-check, resolved.** §22's report
noted every name probed through it, including `FLogGraphics`, reads back as
"not a registered flag", and asked whether that means the probe reads an empty
Java-side registry. Confirmed, and more specifically than suspected: the names
`nativeInitializeNativeFlags` registers and `nativeGetFInt` can answer for are
the **139 Android-app feature flags** in `docs/traces/native-flag-names.txt`
(`EnableAndroidBinaryChannelDownloadTiming`, `PgsTreatmentActive`, and so on) —
an entirely different namespace from the engine's internal `FLog`/`DFLog`
channels, which are read out of the `applicationSettings` document at
`nativeInitClientSettings` time and never touch `FlagJniInterface` at all. The
probe was run again here with `CORDIAL_FLOG_PROBE=DFLogRbxStorage,FLogGraphics,
FLogAppShellReporter` immediately after confirming (by grepping the same run's
stdout) that `nativeInitializeNativeFlags` had already registered its 139
names — so this is not a timing gap, either. `nativeGetFInt` is simply the
wrong instrument for an `FLog`/`DFLog` channel's state; it was never going to
answer this question, for any channel, regardless of ordering.

**`DFLogRbxStorage`, raised correctly, still never appears.** With the shape
mismatch understood and controlled for, `DFLogRbxStorage` was set to `"9"`,
`"100"`, `"Debug"` and `"Verbose"` — bare numbers and severity names, the same
four value-shapes that moved `NativeDM` and `AppShellReporter` above — across
five separate runs. `[DFLog::RbxStorage]` count: **zero, every time**, while in
the same runs `FLogNativeDM` and `FLogAppShellReporter` visibly responded to
their own overrides, proving the mechanism was live and the document was being
read. This is not a new finding — §23.1 already reached zero by a different
route — but it is a second, independent confirmation, ruling out "the channel
is suppressed" as the reason `RbxStorage::init` is absent. Whatever blocks it,
it is not this.

## §23. The answer: the post-settings call was made too early

`nativePostClientSettingsLoadedInitialization3` called once more, after the
surface is handed to the engine, followed by `nativeRetryInit`. That is the whole
fix, and it produces what nineteen sections of this document were looking for:

    [FLog::NativeDM] initialize: state:11. areFlagsLoaded:true.
    [FLog::NativeDM] getFlagsFromEngine_:
    [FLog::NativeDM] bootstrapTheApp_:
    [FLog::Output] settingsUrl: https://clientsettingscdn.roblox.com/v2/settings-compressed/application/GoogleAndroidApp.zst
    [FLog::NativeDM] ... getFlags: success = true, payload's size = 1300800.
    [FLog::NativeDM] continueAfterFlagsLoaded_:
    [FLog::NativeDM] initEngine_:
    [FLog::NativeDM] initializeLuaApp_:
    [FLog::NativeDM] startLuaApp_:

and on Cordial's side `[roblox] flags loaded (1300800 bytes)` —
`gameActivity_onFlagsLoaded`, with a real `ByteBuffer`, for the first time.

Controlled on one build, three consecutive runs each:

| | `flagsLoaded` | `continueAfterFlagsLoaded_` | `Flags-Not-Received` |
|---|---|---|---|
| default | 1 | 1 | 1 |
| `CORDIAL_LATE_POST_MS=off CORDIAL_LATE_RETRY=off` | 0 | 0 | 4 |

### Why fifteen eliminations missed it

Every one of them moved the settings call and the post call **together**. §11
recorded the symptom exactly — "Cordial's call to
`nativePostClientSettingsLoadedInitialization3` returns without the engine's own
body of it having run" — and then spent five sections looking for a missing
argument, a missing prerequisite call, or a wrong document. The body was fine.
The call was early. Nothing that moved both could ever show that, because moving
them together late is `CORDIAL_LATE_SETTINGS`, which dies on the TaskScheduler
gate before the post call matters.

### Three claims in this document were wrong

**§19.1 and §22 on the TaskScheduler.** §19.1 said the gate is satisfied on the
default path; §22 retracted that as reasoning from an absence and declined to
claim the opposite. The engine now states it: `areFlagsLoaded:true`, on the
default path, before anything here changed. The gate was never the blocker.
`NativeDataModelManager` not being told was.

**`client_settings.rs` on the engine never fetching.** It says so on the strength
of breakpoints on `getaddrinfo`, `connect` and `SSL_connect` never being hit
during startup. The engine fetches
`clientsettingscdn.roblox.com/v2/settings-compressed/application/GoogleAndroidApp.zst`
itself, from `bootstrapTheApp_`. Those breakpoints never fired because this code
path had never run. Cordial supplying the document is still correct and still
what makes `areFlagsLoaded` true — but "the engine does not fetch" is false.

**§22 on logging.** It said the verbose `NativeDM` lines were open but not
raised. They were open all along and print at the default level. They never
appeared because the code emitting them never ran. The §22 measurement that
naming an `FLog` channel in `flags.json` can silence it still stands and is still
worth knowing; the conclusion drawn from it was wrong.

### What is still not done

**`RbxStorage::init` is still zero**, on every run, including a real 100-second
join. So the content store is still down and every asset still comes off the
network each session. What is different is that it is now localised rather than
mysterious:

    [DFLog::CaptureStorage] RbxStorage is not initialized, cannot access storage interface
    [DFLog::RbxmFileManager] LocalStorageManager is not available.
    [FLog::LocalStorageHandler] Not available on the current platform.

Storage waits on the platform local-storage handler, which Cordial implements in
`native/local_storage.cpp` but installs only behind
`CORDIAL_LOCAL_STORAGE_SET_PLATFORM_IMPL`. With that on, `setPlatformImpl ok` now
succeeds where §19 recorded it crashing — and the process then dies `SIGTRAP`
after repeated `djinni (djinni_support.cpp:529): weakRef`.

**It is not `NewWeakGlobalRef`.** That was the obvious reading and it is wrong:
instrumenting libjnivm's `NewWeakGlobalRef` to print whenever it returns null
produced **no output at all** across a full run, while the djinni exceptions
carried on. libjnivm implements weak global references and has a test covering
expiry. Whatever djinni asserts on at that line, it is something else, and the
instrument is recorded here as a disproof rather than left in the tree.

Sober logs `LocalStorageHandler] Not available on the current platform.` too and
still reaches `RbxStorage::init`, so that message is not the blocker either.

**The delay is unfinished work, not a constant.** 250 ms, because at 0 ms the run
reaches `Flags-Not-Received=0` — better than any other value tried — and then
segfaults. Something is still racing and the delay hides it.

### §23.1 RbxStorage after the fix: what was tried, and one observation nobody should build on

With the flags chain working, everything §12–§21 eliminated deserved re-testing,
because every one of those eliminations was measured against a baseline where the
chain never ran. Done, and none of it moves storage:

* `FFlagStartRbxStorageInitRighAfterFlags` and `DFFlagRbxStorageInitLatch` set
  true, against a control with neither, on fresh data roots: no storage either
  way. This flag was the premise of §11's whole storage theory.
* Three consecutive launches against one warm root, so the flag cache and
  tombstone are present: no storage on any of them, and the tombstone is never
  *read* on any launch, only written. Sober reads one.
* `CORDIAL_LOCAL_STORAGE_SET_PLATFORM_IMPL=1`: `setPlatformImpl ok` now succeeds
  where §19 recorded it crashing, and the process then dies `SIGTRAP` after
  repeated `djinni (djinni_support.cpp:529): weakRef`. **This is a dead end
  regardless** — Sober logs `[FLog::LocalStorageHandler] Not available on the
  current platform.` too, and reaches `RbxStorage::init` anyway. Storage does not
  need this handler.
* The late-post delay at 250 ms and at 2000 ms, `--run 40`, fresh roots: no
  storage at either.

`[DFLog::CaptureStorage] RbxStorage is not initialized, cannot access storage
interface` fires on a real join, so it is genuinely down rather than quietly
working.

**The observation not to build on.** One data root does contain a real store —
`rbx-storage.db` with a WAL, `rbx-storage-sc`, and partition directories `p14`
and `p15` — created at 17:46:02 during this session. It has not been reproduced:
re-running the two candidates that were live at that moment, on fresh roots with
controls, produces nothing, and a fresh run against that same root does not touch
the database's mtime. The one thing that root has which no fresh root does is 43
`ContentProvider_*` cache directories, so content was being cached there by some
other path. That is a lead, not a result, and it is written down as an
unreproduced observation precisely so nobody quotes it as evidence that storage
works.

**Where this leaves it.** `DFLog::RbxStorage` has never appeared in a Cordial log
at all, on any run, so there is no engine statement about storage to read — the
absence of `RbxStorage::init` is consistent both with "never attempted" and with
"attempted and unlogged", and §22 is the standing warning about which of those
an absence licenses. Establishing which needs the channel genuinely raised, and
§22's measurement is that naming a channel in `flags.json` can silence it rather
than raise it. That mechanism is still not understood and is now the thing
blocking the question, not the storage code.

### §23.2 Storage is never attempted, and this settles it

§23.1 left the question as "never attempted or attempted and unlogged", and said
the logging mechanism was blocking the answer. It is not: the filesystem answers
it without any log channel.

`CORDIAL_TRACE_PATHS=1`, one 35-second run, **19,296 intercepted path calls, zero
of them containing `rbx-storage`**. The absence of `RbxStorage::init` from the
log is therefore "never attempted". Nothing is being initialised quietly.

(That switch is real and works. It is worth saying because an earlier attempt in
this session concluded it produced no output at all — the grep was for `[path]`
and the format is `[paths]`. The tool was fine; the measurement was wrong.)

What the engine does touch, once the 17k `/sys/devices/system/cpu/*/cpufreq`
polls are set aside:

    180  /proc/self
     51  <profile>/data/files/appData
     50  <profile>/data/cache
     30  ./exe
     23  <profile>/data/files/appData/LocalStorage
     14  http                      <- relative
     14  /dev
      8  <profile>/data/cache/wob
      6  cache                     <- relative

So the engine is doing local storage, under `appData/LocalStorage`, and simply
never reaches for the content store.

### The block is short by three steps, and they are consecutive

Against Sober at the same point, Cordial's post-settings block is missing exactly
the three lines that run together at the end of it:

    Sober                                          Cordial
    IxpStorageManager: Failed to open cache file   absent
    TombstoneCache: Tombstone 1 ... read from file absent
    TombstoneCache: Setting holdout state: false   present
    LocalStorageHandler: Not available             present
    RbxStorage::init [INIT] user: flagLoaded       absent

Cordial reaches `Setting holdout` and `LocalStorageHandler`, so it is not that
the block stops early. It skips the Ixp cache open and the tombstone *read* while
still writing a tombstone of its own — and Cordial's write goes to
`cache/tombstone.dat`, **relative**, where Sober's is absolute. Both
`<profile>/run/cache/tombstone.dat` and `<profile>/data/cache/cache/tombstone.dat`
exist on disk here, which is what a relative write and an absolute read look like
when they disagree.

**Whether the tombstone read gates `RbxStorage::init` is not established.** It is
the only difference left inside the block, it is immediately upstream of the
missing line, and the relative-path split is a mechanism that would explain a
silent skip. That is a lead with something behind it rather than another flag to
try, and it is where the next session should start.

### §23.3 The two routes to `flagLoaded` are not the difference

Sober reaches `flagLoaded` from the application handing the settings document
over. Cordial now reaches it from the engine fetching its own inside
`bootstrapTheApp_`. Both end in `continueAfterFlagsLoaded_`, and only Sober's is
followed by `RbxStorage::init [INIT] user: flagLoaded`, which made "the routes
are not equivalent to whatever asks for storage" the obvious next theory.

It is wrong. Delivering the document again on the app's route, immediately before
the late post call, against a control without it, on fresh data roots:

    with late settings     flags loaded = 1   RbxStorage = 0   storage files = 0
    without                flags loaded = 1   RbxStorage = 0   storage files = 0

The switch is kept, off by default, as the record. Sixteen candidates now.

### §23.4 The settings document Cordial supplies is the wrong one, and that is not the storage bug either

Cordial fetches `clientsettingscdn.roblox.com/v2/settings/application/**AndroidApp**`
and separately calls `nativeOverrideChannelPlatformName` to say it is
**`GoogleAndroidApp`**. When the engine went looking for flags itself, it fetched
`.../application/GoogleAndroidApp.zst` — its own name for itself, not the one
Cordial had handed it.

The two documents are not the same. `AndroidApp` carries 22,196 flags,
`GoogleAndroidApp` 22,610; 441 values differ, 27 of them with `Storage`, `Cache`,
`Ixp` or `Tombstone` in the name. So Cordial has been running the client on a
document meant for a slightly different application than the one it claims to be.

**It is not the storage bug.** Supplying `GoogleAndroidApp` via
`--client-settings`, against a control on the stock document, on fresh data
roots:

    GoogleAndroidApp   flags loaded = 1   RbxStorage = 0   storage files = 0
    AndroidApp         flags loaded = 1   RbxStorage = 0   storage files = 0

Candidate seventeen. Worth correcting on its own terms regardless — a client
should be given the flags for the application it says it is — but it is a
separate change from this one and is not made here.

### §23.5 `statvfs` was never intercepted, and §23.2's instrument was blind to it

mocktail's answer to storage is not a flag or an ordering. It is
`EnsureDefaultDataLayout` (`src/libc_shim/libc_shim.cc`, Apache-2.0): it creates
the Android private-data directory tree — including `rbx-storage`,
`appData/rbx-storage`, `files/appData/rbx-storage`, `cache/rbx-storage`,
`appData/LocalStorage`, `files/appData/OTAPatchBackups` — **before** the engine
runs, and its own tests assert that `statvfs` and `statfs` succeed on the
`rbx-storage` path. `RbxStorage::init` reports `availableDiskSpace` as part of
starting, so storage asks the filesystem for room before it builds anything.

Cordial created none of those directories. Two of them,
`appData/LocalStorage` and `appData/OTAPatchBackups`, were already visible as
failed opens in Cordial's own path trace, which should have been the clue.

**And `statvfs` was not intercepted at all.** The engine imports it —
`libroblox.so statvfs` is in `undefined-symbols.tsv`, and `nm -D` shows
`U statvfs@LIBC` — while `native/system_paths.cpp` wrapped `stat`, `lstat`,
`access`, `opendir`, `realpath`, `readlink`, `fopen` and `open`, and not this.
So it was neither path-translated nor traced.

That is a correction to §23.2. Its conclusion — "storage is never attempted,
19,296 intercepted path calls and not one contains `rbx-storage`" — was drawn
from a trace that could not see the call storage actually makes. **A trace that
cannot see a call is not evidence the call did not happen**, and this document
has now made that mistake twice: once reading an absent fatal error as a passed
gate, and once here.

With the interception in place, three `statvfs` calls appear, all succeeding:

    [paths] tid=… statvfs("./appData") = 0
    [paths] tid=… statvfs("…/profiles/default/data/files") = 0
    [paths] tid=… statvfs("…/profiles/default/data/files") = 0

The first is relative, resolved against the working directory, and only succeeds
because the layout above now creates `./appData` there. Before this change it
would have failed.

### The ABI divergence found on the way

bionic's `struct statvfs` runs `f_fsid, f_flag, f_namemax`. glibc's inserts an
`int __f_unused` after `f_fsid`, which on LP64 pushes `f_flag` and `f_namemax`
eight bytes along. The engine reading `f_flag` would get glibc's padding, and
reading `f_namemax` would get glibc's `f_flag`; `ST_RDONLY` lives in `f_flag`.
The free-space fields `f_bsize` through `f_favail` happen to align, so this was
not obviously fatal, which is exactly why it survived. Fixed by filling the
bionic shape field by field, as `sigset_t`, `struct sigaction` and `mallinfo`
already are.

### Still not initialising

`RbxStorage::init` remains zero with the layout created, `statvfs` intercepted
and succeeding, and the flags chain working. So the precondition mocktail
satisfies is necessary-looking but has not proved sufficient here. What is
different now is that the instrument is honest: `statvfs` is traced, so the next
person can see what storage asks for instead of inferring from an absence.

### §23.6 `RbxStorage::init` is entered and declines. It was never "not asked for"

Found by scanning the current binary for references to the
`RbxStorage::init [INIT] user: {}, availableDiskSpace: {} …` format string and
then confirming live under `lldb`.

**The addresses in §3, §9 and §10 are stale.** The build moved:
`gameActivity_onFlagsFailed` is at `0x41e987` here, not `0x40f096`. The scan
technique still works; the numbers do not.

`RbxStorage::init` is `0x230bd3a`, and it is a lazy singleton getter with **63
direct call sites**, each preceded by a `lea` loading its own label string —
`AssetProvider`, `SessionTracking`, `CaptureStorage`, `ClientStorageInterface`,
`LocalRuntimeContentStorage`, `ClientReplicator-init`, `CrashMetric`, `DeviceGL`,
the `http-*` family, `shutdown`, `flagLoaded`, and fifty more. The `user:` field
in the log line is that label. So `flagLoaded` is not *the* trigger, it is
whichever of sixty-three callers happened to get there first on Android.

**Live, with `0xCC` planted at `0x230bd3a` per §10's technique:** it fires. Two
independent runs, `rdi` pointing at `"AssetProvider"`, one of them with three
threads hitting concurrently. And the log-emit branch deeper in the same function
was hit **zero** times in every run.

So storage is entered and returns early. Both previous statements that it was
never attempted were wrong, and they were wrong on two independent pieces of
evidence: §23.2's path trace (retracted in §23.5 for being blind to `statvfs`)
and §22.3's channel sweep. Two lines of evidence agreeing did not make them
right; they were both measuring the same downstream absence.

Two further facts:

* The `flagLoaded` wrapper at `0x230bcc2` has **zero** direct callers in `.text`
  and no raw-pointer reference in `.data`/`.data.rel.ro`, so it is reached only
  by indirect dispatch — the same honest edge §3 and render-gate.md §2 hit — and
  across ~85 s of live breakpoint coverage it was **never hit**. The specific
  call Sober's log shows completing is one this build never issues in Cordial.
* `AssetProvider` fires at **startup** in Cordial. §11 lists it as join-time and
  expected absent from a startup-only run on Sober. Another instance of the
  ordering scramble §11 already names.

A methodological note kept deliberately: the first probe caused a `SIGSEGV` that
looked like an engine crash. It restored a shared `0xCC` and single-stepped only
the selected thread, leaving two others mid-prologue with `push rbp` unexecuted.
The corrected probe hit the same three-thread pattern cleanly and ran to
teardown. **The crash was the instrument.** This document already carries
findings that turned out to be the measuring apparatus; that one was caught
before it became one.

**What is not established:** which of the early-return branches `AssetProvider`
takes, and what writes the condition it tests. That is the next step and it is a
dynamic one — a breakpoint on each branch target, then a hardware watchpoint on
whatever byte the condition reads. Hardware watchpoints do not need the module
registered with the debugger, so unlike breakpoints they work here directly.

### §23.7 The function boundary was wrong, and `.eh_frame` is how to not get this wrong

§23.6 reported `RbxStorage::init` as `0x230bd3a`, with the `[INIT]` log emit at
`0x2312fbc` "further into the same function", and concluded that storage is
entered and returns early. The entry-and-return is real. The attribution is not.

`.eh_frame` carries exact function bounds and survives stripping, so this is a
lookup rather than an investigation — 260,630 FDEs, from
`readelf --debug-dump=frames-interp`:

    0x230bd3a  ->  FDE 0x230bd3a .. 0x230c74a   size  2,576
    0x2312fbc  ->  FDE 0x23121ae .. 0x2315c6a   size 15,036
    0x230bcc2  ->  FDE 0x230bcc2 .. 0x230bd3a   size    120

`0x230bd3a` is a 2,576-byte function that ends at `0x230c74a`. The log emit is in
a **different function**, `0x23121ae`. A backward walk from an address to a
function start has no way to know it has crossed a boundary on a stripped binary,
and it crossed one here — the 29 KB span that reading implies should have been
the tell.

So the fast path traced in §23.6, the `.bss` pointer written by bionic's own
`call_constructors` during `.init_array`, and the conclusion that the branch is
an unconditional singleton-getter check are all **about the getter**. They stand
as facts about `0x230bd3a` and say nothing about storage initialisation.

`0x230bcc2`, the `flagLoaded`-labelled thing, is 120 bytes ending exactly where
the getter begins — a thunk sitting in front of it, not a caller of init.

**Whether `0x23121ae` is entered in Cordial is not established.** That is now the
question, and it is a different one from the last three sections'.

**Use `.eh_frame` for this from now on.** Every address in §3, §9 and §23.6 was
derived by scanning backwards for a prologue, the build has moved at least once
underneath those numbers, and one of them was wrong by a whole function. The FDE
table is authoritative, costs one `readelf`, and is not disassembly.

## §24. Storage initialisation is a scheduled task, which is why this document is about both things

`0x23121ae` — the real `RbxStorage::init`, per §23.7's `.eh_frame` bounds — has
exactly three direct call sites and no raw pointer anywhere in `.data` or
`.data.rel.ro`:

* `0x230c3af`, inside the getter, on the branch taken when its pointer is null;
* `0x6824a78`, in a small function at `0x6824a30`;
* `0x6824b0e`, in a small function at `0x6824af8`.

And immediately before the first of those, at `0x230c393`, the getter loads a
pointer to `0x6824af8`, loads the string **`"RbxStorageInit"`** (`0x4ec600`), and
calls `0x29852f6`. That is a named task being registered, with `0x6824af8` as its
body — the third call site above. **Storage does not initialise inline. It is
scheduled.**

That is the connection this document spent twenty sections not seeing. The
question it opened with was `Can't initialize the TaskScheduler before flags have
been loaded`; the question it has been stuck on is why the content store never
comes up. They are the same question. A store whose initialiser is a scheduled
task cannot come up until something runs the task.

### Live: it never fires

`0xCC` at `0x23121ae`, `probe2.py`, own data root, four full-length runs:

    run 1  real init alone armed                      0 hits, clean exit
    run 2  real init + getter armed                   getter 22 hits, real init 0
    run 3  as run 2, repeated                         getter 22 hits, real init 0
    run 4  real init + both thunks armed              0 hits on all three

The getter's 22 hits per run carry four distinct labels — `AssetProvider`,
`http-available`, `http-write-init-only`, and **`flagLoaded`**.

**That retracts §23.6's claim that the `flagLoaded` call never happens in
Cordial.** It does, twice per run. The earlier reading was an attach race: three
runs missed it because the probe attached after it had already gone past. With
the getter instrumented for longer it appears every time. The rest of §23.6's
getter findings stand; that one does not.

So `flagLoaded` *does* reach the getter, and the getter returns its
already-constructed pointer without ever taking the branch that would register
the task. §23.6 established that pointer becomes non-null during bionic's own
`call_constructors`, running `.init_array` as part of loading the library —
before any caller exists. The slow path is therefore dead from process start.

### What is not established

Neither `0x6824a30` nor `0x6824af8` has a direct caller in `.text` or a raw
pointer in the data sections; both are reached only through indirect dispatch,
the same wall `render-gate.md` §2 and §3 already hit. Whether some third path
reaches storage init was **not found in four runs**, which is not the same as
proven absent, and is stated that way deliberately.

What would make the getter consider its object not-ready at call time is
unresolved, and answering it means reading the layout of an object that is
Roblox's, which is the line AGENTS.md draws.

### The lead this actually opens

mocktail turns the task scheduler on by JNI rather than by patching — two of its
few non-patch knobs, both on by default:
`MOCKTAIL_ASMA_START_TASK_SCHEDULER_FOREGROUND` calls
`setTaskSchedulerBackgroundMode` in foreground mode, and
`MOCKTAIL_TASK_SCHEDULER_FOREGROUND_ON_MAIN_THREAD` routes that call onto the
main thread. Cordial makes the same call — its log carries
`setTaskSchedulerBackgroundMode() enable:false context:ASMA.start` at 0.480s —
but nothing here has ever checked which thread it runs on, and mocktail
considered that worth a dedicated switch.

That is a legitimate JNI call, not a memory write, and it is the first lead in
this document that connects the scheduler to the store by a mechanism rather than
by proximity.

### §24.1 The scheduler is already in foreground mode, and the job still does not run

§24 named mocktail's two non-patch scheduler knobs as the lead. Cordial already
satisfies what they do. From an ordinary run:

    0.554648  [FLog::AndroidGLView] rbx.datamodel: setTaskSchedulerBackgroundMode() enable:false context:ASMA.start
    1.196494  [FLog::NativeDM] startLuaApp_: ... (TaskScheduler) enable-Background = false.
   30.966351  [FLog::NativeDM] pause-LuaApp: ... (TaskScheduler) enable-Background = true.

Foreground from 0.55 s, confirmed again by the data model at 1.20 s, and only
backgrounded at teardown. Giving it threads explicitly
(`FIntTaskSchedulerAutoThreadLimit = 8`) against a control changes nothing:
`RbxStorage` lines stay at zero and no store appears either way.

So the scheduler is up, in the right mode, and the `RbxStorageInit` job still
never executes. Candidate eighteen.

### Where this stops, honestly

The flags half of this document is finished: `onFlagsLoaded` fires, the
`NativeDataModelManager` chain runs to `startLuaApp_`, reproducible with a flat
control. **The store is not up and this session did not get it up.**

What is now known that was not before, all of it measured:

* `RbxStorage::init` is `0x23121ae` (`.eh_frame`, not a prologue scan), and it is
  **never entered** — four runs, `0xCC` planted, including one arming it and both
  of its unreachable callers together.
* Storage initialises as a **scheduled task**, `RbxStorageInit`, registered with a
  function pointer to `0x6824af8`.
* The getter that would register it returns early on every call, because its
  pointer is filled during bionic's `call_constructors` before any caller exists.
* Its callers include `flagLoaded`, twice per run — §23.6 said otherwise and was
  wrong.
* The scheduler is foregrounded and threaded.

Three of this document's own conclusions were retracted getting here — a function
boundary off by a whole function, a path trace blind to `statvfs`, and a channel
sweep that used the wrong value shape. Each was found by measuring something the
previous conclusion had assumed. **That is the method that has worked, and it is
what the next attempt should use** rather than the eighteen candidates already
eliminated.

The one thread with a mechanism behind it and no measurement yet: what makes the
getter's pointer null at the moment `flagLoaded` calls it on a platform where the
store does come up. Answering it means reading the state of an object whose
layout is Roblox's, which is the line AGENTS.md draws, and it should be
approached by observing that object at runtime rather than by decompiling it.

## §25. Storage init *is* entered. It runs too early, fails, and memoises the failure

§24 said `0x23121ae` is never entered, on four instrumented runs. **That is
wrong, and the instrument was the reason.** Every probe in §23 and §24 attached
to an already-running process, and the `lldb` attach handshake is slower than the
moment that matters. Launching `cordial-run` *under* lldb with
`eLaunchFlagStopAtEntry`, then breaking on Cordial's own
`mcpelauncher_linker_notifylldb` (`linker_soinfo.cpp:546`, called the instant
`libroblox.so` is mapped and before one instruction of it runs) and planting the
storage breakpoints in that same stop, changes the answer. Reproduced across six
runs.

The control is the strongest part: the same agent's own attach-based probe got
zero hits on the same getter in the same session. **The difference is the
instrument, not the engine** — which is the third time in this document a
conclusion has turned out to be a property of how it was measured.

### What actually happens

The getter's slow path runs **exactly once**, on the first call, and it takes the
**direct-call branch** (`0x230c3a7`, flag byte zero) straight into `0x23121ae`.
It never takes the schedule-a-task branch: `0x230c373` was hit zero times in
every run, and the registrar `0x29852f6` — which does fire about six times a run
for other subsystems — never once carries the storage body pointer. So §24's
"storage initialises as a scheduled task" is half right: the task branch exists
and is simply not the one taken.

The caller label at that one call is **`"RbxStorage"`**, not `"flagLoaded"`, and
it fires **during `libroblox.so`'s ELF constructors — before `JNI_OnLoad`**.
Backtrace: `notifylldb` ← `soinfo::call_constructors` ← `do_dlopen` ←
`cordial_linker_sys::dlopen` ← `load.rs`.

And it fails. Under `CORDIAL_TRACE_PATHS=1`, same thread, two runs:

    stat("./appData")    = 0
    stat("./appData")    = 0
    statvfs("./appData") = 0
    stat("")             = -1
    stat("")             = -1
    stat("")             = -1

Something it needs resolves to an empty string.

### Why that ends the investigation's confusion

This is a **memoising lazy singleton: first caller wins, permanently.** The
Waydroid capture shows Android's winner:

    [DFLog::RbxStorage] RbxStorage::init [INIT] user: flagLoaded, availableDiskSpace: 60655730688 bytes
    [DFLog::RbxStorage] RbxStorage::init [DONE] … dbOpenCount: 1

On Android `flagLoaded` wins the race and succeeds. In Cordial `"RbxStorage"`
wins it first, during ELF construction, before flags exist — and fails. When
`flagLoaded` arrives later (twice a run, per §24's own count) it is handed the
already-"initialised" broken object and never retries.

So every one of the eighteen eliminated candidates was aimed at the wrong moment.
They were all about the state at `flagLoaded` time. The decision had already been
taken and cached before `JNI_OnLoad` ran.

### The remaining question, now small

**What does the pre-`JNI_OnLoad` caller need that resolves empty?** It cannot be
a JNI or `Context` query — there is no JNI yet. It has to be something native
resolves at constructor time, and every directory Cordial sets
(`nativeSetFilesDirectory` and the rest) is set *after* `dlopen` returns, so none
of them exist yet when this runs.

`INFERRED, not established:` that the empty operand is a per-user or per-session
path component unavailable pre-flags. Nobody has established what builds that
string, and doing so by reading the binary means reading Roblox's own object
layout, which AGENTS.md places off-limits. It should be answered by observation.

The shape of a fix, if the guess is right, is that whatever the engine reads at
constructor time has to be true *before* `dlopen`, not after — which is a
different kind of change from anything tried so far, and cheap to test.

**Reusable, and the most valuable artefact here:** the launch-time race-free
breakpoint technique. `SBTarget.Launch` under a synchronous `SBDebugger` silently
free-runs without `eLaunchFlagStopAtEntry`. Any future probe of anything that
happens during library load must use it; attaching is too late, and this document
has now drawn a wrong conclusion from that twice.

### §25.1 The empty stats come from inside `RbxStorage::init`, after the `[INIT]` emit site

Walked up from the empty `stat("")` by breakpointing Cordial's own `s_stat`
shim — no disassembly needed, since Cordial owns that wrapper — and printing a
backtrace only when the path is empty. Three independent launches, identical
offsets:

    s_stat <- 0x226eea1 <- [0x226ec71|0x226f571] <- [0x231e52b|0x231e53b|0x231e547]
           <- 0x2315ced <- 0x2312fe3 <- 0x230c3b4 <- 0x230bd04  (getter slow path)

The three empty calls are three near-identical sites 16–28 bytes apart inside one
helper, which is exactly the three `stat("")` §25 recorded.

**The control is what makes this trustworthy:** the same thread, in the same
function, makes two *successful* `stat("./appData")` calls at `0x23125ef` and
`0x2312c9d`, and they go through the **same** generic leaf utility as the empty
ones. So this is not a broken subroutine — it is the same "does this path exist"
helper, called from a different point, with an empty argument. Call counts match
the original `CORDIAL_TRACE_PATHS=1` trace line for line: two `./appData`, three
empty, one thread.

### And the ordering raises a bigger question

Those addresses put the observed execution in this order inside
`RbxStorage::init`:

    0x23125ef   stat("./appData")  = 0        observed
    0x2312c9d   stat("./appData")  = 0        observed
    0x2312fbc   [INIT] log emit                not yet checked
    0x2312fe3   helper -> 3x stat("") = -1    observed

`0x2312fe3` is executed — it is a return address in the backtrace — and it is
**0x27 bytes past the `[INIT]` emit site**. `[DFLog::RbxStorage] RbxStorage::init
[INIT]` has never appeared in a Cordial engine log, at any channel setting, in any
run.

If `0x2312fbc` also executes, then that line is being emitted and swallowed, and
this project's evidence that storage "never initialises" is really evidence that
a log channel is silent — with storage in fact running well past that point and
failing later, on the empty paths.

**Not asserted.** `0x2312fe3` executing does not prove `0x2312fbc` did; a branch
between them would explain both. It is one breakpoint to settle and it is being
settled now. Recorded because the possibility changes what several earlier
sections mean, and because a channel read during ELF construction — before
Cordial's settings document is delivered at all — would be unreachable by any
flag, which would explain six clean negatives on `DFLogRbxStorage` that were read
as "storage is not running".

## §26. Storage init runs. It has been running all along, and the log was silent

`0x2312fbc` fires. Five runs, three of them fresh against a wiped data root, all
clean, all in the same order on the same thread:

    0x23121ae   RbxStorage::init entry
    0x2312fbc   [INIT] log emit          <- executed
    0x2312fe3   helper -> 3x stat("")    <- executed, fails

A backward `lea` scan confirms `0x2312fbc`'s argument is exactly
`"[DFLog::RbxStorage] RbxStorage::init [INIT] user: {}, availableDiskSpace: {} bytes, elapsed: {:.3f} ms"`,
so this is the `[INIT]` emit and not a coincidentally nearby address. The call
just before `0x2312fe3` loads the literal `"rbx-storage"`.

**So `RbxStorage::init` executes, emits its `[INIT]` line, and continues into the
empty-path failure.** The line never reaches the log.

### Why the log is silent, and why six flag runs could never have found it

Measured ordering, every run:

    loading libroblox.so …        <- RbxStorage::init runs here, in ELF constructors
    LOADED in N ms
    JNI_OnLoad
    calling GameActivity.initializeNativeCode
    bootstrapTheApp: delivering settings and flags
    nativeInitClientSettings

Settings delivery is unambiguously later than the point where storage init
already ran and failed. **No `DFLogRbxStorage` override at any value could ever
have reached it**, because the mechanism that carries flag values into the engine
fires strictly afterwards. The six clean negatives on that channel were reading a
line that had already been emitted before the flag existed.

`INFERRED` for the gating mechanism itself — that would mean reading Roblox's
logging internals — but the timing underneath it is directly measured.

### What this retracts, and it is most of the document

Every statement in §§12–24 that storage "is never asked for", "is never
attempted" or "is never reached" is wrong. It runs, on every launch, during
library load. Nineteen candidates were eliminated against a question that was
never the right one: they all asked why storage does not start, and it starts.

The three earlier retractions were each an instrument artefact — a path trace
blind to `statvfs`, a channel sweep using the wrong value shape, a function
boundary from a prologue scan, and twice an `lldb` attach that arrived too late.
This is the fourth and largest, and it has the same shape: **an absence in a log
was read as an absence in the engine.**

### The one thing left

`RbxStorage::init` fails on a path component that is empty at ELF-constructor
time, immediately after loading the literal `"rbx-storage"`. It is a memoising
singleton, so that failure is cached and the later `flagLoaded` caller is handed
the broken object.

On Android the winner is `flagLoaded` and it succeeds, so the constructor-time
call either does not happen there or finds that component populated. Which of
those is the question, and it decides the fix: either stop the early call, or
make the component non-empty before `dlopen` — the only window that exists,
since every directory Cordial sets is set after `dlopen` returns.

The `[DONE]` emit site was not located; two mechanical scans found no reference,
and pushing further would have crossed from observing into reading. So how far
init gets past the empty stats is still unknown.
