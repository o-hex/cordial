# Controllers

<!-- description: Controllers work out of the box, but the on-screen button prompts may show the wrong brand. -->
<!-- icon: gamepad -->

Controller support is on by default. Cordial reads your pad from `/dev/input/js*` and passes its buttons and sticks to Roblox, and that part is tested. Switch it off with **Settings → General → Controllers**, which applies to a running game, or start a direct `cordial-run` with `CORDIAL_GAMEPAD=0`.

> [!WARNING]
> **The button prompts may name the wrong brand.** Roblox ships separate glyph sets for PlayStation, Xbox and a generic pad, and picks one with an integer whose meaning is not published anywhere we can read. Cordial sends a value that may be the wrong one. If it is, you see the wrong brand of prompt and every button still works. Sober has the same fault: [#584](https://github.com/vinegarhq/sober/issues/584) and [#1810](https://github.com/vinegarhq/sober/issues/1810).

## If the glyphs look wrong

Try other values, then press a button on the pad. Roblox is reported (a user report, not a measurement) to change its glyph set when an input is used, not when the pad connects.

```bash
CORDIAL_GAMEPAD_TYPE=1 cordial      # then 2, 3, ...
```

Cordial prints the value it used at launch, once, the first time it sees a pad.

> [!TIP]
> If you find the value that draws your controller's own glyphs, [open an issue](https://github.com/luohoa97/cordial/issues) with the pad model and the number. That settles it for everyone, and it is the one thing that cannot be worked out without a controller in front of the engine.

## Rumble

There is no rumble. That is deliberate: a rumble call that silently does nothing is worse than none.

`cordial-run --help` lists the other switches. Why controllers can change in a running game: [ADR-044](adr/ADR-044-settings-reach-a-running-game.md).
