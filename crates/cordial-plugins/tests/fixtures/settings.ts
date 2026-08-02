// A plugin that keeps something between launches, used by the settings
// integration test.
//
// It reports three things back over log.write, each under a field name of its
// own choosing so the Rust side has to look at the payload rather than at the
// shape of the exchange: what the handshake handed it, what came back when it
// asked for somebody else's settings, and what it read back after saving.
//
// The middle one is the point. It asks for `neighbour`'s document by every
// field name a settings API might plausibly have taken, and Cordial must
// answer with this plugin's own — the id is not a parameter these methods
// have.

const enc = new TextEncoder();
const dec = new TextDecoder();

let nextId = 1;
const pending = new Map<number, (r: any) => void>();

// Resolved by the stdin loop when the handshake lands, and raced against a
// timeout below so that a handshake which never arrives fails the test with a
// readable answer rather than hanging it.
let handshakeArrived: (settings: unknown) => void;
const handshake = new Promise<unknown>((r) => (handshakeArrived = r));

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
      if (msg.id === undefined) {
        if (msg.event === "cordial/init") handshakeArrived(msg.payload.settings);
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

const handshakeSaw = await Promise.race([
  handshake,
  new Promise((r) => setTimeout(() => r("no handshake arrived"), 2000)),
]);

const nosey = await call("settings.get", {
  plugin: "neighbour",
  id: "neighbour",
  plugin_id: "neighbour",
});

await call("settings.set", { settings: { panel: "flags", opened: 4 } });
const readBack = await call("settings.get");

await call("log.write", {
  message: JSON.stringify({
    handshakeSaw,
    askingForNeighbourReturned: nosey.result ?? nosey.status,
    afterSaving: readBack.result,
  }),
});
