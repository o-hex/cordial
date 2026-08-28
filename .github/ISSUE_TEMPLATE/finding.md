---
name: Finding
about: Something you established about the engine — including something that turned out to be wrong
labels: finding
---

## What you established

## How you established it

Cordial's rule is that claims are worth what they were measured with. Say what
you ran, not only what you concluded.

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

## Confidence

- [ ] **Verified** — I ran something and this is what it did
- [ ] **INFERRED** — this follows from evidence but I could not test it directly

Both are welcome. Only the labelling matters.

## Does this contradict something already written down?

Several commits in this repository exist only to retract an earlier claim. If
`docs/NEXT.md` or a comment says something you have now disproved, say so — that
is the highest-value contribution here.
