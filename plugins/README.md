# Plugins

A plugin is a directory containing `plugin.json` and its entry module. Cordial
discovers them under `~/.local/share/cordial/plugins/`.

```json
{
  "id": "flag-inspector",
  "name": "Flag Inspector",
  "entry": "main.ts",
  "capabilities": ["flags.read", "log"]
}
```

`capabilities` is what the plugin **requests**. What it actually gets is what you
approved, in the profile you approved it in —
`~/.local/share/cordial/profiles/<profile>/plugin-grants.json`:

```json
{ "flag-inspector": ["flags.read", "log"] }
```

Default deny — a plugin missing from that file gets nothing, and a capability it
asked for but you did not grant is refused at the point of use, by name, so the
author can tell "not allowed" from "broken".

**Grants are per profile, and that is deliberate.** Approving a plugin in a
profile you made to try something out does not approve it in the profile you
actually play on. The plugin is installed once for the machine; what it is
allowed to do is decided per account. An existing
`~/.config/cordial/plugin-grants.json` from before this changed is moved into the
first profile that looks for one, and every other profile starts at default deny
— see [ADR-013](../docs/adr/ADR-013-per-profile-configuration.md).

## What a plugin cannot do

It runs as a separate Deno process started with **no permissions**: no file,
network, environment or subprocess access. That is a second, independent layer
under Cordial's own capability broker, so a plugin cannot reach the machine even
if the broker had a hole in it.

There is no script execution against Roblox and no memory access. Those are
absent from the API rather than disabled — see
[ADR-001](../docs/adr/ADR-001-in-process-hooking.md) and
[ADR-003](../docs/adr/ADR-003-plugin-isolation.md).

A plugin granted `assets.override` **may** provide files that resolve in place
of Roblox's own for the same name — a non-destructive overlay, never a write
into the APK or anything extracted from it. See
[ADR-010](../docs/adr/ADR-010-plugin-asset-overlays.md) for what that does and
does not permit: it covers cosmetic substitution, and it explicitly does not
protect you from a plugin that overlays a gameplay-affecting asset such as a
collision mesh. That is the same trust decision you already make in approving
the capability at all.

## Host resources are brokered, never handed over

A plugin never receives a socket, a D-Bus connection or a file descriptor, and
installing one can never widen Cordial's Flatpak permissions. Where a capability
needs a host resource, Cordial holds the permission and performs the effect.

Discord Rich Presence is the example: `presence.set` takes a presence payload.
Cordial owns the connection to Discord's IPC socket — your plugin never learns
where it is, cannot read Discord's state, and cannot send anything else down it.

The reasoning is in [ADR-007](../docs/adr/ADR-007-host-resources-are-brokered.md),
and the short version is that a Flatpak permission is app-wide and permanent
while a capability is per-plugin and revocable. If installing a plugin could add
a permission, uninstalling it would not take the permission away.

This also means a resource Cordial does not already broker needs a change to
Cordial rather than to your manifest. Slower on purpose: the manifest is the one
place anyone can read the whole sandbox, and it should stay true.

To keep that rare, the common effects are already brokered — `presence.set`,
`notify.send` and `url.open` (`http`/`https` only). If you need one that is not
here, open an issue: a broker is a payload type and an effect, so adding one is a
small change rather than a redesign. If a proposed broker *cannot* be small, that
usually means the capability is too broad and wants splitting.

## Settings: Cordial keeps them, your plugin never touches the file

A plugin runs with no file access, so it has nowhere of its own to remember
anything. `settings.read` and `settings.write` give it one without giving it a
path: Cordial owns `<profile>/plugins/<your-id>/settings.json` and your plugin
exchanges a JSON document.

The document arrives unasked, in the handshake, so the usual case costs no round
trip:

```ts
// A push has no id; the handshake is the one with this event name.
if (msg.id === undefined && msg.event === "cordial/init") {
  const settings = msg.payload.settings;   // your document, {} if you have none,
                                           // null if you were not granted settings.read
}

await call("settings.set", { settings: { panel: "flags", opened: 4 } });
const mine = await call("settings.get");   // takes no arguments — see below
```

`settings.set` replaces the whole document rather than merging, so you can remove
a key you have stopped using. It must be a JSON object, and it is capped at a
megabyte: this is configuration, not a data store.

**Scoped to your own id, structurally.** Neither call takes a plugin id, because
Cordial already knows which process is on the other end of the pipe. Naming
another plugin in your params is not an error and is not honoured — you get your
own document. There is no field to set to somebody else's name, which is the same
defence `events.declare` uses for namespaces.

They are two capabilities, not one, for the reason `events.declare` and
`events.publish` are two: a plugin that only reads its configuration should not
have to be trusted to rewrite it.

## Plugins may declare their own events

`events.declare` registers an event type under your plugin's own namespace —
you provide a bare name, Cordial prefixes it with your plugin id, so
`flag-manager` declaring `profile-changed` gets `flag-manager/profile-changed`
back, never a bare `profile-changed`. `events.publish` broadcasts on a type
you declared; `events.subscribe` receives events, including ones other
plugins declared.

These are three separate capabilities on purpose (ADR-006). Declaring and
publishing are split because a plugin that could publish on any string it
liked could impersonate another plugin's events, and a subscriber would have
no way to tell — declaring first makes a type's origin a fact the registry
checks, not a claim a plugin makes about itself. Subscribing is broader than
publishing: a plugin that only reacts to something should not have to be
trusted to speak.

Subscribing filters at the point you subscribe, not on every event that
arrives — which means you can only subscribe to a type someone has already
declared. If you depend on another plugin's events, that dependency has to
have started first, the same ordering [ADR-006](../docs/adr/ADR-006-plugin-events-and-first-party.md)
already describes for first-party plugins.

Core event types are never available to publish on — only Cordial can speak
for what Cordial did.

## Flags have two lifetimes

`flags.write` contributes flags that take effect at the **next launch**.
`flags.write.dynamic` changes one **while the client runs**, and only works for
the `DFFlag`/`DFInt`/`DFString` families — the static families are read once at
startup and cannot be changed live at all. They are separate capabilities so that
an API call cannot silently do nothing. See
[ADR-005](../docs/adr/ADR-005-flag-service.md).

## Examples

[`flag-inspector/`](flag-inspector) reports which flag overrides are in effect
and where each came from, then deliberately attempts a write it was not granted
so the refusal is visible.

[`discord-presence/`](discord-presence) is first-party — it ships with
Cordial — and is still an ordinary plugin: same manifest, same grants, same
isolation ([ADR-006](../docs/adr/ADR-006-plugin-events-and-first-party.md)
is explicit that "built in" and "a plugin" are not opposites). It listens for
client lifecycle events and keeps Discord Rich Presence in step with them,
through `presence.set`/`presence.clear` — the two lines in its own source say
plainly that its Discord application id is a placeholder and that the
lifecycle push does not yet carry which game is running, because that needs
work in `cordial-runtime` this plugin does not touch.
