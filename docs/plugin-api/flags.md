# Plugin API: FastFlags
<!-- description: How flag layers resolve, and the flags.list, flags.get and flags.set methods. -->
<!-- icon: flag -->

FastFlags reach the engine through a layered resolver in `crates/cordial-runtime/src/flags.rs`. A plugin can read the resolved set with provenance, and write one layer: its own.

## The layers, and who wins

`flags::collect()` builds the layers lowest first and `flags::resolve()` applies them in that order, so the last layer to name a key wins.

| Order | Layer | Source |
|---|---|---|
| 1 | `built-in` | Cordial's own defaults (currently empty) |
| 2 | `performance mode` | The `CORDIAL_PERFORMANCE` tables (empty unless a mode is chosen) |
| 3 | `plugin:<id>` | `<plugin dir>/<id>/flags.json`, one layer per installed, enabled plugin, alphabetical by id |
| 4 | `user` | `<profile>/flags.json` |

The user always wins. No call, parameter or ordering trick lets a plugin beat layer 4.

- **Layer 3 is ordered by plugin id**, not by which root the plugin came from, so a user plugin `zzz` beats a first-party `aaa`. The id is the plugin's directory name.
- **Plugin conflicts are reported, not resolved.** Two plugins setting one key differently are both named in the log, and the later one alphabetically wins.
- **A user plugin may not shadow a first-party one.** The system root claims the id first and a same-id directory in the writable root is skipped, with both paths named.
- **A disabled plugin's `flags.json` is not read.**
- **Roblox's own client-settings document is not a layer.** It is the base the resolved set is merged into, so `flags.list` never shows Roblox's default for a key nobody overrode.

## `flags.list`

Capability `flags.read`. No params. Returns an array with one object per resolved key.

```json
{"status":"ok","id":1,"result":[
  {"key":"DFFlagRbxTransportUseRtcioRna","value":"false","source":"user"},
  {"key":"FIntTaskSchedulerAutoThreadLimit","value":"8","source":"plugin:tuner"}
]}
```

`value` is always a string. Layers read from disk convert JSON numbers and booleans to strings, because Roblox stores every setting as a string. Only the winning value is listed. `source` is one of:

| `source` | Meaning |
|---|---|
| `user` | The user's own `<profile>/flags.json` |
| `plugin:<id>` | That plugin's `flags.json` |
| `built-in` | Cordial's own defaults |
| `performance mode` | The flags the chosen `CORDIAL_PERFORMANCE` mode asks for |

## `flags.get`

Capability `flags.read`. Params `{"key": "FFlagSomething"}`.

```json
{"status":"ok","id":2,"result":{"value":"true","source":"plugin:tuner"}}
```

A key no layer sets answers `{"status":"ok","id":2,"result":null}`, an `ok` with a null result and not an error. A missing or non-string `key` is read as the empty string and also answers `null`. Check for `null` before reading `.value`.

## `flags.set`

Capability `flags.write`. Params are a `values` object, not a key and a value.

```json
{"id":3,"method":"flags.set","params":{"values":{"FFlagFoo":"true","FIntBar":3}}}
```

| Behaviour | Detail |
|---|---|
| Replaces, never merges | The document is replaced whole, so omitting a key withdraws it. Send everything you want, every time. |
| Stringifies | `3` lands as `"3"`. Roblox's own document uses `"True"` and this produces `"true"`, so write the string yourself if the spelling matters. |
| Takes effect at the **next launch**, only | See [two lifetimes](#two-lifetimes). |
| Written atomically | To a sibling file, then renamed, so a kill mid-write leaves the previous valid document. |
| Machine-global | Lands at `~/.local/share/cordial/plugins/<id>/flags.json` and applies in every profile whether or not the plugin was granted anything there. Open question in [ADR-013](../adr/ADR-013-per-profile-configuration.md). |

| Refusal | Cause |
|---|---|
| `flags.set needs a values object` | `values` missing or not an object. An `error`, not `denied`. |
| `<n> bytes of flags is more than the 262144 byte limit` | The serialised document exceeds 256 KiB. |

### `flags.write` reaches Cordial's own settings

Any key beginning `Cordial` rides the same layering and is filtered out before Roblox's settings document, because the engine does not know those keys. The ones that exist today are `CordialGraphicsBackend`, `CordialPresentMode` (what `fps-flex` writes) and `CordialDeviceProfile`. A plugin's request only wins where the user's own Graphics setting is Automatic. This is why the consent text says:

> Change how Cordial itself renders and behaves. Sets Roblox FastFlags, and also Cordial's own settings including the graphics backend and present mode. Takes effect at the next launch. Your own choices in Settings still win.

Every future `Cordial*` key inherits the same reach.

## Two lifetimes

`FFlag`, `FInt` and `FString` are consumed once, during `nativeInitClientSettings`, roughly 100 ms into startup. Only the `DFFlag`/`DFInt`/`DFString` family is re-read while the client runs. That is why there are two capabilities ([ADR-005](../adr/ADR-005-flag-service.md)).

| Capability | Effect |
|---|---|
| `flags.write` | Contributes to your `flags.json`, read before the engine starts. Effective at the next launch. Implemented by `flags.set`. |
| `flags.write.dynamic` | Meant for changing a `DF*` flag live. **Not implemented and never will be.** |

`flags.setDynamic` passes the broker for a plugin holding `flags.write.dynamic` and lands on the catch-all:

```json
{"status":"error","id":4,"message":"flags.setDynamic is not implemented yet"}
```

A live write would need access to the running engine's flag table, which [ADR-001](../adr/ADR-001-in-process-hooking.md) and [ADR-003](../adr/ADR-003-plugin-isolation.md) rule out. The `error` is deliberately not a `denied`.

> [!IMPORTANT]
> A `DF*` override in your `flags.json` governs about the first two seconds. The engine fetches Roblox's own settings document 1.6 to 2.3 s in and reapplies it over the top, so any `DF*` key that document contains is reverted while the client is still starting. Keys it does not contain keep your value for the whole run. Measured in both directions inside one run, with a control. A startup flag is the stronger surface, not the weaker one. Why: [ADR-051](../adr/ADR-051-overrides-are-reapplied-after-the-engines-refresh.md).

## Worked example

Reports what is in effect, then contributes a flag and handles the refusal. The `call` scaffold is the one from the [events example](events.md#worked-example-a-plugin-on-both-buses).

`plugin.json`:

```json
{
  "id": "tuner",
  "name": "Tuner",
  "version": "1.0.0",
  "entry": "main.ts",
  "capabilities": ["flags.read", "flags.write", "log"]
}
```

`main.ts`:

```ts
const log = (message: string) => call("log.write", { message });

// Cordial's layers only; Roblox's own defaults are not in here.
const listed = await call("flags.list");
if (listed.status === "ok") {
  const entries: Array<{ key: string; value: string; source: string }> = listed.result;
  await log(`${entries.length} override(s) in effect`);
  for (const e of entries) {
    await log(`  ${e.key} = ${e.value}  (from ${e.source})`);
  }
} else {
  await log(`could not read flags: ${listed.status}`);
}

// `result` is null, not an error, when no layer sets the key.
const one = await call("flags.get", { key: "FIntTaskSchedulerAutoThreadLimit" });
if (one.status === "ok" && one.result === null) {
  await log("nothing sets the thread limit; this plugin's value will stand");
} else if (one.status === "ok" && one.result.source === "user") {
  // The user's layer is above every plugin's. Writing is not an error and is
  // not honoured, so say so.
  await log(`the user set the thread limit to ${one.result.value}; leaving it alone`);
}

// A whole-document replace: send every key this plugin wants, every time.
// Strings on purpose; `true` would become "true", not Roblox's "True".
const set = await call("flags.set", {
  values: {
    "FIntTaskSchedulerAutoThreadLimit": "8",
    "CordialPresentMode": "uncapped",
  },
});

if (set.status === "ok") {
  await log("flags written; they take effect at the next launch");
} else {
  await log(
    `could not write flags: ${set.status}` +
      (set.capability ? ` (needs ${set.capability})` : "") +
      (set.message ? ` (${set.message})` : ""),
  );
}
```

The two refusals to recognise:

```json
{"status":"denied","id":3,"capability":"flags.write"}
{"status":"error","id":3,"message":"flags.set needs a values object"}
```

`denied` means the grant is missing: check `plugin-grants.json` for this profile. `error` means the call was allowed and the request was wrong.
