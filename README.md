<p align="center">
  <img src="https://raw.githubusercontent.com/luohoa97/cordial/main/packaging/banner.svg" alt="Cordial" width="460">
</p>

# Cordial (v0.18.0)

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](LICENSE)
[![Arch: aarch64 & x86_64](https://img.shields.io/badge/Arch-aarch64%20%7C%20x86__64-orange.svg)]()
[![Platform: Linux & Android](https://img.shields.io/badge/Platform-Linux%20%7C%20Android%20(Termux)-green.svg)]()
[![Graphics: Vulkan & Turnip](https://img.shields.io/badge/Vulkan-Mesa%20Turnip%20%2F%20RADV%20%2F%20ANV-red.svg)]()

Runs Roblox's official Android build **natively on Linux and Android (via Termux)**, with no emulator, virtual machine, or proot container, on **x86-64** and **aarch64**. Distributed under GPL-3.0-or-later.

---

## Highlights

* **Zero Emulation / No VM Overhead**: Executes official ARM64 machine code directly on native CPU cores using an embedded AOSP Bionic dynamic linker and JNI runtime shims.
* **Native Android Bionic Support**: Fully compatible with native Android Bionic libc inside Termux, cleanly bypassing glibc pthread wrappers to eliminate mutex deadlocks.
* **Direct Hardware Acceleration**: Native Vulkan rendering via Mesa Turnip (Qualcomm Snapdragon Adreno GPUs) or desktop Mesa drivers (RADV/ANV/NVK).
* **Responsive Desktop Input**: Smooth, interrupt-driven `epoll` looper with native 64-bit alignment, pointer capture / lock warping, held-key auto-repeat suppression, and Escape key pass-through.
* **Deep Link & Web Intent Support**: Intercepts `roblox-player://` and `roblox://` protocol handlers directly from Linux browsers (Firefox, Chromium) in Termux-X11.
* **Lightweight Headless CLI**: Fast, terminal-friendly launcher (`cordial`) replacing heavy desktop UI dependencies for low overhead and automated APK management.

---

## Demo

<p align="center">
  <img src="https://raw.githubusercontent.com/luohoa97/cordial/main/docs/media/cordial-doors.gif"
       alt="Roblox DOORS running under Cordial on Linux"
       width="560">
</p>

Roblox **DOORS**, unmodified, running on Cordial.  
[Full size](https://raw.githubusercontent.com/luohoa97/cordial/main/docs/media/cordial-doors.mp4) • [More Media](docs/media)

---

## How It Works

Cordial mmaps and relocates Roblox's unmodified `libroblox.so` using a ported AOSP Bionic linker rather than the host system linker. Every symbol the engine imports resolves as:

1. **cordial**: An Android platform behavior implemented directly inside the runtime.
2. **host**: Forwarded directly to the system libc (native Bionic on Termux, glibc on standard Linux).
3. **stub**: Explicit stubs reporting predictable failures rather than silent gaps.

`libjnivm` provides the Java Native Interface (JNI) and Dalvik/ART virtual machine shims, allowing the engine to make Java calls into the platform. Compatibility fixes live entirely within the runtime shims — **the original binary is never patched or modified**.

---

## Getting Started

### Android / Termux (ARM64 Native)

To run Cordial on an Android device inside Termux with Turnip Vulkan acceleration and Termux-X11:

👉 **[Read the Full Termux Installation & Setup Guide (INSTALL_TERMUX.md)](INSTALL_TERMUX.md)**

**Quick Summary:**
```bash
# 1. Install prerequisites in Termux
pkg update && pkg install -y x11-repo tur-repo
pkg install -y git rust clang cmake make binutils pkg-config glib pango \
  gdk-pixbuf cairo gtk4 libadwaita libx11 libxinerama libxi pulseaudio \
  mesa-vulkan-kgsl-turnip termux-x11-nightly

# 2. Clone repository & build
git clone --recurse-submodules https://github.com/o-hex/cordial.git ~/cordial
cd ~/cordial
cargo build --release -p cordial-runtime --bin cordial-run
strip --strip-unneeded target/release/cordial-run
cp target/release/cordial-run /data/data/com.termux/files/usr/bin/cordial-run
```

---

### Desktop Linux (x86-64 / aarch64)

#### From Source:
```bash
git clone --recursive https://github.com/o-hex/cordial.git
cd cordial
cargo build --release
```

#### Flatpak:
```bash
flatpak remote-add --if-not-exists cordial https://luohoa97.github.io/cordial/cordial.flatpakrepo
flatpak install cordial io.github.luohoa97.Cordial
flatpak run io.github.luohoa97.Cordial
```

---

## Usage & CLI Commands

Cordial includes a lightweight CLI launcher for managing installations and running games:

```text
Usage: cordial [COMMAND | ROBLOX-LINK] [OPTIONS]

Commands:
  run [LINK]          Launch Roblox Player directly (default)
  install [--force]   Download, verify and install Roblox APK and engine
  update [--force]    Check for updates and install the newest Roblox build
  diagnostics         Print diagnostic information
  help, -h            Show this help message

Options:
  --force, -f         Force re-download even if already up to date
  --apk <PATH>        Override Roblox APK path
  --profile <NAME>    Roblox profile to use (default: "default")
```

### Direct Join / Deep Linking
Launch experiences or private servers directly:
```bash
cordial "roblox://experiences/start?placeId=1818"
```
Or click **Play** directly in your browser (Firefox / Chromium) after registering protocol handlers (`x-scheme-handler/roblox` and `x-scheme-handler/roblox-player`).

---

## Environment Variables

| Variable | Values / Example | Description |
|---|---|---|
| `CORDIAL_PRESENT_MODE` | `mailbox` (default), `fifo`, `immediate` | Controls Vulkan swapchain presentation. `mailbox` provides tear-free lowest latency. |
| `CORDIAL_PLATFORM_NAME` | `Windows` (default), `Android` | The platform reported to the Roblox Lua engine. `Windows` enables native keyboard and mouse controls. |
| `CORDIAL_AUDIO` | `aaudio`, `opensles` | The Android audio driver layer to use. |
| `CORDIAL_AUDIO_HOST` | `pulse`, `pipewire` | The host audio backend to forward sound to. |
| `CORDIAL_RESOLUTION` | `1920x1080` (default: 1280x720) | Render resolution when not in fullscreen. |
| `CORDIAL_FULLSCREEN` | `1` | Forces fullscreen coverage on active monitor. |

---

## FastFlags Configuration

Custom ClientSettings / FastFlags live inside each profile directory:
```text
~/.local/share/cordial/profiles/<profile>/flags.json
```
For example, to configure custom frame rate limits or rendering behaviors:
```json
{
  "DFIntTaskSchedulerTargetFps": 120,
  "FFlagDebugDisplayFPS": "True"
}
```
Detailed guide: [`docs/fastflags.md`](docs/fastflags.md).

---

## Credits & Acknowledgements

Cordial is built upon the incredible work of open-source contributors and reverse-engineering pioneers:

* **[luohoa97](https://github.com/luohoa97/cordial)**: The original author of Cordial, who designed the modern open-source Android Roblox runtime architecture and reverse-engineered the core JNI/MessageBus interfaces.
* **[Sober / VinegarHQ](https://sober.vinegarhq.org/)**: Pioneers who proved that running Roblox's Android build natively on Linux was possible, providing inspiration and valuable public research.
* **[mocktail](https://github.com/mocktail)**: For WebKit WebView policies, permission bridge architecture, and textbox layout discoveries.
* **[mcpelauncher](https://github.com/minecraft-linux/mcpelauncher-manifest)**: For the foundational AOSP Bionic dynamic linker port and custom ELF relocation implementation.
* **[libjnivm](https://github.com/johndoe31415/libjnivm)**: For providing the JNI virtual machine emulation and interface layer.
* **[Mesa & Turnip Developers](https://gitlab.freedesktop.org/mesa/mesa)**: For creating the outstanding Qualcomm Adreno Vulkan driver enabling native desktop graphics on mobile SoCs.
* **[Termux](https://termux.dev) & [Termux-X11](https://github.com/termux/termux-x11)**: For providing the low-latency X11 display server and native Linux environment on Android.

---

## What Cordial Is Not

* **Not affiliated with Roblox Corporation**, endorsed by it, or approved by it. Roblox does not officially support third-party Linux/Android runtimes.
* **Does not distribute Roblox binaries**: You must supply or download your own official APK through the built-in verified downloader.
* **Not a cheat, exploit, or injector**: Cordial contains zero script execution, memory manipulation, or hooking primitives. Requests for cheating capabilities will be rejected.

---

## Licence

GPL-3.0-or-later. See [`LICENSE`](LICENSE).  
Third-party component licenses are reproduced in [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).
