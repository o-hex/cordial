---
title: "ADR-004: Plugins do not override Roblox's assets"
---
**Status:** superseded by [ADR-010](/adr/ADR-010-plugin-asset-overlays)
**Supersedes:** nothing
**Related:** [ADR-001](/adr/ADR-001-in-process-hooking), [ADR-003](/adr/ADR-003-plugin-isolation)

> Two of this ADR's three supporting claims were checked against primary
> sources — Sober's `asset_overlay` directory and Bloxstrap's `Modifications`
> folder — and did not hold up. [ADR-010](/adr/ADR-010-plugin-asset-overlays)
> reverses the decision below and explains why. The text is kept as written,
> for the record, rather than edited to look like it was right all along.

## Decision

Plugins cannot replace Roblox's asset files — textures, sounds, models, fonts or
anything else in Roblox's content namespace. There is no API for it and the
extracted Roblox asset tree is not reachable from any capability a plugin can be
granted.

Plugins **can** theme Cordial's own surface: the launcher, the shell UI, its
icons, and any sound Cordial itself plays. That is Cordial's namespace and
plugins own it.

## Why

**Asset override is the mechanism, not a slippery slope.** Replacing a wall
texture with a transparent one is a wallhack. Replacing character or material
textures with high-contrast ones is ESP. Replacing a sound with a louder cue is
audio ESP. These are not risks that an asset-override API might eventually be
abused into — they are what the API *is*, expressed differently. An interface
that can substitute rendered content can substitute it advantageously.

**It is ADR-001's principle applied to content instead of code.** ADR-001 refuses
in-process execution on the grounds that the protection is the *absence of the
primitive*: a restriction can be lifted in a fork, a capability that was never
built cannot be extracted from a binary that does not contain it. An asset
override surface is the same primitive wearing different clothes, and the same
reasoning disposes of it.

**It breaks the claim the project's safety argument rests on.** Cordial's README
states that it runs the official Android build *unmodified*, and that is the
distinction between Cordial and the things Roblox bans — the same distinction
that has kept Sober viable. Substituting the client's content is modifying the
client. Whatever the intent, a heuristic detector sees changed assets, and the
honest version of the README would have to stop making that claim.

**The tempting middle ground does not exist.** "Only cosmetic assets" and "only
assets shipped in the APK" both sound safe and neither is. Roblox's built-in
`content/textures/` are used *inside experiences* — default materials and the
terrain atlas among them, visible in the engine's own log as
`Terrain TextureAtlas 2048x2048`. There is no subtree of Roblox's namespace that
is reliably confined to the client's own chrome, so any rule of the form "these
assets are fine" is a rule that will be wrong for some experience.

## Consequences

**Accepted:** custom textures, sound packs and model replacements are not a
Cordial feature, and users who want them will not get them here. This is the
same trade ADR-001 already made for client-UI modification, and it is made for
the same reason.

**Accepted:** theming Roblox's own interface is limited to what Roblox itself
exposes. That is not nothing — `selectedTheme` is already part of
`StartAppParams`, and theme FastFlags are reachable through the layered flag
system in [`crates/cordial-runtime/src/flags.rs`](https://github.com/luohoa97/cordial/blob/main/crates/cordial-runtime/src/flags.rs),
where a plugin can contribute flags without writing to the user's file.

**Accepted:** this is a policy decision, not a technical limitation, and it has
to be enforced structurally to mean anything. Cordial serves assets through
`AAssetManager` from the extracted APK, so an override layer is a lookup
interposition of perhaps fifty lines. Anyone can write it. The decision is
therefore expressed as *what Cordial ships*, and enforced by there being no
plugin-facing path to that lookup — not by a documented request that plugins
behave.

**Rejected alternative:** a namespaced override where plugins may only write into
an asset root Cordial owns, with the Roblox tree structurally unreachable. This
is defensible and would be the shape to build if the decision were ever revised.
It is not adopted now because it is a different feature from what people ask for
when they ask for custom textures, and shipping it under that name would
mislead.

## What would change this

Evidence that Roblox itself sanctions client-side asset replacement — a
documented, supported mechanism for it — would make this a compatibility
question rather than a safety one, and it should be reopened on that basis.

Nothing about user demand changes it. The demand is real and is not the
consideration.
