# Where to start

Cordial loads Roblox's engine on Vulkan, does HTTPS, takes mouse and keyboard,
and reaches the logged-out landing page — on X11, confirmed by repeated
screenshot. The Wayland backend now presents frames too (see §1a) — confirmed
by Vulkan call counts matching X11's, not yet independently confirmed by
screenshot. Neither is yet usable, because you cannot sign in.

This file is the handover. It says what is blocking, how to work on it, and —
the part worth reading even if you are in a hurry — **what has already been
ruled out**.

## The one rule

**Grep the capture before disassembling anything.**

`docs/traces/waydroid-roblox-startup.log.gz` is a logcat capture of this exact
APK running on real Android. When a question comes up about what the engine
expects, that is a lookup rather than an investigation.

Over one long session, **nine consecutive conclusions drawn from reading the
stripped binary were wrong**, and every conclusion drawn from running something
held up. That record is why this rule is first.

## Read the engine's own log, always, before theorising

Roblox narrates itself to:

```
<files>/appData/logs/<version>_<timestamp>_Player_*.log
```

By default `~/.local/share/cordial/instances/default/data/files/appData/logs/`.
It names subsystems, stages, file paths and exceptions in Roblox's own words. It
is the best diagnostic in the project and it answers most questions. Two comments
in this repository once claimed FLog was unrouted; both were wrong, and nobody
had looked in `appData/`.

---

# What is blocking

## 1. Text entry — the last step before you can sign in

**The login form works.** Clicking Sign In on the landing page opens Roblox's
Lua-rendered login form — username, password with a reveal toggle, Quick Sign-in,
Forgot Password. Clicking a field focuses it and shows a caret. All of that is
verified by screenshot.

**What does not work is typing into it.** Characters do not appear. Everything
else about sign-in is now reachable, so this is the single remaining step.

The cause is almost certainly the same shape as the input bug it came out of.
Roblox reads text through its own on-screen-keyboard contract, and Cordial
implements none of the reverse half:

```text
nativeGetTextBoxInfo()                     -> NativeTextBoxInfo
syncTextboxTextAndCursorPosition2(String, I)
updateKeyboardSize(...)
nativeReturnPressedFromOnScreenKeyboard()
GameActivity.setSoftKeyboardActive(Z, I)   <- engine calls INTO Java
```

`nativePassText` and `nativePassKeyEvent` are already wired and are not enough on
their own. The engine calls *out* to Java to raise a soft keyboard and attach an
input connection, and nothing answers — so the field is focused with no text
source attached. Implementing that reverse contract is the job.

**Already ruled out, do not redo:** delivering keys through AGDK's
`onKeyDownNative` (accepted, ignored), delivering editing state through AGDK's
`onTextInputEventNative` with a populated `gametextinput/State` (accepted,
ignored), and re-sending window focus after the Lua app is up (no effect).

### What has since been established

Most of the reverse contract above is now wired, and the picture is much
narrower than "nothing answers".

**`showKeyboard`'s first argument is the handle of the box being edited.** It was
being discarded and text was then sent with handle 0, which the engine drops in
silence. Captured now, along with the box's current contents. `nativePassText`,
`syncTextboxTextAndCursorPosition2` and `updateKeyboardSize` are all driven and
all return without error, and `CORDIAL_TRACE_TEXT=1` shows focus detected and
text accumulating correctly. **It still does not appear in the field.**

**`updateKeyboardSize(visible=true)` destroys focus. This is the important one.**
The trace order is not ambiguous:

```text
textbox focused handle=139759059370112
updateKeyboardSize(visible=true)
textbox blurred
updateKeyboardSize(visible=false)
textbox focused handle=139759059370112
```

Focus bounces continuously while that call is driven. With
`CORDIAL_NO_KEYBOARD_REPORT=1` it is stable — one `focused`, no blur, confirmed
by control in the same session.

That also explains the field appearing to clear on every keystroke:
`edit_text_buffer` reseeds from the engine whenever the focus generation changes,
so a bouncing focus resets the buffer to empty between characters. The clearing
was self-inflicted, not the engine rejecting anything.

**Do not conclude `updateKeyboardSize` is useless and delete it.** The engine
asks for a keyboard, so something is expected to acknowledge one. What is wrong
is a bare `visible=true` with a zero-height rectangle at the window's bottom edge
— plausibly the engine re-lays-out around the reported keyboard and drops the
capture in the process. The call needs different arguments, or a different
moment, not removal.

**Ruled out as the cause of the bounce:** duplicate pointer delivery. Both AGDK's
`onTouchEventNative` and `NativeInputInterface` receive every click, so one press
does arrive twice — but disabling AGDK's copy (`CORDIAL_NO_AGDK_TOUCH=1`) leaves
the bounce exactly as it was.

### Where this is going

Synthesising an input method by hand is the wrong shape and is being abandoned.
Cordial becomes a **bridge**: the platform's own input method on one side,
Android's contract on the other. On Wayland that is `zwp_text_input_v3`, which
the compositor routes to whatever the user actually runs — ibus, fcitx,
squeekboard on a phone — so composition, dead keys and CJK candidate windows stop
being Cordial's to reimplement badly.

The Android half does not go away: the engine only speaks `showKeyboard` and
friends, because it is the Android build. What goes away is Cordial inventing the
editing state in the middle. See
[ADR-011](adr/ADR-011-wayland-and-libadwaita.md).

### The Android half is bigger than `showKeyboard` — AGDK's `InputConnection`

**Correction to "the engine only speaks `showKeyboard` and friends."** A live
run's jnivm log shows the engine also reaching for
`InputConnection.setState`/`setSoftKeyboardActive`/`restartInput` and getting
`Constructed Unresolved symbol` every time — the *outbound* half of AGDK's own
`GameTextInput` contract, engine calling out to report its own idea of the
editing state, as distinct from the *inbound* half
(`onTextInputEventNative`) already ruled out above. Nothing had ever
constructed an `InputConnection` object for the engine to call these on, so
every one of these calls landed on a receiver that did not exist.

Implemented in `native/game_activity.cpp`:

- `InputConnection` (`com/google/androidgamesdk/gametextinput/InputConnection`),
  constructed once and handed to the engine via
  `GameActivity.setInputConnectionNative` — driven directly from
  `load.rs` right after the surface is handed over, the same way
  `cordial_game_activity_init` drives `initializeNativeCode` directly instead
  of waiting for a Java caller that does not exist. Signatures are from the
  shipping APK's dex (`tools/dex_method.py`), not guessed:
  `setState(Lcom/google/androidgamesdk/gametextinput/State;)V`,
  `setSoftKeyboardActive(ZI)V`, `restartInput()V`.
- `GameActivity.setImeEditorInfoFields(III)V`/`setWindowFlags(II)V` — also
  previously unresolved, now real no-op hooks; resolving is the point, per
  `NativeTextBoxInfo`'s own comment on the pending-exception hazard an
  unresolved call carries.
- `android::input::reseed_if_needed` (`crates/cordial-runtime/src/android/
  input.rs`) now prefers `InputConnection.setState`'s text and caret over
  `showKeyboard`'s one-shot byte-array snapshot, once at least one `setState`
  has actually arrived — `setState` is refreshed rather than captured once at
  focus time, and carries a real caret where `showKeyboard`'s array carries
  none.

**Deliberately not done, and why:** reseeding *live*, on every
`ime_state_generation()` change rather than only at the existing
focus-change boundary. `setState` is also how the engine would echo back
whatever Cordial itself just pushed via `pass_text`/`sync_textbox`; treating
every echo as a fresh overwrite mid-keystroke is the same shape of feedback
loop that produced the focus-bounce bug two sections up, and confirming a
live-overwrite version does not reintroduce it needs the interactive test
this change has not yet had (see below).

**Verified:** `setInputConnectionNative` registers cleanly
(`CORDIAL_ANDROID_TRACE=1` shows `InputConnection registered with the
engine`) and a full run reaches `APP_READY (Landing)` with no
`Constructed Unresolved symbol` for `InputConnection` or either `GameActivity`
method, on Wayland. **Not yet verified:** that `setState` actually arrives
once a field is focused and typed into, or that the reseed change makes
characters appear — both need clicking into a real field and typing, which
this session's automated environment could not do (the desktop session was
screen-locked for the remainder of it). Do the interactive test — click a
field on either backend, type, screenshot — before trusting this as the fix
rather than as a well-motivated, resolves-cleanly, unverified-live change.

### Why the text is invisible while you type: Android draws it with a real widget

Established 2026-08-02, and it reframes this whole section. The engine is not
failing to receive the text and there is no message that makes it render.

**The symptom.** Typing into a focused box shows nothing and draws no caret. On
blur the full, correct string appears at once.

**What that rules in.** Cordial sends *nothing* at blur — `nativePassText` is off
by default and `hideKeyboard` only marks the box blurred. So the correct string
that appears on blur can only have arrived through the per-keystroke
`syncTextboxTextAndCursorPosition2` calls. The engine held it the whole time and
withheld the *drawing*, deliberately.

**Why.** On Android the editing-time display is not the engine's job. It belongs
to a real `android.widget.EditText` laid over the GL surface:

```text
Lcom/roblox/client/RbxKeyboard; -> Lq/l; -> Landroid/widget/EditText;
```

Verified from the dex `class_def` superclass chain. `RbxKeyboard` carries
`getCurrentTextBox()J`/`setCurrentTextBox(J)` — the same handle `showKeyboard`
passes — plus `i(NativeTextBoxInfo, String)`, `l(NativeTextBoxInfo)`,
`setManualFocusRelease(Z)`, `onSelectionChanged`, `onKeyPreIme`, `autofill`, an
inner `TextWatcher` and an `OnEditorActionListener`. In
`res/layout/activity_game.xml` it is a sibling of the GL surface's container,
`background=@android:color/transparent`, `visibility=gone`, `match_parent` —
a transparent editor revealed over the surface on demand.

That is what `NativeTextBoxInfo`'s fields are *for*. `x, y, width, height,
fontSize, font, textColor, xAlignment, yAlignment, multiline, textWrapped`
are not IME hints; they are how to style a widget so it looks exactly like the
Roblox box underneath it. Only `textInputType` and `returnKeyType` configure an
IME. And the engine pushes text *out* during editing —
`onLuaTextBoxChangedCallback(String)` and the no-argument
`onLuaTextBoxPropertyChangedCallback()`, whose only sensible response is to
re-read that geometry. A "properties changed" callback is only needed if Java is
displaying the box. **Both are unimplemented in Cordial**
(`docs/analysis/unresolved-java.md` §2c).

So the shadow buffer was never the problem, and deleting it was never going to
help: **the missing piece is a widget, not a message.** Cordial has to draw the
editing text itself, positioned and styled from those 14 fields, which
`NativeTextBoxInfo::init` (`native/android_classes.cpp:220`) currently accepts
and discards.

**There is already an instrument for this and nobody was reading it.**
`FLog::NativeInput` and `FLog::DataModelBindings` are on by default and narrate
the path in Cordial's own engine log, no `flags.json` needed:

```text
onTextBoxFocused: 0x7f366c4d0080
handleTextBoxFocused_AndroidLayer_:
```

in `~/.local/share/cordial/instances/default/data/files/appData/logs/*_Player_*.log`.
Note the engine's own name for it: `_AndroidLayer_`.

**Not yet confirmed**, and the one experiment that would settle it: focus a box
that already contains text and type *nothing*. If the existing text vanishes on
focus alone, the engine stops drawing focused boxes and the diagnosis holds. If
it stays, the diagnosis is wrong and the gap is that `sync` never reaches the
rendered property — chase `onLuaTextBoxChangedCallback` instead. That needs a
keystroke, which no Wayland-safe automation here can supply.

**Counter-evidence, raised 2026-08-02, and it is strong.** Sober shows typed
text live, as you type — and Sober's engine process links raw EGL/GLESv2 with
its own Wayland client and no toolkit at all. Its GTK4/libadwaita/WebKitGTK
usage is in a separate binary for web views, so there is nothing over its
surface that could be drawing that text. If the engine could only ever be drawn
into by an overlay editor, Sober could not do what it demonstrably does. So the
engine *can* draw a focused `TextBox` itself, and the section above is at best
incomplete.

The likelier mechanism, and it is testable: on Android, touching a field does
not raise the soft keyboard when a hardware keyboard is attached, and with no
soft keyboard up the engine draws its own text and its own caret. What tells it
which case it is in is `updateKeyboardSize` — which Cordial has never once
reported correctly *while someone typed*, because the call is gated off behind
`CORDIAL_KEYBOARD_REPORT=1` after an earlier, wrong version of it
(`visible=true` with zero height) bounced focus. **Run that experiment before
building anything shaped like an overlay editor.** It costs one launch:
`CORDIAL_KEYBOARD_REPORT=1 CORDIAL_TRACE_TEXT=1`, click a field, type. Do not
treat "the missing piece is a widget" as established while it is untested.

### The capture does not cover text entry. Do not grep it for this

Checked exhaustively on 2026-08-02, because the repo's one rule sends everyone
here first. `docs/traces/` holds one capture — `waydroid-roblox-startup.log.gz`,
2432 lines — and the session that produced it **never touched a TextBox**. It
launches `ActivitySplash`, reaches the Lua home screen, and is backgrounded
without interaction.

Hits across the whole capture, case-insensitive: `syncTextboxTextAndCursorPosition`
0, `nativePassText` 0, `nativePassKeyEvent` 0, `nativeGetTextBoxInfo` 0,
`nativeReturnPressed` 0, `NativeTextBoxInfo` 0, `TextBox` 0, `InputConnection` 0,
`InputMethodManager` 0, `restartInput` 0, `setImeEditorInfoFields` 0,
`setSoftKeyboardActive` 0. `showKeyboard`'s two hits are both inside the flag
name `EnableTextInputRestoreOnShowKeyboard`, not a call.

So for text entry the trace is not a lookup, and the rule's protection does not
apply — which is exactly when this project has historically gone wrong. **The fix
is another capture, not another theory:** the same `adb logcat` procedure in
`docs/traces/README.md`, driven into the home screen's search box (reachable
without a login), typing a few characters and blurring. `rbx.glview.layout`
already logs at verbosity V, so a keyboard-visible `onUpdateKeyboardSize()` would
appear the moment the IME opens.

**What the capture does establish**, twice, at surface bring-up (lines 1113 and
1263, immediately after the `SurfaceView` resize and before `surfaceCreated`):

```text
rbx.glview.layout: [a.e()-51]: onUpdateKeyboardSize() v:false x:0 y:999 w:2491 h:0
```

That confirms `updateKeyboardSize(Z,I,I,I,I)` is `(visible, x, y, width, height)`,
and that the real client's keyboard-hidden baseline is `visible=false` with a
**real** rectangle — full UI width, zero height, at the bottom. Not an empty one.
`INFERRED`, and flagged by the agent that found it: that line is the app's own
Java-side layout callback, and a 1:1 correspondence with the JNI call was not
established.

The capture never shows `updateKeyboardSize` with `visible=true`, so it cannot
say what rectangle the engine gets when an IME is actually up.

## 1a. The Wayland backend was blank — two independent bugs, both fixed

`CORDIAL_WAYLAND=1` produced a window present in the dock and alt-tab, titled
correctly, completely blank on screen, for the entire time this project has
had a Wayland backend. Two unrelated bugs, both real, both now fixed in
`crates/cordial-runtime/src/android/vulkan.rs` and
`crates/cordial-runtime/src/android/wayland.rs`. Neither was found by reading
FLog — see the note in §2a above about why FLog is not useful for this
question — both were found by instrumenting the actual Vulkan/Wayland calls
and comparing X11 (works) against Wayland (did not) with real numbers.

**Bug 1: `vkGetPhysicalDeviceSurfaceCapabilitiesKHR`'s `currentExtent` was
never patched for Wayland, and the engine cannot handle the value Wayland
sends.** `VK_KHR_wayland_surface` reports `currentExtent` as
`(0xFFFFFFFF, 0xFFFFFFFF)` — the spec's own "the client picks the size, not
the platform" sentinel, because unlike an X11 window or a real Android
`ANativeWindow`, a Wayland surface has no size of its own until a buffer is
attached. Confirmed directly: `vkGetPhysicalDeviceSurfaceCapabilitiesKHR ->
0, ... currentExtent=4294967295x4294967295` on Wayland,
`currentExtent=1280x720` on X11, for the identical call. The engine's own
FLog explains what it does with that: `Vulkan: skipping framebuffer creation,
invalid currentExtent -1x-1`, repeated every frame, forever — its surface
code was written against Android's always-a-real-size `VkSurfaceKHR` and has
no path for "you choose". Confirmed with a second, independent counter:
`vkCreateSwapchainKHR` and `vkAcquireNextImageKHR` were called **zero** times
on Wayland for a whole run that reached `APP_READY (Landing)`, against one
`vkCreateSwapchainKHR` and 653 `vkAcquireNextImageKHR` calls on X11 in the
same window — the engine never even attempted to create a swapchain.

The `Invalid currentExtent -1x-1` line is a trap worth naming explicitly: it
also fires continuously on X11, which renders correctly, from an unrelated,
harmless periodic check elsewhere in the engine. Trusting that line alone
without comparing the two backends' actual `vkCreateSwapchainKHR`/
`vkQueuePresentKHR` call counts would have (and briefly did) point at the
wrong conclusion.

Fix: `vk_get_physical_device_surface_capabilities_khr` in `vulkan.rs`
intercepts the call on the Wayland backend only and, when `currentExtent` is
the undefined sentinel, replaces it with the Wayland window's own current
size — the same "report what an Android surface would report" substitution
this file already makes for the surface identity itself
(`vkCreateAndroidSurfaceKHR -> vkCreateWaylandSurfaceKHR`).

**Bug 2: a second `wl_proxy_add_listener` call on `xdg_surface` silently
failed, leaving a dangling stack pointer registered, which segfaulted the
moment the fixed Vulkan path tried to resize.** Once bug 1 was fixed and the
engine started really presenting, the process reliably segfaulted a few
frames later — `wl_closure_invoke` (inside `libwayland-client`) jumping to
address `0xe0` via `libffi`'s `ffi_call`, i.e. calling through a garbage
function pointer. `open()` in `wayland.rs` used to register a temporary,
stack-local `XdgSurfaceListener` for the *first* `xdg_surface.configure`
(before `WaylandWindow` exists for a steady-state listener to reach via
`current()`), then called `wl_proxy_add_listener` a second time to swap in
the real, `'static` listener once construction finished. Logging that second
call's return value directly showed `-1` — `wl_proxy_add_listener` refuses a
second registration on a proxy that already has one and changes nothing —
so the dangling stack listener (a local in `open()`, which had long since
returned and had its stack frame reused many times over) stayed registered
for the whole session. The *first* subsequent `xdg_surface.configure` — which
never arrived before bug 1 was fixed, because nothing had ever made the
window worth reconfiguring — read whatever unrelated bytes now occupied that
stack slot as a function pointer and jumped to them.

Fix: one `XdgSurfaceListener`, registered once, for the proxy's whole
lifetime. It writes the initial serial into a small static
(`INITIAL_XDG_SURFACE_SERIAL`) when `current()` is still `None`, instead of a
second listener swap. See `xdg_surface_configure`'s own comment in
`wayland.rs` for the full trace that found this.

**While debugging bug 2, two more listener structs were found undersized the
same way** — `PointerListener` was missing `frame`/`axis_source`/
`axis_stop`/`axis_discrete`/`axis_value120`/`axis_relative_direction`
(`wl_pointer` v5/v5/v5/v5/v8/v9) and `KeyboardListener` was missing
`repeat_info` (`wl_keyboard` v4). Both fixed with no-op handlers for the same
reason `XDG_TOPLEVEL_EVENTS` needed `configure_bounds`/`wm_capabilities`
added: `wl_pointer_interface`/`wl_keyboard_interface` are `dlsym`'d from the
host's real `libwayland-client.so`, so their `event_count` is whatever the
host's library version actually declares, not whatever this file happens to
have a listener field for, and a wire event past the end of a too-short
listener array is exactly this crash. Neither has actually been observed to
fire in a captured run yet — they are defensive, following the same
protocol-version-vs-listener-size reasoning that explained bug 2, not each
individually confirmed the way bug 2 was.

**Verified, not inferred:** `vkQueuePresentKHR` went from 0 to 663 calls on
Wayland (668 on X11, same window, same run length), `vkCreateSwapchainKHR`
now succeeds, and the process completes a full run and reaches
`APP_READY (Landing)` repeatedly with no crash — checked across several
consecutive launches, not once. **Not yet verified with a screenshot.** The
desktop session locked partway through this work (screen-idle timeout, not
caused by anything here) and did not unlock again before this session ended.
Everything above is measured through the engine's and Mesa's own return
values and call counts, which is real evidence that frames are being
produced and handed to the compositor — it is not the same claim as "a
screenshot shows Roblox on screen", and the two should not be conflated. Take
that screenshot (`docs/NEXT.md`'s own note on GNOME's `org.gnome.Shell.
Screenshot`/portal screenshot mechanisms working where X11 tools cannot
capture a native Wayland surface still applies) before treating this as
fully closed rather than "presentation demonstrably works; pixels on screen
not yet independently confirmed".

**Do not re-run:** blaming `Invalid currentExtent -1x-1` in FLog by itself —
see above, it is not diagnostic on its own. Do not re-add the "swap listener
after `WINDOW` exists" pattern for `xdg_surface` — see bug 2.

### 1a (cont.) The window is a libadwaita window now, and the engine sits inside it

Landed 2026-08-02. The engine's `wl_surface` was its own `xdg_toplevel`, which
is why there was no titlebar: the canvas *was* the window. It is now a
`wl_subsurface` of a GTK4/libadwaita toplevel built by
`cordial_shell::host_window` — the same definition the shell binary uses — and
positioned over that window's content area.

**Verified by running:** three consecutive 25-second launches reach
`APP_READY (Landing)` with 547, 548 and 550 `vkQueuePresentKHR` calls, no crash;
a screenshot shows the libadwaita header bar above Roblox's landing page, which
the same session's pre-change binary does not have. `WAYLAND_DEBUG=1` shows
`wl_subcompositor.get_subsurface`, `wl_subsurface.set_desync`,
`set_position(25, 71)` and `xdg_toplevel.set_app_id("Cordial")` on the wire —
the app_id had previously only ever been checked by a unit test against the
desktop entry, never observed.

**Resize was verified, and not by dragging.** A temporary local patch (not
committed) called `gtk_window_set_default_size` from a timer at 10s and 16s into
a run, 1280x721 -> 700x460 -> 1500x900. Both took effect with the engine live:
screenshots show Roblox's landing page re-laid out at each size under the header
bar, the run reached `Landing` and presented 553 frames, and nothing crashed.
That exercises the same path a compositor-driven resize takes — content
allocation changes, `sync_canvas_geometry` moves and resizes the subsurface,
`surface_resized` reaches the engine, Mesa rebuilds the swapchain — with a
different trigger. Dragging the window edge by hand is still untested, because
there is no Wayland-safe way to do it from automation.

**What still needs a human, with the exact commands.** Both need a keystroke or
a click, which nothing here may synthesise (see AGENTS.md).

```bash
# 1. Does the engine draw its own text when no soft keyboard is reported?
#    The experiment §1's correction above asks for. Click a field, type "abc".
CORDIAL_KEYBOARD_REPORT=1 CORDIAL_TRACE_TEXT=1 CORDIAL_WAYLAND=1 \
  ./target/release/cordial-run --lib-dir <lib> --apk <apk> \
  --host-libc --game-activity --run 120

# 2. Does the zwp_text_input_v3 "has no event 8" freeze still happen, and on
#    which object? Click into a field; WAYLAND_DEBUG prints every event with
#    its object id, so the id that receives opcode 8 can be matched against
#    what created it earlier in the same log.
WAYLAND_DEBUG=1 CORDIAL_WAYLAND=1 ./target/release/cordial-run ... 2>&1 \
  | tee ~/.cache/ti.log; grep -n "text_input\|no event" ~/.cache/ti.log
```

**Experiment 2 is answered, and the answer did not need a click** — see
§1c below. Event 8 is `preedit_hint`, added in `zwp_text_input_v3` **version
2**, which this file's own `bind` had always asked for while the hand-written
table beside it described version 1. The table is fixed. The experiment is
still worth running once as confirmation, because nobody has yet seen mutter
send opcode 6, 7 or 8 to Cordial — but it is no longer the only way to
diagnose it.

**Three things worth knowing before touching this code.**

`wl_subsurface.set_desync` is mandatory — a subsurface starts synchronised and
its commits then wait for the parent's, and GTK only commits when it draws, so
an idle window would show one engine frame per accidental repaint.

GTK will not open a Wayland display if `GDK_BACKEND` names something else, even
after `gdk_set_allowed_backends("wayland")`; the two are separate filters and
their intersection was empty. It fails silently — `gtk_init_check` returns false
and prints nothing. This session's GNOME desktop exports `GDK_BACKEND=x11`, so
this is not a hypothetical.

Cordial's `wl_pointer` is now a second pointer object on the same seat as GDK's,
so it sees enters and clicks aimed at the header bar. `POINTER_ON_CANVAS` filters
them; without it the engine reacts to a click on the close button and the cursor
vanishes over the titlebar.

**`wl_keyboard` focus is unchanged in effect and stricter in code.** The fix that
stopped Cordial seeing keystrokes typed into other windows — `KEYBOARD_FOCUSED`,
set on `enter` and cleared on `leave`, checked before any key is processed —
still gates every key. `keyboard_enter` now additionally requires the entered
surface to be *this window's* toplevel rather than any surface of the client,
which only narrows what counts as focus. The behaviour was not re-tested against
a real keystroke, because that needs a human; the code path is a strict subset of
the one that was tested.

**Now that Wayland presents frames, [ADR-011](adr/ADR-011-wayland-and-libadwaita.md)'s
removal trigger is close but not met.** The ADR is explicit that `window.rs`
and the X11 backend are deleted "when the Wayland backend can reach sign-in",
not when it renders — and sign-in is still blocked on §2 below, unrelated to
anything in this section. X11 stays in the tree, and stays load-bearing as
the control, until that condition is actually met.

## 1b. The canvas lags the window for a frame after a resize

Reported and reproduced visually: for a split second during a resize the GTK
window is already the new size while the engine's canvas is still the old one,
leaving a black band where nothing has been drawn.

The cause is `wl_subsurface.set_desync` at `wayland.rs:895`, and it is not a
mistake — a subsurface starts *synchronised*, and desync is what lets the engine
present at its own rate instead of waiting for GTK to commit. The cost is that a
resize stops being atomic: GTK resizes the toplevel and commits immediately,
while the canvas keeps its old buffer until the engine happens to render again.

The fix is not to abandon desync. It is to be **synchronised only while
resizing** — `set_sync` on the `xdg_toplevel` configure, `set_desync` once the
engine has presented at the new size — so a resize commits atomically and normal
rendering stays independent. `INFERRED`: this is the standard remedy for a
toolkit window hosting an independently-driven surface and has not been tried
here.

Two things not to reach for. Delaying `ack_configure` until the engine has a
matching buffer makes GTK stall on the engine's frame rate. `wp_viewporter`
scaling the stale buffer hides the seam by showing a stretched old frame, which
is a different wrong picture rather than none.

## 1c. A Wayland protocol error killed a signed-in session. Reproduced logged out, and the timer turns out to be Mesa's

Reported once, on a real signed-in session, minutes after reaching the home
page:

```text
[roblox] datamodel notification: LUA_HOME_PAGE_LOADED
[roblox] datamodel notification: HOME_PAGE_INTERACTIVE
[cookies] periodic: saved 4 domain(s), 5032 bytes
Gdk-Message: 14:10:43.968: Error 71 (Protocol error) dispatching to Wayland display.
```

**Correction: it does happen logged out, and the unmuting worked.** The
paragraph that used to stand here said reproducing it needed a signed-in
account. It does not. One run in eight, logged out, 22-second runs on this
compositor, immediately after `app ready: Landing`:

```text
[roblox] datamodel notification: APP_READY Landing
[roblox] app ready: Landing
[stub] ZSTD_trace_compress_begin
[wayland] wp_commit_timer_v1#105: error 1: Commit already has timestamp

Gdk-Message: 16:04:10.242: Error 71 (Protocol error) dispatching to Wayland display.
```

So the object and the reason are now on the record: `wp_commit_timer_v1`
error 1, which is `commit_timer_v1.error.timestamp_exists` — a second
`set_timestamp` on a surface that already had one before its next commit.

Five earlier logged-out runs (two at 120s and 180s before any change, three at
180s after) reached `APP_READY (Landing)` with no protocol error, which is
consistent with roughly one in eight rather than with "does not happen".

### The timer is Mesa's, on Cordial's own canvas. It is not GTK's, and `queue_commit` is not a commit

**Correction, and this paragraph replaces the one it grew out of.** What used to
stand here read the error as "Cordial commits the *parent* toplevel itself
(`host.queue_commit`), and GTK also drives that same surface through its frame
clock. Two clients of one surface." It also called `wp_commit_timing_v1` "GTK's
frame-timing protocol, on a surface GTK owns". **Every clause of that is wrong.**
`WAYLAND_DEBUG=1` on a run that reproduced the error says so by object id — one
run in thirteen, 20-second runs, logged out. The log is the whole answer.

The surfaces, from that log:

```text
-> wl_compositor#4.create_surface(new id wl_surface#47)      GTK's toplevel
-> wl_compositor#108.create_surface(new id wl_surface#76)    the engine's canvas
-> wl_subcompositor#104.get_subsurface(new id wl_subsurface#75, wl_surface#76, wl_surface#47)
```

and the object the compositor killed the connection over:

```text
{mesa vk display queue} -> wp_commit_timing_manager_v1#63.get_timer(new id wp_commit_timer_v1#105, wl_surface#76)
{mesa vk display queue} -> wp_commit_timer_v1#105.set_timestamp(0, 197598, 391623000)
wl_display#1.error(wp_commit_timer_v1#105, 1, "Commit already has timestamp")
```

The timer belongs to **Mesa's Vulkan WSI**, and it is attached to
**`wl_surface#76` — the engine's own canvas subsurface**, not to GTK's toplevel.
GDK creates no `wp_commit_timer_v1` anywhere in the log; every `set_timestamp`
on #105 is Mesa's, one per present.

`HostWindow::queue_commit` is `gtk_widget_queue_draw`. It does not emit
`wl_surface.commit` and never did — it asks GTK to repaint, and GTK's own commit
is what latches `set_position`. Cordial sends no request whatever on GTK's
surface. Counted over a 45-second run: `wl_subsurface.set_position` fired **0**
times and `queue_commit` **0** times after startup, because `sync_canvas_geometry`
acts only when the content rectangle moves and on an untouched window it never
does.

What the wire does show is Mesa issuing, for every present of #76:

```text
-> wl_surface#76.attach(wl_buffer#122, 0, 0)
-> wl_surface#76.damage(0, 0, 2147483647, 2147483647)
-> wp_commit_timer_v1#105.set_timestamp(0, 197598, 391623000)
-> wp_fifo_v1#106.set_barrier()
-> wp_fifo_v1#106.wait_barrier()
-> wl_surface#76.commit()
-> wp_fifo_v1#106.wait_barrier()
-> wl_surface#76.commit()
```

two commits per present, driven from two of Mesa's event queues at once —
`{mesa vk display queue}` and `{mesa vk surface 76 swapchain 1 queue}` interleave
throughout the trace. Immediately before the error a present was issued **1 ms**
after the previous one against an otherwise steady 20 ms cadence, and
`wl_buffer#122` was re-attached before its `release` had been dispatched. Two
threads inside one swapchain's present path is what that looks like, and a second
`set_timestamp` before the intervening commit is what the compositor refused.

**Not established: whether Cordial provokes it.** The one thing Cordial does that
an ordinary Vulkan client does not is hand Mesa a `wl_display` that GDK also owns
— and there are *two* Vulkan swapchains on that connection, because GTK renders
through Mesa's WSI too and took commit timers on `wl_surface#47` earlier in the
same log. Nothing was measured either way.

**A rate to plan against, so nobody reads a clean run as a fix.** Sixteen
25-second baseline runs gave one occurrence; thirteen 20-second `WAYLAND_DEBUG`
runs gave one. In the reproducing log the error fired ~3.6 s into a presenting
burst, so the exposure that counts is **frames presented, not seconds elapsed** —
on the order of one in ten thousand presents. A 240-second run with continuous
input (~6,000 presents) came back clean, and that is **not** evidence of
anything.

**Do not re-run:** the "two committers on one surface" theory, or a control with
`queue_commit` suppressed. Both are answered above, by object id.

### Why that line was the whole of the evidence, and why it will not be again

Errno 71 is `EPROTO`, and **only** a compositor-sent `wl_display.error`
produces it. Measured, by asking mutter to bind a global it never advertised:

```text
wl_registry#2: error 0: global wl_compositor (999999) is unavailable
roundtrip=-1 wl_display_get_error=71 (Protocol error) errno=71 (Protocol error)
```

So the compositor did name the object and the reason. **GDK then threw it
away.** GTK4 calls `wl_log_set_handler_client` with a handler that logs at
`G_LOG_LEVEL_DEBUG`, and debug is dropped unless `G_MESSAGES_DEBUG` names the
domain — so libwayland's one useful line is discarded about 50ms before GDK
prints its errno and calls `_exit(1)`. Confirmed by planting the same
deliberate bad `bind` inside `open()` and launching the real client: the entire
output was one `Gdk-Message` line, byte-for-byte the shape of the report above.
With `G_MESSAGES_DEBUG=all`, the same run also printed

```text
(Cordial:96812): Gdk-DEBUG: wl_registry#107: error 0: global wl_compositor (999999) is unavailable
```

`cordial_shell::host_window::unmute_waylands_own_errors` now installs a
`Gdk`-domain handler that re-emits those lines as `[wayland] ...` regardless of
`G_MESSAGES_DEBUG`, filtered so the ~122 portal-settings debug lines GDK also
emits per launch stay out of the way. Verified with the deliberate error, three
consecutive launches, against the same three launches of the pre-change binary
that printed nothing.

**So the next occurrence is self-diagnosing.** Whoever hits it again should
paste the `[wayland] <interface>#<id>: error <code>: <reason>` line — that
single line names the offending object and the compositor's own words for what
was wrong with it, which is the whole answer.

### What was found instead, and it is a real bug: the text-input table described the wrong protocol version

Chasing the above through `WAYLAND_DEBUG=1` turned up a different, definite
defect, and **corrects what §1a's module doc said about it.**

`wayland.rs` binds `zwp_text_input_manager_v3` at version 2 — measured on the
wire, `wl_registry#107.bind(26, "zwp_text_input_manager_v3", 2, ...)`, because
GNOME 50's mutter advertises 2. A `zwp_text_input_v3` created by a v2 manager
**is** a v2 object; the version a client passes to `wl_proxy_marshal_flags`
does not change what the compositor believes it may send. And version 2 adds
three events to version 1's six: `action` (6), `language` (7) and
`preedit_hint` (8).

The hand-written table in `wayland.rs` declared six. **Event 8 is
`preedit_hint`** — checked against `wayland-scanner`'s own generated table for
the shipped XML, which matches the corrected table name-for-name and
signature-for-signature. The old comment's explanation, that "event 8 exists in
`zwp_text_input_v2`", named a different protocol and was wrong.

**This is not the EPROTO above, and conflating them would send the next person
the wrong way.** An opcode past the end of a client's own table is refused
*inside libwayland*, not by the compositor; it leaves `errno` at whatever it
happened to be. Reproduced standalone against this compositor by binding
`wl_seat` at version 8 behind deliberately short and complete tables, five
times each in one session:

```text
SHORT  bound wl_seat v8, table declares 1 event(s): roundtrip=-1  wl_display_get_error=11 (Resource temporarily unavailable)
FULL   bound wl_seat v8, table declares 2 event(s): roundtrip= 4  wl_display_get_error=0 (none)

5/5 SHORT runs killed the display; 5/5 FULL runs were clean.
```

11, not 71 — and the whole display dies, every client on the connection
included, which is the freeze §1a recorded rather than a crash.

The fix is the complete v2 table, three no-op listener slots, and `bind` taking
its version from `TEXT_INPUT_MANAGER_INTERFACE.version` so the request and the
table cannot drift apart again. Two unit tests pin it.

**Still unverified:** that mutter ever actually sends opcode 6, 7 or 8 to
Cordial. Those need an input method composing into a focused field, which needs
a click. What *is* established is that the object is live all session — the
trace shows `zwp_text_input_v3#71.enter(wl_surface#47)` arriving as soon as the
toplevel takes keyboard focus, with no `enable` sent and no field clicked — so
there is no window in a session where a v2 event would be harmless.

### Ruled out as the cause of the EPROTO, so nobody re-runs them

- **Short listener arrays on `wl_pointer`/`wl_keyboard`/`wl_registry`.** Dumped
  the host `libwayland-client.so`'s own tables directly: `wl_pointer` declares
  11 events, `wl_keyboard` 6, `wl_registry` 2, and the structs in `wayland.rs`
  have exactly 11, 6 and 2 fields. §1a's defensive padding was correct.
- **§1a's padding being load-bearing.** It is not, on this compositor:
  `wl_seat` is bound at version 1, so mutter never sends `frame` or the axis
  events at all — zero `wl_pointer#70.frame` in a full 120s run. Keep the
  slots; do not cite them as tested.
- **Anything reachable without signing in.** Five runs, no error.

### One thing fixed on the way, of the same family and never observed to fire

`open()` registered the `wl_registry` listener with a pointer to a
`Globals` **local**, and the registry proxy is never destroyed. Any global
appearing later in the session — a monitor hotplug, a seat — would run
`registry_global` against `open()`'s long-dead stack frame. That is §1a bug 2
with a write instead of a call. Now `Box::leak`ed. Not observed to fire; fixed
because it is one of the two shapes this file has already been bitten by.

## 1d. The frame rate. `vkQueuePresentKHR` over a fixed window measures the engine's idle throttle, not the frame rate

**This corrects the metric the rest of this file uses**, including §1a's own
"547, 548 and 550 over three consecutive 25-second runs" and the 1286-1625 over
120-180 s recorded elsewhere. Those numbers are real and they are not frame
rates. Sampled per second instead of totalled, the curve is:

```text
[instr] t=  4.0s presents/s=  59.5
[instr] t= 13.1s presents/s=  60.0
[instr] t= 16.3s presents/s=   1.0
[instr] t= 31.5s presents/s=   1.0     ... and 1.0 for the rest of the run
```

About 60 per second for roughly the first thirteen seconds, and then **exactly
1.0 per second**, for as long as the run lasts. Every historical total is that
curve integrated: 60x13 + 1x12 is 792, 50x11 + 1x14 is 564, and the 526-658
spread across sixteen 25-second baseline runs is the burst ending a second or
two earlier or later.

**The cause is that nothing is happening.** Deliver pointer motion through
Cordial's own input path — `input::deliver_touch` plus `input::pass_mouse_move`,
which "Debugging facts that cost real time" below already permits because no
compositor is involved — and the rate holds at 50-60 per second for a whole
240-second run with no collapse at all. Turn the motion off mid-run and it drops
to 1.0 within two seconds; turn it back on and it is at 50 within one. Both
directions, twice, in one run.

**The control that matters: it is not the Wayland backend.** The identical
binary on X11, no input, same session, shows the same 60-then-1.0 collapse at the
same point in the run — three times on each backend. So this is the engine's own
idle behaviour on the app shell, not `wayland.rs`, not the subsurface, and not
anything about commits.

**Ruled out:** that the engine thinks it lost the window.
`onWindowFocusChangedNative(true)` re-sent 25 s into a run, after the collapse,
returns `Ok(Some(()))` and changes nothing — still 1.0 per second. Do not spend
another session on the focus native; `native/game_activity.cpp` already sends it
once at surface handoff and that call is doing its job.

**What this does and does not say about 45 fps against Sober's 70.** Measured
here, with continuous input:

| | presents/s |
|---|---|
| windowed, 1280x721 | 49-60 |
| fullscreen, 3440x1394 | 49.4 mean over 26 samples |
| idle, any size | 1.0 |

**The rate is the refresh rate of the output the window is on**, and quadrupling
the pixel count does not move it: this desk has a 1920x1200@60.002 panel and a
3440x1440@49.998 monitor, and the numbers above are those two refresh rates. So
the engine is not GPU-bound here at all — it is hard vsync-locked, because Mesa
is using `wp_fifo_v1` on this surface, visible on the wire as
`set_barrier`/`wait_barrier` around every commit.

A reported 70 is *above* both of these refresh rates, so whatever Sober is doing
is not FIFO-throttled. That is a swapchain present-mode difference; it lives in
`vulkan.rs`, which this work did not touch. **Nothing here reproduces or explains
a 45.** With input the engine sits exactly on refresh at both sizes tried. The
owner's 45 is presumably inside an experience, where there is real scene load and
the answer may be entirely different; that was never measured.

**How to measure it next time.** Per-second deltas of
`android::glcount::QUEUE_PRESENT`, with input being delivered, and say which
resolution. A total over a fixed window with an idle app shell answers a
different question than the one being asked.

**Unexplained and left alone:** `ALooper_pollOnce` is called about **9.5 million
times per second**, constantly, on both backends. Cordial's own pump loop
accounts for 20-60 of those per second, counted separately in the same runs, so
the rest is an engine thread spinning. It costs a core and it was not
investigated.

## 1d(ii). The present mode was the ceiling. Asking for MAILBOX takes the landing page from ~36 to a flat 60

§1d ended by saying the difference against Sober "is a swapchain present-mode
difference; it lives in `vulkan.rs`, which this work did not touch". It does, and
this is that work.

**Cordial never requested a present mode at all.** Nothing in the tree mentioned
one, so `VkSwapchainCreateInfoKHR::presentMode` was whatever the engine put
there, and the engine puts `VK_PRESENT_MODE_FIFO_KHR` — the only mode the
specification guarantees exists, and a vsync lock.
`vulkan.rs` now interposes `vkCreateSwapchainKHR` and substitutes
`VK_PRESENT_MODE_MAILBOX_KHR` when the driver advertises it, falling back to
whatever the engine asked for when it does not.

**Measured, with pointer motion delivered in-process at 100 Hz for the whole of
every run**, `--run 120`, windowed at 1280x721 on Wayland, engine at the logged-out
landing page, GameMode registered in all four runs. Build
`v0.2.0-3-g1e1318a-dirty`. Outputs on this desk: eDP-1 1920x1200 at **59.88 Hz**
and HDMI-1 3440x1440 at **49.96 Hz** (`xrandr`). Per-10-second present rate, the
last 60 s of each run once it had settled:

```text
                          per-10s samples from t=30s on            120s total
run 1  off  (FIFO)    41.1 37.4 37.5 37.5 37.5 37.5 35.3 35.0 35.0 36.1   4678
run 3  off  (FIFO)    49.9 49.8 49.7 49.8 49.6 49.9 49.9 49.8 49.8 49.9   5886
run 2  auto (MAILBOX) 60.0 60.0 60.0 60.0 60.0 60.0 60.0 60.0 60.0 60.0   7091
run 4  auto (MAILBOX) 60.0 60.0 60.0 60.1 60.0 60.0 60.0 60.0 60.0 60.0   7091
```

The two MAILBOX runs returned the same 120-second total to the present: 7091.

The control is the same binary in the same session with the substitution turned
off, which is the only thing that makes this a result rather than a number. Both
conditions were run twice, alternating, and GameMode was registered in all four.

**FIFO is variable, MAILBOX is not.** The two controls disagree with each other —
one settled on the 49.96 Hz refresh, the other on 35-37.5, and nothing was
deliberately changed between them. Both are at or below refresh, which is what
FIFO enforces. Every MAILBOX sample in two runs is 60.0 or 60.1. So the honest
statement is a range against a constant: **FIFO 35-50, MAILBOX a flat 60**, and
the floor moved further than the ceiling did.

**This rules out "the engine simply cannot go faster."** The same engine, same
1280x721, same session, goes from 35 to 60.0 on nothing but the present mode. The
FIFO figure was never the engine running out of work.

**One correction to §1d above.** It says the windowed rate "is the refresh rate of
the output the window is on" and gives 49-60 for 1280x721. Run 3 reproduces that
exactly; run 1 does not, sitting at 35-37.5 for six consecutive samples, below
both of this desk's refresh rates. So the claim holds sometimes and is not the
whole story. What produces 37.5 on a 49.96 Hz output was not chased, and the fix
does not depend on the answer.

**The 60.0 is almost certainly the engine's own cap and not a new ceiling.** It
is 600 presents per 10 s, repeated, to the sample. Do not read it as "MAILBOX
gives 60"; read it as "MAILBOX stops the display holding the engine below what it
was already willing to do". A machine whose engine target is higher, or an
experience with real scene load, will not necessarily see this shape.

**The driver here advertises MAILBOX and FIFO and *not* IMMEDIATE**, printed by
the substitution itself. That is the argument for asking rather than assuming:
a client that had been written to force IMMEDIATE would have had to fall back on
this very common Intel/Mesa configuration.

`CORDIAL_PRESENT_MODE=off` is the documented control and stays supported.
`auto` (the default) prefers MAILBOX only — not IMMEDIATE, which also uncaps the
rate but tears.

### Reproducing this

The 100 Hz pointer motion came from a probe thread calling
`input::pass_mouse_move` directly, added to `load.rs` for the measurement and
**removed again before this landed** — `grep -rn perf_probe` finds nothing. It is
half a screen of code to put back and nothing here depends on it being in the
tree. Without it, keep a real pointer moving over the canvas for the whole run,
which drives the same path:

```bash
XDG_DATA_HOME=~/.cache/cordial-perf CORDIAL_COUNT_GL=1 \
  CORDIAL_PRESENT_MODE=off  just client --run 120     # control
XDG_DATA_HOME=~/.cache/cordial-perf CORDIAL_COUNT_GL=1 \
  CORDIAL_PRESENT_MODE=auto just client --run 120     # MAILBOX
```

`vkQueuePresentKHR` in the report at the end, divided by 120, is the rate — but
**only if the pointer was moving the whole time**. Stop moving it and you are
measuring the idle throttle again, which is the trap §1d exists to describe.

## 1d(iii). GameMode is requested, and MangoHUD is a setting that knows when it is absent

Two smaller pieces landed with the present mode.

**Feral GameMode**, in `load.rs`. A D-Bus request rather than a wrapper: nothing
is linked and `gamemoderun` is not involved. `RegisterGame(i pid)` on
`com.feralinteractive.GameMode` before the engine loads,
`UnregisterGame` before `_exit`. On by default, which is what Sober does;
`CORDIAL_GAMEMODE=0` is the off switch and the control. All three paths were run
on this machine and printed:

```text
[gamemode] registered pid 605422: performance governor, raised priority, ...
[gamemode] off (CORDIAL_GAMEMODE=0)
[gamemode] not available, continuing without it: no session bus
```

The third is the one that matters, and it was produced by pointing
`DBUS_SESSION_BUS_ADDRESS` at a path that does not exist. **A missing gamemoded
must never fail a launch** — most machines have none — and the run that produced
that line went on to build its symbol table and load the engine exactly as the
other two did.

**MangoHUD**, in `launch.rs` and the Settings window. `MANGOHUD=1` on the client
process is the whole mechanism, because MangoHUD is a Vulkan implicit layer. The
work is not the switch, it is `launch::mangohud_layer`: **MangoHUD is not
installed on this machine**, and `MANGOHUD=1` with no layer present is not an
error — the client starts, no overlay appears, and nothing says why. So the
layer is looked for across the Vulkan loader's own implicit-layer search path
plus the Flatpak extension mount point, and when it is absent the Settings row
is insensitive and names the packages to install instead of being a switch that
appears to work. That defect has shipped on this page twice; this is the check
that stops a third.

## 1d(iv). What was considered from Lutris's list and rejected

The request behind §1d(ii) and §1d(iii) was to look at what Lutris does. Most of
it does not apply here, and recording that is half the value of this file:

- **DXVK, VKD3D, esync, fsync, every `WINE*` variable** — there is no Wine and no
  Direct3D anywhere in this project. Not applicable, not a judgement call.
- **`DRI_PRIME`, `__NV_PRIME_RENDER_OFFLOAD`, ICD selection** — hybrid-graphics
  selection is a real feature for a two-GPU laptop. This machine has one GPU, an
  Intel UHD (Raptor Lake-P), so **it cannot be tested here and must not ship
  untested**. Worth a later task on hardware that has two.
- **`mesa_glthread`** — GL only. The engine renders through Vulkan on this path
  (the GLES counters read zero in every run above while `vkQueuePresentKHR` read
  thousands), so it would do nothing. Rejected.
- **Shader cache location and size** — genuinely plausible and *not done here*.
  The engine already writes `shadercachevk.bin` into the profile, so the cache
  exists and is per-profile; what is unmeasured is whether it is being evicted or
  is too small, and a cache setting added without that measurement would be a
  knob nobody could tell was working. Left as a real candidate with a real first
  step: instrument whether the file is being rewritten between launches.

## 1e. Fullscreen moves the canvas through two wrong (position, size) pairings

Driven in-process with `gtk_window_fullscreen` from a scripted timer — allowed,
and not input injection — twice per direction in one 240-second run. What Cordial
actually sends, logged at the call:

```text
script: fullscreen
  set_position(0, 46) size=1280x721      <- fullscreen position, windowed size
  surface_resized -> 3440x1394
script: windowed
  set_position(12, 58) size=3440x1394    <- an intermediate inset, fullscreen size
  surface_resized -> 1280x721
  set_position(25, 71) size=1280x721     <- settled, about three seconds later
```

`sync_canvas_geometry` reads one `content_rect()` and applies its position and
its size together, so these are not Cordial tearing them apart — GTK reports the
rectangle in that order as the transition settles, and each intermediate is
faithfully forwarded. Exactly **one** `surface_resized` per transition, so there
is no swapchain-rebuild storm; the cost is that the canvas is visibly out of
register with the window for the seconds in between.

**Which size the intermediate `set_position(12, 58)` carries is a race.** Across
four leave-fullscreen transitions in two runs it was `3440x1394` twice and
`1280x721` twice — GTK had applied the position half of the transition and the
size half in either order by the time the pump sampled it. That is the same
non-atomicity §1b is about, arriving through the allocation rather than through
the buffer, and §1b's `set_sync`-while-resizing remedy is still the untried
candidate for both.

**Why it looks permanent.** At 1.0 present per second (§1d) the wrong frame stays
on screen until the engine draws again, and the engine draws again when the user
moves the mouse — which is exactly what dragging the window edge does. With input
flowing the same transition corrects itself in about three seconds and the rate
never leaves 50/s.

**Not reproduced: a state that stays broken.** The report is of a canvas that
stays wrong until the window is dragged. What is reproducible from a timer is a
transient. `gtk_window_fullscreen` may not exercise the same path as a
compositor-driven fullscreen, and the owner's case may involve a different size
or monitor. Anyone with a keyboard should check it by hand before this is called
closed.

**Tried and it did not help: `onSurfaceRedrawNeededNative` on a geometry
change.** `window.rs` drives that native from the final X11 `Expose` and this
backend drove it from nowhere at all, so the argument was that an idle engine has
nothing telling it the canvas moved. Two otherwise identical 240-second runs,
minutes apart in one session, over the idle fullscreen cycle: **~75 presents
without the call and ~74 with it**, and the per-second shape is the same either
way. The engine already repaints on `surface_resized` by itself. The call was
removed again rather than left in looking like a fix; `sync_canvas_geometry`
carries a comment saying so, so that the next person reaches for something else.

**How both of the above were driven, since it needs no human.** `CORDIAL_INSTR=1`
plus `CORDIAL_SCRIPT=20:motion-on,70:fullscreen,100:windowed,130:motion-off,
160:fullscreen,190:windowed,220:motion-on` runs the whole timeline from
`looper::pump` in one launch — `gtk_window_fullscreen` for the transitions and
Cordial's own `input::deliver_touch`/`pass_mouse_move` for the pointer, so no
compositor is involved and nothing can reach the developer's session. One launch
covers both fullscreen directions twice, with and without input, and prints
presents, looper polls, pump iterations and the content rectangle every second.
Use it instead of a handful of short runs; every launch is a window on somebody's
desktop.

**Three readings from a `WAYLAND_DEBUG=1` run that are wrong, retracted here
before they mislead anyone.** That run showed (a) the pump thread emitting no
tick for 12.6 s after leaving fullscreen, (b) presents at 0-3/s for a further
twelve seconds with input still flowing, and (c) fullscreen running at 20-25/s.
All three looked like findings. All three are artefacts of the tracer: the same
script without `WAYLAND_DEBUG`, minutes later in the same session, has **no tick
gap over two seconds anywhere in the run**, holds 50/s straight through both
transitions, and averages 49.4/s in fullscreen. `WAYLAND_DEBUG` writes a line per
request on a connection three parties share. **Do not measure timing under it** —
use it for object identity and request order, which is what it is good for and
what settled §1c.

## 2. Sign-in itself

Without a session the client sits on the landing page. Avatar thumbnails fail
against user id 0 and there is nothing to do. `NativeUserJavaInterface` is
stubbed with an empty user.

**[`docs/design/sign-in.md`](design/sign-in.md) is the investigation.** Read it
before starting; it is careful about what is verified and what is inferred.

The short version: the blocker is **obtaining a session cookie**, not the stub
code. The engine's own HTTP client takes 401/403 from authenticated endpoints
regardless of what the Java-side user stubs return, so filling those in changes
nothing on its own.

**Good news, and it changes the plan: plain login does not need a WebView.**
Lua-rendered login is the *shipped default* in this build, established three
ways — the dex bytecode for what the native gates, the flag that controls it, and
the shipped content itself, which carries a full `Authentication.Login.*` string
table and a `LoginNative` screen name while `LoginWeb`/`LoginWebView` return zero
matches.

Reproduced here with a control:

```text
default                          nativeIsLuaLoginEnabled() -> true
FIntLuaAppLoginMethod=0          nativeIsLuaLoginEnabled() -> false
```

`CORDIAL_SIGNIN_PROBE=1` asks the engine directly.

**Captcha is still narrowed rather than settled** — there is a `CaptchaNative`
screen name, but also `Turnstile` and `CaptchaV2` strings suggesting
server-selected backends, so budget for a WebView on that path even though the
login form itself does not need one.

One correction that came out of it: the `The requested Ids are invalid`
thumbnail failure is what the **real, logged-out Android client** also produces.
It is not a Cordial defect and it is not evidence of anything.

## 2a. `CORDIAL_COUNT_GL=1`: not broken, just answering "is GLES running?" — and the
answer to that is always no

**Correction to what this section used to say.** It reported zero for
`eglCreateWindowSurface`, `eglSwapBuffers` and `glClear` on *both* backends and
concluded the instrument was broken. It is not. `vkQueuePresentKHR` on the
*same* counter reads real numbers on both backends now (668 on X11, 663 on
Wayland, for comparable runs) — checked directly with `CORDIAL_ANDROID_TRACE=1
CORDIAL_COUNT_GL=1`. The EGL counters read zero because **the engine renders
through Vulkan on both backends in this build, not GLES**, confirmed the same
way: `CORDIAL_ANDROID_TRACE=1` alone shows `vkCreateInstance`/
`vkCreateAndroidSurfaceKHR -> vkCreate{Xlib,Wayland}SurfaceKHR` on every launch,
X11 or Wayland, with no `eglCreateWindowSurface` ever appearing in the same
trace. A GLES counter reading zero is not a broken instrument here, it is a
correct answer to a question ("is the GLES path active") whose answer is no on
this hardware/driver combination. The earlier entry conflated "this counter
reads zero" with "this counter is unreliable" without checking which renderer
was actually live — exactly the control this file's own "Measuring anything"
section asks for, skipped.

The specific "suspected, not confirmed" theory this section used to carry —
that `android::mod::overrides()` appending `glcount::overrides()` *after* the
backend's own list lets the counting wrapper silently replace `window.rs`/
`wayland.rs`'s `eglCreateWindowSurface` — is **confirmed false**, by reading
`glcount::overrides()` directly rather than running anything: it registers
`eglMakeCurrent`, `glClear`, `glDrawElements`, `glDrawArrays`,
`glCompileShader`, `glTexImage2D` and `eglSwapBuffers` (or `swap_buffers_timed`
under `CORDIAL_SWAP_TIMES=1`) — no `eglCreateWindowSurface` entry at all, so
there is no key for it to collide with in the `BTreeMap` `symtab::build`
collects overrides into. It cannot be replacing something it never names.

**Still true and still the right instinct:** do not use `CORDIAL_COUNT_GL=1`'s
*EGL* counters to decide whether a backend renders, because on a Vulkan build
they will read zero regardless. `vkQueuePresentKHR` under the same flag is
reliable and is what actually answered "does Wayland ever present a frame" —
see §1a above. The engine's own FLog is close to useless for this specific
question: `SurfaceController`/`RenderJob` log at startup and then go silent for
the rest of the session on both backends, identically, so there is no
per-frame signal to read there either. What works is the Vulkan call counter,
or looking at the window.

## 2b. Audio never initialises before sign-in, and AAudio is not why

The OpenSL ES backend over PipeWire works in a standalone harness and has never
been seen carrying a single sample through the real client. The reason is not a
bug in it.

> **Correction (audio device work).** The paragraph above was right about
> laziness and wrong to stop there, because it was written without checking
> whether the backend was in the binary at all. It was not. `pkg-config` first
> on `PATH` on the development machine is Homebrew's `pkgconf`, whose
> compiled-in `pc_path` is its own Cellar directories and excludes
> `/usr/lib64/pkgconfig`, so `pkg_check_modules(PIPEWIRE ...)` reported
> libpipewire-0.3 missing on a host with pipewire-devel 1.6.8 installed.
> `CMakeCache.txt` recorded `PIPEWIRE_FOUND:INTERNAL=` and
> `cordial_liblog.dir/flags.make` recorded an empty `CXX_DEFINES`: every
> release build had compiled the `#else` branch of `pipewire_backend.cpp`, and
> `slCreateEngine` had been reporting `SL_RESULT_FEATURE_UNSUPPORTED` for a
> reason that had nothing to do with audio. `cordial_pipewire_backend_test` had
> never run either, for the same reason. `native/CMakeLists.txt` now falls back
> to `find_path` for the headers — the only thing that is wanted, since the
> library is dlopen'd — and the build reports
> `-DCORDIAL_HAVE_PIPEWIRE=1`.
>
> **The conclusion below still holds after that fix, and was re-measured.**
> Three 30-second runs to the Landing screen with the backend genuinely
> compiled in produced no `slCreateEngine` call at all (the backend prints
> `PipeWire session confirmed reachable` on its first use; it never appeared),
> so audio initialisation really is lazy and really does need something past
> sign-in. What changed is that this is now a statement about Roblox rather
> than, unknowingly, a statement about the build.

**Roblox makes exactly one `dlopen` in a 75-second run to the Landing screen:**

```text
[cordial] dlopen(libroblox.so) -> ok in 21896us
```

That is Cordial's own load. `CORDIAL_TRACE_DLOPEN=1` reports every request and
how long it took. Nothing else is asked for — no audio backend, and no
`libvulkan.so` either, which is the control: with no `flags.json` the engine
picks GLES, so the absence of a Vulkan request confirms the trace catches real
calls rather than missing them.

**The AAudio-preference theory does not survive the linkage.** `strings` shows
`FmodFallbackAaudioToOpensl`, and FMOD does prefer AAudio. But:

| | |
|---|---|
| `libOpenSLES.so` | in `DT_NEEDED` — *linked*, so `slCreateEngine` is directly callable and needs no `dlopen` at all |
| `libaaudio.so` | not in `DT_NEEDED`, and **zero** `AAudio*` undefined symbols |

So AAudio is reachable only through `dlopen`, and that `dlopen` never happens.
FMOD's backend selection has not run. Cordial providing a `libaaudio.so`, or not
providing one, cannot currently make any difference — there is nothing to fall
back *from*.

**Therefore audio initialisation is lazy, not eager**, and reaching the
logged-out Landing screen is not enough to observe it. Verifying the PipeWire
path through the real client needs something that actually plays a sound, which
means getting past sign-in. It is blocked on §1, not on itself.

**Do not re-run:** adding a virtual `libaaudio.so` to make FMOD fall back. There
is no evidence FMOD has initialised, and the fallback string is not evidence that
it has.

**Voice chat is a different path and is not covered by any of this.** The
real-Android capture has `MainScreenController: Initializing RTC audio manager`
during startup — that is WebRTC, separate from FMOD, and it is the only audio
line in the whole capture. FMOD does not log to logcat at all, which is why the
capture cannot answer the eager-versus-lazy question and the `dlopen` trace had
to. Note also that `SL_IID_RECORD` is among the referenced symbols and
`native/opensles.cpp` deliberately refuses recorder creation: correct for now,
and exactly what voice chat will need implemented later.

## 2c. Deep links reach the engine. Whether they join needs an account

`cordial-run --join-url <url>` takes a `roblox-player://` or `roblox://` link
from a browser click and hands it to the engine.
[`docs/analysis/deep-links.md`](analysis/deep-links.md) is the investigation and
`crates/cordial-runtime/src/deeplink.rs` is the code.

**The engine asks nobody for a URL.** No `Intent`, no `Uri`, no `getIntent` —
checked in a full launch and in the Waydroid capture, where every `Intent` line
belongs to Google Play services rather than to Roblox's process. The URL is
delivered *to* the engine, which makes this Cordial's statement to make rather
than a question to answer.

**What works, measured twice with a control.** Publishing on the engine's own
linking message during bring-up:

```text
MessageBus.publishRaw("Linking.detectURL", "{\"url\":\"roblox://…\"}")
```

makes the app shell answer, by the first `APP_READY`, with

```text
Game.launch  {"placeId":1818,"referralPage":"DeepLink","joinAttemptId":"fe7bec78-…"}
```

`placeId` and `referralPage` are the engine's words — Cordial passes the URL
through as one opaque string and never parses it. `isColdStartDeeplinkToGame()`
goes false -> true across the same delivery. `CORDIAL_DEEPLINK_NO_PUBLISH=1` is
the control: identical launch, publish suppressed, neither observable moves.

**Two things are not done, and the first is the important one.**

*Whether it joins is unverified and cannot be verified without an account.*
`Game.launch` is the app shell asking for an experience; every run here ends at
`app ready: Landing`, because a signed-out client belongs there. Closing this
needs §2 first, and then one signed-in launch with `--join-url`.

*`roblox-player://` links do not reach an experience.* The engine's own pattern,
the client setting `FStringGameLaunchLinkURL`, matches `roblox://` and
`robloxmobile://` and no other scheme — measured, not read off the regex alone.
That is the scheme roblox.com's desktop play button emits and the handler
Cordial is taking from Sober, so registering it and doing nothing with it is
worse than not registering it. Cordial warns when it is handed one. Translating
the desktop format (`roblox-player:1+launchmode:play+gameinfo:<ticket>+…`) into
an Android-shaped link is the obvious next step and is untested; the desktop
format carries a one-time auth ticket the Android client does not use, so it is
not a scheme swap.

`CORDIAL_DEEPLINK_PROBE=1` prints the linking protocol's own message and field
names, read out of the running engine — that is how they were established, and
it is the cheap way to check whether a Roblox update renamed any of them.

## 3. Plugins: running, but with three methods

`crates/cordial-plugins` has capabilities, a broker, manifests, user grants and a
Deno host. `crates/cordial-runtime/src/plugin_host.rs` joins it to the client:
plugins are discovered, started after bring-up, and served from the real flag
resolver. Verified in a live launch — the example plugin in
[`plugins/flag-inspector`](../plugins/flag-inspector) reads a flag the user
actually set and is refused a capability it did not request.

**Update, still true where it matters:** `presence.set`/`presence.clear`,
`notify.send`, `url.open`, and the three `events.*` methods (ADR-006) are now
real, effect-performing brokers — Discord IPC framing, the freedesktop
notification and OpenURI portals, and an event registry with ownership rules
— all implemented and tested in `crates/cordial-plugins` (see `presence.rs`,
`notify.rs`, `urlopen.rs`, `events.rs`, and `host.rs`'s `Session`, which is the
single dispatcher that authorises and performs all of them). `lifecycle.read`
now has something to push, too: `Session::push_lifecycle` delivers `launch`,
`ready` and `shutdown` to any plugin holding the capability. A first-party
plugin, [`plugins/discord-presence`](../plugins/discord-presence), exercises
the whole path — verified against a local Discord IPC test double, not a real
Discord client (none is available where this was built); see the plugin's own
source comment.

**What is still missing is the join, not the brokers.** `serve()`'s `dispatch`
in `crates/cordial-runtime/src/plugin_host.rs` is a separate, older function
that predates `Session` and still only answers `flags.list`, `flags.get` and
`log.write`, falling through to `error: not implemented yet` for everything
else — including the four methods above, which now have real implementations
sitting unused one crate away. `plugin_host.rs` was out of scope for the work
that added them (other agents were active there), so the live client still
cannot broker Discord presence, notifications, URL-opening or plugin events
until `dispatch` is replaced with (or delegates to) `cordial_plugins::host::
Session::handle`, and `push_lifecycle` is called from wherever the client's
own launch/ready/shutdown transitions are detected. That wiring, plus
`flags.write` and `flags.write.dynamic`, is what remains.

See [ADR-003](adr/ADR-003-plugin-isolation.md) for why isolation is by process,
[ADR-004](adr/ADR-004-plugin-asset-overrides.md) for why plugins cannot replace
Roblox's assets, and [ADR-005](adr/ADR-005-flag-service.md) for why flag writes
are two capabilities.

---

## 4. Accessibility — the AT-SPI bridge is built and verified live; whether Roblox ever reaches it is not

New in this change: `native/accessibility.cpp` hooks
`android.view.accessibility.{AccessibilityManager,AccessibilityNodeInfo,
AccessibilityEvent}` the same way every other class in `android_classes.cpp`
answers a platform service, mirrors whatever the engine populates into a
small registry, and `crates/cordial-runtime/src/android/accessibility.rs`
republishes that registry as a real `org.a11y.atspi.*` application on the
accessibility bus — Linux's TalkBack equivalent, which is what Orca and other
screen readers actually read.

**This was written and tested with no Roblox APK available in the
environment.** `crates/cordial-runtime/src/bin/load.rs --apk` needs one the
user supplies, and none was reachable — a Waydroid instance was present but
came back from a freeze/thaw cycle with a broken guest network (`adb`: `no
route to host`, persisting past several retries and a `waydroid show-full-ui`
re-attach; not debugged further, since resurrecting one stale container is
not the point of this change). That has two consequences, and they should
not be conflated:

**Verified live, with evidence, no Roblox involved:** the AT-SPI-facing half
of the bridge. `crates/cordial-runtime/examples/accessibility_probe.rs`
seeds three synthetic nodes (a button, a checkbox, a label — clearly labelled
as fixtures, never Roblox data) straight into the same C++ registry
`AccessibilityNodeInfo`'s real hooks write to, then starts the bridge for
real. Queried externally over the actual accessibility bus:

```text
$ busctl --address="unix:path=/run/user/1001/at-spi/bus" tree :1.253
└─ /org/a11y/atspi/accessible
   ├─ /org/a11y/atspi/accessible/node
   │  ├─ /org/a11y/atspi/accessible/node/1
   │  ├─ /org/a11y/atspi/accessible/node/2
   │  └─ /org/a11y/atspi/accessible/node/3
   └─ /org/a11y/atspi/accessible/root
```

with `GetRole`/`GetRoleName`/`GetState`/`GetExtents` all reading back
correctly (`push button` for the seeded button, `[1124075776, 0]` for its
state word — hand-verified against `ATSPI_ROLE_PUSH_BUTTON`/the
`AtspiStateType` ordinals in `/usr/include/at-spi-2.0/atspi/atspi-constants.h`,
bit-for-bit), and — after fixing a real bug found this way, not guessed —
`org.a11y.atspi.Registry`'s own tree lists Cordial as an embedded
application, meaning `busctl --user tree org.a11y.atspi.Registry`-style
discovery works, not only a direct connection to a known bus name.

**The bug, for the record:** the first `Socket.Embed` call sent
`&(bus_name, ROOT_PATH)` as the method body, which `zbus`/`zvariant` encodes
as *two* top-level arguments (`ss`) rather than the *one* struct-typed
argument (`(so)`) the real method takes — confirmed via `gdbus introspect`
against the live registry before writing the fix. The registry's own daemon
did not error, it simply never replied (`NoReply: Remote peer disconnected`),
which would have been very easy to misdiagnose as a permissions or
bus-address problem rather than a wire-format one. Fixed in
`android::accessibility::connect` by wrapping the struct in an extra
one-element tuple and using a real `OwnedObjectPath` rather than a bare
`&str` for the path half. **Correction to this task's own brief:**
`busctl --user tree org.a11y.atspi.Registry` alone does not reach the
accessibility bus — `--user` targets the *session* bus, and the AT-SPI bus is
a separate socket obtained via `org.a11y.Bus.GetAddress`; the working form is
`busctl --address="unix:path=<that address>" tree org.a11y.atspi.Registry`.

**Not verified, and not claimed:** anything about what Roblox's engine
actually does. `native/accessibility.cpp`'s own header comment lays out the
structural question this leaves open — real Android's accessibility tree is
*pull* (`AccessibilityNodeProvider`, Java/Kotlin app code the platform calls
into on demand), not *push*, and per this project's own established finding
on `MainGameActivity.bootstrapTheApp()`, Java/Kotlin application logic
cannot execute under Cordial at all. If Roblox's Android build implements
accessibility that way, no amount of hooking `AccessibilityNodeInfo` reaches
it, for the same structural reason hooking getters alone never reached
FastFlags bootstrap. What *is* plausible, and is what this file is written
to catch if true, is the engine building nodes directly over JNI the way it
does everything else in `android_classes.cpp` (a native-to-Java push, no app
subclass involved) — but only a live run with `CORDIAL_ACCESSIBILITY=1
CORDIAL_JNI_TRACE=1` (or `--dump-classes`) against a real APK, past sign-in,
with a genuine assistive technology attached, distinguishes the two. **Do
this before claiming Cordial makes Roblox screen-reader-usable** — everything
in this section is "the pipe works", not "there is water in it".

**Also not done, on purpose:** forwarding
`AccessibilityManager.sendAccessibilityEvent` as a real AT-SPI signal — it is
captured (see `cordial_accessibility_next_event`) and currently only logged
to stderr by the poll loop, because getting `org.a11y.atspi.Event.Object`'s
own signal shapes right needs the same live-verification treatment `Embed`
just got, and this change was long enough already. `Action::DoAction` also
always answers `false`, honestly rather than as a stub that lies — see
`android/accessibility.rs`'s own header comment on why there is no receiver
for an invoked action to reach yet, the same shape of gap as the provider
question above.

**Deaf users need nothing from this work** — captions and visual alerts are a
different, unbuilt piece of work; nothing here touches it, and nothing cheap
and real for it turned up while doing this one.

---

# Do not re-run these

Each was tested and each cost time. The evidence is the point.

**The futex that used to hang startup**
- Never an EGL/GBM surface handshake. It is the engine's ordinary wait primitive
  — offset +0x0C of a 64-byte-aligned object, the same class and call site all
  sixteen idle `RBX Worker` threads park on.
- Never an unserviced `ALooper`. Tested directly: the main thread pumped
  `epoll_wait` continuously on a dedicated thread while the worker still blocked
  at the identical futex.
- It was in `nativeAppBridgeStartLuaAppDM`, not `StartAppWithParams`.
- The block and the crash were **one bug, not two** — a completion handshake with
  a thread that segfaulted before it could signal.

**The frame rate**
- Window focus is not it. `onWindowFocusChangedNative(true)` and
  `onContentRectChangedNative` are both sent; the rate did not move.
- Frame-callback starvation is not it. `AChoreographer_*` is not imported at all.
- `FIntReactSchedulerMinFrameRate` set to 60 changed nothing.
- The render job binds its DataModel fine — `No DM yet` appears exactly twice,
  transiently, around 2.0 s.
- Graphics-quality FastFlags do nothing here. `DebugFRMQualityLevelOverride` and
  both MSAA overrides, at every prefix, left shader count, target size and frame
  rate identical. They govern 3D scene rendering and the landing page is a 2D
  interface. The hardware reports MSAA 16 support, so this is not a capability
  limit.

**The crash that used to kill a third of launches**
- Not a `pthread_create` override skipping per-thread setup — there is no such
  override, it is a plain passthrough.
- Not a `pthread_mutex_t`/`pthread_attr_t` ABI mismatch.
- Not a `malloc`/`free`/`operator new` mismatch directly — none of those are
  undefined symbols in `libroblox.so` at all.

**`onKeyDownNative` is registered, and the code it receives is an Android
keycode**

Worth stating because the opposite was a live theory: only the D key appears to
work in an experience, and evdev `KEY_D` and `AKEYCODE_D` are both 32 — they
collide at exactly one letter, so a raw evdev code reaching something that
wanted an Android one would look precisely like that. It is not happening at
this layer. Measured, with `CORDIAL_ANDROID_TRACE=1` on two consecutive runs:

```text
[android] onKeyDownNative(code=31) -> true      <- C, evdev KEY_C is 46
[android] onKeyDownNative(code=40) -> true      <- I, evdev KEY_I is 23
[android] onKeyDownNative(code=37) -> true      <- H, evdev KEY_H is 35
[android] nativePassKeyEvent(down=true, keyCode=51, modifiers=0x0) -> Ok(())
```

So the AGDK native is in the natives table, it returns `true`, the codes are
`AKEYCODE_*`, and `NativeGLInterface.nativePassKeyEvent` resolves and returns
cleanly on the same keystroke. Whatever makes only D work in an experience is
downstream of both, or is not about keycodes at all. `nativePassKeyEvent` is now
traced under `CORDIAL_ANDROID_TRACE=1`, which it never was before — every
keyboard investigation until now read only the AGDK half.

The reason none of this was visible before: `deliver_key`/`deliver_touch`
answered "the native is not registered" with silence, so a trace run that
printed nothing was indistinguishable from a trace run whose events were all
dropped. They now say so by name, at the first drop and then at each power of
ten — once would be indistinguishable from the normal startup race against
`initializeNativeCode`, and per event would bury the log.

**Resizing the window reflows the interface into many small items — and density
is not the fix**

Widening the window makes Roblox lay out more, smaller things: at roughly
1330px the home feed shows four recommended tiles at a comfortable size, at
2000px it shows six much smaller ones. The cause is understood. `DisplayMetrics`
in `native/init_params.cpp` reports `density = 1.0`, `densityDpi = 160`, and
Android's density is the scale against 160 dpi — so a 2000-pixel window is
described to the engine as a 2000dp-wide screen, which is a tablet the size of a
wall, and the client lays out for one. That is correct Android behaviour and the
wrong thing for a desktop monitor.

Correcting the density was tried and is **reverted**. Raising it (to the
compositor's output scale times 1.5, feeding both `DisplayMetrics` and
`PlatformParams.dpiScale` from one number) was measured by the owner in normal
use as having "absolutely destroyed the DPI on roblox and it still didnt fix the
resizing issue" — worse in the everyday case and no better in the case it was
for. `CORDIAL_DPI_SCALE` remains what it was, an override that changes
`PlatformParams.dpiScale` only.

**The engine reads its density exactly once, and a resize does not make it read
again.** Measured, not inferred: with a counter on every `DisplayMetrics`
construction, one 46-second run printed

```text
[android] DisplayMetrics #1: 1280x720 density=1.000 densityDpi=160
```

from inside `initializeNativeCode` — before the host window exists — and printed
nothing further, including across a live resize from 1280x721 to 2000x1100
driven by `gtk_window_set_default_size` from a timer. `onSurfaceChangedNative`
and `onContentRectChangedNative` are both re-driven on that resize, so the
engine does learn the new size; it simply never re-reads the density. Anything
that hopes to change density in response to a resize therefore cannot work
through this object, and a version that appears to work is a version that is
doing nothing. **`INFERRED`:** Android would deliver such a change as a
configuration change, which nothing here drives; whether the engine would honour
one is untested.

Consequence for the ordering, if anyone does revisit this: the density has to be
settled *before* `initializeNativeCode`, which is earlier than the host window
exists and therefore earlier than the display can be asked about itself.

Resizing is not currently a goal. Do not leave a partial or
disabled-by-default mechanism for it in the tree.

**Flags**
- The flags verdict does not gate rendering. `onFlagsFailed` is a complaint, not
  a gate.
- `nativePreloadFlagOverrides` does nothing observable, despite the name. Merging
  into the client-settings document is the mechanism that works.
- **Do not test whether flags work using FLog channels.** Setting
  `FLogAndroidGLView=7` produces no output even when flags demonstrably work, so
  it is a broken instrument — it produced a confident, wrong "no flag reaches the
  engine" that survived several experiments. Use a flag with an observable
  behavioural effect, and run a control.

**`--lib-dir` without `--host-libc`, and the five pthread symbols**

`pthread_once`, `pthread_key_create`, `pthread_key_delete`,
`pthread_getspecific` and `pthread_setspecific` are implemented, so nothing
needs `--host-libc` for them. They are thin forwards to the host's libc, which
is safe *for these* because the types involved are laid out identically:
`pthread_once_t` and `pthread_key_t` are both 4 bytes in both libcs and both
spell `PTHREAD_ONCE_INIT` as 0, measured off this tree's bionic headers rather
than assumed. Compiling bionic's own `pthread_key.cpp` instead would have been
worse than the stub it replaced, because it reads thread-specific data out of
`__get_tls()[TLS_SLOT_BIONIC_TLS]` — bionic's thread structure, and every thread
in this process belongs to the host's libc, so it would have appeared to work.

**That does not make bare `--lib-dir` a working configuration, and nothing short
of a real libc shim will.** It stubs 358 libc symbols, `memset`,
`pthread_mutex_lock` and `newlocale` among them. Measured after the change: the
load runs further into the engine's static initialisers and ends in SIGSEGV with
`__cxa_atexit` the last stub reported, where before it exited 1 on the
fatal-stub guard at `pthread_once`. Which stub it now cannot survive is *not*
established — `memset` is the obvious guess and the same run disproves it, since
it called `memset` and carried on through five more first-hit stubs. The fatal
list in `stubs.rs` is empty as a result. Put a symbol in it when a run shows the
process dying on that symbol, not because it looks dangerous.

`--host-libc` and `--game-activity` were checked in the same session and are
unaffected: exit 0 with `=== no stubs were called ===`, and `app ready: Landing`
with two ZSTD trace stubs in a 10-second run.

**Corrected on the way: `pthread_cond_t` is the same size in both libcs.** The
commit that introduced `bionic::pthread` recorded "pthread_cond_t is 32 bytes in
bionic, 48 in glibc" as one of three ABI divergences found, and the module's
doc-comment table repeated it. It is wrong. 32 bytes is `pthread_barrier_t`,
`int64_t __private[4]`; `pthread_cond_t` is `int32_t __private[12]` and comes to
48 on LP64, the same as glibc's. Measured by compiling one probe translation
unit twice — `char sz_x[sizeof(x)];` per type, sizes read back with `nm -S` —
once against `third_party/mcpelauncher-linker/bionic/libc/include` at
`-target x86_64-linux-android`, once against the host's glibc. `sem_t` at 16
against 32 is a real mismatch and its wrapper is load-bearing; the condition
variable wrapper is not, and was written for an overrun that could not happen.
It is left in place because removing it changes what runs at every
`pthread_cond_wait` in the engine, which wants its own measurement.

## 5. System time equals user time — and it is almost all the engine's

A 30s run spends about as long in the kernel as in userland (17.3s user, 16.8s
system) and racks up roughly 49,000 voluntary context switches. That looked like
Cordial's pump loop thrashing. It is not.

Measured across four full 30s runs against the real APK on Wayland, sampling
every thread's `wchan` and its own `voluntary_ctxt_switches` counter from
`/proc/<pid>/task/*/status` every 200ms:

| | share of voluntary switches | parked in |
|---|---|---|
| `HttpClient` (engine's network thread) | ~57% | `poll_schedule_timeout` — a scheduled timer, not socket I/O, cycling ~900/s while otherwise idle |
| `RBX Worker A`–`P` (engine task pool, one per core) | ~36% | `futex_do_wait` |
| Cordial's own thread running `pump()` | ~4% | `ep_poll` — the single deliberate 50 ms-bounded `epoll_wait` |
| other engine threads named `Main` | ~3% | |

So **~93% is inside `libroblox.so`**, which is not Cordial's to fix under
[ADR-001](adr/ADR-001-in-process-hooking.md). The per-iteration non-blocking
`poll(fds, 1, 0)` on the Wayland fd and `wl_display_flush()` contribute no
blocking `wchan` at all — they show up in the thread's running samples, not its
sleeping ones. Cordial names no thread `Main`; those four are the engine's.

**Disproved: `FIntTaskSchedulerAutoThreadLimit` is not a lever here.** Set to 1
and to 2 (against an unset default matching core count), verified reaching the
engine by the `flags: 1 override(s) applied` line. `RBX Worker A`…`P` still all
spawn, and user time, system time and switch count are indistinguishable from
baseline across every run. Negative result, repeated, recorded so nobody spends
an afternoon on it again.

**The 1,274 major page faults in the original measurement were environmental.**
Same harness with ~3 GB less swap pressure: single and double digits. Do not
read fault counts taken on a thrashing machine as a property of the client.

---

# How to work on this

## Debugging facts that cost real time

- **lldb breakpoints inside `libroblox.so` do not work and fail silently.**
  Cordial `mmap`s it outside the system linker, so lldb never lists the image and
  every breakpoint stays unresolved with hit count 0. Use `memory write` of
  `0xCC`, then rewind `$pc` and restore the byte on trap. Crash-stop backtraces
  and breakpoints in Cordial's own code work normally.
- **Read syscall arguments from `/proc/<pid>/task/<tid>/syscall`** while lldb has
  the process stopped, not from registers. Number plus all six arguments, no
  guesswork about the libc wrapper's register shuffling. That is how the futex
  was identified without disassembling anything.
- **There are three threads named `Main`.** Use `thread backtrace all`.
- **`CORDIAL_TRACE_PATHS=1` is safe** and logs every path-taking libc call with a
  thread id. **`CORDIAL_TRACE=1` is not** — it wraps variadic functions with
  fixed-arity declarations and aborts the engine.
- **`CORDIAL_SKIP_AGDK=1` skips the flag and app-bridge calls entirely.** Several
  historical results were measured on a path that never ran the code under test.
- With ASLR disabled `libroblox` loads at `0x7fffefec0000`.
- lldb is at `/home/linuxbrew/.linuxbrew/bin/lldb`. No gdb, no strace.
- **Never inject input with `XTestFake*`** — it takes over the real cursor on the
  machine you are using.
- **The `XSendEvent` advice that used to sit here is dead.** It said to target the
  window by `WM_CLASS` `cordial`. Since [ADR-011](adr/ADR-011-wayland-and-libadwaita.md)
  there is no X11 window to target, and an agent following it lost most of a
  session discovering that — Sober is native Wayland too, and forcing
  `XDG_SESSION_TYPE=x11` only makes SDL fail outright because the X11 socket is
  not forwarded into the flatpak on a Wayland session. **Wayland has no
  window-targeted input injection at all**, by design: `wlr-virtual-keyboard` and
  the `RemoteDesktop` portal both inject at the compositor and land on whatever
  has focus, which is the category the rule above forbids.
  - **For Cordial's own window, do not synthesise at the protocol level.** Cordial
    *is* the Wayland client, so call the path directly — `input::pass_key_event`
    and `input::pass_text` are `pub`, and `wayland.rs`'s `dispatch_key` exercises
    the keysym translation above them. No compositor is involved and nothing can
    reach the developer's session.
  - **For another application, nest a compositor.** Run it under a headless
    `cage`/`weston` on its own `WAYLAND_DISPLAY`; a virtual-keyboard client bound
    to *that* compositor is global only within a compositor containing one
    window. Neither is installed on this machine as of 2026-08-02. `INFERRED` —
    the nesting approach is standard practice but has not been tried here.

## Measuring anything

- Use a **control**. The flag mechanism was only established by showing a log
  line vanishes with the flag set and is present without it, in the same session.
- **Repeat.** One bug here reproduced on roughly one launch in three and its rate
  moved with machine load, so before/after samples taken under different load are
  meaningless.
- Label anything you could not test **`INFERRED`**.
- **Know what your instrument costs.** `WAYLAND_DEBUG=1` produced three separate
  timing "findings" in one session that all evaporated when the same script was
  run without it (§1e). It is excellent for object identity and request order and
  worthless for anything with a clock in it.
- **A total is not a rate.** `vkQueuePresentKHR` counted over a fixed window on
  the landing page measures the engine's idle throttle far more than its frame
  rate — §1d has the curve. Sample per second, and say whether input was being
  delivered.
- **Prefer one long run to many short ones.** A launch puts a window on the
  owner's desktop. `CORDIAL_SCRIPT` (§1e) exists so that one `--run 240` can
  carry a whole matrix of conditions with its own controls inside it.

## A limit on the capture, stated honestly

Cordial runs natively on the host; the capture came from Waydroid. It is
trustworthy for **call order, names and contract** — which is what it was taken
for — and **not** for timing or render behaviour. Roblox under Waydroid burns CPU
with little GPU utilisation. Do not read its rendering path as a model of a
healthy one.

## On observing other binaries, including Sober

**Decompilation reconstructs expression.** You end up reading a reconstruction of
someone's source and writing code from it, which is where derivative-work risk
lives. That is why decompiled material is off limits (§16.1,
[ADR-001](adr/ADR-001-in-process-hooking.md)).

**A debugger on a running process yields behaviour** — which libraries load, which
natives are called, in what order, with what arguments. Facts and interfaces, not
expression.

So the line is not the tool, it is **what you take away**:

- Fine: call sequence, load order, argument shapes, resolved symbols, timing,
  syscalls.
- Not fine: stepping into its routines to read how it implements something and
  transcribing that logic. At that point the debugger is a slower decompiler.

**One rule, applied to any binary including Roblox: observe freely, do not
transcribe.** Sober was built by observing Roblox and nobody treats it as tainted
for it.

### What an attempt to trace Sober's text path established, and where it stopped

Attempted 2026-08-02. **No call sequence was captured** — the blocker was input
delivery, not the debugger. Recorded so nobody repeats the dead ends.

- **Sober loads `libroblox.so` the same "outside the system linker" way Cordial
  does.** No mapping in any process of its tree is named `libroblox`; the image
  lives in an unnamed `memfd`, and it is mapped **thirteen times** at different
  bases within one process, each an identically sized `r-xp`/`rw-p` pair. Why
  thirteen is not established and was not pursued — that is engine internals.
- **`LD_PRELOAD` interposition cannot work on these natives, in Sober or here.**
  Cordial's own `crates/cordial-linker-sys/src/lib.rs` resolves them through
  `cordial_linker_dlsym` — its *own* linker, off the ELF symbol table — and calls
  a raw pointer. The system dynamic linker is never asked to resolve
  `Java_com_roblox_engine_jni_NativeGLInterface_*`, so a shim shadowing that name
  is never consulted. The `memfd`/no-named-mapping result says Sober's loader
  does the equivalent. This is a dead end, not an untested idea.
- **The six text-path natives are exported and their offsets are one command
  away**, so the `0xCC` technique needs no preparation beyond a load base:
  ```bash
  readelf --dyn-syms -W lib/x86_64/libroblox.so | grep NativeGLInterface_
  ```
  Verified against the APK Sober had already downloaded. Offsets are not written
  down here on purpose: they change every build, and a stale table would be read
  as fact.
- **Sober's own binaries are stripped bare.** `nm -D` and `readelf --dyn-syms` on
  `/app/bin/sober` and `libloader.so` return zero dynamic symbols, no symtab, and
  `strings` finds no `showKeyboard`, `setSoftKeyboardActive`, `restartInput` or
  `gametextinput`. So the host→engine direction has no entry point reachable by
  name. The legitimate route is breakpointing `RegisterNatives`/`GetMethodID` and
  reading the `name`/`fnPtr` arguments off the call — recognising an unnamed
  function by its shape instead would be the forbidden move.
- **The remaining blocker is a keystroke**, and per the input rule above that
  means either a human typing while breakpoints are already planted, or a nested
  headless compositor. `ptrace_scope` is 0, so attaching works; that step was
  simply never reached.

---

# Solved, for reference

Kept because the *shape* recurs — most of these were ABI or contract mismatches
that presented as something else entirely.

| Symptom | Actual cause |
|---|---|
| Startup hung in a futex, then died | Asset folder passed as the unpack root. The engine wants `<root>/content` and resolves siblings against the parent, so `canonical` threw and `SingleSurfaceApp` aborted before instantiating its controllers |
| Every hostname failed to resolve | `struct addrinfo` has `ai_canonname`/`ai_addr` transposed between bionic and glibc, and the `AI_*` constants differ — bionic's `AI_DEFAULT` sets a bit glibc rejects with `EAI_BADFLAGS` |
| Every HTTPS request failed | Engine builds `./exe/cacert.pem` from a root it was never given; it now has its own run directory with the APK's CA bundle linked in |
| SIGSEGV on `HttpClient`, one launch in three | `realpath(path, NULL)` allocates with the host allocator; Roblox statically links mimalloc and freed a pointer its arena table never registered |
| `eglCreateWindowSurface` returned `EGL_BAD_ALLOC` | The engine was handed Cordial's `ANativeWindow*`; host EGL on X11 wants an XID |
| Vulkan refused to initialise | Roblox needs `VK_KHR_android_surface`, which desktop Mesa never exposes. Translated to `VK_KHR_xlib_surface` behind one interposed `vkGetInstanceProcAddr` |
| Engine reported "Android API 15" and refused Vulkan | `DeviceParams.osVersion` is read as an API *level*. Neither the system property nor `android_get_device_api_level()` fed it |
| Paths resolved against the working directory | `NativeSettingsInterface`'s directory setters were never called |
| GLES ran at about 1 fps while Vulkan was fine | Every `eglSwapBuffers` blocked ~1.00 s inside Mesa. The engine asks for vsync and Xwayland owns no CRTC, so DRI3's vblank query fell back to a one-second wait. The interval Mesa receives is now forced to 0; 20 → 652 swaps in 20 s |
| Interface looked like a low-end phone | Surface hardcoded to 720p and `dpiScale` to 1.0 — Roblox lays out in dp and picks asset resolutions from exactly those |

## Branches

`archive/gameactivity-per-callback` holds per-callback GameActivity dispatch that
was never merged. **Read it, do not merge it** — it is built on the disproved
ALooper theory and restructures the App Bridge onto a worker thread to fix a hang
whose real cause was the asset path. Merging it produced code that did not
compile against current main. One good idea in it: sharing one GameActivity
`thiz` and one Surface across calls, the way Android does.
