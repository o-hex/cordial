# Can a user choose which architecture's Roblox build Cordial runs?

Spike, 2026-09-30. Written against `main` at `8d0537a`.

**Short answer: not yet, and the reason is not in the updater.** Choosing the
other architecture's `libroblox.so` is the easy half (the download already
carries it). Running it needs a second copy of `cordial-run` built for that
architecture, its whole userland (GTK 4, libadwaita, Wayland, Vulkan loader, a
GPU driver) in that architecture too, and a CPU translator underneath. Nothing
measured or read here shows a translation route that reaches the GPU.

**What was and was not measured.** Everything below marked *measured* was
observed this session: file reads, `curl` against the mirror's metadata
endpoint, `flatpak remote-info`. **Phase 1, running an arm64 `cordial-run` under
qemu-user, was not done.** The one step it began with, pulling an arm64 Fedora
image, was refused by the session's permission classifier on a machine that had
frozen from memory exhaustion earlier the same day (about 5 GB available, 12 GB
of disk free), and it was not retried another way. The section "What remains to
measure" says exactly what is missing. No FPS, CPU or landing-UI figure for a
translated build exists in this document, and none should be quoted from it.

*Corrected 2026-10-01:* the short answer's "needs a second copy of `cordial-run`" and
section 2's "There is no (c)" were wrong, and section 4's verdict on Quest with them.
A third shape exists and has been built: the x86-64 `cordial-run` links an arm64
engine in-process, runs it under dynarmic, and answers its imports through generated
thunks, so its Vulkan reaches the host GPU
([ADR-053](../adr/ADR-053-vr-is-a-mode-of-the-android-runtime.md),
[`vr/dynarmic-design.md`](../vr/dynarmic-design.md)). It is used only for the Meta
Quest build. Nothing here was re-measured for the arm64 phone build, which Settings
still does not offer.

## 1. How the architecture is chosen today

It is a compile-time property of the binary. Nothing at run time chooses it and
there is no setting.

| Where | What | Fixed by |
|---|---|---|
| `cordial-update/src/apk.rs` | `HOST_ABI` (`x86_64` / `arm64-v8a`), `LIBRARY_IN_APK` | `#[cfg(target_arch)]`; `compile_error!` on anything else |
| `cordial-update/src/install.rs` | `SPLIT_APK` (`split_config.x86_64.apk` / `split_config.arm64_v8a.apk`), `build_dir()` = `~/.cache/cordial/build/<abi>`, `engine_dir()` = `~/.cache/cordial/lib/<abi>` | `cfg`, and `HOST_ABI` |
| `cordial-update/src/provider/mirror.rs` | `ABI_EXACT = "x86_64"` on **both** hosts; `ABI_BROAD` on the retry | constant, see below |
| `cordial-update/src/provider/local.rs`, `cordial-shell/src/install.rs` | Sober's `packages/<abi>/…` directory and the split filename | `HOST_ABI` / `SPLIT_APK` |
| `cordial-update/src/deno.rs` | Deno asset per architecture | `cfg` |
| `cordial-shell/src/launch.rs` | `loader_path()` finds `cordial-run` beside the shell or on `PATH` | the shell's own directory |
| `justfile` `client` recipe | `uname -m` to pick `abi_dir` | host |

The mirror is the subtle one. APKPure keys its bundles on the `x-abis` header,
and `x-abis: x86_64` returns one monolithic APK holding
`lib/{arm64-v8a,armeabi-v7a,x86_64}/libroblox.so`; `x-abis: arm64-v8a` returns
XAPK bundles that `held()`/`classify()` cannot open (`docs/multiarch.md`,
2026-09-24). So an aarch64 Cordial also asks with `"x86_64"` and extracts
`lib/arm64-v8a/libroblox.so` from the same file. Measured today against the
metadata endpoint (`x-cv: 3172501`, the values `mirror.rs` sends):

```
x-abis: x86_64      2.738.1397  2.738.1393  2.736.1408  2.732.1043  2.727.1199  2.721.1108
x-abis: arm64-v8a   2.738.1397  2.738.1393  2.737.1584  2.736.1408  2.735.1138  2.732.1043
```

Two ARM-only releases (2.737.1584 and 2.735.1138) are invisible to the narrow
filter. `docs/analysis/apk-mirrors.md` already recorded 2.735.1138; the second
is new. So: on an x86_64 host, an "arm64" choice would see **more** releases than
Auto does, and an aarch64 host is currently blind to arm64-only releases until an
x86_64 one follows (the known gap in `mirror.rs`'s `ABI_EXACT` comment).

**Where the "lockout" lives.** There is no lock to remove: the other
architecture is not refused, it is absent. `HOST_ABI`, `LIBRARY_IN_APK`,
`SPLIT_APK` and the two cache directory names are `cfg` constants read from
about a dozen call sites, and `cordial-run` is a binary of the host's
architecture. `Checked::obtainable` (shell `updater.rs`) already tells a user
"Roblox announced a build newer than anything installable here"; that is the
existing seed of the "ask the user to switch" behaviour.

**The store is not keyed by architecture.** `~/.cache/cordial/builds/<version>/`
holds one `libroblox.so` (ADR-033, ADR-037). Two architectures at the same version
would collide on the directory name, and `.content-sha256` would silently
describe whichever landed last. The single-slot symlinks (`build/<abi>`,
`lib/<abi>`) are already ABI-named, which is the only part that is ready.

## 2. The coupling: the other architecture's engine needs the other architecture's `cordial-run`

**Confirmed.** No thunk layer sits between them, and building one is not a
reasonable option. *(Corrected 2026-10-01: one has been built, for the Quest build:
signatures generated from Khronos's `vk.xml` and `xr.xml` and hand-written stubs for
the rest, with layout gates finding 0 differences between the arm64 and x86-64
structs it carries. See the note at the top.)*

- The bionic linker loads `libroblox.so` **into `cordial-run`'s own address
  space** and refuses a mismatched ELF: `GetTargetElfMachine()` in
  `third_party/mcpelauncher-linker/bionic/linker/linker_phdr.cpp:53` is compared
  against `e_machine` at line 263 and the load fails with a readable message.
- Engine and host then call each other directly, by function pointer, in the
  native ABI: `symtab.rs` hands the engine host `libm`, `libz`, GLES2/EGL and a
  handful of `pthread_once`-style libc entry points as raw addresses, plus 650
  Cordial-written stubs and the `libjnivm` surface. A cross-architecture boundary
  there would need per-symbol marshalling for over a thousand entry points,
  callbacks in both directions, and struct-layout translation (the pthread
  work in `bionic/pthread.rs` is exactly that problem at five types).
- `cordial-run` links GTK 4, libadwaita, Pango, Cairo and GLib in-process
  (`readelf -d` on `target-aarch64/release/cordial-run`). The text field is a
  real GTK widget (AGENTS.md), so the GTK stack is not a helper process that
  could stay native.
- The engine `dlopen`s `libvulkan.so.1` from **inside** that process
  (`symtab.rs:367`, `:385`), so the Vulkan loader and the ICD are whatever
  architecture `cordial-run` is.
- Architecture-specific source is small (`bionic/pthread.rs` aarch64 wrappers,
  `looper.rs` `EpollEvent` packing, `native/thread_trace.cpp`, and `libbadcpu`
  which is x86-only). It is a *port*, not a coupling; the coupling is the
  in-process load.

So the only shapes that can work are (a) `cordial-run` of architecture B under a
CPU translator on an architecture A host, with B's userland, or (b) two host
installs. There is no (c).

## 3. Translation candidates

### 3a. arm64 `cordial-run` on an x86_64 host, under qemu-user

- **CPU.** qemu-user with binfmt_misc is present on this host (`qemu-aarch64-static`
  10.2.2, `binfmt_misc/qemu-aarch64` enabled with the `F` flag). `docs/multiarch.md`
  records (2026-09-23/24, not repeated here) that an emulated arm64 `cordial-run`
  reached the bionic linker, `JNI_OnLoad`, GameActivity init and 139 flags with no
  crash. That says the loader works under emulation. It says nothing about
  speed or graphics.
- **Userland.** Needs an aarch64 root: glibc, GTK 4, libadwaita, libwayland-client,
  libvulkan, xkbcommon, PipeWire/Pulse client libs, WebKitGTK if the web view is
  in-process. An aarch64 OCI image is the practical sysroot (the same way the
  existing `target-aarch64/` was built: a Fedora 44 aarch64 container under
  qemu, `bionic/pthread.rs` header comment).
- **GPU.** This is the wall. The Vulkan ICD would be an *aarch64* Mesa driver
  talking to `/dev/dri/renderD*` with DRM ioctls. qemu-user translates ioctls
  from a table of known numbers and structure layouts; the driver-specific DRM
  ioctls (amdgpu, i915/xe, nouveau, NVIDIA's own) are variable-length and
  driver-defined. **INFERRED**, from how qemu-user's ioctl table is built, not
  measured here: a hardware ICD will not work, and the loader will fall back to
  llvmpipe/lavapipe, i.e. CPU rendering running on a CPU emulator. That is the
  "hopelessly slow" case, and I would not expect it to reach a playable frame
  rate. The measurement that would settle it is small (`vulkaninfo --summary` in
  the aarch64 container with `/dev/dri` passed through) and is listed at the end.
- **Flatpak.** Not supported by any Flathub extension. Measured with
  `flatpak remote-info --system flathub`: `org.freedesktop.Platform.Compat.aarch64`
  exists **only** at `aarch64/25.08` and `Compat.x86_64` **only** at `x86_64/25.08`;
  asking for the crossed pairs answers `Can't find ref`. Both are extensions of
  the same-architecture Platform, so none of these supply a foreign-architecture
  userland. `flatpak --supported-arches` here lists `x86_64` and `i386`. An
  arm64 Cordial would have to be installed as its own `--arch=aarch64` Flatpak
  and run under host qemu binfmt, and `flatpak-spawn`ing it from the x86_64
  shell crosses the sandbox. **INFERRED**: workable only with two apps, and the
  shell could not launch one from the other without a host permission ADR-007
  would object to.
- **Packaging.** A second `cordial-run` (244 MB unstripped here) plus a sysroot
  of several hundred MB inside every x86_64 package. Not proportionate.

### 3b. x86_64 `cordial-run` on an arm64 host, under FEX-Emu

Cannot be tested here (no arm64 hardware); assessed from documentation only.

- **Model.** FEX needs an x86-64 RootFS and offers thunks that forward calls
  such as Vulkan and OpenGL to the host's native libraries (FEX README,
  fetched today). That is the only route in this document with a documented
  path to GPU acceleration, and the reason it is plausible where 3a is not: the
  guest's `libvulkan` becomes a thin shim over the host's driver.
- **Requirements.** ARMv8.0 with FEAT_FP and FEAT_CRC32 (README). Page size: the
  fetched pages say nothing, and this repo's own `docs/multiarch.md` records
  that half the ARM hardware people ask about (Asahi, Pi 5) is 16K, which
  breaks the assumptions of the bionic linker's 16K path. Whether FEX presents
  4K pages to an x86_64 guest on a 16K host was **not established**.
- **What Cordial adds to the problem.** The thing FEX would emulate is not a
  game but a process that loads a 117 MB Android engine with its own linker, its
  own TLS setup and a SIGILL handler for CPU-feature emulation (`libbadcpu`).
  Whether FEX handles that is **INFERRED unknown**; it is the same class of
  thing Wine-under-FEX does, but nobody has tried it here.
- **Flatpak.** FEX wants its binaries in an x86_64 rootfs plus extra host
  libraries for thunking (graphics drivers, libwayland, libdrm); a Codeberg
  project (`valpackett/fexwrap`) exists to run Flatpaks under FEX, described as
  "janky". A Flatpak cannot depend on FEX today: there is no runtime extension
  supplying either. Sober users on arm64 hit the same wall: Sober issue #346, "I
  could run it with box64, but flatpak won't let me install it".
- **Packaging.** An arm64 Cordial that offers "x86_64" would ship or depend on
  an x86_64 `cordial-run`, an x86_64 GTK 4 rootfs and FEX. Same disproportion
  as 3a, and worse because the native arm64 build exists and is the default.

### 3c. x86_64 `cordial-run` on an arm64 host, under box64

Also untested. box64 "leverages native system libraries" rather than requiring
a full rootfs, and wraps a list of libraries including some GTK, EGL, GLESv2 and
Vulkan (box64 README and `docs/USAGE.md`, fetched today; the fetched excerpt did
not say which GTK). Two points cut against it for Cordial:

- Its wrapped GTK is documented as GTK 2, and GTK 3 "being worked on" in the
  search results; **GTK 4 and libadwaita are not documented as wrapped**, so
  `cordial-run` would emulate them, which is slow and unproven. **INFERRED**, not
  tested.
- Its dynarec cache writes up to 2 GB of files (README).

### 3d. What Sober did about it

Read from `tools/sober-corpus/data/raw.jsonl` (2,000+ issues, ARM keyword
search: 22 hits on title, eight relevant):

- Sober declined translation. Issue #687 (maintainer): the compatibility layer
  depends on Linux **syscall user dispatch**, "only available on x86_64"; ARM
  "would massively increase the maintenance overhead and most of us don't even
  have a device". #346: "wont be happening for a while due to various technical
  issues"; a suggestion to distribute outside Flatpak so FEX can translate was
  answered "We don't have plans for this".
- Sober went the way Cordial went: a native ARM port. #1148 says ARM was added
  to their update service; #1221 (open) reports "lots of progress" with a
  screenshot, and testers on Asahi, Snapdragon X and Jetson offering.
- No issue in the corpus reports Sober running under FEX, box64 or qemu with a
  measured result. The one FEX/box64 mention is a user who could not install
  Sober under them because of Flatpak. There is no Sober evidence for or against
  the speed of any translation route.

So the only other Android-Roblox-on-Linux project chose native ports over
translation, and its blocker was a kernel feature Cordial does not have to worry
about, which is a difference worth remembering rather than borrowing.

## 4. Quest

**Not viable, on evidence that is partly measured and partly inferred.**

*Superseded 2026-10-01:* the Quest build runs in VR mode under the in-process
translator ([ADR-053](../adr/ADR-053-vr-is-a-mode-of-the-android-runtime.md)). What
this section did not know is now known from a copy pulled off a Quest 3: the package
is `com.roblox.client`, the same name as the phone build; versionName 2.740.0.927
(2.740.927 in the design document); `lib/arm64-v8a/` only; signed by a certificate of its own
(`6d13fc84…`, self-signed, O=Roblox Corporation), pinned separately in
`packaging/trust/roblox-signing-certificates.json`. The mirror probe below is still
the only one made, and no mirror is known to serve the Quest build; Cordial
downloads none and takes it only from the user's own headset or an APK file they
supply. The Platform SDK it brings is answered as a host without Meta services,
every request failing ([`guest_ovr.rs`](../../crates/cordial-runtime/src/guest_ovr.rs)),
and no other Horizon OS service has stopped a recorded run. The "Dependencies" bullet's VR
finding is about the phone build.

- **What it is.** Roblox on Meta Quest is an app distributed through the Meta
  Horizon Store (Roblox Help "Meta Quest FAQ" and the Meta store listing exist;
  the store page carries no package name, version or technical requirements). A
  SideQuest listing also exists ("Roblox on Oculus Quest 2") and could not be
  confirmed as an official build.
- **Obtainable through Cordial's providers? No (measured).** The mirror's
  metadata endpoint, with the same headers `mirror.rs` sends, returned the normal
  510,604-byte listing for `com.roblox.client` and a 36-byte `INVALID_COMMAND`
  answer for each of `com.roblox.client.vr`, `com.roblox.vr`, `com.roblox.quest`,
  `com.roblox.client.quest` and `com.roblox.horizon`. **The real package name is
  unknown to me**, so this proves those five guesses are absent, not that the
  mirror has no Quest build. The store, local provider and Cordial's signature
  pin (`apk_signature.rs`, Roblox's certificate) are all built around
  `com.roblox.client`. Nothing was downloaded, so no manifest or dex was read.
- **Architecture.** Quest hardware is a Snapdragon XR2-class ARM part, so the
  build is arm64-v8a. **INFERRED**; `docs/multiarch.md` states "the Quest build
  ships no x86 code" without a source. Anything Quest is therefore a subset of
  the arm64 translation problem in 3a, with extras on top.
- **Dependencies.** `docs/analysis/vr-reachability.md` shows the desktop-Android
  engine never constructs a VR device under any flag, runtime, or Monado, with a
  4584-line class dump identical across every arm, and `libroblox.so` imports
  no OpenXR/Oculus symbol. A Quest build would be expected to bring Meta's
  OpenXR loader and Platform SDK (entitlement check) and to need Horizon OS
  services Cordial has no answer for. **INFERRED**, not read from any manifest.
  ADR-001/003 also rule out the only demonstrated way of making a VR device
  appear (a byte written at a per-build address).
- **A "Quest" dropdown entry would be a control that changes nothing.** That is
  the stub-lies rule (AGENTS.md) applied to UI. Do not ship it.

## 5. Recommendation

**Do not ship the non-host options.** Ship no dropdown that lists an option
Cordial cannot run. Specifically:

1. **x86_64 host, "arm64".** Not worth shipping. The GPU path is the unmeasured
   crux and the a-priori expectation (**INFERRED**) is CPU rendering on an
   emulated CPU. Do not add the option until a qemu-user `vulkaninfo` shows a
   hardware device and an in-game frame-rate number exists (section 7).
2. **arm64 host, "x86_64".** The only route with a documented GPU story (FEX
   thunks). It cannot be assessed without arm64 hardware, so it should be a
   request to a tester on an arm64 machine, not a feature. A tester's `fex`
   result on the landing screen (fps with input driven, per AGENTS.md, CPU) is
   the bar.
3. **Quest.** Not viable; no option. *(Superseded 2026-10-01: it runs, and is
   offered as "Play in VR" rather than as an option in this row; ADR-053.)*
4. **Auto.** Must mean "the architecture of the running binary" and nothing
   else: `HOST_ABI`, resolved once, never re-derived from what a mirror lists,
   never changed because a newer release exists elsewhere. That is already
   today's behaviour, so **shipping a read-only "Roblox build: <arch> (this
   computer)" row is honest and free.** If a later change makes it a dropdown,
   Auto must (a) write no config key of its own, so that a fresh profile has no
   state that could drift, and (b) when the host has no newer release, say so
   using the existing `newer_announced_than_obtainable` path rather than
   offering another architecture.
5. **Correct `docs/multiarch.md`, which is now partly stale.** It says "Reopening
   this requires reopening Task A". This spike is the reopening; the doc should
   link here. It also still describes the arm64 qemu smoke test as done without a
   command line anyone can repeat.
6. **The lockout should stay until a route works.** Consistent with the
   maintainer's instruction. Nothing here shows one.

If the maintainer wants the dropdown regardless, the honest version is a
setting that is shown only when a working translator is detected on `PATH`
(`qemu-aarch64` binfmt, or `FEXInterpreter`), labelled experimental, and whose
first launch says the cost. That still requires the second `cordial-run`, so it
is a packaging decision before it is a UI one.

## 6. Draft ADR outline (not an ADR; next free number is ADR-043)

**ADR-043: The Roblox build's architecture is chosen by the binary, and choosing
the other one needs a second runtime, not a setting**

- *Status:* proposed. *Supersedes in part:* the sentence in `docs/multiarch.md`
  that no translation layer will be designed. *Related:* ADR-001, ADR-003,
  ADR-007, ADR-025, ADR-033, ADR-037, ADR-039.
- *Context:* the maintainer wants a Settings > Updater "Roblox build" control
  (Auto / x86_64 / arm64 / Quest); the in-process load of the engine (section 2);
  what section 3 measured and did not.
- *Decision (proposed):* (1) Auto is the host ABI and never changes on its own.
  (2) The store becomes keyed by ABI *and* version before any second ABI is
  fetched (ADR-033/037 amendment: `builds/<abi>/<version>` or an `abi` stamp with
  the `.content-sha256` scoped to it). (3) `HOST_ABI` and friends become a
  runtime `Abi` value passed down, replacing `cfg` constants at the call sites in
  section 1; `LIBRARY_IN_APK` and `SPLIT_APK` become functions of `Abi`. (4) A
  non-host `Abi` is selectable only when a translator for it is detected and the
  matching `cordial-run` plus userland exist; otherwise the row does not offer it.
  (5) The updater never switches architecture silently: on "no release for the
  chosen build" it asks, names the cost, and leaves the choice. (6) Quest is
  rejected.
- *Consequences:* the `x-abis: x86_64` mirror pin has to grow XAPK reading
  before an ARM-only release (2.737.1584, 2.735.1138) can be seen; cost of
  the second `cordial-run` per package format; Flatpak has no route (section 3).
- *Alternatives considered:* a thunk layer between engine and host (rejected,
  section 2); a two-application Flatpak; distributing the translator; a
  Waydroid-style container.
- *Reopen when:* a qemu-user or FEX run shows a hardware Vulkan device and a
  measured frame rate.

## 7. What remains to measure

Each needs a machine state this one was not in (memory 5 GB available, disk
12 GB, and an explicit refusal of the image pull):

1. **Vulkan reachability under qemu-user (decides 3a): measured 2026-09-30,
   and it fails.** In an arm64 Fedora 44 container (`podman run --arch arm64
   --device /dev/dri`, `uname -m` = `aarch64`) with `vulkan-tools
   mesa-vulkan-drivers` installed, `vulkaninfo --summary` on this Intel host
   lists one device:

       deviceType = PHYSICAL_DEVICE_TYPE_CPU
       deviceName = llvmpipe (LLVM 22.1.8, 128 bits)

   So an arm64 client under qemu-user would render on the CPU, emulated. The
   route in 3a is closed on this hardware. Not separated: whether the Intel
   driver is missing from Fedora's aarch64 Mesa build or present but unable to
   reach the GPU through qemu-user; either way there is no GPU device.
2. **The arm64 `cordial-run`.** `target-aarch64/release/cordial-run` (built
   2026-09-24, 61 commits behind `main`) exists but was not used: it belongs to
   another session's target directory and is stale. A fresh build with
   `CARGO_BUILD_JOBS=2 nice -n 19` inside the arm64 container under emulation is
   many hours and several GB of RAM; not attempted.
3. **The arm64 engine.** The 229 MB universal APK from the mirror (which carries
   `lib/arm64-v8a/libroblox.so`) is not on this machine: the local APKs are
   Sober's x86_64-only `base.apk` and `split_config.x86_64.apk`. It would be
   fetched by `cargo run -p cordial-update --example fetch_probe -- --download`
   and checked by `--example verify-apk`.
4. **Landing UI, CPU and fps, native x86_64 versus qemu-user, same screen.**
   Nested headless sway (`sway` and `swaymsg` are in the `cordial` distrobox;
   the host has none), own `XDG_DATA_HOME`, profile `arch`, signed out, input
   driven for the whole window (AGENTS.md), sequential not concurrent. The x86_64
   baseline binary is `target-toolbox/release/cordial-run` (built 2026-09-30).
5. **FEX and box64.** Need an arm64 machine with an x86_64 rootfs. Not testable
   here. The questions are page size on 16K hosts, whether FEX thunks Vulkan for
   a process that `dlopen`s it under a custom linker, and GTK 4 speed.
6. **Quest package name.** One legitimate source for it (Roblox's own support
   article or the store listing's package metadata) would let the mirror probe be
   repeated with the right name. Even then a download would fail the signature
   pin unless Meta's build is signed by the same certificate, which is unknown.
   *(Answered 2026-10-01 from the headset: `com.roblox.client`, signed by a
   different certificate from the phone build's. So a mirror listing under that
   name is the phone build's, and a Quest copy from one would fail the phone pin.
   Cordial does not look; see section 4.)*

## Rules and caveats for this spike

- No signed-in launch, no client started, nothing on `wayland-0`.
- `git status` showed ` m third_party/mcpelauncher-linker` before this file was
  written; it was not touched and is not part of the commit.
- Eight small files in the session scratchpad are metadata responses only. Nothing
  was downloaded from the APK CDN.
