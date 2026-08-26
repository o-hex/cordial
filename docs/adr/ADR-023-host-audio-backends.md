# ADR-023: PipeWire is the primary audio backend, and the others go behind a seam

**Status:** accepted
**Related:** [ADR-011](ADR-011-wayland-and-libadwaita.md), [`docs/multiarch.md`](../multiarch.md)

## Decision

**PipeWire stays the primary and default host audio backend.** Alongside it,
the three stream classes in `native/pipewire_backend.h` move behind a backend
interface, selected by a new `CORDIAL_AUDIO_HOST` variable, with PipeWire as
the only implementation until that refactor is proved to change nothing.

Then, in this order: **PulseAudio**, **ALSA**, and **OSS only if somebody with
FreeBSD hardware will run it**.

Each backend is `dlopen`'d, never linked, and its headers are optional at build
time — the arrangement `pipewire_backend.cpp` already uses, so no new hard
dependency is added by any of this.

## Why this is being decided at all

A user asked for it, in these words: *"pipewire-only, no ALSA nor OSS Support,
no FreeBSD support, wayland only"*. The first clause is accurate. Cordial has
three output paths into the engine — AAudio's pull callback, OpenSL ES and
FMOD's Java `AudioDevice` push path, and capture — and every one of them ends
in PipeWire. On a desktop running PipeWire this is invisible, because
`pipewire-pulse` and the ALSA plugin cover PulseAudio and ALSA clients. On a
machine running PulseAudio or bare ALSA, Cordial is silent.

Sober, which is the reference implementation this project studies, links
PulseAudio (`docs/analysis/sober-input-stack.md`). That is a data point about
what the people who already run Roblox on Linux have, and it is why PulseAudio
comes before ALSA rather than after it.

## Why the seam comes first, on its own, changing nothing

`CallbackStream` is a concrete pimpl class that `aaudio.cpp` holds **by value**,
and the only polymorphism in the file today is a compile-time `#ifdef` with an
honest "no audio" arm. There is no interface to implement against, so the first
version of any second backend would arrive tangled with the refactor that made
room for it — and then a silent regression in PipeWire, which every current user
depends on, would be indistinguishable from a bug in the new code.

**A refactor that changes nothing is the one refactor that can be proved.**
`cordial_pipewire_backend_test` and `audio_probe` both exist and both pass
today; they are the control, and they must still pass with PipeWire reached
through the seam before a line of PulseAudio is written.

## The one-way door, which is the real risk here

`native/aaudio.cpp` records it: **once FMOD has been told AAudio exists, there
is no way back.** A control run with `CORDIAL_AUDIO=aaudio-refuse` answered
`supportsAAudio()` true and then failed every `openStream`; FMOD tried twice and
abandoned audio for the rest of the process — no `AudioDevice.init`, no
`slCreateEngine`, no node of any kind, and a place that loaded and played in
silence.

Every new backend sits behind that commitment. So **each one must have an
availability probe as cheap and as trustworthy as PipeWire's `pw_core_sync`
round trip, and `supportsAAudio()` must consult it before the door closes.** For
ALSA that means opening and closing a PCM at probe time, which is neither free
nor reliably repeatable. A backend that cannot answer "am I really going to
work" honestly is a backend that trades silence-with-a-reason for
silence-without-one, and AGENTS.md's rule about stubs that lie applies exactly.

The instrument problem compounds it. `getXRunCount()` reads zero on output
whether the stream is perfect or underrunning every 40 ms — the file says so, and
says it is there so the counter does not become this project's fifth constant
mistaken for a measurement. What does discriminate is `CORDIAL_TRACE_AUDIO=1`'s
peak meter and `pw-top`'s error column, and the second of those is PipeWire's.
**Under ALSA or OSS we would be shipping with fewer instruments than the path we
are copying.** Whatever lands must bring its own way of telling working audio
from silent audio, or it is not finished.

## What is rejected

**Adding `alsa`/`pulse`/`oss` as values of `CORDIAL_AUDIO`.** That variable
selects which *Android* API FMOD reaches Cordial through — AAudio, OpenSL, or
FMOD's Java path — and every combination of those with a host backend is
meaningful. One variable for two orthogonal axes is a variable nobody can
document. Hence `CORDIAL_AUDIO_HOST`, parsed in one place, announced at startup
the way the existing one already is.

**Trusting `pkg_check_modules` for ALSA or PulseAudio.** On the machine this was
written on, `pkg-config` first on `PATH` is Homebrew's, whose search path is its
own Cellar. For PipeWire that failed loudly and `find_path` rescued it. For
these two it *succeeds*, resolving to Homebrew headers for a library that is not
the system one — a silent wrong answer, which is worse. The `find_path` fallback
pattern in `native/CMakeLists.txt` is mandatory here, not optional.

**OSS as a route to FreeBSD.** It is not one, and saying so to the person who
asked is more useful than shipping it. Cordial is AOSP's bionic linker over a
Linux syscall and `epoll` shim, in a GTK4/Wayland window fixed by ADR-011, and
`docs/multiarch.md` restricts supported targets to Linux. Audio is not what
stops FreeBSD; approximately everything else is. An OSS backend would be a
correct answer to a question nobody is actually asking.

## Consequences

**Verification is asymmetric, and that has to be stated in each commit rather
than discovered later.** On the development machine PulseAudio can be exercised
against `pipewire-pulse` — which is PipeWire's reimplementation of the protocol,
not the PulseAudio daemon, and differs in latency reporting and underflow
callbacks. ALSA can be exercised against `default`, which routes through
PipeWire's ALSA plugin and therefore proves the `snd_pcm_*` sequencing but not
that real hardware is driven; `hw:0,0` is reachable only by taking the device
away from the running session. OSS cannot be exercised at all: no `/dev/dsp`, no
`snd-pcm-oss` in this kernel, and OSSv3-through-ALSA would not transfer to
OSSv4-on-FreeBSD if it existed.

So PulseAudio ships "verified against pipewire-pulse", ALSA ships "verified
against the PipeWire ALSA plugin, and once against hardware", and OSS ships
`INFERRED` and never-executed or it does not ship. AGENTS.md already requires
this; writing it down here is so that nobody has to decide it under pressure at
review time.

**`CORDIAL_AUDIO_SINK` does not port cleanly, and one property of it is load-
bearing.** Empty means "follow whatever the session calls the default sink, and
keep following it" — a standing instruction, not a snapshot, so changing the
default sink mid-game moves the stream. PulseAudio keeps that property exactly.
**ALSA and OSS cannot have it**: `default` is resolved once at `snd_pcm_open`,
and nothing moves a running stream. The device-name string is not portable
either — a PipeWire `node.name` is not a PCM name. The shell's sink picker walks
PipeWire's registry through a C ABI, so under another backend it goes stale
unless that ABI grows an implementation too.

**The picker is part of the feature, not a follow-up.** A backend that plays
sound but leaves the user's output device menu empty or wrong is half a feature,
and the half that is missing is the one people notice.

## ALSA cannot work in the Flatpak, and that is not going to be fixed

Asked directly on 2026-08-26 -- should ALSA be dropped and the backends be
PipeWire and PulseAudio only? -- and the answer turns on a fact about the
sandbox that was not written down anywhere.

**The Flatpak has no route to `/dev/snd`.** `packaging/io.github.luohoa97.Cordial.yml`
grants `--socket=pulseaudio` and the native PipeWire socket, and `--device=dri`
and nothing else. `alsa_available()` proves a device by opening a PCM and
closing it, which is the right probe and which cannot succeed against a
directory that is not in the sandbox. So for everybody installing Cordial the
recommended way, `CORDIAL_AUDIO_HOST=alsa` is a variable that reports it did not
get what it asked for.

**The grant it would need is `--device=all`.** Flatpak has no narrower lever --
there is no `--device=snd` -- so reaching the sound devices means every device
node in `/dev`, cameras and input devices included. The manifest already refuses
that exact widening for FIDO2, in a comment arguing that a permission granting
far more than the capability needs is a permission that lies. That reasoning
does not get weaker because the capability this time is audio.

**ALSA stays anyway, and the reason is the users the Flatpak is not for.** The
AUR package, an RPM, and `cargo build` all produce a Cordial with no sandbox
around it, and a machine with no sound server at all is exactly the case ALSA
exists to answer. It is opt-in, it is probed before it is trusted, it compiles
out entirely without `alsa-lib-devel`, and it costs nothing on a machine that
never names it. Deleting a working fallback to tidy the backend list would take
sound away from the only people who need that fallback.

What was wrong was leaving this undocumented, because the failure it produces is
the expensive kind: the variable is accepted, the probe fails, and the user gets
PipeWire with no indication that the sandbox was the reason. Anybody debugging
ALSA under the Flatpak should be sent here in the first minute rather than the
second hour.

## OSS is built, and the reasoning that deferred it was about who had spoken up

This document said OSS was "an answer to nothing" and scheduled it behind
everything else, on the grounds that Linux has not used OSS as its sound layer
since ALSA replaced it and the remaining users are on FreeBSD. The "what would
change this" section below named the condition: somebody arriving who needs it.

Somebody did, on 2026-08-26. So it exists, and the old reasoning stays above
rather than being quietly deleted, because it turned out to be a claim about who
had asked rather than about who was there.

**It is the simplest backend here and the only one that works with no sound
daemon at all.** No client library to find, no server to connect to, no node
graph: `/dev/dsp`, three `ioctl`s to agree a format, then `write()`. PipeWire
and PulseAudio both need something running and ALSA needs a configured `default`
PCM, so this is the one that serves a machine with none of that.

Three things about it are worth knowing before anybody selects it:

- **The device is exclusive on most drivers.** While Cordial holds `/dev/dsp`,
  nothing else on the machine gets sound. That is what the interface is, not a
  bug to work around, and it is why this is opt-in behind
  `CORDIAL_AUDIO_HOST=oss` and never chosen for anybody who did not ask.
- **Even the probe takes the device**, briefly, for the same reason. It is
  cached so it happens once.
- **OSS speaks integer PCM.** The engine's float frames are converted to
  `AFMT_S16_NE` on the way out, on the audio thread -- a multiply and a clamp
  per sample, allocating nothing, per `pipewire_backend.h`'s realtime rule.

The negotiated rate and channel count are read back out of the `ioctl`
arguments rather than assumed, because OSS negotiates by writing through them:
a driver that only does 44100 says so there and nowhere else, and reporting the
requested rate instead is how a stream ends up playing at the wrong speed with
nothing in the log.

### What has not been verified

**Playback.** This was written and tested on Fedora, which ships no OSS
emulation and has no `/dev/dsp`, so `oss_available()` correctly answers no here
and no audio has been produced through this path by anybody yet.

What *is* verified on this machine: it compiles into the build, `find_path`
detects `sys/soundcard.h`, `CORDIAL_AUDIO_HOST=oss` selects it, the probe
reports honestly, and the fallback to PipeWire announces itself and names
`CORDIAL_AUDIO_DEVICE` as the escape hatch:

```text
W/Cordial-Audio  CORDIAL_AUDIO_HOST=oss, but /dev/dsp would not open
                 (set CORDIAL_AUDIO_DEVICE for another node); using pipewire.
host_backend_name(): oss
oss_available():     NO
```

Everything from `open()` onwards is **`INFERRED`** and labelled as such here
rather than in a commit message nobody will find. The first person with a real
OSS device is the test.

## What would change this

A measurement showing PipeWire's ALSA plugin or `pipewire-pulse` failing for
real users in a way a native backend fixes would reorder the work. So would
somebody arriving with FreeBSD hardware and the appetite to carry the rest of
the port — at which point OSS stops being an answer to nothing.
