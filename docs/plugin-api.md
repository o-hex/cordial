# The Cordial plugin API
<!-- description: Reference for what a Cordial plugin can ask for, and the protocol it speaks. -->
<!-- icon: book -->

A plugin is a directory holding a `plugin.json` and, usually, one TypeScript module. Cordial runs that module as a separate Deno process with **no permissions at all** and talks to it in newline-delimited JSON over stdin and stdout. Everything a plugin can reach arrives down that pipe, and every request is checked against the capabilities the user granted it, in this profile, first.

A plugin can:

- read and contribute FastFlags and Cordial's own render settings;
- keep a settings document and have Cordial draw it a preferences page;
- read which experience, server and user the client is on;
- publish Discord Rich Presence, post a desktop notification, and open an `http` or `https` page;
- talk to other plugins over a small event bus;
- shadow Roblox's own textures, sounds, fonts and models with files of its own.

It cannot run code inside the Roblox process, read the DataModel or the Lua state, draw on screen, open a socket or a file, or widen Cordial's own sandbox. Those are absent from the surface rather than switched off ([ADR-001](adr/ADR-001-in-process-hooking.md), [ADR-003](adr/ADR-003-plugin-isolation.md)). See [what you cannot do](plugin-api/limits.md).

> [!WARNING]
> Two things look implemented and are not. **`flags.setDynamic` has no effect and never will.** And **`cordial/client.ready` and `cordial/window.resized` are published by nothing**, so a plugin that waits for either waits forever. The other core events do arrive.

<!-- cards -->

- [Getting started](plugin-api/getting-started.md): a first plugin, `plugin.json`, testing without packaging.
- [Protocol and sandbox](plugin-api/protocol.md): requests, responses, pushes, what Deno permits, how a plugin starts.
- [Capabilities and grants](plugin-api/capabilities.md): the fifteen capabilities, default deny, refusals, per-profile grants.
- [Events](plugin-api/events.md): core events and plugin-declared events.
- [FastFlags](plugin-api/flags.md): layers, `flags.list`, `flags.get`, `flags.set`.
- [The rest of the surface](plugin-api/surface.md): notifications, presence, URLs, session state, settings, preferences, asset overlays.
- [What you cannot do](plugin-api/limits.md): the walls, and the decision behind each.

<!-- /cards -->

New to plugins? Start with [plugin development](plugin-development.md). `plugins/README.md` covers versions, dependencies and publishing.

## How this reference was checked

Signatures, parameter names, refusal messages and limits were read out of the handlers that implement them. The text was first written at `v0.10.0-8-g767ce98-dirty` and later corrected where a run or the source disagreed. Apart from the measurements quoted where they arise, it was not observed by running a client, so anything inferred from a code path rather than stated by the code is labelled **INFERRED**. Where a page contradicts a comment in the source, it says so.
