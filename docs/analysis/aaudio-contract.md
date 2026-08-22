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
the Java path. Cordial registers **fourteen** virtual libraries and
`libaaudio.so` is not among them, so the engine's `dlopen` fails and it falls
all the way through. Implementing AAudio would delete the JNI hop entirely:
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

## What is not settled

Whether FMOD on this build actually prefers AAudio once it resolves, and what
it does when a stream refuses to open. Both are answerable by registering
`libaaudio.so` as a virtual library whose entry points report failure honestly,
and reading what the engine does next — the same shape as
`native/opensles.cpp` returning `SL_RESULT_FEATURE_UNSUPPORTED` rather than
handing back a dead engine object.

That is the cheap first step and it should come before any PipeWire plumbing.

## How it should land

Behind `CORDIAL_AUDIO=aaudio`, off by default, measured against the Java path
in one session on `getXRunCount` and on whether a game plays sound at all,
and only then made default in its own commit quoting the numbers. The
`Performance` modes in `crates/cordial-runtime/src/flags.rs` are the precedent:
`Balanced` sets nothing and is the default until something is measured.

**Note for whoever picks this up:** `CORDIAL_AUDIO` does not exist yet. It was
described in conversation as the intended design and then tried on a live run,
where it was silently ignored — an env var nobody reads looks exactly like a
feature that did nothing.
