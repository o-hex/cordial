# Installing Cordial

The README has the quickstart commands. This is the detail behind them: what
each package format is for, the honest state of the AppImage's web view, and
how the release signatures and repository trust actually work.

## What you need

- x86-64 Linux, or aarch64 (see [`multiarch.md`](multiarch.md) for what has
  been checked there)
- A Wayland session. X11 is supported too, as of
  [ADR-024](adr/ADR-024-x11-is-supported-again.md), which superseded
  ADR-011's "not developed further"; Wayland is still the primary backend
- Roblox's official Android client, which **you supply** — Cordial ships no
  Roblox code, APK or assets and never will

From an installed APK you need the `lib/x86_64/` objects and the base APK.

**The shortest route to one is the Download Roblox button**, which fetches and
verifies a build without you leaving Cordial. That is new; it used to be
"install Sober first", and that answer still works.

[Sober](https://sober.vinegarhq.org/) downloads Roblox's Android build for its
own use, and Cordial still looks for it there —
`~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/`. Nothing is copied
and nothing is modified; Cordial reads the APK where it already is. If you have
Sober, Cordial finds its build and never asks you for one. You are free to keep
using Sober afterwards, or not.

If you have an APK of your own, Settings takes a path to it, and `--apk` takes
one on the command line. On a split build the engine is in
`split_config.x86_64.apk` rather than `base.apk`; Cordial checks the siblings
itself and says which it tried when it cannot find one.

Nothing else. The Flatpak carries the toolchain and the libraries with it; the
build dependencies are under [Building from source](#building-from-source) below.

## Flatpak vs AppImage

Building from source is for people changing Cordial, not for people running
it — see below.

**Flatpak is the recommended install.** It is sandboxed, it updates in place,
and the manifest is the reference every other package here is built to match.
It also runs on one fixed runtime (GNOME 50), so the GTK, WebKit, Vulkan loader
and audio libraries are the ones Cordial is tested against rather than whatever
version a distribution ships. Most compatibility problems reported with the
other packages come from those differences.

```bash
flatpak remote-add --if-not-exists cordial \
    https://luohoa97.github.io/cordial/cordial.flatpakrepo
flatpak install cordial io.github.luohoa97.Cordial
```

Then launch Cordial from your desktop's application list, or
`flatpak run io.github.luohoa97.Cordial`. `flatpak update` picks up new builds.
Uninstall with `flatpak uninstall io.github.luohoa97.Cordial`, and
`flatpak uninstall --delete-data io.github.luohoa97.Cordial` if you also want the
profiles, the sign-in and the extracted Roblox build gone.

That command installs the `stable` branch, which only moves on a tagged
release. There is also `master`, which moves on every commit to main. The
remote used to publish only `master`, so an install from before `stable`
existed is on it and keeps tracking main until you move it:

```bash
flatpak uninstall io.github.luohoa97.Cordial//master
flatpak install cordial io.github.luohoa97.Cordial//stable
```

(add `--user` to both if that is how you installed it). If you want main
rather than releases, `flatpak install cordial io.github.luohoa97.Cordial//master`
says so explicitly.

**The AppImage is one file that runs on any distribution.** No remote to add,
no package manager, nothing installed system-wide — download
`Cordial-<version>-<commit>-x86_64.AppImage` (or `-aarch64`) from
[the releases page](https://github.com/luohoa97/cordial/releases), then:

```bash
chmod +x Cordial-*.AppImage
./Cordial-*.AppImage
```

It carries GTK4, libadwaita and WebKitGTK with it, so it does not care what
your distribution ships. It installs nothing; delete the file and Cordial is
gone, though your profiles stay in `~/.local/share` until you remove them
yourself. It needs FUSE, which nearly every desktop has; if it refuses to
start, run it with `--appimage-extract-and-run` and it will unpack to a
temporary directory instead. Updates are manual — the Flatpak updates itself,
which is the main practical reason to prefer it. The AppImage is newer and
less proven; read the rest of this section before choosing it.

### AppImage

**The web view, and what is still not established.** WebKitGTK does not link
the processes that draw a page. It spawns `WebKitWebProcess` and
`WebKitNetworkProcess`, loads an injected bundle, and runs `bwrap` and
`xdg-dbus-proxy` for its own sandbox — five things reached through absolute
paths fixed when WebKitGTK itself was built, `/usr/libexec/webkitgtk-6.0` on
Fedora and somewhere different on every other distribution. Up to and including
v0.13.0 the AppImage carried copies of them and nothing made WebKitGTK look at
the copies, so on a host that had never installed WebKitGTK the sign-in window
came up blank and the log said `Failed to spawn child process
".../WebKitNetworkProcess"`. Installing WebKitGTK did not help unless you were
on Fedora, because nobody else uses that path.

Cordial now makes those paths resolve to its own copies inside a private mount
namespace, which needs `bwrap` and unprivileged overlay mounts. If your kernel
or distribution refuses either, the AppImage says so on standard error and
carries on without them, and the web view then needs WebKitGTK 6.0 installed at
Fedora's path. **This has been measured on a stand-in for a machine with no
WebKitGTK, but not yet on a real one, and not on any distribution other than
Fedora** — if the sign-in window is blank, please report it with whatever the
terminal printed rather than assuming Cordial is broken. The Flatpak is
unaffected either way.

## APT (Debian/Ubuntu)

**The `.deb` on the release page installs today and needs no repository.**
Every release attaches one, with a `.cosign.bundle` beside it:

```bash
# From https://github.com/luohoa97/cordial/releases/latest
sudo apt install ./cordial_*_amd64.deb
```

Verifying it first is worth the two commands — see
[Verifying a release download](#verifying-a-release-download). The repository
below is the other route, and it keeps you current through `apt upgrade`.

Cordial's own repository, not a package in Debian or Ubuntu itself — see
[`docs/design/apt-repository.md`](design/apt-repository.md) for why
those are two different things and where this one currently stands.

```bash
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://luohoa97.github.io/cordial/apt/cordial-archive-keyring.gpg \
    -o /etc/apt/keyrings/cordial-archive-keyring.gpg
echo "deb [signed-by=/etc/apt/keyrings/cordial-archive-keyring.gpg] https://luohoa97.github.io/cordial/apt stable main" \
    | sudo tee /etc/apt/sources.list.d/cordial.list
sudo apt update
sudo apt install cordial
```

That is the modern, `apt-key`-free form: the key lives in one file named on
the `deb` line, not in a system-wide trusted keyring every other repository
also writes to. `apt update` after that picks up new releases the same way
it does for any other repository; `sudo apt remove cordial` uninstalls, and
your profiles stay in `~/.local/share` until you remove them yourself, same
as every other package format here.

**Verify the key before you trust it.** A `curl` in a doc is exactly the kind
of instruction a supply-chain attack looks like, so check what you just
downloaded against the fingerprint below:

```bash
gpg --show-keys --with-fingerprint /etc/apt/keyrings/cordial-archive-keyring.gpg
```

The repository is signed: `dists/stable/InRelease` carries an OpenPGP
signature, and the key above has this fingerprint:

    E6BE 3043 5BD6 3471 FD1A  B331 DC05 1D16 7161 8AA6

**INFERRED, not yet checked out of band:** that fingerprint was read from the
published keyring on 2026-09-30, the same place a compromised site would have
changed it. Whoever holds the key should confirm it, and this line should say
so when they have. What was observed that day: `InRelease` verified as a good
signature against that keyring, and the `amd64` and `arm64` `Packages` files
both listed 0.20.1-1. `apt install` itself was not run.

## dnf (Fedora, RHEL, and derivatives)

**The `.rpm` on the release page installs today and needs no repository.**
Every release attaches one, with a `.cosign.bundle` beside it:

```bash
# From https://github.com/luohoa97/cordial/releases/latest
sudo dnf install ./cordial-*.x86_64.rpm
```

Note the `.fcNN` in the filename: only one Fedora release is built at a time
(Fedora 44 as of this writing), for the reasons in
[`packaging/rpm/build-rpm.sh`](../packaging/rpm/build-rpm.sh)'s header. Verifying
first is worth the two commands — see
[Verifying a release download](#verifying-a-release-download). The repository
below is the other route, and it keeps you current through `dnf upgrade`.

Cordial's own repository, not a package in Fedora's own repos — see
[`docs/design/rpm-repository.md`](design/rpm-repository.md) for why
those are two different things and where this one currently stands.

```bash
sudo curl -fsSL https://luohoa97.github.io/cordial/cordial.repo \
    -o /etc/yum.repos.d/cordial.repo
sudo dnf install cordial
```

**Only Fedora 44 has a build today.** The repository is split by
`$releasever` because a `.rpm` built against Fedora 44's `gtk4`/`libadwaita`
is not guaranteed to install on a different release — see
[`docs/design/rpm-repository.md`](design/rpm-repository.md#why-releasever-and-what-that-honestly-costs)
for the full argument. **If your `dnf` reports a `$releasever` other than
44, the command above 404s honestly** rather than installing a build meant
for a different release; that is by design, not a bug to report, until
`release.yml` builds a second release.

**Verify the key before you trust it:**

```bash
curl -fsSL https://luohoa97.github.io/cordial/rpm/RPM-GPG-KEY-cordial | gpg --show-keys
```

against this fingerprint:

    E5FA CC1B D170 8EC9 4FFF  5817 FDD1 0A8B 6D7F 10B9

The `.repo` file sets `repo_gpgcheck=1` and `gpgcheck=0`: what is signed is
each release directory's `repodata/repomd.xml`, not the individual `.rpm`
([`docs/design/rpm-repository.md`](design/rpm-repository.md) has the reason).

**INFERRED, not yet checked out of band**, on the same terms as the apt key
above: read from the published site on 2026-09-30. That day
`rpm/44/x86_64/repodata/repomd.xml.asc` verified as a good signature against
that key, and `dnf install` was not run.

## pacman (Arch and derivatives)

**Install the package from the release page. That works today and needs no
key.** Every release attaches a `.pkg.tar.zst` built by the same `makepkg` run
an AUR user's own machine would do, with a `.cosign.bundle` beside it:

```bash
# From https://github.com/luohoa97/cordial/releases/latest
sudo pacman -U cordial-*-x86_64.pkg.tar.zst
```

Verifying it first is two commands and is worth doing — see
[Verifying a release download](#verifying-a-release-download) below. That
signature is keyless, so there is no Cordial key to add to your keyring and
none to trust.

**The AUR packages are not published by this project.** `cordial`,
`cordial-bin` and `cordial-git` on the AUR belong to an account that is not the
maintainer's (`taxin-404`). On 2026-09-30 they were copies of
[`packaging/aur/`](../packaging/aur) from 0.17.0, fetching only from this
repository, and `cordial-bin`'s checksum matched the official 0.17.0 release
file. Nobody here reviews what that account publishes next, so prefer the
release package or the signed pacman repository below.

**Cordial's own pacman repository** is published and signed. Import its key and
add it to `pacman.conf`:

```bash
curl -fsSL https://luohoa97.github.io/cordial/arch/cordial-archive-keyring.asc \
    | sudo pacman-key --add -
sudo pacman-key --lsign-key C82BBD7D82744F804A68DA8B3A69D3241BA6288F
```

```
[cordial]
Server = https://luohoa97.github.io/cordial/arch/$arch
SigLevel = DatabaseRequired PackageNever
```

The database is signed and the packages are not, which is what
`DatabaseRequired PackageNever` says
([`docs/design/pacman-repository.md`](design/pacman-repository.md) explains why).
**INFERRED, not yet checked out of band:** that fingerprint was read from the
published keyring on 2026-09-30. On that day `cordial.db.sig` verified as a
good signature against it; `pacman -Sy` was not run, and neither was the
`pacman.conf` stanza. The `SigLevel` line comes from the design note, which says
itself that it was reasoned from `pacman.conf(5)` rather than tried.

## Verifying a release download

**Every `.deb`, `.rpm`, `.AppImage` and Arch package on a release page is signed**,
and each has a `.cosign.bundle` beside it. The signature is keyless: there is no
Cordial signing key anywhere, and there is nothing for a maintainer to lose. What
the signature proves is that the file came out of this repository's own release
workflow, at that tag, and not from someone who obtained a key.

Install [cosign](https://docs.sigstore.dev/cosign/system_config/installation/), then,
for whichever file you downloaded:

```bash
cosign verify-blob \
  --bundle cordial_0.11.0-1_amd64.deb.cosign.bundle \
  --certificate-identity-regexp '^https://github\.com/luohoa97/cordial/\.github/workflows/release\.yml@refs/tags/v' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  cordial_0.11.0-1_amd64.deb
```

`Verified OK` is the whole of the answer. **Do not drop the two `--certificate-*`
flags** — without them cosign will happily confirm that *somebody* signed the
file, which is not the question you are asking.

The trade is that every signature is recorded permanently in Sigstore's public
transparency log. For public release artefacts that is the point rather than a
cost: it is what lets you check, a year later, that a file was signed by this
workflow at that tag.

This covers the release page. **The Flatpak remote and the apt, dnf and pacman
repositories are a different question**: they use OpenPGP keys that Sigstore
cannot supply, and the next section says what those cover.

## Trust, and what the Flatpak signature covers

**The Flatpak remote is signed.** The published `cordial.flatpakrepo` carries a
GPG key, and the repository summary has a detached signature, so `flatpak
install` verifies that what it downloads was signed by that key and not merely
that it matches the repository's own checksums. The key:

    8364 5E9B 8F6C 4B29 227D  4629 4310 E617 967A BDD8

Observed on 2026-09-30: `summary.sig` is published, and with that key imported
into a throwaway OSTree repository `ostree remote refs` reads the summary with
`gpg-verify-summary` on; the same command against a remote with no key fails
with `Can't check signature: public key not found`. `flatpak install` itself was
not run against it. The fingerprint has the same standing as the apt one above:
read from the published file, not confirmed out of band.

**A remote added while it was unsigned stays unverified.** Flatpak records
`gpg-verify=false` when the remote is added, and a later signed definition does
not change it. If you added it before the signing key existed, remove and re-add
it (`flatpak remote-delete cordial`, then the `remote-add` above).

What a signature does not do is make the GitHub Pages site trustworthy to host
it: whoever holds the private key, which lives as a repository secret, can sign
anything. That is still a weaker arrangement than Flathub's. Signing is set up in
[`.github/workflows/flatpak.yml`](../.github/workflows/flatpak.yml), and
[`docs/design/flatpak-remote-signing.md`](design/flatpak-remote-signing.md) is how
the key was meant to be generated. If you would rather not extend that trust,
[building from source](#building-from-source) below is the whole of the
alternative.

**Cordial is not on Flathub, and on current policy it cannot be.** Flathub's
generative-AI policy does not allow applications containing AI-generated or
AI-assisted code, documentation or content, and Cordial contains a great deal of
both — the git history records it in `Co-Authored-By` trailers rather than
hiding it. The policy allows exceptions for mature, well-maintained projects,
and that is the only route; it is not one to take by quietly deleting the
evidence. **This remote is therefore the distribution channel, not a stopgap
until a better one arrives.** Being a third-party client that fetches a
proprietary build at the user's request is not itself the obstacle — Sober's own
published manifest for `org.vinegarhq.Sober` grants `--share=network` and
downloads Roblox's Android build at runtime with no `extra-data` source and
nothing bundled, the same shape this project uses, and it has been live on
Flathub throughout. The AI policy is the whole of what stands in the way, not
what Cordial downloads or when.

> [!NOTE]
> **Measured end to end on 2026-08-05, flatpak 1.18.0**, against the published
> URL rather than a stand-in: `remote-add` accepted, `remote-ls` returning
> `app/io.github.luohoa97.Cordial/x86_64/master`, `install` placing both
> `cordial-shell` and `cordial-run` in `/app/bin`, and `flatpak run` bringing up
> the launcher window and holding it. The appstream branch resolves and the
> metainfo validates, so a software centre lists it too.
>
> **One known limitation of the Flatpak specifically.** The updater asks
> NetworkManager on the system bus whether your connection is metered, the
> sandbox has no system bus, and the check fails closed — so a Flatpak install
> treats every connection as metered and will not download a Roblox build in the
> background unless you turn on *Download on metered connections*. Manual
> downloads are unaffected.
>
> [The workflow](https://github.com/luohoa97/cordial/actions/workflows/flatpak.yml)
> is worth a glance before a fresh install: it publishes only on a green run, so
> a red one on `main` means the remote is serving the previous build.

## Building from source

**You do not need this to run Cordial** — the package routes above are
measured to work. Build from source if you are changing Cordial, if you would
rather not extend trust to a remote signed by a key held as a CI secret, or if
you want a build with your own patches in it.

Building needs rather more than running does:

- **Clang** — AOSP bionic uses C11 `_Atomic` inside C++ headers and GCC rejects it
- **GTK4 (≥ 4.12) and libadwaita (≥ 1.5)** development packages — the core shell
  in `crates/cordial-shell` is `AdwApplicationWindow`/`AdwToolbarView` end to
  end (see [ADR-002](adr/ADR-002-core-shell-and-ui-handoff.md) and
  [ADR-011](adr/ADR-011-wayland-and-libadwaita.md)), and `gtk4-sys`/
  `libadwaita-sys` link against them via `pkg-config` at build time. Fedora:
  `dnf install gtk4-devel libadwaita-devel`. Debian/Ubuntu:
  `apt install libgtk-4-dev libadwaita-1-dev`. Arch: `pacman -S gtk4 libadwaita`
- **Boost's headers** on x86-64 (`boost-devel` / `libboost-dev` / `boost`), for
  dynarmic, the VR mode's translator. `git clone --recursive` fetches it and
  its own submodules.
- **PipeWire's development headers** (`pipewire-devel` / `libpipewire-0.3-dev`),
  optional — for OpenSL ES audio. `native/CMakeLists.txt` detects them via
  `pkg-config` and compiles the real audio backend if found, or the previous
  link-only stub (no sound, but everything else works) if not. Either way
  `libpipewire-0.3.so` itself is `dlopen`'d at run time, never linked, so a
  build made with the headers still runs — audio-less — on a machine that
  only has the runtime library, or neither.

To build the Flatpak yourself, which produces the same package the remote
serves:

```bash
git clone https://github.com/luohoa97/cordial
cd cordial
packaging/build-flatpak.sh --install
```

That one needs no submodules: the manifest pins `third_party/libjnivm` and
`third_party/mcpelauncher-linker` by commit and fetches them itself, and it
pins every crate by the sha256 already in `Cargo.lock`
(`packaging/cargo-sources.json`). flatpak-builder downloads the lot up front;
the compile itself runs with the network unshared, so what comes out is
reproducible ([issue #3](https://github.com/luohoa97/cordial/issues/3)). If you
change a dependency, run `python3 packaging/cargo-sources.py` in the same
commit as the `Cargo.lock` change or the Flatpak build will fail with
`no matching package`.

For development, skip Flatpak and build the binaries directly. This one *does*
want the submodules:

```bash
git clone --recursive https://github.com/luohoa97/cordial
cd cordial
cargo build --release
```
