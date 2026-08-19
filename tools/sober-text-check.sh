#!/usr/bin/env bash
# Does Sober patch the engine's code, or run it honestly?
#
# This is the last open question in the RbxStorage investigation and it decides
# whether that investigation's conclusion is right.
#
# docs/analysis/flag-init.md §30 concludes the value RbxStorage fails on is
# engine-internal and unreachable through any interface Roblox exposes to a host
# application — twenty-one candidates eliminated with controls, the last one
# instrumented rather than inferred. If that is true, the only route left is the
# memory write ADR-001 makes deliberately absent, and that is a decision about
# Cordial's security posture rather than an engineering problem.
#
# Sober reaches `RbxStorage::init [INIT] user: flagLoaded` on this machine. It is
# the only existence proof that the state is reachable by *something*. So:
#
#   0 differing bytes  -> Sober runs the engine honestly. A legitimate route
#                         exists and §30 is wrong. Worth a great deal.
#   anything above 0   -> Sober patches like mocktail, and nobody has solved this
#                         without writing to the engine. §30 stands, and the
#                         decision is ADR-001's, not engineering's.
#
# **Zero is not proof of innocence.** A data byte can be forced without touching
# text — mocktail's own flags-loaded patch does exactly that. What zero rules out
# is code patching, which is the 116 `PatchCode` sites mocktail has beside it.
#
# ## Why it needs root
#
# Sober is a Flatpak. Its engine runs inside the sandbox's PID namespace, so no
# host /proc/<pid>/maps contains the mapping at all, and `flatpak enter` needs
# CAP_SYS_ADMIN. `nsenter -p -m` crosses it. Cordial reads clean as the control,
# and you can check that first without any privileges:
#
#     tools/engine-text-diff.py            # against a running cordial-run
#
# ## Usage
#
#     tools/sober-text-check.sh            # launches Sober if it is not running
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIFF="$HERE/engine-text-diff.py"
[ -x "$DIFF" ] || { echo "missing $DIFF" >&2; exit 1; }

pid_of_sober() { pgrep -f 'bwrap.*sober' 2>/dev/null | head -1; }

PID="$(pid_of_sober)"
if [ -z "$PID" ]; then
    echo "Sober is not running; starting it. Wait for the landing page, then"
    echo "come back to this terminal and press Enter."
    flatpak run org.vinegarhq.Sober >/dev/null 2>&1 &
    # The engine takes a few seconds to map; a prompt is more honest than a
    # fixed sleep that is wrong on a slow disk.
    read -r -p "Press Enter once Sober has reached its landing page... " _
    PID="$(pid_of_sober)"
fi

[ -n "$PID" ] || { echo "could not find Sober's bwrap process" >&2; exit 1; }
echo "Sober bwrap pid $PID — entering its namespaces (this is the sudo prompt)"

# `--mount-proc` is an `unshare` option, not an `nsenter` one; `-m` already
# brings the sandbox's /proc along with its mount namespace.
#
# The script is fed on stdin rather than by path: inside Sober's mount namespace
# this repository does not exist, so a path argument would resolve to nothing.
# The redirect is performed by this shell, on the host, before nsenter runs.
sudo nsenter -t "$PID" -p -m /usr/bin/python3 - < "$DIFF"
