# Plugin API: events
<!-- description: Core events Cordial publishes, events plugins declare for each other, and what is and is not delivered. -->
<!-- icon: bell -->

There are two event buses. They share the wire format and nothing else.

| | Core events | Plugin events |
|---|---|---|
| What | Cordial reporting what it observed | Plugins talking to each other |
| Names | `cordial/<name>`, a closed table | `<plugin-id>/<name>`, declared at runtime |
| Authorised by | A capability, per event family | `events.declare`, `events.publish`, `events.subscribe` |
| You get them by | Holding the capability. There is no subscription. | Calling `events.subscribe` on a declared type |
| Delivery | Queued 256 deep per plugin, drops when full | Blocking write into your stdin, nothing dropped |
| Can a plugin answer | No | No |

Both arrive as a push: a line with `event` and `payload` and no `id` or `status` ([protocol](protocol.md#messages)). `crates/cordial-plugins/src/protocol.rs` has a test asserting `Push` never grows either field.

The `cordial/init` handshake uses the same shape. It is not an event anyone published and no capability gates it. Match on the name and ignore it.

## Core events

`crates/cordial-plugins/src/core_events.rs` holds the whole vocabulary. Each entry is gated by one capability. **A dispatcher comparing `msg.event === "client.launch"` never fires**: the wire name is `cordial/client.launch`.

| Wire name | Capability | Payload | Published |
|---|---|---|---|
| `cordial/client.launch` | `lifecycle.read` | `{"profile": …}` | yes |
| `cordial/engine.version` | `lifecycle.read` | `{"version": …}` | yes |
| `cordial/client.shutdown` | `lifecycle.read` | `null` | yes |
| `cordial/game.presence` | `presence.set` | the merged presence, see below | yes |
| `cordial/client.ready` | `lifecycle.read` | none | **no, nothing publishes it** |
| `cordial/window.resized` | `lifecycle.read` | none | **no, nothing publishes it** |

> [!WARNING]
> A plugin that waits for `client.ready` or `window.resized` waits forever. They are in the table so that adding a publisher later cannot forget the capability. Design around the others.

- **`client.launch`** goes out immediately after `start_all` returns, the first moment anybody can be told. `profile` is the active profile's *name*, not its path, and `null` if the path has no final component.
- **`engine.version`** follows once the version has been read from `libroblox.so`. If it cannot be read nothing is published and Cordial prints `plugins: engine version not readable, so cordial/engine.version is not published`.
- **`client.shutdown`** is the last thing any plugin is told and is followed by a bounded flush.
- **`game.presence`** is what a running experience set as its Rich Presence (BloxstrapRPC lines the engine writes to its own log, read by Cordial; nothing is hooked). The payload is the merged presence so far, with keys such as `details`, `state`, `start`, `end`, `large_image_key`, `large_text`, `small_image_key`, `small_text`, `place_id` and `job_id`, and it is `{}` when you leave the game. Gated on `presence.set` rather than `lifecycle.read`, so a plugin allowed to know the client started does not thereby learn what you play. Described from the source, not observed in a run (INFERRED).
- Handlers should treat a missing field as normal. `client.shutdown` carries `null` because the name is the whole information.
- There is no `network.connected`. If you saw it in a list, that list was reading a negative test that proves a name absent from the table reaches nobody.

Core events carry nothing about a place, game or user except `game.presence` (and `place_id`, `job_id` inside it). The session snapshot with the user id is a separate call, [`state.get`](surface.md#session-state-stateget). Why: [ADR-007](../adr/ADR-007-host-resources-are-brokered.md).

### The capability is the subscription

`events.subscribe` on a `cordial/…` type is refused (`"cordial/client.launch" has not been declared by any plugin`). Core events are never declared into the registry. Request `lifecycle.read` in your manifest, be granted it, and pushes are addressed to you. An event absent from the table needs a capability nobody holds and reaches nobody (`plugin core event "…" is not in the capability table, so nobody receives it`).

`lifecycle.subscribe` returns `{"status":"ok","id":N,"result":null}` and records nothing. It confirms you hold `lifecycle.read`, which a push cannot do: silence cannot distinguish "not granted" from "nothing has happened yet". A plugin that never calls it hears the same events.

A plugin may not declare under `cordial`: `"cordial" is reserved for Cordial's own events; a plugin may not declare under it`. So no plugin can mint a convincing `cordial/…` event.

### Observed, never vetoed

Core events are observed and cannot be vetoed, delayed or altered ([ADR-026](../adr/ADR-026-the-core-event-bus.md)). This is structural: delivery is a push with no `id`, so there is nothing to answer on, and `publish_core` never reads the plugin's stdout. Publishing is a `try_send` onto a per-plugin queue and never blocks the thread publishing a platform event. Why a structural absence is a stronger guarantee than an ignored return value: [ADR-026](../adr/ADR-026-the-core-event-bus.md).

### A slow subscriber misses events

The loss is counted, not silent.

| Fact | Value |
|---|---|
| Queue depth | `QUEUE_DEPTH = 256` pushes per plugin |
| Queue full | The event is dropped and that plugin's counter incremented. The plugin is not told and a push has no sequence number. |
| Reporting | The client prints `  plugin <id>: <n> core event(s) dropped, its queue was full` at exit. A plugin that already exited is not listed. |
| Shutdown flush | `flush_core_events(limit)` waits up to `limit` per plugin. The client uses 500 ms, then prints `  plugins: a plugin did not read its queue within 500 ms; exiting without it`. |
| Measured | In `host.rs`'s own test, 4000 events of 4 KiB published in about 6 ms, 3735 dropped and counted, consumer far behind. That shows the publisher's cost does not track the reader's speed. It does not show a genuinely wedged consumer was survived. |

Write a plugin that expects to miss things. One needing exactly-once delivery should ask Cordial for state instead.

> [!NOTE]
> The repository has two plugin hosts. `crates/cordial-runtime/src/plugin_host.rs` is the one `cordial-run` starts and the one whose messages and limits this page quotes. `cordial_plugins::host::Session` has the same shape and is constructed only in that crate's tests.

## Plugin-declared events

Unlike the core bus, this one is wired up in the host the client runs.

| Method | Capability | Params | `result` |
|---|---|---|---|
| `events.declare` | `events.declare` | `{"name": "<bare name>"}` | `{"type": "<your-id>/<bare name>"}` |
| `events.publish` | `events.publish` | `{"type": "<full type>", "payload": <any>}` | `null` |
| `events.subscribe` | `events.subscribe` | `{"type": "<full type>"}` | `null` |

You supply a bare name and Cordial supplies the namespace, from its own record of which process is on the pipe. Take `type` from the response rather than assembling it. A name containing a slash gives `your-id/a/b` and still cannot escape your prefix.

| Error | Cause |
|---|---|
| `"evil" may not publish on "flag-manager/profile-changed"; it must declare that type before publishing on it` | Publishing on a type you did not declare. An `error`, not `denied`: you may well hold `events.publish`. |
| `"flag-manager/profile-changed" has not been declared by any plugin` | Subscribing to a type nobody has declared yet. |

Re-declaring your own type is not a conflict, so a restarting plugin lands where it was.

### Start order and dependencies

Nothing orders start-up so that declarations happen first. `dependencies` does not affect start order.

`plugin_host::start_all` iterates plugins sorted by directory name and never consults `dependencies`. The dependency planner in `crates/cordial-plugins/src/resolve.rs` decides what gets installed, never what starts first. Nothing refuses to start a plugin whose declared dependency is absent either, so [ADR-006](../adr/ADR-006-plugin-events-and-first-party.md)'s "must surface as a named error to the dependent" is also unimplemented. Two things work:

- **Retry.** Treat `has not been declared by any plugin` as "not yet" and try again later.
- **Subscribe late**, at the point you first need the events.

Still list a real dependency: the installer reads it, so it is how the plugin gets onto the machine.

### Delivery between plugins is not lossy

`events.publish` writes to each subscriber with a blocking `write_all` into its `ChildStdin` with no timeout. Nothing is dropped and nothing counted. A subscriber that has stopped reading can fill the pipe and the publisher's `events.publish` is what waits. A subscriber that has died costs the publisher nothing. INFERRED from the code path: nobody has produced a wedged subscriber and measured it.

## Worked example: a plugin on both buses

Listens for the client launching, announces its own event, and subscribes to another plugin's. `flag-manager` is a stand-in; no plugin of that name exists in this repository.

`plugin.json`:

```json
{
  "id": "session-watch",
  "name": "Session Watch",
  "version": "1.0.0",
  "entry": "main.ts",
  "capabilities": [
    "lifecycle.read",
    "events.declare",
    "events.publish",
    "events.subscribe",
    "log"
  ],
  "dependencies": { "flag-manager": "^1.0.0" }
}
```

`main.ts`:

```ts
const enc = new TextEncoder();
const dec = new TextDecoder();

let nextId = 1;
const pending = new Map<number, (r: any) => void>();

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
      // Not awaited: onPush calls back out, and those replies arrive on this
      // loop. Awaiting here deadlocks on the first event.
      if (msg.id === undefined) {
        onPush(msg);
      } else {
        pending.get(msg.id)?.(msg);
        pending.delete(msg.id);
      }
    }
  }
})();

function call(method: string, params: unknown = {}): Promise<any> {
  const id = nextId++;
  const p = new Promise<any>((resolve) => pending.set(id, resolve));
  Deno.stdout.write(enc.encode(JSON.stringify({ id, method, params }) + "\n"));
  return p;
}

const log = (message: string) => call("log.write", { message });

const LAUNCH = "cordial/client.launch";
const SHUTDOWN = "cordial/client.shutdown";

// Filled in by declare(); never assembled by hand.
let MY_EVENT = "";

async function onPush(push: { event: string; payload: unknown }) {
  if (push.event === "cordial/init") return; // the handshake, not an event

  if (push.event === LAUNCH && MY_EVENT) {
    // Read the field where it is there and never require it.
    const profile =
      (push.payload as { profile?: string | null } | null)?.profile ?? null;
    await call("events.publish", {
      type: MY_EVENT,
      payload: { at: Math.floor(Date.now() / 1000), profile },
    });
  } else if (push.event === SHUTDOWN) {
    await log("client is going away");
  } else {
    await log(`heard ${push.event} ${JSON.stringify(push.payload)}`);
  }
}

// Confirms the grant and nothing more: delivery is gated on the capability.
const life = await call("lifecycle.subscribe");
if (life.status !== "ok") {
  await log(
    `no lifecycle events: ${life.status}` +
      (life.capability ? ` (needs ${life.capability})` : ""),
  );
}

// Declare before publishing, and take the namespaced type from the answer.
const declared = await call("events.declare", { name: "session-started" });
if (declared.status === "ok") {
  MY_EVENT = declared.result.type; // "session-watch/session-started"
  await log(`declared ${MY_EVENT}`);
} else {
  await log(
    `could not declare: ${declared.status}` +
      (declared.capability ? ` (needs ${declared.capability})` : ""),
  );
}

// Nothing starts plugins in dependency order, so the publisher may not be up
// yet. "has not been declared by any plugin" means "not yet", not "never".
async function subscribeWhenAvailable(type: string, attempts = 10) {
  for (let n = 0; n < attempts; n++) {
    const sub = await call("events.subscribe", { type });
    if (sub.status === "ok") return true;
    if (sub.status === "denied") {
      // A permission problem, which waiting does not fix.
      await log(`cannot subscribe to ${type} (needs ${sub.capability})`);
      return false;
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  await log(`${type} was never declared; giving up`);
  return false;
}

await subscribeWhenAvailable("flag-manager/profile-changed");
```
