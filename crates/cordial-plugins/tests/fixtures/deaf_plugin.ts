// A plugin that is alive, holds its stdin open, and never reads a byte of it.
//
// **The fixture the pump's tests said did not exist.** `events_subscriber.ts`
// is the closest thing there was, and it reads eagerly: measured on
// 2026-08-28, two of them took 265 queued events of 4 KiB in 35 ms, so a
// shutdown flush against them always drained and could not tell a shared
// deadline from one per plugin. Whoever stops reading has to actually stop,
// or the queue never stays full long enough to be the thing under test.
//
// Nothing here is a plugin in any other sense: it asks for nothing, answers
// nothing, and holds no capability. That is the point -- the only behaviour
// under test is what Cordial does when the far end of a pipe stops moving.
//
// A timer rather than a promise that simply never resolves: Deno ends the
// process the moment its event loop has nothing pending, and a bare
// `new Promise(() => {})` is not a pending op. The first version of this
// fixture did exactly that, exited immediately, and made the pump's write fail
// -- which is a *dead* plugin, the other case entirely, and the flush drained
// in 35 ms again. A timer is a real op, so the process stays up holding its
// stdin open, which is the state being tested. An hour is longer than any test
// here, and every caller kills it.
await new Promise((resolve) => setTimeout(resolve, 3_600_000));
