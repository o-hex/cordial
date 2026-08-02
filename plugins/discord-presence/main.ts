// Discord Presence — a first-party plugin, not a special case.
//
// It listens for client lifecycle events and keeps Discord Rich Presence in
// step with them: presence.set on launch and ready, presence.clear on
// shutdown. It requests lifecycle.read and presence.set like any third-party
// plugin would, and is granted them the same way — see ADR-006's "first-party
// plugins are still plugins": nothing here is special-cased into core.
//
// Note what it does not do. It never learns where Discord's IPC socket is,
// never opens it, and cannot send anything down it except the presence
// payload built below — Cordial owns the connection (ADR-007). It also runs
// with no Deno permissions at all, the same as every other plugin, so none of
// that containment depends on this file behaving.
//
// Two honest limitations, stated rather than hidden:
//
// - `CLIENT_ID` below is a placeholder, not a real registered Discord
//   application id. Presence will not carry Cordial's real name or icon in
//   Discord's UI until someone registers an application at
//   https://discord.com/developers/applications and this constant is
//   updated. `presence.set` will still work end-to-end with the placeholder
//   — Discord's IPC only refuses a client_id it cannot look up as an
//   application, it does not refuse for the id merely being unfamiliar.
// - `push_lifecycle`'s payload is currently empty (see host.rs): Cordial
//   does not yet thread which game or place is running through to the
//   lifecycle push, because that lives in cordial-runtime and this plugin
//   was built without touching it. The `details`/`state` text below is
//   therefore generic ("Using Cordial") rather than naming the game, and
//   should be revisited once the runtime side carries real place data.

const enc = new TextEncoder();
const dec = new TextDecoder();

// Not a real application. Replace before this plugin's presence should carry
// Cordial's actual name and icon in Discord's UI.
const CLIENT_ID = "1234567890123456";

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
      // A push (a lifecycle event Cordial sends unasked) has no id; a reply
      // to one of our own calls always does. See protocol.rs's Push type.
      //
      // Deliberately not awaited: onLifecycleEvent makes its own calls back
      // out (presence.set, log.write) whose replies arrive on this very
      // same stdin loop. Awaiting it here would block this loop from ever
      // reading those replies, so it would deadlock against itself on the
      // first lifecycle event.
      if (msg.id === undefined) {
        // `cordial/init` is the handshake every plugin is sent before it has
        // asked for anything, carrying whatever settings it had saved. This
        // plugin keeps nothing, so it only has to not mistake the handshake
        // for a lifecycle event — which it did, and reported as an
        // unrecognised event, until this line existed.
        if (msg.event !== "cordial/init") onLifecycleEvent(msg.event);
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

async function onLifecycleEvent(event: string) {
  if (event === "launch" || event === "ready") {
    const res = await call("presence.set", {
      client_id: CLIENT_ID,
      details: "Using Cordial",
      state: event === "launch" ? "Starting up" : "In session",
      start: Math.floor(Date.now() / 1000),
    });
    await log(`presence.set on ${event} came back: ${res.status}`);
  } else if (event === "shutdown") {
    const res = await call("presence.clear");
    await log(`presence.clear on shutdown came back: ${res.status}`);
  } else {
    await log(`ignoring unrecognised lifecycle event ${JSON.stringify(event)}`);
  }
}

const subscribed = await call("lifecycle.subscribe");
await log(`lifecycle.subscribe came back: ${subscribed.status}`);
