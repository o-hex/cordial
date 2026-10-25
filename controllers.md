---
title: "Controllers"
description: "Controllers work out of the box, but the on-screen button prompts may show the wrong brand."
icon: "gamepad"
---
Controller support is on by default. Cordial reads your pad from `/dev/input/js*` and passes its buttons and sticks to Roblox, and that part is tested. Switch it off with **Settings → General → Controllers**, which applies to a running game, or start a direct `cordial-run` with `CORDIAL_GAMEPAD=0`.

<Warning>

**The button prompts may name the wrong brand.** Roblox ships separate glyph sets for PlayStation, Xbox and a generic pad, and picks one with an integer whose meaning is not published anywhere we can read. Cordial sends a value that may be the wrong one. If it is, you see the wrong brand of prompt and every button still works. Sober has the same fault: [#584](https://github.com/vinegarhq/sober/issues/584) and [#1810](https://github.com/vinegarhq/sober/issues/1810).

</Warning>

## In the Flatpak

The Flatpak could not see controllers at all before the release after 0.27.0: its sandbox had no `/dev/input`. The manifest now grants `--device=input`, which exposes that directory and nothing else in `/dev`. It also exposes the event nodes of keyboards and mice, as far as your own file permissions allow, because Flatpak has no grant for the pad alone.

If you installed 0.27.0 or earlier and want a pad now, give it the permission yourself:

```bash
flatpak override --user --device=input io.github.luohoa97.Cordial
```

Flatpak older than 1.15.6, which is what Ubuntu 24.04 and Debian 12 ship, does not know `input` and refuses that command. Use `--device=all` there instead; it is wider and the only grant that old versions have. A newer Flatpak installing a Cordial release that carries the grant needs nothing.

Checked: `flatpak run --device=input` makes `/dev/input` appear in the sandbox. **INFERRED:** a pad then works there; no pad was attached for that check.

## If the glyphs look wrong

Try other values, then press a button on the pad. Roblox is reported (a user report, not a measurement) to change its glyph set when an input is used, not when the pad connects.

```bash
CORDIAL_GAMEPAD_TYPE=1 cordial      # then 2, 3, ...
```

Cordial prints the value it used at launch, once, the first time it sees a pad.

<Tip>

If you find the value that draws your controller's own glyphs, [open an issue](https://github.com/luohoa97/cordial/issues) with the pad model and the number. That settles it for everyone, and it is the one thing that cannot be worked out without a controller in front of the engine.

</Tip>

## If a stick or button does the wrong thing

Cordial works out which stick and button is which from what the kernel says your pad has. A pad whose D-pad is four buttons rather than a hat (the AYN Odin pads) gets its D-pad, a pad with digital triggers is read correctly, and a four-axis pad (X, Y, Z, RZ) has its right stick on Z and RZ. A DualShock 4 on the older `hid-sony` driver is recognised by its ids and has its right stick on Z and RZ too.

If a stick is still on the wrong axis, you can say which axis is which for this run with `CORDIAL_GAMEPAD_AXES`, as `linux=android` pairs:

```bash
CORDIAL_GAMEPAD_AXES=z=rz,rz=z cordial         # Z and RZ become the right stick
CORDIAL_GAMEPAD_AXES=rx=ltrigger,ry=rtrigger,z=z,rz=rz cordial
```

The left side is the axis the pad declares (`x y z rx ry rz hat0x hat0y`), the right is what Roblox is told (`x y z rx ry rz hat_x hat_y ltrigger rtrigger`, or `none` to drop it). It applies to every pad, and a pair Cordial cannot read is named in the log and ignored. If something works, open an issue with the pad's name and the line, and it becomes an entry in Cordial's own table.

If none of that helps, open an issue with these lines from a terminal (use `js1` and so on for another pad):

```bash
cat /sys/class/input/js0/device/name
cat /sys/class/input/js0/device/capabilities/abs
cat /sys/class/input/js0/device/capabilities/key
cat /sys/class/input/js0/device/id/{vendor,product,version}
```

Starting Cordial with `CORDIAL_TRACE_GAMEPAD=1` also prints the Android code each axis and button was given.

Sticks, the D-pad hat and triggers are passed in the shape Roblox's own menu was seen to accept: a stick is one event carrying both its axes, with up as positive. The left stick and the hat were checked by moving the menu selection; **INFERRED:** the right stick and the triggers, which no screen reachable without a signed-in account reads. Why: [ADR-064](/adr/ADR-064-a-gamepad-axis-call-is-a-stick-not-an-axis).

**INFERRED:** the Odin 3 and DualShock 4 mappings come from Android's key layout and SDL's controller database, not from a pad plugged into the machine that wrote this. The table is `QUIRKS` in `crates/cordial-runtime/src/android/gamepad.rs`.

## Rumble

There is no rumble. That is deliberate: a rumble call that silently does nothing is worse than none.

`cordial-run --help` lists the other switches. Why controllers can change in a running game: [ADR-044](/adr/ADR-044-settings-reach-a-running-game).
