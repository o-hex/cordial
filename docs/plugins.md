# Installing plugins

## Plugins need Deno, and Cordial will fetch it

Plugins are TypeScript run under [Deno](https://deno.com)
([ADR-008](adr/ADR-008-plugins-are-typescript-on-deno.md)), so there has to
be an interpreter on the machine. **Arch is the only distribution that packages
one**, and Cordial's AUR packages depend on it; Fedora and Debian ship none, and
inside the Flatpak there is no host to install one on at all.

So where there is no `deno` on `PATH`, **Settings → Plugins** shows a row
offering to download it, and that row is absent on a machine that already has
one. The download is a pinned Deno release with its checksum written into
Cordial's source, verified before the file is put in place, and it lands under
Cordial's own data directory rather than anywhere system-wide. It is about
39 MB and you only do it once.

If you would rather install it yourself, any `deno` on `PATH` is used in
preference to the downloaded one.

**Before 0.13.1 there was no interpreter and no row**, so on every install
except a hand-built one with Deno already present, plugins were listed, granted
and switched on without ever running a line.

## Installing somebody else's plugin

There is no plugin store, and this is the honest state of it: there is a
registry format, signature checking and an installer, and no populated registry
to point them at. Until there is, a plugin arrives as a directory or an archive
and you put it in place yourself.

**Settings → Plugins → Install from a file → Plugin archive (`.tar.zst`)**, and choose the file.
Cordial unpacks it into place, and it then appears under Plugins, switched off,
with the permissions it is asking for listed. Nothing runs until you say so.

That is the whole procedure. You do not need a terminal and you do not need to
know where plugins live.

**The archive is how a plugin travels; a folder is what it is.** A `.tar.zst`
holds the plugin directory's contents, zstd-compressed — zstd for ratio and
speed, tar because zip's Unix mode bits are optional and a plugin arriving
without its execute bit is a confusing failure. **It is not a `.tar.gz`.** If
somebody hands you one of those it is not a Cordial plugin archive, whatever is
inside it, and the picker will not take it.

If you are writing a plugin rather than installing one, skip the archive: put
the folder straight into `~/.local/share/cordial/plugins/<plugin-id>/` so that
its `plugin.json` is at `…/<plugin-id>/plugin.json`. Under Flatpak that path is
`~/.var/app/io.github.luohoa97.Cordial/data/plugins/` instead, since that is
where the sandbox keeps its data.

**Or load the folder where it is.** Settings → Plugins → Developing a plugin →
Add a plugin folder, and choose the plugin's own folder, the one with
`plugin.json` in it. It is listed under Installed with a Development tag and
its folder path, with the same switch, permissions and health line as any other
plugin, and no update or uninstall, because there is no archive to replace. The
minus button on its row stops Cordial loading the folder; it asks first, and it
never deletes the folder or what is in it. What you allowed the plugin stays on
the profile. Adding or removing a folder takes effect the next time you press
Roblox, since development folders are deliberately outside the hot-swap
reconciler ([ADR-038](adr/ADR-038-plugin-hot-swap.md)); edits inside a listed
folder reload as you save. A folder whose id an installed or built-in plugin
already has, and one that has gone missing, are listed with the reason they did not load.
Folders you put in `CORDIAL_UNPACKED_PLUGINS` yourself, outside Settings, are
not shown on this page and cannot be removed from it.

Switching **Use Plugins** off greys out everything on this page, development
folders and installing included.

**No restart needed, since [ADR-038](adr/ADR-038-plugin-hot-swap.md).** A
client already running notices the new directory, the grant you add for it,
and Settings' own switch within a second or two, and starts, stops or
restarts exactly the plugin that changed rather than needing a fresh launch.
That covers installing, updating, removing, enabling, disabling and granting
— everything except a `flags.write` layer, which has always taken effect at
the next launch and still does (ADR-005), because `FFlag`/`FInt`/`FString`
are read once at startup regardless of who is asking to change them.

**Trust the source.** A plugin runs as a real process on your machine. Cordial
gives it no ambient permissions — no file access, no network, no environment, no
subprocess, and every capability it uses is one you approved by name — but that
is a boundary, not a guarantee about intent, and installing something because a
stranger linked it is the same decision it is anywhere else.

Writing one is [`plugins/README.md`](../plugins/README.md), and the capability
model is [ADR-007](adr/ADR-007-host-resources-are-brokered.md).
