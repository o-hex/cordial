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

### T1. `FStringGraphicsTextureManager2DenyPattern2 = ".*"`

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
- **Scope:** trivial. Isolated PR.
- **Verification:** requires a join and a visual comparison, since textures do
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
