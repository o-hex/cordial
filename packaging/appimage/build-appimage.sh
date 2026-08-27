#!/usr/bin/env bash
# Build a distro-agnostic AppImage for Cordial.
#
# This is the packaging format with the most work still to prove out, and
# that is worth saying before the recipe: an AppImage's whole job is to carry
# its own copies of libraries a host might not have at the right version, and
# Cordial links four things a bare `ldd`-following bundler does not fully
# reach -- GTK4, libadwaita and WebKitGTK are ordinary DT_NEEDED dependencies
# that linuxdeploy's ELF walk does find, but WebKitGTK's own helper
# processes (WebKitWebProcess, WebKitNetworkProcess) are separate executables
# it spawns rather than links, and GSettings schemas are found by path rather
# than by symbol. Both are handled below by hand rather than by the bundler,
# and neither has been exercised on a second machine -- see AppRun's own
# comment on WEBKIT_EXEC_PATH, which is the single most likely thing to need
# a follow-up fix once someone runs the result outside this build's own
# container. Say so in any report of this rather than claiming the AppImage
# works; nobody has launched one yet.
#
# Built inside registry.fedoraproject.org/fedora:44 -- the one environment
# this repository has proven builds gtk4 4.22/libadwaita 1.9 correctly
# (test.yml exists because Fedora 43 and Ubuntu 24.04 are both older and fail
# three *-sys build scripts). The AppImage bundles what that container has so
# the result runs on hosts with much older, or no, GTK4 at all.
#
# Usage:
#     packaging/appimage/build-appimage.sh [--outdir DIR]
#
# Needs: cargo, clang/clang++, cmake, pkg-config, the GTK4/libadwaita/
# WebKitGTK development headers, rsvg-convert (for the AppDir's PNG icon --
# AppImage integration tooling looks for one at the AppDir root even though
# Cordial's own icon is scalable SVG everywhere else), and network access to
# fetch linuxdeploy and appimagetool on first use.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
outdir="$repo/dist/appimage"
while [ $# -gt 0 ]; do
    case "$1" in
        --outdir) outdir=$2; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

cd "$repo"
eval "$(packaging/version.sh)"
echo "==> building cordial ${CORDIAL_DESCRIBE}"

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "error: $1 is not installed" >&2
        exit 1
    }
}
for tool in cargo clang rsvg-convert readelf glib-compile-schemas; do
    need "$tool"
done

tools_dir="${CORDIAL_APPIMAGE_TOOLS_DIR:-$repo/target/appimage-tools}"
mkdir -p "$tools_dir"

fetch_pinned() {
    # A fixed release and its own sha256, computed by hand against the file
    # this pins and checked in here rather than trusted from a remote
    # checksums file, because neither AppImage/appimagetool nor
    # linuxdeploy/linuxdeploy publishes one on their release pages -- GitHub's
    # own asset listing carries a size and a download count, nothing more.
    # Bump the version and the sum together if either tool is ever updated.
    local url=$1 sha256=$2 dest=$3
    if [ -f "$dest" ] && echo "${sha256}  ${dest}" | sha256sum --check --status; then
        return 0
    fi
    curl --fail --location --retry 3 --output "$dest" "$url"
    echo "${sha256}  ${dest}" | sha256sum --check --status || {
        echo "error: $dest does not match the pinned checksum" >&2
        exit 1
    }
    chmod +x "$dest"
}

fetch_pinned \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage \
    c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d \
    "$tools_dir/linuxdeploy-x86_64.AppImage"
fetch_pinned \
    https://github.com/AppImage/appimagetool/releases/download/1.9.0/appimagetool-x86_64.AppImage \
    46fdd785094c7f6e545b61afcfb0f3d98d8eab243f644b4b17698c01d06083d1 \
    "$tools_dir/appimagetool-x86_64.AppImage"

# Both tools are themselves AppImages, and this normally needs FUSE to mount.
# A container has none, so this extracts and runs instead -- the same
# variable mocktail's own packages.yml sets around the equivalent step.
export APPIMAGE_EXTRACT_AND_RUN=1

export CC=clang CXX=clang++
export CORDIAL_BUILD_VERSION="$CORDIAL_DESCRIBE"

# Both crates' `webview` features, never one alone -- see the identical
# comment in packaging/rpm/cordial.spec's %build and packaging/deb/build-deb.sh
# for the shape of the bug that taught this project to say so at every
# callsite: with only one crate's feature on, the linker collects
# webview::open silently and the binary carries no WebKitGTK, with no error
# anywhere in the build.
cargo build --release --locked \
    --features cordial-shell/webview,cordial-runtime/webview

readelf -d target/release/cordial-run | grep -qi webkit || {
    echo "cordial-run linked no WebKitGTK; the webview features did not take" >&2
    exit 1
}

appdir="$repo/target/appimage/AppDir"
rm -rf "$appdir"
mkdir -p "$appdir"

install -Dm755 target/release/cordial-shell "$appdir/usr/bin/cordial-shell"
install -Dm755 target/release/cordial-run   "$appdir/usr/bin/cordial-run"

install -Dm644 packaging/io.github.luohoa97.Cordial.desktop \
    "$appdir/usr/share/applications/io.github.luohoa97.Cordial.desktop"
install -Dm644 packaging/io.github.luohoa97.Cordial.metainfo.xml \
    "$appdir/usr/share/metainfo/io.github.luohoa97.Cordial.metainfo.xml"
install -Dm644 packaging/icons/hicolor/scalable/apps/io.github.luohoa97.Cordial.svg \
    "$appdir/usr/share/icons/hicolor/scalable/apps/io.github.luohoa97.Cordial.svg"
install -Dm644 packaging/icons/hicolor/scalable/apps/io.github.luohoa97.Cordial.Frostbite.svg \
    "$appdir/usr/share/icons/hicolor/scalable/apps/io.github.luohoa97.Cordial.Frostbite.svg"

licdir="$appdir/usr/share/licenses/cordial"
install -Dm644 LICENSE "$licdir/LICENSE"
install -Dm644 NOTICE "$licdir/NOTICE"
install -Dm644 THIRD-PARTY-NOTICES.md "$licdir/THIRD-PARTY-NOTICES.md"
install -Dm644 third_party/libbadcpu/LICENSE.upstream "$licdir/libbadcpu-MIT.txt"
install -Dm644 third_party/mcpelauncher-linker/LICENSE "$licdir/mcpelauncher-linker-MIT.txt"
install -Dm644 third_party/mcpelauncher-linker/core/NOTICE "$licdir/aosp-NOTICE.txt"
install -Dm644 third_party/libjnivm/LICENSE "$licdir/libjnivm-MIT.txt"
install -Dm644 third_party/mocktail-webview/LICENSE "$licdir/mocktail-webview-Apache-2.0.txt"

install -Dm755 packaging/appimage/AppRun "$appdir/AppRun"

# AppImage integration tooling (and appimagetool's own validation) wants a
# desktop file and an icon at the AppDir root, not only under usr/share/. A
# copy rather than a symlink, because appimagetool refuses to package a
# symlink pointing outside the tree it is squashing.
cp "$appdir/usr/share/applications/io.github.luohoa97.Cordial.desktop" \
    "$appdir/io.github.luohoa97.Cordial.desktop"
# Rasterised because AppImage's own integration (and thumbnailers that read
# AppImages without extracting them) commonly assume a PNG at the root even
# where the desktop's Icon= key resolves an SVG everywhere else -- 256x256
# matches the largest size Cordial's own icon theme directory would carry had
# one been rendered, and is large enough not to look soft in a file manager.
rsvg-convert --width 256 --height 256 \
    packaging/icons/hicolor/scalable/apps/io.github.luohoa97.Cordial.svg \
    -o "$appdir/io.github.luohoa97.Cordial.png"

echo "==> bundling shared libraries with linuxdeploy"
# Plain linuxdeploy, deliberately with no GTK plugin. linuxdeploy-plugin-gtk
# targets GTK3's module and schema layout; this workspace is GTK4 and
# libadwaita, which the plugin does not know how to bundle, and using it
# for the wrong toolkit version risks bundling GTK3 pieces alongside GTK4
# ones rather than helping. The library discovery linuxdeploy does on its
# own -- walking each --executable's ELF dependencies and copying what it
# finds into usr/lib, then rewriting rpaths -- is toolkit-agnostic and is
# the part actually needed here; GSettings schemas and the WebKitGTK helper
# binaries, which that walk cannot see because neither is a DT_NEEDED entry,
# are handled by hand below instead.
webkit_libexec=$(rpm -ql webkitgtk6.0 2>/dev/null | grep -m1 '/libexec/webkitgtk-6.0$' || true)
executables=(--executable "$appdir/usr/bin/cordial-shell" --executable "$appdir/usr/bin/cordial-run")
if [ -n "$webkit_libexec" ] && [ -d "$webkit_libexec" ]; then
    # Every helper binary passed as its own --executable, not just copied,
    # so linuxdeploy's dependency walk covers *their* DT_NEEDED entries too
    # -- a WebProcess is a separate executable with its own library needs,
    # not merely a data file sitting next to cordial-run.
    while IFS= read -r -d '' helper; do
        executables+=(--executable "$helper")
    done < <(find "$webkit_libexec" -maxdepth 1 -type f -executable -print0)
else
    # Never make a stub lie: better a loud build failure than an AppImage
    # that silently ships a web view with no process to run it in, which is
    # exactly the "webview doesnt work" shape this project has already hit
    # once from the Flatpak missing a Cargo feature rather than a binary.
    echo "error: could not find webkitgtk-6.0's libexec directory (WebKitWebProcess, WebKitNetworkProcess)" >&2
    echo "  the AppImage's web view would have no process to run in" >&2
    exit 1
fi

"$tools_dir/linuxdeploy-x86_64.AppImage" \
    --appdir "$appdir" \
    "${executables[@]}" \
    --desktop-file "$appdir/io.github.luohoa97.Cordial.desktop" \
    --icon-file "$appdir/io.github.luohoa97.Cordial.png"

echo "==> bundling the WebKitGTK helper binaries themselves"
# linuxdeploy was just given each helper as an --executable so their own
# library dependencies land in usr/lib, but linuxdeploy places binaries
# named as --executable next to usr/bin, not back where WebKitGTK's
# ProcessLauncher expects to find them. Put the actual helper tree at
# usr/libexec/webkitgtk-6.0, which is what AppRun's WEBKIT_EXEC_PATH points
# WebKitGTK back at.
install -d "$appdir/usr/libexec/webkitgtk-6.0"
find "$webkit_libexec" -maxdepth 1 -type f -executable -exec \
    install -m755 {} "$appdir/usr/libexec/webkitgtk-6.0/" \;
# WebKitGTK also reads its own injected bundle and sandbox profile from
# beside the libexec directory in some layouts; copied best-effort rather
# than gated on, since an absent one is a narrower loss (likely the GPU
# process sandbox) than an absent helper binary is.
webkit_share=$(rpm -ql webkitgtk6.0 2>/dev/null | grep -m1 '/share/webkitgtk-6.0$' || true)
if [ -n "$webkit_share" ] && [ -d "$webkit_share" ]; then
    install -d "$appdir/usr/share/webkitgtk-6.0"
    cp -a "$webkit_share/." "$appdir/usr/share/webkitgtk-6.0/"
fi

echo "==> compiling GSettings schemas into the AppDir"
# Looked up by GIO through GSETTINGS_SCHEMA_DIR at runtime (see AppRun), not
# discoverable from any binary's DT_NEEDED entries, so linuxdeploy's walk
# above never touches this. Best-effort: GTK4/libadwaita read a handful of
# schemas for things like colour-scheme preference, and their absence is a
# fallback-to-default rather than a crash, which is a materially smaller risk
# than the WebKitGTK helper binaries above -- hence no hard failure here.
schemas_src=/usr/share/glib-2.0/schemas
if [ -d "$schemas_src" ]; then
    install -d "$appdir/usr/share/glib-2.0/schemas"
    cp "$schemas_src"/*.xml "$appdir/usr/share/glib-2.0/schemas/" 2>/dev/null || true
    glib-compile-schemas "$appdir/usr/share/glib-2.0/schemas"
else
    echo "warning: $schemas_src not found; the AppImage ships no compiled GSettings schemas" >&2
fi

echo "==> appimagetool"
outfile="$outdir/Cordial-${CORDIAL_DESCRIBE}-x86_64.AppImage"
mkdir -p "$outdir"
ARCH=x86_64 "$tools_dir/appimagetool-x86_64.AppImage" "$appdir" "$outfile"

chmod +x "$outfile"
ls -lh "$outfile"
echo "built: $outfile"
echo
echo "UNVERIFIED: this AppImage has not been launched on a second machine."
echo "See AppRun's comment on WEBKIT_EXEC_PATH before trusting the web view"
echo "works, and test on a distro other than Fedora before calling this done."
