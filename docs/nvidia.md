# NVIDIA graphics

**Cordial has not been run on an NVIDIA GPU.** Nobody working on it has one, so
none of this is tested on NVIDIA hardware. What follows is what people running
the same Roblox engine (through Sober) have reported, what Cordial does about
the reports it can act on, and what to try if you hit one. Where something is a
guess it says so. If you have an NVIDIA card, the steps at the end of
[`docs/analysis/nvidia-support.md`](analysis/nvidia-support.md) are the most
useful thing you can do for this page.

## What Cordial does when the GPU is NVIDIA's

It decides by the GPU the game is actually drawing on, not by whether an NVIDIA
driver is loaded, so a laptop that renders on its Intel or AMD chip is left
alone.

- **Prints which GPU it is using** at start, for every vendor:
  `[android] vulkan: physical device "..." vendor 0x10de`. `0x10de` is NVIDIA.
- **Warns about driver series 535 and 550.** On Roblox builds from June 2026
  those drivers were reported to crash the game the first time the window is
  resized. Untested here.
- **Asks again if the driver refuses to list the display's present modes**, the
  `vkGetPhysicalDeviceSurfacePresentModesKHR failed` error that some laptops
  with two GPUs hit on the first launch after boot. A guess: it may not help.
- **Says so, in a Flatpak, if the NVIDIA driver inside the sandbox does not match
  the one on your machine**, on the crash page and in `cordial --diagnostics`
  (the `Graphics` line).
- **Adds a hint to the crash page** when the game stops after any of the above,
  or after `RBXCRASH: OutOfMemory`, on an NVIDIA GPU.

It does not switch renderer, window system or GPU for you.

## Drivers

There is no minimum driver version that anyone has established, so none is
claimed here.

- **535 and 550 series:** if the game closes as the window first appears or is
  resized, update to driver 580 or newer, or set **Settings, Graphics,
  Renderer** to **OpenGL ES** and relaunch. OpenGL ES has been shown to reach the
  landing page here, not shown to be stable in play.
- **GTX 10-series and older (Pascal, Maxwell):** the 580 series is the newest
  driver they can use, so "update to 580" is the ceiling.
- **Wayland:** `nvidia-drm.modeset=1` is needed on drivers before 595 (595 turns
  it on itself). Explicit sync needs driver 555 or newer, kernel 6.8 or newer and
  a compositor that supports it (KWin 6.1, Mutter 46.1, Hyprland 0.42, Sway 1.11).
  If the game window will not appear, or the compositor drops it, try
  `CORDIAL_X11=1` for the game. The cost is that the embedded web windows do not
  attach on X11.
- The `__GL_THREADED_OPTIMIZATIONS`, `__GL_YIELD` and `__GL_SYNC_TO_VBLANK`
  variables are for OpenGL. They do nothing for this renderer; do not bother.

## Laptops with two GPUs

If the game exits at start with `vkGetPhysicalDeviceSurfacePresentModesKHR
failed`, several people got past it by using the NVIDIA GPU once after boot
before launching: `vulkaninfo`, or `switcherooctl glxgears`. It is a
once-per-boot effect in their reports.

To make the game use the NVIDIA GPU rather than the Intel or AMD one, people
report that these work (they are not Cordial settings, and untested here):

```
__NV_PRIME_RENDER_OFFLOAD=1 __VK_LAYER_NV_optimus=NVIDIA_only cordial-shell
flatpak override --user --env=__NV_PRIME_RENDER_OFFLOAD=1 \
  --env=__VK_LAYER_NV_optimus=NVIDIA_only io.github.luohoa97.Cordial
```

Check the `physical device` line in the output afterwards; if it names the wrong
GPU, nothing else about your setup matters yet.

## Flatpak

The Flatpak carries its own copy of the NVIDIA driver, as an extension called
`org.freedesktop.Platform.GL.nvidia-<version>`, and it has to match the driver
on your machine exactly. When it does not, the sandbox has only the open-source
drivers: Cordial finds no NVIDIA GPU, or quietly uses the integrated one and runs
slowly.

```
flatpak list | grep GL.nvidia        # what the sandbox has
cat /proc/driver/nvidia/version      # what your machine runs
flatpak update
```

After a driver update on your machine, Flathub can be a day behind; wait, or
install the older version. `cordial --diagnostics` prints the `Graphics` line
that compares them. There is nothing to do on the AppImage or a native package;
they use your machine's driver directly.

## Out of graphics memory

`RBXCRASH: OutOfMemoryGraphics`, some minutes into a big place. It is reported
for NVIDIA's proprietary driver on every version up to the newest checked, the
Sober maintainers consider it a driver problem, and nobody has a fix. Sober's own
warning recommends lowering the graphics quality heavily. Some people reported
success with these in `flags.json` (the names exist in the engine; the effect is
untested here, did nothing for several people, and values above 2 crashed one):

```json
{ "DFFlagTextureQualityOverrideEnabled": true, "DFIntTextureQualityOverride": 2 }
```

The "video memory: 64 MiB" line in the log is the engine's fixed figure on every
vendor, not a sign your card is misread.

## What is not done

- **`FStringGraphicsVulkanShaderMTDenyPattern`.** Another project disables
  Vulkan shader threading on NVIDIA with it. Its reason turned out to be that
  project's own libc, not NVIDIA, and nothing in Sober's reports involves it, so
  Cordial does not. It works from `flags.json` if you want to test it; the test
  is in the analysis page.
- **Patching NVIDIA's driver library** the way Sober does for one problem.
  Cordial does not modify the running client
  ([ADR-001](adr/ADR-001-in-process-hooking.md)).

## Reporting a problem

Use **Report a Problem** in the main menu, or run `cordial --diagnostics` and
paste the block, including the `Graphics` line. Add:

1. The `physical device` line from the output when you launch from a terminal.
2. `vulkaninfo --summary`, `nvidia-smi` and `cat /proc/driver/nvidia/version`.
3. For a crash, the last twenty lines of output; for a freeze, do not kill it
   yet, see [`docs/analysis/nvidia-support.md`](analysis/nvidia-support.md).

The catalogue of known NVIDIA failures, with the issue numbers, the evidence
and the test plan, is [`docs/analysis/nvidia-support.md`](analysis/nvidia-support.md).
The rule the code follows is
[ADR-046](adr/ADR-046-nvidia-is-gated-on-the-vendor-id.md).
