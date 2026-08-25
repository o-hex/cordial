# Review: HaltingMachine's X11 pointer-lock patch

**Reviewed 2026-08-26**, against
`HaltingMachine/cordial@e2f6f537d1535cf25d2922bcf72670f14413febf`, branch
`x11-pointer-lock-test`. 387 added lines, all in
`crates/cordial-runtime/src/android/window.rs`. Read from the commit diff; the
fork was not cloned.

The patch works — the author reports cursor capture functioning on X11 where
vanilla Cordial's does not — and every helper it calls (`engine_wants_pointer_
lock`, `pass_mouse_move_delta`, `reset_mouse_delta`, `trace_mouse`,
`BUTTON_SECONDARY`, `BUTTON_TERTIARY`) already exists upstream, so it applies
cleanly and depends on nothing else in the fork.

Three faults were reported with it: *"the camera can get jerky, sometimes the
player won't move after joining an experience despite me pressing WASD, and
multimedia/audio keys can occasionally jerk the camera as well as spam
right-click"*. They have three different causes, and only two are in this patch.

## The media-key fault, which is the one that explains the most

**`FocusOut` is acted on unconditionally, and a desktop environment's global
hotkeys generate it.**

```rust
FOCUS_OUT => {
    self.release_pointer_lock();
    let mut state = self.input.lock()...;
    state.buttons = 0;
}
```

X11 has four kinds of focus change, distinguished by the event's `mode` field:
`NotifyNormal`, `NotifyGrab`, `NotifyUngrab` and `NotifyWhileGrabbed`. **A
desktop environment binds media keys with `XGrabKey`, and when such a grab
activates the focused client is sent `FocusOut` with `mode == NotifyGrab`,
followed by `FocusIn` with `NotifyUngrab` when it ends.** The window never
actually lost focus. Nothing happened that the client should react to.

So pressing volume-up while playing does this:

1. `FocusOut(NotifyGrab)` arrives.
2. `release_pointer_lock()` ungrabs **and warps the pointer back to
   `saved_root`** — a jump across the screen, delivered to the engine as motion.
3. The next `pump_input_events` calls `sync_pointer_lock()`, the engine still
   wants the lock, so it grabs again and warps to the centre — a second jump.
4. `state.buttons = 0` runs while the right button is still physically held, so
   the engine's view of the buttons and the shadow state disagree until the next
   `ButtonPress`.

Two warps and a desynchronised button state, per media keypress. That is
"occasionally jerk the camera as well as spam right-click", exactly.

**Fix: filter on `mode`, and act only on `NotifyNormal`.** There is a catch in
this file: `XInputEvent` is laid out for `XKeyEvent`/`XButtonEvent`/
`XMotionEvent`, and `XFocusChangeEvent` is a different shape —
`{type, serial, send_event, display, window, mode, detail}` — so `mode` sits
where `XInputEvent::root` is and the field the patch needs is not in the struct
it reads. It needs its own view of the event.

**And clearing `buttons` on focus loss is wrong even when the focus loss is
real.** Setting the shadow copy to zero does not tell the engine anything; it
just makes the two disagree. A real focus loss should *deliver* the releases —
the same problem Wayland's `pointer_leave` had, and it was fixed there by
sending the button-up rather than forgetting the button was down.

## The jerky camera: warp-and-recentre double-counts

```rust
if state.ignore_next_warp && ev.x == cx && ev.y == cy {
    state.ignore_next_warp = false;
    return;
}
let dx = ev.x - cx;
let dy = ev.y - cy;
```

The filter is one boolean plus an exact coordinate match. Motion events that the
server generated *before* the warp took effect still arrive after it, and their
`dx`/`dy` is computed against a centre the pointer has already been moved to —
so the same physical movement is counted twice. This is the classic failure of
warp-based mouselook and it looks precisely like a camera that is fine and then
lurches.

**The robust answer on X11 is not a better filter. It is not to warp at all:**
XInput2's `XI_RawMotion`, selected with `XISelectEvents` on the root window,
delivers true relative deltas straight from the device, before pointer
acceleration and before any screen-edge clamping. `XGrabPointer` still confines
the pointer; nothing needs recentring, so there is nothing to filter and no
double count is possible. SDL and GLFW both do this for relative mouse mode, for
this reason.

If the warp approach is kept, the filter has to be a **serial** rather than a
boolean: remember the serial `XWarpPointer` returned and discard any motion
event whose serial is below it, instead of matching coordinates.

## The dead WASD is probably not this patch

`input::pass_key_event` drops text keys while a TextBox has focus:

```rust
if !keys_to_game_while_typing()
    && evdev_is_text_key(evdev_code)
    && cordial_linker_sys::game_activity::focused_textbox().is_some()
```

W, A, S and D are evdev 17, 30, 31 and 32 — all text keys. **On X11 there is no
editor widget at all**, so a box that takes focus and never cleanly blurs leaves
that guard engaged and every movement key is silently eaten. That matches
"sometimes the player won't move after joining an experience" better than
anything in the pointer-lock code does, including the "sometimes".

Two commands settle it:

- Run with `CORDIAL_TRACE_TEXT=1` and grep for
  `pass_key_event suppressed: code=17`. If it is there, this is the guard.
- Then `CORDIAL_KEYS_TO_GAME_WHILE_TYPING=1` as the control: movement should
  come back while a box is focused, and text should start walking the character.

## Why the two machines differ

Vanilla works on the Ubuntu 22.04 laptop and not on the Linux Mint 22.1 desktop,
both X11, which the author reasonably finds confusing. The likeliest axis is the
desktop environment rather than X11 itself: Mint runs Cinnamon and Ubuntu 22.04
runs GNOME, and **which keys are grabbed globally, and which focus events a
window manager synthesises, are both DE decisions.** That is the same axis the
`FocusOut` fault above rides on.

That is a hypothesis and not a measurement. The measurement that would settle it
needs no code change: log every `FocusIn` and `FocusOut` with its `mode` and
`detail`, run the same session on both machines, and diff. If the desktop emits
`NotifyGrab` pairs the laptop does not, that is the whole difference.

## What this means for Cordial

[ADR-024](../adr/ADR-024-x11-is-supported-again.md) makes X11 supported again,
so this work has somewhere to land. Two notes for whoever merges it:

**The pointer-lock design should be settled before the merge, not after.**
Warp-and-recentre and XInput2 raw motion are not variations on one approach; the
second deletes the entire class of bug the first spends its filter on. Taking
the warp version now means the filter gets tuned for months.

**Whatever lands needs a control.** `CORDIAL_NO_POINTER_LOCK`,
`CORDIAL_FORCE_POINTER_LOCK` and `CORDIAL_NO_DRAG_LOCK` in this patch are good
instincts and exactly the shape this project asks for. What is missing is a
reading that distinguishes working from broken: a jerk is visible to a human and
invisible to every counter Cordial currently has. A trace of raw deltas against
delivered deltas would show a double count immediately.
