# ADR-058: A game started by another program is a Cordial of its own, and inside gamescope it is X11

**Status:** accepted
**Date:** 2026-10-15
**Extends:** [ADR-012](ADR-012-profiles-and-instances.md), [ADR-024](ADR-024-x11-is-supported-again.md)
**Related:** [ADR-011](ADR-011-wayland-and-libadwaita.md), [ADR-013](ADR-013-per-profile-configuration.md), [ADR-019](ADR-019-development-control-surface.md), [ADR-031](ADR-031-the-launcher-outlives-its-window.md), [ADR-044](ADR-044-settings-reach-a-running-game.md), [ADR-055](ADR-055-the-launcher-and-the-runtime-are-two-programs.md)
**User page:** [`docs/partydeck.md`](../partydeck.md)

## Context

The ask was a way for [PartyDeck](https://github.com/partydeck/partydeck) to run Cordial, and gamescope support. PartyDeck is a split-screen launcher. Read from its source on 2026-10-15 (`src/launch.rs`, `src/handler.rs`; read for ideas, nothing copied), it starts one `gamescope -W w -H h -- bwrap ... <exec> <args>` per player. Inside, `bwrap` binds the host in with `--dev-bind / /`, hides `/tmp` and `/dev/input` behind tmpfs, and re-binds only the evdev and `js` nodes of the pads assigned to that player. It sets `SDL_VIDEODRIVER=x11`, `SDL_JOYSTICK_HIDAPI=0` and `ENABLE_GAMESCOPE_WSI=0`. A handler is a folder with a `handler.json`: an executable under a game root, an argument string in which `$PROFILE`, `$WIDTH`, `$HEIGHT`, `$RESOLUTION` and `$INSTANCENUM` are replaced, `K=V` environment and per-handler overrides of PartyDeck's own settings. By default it also redirects `HOME` per player.

That is a program that can pass an argument vector and an environment, and nothing else. Cordial could not be started that way, for three separate reasons, each measured below.

## Measured

On Fedora 44, gamescope 3.16.29, `--backend headless`, in the `cordial` distrobox. The build was `7abbaf5d5` plus this change; the controls are the same tree before it.

1. **gamescope without `--expose-wayland` exports `WAYLAND_DISPLAY=""`, set and empty**, with `DISPLAY=:2`. `android::backend()` asked whether the variable was *set*, chose Wayland, and `cordial-run` printed `no window: libadwaita would not initialise: Failed to initialize GTK` and exited 0. The launcher then reported `the client exited cleanly`.
2. **With `--expose-wayland` the socket is real and the compositor advertises no `wl_subcompositor`**, which is how the engine's surface is embedded in the GTK toplevel (ADR-011). The client printed `no window: compositor advertises no wl_subcompositor` and exited 0.
3. **The X11 path works under gamescope.** With `CORDIAL_X11=1` forced on the old build, the engine presented about 780 frames at a median 60 fps for 960x540, 960x1080 and 1280x720 outputs. `CORDIAL_FULLSCREEN=1` made the swapchain extent equal the output (`extent=960x1080`) and the compositor's own capture (`gamescopectl screenshot`, which sees what gamescope composited and not Cordial's swapchain) showed the page laid out for that size. Without it the 1280x720 window was scaled down into the output.
4. **The GTK launcher itself renders on gamescope's Wayland** (libadwaita header bar, dialog and all), so the launcher is not the problem; the embedding is.
5. **A second invocation of the shell is swallowed.** Two `cordial-shell` on one session bus: the second exited with status 0 after 0.03 s while the first kept running. `GApplication` registered under a fixed name forwards every later invocation, which is correct for a link clicked on a website and wrong for four players.
6. **The controller scan was `js0` to `js3`.** With a `/dev/input` that held only `js5`, as PartyDeck leaves it for the instance owning that pad, the old build never opened it (no `gamepad: opened` line with the natives resolved); the new one did.

## Decision

**1. The shell takes the instruction, not `cordial-run`.** `cordial --profile NAME --play [--place-id N] [--fullscreen] [--set KEY=VALUE]` ([`cli.rs`](../../crates/cordial-shell/src/cli.rs)). `cordial-run --profile X` already existed and takes its own lock, but it cannot find a build, owns no plugin host (ADR-055) and cannot read Settings. The request was to launch the shell with settings, and the shell is where those live.

`--play` goes through the `win.launch` action the Roblox button uses, never presents the launcher, and turns every dialog into a stderr line and an exit status: `1` could not start, `2` bad command line, `3` profile open elsewhere (as `cordial-run` exits), `4` no build, `5` the client could not open a window, otherwise the client's own. A caller cannot read a dialog, so a dialog would be a game that neither starts nor fails. `--place-id` queues `roblox://experiences/start?placeId=N`, the one shape `cordial_runtime::deeplink` documents the engine acting on; it reached the engine in a test ("the app shell asked to launch an experience").

**2. An invocation that names a profile or plays is `NON_UNIQUE`.** The fixed application id stays and keeps its meaning for a bare `cordial` and for a link: both are still forwarded to the open launcher. A non-unique application does not claim the bus name, so the user's own launcher keeps it. Two instances on one session bus ran side by side, one profile each; the control (5) is the same two starts without it. The profile lock (ADR-012) is unchanged and is what refuses two instances on one profile: a second `--profile BusyOne --play` printed the refusal and exited 3.

**3. What the command line asks for is not saved.** `--profile` and `--set` apply to this process; `shell_config::save` does nothing under them (`shell_config::hold_unsaved`). Two instances racing to write `shell.json` would leave one player's profile as the user's default. The overrides are applied inside `shell_config::load`, not only to the copy `main` holds, because `launch::spawn` and the live-settings watcher each read the file themselves. `--set` is typed by deserialising the whole document again, so it is checked by the rules `shell.json` is, and a wrong key lists the right ones. After a run, `shell.json` did not exist in the redirected config directory. The migration's own save runs first and is the one write allowed.

**4. Inside gamescope the game opens on X11.** The rule is `cordial_shell::gamescope::wayland_usable`: an empty `WAYLAND_DISPLAY` is no compositor, and gamescope's own socket (recognised by `GAMESCOPE_WAYLAND_DISPLAY`) is no compositor the engine can embed in, as long as there is a `DISPLAY` to fall back on. It is one function in the library half, as `doctor::uses_x11` is, so the loader, the launcher's banner and `--doctor` agree. The cost is the one X11 always carries (ADR-024): no in-app web window, so no sign-in from inside gamescope. The user page says to sign each profile in first.

**5. `--fullscreen` is a flag.** It sets `CORDIAL_FULLSCREEN=1`, which the launcher already hands to the client, because a split-screen window has to fill the output it is given and PartyDeck chooses the size.

**6. Choosing which controller an instance gets is the launcher's sandbox's job, not a Cordial setting.** PartyDeck's `bwrap` makes exactly one pad visible; Cordial's job is to use whatever is visible. So the scan reads `/dev/input` for `js*` nodes and gives each pad the lowest free engine id (up to four), rather than assuming `js0` to `js3`. The engine's id stays small and dense; whether it wants that is still `INFERRED`, as it was. A `CORDIAL_GAMEPAD_DEVICE` was rejected: it would duplicate, less safely, a mask the launcher already enforces, and PartyDeck would not set it.

**7. A client that cannot open a window exits 5.** It exited 0, which is why finding 1 looked like success. Measured on a nonexistent `DISPLAY`: the old build exits 0 and the new one 5.

**8. Profile names are not rewritten.** PartyDeck's guest profiles are `.Name`. ADR-012 refuses a name rather than altering it, a guest profile could not sign in under X11 anyway, and mapping `.Blinky` to a directory Cordial then keeps for ever would be a quiet side effect. The command line says so and exits 2. A profile that does not exist yet is allowed and is said once on stderr, because a typo creates one.

## Handlers

`packaging/partydeck/Cordial` runs `/usr/bin/cordial`; `packaging/partydeck/Cordial-Flatpak` runs `flatpak run`, so it needs no game-root path edit for a user install. Both set `profile_unique_dirs: false`, because PartyDeck's default of a private `HOME` per player would hide Cordial's profile store and Roblox build from every instance, and Cordial's own profiles are the isolation here. They also set `disable_mount_gamedirs`, since there is no game directory to overlay.

## Rejected

- **`cordial --gamescope`, wrapping itself the way `cordial-run --headless` wraps in `cage`.** Under PartyDeck it would be a gamescope inside a gamescope, composited twice, and gamescope's flags (`-W`, `--backend`, `-f`, `--force-grab-cursor`) are the caller's. The plain command is in the user page.
- **A Wayland path for gamescope.** It would mean embedding the engine's surface without `wl_subcompositor`, which is `wayland.rs`'s design, and the X11 path already works.
- **Forwarding a second `--play` to the first.** What a launcher of several games needs is the opposite.

## Not verified

- **PartyDeck itself was not run.** It is not installed here. What ran was a stand-in with its command shape: two headless gamescopes, each around `bwrap --die-with-parent --dev-bind / / --tmpfs /tmp --tmpfs /dev/input --ro-bind <pad> <pad>`, with `ENABLE_GAMESCOPE_WSI=0`; two instances rendered and each saw only its own `js` node. That stand-in's `--tmpfs /tmp` also hid its session bus socket, so Cordial ran there with no session bus; the shared-bus run (finding 5) is a separate one without the mask.
- **Pointer lock and mouse look under gamescope**, a real pointer, real pads, nested gamescope on a real compositor, SteamOS gaming mode. The test sessions were headless. `INFERRED` throughout.
- **The Flatpak.** It was not run, so as not to touch the maintainer's installed data. `INFERRED`: `--socket=fallback-x11` reaches gamescope's display; `--device=dri` alone means no pad reaches it; the sandbox does not see `GAMESCOPE_WAYLAND_DISPLAY`, so under `--expose-wayland` it needs `CORDIAL_X11=1`.
- **Concurrent first launches** share the build store. Two started 0.5 s apart ran fine once; that is one trial and not a result.
- **A signed-in profile** was not used (no signed-in launches here), so "starts signed in" is `INFERRED` from the profile directory being the thing every other launch uses.
