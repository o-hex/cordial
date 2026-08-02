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
  ./target/release/cordial-load --lib-dir <lib> --apk <apk> \
  --host-libc --game-activity --run 120

# 2. Does the zwp_text_input_v3 "has no event 8" freeze still happen, and on
#    which object? Click into a field; WAYLAND_DEBUG prints every event with
#    its object id, so the id that receives opcode 8 can be matched against
#    what created it earlier in the same log.
WAYLAND_DEBUG=1 CORDIAL_WAYLAND=1 ./target/release/cordial-load ... 2>&1 \
  | tee /tmp/ti.log; grep -n "text_input\|no event" /tmp/ti.log
```

The second one is the only way anyone has to diagnose that bug rather than
guess at it, and this change did not touch it.

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
