---
title: "MangoHud overlay"
description: "Draw frame rate, a frame-time graph and CPU and GPU load over the game with MangoHud."
icon: "gauge"
---
Cordial can turn on [MangoHud](https://github.com/flightlessmango/MangoHud), an open-source Vulkan layer, to draw a frame rate, a frame-time graph and CPU and GPU load over the client. It is off by default because it draws over the game.

Turn it on at **Settings → General → Performance → MangoHUD overlay**. It applies at the next launch.

The switch is only offered once MangoHud's Vulkan layer is installed. Without the layer the row is greyed out and its subtitle says what to install. (`MANGOHUD=1` with no layer is not an error, so the client would start and nothing would appear, which looks like a broken setting.)

## Install MangoHud

A host package is invisible to a Flatpak build, and the Flatpak extension exists only inside the sandbox, so install the one that matches how Cordial was installed.

<Tabs>
<Tab title="Fedora">


```bash
sudo dnf install mangohud
```


</Tab>
<Tab title="Arch">


```bash
sudo pacman -S mangohud
```


</Tab>
<Tab title="Flatpak">


```bash
flatpak install flathub org.freedesktop.Platform.VulkanLayer.MangoHud//25.08
```

Name the `25.08` branch. If `flatpak` asks which one, the `stable` branch is end-of-life and Cordial never loads it. This is a runtime extension, not a Cordial package, so Cordial's manifest needs nothing added.


</Tab>
</Tabs>

Settings checks for the layer each time it opens, so a host package is picked up by closing and reopening Settings. A Flatpak extension is mounted when the sandbox starts, so quit Cordial and start it again.

## What it shows

Cordial sets two variables on the client and nothing else:

```text
MANGOHUD=1
MANGOHUD_CONFIG=fps,frametime,frame_timing=1,cpu_stats,gpu_stats
```

That is the frame rate, the frame-time graph, and CPU and GPU load. Cordial sets the value itself so the switch turns on a known overlay and not whatever config file is lying around. At launch the shell prints `shell: MangoHUD on, via <layer path>`, or says the layer is missing if the switch is on and MangoHud has since been removed.

## Change what it shows

Cordial has no setting for it. The `MANGOHUD_CONFIG` string is fixed in `crates/cordial-shell/src/launch.rs`, and because Cordial sets it unconditionally, a `MANGOHUD_CONFIG` exported in your own environment is replaced, not merged. Changing the overlay means changing `launch.rs` and rebuilding.

<Note>

**INFERRED, not run here:** MangoHud's documentation says a config file (`MangoHud.conf`) is ignored whenever `MANGOHUD_CONFIG` is set, unless `read_cfg` is one of the options. Cordial's string does not include `read_cfg`, so expect a `MangoHud.conf` to have no effect. The installed 0.8.4 library contains the `read_cfg` and `MANGOHUD_CONFIGFILE` strings, but nobody has launched a client with a config file to see which wins.

</Note>

## What is not checked

The overlay was not screenshotted for this page and its frame cost was not measured. `cordial_screenshot` reads the frame from Cordial's own swapchain, before any layer runs, so it cannot show the overlay; a `grim` capture in a nested compositor can.

<Warning>

MangoHud is a third-party library loaded into the client's process by the Vulkan loader. Sober disabled and later restored it over anti-cheat concerns ([sober#868](https://github.com/vinegarhq/sober/issues/868)). Cordial ships neither it nor [vkBasalt](/shaders).

</Warning>

Why: [ADR-041](/adr/ADR-041-vkbasalt-post-processing).
