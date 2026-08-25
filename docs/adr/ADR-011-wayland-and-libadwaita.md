# ADR-011: Wayland is the display backend, and the window is libadwaita

**Status:** superseded in part by [ADR-024](ADR-024-x11-is-supported-again.md), which restores X11 as a supported backend. Everything else here stands: Wayland is still primary, and the window is still GTK4 + libadwaita.
**Supersedes:** the X11-first choice recorded in [`android/window.rs`](../../crates/cordial-runtime/src/android/window.rs)
**Related:** [ADR-002](ADR-002-core-shell-and-ui-handoff.md), [ADR-009](ADR-009-capture-yes-overlay-injection-no.md)

## Decision

Cordial targets **Wayland natively**. The window is a GTK4 + libadwaita window,
and the engine renders into a Wayland surface belonging to it.

X11 is not developed further.

## Why this reverses the earlier choice

`window.rs` opens by explaining that X11 came first because
`eglCreateWindowSurface` takes an X `Window` directly, whereas Wayland needs a
`wl_egl_window` and a surface role — "more moving parts for the same first
frame". That was a correct judgement about reaching a first frame fastest. It is
the wrong judgement about the client people actually use, and two symptoms made
that concrete.

**Resize.** Dragging Cordial's window flashes flat colour for about a tenth of a
second before the engine catches up. Sober, which is native Wayland, resizes
cleanly. On Xwayland the compositor cannot know when the client's new-size
content is ready, so it shows *something* in the meantime; on Wayland the
`xdg_toplevel` configure/ack/commit sequence exists precisely so that a resize is
atomic and the surface never appears in a half-updated state. The X11 path can be
patched to flash less. It cannot be made correct, because the protocol has no way
to express what is needed.

**Text entry.** The remaining sign-in blocker is that Cordial has to act as the
IME. On Wayland there is a protocol for exactly this — `zwp_text_input_v3` —
which is how squeekboard, Maliit, ibus and fcitx all deliver text. Bridging to it
gets Linux phones and CJK input for the same work, rather than hand-rolling an
input method against raw X keysyms. That was already the plan; Wayland is where
the plan is expressible.

*Still open, and not settled by the subsurface work below.* Cordial's own
`zwp_text_input_v3` client has a live bug — `interface 'zwp_text_input_v3' has
no event 8`, which freezes the landing page — and it was neither fixed nor
removed when the window changed hands to GTK. Removing it was considered and
rejected: it would only have been right if GTK were taking the IME over, and
the evidence for the widget-overlay theory that implied has weakened (Sober
shows typed text live with no toolkit in its engine process at all). One thing
was established: bringing GTK into the process does *not* create a second
text-input object, because GDK creates one only when a GTK text widget takes
focus and this window has none.

**The background behind the canvas.** While the engine catches up, whatever is
behind its canvas is visible. It should be the desktop's own background colour,
following light and dark mode, rather than a flash of white. That is a themed
window's job, and libadwaita answers it directly through `AdwStyleManager`.
Suppressing the repaint instead — the first attempt here — is a workaround for
not having a themed window, and was reverted.

## Why libadwaita specifically

[ADR-002](ADR-002-core-shell-and-ui-handoff.md) already specifies a native
GTK/libadwaita core shell that paints the chooser before the plugin host is up,
and a UI plugin that takes over afterwards. That shell and this window are the
same window. Building the engine's host window as a bare Wayland surface would
mean building the shell twice, and the second one would have to inherit the
theme anyway.

It also settles the light/dark question without a portal round-trip of Cordial's
own: `AdwStyleManager` already tracks `org.freedesktop.appearance color-scheme`
and updates live when the user switches.

## How the engine's surface attaches to that window

Decided and implemented 2026-08-02. "The engine renders into a Wayland surface
belonging to it" was true of the intent and false of the code: the engine owned
a bare `xdg_toplevel` of its own, so the engine's canvas *was* the whole window
and there was no titlebar, because there was no libadwaita window for one to
belong to.

**The engine's `wl_surface` is a `wl_subsurface` of the GTK toplevel.** Three
things follow from that and none of them is optional.

**One connection, therefore one process.** Wayland object ids are scoped to the
connection that created them, so a subsurface cannot parent to a surface on
another connection — and a connection cannot be shared between processes. GTK
opens the connection; `wayland.rs` takes `gdk_wayland_display_get_wl_display`
rather than calling `wl_display_connect`, and Mesa is handed the same pointer it
already was. So `cordial-load` links GTK4 and libadwaita. ADR-002 accepted core
taking a GTK dependency; this is the runtime taking it too, and it is not
avoidable by any amount of process separation. Sober's shape — a GTK process
beside a toolkit-free engine process — is a different arrangement that buys
different things; it cannot produce one window with the engine inside it.

**The window definition is shared, not duplicated.** `cordial-shell` becomes a
library as well as a binary, and `cordial_shell::host_window` is the one place
the `AdwWindow`/`AdwToolbarView`/`AdwHeaderBar` are built. The shell binary puts
the chooser in the content slot; the runtime puts the engine's subsurface over
it. This ADR's "that shell and this window are the same window" was an intention
until now.

**`set_desync` is load-bearing.** A subsurface is created *synchronised*: its
commits do not take effect until the parent commits. GTK commits when it draws,
which for an idle window is never, so without `wl_subsurface.set_desync` the
engine presents into a surface nobody ever latches. Conversely
`wl_subsurface.set_position` *is* latched on the parent's commit, so moving the
canvas requires asking GTK to repaint.

**Measured:** the client reaches `APP_READY (Landing)` on three consecutive
25-second runs with 547-550 `vkQueuePresentKHR` calls each, and a screenshot
shows the libadwaita header bar above Roblox's landing page. `WAYLAND_DEBUG=1`
confirms `get_subsurface`, `set_desync`, `set_position(25, 71)` and
`xdg_toplevel.set_app_id("Cordial")` on the wire.

**Not verified:** anything needing a keystroke or a drag — window resize with
the engine live, and text entry. See `docs/NEXT.md` §1a.

**A trap that cost an hour, recorded so it costs nobody else one.** GTK will
not open a Wayland display if `GDK_BACKEND` names something else, *even when*
`gdk_set_allowed_backends("wayland")` has been called: the two are separate
filters and their intersection was empty. The symptom is `gtk_init_check`
returning false with nothing printed, which surfaces as "no window" and no
further explanation. This developer's ordinary GNOME session exports
`GDK_BACKEND=x11`. `host_window::init_wayland` sets both.

## Consequences

**Accepted:** a native Wayland backend is substantially more code than the X11
one — registry binding, `wl_seat` input with xkbcommon keymaps,
`wl_egl_window` for EGL, and `VK_KHR_wayland_surface` in place of
`VK_KHR_xlib_surface` in the Vulkan interposition. The X11 backend reached a
first frame sooner and that was worth having; it is not worth keeping two.

*Corrected:* this list used to include `xdg_shell`, hand-marshalled because no
protocol XML was available to generate from. GTK owns the toplevel now, so that
code is gone and with it two of the three crashes this file's history records.
`text-input-unstable-v3` is still hand-written and still the one interface whose
event table is Cordial's assertion rather than libwayland's.

**Accepted, with a hard trigger:** the existing X11 code stays only until the
Wayland path runs the sign-in flow end to end, and **is deleted in that same
change** — not in a follow-up, not when someone gets to it.

That is deliberately a rule and not an intention, because this ADR already
rejected maintaining two backends on the grounds that a fallback nobody runs
rots. Keeping X11 "for now" is that same arrangement wearing a deadline, and it
decays the moment the deadline is soft. Two specifics make it decay faster than
usual here: once Wayland works nobody will run X11, including CI, which runs
neither; and the shared `android/input.rs` means a rotting X11 path drags shared
editing logic with it rather than rotting in isolation.

So the condition is mechanical. When the Wayland backend can reach sign-in,
`window.rs` and its `Backend::X11` arm go in the same commit. A change that makes
Wayland work and leaves X11 in place has not finished. Until that point X11 is
not a fallback, it is the *only* working backend and is load-bearing — the risk
described here starts the day that stops being true.

Removal is cheap by construction: what remains in `window.rs` is X-specific
surface and event plumbing, because the display-independent parts already moved
to `input.rs`. Nothing else has to be untangled first.

**Accepted:** input has to be rebuilt, not ported. X11 delivers keysyms; Wayland
delivers keycodes plus an xkb keymap the client is expected to interpret. The
`TextField` editing logic is unaffected — it takes committed text and a caret,
which is what `zwp_text_input_v3` hands over — but everything below it changes.

**Accepted:** `WM_CLASS` becomes the `xdg_toplevel` app_id, which must keep
matching the desktop entry for the reasons in
[ADR-009](ADR-009-capture-yes-overlay-injection-no.md). The test that pins them
together needs to follow.

**Rejected: Wayland with an X11 fallback maintained in parallel.** Two backends
means every input and surface bug is asked "on which one?", and the fallback
rots because nobody runs it. Xwayland exists for hosts without a Wayland
compositor and Cordial does not need to reimplement it.

## What would change this

Evidence that a Wayland compositor in common use cannot do something the engine
requires — an EGL configuration, a presentation timing guarantee — would make
this a compatibility question rather than a quality one. Nothing observed so far
suggests that.
