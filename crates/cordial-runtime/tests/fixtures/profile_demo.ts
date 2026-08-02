// A plugin that only remembers something, used by the profile configuration
// test. Copied into a scratch plugin directory by that test rather than shipped
// under plugins/, because it is a fixture and not an example.
//
// It records what the handshake gave it and how many times it has been started,
// then exits. The test reads that document back out of the profile.

const enc = new TextEncoder();
const dec = new TextDecoder();

let nextId = 1;
const pending = new Map<number, (r: any) => void>();

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

// Raced against a timeout so a handshake that never arrives fails the test with
// a readable document rather than hanging it.
const handshakeSaw = await Promise.race([
  handshake,
  new Promise((r) => setTimeout(() => r("no handshake arrived"), 5000)),
]);

const previous = (handshakeSaw as { launches?: number })?.launches ?? 0;
await call("settings.set", { settings: { handshakeSaw, launches: previous + 1 } });
Deno.exit(0);
