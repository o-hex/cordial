# Development shortcuts.
#
# These exist because the working command line for a real run is long enough
# that people were retyping it wrong — the wrong --lib-dir in particular fails
# late and looks like an engine bug rather than a typo. `just dev` takes the one
# path that actually varies and derives the rest.

_default:
    @just --list

# Build everything, release (Clang required; AOSP bionic will not build with GCC)
build:
    cargo build --release

# Run the workspace tests
test:
    cargo test --workspace

# Build and test, the pre-pull-request gate
check: build test

# Run the client: just dev --apk /path/to/base.apk [--lib-dir DIR] [--run SECS]
dev *args:
    #!/usr/bin/env bash
    set -euo pipefail
    # A shebang recipe gets its arguments substituted into the script text, not
    # handed to it as $@, so they have to be planted before anything can parse
    # them. Without this the loop below sees an empty argument list and every
    # invocation prints the usage line, which looks like a broken recipe rather
    # than a missing `set --`.
    set -- {{ args }}
    apk="" lib="" run="600" extra=()
    # `--apk` is spelled the same way cordial-load spells it, deliberately: the
    # point of this recipe is to stop people assembling that command by hand, not
    # to teach them a second vocabulary for the same arguments. Anything not
    # recognised is passed straight through, so --dump-classes and friends work.
    while [ $# -gt 0 ]; do
        case "$1" in
            --apk)       apk="${2:-}"; shift 2 ;;
            --apk=*)     apk="${1#*=}"; shift ;;
            --lib-dir)   lib="${2:-}"; shift 2 ;;
            --lib-dir=*) lib="${1#*=}"; shift ;;
            --run)       run="${2:-}"; shift 2 ;;
            --run=*)     run="${1#*=}"; shift ;;
            *)           extra+=("$1"); shift ;;
        esac
    done
    # Cordial ships no Roblox build and never will, so an APK has to come from
    # somewhere the contributor already has. The least painful route by a wide
    # margin is to let Sober download one: it fetches the same official Android
    # build this runtime loads, and leaves it unpacked where anyone can point at
    # it. Checking there first means the common case needs no arguments at all.
    sober_apk="$HOME/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk"
    if [ -z "$apk" ] && [ -f "$sober_apk" ]; then
        apk="$sober_apk"
        echo "using the build Sober downloaded: $apk"
    fi
    if [ -z "$apk" ] || [ ! -f "$apk" ]; then
        # Every line here is indented because an unindented line ends a `just`
        # recipe, and a heredoc body at column 0 is parsed as justfile syntax
        # rather than as text.
        [ -n "$apk" ] && echo "no APK at $apk" >&2
        {
            echo "usage: just dev --apk /path/to/base.apk [--lib-dir DIR] [--run SECS]"
            echo
            echo "Cordial ships no Roblox build. The easiest way to get one is to install"
            echo "Sober and let it install Roblox for you:"
            echo
            echo "    flatpak install flathub org.vinegarhq.Sober"
            echo "    flatpak run org.vinegarhq.Sober    # let it finish downloading, then quit"
            echo
            echo "\`just dev\` then finds it on its own, under"
            echo "~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/"
            echo
            echo "Any APK of the official Android x86-64 build works; that is simply the"
            echo "least fiddly way to obtain one."
        } >&2
        exit 1
    fi
    # Default to the libraries beside the APK, because that is where they land if
    # you unzip it in place.
    [ -n "$lib" ] || lib="$(dirname "$apk")/lib/x86_64"
    if [ ! -f "$lib/libroblox.so" ]; then
        # Nothing unpacked, so unpack it. The engine is inside the APK, and on a
        # split build it is in split_config.<abi>.apk rather than base.apk — so
        # try the APK given and then its siblings, rather than asserting which
        # one holds it. Extracting once into the cache beats making every
        # contributor discover this by hand.
        cache="${XDG_CACHE_HOME:-$HOME/.cache}/cordial/lib/x86_64"
        if [ ! -f "$cache/libroblox.so" ]; then
            mkdir -p "$cache"
            for candidate in "$apk" "$(dirname "$apk")"/split_config*.apk; do
                [ -f "$candidate" ] || continue
                if unzip -o -j -q "$candidate" 'lib/x86_64/libroblox.so' -d "$cache" 2>/dev/null \
                   && [ -f "$cache/libroblox.so" ]; then
                    echo "extracted libroblox.so from $(basename "$candidate") into $cache"
                    break
                fi
            done
        fi
        lib="$cache"
    fi
    if [ ! -f "$lib/libroblox.so" ]; then
        echo "no libroblox.so in $apk, its split_config siblings, or $lib" >&2
        echo "pass --lib-dir if the engine lives somewhere else" >&2
        exit 1
    fi
    cargo build --release
    # A long default timer on purpose: a run that ends on its own while somebody
    # is still reading the screen looks exactly like a crash, and that has
    # already cost one debugging session here.
    exec ./target/release/cordial-load \
        --lib-dir "$lib" --apk "$apk" \
        --host-libc --game-activity --run "$run" ${extra+"${extra[@]}"}

# `just dev` with text-entry tracing, which is the one wanted most often
dev-text *args:
    @CORDIAL_TRACE_TEXT=1 just dev {{ args }}

# The launcher shell on its own
shell:
    cargo run --release --bin cordial-shell

# What a new APK needs that the stub table lacks: just symbols --lib-dir DIR
symbols lib_dir:
    #!/usr/bin/env bash
    set -euo pipefail
    # AGENTS.md explains why one missing *data* symbol stops the whole client at
    # load time rather than at first use.
    new=$(mktemp) old=$(mktemp)
    trap 'rm -f "$new" "$old"' EXIT
    readelf --dyn-syms -W {{ quote(lib_dir) }}/libroblox.so \
      | awk '$7=="UND" {print $8}' | sed 's/@.*//' | sort -u > "$new"
    cut -f2 docs/analysis/undefined-symbols.tsv | sort -u > "$old"
    comm -23 "$new" "$old"
