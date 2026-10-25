---
title: "ADR-026: Cordial publishes what it observes, and plugins may never veto it"
---
**Status:** accepted
**Related:** [ADR-001](/adr/ADR-001-in-process-hooking), [ADR-003](/adr/ADR-003-plugin-isolation), [ADR-006](/adr/ADR-006-plugin-events-and-first-party), [ADR-007](/adr/ADR-007-host-resources-are-brokered)

## Decision

Cordial publishes the facts it observes at the platform boundary as **core
events**, on the registry ADR-006 already built, under the reserved `cordial/`
owner.

Three rules, and the first is the one the others exist to protect.

1. **A core event is an observation and is never cancellable.** Delivery is
   one-way, the return value is nothing, and no plugin response can change what
   the client does.
2. **Each event family is gated by its own capability**, from a closed table. An
   event absent from the table reaches nobody.
3. **Delivery is lossy and the loss is counted.** Publishing never blocks on a
   plugin.

## Why Cordial can do this without hooking anything

Roblox's build runs on a bionic linker this project ported, against a libc shim
it wrote, on a JNI VM standing in for ART, talking to a framework layer that
answers every call the client makes into the platform. The looper it polls is
ours. The surface it draws to is ours. Its sockets, paths, audio streams and
input events all cross a boundary this codebase owns.

So there is nothing to patch, trace or intercept. Everything a core event
reports was already coming through Cordial on its way in, and reporting it is
Cordial describing what it was asked to do. That is why this is compatible with
ADR-001, which forbids in-process code execution against the Roblox process
permanently and absolutely.

**This is a stronger position than the modding buses it resembles.** NeoForge
and its relatives have to inject themselves into the program they observe.
Cordial is underneath it already.

## Why events are not cancellable, which is the whole of the design

The obvious next step from "a plugin can hear it" is "a plugin can handle it",
and every modding bus worth copying has cancellable events. They are cancellable
because mods exist to change the game.

**A plugin here cannot change the game, and a cancellable platform event is that
prohibition defeated by a callback.** A plugin returning "handled" from a socket
event can cut the engine's network. One vetoing a path event can deny it a file.
One vetoing an input event is a macro engine. None of that is less of an
in-process control surface for being reached through a callback rather than a
patched instruction, and the distinction would not survive contact with anybody
who wanted to abuse it — which is the test ADR-001 sets for itself: *not
disabled, absent, so there is no primitive to extract or re-enable in a fork.*

A plugin that wants an effect asks for one through a capability Cordial performs.
That is ADR-007, unchanged, and it is a different sentence from "a plugin may
stop the client doing something".

**Cordial's own decisions are explicitly not covered by this.** Whether
*Cordial* shows a toast, or opens a URL in its own web view, is not the engine's
behaviour and could sensibly be influenced one day. If that is ever built it
wants its own name and its own ADR, so that nobody reaches for it as a way to
make platform events vetoable after all.

## Why a capability per family, not one for the bus

`lifecycle.read` gates the events that exist today, which are launch, ready,
shutdown, engine version and window size — all unremarkable.

The events worth adding next are not unremarkable. They are the ones Cordial is
uniquely placed to see: which paths the engine opened, which addresses it
connected to, what was typed into a focused field. **Nobody should receive those
because they were once granted `lifecycle.read` to show a Discord status.** A new
family gets a new permission, and the mapping is a closed table rather than a
prefix convention, for the reason `protocol::required_capability` already gives
about methods: a typo must fail as unknown rather than fall through to a check
that happens to pass.

An event with no entry in the table requires a capability nobody holds, so it
reaches no one. That is the safe direction for a name somebody added and forgot
to gate, and there is a test for it.

## Why delivery may be dropped

A push is a blocking write into a plugin's stdin. A plugin that stops reading
fills the pipe — 64 KiB on Linux — and then whoever published waits.

For a platform event that publisher is a thread the client is waiting on. The
engine's looper runs at millions of polls a second and cannot queue behind a
plugin; a bus that let it would be a worse bug than anything it was built to
observe.

So each plugin has a bounded queue and its own writer thread, and a publish that
finds the queue full drops the event and counts it. Dropping is the honest
outcome for an observation — there is no correct way to make the client wait for
a plugin to catch up — and the count is what stops it being silent, which is the
same rule `native/opensles.cpp` follows in reporting failure rather than handing
back a dead engine object.

Measured: 4000 events of 4 KiB each published in about 6 ms, 3735 of them
dropped and counted, with the consumer far behind. The publisher's cost does not
track the reader's speed.

**Requests and their responses do not go through this and are not lossy.** An
answer nobody receives is a plugin hung waiting for it.

## Consequences

**Plugins may miss events, and must be written knowing it.** A plugin that
counts things will undercount under load. A plugin that needs exactly-once
delivery cannot have it here and should be asking Cordial for state instead.

**The table is the security surface.** Adding an event is adding a thing some
plugin can learn; the review question is not "is this useful" but "who should be
allowed to know it, and does that family already have the right capability".

**Nothing here gives a plugin a UI.** A widget handle is a channel in ADR-007's
sense — a plugin holding one can walk to its parent, its window, the
application — so UI stays a set of named effects Cordial performs, as
`notify.send` already is.

## What would change this

A demonstrated need for a plugin to influence Cordial's *own* behaviour, which
is a different ADR and must not be reached by widening this one. Or evidence
that lossy delivery is losing something that matters, which would argue for a
deeper queue or a durable side channel for one specific family — not for making
the client wait.

## Notes moved from docs/plugin-api.md (2026-10-02)

### A core event may be observed and never vetoed, delayed or altered

ADR-026 states the rule; the enforcement is structural rather than a check
somebody remembered to write.

**There is no channel to answer on.** Delivery is a `Push`, and a `Push` has no
`id`. A `Response` is only ever matched to a request by its `id`, so a plugin
has nothing to correlate a reply to. `publish_core` returns
`Delivered { sent, dropped }` — two counts for the publisher — and does not
read the plugin's stdout at all. There is no return value to make meaningful,
which is a stronger guarantee than a return value that is documented as ignored.

**Nothing waits.** `publish_core` hands each recipient's event to that plugin's
own `Pump` via `offer`, which is a `try_send` on a
`std::sync::mpsc::sync_channel` 256 deep. `try_send` never blocks. A separate
thread per plugin does the actual write into that plugin's stdin. So the
publisher's cost is a queue push, and it does not track how fast — or whether —
the plugin reads.

That matters because a push is a blocking write into a pipe, 64 KiB on Linux,
and the thread publishing a platform event is a thread the client is waiting
on. The engine's looper is measured in millions of polls a second. A bus that
let it queue behind a wedged plugin would be a worse bug than anything it was
built to observe.

**Cordial's own decisions are explicitly outside this rule.** Whether *Cordial*
shows a toast or opens a URL in its web view is not the engine's behaviour and
could sensibly be influenced one day. If that is ever built it gets its own
name and its own ADR, so nobody reaches for it as a way to make platform events
vetoable after all.

### What happens when a subscriber is slow

It misses events, and the loss is counted rather than silent.

- The queue is `QUEUE_DEPTH = 256` pushes deep, per plugin.
- A publish that finds it full **drops the event** and increments that plugin's
  drop counter. The plugin is not told, and there is no sequence number in a
  `Push` from which it could infer a gap.
- `plugin_host::dropped_core_events()` returns `(id, count)` for every plugin
  that lost something, and **the client calls it at exit**, printing
  `  plugin <id>: <n> core event(s) dropped, its queue was full` for each. A
  plugin that has already exited is not listed — its listener went with it — so
  "no drops reported" is a weaker claim than it looks for a plugin that died
  mid-run. (`Session::dropped_by_plugin()` is the same thing on the host nobody
  runs, and has no non-test caller.)
- `plugin_host::flush_core_events(limit)` waits, bounded, for queued pushes to
  reach every plugin. `limit` is applied **per plugin**, not to the call as a
  whole, so the worst case is `limit` multiplied by the number of running
  plugins. The client calls it with 500 ms immediately after publishing
  `client.shutdown`: delivery is asynchronous, so a publish followed by an exit
  is a race the exit wins, and that is the one event whose whole point is being
  last. It returns `false` if a deadline passed first, and the client then
  prints `  plugins: a plugin did not read its queue within 500 ms; exiting
  without it`, because a plugin that stopped reading must not be able to hold up
  the exit.

Measured, in `host.rs`'s own test: 4000 events of 4 KiB each published in about
6 ms, 3735 of them dropped and counted, with the consumer far behind. That test
is honest about what it does *not* prove — the fixture process exits rather
than wedging, so what is demonstrated is that the publisher's cost does not
track the reader's speed, not that a genuinely wedged consumer was survived.

