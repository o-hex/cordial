#!/usr/bin/env bash
# Build the two persistent virtual input devices the text-entry tests drive.
#
# They are built rather than vendored because they need `wayland-scanner` and
# the protocol XML, and both live in the container alongside sway -- the host is
# immutable ostree and has neither. Output goes to a build directory rather than
# into the tree, because these are test instruments and not part of the client.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${CORDIAL_HOLDER_BIN:-/tmp/cordial-wl-holders}"
mkdir -p "$OUT"

VK_XML=$(ls /usr/share/cargo/registry/wayland-protocols-misc-*/protocols/virtual-keyboard-unstable-v1.xml 2>/dev/null | tail -1)
VP_XML=/usr/share/wlr-protocols/unstable/wlr-virtual-pointer-unstable-v1.xml

[ -n "$VK_XML" ] || { echo "no virtual-keyboard XML: dnf install rust-wayland-protocols-misc-devel" >&2; exit 1; }
[ -f "$VP_XML" ] || { echo "no wlr-virtual-pointer XML: dnf install wlr-protocols-devel" >&2; exit 1; }

gen() { # xml stem
  wayland-scanner client-header "$1" "$OUT/$2-client-protocol.h"
  wayland-scanner private-code  "$1" "$OUT/$2-protocol.c"
}
gen "$VK_XML" virtual-keyboard-unstable-v1
gen "$VP_XML" wlr-virtual-pointer-unstable-v1

cc -O2 -Wall -Wextra -I"$OUT" -o "$OUT/wl-keyboard-holder" \
   "$ROOT/tools/wl-keyboard-holder.c" "$OUT/virtual-keyboard-unstable-v1-protocol.c" \
   $(pkg-config --cflags --libs wayland-client xkbcommon)
cc -O2 -Wall -Wextra -I"$OUT" -o "$OUT/wl-pointer-holder" \
   "$ROOT/tools/wl-pointer-holder.c" "$OUT/wlr-virtual-pointer-unstable-v1-protocol.c" \
   $(pkg-config --cflags --libs wayland-client)

echo "built: $OUT/wl-keyboard-holder $OUT/wl-pointer-holder"
