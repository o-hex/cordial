#!/usr/bin/env bash
# One launch of the client, with no input at all, measured. Prints one TSV row.
#
# This exists because the startup freeze is a race and every conclusion drawn
# about it from a handful of launches has been wrong. Measured on 2026-08-25
# over twenty-five launches of one signed-in profile: **eight frozen, seventeen
# healthy**. At a 32% base rate, three runs agreeing happens about one time in
# thirty, and this bug has twice been "reliably reproduced" and twice been
# something else. Do not believe a change to it on fewer than about twenty runs
# in each arm.
#
#   tools/startup-freeze-survey.sh <run-number> [profile]
#
# Columns:
#
#   run         the number you passed, so rows collate
#   ready       the last `app ready:` screen the shell reached
#   presents10  vkQueuePresentKHR count ten seconds after that
#   presents25  the same fifteen seconds later
#   cookies     how many domains the saved session restored
#   verdict     FROZEN when the count stopped climbing and stayed under ten
#   stage       the last `StartupController started: stage` the ENGINE logged
#
# **No input is driven, deliberately.** The freeze is reported as happening when
# nobody touches the client while it loads, so touching it measures something
# else -- and a run that was nudged is not comparable with one that was not.
#
# `presents=1` is the usual frozen reading but not the only one: the survey saw
# 0, 1 and 5. Read the count, not the verdict, when a row looks odd.
#
# Set `CORDIAL_POLL_COALESCE_US=0` to run the looper's idle backoff off as a
# control, and `TAG=<word>` to keep one arm's logs from overwriting another's.
#
# `NUDGE=1` drives real pointer motion through the nested compositor from the
# moment the client starts until the shell is ready. That is the one arm that
# is *not* a clean measurement of the bug -- it is a measurement of the user's
# own report, which is that giving the client input while it loads stops it
# freezing. It has never been tested. It is not a fix and must not become one:
# see AGENTS.md on synthesising input, and note this only ever happens inside
# the harness's own compositor.
set -uo pipefail
RUN="${1:-0}"
PROFILE="${2:-default}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${CORDIAL_FREEZE_OUT:-/tmp/cordial-freeze}"
mkdir -p "$OUT"
APK="${CORDIAL_APK:-$HOME/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk}"
LIB="${CORDIAL_LIB_DIR:-$HOME/.cache/cordial/lib/x86_64}"
LOG="$OUT/${TAG:-run}$RUN.log"
SOCK="$HOME/.local/share/cordial/profiles/$PROFILE/devctl.sock"

# Never `pkill -f`: the pattern matches the shell running it, and that has
# killed the session issuing it five times in one day. `pidof` matches the
# executable, which the engine's rename of its main thread does not touch.
for p in $(pidof cordial-run 2>/dev/null); do kill -9 "$p" 2>/dev/null; done
distrobox enter cordial -- bash -lc 'for p in $(pidof sway); do kill -9 $p 2>/dev/null; done' >/dev/null 2>&1
sleep 3

# Windowed every time. Cordial remembers window state per profile, and a run
# that inherited fullscreen from the previous one is a different experiment --
# that cost two runs before anyone noticed the file.
printf '{\n  "fullscreen": false,\n  "maximised": false,\n  "width": null,\n  "height": null\n}\n' \
  > "$HOME/.local/share/cordial/profiles/$PROFILE/game-window.json"

# sway rather than cage: cage's seat never gains a keyboard, which changes which
# of Cordial's code runs. Headless, on its own display, so nothing is injected
# anywhere near the developer's session.
DISP=$(python3 - <<'PY'
import subprocess, os, time
stamp, cfg = "/tmp/cordial-freeze-display", "/tmp/cordial-freeze-sway.cfg"
for f in (stamp, cfg):
    if os.path.exists(f):
        os.unlink(f)
open(cfg, "w").write(
    "xwayland disable\noutput HEADLESS-1 mode 1280x800\ndefault_border none\n"
    "focus_follows_mouse no\n"
    "exec sh -c 'printf %%s \"$WAYLAND_DISPLAY\" > %s'\n" % stamp)
subprocess.Popen(["distrobox", "enter", "cordial", "--", "bash", "-lc",
  "exec env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 "
  "WLR_HEADLESS_OUTPUTS=1 sway -c %s > /tmp/cordial-freeze-sway.log 2>&1" % cfg],
  stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(120):
    if os.path.exists(stamp) and os.path.getsize(stamp):
        print(open(stamp).read().strip())
        break
    time.sleep(0.2)
PY
)
[ -n "$DISP" ] || { printf '%s\tNO-COMPOSITOR\t-\t-\t-\t-\t-\n' "$RUN"; exit 1; }

# GDK_BACKEND=wayland explicitly: the container exports x11, and a GTK window
# that lands on the host's X server is a window nobody in the harness can see.
# **`env`, not a bare assignment prefix.** `${VAR:+VAR=value}` looks like one
# and is not: bash decides what is an assignment before it expands, so the
# expanded word becomes the command name and the run dies with
# `CORDIAL_POLL_COALESCE_US=0: command not found` -- which this script then
# scores as a freeze, because a client that never started never presents. That
# turned a whole control arm into noise before anyone read the log.
env WAYLAND_DISPLAY=$DISP GDK_BACKEND=wayland CORDIAL_DEV_CONTROL=1 \
  ${CORDIAL_POLL_COALESCE_US:+CORDIAL_POLL_COALESCE_US=$CORDIAL_POLL_COALESCE_US} \
  "$ROOT/target/release/cordial-run" --lib-dir "$LIB" --apk "$APK" --host-libc \
  --game-activity --run 0 --profile "$PROFILE" > "$LOG" 2>&1 &
CLIENT=$!

# The nudge arm. A persistent virtual pointer, moving continuously, inside this
# harness's own headless compositor and nowhere near the developer's session.
NUDGE_PID=
if [ "${NUDGE:-0}" = "1" ]; then
  NUDGE_FIFO=$OUT/nudge.fifo
  rm -f "$NUDGE_FIFO"; mkfifo "$NUDGE_FIFO"
  distrobox enter cordial -- bash -lc     "export WAYLAND_DISPLAY=$DISP; exec ${CORDIAL_HOLDER_BIN:-/tmp/cordial-wl-holders}/wl-pointer-holder 1280 800"     < "$NUDGE_FIFO" > /dev/null 2>&1 &
  NUDGE_PID=$!
  exec 8> "$NUDGE_FIFO"
  ( i=0
    while kill -0 $CLIENT 2>/dev/null && [ $i -lt 900 ]; do
      printf 'move %d %d
' $((300 + i % 400)) $((300 + i % 200)) >&8
      i=$((i + 1)); sleep 0.05
    done ) &
  NUDGER=$!
fi

for _ in $(seq 1 100); do
  grep -qE "app ready: (Home|Landing)" "$LOG" && break
  kill -0 $CLIENT 2>/dev/null || break
  sleep 1
done
if [ -n "$NUDGE_PID" ]; then
  kill "${NUDGER:-0}" 2>/dev/null
  exec 8>&-
  kill "$NUDGE_PID" 2>/dev/null
fi
READY=$(grep -oE "app ready: [A-Za-z]+" "$LOG" | tail -1 | cut -d' ' -f3)
[ -n "$READY" ] || READY=NEVER

ask() { timeout 10 python3 -c "
import socket
s = socket.socket(socket.AF_UNIX); s.settimeout(8)
try:
    s.connect('$SOCK'); s.sendall(b'info\n'); print(s.recv(4096).decode().strip())
except Exception as e:
    print('err', e)
" 2>/dev/null | grep -oE "presents=[0-9]+" | cut -d= -f2; }

sleep 10; P10=$(ask); sleep 15; P25=$(ask)
COOKIES=$(grep -oE "\[cookies\] restored [0-9]+ domain" "$LOG" | grep -oE "[0-9]+" | tail -1)
[ -n "$COOKIES" ] || COOKIES=none

# The engine's own narration, which is the instrument that finally separated a
# frozen run from a healthy one. It prunes its own directory, so read it now.
# `profiles/`, not `instances/` -- the latter holds no `_Player_` logs at all.
# And only if it is *this* run's: a run that produced none would otherwise be
# scored from the previous run's log, silently, which is the worst kind of
# wrong number because it looks plausible.
ELOG=$(ls -t "$HOME/.local/share/cordial/profiles/$PROFILE/data/files/appData/logs/"*_Player_*.log 2>/dev/null | head -1)
if [ -n "$ELOG" ] && [ "$ELOG" -nt "$LOG" ]; then
  STAGE=$(grep -oE "StartupController started: stage = [0-9]+" "$ELOG" | tail -1 | grep -oE "[0-9]+$")
  cp "$ELOG" "$OUT/${TAG:-run}$RUN.engine.log" 2>/dev/null
else
  ELOG=
fi
[ -n "${STAGE:-}" ] || STAGE=none

VERDICT=HEALTHY
[ -z "$P10" ] && P10=0
[ -z "$P25" ] && P25=0
[ "$P10" = "$P25" ] && [ "$P25" -lt 10 ] && VERDICT=FROZEN

kill -9 $CLIENT 2>/dev/null
distrobox enter cordial -- bash -lc 'for p in $(pidof sway); do kill -9 $p 2>/dev/null; done' >/dev/null 2>&1
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$RUN" "$READY" "$P10" "$P25" "$COOKIES" "$VERDICT" "$STAGE"
