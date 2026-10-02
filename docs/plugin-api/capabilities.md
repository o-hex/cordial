# Plugin API: capabilities and grants
<!-- description: The fifteen capabilities, default deny, what a refusal looks like, and how grants are stored and changed. -->
<!-- icon: shield-halved -->

Three things must be true before a call does anything, and they are recorded in three places owned by different parties:

| Act | Where | Who |
|---|---|---|
| **Request** | `capabilities` in `plugin.json` | The plugin author |
| **Grant** | `plugin-grants.json` inside the profile | The user |
| **Check** | `cordial_plugins::broker::Broker`, on every call, before any handler runs | Cordial |

If installing a plugin were enough to grant what it asked for, the manifest would be a formality.

## Default deny

A plugin absent from the grants file gets nothing, and a capability it requested but was not granted is refused at the point of use, by name.

| Situation | Result |
|---|---|
| `plugin-grants.json` is missing | Nothing is granted. |
| It is malformed | Nothing is granted, and Cordial says so: `plugin grants: <path> is not usable (<error>); granting nothing`. A typo must not become a privilege escalation. |
| It names an unknown capability, anywhere | **Nothing is granted to anybody.** The first unknown name abandons the whole document: `… (unknown capability "process.spawn" granted to "x"); granting nothing`. |
| `plugin-enabled.json` is malformed | Every plugin is treated as enabled (`plugins: <path> is not usable (<error>); treating every plugin as enabled`), except ids in `SHIPS_DISABLED`, today `fps-flex`, which stay off. The grants file fails closed; the enablement file does not. Why: [ADR-003](../adr/ADR-003-plugin-isolation.md). |

There is no capability meaning "anything" and no grants entry meaning "all". The list is closed ([ADR-003](../adr/ADR-003-plugin-isolation.md)): there is no `process.spawn`, filesystem path or memory access to ask for.

## The fifteen capabilities

The list is closed and lives in `crates/cordial-plugins/src/capability.rs`. The method-to-capability mapping is a closed table in `protocol.rs::required_capability`, so a typo in a method name fails as unknown rather than falling through to a check that happens to pass.

| Capability | What it permits | Methods it gates |
|---|---|---|
| `assets.override` | Register a directory whose files resolve ahead of Roblox's own of the same name | `assets.override` |
| `events.declare` | Register event types under the plugin's own id | `events.declare` |
| `events.publish` | Broadcast on a type this plugin declared | `events.publish` |
| `events.subscribe` | Receive events, including ones other plugins declared | `events.subscribe` |
| `flags.read` | Read the resolved flag set and which layer set each value | `flags.get`, `flags.list` |
| `flags.write` | Contribute flags that take effect at the next launch, including Cordial's own `Cordial*` settings | `flags.set` |
| `flags.write.dynamic` | Change a `DFFlag`/`DFInt`/`DFString` while the client runs | `flags.setDynamic` (**never works**) |
| `lifecycle.read` | Hear the core events under `cordial/` | `lifecycle.subscribe` (an acknowledgement) |
| `log` | Write lines into Cordial's own output | `log.write` |
| `notify.send` | Post a desktop notification through the portal | `notify.send` |
| `presence.set` | Publish and clear Discord Rich Presence; also hear `cordial/game.presence` | `presence.set`, `presence.clear` |
| `settings.read` | Read this plugin's own settings document and the user's answers to its declared preferences | `settings.get`, `preferences.get` |
| `settings.write` | Replace this plugin's own settings document | `settings.set` |
| `state.read` | Read which experience, server and Roblox user id the client is on | `state.get` |
| `url.open` | Open an `http`/`https` address in the browser | `url.open` |

> [!WARNING]
> `flags.write.dynamic` is permanently unimplemented: `flags.setDynamic` always answers `flags.setDynamic is not implemented yet`. A live write would need the in-process access [ADR-001](../adr/ADR-001-in-process-hooking.md) and [ADR-003](../adr/ADR-003-plugin-isolation.md) rule out. Plan around it. `lifecycle.read` is partial: it gates five core events, of which two are published by nothing ([events](events.md#core-events)). Everything else in the table works.

Every capability also has a second-person consequence sentence, which the install dialog shows instead of the wire name. A test asserts that no sentence contains its own dotted name.

### Why the capabilities are split

| Pair | Why |
|---|---|
| `flags.write` and `flags.write.dynamic` | Two lifetimes, not two degrees of one power. Static flags are read once at startup, so `flags.set` takes effect next launch ([ADR-005](../adr/ADR-005-flag-service.md)). |
| `settings.read` and `settings.write` | A plugin that only reads its configuration should not be trusted to rewrite it, and `settings.set` replaces the whole document. |
| `events.declare` and `events.publish` | Declaring is what makes an event's origin a fact the registry checks. Only the declaring plugin may publish on a type. |
| `events.subscribe` and `events.publish` | Hearing that something happened is a lesser power than being believed when you say it did. |
| `presence.set` and `presence.clear` share one capability | Clearing presence says as much about what the user is doing as setting it. |

`preferences.get` sits under `settings.read`, and there is deliberately no `preferences.set`: those answers are the user's ([ADR-020](../adr/ADR-020-declarative-plugin-preferences.md)).

### Two things that are not capabilities

A static file a plugin ships is not a request a process is making. It is what the plugin is, and installing and enabling it is the consent.

- A plugin's own `flags.json` is read for every enabled plugin with no capability check. `flags.write` gates a *running* plugin rewriting that file through `flags.set`.
- A plugin's own `overlay/` directory is registered for every enabled plugin with no capability check. `assets.override` gates a *running* plugin registering a directory at runtime. Without this a texture pack, which has no process, could not overlay anything. See [ADR-021](../adr/ADR-021-everything-is-a-plugin.md) and [ADR-010](../adr/ADR-010-plugin-asset-overlays.md).

## What a refusal looks like

`host::authorise` runs before dispatch and returns one of two refusals itself; the `ok` is the handler's.

```json
{"status":"denied","id":3,"capability":"flags.write"}
{"status":"error","id":4,"message":"unknown method \"flags.nonsense\""}
{"status":"ok","id":5,"result":[]}
```

A denial is also recorded: `Broker::allows` pushes a `Denial { plugin, capability }` onto a list each time it refuses.

Passing the broker is not the same as reaching an effect. Refusals that come from the handler, all `error`:

| Message | Cause |
|---|---|
| `flags.setDynamic is not implemented yet` | Granted `flags.write.dynamic`. Permanent. |
| `scheme "file" is refused; only http and https may be opened` | `url.open` with a scheme other than `http`/`https`. |
| `"../../etc" must be a path inside the plugin's own directory` | `assets.override` with an absolute or `..` path. |
| `Discord is not running` | `presence.set` with no Discord IPC socket answering. |
| `no broker wired for "flags.set"` | Only from `cordial_plugins::host::Session`, the test host. The client's own host in `plugin_host.rs` serves it. |

An `error` is used for an unwired method rather than `denied`, so you are not sent looking for a permission that was never the problem. Each method's own refusals are on its page: [flags](flags.md), [surface](surface.md).

## Grants are per profile

The file is `<profile>/plugin-grants.json`:

```json
{
  "flag-inspector": ["flags.read", "log"],
  "themer": ["log"]
}
```

Plugin code is installed once for the machine. What a plugin may do belongs to the account, so approving something in a profile you made to try it out does not approve it in the profile you play on ([ADR-013](../adr/ADR-013-per-profile-configuration.md)).

- A pre-existing global `~/.config/cordial/plugin-grants.json` is **moved** into whichever profile first looks for one, in practice `default`. Every other profile starts at default deny. The move is skipped if the profile already has its own file, and a failed move leaves the old file untouched. Why: [ADR-013](../adr/ADR-013-per-profile-configuration.md).
- `CORDIAL_PLUGIN_GRANTS` overrides the path for every profile at once. It is a development switch.

## Granting and revoking

<!-- steps -->

### Install

`consent::verdict` decides whether to ask. A plugin with no entry module and no capabilities installs silently. Anything else gets a dialog listing each capability's consequence sentence, and Allow writes **every requested capability** into the grants file at once. Escape and "Not now" grant nothing. Why: [ADR-003](../adr/ADR-003-plugin-isolation.md).

### Switch it on

Allowing is not starting. A plugin with code is written into `plugin-enabled.json` as off whatever the dialog said, and the success subtitle says so: "It is switched off until you turn it on." Data-only plugins stay absent from the file, and therefore on.

### Adjust per capability

On the plugin's row in Settings, one switch per requested capability. Revoking a plugin's last capability removes its key from the file rather than leaving `"id": []`.

<!-- /steps -->

**Disabling is not revoking.** Grants survive a disable untouched. Settings shows "Off. What you allowed it to do is kept." for a disabled plugin that holds at least one grant, and a bare "Off" otherwise.

Two states look alike and are reported differently:

| Log line | Meaning |
|---|---|
| `no capabilities granted, not started` | You have not decided what to allow. Default deny working as intended. |
| `disabled in Settings, not started` | You switched it off. |

Withheld capabilities are named at startup: `plugin <id>: not granted flags.write, presence.set`.

**The grants file is authoritative, and it is not intersected with the manifest.** `start_all` hands this profile's entry straight to `Broker::grant`; the manifest's list only produces the "not granted" message and decides which switches Settings draws. A capability written into the file by hand that the manifest never requested is therefore granted at runtime (INFERRED from the code path, no client run). The one exception is a hot-swap restart after an update, which intersects ([ADR-038](../adr/ADR-038-plugin-hot-swap.md)).

## Effects, never channels

A plugin never receives a socket, a file descriptor or a D-Bus connection. Cordial holds the permission and performs the effect; the plugin sends a payload. `presence.set` takes a presence structure and Cordial owns the Discord socket. `notify.send` and `url.open` are the same shape over the portal. Why: [ADR-007](../adr/ADR-007-host-resources-are-brokered.md), [ADR-018](../adr/ADR-018-plugin-sub-sandboxing.md).

**If you need something the capabilities do not cover, open an issue.** A resource Cordial does not already broker needs a change to Cordial, not to your manifest. A broker is a payload type and an effect, which makes adding one small, and that is also the test: if a proposed broker cannot be small, the capability is too broad and wants splitting.

[ADR-027](../adr/ADR-027-plugin-overlays.md) proposes `ui.notify`, `ui.hud` and `ui.panel`. Its status is proposed and none of those names exists in `capability.rs`. It is a design under discussion, not something to call.
