# ADR-020: A plugin declares its preferences; Cordial draws them

**Status:** proposed
**Related:** [ADR-003](ADR-003-plugin-isolation.md), [ADR-007](ADR-007-host-resources-are-brokered.md), [ADR-011](ADR-011-wayland-and-libadwaita.md), [ADR-013](ADR-013-per-profile-configuration.md), [ADR-018](ADR-018-plugin-sub-sandboxing.md)

## Decision

A plugin may declare a list of settings in its `plugin.json`. Cordial renders
that declaration as an `AdwPreferencesPage` inside its own Settings window,
using its own widgets, in its own process. The user's answers are written by
Cordial to `<profile>/plugins/<id>/preferences.json` and delivered to the plugin
read-only.

**A plugin never draws.** It receives no widget, no window, no surface, no
toolkit binding and no drawing call, and there is no capability by which it
could ask for one.

Four field types, because four is what the renderer can draw:

| `type` | row | keys |
|---|---|---|
| `bool` | `AdwSwitchRow` | `default` |
| `int` | `AdwSpinRow` | `default`, `minimum`, `maximum`, `step` |
| `choice` | `AdwComboRow` | `default`, `options[{value,label}]` |
| `text` | `AdwEntryRow` | `default` |

Every field carries `key`, `title`, an optional `description` and an optional
`group`. Fields sharing a `group` become one `AdwPreferencesGroup`, in the order
the groups first appear.

**The declaration is the only signal that a plugin has a page.** There is no
`has-preferences` manifest key and no capability meaning the same thing, so the
gear on a plugin's row appears exactly when there is something behind it.

## Why not the GNOME Shell arrangement

The comparison a reader reaches for first is GNOME's Extensions app, and it is
the right comparison: a list of extensions, each row with a gear, and the gear
opens that extension's own preferences. Cordial copies the affordance exactly —
name, identifier, gear where there is something to configure, enable switch —
and deliberately does not copy the mechanism.

GNOME extensions can build their own window because **their code runs inside
the host's own process**. The extension itself is GJS executing inside
gnome-shell; its preferences dialog is GJS executing inside the Extensions app,
which loads the extension's `prefs.js` and lets it return a widget. Either way
the extension is not sandboxed from the process drawing it in any meaningful
sense — it *is* that process, for the duration, with the same GTK and the same
widget hierarchy.

Cordial's plugins are the opposite by design. [ADR-003](ADR-003-plugin-isolation.md)
puts each one in its own address space with no memory access to the core, and it
argues at length that this is the only isolation that actually holds — "an
address-space boundary is enforced by the MMU and does not depend on Cordial
being correct." A plugin is a Deno process with no file, network, environment or
subprocess permission at all. It has no display connection. Handing it one, or a
widget handle, or an RPC that constructs widgets on its behalf, would each undo
that in a different way.

Three specific reasons, beyond "the architecture says so":

**A drawing API is the largest channel in the system.**
[ADR-007](ADR-007-host-resources-are-brokered.md) draws the line as *effects,
not channels*: `presence.set` takes a presence structure and Cordial owns the
Discord socket, because "a plugin that can open a socket of its choosing has the
channel, and every narrow guarantee above evaporates." A general widget-
construction API is that failure in its most complete form. Not one resource —
every pixel of the launcher, with a callback surface attached. There is no
narrow version of "build any interface you like" to broker.

**A plugin that can draw in Cordial's window can draw Cordial's sign-in
dialog.** This is not hypothetical and it is not a new class of risk here:
`crates/cordial-shell/src/webview_policy.rs` exists specifically to keep
`https://www.roblox.com@evil.example/` from rendering, because "a reader
skimming the address sees Roblox." That module refuses a *URL* on the grounds
that it might be misread. It would be incoherent to spend that care on the
address bar and then let an installed plugin construct a pixel-identical
credential prompt inside the same window, with no address bar to skim at all. A
plugin's page is reached from Settings, it is drawn from a declaration, and
every string in it goes through Cordial's own escaping — so the worst a hostile
declaration achieves is a badly-worded switch.

**It would make the plugin boundary depend on Cordial being correct again.**
ADR-003's whole argument is that in-process sandboxing is "a boundary only as
strong as the absence of bugs in it." A widget-building RPC is exactly that
boundary: a list of allowed constructors and properties, maintained forever,
where one missing check is arbitrary drawing. The declarative schema has no such
list to get wrong, because the renderer is ordinary Cordial code and the plugin's
input is data.

### What the declarative form gives up, honestly

A plugin cannot build a page Cordial cannot already draw. No custom widget, no
live preview, no field that appears only when another is switched on, no button
that runs the plugin's own code. Some of those are real losses. GNOME extension
preferences do contain genuinely bespoke interfaces, and nothing here can
reproduce them.

Two of them are worth building later and are not built now: conditional
visibility (`show-when`), and an action row that sends a named message to the
plugin. Both are additions to the schema rather than departures from it — they
keep the property that Cordial constructs the widget — so neither needs this
decision reversed. The rest are the price, and it is the same price ADR-003
already decided to pay for everything else.

### The argument for the other side, and why it loses

The strongest case for plugin-drawn windows is that a plugin could draw its
preferences in *its own* process, in its own window, with its own toolkit — not
in Cordial's. That is genuinely different from the phishing shape above: a
separate top-level window is the plugin's, not the launcher's, and a compositor
that draws window decorations makes the distinction visible.

It fails on three counts. It requires the plugin to hold a display connection,
which is a host resource ADR-007 says a plugin never holds, and on Wayland a
display connection is a fat channel — clipboard, input, and under a permissive
compositor a good deal more. It requires the plugin to bundle a toolkit,
abandoning the property that every Cordial page looks and behaves the same.
And it does not actually solve the impersonation problem, because a plugin's own
window can still be titled "Cordial" and still ask for a password;
[ADR-018](ADR-018-plugin-sub-sandboxing.md)'s sub-sandbox does not help, since
the risk is what the window *says*, not what the process can reach.

## Mechanism

**Declaration.** `preferences` in `plugin.json`, validated by
`cordial_plugins::preferences::check_all` at manifest-parse time. A bad field
refuses the whole plugin rather than being skipped — the same treatment
`manifest::parse` already gives an unknown capability, and for the reason it
gives: a plugin whose page is quietly missing one row installs looking correct
and then behaves strangely.

Validation refuses a default outside its own range, a `choice` whose default is
not one of its options, a duplicate key, more than 64 fields, and any title,
description, group name or option label containing a control character. That
last one is not fussiness: `AdwPreferencesRow` interprets its title as Pango
markup by default, so the renderer turns `use-markup` off for titles and escapes
subtitles, and refusing control characters at parse closes the rest.

**Storage.** `<profile>/plugins/<id>/preferences.json`, per profile for
[ADR-013](ADR-013-per-profile-configuration.md)'s reason — the same installed
plugin tuned one way on a test account must not carry those answers onto the
account somebody plays.

**It is deliberately a different file from `settings.json`, and this is the part
most likely to be "simplified" later by someone who has not hit the bug.**
`crate::settings` is the plugin's own scratch document and `settings.set`
replaces it *whole*, which is correct there: the plugin is its only writer and
needs a way to drop a key. If the user's answers lived in that document, the
first time the plugin saved anything it would erase them. Two writers, one of
whom replaces wholesale, loses the other's data on the first write. So Cordial
owns this file, writes it one key at a time, and there is no `preferences.set`
on the wire at all — a plugin that could rewrite its own answers could set them
to whatever it liked and have the page show the result back as though the user
had chosen it.

**Delivery.** `preferences.get`, and the same document on the `cordial/init`
handshake so the common case costs no round trip. Gated on the existing
`settings.read` capability rather than a new one: both mean "read what Cordial
keeps for you", the data is the plugin's own by construction, and a separate
permission a user could deny would leave a plugin declaring questions it is not
allowed to hear the answers to.

The document handed over is always **complete and always valid** — every
declared key present, every value fitting its declaration. A saved value that no
longer fits, because an update narrowed a range or renamed an option, falls back
to the current default and says so in the log. This is most of what the
declaration buys an author: the parsing, the range check and the fallback happen
once, in Cordial, rather than once per plugin in whatever way each author
thought of.

**The row.** Following GNOME's Extensions app: name, identifier, then the gear
where a schema exists, then the enable switch. No schema, no gear — an
insensitive button reads as broken, an absent one reads as "nothing to
configure".

Two further row states are designed here and **not built**:

- **A failed plugin should carry an error indicator on its own row.** GNOME puts
  a red mark on an extension that failed to load. A plugin that is installed,
  enabled and silently doing nothing is exactly the shape AGENTS.md's rule about
  stubs exists to prevent, and a row that says so is that rule in the interface.
  `plugin_host::start_all` already prints every one of these — not granted, no
  capabilities, could not start — to stdout, where nobody looks. Surfacing it
  needs a channel from the runtime's plugin host to the shell that does not
  exist yet.
- **An accent-coloured gear when an update is available for that plugin**, which
  is what GNOME's coloured gear actually means. Blue and not orange: orange is
  libadwaita's warning colour and says something is wrong, whereas an available
  update is information. `plugin_preferences::gear_for` takes the flag and
  nothing passes `true`, because Cordial has no plugin update detection at all.
  When it gains one it must go through `cordial_update::metered` like every
  other download here — an application that updates itself quietly while nagging
  about its plugins is two policies where there should be one.

## The vsync unlock, and why it is not a new capability

A plugin that wants an uncapped frame rate needs the Vulkan swapchain's present
mode changed, which is Cordial's graphics behaviour and not a FastFlag. That is
the first thing a plugin has wanted that is neither a flag nor an existing
brokered effect, so it is the test of whether the contract is wide enough.

It needed no widening. `CordialPresentMode` is a Cordial-owned key in the
ordinary flag layers, read by `android::vulkan::present_mode_choice` and
filtered out of Roblox's settings document by `client_settings::is_roblox_flag`
— the arrangement `graphics.rs`'s `CordialGraphicsBackend` has used since it was
written. A plugin contributes a value with `flags.set`; Cordial decides what to
hand `vkCreateSwapchainKHR`. The plugin never learns a swapchain exists, cannot
name a mode the driver does not advertise, and cannot reach any other Vulkan
call. Effect, not channel.

It also inherits the layering's precedence for free: the user's `flags.json`
beats every plugin's, so a plugin cannot overrule a present mode somebody chose,
and `CORDIAL_PRESENT_MODE` beats both.

**One consequence of this pattern deserves stating plainly, because it is easy
to arrive at by accident.** `flags.write` lets a plugin set *any*
`Cordial`-prefixed key, not only this one — `CordialGraphicsBackend` and
`CordialDeviceProfile` included. That was already true before this change;
`graphics.rs::plugin_request` reads the plugin layers deliberately and says so.
It is worth knowing that `flags.write` is therefore "contribute to Cordial's own
runtime configuration" and not merely "contribute FastFlags", and that every
future `Cordial*` key inherits that. Keys where a plugin's opinion would be
inappropriate need to be read from somewhere other than the flag layers, and
that is a decision to make per key rather than a hole to patch here.

## Consequences

**Accepted:** the set of things a plugin's page can contain is fixed by
Cordial's renderer, and widening it means a change to Cordial, reviewed and
released. This is the same trade ADR-007 already accepted for brokered
resources, and for the same reason.

**Accepted:** adding a field type is a change to the `Field` enum *and* to the
renderer, together. That is a feature. A schema able to express something the
page cannot draw would present to the user as a row that silently does not
appear.

**Accepted:** a plugin cannot react to a preference changing while it runs. The
document is delivered at handshake and on request, and nothing pushes an update.
Most plugins here already take effect at the next launch — `flags.set` says so
explicitly — so this matches. A `cordial/preferences-changed` push is the
obvious addition when something needs it.

**Accepted, and to be watched:** the renderer is generic and must stay generic.
The first `if plugin.id == "…"` in `plugin_preferences.rs` makes every other
plugin's page the second-class one. Its tests use a fabricated schema rather
than any real plugin's, so the file has no example to grow around.

## What would change this

A plugin whose preferences genuinely cannot be expressed as a list of typed
fields, where the missing thing is not conditional visibility or an action
button. That would be an argument for extending the schema first, and only for
plugin-drawn interfaces if extending it turned out to be endless.

Evidence that the row-level affordances here are the wrong ones — that people
look for a plugin's settings somewhere other than the gear on its row — would
change the interface without touching the decision, which is about who
constructs the widget.
