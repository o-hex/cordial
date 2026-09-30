# ADR-044: Settings that can change reach a running game

**Status:** accepted
**Date:** 2026-09-30
**Related:** [ADR-003](ADR-003-plugin-isolation.md), [ADR-007](ADR-007-host-resources-are-brokered.md), [ADR-012](ADR-012-profiles-and-instances.md), [ADR-019](ADR-019-development-control-surface.md), [ADR-038](ADR-038-plugin-hot-swap.md)

## Decision

1. **Each setting is either live or next-launch, and the row says which.** Live
   settings are sent to every client the shell started, as soon as `shell.json`
   changes. Next-launch rows carry "Applies at next launch". The classification
   is `live::CLASSIFICATION`, a table with a reason per key; a test fails if
   `ShellConfig` gains a key that is not in it.
2. **A new socket, not devctl.** The client listens on
   `<profile>/live/settings.sock`, in a `0700` directory, always on. devctl
   (ADR-019) is opt-in and can inject input and capture frames; switching it on
   for everyone to move a slider would be the wrong trade. The live socket has
   two verbs, `set` and `get`, and `set` accepts only the keys in
   `cordial_shell::live_wire::KEYS`. Unknown keys are reported back and ignored;
   a bad value refuses the whole message. Nothing runs, reads a file, or reaches
   the engine. Plugins cannot open it: the sandbox does not bind the profile
   (ADR-003, ADR-007). No peer-credential check sits on top; the only account that
   can open the path is the user's own, which can equally edit `shell.json`.
3. **One trigger.** The shell watches the config directory (GIO monitor, so
   rename-saves are seen), debounces 150 ms, parses strictly, and pushes the
   keys that moved. The shell's own saves, hand edits and a second shell
   instance all arrive this way, so no settings row has to call anything. A
   file that fails to parse changes nothing; it is not read as the defaults.
4. **Client side, each live setting is an atomic** in the module that uses it,
   initialised from the launch environment. The launch environment is still
   set, so a client started by hand or by an older shell behaves as before.
5. **A client that has not answered yet is retried** once a second for about
   half a minute, then left until the wanted values change. A client registers
   with the values its environment carried, so a change made while it loads is
   delivered when its socket appears.

## Classification

| Key | Applies | Reason |
|---|---|---|
| `pointer_acceleration` | live | read on every locked-pointer motion event |
| `throttle` | live | the pump reads it each tick (it used to be read once) |
| `close_on_leave` | live | consulted when the log reports leaving a game |
| `carry_launch_ticket` | live | consulted each time a link is translated |
| `gamepad` | next launch | switching off mid-session needs a disconnect for every announced pad; unmeasured |
| `gamemode` | next launch | registered with gamemoded once |
| `graphics`, `graphics_optimization_mode` | next launch | settled before engine initialisation |
| `present_mode` | next launch | read at swapchain creation |
| `mangohud`, `vkbasalt` | next launch | Vulkan layers load at instance creation |
| `audio_output` | live | the playing streams are re-linked to the new sink in place; see below |
| `title_bar`, `roblox`, `profile`, `unpacked_plugins`, `fullscreen_accel` | next launch | built or chosen at launch |
| `appearance`, `automatic_updates`, `download_on`, `marketplace_*`, `multi_instance_warning_seen` | shell | read by the shell itself |

## Audio output

`audio_output` was next-launch because the PipeWire backend cached the sink name
in a function-local static. The cache is now a mutex-guarded string seeded from
`CORDIAL_AUDIO_SINK`, and a change re-links the streams that are already
playing. **The engine's streams are not recreated**: its OpenSL ES players and
AAudio streams keep their `pw_stream`, buffers and callbacks, so nothing FMOD
holds goes stale and the mixer sees no gap.

How the move is made was the part that needed measuring. The obvious route,
`pw_stream_update_properties` with a new `target.object`, returns success on
WirePlumber 0.5.14 (PipeWire 1.6.8) and leaves the link where it was; a stream
left on one sink was still on it four seconds and two updates later. What does
move it is what `pw-metadata <node> target.object <sink>` and `pactl
move-sink-input` do: write `target.object` for the stream's node into the
`default` metadata. A sink name goes in as `Spa:String`. Going back to the
system default is `-1` as `Spa:Id`; *deleting* the key is not the same thing, it
restores the sink the stream was opened on.

Measured with `tools/audio-switch-e2e.py`, which plays through the same
`CallbackStream` and `PlaybackStream` the engine uses, changes sink the way the
socket does, and reads the links out of `pw-dump` after each step: A to B to A to
the system default, for both the AAudio path (the default) and the OpenSL ES
path, all four links where they should be; the control, asking for the sink the
stream is already on, stays put. The AAudio stream delivered 47.5 to 48.0 thousand
frames a second across the whole run against a negotiated 48 000, which a
torn-down-and-rebuilt stream would not.

What this does not cover, and the reply says so: a host backend other than
PipeWire (`CORDIAL_AUDIO_HOST=pulse|alsa|oss`) has no sink to re-link a stream
between, so the choice applies to streams opened afterwards and the client
reports that in `notes`. Other session managers than WirePlumber, or a session
with no `default` metadata, are **INFERRED**: the client checks for the metadata
object and reports its absence instead of claiming the move. The client itself
was not run for this change; the native backend and the socket handler were
exercised separately.

## Consequences

`carry_launch_ticket` is live and moves a credential, so flipping it applies to
the next link the running client translates. That is what the row says.

The Settings window does not refresh its rows when another process edits
`shell.json`, and its next save writes its own stale copy back. That predates
this decision and is not fixed by it.

`unpacked_plugins` edits inside a listed folder already reload (ADR-038); adding
or removing a folder is next launch.
