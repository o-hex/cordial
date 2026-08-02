# Development shortcuts.
#
# `just dev` starts the shell, which is what a user starts — the shell is the
# thing that finds a Roblox build, explains itself when there is not one, and
# launches the client. `just client` skips it and runs the engine directly,
# which is what a debugging session wants and nobody else does.

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

# Start Cordial: just dev [--apk /path/to/base.apk]
dev *args:
    #!/usr/bin/env bash
    set -euo pipefail
    # A shebang recipe gets its arguments substituted into the script text rather
    # than handed to it as $@, so they have to be planted before anything can
    # parse them. Without this every invocation behaves as though it were given
    # no arguments, which looks like a broken recipe rather than a missing
    # `set --`.
    set -- {{ args }}
    apk="" extra=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --apk)   apk="${2:-}"; shift 2 ;;
            --apk=*) apk="${1#*=}"; shift ;;
            *)       extra+=("$1"); shift ;;
        esac
    done
    cargo build --release
    # Deliberately no check that the APK exists and no advice about how to get
    # one. Finding a build, and explaining what to do when there is not one, is
    # the shell's job — it is what the user actually sees, and a second copy of
    # that message here would drift out of step with it.
    if [ -n "$apk" ]; then
        export CORDIAL_APK="$apk"
    fi
    exec ./target/release/cordial-shell ${extra+"${extra[@]}"}

# Run the engine directly, skipping the shell: just client [--apk PATH] [--run SECS]
client *args:
    #!/usr/bin/env bash
    set -euo pipefail
    set -- {{ args }}
    apk="" lib="" run="600" extra=()
    # `--apk` and `--lib-dir` are spelled the way cordial-load spells them: this
    # recipe exists to stop people assembling that command by hand, not to teach
    # a second vocabulary for it. Anything unrecognised passes straight through,
    # so --dump-classes and friends still work.
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
    # Cordial ships no Roblox build. Sober downloads the same official Android
    # one this runtime loads, so checking there first means a debugging run needs
    # no arguments. The shell says all this properly to a user who has neither.
    sober_apk="$HOME/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk"
    if [ -z "$apk" ] && [ -f "$sober_apk" ]; then
        apk="$sober_apk"
        echo "using the build Sober downloaded: $apk"
    fi
    if [ -z "$apk" ] || [ ! -f "$apk" ]; then
        [ -n "$apk" ] && echo "no APK at $apk" >&2
        echo "usage: just client --apk /path/to/base.apk    (or run \`just dev\`, which explains how to get one)" >&2
        exit 1
    fi
    [ -n "$lib" ] || lib="$(dirname "$apk")/lib/x86_64"
    if [ ! -f "$lib/libroblox.so" ]; then
        # Nothing unpacked, so unpack it. On a split build the engine is in
        # split_config.<abi>.apk rather than base.apk, so try the APK given and
        # then its siblings instead of asserting which one holds it.
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

# `just client` with text-entry tracing, which is the one wanted most often
client-text *args:
    @CORDIAL_TRACE_TEXT=1 just client {{ args }}

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
