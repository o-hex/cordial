# ADR-025: Cordial may fetch the build from a third-party mirror, if it can prove Roblox signed it

**Status:** accepted
**Extends:** [ADR-015](ADR-015-cordial-may-fetch-the-roblox-build.md)
**Related:** [ADR-012](ADR-012-profiles-and-instances.md), [ADR-016](ADR-016-per-profile-network-egress.md)

## Decision

Cordial may download the official Roblox Android build **from a distributor that
is not Roblox**, to the user's own machine, at the user's request.

**It may only install one it can prove Roblox signed.** The APK's signing block
is verified and the signing certificate is checked against a pinned set before
anything is extracted. A build that fails that check is deleted, not installed,
and not offered again.

Everything ADR-015 forbids still holds: nothing committed, nothing vendored,
nothing bundled in a release, nothing served to a third party, nothing modified
on the way through.

## Why ADR-015 was not enough

ADR-015 permits fetching "from Roblox's own distribution". The investigation
recorded in `crates/cordial-update/src/download.rs` established, on 2026-08-03
and at length, that **there is no such distribution for the Android build**.
Roblox's deployment CDN carries the desktop clients only — 7210 deployment
records with not one occurrence of `android` or `apk`; the Android prefix
answers 403; `client-version` answers 500 for `AndroidApp`; and Roblox's own
download page links no file, only Google Play and the Amazon Appstore.

So ADR-015 granted a permission that could not be exercised, and the install
step it was written to remove is still there. That is the gap this closes.

## Why a mirror is acceptable when signature verification is not optional

**Because the alternative in practice is worse, and it is what people already
do.** Today Cordial tells the user to supply an APK. In the project's own
Discord this week, a newcomer asking how was told to download one from APKPure
by hand — and then, separately, warned that any APK advertising itself as
"modded" is likely malware. That is the status quo: **a human fetching an
unverified archive from the same mirror, with no check on it beyond their own
judgement, and Cordial trusting it because a person chose it.**

Signature verification inverts that. A mirror can serve the wrong bytes; it
cannot forge Roblox's signing key. Once the certificate is pinned, the question
"is this really Roblox's build" stops depending on where it came from — which
means an untrusted source becomes acceptable *and* the APK a user supplies by
hand becomes checkable for the first time.

**So the check is the feature.** Downloading is the convenience it buys.

## What must be true before this ships

These are requirements, not aspirations, and the download path stays off until
each is met:

- **The signing block is verified, not merely present.** Parsing the block and
  reading a certificate out of it proves nothing on its own: an attacker
  supplying the archive also supplies the block. The signature over the signed
  data must be checked with the signer's key, and the content digest recomputed
  over the archive and compared. A verifier that skips either step is a
  decoration.
- **The certificate is pinned by digest**, and the pinned values live in a file
  a person can read and audit rather than a constant nobody looks at.
- **A v1-only archive is refused.** v1 signatures cover entries rather than the
  file, and an archive that offers nothing better is one whose provenance cannot
  be established this way.
- **The base archive and the split must agree**, signed by the same certificate.
- **A failure deletes the download and keeps the working build.** An install
  that half-succeeds is worse than one that does not start.

## What this costs, stated plainly

**A hand-written parser for an undocumented format.** The mirror's API publishes
no schema, so recovering a version and a URL from its response is pattern
matching over bytes. It will break when the mirror changes, and it will break in
a way that looks like the mirror being down.

**A dependency on somebody else's uptime and goodwill.** A single mirror is a
single point of failure, and mirrors go down: the one under consideration
answered 503 for a full day this month. **So the chain must have a real second
source or an honest local path, not a broken fallback that adds a second way to
fail.**

**A posture Cordial did not previously have.** "Cordial downloads Roblox for
you, from a mirror" is a sentence about what this project is, and it is now true.
It should appear in the README in those words rather than being discovered.

## What is rejected

**Google Play with account credentials.** It is how Sober does it, it needs a
Google account and a device identity, and it puts Cordial in the business of
holding or brokering somebody's authentication. The mirror route needs no
credential at all, which is the whole reason it is available.

**Shipping any part of the build.** Unchanged from ADR-015 and not reopened.

**Trusting a locally-supplied APK because a human chose it.** Once the verifier
exists, the file a user points at goes through the same check as a download. It
would be strange to verify the convenient path and not the manual one, and the
manual one is where the "modded APK" advice actually lands.

## Consequences

**The install instruction changes** from "obtain an APK" to "press the button".

This originally went on to say the Sober route becomes "one option among
several rather than the recommended route", and that is **not** what was built,
so it is corrected here rather than left to describe something else. Both routes
are on the first-run screen with the difference stated: the mirror is a third
party that sees who asked and can be down, and Sober going through Google Play
on the user's own account is a different trade and a reasonable one to prefer.
Offering one and hiding the other would be deciding that on the user's behalf.

**Cordial acquires a security-critical component.** A signature verifier is code
where a subtle mistake is invisible until it matters. It needs tests with known
answers, including negative ones — a tampered archive, a wrong certificate, a
v1-only archive, a truncated block — and those tests are the deliverable as much
as the verifier is.

**The pinned set needs maintenance.** If Roblox rotates its signing certificate,
every download fails closed until the new digest is added. That is the correct
direction to fail, and it needs to be a one-line change somebody can make
quickly, with an obvious error message saying what happened.

## What was built, and what it measured

Implemented on 2026-08-26 in `crates/cordial-update/{apk_signature,url_policy,provider}`,
clean-room from `docs/design/fetching-the-roblox-build.md`. Every protocol fact
in that spec was re-checked against the live service before being coded against,
and all of them held.

**The condition this ADR turns on is satisfied, and here is the measurement that
says so.** APKPure served version 2.734.917 as a single 229 140 095-byte APK.
The archive Sober downloaded from Google Play for the same build is a different
file — 97 MB of assets plus a 53 MB x86-64 split, 150 MB against 229 MB, because
the mirror's copy carries every ABI. Both verify against the same Roblox signing
certificate:

```text
44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477
```

Two distribution routes that share nothing, one signing key, and one verifier
saying so about both. That is the strongest statement available about whether a
mirror is serving Roblox's build, and it is the measurement to repeat whenever
anyone asks whether this source is safe.

**Two sources ship, not one, and the free one is tried first.** A build already
on the machine costs no request, no disclosure and no bytes on a metered
connection, and most people who will run Cordial already have this exact file
because the README told them to install Sober. Reaching a mirror for a file that
is already present would be the worst version of this feature.

**The pinned set holds one certificate, not the two the design spec names.** The
second was not observed on this machine, and a digest in that file is a key
Cordial accepts builds from forever. Taking one from a report rather than from
an archive somebody verified is the wrong way to spend that, so it is not there.
Adding it is a one-line change for whoever does observe it.

### One thing the ADR did not anticipate

The design review found that the verifier as first written had **no downgrade
protection**. Roblox signs with both v2 and v3, and an archive verifies under
either, so an attacker who wants the weaker scheme deletes the v3 block rather
than defeating the preference for it. Measured: strip the v3 pair from the
shipping build, fix the EOCD offset, and the result still verifies under v2 and
still reports Roblox's genuine certificate — because the content digest
substitutes the signing block's offset for the central directory's, which is
what makes a signature survive the block being inserted and equally makes it
blind to the block being resized.

What closes it is the `0xbeeff00d` attribute inside the v2 signed data, which
says the signer also applied scheme 3 and cannot be edited without breaking the
v2 signature. It was being parsed past and ignored. **A verifier for this ADR is
not finished when it accepts genuine archives**; that was true of the first
version and it was not secure.

## What would change this

A Roblox-operated source for the Android build appearing would make the mirror
unnecessary, and ADR-015's original wording would be enough again. If the mirror
route becomes unreliable enough that most attempts fail, an honest "supply your
own, and Cordial will verify it" is a better product than a download that
usually does not work.
