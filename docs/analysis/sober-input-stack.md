# What Sober uses for text input

**Established 2026-08-25 by observing a running Sober**, on this host, against
`org.vinegarhq.Sober` from Flathub. Nothing here comes from Sober's source or
from disassembling it: Sober is not source-available and AGENTS.md permits
observing it running, which is what this is. The evidence is `/proc` maps, ELF
`DT_NEEDED` metadata, and a `WAYLAND_DEBUG=1` protocol trace of a launch in a
nested compositor.

## The answer

**Sober uses no widget toolkit, and it does speak `zwp_text_input_v3`
directly.**

Those are two separate facts and it is easy to get the second one wrong from
the first. A first pass here concluded Sober had no text path at all, on the
strength of having no toolkit. That was wrong, and the trace below is what
corrected it.

## No toolkit

Sober's own `DT_NEEDED` lists no GTK, no Qt, no SDL:

```
libloader.so libmimalloc.so libcrypto.so.3 libcurl.so.4 libfreetype.so.6
libfontconfig.so.1 libz.so.1 libsecret-1.so.0 libgobject-2.0.so.0
libglib-2.0.so.0 libxml2.so.16 libGLESv2.so.2 libEGL.so.1
libgstreamer-1.0.so.0 libgstapp-1.0.so.0 libgstvideo-1.0.so.0 libdbus-1.so.3
libm.so.6 libgcc_s.so.1 libc.so.6 ld-linux-x86-64.so.2
```

`libgtk-3.so.0` and `libgdk-3.so.0` *are* in the running process's maps, and
that is misleading. They arrive down exactly one chain:

```
sober -> libdecor-0.so.0 -> libdecor-gtk.so -> libgtk-3.so.0, libgdk-3.so.0
```

`libdecor-gtk.so` is libdecor's client-side **window-decoration** plugin. It is
supplied by the Freedesktop runtime rather than shipped by Sober, and it is
dlopened to draw the title bar and borders. It is not evidence of a UI built
from GTK widgets, and taking it as such is the trap this section exists for.

What the process actually maps for rendering and input is the raw stack:
`libwayland-client`, `libwayland-egl`, `libwayland-cursor`, `libxkbcommon`,
EGL/GLESv2 and Vulkan, with PulseAudio for sound.

## But it binds text-input

From the protocol trace:

```
-> wl_registry#2.bind(23, "zwp_text_input_manager_v3", 1, new id [unknown]#14)
-> zwp_text_input_manager_v3#14.get_text_input(new id zwp_text_input_v3#23, wl_seat#21)
```

The full set it bound, under a compositor advertising all of them:

```
wl_compositor  wl_data_device_manager  wl_output  wl_seat  wl_shm
wp_alpha_modifier_v1  wp_cursor_shape_manager_v1  wp_fractional_scale_manager_v1
wp_viewporter  xdg_activation_v1  xdg_wm_base  zwp_idle_inhibit_manager_v1
zwp_pointer_constraints_v1  zwp_pointer_gestures_v1
zwp_primary_selection_device_manager_v1  zwp_relative_pointer_manager_v1
zwp_tablet_manager_v2  zwp_text_input_manager_v3  zxdg_decoration_manager_v1
zxdg_exporter_v2
```

`zwp_pointer_constraints_v1` is there too, which is the cursor-lock primitive.

## Why this explains a bug they have and we do not

Sober has the IME plumbing and nothing to paint with. An input method can
compose into a Sober TextBox and the text reaches the engine -- and the engine
does not paint a focused TextBox's own text, which is established separately in
`docs/NEXT.md`. So nobody paints it.

That is the structural reason Sober #987 (*"when typing in chat or any other
textboxes i cant see text i typed"*) and #1026 are open and unfixed. It is not
a bug nobody has got round to; there is no surface in that architecture on
which the fix could be written without adding one.

**This is the architecture Cordial had until `fd0f0c6`** -- protocol-level
text-input against a hand-rolled Wayland client, no widget. Cordial's
`gtk::Text` is a divergence from Sober rather than a copy of it, and it is what
makes typed text visible. There was an idea earlier in the project of adopting
"Sober's exact typing mechanism"; that is moot, because the part that would
have been worth copying does not exist.

## The methodology trap, which cost the first run

**A negative result from a nested compositor is worthless until you check the
compositor advertised the global.**

The first trace was taken under `cage`, which Cordial's `--headless` uses.
Sober's bind list came back with no text-input in it and looked like a clean
negative. It was an artefact: **cage does not advertise
`zwp_text_input_manager_v3` at all**, only `zwp_virtual_keyboard_manager_v1`,
so Sober could not have bound it whatever it wanted. Re-running under `sway`
-- which advertises `zwp_text_input_manager_v3`, `zwp_input_method_manager_v2`
and `zwp_virtual_keyboard_manager_v1` -- inverted the answer.

Always grep the trace for the `wl_registry.global` line before believing the
absence of a `bind`.

## How to repeat it

`sway` is the nested compositor, not `cage`, for the reason above.

```bash
# a compositor that offers the protocol
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway -c <(echo 'exec sleep 900')
# then, against its socket
WAYLAND_DISPLAY=wayland-N flatpak run --env=WAYLAND_DEBUG=1 \
  --env=WAYLAND_DISPLAY=wayland-N org.vinegarhq.Sober
```

**Sober is single-instance and will abort with "An instance of Sober is already
running" if one is up. That does not matter for this question**: it completes
its entire registry bind pass before the lock check, so an aborted launch still
yields the full list. There is no need to close a Sober the machine's owner is
using.

Two cautions. Sober's supervisor process `ptrace`s its own main process
(`TracerPid` on the 66-thread process points at the single-threaded parent), so
**gdb cannot attach** -- `ptrace: Operation not permitted` -- and detaching
their tracer to make room is not something to do to a client someone is using.
And kill only the instance you started: `flatpak ps` lists instance ids, and
the one you launched is the one that was not there before.
