---
name: A Roblox feature does not work
about: Something the client does on Android that does nothing, or does the wrong thing, on Cordial
labels: framework-gap
---

Roblox expects Android underneath it. Cordial answers those expectations one at a
time, and everything not yet answered is a feature that silently does nothing.
Audio is the current example — `slCreateEngine` reports
`SL_RESULT_FEATURE_UNSUPPORTED`, honestly and on purpose, so there is no sound.

This is the project's main body of work, not a defect queue.

## Which feature

What you tried to do, and what happened instead. "Nothing happened" is a
perfectly good answer and is the usual one.

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

## What the engine asked for

Cordial records what Roblox reached for. Please include whichever of these
applies — a name here is most of the work.

**A Java class or method nobody implemented.** libjnivm prints these as it goes:

```text
[JNIVM]: Constructed Unresolved symbol, Class=`com/roblox/engine/jni/NativeGLJavaInterface`,
         StaticMethod=`onLuaTextBoxChangedCallback`, Signature=`(Ljava/lang/String;)V`
[JNIVM]: Call Unknown Static Function Class=... Method=...
```

`--dump-classes <file>` writes the whole set Roblox asked for.
`docs/analysis/observed-java-surface.md` is the running record.

**A native symbol answered by a stub.** Printed on exit:

```text
=== stubs called: N distinct of TOTAL ===
```

TOTAL is however many stubs this build of Cordial compiles in, which grows
whenever `docs/analysis/undefined-symbols.tsv` gains an entry -- it will not
match this example, and that is not a sign anything is wrong.

**Neither, but it still does nothing.** Say that. A feature can be broken because
a call is answered with a plausible lie rather than not answered at all, and
those are the expensive ones.

## What the right answer is

Three shapes, and picking the wrong one is the main way this goes wrong:

- [ ] **Stub honestly.** The platform genuinely is not there and the engine must
      be told so. `slCreateEngine` returns failure rather than handing back a
      dead engine object; `native/opensles.cpp` explains why at length.
- [ ] **Answer with real data.** Cordial knows the truth and just has to say it —
      the locale, the display metrics, the storage directories.
- [ ] **Translate to the host's equivalent.** Android API in, freedesktop or
      Linux facility out. Audio is OpenSL ES in and PipeWire out. Notifications
      are Android's in and the portal's out.

If you do not know which, say so and describe the feature. Choosing between them
is usually the actual design question.

## Please do not make it lie

A stub that returns success is worse than one that returns failure. The engine
proceeds on an answer that is not true and fails somewhere with no relationship
to the cause — `docs/NEXT.md` keeps a list of the times this project chased
exactly that. Reporting failure is not giving up; it keeps the gap where someone
can find it.

## On translations specifically

A translation is a real design decision and deserves an issue of its own before
code. Two things worth stating up front:

**It changes what Cordial needs from the sandbox.** A host facility means a
Flatpak permission, and Cordial's manifest is meant to stay narrow enough that a
person can read it and know the whole story
([ADR-007](../../docs/adr/ADR-007-host-resources-are-brokered.md)).

**Some have no clean equivalent yet, and saying so is a valid outcome.**
Passkeys are the honest example: Android routes them through Credential Manager
and FIDO2, and the desktop has no settled portal for the same thing. A partial
translation that half-works during sign-in is worse than a documented gap.

## Reading the binary

Symbol tables, `DT_NEEDED`, call order, and argument shapes — including method
prototypes declared in the dex, which is what `tools/dex_method.py` reads — are
all fair game and are how most of this gets solved. Transcribing how Roblox
*implements* something is not, and nothing from a decompiler belongs in this
repository. See [CONTRIBUTING.md](../../CONTRIBUTING.md).

## Confidence

- [ ] **Verified** — I ran it and this is what it did
- [ ] **INFERRED** — this follows from evidence but I could not test it directly

Both are welcome. Only the labelling matters.
