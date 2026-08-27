#!/usr/bin/env bash
# Derive a package version from `git describe`, and print it as shell
# assignments -- the same source AGENTS.md names for the window title itself:
# `git describe --tags --always --dirty`, stamped by
# crates/cordial-shell/build.rs. A package numbered from anywhere else
# disagrees with the string the running client prints, which is exactly the
# confusion "Say which build you are talking about" exists to prevent.
#
# Printed rather than exported, so this is meant to be evaluated rather than
# sourced or run for effect:
#
#     eval "$(packaging/version.sh)"
#     echo "$CORDIAL_VERSION"
#
# That is a deliberate choice over `source`-ing it: a sourced script that
# forgets to `export` leaves the caller with nothing and no error, and one
# that changes `set -e` underneath a caller's own shell options is a footgun
# for exactly the reason this project's own justfile avoids it. `eval` fails
# loudly if this script fails, because a broken command substitution with
# `set -e` upstream stops the pipeline.
#
# packaging/rpm/make-srpm.sh carries its own copy of this derivation rather
# than calling this script. That script already works and is exercised by
# Copr builds; reworking it to share this file was judged not worth the risk
# to something that already ships. If you change the transform here, check
# whether make-srpm.sh's needs the same fix -- they are two implementations of
# one idea and can drift.
#
# Sets:
#   CORDIAL_DESCRIBE   the raw `git describe --tags --long` string with the
#                      leading v stripped, e.g. 0.7.0-37-gcbd53e5 -- what
#                      CORDIAL_BUILD_VERSION wants, so the packaged binary's
#                      window title agrees with the package that shipped it
#   CORDIAL_VERSION    the tag alone, e.g. 0.7.0
#   CORDIAL_COMMITS    commits since that tag; 0 at an exact tag
#   CORDIAL_SHORTHASH  the abbreviated commit, e.g. cbd53e5
set -euo pipefail

describe=$(git describe --tags --long --abbrev=7)
case "$describe" in
    v*) ;;
    *)
        echo "expected a v-prefixed tag reachable from HEAD, got '$describe' -- fetch tags (git fetch --tags) or check out a release" >&2
        exit 1
        ;;
esac

version=${describe#v}; version=${version%%-*}
rest=${describe#v${version}-}
commits=${rest%%-*}
shorthash=${rest#*-g}

printf 'CORDIAL_DESCRIBE=%s\n' "${describe#v}"
printf 'CORDIAL_VERSION=%s\n' "$version"
printf 'CORDIAL_COMMITS=%s\n' "$commits"
printf 'CORDIAL_SHORTHASH=%s\n' "$shorthash"
