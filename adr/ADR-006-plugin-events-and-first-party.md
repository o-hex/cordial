---
title: "ADR-006: Plugins may declare their own events, and some plugins ship with Cordial"
---
**Status:** accepted
**Related:** [ADR-002](/adr/ADR-002-core-shell-and-ui-handoff), [ADR-003](/adr/ADR-003-plugin-isolation), [ADR-005](/adr/ADR-005-flag-service)

## Decision

Two things, which are separable but arrive together.

**1. A plugin may declare event types and broadcast on them**, behind two new
capabilities:

| | |
|---|---|
| `events.declare` | register event types under the plugin's own namespace |
| `events.publish` | broadcast on an event type the plugin declared |
| `events.subscribe` | receive events, including ones other plugins declared |

A plugin may only publish on types it declared itself. Namespacing is by plugin
id and is not optional: `flag-manager/profile-changed`, never `profile-changed`.

**2. Some plugins ship with Cordial as first-party**, installed by default and
loaded on demand when something depends on them. They are ordinary plugins —
same manifest, same capability grants, same isolation — and they are visible and
individually disableable in settings.

## Why plugin-declared events

**Because the alternative is a core that grows a case for every plugin.** A
multi-instance plugin wants to say "an instance started"; a launcher wants to
hear it. If every such pair needs a new core event type, the core's event
vocabulary becomes a list of whatever plugins happened to exist, and every new
integration is a core change. Letting plugins declare their own keeps the core's
vocabulary about *Cordial*, and lets the ecosystem grow sideways.

**Declaration is separate from publication on purpose.** A plugin that can
publish on any string can impersonate another plugin's events, and a subscriber
has no way to tell. Declaring first, under a namespace derived from the plugin
id rather than chosen by the plugin, makes the origin of an event a fact rather
than a claim.

**Subscription is deliberately broader than publication.** Hearing an event tells
you something happened; publishing tells everyone else something happened. Those
are different powers and it would be a mistake to grant them together — a plugin
that only reacts should not have to be trusted to speak.

## Why first-party plugins are still plugins

**Because "built in" and "a plugin" are not opposites, and pretending otherwise
costs the architecture.** The moment a behaviour is implemented in core because
it ships by default, it stops being subject to the capability model, stops being
inspectable the way a plugin is, and stops being removable. Cordial would then
have two kinds of behaviour with two sets of rules, and the interesting one would
be the one users cannot see.

Keeping them as plugins means the default feature set is *an example of the
plugin API being sufficient*. If a first-party plugin needs something the API
cannot express, that is a signal the API is incomplete — exactly the argument
[ADR-001](/adr/ADR-001-in-process-hooking) makes about the framework layer.

**Loaded on demand, not eagerly.** A first-party plugin that nothing depends on
should not be running. Resolution is by dependency: a plugin declares that it
needs `cordial/multi-instance`, and that plugin starts if it is not already up.
This is the same shape as an ESM import graph and it should behave like one —
resolved once, shared, not restarted per dependent.

**On by default is a defaults question, not an architecture question.** Some
first-party plugins will be on out of the box because the client is worse without
them. That is a setting, and settings are reversible. What must not happen is a
plugin that cannot be turned off being described as a plugin.

## Consequences

**Accepted:** the event registry is a real runtime object with ownership rules,
not a hashmap of strings. It has to record which plugin declared each type,
refuse a publish from anyone else, and survive a plugin restarting without
letting a different plugin claim its namespace in the gap.

**Accepted:** a first-party plugin ships in the repository and is covered by its
tests, so the plugin API is exercised by Cordial's own CI rather than only by
third parties.

**Accepted:** dependency resolution introduces a failure mode the current host
does not have — a plugin that depends on one that will not start. That must
surface as a named error to the dependent, not a silent absence, for the same
reason `denied` and `error` are distinct in the protocol.

**Rejected:** letting plugins publish on core event types. Core events describe
what Cordial did; a plugin claiming Cordial did something it did not is a
correctness problem for every subscriber. Plugins declare their own or say
nothing.

**Resolved:** the registry filters at subscribe time, not on receipt. Since
declaring already has to record who owns each type to authorise `publish`,
`subscribe` reuses that same lookup rather than making every plugin carry
its own filter list and having `publish` consult all of them. The cost is
that `subscribe` refuses a type nobody has declared yet, rather than parking
the subscription to match something that shows up later — a subscriber that
starts before its dependency has to wait for it, which is the dependency
resolution this ADR already describes for first-party plugins ("resolved
once, shared, not restarted per dependent"), not a new problem. See
`crates/cordial-plugins/src/events.rs`.

## What would change this

If the event registry turns out to be a way for plugins to fingerprint each
other — learning what is installed by watching what is declared — subscription
would need to be scoped to declared dependencies rather than open. That is worth
checking before the first plugin that handles anything account-shaped ships.

## Notes moved from docs/plugin-api.md (2026-10-02)

### Evidence that `start_all` ignores `dependencies`

**And nothing orders the start-up so that the declaration happens first. Read
this before you rely on `dependencies`.** ADR-006 describes dependency-resolved
loading, and `events.rs`'s own module comment leans on it to explain why
subscribe-time filtering is acceptable. **That start ordering is not
implemented.** `plugin_host::start_all` — the only path that starts a plugin in
the running client, called once from `crates/cordial-runtime/src/bin/load.rs` —
iterates `manifest::discover(&manifest::plugin_root())`, which returns plugins
sorted by **directory name** (`dirs.sort()` in `manifest.rs`) and nothing else.
`plugin_host.rs` contains no reference to `dependencies` or `Dependency`
anywhere in the file, and nothing in it reaches `cordial_plugins::resolve`. (The
word `resolve` does appear there, in `crate::flags::resolve` and a local
`resolve_within`; grep for the module rather than the word.) The dependency
planner in
`crates/cordial-plugins/src/resolve.rs` has non-test callers only in
`marketplace.rs` and the settings window's install-confirmation UI: it decides
what gets *installed*, never what starts first. Nothing refuses to start a
plugin whose declared dependency is absent, either, so ADR-006's "must surface
as a named error to the dependent" is unimplemented as well.

So naming a plugin in `dependencies` has no effect on start order, and a
start-up `events.subscribe` on another plugin's type is a race the manifest
cannot influence. Two things actually work:

- **Retry.** Treat `has not been declared by any plugin` as "not yet" rather
  than as a permanent failure, and try again later — on a timer, or when you
  next have reason to.
- **Subscribe late.** Do it at the point you first need the events, by which
  time a plugin that starts earlier in directory-name order has usually
  declared.

Still list a real dependency in `dependencies`: it is what the installer reads,
so it is how the plugin gets onto the machine at all. Just do not read it as a
scheduling promise. (**INFERRED:** `start_all` reads only
`manifest::plugin_root()`, the user root, and never `system_plugin_root()`,
whereas `flags::collect` reads both. On the face of the two call sites a
first-party plugin shipped under the system prefix is never spawned by the
runtime, which would make a first-party publisher unavailable at any time.
Nobody has run the client to confirm the consequence.)
