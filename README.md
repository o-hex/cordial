<p align="center">
  <img src="https://raw.githubusercontent.com/luohoa97/cordial/main/packaging/icons/org.cordial.Cordial.svg" alt="Cordial" width="460">
</p>

# Run Roblox on Linux natively — Plugins, all yours. (Please don't DMCA take down this hobby project)

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

## Status: early. It runs and draws; you cannot sign in yet.

| | |
|---|---|
| Loads `libroblox.so` natively | ✅ |
| App shell reaches `APP_READY (Landing)` | ✅ |
| Renders — Vulkan, with GLES2 fallback | ✅ |
| Networking / HTTPS | ✅ |
| Mouse: navigation, buttons, field focus | ✅ |
| Typing into text fields | ❌ the last step before sign-in |
| Audio | ❌ OpenSL ES is stubbed to report failure; nothing translates it to PipeWire yet |
| Clean shutdown | ✅ full pause/stop/destroy sequence |
| Stable | ✅ 26 consecutive clean launches |
| Frame rate | ✅ ~27 fps on Vulkan, ~33 fps on GLES |
| Signed in | ❌ login form renders and is reachable; typing is the blocker |
| Plugins | 🟡 host and capability broker built; not yet wired to the running client |

Measured with `vkQueuePresentKHR`: 656, 656 and 655 presents over 24 s across
three runs, unchanged by injected input — so it renders continuously rather than
on demand.

The blocker now is sign-in. Without a session the client sits on the logged-out
landing page, so there is nothing much to do with it.

**Do not install this expecting to play Roblox.** Install it if you want to work
on it.

## Install

> Cordial is **not** ready to play Roblox on. Install it if you want to work on
> it, or to watch it come up. You cannot sign in yet.

### 1. What you need

- x86-64 Linux
- **Clang** — AOSP bionic uses C11 `_Atomic` inside C++ headers and GCC rejects it
- An X11 session (Wayland works through XWayland)
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
- Roblox's official Android client, which **you supply** — Cordial ships no
  Roblox code, APK or assets and never will

From an installed APK you need the `lib/x86_64/` objects and the base APK.

### 2. Build it

Flatpak is the intended way. There is no hosted remote yet, so it builds from
source:

```bash
git clone --recursive https://github.com/luohoa97/cordial
cd cordial
packaging/build-flatpak.sh --install
```

For development, skip Flatpak and build the loader directly:

```bash
git clone --recursive https://github.com/luohoa97/cordial
cd cordial
cargo build --release
```

### 3. Run it

```bash
cargo run --release --bin cordial-run -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

A window opens, the engine comes up, and it renders Roblox's logged-out landing
page at about 27 fps. `--run` is how many seconds to stay up.

### 4. Useful knobs

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
`CORDIAL_FLAGS` at another file) with a flat object:

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

### 5. When something goes wrong

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
