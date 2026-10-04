# Architectures

<!-- description: Which CPU architectures Cordial runs on, how the Roblox build is chosen, and what has and has not been checked on aarch64. -->
<!-- icon: microchip -->

Cordial runs Roblox's Android build **natively**: it loads the engine built for
the host's own architecture and never translates machine code. There is no
translation layer, and none is planned
([ADR-043](adr/ADR-043-the-roblox-build-is-the-binarys-architecture.md)).

| Host | Phone build | State |
|---|---|---|
| x86-64 | `lib/x86_64/` from the APK | Supported. x86-64-v2 (SSE4.1) is the effective CPU floor |
| aarch64 | `lib/arm64-v8a/` from the APK | Packages are published; **untested on real ARM64 hardware** |
| Anything else | | Not supported |

The architecture is fixed when Cordial is built, so a Cordial built for aarch64
installs and runs the aarch64 build of Roblox. Settings shows it in a read-only
**Roblox build** row; there is no dropdown, because choosing the other
architecture's build would change nothing Cordial can run. Builds are kept in
`~/.local/share/cordial/builds/<version>`, which is not keyed by architecture:
two architectures sharing one home directory and one version number would meet
in one directory. Cordial refuses a build whose engine differs from the one kept
under that version, by name, rather than overwriting it (INFERRED for two
architectures; not tried).

## aarch64

**What has been checked.** The code builds for aarch64, and the packaging,
release and CI jobs have aarch64 legs. A real, Roblox-signed `arm64-v8a`
`libroblox.so` was fetched through Cordial's own mirror path, its signature
verified, and loaded under qemu-user emulation (4K pages) in a one-off session:
`cordial-run` got through the bionic linker, `JNI_OnLoad`, GameActivity native
init and the engine's own flag initialisation (139 flags) before the time bound
ended, with no crash and no undefined symbol. No command line for that session
was kept, so it cannot be repeated from here.

**What has not.**

- **Anything on real ARM64 hardware.** Emulation says nothing about 16K-page
  machines.
- **16K-page kernels.** Asahi on Apple silicon is 16K, and recent Raspberry Pi OS
  defaults the Pi 5 to a 16K kernel; most postmarketOS phone SoCs and Graviton are
  4K. On 16K the linker maps the engine read-write-execute and never re-protects
  it, which bypasses the patch that makes the engine's text read-only
  ([ADR-001](adr/ADR-001-in-process-hooking.md)). Android 15 also requires 16 KB
  aligned libraries, so an `arm64-v8a` build may take that path even on a 4K host.
  A 4K aarch64 host is plausibly a mechanical job; a 16K one is not.
- **The `arm64-v8a` engine's `DT_TEXTREL` and `p_align`.** The `readelf` check
  has not been run against that binary.

## Known gaps

- **A Roblox version released only for ARM64 is not shown on x86_64 machines**,
  and on an aarch64 machine an ARM-only release newer than the current x86_64 one
  does not show as the newest. The mirror is queried for the x86_64 bundle shape
  on both architectures. Fixing it needs XAPK split-bundle support in the
  updater.
- **Arch is x86-64 only.** Arch Linux does not build for aarch64.

## VR is the one translated build

The Meta Quest build ships only `arm64-v8a`. The VR mode runs its engine under an
in-process translator (dynarmic) inside the x86-64 `cordial-run`, with the host's
own Vulkan driver and OpenXR runtime
([ADR-053](adr/ADR-053-vr-is-a-mode-of-the-android-runtime.md), [VR](vr.md)).
It is stored apart under `builds/arm64-v8a/`. It has been measured on Monado's
simulated headset only: 90 frames/s at 90 Hz on the landing panel, 44 to 65 in
game. Nothing has been measured on WiVRn or a real headset.
