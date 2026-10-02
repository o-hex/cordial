# Plugin API: notifications, presence, state, settings and overlays
<!-- description: Reference for notify.send, presence.set, url.open, state.get, settings, preferences and asset overlays. -->
<!-- icon: sliders -->

Every method here is one shape from your side of the pipe: a JSON object goes out and a reply comes back, whatever Cordial does underneath (a D-Bus portal, a Unix socket, JSON files, a filesystem index). Plugins receive the effect, never the channel ([ADR-007](../adr/ADR-007-host-resources-are-brokered.md)). Branch on `status`, not on the presence of `result` ([protocol](protocol.md#messages)).

`denied` names the capability, not the method. A refused `presence.clear` comes back as `"capability":"presence.set"` and a refused `preferences.get` as `"capability":"settings.read"`.

The examples use the `call` helper that every shipped example plugin defines. [`plugins/flag-inspector/main.ts`](../../plugins/flag-inspector/main.ts) is the shortest to read.

| Method | Capability | Page section |
|---|---|---|
| `notify.send` | `notify.send` | [Notifications](#notifications-notifysend) |
| `presence.set`, `presence.clear` | `presence.set` | [Discord presence](#discord-presence-presenceset-and-presenceclear) |
| `url.open` | `url.open` | [Opening a web page](#opening-a-web-page-urlopen) |
| `state.get` | `state.read` | [Session state](#session-state-stateget) |
| `settings.get`, `settings.set` | `settings.read`, `settings.write` | [Settings](#settings-the-document-your-plugin-owns) |
| `preferences.get` | `settings.read` | [Preferences](#preferences-the-users-answers) |
| `assets.override` | `assets.override` | [Asset overlays](#asset-overlays-assetsoverride) |

## Notifications: `notify.send`

| Parameter | Type | Required | Notes |
|---|---|---|---|
| `summary` | string | yes | The title |
| `body` | string | no | The line under it, default `""`. A non-string is read as absent. |

Returns `null`.

```ts
await call("notify.send", {
  summary: "Flag preset applied",
  body: "12 overrides will take effect at the next launch",
});
```

Cordial sends it through `org.freedesktop.portal.Notification.AddNotification` rather than `org.freedesktop.Notifications`, so it costs nothing in the Flatpak manifest: a portal interface is reachable from the sandbox with no `--talk-name` entry.

| Refusal | Cause |
|---|---|
| `notify.send needs a summary` | `summary` absent or not a string |
| `notify.send needs a non-empty summary` | Empty or all whitespace. Checked before the bus is touched. |
| `could not reach the session bus: …` | No session bus in this environment |
| `the notification portal refused the call: …` | The portal answered with an error |

You cannot withdraw or update a notification. Cordial picks the portal id (`cordial-1`, `cordial-2`, …) and does not tell you.

## Discord presence: `presence.set` and `presence.clear`

[`plugins/discord-presence/main.ts`](../../plugins/discord-presence/main.ts) is the working example. `presence.set` takes a **closed** object: any field not in this table refuses the whole call.

| Parameter | Type | Required | Notes |
|---|---|---|---|
| `client_id` | string | yes | A Discord application snowflake, ASCII digits only |
| `details` | string | no | First line, at most 128 characters |
| `state` | string | no | Second line, at most 128 characters |
| `start` | integer | no | Unix seconds. Alone it shows an elapsed counter. |
| `end` | integer | no | Unix seconds. With `start` it shows a countdown. |
| `large_image_key` | string | no | A key Cordial issued, not a URL. An empty string clears the picture. |
| `large_text` | string | no | Hover text |
| `small_image_key` | string | no | As `large_image_key` |
| `small_text` | string | no | Hover text |
| `place_id` | integer | no | The experience being played. Cordial turns it into buttons. |
| `job_id` | string | no | The server instance, a UUID. Needed for a "Join server" button. |

Returns `null`. Cordial assembles Discord's `timestamps` and `assets` sub-objects itself from the flat fields. A key that nothing has resolved renders as no picture. A plugin cannot set a button link: Cordial builds `roblox://experiences/start?placeId=…&gameInstanceId=…` itself, offers the join button only when a `job_id` is present, and falls back to the game page.

```ts
await call("presence.set", {
  client_id: "1234567890123456",
  details: "Using Cordial",
  state: "In session",
  start: Math.floor(Date.now() / 1000),
});
```

`presence.clear` takes no parameters and returns `null`. If nothing has been set in this plugin's run it answers `ok` immediately and opens no connection. One connection is held for as long as the plugin runs, and a different `client_id` drops it and opens a new one.

The payload is a closed struct rather than a JSON value forwarded verbatim, which is the whole reason this is brokered ([ADR-007](../adr/ADR-007-host-resources-are-brokered.md)).

| Refusal | Cause |
|---|---|
| `bad presence payload: …` | The object did not deserialise, including any unknown field (a plugin asking for `buttons` is refused here) |
| `client_id must be a Discord application snowflake (digits only)` | |
| `details must be at most 128 characters, Discord's own limit` | Same sentence for `state`. Refused here so you hear it from the response, not from Discord silently dropping the activity. |
| `job_id must be a server instance UUID` | |
| `large_image_key is not a key Cordial issued` | Same for `small_image_key` |
| `Discord is not running` | No IPC socket answered. The ordinary case, since most users do not have Discord open. The call fails every time; only the logging is throttled. |
| `Discord refused the activity (<code>): <message>` | Discord answered with an error |
| `presence update failed: …` | The connection died mid-write. Cordial drops it so your next call starts a fresh handshake. |

## Opening a web page: `url.open`

| Parameter | Type | Required | Notes |
|---|---|---|---|
| `url` | string | yes | An absolute `http://` or `https://` URL |

Returns `null`. The scheme is checked before the D-Bus connection is touched, so a refusal never depends on a session bus. The check is strict about `://` and case-insensitive: `HTTPS://example.com` is accepted, `http:example.com` is not. Without it the capability would be a way to open `file://` paths or hijack a handler for an arbitrary scheme.

| Refusal | Cause |
|---|---|
| `url.open needs a url` | Absent or not a string |
| `scheme "file" is refused; only http and https may be opened` | The scheme is quoted back |
| `not an absolute http or https URL` | No `://` at all |
| `could not reach the session bus: …` or `the OpenURI portal refused the call: …` | The portal |

## Session state: `state.get`

Capability `state.read`. No parameters. Returns what Cordial knows about the current game, kept from the engine's own log so plugins do not each fold the event stream into a private copy. Every key is optional, and an **absent key means not known**.

| Key | Type | Meaning |
|---|---|---|
| `place_id` | integer | The place, what `roblox.com/games/<id>` names |
| `universe_id` | integer | The experience the place belongs to, the one that resolves to a title |
| `job_id` | string | The server instance UUID, when the join line named one |
| `server_address` | string | The address the client connected through |
| `user_id` | integer | The Roblox user id of the player |
| `joined_at` | integer | Unix seconds at which the current game was joined |

Outside a game the result is `{}`, and leaving a game clears every key. There is no username, display name or server location: a name needs a request to `users.roblox.com` and a location needs a geo-IP lookup, both third-party calls made on the player's behalf. A plugin that starts mid-session can read this once and follow events afterwards ([events](events.md#core-events)). Described from the source (`crates/cordial-plugins/src/state.rs`), not observed in a run (INFERRED).

## Settings and preferences

| | **Settings** | **Preferences** |
|---|---|---|
| Whose answers | Your plugin's | The user's |
| Who writes them | Your plugin, via `settings.set` | Cordial, from its own page |
| Can your plugin write them | Yes | **No, and no method could** |
| What defines the shape | Nothing; any JSON object | The `preferences` array in `plugin.json` |
| Where it lives | `<profile>/plugins/<id>/settings.json` | `<profile>/plugins/<id>/preferences.json` |
| How it is updated | Whole-document replace | One key at a time, read-modify-write |
| Capability to read | `settings.read` | `settings.read` |
| Capability to write | `settings.write` | none exists |

They are two files because `settings.set` replaces wholesale, which is right for scratch state and fatal for anything a person typed. Both live in the profile, not beside your installed code, because a settings document is where a plugin records a username, a server or a webhook ([ADR-013](../adr/ADR-013-per-profile-configuration.md)).

## Settings: the document your plugin owns

`settings.get` takes no parameters and returns your document, `{}` if you have never saved anything (capability `settings.read`). `settings.set` takes `{"settings": <complete new document>}` and returns `null` (capability `settings.write`).

```ts
await call("settings.set", { settings: { panel: "flags", opened: 4 } });
const mine = await call("settings.get");   // mine.result is your document
```

The usual case costs no round trip: the document arrives in the `cordial/init` handshake as `msg.payload.settings` (`{}` if empty, `null` if you were not granted `settings.read`).

**Neither method takes a plugin id.** Cordial knows which process is on the pipe. Naming another plugin in your params is not an error and is not honoured: you get your own document.

| Refusal | Cause |
|---|---|
| `settings.set needs a settings object` | No `settings` key in `params` |
| `settings must be a JSON object` | An array, number or string |
| `settings are 1234567 bytes; the limit is 1048576` | Capped at one mebibyte, measured on the pretty-printed text |
| `<path> is not a JSON object` or `<path> is not usable (…)` | The file on disk is unreadable. Reported rather than answered as empty, so you do not overwrite what the user had. |
| `settings.get needs an open profile; this Cordial has no settings store` | Only from the test `Session` built without a profile |

A write goes to `settings.json.new` and is renamed, so a plugin killed mid-write leaves the previous document.

## Preferences: the user's answers

You declare fields in `plugin.json` and Cordial builds the page. There is no capability for declaring: declaring a field is how you get a page ([ADR-020](../adr/ADR-020-declarative-plugin-preferences.md)).

```json
{
  "id": "example",
  "entry": "main.ts",
  "capabilities": ["settings.read"],
  "preferences": [
    { "key": "loud", "type": "bool", "title": "Be loud",
      "description": "Shown under the title.", "default": false },
    { "key": "level", "type": "int", "title": "Level", "default": 3,
      "minimum": 1, "maximum": 10, "step": 1, "group": "Tuning" },
    { "key": "mode", "type": "choice", "title": "Mode", "default": "slow",
      "options": [ { "value": "slow", "label": "Slow" },
                   { "value": "fast", "label": "Fast" } ] },
    { "key": "note", "type": "text", "title": "Note", "default": "" }
  ]
}
```

| `type` | Row | Its own keys |
|---|---|---|
| `bool` | `AdwSwitchRow` | `default` |
| `int` | `AdwSpinRow` | `default`, `minimum`, `maximum`, `step` |
| `choice` | `AdwComboRow` | `default`, `options` of `{value,label}` |
| `text` | `AdwEntryRow` | `default` |

Every field takes `key` and `title`, and optionally `description` and `group`. Fields sharing a `group` become one group on the page, in order of first appearance, with ungrouped fields first. In a `choice`, `value` is what lands in the document and `label` is prose, so renaming a label does not reset anybody's choice.

The manifest is refused at install, by name with the key quoted, if any of these fails:

| Rule |
|---|
| At most **64** fields |
| Keys of letters, digits, dashes and underscores, at most 64 characters, unique within the plugin |
| A non-empty `title` with no control characters; the same for `description`, `group` and every option `label` where present |
| `int`: `minimum` no greater than `maximum`, a `default` inside the range, `step` greater than zero if given |
| `choice`: at least one option, no two sharing a `value`, a `default` that is one of them |
| `text`: a `default` of at most 4 KiB with no control characters |

Length caps on drawable text are bytes, not codepoints, so 150 characters in a non-Latin script can be refused by a message saying "at most 200 characters". Your words are drawn as text, never markup, and a control character anywhere refuses the whole plugin rather than being stripped.

Reading needs `settings.read` and arrives as `msg.payload.preferences` in the handshake or from `preferences.get` (no parameters):

```ts
const prefs = (await call("preferences.get")).result;
```

**The document is always complete and valid**: every declared key is present and every value fits its declaration. No `?? default` and no range checks. A saved value that no longer fits your manifest falls back to the current default and Cordial logs `plugin <id>: preference …`. Keys nothing declares are dropped. A plugin that declares no fields and holds `settings.read` gets `{}`; `null` still means not granted.

**There is no `preferences.set`.** A plugin that could rewrite the answers could have the page show its choice back as the user's. Your own state goes in `settings.json`. You cannot draw the page yourself: you are a separate process with no display, and one able to draw in Cordial's window could imitate its sign-in dialog.

## Asset overlays: `assets.override`

A plugin, or the user directly, may supply files that resolve in place of Roblox's own for the same name. Nothing is written into the APK or into anything extracted from it, so there is no cleanup: stop consulting a root and the original resolves again ([ADR-010](../adr/ADR-010-plugin-asset-overlays.md), which reverses [ADR-004](../adr/ADR-004-plugin-asset-overrides.md)).

### Two routes

**A shipped `overlay/` directory needs no capability.** Cordial registers it at launch, before the engine reads an asset. It is the only route for a data-only plugin, and the only reliable one, because **an asset served once stays cached for the rest of the process**: an overlay registered after a texture loads cannot change it.

The `assets.override` **method** is for a *running* plugin registering a directory at runtime. Plugins start after the client is up, so this usually arrives after the engine has read a great deal. Use it to swap a root mid-session, not to ship a texture pack.

| Parameter | Type | Required | Notes |
|---|---|---|---|
| `dir` | string | no | A relative path inside your installed directory. Default `"overlay"`. |
| `clear` | bool | no | `true` unregisters your root and ignores `dir` |

Returns `{"registered": "/absolute/path"}`, or `null` for a clear. The path is a string for your log, not a handle.

```ts
await call("assets.override", { dir: "packs/winter" });
await call("assets.override", { clear: true });
```

| Refusal | Cause |
|---|---|
| `"../../etc" must be a path inside the plugin's own directory` | Absolute path or `..`. Refused rather than rewritten, as a manifest's `entry` is. |

The directory does not have to exist: registering a missing one succeeds and contributes no files. If an overlay appears to do nothing, check the path first. Your root is unregistered when your process ends, however it ends.

### What resolves, and in what order

A root mirrors the APK's `assets/` layout, the same shape Sober's `asset_overlay` uses. `<root>/content/textures/wood.png` stands in for `assets/content/textures/wood.png`.

Roots form a stack, lowest first: every plugin in registration order, then the user's root last. **The user's root beats every plugin's**, and among plugins the most recently registered wins (re-registering moves you to the end). The user's root is `$XDG_CONFIG_HOME/cordial/overlay`, falling back to `$HOME/.config/cordial/overlay`, overridable with `CORDIAL_OVERLAY`. Paths that would escape their root (`..`, an absolute name, a symlink pointing outside) are dropped when the index is built.

| | |
|---|---|
| **Can be replaced** | Anything under the APK's `assets/` tree that the engine reads through `AAssetManager`: textures, sounds, fonts, models. Lookup checks the process cache, then the overlay stack, then the archive. |
| **Cannot, today** | Anything the engine opens by a real filesystem path. The resolver exists and is tested, but `native/system_paths.cpp` is not wired, and it must be wired for `stat`, `open`, `fopen` and `access` together. Assume the `AAssetManager` route only. |
| **Cannot, ever** | Writes. Reads resolve to the overlay and writes go to the original. |
| **Cannot** | Anything outside the asset tree. It is not a general filesystem redirect. |

> [!CAUTION]
> Gameplay-affecting substitution is possible and Cordial builds no detection for it. Replacing a collision or hitbox mesh with a smaller or absent one is an advantage, not a cosmetic change. [ADR-010](../adr/ADR-010-plugin-asset-overlays.md) leaves it to the user's responsibility, as Sober and Bloxstrap do, and the capability's consent text says so.

Overlays are resolved by interception rather than a mount (Why: [ADR-010](../adr/ADR-010-plugin-asset-overlays.md)), which is what makes the diagnostics possible. `--check-overlays` reports which of your files match nothing in the current build, and the shadow report names every case where two layers offered one file and which won:

```text
user wins over plugin:winter   content/textures/wood.png
```

Read it in the direction the stack is built. The user's layer is last, so it appears on the left against any plugin. A plugin appears on the left only against another plugin registered before it. If your file is on the right, the fix is not in your plugin.
