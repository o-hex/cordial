# Shaders (vkBasalt)

<!-- description: Add sharpening and anti-aliasing over the game with vkBasalt, an open-source Vulkan layer. -->
<!-- icon: palette -->

Cordial can hand the client's frame to [vkBasalt](https://github.com/DadSchoorse/vkBasalt) for a sharpen pass (CAS) and an anti-alias pass (SMAA) before it reaches the screen. It is off by default because it changes what is drawn.

Turn it on at **Settings → General → Performance → Shaders (vkBasalt)**. It applies at the next launch. The switch is only offered once vkBasalt is installed.

## Install vkBasalt

Which one you need depends on how Cordial was installed. A host package is invisible to a Flatpak build.

<!-- tabs -->

### Fedora

```bash
sudo dnf install vkBasalt
```

On an immutable host, run it in a `distrobox`, or layer it with `rpm-ostree`.

### Arch

```bash
sudo pacman -S vkbasalt
```

Add the multilib package for a 32-bit game.

### Flatpak

```bash
flatpak install flathub org.freedesktop.Platform.VulkanLayer.vkBasalt//25.08
```

Name the `25.08` branch. If `flatpak` asks which one, the `stable` branch is end-of-life and Cordial never loads it. This is a runtime extension, not a Cordial package, so Cordial's manifest needs nothing added.

<!-- /tabs -->

Settings looks for the layer each time it is opened, so a host package is picked up by reopening Settings. A Flatpak extension is mounted when the sandbox starts, so quit Cordial and start it again.

## Change the effects

The first time you turn the switch on, Cordial writes `<profile>/vkBasalt.conf` in that profile's data directory with sharpening and anti-aliasing at vkBasalt's documented defaults. **Cordial never rewrites this file again.** Edit the effects list, the sharpening strength or anything else vkBasalt supports and your changes stay. Once vkBasalt is detected, the settings row names the exact path for your profile.

The full key reference is vkBasalt's own: [`vkBasalt.json.in`](https://github.com/DadSchoorse/vkBasalt/blob/master/config/vkBasalt.json.in).

## Toggle the effect on and off

The generated config sets `toggleKey = Scroll_Lock`, not vkBasalt's own `Home`. Home is a Roblox chat key (it jumps to the start of a line), and vkBasalt does not care which window has focus, so Home would toggle the effect every time you typed a message.

> [!NOTE]
> On Cordial's default Wayland backend the toggle key does nothing. vkBasalt reads a real X11 keyboard and only when `$DISPLAY` is set, and a Wayland session sets `WAYLAND_DISPLAY` instead. The generated config sets `enableOnLaunch = True` for this reason: the effect is on from the start and stays on. The key works on Cordial's X11 backend, or when XWayland is running alongside a Wayland session.

## What is checked and what is not

- **The layer loads.** With `ENABLE_VKBASALT=1` and `VKBASALT_LOG_LEVEL=info`, the Vulkan loader inserts `VK_LAYER_VKBASALT_post_processing` and vkBasalt logs the config Cordial generated.
- **The effect is visible** on the landing screen, where edges are visibly sharper. The difference is modest on that mostly flat screen, and it was not checked against in-game 3D content.
- **Cost:** on the landing screen, CPU for the whole client went from about 6.7% with shaders off to 7.0 to 7.1% with them on. That is one screen and one input pattern, not a claim that vkBasalt costs nothing.
- **Untested:** AMD and NVIDIA GPUs. Everything above ran on the Mesa driver.

> [!WARNING]
> vkBasalt is a third-party library loaded into the client's process by the Vulkan loader. Sober disabled and later restored MangoHud and vkBasalt over anti-cheat concerns ([sober#868](https://github.com/vinegarhq/sober/issues/868)). Cordial accepts the same risk it already accepts for [MangoHud](mangohud.md) and ships neither.

Why: [ADR-041](adr/ADR-041-vkbasalt-post-processing.md), which also holds the measurements.
