# ADR-039: A runtime-backend seam is cheap to describe and not worth building yet

**Status:** accepted — no code changes
**Date:** 2026-09-24
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-002](ADR-002-core-shell-and-ui-handoff.md), [ADR-003](ADR-003-plugin-isolation.md), [ADR-007](ADR-007-host-resources-are-brokered.md), [ADR-011](ADR-011-wayland-and-libadwaita.md), [ADR-012](ADR-012-profiles-and-instances.md), [ADR-019](ADR-019-development-control-surface.md), [ADR-033](ADR-033-roblox-versions-are-a-keyed-store.md), [ADR-036](ADR-036-unsafe-is-a-boundary-not-a-convention.md), [ADR-037](ADR-037-one-lock-and-a-content-hash-for-the-build-store.md), [ADR-038](ADR-038-plugin-hot-swap.md)

## Context

The maintainer asked whether `cordial-runtime` should be made more modular so
that a second backend — Roblox's macOS client — could be added later without
rewriting `cordial-shell`. Their framing: check whether the macOS client uses
Vulkan (if not, Metal would need translating to Vulkan); if there is an
x86-64 macOS build, run it directly the way Cordial runs the x86-64 Android
build; an aarch64 macOS build on an x86-64 host would need CPU translation;
some features (the text editor) might not be needed on macOS; a Settings
picker would read "macOS or Android", with settings unavailable on one
backend shown disabled; issue templates would ask which backend was in use.

This ADR does two things. First, it reads the existing boundary between the
shell and the runtime honestly, from the code rather than from what the crate
names suggest. Second, it assesses what a macOS backend would actually
require, and recommends against designing a general backend abstraction
before that requirement is real. **No code changes accompany this ADR.**

## Today's boundary, read from the code

### The process boundary is already generic in mechanism, specific in content

`crates/cordial-shell/src/launch.rs::spawn` starts `cordial-run` as a
separate process — `Command::new(loader_path()?)` plus a fixed set of
arguments and environment variables — and that separation exists for reasons
that have nothing to do with Android: ADR-012 makes an instance a window and
a window a process, so a crash in the engine cannot take the launcher with
it. The *mechanism* (spawn a sibling binary, hand it argv and env, read its
stdout/stderr, wait for it to exit) is not Android-specific at all.

The *content* passed through it is. `spawn` always sends:

```
--lib-dir <dir> --library libroblox.so --apk <path> --host-libc
--game-activity --run <secs> --profile <name> [--join-url <url>]
```

`--lib-dir` names a directory of `lib/<abi>/*.so` objects extracted from an
Android APK; `--host-libc` and `--game-activity` are meaningless outside the
bionic-linker/AGDK world `cordial-run` implements (see
`crates/cordial-runtime/src/bin/load.rs`'s own `USAGE`, which documents both
in exactly those terms). The environment variables layered on top —
`CORDIAL_GRAPHICS` (Vulkan/GLES loader selection for `libroblox.so`'s own
`dlopen`), `CORDIAL_DEVICE_PROFILE`/`CORDIAL_PERFORMANCE`, `CORDIAL_GAMEPAD`,
`CORDIAL_AUDIO_SINK`, `CORDIAL_UNPACKED_PLUGINS` — are all read by
`android::*` modules inside `cordial-runtime` and have no meaning to any
other kind of client.

### The runtime is not only spawned — it is also *linked*

`crates/cordial-runtime/Cargo.toml` depends directly on `cordial-shell` as a
library, not only launches it as a sibling process:

```
cordial-shell = { path = "../cordial-shell" }
```

The comment on that dependency names the reason: ADR-011 makes the engine's
`wl_surface` a `wl_subsurface` of the window `cordial_shell::host_window`
builds, and "a Wayland subsurface must share a connection — and therefore a
process — with its parent." So `cordial-shell` is a library as well as a
binary, `cordial_shell::host_window` is the one place the
`AdwWindow`/`AdwToolbarView`/`AdwHeaderBar` are built, and `cordial-runtime`
links that library to get its window rather than opening its own toplevel.
This is a second, tighter coupling than the CLI/env process boundary above,
and it is load-bearing for the property ADR-011 was written to get: "that
shell and this window are the same window."

Any second backend that wants Cordial's own chrome — the same header bar,
the same theming, the same window — inherits this same constraint: it would
need to link `cordial-shell` as a library the way `cordial-runtime` does, not
merely be spawned as an unrelated process with its own top-level window. A
backend with its own separate window is possible (it gives up ADR-011's "one
window" property) but is a materially different design, closer to Sober's
shape — "a GTK process beside a toolkit-free engine process" — which ADR-011
explicitly notes "is a different arrangement that buys different things" and
"cannot produce one window with the engine inside it."

### Android-specific modules, by file

In `cordial-shell`:

| File | What is Android-shaped |
|---|---|
| `src/install.rs` | `Build { apk: PathBuf, lib_dir: PathBuf }`; `sober_apk()` names Sober's `.../packages/<ABI>/com.roblox.client/base.apk`; resolution order handles split APKs where `libroblox.so` lives in `split_config.x86_64.apk` rather than `base.apk` |
| `src/launch.rs` | The CLI contract quoted above; env vars keyed to `android::*` readers in `cordial-runtime` |
| `src/roblox_versions.rs` | The Version page, built on `cordial_update::store`'s Android-build-keyed entries |
| `src/settings.rs` | The Renderer row (`CORDIAL_GRAPHICS`, "what decides the backend is whether Cordial offers a Vulkan loader at all, since `libroblox.so` links none and `dlopen`s it") and the MangoHUD row (a Vulkan implicit layer) |
| `src/updater.rs`, `src/download_progress.rs` | Driven by `cordial-update`'s APK-shaped provider |

In `cordial-update`:

| File | What is Android-shaped |
|---|---|
| `src/apk.rs` | `HOST_ABI` and `LIBRARY_IN_APK` are compile-time `cfg(target_arch)` constants spelling Android ABI directory names (`"x86_64"`/`"arm64-v8a"`, `"lib/x86_64/libroblox.so"`/`"lib/arm64-v8a/libroblox.so"`) |
| `src/apk_signature.rs` | Verifies an Android APK's v2/v3 signing block against Roblox's pinned certificates (ADR-025) — meaningless for a codesigned macOS bundle |
| `src/store.rs` | ADR-033/037's keyed store: `~/.cache/cordial/builds/<version>/` holds one `libroblox.so` plus the archives it came from, addressed by a digits-and-dots version string scanned out of the engine and, since ADR-037, a SHA-256 of that same file |

In `cordial-runtime`: essentially the whole crate. `src/bin/load.rs`'s own
module doc says what it is — "Loads and (eventually) runs Roblox's Android
x86-64 build on Linux" — and `native/` is not incidentally Android-shaped,
it *is* the Android framework layer: `android_classes.cpp`,
`game_activity.cpp` (AGDK's `GameActivity`), five separate audio backends
answering `AAudio`/OpenSL ES (`aaudio.cpp`, `opensles.cpp`, plus the host
backends they route to — ALSA, PulseAudio, PipeWire, OSS), `accessibility.cpp`
answering an Android accessibility-service surface that ADR-019 records as
unused by the engine but still implemented. `android/mod.rs` documents "the
Android NDK APIs in `libandroid.so`" across `AAsset*`, `ANativeWindow_*`,
`ALooper_*`, `AConfiguration_*`. `cordial-linker-sys`'s own
`Cargo.toml` description is "Rust bindings to the AOSP bionic linker, built
for the host" — an ELF loader, not a generic one.

The one concrete, useful precedent already in this codebase for "different
CPU architecture, same OS" is `docs/multiarch.md`: "Execute natively when the
host ABI matches an ABI the APK ships. Do not build a translation layer."
Measured there, on 2026-08-26: `git grep -E
'__x86_64__|__aarch64__|asm volatile|target_arch' -- crates native` matches
nothing across all tracked files, because TLS layout, relocation types and
syscall numbers are upstream AOSP bionic's problem, already solved
per-architecture, not Cordial's own code. That result does not transfer to
macOS, for the reason given below.

### What is already backend-neutral

- **Profiles (ADR-012).** A directory plus an advisory `flock`. Nothing about
  `profile::acquire`/`Claim` names Android. The things *inside* a profile
  today are Android-shaped (`roblox-version`, the engine's own `appData`
  layout), but the container is not.
- **The secret store (ADR-012's later sections).** Reads and writes a
  session blob through `org.freedesktop.secrets` or a `0600` file, keyed by
  profile path. What is stored is a Roblox session cookie read out of the
  *this* engine's memory through `nativeGetCookiesForDomain`; a different
  backend would supply its own bytes through its own mechanism, but the
  store itself does not care what the bytes mean.
- **The plugin broker's effect model (ADR-007).** "The plugin sends a
  payload; Cordial performs the effect" does not reference Android anywhere
  in its reasoning. Individual capabilities are a different matter — e.g.
  `flags.write` operates on Roblox's own `FFlag`/`FInt`/`FString` system,
  which is a property of the Roblox client generally, not of the Android
  build specifically, so it likely generalises to a second Roblox client
  with little change; this is not verified against a macOS build.
- **The devctl protocol's shape, not its implementation (ADR-019).** The
  `Cmd` enum in `crates/cordial-runtime/src/devctl.rs` — `Move`, `Button`,
  `Key`, `Text`, `Scroll`, `Fullscreen` — is a small, backend-agnostic
  vocabulary. What answers it is not: screenshots come from a Vulkan
  swapchain read inside `vkQueuePresentKHR`, and input is delivered by
  calling `android::input::pass_*` directly. A second backend would need to
  implement the same socket contract against its own present path and its
  own input entry points; nothing here is shared code today because there is
  only one implementation.

## Proposing the seam, without building it

If a second backend existed, the smallest seam that would let
`cordial-shell` support it without a rewrite looks like this, in descending
order of how confident this analysis is:

1. **A backend tag threaded through the three places that currently assume
   one Android build: `install::Build`, the version/store path in
   `cordial-update`, and the profile's version pin.** Concretely,
   `~/.cache/cordial/builds/<version>/` would need to become
   `~/.cache/cordial/builds/<backend>/<version>/` (or equivalent), and
   `profiles/<name>/roblox-version` would need a sibling naming which backend
   it pins. This is a data-shape change, not an architectural one, and it is
   the same shape ADR-033 already used for versions.
2. **`launch.rs::spawn`'s argument- and environment-building split by
   backend.** The `Command::new`/piped-stdio/tail-buffer/crash-page
   machinery in `spawn` has nothing to do with Android; only the block that
   fills in `--lib-dir`/`--apk`/`--host-libc`/`--game-activity` and the long
   run of `command.env(...)` calls does. That block moving behind a
   two-armed match (or, if a third backend ever appears, a trait) is a
   contained change inside one function.
3. **A second binary that links `cordial-shell` as a library, the way
   `cordial-runtime` does today**, if the second backend wants to keep the
   Wayland-subsurface "one window" property ADR-011 designed for. This is
   the part of the seam that is not free: it is not "spawn a different
   executable", it is "build a second executable that satisfies the same
   link-time contract `cordial-runtime` satisfies today", which is real work
   independent of whatever the backend's own engine-loading problem turns
   out to be.
4. **Settings rows gated on backend capability, using the pattern already in
   `settings.rs`.** `set_sensitive(false)` is already how this file disables
   a control that does not apply — the MangoHUD row, the clear-storage
   button, several plugin rows. A `Backend::supports(Capability)` table
   consulted when building each row is a small, idiomatically consistent
   addition once there is a second backend to consult it about.

This is deliberately not a plugin system for backends, and not a trait
designed in the abstract. `cordial-plugins` already exists for the case
"third-party code extends Cordial through a narrow, capability-checked
surface" (ADR-003, ADR-007), and a runtime backend is not that: it is
first-party, it needs to reach far more of the shell than any plugin capability
does (the launch path, the version store, the window itself), and pretending
otherwise would recreate the exact anti-pattern ADR-002 corrected out of the
original proposal — "the plugin declares what, core decides how," except
here there would be no plugin, only two engines wearing the same interface.

## What a macOS client would actually require

Reasoned from public knowledge of macOS and prior art; no macOS Roblox build
was downloaded or examined, per AGENTS.md's "no Roblox code, ever" and this
task's own instruction not to fetch one. Everything in this section that is
not measured against Cordial's own code is marked **INFERRED**.

**Mach-O, not ELF — `cordial-linker-sys` does not carry over.** Cordial's
existing loader is a ported AOSP bionic linker: it mmaps and relocates an
ELF shared object, understanding `PT_LOAD` segments, `.rela.plt` and ELF
symbol versioning (see ADR-001's amendment on segment protection for how
deep that ELF-specific knowledge already runs). A macOS binary is Mach-O:
different load commands, a different segment/section model, different
relocation types, and dynamic linking normally performed by `dyld`, which
Cordial has never had to reimplement because Android already ships glibc-
adjacent ELF loading that a ported bionic linker resembles. **INFERRED:**
none of `cordial-linker-sys`'s code is reusable for Mach-O; a macOS backend
needs its own loader from nothing.

**No JNI-shaped framework layer to reuse.** `libjnivm` stands in for
Android's ART and the JNI ABI Roblox's Java side calls through. A macOS
client almost certainly uses Objective-C message dispatch and/or Swift's
ABI, calling into AppKit/Cocoa/CoreFoundation rather than a JNI-described
Android SDK surface. The *pattern* Cordial used for Android — implement the
OS API beneath the app rather than patch the app (ADR-001's "Wine
relationship") — is reusable as an approach. None of the ~32 NDK functions
in `android/mod.rs`, none of the JNI class stubs, and none of `native/`'s
Android framework code is reusable as code. **INFERRED**, by the same
reasoning as the Mach-O point: this is a second framework-layer
implementation project, not an extension of the first one.

**No Linux-kernel shortcut.** This is the asymmetry worth stating plainly,
because it is what makes "just port the same idea" misleading. Android
*already runs a Linux kernel*, which is the specific fact that lets
Cordial's symbol resolution work the way README describes it: "every symbol
the engine imports resolves as cordial..., host..., or stub" — the "host"
case exists because an Android NDK binary's libc calls are, underneath,
calls a Linux host can often answer directly (ADR-034). macOS's kernel is
XNU/Darwin, not Linux. **INFERRED:** a macOS binary's syscalls do not
resolve against a Linux host the way an Android binary's substantially do;
a Darwin syscall-compatibility layer would need building, and it is a bigger
gap than the bionic/glibc shim closes today, because that shim is bridging
two things that already agree about being Linux underneath.

**Metal, and no known Metal-to-Vulkan translation.** MoltenVK translates
*Vulkan calls into Metal*, so a portable Vulkan engine can ship on Apple
hardware — it runs in the opposite direction from what this idea needs.
**INFERRED, and not found:** a project translating Metal calls into Vulkan,
for running a Metal-targeting binary on non-Apple hardware, was not located
in general public knowledge at the time of writing. This is the opposite of
Cordial's own Android graphics story, where the trick was narrower and
already existed on the *engine's* side: `libroblox.so` tries `dlopen`ing
Vulkan and falls back to its own GLES2 path on failure, so Cordial only had
to control which loader was visible (`settings.rs`'s Renderer row). Nothing
in general knowledge suggests Roblox's macOS client carries an equivalent
built-in fallback away from Metal that a Linux host could serve natively.
If it does not, a macOS backend's graphics path is not "control which
loader is visible" but "implement or find a working Metal-semantics-on-
Vulkan translation layer," which is a categorically harder problem than the
CPU-instruction-level emulation `libbadcpu` already does for x86 feature
gaps, and multiarch.md's own principle — "do not build a translation
layer" — argues against building this one from scratch even harder than it
argued against a CPU-architecture translator, because a shader/GPU-API
translator is a larger and less well-trodden problem than instruction
translation.

**The closest prior art is Darling, and it is not there yet.** Darling
(darlinghq/darling) runs unmodified macOS binaries on Linux: Mach-O loading,
a translated Darwin syscall layer, an Objective-C runtime, and partial
Cocoa/AppKit support. **INFERRED**, from public knowledge and not from
reading Darling's source (AGENTS.md's "observe running, never copy from"
posture, applied here as "cite public reputation, don't read the code," is
the conservative reading in the absence of a running instance to observe):
Darling's GUI and graphics support has historically been the hardest and
least complete part of the project, without a mature story for a
Metal-heavy 3D application. It is the right project to watch, not a
component to borrow from at this stage — it is closer to what Sober was to
Cordial before Cordial existed: an existence proof to watch for, not yet a
green light.

**CPU architecture is the smaller problem, and only if it is even a
problem.** If Roblox still ships (or ships again) an x86-64 macOS build, a
macOS backend on an x86-64 Linux host needs no CPU translation, by the same
"execute natively when the host ABI matches" principle multiarch.md already
states for Android. **INFERRED, unverified:** whether such a build currently
exists was not checked, per this task's instruction not to fetch a macOS
Roblox build. If only an arm64 macOS build exists, CPU translation
(box64/FEX-class user-space emulation, in whichever direction is needed)
becomes a second large component stacked on top of the Mach-O/Cocoa/Metal
gap above — again something multiarch.md already declined to build for a
narrower version of the same problem ("not supported").

**Legal and ToS position is unchanged, not new.** ADR-001's "no Roblox code,
ever," "observe a running binary, never decompile it," and the absolute
in-process-hooking prohibition apply to a second binary exactly as they
apply to the first — nothing about running a macOS build instead of an
Android one weakens or strengthens that reasoning. README's existing
disclosure that Roblox does not support third-party clients and bans
accounts in waves is a property of the account and the server, not of which
official binary a given session happens to be running, so it carries over
unchanged rather than compounding.

**The honest scale comparison.** The Android port needed: a ported ELF
loader, a JNI/ART substitute, a framework layer answering roughly thirty
NDK functions plus whatever JNI classes Roblox calls (five separate audio
backend implementations for one subsystem alone), and it leans on Android
already running atop a Linux kernel for most of its libc surface. That
project is, per README's own Status section, still "Experimental," with ten
named known-broken behaviours, after what git history here shows as months
of sustained work. A macOS backend needs a Mach-O loader (new), a Darwin
syscall-compatibility layer with no Linux-kernel shortcut underneath it
(new and larger in kind, not just in degree), an Objective-C/Swift runtime
(new, no JNI analogy), an AppKit/Cocoa/CoreFoundation framework surface
(new, and broader than a mobile NDK surface because it is a full desktop
GUI/app-lifecycle stack), and a graphics path with no known translation
project running in the needed direction. **This reads as a strictly larger
undertaking than the Android port, not an incremental extension of it**,
and the one component Cordial already has confidence solving quickly for a
second target — CPU-architecture handling — is the smallest piece of the
macOS gap, not the largest.

## Recommendation

**Do not build a general backend abstraction now.** The technical shape of
what a macOS backend would need is not established well enough to design a
sound contract for it — in particular, the graphics path (Metal versus a
translation layer versus reimplementing Roblox's renderer surface directly)
is unresolved even in outline, and any trait or protocol written today would
be a guess. This project's own ADRs repeatedly reject building ahead of a
real second user of a mechanism: ADR-033 left its own rotation and
per-launch questions open rather than guess at them; ADR-038 rejected
building "a production, always-on shell-to-client channel... for this
feature" before one existed for its own reasons. The same reasoning applies
here, more strongly, because there the second user (a plugin wanting to
react to a hot swap) was concrete and near; here it is speculative.

**What is worth doing regardless of macOS**, because it pays for itself with
the Android backend alone and is cheap now versus expensive as a retrofit
later, is left to a separate decision rather than mandated by this one: the
seam described above — a backend tag on `Build` and the version store, and
splitting `spawn`'s argument-building out of its process-management
machinery — is small, and this ADR records it so a future author is not
starting from nothing. It is not being scheduled here.

**What to defer:** everything described in "What a macOS client would
actually require." None of it is worth prototyping until the graphics
question has an answer, because every other piece (the loader, the syscall
layer, the framework surface) is buildable in principle and "large but
known" the way the Android port was; the graphics question is the one place
this analysis found no known path at all, and it gates whether the rest is
worth starting.

## What would change this

- **A working, public Metal-semantics-on-non-Apple-hardware translation
  path**, of any provenance, that could plausibly sit under Roblox's macOS
  renderer. This is the single largest gate identified above.
- **Darling (or an equivalent) reaching practical support for a
  Cocoa/AppKit/Metal GUI application**, as the existence proof Sober was for
  the Android idea before Cordial started.
- **Confirmation of which macOS builds Roblox actually ships** — x86-64,
  arm64, or both — which changes whether CPU-architecture translation is
  needed at all, though not whether the larger gaps above exist.
- **Somebody willing to fund or do a framework-layer reimplementation on the
  scale of, or larger than, the Android port**, given there is no
  Linux-kernel shortcut under a Darwin binary the way there is under an
  Android one.

None of those are close. Revisit this ADR, not silently around it, when one
of them changes.

## Settings picker and issue templates

**Neither belongs now.** AGENTS.md's rule for user-facing writing —
"document what exists, never what is planned" — applies exactly as much to a
settings control and an issue-template field as to prose: a "macOS or
Android" picker in Settings describes a choice nobody actually has, and an
issue template that asks which backend a report is about implies a second
backend somebody could be running. Both would mislead a reader into
believing macOS support is real or imminent, which is precisely the failure
AGENTS.md's documentation section spends most of its length warning against.

`set_sensitive(false)` is already an established idiom in `settings.rs` (the
MangoHUD row, the clear-storage button, several plugin rows), so *building*
a disabled-when-unsupported row is not the hard part once there is a second
backend to gate on — it is idiomatically free. The reason to wait is not
implementation cost; it is that there is nothing today for the picker to
pick between.
