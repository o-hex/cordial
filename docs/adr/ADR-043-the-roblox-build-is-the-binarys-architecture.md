# ADR-043: The Roblox build's architecture is the binary's, and choosing another needs a second runtime

**Status:** accepted; decision 5 and the premise of decision 3 superseded by [ADR-053](ADR-053-vr-is-a-mode-of-the-android-runtime.md). The reasoning below is kept: it was right about a second `cordial-run` under qemu-user, which is not what the VR mode does.
**Date:** 2026-09-30
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-033](ADR-033-roblox-versions-are-a-keyed-store.md), [ADR-037](ADR-037-one-lock-and-a-content-hash-for-the-build-store.md), [ADR-039](ADR-039-a-runtime-backend-seam-and-why-macos-waits.md)
**Evidence:** [`docs/analysis/roblox-build-architecture.md`](../analysis/roblox-build-architecture.md)

## Decision

1. **The architecture stays compile-time.** `cordial_update::apk::HOST_ABI` is
   a `cfg` constant and Cordial installs the build for the machine it was built
   for. Nothing resolves it at launch and nothing re-derives it from what a
   mirror lists.
2. **Auto is the host, set and forget.** There is no `shell.json` key for the
   architecture, so a fresh profile has no state that could drift from the
   binary. Settings shows the value in a read-only "Roblox build" row, with
   "(this computer)" beside it.
3. **A non-host build is offered only once a route to run it has been measured
   to work**: a translator on `PATH`, a second `cordial-run` of that
   architecture with its userland, and a Vulkan device that is not a CPU
   rasteriser, with a frame rate taken under driven input. The spike found
   none. Until then a dropdown would list an option that changes nothing, which
   is a stub that lies (AGENTS.md) in interface form.
   *(Premise superseded by ADR-053, 2026-10-01: this assumed a non-host build
   needs a second `cordial-run` of its own architecture under a translator. The
   Quest build runs in the x86-64 `cordial-run` itself, under an in-process
   translator, with the host's own Vulkan driver. The rule that the Settings
   row offers no option it cannot run still holds for the phone build.)*
4. **When a release exists only for another architecture, the updater says
   so.** `Checked::newer_announced_than_obtainable` is the place. It cannot
   name the architecture today: it holds the announced major and the newest
   version this architecture can obtain, and the mirror is queried with
   `x-abis: x86_64` on both hosts, so an ARM-only release (2.737.1584,
   2.735.1138) is not seen at all. Wording that names both architectures needs
   the broad listing to be read first; that is follow-up, not part of this
   decision.
5. **Quest is rejected.** It is an arm64 Horizon OS build with dependencies
   Cordial has no answer for. *(Superseded by ADR-053: the x86-64 client runs
   the Quest build's engine in-process under a translator and answers its
   OpenXR and Vulkan calls with the host's own, measured at 90 frames/s on
   Monado's simulated HMD on the landing panel and 44 to 65 in game. Meta's
   Platform SDK, the dependency this sentence had in mind, is answered as a
   host without Meta services: every request fails, and the engine carries on.
   The phone build's architecture is still the
   binary's, as decisions 1 to 4 say.)*

## Why

The engine is loaded into `cordial-run`'s own address space and calls the host
by function pointer, so the other architecture's `libroblox.so` needs the other
architecture's `cordial-run`, GTK 4 and Vulkan loader, under a CPU translator.
**Measured 2026-09-30:** `vulkaninfo --summary` in an arm64 Fedora 44 container
under qemu-user, `/dev/dri` mapped, lists a single device, llvmpipe
(`PHYSICAL_DEVICE_TYPE_CPU`), so an arm64 client on an x86_64 host would render
on an emulated CPU. The spike does not separate a missing Intel driver in
Fedora's aarch64 Mesa from one that cannot reach the GPU through qemu-user;
either way there is no GPU device. FEX and box64 could not be tried without
arm64 hardware. The store is also keyed by version alone, so two architectures at one
version would collide.

*Corrected 2026-10-01:* the first sentence of this section is true of a whole
foreign `cordial-run` and was taken to be the only route. It is not. The
engine's imports can be answered across the architecture boundary by generated
thunks: the x86-64 `cordial-run` links the Quest build's arm64 `libroblox.so`,
runs it under dynarmic and hands its Vulkan and OpenXR calls to the host's own
driver and runtime ([ADR-053](ADR-053-vr-is-a-mode-of-the-android-runtime.md),
[design §9.5](../vr/dynarmic-design.md)). The qemu-user measurement above stands
for what it measured.

## Consequences

If a route is ever measured to work, this ADR is superseded: the store becomes
keyed by ABI and version, `HOST_ABI` and its neighbours become a value passed
down, and the row becomes a dropdown that lists only what a detected translator
can run. Until then the row is honest and free.

*2026-10-01:* for the Quest build that has now happened in part. ADR-053 keys
the store by ABI and version (`builds/arm64-v8a/<version>/`), and the Quest
build is reached through Settings → VR and "Play in VR", not through this row.
`HOST_ABI` and the read-only row are unchanged, because nothing translates the
phone build.

**Reopen when** a qemu-user or FEX run shows a hardware Vulkan device and a
measured frame rate. The measurements still owed are listed in section 7 of the
spike.
