---
title: "Plugin API: getting started"
description: "Write, grant and run a first plugin, and what plugin.json may contain."
icon: "rocket"
---
A plugin is a directory holding a `plugin.json` and, usually, one TypeScript module. Cordial runs the module as a sandboxed Deno process and talks to it in newline-delimited JSON over stdin and stdout. A plugin cannot open a file, a socket or a subprocess, so asking Cordial to do things through [capabilities](/plugin-api/capabilities) is the only surface there is. Why: [ADR-003](/adr/ADR-003-plugin-isolation), [ADR-007](/adr/ADR-007-host-resources-are-brokered).

A manifest with no `entry` is a plugin made of data: a texture pack, a set of flags, a preferences page. Cordial logs `data only, nothing to start` and carries on ([ADR-021](/adr/ADR-021-everything-is-a-plugin)). The rest of this page is about the kind that runs.

## The smallest plugin that works

<Steps>
<Step title="Create the files">


```text
~/.local/share/cordial/plugins/
└── hello/
    ├── plugin.json
    └── main.ts
```

`plugin.json`:

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "1.0.0",
  "entry": "main.ts",
  "capabilities": ["log"]
}
```

`main.ts`:

```ts
const enc = new TextEncoder();
const dec = new TextDecoder();

let nextId = 1;
const pending = new Map<number, (msg: any) => void>();

// One reader for the whole process. Replies and unsolicited pushes arrive on
// the same stream, and a plugin that stops reading fills the pipe.
(async () => {
  let buf = "";
  for await (const chunk of Deno.stdin.readable) {
    buf += dec.decode(chunk);
    let i: number;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (!line.trim()) continue;
      const msg = JSON.parse(line);
      // A push has no `id`; a reply always has one. This plugin subscribes to
      // nothing, so it drops pushes on purpose.
      if (msg.id === undefined) continue;
      pending.get(msg.id)?.(msg);
      pending.delete(msg.id);
    }
  }
})();

function call(method: string, params: unknown = {}): Promise<any> {
  const id = nextId++;
  const p = new Promise<any>((resolve) => pending.set(id, resolve));
  Deno.stdout.write(enc.encode(JSON.stringify({ id, method, params }) + "\n"));
  return p;
}

await call("log.write", { message: "hello from a plugin" });
```


</Step>
<Step title="Grant it `log`">


Until something is granted the plugin is not refused, it is never started: `plugin hello: no capabilities granted, not started`. Flip the `log` switch on the plugin's row in Settings, or write `~/.local/share/cordial/profiles/default/plugin-grants.json`:

```json
{ "hello": ["log"] }
```

Grants are per profile ([ADR-013](/adr/ADR-013-per-profile-configuration)), so approving a plugin in a throwaway profile does not approve it in the one you play on.


</Step>
<Step title="Run it">


Start Cordial and look for the plugin's line in the client output:

```text
  [hello] hello from a plugin
```


</Step>
</Steps>

<Warning>

Never write anything but protocol lines to stdout. `console.log` in Deno writes to stdout, which is the wire. The first line Cordial cannot parse as a request ends the session (`plugin hello: sent something unreadable (…)`) and the process is killed. Use `console.error`, which goes to Cordial's own output, or `log.write`.

</Warning>

<Info>

Keep the directory name and the manifest `id` identical. Cordial does not check. Grants, settings, events and `flags.set` key off the manifest `id`, but the flag layer read back at startup and the enablement check key off the directory name. A plugin in `hello-dev/` calling itself `hello` writes flags into a `hello/` directory that nothing installed.

</Info>

## `plugin.json`

Only `id` is required.

| Key | Required | If missing |
|---|---|---|
| `id` | yes | The manifest does not load: `missing field id`. Present but empty gives `id must not be empty`. |
| `name` | no | Empty. Settings and the install prompt show the `id`. |
| `entry` | no | A data-only plugin. Nothing is spawned. |
| `capabilities` | no | Requests nothing. Settings shows `Requests no capabilities`. |
| `version` | no | Loads, but cannot be published to an index or depended upon. |
| `dependencies` | no | Depends on nothing. Read by the installer only, not at start-up ([events](/plugin-api/events#start-order-and-dependencies)). |
| `preferences` | no | No preferences page and no gear button ([surface](/plugin-api/surface#preferences-the-users-answers)). |

| Rule | What happens |
|---|---|
| `id` may contain only ASCII letters, digits, `-` and `_` | Refused with the id quoted back. The id names your installed directory, your settings directory and every key the broker uses. |
| `entry` must be a relative path inside the plugin directory | An absolute path or one containing `..` parses, then fails at run: `entry "../../../etc/shadow" must be a path inside the plugin directory`. |
| An unrecognised capability name | The whole manifest is refused: `unknown capability "process.spawn"`. A malformed `preferences` declaration is refused the same way. |
| An unrecognised manifest key | Ignored. Do not rely on that. |

Discovery is quiet in two cases:

- A directory with no `plugin.json` is passed over without comment. One whose `plugin.json` will not parse is announced as `plugin: /path/plugin.json is not loadable (…)`.
- A directory whose name starts with `.` is skipped, because the installer stages half-unpacked plugins there.

## Capability names are not method names

`flags.write` is a capability: what a user grants, and the string a refusal names. `flags.set` is the method it gates. Calling `flags.write` gets `unknown method "flags.write"`, an `error` rather than a permission error. `settings.read` is a capability and `settings.get` is the method.

The full list and the method each one gates is on [Capabilities and grants](/plugin-api/capabilities#the-fifteen-capabilities).

## Test without packaging

You do not need an archive, an index or an install step. Cordial discovers plugins from a directory and follows symlinks to directories. Give the run its own data root so you stay off the profile you play on:

```bash
export XDG_DATA_HOME=~/.cache/cordial-dev
mkdir -p "$XDG_DATA_HOME/cordial/plugins" "$XDG_DATA_HOME/cordial/profiles/default"
ln -s ~/code/hello "$XDG_DATA_HOME/cordial/plugins/hello"
echo '{"hello":["log"]}' > "$XDG_DATA_HOME/cordial/profiles/default/plugin-grants.json"

cargo run --release --bin cordial-run -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 60
```

Use a path on real disk rather than `/tmp`, which is tmpfs and comes out of RAM, and delete it afterwards. `just dev` and `just client` take the same variable.

Two development switches exist. Neither is supported configuration:

| Variable | What it moves |
|---|---|
| `CORDIAL_PLUGIN_DIR` | The plugin root: discovery, and where `flags.set` writes your flag layer. |
| `CORDIAL_PLUGIN_GRANTS` | The grants file, for every profile at once. Use it for a scratch run, not a machine you play on. |

<Tip>

Set `--run 60` generously. Plugins start late in bring-up, after the engine reports itself up, so a short run can exit before your plugin has said anything. How short is too short has not been measured (INFERRED from where `start_all` sits in bring-up).

</Tip>

## Seeing a plugin's output

| Channel | How | Where it lands |
|---|---|---|
| `log.write` | Needs `log`; takes `{message}` | The client's stdout, indented and tagged with your id, in order beside the grant and spawn lines. Nothing leaves the machine. |
| stderr | `console.error`, uncaught exceptions, Deno's complaints about your TypeScript | Cordial's output directly, without passing through the protocol. The first place to look when a plugin starts and goes quiet. |

If a plugin subscribes to events and receives none, rule out that there are none to receive. A plugin event is written straight into your stdin with no queue, so nothing arrives if the publisher is not running or was never granted `events.publish`. A core event is addressed by capability, so check `lifecycle.read` was granted in this profile, and which of the events you are waiting for are actually published ([events](/plugin-api/events#core-events)).

## Next

<Columns cols={2}>
  <Card title="Protocol and sandbox" href="/plugin-api/protocol">
    messages, pushes, what Deno permits, how a plugin starts.
  </Card>
  <Card title="Capabilities and grants" href="/plugin-api/capabilities">
    the list, default deny, refusals, per-profile grants.
  </Card>
  <Card title="Events" href="/plugin-api/events">
    core events and plugin-declared events.
  </Card>
  <Card title="FastFlags" href="/plugin-api/flags">
    read the resolved set, contribute a layer.
  </Card>
  <Card title="The rest of the surface" href="/plugin-api/surface">
    notifications, presence, URLs, session state, settings, preferences, asset overlays.
  </Card>
  <Card title="What you cannot do" href="/plugin-api/limits">
    the walls.
  </Card>
</Columns>

`plugins/README.md` covers versions, dependencies and publishing. Decisions: [ADR-008](/adr/ADR-008-plugins-are-typescript-on-deno) (Deno), [ADR-018](/adr/ADR-018-plugin-sub-sandboxing) (sub-sandbox), [ADR-020](/adr/ADR-020-declarative-plugin-preferences) (preferences).
