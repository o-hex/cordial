<p align="center">
  <img src="https://raw.githubusercontent.com/luohoa97/cordial/main/packaging/icons/io.github.luohoa97.Cordial.svg" alt="Cordial" width="460">
</p>

# Run Roblox on Linux natively — Plugins, all yours. (Please don't DMCA take down this hobby project)

> [!IMPORTANT]
> **This project is dormant and looking for a maintainer.** Its author has
> stepped back for a while and intends to return to hand it over rather than to
> resume work on it. Saying so at the top is fairer than letting anyone discover
> it after investing a weekend.
>
> It is not abandoned mid-collapse — it works. You can sign in, stay signed in,
> load a game, move around and turn the camera.
> [**docs/HANDOVER.md**](docs/HANDOVER.md) is written for whoever takes it on:
> every open thread with what is genuinely known about it, which claims are
> `INFERRED` and why, and the measurement traps that have already cost people
> afternoons.
>
> **If you fork this, please open a pull request as well.** Not instead — as
> well. Run your fork, ship it, do what you like with it; the licence is GPL-3.0
> and that is the point. But a change that exists only in a fork is a change
> that has to be archaeology later, and a change that exists as a PR can be
> merged in ten minutes by whoever picks this up.
>
> Pull requests opened during the dormant period **will** be read when a
> maintainer is assigned. They will sit for a while first, and that is worth
> knowing before you spend an evening — but they are not going into a void, and
> the queue is the first thing a new maintainer inherits. Keeping your branch
> rebased on `main` is the single most useful thing you can do to make that
> merge cheap.
>
> If you want to start smaller, the [good first
> issues](https://github.com/luohoa97/cordial/labels/good%20first%20issue) are
> real ones, and none of them needs a Roblox account.

Cordial loads Roblox's official Android x86-64 engine directly on Linux through a
purpose-built runtime: the AOSP bionic linker, a bionic/glibc shim, a JNI VM in
place of Android's, and a framework layer that answers the client's calls. No
emulator, no container, no virtual machine. It talks to your GPU through Vulkan
or GLES2 the way any native application does.

**It is also, as far as we know, the first user-extensible Roblox client.** Not
extensible in the sense of replacing files or setting flags — other launchers do
both — but in the sense that *you can write code that runs as part of the client
and adds functionality to it*. Plugins are ordinary programs in their own
processes, they get named capabilities rather than access, and Cordial's own
default features are built as plugins so the API has to be good enough for them.

To be exact about the claim, since "first" invites correction: browser extensions
extend Roblox's **website**; launcher mods replace **assets**; FastFlag managers
change **settings Roblox already reads**. None of those load user-written code
into the client. If a client that does already exists, we would genuinely like to
know.

What this is **not** is a way to modify Roblox itself. There is no script
execution, no hooking, and no memory access — absent from the API rather than
disabled. Plugins extend *Cordial*.

## Get started

- [Read the documentation 📖](docs)
- [Start here — what works and what is blocking 🧭](docs/NEXT.md)
- [Install it 🔽](#install)
- [How it actually works 🔬](docs/findings.md)
- [Why there is no script execution, ever 🔒](docs/adr/ADR-001-in-process-hooking.md)
- [Report a bug 🐛](https://github.com/luohoa97/cordial/issues)
- [Contribute 🛠️](CONTRIBUTING.md)

**New here?** Read the warning below first, then
[`docs/NEXT.md`](docs/NEXT.md) — it is written for someone picking the project
up cold and says plainly what is broken and what has already been ruled out.

### Disclosure

**This is NOT an official Roblox client. This project is in no way endorsed or
sponsored by Roblox Corporation.** Roblox is a trademark of Roblox Corporation.

**It was built in two days by [Claude Code](https://claude.com/claude-code)** —
Anthropic's Claude, model Opus 5 — with the architecture directed by a human
working alongside it. That is not a footnote. It is why the commit messages are
long, why `docs/` records what was disproved as carefully as what worked, and why
nobody should assume a human reviewed every line. The engineering is real and
every finding was verified by running the thing rather than reasoning about it.
It has still only existed for two days.

> ### ⚠️ Read this before using an account you care about
>
> Roblox does not support third-party clients and operates automated systems that
> ban accounts for using them, up to permanent termination. Those systems have
> produced false positives against innocent players.
>
> **Roblox has not approved this project, and has not been asked to.** There is
> no green light, no arrangement, and no reason to assume tolerance. Treat every
> claim below as our reasoning about risk, not as permission.
>
> Cordial does not modify the Roblox client — it runs the official Android build,
> does not touch the engine's process, and any asset overlay you enable is
> non-destructive and off by default — but it necessarily presents a synthesised
> Android environment, and a heuristic detector does not owe you that
> distinction. Alternate accounts are not a shield; Roblox's Terms reserve the
> right to terminate those too.
>
> Enforcement at this scale is automated and runs in waves, and accounts sharing
> an address get associated with each other. If you test, use a throwaway account
> on a different IP — see [CONTRIBUTING.md](CONTRIBUTING.md).
>
> **If your account matters to you, do not use it here.** If you use Cordial and
> get banned, that is on you, and the maintainers cannot get it reversed.

### We do not endorse exploiting

Cordial is a compatibility runtime, not a cheat tool, and **we do not endorse or
support using it to exploit Roblox or any experience running on it.**

That is not only a position, it is a property of the build. Cordial has no
script executor, no hooking, no memory access to the Roblox process and no API
by which a plugin could request any of them — not disabled, *absent*, so there
is no primitive in the binary to extract or re-enable in a fork. Plugins run in
a separate process behind a capability broker and cannot read Cordial's memory,
let alone Roblox's. The reasoning is in
[ADR-001](docs/adr/ADR-001-in-process-hooking.md) and
[ADR-003](docs/adr/ADR-003-plugin-isolation.md), and it is deliberately load-
bearing: a restriction can be lifted in a fork, a capability that was never
built cannot.

If you want an executor, this is the wrong project, and pull requests adding one
will be declined.

## Status: early, but playable. Sign in, load a game, move around.

| | |
|---|---|
| Loads `libroblox.so` natively | ✅ |
| Warm start | ✅ the engine is extracted once and reused; only a new Roblox build re-extracts |
| App shell reaches `APP_READY (Landing)` | ✅ |
| Renders — Vulkan on both backends | ✅ |
| Networking / HTTPS | ✅ |
| **Signing in** | ✅ **via Quick Sign-in**, which is a code flow and needs no typing |
| **Keyboard in an experience** | ✅ WASD, space, the lot |
| Mouse: navigation, buttons, field focus | ✅ |
| Mouse: turning the camera | ✅ right-drag; every button and a real delta reach the engine |
| Scroll wheel | ✅ |
| Frame rate | ✅ a flat 60 on MAILBOX, where FIFO gave a variable 35–50 |
| Feral GameMode | ✅ registered while the client runs |
| Typing into text fields | ❌ characters reach the engine and are not drawn until the field loses focus |
| Pointer capture in first person | ❌ Roblox is never told it may capture, so the cursor walks off the window |
| Staying signed in across a restart | ✅ cookies and identity kept in the **desktop keyring**, not a file |
| Loading into an experience | ✅ world, avatar and UI render, signed in |
| **Two accounts at once** | ✅ two profiles, two instances, side by side — see below |
| Window — libadwaita header bar, engine as a subsurface | ✅ |
| Launching from the shell | ✅ finds a build, or explains how to get one |
| Choosing a profile | ✅ a chooser above the Launch button; creates one, and shows a profile another window holds as unavailable |
| Audio | 🟡 sound leaves the OpenSL ES bridge into PipeWire, measured with a control; never yet verified inside an experience, and FMOD may take a Java path that bypasses it entirely |
| Web views (Marketplace, Profile, Communities…) | ❌ the surface is mapped; `openNativeOverlay` now reports instead of silently swallowing |
| Clean shutdown | ✅ full pause/stop/destroy sequence, observed in the engine's own log |
| Plugins | 🟡 host, broker, per-profile grants and settings, an on/off switch, and a registry with hardened unpacking; no marketplace yet |

Frame rate measured with pointer motion driven for the whole run, because
presents drop to exactly 1/s when nothing is happening and every earlier figure
in this repository was that idle throttle integrated: a flat 60.0 on MAILBOX
against a variable 35–50 on FIFO, four runs of 120 s.

**What is left is polish and two real gaps.** Text fields take the characters
and do not draw them until focus leaves, because on Android a transparent
`EditText` draws a focused box and there is none here. And Roblox is never told
it may capture the pointer, so in first person the cursor walks off the window —
the native that says so is exported and has never been called.

**The keyboard took a week and the answer was one number.**
`nativePassKeyEvent` wants Linux evdev codes; it was being handed Android
keycodes. Exactly one key worked — `D`, because `AKEYCODE_D` and `KEY_D` are
both 32 — and Alt made the character jump, because `AKEYCODE_ALT_LEFT` is 57 and
so is `KEY_SPACE`. Four theories were measured and disproved first, every one of
them assuming a number was wrong somewhere. The numbers were fine; the
vocabulary was.

**Two accounts at once, and it was not built as a feature.** A profile is
storage and an instance is a window ([ADR-012](docs/adr/ADR-012-profiles-and-instances.md)),
with an `flock` so one profile cannot be opened twice — which leaves nothing
stopping two *different* profiles running side by side, each with its own
session, settings and plugin grants. On Windows this traditionally needed a
second desktop session. Each instance is a whole engine, so budget around 1.5 GB
of memory apiece.

**Install it expecting rough edges.** It plays; it is not finished.

## Install

> Cordial is early. You can sign in, load an experience and play it with a
> keyboard and mouse; text fields still do not draw what you type, and the
> pointer is not captured in first person. The status table above says which
> claims were measured and how — read it before you install.

### 1. What you need

- x86-64 Linux
- A Wayland session. X11 still starts, through Flatpak's fallback socket, but
  [ADR-011](docs/adr/ADR-011-wayland-and-libadwaita.md) makes Wayland the
  backend Cordial targets and says X11 is not developed further
- Roblox's official Android client, which **you supply** — Cordial ships no
  Roblox code, APK or assets and never will

From an installed APK you need the `lib/x86_64/` objects and the base APK.

Nothing else. The Flatpak carries the toolchain and the libraries with it; the
list of build dependencies moved down to §3, where it belongs.

### 2. Install it

> [!NOTE]
> **The remote exists and serves, and the package installs.** Measured on
> 2026-08-05 against the published URL with flatpak 1.18.0: `remote-add`
> accepted, `remote-ls` returning the application ref at 6.1 MB to download and
> 16.1 MB installed, `install` placing both `cordial-shell` and `cordial-run` in
> `/app/bin`, and `cordial-run --help` answering from inside the sandbox. The
> appstream branch resolves and the metainfo validates, so a software centre can
> list it.
>
> Those measurements were taken under the previous application ID. The ID
> changed to `io.github.luohoa97.Cordial` in 0.5.1 — see
> [CHANGELOG.md](CHANGELOG.md) — so the ref name above is the one the next
> published build carries, not the one that was measured.
>
> **What has not been made to work is the shell opening a window.** From the
> installed package `cordial-shell` exits 0 immediately, printing nothing. The
> leading candidate is that a `GApplication` with a fixed id is single-instance
> by design, and a development build running from `target/release` already owned
> the name — but **the control for that has not been run**, so treat it as an
> open question rather than a diagnosis. §3 builds the same thing from source.
>
> [The workflow](https://github.com/luohoa97/cordial/actions/workflows/flatpak.yml)
> is worth a glance before a fresh install: it publishes only on a green run, so
> a red one on `main` means the remote is serving the previous build.

```bash
flatpak remote-add --if-not-exists cordial \
    https://luohoa97.github.io/cordial/cordial.flatpakrepo
flatpak install cordial io.github.luohoa97.Cordial
```

Then launch Cordial from your desktop's application list, or:

```bash
flatpak run io.github.luohoa97.Cordial
```

`flatpak update` picks up new builds. Uninstall with
`flatpak uninstall io.github.luohoa97.Cordial`, and
`flatpak uninstall --delete-data io.github.luohoa97.Cordial` if you also want the
profiles, the sign-in and the extracted Roblox build gone.

**The remote is not signed.** There is no GPG key on it, so `flatpak install`
verifies that the download matches the repository's own checksums and nothing
beyond that. What it does not do is prove who built it: anyone who can write to
the GitHub Pages site — including anyone who takes over the GitHub account, and
GitHub itself — can serve a different package under the same name and your
machine will install it without complaint. That is a weaker guarantee than
Flathub's and you should know which one you are getting. Signing is wired up in
[`.github/workflows/flatpak.yml`](.github/workflows/flatpak.yml) and switches on
the day a maintainer adds a key; the commands above do not change when it does,
but a remote added while it was unsigned stays unverified, so re-add it.

If you would rather not extend that trust, §3 builds the same package from
source and is the whole of the alternative.

### 3. Build it from source instead

Building needs rather more than running does:

- **Clang** — AOSP bionic uses C11 `_Atomic` inside C++ headers and GCC rejects it
- **GTK4 (≥ 4.10) and libadwaita (≥ 1.4)** development packages — the core shell
  in `crates/cordial-shell` is `AdwApplicationWindow`/`AdwToolbarView` end to
  end (see [ADR-002](docs/adr/ADR-002-core-shell-and-ui-handoff.md) and
  [ADR-011](docs/adr/ADR-011-wayland-and-libadwaita.md)), and `gtk4-sys`/
  `libadwaita-sys` link against them via `pkg-config` at build time. Fedora:
  `dnf install gtk4-devel libadwaita-devel`. Debian/Ubuntu:
  `apt install libgtk-4-dev libadwaita-1-dev`. Arch: `pacman -S gtk4 libadwaita`
- **PipeWire's development headers** (`pipewire-devel` / `libpipewire-0.3-dev`),
  optional — for OpenSL ES audio. `native/CMakeLists.txt` detects them via
  `pkg-config` and compiles the real audio backend if found, or the previous
  link-only stub (no sound, but everything else works) if not. Either way
  `libpipewire-0.3.so` itself is `dlopen`'d at run time, never linked, so a
  build made with the headers still runs — audio-less — on a machine that
  only has the runtime library, or neither.

To build the Flatpak yourself, which produces the same package the remote
serves:

```bash
git clone https://github.com/luohoa97/cordial
cd cordial
packaging/build-flatpak.sh --install
```

That one needs no submodules: the manifest pins `third_party/libjnivm` and
`third_party/mcpelauncher-linker` by commit and fetches them itself. It still
wants the network for the crates, which is
[issue #3](https://github.com/luohoa97/cordial/issues/3) and the reason Cordial
is not on Flathub.

For development, skip Flatpak and build the binaries directly. This one *does*
want the submodules:

```bash
git clone --recursive https://github.com/luohoa97/cordial
cd cordial
cargo build --release
```

### 4. Run it

From the package, the shell is what starts — it finds a Roblox build, or
explains how to get one, and launches the engine for you:

```bash
flatpak run io.github.luohoa97.Cordial
```

From a source build, the loader can be run on its own, which is what a debugging
session wants and nobody else does:

```bash
cargo run --release --bin cordial-run -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

A window opens, the engine comes up, and it renders Roblox's logged-out landing
page at about 27 fps. `--run` is how many seconds to stay up.

### 5. Useful knobs

| | |
|---|---|
| `CORDIAL_MONITOR=<n>` | open on the nth monitor instead of the primary one |
| `CORDIAL_FULLSCREEN=1` | cover that monitor |
| `CORDIAL_WINDOW_POS=<x>,<y>` | explicit position, overrides the above |
| `CORDIAL_RESOLUTION=<w>x<h>` | render resolution, default 1280x720 |
| `CORDIAL_DPI_SCALE=<f>` | UI density Roblox lays out against; 1.0 is a low-density phone |
| `CORDIAL_ANDROID_TRACE=1` | log Android API calls |
| `CORDIAL_COUNT_GL=1` | report graphics calls on exit |

```bash
CORDIAL_MONITOR=1 CORDIAL_FULLSCREEN=1 cargo run --release --bin cordial-run -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

`cordial-run --help` lists the rest.

### Changing FastFlags

Roblox is configured by FastFlags, and Cordial lets you override any of them.
Create `~/.local/share/cordial/profiles/<profile>/flags.json` (or point
`CORDIAL_FLAGS` at another file) with a flat object. Installed as a Flatpak the
sandbox moves `~/.local/share` to `~/.var/app/io.github.luohoa97.Cordial/data`, so the
same file is `~/.var/app/io.github.luohoa97.Cordial/data/cordial/profiles/<profile>/flags.json`
— `INFERRED` from how Flatpak remaps `XDG_DATA_HOME`, not yet checked against an
installed package.

```json
{
  "DFFlagRbxTransportUseRtcioRna": false,
  "FIntTaskSchedulerAutoThreadLimit": 8,
  "FStringDebugGraphicsPreferredBackend": "Vulkan"
}
```

Values may be written as booleans, numbers or strings — Roblox stores them all
as strings and Cordial converts. The overrides are merged into the settings
document the engine is given at startup, and the launch log reports how many
were applied.

**`FFlag`, `FInt` and `FString` are read once at startup**, so changing them
needs a relaunch. Only the `DFFlag`/`DFInt`/`DFString` family is re-read while
the client is running. That distinction matters if you are building anything
that changes flags dynamically — a plugin loaded part-way through a session
cannot change a startup flag, whatever it writes.

#### Layers and provenance

Flags come from more than one place, and each source owns its own file:

```text
<profile>/flags.json                             user    (always wins)
~/.local/share/cordial/plugins/<id>/flags.json   plugin
the client-settings document from Roblox         base
```

Your overrides live in the profile, so a flag you set while testing something on
one account is not silently still set on the account you play. A file left at
the old `~/.config/cordial/flags.json` is moved into the first profile that goes
looking for one — see [ADR-013](docs/adr/ADR-013-per-profile-configuration.md).

A plugin never writes to your file. That keeps three things true: a plugin
cannot silently overwrite a value you chose, removing a plugin removes its
flags, and "why is this flag set to that?" has an answer. Conflicts are reported
rather than resolved quietly:

```text
flags: FIntTaskSchedulerAutoThreadLimit = 8 from user
       (overrides plugin:fps-tweaks=4, plugin:net-tuner=16)
```

Two plugins disagreeing is a real disagreement, so both are named. The later one
wins so the outcome is deterministic, but nothing is hidden.

**If the interface looks coarse**, it is being laid out for a low-density phone.
Raise both — the render resolution is 720p by default and `dpiScale` is 1.0,
which is what Roblox treats as a cheap handset:

```bash
CORDIAL_MONITOR=1 CORDIAL_RESOLUTION=1920x1200 CORDIAL_DPI_SCALE=1.75 \
cargo run --release --bin cordial-run -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

Roblox's graphics-quality FastFlags (`DebugFRMQualityLevelOverride` and the MSAA
overrides) were tested and change nothing here, because they govern 3D scene
rendering and the logged-out landing page is a 2D interface. Resolution and
density are the levers that apply to it.

### 6. When something goes wrong

**Read the engine's own log first.** Roblox writes it to
`<files>/appData/logs/*.log` and it names subsystems, stages, paths and
exceptions in its own words. It is the best diagnostic in the project and most
questions are answered by the newest file in that directory.

To check whether input is reaching the engine, run with
`CORDIAL_ANDROID_TRACE=1` and look for `onTouchEventNative(...) -> true`.

## Documentation

Start with [`docs/NEXT.md`](docs/NEXT.md). The rest is reference.

| | |
|---|---|
| [`docs/NEXT.md`](docs/NEXT.md) | Where to start, what is blocking, and what has already been ruled out |
| [`docs/HANDOVER.md`](docs/HANDOVER.md) | Written for whoever takes this on: every open thread, which claims are `INFERRED`, and the traps |
| [`CHANGELOG.md`](CHANGELOG.md) | What changed between releases, retractions included. [Releases](https://github.com/luohoa97/cordial/releases) |
| [`docs/findings.md`](docs/findings.md) | Bootstrap analysis: the architecture verdict and what is unknown |
| [`docs/framework-api-inventory.md`](docs/framework-api-inventory.md) | The framework backlog, enumerated from the shipping APK |
| [`docs/traces/`](docs/traces) | A capture of the same APK on real Android — the ground truth this project checks itself against |
| [`docs/adr/ADR-001-in-process-hooking.md`](docs/adr/ADR-001-in-process-hooking.md) | Why Cordial has no in-process hooking, ever |
| [`docs/adr/ADR-004-plugin-asset-overrides.md`](docs/adr/ADR-004-plugin-asset-overrides.md) | Superseded by ADR-010 — why plugins were once refused asset overrides |
| [`docs/adr/ADR-005-flag-service.md`](docs/adr/ADR-005-flag-service.md) | Why the flag service has two surfaces |
| [`docs/adr/ADR-006-plugin-events-and-first-party.md`](docs/adr/ADR-006-plugin-events-and-first-party.md) | Plugin-declared events, and why built-in features are still plugins |
| [`docs/adr/ADR-007-host-resources-are-brokered.md`](docs/adr/ADR-007-host-resources-are-brokered.md) | Why a plugin never holds a socket, and Discord RPC as the worked example |
| [`docs/adr/ADR-008-plugins-are-typescript-on-deno.md`](docs/adr/ADR-008-plugins-are-typescript-on-deno.md) | Why plugins are TypeScript rather than Lua, and what a Deno start actually costs |
| [`docs/adr/ADR-009-capture-yes-overlay-injection-no.md`](docs/adr/ADR-009-capture-yes-overlay-injection-no.md) | Recording Cordial is supported; loading an overlay into it is not |
| [`docs/adr/ADR-012-profiles-and-instances.md`](docs/adr/ADR-012-profiles-and-instances.md) | A profile is storage, an instance is a window, and why one profile takes a lock |
| [`docs/adr/ADR-013-per-profile-configuration.md`](docs/adr/ADR-013-per-profile-configuration.md) | Flags, grants and plugin settings belong to the profile; plugin code belongs to the machine |
| [`docs/adr/ADR-010-plugin-asset-overlays.md`](docs/adr/ADR-010-plugin-asset-overlays.md) | Why plugins may now overlay Roblox's assets, non-destructively |
| [`docs/adr/ADR-014-plugin-registry-and-unpacking.md`](docs/adr/ADR-014-plugin-registry-and-unpacking.md) | Where plugins come from, and how an archive is unpacked without trusting it |
| [`docs/adr/ADR-015-fetching-the-roblox-build.md`](docs/adr/ADR-015-fetching-the-roblox-build.md) | Cordial may fetch a Roblox build and may never ship one |
| [`docs/adr/ADR-016-per-profile-network-egress.md`](docs/adr/ADR-016-per-profile-network-egress.md) | Why a profile can require a VPN, and what that does and does not guarantee |
| [`docs/adr/ADR-017-sober-issue-corpus.md`](docs/adr/ADR-017-sober-issue-corpus.md) | Why the local Sober issue corpus exists and what it deliberately drops |
| [`docs/adr/ADR-018-plugin-sub-sandboxing.md`](docs/adr/ADR-018-plugin-sub-sandboxing.md) | A kernel sandbox under Deno, why it cannot replace the broker, and the Flatpak grant not taken |
| [`docs/design/instances-and-launch.md`](docs/design/instances-and-launch.md) | Multi-instance, multi-account, and `roblox://` |
| [`plugins/README.md`](plugins/README.md) | Writing a plugin, and what a plugin cannot do |
| [`docs/design/sign-in.md`](docs/design/sign-in.md) | What signing in actually requires — the current blocker |
| [`docs/design/path-to-a-frame.md`](docs/design/path-to-a-frame.md) | GameActivity, assets, surface |
| [`docs/design/instances-and-launch.md`](docs/design/instances-and-launch.md) | Multi-instance, multi-account, `roblox://` handling |
| [`docs/base-evaluation.md`](docs/base-evaluation.md) | Port-vs-write assessment of the prior art |
| [`docs/multiarch.md`](docs/multiarch.md) | Multi-architecture decision |

## Headline findings

**Roblox ships a complete x86-64 Android build.** `split_config.x86_64.apk`
carries `lib/x86_64/libroblox.so` — 116 MB of x86-64 machine code built by NDK
r28c. Cordial executes it natively and needs **no CPU architecture translation**,
only CPU *feature* emulation. That is the difference between a tractable systems
project and one an order of magnitude larger.

**The runtime surface is bounded:** 13 Android libraries linked, 644 undefined
symbols, GLES2 + EGL mandatory with Vulkan `dlopen`ed as an optional upgrade.

**Roblox's game surface is AGDK `GameActivity`**, which is Apache-2.0 open
source — so the activity, surface, input and IME contract can be read rather than
inferred.

## Not in scope, permanently

No in-process code execution against the Roblox process: no hooking, no memory
patching, no injected script environment. Not "disabled by default" — absent from
the API vocabulary, so there is no injection primitive in the binary to extract.
Reasoning in [ADR-001](docs/adr/ADR-001-in-process-hooking.md).

Also out: client-side integrity flags or watermarks, and obfuscation-as-security.

## Licence

GPL-3.0-or-later. See [`LICENSE`](LICENSE).

Third-party components keep their own licences and notices, reproduced in
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) and installed alongside the
binary by the Flatpak:

- [`third_party/libbadcpu`](third_party/libbadcpu) — MIT, vendored from
  [Sober OSS](https://github.com/Z3ki/sober-oss)
- `mcpelauncher-linker` — MIT, ChristopherHX and MCMrARM
- AOSP bionic, carried within it — Apache-2.0 and BSD
- `libjnivm` — MIT, ChristopherHX

MIT and Apache-2.0 are satisfied while the combined work is offered under the
GPL, provided those notices travel with it. That is a condition, not a
courtesy.
