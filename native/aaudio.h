// The two questions the rest of the tree asks the AAudio bridge, kept in a
// header of their own so that `audio_classes.cpp` — which is compiled into a
// different static library and has no business including the whole AAudio ABI
// — can ask them without pulling in `aaudio.cpp`'s internals.
//
// One reader of `CORDIAL_AUDIO`, in `aaudio.cpp`. Everything else asks it.
// The alternative, each file calling `getenv` for itself, is how a switch
// comes to mean two different things in one process.

#pragma once

#ifdef __cplusplus
extern "C" {
#endif

/// Non-zero when `CORDIAL_AUDIO` selected an AAudio mode — either the working
/// path or the refuse-every-stream control. Both register `libaaudio.so` and
/// both make `org.fmod.FMOD.supportsAAudio()` answer true, because the
/// control is only a control if FMOD takes the same route to reach it.
///
/// **AAudio is the default**, so this answers non-zero for an unset
/// `CORDIAL_AUDIO`; `CORDIAL_AUDIO=java` is what makes it zero.
int cordial_audio_backend_is_aaudio(void);

/// Forces the one-time selection and its log line to happen now, so that the
/// backend is announced during startup rather than whenever the engine first
/// touches audio. An env var nobody prints is indistinguishable from an env
/// var nobody reads — which is exactly what happened to `CORDIAL_AUDIO` when
/// it was first tried before it existed.
void cordial_audio_backend_announce(void);

#ifdef __cplusplus
} // extern "C"
#endif
