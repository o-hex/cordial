# ADR-003: Plugins have no memory access to Cordial

**Status:** accepted
**Supersedes:** nothing
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-002](ADR-002-core-shell-and-ui-handoff.md)

## Decision

Plugins run in their own address space. They cannot read or write Cordial's
memory, and there is no API by which they could ask to. The capability broker is
the entire surface between a plugin and the core.

This is not a default that a manifest key can turn off. There is no
`cap:core.memory.*`, in the same way and for the same reason that there is no
`cap:core.process.spawn` (ADR-002): a capability that hands over the machine is
not a capability, it is the absence of one.

## Why

**Otherwise the capability system is decorative.** Capabilities are worth
declaring only if declaring less means being able to do less. A plugin granted
nothing but `cap:core.window.title`, but able to reach into the core's memory,
can do everything the core can do — rewrite the broker's own allow-lists
included. Every other permission in the system becomes a suggestion, and the
manifest becomes documentation rather than enforcement.

**It would break the recoverability property ADR-002 depends on.** That ADR
splits the core shell from the UI so that a failing UI plugin can be restarted
without losing the session. That only holds if a plugin's failures are confined
to its own address space. With shared memory, any plugin bug is a core bug: a
stray write corrupts the core's heap, and the crash surfaces somewhere else
entirely, arbitrarily later. The debugging cost of that failure mode is paid by
Cordial, and the user experiences it as "Cordial is unstable", not as "that
plugin is broken".

**It is ADR-001's principle applied inward.** ADR-001 rejects in-process code
execution against the Roblox process — no hooking, no memory patching, no
injected script environment. Granting third-party plugins that same power over
Cordial would be inconsistent, and worse: plugin code is *ecosystem* code,
installed casually and in volume, from many authors of unknown intent. It
warrants more isolation than we grant ourselves, not less.

**Process isolation is the only kind that actually holds.** In-process
sandboxing of native plugins — a restricted API surface, a scripting runtime, a
"please don't" in the docs — is a boundary only as strong as the absence of bugs
in it. An address-space boundary is enforced by the MMU and does not depend on
Cordial being correct.

## Consequences

- Plugins are separate processes and communicate over IPC. The broker mediates
  every call; there is no fast path that bypasses it.
- Anything a plugin needs to observe about core state must be an explicit,
  named capability with an explicit payload. "Read this struct" is not available
  as a shortcut, so each such need becomes a deliberate API decision.
- Bulk data (frames, textures, large buffers) needs an explicit shared-memory
  transport if it is ever required — negotiated per use, scoped to a specific
  buffer, and granted by capability. That is a *narrow, named* sharing of one
  region, not access to Cordial's address space, and it does not weaken this
  decision. It is called out here so it is designed deliberately rather than
  arrived at by erosion.
- IPC costs latency and serialisation. This is accepted. The cold-start argument
  in §5 already assumes the plugin host is a separate thing being warmed in
  parallel, so the architecture is priced for it.

## What would change this

Nothing about performance. If an interaction is too slow across IPC, the answer
is a better-shaped capability — coarser calls, batched payloads, a negotiated
shared buffer for the specific data — not a hole in the boundary. A plugin API
that is fast because it is unsafe is not a plugin API; it is a patch loader with
a manifest file.

## Notes moved from docs/plugin-api.md (2026-10-02)

### The grants file fails closed; the enablement file does not

Its sibling `plugin-enabled.json` fails in the other direction, though not
quite as either the file or its own comment claims. An unreadable or malformed
enablement file reads as "no opinions recorded" and prints `plugins: <path> is
not usable (<error>); treating every plugin as enabled` — but `is_enabled` then
falls back to `enablement::default_for`, which returns **false** for any id in
`SHIPS_DISABLED`, today `fps-flex`. So a first-party plugin that ships switched
off stays off, and both that message and the module comment above
`enablement::load` ("an enablement file that will not parse enables everything")
are stale in the source as well as in this paragraph's first draft. The
asymmetry they are reaching for is real and is deliberate: the grants file is
the thing that decides what a plugin *can do*, and it is the one that has to
fail closed.

### Why there is no "anything" capability, and what an unknown name does

There is no capability meaning "anything", and no grants entry that means "all".
Those are two headers, each giving its own reason. `capability.rs` says the list
is closed and cites ADR-003 — a capability handing over the machine is not a
capability but the absence of one, which is why there is no `process.spawn`, no
filesystem path and no memory access in the enum. `grants.rs` says a "grant
everything" entry would be the one line anybody pastes from a forum.

An unrecognised capability name is an error in both files, and the two errors
have different blast radii. A manifest naming one fails to load with `unknown
capability "process.spawn"`, which affects that plugin. A grants file naming one
grants **nothing to anybody**: `grants::parse` returns `Err` on the first
unknown name and abandons the whole document, so `load` takes exactly the
malformed-file path above with a more specific reason inside the parentheses —

```text
plugin grants: <path> is not usable (unknown capability "process.spawn" granted to "x"); granting nothing
```

Skipping the name quietly would mean granting less than you believe you granted,

### The install consent prompt, granting, revoking and switching off

The install prompt is all-or-nothing. `consent::verdict` decides whether to ask
at all: a plugin with no entry module *and* no capabilities installs silently,
because if every import prompts then the prompt means nothing by the third one.
Anything with code or capabilities gets a dialog listing each capability's
consequence sentence, and pressing Allow writes **every requested capability**
into the grants file at once. "Not now" is both the default response and the
close response, so dismissing the dialog with Escape grants nothing.

Allowing is still not starting. `consent::starts_disabled` returns whether the
plugin has an entry module, and the shell writes a plugin with code into
`plugin-enabled.json` as **off** whatever you told the dialog — the success
subtitle says so: "Installed {name} and allowed what it asked for. It is
switched off until you turn it on." An install dialog's OK button granting the
capabilities *and* starting the process would be one act where there should be
two. Data-only plugins are left absent from the file and therefore on, because
there is nothing to start and a switch with no argument behind it is not a
choice.

Per-capability control comes afterwards, on the plugin's row in Settings: one
switch per capability the plugin requested, each writing through `grants::set`,
which flips one entry and leaves every other plugin — and every other capability
of this one — alone. Revoking a plugin's last capability drops its key from the
file entirely rather than leaving `"id": []`, since an empty set and an absent
key mean the same thing to both `load` and the broker.

**Disabling is not revoking.** `plugin-enabled.json` sits beside the grants file
and answers a different question: is this thing running, as against what it is
allowed to do. Grants survive a disable untouched, so switching a plugin off for
an afternoon costs nothing to undo. Conflating them would mean the price of
turning something off is every approval decision you already made, and the likely
response to that price is leaving a suspect plugin enabled. The Settings subtitle
says so out loud, when there is anything to say it about: `capability_summary`
returns "Off. What you allowed it to do is kept." for a disabled plugin that
holds at least one granted capability, and the bare "Off" for one that holds
none — the sentence exists to answer a fear about losing approvals, and a plugin
with no approvals has none to lose.

Two states look identical from outside and are not, so Cordial reports them
differently. A plugin with code that has been granted nothing is not started at
all, and says `no capabilities granted, not started`; a plugin the user switched
off says `disabled in Settings, not started`. One is "you have not decided what
to allow", the other is "you switched it off", and a plugin installed, enabled,
and never granted anything has been reported as broken by somebody looking at
ADR-003's default deny working exactly as intended.

Whatever is withheld is named at startup, too:

```text
plugin <id>: not granted flags.write, presence.set
```

because a plugin silently doing less than it asked for is otherwise
indistinguishable from a plugin that is broken.

One thing the handshake does *not* do is enumerate your grants. `cordial/init`
carries `settings` and `preferences`, and each is `null` when `settings.read`
was not granted and `{}` when it was granted but nothing is saved yet — the two
are told apart deliberately, so a first launch is distinguishable from a missing
capability. (`settings` is also `null` when the session has no profile at all,
or when the read itself failed; each of those prints a line saying which.) There
is no "here is what you hold" field; you learn what you hold by calling and
