# Updating the Roblox build

**Status:** specified, not implemented. No code exists for any of this.

Cordial ships no Roblox build and never will, so today the APK arrives because
somebody installed Sober and let it download one. That works and it is a poor
dependency: the install instructions currently tell people to install a
different Roblox client first, in order to obtain a file.

This is the design for fetching it directly. **Whether Cordial should fetch it
at all is a decision, not an implementation detail**, and it wants an ADR before
any of this is written — see the last section.

## The control, in one button

A single button in the shell's header bar, immediately left of Settings. It is
**always present**. Its icon says which of three states it is in.

| state | icon | what clicking it does |
|---|---|---|
| up to date | download | the current version, its changelog, and that there is nothing newer |
| update available | download, with attention styling | the new version's changelog, and an **Update** button |
| checking disabled | refresh (circular arrow) | checks once, now; if that finds an update the button becomes the row above |

**Always present, rather than appearing when an update exists.** A control that
comes and goes is hard to find at the moment you want it, and it moves
everything beside it when it arrives. The cost of keeping it is one icon; the
cost of hiding it is that nobody can answer "what version am I on" without
finding a menu. Showing the current version and its changelog is useful on its
own, which is why the up-to-date state is not a dead end.

The disabled state deliberately still offers a manual check. Turning off
automatic checking is a statement about background network use, not a refusal to
ever know.

## Settings

**Automatic updates** — one dropdown:

- *Download in the background* — fetch when one appears, subject to the network
  setting below.
- *Ask first* — check, then raise the button's attention state and wait.
- *Never check* — no background request of any kind. The header-bar button
  becomes the manual refresh described above.

**Download over** — one dropdown:

- *Any connection*
- *Unmetered connections only* (default)

Two dropdowns rather than a mode plus two independent toggles. Three controls
that can each be set independently produce combinations with no defined meaning
— "update in the background, but never download" is a setting that has to either
lie or explain itself, and a settings page that can express a contradiction will
eventually be asked to honour one.

## Metered connections

`org.freedesktop.NetworkManager`'s `Metered` property, over D-Bus. Same
mechanism as GameMode and the Secret Service, so no new dependency.

**It has four values, not two**: yes, no, guess-yes, guess-no. A phone hotspot
commonly reports *guess-yes*. Both guesses must be treated as metered — reading
a guess as "not metered" is how somebody's data allowance pays for a 115 MB
download they never asked for. Unknown is also metered, for the same reason.

## Checking is cheap; downloading is not

Check on launch, in the background, after the window is up. One request against
a version endpoint costs nothing, and Sober doing it every launch is not the
waste it appears to be — the mistake would be doing it *synchronously*, where a
slow or absent network delays the window.

The download is the expensive half and the only part the settings above govern.

## Verifying what arrives

Whatever is downloaded is verified before it is used, for the same reason the
plugin registry hashes archives: an unverified download is a URL you are
trusting, and a URL is not a claim about content. See
[ADR-014](../adr/ADR-014-plugin-registry-and-unpacking.md), whose extraction
rules apply here in full — the APK is a zip and every refusal in that list is
about zips.

The extracted engine cache in `~/.cache/cordial/lib/x86_64` is stamped with the
APK it came from, so a new build re-extracts and an unchanged one does not.
Presence alone used to be the whole test, which meant a new Roblox build left
the old engine in place and Cordial ran it against the new APK's assets.

## The decision this needs first

The README says, and it is load-bearing: *Cordial ships no Roblox code, ever. No
APK, asset, or decompiled material committed, vendored, or pasted.*

Fetching at the user's request does not violate the letter of that. Nothing is
committed, nothing is vendored, nothing is redistributed, and the file comes
from Roblox's own servers to the user's own machine. Sober does exactly this in
the open.

But it changes what Cordial *is* — from "bring your own build" to "a thing that
fetches Roblox binaries" — and this project has no arrangement with Roblox and
has been careful not to imply one. That is a posture decision and it belongs in
an ADR that says plainly what this does and does not do: fetches at the user's
request, verifies what it got, never modifies it, never serves it to anyone
else. So the answer exists before the question is asked.
