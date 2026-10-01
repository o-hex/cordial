# Changing FastFlags

Roblox is configured by FastFlags, and Cordial lets you override any of them.
Create `~/.local/share/cordial/profiles/<profile>/flags.json` (or point
`CORDIAL_FLAGS` at another file) with a flat object. Installed as a Flatpak the
sandbox moves `~/.local/share` to `~/.var/app/io.github.luohoa97.Cordial/data`, so the
same file is `~/.var/app/io.github.luohoa97.Cordial/data/cordial/profiles/<profile>/flags.json`
— `INFERRED` from how Flatpak remaps `XDG_DATA_HOME`, not yet checked against an
installed package.

```json
{
  "DFFlagRbxTransportUseRtcioRna": false,
  "FIntTaskSchedulerAutoThreadLimit": 8,
  "FFlagDebugGraphicsDisableVulkan": false
}
```

**All three of those exist in the Android engine, and this example used to
carry one that does not.** It offered
`"FStringDebugGraphicsPreferredBackend": "Vulkan"`, which reads perfectly and
is not a Roblox flag: `DebugGraphicsPreferredBackend` appears **zero** times in
`libroblox.so`, and nothing resembling it does either — the real names in that
family are `DebugGraphicsDisableVulkan`, `DebugGraphicsDisableOpenGL`,
`DebugGraphicsDisableVulkan11` and so on. Reported by a user, checked against
the binary, and worth stating plainly because a documented example is the first
thing anybody copies.

**A name the engine does not know is accepted and ignored**, silently — it goes
into the settings document like any other key and nothing rejects it, so an
invented flag looks exactly like a working one. If a flag seems to do nothing,
check that it is real before assuming it did not help:

```bash
strings ~/.cache/cordial/lib/x86_64/libroblox.so | grep -x DebugGraphicsDisableVulkan
```

The name in the file carries the `FFlag`/`FInt`/`FString` prefix; the engine's
own table stores it without one, which is why the `grep` above drops it.

To choose a graphics backend, use Settings rather than a flag — Cordial decides
that before the engine starts, and the setting is what it reads.

**Raising the frame rate takes two separate levers, and neither is in Roblox's
own menu.** The in-game settings have no frame-rate row because the *Android*
client has none — the Windows client does, and so do the desktop menus people
remember, but Cordial runs the Android build and nothing here can add a row the
client does not draw. Reported as a missing feature, which is a fair reading of
an interface that simply has no such control.

| What you want | Where it is |
|---|---|
| Stop drawing being pinned to your display's refresh | **Settings → General → Graphics → Frame pacing** (Mailbox is the default since it was measured more responsive than FIFO), or the FPS Flex plugin — the same lever, so use one or the other |
| Raise the engine's own target frame rate, and keep it there | **Settings → General → Graphics → Frame rate limit** |

They are not the same setting and neither substitutes for the other: Frame pacing
is `VkSwapchainCreateInfoKHR::presentMode`, which decides whether a finished
frame waits for the next refresh, and Frame rate limit is what the engine's own
scheduler aims at. Leaving Frame pacing on FIFO caps you at your panel's rate
whatever Frame rate limit says.

**Flags you set stay set.** Roblox's own client refetches its settings about
every two minutes and applies them over the top, which used to put any `DF*`
flag Roblox also ships back to Roblox's value — `DFIntTaskSchedulerTargetFps`
fell back to 60 a couple of minutes into a session that way. Cordial now watches
the engine's own log for the end of each refresh and hands the engine your
overrides again straight after it. That covers everything in your `flags.json`,
every plugin's flags, and the Performance and Frame rate limit rows, and it does
nothing at all if the profile has no overrides. The client prints one
`[reapply]` line each time, with what it cost; a flag you want to check is
still in force can be read against that.

Two things it does not do. It does not touch the first couple of seconds, before
the engine's first fetch, and it hands over the cached copy of Roblox's document
rather than the one the engine just fetched, so a flag Roblox changed during
your session goes back to its older value, the same as at launch. And
`CORDIAL_NO_FLAG_REDELIVERY=1` in the client's environment turns it off, which is only
useful for confirming that a flag reverts without it.

**Frame rate limit** (Settings → General → Graphics) sets
`DFIntTaskSchedulerTargetFps` for you: Display refresh sets nothing, or pick 90,
120, 144, 165 or 240. It applies to a game that is already running: a new cap
takes effect at once, and going back to Display refresh takes up to two minutes,
because the engine does not unset a flag Roblox's settings leave out and only its
own next refresh resets it. The row beats a plugin that sets the same flag (FPS
Flex does), and a flag you set in `flags.json` beats the row.

Three limits on what it can do. A value above your display's refresh needs a
monitor that fast, and Frame pacing on FIFO still holds you to your panel's rate.
There is nothing above 240, because a contributor found the engine stops there
(raising its own frame-rate settings to 1000 on a 144 Hz monitor still held at
240); this project has no monitor that fast to check. And on a 60 Hz output, a
cap above 60 measured *worse* than leaving it alone: 90 presented about 31 a
second and 240 about 36, against 57 to 60 with nothing set (a headless 60 Hz
compositor, driven with input throughout). That is one environment, and users on
fast monitors report the opposite, so the caps stay, but do not pick one higher
than your display runs.

Values may be written as booleans, numbers or strings — Roblox stores them all
as strings and Cordial converts. The overrides are merged into the settings
document the engine is given at startup, and the launch log reports how many
were applied.

**`FFlag`, `FInt` and `FString` are read once at startup**, so changing them
needs a relaunch. Only the `DFFlag`/`DFInt`/`DFString` family is re-read while
the client is running, and edits to `flags.json` made while the client runs are
picked up at the next refresh for that family. That distinction matters if you
are building anything that changes flags dynamically — a plugin loaded part-way
through a session cannot change a startup flag, whatever it writes.

## Importing a list from another launcher

**Settings, FastFlags, Import…** reads a Bloxstrap or Fishstrap
`ClientAppSettings.json`, or Sober's `config.json` (only its `fflags` object;
the `FFlagExample` placeholder a stock Sober install carries is dropped), and
merges the flags into this profile's. The same from a terminal:

```bash
cordial --import-flags ClientAppSettings.json     # or - for standard input
cordial --import-flags --sober                    # finds Sober's own config
cordial --import-flags list.json --profile NAME --replace
```

Flags already set are kept unless the list sets them again; `--replace` starts
from empty instead. Values are checked by the flag's prefix: `FFlag` takes
`True` or `False`, `FInt` a whole number, and `FLog` takes anything, because
log channels are declared as a number (`"7"`) or a severity (`"Info"`,
`"Warning,6"`) and which one a channel wants is not visible from outside.

**One entry the check refuses does not stop the rest.** It is skipped and named,
and everything else is imported. A name with no FastFlag prefix is imported and
listed, so a typo shows. The Sober config path outside the Flatpak
(`~/.config/sober/config.json`) is `INFERRED`; only the Flatpak's has been seen.

## Layers and provenance

Flags come from more than one place, and each source owns its own file:

```text
<profile>/flags.json                             user    (always wins)
~/.local/share/cordial/plugins/<id>/flags.json   plugin
the client-settings document from Roblox         base
```

Your overrides live in the profile, so a flag you set while testing something on
one account is not silently still set on the account you play. A file left at
the old `~/.config/cordial/flags.json` is moved into the first profile that goes
looking for one — see [ADR-013](adr/ADR-013-per-profile-configuration.md).

A plugin never writes to your file. That keeps three things true: a plugin
cannot silently overwrite a value you chose, removing a plugin removes its
flags, and "why is this flag set to that?" has an answer. Conflicts are reported
rather than resolved quietly:

```text
flags: FIntTaskSchedulerAutoThreadLimit = 8 from user
       (overrides plugin:fps-tweaks=4, plugin:net-tuner=16)
```

Two plugins disagreeing is a real disagreement, so both are named. The later one
wins so the outcome is deterministic, but nothing is hidden.

**If the interface looks coarse**, it is being laid out for a low-density phone.
Raise both — the render resolution is 720p by default and `dpiScale` is 1.0,
which is what Roblox treats as a cheap handset:

```bash
CORDIAL_MONITOR=1 CORDIAL_RESOLUTION=1920x1200 CORDIAL_DPI_SCALE=1.75 \
cargo run --release --bin cordial-run -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

Roblox's graphics-quality FastFlags (`DebugFRMQualityLevelOverride` and the MSAA
overrides) were tested and change nothing here, because they govern 3D scene
rendering and the logged-out landing page is a 2D interface. Resolution and
density are the levers that apply to it.
