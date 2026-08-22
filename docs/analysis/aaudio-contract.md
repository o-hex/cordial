# What the engine wants from AAudio

Measured on 2026-08-22 against `libroblox.so` 2.734.0.917, by reading the
dlsym name table out of the binary. Nothing here is inferred from Android's
documentation: it is the list of symbols this build actually looks up.

## Why this matters

Audio reaches PipeWire today through the Java `org/fmod/AudioDevice` class,
over JNI and jnivm. That is the layer the 2026-08-22 freeze lived in — an
AB-BA deadlock between `AudioDevice::close` and PipeWire's thread loop, fixed
in `c7215eb`, which took two days because the symptom looked like a wedged
renderer.

FMOD prefers AAudio on modern Android and falls back to OpenSL ES and then to
the Java path. Implementing AAudio would delete the JNI hop entirely:
the callback model is the same shape as PipeWire's own `process()`, so there is
no cross-thread mutex to get wrong, and the class of bug `c7215eb` fixed
becomes structurally impossible rather than avoided by discipline.

## The 25 symbols, and what is *not* there

    AAudio_createStreamBuilder
    AAudioStreamBuilder_delete            AAudioStreamBuilder_openStream
    AAudioStreamBuilder_setBufferCapacityInFrames
    AAudioStreamBuilder_setDataCallback   AAudioStreamBuilder_setDirection
    AAudioStreamBuilder_setErrorCallback  AAudioStreamBuilder_setFormat
    AAudioStreamBuilder_setInputPreset    AAudioStreamBuilder_setPerformanceMode
    AAudioStreamBuilder_setUsage
    AAudioStream_close                    AAudioStream_getBufferCapacityInFrames
    AAudioStream_getBufferSizeInFrames    AAudioStream_getChannelCount
    AAudioStream_getFormat                AAudioStream_getFramesPerBurst
    AAudioStream_getSampleRate            AAudioStream_getState
    AAudioStream_getXRunCount             AAudioStream_read
    AAudioStream_requestPause             AAudioStream_requestStart
    AAudioStream_requestStop              AAudioStream_setBufferSizeInFrames

**The absences carry more design information than the presences.**

* **No `AAudioStream_write`.** Only `_read`. Playback is therefore
  callback-driven and nothing else: the engine installs a data callback and
  expects to be asked for frames. Capture is a blocking read. Any
  implementation that expects to be written to is answering a call this build
  never makes.
* **No `setSampleRate` and no `setChannelCount`.** The engine does not request
  a format; it opens a stream, then reads back `getSampleRate`,
  `getChannelCount` and `getFormat` and adapts. That is easier for us than the
  alternative — Cordial picks what PipeWire is already running at and reports
  it honestly, and no resampling is needed on either side.
* **No device selection.** `AAudioStreamBuilder_setDeviceId` is absent, so the
  engine will never ask for a particular output on this path. Roblox's own
  device picker (`FmodAudioDevice::setOutputDevice deviceIndex`,
  `GetOutputDevices`) is populated by FMOD's backend rather than by AAudio, so
  exposing PipeWire sinks in Roblox's own settings is a separate job from this
  one and does not block it.
* **`getXRunCount` is present**, which is the measurement this work should be
  scored with. It is the engine's own count of underruns, so a before/after is
  available without inventing an instrument — and this project's history says
  an invented instrument is how four CPU "fixes" came to measure nothing.

## Correction, 2026-08-22: the gate is a Java predicate, not `dlopen`

**This document said, above, that Cordial registers fourteen virtual libraries,
`libaaudio.so` is not among them, and "the engine's `dlopen` fails and it falls
all the way through". That is wrong, and the error matters because it points
the work at the wrong place.** There is no failing `dlopen`. There is no
`dlopen` at all.

Measured on a signed-in run into place 1818 with `CORDIAL_TRACE_DLSYM=1`, which
traces every `dlopen` and `dlsym` the guest makes through the virtual
`libdl.so`. The complete list of libraries Roblox asked for, over a run that
loaded the place and played audio:

```text
[cordial-dlsym-trace] dlopen(libc.so, 2)
[cordial-dlsym-trace] dlopen(libcamera2ndk.so, 1)
[cordial-dlsym-trace] dlopen(libmediandk.so, 1)
[cordial-dlsym-trace] dlopen(libvulkan.so.1, 2)
[cordial-dlsym-trace] dlopen(libandroid.so, 0)      (three times)
[cordial-dlsym-trace] dlopen(libandroid.so, 1)
```

No audio library by any name. Sixteen trace lines for the whole run.

The actual gate is `org.fmod.FMOD.supportsAAudio()Z`, a Java static that
`native/audio_classes.cpp` answers, and which answered **false** — a
deliberate decision recorded in a comment there, on the grounds that
`libaaudio.so` did not exist. FMOD asks that first and never goes looking for
the library when the answer is no. So registering the virtual library on its
own would have changed nothing at all, and a run testing it would have looked
exactly like a run in which AAudio was tried and declined.

The two must therefore be flipped together, and they now are: both read
`cordial_audio_backend_is_aaudio()` in `native/aaudio.cpp`, which is the only
thing in the process that parses `CORDIAL_AUDIO`.

## Correction, 2026-08-22: `getXRunCount` cannot be the measurement

The bullet above calls `getXRunCount` "the measurement this work should be
scored with ... the engine's own count of underruns, so a before/after is
available without inventing an instrument". On Android that is true. Over
PipeWire it is not, and the reason is structural rather than a gap in the
implementation.

Android's number comes from AudioFlinger, which sees the client miss a
deadline. Cordial's bridge has no equivalent vantage point: PipeWire pulls, and
the engine's data callback is invoked *synchronously inside that pull*. A late
callback is therefore a late PipeWire cycle. The glitch is real and audible,
but it is counted on the server side of the socket, and neither `pw_time` nor
any other part of `pw_stream`'s API hands that count back to the client. There
is nothing for `AAudioStream_getXRunCount` to return except a number Cordial
made up.

So it returns the one underrun Cordial can honestly see — cycles it had to
fill with silence because nothing was there to fill them — which is zero in a
healthy run and stays zero. **A flat zero from it is not evidence of anything.**
The instrument that does work is `pw-top`'s `ERR` column against Cordial's own
node: it is the server's count, it is comparable across the AAudio path and
the Java one because both appear there as ordinary PipeWire nodes, and it needs
no code of ours to produce it.

This correction is written down rather than quietly worked around because the
alternative was to keep the sentence above and report a zero against it. This
project has four CPU "fixes" on record scored with an instrument that turned
out to be constant across every run, and a counter that is structurally always
zero is the purest possible form of that mistake.

## What is not settled

Whether FMOD on this build actually prefers AAudio once `supportsAAudio()`
says yes, and what it does when a stream then refuses to open. Both are
answerable with `CORDIAL_AUDIO=aaudio-refuse`, which registers the library,
answers the predicate true, and reports `AAUDIO_ERROR_UNAVAILABLE` from every
`openStream` — the same shape as `native/opensles.cpp` returning
`SL_RESULT_FEATURE_UNSUPPORTED` rather than handing back a dead engine object.
It exists as a run-time mode rather than a separate build precisely so that
the working path and the refusing one differ by one environment variable.

## Measured, 2026-08-22: four runs, one session

All four on the same machine, same build, same profile (`CordialTest`), same
place (`roblox://experiences/start?placeId=1818`), joined by `--join-url` with
no hand-driving. `ERR` is `pw-top`'s xrun column for Cordial's own node,
sampled once a second for the whole run. "Peak" is the largest absolute sample
the engine handed the data callback, as a fraction of full scale, from
`CORDIAL_TRACE_AUDIO=1`.

| `CORDIAL_AUDIO` | PipeWire node | negotiated | joined | audio | `ERR` | teardown |
|---|---|---|---|---|---|---|
| unset (`java`) | `cordial-audioplayer-0` | S16LE 2ch 48000 | yes | yes | 0 of 45 samples | clean |
| `aaudio-refuse` | **none** | — | yes | **none at all** | no node existed | clean |
| `aaudio` | `cordial-aaudio-2` | F32LE 2ch 48000 | yes | peak 0.637 | 0 of 243 samples | clean |
| `aaudio` (repeat) | `cordial-aaudio-2` | F32LE 2ch 48000 | yes | peak 0.735 | 0 of 183 samples | clean |

What the two `aaudio` runs agreed on, to the digit: PipeWire negotiated
**48000 Hz, 2 channels, `AAUDIO_FORMAT_PCM_FLOAT`, 1024 frames per burst**, and
FMOD opened four streams — three output, one input:

```text
openStream requested: direction=OUTPUT format=UNSPECIFIED bufferCapacity=0    dataCallback=no
openStream requested: direction=OUTPUT format=UNSPECIFIED bufferCapacity=0    dataCallback=yes
openStream requested: direction=OUTPUT format=UNSPECIFIED bufferCapacity=9216 dataCallback=yes
openStream requested: direction=INPUT  format=UNSPECIFIED bufferCapacity=0    dataCallback=no
```

The first two are probes, closed immediately; 9216 is FMOD sizing itself from
what the second one reported (nine bursts). The input stream is refused —
AAudio capture is not implemented — and **playback carried on unaffected for
the rest of the run**, so refusing capture costs capture and nothing else.

The callback rate is the arithmetic it should be: 11 234 callbacks delivering
11 503 616 frames over a four-minute run, 8 414 and 8 615 936 over three
minutes; both work out at 1024 frames a callback and 48 000 frames a second.
Zero cycles in either run had to be filled with silence.

**`getXRunCount` returned zero throughout, and that is not the good news it
looks like** — see the correction above. The number that carries weight is the
`ERR` column, and it is zero on both the Java path and the AAudio path. On this
machine, at this quantum, neither backend underruns. **AAudio is not measurably
better here**, and nothing in these four runs argues for making it the default.
What it is, is structurally different: no `jbyteArray` copy, no `std::deque`,
no mutex between the engine and PipeWire's callback, and F32 end to end
instead of S16 through PipeWire's converter.

### One unexplained failure, recorded rather than explained away

An earlier `aaudio-refuse` run (before `supportsAAudio()` was gated on
`pipewire_available()`, and on a build that still refused FMOD's callback-less
probe) never reached the place, logged `the engine has presented nothing for
5s after 1 frames`, and then hung in teardown. `gdb -p` put the main thread
here:

```text
#3  pthread_cond_wait@@GLIBC_2.3.2 () from /lib64/libc.so.6
#4  0x00007f47ceee2b9b in ?? ()                       <- inside libroblox.so
#12 cordial_game_activity_lifecycle ()
#13 cordial_linker_sys::game_activity::lifecycle (...) at lib.rs:1860
#14 cordial_runtime::android::looper::teardown (...) at looper.rs:957
```

**It did not reproduce.** The later `aaudio-refuse` run reached place 1818 and
tore down cleanly, as did all three others. No audio had been initialised at
all in the run that hung — no `openStream`, no `AudioDevice.init` — so there is
no evident path from AAudio to that stack. It is written down because an
unreproduced hang is still a hang, and because the next person to see this
backtrace should know it has been seen once before, on a run that had also
failed to render.

### `pgrep -x cordial-run` cannot see a running client

Not an AAudio finding, but it was found here and it matters to anyone
coordinating runs. **The engine renames the main thread**, so `/proc/<pid>/comm`
reads `Main`, and `pgrep -x cordial-run` returns nothing for a client that is
in a game with audio playing. `pidof cordial-run` matches on the executable and
works. A safety check that quietly answers "nothing is running" is worse than
no check.

## How it should land

Behind `CORDIAL_AUDIO=aaudio`, off by default, measured against the Java path
in one session on `getXRunCount` and on whether a game plays sound at all,
and only then made default in its own commit quoting the numbers. The
`Performance` modes in `crates/cordial-runtime/src/flags.rs` are the precedent:
`Balanced` sets nothing and is the default until something is measured.

**`CORDIAL_AUDIO` now exists**, with three values — `java` (the default, and
what an unset variable means), `aaudio`, and `aaudio-refuse` — and it
announces which one it chose during startup, on the line beginning
`I/Cordial-AAudio          audio backend:`. It was previously described in
conversation as the intended design and then tried on a live run before
anything read it, which is indistinguishable from a feature that did nothing;
the startup line exists so that cannot happen again.
