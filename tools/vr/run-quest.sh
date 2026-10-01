#!/usr/bin/env bash
# usage: run-quest.sh <logname> [run-seconds] [extra cordial-run args...]
#
# The aarch64 cordial-run from tools/vr/build-aarch64.sh, under qemu-user with
# llvmpipe, running the Quest build natively as arm64: the phase-1 route that
# dynarmic replaced, kept for comparisons (docs/vr/dynarmic-design.md). S and
# DATA as in run-quest-dynarmic.sh.
S=${S:?set S to a directory holding quest/ (the pulled APK and extracted libs) and runs/}
Q=$S/quest
DATA=${DATA:-$S/data}
name=$1; secs=${2:-90}; shift 2 || shift $#
cd "$(dirname "$0")/../.."
mkdir -p "$S/runs"
SYS=$PWD/target-aarch64/sysroot-aarch64
export XDG_DATA_HOME=$DATA XDG_CACHE_HOME=$DATA/cache
export CORDIAL_DEV_CONTROL=1
export VK_DRIVER_FILES=$SYS/usr/share/vulkan/icd.d/lvp_icd.json VK_ICD_FILENAMES=$SYS/usr/share/vulkan/icd.d/lvp_icd.json
start=$(date +%s.%N)
timeout -s TERM -k 20 $((secs + 240)) qemu-aarch64 -L $SYS target-aarch64/aarch64-unknown-linux-gnu/release/cordial-run \
  --lib-dir $Q/ex/lib/arm64-v8a --apk $Q/quest-roblox.apk --profile vrprobe --host-libc --run $secs "$@" \
  2>&1 | perl -MTime::HiRes=time -ne 'BEGIN{$|=1;$t0=time} printf("[%7.2f] %s", time-$t0, $_)' > $S/runs/$name.log
rc=${PIPESTATUS[0]}
end=$(date +%s.%N)
echo "exit=$rc elapsed=$(echo "$end - $start" | bc)s" | tee -a $S/runs/$name.log
