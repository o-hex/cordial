# ADR-018: Plugins get an OS sandbox under Deno, and it does not replace the broker

**Status:** accepted
**Extends:** [ADR-003](ADR-003-plugin-isolation.md), [ADR-008](ADR-008-plugins-are-typescript-on-deno.md)
**Related:** [ADR-007](ADR-007-host-resources-are-brokered.md)

## Context

A plugin is a Deno process started with no permissions at all — no file,
network, environment or subprocess access — and everything it asks for arrives
over stdio and is checked by the capability broker. That is two layers, and
ADR-003 is the argument for them.

Both layers are in userspace. Deno's permission model is enforced by Deno; if a
plugin escapes it, nothing below stops it reaching the host. Nothing did, and
nothing was watching for it either.

A second question arrived with it: whether a sandbox could *replace* the broker
— "sub-sandbox the plugin and let it do the thing itself" — which would remove
the need to widen Cordial's Flatpak manifest every time a plugin needs anything.

## Decision

**Add a third layer, below Deno,** using `bwrap` on a host install.

**Do not take the Flatpak route.** A Flatpak install keeps the two layers it
already had. See below — the portal that would provide the third one costs more
than it buys.

**Keep the broker exactly as ADR-007 states it.** A sub-sandbox is not an
alternative to brokering and cannot become one.

**A missing sandbox binary does not stop a plugin running.** It is reported
instead: `crates/cordial-plugins/src/sandbox.rs`'s `Sandbox::describe` is printed
at every spawn, naming the layers that are actually in force.

## Why a sandbox cannot replace the broker

Because **a sub-sandbox only ever subtracts.** `bwrap` and `flatpak-spawn
--sandbox` both narrow what the parent holds; neither grants anything the parent
lacks. So confinement can never let a plugin do something Cordial cannot, and
every effect still has to be performed by Cordial. ADR-007 is untouched by this
change, not weakened by it.

The concrete case, because the abstract one invites cleverness. The Flatpak
manifest grants **Cordial** Discord's IPC socket, narrowly, by path. Hand that
socket to a confined plugin so it can set its own presence and the plugin holds
a Discord IPC connection — and that protocol does considerably more than set
presence. The sandbox did not narrow the socket; it narrowed the filesystem
around it.

**"We sandbox plugins now" is the argument someone will eventually use to
justify passing a plugin a file descriptor.** It is written down here, and in
that module's header, so the next person meets the counter-argument before they
meet the idea.

## Why its absence is a downgrade rather than a hole

This is the whole reason the layer can be optional, and it is worth being
precise about. With `bwrap` absent, a plugin still has zero Deno permissions and
still reaches nothing except through the broker. The security model does not
rest on the new layer; the new layer catches a *failure* of the old one.

The alternative — refusing to run plugins without `bwrap` — would turn a
packaging detail into a permission. A machine without the binary would either
lose plugins entirely or, worse, run them and imply confinement it did not have.
The second is this project's stub-that-lies failure wearing different clothes,
which is why the layers in force are printed rather than assumed.

## Why a Flatpak install gets nothing here

The original draft of this ADR added `--talk-name=org.freedesktop.Flatpak` to
the manifest and called it "one override, once". **That was wrong, and the
manifest itself said so before the change was written.** Its header has always
listed the absence of that grant as deliberate: "arbitrary command execution on
the host, which would hand every plugin the sandbox escape the capability model
exists to prevent — below the level any broker can see" (ADR-002 §2).

The reason is that `flatpak-spawn --sandbox` and `flatpak-spawn --host` are the
same D-Bus name. A permission that lets Cordial create a *narrower* sandbox is
the same permission that lets it run an arbitrary command on the host, outside
the sandbox entirely. There is no way to take one and not the other.

So taking it would mean **adding a sandbox escape to Cordial in order to sandbox
plugins** — strictly worse than not having the layer, because the layer defends
against a Deno escape while the grant hands out a host escape unconditionally.

`bwrap` cannot substitute inside a Flatpak either: the outer sandbox blocks the
user-namespace nesting it needs. It is deliberately not attempted, because a
failed spawn is a plugin that does not start rather than one that starts
unconfined.

A Flatpak install therefore reports `Sandbox::None` and keeps Deno's zero
permissions and the broker — exactly what it had before this ADR. That is the
downgrade-not-a-hole case, and it is why that principle had to be settled first.

## Consequences

- Three layers where there were two, and the third is kernel-enforced.
- **No change to the Flatpak manifest**, and a Flatpak install gains nothing
  from this ADR. Host installs get the third layer; Flatpak keeps two.
- Plugin startup names its own confinement, so "was this plugin sandboxed" is a
  log lookup rather than an inference about what the host had installed.
- The sandbox has to make the interpreter reachable, which is less obvious than
  it sounds: binding `/usr`, `/lib`, `/lib64` and `/bin` is where a distribution
  puts Deno and is not where a Homebrew or Nix install puts it. The first
  version bound only those and every sandboxed plugin failed to start. The
  interpreter's real path and its install prefix are resolved and bound
  read-only; `interpreter()` carries the failure that motivated it.
- A cold Deno cache per launch, because `DENO_DIR` points into the sandbox's own
  tmpfs rather than the user's. For a single local module this is not
  measurable, and it keeps a plugin's cache off the host.

## What would change this

Evidence that the confinement blocks something a plugin legitimately needs and
the broker cannot express. That would be an argument for extending the broker,
not for loosening the sandbox — but it would be an argument, and it should be
made here rather than by widening the bind list until a symptom goes away.

A Flatpak portal that creates sub-sandboxes *without* also granting host command
execution would change the Flatpak half of this immediately. The objection is to
what `org.freedesktop.Flatpak` bundles together, not to sub-sandboxing.

If a future Deno gains an isolation primitive that subsumes this layer, drop the
layer rather than keeping two that overlap. Note that Deno's permission model
was already the second layer when this was written, and it was not enough on its
own precisely because it is enforced by the thing being confined.
