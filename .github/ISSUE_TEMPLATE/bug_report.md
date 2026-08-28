---
name: Bug report
about: Something behaves differently from what you expected
labels: bug
---

## What happened

## What you expected

## How to reproduce

The exact command, including any `CORDIAL_*` environment variables:

```bash

```

## Diagnostics

**Required.** Paste it below, exactly as it prints, in the fenced block:

```text

```

Get it from **Settings → Report a Problem**, which has a **Copy diagnostics**
button, or from a terminal:

```bash
cordial --diagnostics                                    # .deb / .rpm / Arch
flatpak run io.github.luohoa97.Cordial --diagnostics     # Flatpak
./Cordial-*.AppImage --diagnostics                       # AppImage
cordial-shell --diagnostics                               # built from source
```

It works even when the client cannot start at all — a missing library, a GTK
too old, no display — because it is answered before anything else in the
process runs, which is exactly the report that most needs it.

It reports which Cordial build you have and how it was installed, the Roblox
build if Cordial fetched it itself, `uname -a`, your distribution's name, and
your session type and desktop. A field Cordial cannot establish says
`unknown` rather than being guessed or left out.

**What it does not carry:** no account name, no session token, no profile
name, and no path under your home directory. It does carry your hostname,
from `uname -a` — not secret, but not nothing, which is why the block is
shown on screen before it is copied. Edit the hostname out first if you would
rather it not travel.

## The engine's own log

**This is the most useful thing you can attach.** Roblox writes its own log to
`<files>/appData/logs/<version>_<timestamp>_Player_*.log` — by default under
`~/.local/share/cordial/instances/default/data/files/appData/logs/`. Attach the
newest one, or the last ~50 lines.

```

```

## How often

Some bugs here reproduce on roughly one launch in three, and at least one moved
with machine load. If you can, say how many runs you tried and how many failed.

- Runs attempted:
- Runs that failed:

## System

Not covered by Diagnostics above:

- GPU and driver:
- Renderer (the log says `Vulkan Device:` or `GL Renderer:`):
