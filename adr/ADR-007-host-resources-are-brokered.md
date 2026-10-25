---
title: "ADR-007: Plugins never hold host permissions; Cordial brokers them"
---
**Status:** accepted
**Related:** [ADR-002](/adr/ADR-002-core-shell-and-ui-handoff), [ADR-003](/adr/ADR-003-plugin-isolation), [ADR-006](/adr/ADR-006-plugin-events-and-first-party)

## Decision

A plugin can never cause a Flatpak permission to be granted, and never receives a
handle to a host resource — no socket, no D-Bus connection, no file descriptor.

Where a capability needs one, **Cordial holds the permission and performs the
effect**. The plugin sends a payload describing what it wants; Cordial does it.

Discord Rich Presence is the worked example. The capability is `presence.set`
and its payload is a presence structure. Cordial owns the connection to Discord's
IPC socket. The plugin never learns where that socket is, never opens it, and
cannot send anything down it except a presence update.

## Why

**Because Flatpak permissions and plugin capabilities have incompatible
lifetimes.** A Flatpak permission is static, app-wide, and granted at install
time. A plugin capability is dynamic, per-plugin, and granted by the user
whenever they approve a plugin. There is no mechanism by which the second can
produce the first, and inventing one would be worse than not having it.

**Because "the manifest declares what plugins need" makes the sandbox as weak as
the most demanding plugin anyone might install.** If installing a Discord plugin
means adding socket access to Cordial's manifest, then every plugin — and Cordial
itself — gains that access, permanently, for as long as the manifest says so.
Uninstalling the plugin does not take it away. The user approved a plugin and
silently widened the sandbox of the whole application.

**Because the Deno host already makes it impossible, and the architecture should
agree with itself.** Plugins run with no Deno permissions at all: no file, no
network, no environment, no subprocess. A plugin *cannot* open a socket even if
Cordial wanted it to. Brokering is not an extra restriction bolted on; it is the
only thing that works given the containment that already exists.

**Because the resource is fiddly and should be solved once.** Discord's IPC
socket is `$XDG_RUNTIME_DIR/discord-ipc-0`, except when it is `-1` through `-9`,
and except when Discord is itself a Flatpak and it lives under
`app/com.discordapp.Discord/`. Every plugin that wanted presence would
reimplement that search, and they would each get it subtly wrong. One
implementation in Cordial is both safer and better.

## Consequences

**Accepted:** the set of host resources Cordial can broker is fixed when the
Flatpak manifest is written, not when a plugin is installed. A plugin wanting a
resource Cordial does not already broker needs a change to Cordial, reviewed and
released. That is slower, and it is the point — the manifest is the one place a
user or packager can read the whole sandbox, and it stays true.

**Accepted:** a brokered capability is narrower than the resource behind it.
`presence.set` cannot read Discord's state, cannot enumerate other applications'
presence, and cannot send arbitrary IPC frames — because the broker exposes a
presence update and nothing else. That asymmetry is deliberate and should be
preserved for every future broker: expose the *effect*, never the *channel*.

**Accepted:** Cordial's manifest will grow specific, narrow entries rather than
broad ones. `--filesystem=xdg-run/discord-ipc-0` is acceptable; `--filesystem=host`
is not, and neither is `--talk-name=org.freedesktop.Flatpak`, which is arbitrary
host command execution and is already refused in
[ADR-002](/adr/ADR-002-core-shell-and-ui-handoff) §2.

**Accepted:** presence is privacy-relevant. What game a user is in, and when, is
information they may not want broadcast, so `presence.set` is off unless granted
like any other capability — and the UI should say what it publishes, not merely
that it is enabled.

**Rejected: per-plugin Flatpak sub-sandboxes.** Flatpak's model does not offer
per-plugin permission granularity, and building one would mean nesting sandboxes
and brokering portal calls between them. The process isolation plugins already
have provides the containment; the missing piece was only ever *who holds the
host resource*, and this answers it without a second sandbox layer.

**Rejected: a generic `host.socket.connect` capability.** It would be simpler to
implement and it is the whole decision undone — a plugin that can open a socket
of its choosing has the channel, and every narrow guarantee above evaporates.

## What would change this

If Flatpak grows real per-child permission scoping, per-plugin sandboxes become
worth revisiting for resources too broad to broker usefully. Nothing currently
proposed needs it.

## Notes moved from docs/rich-presence.md (2026-10-02)

**The Discord broker, as the first plugin to use it.** The plugin never learns
where Discord's socket is. It sends a payload (an application id, `details`,
`state`, timestamps and image keys) and Cordial does the rest: searching
`discord-ipc-0` through `-9` and the nested path Discord's own Flatpak uses,
performing the handshake and writing the frames. The payload is a closed struct
that refuses any field Discord does not define, so nothing a plugin invents
crosses the wire, and `details` and `state` are refused past Discord's own
128-character limit, so the author hears that from the call rather than from
Discord quietly dropping the whole activity. A plugin cannot read Discord's state
and cannot send anything else down the connection. Buttons and picture URLs are
built or resolved by Cordial, never supplied by the plugin, for the same reason:
a string that could become a link under Cordial's name and icon would be an
arbitrary link published as Cordial.

**Retracted: "nothing reaches Discord in an actual session yet".** An earlier
version of `docs/rich-presence.md` said the client's plugin host answered
`presence.set` with `not implemented yet` and that nothing outside a test pushed
a lifecycle event. That stopped being true when the plugin host gained the
presence handlers (`crates/cordial-runtime/src/plugin_host.rs`) and built-in
plugins began to be started (v0.13.0 release notes), and the page was not updated.
The same page also called the Discord application id a placeholder; the plugin now
carries Cordial's own id and takes an override through its `client_id` preference.
