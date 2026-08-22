#!/usr/bin/env bash
# Build a source RPM for Cordial from a git checkout.
#
# Two tarballs, and both exist because a Copr build may run with networking
# switched off and a build that only works when it happens to be on is not
# reproducible:
#
#   <archive>.tar.zst         the working tree, submodules included -- `git
#                             archive` cannot do that, so this uses
#                             `git ls-files --recurse-submodules`
#   <archive>-vendor.tar.zst  `cargo vendor`, the 200-odd crates Cargo.lock
#                             already pins
#
# Run it from anywhere; it locates the repository from its own path.
#
#     packaging/rpm/make-srpm.sh              # SRPM into ~/rpmbuild/SRPMS
#     packaging/rpm/make-srpm.sh --outdir /tmp/out
#
# Needs `git`, `cargo`, `rpmbuild` and `zstd`. On an atomic host none of those
# are on the host filesystem; run this inside a toolbox.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(git -C "$here" rev-parse --show-toplevel)
outdir=""
while [ $# -gt 0 ]; do
    case "$1" in
        --outdir) outdir=$2; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

cd "$repo"

# **The version comes from `git describe --tags` and from nowhere else.**
# The window title is `git describe --tags --always --dirty`, stamped by
# crates/cordial-shell/build.rs, so a package numbered from any other source
# would disagree with the string the running client prints -- and a build with
# no tag in reach shows a bare hash, which has already produced one confusing
# bug report.
describe=$(git describe --tags --long --abbrev=7)   # v0.6.0-108-g9d9c980
case "$describe" in
    v*) ;;
    *) echo "expected a v-prefixed tag, got '$describe'" >&2; exit 1 ;;
esac
version=${describe#v}; version=${version%%-*}       # 0.6.0
rest=${describe#v${version}-}                       # 108-g9d9c980
commits=${rest%%-*}                                 # 108
shorthash=${rest#*-g}                               # 9d9c980
commit=$(git rev-parse HEAD)
date=$(git show -s --format=%cd --date=format:%Y%m%d HEAD)

if [ -n "$(git status --porcelain)" ]; then
    # Not fatal, because the tarball is built from HEAD and is therefore
    # unaffected -- see the staging step, and read the warning there before
    # changing it back.
    echo "note: the working tree is dirty; the tarball is built from HEAD ($describe) and ignores it" >&2
fi

if [ "$commits" = "0" ]; then
    snapinfo=""
    release='1%{?dist}'
    archive="cordial-${version}"
else
    snapinfo="${commits}.${date}git${shorthash}"
    release="1.${snapinfo}%{?dist}"
    archive="cordial-${version}-${snapinfo}"
fi

# /var/tmp rather than /tmp: on the host this was written for, /tmp is tmpfs
# with under 5 GB, and the vendored crate tree plus two tarballs comes to a few
# hundred megabytes before rpmbuild has started. Override with TMPDIR.
work=$(mktemp -d -p "${TMPDIR:-/var/tmp}")
trap 'rm -rf "$work"' EXIT
stage="$work/cordial-${version}"

echo "==> staging HEAD (submodules included)"
# **`git archive HEAD`, not the working tree, and this cost a whole build to
# learn.** The first version of this used `git ls-files --recurse-submodules`
# piped into tar, which lists the tracked *paths* and then reads their contents
# out of the working tree. Another agent was editing crates/ at the moment the
# tarball was cut, and it captured a half-finished change: the new
# implementation of DeviceProfile::parse alongside the old assertion about it.
# %check duly failed with
#
#   left: Some(PcWindows11)  right: Some(AndroidTablet)
#
# which looks exactly like a broken package and was a broken snapshot. The same
# commit built from a pristine clone passes. A source tarball must be a
# function of a commit and nothing else.
#
# One `git archive` per repository, because git archive does not descend into
# submodules -- and without third_party/mcpelauncher-linker present,
# native/CMakeLists.txt has no linker to build and the *-sys build script
# panics with "is not checked out". `git submodule status --recursive` prints
# paths relative to the top level, nested ones included, so the loop is flat.
git archive --format=tar --prefix="cordial-${version}/" HEAD | tar -xf - -C "$work"
while read -r path; do
    [ -n "$path" ] || continue
    ( cd "$path" && git archive --format=tar --prefix="cordial-${version}/${path}/" HEAD ) \
        | tar -xf - -C "$work"
done < <(git submodule status --recursive | awk '{print $2}')

echo "==> vendoring crates"
( cd "$stage" && cargo vendor --locked "$work/vendor" >/dev/null )

echo "==> writing tarballs"
srcdir=${outdir:-$(rpm --eval %{_sourcedir})}
mkdir -p "$srcdir"
tar --zstd -cf "$srcdir/${archive}.tar.zst" -C "$work" "cordial-${version}"
tar --zstd -cf "$srcdir/${archive}-vendor.tar.zst" -C "$work" vendor

echo "==> writing the spec"
spec="$work/cordial.spec"
sed -e "s|^%global snapinfo .*|%global snapinfo ${snapinfo}|" \
    -e "s|^%global commit   .*|%global commit   ${commit}|" \
    -e "s|^%global describe .*|%global describe ${describe#v}|" \
    -e "s|^Version: *.*|Version:        ${version}|" \
    -e "s|^Release: *.*|Release:        ${release}|" \
    "$here/cordial.spec" > "$spec"

echo "==> rpmbuild -bs"
rpmbuild -bs "$spec" \
    --define "_sourcedir $srcdir" \
    ${outdir:+--define "_srcrpmdir $outdir"}
