# ADR-051: Overrides are handed to the engine again after its own settings refresh

**Status:** accepted
**Date:** 2026-10-01
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-005](ADR-005-flag-service.md), [ADR-044](ADR-044-settings-reach-a-running-game.md)

## Context

Users set `DFIntTaskSchedulerTargetFps`, saw it work, and found it back at 60
after a few minutes. The engine runs a `DynamicFastVariableReloader` that
refetches Roblox's settings document and applies it over whatever is in force
(`docs/analysis/flag-init.md` section 47). The log of every run that lasted two
minutes shows it finishing at about 120, 241 and 362 seconds, signed in or out.
A flag merged into the launch document is therefore in force until the first of
those and not after.

## Decision

1. **After each refresh, hand the engine the document again with the overrides
   merged in.** `nativeInitClientSettings` takes a second call and answers `0`
   (measured 2026-09-01); it is the entry point Cordial already calls at launch,
   so nothing new reaches into the process and ADR-001 is untouched.
   `cordial_runtime::flag_reapply` owns it.
2. **The trigger is the engine's own log, not a timer.** `game_log` already tails
   it from the pump, and the reloader writes `DynamicFastVariableReloader
   finished flag fetch` (or `Skipping flag cache write: ...` in a session on
   another channel) when it is done. `game_log::poll` notes it; a worker thread
   waits 250 ms, so a burst is one apply, and never applies twice within 2 s. A
   failed fetch (`Could not fetch settings`) applies nothing and triggers nothing.
3. **A timer is only a net.** A client that has gone 150 s without ever seeing a
   refresh line re-applies on that interval; one line seen turns it off for good.
4. **Every override counts, not only `DF*`.** Anything a layer above Cordial's own
   built-in default contributes (user file, plugin, Performance, Frame rate
   limit) is kept. Only the dynamic family is measured to be reverted, so a
   profile of `FFlag`s alone pays one pointless call per refresh; that was chosen
   over a list of which names the reloader touches, which would have to be right
   on every Roblox build. **With no overrides nothing is read and nothing is
   called.** The check is made when the trigger fires, so a flag added to
   `flags.json` while the client runs is kept from the next refresh.
5. **The work is off the pump.** The document is built (resolve the layers, read
   the cached base, merge, serialise) and handed over on the worker's own
   thread, and one `[reapply]` line records what it cost.
6. **The Frame rate limit row uses the same path and is live.** It rides the
   settings socket (ADR-044); the client stores the choice, the flag layer reads
   it, and the worker applies at once. The row sits in the flag layers above
   plugins and below the user's own `flags.json`.
7. `CORDIAL_NO_FLAG_REDELIVERY=1` turns the whole thing off. It exists for the
   control below.

**One mechanism, and where it came from.** A contributor's pull request (73)
reached the same trigger independently: the engine's log, then the launch
document sent again. This implementation keeps that switch name and the
`Skipping flag cache write` wording from it. It differs in rebuilding the
document each time (so live changes and edits to `flags.json` are picked up, for
about 25 to 40 ms in all) rather than resending the launch document held in memory,
in covering every family instead of `DF*` alone, and in holding the call to a
worker with a minimum gap. Both should not ship; this one is the superset.

## Measured

Signed out, own nested headless sway (60 Hz output), throwaway data root, input
driven at 60 moves a second for the whole run, presents read from the devctl
`info` counter every five seconds. `DFIntTaskSchedulerTargetFps=20` in the
profile's `flags.json`. 20 is the value `flag-init.md` section 49 used as its
positive control, because the machine delivers about 57 with nothing set.

| Arm | presents/s before 120 s | at and after the 120 s refresh |
|---|---|---|
| A: re-apply on | 20.0 throughout | 20.0 to the end (175 s run: 19.9, 20.0, 20.0; 195 s run: 18.8 to 20.7) |
| B: `CORDIAL_NO_FLAG_REDELIVERY=1` | 20.0 throughout | 56.5, 56.2, 57.6 ... 58.1 from the first sample after the refresh |
| C: no flags | 55 to 58 | no `[reapply]` line, nothing called |

The engine log's refresh line sat at 120.3 s (A), 120.1 s (B) and 120.1 s (C),
and A's client output carries exactly one `[reapply]` line after it. B is the
control: same build, same profile, only the switch differs, and the rate jumps
at the refresh.

**Cost of one call**, from the `[reapply]` lines of five calls:

```text
engine settings refresh seen: nativeInitClientSettings -> 0 (2 override(s), 1373891 bytes; load 2 ms, merge 19 ms, call 21 ms)
engine settings refresh seen: nativeInitClientSettings -> 0 (2 override(s), 1373891 bytes; load 1 ms, merge 12 ms, call 13 ms)
live change: nativeInitClientSettings -> 0 (1 override(s), 1373856 bytes; load 0 ms, merge 10 ms, call 14 ms)
live change: nativeInitClientSettings -> 0 (2 override(s), 1373891 bytes; load 3 ms, merge 10 ms, call 19 ms)
live change: nativeInitClientSettings -> 0 (1 override(s), 1373856 bytes; load 0 ms, merge 9 ms, call 13 ms)
```

Building the document is 10 to 20 ms and the engine's call 13 to 21 ms, all on
the worker. The pump is not involved, and presents held 20.0 a second across the
call. Whether a 60 fps game hitches when the engine takes the document was not
measured with a frame-time trace; the call is a once-in-two-minutes event.

### The live row

`CORDIAL_FRAME_RATE_LIMIT=20` at launch, then over the socket: `90` at 45 s,
`Display` at 85 s, 140 s run.

- 20 to 90 took effect at once: about 20 a second before, about 31 after.
- Back to Display did **not**. The re-apply carried no override and the rate
  stayed at 31 until the engine's own refresh at 120 s, then went to 56.5. A
  call that merely stops carrying a key does not unset it; only the reloader
  does. Display refresh is the absence of the flag, and Cordial does not know the
  engine's compiled default for it (the key is not in Roblox's document), so it
  does not send one. The reply carries a note saying so, and the Settings row's
  subtitle says going back takes up to two minutes.
- After that refresh, with no overrides left, no `[reapply]` line was printed.

## Consequences

- **A key absent from Roblox's document is reset by the refresh too.**
  `DFIntTaskSchedulerTargetFps` is not in the document and reverted at 120 s.
  Section 47's advice to pick a key Roblox does not ship for a long experiment
  no longer holds without the re-apply.
- **The base is the cached document, not the one the engine just fetched.** That
  one is not reachable without touching the engine. A key Roblox changed since
  the cache was written is sent at its older value, as at launch.
- **Nothing is applied for the engine's first fetch (about 2 to 4 s).** The
  reloader logs no line for it, and measured values held through it.
- **No cap above 240.** A contributor reports the engine stops there. This
  project could not check.
- **On the 60 Hz headless output a cap above 60 measured worse than none**: 90
  presented about 31 a second, against 57 to 60 with nothing set. Earlier
  measurements of 240 and 9999 gave about 36. Users on fast monitors report the
  opposite. This is one environment, not a finding about monitors, and it is why
  Display refresh is the default and why FPS Flex's own default (240) deserves a
  second look.
- Not run: any signed-in client. Whether the refresh behaves differently once
  signed in is **INFERRED** from the earlier logs that show it at the same
  times; no signed-in launch was spent on it.

## Alternatives

- **A fixed timer** (the first draft re-called every 30 s). Wasteful: it hands
  a 1.3 MB document to the engine every 30 s for a refresh that is announced in
  the log.
- **Resend the launch document held in memory** (the pull request). Cheaper per
  call, but it cannot carry a live change or an edited `flags.json`, so a live
  Frame rate row would have needed a second path.
- **Stop the reloader.** Reaching into the engine's settings service; ADR-001
  rules it out.
