// FPS Flex — take the frame-rate cap off, or put a specific one back.
//
// Roblox's Android build asks Vulkan for FIFO, which locks presentation to the
// display's refresh. On a phone that is the right answer and on a 60 Hz panel
// nobody notices. On a desktop with a faster panel it is the difference between
// the monitor you bought and the one the engine assumes you have.
//
// Cordial reads one FastFlag-layer key, `CordialPresentMode`, and hands the
// answer to `vkCreateSwapchainKHR`. This plugin sets that key. It never learns
// a swapchain exists, cannot name a mode the driver does not advertise, and
// cannot reach any other Vulkan call: the effect, never the channel (ADR-007).
//
// It also cannot overrule you. The flag layering puts the user's own
// `flags.json` above every plugin's, so a mode you set by hand wins over this
// and there is nothing this file can do about it.
//
// **Off by default**, and the reason is not caution for its own sake: uncapping
// presentation makes the GPU work as hard as it can for frames nobody asked
// for, which on a laptop is heat and battery. A plugin that shipped enabled
// would change how somebody's machine behaves without their having chosen it.
//
// ## This file did nothing at all until 2026-08-28
//
// Both of the calls it made were wrong, and neither failure was visible. It
// asked for `settings.read`, which is a *capability* name and not a method —
// the method is `settings.get` — and it sent `flags.set` a `{key, value}` pair
// when the handler requires `{values: {...}}`. So the present mode was never
// written, on any machine, for the whole time this shipped as a built-in
// plugin that Settings advertises by name.
//
// Nothing caught it because nothing ran it: the plugin tests in
// `crates/cordial-plugins` drive `host::Session`, which the client never
// constructs, and the host the client does run is
// `crates/cordial-runtime/src/plugin_host.rs`. `plugin_call_shapes.rs` now
// checks every method these shipped plugins name against the one closed table
// both hosts agree on, which is the half of this that a test can hold.

const enc = new TextEncoder();
const dec = new TextDecoder();

let nextId = 1;
const pending = new Map<number, (r: any) => void>();

// **A push is not a reply, and a dispatcher that cannot tell them apart loses
// every event.** A reply carries `status` and the `id` it answers; a push
// carries `event` and no id, so `pending.get(res.id)` on a push looks up
// `undefined`, finds nothing, and drops it silently. That is how the handshake
// below used to vanish. Copy this shape rather than the lookup-only one.
let onPush: (p: { event: string; payload: any }) => void = () => {};

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
      if (typeof msg.id === "number" && pending.has(msg.id)) {
        pending.get(msg.id)!(msg);
        pending.delete(msg.id);
      } else if (typeof msg.event === "string") {
        onPush(msg);
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

// The spellings `parse_present_mode` in `android/vulkan.rs` accepts, and the
// same set the manifest offers as a preferences page. Kept here so a value that
// somehow reached the document without going through the page is refused by
// this plugin with a list, rather than reaching the flag layer and being
// reported as an unreadable setting from somewhere the user cannot see.
const MODES = [
  "uncapped",
  "mailbox",
  "immediate",
  "fifo",
  "fifo-relaxed",
  "auto",
  "off",
];

// `uncapped` rather than `immediate`: it prefers MAILBOX, which uncaps without
// tearing where the driver offers it, and falls back to IMMEDIATE where it does
// not. One name for the intent, so this does not have to know what the surface
// advertises.
const DEFAULT_MODE = "uncapped";

// Cordial pushes `cordial/init` before the plugin has asked for anything,
// carrying this plugin's settings document and the answers to the preferences
// its manifest declares. Waiting for it costs no round trip, which is the point
// of the handshake — but waiting *forever* would mean a host that stopped
// sending it turned this plugin into a process that starts and never speaks.
// So: a short wait, then ask.
function waitForInit(ms: number): Promise<any | null> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(null), ms);
    onPush = (p) => {
      if (p.event !== "cordial/init") return;
      clearTimeout(timer);
      onPush = () => {};
      resolve(p.payload ?? null);
    };
  });
}

const init = await waitForInit(2000);
let answers = init?.preferences ?? null;

if (answers === null || typeof answers !== "object") {
  // No handshake, or a handshake that carried nothing — which is what a
  // Cordial with no open profile sends. Ask, and let the refusal be reported
  // rather than read as "the user chose the default".
  const got = await call("preferences.get");
  if (got.status === "ok") {
    answers = got.result;
  } else {
    await log(
      `could not read your preferences: ${got.status}` +
        (got.capability ? ` (needs ${got.capability})` : "") +
        (got.message ? ` (${got.message})` : "") +
        `. Using ${DEFAULT_MODE}.`,
    );
    answers = {};
  }
}

let mode = DEFAULT_MODE;
const wanted = typeof answers.mode === "string" ? answers.mode.trim().toLowerCase() : "";

if (wanted && MODES.includes(wanted)) {
  mode = wanted;
} else if (wanted) {
  // Said rather than silently corrected. A setting that looks applied and is
  // not is the failure this project keeps finding in its own code, and it is
  // no better in a plugin.
  await log(
    `mode is "${answers.mode}", which is not one of ${MODES.join(", ")}. ` +
      `Using ${DEFAULT_MODE}.`,
  );
}

// `{values: {...}}`, which is what `plugin_host.rs`'s `flags.set` requires —
// it refuses anything else with "flags.set needs a values object", and this
// file spent its entire shipped life collecting that refusal without reporting
// it, because it never checked.
const set = await call("flags.set", {
  values: { CordialPresentMode: mode },
});

if (set.status === "ok") {
  await log(
    `present mode set to ${mode}. The engine asks for FIFO; this asks Cordial ` +
      `for something else when the driver has it. Takes effect at the next launch.`,
  );
} else {
  // Including the capability, because "denied" without saying what was needed
  // is the error message somebody files a bug about instead of fixing.
  await log(
    `could not set the present mode: ${set.status}` +
      (set.capability ? ` (needs ${set.capability})` : "") +
      (set.message ? ` (${set.message})` : "") +
      `. The frame rate is whatever the engine asked for.`,
  );
}
