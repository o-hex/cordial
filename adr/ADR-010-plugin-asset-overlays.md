---
title: "ADR-010: Plugins may overlay Roblox's assets, non-destructively"
---
**Status:** accepted
**Supersedes:** [ADR-004](/adr/ADR-004-plugin-asset-overrides)
**Related:** [ADR-001](/adr/ADR-001-in-process-hooking), [ADR-003](/adr/ADR-003-plugin-isolation), [ADR-009](/adr/ADR-009-capture-yes-overlay-injection-no)

## Decision

Plugins, and the user directly, may provide files that Cordial serves in place
of the matching path under the APK's `assets/` tree. Resolution happens before
every lookup Roblox makes through `AAssetManager`: an overlay is consulted
first, and only a miss falls through to the APK. Nothing is ever written into
the APK or into Cordial's own extracted asset tree to make this work — see
`crates/cordial-runtime/src/android/asset.rs` for the mechanism.

This reverses [ADR-004](/adr/ADR-004-plugin-asset-overrides), which refused
plugin asset overrides outright. Two of ADR-004's three supporting claims were
checked against primary sources for this decision and did not hold up.

## Why ADR-004 was wrong

**Claim 1 — "a transparent texture is a wallhack" — wrong.** This was imported
from games where world geometry is textured brushes: swap the texture,
see through the wall. Roblox parts are geometry with a `BasePart` colour and
material; the surface Roblox renders is the part's own colour modulated by a
material, and a texture asset changes shading, not occlusion. Substituting a
transparent or high-contrast material texture yields a differently-shaded
surface, not a see-through one. The ADR-004 argument treated the mechanism as
inherently the exploit — "an interface that can substitute rendered content can
substitute it advantageously" — but that reasoning assumed a rendering pipeline
Roblox does not have.

**Claim 2 — "it breaks the claim that keeps Sober viable" — wrong.** Sober is
the same architecture Cordial is: Roblox's official Android build, run
unmodified, on Linux. Sober ships an `asset_overlay` directory —
`~/.var/app/org.vinegarhq.Sober/data/sober/asset_overlay`, mirroring
`base.apk/assets` exactly — and remains a viable, widely-used project. Bloxstrap
does the equivalent on Windows through a `Modifications/` folder, and Roblox
staff have stated publicly that using it is not bannable. Neither project's
"runs the official build" claim was broken by shipping this. The actual line
both projects observe, and the one that matters, is **not touching the client
process** — Cordial's own [ADR-001](/adr/ADR-001-in-process-hooking) line. An
asset file resolved before the engine reads it never executes; it sits on the
same side of that line as a config file.

**Claim 3 — the middle ground does not exist — still true, and is why this ADR
is narrower than "override anything".** Roblox's built-in `content/textures/`
really are used inside experiences, and there is no subtree of the namespace
that is reliably confined to client chrome. This ADR does not attempt to draw
that line. It permits overlaying the whole `assets/` tree — the same scope
Sober's `asset_overlay` covers — rather than inventing a "safe subset" that
ADR-004 correctly identified as fictional.

## What is still refused

**Gameplay-affecting substitution is a real risk, and this ADR does not solve
it.** Replacing a collision or hitbox mesh with a smaller or absent one is a
substantive advantage, not a cosmetic change, and it is not caught by anything
"non-destructive" implies. This ADR documents that risk as the **user's own
responsibility** — the same posture Sober and Bloxstrap both take — and builds
no special handling for it: no content inspection, no allow-list of "safe"
asset types, no attempt to distinguish a texture overlay from a mesh overlay.
Building that would mean maintaining a classifier for every asset type Roblox
ships, forever, and getting it wrong silently. An honest warning is cheaper and
more durable than a heuristic that is wrong some of the time.

**In-process content injection is still refused**, per
[ADR-001](/adr/ADR-001-in-process-hooking) and
[ADR-009](/adr/ADR-009-capture-yes-overlay-injection-no). This ADR is about which
file `AAssetManager_open` resolves to, decided entirely outside the engine's
address space, before the engine ever asks. It says nothing about drawing over
a composited frame or hooking a presentation call, both of which remain
unavailable for the same reasons ADR-001 gives.

## Mechanism

Overlay roots form a stack: one root per plugin, registered by plugin id, plus
one root owned by the user directly at
`$XDG_CONFIG_HOME/cordial/overlay` (falling back to `$HOME/.config/cordial/overlay`,
overridable with `CORDIAL_OVERLAY`) — the same layout Sober's `asset_overlay`
uses, so a directory built for one is usable, unmodified, for the other.

Resolution order is fixed: the user's root beats every plugin's, and among
plugins the most recently registered wins. This mirrors the precedence rule
[`flags.rs`](https://github.com/luohoa97/cordial/blob/main/crates/cordial-runtime/src/flags.rs) already uses for
FastFlags, for the same reason — an explicit choice the user made must not be
silently overridden by something they installed to do something else.

Every asset lookup checks the overlay stack first and the APK only on a miss.
A hit records which layer supplied it (`android::asset::explain`), so "why did
this file change" has an answer. Path resolution rejects any name that would
resolve outside the owning root — `..`, an absolute path, or a symlink that
escapes it — with the same rigour `extract_to`'s existing zip-slip defence
uses against a hostile zip entry.

## Consequences

**Accepted:** plugins and users can now do what ADR-004 refused — replace a
texture, a sound, a font, anything under `assets/` — without Cordial copying,
patching, or otherwise touching Roblox's own files. Uninstalling a plugin (or
deleting its overlay root) makes the original resolve again with no cleanup
step, because nothing was ever overwritten to begin with.

**Accepted, and worth stating plainly:** this makes gameplay-affecting
substitution possible, same as it is on Sober and Bloxstrap. Cordial documents
it and does not build detection for it. See "What is still refused" above.

**A real limitation, not a silent gap:** `extract_to` — used for the CA bundle
and for anything the engine opens by a real filesystem path rather than through
`AAssetManager` — is **not** overlay-aware in this change. It continues to
extract only the APK's own files, unmodified, and skips a destination that
already exists so repeat launches stay cheap. Materialising overlay content
into that same tree would require a way to tell "this file is the original,
skipped because it's already there" from "this file is an overlay copy, and
restoring it means deleting it so the original re-extracts" — which needs a
manifest of what was overlaid, not the presence check `extract_to` uses today.
Building that manifest without risking a corrupted or half-restored tree was
judged not worth doing in this change; the honest scope of this ADR is the
`AAssetManager` path, which is what covers the textures, sounds, models and
fonts people actually ask to override. Extending `extract_to` is future work,
tracked by this paragraph rather than a silent limitation someone has to
rediscover.

## What would change this again

Evidence that gameplay-affecting overlay substitution is being used at scale
for cheating, in a way that changes the calculus Sober and Bloxstrap have
already made, would be grounds to revisit the "no special handling" stance
above. Nothing else is expected to.

## Notes moved from docs/plugin-api.md (2026-10-02)

### Writes, and gameplay-affecting substitution

**Cannot, ever, by design:** writes. A write is never redirected. ADR-010's whole
claim is that nothing is written into the APK or into anything extracted from it,
and handing the engine a writable descriptor onto a plugin's file would make the
overlay a place the engine can scribble — neither non-destructive nor anything the
plugin's author agreed to. Reads resolve to the overlay; writes go to the
original.

**Cannot:** anything outside the asset tree. The overlay is not a general
filesystem redirect, which is the same line ADR-007 draws between an effect and a
channel.

### What Cordial does not do about it

**Gameplay-affecting substitution is possible and Cordial builds no detection for
it.** Replacing a collision or hitbox mesh with a smaller or absent one is a
substantive advantage rather than a cosmetic change, and nothing "non-destructive"
implies catches it. ADR-010 documents this as the user's own responsibility — the
same posture Sober and Bloxstrap both take — and builds no content inspection, no
allow-list of "safe" asset types, and no attempt to tell a texture from a mesh.
Maintaining a classifier for every asset type Roblox ships, forever, and getting
it wrong silently, is worse than an honest warning. The capability's own consent
text says so to the user in as many words.

### Why interception rather than a mount

The assets are zip entries inside
`base.apk`, so overlayfs has nothing to overlay without extracting the whole
archive first, and Flatpak cannot mount overlayfs unprivileged in any case.
Interception also yields diagnostics a mount cannot: `--check-overlays` and the
shadow report that names which layer won a contested file.
