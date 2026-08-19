# Security policy

## Reporting

Report security issues through GitHub's private vulnerability reporting on this
repository ("Security" → "Report a vulnerability"). Please do not open a public
issue for anything exploitable.

Include what you did, what happened, and what you expected. A reproduction that
someone else can run is worth more than a description, and this project's whole
method is that claims are verified by running them.

## What counts

Cordial runs a large proprietary binary inside a runtime it supplies, and hosts
plugins alongside it. In scope:

- **Sandbox escape from a plugin** — a plugin reaching Cordial's memory, the
  Roblox process, or the host beyond its granted capabilities. Plugins are
  isolated by process ([ADR-003](docs/adr/ADR-003-plugin-isolation.md)); a way
  around that is the most serious class of bug here.
- **Capability broker bypass** — obtaining an effect without the capability that
  should gate it.
- **Anything in the runtime that lets untrusted content reach the host** — path
  traversal out of the asset tree, the `/system` redirect
  (`native/system_paths.cpp`) resolving somewhere it should not, or the flag
  layering reading a file it should not.
- **Memory-safety bugs in Cordial's own `unsafe` code**, of which there is a
  great deal: the linker bindings, the bionic shims, and the libc interposers
  in `native/`.

## What does not

- **Crashes in Roblox's own code.** The engine is not ours and most crashes are
  Cordial handing it something malformed. Those are ordinary bugs — open a
  normal issue.
- **The ban risk from using a third-party client.** That is documented in the
  README, is inherent to what this is, and is not a vulnerability.
- **Requests for an exploit surface.** Cordial deliberately has no script
  execution, no hooking and no asset override
  ([ADR-001](docs/adr/ADR-001-in-process-hooking.md),
  [ADR-004](docs/adr/ADR-004-plugin-asset-overrides.md)). "Cordial cannot cheat"
  is the design, not a bug.

## Forks, and clients built on Cordial

Cordial does not support script execution, exploiting, botting or
multi-accounting, and it will not. That is not a gap waiting to be filled.
[ADR-001](docs/adr/ADR-001-in-process-hooking.md) makes in-process hooking,
memory patching and injected script environments **absent** rather than
disabled — there is no primitive here to switch on, and no API by which a plugin
could ask for one. [ADR-003](docs/adr/ADR-003-plugin-isolation.md) is why a
plugin never receives a socket, a file descriptor or a connection: it sends a
payload and Cordial performs the effect.

**Cordial is GPL-3.0, so anyone may fork it, including in directions we
disagree with.** That is the licence working as intended and we are not going to
pretend otherwise. It does mean:

- A fork is an independent project. It is not endorsed by us, not affiliated
  with us, and not supported here.
- If you are using something built on Cordial that adds script execution, **you
  are not using Cordial**, and this issue tracker cannot help you. We do not
  know what that fork changed and we cannot reason about its behaviour.
- Upstream will not accept commits that enable exploiting. Contributing here is
  not a route to getting one merged. Nothing happens *to* you for having
  contributed — this is a statement about patches, not about people.

If you are considering using such a fork, understand what you are accepting.
Roblox's enforcement is automated, runs in waves, and associates accounts sharing
an address. A fork that adds an exploit surface does not carry the risk alone;
it carries it into every account on your network.

## WSL is not a supported target

Running Cordial under WSL sidesteps client integrity checks, and that is the
reason it is unsupported rather than merely untested. We will not help with WSL
issues and will not take patches that exist to make that path work.

This is not a judgement about Windows. It is that the value of running there is
mostly the evasion, and building for it would make this project a tool for
something it has said it is not.

## Expectations

This is a hobby project with no funding and no on-call. There is no bounty and
no response-time guarantee. It is also two days old and largely written by an AI
with a human directing architecture — read the disclosure in the README — so
treat its security posture as unproven rather than assumed.
