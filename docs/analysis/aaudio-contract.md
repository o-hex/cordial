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

## Measured, 2026-08-22: capture, and the microphone rule under it

`AAudioStream_read` is implemented. Capture reuses `CaptureStream` — the same
class `android.media.AudioRecord` and `SLRecordItf` already record through —
and the whole of the design is *when* that class is asked to open:

| AAudio call | PipeWire capture stream |
|---|---|
| `openStream(direction=INPUT)` | **none created** |
| `requestStart` | created |
| `requestPause` | destroyed |
| `requestStop` | destroyed |
| `close` | destroyed |

Pause destroys rather than deactivates, deliberately. AAudio's pause means
"stop consuming, keep the stream", and for output that is a flag nobody can
see; for input it would be a node left in the graph with the desktop's
microphone indicator still lit. The rule at the top of `native/audio_classes.cpp`
does not admit that state, so an AAudio input pause is a stop.

The reading is not callback-driven because the engine's own symbol set says it
is not: `AAudioStream_read` is among the 25 and `AAudioStream_write` is not.
An input stream that arrives carrying a data callback is refused with
`AAUDIO_ERROR_UNIMPLEMENTED` and a line saying why, rather than opened and
never fed.

### Checked from outside, not asserted

`native/audio_probe.cpp` gained `aaudio-record` and `aaudio-record-never`,
which reach the bridge through `cordial_aaudio_symbols` — the same table the
bionic linker turns into the virtual `libaaudio.so`, so what runs is the real
path. `pw-cli ls Node | grep -c cordial-audiorecord`, sampled once a second
from another process for the whole run:

```text
T+1s audiorecord-nodes: 0     <- openStream(INPUT) has returned; nothing is open
T+2s audiorecord-nodes: 0
T+3s audiorecord-nodes: 0
T+4s audiorecord-nodes: 1     <- requestStart
T+5s audiorecord-nodes: 1
T+6s audiorecord-nodes: 1
T+7s audiorecord-nodes: 0     <- requestPause
T+8s audiorecord-nodes: 0
T+9s audiorecord-nodes: 0
T+10s audiorecord-nodes: 1    <- requestStart again
T+11s audiorecord-nodes: 1
T+12s audiorecord-nodes: 1
T+13s audiorecord-nodes: 0    <- requestStop, then close
... 0 for the remaining nine samples
```

and the run's own log:

```text
openStream(INPUT) -> 0  (capture streams open: 0)
reported: 48000 Hz, 1 channel(s), format 1 (1 == PCM_I16), burst 480 frames,
          capacity 24000 frames, state 2
read before requestStart -> -895 (must be negative: -895 is INVALID_STATE)
MIC-OPEN requestStart -> 0  (capture streams open: 1)
RECORDED 142560 frame(s) in 3.0 s, peak 0.06644 of full scale, xruns 0
MIC-PAUSE requestPause -> 0  (capture streams open: 0)
MIC-REOPEN requestStart -> 0  (capture streams open: 1)
RE-RECORDED 143520 frame(s), peak 0.08679 of full scale
MIC-STOP requestStop -> 0  (capture streams open: 0)
read after requestStop -> -895 (must be negative)
MIC-CLOSED close -> 0  (capture streams open: 0)
AAUDIO-CAPTURE-LIFETIME PASS
```

`aaudio-record-never` — open an input stream, wait, close it, never record —
held zero capture streams throughout and left zero behind.

### The peak meter, and the control that makes it mean something

A connected capture node proves as little as a connected playback one: a
stream linked to the wrong place, or to a muted source, delivers frames
forever and every one of them is zero. So the read path carries the same peak
meter the fill callback does, and it was scored against a *known* signal
rather than against a room.

`audio_probe play` puts a 440 Hz tone at 1/512 of full scale (0.001953) into
one sink; `aaudio-record` captures that sink's monitor via `PIPEWIRE_NODE` and
`stream.capture.sink=true`. Same command, same node, twice, with the only
difference being whether the tone was playing:

| monitor of the same sink | frames in 2.0 s | peak of full scale |
|---|---|---|
| 440 Hz tone at 0.001953 playing | 95 520 | **0.00192** |
| nothing playing | 95 520 | **0.00000** |

0.00192 against 0.001953 played is 98% of the amplitude, which is the sine's
crest falling between sample instants and S16 quantisation, and it is the
reading that says the path carries the signal at the right scale rather than
merely carrying something. The zero row is the control: identical frame count,
identical everything, and exactly digital silence.

Against the machine's actual microphone the two recording windows read 0.06644
and 0.08679 — different from each other, which a constant artefact would not
be. `xruns` (now `CaptureStream::dropped_bytes` in frames, which unlike the
output side is a number Cordial can honestly see) was 0 in every run.

### Playback, after the capture work, on the same harness

`audio_probe aaudio-play` installs a data callback and lets `CallbackStream`
pull it, which is the shape FMOD's own callback has. It exists because the
output path now shares a `Stream` struct and four getters with the input one,
and finding out that a getter changed by waiting for a signed-in client to
sound wrong is a slow way to find out. Twice, five seconds each:

```text
negotiated 48000 Hz, 2 channel(s), format 2 (2 == PCM_FLOAT), burst 1024, capacity 1024
PLAYED 239616 frame(s) in 5.0 s (47923 a second; the negotiated rate is 48000), xruns 0
AAUDIO-PLAYBACK PASS
```

identical to the digit on both runs, and identical to the format and burst the
four signed-in runs above negotiated.

### One thing is still not measured, and it is the one to measure next

**Nothing here shows what FMOD does with a capture stream that opens.** The
earlier runs measured FMOD asking for one input stream and carrying on when it
was refused. Whether it then calls `requestStart`, when, and whether it ever
stops, is unmeasured. A signed-out client cannot answer it: a 45-second run to
the Landing screen on 2026-08-22 logged no `openStream` of any direction and no
`AudioDevice.init` either, because FMOD is not initialised until there is
something to play. It takes a signed-in run into a place, and on the day this
was written the only signed-in profile was held by a client that was in one for
the whole session ([ADR-012](../adr/ADR-012-profiles-and-instances.md): one
instance per profile, by `flock`).

So, precisely:

* Opening the microphone without an explicit `requestStart` is structurally
  impossible, and the registry table above is the check on that from outside
  the process.
* "FMOD cannot open it by accident" is a different claim from "FMOD does not
  start it and leave it started", and only the first is established. The
  second is **`INFERRED`**, on this: the input stream FMOD opens has
  `bufferCapacity=0` and `dataCallback=no`, which is the same signature as the
  two *output* probes in the same log — both of which were closed immediately
  — and reading `getSampleRate`/`getChannelCount` off a probe needs no
  `requestStart` at all.

The next signed-in run answers it with no extra instrumentation, because every
transition already prints. One grep does it:

```bash
grep -E "input stream opened|microphone opened|microphone closed|recording (paused|stopped)" run.log
```

A `microphone opened` with no `microphone closed` behind it, in a run where
nobody spoke, is the failure this paragraph exists to look for, and it would
mean the default has to go back.

**The default was NOT flipped, and this section used to say it was.** The
argument for flipping is recorded here because it is a good one and it may yet
win: the exposure is created by implementing capture, not by defaulting to it,
since anyone following `CORDIAL_AUDIO=aaudio` runs identical code; and an unset
variable taking the common path is what gets the question answered soonest.

It lost on one point. The two failures are not symmetric. "Voice chat never
worked" is a feature gap someone reports; "Cordial is holding your microphone"
is a reason someone uninstalls it, and it is the one a user *sees*, as a lit
indicator, for a whole session. The default decides how many people meet an
unmeasured failure first, and "everyone" is the wrong answer to that for want
of a single run. So `parse_backend` returns `Backend::Java` for an unset
variable, and the comment beside it names the grep that flips it.

## How it landed

The plan was: behind `CORDIAL_AUDIO=aaudio`, off by default, measured against
the Java path in one session, and only then made default in its own commit
quoting the numbers. That is what happened up to the last step, with one
substitution — the measurement is `pw-top`'s `ERR` column and a peak meter, not
`getXRunCount`, for the reason two sections above. The last step has not
happened: the commit that would flip the default is waiting on the microphone
lifetime run, and until it lands every claim about "the default" in this file
means Java.

**AAudio is not the default; Java still is** — see the section above, which
this paragraph contradicted until it was corrected. What follows is the case
for AAudio on its merits, which is unaffected by which one is default, and the
honest summary of it is not that AAudio is better. On this machine it is not measurably better: zero `ERR` on both paths,
same rate, same clean teardown. What it is, is structurally different — no JNI
hop, no `jbyteArray` copy, no `std::deque`, no mutex between the engine and
PipeWire's callback, F32 end to end — and, since capture was implemented, the
only one of the two that can record at all. The Java path is
`CORDIAL_AUDIO=java` and remains what a host with no PipeWire session gets,
because `supportsAAudio()` answers false there regardless of this switch.

**`CORDIAL_AUDIO` now exists**, with three values — `java` (the default, and
what an unset variable means), `aaudio`, and `aaudio-refuse` — and it
announces which one it chose during startup, on the line beginning
`I/Cordial-AAudio          audio backend:`. It was previously described in
conversation as the intended design and then tried on a live run before
anything read it, which is indistinguishable from a feature that did nothing;
the startup line exists so that cannot happen again.
