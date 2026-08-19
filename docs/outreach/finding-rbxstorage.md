# Finding: RbxStorage is reachable, has run once, and its log channel is silent

Draft in the shape `.github/ISSUE_TEMPLATE/finding.md` asks for. Not filed.

## What was established

**The content store works.** A data root on this machine holds a real
`rbx-storage.db` — one `files` table, nine rows, real content blobs (786, 787,
1673, 1786, 2382, 259040, 8 bytes) across categories 1, 10 and 11, several
carrying `RBXH` magic — beside eight engine-created partition directories
(`p14 p15 p16 p19 p20 p30 p36 p5`). Sober's working store has the same shape.

So `RbxStorage` is not unreachable through the host-application interface. It has
been reached, by Cordial, on ordinary hardware.

## What was disproved, and it is most of the prior record

**The `DFLog::RbxStorage` channel is silent even when storage succeeds.** The
engine log from the run that created that store contains **zero** `RbxStorage`
lines.

Every conclusion of the form *"storage never runs, because no `RbxStorage::init`
line appears"* was therefore reasoning from an absence on a channel that says
nothing either way. That covers the framing of `flag-init.md` §§12–24 and the
negatives in §29–§35, and it voids every `grep -c 'RbxStorage::init'` score in
that file.

**The only instrument that works is the filesystem.** Score reproductions on
`rbx-storage.db` and on rows inside it, never on the log.

## What is still not known

What triggers it. Measured on the filesystem, on fresh roots, all negative:

| variable | result |
|---|---|
| late-post delay 250 ms vs 2000 ms | no store, either |
| second warm pass | no store |
| `FFlagStartRbxStorageInitRighAfterFlags` + `DFFlagRbxStorageInitLatch` | no store, three passes |
| sustained content activity | `CordialTest` has **91** `ContentProvider_*` dirs and no store |

The root that worked differs only in having far more history — around twenty runs
under a dozen environment combinations, several of which crashed part-way.

## Why this is worth someone's time

No content store means every asset is fetched from the network every session,
which is the "sometimes slow and complete, sometimes fast and untextured" loading
users report. The store that did appear cached nine assets immediately.

## How to continue

Start at `flag-init.md` §36. Score on disk. A real reproduction shows **rows in
the `files` table**, not merely directories — the schema is
`files(id BLOB PRIMARY KEY, content BLOB, size, hits, atime, category, score)`
with indices on atime, size, category and score, an eviction cache.

## The methodological finding, which may outlast the bug

Nine instrument faults are recorded in `flag-init.md`. **Eight were an absence
read as evidence** — a path trace that did not wrap `statvfs`, a channel sweep
using the wrong value shape, a function boundary from a prologue scan, two `lldb`
attaches arriving too late, a halting harness untrustworthy for timing, two
output streams with different buffering read as one timeline, and a log channel
quiet on success.

> In this codebase, when a measurement says nothing happened, suspect the
> measurement first. It has been right eight times out of nine.
