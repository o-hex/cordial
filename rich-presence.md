---
title: "Discord Rich Presence"
description: "Show what you are playing in Discord, using the Discord Presence plugin that comes with Cordial."
icon: "gamepad"
---
Cordial comes with a Discord Rich Presence plugin, [`plugins/discord-presence/`](https://github.com/luohoa97/cordial/blob/main/plugins/discord-presence). While Cordial runs, your Discord status reads "Playing Cordial" with an elapsed timer. When a game says more, the status says more.

## What your status shows

| Situation | Status |
|---|---|
| Cordial is open, no game | "Playing Cordial", the Cordial icon, the elapsed timer |
| A game that speaks BloxstrapRPC | The lines, timer and pictures the game sets replace Cordial's, line by line |
| A game that sends nothing | The experience name, `by <creator>`, the game's icon, and the time you joined |
| Any of the above | Discord's buttons: **Join server** (a `roblox://` link naming the exact server) or **See game page** when the server is not known, then **Cordial on GitHub** |

Discord allows two buttons, so the game's button goes first and Cordial's link is always last. If Discord is not running yet, the plugin re-sends every 20 seconds and picks it up within half a minute of it opening.

The "game that sends nothing" row is from the v0.13.1 release notes. This page has not re-run it.

## Turn it on or off

The plugin is built in and starts enabled. The first time you open the **Plugins** page of Settings in a profile, Cordial asks whether to **Allow** what the built-in plugins request, in one dialog. For this plugin that is: `lifecycle.read`, `presence.set`, `settings.read` and `log`. Choose **Allow**; **Not now** leaves it without those permissions, so it publishes nothing.

To stop it, switch **Discord Presence** off in **Settings → Plugins**. You can change any of its permissions there at any time. Grants belong to the profile.

## Use your own Discord application

By default the status appears as Cordial. To show a different name and icon, paste an application ID from [discord.com/developers/applications](https://discord.com/developers/applications) into the plugin's **Discord application ID** setting (the gear on its row in Settings → Plugins). It is 17 to 20 digits and is not a secret. Anything else is ignored, the plugin says so in its log, and the status stays Cordial's.

## In the Flatpak

Cordial's Flatpak can reach Discord's socket at `discord-ipc-0`, and Discord's own Flatpak socket under `xdg-run/app/com.discordapp.Discord`. A second Discord instance on the same machine uses `discord-ipc-1` or higher, which the Flatpak does not cover.

## Limits

- **A refused status is now reported as refused.** In 0.13.0 and earlier, a `presence.set ... came back: ok` log line means "Discord answered", not "Discord accepted".
- The plugin cannot read Discord's state and cannot send anything to Discord except the presence payload. Cordial owns the connection and the buttons; the plugin never sees a socket or builds a link.

Why: [ADR-006](/adr/ADR-006-plugin-events-and-first-party) (built-in features are plugins) and [ADR-007](/adr/ADR-007-host-resources-are-brokered) (Cordial holds the permission, the plugin sends a payload).
