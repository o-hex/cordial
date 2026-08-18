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

---

## mimalloc: measured, and the answer is no. Roblox already ships it

Adopting was agreed on the argument that Roblox's workload — many small
allocations, many threads, CPU-bound — is mimalloc's designed-for case. The
argument is sound. It does not apply, because the engine got there first.

`libroblox.so` imports **no allocation entry point at all**. Filtering its 578
undefined symbols for `malloc`, `calloc`, `realloc`, `free`, `reallocarray`,
`posix_memalign`, `aligned_alloc`, `memalign`, `malloc_usable_size`, `strdup`,
`strndup` and the whole `operator new`/`delete` family leaves **`realpath` and
`vasprintf`** and nothing else. The C++ allocation operators come back **0**.

An engine that never calls the system allocator cannot be given a different one.
And `strings libroblox.so` says why:

    10  mimalloc
    48  Mimalloc

**Roblox links mimalloc statically into the engine.** So it is already running
under exactly the allocator this task proposed to give it, and the version it
uses is Roblox's own choice rather than ours.

Overriding `malloc` in `bionic::function_overrides` would therefore have reached
only Cordial's own Rust allocations, which are trivial next to the engine's. That
is the no-op-wearing-a-performance-badge outcome, and it is now measured rather
than guessed at.

**Do not adopt.** This also weakens the "Sober does it, so should we" reasoning
generally: Sober's mimalloc serves Sober's own process, not the engine's heap.

## detex: premise unproven, do not vendor yet

The engine already knows the formats. `strings libroblox.so`:

    DXT 21   ASTC 14   KTX2 14   ETC2 10   ETC1 7   BC3 3   BC7 1   BC1 0

and it links no third-party decoder — no detex, bcdec, etcdec, rgbcx, astcenc.
So format handling is the engine's own, and whether it ever hands the driver
something a desktop GPU cannot take is a **runtime** question that no amount of
reading answers.

Vendoring detex before knowing that is the same mistake mimalloc nearly was.
**The measurement first:** one join with the graphics log turned up, recording
which `VK_FORMAT_*` the engine requests and whether any are refused. If ETC
formats reach the driver, detex earns its place; if the engine transcodes to DXT
itself, it does not.

## JNI surface diff — 24 classes mocktail names that Cordial does not

The comparison that is actually apples-to-apples between a Rust project and a C++
one: not source, but which Java classes each answers. Cordial answers 62;
mocktail names 86.

Spot-checked rather than trusted, four of the five confirmed absent from Cordial:

| class | Cordial | mocktail |
|---|---|---|
| `com/roblox/engine/jni/memstorage/MemStorage` (+ `Connection`, `Callback`) | **absent** | yes |
| `com/roblox/engine/jni/autovalue/StartGameParams` | **absent** | yes |
| `com/roblox/engine/jni/EngineJavaCallback2` | **absent** | yes |
| `android/hardware/SensorManager` | **absent** | yes |
| `com/roblox/engine/jni/NativeAppBridgeInterface` | present | yes |

Also named by mocktail and unanswered here: `RobloxActivity`,
`MainGameActivity`, the `localstorageplatforminterface/generated/` family,
`PackageManager`/`PackageInfo`/`ApplicationInfo`, `DisplayManager`, `Display`,
`InputMethodManager`, `SurfaceView`/`SurfaceHolder`/`View`/`Window`/
`WindowManager`, `ViewRootImpl`, `Context`.

**`MemStorage` is the most interesting.** The engine exports
`MemStorage_bind`, `_fire`, `_getItem`, `_setItem`, `hasItem`, `removeItem` and
`Connection_disconnect`/`_releaseConnection` — a complete key-value channel with
a subscription mechanism — and Cordial answers none of it. **`StartGameParams`**
is second: it is on the join path, which is where Cordial's remaining problem
lives.

Two honest caveats. mocktail *naming* a class is not proof it implements it
usefully. And this diff was built by grepping `GetClass("...")` out of Cordial
and string literals out of mocktail, so a class Cordial answers by another route
would read as absent. **This is a lead list, not a verdict** — each entry needs
confirming against a JNI trace before anyone builds to it.

**Next:** run one `CORDIAL_JNI_TRACE=1` join and intersect the unresolved-symbol
lines with this list. That turns 24 leads into the subset the engine actually
asks for, and it costs one run.

---

## Correction: the 24-class list, re-read against the dex

The "Cordial answers none of `MemStorage`" framing was wrong in a way that
changes what the work is.

`MemStorage`'s methods are **engine natives**. `libroblox.so` exports
`Java_com_roblox_engine_jni_memstorage_MemStorage_bind`, `_fire`, `_getItem`,
`_setItem`, `hasItem`, `removeItem` and `Connection_disconnect`/
`_releaseConnection`. So the engine *implements* that channel and the application
*calls* it. There is nothing for Cordial to answer there, and an implementation
would be shadowing the engine's own.

What Cordial does owe is the two classes the engine constructs and calls back
into, exactly the `NativeFlagsInitResult` pattern:

    com/roblox/engine/jni/memstorage/Connection   <init> (J)V
                                                  disconnect ()V
                                                  finalize ()V
    com/roblox/engine/jni/memstorage/Callback     (engine calls into it)

`Connection` takes a native handle as a `jlong`, so it is a real, well-defined
object with no guessing involved. **Safe to implement.**

### `EngineJavaCallback2` is not safe to implement blind

Its entire surface is obfuscated — `a(I)V`, `b(J)V`, `c(I)V`, `d()V`,
`e(String)V`, through to `q(JZ[BLcom/roblox/engine/jni/model/NativeTextBoxInfo;)V`
— seventeen single-letter methods whose meanings the dex does not carry.

They are all `void`, so *receiving* them is honest in the way
`NativeHelper`'s lifecycle callbacks are: an announcement's honest answer is to
have received it. But anything beyond logging the call would be inventing
semantics. **Implement as receivers that log, and nothing more, and say so in
the comment.** Do not let a later reader mistake a named parameter for known
meaning.

### And the startup trace does not settle this either way

A `CORDIAL_JNI_TRACE=1` startup on 2.734.0.917 leaves only five classes
unresolved — `NativeGLJavaInterface` (14), `GameActivity` (6), `java/util/List`
(2), `java/lang/Class` (2), `NetworkUtils` (2). None of the 24 appear.

That is **not** evidence they are unneeded. `KeyRing` was recorded earlier in
this same investigation as reaching nothing on startup and logging normally on a
join, and `StartGameParams` is by its name a join-path class. A ten-second
startup trace only proves what startup asks for. The list stands; the trace to
settle it has to be a join.

## Discord Rich Presence: it exists and it works

Asked as "where is the plugin". It is at `plugins/discord-presence/` —
`plugin.json` declaring `lifecycle.read`, `presence.set` and `log`, plus
`main.ts`. The host side is `crates/cordial-plugins/src/presence.rs`, ADR-007's
worked example.

It is not a sketch. Four tests pass, including an end-to-end one that runs the
real shipped plugin:

    presence::tests::presence_set_speaks_discords_framing_to_a_local_socket ... ok
    presence::tests::details_over_the_discord_limit_is_refused ... ok
    presence::tests::a_payload_rejects_fields_discord_does_not_define ... ok
    discord_presence_follows_lifecycle_events_all_the_way_to_the_wire ... ok

**The gap is documentation, not implementation.** `README.md` mentions plugins in
general and links the Discord server, and never mentions that a Discord presence
plugin ships or how to enable it. `site/index.html` is the website. Both want a
section. That is a writing task, not an engineering one, and it should not be
filed as "fully implement Discord RPC" — the misfiling is the bug.
