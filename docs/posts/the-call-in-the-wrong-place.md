# The call in the wrong place

*How a nineteen-section investigation ended in one repeated function call, and
why six of its conclusions had to be withdrawn first.*

Cordial runs Roblox's Android client natively on Linux. Not in an emulator, not
in Waydroid — the actual `libroblox.so`, loaded by a ported AOSP bionic linker,
with a shim mapping bionic onto glibc and libjnivm standing in where Android's
ART would be. It signs in, loads an experience, and you can play it.

For weeks it could not do one thing: bring up the engine's content store. Every
asset came off the network every session. Loading was either slow and complete
or fast and untextured, and which one you got felt like weather.

## The symptom, and the wrong question

The engine reports its flag state to the host application through one of two
callbacks: `gameActivity_onFlagsLoaded(ByteBuffer)` on success, or
`gameActivity_onFlagsFailed()` on failure. Cordial got the failure one. Twice,
every launch, for months.

Downstream of that, nothing: no `ClientRunInfo`, no `RbxStorage::init`, none of
the block a real Android client produces in the same window. And a related
crash, when the call ordering was changed to match another implementation's,
came back as a memory fault with no message at all.

So the question everyone asked was *why do the flags fail to load?* Fifteen
candidates were eliminated against it, each with a control: the settings
document's contents and shape, its timing, the device identity, the channel
platform name, an empty `ArrayList`, every flag that routes storage, four
settings-document variants, the `Configuration` object. Each elimination was
honest and each was measured. None of them moved anything, because the question
was wrong.

## The first real break was making a crash speak

Three separate engine calls were faulting at address `0x8` inside glibc's
`_IO_fflush`. That address is what you reach when something dereferences a
pointer field a few bytes into a zeroed `FILE`.

Pre-API-23 bionic exports `FILE __sF[3]` — the legacy `stdin`/`stdout`/`stderr`
array. Cordial supplied zeroed storage for the symbol so that it resolved, with
a comment saying honestly that this was half the job and the other half was only
worth building once something was observed using it.

Something was. Ten wrapper functions later, mapping the three legacy slots onto
the host's real streams, the segfaults became sentences. One of them said:

    RBXCRASH: FatalRuntimeError
      (Can't initialize the TaskScheduler before flags have been loaded)

The engine had been saying that all along and it was arriving as a memory fault.
Nothing about a crash at `0x8` in `_IO_fflush` points back at legacy stdio, which
is why it survived so long.

## The answer was that two calls had never been separated

Android's client calls `nativeInitClientSettings` to hand the engine its flags,
then `nativePostClientSettingsLoadedInitialization3` to finish the handshake.
Cordial called both, in that order, from inside the engine's own
`bootstrapTheApp` callback.

An earlier session had recorded, precisely, that the second call "returns without
the engine's own body of it having run" — and then spent five sections looking
for a missing argument, a missing prerequisite, a wrong document. The body was
fine. It was being called too early, before the engine had opened its own log,
and it quietly did nothing.

Every ordering experiment had moved the two calls **together**, because they are
conceptually one handshake. Moving them together late is a different failure —
it dies on the TaskScheduler gate before the second call matters. So the one
arrangement nobody tried was: leave the settings where they are, and repeat the
post call later, after the surface is handed to the engine. Then
`nativeRetryInit`, which Cordial already called but only early, re-runs init
against that new state.

    [FLog::NativeDM] initialize: state:11. areFlagsLoaded:true.
    [FLog::NativeDM] getFlagsFromEngine_:
    [FLog::NativeDM] bootstrapTheApp_:
    [FLog::Output]   settingsUrl: …/settings-compressed/application/GoogleAndroidApp.zst
    [FLog::NativeDM] … getFlags: success = true, payload's size = 1300800.
    [FLog::NativeDM] continueAfterFlagsLoaded_:
    [FLog::NativeDM] initEngine_:  →  initializeLuaApp_:  →  startLuaApp_:

And on Cordial's side, for the first time, `flags loaded (1300800 bytes)`. Three
consecutive runs, with the switch off as a control: `flagsLoaded=0`,
`continueAfter=0`.

## What it cost to get there was six retractions

This is the part worth writing down, because it is the transferable bit.

**Every wrong conclusion in that investigation was an instrument, never the
engine.** Six of them:

1. A `CORDIAL_TRACE_PATHS` run concluded storage was never attempted — 19,296
   intercepted path calls, none naming it. The tracer did not wrap `statvfs`,
   which is the call storage actually makes.
2. A sweep set 135 log channels "to maximum" and concluded loud logging was
   exhausted. Channel values are heterogeneous: most take a number, a minority
   take a severity name, and giving one the wrong shape *silences* it. The sweep
   had partly turned logging off.
3. A function boundary came from scanning backwards for a prologue and crossed
   into the previous function. `.eh_frame` carries exact bounds, survives
   stripping, and costs one `readelf`.
4. and 5. Two `lldb` attaches concluded a function was never entered. The attach
   handshake is slower than the event. Launching with `eLaunchFlagStopAtEntry`
   and breaking as the library is mapped gives the opposite answer.
6. A timing measurement under a halting debugger contradicted the source — an
   unconditional 250 ms sleep appeared already complete 150 ms after launch. And
   separately, stdout and stderr have different buffering when redirected, so
   two traces interleaved in one file are not a timeline.

Reading that list back, the pattern is not that people were careless. Each
conclusion was drawn from a real measurement, by someone being careful. The
failure mode is subtler: **an absence in an instrument reads exactly like an
absence in the thing being measured**, and nothing in the output distinguishes
them. The only defence is a control that makes the instrument prove it can see
the thing at all.

## The half that is still not solved, and why that is fine to say

The content store still does not come up. Twenty-one candidates are eliminated.
What is now established, rather than assumed: storage init *does* run, on every
launch, on an engine-internal thread-pool worker; it fails on a path component
that is empty; and it memoises the failure, so the caller that succeeds on
Android is later handed the broken object and never retries. No JNI call happens
at the moment of failure, and neither the spawning site nor the worker
corresponds to any entry point the interface exposes.

Which makes it, as far as anything can currently show, not reachable from a host
application — and the remaining route is the memory write this project
deliberately does not have. That is a decision about what Cordial is willing to
be, not an engineering problem, and it is not one to make quietly at the end of a
long day.

Writing that down honestly is worth more than a nineteenth guess. The other half
of the same investigation turned out to be an ordering mistake in our own code,
and only measurement told the two apart.
