# ADR-022: A plugin observes, decides and acts; that is what justifies a runtime

**Status:** proposed
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-003](ADR-003-plugin-isolation.md), [ADR-005](ADR-005-flag-layers.md), [ADR-007](ADR-007-host-resources-are-brokered.md), [ADR-011](ADR-011-wayland-and-libadwaita.md), [ADR-018](ADR-018-plugin-sub-sandboxing.md), [ADR-019](ADR-019-development-control-surface.md), [ADR-020](ADR-020-declarative-plugin-preferences.md), [ADR-021](ADR-021-everything-is-a-plugin.md)

## The problem, stated plainly

Cordial embeds Deno to run plugins, and the justification on the record is
sandboxing: a plugin is untrusted code, so it runs somewhere it cannot reach the
filesystem or the network except through a broker.

That justification is circular while plugins have nothing to compute.

Look at what the one real plugin does. FPS Flex reads a few preference values,
maps them to a set of flag names, and writes a JSON document that is consumed
at the *next* launch. Every capability it holds — `flags.read`, `flags.write`,
`settings.read`, `log` — is spent producing a static file. **That is a build
step.** A declarative flag profile with conditionals would do the same job, and
the sandbox would be protecting a program that never makes a decision.

If that is the ceiling, the honest conclusion is to delete the runtime and ship
declarative flag presets. This ADR argues the ceiling is wrong, and says what
has to be true instead.

## Decision

**A plugin is a program that observes the running client, decides something,
and acts on it. Cordial provides all three, or it should not provide a runtime
at all.**

Three things are added, and they are one thing in three parts:

1. **An event bus.** A plugin subscribes to what the client is doing.
2. **Live effects.** Some acts take effect now rather than at the next launch,
   and the API never lies about which.
3. **Contributed UI.** A plugin can draw, in Cordial's own window, over the
   engine's canvas.

### The admission rule for any future capability

**Could a static configuration file do this?** If yes, it is configuration and
belongs in the manifest, where ADR-020 already puts preferences and ADR-021
already puts assets and flags. If it requires observing something that is only
known while the client runs, deciding from it, and acting before the moment
passes, it is a program — and then the sandbox is containing something that
genuinely has reach, which is the only condition under which embedding a
JavaScript runtime is worth its cost.

Applied to what exists today, this rule says FPS Flex should largely be
declarative, and that the interesting version of FPS Flex — one that watches
frame time and gives quality back when a scene gets cheap — is a program.

## 1. The event bus

`cordial-plugins/src/events.rs` has an `EventRegistry` and essentially nothing
flowing through it. The client already knows everything below and tells nobody:

| Event | Where Cordial already knows it |
|---|---|
| `client.ready` | `[roblox] engine initialised` |
| `place.joined` / `place.left` | `gameLoadedCallback`, the deep-link path |
| `window.focus` / `window.blur` | `report_focus`, `wl_keyboard.leave` |
| `surface.resized` | `sync_canvas_geometry` |
| `frame.stalled` | the watchdog that prints `presented nothing for 5s` |
| `textbox.focused` / `textbox.blurred` | `showKeyboard` / `hideKeyboard` |
| `asset.slow` | the asset path §1194e99 was measured in |

Rules, each of which exists because of something already learned here:

- **Delivery is best-effort and never blocks the client.** A plugin that is slow,
  wedged or crashed must cost frames from nobody. The engine's pump is the one
  thread this project has repeatedly had to defend; nothing subscribed gets to
  stall it.
- **Events are facts, not requests.** A plugin cannot veto a join or swallow a
  keystroke. Anything else is an in-process hook wearing a subscription, and
  ADR-001 rules that out permanently — not disabled, *absent*.
- **Payloads carry no secrets.** No `.ROBLOSECURITY`, no auth headers, no text
  the user typed. `input.rs` already redacts its trace line because it "used to
  print a password in full on every keystroke", and an event bus is a much wider
  pipe than a trace line.
- **Subscription is a capability**, per event family, granted like any other
  under ADR-003's default deny.

## 2. Live effects, and honesty about which is which

`flags.rs` records something measured that inverts the obvious framing, and the
API must be built on it rather than around it:

> Being re-read is a cost, not a capability. The engine fetches Roblox's
> settings document itself about two seconds in and applies it over the top, so
> a `DF*` override of a key Roblox also sets is reverted to Roblox's value while
> the client is still starting.

So `FFlag`/`FInt`/`FString` are **durable** and take effect at the next launch.
`DFFlag`/`DFInt`/`DFString` are **live** and revocable, and govern the first
couple of seconds unless Roblox never sets the key. Dynamic is the weaker
guarantee, not the stronger one.

`Capability::FlagsWrite` and `Capability::FlagsWriteDynamic` already exist as
two capabilities. What has to follow:

- **The prefix is validated and mismatches are errors.** Writing `FFlagX`
  through the live primitive must fail loudly. The engine will never re-read it,
  so it would report success and do nothing — the stub-that-lies AGENTS.md
  exists to forbid.
- **The result says when it took effect**, `applied_now` or
  `applies_next_launch`, because that is the only thing a plugin needs in order
  to tell the user whether to relaunch.
- **Neither is named for speed.** A plugin author who reads "dynamic" as
  "better" picks the one that silently stops working.

## 3. Contributed UI, over the engine's canvas

This is the strongest justification of the three, because drawing a panel from
declarative config is painful and drawing it from code is easy — and because it
is the only one that gives the user something they cannot get any other way: a
web view, a toast, or a plugin panel *on top of Roblox*, in one window.

The mechanism is not a hook and not a second toplevel. GTK owns the
`xdg_toplevel` (ADR-011) and the engine's canvas is a `wl_subsurface` of it,
today placed **above** the parent, with
`webview_dialog_opened`/`webview_dialog_closed` lowering it for exactly as long
as a dialog is open. That is why a web view covers Roblox entirely instead of
sitting over it.

The intended arrangement inverts it: **the engine's subsurface sits permanently
below the parent, and the parent is transparent and input-transparent over the
canvas region.** Then every GTK widget composites above the engine for free, and
a plugin panel is an ordinary widget rather than a special case.

**This is not yet established and must not be built on until it is.** The three
open questions, each with a way to answer it:

1. Can the GTK toplevel be transparent over the canvas area?
2. Does `wl_surface.set_opaque_region` — GTK's, not ours — let a compositor skip
   painting the subsurface underneath?
3. Do pointer events fall through to the engine, and does an input region we set
   survive GTK's next commit?

Question 3 decides the design. If GTK will not yield the input region, the
fallback is a dedicated overlay subsurface placed *above* the engine and painted
by Cordial rather than by GTK — which serves toasts and plugin panels but not a
WebKit view, and that is a materially worse outcome worth knowing early.

### What a plugin declares, and the escape hatch that is not a channel

A plugin never receives a `wl_surface`, a GL context, or a handle to anything
Cordial composites. It declares content and Cordial draws it, which is ADR-007
applied to pixels rather than to sockets. The vocabulary is three things:

1. **A toast** — text, a position, a duration. The server-location indicator is
   this and nothing more.
2. **A panel** — the declarative widget set ADR-020 already defines for
   preferences, rendered somewhere other than the settings window.
3. **An image** — the plugin renders offscreen and hands over a buffer.

The third exists because refusing it makes the other two a cage. A plugin
wanting a chart, a minimap or a custom readout has no way to express it
declaratively, and a design that answers "you cannot" invites somebody to
propose a surface capability again in six months. **Pixels are content; a
surface is a channel**, and handing over a finished image keeps every property
that matters:

- it cannot wedge the compositor, because the plugin never commits and never
  blocks — a slow plugin yields a stale frame, not a hung client;
- it is droppable, because Cordial owns the frame budget and can skip a
  plugin's buffer while the engine is busy, which is impossible if the plugin
  holds the surface;
- it is bounded, because a size and a refresh cap are enforceable on a handover
  and unenforceable on somebody else's surface.

**Two constraints ride with all three, and they are not negotiable.**

**Plugin content is always visually attributed.** A plugin's pixels are visibly
a plugin's — Cordial draws the frame around them, and a plugin cannot suppress
it. **Plugin content can never cover Cordial's own chrome and can never go
fullscreen.**

These are the constraints browsers put on extension UI, for the reason that
applies here with more force: anything composited over Roblox can be drawn to
look like Roblox. A plugin able to paint arbitrary unattributed pixels over the
canvas can paint a convincing sign-in prompt, and this project keeps the user's
`.ROBLOSECURITY` in the desktop secret service. Attribution and the chrome
exclusion are what make an overlay a feature rather than a phishing surface, and
they must be enforced by the compositing code rather than requested of the
plugin.

A compositor-level screenshot can now answer all three, which was impossible
until `a4abe15`: `--headless` runs the client under a wlroots compositor we
control, and `wlr-screencopy` will photograph the composited result. The
swapchain screenshot cannot — it shows the engine's own output and looks
identical whether or not GTK is covering it.

## What this does not change

**No in-process code execution against the Roblox process.** No hooking, no
memory patching, no injected script environment. An event is something Cordial
observed and chose to publish, not a callback the engine calls. A live flag is a
value the engine re-reads of its own accord. A contributed panel is a Wayland
surface Cordial composites. None of these is a primitive a fork could extract
into a hook, which is the property ADR-001 and ADR-003 exist to preserve.

**Plugins still receive effects, not channels** (ADR-007). A plugin subscribing
to `place.joined` does not get a socket; it gets a payload. A plugin drawing a
panel does not get a `wl_surface`; it declares content and Cordial draws it.

**Default deny still holds** (ADR-003). Every event family and every UI surface
is a capability, granted per profile, and a plugin that has been granted nothing
observes nothing — which is what a freshly installed plugin does today, and what
the Plugins page now names the profile for.

## Consequences

Cordial gains a plugin API that can express an adaptive quality controller, a
presence integration that reflects the actual place, and an overlay — none of
which a configuration file can express, and all of which need containing. The
sandbox stops being a precaution around a config generator and starts being the
thing that makes untrusted programs safe to run, which is the justification the
runtime always claimed.

The cost is a much larger surface to keep stable, and a real risk that the event
bus becomes a way for a plugin to slow the client down. The best-effort,
non-blocking rule is the whole defence, and it needs a measurement, not a
comment: an event bus with a deliberately wedged subscriber must cost zero
frames against a control with none, or this ADR is wrong about its own premise.
