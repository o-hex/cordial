# ADR-054: Cordial owns its Roblox builds, and Sober's copy is an import

**Status:** accepted, implemented (see "As built" for what is still INFERRED)
**Date:** 2026-10-04
**Amends:** [ADR-033](ADR-033-roblox-versions-are-a-keyed-store.md), [ADR-037](ADR-037-one-lock-and-a-content-hash-for-the-build-store.md), [ADR-025](ADR-025-fetching-from-a-third-party-mirror.md) (its "free source first" ordering and the paragraph keeping Sober on the first-run screen)
**Related:** [ADR-012](ADR-012-profiles-and-instances.md), [ADR-013](ADR-013-per-profile-configuration.md), [ADR-015](ADR-015-fetching-the-roblox-build.md), [ADR-043](ADR-043-the-roblox-build-is-the-binarys-architecture.md), [ADR-053](ADR-053-vr-is-a-mode-of-the-android-runtime.md)

## Context

ADR-033 built most of a store: `builds/<version>/` holds the engine and hard
links to the archives, a profile may pin one in `profiles/<name>/roblox-version`,
and Settings -> Version lists, pins, downloads and removes. What it did not do
is make the store the *only* place a launch gets its build from. Three things
still run around it, and the maintainer's complaint is all three.

**1. A launch resolves to Sober by reference.** `install::effective_apk`
(`crates/cordial-shell/src/install.rs`) goes `CORDIAL_APK`, then the Settings
APK, then `cordial_update::install::managed_base()` (`build/<abi>/base.apk`,
which exists only after Cordial has installed a build itself), then Sober's
`.var/app/org.vinegarhq.Sober/data/sober/packages/<abi>/com.roblox.client/base.apk`.
A machine with Sober and no prior Cordial install therefore never meets the
first-run screen and silently follows whatever Sober last downloaded, and
Settings says "Found in Sober's download". The comment on `locate` gives the
reason: Sober's copy is "far and away the least painful way for someone to
obtain" a build. `provider::local` says it "is the source most users should end
up on" and ADR-025 says the free source is tried first because the README told
people to install Sober. Those were true when Cordial could not download.

**2. Even "Download Roblox" prefers Sober.** `provider::all()` puts
`local::OnThisMachine` ahead of the mirror, and the first-run button asks for
`Want::Any` (`instructions.rs:154`), so with Sober present it *copies Sober's
archives* and reports a download. Only the Updates button asks for `Want::Newest`.

**3. The store is not what a launch runs from.** A launch uses the single
slot `lib/<abi>` (a symlink to the current entry when the version is known) and
only swaps in a store entry if the profile is pinned (`apply_pin`). "Follows
the current build" is "follows the slot", and the slot follows whichever
archive `effective_apk` found. Measured on this machine's Flatpak cache:
`lib/x86_64` is a real directory with `.from` naming Sober's `base.apk`, no
`.version`, and `builds/` empty. `adopt_current` keys only a build whose
version is recorded, and the up-to-date launch path never records one, so this
install has never been in the store at all. (The cause is INFERRED from the
code; the empty store is observed.)

**What the store cannot yet do is run two versions at once.** The engine
directory differs per entry, but assets do not: `load.rs::assets_root()` is
`$XDG_CACHE_HOME/cordial/assets`, one tree per build flavour, stamped with the
APK's path, size and mtime (`cache::stamp_for`). A second profile on another
version re-extracts over the first one's tree while it is running, which is
the failure ADR-053 records for phone against Quest ("rewrite files under a
client of the other build still reading them"). It also re-extracts when the
*same* version is reached once through `lib/<abi>` and once through an entry,
because the stamp includes the path. Both are read from the code, not run.

## Decision

**A launch resolves to a store entry, or to an explicit developer override,
and to nothing else.** The sources, in order:

1. `CORDIAL_APK` / `cordial-run --apk`: the developer override, never filed,
   never updated, labelled as such (unchanged).
2. The profile's pin, if it has one: that entry, or a refusal (unchanged, ADR-033).
3. **Latest**: the newest *complete, signature-recorded* entry in the store.

Sober's directory, `CORDIAL_APK_DIR` and the Settings APK path stop being
launch sources. They become **imports**: a user action that verifies the
archives, copies them into the store and is then forgotten.

**The store is keyed by what the engine says it is, and an entry proves its
bytes.** The directory stays named by the engine's version (`2.738.0.1397`),
as ADR-033 and ADR-037 decided; the maintainer's "by version code and
signature" is met as follows, not by renaming:

- **Version code is not a key.** The mirror reports one (`Available::code`);
  an APK from Sober or a file does not, and reading it means parsing the binary
  manifest (`cache::recorded_version` says so). A key some sources cannot supply
  is not a key. It is recorded in `.version-code` when known, for display only.
- **Signature is a record.** Each entry carries `.signer` (certificate SHA-256
  and the archive stamp it was checked against), written when the entry is
  filed by any route. An entry with no `.signer` is verified once at its first
  launch and never offered as a pin before that.
- **Content is `.content-sha256`** (ADR-037), now consulted: filing a build whose
  version exists with a *different* engine hash is refused by name, because two
  byte-different engines claiming one version is the one thing this store must
  not paper over. A build whose hash matches an existing entry under another
  label is linked, not duplicated (ADR-037's left-open `find_by_content_hash`).
- **Provenance is `.source`**: `mirror`, `sober`, `file` or `legacy`. The UI
  quotes it, so "imported from Sober" is a fact on disk and not a guess.

**Profiles choose "Latest" or a version, per profile.** The file
`profiles/<name>/roblox-version` is unchanged; absent means Latest. Latest
means the newest entry *in the store*, not the newest the mirror lists and
not whatever Sober holds. Updating is "download the newest into the store"
(`obtain_into_store`, which exists) and moves every Latest profile at their
next launch, without touching a pinned one.

**Cordial downloads by default.** `provider::all()` is the mirror alone.
The first-run button always downloads. Sober appears only as a second, explicit
action when its package directory is detected and holds a build the store does
not (compared by `version::same_build`):

> **Roblox build**, first run: "Cordial downloads Roblox for you and keeps its
> own copy. It checks Roblox's signing certificate before using it."
> Below it, only when Sober's build is found: "Sober's Roblox 2.738.0.1397 is
> on this computer. Copy it into Cordial instead of downloading (about 150 MB;
> Cordial does not change Sober's files and does not follow its updates)."
>
> **Settings -> Roblox**, replacing the APK and Engine rows: a store view and two
> actions, "Import from a file..." and (only when detected and not already kept)
> "Import from Sober...". An entry from Sober reads "Imported from Sober on 4
> Oct 2026". Nothing on the page says Cordial depends on Sober.

**Two versions at once is made true, not assumed.** Assets are extracted into
the entry (`builds/<version>/assets/`), derived by the runtime from
`store::entry_at(--lib-dir)`, falling back to today's shared tree for anything
outside the store (`--lib-dir` by hand). The assets stamp keys on the entry's
own `base.apk`, so Latest and a pin on the same version share one tree. Each
client holds a shared `flock` on `builds/<version>/.in-use` for its lifetime, so
garbage collection can tell a build in use from one that merely is not pinned.
The Quest store (`builds/arm64-v8a/`, ADR-053) is not a version directory,
is skipped by `list_in`, and stays out of this entirely.

**Garbage collection removes builds nothing uses, and says so.** Pure function
`gc_plan(entries, pins, in_use, spare)` returns the versions to delete:
everything that is not the newest, not pinned by any profile, not in use, and
beyond the `spare` newest of the rest. It replaces `prune_in`'s count bound
`KEEP = 3` (which counted pinned builds against the bound and kept two
unreferenced old builds). It runs after a filing and from the store view, never
at launch with a window open. **Default `spare = 1`**: the build before the
newest stays, because the reason the store exists (ADR-033) is that an
update regresses and the user wants yesterday's build in two clicks; spare 0 is
the literal "keep the newest only" and is an open question below.

**Migration, first launch after the upgrade, once and by construction.** For
the legacy state found on disk:

| Found | Does | Profiles |
|---|---|---|
| Slot already a link into the store (host, Sober or Cordial origin) | nothing to copy; the entry is already there | follow Latest |
| Slot a real directory / unkeyed build (this machine's Flatpak), or `effective_apk` is Sober's | verify, **copy** the archives (not link: the Flatpak sees Sober's directory through a read-only grant, so a hard link across that mount is expected to fail, INFERRED), extract, file as `source=legacy`/`sober` | follow Latest |
| Settings APK set | import it | **every profile without a pin is pinned to it**, because a user who chose a file meant that file and Latest would otherwise move them off it |
| `CORDIAL_APK` | nothing; it is a per-run override | unchanged |

It runs when the store has no complete entry and a legacy slot exists. After
it the store is non-empty and the newest entry cannot be removed, so it cannot
run again; a wiped cache with Sober still installed gets the first-run screen
with both buttons, not a silent re-import. **Nothing of Sober's is deleted,
moved or written to, in any path.** The `roblox.apk` Settings field is cleared
after its import and the release note says so.

**Offline.** Launching never touches the network. Latest and pins resolve from
disk. The Updates page and the Version page's list are the only network
callers, as today. A pin whose entry is missing is refused with the existing
message, and the Version page offers its download.

**A pin Roblox no longer accepts.** Roblox enforces a minimum client
version server-side and Cordial cannot ask what it is: the `AndroidApp`
version endpoint answers 500 (`version.rs`). What the user sees is therefore
whatever the engine shows when joining is refused, which Cordial has never
captured (**INFERRED**: an update-required message or a failed join; the doctor
text assumes "an update message"). Cordial does not guess. A pinned profile's
row says "Pinned to Roblox X, N releases behind the newest build on offer"
when the list is cached, the existing sentence "A pinned build that stops
joining games was retired by Roblox, not broken by Cordial" stays, and a
pinned launch that exits within the early-exit window adds one line to the crash
page: "This profile is pinned. If Roblox asked you to update, choose Latest."
A build whose signer is no longer in the pinned certificate set (the set only
grows, ADR-033) is refused at launch naming the fingerprint.

**Moving a profile between versions is a warning, not a migration.**
A profile records nothing about the build that last ran it. Add
`profiles/<name>/last-roblox-version`, written when the client is spawned. Pinning
a build older than it shows: "This profile last ran Roblox X. An older build may
not understand what a newer one saved. Cordial has not measured what happens;
Settings -> Roblox -> Stored data clears it." Per-profile `data/` is not
snapshotted (`profile.rs` records its cache alone at 2.0 GB) and is not
touched. Nothing measured says same-flavour downgrades corrupt anything;
ADR-053 measured different caches and different `GlobalBasicSettings_13.xml`
keys between 2.737 and 2.740 only across phone/Quest builds. **INFERRED**
throughout.

## Consequences

**Cordial depends on nothing of Sober's after one import.** The cost is the
one ADR-025 named for the first-run screen: a mirror is a third party that sees
who asked and can be down. Mitigated as before by the signature check, and by
Sober remaining an explicit, one-click import for anyone who prefers Google
Play's route.

**A Sober user's updates change hands.** They used to move when Sober's app
updated. They now move when Cordial's Updates run, which under ADR-015 does not
download unasked on a metered connection (the default for most desktops).
Roblox refusing the old build then looks, to them, like Cordial breaking. The
release notes must say this under "what will still bite", and the first launch
after upgrade should offer Update once.

**Disk.** Measured here: one entry is 392 MB (base.apk 229 MB, split 54 MB,
engine 118 MB; `du`, hard links counted once) plus a 102 MB assets tree today,
so about 0.5 GB per build, moving the assets under the entry costs one more
tree per kept version. Newest + 1 spare + N pins is the bound, and the store
view shows the number. The engine (118 MB) is derivable from the archive in
about 0.6 s (the figure in `install.rs`), so non-current entries could keep
only archives; left out, because it is a second state an entry can be in.

**Flatpak.** The store is `$XDG_DATA_HOME/cordial/builds`, i.e.
`~/.var/app/io.github.luohoa97.Cordial/data/cordial/builds`, separate from a
host install's (ADR-037); nothing is shared and a user with both pays twice. The
read-only grant `~/.var/app/org.vinegarhq.Sober/data/sober/packages:ro`
(`packaging/io.github.luohoa97.Cordial.yml:159`) must stay: it is now what the
import reads, and nothing else.

**Cache semantics are wrong for a pin.** `~/.cache` is what cleaners delete,
and an old pinned build may not be re-obtainable (the mirror keeps old
versions today; nobody promises it will). The store moves to
`$XDG_DATA_HOME` for that reason (decision 2 below).

**Amended.** ADR-033: "follow the current build" now means the newest store
entry and not the slot; `KEEP = 3` is replaced by `gc_plan`; the single-slot
link is kept for `justfile` and hand-typed `--lib-dir` but is no longer read by
a launch. ADR-037: `find_by_content_hash` is wired, and `.signer` is per entry.
ADR-025: the free-source-first ordering and "both routes are on the first-run
screen" no longer hold. ADR-013's per-profile table gains
`last-roblox-version`.

## Decided when accepted (2026-10-04)

1. **`spare = 1`.** The build before the newest stays, for the reason ADR-033
   gives: an update that regresses should be two clicks from undone.
2. **The store moves to `$XDG_DATA_HOME/cordial/builds`.** A pinned build may
   not be obtainable again, and `~/.cache` is what cleaners delete. The
   migration moves an existing cache store once, with a rename where the two
   are on one filesystem and a copy otherwise. In the Flatpak that is
   `~/.var/app/io.github.luohoa97.Cordial/data/cordial/builds`.
3. **The Settings APK path is imported and cleared,** and every profile with no
   pin is pinned to it, so nobody is moved off a file they chose.
4. **A Sober user's first launch after the upgrade asks once** whether to
   download the newest build. Updates stay never-unasked otherwise (ADR-015).
5. **A same-version build with a different engine hash is refused** by name.
6. `shell.json`'s `lib_dir` is gone (4027469); nothing here reads it.

## As built, 2026-10-04

Implemented in ten steps as ordered in the plan; what differs from the text
above, what was measured, and what is still not known.

**Measured.**

- *Two versions at once.* Two signed-out clients on 2.736.0.1408 and
  2.738.0.1397 ran together in a headless compositor with input flowing:
  presents read 776 and 774, then 918 and 915 about five seconds later; each
  extracted its assets into its own `builds/<version>/assets/` (1,836 and 594
  files) and both `.in-use` locks were refused to an exclusive request. The
  control, the same two with a `--lib-dir` outside the store, used one shared
  tree and the second client re-extracted over it while the first ran. Sequential,
  one human session on the machine, not a frame rate.
- *Migration.* Run against throwaway data directories: a Sober copy
  (`source=sober`, Sober's files byte-identical by `stat` before and after), a
  cache store in the maintainer's own shape (moved, verified once at launch,
  launched from the store to the sign-in page), and a Settings APK (imported,
  two unpinned profiles pinned, an existing pin kept, the setting cleared).
- *Disk.* An entry measured 348 to 350 MB with extracted assets, against the
  0.5 GB estimated above.

**Corrected.** Context 3 measured `lib/x86_64` as a real directory naming
Sober's `base.apk` with `builds/` empty. On the host install, measured again on
2026-10-04, the slot is a link into `~/.cache/cordial/builds/2.738.0.1397`,
which holds that entry with a signer record in the slot's format. The migration
handles both shapes; the earlier measurement described a machine that has
since pressed Download.

**Differs from the text.**

- **`.signer` records the archive by size and mtime, not by path.** The slot's
  stamp includes the path, and the store moves, so a path-bearing record would
  have unverified every entry on the move. A record in the slot's format reads
  as unchecked and is replaced by one verification. A copy loses mtime, so a
  store moved by copy (not rename) is verified again once.
- **Sober's directory alone files nothing.** The migration copies Sober's build
  only when the old launch was running it, which always left a real directory in
  the slot. A machine with Sober and no slot, or a wiped cache, gets the
  first-run screen with both buttons, as the table above intends.
- **Entries from before the records are verified at their first launch**, not
  filed unsigned: `Latest` ignores an unsigned entry, so the shell checks each
  once before resolving.
- **A recorded signer must still be trusted.** `Latest` skips an entry whose
  certificate is no longer pinned and a pin to one is refused naming the
  fingerprint.
- **The Quest store takes no `.in-use` lock and collects with a spare of two**,
  which is the bound it had; it stays out of the rest of this ADR.
- **"From the store view" is a Remove row, not a collection on opening the
  page**, which would delete builds by looking at them.
- **The mirror's `versionCode` is recorded** in `.version-code` when the
  download supplies one, for display only.

**Not done.** The profile row does not say "N releases behind the newest build
on offer": that needs the mirror's list cached somewhere, and the Version page
fetches it fresh. `CORDIAL_APK_DIR`, which was undocumented, is no longer read.

**Still INFERRED.**

- That a hard link from Sober's directory fails across the Flatpak's read-only
  grant (the import copies and nothing depends on it).
- What the engine shows when Roblox refuses an old client, and so the 60 second
  window and wording of the crash-page hint for a pinned profile.
- That an older build may not understand what a newer one saved; the Version
  page says it has not measured.
- That the Flatpak's file portal hands over only the chosen file, so a split
  APK's other half cannot be seen when imported that way.
- The Flatpak itself was not run: every check above used a host build.
