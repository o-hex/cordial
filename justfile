# Development shortcuts.
#
# `just dev` starts the shell, which is what a user starts — the shell is the
# thing that finds a Roblox build, explains itself when there is not one, and
# launches the client. `just client` skips it and runs the engine directly,
# which is what a debugging session wants and nobody else does.

_default:
    @just --list

# Build: just build [host|distrobox|nix]  (Clang required; bionic will not build with GCC)
build where="host":
    #!/usr/bin/env bash
    set -euo pipefail
    # Each environment gets its own CARGO_TARGET_DIR. They have different
    # toolchains and different libraries visible, so sharing target/ between
    # them means every switch is a full rebuild at best, and at worst a binary
    # linked against headers from one and libraries from the other.
    #
    # A container-built binary still runs on the host: distrobox shares the
    # filesystem, and the libraries this needs are present on the host already —
    # webkitgtk in particular is installed there as a runtime library, and only
    # its *headers* were missing, which is why building outside the host works
    # and running outside it is unnecessary.
    case "{{ where }}" in
      host)
        cargo build --release
        ;;
      distrobox)
        # The optional dependencies are the point of this one. native/CMakeLists.txt
        # silently omits the audio backend when libpipewire-0.3 is absent, and the
        # web views want webkitgtk6.0-devel, both of which mean `dnf install` and a
        # reboot on an immutable host. A container is neither.
        box="${CORDIAL_DISTROBOX:-my-distrobox}"
        # Not `grep -q`. It exits at the first match and closes the pipe, which
        # SIGPIPEs `distrobox list`, and `set -o pipefail` then reports the whole
        # pipeline as failed — so the check inverted and claimed a container that
        # plainly exists did not. Plain grep reads all of its input.
        if ! distrobox list 2>/dev/null | grep "| $box " >/dev/null; then
            echo "no distrobox called '$box' (set CORDIAL_DISTROBOX to pick another)" >&2
            distrobox list 2>/dev/null >&2 || true
            exit 1
        fi
        distrobox enter "$box" -- bash -lc \
          'CARGO_TARGET_DIR=target-distrobox cargo build --release'
        echo "built into target-distrobox/release"
        ;;
      nix)
        # Unverified on an ostree host, where /nix is part of the read-only image
        # and nix-daemon is disabled. See CONTRIBUTING.md.
        nix develop --command bash -c \
          'CARGO_TARGET_DIR=target-nix cargo build --release'
        echo "built into target-nix/release"
        ;;
      *)
        echo "usage: just build [host|distrobox|nix]" >&2
        exit 1
        ;;
    esac

# Run the workspace tests
test:
    cargo test --workspace

# Build and test, the pre-pull-request gate
check: build test

# Start Cordial: just dev [--in host|distrobox|nix] [--apk /path/to/base.apk]
dev *args:
    #!/usr/bin/env bash
    set -euo pipefail
    # A shebang recipe gets its arguments substituted into the script text rather
    # than handed to it as $@, so they have to be planted before anything can
    # parse them. Without this every invocation behaves as though it were given
    # no arguments, which looks like a broken recipe rather than a missing
    # `set --`.
    set -- {{ args }}
    apk="" env=host extra=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --apk)   apk="${2:-}"; shift 2 ;;
            --apk=*) apk="${1#*=}"; shift ;;
            # `--in` rather than a positional, because a positional would swallow
            # `--apk` as its value — which it did, the first time this was written.
            --in)    env="${2:-}"; shift 2 ;;
            --in=*)  env="${1#*=}"; shift ;;
            *)       extra+=("$1"); shift ;;
        esac
    done
    just build "$env"
    # Each build environment has its own target directory, so the runner has to
    # be told which one it is looking at. Assuming ./target meant `just build
    # distrobox` followed by `just dev` silently ran the older HOST binary — a
    # stale build that still executes is this project's most expensive kind of
    # mistake, and it has already cost two measurements of code that was never
    # under test.
    case "$env" in
      host)      bindir=target/release ;;
      distrobox) bindir=target-distrobox/release ;;
      nix)       bindir=target-nix/release ;;
      *)         echo "usage: just dev [--in host|distrobox|nix] [--apk PATH]" >&2; exit 1 ;;
    esac
    if [ ! -x "$bindir/cordial-shell" ]; then
        echo "no cordial-shell in $bindir — did 'just build $env' succeed?" >&2
        exit 1
    fi
    # Deliberately no check that the APK exists and no advice about how to get
    # one. Finding a build, and explaining what to do when there is not one, is
    # the shell's job — it is what the user actually sees, and a second copy of
    # that message here would drift out of step with it.
    if [ -n "$apk" ]; then
        export CORDIAL_APK="$apk"
    fi
    exec "./$bindir/cordial-shell" ${extra+"${extra[@]}"}

# Run the engine directly: just client [--in host|distrobox|nix] [--apk PATH] [--run SECS]
client *args:
    #!/usr/bin/env bash
    set -euo pipefail
    set -- {{ args }}
    apk="" lib="" run="600" env=host x11=0 extra=()
    # `--apk` and `--lib-dir` are spelled the way cordial-run spells them: this
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
            --in)        env="${2:-}"; shift 2 ;;
            --in=*)      env="${1#*=}"; shift ;;
            --x11)       x11=1; shift ;;
            *)           extra+=("$1"); shift ;;
        esac
    done
    # Wayland unless asked otherwise, because ADR-011 makes it the target and the
    # shell has always launched with it. This recipe did not, so `just dev` and
    # `just client` ran DIFFERENT display backends, and every fix verified through
    # one was being tested through the other. X11 is still reachable with --x11
    # until ADR-011's removal trigger fires.
    if [ "${x11:-0}" != 1 ]; then
        export CORDIAL_WAYLAND=1
    fi
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
        # Re-extract when the APK it came from has changed. Presence alone was
        # the test until now, so installing a new Roblox build left the OLD
        # engine cached and Cordial ran it against the new APK's assets --
        # a silent version mismatch, which is worse than the cold start the
        # cache exists to avoid. mtime on the extracted file cannot be used: zip
        # preserves the stored timestamp, so it reads 1981.
        stamp="$cache/.from"
        want="$(stat -c '%s %Y %n' "$apk" 2>/dev/null)"
        if [ -f "$cache/libroblox.so" ] && [ "$(cat "$stamp" 2>/dev/null)" != "$want" ]; then
            echo "the APK changed since libroblox.so was extracted; re-extracting"
            rm -f "$cache/libroblox.so"
        fi
        if [ ! -f "$cache/libroblox.so" ]; then
            mkdir -p "$cache"
            for candidate in "$apk" "$(dirname "$apk")"/split_config*.apk; do
                [ -f "$candidate" ] || continue
                if unzip -o -j -q "$candidate" 'lib/x86_64/libroblox.so' -d "$cache" 2>/dev/null \
                   && [ -f "$cache/libroblox.so" ]; then
                    printf '%s' "$want" > "$stamp"
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
    just build "$env"
    # Whichever environment built it, run that one. Defaulting to ./target here
    # would silently run a stale host binary after a container build.
    case "$env" in
      host)      bindir=target/release ;;
      distrobox) bindir=target-distrobox/release ;;
      nix)       bindir=target-nix/release ;;
      *)         echo "usage: just client [--in host|distrobox|nix] ..." >&2; exit 1 ;;
    esac
    if [ ! -x "$bindir/cordial-run" ]; then
        echo "no cordial-run in $bindir — did 'just build $env' succeed?" >&2
        exit 1
    fi
    # A long default timer on purpose: a run that ends on its own while somebody
    # is still reading the screen looks exactly like a crash, and that has
    # already cost one debugging session here.
    exec "./$bindir/cordial-run" \
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
