---
title: "PartyDeck and gamescope"
description: "Run several Roblox windows side by side, one profile each, from PartyDeck or from a gamescope you start yourself."
icon: "users"
---
Cordial can start a game straight from the command line, with no launcher window, and each such start is a Cordial of its own. That is what [PartyDeck](https://github.com/partydeck/partydeck) needs to run several players on one machine, and it is also all gamescope needs.

```bash
cordial --profile P1 --play --fullscreen
```

<Warning>

**Sign every profile in before you use it here.** Inside gamescope the game opens on X11, which has no in-app web window, so the Sign In button cannot finish. Sign each profile in once from a normal Cordial window, then start it from PartyDeck.

</Warning>

## Set it up with PartyDeck

<Steps>
<Step title="Create and sign in one profile per player">


Open Cordial normally. In the header bar, open the profile menu, choose **New Profile…**, name it with letters, digits, `-` and `_` (`P1`, `couch-left`), and sign in. Repeat for each player, then close Cordial. A profile is one account's storage, and two windows cannot share one.


</Step>
<Step title="Add the handler to PartyDeck">


PartyDeck reads handler folders from `~/.local/share/partydeck/handlers/`. Copy the one that matches how you installed Cordial from `packaging/partydeck/` in this repository.

For the deb, rpm and Arch packages, which put `cordial` in `/usr/bin`:

```bash
cp -r packaging/partydeck/Cordial ~/.local/share/partydeck/handlers/
```

For the Flatpak, which runs `flatpak run` and so works for a user or a system install (read the Flatpak note under [What is broken](#what-is-broken) first):

```bash
cp -r packaging/partydeck/Cordial-Flatpak ~/.local/share/partydeck/handlers/
```


</Step>
<Step title="Give each PartyDeck player a profile of the same name">


In PartyDeck, create a profile named exactly like each Cordial profile (`P1`, `P2`) and pick it for that player. The name is what Cordial runs, so it must exist in both places.


</Step>
<Step title="Start">


Pick Cordial in PartyDeck and start. Each player gets a Cordial window running their own profile.


</Step>
</Steps>

## Start a game from the command line

Everything PartyDeck does is these options, so you can use them from a script, a shortcut or your own gamescope.

| Option | What it does |
|---|---|
| `--profile NAME` | Run this profile in this process only. It is not remembered as your profile and it does not hand over to a Cordial that is already open. |
| `--play` | Start Roblox at once with no launcher window. Exits when the game does. |
| `--place-id N` | Join experience `N` instead of the home screen. Implies `--play`. |
| `--fullscreen` | Fill the display the window opens on. gamescope wants this. |
| `--set KEY=VALUE` | Override one setting from Settings for this run, for example `--set present_mode=fifo`. Nothing is saved. A wrong `KEY` lists the right ones. |

Without `--play` and `--profile`, `cordial` behaves as it always has: one launcher, and a second start hands over to it.

Exit status, for a program that starts games: `0` the game ended normally, `1` it could not start (the reason is on stderr), `2` the command line was wrong, `3` the profile is already open in another Cordial, `4` there is no Roblox build to run, `5` the game could not open a window.

### Plain gamescope

```bash
gamescope -W 1280 -H 720 -- cordial --profile P1 --play --fullscreen
```

Add `--backend headless` to run with no display at all. `--expose-wayland` is not needed and makes no difference: Cordial uses the X11 display gamescope provides either way.

## What is broken

<Warning>

**No sign-in inside gamescope.** The game opens on X11 there (gamescope's own Wayland server cannot host the engine's surface), and X11 has no web window. Sign in from a normal Cordial window first. A profile that does not exist yet starts signed out and says so on the console.

</Warning>

<Note>

**PartyDeck guests do not work.** A guest is named `.Name`, which Cordial refuses as a profile name, and a fresh profile could not sign in anyway. Give every player a named profile.

</Note>

- **Controllers.** Cordial reads `/dev/input/js*`, and PartyDeck leaves each instance only its own pad, so each player's controller reaches only their window. This was tested with a stand-in device node, not with PartyDeck and real pads. The Flatpak sandbox held no input devices until `--device=input` was added to its manifest, so a Flatpak older than the release after 0.27.0 cannot see a pad; see [Controllers](/controllers#in-the-flatpak). Whether PartyDeck's one-pad-per-instance binding carries into a Flatpak is untested; Use the native package for split-pad play.
- **Mouse look.** Pointer capture under gamescope has not been tested, because the test sessions had no pointer. **INFERRED:** it works as on any X11 window. If the camera drifts, `gamescope --force-grab-cursor` is the first thing to try.
- **Flatpak.** The handler is untested. **INFERRED:** a Flatpak reaches gamescope's X11 display through `--socket=fallback-x11`. If you started gamescope with `--expose-wayland`, add `--env=CORDIAL_X11=1` right after `run` in the handler's arguments, because the Flatpak does not see gamescope's variables.
- **Starting together.** PartyDeck starts instances half a second apart unless a handler says otherwise, and these ask for three seconds. Two instances started half a second apart on one build store ran side by side in the one test made, which used a stand-in for PartyDeck's command line and not PartyDeck itself.

Why it works this way, and what was measured: [ADR-058](https://github.com/luohoa97/cordial/blob/main/docs/adr/ADR-058-partydeck-and-gamescope.md).
