# Publishing `cordial-git` to the AUR

The package in `cordial-git/` is complete and has never been published. That is
the single cheapest piece of reach Cordial is leaving on the table: Arch users
are a large share of the people who run Roblox on Linux at all, mocktail has
three AUR packages, and Cordial has none — not because the packaging is missing
but because nobody has pushed it.

**This cannot be done for you by an agent, and the reason is worth stating
rather than working around.** Publishing needs an AUR account and an SSH key
registered against it. Creating accounts and handling credentials is exactly
the class of thing a coding agent must not do on someone's behalf, so what
follows is the whole procedure for a person to run once.

## Once, to set up

Register at <https://aur.archlinux.org/register>, then add your public key under
*My Account → SSH Public Key*. The AUR authenticates by key alone; there is no
password prompt on push.

```bash
cat >> ~/.ssh/config <<'CONF'
Host aur.archlinux.org
  User aur
  IdentityFile ~/.ssh/aur
  IdentitiesOnly yes
CONF
```

## Each time

The AUR wants a repository whose root *is* the package directory — `PKGBUILD`
and `.SRCINFO` at the top level, not under `packaging/aur/cordial-git/`. So this
is a separate checkout that the files are copied into, rather than a remote on
this repository.

```bash
git clone ssh://aur@aur.archlinux.org/cordial-git.git /tmp/aur-cordial-git
cp packaging/aur/cordial-git/{PKGBUILD,.SRCINFO,cordial-git.install} /tmp/aur-cordial-git/
cd /tmp/aur-cordial-git
git add -A && git commit -m "Update to 0.7.0" && git push
```

**Regenerate `.SRCINFO` on an Arch machine before pushing**, with
`makepkg --printsrcinfo > .SRCINFO`. The copy in this repository is maintained
by hand because the machine Cordial is developed on is Fedora and has neither
`makepkg` nor `namcap`, so it is kept in step deliberately rather than
generated. A hand-maintained `.SRCINFO` that has drifted from its `PKGBUILD` is
the most common way an AUR package breaks, and it breaks silently: the AUR
serves the metadata from `.SRCINFO` and builds from `PKGBUILD`.

## Check before pushing, on Arch

```bash
cd packaging/aur/cordial-git
makepkg --printsrcinfo | diff -u .SRCINFO -   # must be empty
makepkg -si                                    # it must actually build
namcap PKGBUILD
```

`makepkg -si` is not optional. Cordial's native subtree refuses a non-Clang
compiler outright, needs `binutils` for CMake's archiver step, and compiles the
"this backend is unavailable" arm of each audio backend when its headers are
missing — so a package that builds without `libpipewire`, `libpulse` or
`alsa-lib` present produces a client with no sound and no error, which nobody
would attribute to packaging.

## What is deliberately not here

There is no `cordial` (release) or `cordial-bin` package yet. `cordial-git`
builds from `main`, which is where the project actually is; a stable package
should wait until releases are frequent enough that it is not immediately three
hundred commits behind, which is the state the tags were in before v0.7.0.
