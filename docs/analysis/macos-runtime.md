# Running Roblox's macOS client: what Mac O' Blox does, what it would cost, and a runtime spec Cordial could publish

Spike, 2026-10-01, written against `main` at `8f25a08`. **Read-only: nothing was
built, installed, run or downloaded.** Everything marked *measured* is a file
read, a `gh api` query or an HTTP `HEAD`/`GET` of public metadata made today.
Everything marked **INFERRED** was not observed. No Roblox binary was opened,
and the macoblox clone (MIT, `~/.cache/cordial-agent-mac/macoblox`, commit
`7335ab3`, 2026-09-24) was read for ideas and is deleted afterwards. Cordial
copied nothing from it.

## Verdicts

| # | Question | Verdict |
|---|---|---|
| 1 | How does Mac O' Blox work? | Darling plus an injected shim of about 8,000 lines of C and Objective-C (`DYLD_INSERT_LIBRARIES`). Frames come from **Roblox's own OpenGL 3.2 renderer** through Darling's OpenGL-over-EGL, drawn on X11. No Metal is involved today. |
| 2 | Is Metal to Vulkan easy? | **No evidence for "easy".** A Metal-on-Vulkan layer exists (Darling's Indium, 0BSD) but its last commit is April 2023 and nothing shows a real app on it. It is also not needed today, because the client falls back to GL. |
| 3 | Architecture | Roblox ships **separate** x86_64 and arm64 builds. Darling runs x86_64 only. "Add translation" has no working route in either direction. |
| 4 | Integrity | **Unknown, and it decides whether this can be offered.** No Mac Hyperion announcement found, no ban reports found, but the project is eight days old. Roblox's Aug 2025 emulator-kick policy is the nearest public statement. |
| 5 | Packaging | Not in Cordial's Flatpak as it stands. `darling` is setuid root and needs overlayfs and namespaces. Mac O' Blox's own Flatpak fakes root inside Darling with `LD_PRELOAD`. |
| 6 | Integration | Most of Cordial's features can be driven from outside the client. The injected shim cannot be reused, because it is an ADR-001 red line. |
| 7 | Legal | Darling is GPL-3.0 with mixed-licence parts. Launching a separately installed Darling is clean; bundling it is not something to decide here. Roblox's ToS position on this was not retrievable. |

### Overall: not viable as framed; viable only as an externally owned, experimental runtime behind a spec, after one proof of concept

"Native Cordial macOS support the way Mac O' Blox does it" fails on four things
that are each independent:

1. **The shim is injection.** Mac O' Blox launches `RobloxPlayer` with
   `DYLD_FORCE_FLAT_NAMESPACE=1` and `DYLD_INSERT_LIBRARIES=libMacOBloxShims.dylib`
   and then interposes about 80 functions and swizzles AppKit methods, including
   methods of Roblox's own `RBXWindow` class. That is exactly the primitive
   ADR-001 says Cordial never contains. Each fix could in principle move into
   Darling itself (the Wine relationship), but that is a maintained Darling
   fork, not a launcher feature.
2. **The window is X11.** Darling's AppKit (Cocotron) draws through X11, and
   Mac O' Blox's Flatpak says so (`--socket=x11`, "Xwayland on Wayland"). ADR-011's
   "one window, engine as a Wayland subsurface" is unreachable, and the three
   open Plasma/Wayland reports in Mac O' Blox's tracker (#4, #5, #6) all
   include the camera or pointer lock failing.
3. **x86-64 only.** See section 3.
4. **Integrity is unmeasured**, and the measurement needs a test account.

What is viable is the half of the maintainer's idea that does not depend on the
above: **a documented launcher spec that a runtime implements** (last section),
with the Android runtime as the first implementation. That is useful on its own,
costs little (the seam is small, measured below), and lets a macOS runtime exist
outside this repository, owned by whoever accepts the shim question.

## 1. How Mac O' Blox works

Sources: `narezy/macoblox` (MIT, created 2026-09-23, 47 stars, one release,
`v0.13`), read at the paths below.

**What it installs.** `install.sh` installs Darling (AUR `darling-bin` on Arch,
the Ubuntu `.deb`s from Darling's GitHub release on Debian and Ubuntu, a source
build on Fedora) plus `clang`, `lld`, `unzip`, PipeWire tools and GTK 4 /
libadwaita for its Python launcher. It refuses anything but x86_64
(`install.sh`: "Darling runs only on x86_64"). The Flatpak
(`flatpak/xyz.narez.MacOBlox.yml`) bundles the official Darling debs, pinned by
sha256, and a GNOME 50 runtime.

**Kernel module, root.** No kernel module: Darling's current design is
`darlingserver`, a userspace process (Darling README). Root is still required:
the `.deb`s ship `usr/bin/darling` as `4755 root/root`
(`debian/darling.lintian-overrides`), and Mac O' Blox's own notes record
`binary is not setuid root, which is mandatory` and an overlayfs kernel module
that a kernel update without a reboot had made unavailable
(`docs/NOTES.md`). Darling needs Linux 5.0+, mount and PID namespaces, overlayfs
for the prefix, and an SSE3 CPU (docs.darlinghq.org build page).

**How it gets the client.** `launcher/macoblox/core.py`:
`GET https://clientsettingscdn.roblox.com/v2/client-version/MacPlayer`, then
`https://setup.rbxcdn.com/mac/{clientVersionUpload}-RobloxPlayer.zip`.
Measured today:

```
MacPlayer -> {"version":"0.741.0.7411056","clientVersionUpload":"version-3bc33ee7ffad426f",...}
HEAD mac/version-3bc33ee7ffad426f-RobloxPlayer.zip       200  152,739,120 B  Last-Modified 2026-09-28
```

**Verification: none.** The launcher checks `zipfile.is_zipfile` and that the
archive contains `RobloxPlayer.app`, then unzips it. No hash, no code-signature
check. (Its Studio path does check MD5s from `rbxPkgManifest.txt`, but those
come from the same CDN.) The bundle is presumably Developer-ID signed and
notarised; nothing here verifies that. Cordial's APK path pins Roblox's
certificate (ADR-025), so a macOS equivalent would mean Apple code-signature
verification on Linux, for example with `rcodesign`; licence of that tool not
checked, and not tried.

**How it launches.** `RobloxSession.start` in `core.py`: `darling shell
/bin/bash -c LAUNCH_SCRIPT`, which `cd`s into the bundle, exports the two
`DYLD_*` variables above, and `exec ./RobloxPlayer`. `EGL_PLATFORM=x11` is
forced. One Darling prefix (`~/.darling`), one session, no profiles, no
multi-instance, no deep-link handler for `roblox://` joins.

**Graphics: the crux.** What renders frames is *Roblox's* OpenGL renderer:

- `gl_profile.c`: Roblox asks `NSOpenGLPixelFormat` for a 3.2 Core profile.
  Darling's CGL creates every context through `eglCreateContext` with no
  attributes (a compatibility profile), so the shim adds the Core attributes.
  Without that Roblox reported no MSAA support.
- `libMacOBloxShims.m` interposes `glShaderSource`/`glCompileShader`, and one
  fix rewrites a `POSITION.w` shader expression that Mesa rejects. The shader
  text in those comments is GLSL (`#version 150`, `VARYING1`, `_entryPointOutput`).
- `missing_symbols.c` answers `CVMetalTextureCacheCreate*` with "unsupported",
  and its header says it does what macOS would "so Roblox takes its normal
  fallback path".
- Presentation is Darling's `X11SubWindow` plus EGL, so the path is Roblox GL ->
  Darling OpenGL.framework -> Mesa (or the NVIDIA GL driver) -> an X11 window,
  which on Wayland is Xwayland.

**INFERRED**: that Roblox *chooses* GL because `MTLCreateSystemDefaultDevice`
yields nothing. Not observed. It is consistent with all four bullets and with
Roblox's own history (macOS was 86% Metal, 14% OpenGL in Dec 2020, per a Roblox
engineer's published stats). **Not known** whether Darling's official `.deb`s
contain a working Metal framework (section 2).

**Audio.** Darling's CoreAudio-over-PulseAudio "overflows Darling's workqueue
thread stacks within seconds", so Mac O' Blox turns it off
(`MACOBLOX_AUDIO=0`) and the shim writes raw float32 stereo into a FIFO that
`pw-cat` (or `pacat`) plays (`HostAudio` in `core.py`, `audio_hal.c`, which also
answers CoreAudio HAL properties FMOD asks for).

**Input and mouse lock.** AppKit events come from Cocotron's X11 backend. The
shim swizzles `NSWindow acceptsMouseMovedEvents` for `RBXWindow`, `NSEvent`
button numbers, and interposes `CGAssociateMouseAndMouseCursorPosition` /
`CGWarpMouseCursorPosition` onto `XWarpPointer`, hiding the cursor with XFixes
over a hand-written X11 socket client (`xfixes_raw.c`) because Xwayland only
emulates warps while the cursor is hidden. Controller hotplug is stubbed to an
empty iterator (`IOServiceAddMatchingNotification`). **Open issues #4, #5 and
#6** (Plasma/Wayland) report the camera spinning, right-click not locking, and
no controllers; #5 also reports no fractional-scaling inheritance, and #2
a crash on an RX 9060 XT. #5 says it otherwise "works incredibly well" on AMD
CPU plus NVIDIA GPU.

**Sessions and login.** Darling does not persist `SecItemAdd` or cookies, so the
shim stores the `.ROBLOSECURITY` cookie in `~/Library/MacOBlox/Cookies.plist`
(0600) and Keychain items under `~/Library/MacOBlox/Keychain/` inside the prefix.
Quick Login works. Password sign-in and sign-up open Roblox's `WKWebView`
captcha, which Darling lacks, and the client exits (`exit_reason()` looks for
`class WKWebView`); the README tells users to create the account in a browser first.

**FastFlags.** `FAST_FLAGS = <bundle>/Contents/MacOS/ClientSettings/ClientAppSettings.json`,
edited by the launcher, preserved across a client update. The client reads
the same file name as Windows. A third-party Mac launcher's docs (AppleBlox) say
Roblox "now uses a whitelist" and silently ignores many flags. Whether
server-pushed `DF*` values overwrite the file on Mac, as they do on Android
(ADR-050), is **INFERRED yes and unmeasured**; the Mac settings document is
public (`.../v2/settings/application/MacDesktopClient`, 23,168 entries today).

**What it patches.** Not the Roblox binary: `docs/NOTES.md` says the original
binary is unchanged. It does patch:

- **Darling's files**: `_patched_ffmpeg_bridges` writes a `ret` over the
  initialiser of Darling's `libav*` bridge dylibs when the host has another
  ffmpeg, and drops stub `CoreML`, `CoreHaptics` and `DeviceCheck` frameworks
  into the prefix (`DeviceCheck` is an empty `DCDevice` class, so no attestation
  token is invented).
- **The Roblox process, at run time**: roughly 80 `DYLD_INTERPOSE` entries across
  ten C/ObjC files (libc `memcpy`/`strlen`, `pthread_mutex_lock`, GL, CGL, EGL,
  `getaddrinfo`, socket calls, Keychain) and 47 `method_setImplementation`
  calls, one of them on `RBXWindow`. `net_trace.c` wraps `recvfrom`/`kevent`
  to run a watchdog for hung RakNet threads. `libMacOBloxShims.m` rewrites
  Roblox-generated shader text.
- **Darling's own server**, in the Flatpak only: `flatpak/darling-noroot.c`
  answers root checks, `unshare` and mounts "as if they succeeded" under
  `LD_PRELOAD`. That is a lying stub in the sense of AGENTS.md, aimed at Darling
  rather than Roblox.

All of the second group is a red line for Cordial. The first two groups'
*purposes* (Darling bugs, missing CoreAudio properties, GL context profile) are
legitimate framework-layer work; the *mechanism* is not.

## 2. Metal to Vulkan

What exists, measured from GitHub today:

| Project | Direction | Licence | State |
|---|---|---|---|
| `darlinghq/darling-metal` | ABI-compatible `Metal`, `MetalKit`, `MetalPerformanceShaders` frameworks for Darling; thin Objective-C++ over Indium | MPL-2.0 (relicensed from GPL-3 on 2026-02-27) | created 2022-12, 31 commits, 79 files, last push 2026-03; later commits add symbols for apps (GOG Galaxy, ShapeScript) |
| `darlinghq/indium` | Metal API on **Vulkan 1.3** (needs timeline semaphores); includes **Iridium**, AIR (compiled Metal shaders) to SPIR-V, and a `mtl2spv` tool | 0BSD | 54 commits, **last commit 2023-04-11**, README says "NOT a drop-in replacement"; no render-correctness claims |
| `steelbrain/metal2vulkan` | AIR to SPIR-V, Rust | LGPL-3.0 | created 2026-07-22, **self-declared alpha**, 268 stars |
| `Hi-Jiajun/metal-api-emulator` | Metal compute (and a little offscreen render) on Vulkan | LGPL-3.0 (plus GPL parts) | created 2026-09-05; says "not a Metal.framework ABI implementation" |
| Darling discussion #1646 (2026-01-18) | a user proposes "MetalVK", unaware of Indium | n/a | no replies |
| MoltenVK | the **opposite** direction (Vulkan on Metal) | Apache-2.0 | irrelevant to this problem |

Darling's README claims "an initial Metal backend powered by Vulkan translation".
`debian/control` lists `llvm-dev` and `libvulkan-dev`, and Darling's CMake builds
Metal when both are found (`ENABLE_METAL=AUTO`), so the official debs
**probably** include it (**INFERRED**: nobody opened a package). I found no
public report of any real app, let alone a game engine, rendering through it.

**Testing the belief that it is easy.** The evidence against:

1. The only layer that exists is a foundation: roughly 5 MB of source whose
   author stopped in 2023, whose shader path (AIR to SPIR-V) is the part two
   2026 hobby projects are still calling alpha.
2. The mapping of API calls is the smaller part of the problem. Public Roblox
   flag names (Mac settings document, measured today) show the Mac Metal
   renderer using memoryless textures (`FFlagMacOSMemorylessTextureSupport`,
   `FFlagGraphicsDisableMemorylessMetal`), managed and write-combined constant
   buffers (`FFlagGraphicsMetalManagedConstantBuffers`), multithreaded shader
   loading (`FFlagGraphicsMetalMTShaderLoading`) and Core Animation vsync
   statistics (`FFlagMetalUpdateCoreAnimation`). Names only, meaning
   **INFERRED**, but each is a Metal feature with no one-to-one Vulkan form, and
   presentation through `CAMetalLayer` needs Darling's QuartzCore to grow a
   Vulkan swapchain.
3. **Roblox ships GL on Mac as a fallback and may remove it.** The Mac
   settings still carry `FFlagDisableHQShadersOnLowendGLMac`, so GL code is live
   in the current build. Roblox's 2020 post on Metal said it "may be practical
   for us to stop supporting desktop OpenGL at all". If that happens the
   Mac O' Blox approach stops working until a Metal path exists, which is the
   one situation in which this section's work becomes necessary.

**Effort if Cordial had to build or extend it.** A judgement, not a measurement:
**roughly 6 to 12 person-months** of a specialist GPU engineer to reach correct
frames from Roblox's Metal renderer on Indium, and then permanent maintenance
against a client that updates weekly. The error bar is about 2x either way,
because the Metal surface Roblox uses is not known without examining its binary,
which ADR-001 and AGENTS.md rule out. **Recommendation: do not start.** The
cheapest decision-relevant measurement is whether the client picks Metal when
Darling offers a device (proof of concept, item 2).

## 3. Architecture

**Is it universal?** No, by every public signal: Roblox publishes **two
separate packages** under one upload id. Measured today:

```
mac/version-3bc33ee7ffad426f-RobloxPlayer.zip         200  152,739,120 B
mac/arm64/version-3bc33ee7ffad426f-RobloxPlayer.zip   200  144,811,231 B
mac/arm64/version-3b0126187faf4c64-RobloxPlayer.zip   403  (an Aug-2023 upload has no arm64 package)
clientsettingscdn .../client-version/MacPlayerArm64   "Invalid binaryType"
```

Sizes within 6% of each other point to two thin builds (a universal zip would be
nearly double); **INFERRED**, not opened. A DevForum tutorial dates Roblox's
native Apple-silicon rollout to version 582 (July 2023) via separate
`zmacarm64` packages. The version API only answers for x86_64 (`MacPlayer`), so
finding the arm64 package means reusing the upload id on the `mac/arm64/` path.

**Which slice runs on which host.** On an x86-64 Linux host the x86_64 package
runs under Darling with no translation. Darling is a translation layer, not a
CPU emulator (the `darling-arm64` README's words). Darling upstream is **x86_64-only**: `darling/issues/642`
(arm64 build, opened 2020) is unresolved, and the docs list x86-64 hosts only.
There are 2026 experiments: `xchemtina/darling-arm64` (created 2026-09-05)
reports iTerm2 working but GUI applications failing on Darling's AppKit
implementation, and `VibeDarling/darling` (fork, 5 stars) is making Darling's
libraries build for arm64/arm64e and use the host page size. Nothing there is
merged upstream.

**So "detect x86-64 versus arm64 and add translation" means, in practice:**

| Host | What would be needed | State |
|---|---|---|
| x86-64 | x86_64 package, native Darling | works for some users per Mac O' Blox |
| arm64 | arm64 package and arm64 Darling | upstream does not exist; forks cannot run Cocotron GUI apps |
| arm64 | x86_64 package, x86_64 Darling under FEX or box64 | nobody has tried it; unmeasured. Darling's Mach-O loader, `mldr`, and `darlingserver` would all run under the translator |
| x86-64 | arm64 package, arm64 Darling under qemu-user | **closed by measurement**: ADR-043 found qemu-user exposes only llvmpipe, no GPU |

So detection should answer one question, "is this host x86-64?", and **hide the
runtime otherwise** rather than offer translation. That matches ADR-043's rule
that an option which changes nothing is a lying stub.

## 4. Integrity and anti-tamper

Public facts only:

- Hyperion (Byfron) is announced for the 64-bit **Windows** client (May 2023),
  with Android planned (Roblox fan wiki and news posts). I found **no
  announcement of Hyperion on macOS**. Absence from search is weak evidence.
- Roblox's DevForum post of 2025-08-07, "An Update on Using Third-Party
  Emulators", says players detected on an emulator are kicked, names Android
  emulators, exempts users with edit permission, and says virtual machines are
  supported. The text I retrieved does not mention Linux compatibility layers,
  Sober or Darling, and does not say how detection works.
- The Mac settings document carries `FFlagEnableMacIdentifierTelemetry`,
  `FFlagEnrollMacIdentifierOnWebLaunch` and `FFlagReportMetalHardwareInformation`.
  Names only; they suggest the Mac client identifies hardware and reports GPU
  and Metal details, which Darling cannot supply honestly (**INFERRED**).
- `DFIntIntegrityCheckedProcessorClientMinimumAccountAge` appears in the Mac and
  the PC desktop documents alike; what it gates on Mac is unknown.
- **Ban or kick reports for Mac O' Blox:** none in its tracker (five issues, none
  about it). Its Discord is not publicly readable. The project is eight days
  old, so silence means nothing yet.
- **Injection** is the part Cordial controls. Mac O' Blox injects a dylib into
  the process; whether the Mac client scans for that is unknown. A Cordial
  runtime that does the same would put ADR-001's "ecosystem risk" paragraph on
  the line, because Roblox's tolerance of Linux play is contingent.

**This decides whether it can be offered, and nothing here answers it.** The
proof of concept below measures kicks per launch on a test account. A clean
result does not show safety; the first kick stops the work.

## 5. Packaging

| Where | Verdict |
|---|---|
| Cordial's Flatpak | **No.** Needs a setuid `darling`, mounts and namespaces. Mac O' Blox's Flatpak gets round it with `LD_PRELOAD` (`darling-noroot.c`) plus `DARLING_NOOVERLAYFS=1`, which makes `darlingserver` copy the whole macOS root into the prefix. Its release asset is 74 MB and labelled "testing". Whether that fakery is sound was not assessed. |
| Cordial's own packages | Darling is not packaged for Fedora; official packages are Ubuntu debs (118 MB zip) and a 506 MB source tarball (release `v0.1.20260608`). Bundling is a licensing and maintenance decision (section 7). |
| Immutable hosts (Bluefin, Silverblue, the maintainer's host) | Official debs do not apply, and `/usr/libexec/darling` plus a setuid `/usr/bin/darling` mean `rpm-ostree` layering or a source build under `/usr/local`. A distrobox gives no real root, so setuid and overlayfs are unavailable inside it. **INFERRED**, not tried. Cheapest route: `flatpak install --user` of Mac O' Blox's own file. |
| Root, kernel module, setuid | Setuid root: yes. Kernel module: no. Overlayfs and namespaces: yes (unless the no-overlay hack is used). |

## 6. Integration points

| Cordial feature | With a macOS runtime |
|---|---|
| FastFlags | Cordial resolves its layers (`cordial-runtime/src/flags.rs`: user, plugin, base) and **writes a flat `ClientAppSettings.json` into the bundle** before launch. No `nativeInitClientSettings` handover exists here (the Android client does that from `client_settings.rs`). Allowlist and DF refresh behaviour unmeasured. |
| Profiles | One Darling prefix per profile (`DPREFIX`). A full-copy prefix is large; an overlay prefix needs the mount. The `flock` (ADR-012) works unchanged, because the container is neutral. Session lives in the prefix, not in Cordial's secret store. |
| Plugins and presence | Plausibly yes, **INFERRED**: the client writes `~/Library/Logs/Roblox/*.log` (Roblox's creator docs name that path); inside Darling it is `$DPREFIX/Users/$USER/Library/Logs/Roblox`. Whether the Mac log carries the same `GameJoinLoadTime` and `UDMUX` lines `game_log.rs` parses is unmeasured. BloxstrapRPC rides the same log. |
| Doctor | A runtime contributes its own checks: `darling` present and setuid, overlayfs loaded, Xwayland running, Mesa GL renderer not llvmpipe. Cordial's `doctor.rs` already has the `Check { level, what, fix }` shape. |
| Report | Add runtime id, runtime version, client version and renderer to `diagnostics.rs`, so a Mac O' Blox report can be routed to the right tracker (ADR-050). |
| Updates | The client has its own version API (above). Verification is the open gap (section 1). The mirror, APK-signature and build-store code (`cordial-update`) does not carry over. |
| Live settings | Almost none. `title_bar`, `fullscreen_accel`, `gamepad` and `audio_output` act on Cordial's own window and audio, which a Darling client has neither of. Throttle, close-on-leave and carry-launch-ticket are about what Cordial sees and could apply to any runtime. |
| Not needed | The GTK text-editor overlay (Darling's AppKit handles text; IME under Xwayland is **INFERRED** fine and unmeasured), the Android fixes, the engine-memory cookie read, the AGDK input path, asset overlays (ADR-010 is APK-specific). |

## 7. Legal and scope

- **Darling** is GPL-3.0 (`LICENSE` read), and its README says submodules are
  licensed individually. Seen today: `darling-metal` MPL-2.0, `indium` 0BSD,
  `darling-foundation` LGPL-2.1, `darling-appkit` GPL-3.0, much of the base
  system from Apple's open-source releases. Cordial is GPL-3.0-or-later.
  **Launching a separately installed Darling needs no licence decision.**
  Bundling it would make Cordial's packages carry Darling's source obligations,
  and whether the Apple-derived parts' licence (APSL) is compatible with
  GPL-3.0 is Darling's question that I did not resolve. **Uncertain; not legal
  advice.**
- **Apple frameworks.** Darling reimplements them from Apple's open releases and
  clean code; no Apple binary is shipped, so no Apple EULA is in play (**INFERRED**).
- **Mac O' Blox** is MIT. Cordial may adapt ideas with credit. Its shim is the
  red line regardless of licence.
- **Roblox ToS.** I could not retrieve the Terms (HTTP 403). Cordial's README
  already says Roblox does not support third-party clients and bans accounts.
  No public statement on running the Mac client on non-Apple hardware was found.
  Treat it as unsupported, like Sober.
- **No Roblox code** is changed by this: the zip is downloaded from Roblox's CDN
  by the user's own machine, as with the APK.

## Corrections to what is written down

- **ADR-039** says no Metal-to-Vulkan project "was located". Indium has existed
  since 2022 and Darling's README mentions it. That ADR's INFERRED paragraph
  should be amended, and its sentence that a Mach-O loader and framework layer
  "from nothing" are needed is superseded by Darling.
- **ADR-050** says a macOS client needs "its own graphics translation". The
  evidence in Mac O' Blox's source is that it uses Roblox's OpenGL renderer and
  needs none (not run; **INFERRED**). The ADR's "launch only, features do not
  carry over" position is what the spec below would replace.
- ADR-039's "Settings picker and issue templates neither belong now" still
  holds until a second runtime exists.

## Smallest proof of concept

Settles risks 1 (renders at all), 2 (Metal or GL), 3 (flags), 4 (log format) and
5 (integrity), in that order, stopping at the first failure.

**Machine.** An x86-64 Linux box with a real GPU and a graphical session. The
maintainer's host works through `flatpak install --user MacOBlox-0.13-x86_64.flatpak`
(74 MB, no layering). A normal Arch or Ubuntu 24.04 install is the fallback. A
Roblox **test account on a separate IP**, signed in over Quick Login, with
launches at least 90 s apart and a daily cap (the project's pacing rule).
Sequential, never beside another Cordial client.

**Measure**, with the control named:

1. **Renders.** Reaches Home and one join. Record GPU, driver, compositor.
2. **Which API.** Read the client's own log for its renderer and `Caps` lines.
   Check whether the prefix contains `Metal.framework` and whether the client
   uses it (the Darling `ENABLE_METAL` question). Control: the same client with
   the Metal framework removed from the prefix. Note the one thing this cannot
   show: that GL is chosen *because* Metal is absent.
3. **Frame rate and CPU** with input driven for the whole window and the input
   rate beside the figure (AGENTS.md), against Cordial's Android runtime on the
   same scene, sequential.
4. **Flags.** Write `DFIntTaskSchedulerTargetFps` and one `FFlag` into
   `ClientSettings/ClientAppSettings.json`; check each applied; wait 3 minutes
   and re-read for a DF revert (the Android refresh loop ran at about 120 s).
5. **Log shape.** Does `~/Library/Logs/Roblox/*.log` contain the join and leave
   lines `game_log.rs` parses? Run the existing parser over it (the module's
   test already takes `CORDIAL_GAME_LOG`).
6. **Integrity.** Kicks and error codes over at least 10 launches across 3 days.
   A clean run does not prove safety.
7. **Signature.** Verify the downloaded bundle's Developer-ID signature with
   `rcodesign` on a copy outside the prefix, and note the Team ID.
8. **Wayland.** Pointer lock and fractional scaling on Plasma and on Sway, to
   see whether the three open reports reproduce.

Not needed for the decision: arm64 anything, Studio, audio quality.

## Draft launcher spec, v0.1 (for review; not yet an ADR)

**Name.** The "Cordial runtime spec", wire identifier `cordial.runtime/1`. The
crate called `cordial-runtime` is the *Android* runtime; rename it
(`cordial-android`) before the word means two things.

**Premise.** A *runtime* is whatever turns "Play" into a running Roblox client.
Cordial is the *launcher*: window chrome, profiles, FastFlag layers, plugins,
presence, doctor, report, updates UI. The spec carries **events and effects, never
channels**: the shape ADR-007 already sets for plugins.

### 1. Discovery: `runtime.json`

Searched in `$XDG_DATA_HOME/cordial/runtimes/<id>/` then each
`$XDG_DATA_DIRS/cordial/runtimes/<id>/`, and in `/app/share/cordial/runtimes`
for a Flatpak extension. Discovery across Flatpak sandboxes is **unsolved**:
the socket in section 2 must be reachable from both sides, and sandboxes do not
share paths. The first version is same-sandbox or native only.

```json
{
  "spec": "cordial.runtime/1",
  "spec_version": "1.0",
  "id": "org.example.macoblox",
  "name": "Mac O' Blox",
  "version": "0.13",
  "arch": ["x86_64"],
  "launch": { "exec": ["bin/macoblox-run"], "args": ["--socket", "{socket}", "--profile", "{profile_dir}"] },
  "capabilities": { "lifecycle": 1, "events.core": 1, "flags": 1, "doctor": 1 },
  "support_url": "https://example.org/issues",
  "licence": "MIT"
}
```

`exec` is resolved relative to the manifest's directory. The manifest is a
static advertisement; the live truth is the handshake. A manifest that names an
unknown `spec` major is listed as "needs a newer Cordial", not hidden.

### 2. Transport

JSON lines (UTF-8, `\n`-terminated, 64 KiB per line, longer is a protocol error)
over a Unix socket the **runtime opens** per session at `{socket}`, inside a
`0700` directory in the profile (`<profile>/runtime/<session>/`, the shape ADR-044
uses for `live/settings.sock`). Cordial connects. Plugins never see it (the
sandbox does not bind the profile; ADR-003).

```
{"v":1,"id":7,"m":"flags.apply","p":{...}}        request, Cordial -> runtime
{"v":1,"id":7,"ok":true,"p":{...}}                reply
{"v":1,"id":7,"ok":false,"e":{"code":"unsupported","detail":"..."}}
{"v":1,"ev":"game.joined","p":{...}}              event, runtime -> Cordial
```

Handshake: Cordial sends `hello {spec:"1.0", cordial, session, profile}`; the
runtime answers `{runtime:{id,version}, client:{name,version,build}, capabilities:{name:{ver, ...}}}`.
The live set is the intersection. **A capability that is not offered is shown as
unsupported in the interface, with the runtime's name, and is never faked**: no
request for it is sent, and a plugin that needs it is shown "limited on <runtime>"
rather than loaded and silently dead (`CLIENT_READY` in `core_events.rs` is
declared and published by nothing today; the spec's rule is that a table lists
only what the active runtime emits).

Replies never default to success. Error codes: `unsupported`, `invalid`,
`failed`, `busy`. An unknown request gets `unsupported`; an unknown event is
ignored. A message that fails to parse closes the session with an
explanation in the report.

### 3. Capabilities

| Capability | What it carries | Required |
|---|---|---|
| `lifecycle` | Cordial starts the runtime from the manifest; `lifecycle.stop` asks for a graceful exit; the runtime emits `lifecycle.ready`, `lifecycle.exit {status, reason}` and `crash {signal, summary, log_path}` | yes |
| `events.core` | `game.joined {place_id, universe_id?, job_id?, server_address?, user_id?, at}`, `game.left`, `session.state {signed_in}` (never the token) | no |
| `events.presence` | `game.presence`: the folded BloxstrapRPC payload | no |
| `events.log` | alternative to the two above: `{dir, glob}` of the client's own log, which Cordial parses with `game_log.rs` | no |
| `flags` | Cordial **resolves** layers (user wins, ADR-013) and writes the flat document to `flags.path` from the handshake, then sends `flags.apply`. Declares `{families, allowlist: bool, live: bool}`. `flags.live` accepts `DF*` only and returns applied or ignored per name | no |
| `settings` | Declares which of Cordial's closed key set it honours, each `live` or `next-launch` (ADR-044's table, per runtime), and optionally a typed form for its own keys (bool, enum, int range, label, help). Declarative; no code is run | no |
| `profile` | Cordial gives `{profile_dir}`. The runtime declares `instances.max` and whether session data lives in the profile or in the runtime's own store | yes |
| `session.vault` | an opaque blob that Cordial keeps in the keyring (`secrets.rs`). Never offered to plugins | no |
| `doctor` | declarative `requires` in the manifest (`executable`, `kernel-module`, `setuid`, `socket`), run by Cordial with no runtime code, plus `doctor.run` returning `Check {level, what, fix}` as `doctor.rs` already shapes it | no |
| `diagnostics` | `diagnostics.get`: ordered, redacted key/value lines for the report; the report always names the runtime id and `support_url` | no |
| `updates` | `updates.check {installed, latest, obtainable, notes_url}`, `updates.install` with progress events, and a mandatory `verify` field saying how it is verified (`apk-signature`, `apple-codesign`, `sha256`, `none`). Cordial shows it | no |
| `window` | `mode: "embedded" | "toplevel"`, plus `title_bar`, `fullscreen` and `resized` events. A foreign X11 toplevel declares `toplevel` and Cordial hides rows that act on a window it does not own | no |
| `launch.join` | accepts a translated join URL (`deeplink.rs`/`deep_link.rs` stay in Cordial) | no |
| `assets.overlay` | register a plugin asset root (ADR-010) | no |
| `debug.control` | devctl's screenshot and input verbs; present only when `CORDIAL_DEV_CONTROL` is set (ADR-019) | no |

### 4. What stays in Cordial

The plugin host, grants and broker (ADR-003, ADR-007), the Discord socket,
notifications, URL opening, the secret store, the profile lock, the settings
and report screens, deep-link translation, and the aggregation of doctor
output. **Plugins never talk to a runtime.** Cordial maps runtime events onto
the existing plugin API: `game.joined` becomes `StateRead`'s `SessionState`,
`lifecycle.*` becomes `LifecycleRead`'s `client.*` events, `game.presence`
becomes `cordial/game.presence`, `flags.apply`/`flags.live` back
`FlagsWrite`/`FlagsWriteDynamic`, and `assets.overlay` backs `AssetsOverride`.
Plugins therefore work unchanged on any runtime, and a plugin whose capability
has no backing is marked limited.

### 5. Hard limits, written into the spec

- The verb set is **closed per spec version**. Not offered: engine memory, loading
  code into the client, calling engine functions, executing a command, passing a
  path or descriptor, evaluating anything, or a generic "set raw" or "call".
- Plugin-supplied strings reach a runtime only as typed payloads Cordial has
  validated (flag names and values, never paths).
- No capability may be added because it would be convenient. A proposal to
  carry something new needs an ADR and a spec bump (ADR-001).
- **What the spec cannot do** is police what a runtime does *inside its own
  process*. Mac O' Blox injects code into Roblox; no protocol can stop that
  (ADR-001: enforcement must live outside the boundary it enforces). The
  spec's only lever is whether Cordial *lists* a runtime. Options for the
  maintainer, not a recommendation: (a) a manifest field `integrity.injection`
  that is an attestation, shown to the user, unverifiable; (b) list only runtimes
  that Cordial's maintainers have read; (c) list none and ship only the
  built-in Android runtime. The Mac O' Blox question is a decision about this,
  not a technical one.

### 6. The built-in Android runtime as the first implementation

**One code path**, and the seam is small. Measured by grep today: the portable
core in `cordial-runtime` is `flags.rs` (0 references to `android`),
`bloxstrap_rpc.rs` (0), `client_settings.rs` (0), `game_log.rs` (1, a call to
`looper::request_quit`) and `plugin_host.rs` (9, all asset-overlay
registration and its tests). The coupled files are `live_settings.rs` (23) and
`devctl.rs` (24).

- Extract a `cordial-runtime-api` crate: the message types (generalising
  `live_wire.rs`), the capability names, the manifest, and a `trait Runtime` that
  yields events and accepts requests.
- Two transports behind one `RuntimeSession`: **in-process** (a channel) and
  **Unix socket** (the serialised form). The Android runtime registers as
  in-process; the socket is the same messages encoded, so a test can run one
  against the other.
- The shell consumes `RuntimeSession`. Today `live.rs` pushes to
  `<profile>/live/settings.sock`; that becomes `settings.set` on the session,
  and the old socket stays as a v0 alias until the shell migrates.
- A new thin **instance host** binary links the shell and a `cordial-host-core`
  crate (the portable files above) and not `cordial-linker-sys`, spawns a
  third-party runtime and serves it the plugin host. The Android runtime keeps
  running its plugin host in `cordial-run`, because ADR-011 needs its window
  there, but feeds it from the same trait.

### 7. Versioning and compatibility

- `spec_version` is `major.minor`. **Minor** only adds optional capabilities,
  events and fields; receivers ignore unknown fields and events. **Major** is
  breaking. Cordial speaks the newest major it knows and refuses a runtime
  whose major it does not.
- Each capability has its own integer version. Within a major, a capability
  never changes meaning or loses a field; it is withdrawn only at a major.
- Runtime-private events use the prefix `x-<id>.`, are shown nowhere to plugins,
  and appear only in the report.
- **Conformance:** a fake client and `cordial --runtime-check <manifest>` that
  performs the handshake and prints doctor-shaped results, so a runtime's author
  can find out before a user does.
- The spec is a documented contract (`docs/runtime-spec.md`) kept beside the
  ADR that records why; the Android runtime is its conformance suite.

## Outline for the runtime-abstraction ADR

1. **Context.** The maintainer's macOS request; ADR-039 (seam, "why macOS
   waits"), ADR-043 (architecture is the binary's), ADR-050 (launch-only,
   parked); the corrections above.
2. **Decision.** Adopt a published runtime spec; the Android runtime is the
   first implementation; launch-only (ADR-050) is superseded; the verb set is
   closed.
3. **Capability interface and its homes.** The table in section 3, with today's
   code: install and update (`install.rs`, `updater.rs`, `roblox_versions.rs`,
   `cordial-update`), launch (`launch.rs`), flags (`flags.rs`, `client_settings.rs`,
   `flag_import.rs`), events (`game_log.rs`, `bloxstrap_rpc.rs`, `core_events.rs`,
   `state.rs`, `plugin_host.rs`), live settings (`live_wire.rs`, `live.rs`,
   `live_settings.rs`), profile (`profile.rs`, `secrets.rs`), doctor and report
   (`doctor.rs`, `diagnostics.rs`, `report.rs`), debug (`devctl.rs`).
4. **Naming and crate split.** `cordial-runtime` renamed; `cordial-runtime-api`
   and `cordial-host-core` extracted.
5. **Integrity policy.** What Cordial lists and why (the three options above);
   how ADR-001 applies to third-party runtimes.
6. **Packaging.** Same-sandbox first; the Flatpak-extension question open;
   Cordial installs nothing for a third-party runtime unless the runtime's own
   `updates` capability does it.
7. **Alternatives considered.** A plugin-style runtime (ADR-039 rejected it); a
   trait with no wire form; in-tree macOS runtime (rejected by section 1);
   building Metal on Vulkan (section 2).
8. **Consequences and reopen conditions.** Reopen the macOS question when the
   proof of concept reports, when Darling gains Wayland or arm64, or when
   Roblox removes desktop OpenGL.

## Measured versus inferred

**Measured today:** the three Roblox endpoints and the zip sizes and
timestamps above; the Mac and PC client-settings documents' flag counts and
names; GitHub metadata, licences, dates and commit counts for the Darling,
Indium, darling-metal, metal2vulkan, metal-api-emulator, darling-arm64 and
macoblox repositories; macoblox's source; Cordial's own import counts.

**Not measured:** every behaviour. Nothing was installed or run: whether the
client renders, which API it picks, whether flags apply, what the log contains,
whether Roblox kicks it, whether Darling's debs include Metal, and whether a
Darling Flatpak without root is sound. Each is a proof-of-concept item.

**Sources.** `github.com/narezy/macoblox`; `github.com/darlinghq/darling`,
`darling-metal`, `indium`, `darling-cocotron` (no Wayland backend found by code
search); `docs.darlinghq.org` (build instructions, containerization);
`github.com/darlinghq/darling/issues/642`, `/discussions/1646`;
`github.com/xchemtina/darling-arm64`; `github.com/VibeDarling/darling`;
`github.com/steelbrain/metal2vulkan`; `github.com/Hi-Jiajun/metal-api-emulator`;
`devforum.roblox.com/t/an-update-on-using-third-party-emulators/3867040`;
`devforum.roblox.com/t/how-to-get-the-native-version-of-roblox-roblox-studio-for-apple-silicon/2459091`;
`about.roblox.com/newsroom/2020/05/3-years-metal`;
`gist.github.com/zeux/51077c80bd7ffd4d558e3f1d9cbb2d92`;
`vinegarhq.org/Home/rol_faq.html`; `docs.appleblox.com`;
`clientsettingscdn.roblox.com` and `setup.rbxcdn.com` as quoted.
