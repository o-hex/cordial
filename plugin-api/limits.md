---
title: "Plugin API: what you cannot do"
description: "The things a plugin cannot do, and the ADR behind each."
icon: "ban"
---
Each limit is a decision, not a gap. Knowing which one you have hit stops you designing against a wall.

| You cannot | Why | Decision |
|---|---|---|
| Run code inside the Roblox process: no hooking, no memory patching, no injected script environment | Enforcement has to live outside the boundary it enforces, and a capability that revocation cannot revoke is a promise, not a guarantee. Absent from the surface, not disabled, so there is nothing to re-enable in a fork. | [ADR-001](/adr/ADR-001-in-process-hooking) |
| Read or write Cordial's memory, or use a shared-memory transport | Capabilities mean something only if holding fewer lets you do less, and an address-space boundary does not depend on Cordial being correct. A call that is too slow across the pipe needs a better shape, not a shortcut past the broker. | [ADR-003](/adr/ADR-003-plugin-isolation) |
| Draw on top of the game, or in Cordial's own window | A third party hooking the presentation path is what Cordial refuses to do, and shipping a supported way for someone else to do it would be the same refusal in name only. Capture works today and needs nothing from Cordial. | [ADR-009](/adr/ADR-009-capture-yes-overlay-injection-no) |
| Read the DataModel, the Lua state or anything else inside the engine | Those live in the engine's address space. Cordial answers the Android platform calls the client makes, and the DataModel is never one of them. | [ADR-001](/adr/ADR-001-in-process-hooking) |
| Touch files, network, environment or subprocesses | A Deno process with no permissions, under the capability broker. A second, independent layer. | [ADR-003](/adr/ADR-003-plugin-isolation), [ADR-018](/adr/ADR-018-plugin-sub-sandboxing) |
| Widen the sandbox from your manifest | A Flatpak permission is app-wide and permanent while a capability is per-plugin and revocable. Open an issue instead; see [capabilities](/plugin-api/capabilities#effects-never-channels). | [ADR-007](/adr/ADR-007-host-resources-are-brokered) |
| Write your own preference values, or read or write another plugin's settings | There is no `preferences.set`, and `settings.*` has no field to name another plugin. Enforced by an absent parameter, not a check. | [ADR-020](/adr/ADR-020-declarative-plugin-preferences) |
| Write into Roblox's own files | Overlays resolve reads and never redirect a write, and nothing is copied into the APK or the extracted asset tree, so uninstalling is a complete undo. | [ADR-010](/adr/ADR-010-plugin-asset-overlays) |

What does exist for the surfaces above: `notify.send`, asset overlays and the declarative preferences page. There is no general UI surface.

## What a plugin can learn about the game

Less than the DataModel and more than nothing. A plugin holding `state.read` can call [`state.get`](/plugin-api/surface#session-state-stateget) for the place, universe, server, user id and join time. A plugin holding `presence.set` can hear [`cordial/game.presence`](/plugin-api/events#core-events). Three more core events arrive under `lifecycle.read`: a profile name, a version string and a shutdown. There is no player list, frame rate or memory figure: those need engine introspection.

## A known method with no effect answers `error`

A method Cordial knows but has not wired an effect for answers `error`, not `denied`, so you are not sent looking for a permission that was never the problem.

| Message | Where |
|---|---|
| `flags.setDynamic is not implemented yet` | The client's plugin host. Permanent: its effect needs a live write into the engine's flag table. |
| `no broker wired for "assets.override"` | The `cordial-plugins` crate's test-only `Session`. |
