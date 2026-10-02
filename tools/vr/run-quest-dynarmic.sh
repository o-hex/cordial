#!/usr/bin/env bash
# usage: run-quest-dynarmic.sh <logname> [run-seconds] [extra cordial-run args...]
#
# The x86-64 client running the Quest build's arm64 engine under dynarmic
# (docs/vr/dynarmic-design.md, M4), with the same data root, profile and log
# shape as run-quest.sh's qemu runs so the two can be compared line for line.
# The engine's own FLog goes under the data root, not to stdout; it is copied
# next to the run's log as <logname>.flog.
#
# OpenXR (M6) reaches whatever runtime the host loader selects, so pass the
# runtime in the environment, never through ~/.config/openxr. For Monado's
# simulated HMD (design §9.5), with Monado installed under <prefix>:
#   XR_RUNTIME_JSON=<prefix>/share/openxr/1/openxr_monado.json
#   SIMULATED_ENABLE=1 XRT_COMPOSITOR_COMPUTE=0 XRT_COMPOSITOR_DEFAULT_FRAMERATE=90
# and CORDIAL_XR_FRAME_LOG=<file> for one line per xrEndFrame.
# CORDIAL_XR_INPUT_LOG=1 logs, in the run log on one clock, every action
# state, space-location flag and event that changes, each haptic call, and a
# one-second summary (guest_xr_input.rs).
#
# The frame log's format, two decimal integers a line, one line per
# xrEndFrame the engine calls: the monotonic time of that call in
# nanoseconds since the *first* xrEndFrame (so line 1 is about 0; it is not
# the run's clock -- find "xrEndFrame #1" in the run log for that), and the
# predictedDisplayPeriod the last xrWaitFrame returned, in nanoseconds
# (13888889 is 72 Hz, 11111111 is 90 Hz). Frame intervals are differences of
# the first column; the rate is lines per second of it. Lines are buffered and
# flushed every 32 frames, so a run that aborts loses up to 31.
#
# BIN=<path> runs another cordial-run build (a control's) with everything
# else the same.
#
# S is a working directory holding quest/quest-roblox.apk (the APK pulled
# from your own headset), quest/ex/ (that APK unzipped, so quest/ex/lib/
# arm64-v8a exists) and runs/, where the logs go. DATA is the throwaway data
# root the client runs in, $S/data unless set; never point it at the data
# root you play with (AGENTS.md, "Give your runs their own data root").
S=${S:?set S to a directory holding quest/ (the pulled APK and extracted libs) and runs/}
Q=$S/quest
DATA=${DATA:-$S/data}
name=$1; secs=${2:-90}; shift 2 || shift $#
cd "$(dirname "$0")/../.."
mkdir -p "$S/runs"
export XDG_DATA_HOME=$DATA XDG_CACHE_HOME=$DATA/cache
export CORDIAL_DEV_CONTROL=1
start=$(date +%s.%N)
timeout -s TERM -k 20 $((secs + 120)) ${BIN:-target/x86_64-unknown-linux-gnu/release/cordial-run} \
  --lib-dir $Q/ex/lib/arm64-v8a --apk $Q/quest-roblox.apk --profile vrprobe --host-libc \
  --guest-arm64 --app-bridge --run $secs "$@" \
  2>&1 | perl -MTime::HiRes=time -ne 'BEGIN{$|=1;$t0=time} printf("[%7.2f] %s", time-$t0, $_)' > $S/runs/$name.log
rc=${PIPESTATUS[0]}
end=$(date +%s.%N)
echo "exit=$rc elapsed=$(echo "$end - $start" | bc)s" | tee -a $S/runs/$name.log
flog=$(ls -t $XDG_DATA_HOME/cordial/profiles/vrprobe/quest/data/files/appData/logs/*_last.log 2>/dev/null | head -1)
[ -n "$flog" ] && cp "$flog" $S/runs/$name.flog
