#!/usr/bin/env bash
# One instrumented join, then a summary of what the engine said about it.
#
# Usage: tools/join-run.sh <tag> [bindir] [run-seconds] [extra cordial-run args...]
#
# Runs the client the way cordial-shell runs it — same `--profile`, so the same
# appData, cookie store and saved identity — but with `--join-url`, which is the
# only path into a game that does not need somebody to press a button.
#
# This lived in a scratch directory for most of its life and was lost twice when
# that directory was cleaned, taking the one repeatable measurement of the 60
# second disconnect with it. It is in the repository now for that reason.
#
# AGENTS.md applies to anything measured with this:
#
#   * **Use the test account.** Enforcement is automated and associates accounts
#     sharing an address. `--profile CordialTest` is not a default to change
#     casually.
#   * **One run is not a result.** The disconnect this exists to study reproduced
#     on roughly one launch in three before it was understood; a single clean run
#     proves nothing.
#   * **Change one thing at a time.** Two changes against a failure that takes
#     sixty seconds to reproduce tell you nothing about either.
set -uo pipefail

TAG="${1:?usage: tools/join-run.sh <tag> [bindir] [seconds] [extra args...]}"
BIN="${2:-./target/release}"
SECS="${3:-90}"
shift $(( $# < 3 ? $# : 3 ))

APK="$HOME/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk"
LIB="${XDG_CACHE_HOME:-$HOME/.cache}/cordial/lib/x86_64"
PLACE="${CORDIAL_TEST_PLACE:-17625359962}"
PROFILE="${CORDIAL_TEST_PROFILE:-CordialTest}"
LOGS="$HOME/.local/share/cordial/profiles/$PROFILE/data/files/appData/logs"
OUT="${TMPDIR:-/tmp}/cordial-join-$TAG.log"

if [ ! -f "$APK" ]; then
    echo "no APK at $APK — Cordial ships none; Sober downloads the same build" >&2
    exit 1
fi
if [ ! -x "$BIN/cordial-run" ]; then
    echo "no cordial-run in $BIN — did 'just build' succeed?" >&2
    exit 1
fi

echo "=== $TAG: $(date -u +%H:%M:%SZ) place=$PLACE profile=$PROFILE bin=$BIN run=${SECS}s"
# Anything already in the environment is passed through untouched, which is how
# a one-variable experiment is run against this:
#   CORDIAL_DEVICE_PROFILE=pc-windows-11 tools/join-run.sh pc
CORDIAL_WAYLAND=1 "$BIN/cordial-run" \
    --lib-dir "$LIB" --apk "$APK" \
    --host-libc --game-activity \
    --run "$SECS" --profile "$PROFILE" \
    --join-url "${CORDIAL_TEST_LINK:-roblox://experiences/start?placeId=$PLACE}" \
    "$@" > "$OUT" 2>&1
echo "exit=$? out=$OUT ($(wc -l < "$OUT") lines)"

F=$(ls -t "$LOGS"/*Player*.log 2>/dev/null | head -1)
[ -z "$F" ] && { echo "no Player log under $LOGS"; exit 1; }
echo "--- engine log: $(basename "$F")"

acc=$(grep -m1 "Connection accepted from" "$F")
if [ -z "$acc" ]; then
    echo "NEVER CONNECTED — the join did not happen"
    grep -iE "join|placelauncher|auth|ticket|deeplink" "$OUT" | tail -15
    exit 2
fi
ip=$(echo "$acc" | grep -oE '[0-9.]+\|' | tr -d '|')
code=$(grep -m1 -oE "Disconnect reason received: [0-9]+" "$F" | awk '{print $NF}')

# Take the lifetime from the engine's own "Connection lost" line rather than
# differencing two greps. A session that teleports has several connections, and
# pairing the FIRST "Connection accepted" with the LAST disconnect once reported
# 77s for a connection that actually lived 60.8s — the rule looked broken when
# only the arithmetic was. timeMS and connectionTime are on the one line and both
# belong to the same connection.
lost=$(grep -m1 "connectMode: Peer Disconnected" "$F")
if [ -n "$lost" ]; then
    t=$(echo "$lost" | grep -oE "timeMS:[0-9]+" | cut -d: -f2)
    ct=$(echo "$lost" | grep -oE "connectionTime [0-9]+" | awk '{print $2}')
    alive=$(awk -v a="$t" -v b="$ct" 'BEGIN{printf "%.1f", (a-b)/1000}')
else
    alive="still connected at exit"; code="${code:-none}"
fi

echo "RESULT $TAG server=$ip alive=${alive}s reason=${code:-none} (connections: $(grep -c 'Connection accepted from' "$F"))"
# The chain flag-init.md §13 is about: these three appear together in a client
# that raises the flags-loaded event and in none that does not.
printf '  RbxStorage::init=%s ClientRunInfo=%s onFlagsFailed=%s webview-open=%s\n' \
    "$(grep -c 'RbxStorage::init' "$F")" \
    "$(grep -c 'ClientRunInfo' "$F")" \
    "$(grep -c 'onFlagsFailed' "$OUT")" \
    "$(grep -c 'openWindow message arrived' "$OUT")"
