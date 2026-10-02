# Plugins

<!-- sidebarTitle: What plugins are -->
<!-- description: What plugins are for, what they can and cannot do, and how to install one. -->
<!-- icon: puzzle-piece -->

A plugin adds a feature to **Cordial**, not to Roblox. It can set your Discord
status from the game you are in, apply a set of FastFlags, send a desktop
notification, or carry a pack of replacement textures. It cannot run code
inside Roblox, read or change the game's memory, or give you a script
executor. Those abilities are not switched off in Cordial; they do not exist in
it at all, so no plugin and no fork can switch them on.

## How a plugin is kept in check

Each plugin is a separate program, written in TypeScript and run by
[Deno](https://deno.com) with every one of Deno's permissions turned off. It
cannot open files, use the network or start programs on its own. It asks
Cordial for things, and Cordial does them only if you allowed that kind of
request.

Those kinds of request are called **capabilities**. A plugin lists the ones it
wants, and starts with none of them granted. You grant them per profile, and
Cordial describes each one in plain words before you do.

<details>
<summary>Every capability a plugin can ask for</summary>

| Capability | What it lets the plugin do |
|---|---|
| `flags.read` | See the FastFlags in effect and where each one came from |
| `flags.write` | Set FastFlags of its own. They apply at the next launch, and your own `flags.json` still wins |
| `log` | Write to Cordial's log |
| `lifecycle.read` | Know when the client starts, which Roblox version it is, and when it stops |
| `state.read` | Read which place, server and user the client is on |
| `presence.set` | Set your Discord Rich Presence. Cordial talks to Discord, not the plugin |
| `notify.send` | Show a desktop notification |
| `url.open` | Open an `http` or `https` link in your browser |
| `assets.override` | Point Cordial at a folder of replacement assets while running |
| `settings.read`, `settings.write` | Keep its own settings, which Cordial stores for it |
| `events.declare`, `events.publish`, `events.subscribe` | Exchange events with other plugins |

</details>

> [!NOTE]
> A plugin never receives a network connection or a file. When it sets your
> Discord status, it hands Cordial the text and Cordial sends it.

## The plugins that come with Cordial

| Plugin | What it does | On by default |
|---|---|---|
| **Discord presence** | Shows "Using Cordial", or the game's own status if the game provides one, in Discord | Yes, after you allow it |
| **Flag inspector** | Logs every FastFlag in effect and which file set it. Mostly an example to learn from | Yes, after you allow it |
| **FPS Flex** | Picks how frames are presented and caps the frame rate (60 to 240). Applies at the next launch | No: an uncapped frame rate costs heat and battery |
| **Hide the interface** | Unlocks Roblox's shortcuts for hiding the game's interface, for screenshots and videos | Yes, after you allow it |

The first time you open **Settings → Plugins**, each of these asks once for
the capabilities it needs. Until you choose **Allow**, it does nothing.

> [!WARNING]
> **Hide the interface** works only for accounts in the Roblox group that
> unlocks those shortcuts (the one Bloxstrap uses). Cordial cannot check whether
> yours is.

## Installing a plugin

There is no plugin store yet. A plugin reaches you as a file someone shares,
or as a folder you are writing yourself.

<!-- steps -->

### Turn plugins on

Open **Settings → Plugins** and switch on **Use Plugins**. If Deno is not
installed, the page offers a **Download** button (about 39 MB). A `deno`
already on your system is used instead.

### Install the archive

Under **Install from a file**, choose **Choose file…** and pick the plugin.
It must be a `.tar.zst` archive; a `.tar.gz` is refused. Installing a
plugin does not allow it to do anything: a plugin that contains code
arrives switched off.

### Switch it on and grant what it needs

The plugin appears under **Installed**. Switch it on, then turn on only the
capabilities you are happy to give it. Changes reach a running game within
about a second. FastFlags are the exception: they apply at the next launch.

<!-- /steps -->

To remove a plugin, use its **Remove** button. Built-in plugins cannot be
removed, only switched off.

### Where plugins live

| Install | Folder |
|---|---|
| Flatpak | `~/.var/app/io.github.luohoa97.Cordial/data/cordial/plugins/` |
| Other installs | `~/.local/share/cordial/plugins/` |

Which plugins are switched on, and what each may do, is stored per profile, so
two profiles can have different plugins on.

## Writing a plugin

See [Writing a plugin](plugin-development.md) for a working plugin in five
minutes, and the [plugin API reference](plugin-api.md) for every method and
event.

## What does not work yet

- **No plugin store.** The signed index format and its checks exist, but
  nobody runs an index, so there is nothing to browse.
- **FastFlags apply at the next launch.** A plugin cannot change a flag in a
  running game.
- **Two events are never sent.** `client.ready` and `window.resized` are
  declared but nothing publishes them yet, so a plugin waiting for them waits
  forever.
- **Plugins cannot draw anything** over the game or add buttons to Cordial's
  window.
