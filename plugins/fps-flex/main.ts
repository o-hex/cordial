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
      const res = JSON.parse(line);
      pending.get(res.id)?.(res);
      pending.delete(res.id);
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

// The spellings Cordial's own parser accepts. Kept here so a typo in settings
// is refused by the plugin with a list, rather than reaching the flag layer and
// being reported as an unreadable setting from somewhere the user cannot see.
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

const settings = await call("settings.read");
let mode = DEFAULT_MODE;

if (settings.status === "ok" && settings.result && typeof settings.result.mode === "string") {
  const wanted = settings.result.mode.trim().toLowerCase();
  if (MODES.includes(wanted)) {
    mode = wanted;
  } else {
    // Said rather than silently corrected. A setting that looks applied and is
    // not is the failure this project keeps finding in its own code, and it is
    // no better in a plugin.
    await log(
      `settings.mode is "${settings.result.mode}", which is not one of ` +
        `${MODES.join(", ")}. Using ${DEFAULT_MODE}.`,
    );
  }
}

const set = await call("flags.set", { key: "CordialPresentMode", value: mode });

if (set.status === "ok") {
  await log(
    `present mode set to ${mode}. The engine asks for FIFO; this asks Cordial ` +
      `for something else when the driver has it.`,
  );
} else {
  // Including the capability, because "denied" without saying what was needed
  // is the error message somebody files a bug about instead of fixing.
  await log(
    `could not set the present mode: ${set.status}` +
      (set.capability ? ` (needs ${set.capability})` : "") +
      `. The frame rate is whatever the engine asked for.`,
  );
}
