// A minimal subscriber, used by the events integration test.
//
// It asks to subscribe to a type it did not declare — proving subscribe is
// available to a plugin holding only events.subscribe — then waits for
// whatever arrives on that type and reports it back over log.write so the
// Rust side of the test can see it. Reports the subscribed and the pushed
// event through the same log.write channel flag-inspector uses, so the two
// fixtures stay consistent in style.

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
      // A push has no id at all; a reply always does. That is the whole of
      // how a plugin tells the two apart — see protocol.rs's Push type.
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

function onPush(push: { event: string; payload: unknown }) {
  // `cordial/init` is the handshake every plugin receives before it has asked
  // for anything, not an event anyone published. Ignored here so this fixture
  // stays about events; `settings.ts` is the one that checks the handshake.
  if (push.event === "cordial/init") return;
  call("log.write", { message: `push: ${push.event} ${JSON.stringify(push.payload)}` });
}

const subscribed = await call("events.subscribe", { type: "flag-manager/profile-changed" });
await call("log.write", { message: `subscribed: ${subscribed.status}` });
