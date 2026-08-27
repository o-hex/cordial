// Flag Inspector — the first real Cordial plugin.
//
// It asks Cordial which FastFlags are in effect and where each one came from,
// then logs a summary. Deliberately small: the point is that the whole path
// works end to end — manifest, grant, spawn, brokered call, real data — not that
// it does anything clever.
//
// Note what it cannot do. It runs with no Deno permissions at all: no file, no
// network, no environment, no subprocess. Everything it can reach arrives over
// stdio and was checked against its grant first. It requests `flags.read` but
// not `flags.write`, so the write call below is refused — by design, as a
// demonstration.

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
      // A reply carries `status` and the `id` it answers; a push carries
      // `event` and no id. Looking every line up in `pending` discards every
      // push silently, which is a plugin that subscribes to events and never
      // hears one. This plugin subscribes to nothing, and the split is here
      // anyway because this file is the one people copy.
      if (typeof msg.id === "number" && pending.has(msg.id)) {
        pending.get(msg.id)!(msg);
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

const flags = await call("flags.list");
if (flags.status !== "ok") {
  await log(`could not read flags: ${flags.status}`);
} else {
  const entries: Array<{ key: string; value: string; source: string }> = flags.result;
  await log(`${entries.length} flag override(s) in effect`);
  for (const e of entries) {
    await log(`  ${e.key} = ${e.value}  (from ${e.source})`);
  }
}

// Requested flags.read but not flags.write, so this must come back denied.
// Included so the boundary is visible in the output rather than only in a test.
//
// `{values: {...}}` is the shape `flags.set` actually takes. The refusal would
// arrive either way -- the broker checks the capability before the handler ever
// looks at the params -- but this file is an example, and an example that
// teaches the wrong shape while still appearing to work is worse than one that
// fails. It shipped with `{key, value}` until 2026-08-28; `fps-flex` had the
// same mistake where the call was meant to succeed, and did nothing for its
// whole shipped life as a result.
const refused = await call("flags.set", { values: { FFlagAnything: "true" } });
await log(`writing a flag came back: ${refused.status}` +
  (refused.capability ? ` (needs ${refused.capability})` : ""));
