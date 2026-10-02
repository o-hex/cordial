# `cordial.runtime/1`: the runtime spec

**Status: draft v0.1, accepted as design in [ADR-052](adr/ADR-052-the-runtime-spec.md). Nothing here is implemented.** No runtime speaks it, `cordial` has no code to load one, and no `--runtime-check` exists. Treat every section as a proposal until the ADR's implementation lands. Sections marked **draft** are the least settled.

Written for someone building a runtime. Cordial lists only its own built-in runtime for now (ADR-052), so a runtime written to this spec will not appear in Cordial until the maintainer vets it, and one that injects code into the Roblox client will not be listed at all.

A **runtime** is whatever turns Play into a running Roblox client. Cordial is the launcher: window chrome, profiles, FastFlag layers, plugins, presence, doctor, report and the update UI. The spec carries events and effects, never channels or code.

## 1. Manifest

`runtime.json`, searched in `$XDG_DATA_HOME/cordial/runtimes/<id>/`, then each `$XDG_DATA_DIRS/cordial/runtimes/<id>/`. **Draft:** discovery across Flatpak sandboxes is unsolved, because the socket must be reachable from both sides. Version 1 is native or same-sandbox only.

```json
{
  "spec": "cordial.runtime/1",
  "spec_version": "1.0",
  "id": "org.example.runtime",
  "name": "Example",
  "version": "0.13",
  "arch": ["x86_64"],
  "launch": { "exec": ["bin/run"], "args": ["--socket", "{socket}", "--profile", "{profile_dir}"] },
  "capabilities": { "lifecycle": 1, "profile": 1, "events.core": 1 },
  "support_url": "https://example.org/issues",
  "licence": "MIT"
}
```

`exec` is relative to the manifest's directory. `arch` is the host architectures the runtime runs on; Cordial hides it on any other, and does not offer translation. The manifest is advertisement only, and the handshake is the truth. A manifest naming a `spec` major Cordial does not know is shown as "needs a newer Cordial", not hidden.

## 2. Transport

JSON lines, UTF-8, `\n`-terminated, at most 64 KiB a line; a longer one is a protocol error. The **runtime opens** a Unix socket at `{socket}` per session, inside a `0700` directory under the profile (`<profile>/runtime/<session>/`, the shape ADR-044 uses). Cordial connects. Plugins never see it.

```
{"v":1,"id":7,"m":"flags.apply","p":{}}                                request, Cordial to runtime
{"v":1,"id":7,"ok":true,"p":{}}                                         reply
{"v":1,"id":7,"ok":false,"e":{"code":"unsupported","detail":"..."}}    error reply
{"v":1,"ev":"game.joined","p":{}}                                       event, runtime to Cordial
```

## 3. Handshake and versioning

Cordial sends `hello {spec:"1.0", cordial, session, profile}`. The runtime answers `{runtime:{id,version}, client:{name,version,build}, capabilities:{name:{ver}}}`. The live set is the intersection of both sides.

- `spec_version` is `major.minor`. A minor adds optional capabilities, events and fields; receivers ignore what they do not know. A major breaks. Cordial refuses a runtime whose major it does not speak.
- Each capability has an integer version. Within a major it never changes meaning or loses a field.
- Runtime-private events are prefixed `x-<id>.`. Plugins never see them; they appear only in the report.
- **Draft:** a conformance check, `cordial --runtime-check <manifest>`, that runs the handshake against the runtime and prints doctor-shaped results.

## 4. Unsupported is never faked

A capability that is not offered is shown as unsupported in the interface, with the runtime's name. No request for it is sent. A plugin that needs it is shown "limited on `<runtime>`" rather than loaded and silently dead. Replies never default to success.

Error codes: `unsupported`, `invalid`, `failed`, `busy`. An unknown request gets `unsupported`. An unknown event is ignored. A message that does not parse closes the session, and the reason goes in the report. Emit only events the client actually produces; a declared event nothing publishes is a lie of the same kind.

## 5. Capabilities

| Capability | Carries | Required |
|---|---|---|
| `lifecycle` | Cordial starts the runtime from the manifest. `lifecycle.stop` asks for a graceful exit. The runtime emits `lifecycle.ready`, `lifecycle.exit {status, reason}` and `crash {signal, summary, log_path}` | yes |
| `profile` | Cordial gives `{profile_dir}`. The runtime declares `instances.max` and whether session data lives in the profile or its own store | yes |
| `events.core` | `game.joined {place_id, universe_id?, job_id?, server_address?, user_id?, at}`, `game.left`, `session.state {signed_in}`. Never the token | no |
| `events.presence` | `game.presence`, the folded BloxstrapRPC payload | no |
| `events.log` | `{dir, glob}` of the client's own log, which Cordial parses itself, as an alternative to the two above | no |
| `flags` | Cordial resolves the layers (user wins, ADR-013) and writes the flat document to `flags.path` from the handshake, then sends `flags.apply`. Declares `{families, allowlist, live}`. `flags.live` takes `DF*` only and answers applied or ignored per name | no |
| `settings` | Declares which of Cordial's closed key set it honours, each `live` or `next-launch` (ADR-044), and optionally a typed form for its own keys (bool, enum, int range, label, help). Declarative; no code runs | no |
| `session.vault` | An opaque blob Cordial keeps in the keyring. Never offered to plugins | no |
| `doctor` | Declarative `requires` in the manifest (`executable`, `kernel-module`, `setuid`, `socket`), checked by Cordial with no runtime code, plus `doctor.run` returning `Check {level, what, fix}` | no |
| `diagnostics` | `diagnostics.get`: ordered, redacted key/value lines. The report always names the runtime id and `support_url` | no |
| `updates` | `updates.check {installed, latest, obtainable, notes_url}`, `updates.install` with progress events, and a mandatory `verify` field (`apk-signature`, `apple-codesign`, `sha256`, `none`) that Cordial shows | no |
| `window` | `mode: "embedded"` or `"toplevel"`, plus `title_bar`, `fullscreen` and `resized` events. A foreign toplevel declares `toplevel`, and Cordial hides rows that act on a window it does not own | no |
| `launch.join` | Accepts a translated join URL. Deep-link translation stays in Cordial | no |
| `assets.overlay` | Registers a plugin asset root (ADR-010) | no |
| `debug.control` | The screenshot and input verbs of ADR-019. Present only when `CORDIAL_DEV_CONTROL` is set | no |

## 6. What stays in Cordial

The plugin host, grants and broker, the Discord socket, notifications, URL opening, the secret store, the profile lock (ADR-012), the settings and report screens, deep-link translation, and aggregation of doctor output.

**Plugins never talk to a runtime.** Cordial maps events onto the existing plugin API: `game.joined` to `SessionState`, `lifecycle.*` to the `client.*` events, `game.presence` to `cordial/game.presence`, `flags.apply` and `flags.live` behind the flag write grants, `assets.overlay` behind asset overrides. A plugin whose capability has no backing is marked limited.

## 7. Hard limits

- The verb set is closed per spec version. Not offered, ever: engine memory, loading code into the client, calling engine functions, executing a command, passing a path or descriptor, evaluating anything, a generic "set raw" or "call".
- Plugin-supplied strings reach a runtime only as typed payloads Cordial has validated: flag names and values, never paths.
- A capability is not added because it would be convenient. Adding one needs an ADR and a spec bump (ADR-001).
- **The spec cannot police a runtime's own process.** A runtime that injects code into Roblox is outside anything a protocol can prevent. The only lever is Cordial's listing policy (ADR-052): a runtime that injects is not listed.

## 8. The built-in Android runtime

The Android runtime implements this spec in-process: the same messages over a channel, not a socket, so there is one code path and the Android runtime is the conformance suite. The portable core (`flags.rs`, `bloxstrap_rpc.rs`, `client_settings.rs`, `game_log.rs`, `plugin_host.rs`) has almost no Android coupling today; `live_settings.rs` and `devctl.rs` are the coupled ones (spike, section 6). Splitting the crates is future work.
