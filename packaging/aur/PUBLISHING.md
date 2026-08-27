# Publishing to the AUR

Two packages live under `packaging/aur/`, both complete and neither ever
published. That is the single cheapest piece of reach Cordial is leaving on the
table: Arch users are a large share of the people who run Roblox on Linux at
all, mocktail has three AUR packages, and Cordial has none — not because the
packaging is missing but because nobody has pushed it.

- **`cordial-git/`** builds whatever commit is at the tip of `main`.
- **`cordial/`** builds a tagged release, and is also what
  `.github/workflows/release.yml` runs on every tag to produce the `.pkg.tar.zst`
  attached to the GitHub release — so by the time you read this, CI has already
  built it successfully at least once. That is a real test of this exact file,
  not a stand-in for one; it is not the same as this file having been pushed
  to the AUR, which still needs the steps below.

**This cannot be done for you by an agent, and the reason is worth stating
rather than working around.** Publishing needs an AUR account and an SSH key
registered against it. Creating accounts and handling credentials is exactly
the class of thing a coding agent must not do on someone's behalf, so what
follows is the whole procedure for a person to run once per package.

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
and `.SRCINFO` at the top level, not under `packaging/aur/<name>/`. So this is a
separate checkout that the files are copied into, rather than a remote on this
repository. Substitute `cordial-git` for `cordial` throughout for the other
package; the steps are the same.

```bash
git clone ssh://aur@aur.archlinux.org/cordial.git /tmp/aur-cordial
cp packaging/aur/cordial/{PKGBUILD,.SRCINFO,cordial.install} /tmp/aur-cordial/
cd /tmp/aur-cordial
git add -A && git commit -m "Update to 0.7.0" && git push
```

**Regenerate `.SRCINFO` on an Arch machine before pushing**, with
`makepkg --printsrcinfo > .SRCINFO`. The copy in this repository is maintained
by hand because the machine Cordial is developed on is Fedora and has neither
`makepkg` nor `namcap`, so it is kept in step deliberately rather than
generated. A hand-maintained `.SRCINFO` that has drifted from its `PKGBUILD` is
the most common way an AUR package breaks, and it breaks silently: the AUR
serves the metadata from `.SRCINFO` and builds from `PKGBUILD`.

For `cordial/PKGBUILD` specifically, bumping `pkgver` and regenerating
`.SRCINFO` is the whole of what a new release needs done to this file by hand.
Nothing else in it should change from one release to the next unless the
dependency list or the build itself changed too.

## Check before pushing, on Arch

```bash
cd packaging/aur/cordial       # or cordial-git
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

## What CI already does, and does not

`.github/workflows/release.yml` runs `makepkg` against `packaging/aur/cordial/PKGBUILD`
on every tag, inside a fresh `archlinux:base-devel` container, and attaches the
resulting package to the GitHub release. That is real evidence the PKGBUILD
builds — but CI cannot register an AUR account or hold the SSH key that pushing
needs, so it stops there. Nothing here is published to the AUR until a
maintainer runs the steps above by hand, and CI building green is not that.

There is no `cordial-bin` package. A prebuilt-binary AUR package would need
somewhere to host the binary CI already produces (the GitHub release itself
would do), and is worth adding once `cordial` and `cordial-git` are both
actually on the AUR and someone asks for it — not before, on the same
reasoning `cordial` itself waited for releases to be frequent enough that it
would not immediately be three hundred commits behind, which was the state the
tags were in before v0.7.0.
