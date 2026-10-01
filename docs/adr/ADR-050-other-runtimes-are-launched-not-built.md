# ADR-050: Other runtimes are launched, not built

**Status:** accepted, implementation parked until after 1.0.
**Date:** 2026-10-01

## Context

[Mac O' Blox](https://github.com/narezy/macoblox) (MIT) runs Roblox's real
macOS desktop client on Linux through [Darling](https://www.darlinghq.org), a
compatibility layer for macOS programs. It installs Darling and the client
itself, and puts its own launcher in the app menu.

The desktop client has none of the problems that come from Cordial running the
Android build: the mobile (touch) interface in some games, the character
control scheme suspected in #29, a GTK editor drawn over Roblox's text boxes,
and `DF` flags reset by the engine's settings refresh. So the question was
asked whether Cordial should run the macOS client too.

## Decision

Cordial does not build a macOS runtime. Every part of Cordial below the launcher
exists to load the Android engine: the bionic linker, the JNI layer, the Android
framework answers and the AGDK input path. A macOS client needs Darling's
frameworks and its own graphics translation, and would reuse none of that.

What Cordial may do instead is **launch another runtime the user already has
installed**. A **Runtime** choice in Settings would list Cordial (the Android
build, the default) and every other runtime found on the machine, starting with
Mac O' Blox. Choosing one makes Play start that runtime's own launcher or
command.

- **Detection.** Look for the other runtime's own install: its `.desktop` entry
  and the command it installs. Offer it only when present, and say "not
  installed" with a link to its install instructions otherwise, the way the
  MangoHUD and shaders rows already work.
- **What carries over.** Only starting it. Cordial's profiles, plugins,
  FastFlags, live settings, doctor checks and report screen apply to the
  Android runtime only. The Settings row has to say so, rather than letting a
  user believe their flags reach a client Cordial is not running.
- **Responsibility.** Cordial installs nothing for another runtime: no Darling,
  no client download, no kernel module. Problems with that runtime go to its own
  project. The report screen names the runtime in use, so a report filed here
  about Mac O' Blox can be redirected.
- **Account risk.** The macOS client under Darling has its own integrity checks,
  and nobody here has measured how it behaves. Cordial makes no claim about it.

## Why parked

1.0 is gated on the signed-in startup freeze and on text boxes. Adding a runtime
switch now would divide attention just as those are being fixed. Detecting and
launching a runtime is small, so it is quick to do once 1.0 ships.

## Consequences

- If the launch-only switch is built, a new `runtime` key joins the shell
  configuration, applied at the next Play, with "Applies at next launch"
  wording, and Settings gains one row.
- If someone later proposes running a non-Android client inside Cordial itself,
  this record is the place to argue it, with the reuse question answered first.
