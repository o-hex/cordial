---
title: "ADR-008: Plugins are TypeScript on Deno"
---
**Status:** accepted
**Related:** [ADR-002](/adr/ADR-002-core-shell-and-ui-handoff), [ADR-003](/adr/ADR-003-plugin-isolation), [ADR-007](/adr/ADR-007-host-resources-are-brokered)

## Decision

Plugins are TypeScript, executed by Deno, in their own process. Not Luau, not
Lua, not an embedded interpreter.

## Why Deno rather than Lua

**Its permission model is the second half of the containment, and it is already
built.** Deno starts a plugin with no file, network, environment or subprocess
access. That is what makes [ADR-007](/adr/ADR-007-host-resources-are-brokered)
enforceable rather than aspirational: a plugin cannot open a socket even if the
broker had a hole in it. An embedded Lua interpreter has no equivalent — we would
be writing the sandbox ourselves, which is exactly the in-process sandboxing
[ADR-003](/adr/ADR-003-plugin-isolation) rejects as the kind that does not hold.

**TypeScript is mature and the audience already has it.** Roblox developers reach
for `roblox-ts` in numbers; the tooling, type system and editor support are not
things this project has to provide.

**Luau would be actively confusing here.** Cordial is a Roblox client that
deliberately cannot execute Lua against Roblox
([ADR-001](/adr/ADR-001-in-process-hooking)). Shipping a Lua plugin runtime would
put two Lua environments in one application, one of which is emphatically not an
executor, and every explanation of the boundary would start by disambiguating
them. A different language makes the boundary self-evident.

**Rejected: an embedded interpreter of any language.** In-process is the problem,
not the syntax. The isolation argument does not depend on which language runs
in the sandbox.

## The startup cost, measured

ADR-002 asserted that a Deno cold start needs the window of human decision time,
and built an ordering around hiding it. That assertion was never measured, and it
is wrong.

```
deno run --no-prompt --quiet hello.ts
  first run   0.14 s
  then        0.02 - 0.03 s
node hello.js 0.06 - 0.07 s
```

Deno starts in twenty to thirty milliseconds warm, and is faster than Node. The
first run pays for page cache and is still well inside a frame budget at 140 ms.

**This does not invalidate ADR-002's ordering, but it does change its
justification.** Booting the plugin host in parallel with the chooser is still
right — it is free, and it means the host is unambiguously up before anything
needs it. It is no longer *load-bearing*, and nobody should preserve that
ordering at the cost of a simpler design believing a cold start would otherwise
be visible. It would not be.

Recorded because this project's rule is that claims are worth what they were
measured with, and that applies to its own design documents.

## On ecosystem velocity

ADR-007 means a host resource Cordial does not already broker needs a change to
Cordial rather than a line in a plugin's manifest. That is slower, and the
concern that it makes for a closed ecosystem is a fair one. Three things bound
it.

**There is no faster alternative, because Flatpak permissions cannot be added at
runtime by anyone.** A plugin declaring `--filesystem=xdg-run/whatever` in its
own manifest would still require rebuilding and re-releasing the Flatpak for that
permission to exist. The choice was never "fast or slow" — it was whether the
permission set is *broad and pre-emptive* or *narrow and brokered*. Brokering
costs no velocity that the packaging format was not already charging.

**Brokers are small.** A broker is a payload type and an effect. `presence.set`
is a struct and a socket write. Adding one is a small, reviewable change, not a
redesign, and it should stay that way — if a broker starts needing a design
document, that is a sign the capability is too broad.

**The common cases should be declared up front rather than discovered one release
at a time.** Presence, desktop notifications, and opening a URL in the user's
browser are wanted by enough plugins to be worth brokering before anyone asks.
Doing that work early is what keeps "needs a Cordial release" rare rather than
routine.

The honest residue: a genuinely novel host resource will need a release, and
Cordial will sometimes be the bottleneck. That is the cost of the sandbox being
readable in one file and staying true, and it is worth paying.

## Notes moved from docs/plugins.md (2026-10-02)

**Before 0.13.1 there was no interpreter and no download row**, so on every install except a hand-built one with Deno already present, plugins were listed, granted and switched on without ever running a line. Arch is the only distribution that packages Deno, and Cordial's AUR packages depend on it; Fedora and Debian ship none, and inside the Flatpak there is no host to install one on, which is why Cordial offers to fetch a pinned release itself (checksum written into the source, verified before the file is put in place, stored under Cordial's own data directory).
