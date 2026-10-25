# Installing plugins

<!-- description: Getting Deno, loading a plugin from a folder while you develop it, and what changes apply without a restart. -->
<!-- icon: download -->

The steps for installing a plugin archive are in [Plugins](plugins-overview.md#install-a-plugin). This page covers what they assume (Deno) and the folder route for plugins you are writing.

## Deno

Plugins are TypeScript run under [Deno](https://deno.com) ([ADR-008](adr/ADR-008-plugins-are-typescript-on-deno.md)), so a `deno` has to exist on the machine. Arch is the only distribution that packages one, and Cordial's AUR packages depend on it. Fedora and Debian ship none, and inside the Flatpak there is no host to install one on.

Where there is no `deno` on `PATH`, **Settings → Plugins** shows a row offering to download it. The row is absent when you already have one, and a `deno` on `PATH` is always used in preference to the downloaded one.

- It is a pinned Deno release, about 39 MB, downloaded once.
- Its checksum is written into Cordial's source and verified before the file is put in place.
- It lands under Cordial's own data directory, not anywhere system-wide.

> [!NOTE]
> Before 0.13.1 there was no interpreter and no row. On every install except a hand-built one with Deno already present, plugins were listed, granted and switched on without running a line.

## Load a plugin from a folder

While writing a plugin, skip the archive. Either put the folder in the plugins directory, so that `plugin.json` is at `<plugin-id>/plugin.json`, or load it where it is.

| Install | Plugins directory |
|---|---|
| Flatpak | `~/.var/app/io.github.luohoa97.Cordial/data/cordial/plugins/` |
| Other installs | `~/.local/share/cordial/plugins/` |

To load a folder in place, open **Settings → Plugins → Developing a plugin → Add a plugin folder** and choose the folder that has `plugin.json` in it.

- It is listed under Installed with a Development tag and its path, with the same switch, permissions and health line as any other plugin. There is no update or uninstall, because there is no archive to replace.
- The minus button on its row stops Cordial loading the folder. It asks first and never deletes the folder or anything in it. What you allowed the plugin stays on the profile.
- Adding or removing a folder takes effect the next time you press Roblox. Edits inside a listed folder reload as you save. Why: [ADR-038](adr/ADR-038-plugin-hot-swap.md).
- A folder whose id an installed or built-in plugin already has, and one that has gone missing, are listed with the reason they did not load.
- Folders you put in `CORDIAL_UNPACKED_PLUGINS` yourself, outside Settings, are not shown on this page and cannot be removed from it.

Switching **Use Plugins** off greys out everything on the page, development folders and installing included.

## What needs a restart

Nothing, except FastFlags. A running client notices a new plugin directory, a grant, and Settings' own switch within a second or two, and starts, stops or restarts only the plugin that changed ([ADR-038](adr/ADR-038-plugin-hot-swap.md)). That covers installing, updating, removing, enabling, disabling and granting.

A `flags.write` layer takes effect at the next launch, because the engine reads its flags once at startup.

## Writing one

See [Writing a plugin](plugin-development.md). The capability model is [ADR-007](adr/ADR-007-host-resources-are-brokered.md).
