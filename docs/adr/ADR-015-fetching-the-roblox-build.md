# ADR-015: Cordial may fetch the Roblox build, and may never ship one

**Status:** accepted
**Related:** [ADR-014](ADR-014-plugin-registry-and-unpacking.md),
[ADR-010](ADR-010-plugin-asset-overlays.md),
[ADR-001](ADR-001-in-process-hooking.md)
**Implements:** [docs/design/updating-roblox.md](../design/updating-roblox.md)

## Decision

Cordial may **download** the official Roblox Android build, at the user's
request, from Roblox's own distribution, to that user's own machine.

Cordial may not **ship** one, in any sense: nothing committed, nothing vendored,
nothing bundled in a release artefact, nothing served to a third party, and
nothing modified on the way through.

## Why this needs an ADR at all

The README says, and it is load-bearing:

> No Roblox code, ever. No APK, asset, or decompiled material committed,
> vendored, or pasted into an issue.

Fetching does not violate the letter of that. Nothing is committed or vendored;
the bytes travel from Roblox's servers to the user's disk and Cordial keeps no
copy anyone else can reach. But the sentence was written to mean *this project
does not put Roblox's property anywhere*, and a reasonable reader could take
adding a download button as walking it back. So the answer is recorded here
rather than assembled after somebody asks.

## Why fetching is right

**The current arrangement is worse, not purer.** Today the install instructions
tell people to install Sober — a different Roblox client — in order to obtain a
file, and then not use it. That is a dependency on an unrelated project for a
step Cordial could do itself, and it is the weakest part of getting started.

**Nothing about it is unusual.** Sober fetches the same build from the same
place, in the open, and has not been troubled for it. The file is free, public,
unmodified, and useless without a Roblox account.

**A stale client is not a working client.** Roblox refuses old builds
server-side, so "update" is not a convenience feature. A client that cannot
update is a client that stops working on Roblox's schedule, and leaving the user
to notice that themselves — with no error that names the cause — is the failure
mode this project keeps writing down.

## What it does not do

- **Never modifies what it fetched.** Asset overlays are a separate feature with
  their own decision ([ADR-010](ADR-010-plugin-asset-overlays.md)); they are
  non-destructive, off by default, and never write into the APK or anything
  extracted from it.
- **Never redistributes.** No mirror, no cache anyone else can read, no
  re-upload, no torrent, no "here is a copy" in an issue.
- **Never fetches unasked in the sense that matters.** Checking is a version
  query; downloading is governed by settings the user sets, defaulting to
  unmetered connections only.
- **Never pretends to be the official client.** Cordial identifies itself as
  Cordial and reports the platform truthfully (`Linux`, which is the engine's
  own vocabulary).

## Verification is not optional

Whatever arrives is verified before it is used. An unverified download is a URL
being trusted, and a URL is not a claim about content.

[ADR-014](ADR-014-plugin-registry-and-unpacking.md)'s extraction rules apply
here in full and were written for exactly this shape of problem — an APK is a
zip, and every refusal in that list is about zips: parent traversal, absolute
paths, symlinks, hardlinks, device nodes, setuid bits, entry-count and
uncompressed-size caps, and the hash checked *before* anything is written.

## Consequences

**Accepted:** Cordial becomes a thing that fetches Roblox binaries, and that is
a different posture from "bring your own build" even though no line of the
existing rule changes. Anyone assessing this project should be able to read this
file and know precisely where the line is.

**Accepted:** the fetcher is a maintenance surface. Distribution URLs, version
endpoints and channel names are Roblox's to change without notice, and when they
change Cordial stops updating until somebody fixes it. It must fail with a
message naming what it could not reach, rather than appearing to work.

**Accepted, and it is the uncomfortable one:** this makes it easier to run
Roblox in a way Roblox does not support. That was already true of the whole
project; the download button does not change the analysis, and pretending
otherwise by making the file harder to obtain would be security theatre against
a file anyone can download in a browser.

**Rejected:** bundling the APK in a Flatpak. It is not ours to redistribute, it
would make the package enormous and immediately stale, and it converts "the user
obtained Roblox's software" into "we handed them Roblox's software", which is
the distinction this whole ADR turns on.

## What would change this

Roblox asking us not to. There is no arrangement here and no green light, and if
that request came the honest answer is to remove the fetcher and go back to
bring-your-own — which costs a convenience, not the project.
