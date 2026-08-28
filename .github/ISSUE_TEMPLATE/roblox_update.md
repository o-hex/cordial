---
name: Roblox updated and something broke
about: A new Roblox build fails to load, or needs symbols Cordial does not provide
labels: roblox-update
---

Roblox ships new builds constantly and Cordial tracks a moving target. This is
expected breakage, not a defect report — it is the most useful routine
contribution anyone can make.

## Roblox build version — required, and the whole point of this form

The build number. It is the first thing anyone needs to act on this, and the
easiest thing to leave out by accident, so put it here even though it will
usually also show up in the diagnostics block below.

Where to find it:

- **The diagnostics block's `Roblox` line**, below — but only if Cordial
  fetched this build itself. For an APK obtained elsewhere (Sober's cache, a
  manual download) it says `unknown (an APK Cordial did not fetch records no
  version)`, honestly, because Cordial has nothing to read the number from
  without parsing Android's binary manifest.
- **Roblox's own log filename**, which has it regardless of where the APK
  came from: `<files>/appData/logs/<version>_<timestamp>_Player_*.log`.

Also say where you got the APK.

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

## What it did

Paste the failure. The two shapes worth recognising:

```text
LOAD FAILED: dlopen failed: cannot locate symbol "SL_IID_ENGINE" referenced by "libroblox.so"
```

A **missing symbol** at load time. Data symbols fail the `DT_NEEDED` walk
outright rather than at first use, so the client will not start at all.

```text
=== stubs called: N distinct of TOTAL ===
```

A **called stub** — it loaded, but the engine reached for something Cordial
answers with nothing. Paste the names. TOTAL is however many stubs this
build compiles in and grows over time, so it will not match this example.

## Which symbols are new

`docs/analysis/undefined-symbols.tsv` is the list the stub table is generated
from. This is how to find what your build needs that it does not contain:

```bash
readelf --dyn-syms -W /path/to/lib/x86_64/libroblox.so \
  | awk '$7=="UND" {print $8}' | sed 's/@.*//' | sort -u > /tmp/new.txt
cut -f2 docs/analysis/undefined-symbols.tsv | sort -u > /tmp/old.txt
comm -23 /tmp/new.txt /tmp/old.txt
```

Paste the output. If it is short, that is the whole fix: append each as
`libroblox.so<TAB><symbol>` and rebuild. Plain libc names resolve against the
host automatically once listed.

## If a symbol needs a real implementation

Say what it belongs to. A symbol that needs behaviour rather than a name gets a
file in `native/` — `native/opensles.cpp` is the worked example, and its comment
explains why it reports failure rather than pretending to succeed.

**Please do not make a stub return success.** A stub that lies moves the failure
somewhere unrelated and costs the next person far more than the missing symbol
did.

## Reading the binary

Reading the symbol table, `DT_NEEDED`, call order and argument shapes is fine and
expected. Transcribing how Roblox *implements* anything is not, and nothing from
a decompiler belongs in this repository. See
[CONTRIBUTING.md](../../CONTRIBUTING.md).
