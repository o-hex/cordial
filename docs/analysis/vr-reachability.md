# Is the compiled-in VR path reachable from Cordial's side?

2026-09-15, Roblox 2.738.0.1397, Cordial 0.15.1 (worktree build).

**No, not through anything this experiment could exercise, and the flags
produce no observable effect at all rather than hitting a nameable wall.**
`TASKS.md`'s "Closed, not deferred" verdict (dex class-count of `Oculus`,
`OpenXR`, `Cardboard`, `GvrLayout` = 0) survives, but for a different and
stronger reason than the one it gave. That reason does not apply here — Cordial
supplies the Java surface itself, so Roblox's dex having no VR classes was never
proof the engine never asks for one (see issue #35, `DeviceUtils`). What was
actually tested is what the engine asks the *platform* for with VR flags on,
and the answer is: nothing new, on any of three independent instruments.

## What was set

`docs/traces/native-flag-names.txt` — the 139 flags the real Android client
pre-registers via `nativeInitializeNativeFlags` — contains **no** VR/XR name at
all. That path was never a candidate.

`docs/traces/client-settings-flag-names.txt` — the ground-truth list of flag
names Roblox actually publishes for the Android client — has good VR coverage
(`FFlagCacheVREnabled`, `FFlagEnableOptionalVRDeviceCreation`,
`FFlagVrCheckVrEnableBeforeDeviceInit`, `FFlagExposeOpenXrAPI1`,
`FIntOpenXrASW`, `FFlagOpenXrInputRewrite`, `FFlagSupportGenericHapticsOnVR`,
and around 90 more). None of the specific binary-string names named in the task
brief — `DebugEnableVREmulator`, `IsVRAppBuild`, `DebugVRAppBuildInStudio`,
`OpenXrForWin32`, `HasEverUsedVR` and others — appear in that published list
under any `F`/`DF` prefix I tried. They may be PC/Studio-only flags Roblox never
serves to Android clients, or something other than a FastFlag (a compile-time
switch, a different settings surface). **That distinction is not resolved
here** and is worth somebody's attention before trusting those particular
names again.

The "on" arm's `flags.json` (profile `default`, own data root) set 38 keys:
every VR/XR name confirmed in the published list, plus the brief's binary
strings under both `FFlag`/`DFFlag` prefixes on the chance they are real but
unpublished, plus `FLogVRService`/`DFLogVRService`/`FLogVR` set to `"7"` to
open the VRService log channel issue #13 asks about. Full list is in the run
artefacts (not committed — see Reproducing, below).

## Control and result

Two runs, identical command, same profile shape, only the flags file present
or absent:

```
XDG_DATA_HOME=<root> cordial-run --lib-dir ~/.cache/cordial/lib/x86_64 \
  --apk ~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk \
  --host-libc --game-activity --dump-classes classes.txt --run 30
```

The override was verifiably delivered and accepted, not just written to disk:

```
off:  flags: 1 override(s) applied     (Cordial's own built-in default)
on:   flags: 39 override(s) applied    (38 from flags.json + the same built-in)

off:  nativeInitClientSettings -> 0    client settings: 1289398 bytes (cache)
on:   nativeInitClientSettings -> 0    client settings: 1290016 bytes (cache)
```

`0` is the documented success return (`client_settings.rs`), and the settings
document grew by 618 bytes — consistent with 38 extra `"FFlagName":"value"`
pairs landing inside `applicationSettings`, not with the call silently
rejecting them.

Three instruments, each compared on and off:

**1. Java surface requested (`--dump-classes`).** Byte-identical, 4584 lines
each. `diff off/classes.txt on/classes.txt` produced no output.

**2. Unresolved-JNI-symbol log.** `Constructed Unresolved symbol` appears zero
times in either run's stdout or stderr. Cordial's own end-of-run line agrees in
both: `=== nothing went unanswered this run ===`, and the stub tally is
identical too: `=== stubs called: 2 distinct of 650 ===` in both arms (the two
being `ZSTD_trace_decompress_begin`/`ZSTD_trace_compress_begin`, unrelated to
VR).

**3. The engine's own log stream (stderr, the Android-log-format lines).**
Stripping timestamps, pids and pointer values, the two runs diff to nothing
except one adjacent pair of identical-content `datamodel notification:
APP_READY` lines swapping order — ordinary scheduling jitter, not a content
difference. No `FLog::VRService` line, no `OpenXr`/`Oculus` line, no VR content
of any kind appears in either run despite `FLogVRService`/`DFLogVRService`
being set to `"7"` — the same numeric-severity form documented to work for
`DFLogHttpTraceLight` elsewhere in this codebase. **Issue #13 (which channels
take a number vs. a severity name) is not resolved by this**: with no VRService
output in either arm, there is no signal to calibrate the channel's log
verbosity syntax against, on or off.

**Retracted 2026-09-16. The fourth check below never measured what it claimed,
and its conclusion does not follow.** It read: `CORDIAL_TRACE_PATHS=1` traces
every path-taking libc call, *including `dlopen`*, and since grepping ~10,600
logged calls for `openxr`, `oculus`, `libopenxr` or any `.so` name containing
`vr` returned nothing, **the engine never attempted to open a VR runtime
library**.

`dlopen` is not traced. `native/system_paths.cpp` wraps nine calls and `dlopen`
is not among them: `stat`, `lstat`, `access`, `opendir`, `realpath`,
`readlink`, `fopen`, `open`, `statvfs`. Every `dlopen` in the tree belongs to
upstream bionic under `third_party/`. A grep of that log for a library name
could only ever have found one incidentally, through an `open` of the file --
so "returns nothing" is what this instrument prints whether the engine tried to
load a VR runtime or not, and the bolded sentence was unfalsifiable as written.

The three remaining instruments -- the dumped Java class surface, the
`Constructed Unresolved symbol` / stub-call log, and the engine's own log
stream -- are unaffected and are what this document's verdict actually rests
on. Left in place rather than deleted, because the reasoning is the point: this
is the fourth measurement in one investigation that could not distinguish the
two cases it was quoted to separate, and it produced the most confident
sentence in the file.

## Classification of the wall

Per the brief's categories: this is **"the flags do nothing observable"**, not
a Java-surface route, not a native-runtime dead end, and not a nameable
platform gate — because nothing happened for any of those to be a gate *on*.

**What this experiment did not reach, stated plainly rather than folded into
the verdict above:** every run here sits at the account-router/menu shell
(`datamodel notification: APP_READY PlatformAccountRouter`, `APP_READY
Startup`) because there is no signed-in session and no place was joined —
joining one was out of this experiment's scope. If VR capability detection
happens only when a `DataModel`/`Workspace` stands up for an actual place,
rather than at the shell/bootstrap stage tested here, this experiment would not
see it, and that path remains genuinely untested. Given that FastFlags of this
shape are ordinarily consulted very early (capability gates for a device class
are typically read once per process, not per place), and given the
already-established absence of any OpenXR/Oculus/ovr import in `libroblox.so`'s
dynamic symbol table, a null result at the place level would not be surprising
— but it is **not established** here, and is not claimed as established. Label:
**INFERRED** that the same null result would hold inside a joined place;
**observed** that it holds through the entire startup and account-shell
sequence with every VR flag this experiment could identify turned on.

## What would settle the untested part

Reaching a joined, signed-in place with a VR flag set and repeating the same
three-instrument comparison (`dump-classes`, unresolved-symbol log,
`FLog`/`DFLog` VRService output) would close the one gap left here. That needs
a signed-in test account and a place to join, both out of scope for this run.

## Four further routes, all tested 2026-09-16, all negative

This document's own hedge above is the honest one and it still stands: the
joined-place arm is untested. Everything else reachable from the shell has now
been tried, and the baseline it is measured against is a **4584-line**
`--dump-classes` dump, byte-identical across every arm of all four.

**A live OpenXR runtime present.** The earlier absence of one was an unrecorded
gap in this document, and closing it changed nothing: three arms -- flags off,
flags on with no runtime, flags on with Monado 25.1.0 serving and
`XR_RUNTIME_JSON` set -- gave an identical class dump, no unresolved symbols,
and normalised stderr streams diffing to zero lines, reproduced twice.

**The unpublished binary-string names through `flags.json`.** This document
notes at line 26 that `DebugEnableVREmulator`, `IsVRAppBuild` and the rest were
never injected. They have been now, under `FFlag`, `DFFlag`, `SFFlag` and bare,
delivered and accepted (`nativeInitClientSettings -> 0`, override counts exactly
as written), across three launches and a control. Nothing moved. Checking for a
prefix rule first showed there is none to find: 44 served names are
`FFlagDebug*` and 45 are `DFFlagDebug*`.

**The overrides channel, and this is the strongest of the four because it
carries its own positive control.** `nativeInitClientSettings`'s *second*
argument is the overrides document -- established by return-code
discrimination, malformed JSON giving `1` there and nothing in the third
position. That channel demonstrably works: `{"DFLogHttpTraceLight":"7"}`
through it alone moved the engine's own trace-line count from 0 to 47, twice,
with `HttpTraceError` unchanged as a specificity check. The VR names through
that same proven-live channel, in the same shape, did nothing.

**Calling the exported VR natives directly.** `NativeGLInterface` exports a
whole VR input surface -- `isVRVirtualInputFlagEnabled()Z`,
`enableVRVirtualInput(Z)V`, `setVRCameraCFrame(FFF)V`,
`setVirtualVRJoyStickPosition(FF)V`,
`setVRWorldRayCFrameFromScreenPosition(IFFI)V` -- all `GLOBAL FUNC` in dynsym
and all declared in the dex. Cordial is the Java side, so calling them is the
published interface working as designed, not interposition. The getter returns
`false`; `enableVRVirtualInput(true)` returns cleanly and the getter still
returns `false`; no `VRService` line appears with all three log channels at
`"7"`. These are the input half of a path that needs a device to already exist.

Taken together: the wall is not the flags, not the settings documents, not the
platform layer and not the exported natives. It is that nothing constructs a VR
device, and the only demonstrated way to make one is a byte written at a
per-build address -- which [ADR-001](../adr/ADR-001-in-process-hooking.md) and
[ADR-003](../adr/ADR-003-plugin-isolation.md) make absent rather than disabled.

**Two corrections to claims made while chasing this, both mine.** A previous
summary asserted `SurfaceController::disableVR` fires unconditionally at
startup; the string exists in the binary but never appears in the log, whose
actual shape is `HH:MM:SS.mmm D/tag message` rather than `FLog::Tag`. And the
baseline was quoted at 4572 lines in one brief; it is 4584. Neither changed a
verdict, and both are recorded because an unchecked number repeated twice
becomes a fact.

## Reproducing

```
mkdir -p <root>/data/cordial/profiles/default
# write flags.json there with the VR overrides, or omit it for the control
XDG_DATA_HOME=<root>/data cordial-run --lib-dir ~/.cache/cordial/lib/x86_64 \
  --apk ~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk \
  --host-libc --game-activity --dump-classes classes.txt --run 30 \
  >stdout.log 2>stderr.log
```

Run artefacts (`flags.json`, `stdout.log`, `stderr.log`, `classes.txt` for both
arms, and the `CORDIAL_TRACE_PATHS=1` capture) are not committed — they contain
absolute paths to this machine's home directory and are large. They were
produced under `~/cordial-vr-agent/` on this host and deleted after the counts
and greps quoted above were taken from them; the log excerpts pasted into this
document and its writing session are what is left of them.
