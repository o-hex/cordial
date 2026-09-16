# ADR-033: Roblox builds live in a keyed store, and a profile names one

**Status:** accepted, partly implemented (see the end)
**Date:** 2026-09-13
**Extends:** [ADR-015](ADR-015-fetching-the-roblox-build.md), [ADR-025](ADR-025-fetching-from-a-third-party-mirror.md)
**Related:** [ADR-012](ADR-012-profiles-and-instances.md), [ADR-013](ADR-013-per-profile-configuration.md)

## Context

There is one slot. `~/.cache/cordial/lib/x86_64` holds one `libroblox.so` and a
stamp naming which APK it came out of; `cordial_update::cache` compares the
stamp so a new APK does not leave the old engine in place. Nothing keeps a
second version and there is no way back.

That is fine until a Roblox build regresses, and then it is the whole problem.
The engine is the one component here nobody controls: a build lands, something
that worked stops working, and the user's only options are to wait or to find
an APK themselves. Sober users hit this and it is the most common reason to
want an older client.

**Fetching is already decided and is not reopened here.** ADR-025 permits
downloading from a third-party mirror — APKPure in practice, with
`pureapk.com`, `apkpure.com` and `winudf.com` allow-listed in `url_policy.rs` —
provided the APK's signing block verifies against the pinned certificate set
before anything is extracted. Everything ADR-015 forbids still holds. This ADR
changes where the result is *put* and how one is *chosen*, nothing else.

## Decision

**The cache becomes a store keyed by Roblox version.**
`~/.cache/cordial/builds/<version>/` holds the extracted library and the stamp,
and the current single-slot path becomes a symlink into it so existing installs
keep working without a migration step the user has to notice.

**A profile may name a version, and by default does not.** A profile with no
version pinned follows whatever the store's current build is, which is what
almost everyone wants and what happens today. A profile that names one gets
that one, is not moved by an update, and says so in the launcher. This is
ADR-013's shape — per-profile configuration, defaulting to the global answer —
and not a new mechanism.

**Rollback is selection, not a separate feature.** With a keyed store and a
per-profile pin, "roll back" is picking an earlier entry, so there is no
rollback code path to get wrong and no state that exists only during a
rollback.

**Each entry records the Cordial version that last loaded it.** This is a
compatibility matrix and not a list, because Cordial's own shim is versioned
too: an old Roblox build can need a symbol the current shim does not answer, and
that fails at load with `cannot locate symbol` before any window appears. A
picker that offers a build nothing here has ever loaded is offering a crash. An
entry with no such record is offered with that said, not hidden.

**The store is bounded and the bound is by count, not age.** Keep the current
build and the two before it by default. An engine directory is not small and an
unbounded store is a disk-full bug reported as something else — this project
has already lost a session to a full disk once.

## What this deliberately does not do

**No per-profile downloads.** One store, shared; a profile names an entry in
it. Per-profile copies would multiply a large directory by however many
profiles someone keeps, for no benefit — the profile is storage and identity
(ADR-012), not a place to keep an engine.

**No pinning to a version the store cannot verify.** A named version that is
not present is fetched through ADR-025's path, signature check included, or
refused. There is no "use this APK unchecked" escape hatch, because that is
the one thing the pinned certificate set exists to prevent.

**No claim that older builds still work.** Roblox enforces a minimum client
version server-side and will refuse an old one whenever it chooses. The
launcher must say that plainly next to the picker rather than let a user
conclude Cordial broke. An old build that the server rejects is the expected
end state of every pin, eventually.

## Open, and worth arguing about

**Whether discovery goes to the network.** APKPure keeps old versions, which is
most of why it is the mirror, but listing them means parsing an index nobody
publishes as an interface and which can change shape without warning. Putting
that on the launcher's startup path buys a longer list at the cost of a new way
for the launcher to be slow or wrong. The alternative is to offer only what the
store already holds plus whatever the current fetch finds -- a shorter list,
honest about itself, and available offline. That is where this should start.

**Certificate rotation.** ADR-025 pins Roblox's signing certificates, and an
older APK verifies against whichever certificate signed it. So the pinned set
can only ever grow: prune it and old versions silently become unfetchable, with
a signature failure as the symptom and no hint that the cause was a tidy-up
years earlier. Cheap to get right now, expensive to discover later.

**Whether the pin belongs to the profile or to the launch.** A profile-level pin
is simpler and matches ADR-013; a per-launch override would let somebody test an
older build without disturbing a profile they play on. The second is cheap to
add later and impossible to remove, so it is left out until somebody wants it.

## As built, 2026-09-13

`cordial_update::store` is the store. What the decision above left open, or got
wrong, and what was settled while building it:

**An entry is the engine and the archives, not the engine alone.** Assets come
out of `base.apk` at runtime, so an entry holding an old `libroblox.so` beside
whatever APK is newest is exactly the silent version mismatch
`cordial_update::cache` exists to prevent. Entries keep `base.apk` and the split
by hard link, which costs no disk. There is deliberately no copy fallback: an
APK on another filesystem (Sober's, say) is not duplicated silently at 230 MB,
and the entry is recorded as incomplete instead.

**A pin refuses; it never falls back.** A profile pinned to a version the store
lacks, or holds without its APK, does not launch, and says which. The paragraph
above promising to fetch a missing pinned version is not built at launch: a
missing pin is still refused there. Fetch-by-version exists only as the Version
page's download, below, which the user starts.

**The pin is `profiles/<name>/roblox-version`**, one line of text, checked
against the same digits-and-dots whitelist as the directory name. Pruning
protects every profile's pin, and a pin counts towards the bound of three, so
pins displace unpinned builds first. Four profiles pinned to four versions
keep four; a pin is never pruned to honour the bound.

**The single-slot path is only a link when the version is known.** A build
whose version was never recorded stays a real directory, as before. Anything
writing to the slot has to detach the link first or it writes into a kept
build; the updater, the shell's extraction and `just client` all do.

**`.loaded-by` is written by `cordial-run` after `dlopen` succeeds**, and names
the Cordial version. Nothing reads it yet.

**The shell had never recorded a version for a Sober-sourced build.** It scanned
`split_config.x86_64.apk` for the version string. Measured, that returns
nothing and the extracted `libroblox.so` returns `2.738.0.1397` (INFERRED: the
library is stored compressed in the archive). It scans the engine now.

**The picker is Settings → Version.** One row per build the store holds, plus
"Follow the current build", each with a tick on the one the profile uses, a play
button and a remove button. Clicking a row pins it; play pins it and launches.
Play writes the pin rather than overriding one launch, so the question below
about per-launch pins is still open. A build kept without its APK is listed and
cannot be chosen. The minimum-version warning sits above the list. The current
build and any pinned build cannot be removed. The launcher's profile row says
"Pinned to Roblox X".

**An up-to-date install is keyed on launch**, not only when Roblox changes:
the shell's early return for a current cache used to skip the store entirely,
so an existing install would never have been migrated until the next update.

**Discovery goes to the network, but only from the Version page.** This
settles the first open question, differently from where it said to start. The
page asks APKPure for its x86-64 version list the first time it is shown, off
the main thread, and never at startup, so the cost it warned about -- a new way
to be slow or wrong -- is paid by a row on one page saying the list could not be
had, not by the launcher. Versions the store already holds whole are not
offered; the mirror's `2.738.1397` and the store's `2.738.0.1397` are compared
as one build. A download goes through `provider::obtain_into_store`: the same
signature check and shared-certificate rule as an update, then
`install::file_into_store`, which files the build as a store entry and leaves
the single-slot build and every pin alone. It takes its own lock and staging
directory under the store root rather than the install's, so a download does
not block an update.

**A download pins itself to the profile whose page started it.** Pruning runs
at launch as well as on install, and a freshly downloaded old build is the
oldest unpinned entry, so without the pin it would be removed the next time
anything launched. The page says so beside the list. The pin is written when
the download finishes even if Settings has been closed by then.

The rotation and per-launch questions stand.
