# ADR-001: In-process hooking of the Roblox process

**Status:** Rejected
**Date:** 2026-07-31
**Supersedes:** nothing
**Related:** architecture spec §1.2, §4, §7.3, §9b

---

## Context

During architecture design, an in-process hooking facility was considered: executing
Cordial-controlled code inside the Roblox process in order to patch its behaviour. Two
things motivated it.

1. **Compatibility gaps.** Roblox's Android build behaves in ways that are wrong on a
   desktop — passkey/WebAuthn authentication fails, the client believes it is on mobile,
   the communities view opens as a separate window. Patching the client is one way to fix
   these.
2. **Plugin reach.** An in-process extension point would let plugins modify the running
   game — arbitrary UI changes, in-client overlays, behaviour the client does not expose a
   toggle for. It is the most powerful extension surface available, and it is what
   "extensible Roblox client" suggests to most people.

The question was not whether it is possible. It is possible. The question was whether it
should exist in Cordial's binary at all.

## Decision

**Not implemented.** Compatibility gaps are solved at the framework layer (spec §4)
instead. The plugin system never gains a capability that references the Roblox process's
memory or code.

This is not a default-off feature, a first-party-only feature, or a flag. There is no
injection primitive in the binary, no capability in the vocabulary that names one, and no
placeholder for one.

## Reasoning

### In-process enforcement is self-defeating

Any code with enough authority to patch a function has enough authority to patch the code
that would police, unload, or watermark it. A hooking facility that also polices its own
use is asking the fox to audit the henhouse from inside the henhouse.

The general principle, stated in spec §7.3: **enforcement must live outside the boundary
it enforces**, and nothing lives outside a process from that process's own perspective.
Cordial's other boundaries work precisely because core sits outside them — core closes a
portal grant, core kills a sandboxed process, core stops answering a plugin's bus
messages. None of those mechanisms have an in-process analogue, because there is no
"outside" to put them in.

So an in-process facility could be offered, but it could not be *governed*. Shipping a
capability that cannot be enforced is worse than not shipping it: it converts a hard
guarantee into a promise, and users cannot tell the difference until it fails.

### There is no root of trust

A trustworthy client-side integrity signal requires something the user does not control —
TPM attestation, secure boot, a kernel component. Cordial runs on a machine its user owns
entirely. That foundation is simply absent.

Consequently any "this client is modified" marker is removable by exactly the actors who
would most want to remove it. It marks honest users, who leave it in place, and misses
dishonest ones, who strip it in an afternoon. The signal does not merely fail — it
inverts, becoming actively misleading to anyone who consumes it. This is why spec §1.2
also rules out client-side integrity flags and watermarks generally; this ADR is the
same argument applied to the feature that would have needed them most.

### Restart is the only integrity boundary

In-process state can only be guaranteed at process start, because a fresh process starts
from a known-good state — the previous resident code is gone. Once code is resident and
holds authority, nothing outside can make it relinquish that authority; it can only be
asked, and asking is not enforcement.

This makes mid-run revocation of an in-process capability unenforceable, which puts it in
a different class from every other capability Cordial grants. Portal capabilities are
revoked by closing the grant at the source. Deno sandbox flags are revoked by terminating
and respawning the process. Core capabilities are revoked by core declining to answer.
All three are performed *by core, from outside*, and the plugin's cooperation is never
load-bearing (spec §7.2: the event is a courtesy, the kill is the enforcement). An
in-process capability would have no equivalent, and would silently be the one permission
in the system that a revocation UI could not actually revoke.

### The feature that motivated it does not need it

Passkeys are a framework-API implementation, not a patch. Roblox calls
`androidx.credentials.CredentialManager`; that call crosses a JNI boundary that is
*already Cordial's own code*, because the framework layer must exist for the app to run at
all. Implementing the API beneath the app is strictly less work than patching the app —
it is the layer Cordial is already building, it survives Roblox updates because the
Android API is stable while binary offsets are not, and it requires no offset database
and no per-release maintenance (spec §4.1, §4.3).

This is the Wine relationship: Wine does not patch Windows applications, it implements the
API beneath them. Every other motivating gap resolves the same way — desktop
identification via system properties and `Build.*` values, the communities window via
activity and window-lifecycle stubs, graphics and quality settings via FastFlags the
client already reads, multi-instance via namespace and data-directory separation.

The motivating feature list turned out to be an argument *for* the framework layer, not
for hooking.

### Ecosystem risk

Shipping an in-process execution surface on a Roblox client is indistinguishable in effect
from shipping an executor, whatever the stated intent and whatever restrictions are
declared around it. The capability is the artifact; the policy around it is not.

The consequence is not borne by this project alone. Roblox tolerating the
Android-on-desktop pathway is what makes Linux play possible for everyone using it, and
that tolerance is contingent. This is the same reasoning that keeps Sober closed-source,
and it is the reason spec §1.2 frames the protection as *the absence of the primitive*
rather than secrecy or restriction: a restriction can be removed in a fork, but a
primitive that was never built cannot be extracted from a binary that does not contain it.

That property is worth more than any feature it costs.

## Consequences

**Accepted:** full-arbitrary client-UI modification is not offered. A plugin cannot strip
in-game HUD elements Roblox provides no toggle for, cannot redraw the client's own
interface, and cannot alter game behaviour. Users who want that will not get it from
Cordial.

Everything in the shipped feature set (spec §9b) is achievable without it: launcher
replacement, FastFlags, UI themes, Discord Rich Presence, external tool integrations,
multi-instance, join notifications. The parity target is met.

**Also accepted:** if a future compatibility gap appears to need in-process access, that is
a signal the framework layer is incomplete — not a signal to revisit this decision. Fix it
at the framework layer.

**Amendment, 2026-08-19:** "no primitive in the binary" turned out to have a gap that had
nothing to do with the plugin system. `third_party/mcpelauncher-linker`'s port of bionic's
`ElfReader::LoadSegments()` mapped every `PT_LOAD` segment — including the engine's text —
with `prot | PROT_WRITE` unconditionally, and the counterpart that is supposed to strip that
back off after relocation (`phdr_table_protect_segments()`) turned out to have no call site
reached on `__LP64__` at all: both places that call it are wrapped in `#if !defined(__LP64__)`
or its own `#if 0`-gated body, legacy scaffolding for the 32-bit `DT_TEXTREL` case. The result,
observed with `tools/engine-text-diff.py` against a live `cordial-run`, was the engine's ~112 MB
text mapping sitting `rwxp` for the whole process lifetime — not patched (0 differing bytes
against the file), but writable the entire time regardless.

That is exactly the foothold this ADR argues against handing over: `mprotect` is the step a
hooking primitive needs before it can patch anything, and an `rwxp` engine mapping does that
step for free, pre-armed, for any code already running in the process — including a plugin
that found its own way in through some future bug, and including any fork that adds the
executor this ADR was written to keep out. "The primitive is absent" is a claim about the
loader's behaviour as much as about the plugin API, and the loader was not living up to it.

Fixed by mapping each segment at its real, described protection from the start (matching what
upstream bionic actually does outside the 32-bit-legacy path), rather than forcing writability
that nothing later revokes. Verified against Roblox's own x86-64 build: no `DT_TEXTREL`, and
its 546 `.rela.plt` relocations all target the writable data segment, never text, so there is
no relocation-time need for the write bit on that mapping at any point. Confirmed with the
tool: `rwxp` before, `r-xp` after, `DIFFERING BYTES: 0` in both, and the client still reaches
the landing page on repeated runs. This does not change the decision above — it was already
the decision — it closes a gap between what the ADR claimed and what the binary did.

## Revisit criteria

If Roblox ships an official, sanctioned client-extension mechanism, revisit — but as a
**new** design against whatever interface they provide, with its own ADR. Do not resurrect
this approach.

Nothing else reopens this. In particular, neither a popular feature request, a
demonstration that it can be done safely in some narrow case, nor a first-party-only
restriction changes any of the reasoning above.
