# Backlog from the mocktail audit

Source: `komaruworld/mocktail` @ 41 commits, Apache-2.0, cloned to
`../mocktail`. Apache-2.0 into GPL-3.0 is one-way compatible, so adoption is
permitted with attribution and a NOTICE entry. Everything below that involves
code is to be **written fresh against the problem**, not transcribed.

Cordial and mocktail vendor the same two load-bearing dependencies —
`ChristopherHX/libjnivm` and `mcpelauncher-linker` — so the comparison is real
rather than superficial.

---

## Corrections to the assumptions this audit started from

**mocktail uses SDL3, heavily.** The brief said "GTK4/libadwaita/Vulkan, no
SDL3". Its CMake builds `mocktail_platform_sdl`, `mocktail_sdl_vulkan_wsi` and
`mocktail_audio_sdl`, and links `SDL3::SDL3`. GTK4/libadwaita is the launcher
shell; SDL3 is the platform layer under the game surface. Cordial's canvas is a
Wayland subsurface of the libadwaita window with no intermediate layer
(ADR-011), which is a different and, for a Wayland-only target, simpler design.
**Recommendation: do not adopt SDL3.**

**Nine of the ten libraries proposed for adoption are not in mocktail.**
Checked against its `CMakeLists.txt` and `.gitmodules`:

| proposed | in mocktail |
|---|---|
| nlohmann/json | **yes** |
| capstone (not on the list, but present) | **yes** |
| mimalloc, detex, volk, fmt, mcl/oaknut, imgui, dyncall, libxml2 | **no** |

That list is Sober's dependency set as documented by `sober-oss`, not
mocktail's. Adopting it wholesale would be importing another project's
architecture, not this one's. Each is assessed separately below.

**Cordial has no CMake.** It is a Cargo workspace; the native side builds via
`build.rs` with Clang. So "extract `find_package`/`target_link_libraries`" has
no target here, and the `FetchContent` suggestion does not apply — the Cargo
equivalent for anything new is a normal crate dependency, or a git dependency
pinned by tag.

---

## Tier 1 — no code, testable immediately

### T1. `FStringGraphicsTextureManager2DenyPattern2 = ".*"` — DONE, unverified

**This is the "fix low resolution textures" commit** (`e161fec`), and it is one
line of configuration, not code.

`FStringGraphicsTextureManager2DenyPattern2` is **absent from Cordial's live
settings document**, so the engine uses its compiled-in default. mocktail sets
it to `".*"`, which denies every pattern and takes the engine off TextureManager2
entirely. The live set carries `FFlagTextureManager2SupportLegacyStreaming2 =
True`, so there is a legacy path to fall back to — consistent with this being
the intended escape hatch rather than a hack.

The related `FStringGraphicsTextureManager2DenyPattern` (no `2`) *is* in the
document, as `.*:.*:.*:1|.*:.*:.*:2|.*:.*:.*:3` — a tiered deny list. The
plausible reading is that unrecognised hardware falls into a low tier and gets
low-residency textures, and denying the manager outright restores full
resolution. **`INFERRED` — the causal story is not established, only the flag
value and its absence from our document.**

- **Touches:** Cordial's default flag layer only. No source change to the
  runtime or native side.
- **Status:** shipped in a new built-in flag layer (`flags.rs`), below plugins
  and the user so one line in `flags.json` overrules it. Confirmed applying, and
  confirmed overridable: `= USER-WINS from user (overrides built-in=.*)`.
- **Scope:** trivial. Isolated PR.
- **Verification:** still outstanding — requires a join and a visual comparison, since textures do
  not load on a startup-only run. Control by toggling the flag in the same
  session. The flag pipeline itself is proven — `FLogGraphics=0` takes
  `[FLog::Graphics]` from 30 lines to 0 on the same build.

### T2. `FStringGraphicsVulkanShaderMTDenyPattern` — append, do not replace

mocktail sets this to `"4318:.*"`. 4318 is `0x10DE`, the NVIDIA vendor ID, so
this disables Vulkan shader multithreading on all NVIDIA parts.

**Do not copy their value.** They *replace* the live string, which discards the
entries Roblox ships (`4112:*` and `1010:*` device pairs). Cordial should append
`|4318:.*` to whatever the document carries, so both the vendor's own deny list
and the NVIDIA workaround are in effect.

- **Touches:** flag layer, plus a small append-rather-overwrite helper if the
  current merge only supports replacement.
- **Scope:** trivial. Isolated PR.
- **Caveat:** unverifiable on this machine — the development box is an Intel
  13th-gen part, so the NVIDIA path cannot be exercised here. Ships as
  `INFERRED` unless someone with NVIDIA hardware confirms it.

---

## Tier 2 — real code, real gaps

### T3. ETC1 → BC1 texture transcoding (skybox and beyond)

mocktail's `dcf22a1` adds a 294-line ETC1→BC1 transcoder and wires it into the
asset path. The underlying problem is architectural and Cordial has it too:
**Roblox ships Android texture formats, and desktop GPUs do not implement
ETC1.** An engine that asks for an unsupported format gets a black or missing
surface, which is what "skybox issues" means.

This is the single most substantial thing in mocktail that Cordial lacks.

- **Touches:** new native module, plus the asset/AssetManager path in
  `native/`. Likely interacts with the graphics backend selection.
- **Scope:** major. Own PR, and **must be an independent implementation** —
  BCn and ETC block formats are specified publicly, so writing an encoder from
  the format specs is straightforward and avoids any question of derivation.
  Do not read their file while writing ours.
- **Prerequisite:** first establish that Cordial actually hits this. Look for
  ETC1/`VK_FORMAT_ETC2` requests or missing-format failures in a join run before
  building anything. **Not yet established for Cordial.**

### T4. ANGLE fallback path

mocktail vendors `third_party/angle_headers` and gates behaviour on
`MOCKTAIL_DISABLE_AUTO_ANGLE_FALLBACK` and `MOCKTAIL_SOFTWARE_WINDOW_FALLBACK`.
Cordial has `CordialGraphicsBackend` deciding whether to offer the engine a
Vulkan loader, but no GL-over-Vulkan translation fallback.

Roblox renders through Vulkan now, so this is insurance for hardware where the
Vulkan path fails rather than a feature. **Lower priority than it looks.**

- **Touches:** `crates/cordial-runtime/src/graphics.rs`, backend selection.
- **Scope:** major. Deferred pending evidence that any target hardware needs it.

### T5. WebView

mocktail has a 1470-line `src/webview/` on WebKitGTK6. Cordial already links
WebKitGTK for the cookie/login path (`crates/cordial-runtime/src/cookies.rs`),
so the dependency is present and the gap is scope, not plumbing.

- **Touches:** `cordial-shell`, cookies module.
- **Scope:** moderate. Worth scoping against what it would actually be *for* —
  if it is the login flow, Cordial has that; if it is in-experience web content,
  that is a different and larger ask.

---

## Tier 3 — library assessments

Each judged on whether it solves a problem **Cordial has**, not on whether
another project uses it.

**mimalloc** (MIT) — general-purpose allocator, materially faster than glibc
malloc under heavy threaded allocation. Sober uses it; mocktail does not.
Cordial's engine is allocation-heavy, so this is a plausible general win. But
Cordial loads a bionic-linked engine through a glibc shim, and interposing an
allocator across that boundary is exactly where the shim's assumptions live.
**Effort: moderate, risk higher than the usual "just link mimalloc".** Wants a
measurement first: is allocation actually hot? Nothing here has measured that.

**detex** (ISC) — texture decompression for BCn/ETC/ASTC. Directly relevant to
T3, and the reason to consider it is precisely T3. If T3 proves real, detex is
the honest alternative to writing a transcoder: it is a small ISC-licensed C
library that already handles the formats. **Effort: trivial to link, moderate to
wire in. Assess together with T3, not separately.**

**nlohmann/json** (MIT) — header-only C++ JSON. mocktail uses it. Cordial's JSON
is on the Rust side with `serde_json`; the native side barely parses JSON at
all. **No need. Skip.**

**volk** (MIT) — Vulkan meta-loader, avoids the loader trampoline. A real but
small win, and only worth it if Vulkan call overhead shows up in a profile.
Cordial currently links `libvulkan.so.1` directly. **Effort: trivial. Value:
unmeasured, probably marginal. Skip until profiled.**

**fmt** (BSD) — C++ formatting. Cordial's native side uses `fprintf`/`snprintf`
throughout and the codebase has a consistent voice built around that. Adding fmt
would mean either a mixed style or a sweep. **Skip.**

**mcl / oaknut** (MIT, merryhime) — ARM64 code emission. These exist to
*generate* AArch64 instructions, which is a JIT/binary-translation concern.
Cordial loads the **x86-64** Android build natively on x86-64 hardware; there is
nothing to translate. **This is Sober-architecture-specific. Do not adopt.**

**dyncall** (ISC) — dynamic FFI calls with runtime-constructed signatures. Same
category: it exists for a translation layer that must call across an ABI it
learns at runtime. Cordial's JNI boundary is statically typed through libjnivm.
**Sober-specific. Do not adopt.**

**libbadcpu** — already vendored in Cordial's `third_party/`. Listed in the
brief as something Cordial lacks; it does not.

**imgui** (MIT) — immediate-mode debug UI. Cordial's UI is libadwaita and its
diagnostics are logs and traces. An imgui overlay would be a genuinely useful
*debug* surface, but it is a new UI stack for a project that deliberately has
one. **Skip unless a specific debugging need appears that logs cannot serve.**

**libxml2** (MIT) — XML parsing. Nothing in Cordial parses XML. **Skip.**

**capstone** — in mocktail, not on the proposed list. Disassembly framework;
mocktail presumably uses it for diagnostics. Given AGENTS.md's rule that reading
the stripped binary has been wrong nine times running and running it has never
been, **adding a disassembler is pointed the wrong way for this project. Skip.**

**AOSP portions** (Apache-2.0) — already present as the ported bionic linker.

**SDL3** — see the correction above. **Do not adopt.** If gamepad support is
wanted, `libmanette` is the GTK4-native option and does not bring a second
platform layer.

**libplacebo** — not used by mocktail. It is a GPU video/image processing
library aimed at scaling, tone-mapping and colour management. Cordial presents
the engine's own Vulkan output; there is no processing stage for it to live in.
**Skip unless a post-processing pipeline is on the roadmap.**

---

## Order of work

1. **T1** — one flag, plausibly fixes a visible quality problem, trivial to
   revert. Do this first.
2. **T3 prerequisite** — establish whether Cordial hits the ETC1 gap at all.
   One join run with format logging answers it and costs nothing.
3. **T2** — trivial, but unverifiable on current hardware; ship as `INFERRED`.
4. **T3 proper** — only if step 2 confirms it, and with detex assessed as the
   alternative to hand-writing a transcoder.
5. **T5** — scope the actual requirement before writing anything.
6. **T4** — deferred.

Nothing above has been implemented. `INFERRED` markers are load-bearing: T1's
mechanism, T2 entirely, and T3's applicability to Cordial are all unestablished.

---

## Direct verdicts on the proposed libraries

| library | verdict | why |
|---|---|---|
| **mimalloc** | **yes — adopt** | See the note below. The earlier "measure first" verdict was over-cautious. |
| **detex** | **yes, if T3 is real** | It is the honest alternative to hand-writing an ETC1→BC1 transcoder. Tied to T3; assess together. |
| **nlohmann/json** | **no** | Cordial's JSON is `serde_json` on the Rust side. The native side barely parses JSON. |
| **volk** | **no** | Saves the Vulkan loader trampoline. Unmeasured, probably marginal. Revisit only if a profile shows it. |
| **fmt** | **no** | The native side is `fprintf`/`snprintf` throughout with a consistent voice. Adding fmt means a mixed style or a sweep, for no capability. |
| **mcl / oaknut** | **no** | AArch64 instruction emission. Cordial loads the **x86-64** build natively on x86-64. Nothing to emit. Sober-architecture-specific. |
| **dyncall** | **no** | Runtime-signature FFI, for a translation layer that learns ABIs at runtime. Cordial's JNI boundary is statically typed through libjnivm. Sober-specific. |
| **imgui** | **no** | New UI stack for a project that deliberately has one (libadwaita). Diagnostics here are logs and traces. |
| **libxml2** | **no** | Nothing in Cordial parses XML. |
| **AOSP portions** | **already have it** | The ported bionic linker is exactly this. |
| **SDL3** | **no** | mocktail uses it as its platform layer under a GTK shell. Cordial's canvas is a Wayland subsurface of the libadwaita window (ADR-011) and works. Two platform layers means two event loops competing to be the Wayland client. |
| **libplacebo** | **no** | GPU scaling/tone-mapping/colour. Cordial presents the engine's own Vulkan output; there is no processing stage for it to live in. |

## Input, voice, camera, VR — what the engine we load actually supports

Measured by class-name presence across the three dex files of the shipped
x86-64 Android build, `2.730.0.790`:

| surface | hits in the dex | verdict |
|---|---|---|
| `hardware/camera2`, `CameraDevice`, `CameraManager` | **89** | reachable |
| `WebRtcAudio` | **40** | reachable, and partly hooked already |
| `InputDevice`, `MotionEvent` | **21** | reachable |
| `AppRtcDevice` | **5** | reachable — this is the real voice path |
| `Oculus`, `OpenXR`, `Cardboard`, `GvrLayout` | **0** | **not present** |

### VR is not available and this is not a Cordial limitation

The Android build contains no VR surface of any kind. Roblox's VR support is a
PC-client feature; the mobile engine this project loads does not have it. There
is no shim, capability or amount of platform work that adds it, because there is
no engine code on the other side to call. **Closed, not deferred.**

### Voice chat — the nearest of the three

`AppRtcDeviceWrapper` is the real voice path and Cordial implements none of it.
The WebRTC audio classes in `native/audio_classes.cpp` are hooked and this
session fixed seven of them that were registered as instance methods when the dex
declares them static. So the capture side has been worked; the device-wrapper
layer above it has not.

- **Touches:** `native/audio_classes.cpp`, a new device-wrapper module, the
  PipeWire capture path.
- **Scope:** major, but the best-understood of the three. Its own PR.

### Gamepad and extended input

`InputDevice`/`MotionEvent` is the Android surface, so the engine will take
gamepad input if something feeds it. Cordial's own input path is
`input::pass_key_event`/`pass_text` into GameActivity.

Use **`libmanette`**, not SDL3 — it is GNOME's gamepad library, integrates with
the GTK4 main loop Cordial already runs, and carries the SDL gamepad mapping
database without bringing a second platform layer.

- **Touches:** `crates/cordial-runtime/src/android/input.rs` (or equivalent),
  the GameActivity motion-event path.
- **Scope:** moderate. Isolated PR.

### Camera

89 hits on Camera2. The engine expects an Android `CameraManager`/`CameraDevice`
and Cordial answers none of it, so anything camera-driven silently does nothing —
the `broken_feature` shape from AGENTS.md.

- **Touches:** new `native/camera_classes.cpp`, a PipeWire/v4l2 source.
- **Scope:** major. Lower priority than voice unless something specific needs it.

**Order for these four:** gamepad (moderate, isolated, immediately useful) →
voice (major, best understood) → camera (major, no current demand) → VR (closed).

---

## mimalloc — revised to yes

The earlier verdict here was "try it, measure it", which was over-cautious. The
case for adopting is stronger than that, on three grounds that do not require a
profile first:

**The workload is mimalloc's designed-for case.** A game engine doing many small,
short-lived allocations across many threads is precisely the pattern mimalloc's
free-list sharding and thread-local heaps target, and precisely where glibc's
malloc arena contention costs most. This is not a general "allocators are faster"
claim; it is a specific match between this workload and this allocator.

**Sober uses it, and Sober works.** When the reference implementation that reaches
gameplay has made a choice and we are diverging from it for no reason, the
divergence is the thing that needs justifying, not the adoption.

**It is cheap and trivially revertible.** Link mimalloc's override into
`cordial-run` so it wins symbol resolution ahead of glibc; remove one line to
undo it.

**The one real technical question, which is not a hedge.** Cordial runs with
`--host-libc`, so the engine resolves libc symbols through the host, and the AOSP
bionic linker does its own symbol resolution. Whether mimalloc's override
actually captures the *engine's* allocations — as opposed to only Cordial's own —
depends on how that resolution binds `malloc`. If the ported linker binds it from
`libc.so`'s handle directly rather than through the global symbol table, the
override captures nothing and the change is a no-op wearing a performance badge.

That is checkable rather than arguable: mimalloc reports allocation statistics
(`MIMALLOC_SHOW_STATS=1`). If the engine's allocations are flowing through it,
the counts will be large; if they are not, they will be Cordial-sized. **Do that
check first — not to decide whether to adopt, but to know whether the adoption
did anything.** Shipping a no-op as a performance improvement is the exact
failure mode AGENTS.md exists to prevent.

- **Touches:** `crates/cordial-runtime` dependency list, one `#[global_allocator]`
  or link-order change in `cordial-run`. Possibly the linker's symbol table
  handling if the check comes back negative.
- **Scope:** trivial to add, moderate if the shim needs to route it.
- **Priority:** high. Do it after T1.

---

## Client integrity: mocktail does not address it

Asked directly and answered plainly, because the possibility was worth checking
and the answer is no.

Across all 41 commits there is **no** commit mentioning integrity, signature
verification, anti-cheat, Hyperion, Byfron, bans, or disconnects. Searching the
source, the only `integrity` hits are `src/update/payload_integrity.cc`,
`candidate_approval.cc` and `apk_bundle.cc` — that is *update payload* integrity,
verifying a downloaded APK bundle before installing it. Unrelated to client
attestation. There is no handling of a 304 or any server-initiated disconnect
anywhere in the tree.

So mocktail is not a source of a fix for the error Cordial hits at 60 seconds.
It is a useful source for texture handling, platform layering and packaging, and
it is silent on this. Anyone reading this backlog hoping the comparison would
close the 304 should stop here rather than go looking.
