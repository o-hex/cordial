# MangoHUD: frame rate and load over the game

Cordial can turn on [MangoHud](https://github.com/flightlessmango/MangoHud), an
open-source Vulkan implicit layer, to draw a frame rate, a frame-time graph and
CPU and GPU load over the client. Off by default, because it draws over the
game whether or not you wanted it there: **Settings → General → Performance →
MangoHUD overlay**.

The switch is only offered once MangoHud's Vulkan layer is actually installed.
`MANGOHUD=1` with no layer is not an error; the client starts and nothing
appears, which looks exactly like a broken setting. Without the layer the row is
greyed out and its subtitle says what to install.

## Install

Which one is right depends on how *Cordial* was installed, and the two are not
interchangeable: a host package is invisible to a Flatpak build, and the Flatpak
extension's library path exists only inside a sandbox.

- **Fedora:** `dnf install mangohud`.
- **Arch:** `pacman -S mangohud`.
- **Flatpak:** `flatpak install flathub org.freedesktop.Platform.VulkanLayer.MangoHud//25.08`. Name the `25.08` branch: if `flatpak` asks which one, the `stable` branch is end-of-life and Cordial never loads it.
  A runtime extension, not a Cordial package: it mounts under
  `org.freedesktop.Platform`'s `VulkanLayer` extension point, so Cordial's
  manifest needs nothing added for it to be seen
  ([ADR-041](adr/ADR-041-vkbasalt-post-processing.md)).

Cordial looks for a `mangohud*.json` file in the Vulkan loader's implicit-layer
directories (`$XDG_DATA_HOME` or `~/.local/share`, `$XDG_CONFIG_HOME` or
`~/.config`, each of `$XDG_DATA_DIRS`, `/etc`, all under `vulkan/implicit_layer.d`)
and in the Flatpak extension's mount at `/usr/lib/extensions/vulkan/MangoHud`.
It matches on the prefix because upstream ships the file as `MangoHud.json`,
`MangoHud.x86_64.json` or `MangoHud.x86.json` depending on version.

The check runs each time Settings is opened, so a host package is picked up by
closing and reopening Settings. A Flatpak extension is mounted when the sandbox
starts, so quit Cordial and start it again.

## What it shows

Cordial sets two variables on the client and nothing else:

```text
MANGOHUD=1
MANGOHUD_CONFIG=fps,frametime,frame_timing=1,cpu_stats,gpu_stats
```

That is the frame rate, the frame-time graph, and CPU and GPU load. The value is
set by Cordial rather than left to MangoHud's default, so what the switch turns
on is a known overlay and not whatever config file happens to be lying around.
At launch the shell prints `shell: MangoHUD on, via <layer path>`, or says that
the layer is missing if the switch is on and MangoHud has since been removed.

## Changing what it shows

Cordial has no setting for it. The `MANGOHUD_CONFIG` string is fixed in
`crates/cordial-shell/src/launch.rs`, and because Cordial sets it on the client
unconditionally, a `MANGOHUD_CONFIG` exported in your own environment is
replaced, not merged.

**INFERRED, not run here:** MangoHud's documentation says a config file
(`MangoHud.conf`) is ignored whenever `MANGOHUD_CONFIG` is set, unless
`read_cfg` is one of the options. Cordial's string does not include `read_cfg`,
so expect a `MangoHud.conf` to have no effect. The installed 0.8.4 library does
contain the `read_cfg` and `MANGOHUD_CONFIGFILE` strings, but nobody has
launched a client with a config file to see which wins. Until Cordial exposes
the string, changing the overlay means changing `launch.rs` and rebuilding.

## What was not checked

The overlay was not screenshotted for this page, and its frame cost was not
measured. `cordial_screenshot` reads the frame out of Cordial's own swapchain,
which is filled before any implicit layer runs, so it cannot show the overlay
either; a nested-compositor `grim` capture, as [shaders.md](shaders.md)
describes, can.

Sober disabled and later restored MangoHud over anti-cheat concerns
([sober#868](https://github.com/vinegarhq/sober/issues/868)). The risk is the
one [ADR-041](adr/ADR-041-vkbasalt-post-processing.md) already weighs for
vkBasalt: it is a third-party library loaded into the client's process by the
Vulkan loader, and Cordial ships neither.
