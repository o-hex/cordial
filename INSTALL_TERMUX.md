# Native Roblox on Android (Termux) via Cordial & Turnip Vulkan

This guide walks you through compiling, configuring, and running **Roblox natively on Android** inside Termux with Turnip Vulkan hardware acceleration, raw PC controls spoofing, and mouse pointer capture.

No proot, no chroot, no emulation layer. The official ARM64 Android engine executes directly on your device's native CPU with direct GPU rendering via Mesa Turnip.

---

## Prerequisites

1. **Android Device**:
   - 64-bit ARM (`aarch64`).
   - Qualcomm Snapdragon processor recommended for optimal performance using the **Turnip** Adreno Vulkan driver (Adreno 6xx / 7xx / 8xx series).
2. **Termux**:
   - Install the latest Termux release from [GitHub Releases](https://github.com/termux/termux-app/releases) or F-Droid. *(Do not use the obsolete Google Play Store version).*
3. **Termux-X11**:
   - Install the companion [Termux-X11 APK](https://github.com/termux/termux-x11/releases) for hardware-accelerated X11 display output and mouse pointer capture.

---

## 1. Install Required Packages

Launch Termux and run the following to install all compilers, build tools, graphics drivers, and X11 libraries:

```bash
# Update repositories
pkg update && pkg upgrade -y

# Enable X11 and TUR package repositories
pkg install -y x11-repo tur-repo

# Install build tools, compilers, and dependencies
pkg install -y \
  git rust clang cmake make binutils \
  pkg-config glib pango gdk-pixbuf cairo gtk4 libadwaita \
  libx11 libxinerama libxi pulseaudio \
  mesa-vulkan-kgsl-turnip termux-x11-nightly xfce4
```

---

## 2. Clone the Repository

Clone this repository with all submodules (including the ported Bionic linker and ART JNI shims):

```bash
cd ~
git clone --recurse-submodules https://github.com/o-hex/cordial.git
cd cordial
```

---

## 3. Build & Install Cordial

This fork includes dedicated flash storage wear mitigations (`debug = false`, `strip = "symbols"`, and compiler `-pipe` flags) to prevent intermediate build artifacts from degrading internal UFS storage.

Compile the release binary:

```bash
cd ~/cordial
cargo build --release -p cordial-runtime --bin cordial-run
```

Once compilation completes, strip the binary, install it to your user path, and clean the build cache:

```bash
# Strip unneeded symbols (reduces binary size to ~12 MB)
strip --strip-unneeded target/release/cordial-run

# Install binary to Termux bin directory
cp target/release/cordial-run /data/data/com.termux/files/usr/bin/cordial-run

# Free up disk space immediately
cargo clean
```

---

## 4. Configure Termux-X11 for Gaming

Run the following commands in Termux to configure optimal input and display settings in Termux-X11:

```bash
# Enable pointer lock / mouse capture for first-person and camera drag
termux-x11-preference pointerCapture:true

# Prevent Termux-X11 from intercepting Escape key (prevents breaking mouse capture)
termux-x11-preference pauseKeyInterceptingWithEsc:false

# Set captured pointer speed factor to 100%
termux-x11-preference capturedPointerSpeedFactor:100

# Optional: Set preferred display scaling and full screen
termux-x11-preference fullscreen:true
```

---

## 5. Create the Launch Script

Create a convenient launch command at `/data/data/com.termux/files/usr/bin/roblox`:

```bash
cat << 'EOF' > /data/data/com.termux/files/usr/bin/roblox
#!/data/data/com.termux/files/usr/bin/bash
set -e

export DISPLAY="${DISPLAY:-:0}"
export GDK_BACKEND=x11
export CORDIAL_AUDIO=java
export PULSE_SERVER="${PULSE_SERVER:-127.0.0.1}"
export CORDIAL_PLATFORM_NAME="${CORDIAL_PLATFORM_NAME:-Windows}"
export CORDIAL_PRESENT_MODE=immediate
export CORDIAL_POLL_TIMEOUT=1

# Start PulseAudio daemon if not running
if ! pulseaudio --check >/dev/null 2>&1; then
    pulseaudio --start --load="module-native-protocol-tcp auth-ip-acl=127.0.0.1 auth-anonymous=1" --exit-idle-time=-1 >/dev/null 2>&1 || true
fi

exec /data/data/com.termux/files/usr/bin/cordial-run \
  --lib-dir /data/data/com.termux/files/home/.cache/cordial/lib/arm64-v8a \
  --apk /data/data/com.termux/files/home/.cache/cordial/build/arm64-v8a/base.apk \
  --game-activity \
  --run 0 \
  --profile default \
  "$@"
EOF

chmod +x /data/data/com.termux/files/usr/bin/roblox
```

---

## 6. How to Play

1. Start your X11 session (e.g. via Termux-X11 and XFCE or directly):
   ```bash
   # Launch Termux-X11 display
   termux-x11 :0 &
   ```
2. Launch Roblox:
   ```bash
   roblox
   ```
3. **First-time Setup**:
   - On the first launch, click **Download Roblox** to download and verify the official ARM64 Android engine.
   - For web authentication, Cordial will automatically open your default Android web browser via `termux-open-url`. Complete sign-in in your browser and you will be returned directly to the game.

### Controls & Tips:
* **Fullscreen**: Press `F11` to toggle borderless fullscreen and reduce window compositor latency.
* **Mouse Look**: Click into the game window to lock the cursor for 3D camera control.
* **Interaction Holds**: Press and hold `E` (or any interaction key) to interact with ProximityPrompts without auto-repeat interruptions.
* **In-Game Menu**: Press `Esc` to toggle the Roblox pause menu.

---

## Optimizations Included

* **Native Bionic Integration**: Direct zero-overhead Android Bionic `pthread` calls, bypassing glibc emulation wrappers.
* **Turnip Vulkan Acceleration**: Native Vulkan backend support via Turnip KGSL driver.
* **Auto-Repeat Hold Fix**: Enabled XKB detectable auto-repeat and suppressed duplicate key-down events for continuous in-game hold actions (ProximityPrompts).
* **High Polling Rate Mouse Engine**: True incremental relative motion deltas with deferred 60Hz warp recentering, eliminating stutter and sensitivity spikes on gaming mice.
* **Escape Key Fix**: Unlatched pointer lock suppression so Escape opens the in-game menu on the first press without breaking mouse capture.
* **Browser Auth Integration**: Native `termux-open-url` fallback for seamless browser login.
