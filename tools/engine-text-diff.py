#!/usr/bin/env python3
"""Compare a process's in-memory engine text against the file it was loaded from.

Executable pages of a PIE carry no relocations on x86-64, so a faithful loader
leaves `.text` byte-identical to the file it mapped. A non-zero difference count
is code patching. Zero is not proof of innocence -- a data byte can be forced
without touching text, which is exactly what mocktail's own flags-loaded patch
does -- but it does rule out the 116 `PatchCode` sites that sit beside it there.

Written for one question `docs/analysis/flag-init.md` §22.1 could not answer: is
the engine's flags-loaded state reachable through the interface Roblox exposes to
its host application, or does every implementation that reaches it force it?
Cordial cannot reach it, after fifteen eliminated candidates. mocktail forces it.
Sober reaches it and is the only existence proof that it is reachable at all.

Usage:

    tools/engine-text-diff.py [--pid PID] [--lib /path/to/libroblox.so]

Cordial is its own control: run it against a live `cordial-run` and it must
report zero differing bytes, because Cordial does no patching -- ADR-001 makes
the primitive absent rather than disabled. **A non-zero reading against Cordial
means this script is wrong, not that Cordial is.** Run that control before
believing any reading taken against anything else.

**Sober needs root and a namespace hop**, because it is a Flatpak and its engine
runs inside the sandbox's PID namespace -- no host `/proc/<pid>/maps` contains
the mapping at all, and `flatpak enter` needs `CAP_SYS_ADMIN`:

    flatpak run org.vinegarhq.Sober &     # let it reach the landing page first
    sudo flatpak enter \\
        $(flatpak ps --columns=instance,application | awk '/Sober/{print $1; exit}') \\
        /usr/bin/python3 - --lib /path/inside/sandbox/libroblox.so \\
        < tools/engine-text-diff.py

## Why it matches on size rather than on name

Both Cordial's ported bionic linker and Sober's map the engine anonymously, so
`/proc/<pid>/maps` carries no path to grep for. Matching "any large executable
anonymous mapping" instead picks up JIT arenas -- the first version of this
script confidently selected a 512 MB region belonging to Chrome. So the
executable `PT_LOAD` is read out of the reference ELF and its size is what a
mapping has to match.
"""

import argparse
import glob
import os
import struct
import sys

DEFAULT_LIB = os.path.expanduser("~/.cache/cordial/lib/x86_64/libroblox.so")


def exec_segment(path):
    """The (file offset, memory size) of the ELF's executable PT_LOAD.

    Read by hand rather than with pyelftools, which is not a dependency this
    repository has and is not worth adding for sixteen bytes of header.
    """
    with open(path, "rb") as f:
        data = f.read(64)
        if data[:4] != b"\x7fELF" or data[4] != 2:
            raise SystemExit(f"{path} is not a 64-bit ELF")
        e_phoff, = struct.unpack_from("<Q", data, 0x20)
        e_phentsize, e_phnum = struct.unpack_from("<HH", data, 0x36)
        f.seek(e_phoff)
        table = f.read(e_phentsize * e_phnum)
    for i in range(e_phnum):
        p = table[i * e_phentsize:(i + 1) * e_phentsize]
        p_type, p_flags = struct.unpack_from("<II", p, 0)
        if p_type != 1:  # PT_LOAD
            continue
        if not p_flags & 0x1:  # PF_X
            continue
        p_offset, = struct.unpack_from("<Q", p, 0x08)
        p_filesz, = struct.unpack_from("<Q", p, 0x20)
        return p_offset, p_filesz
    raise SystemExit(f"{path} has no executable PT_LOAD")


def candidates(pid_filter):
    for m in sorted(glob.glob("/proc/[0-9]*/maps")):
        pid = m.split("/")[2]
        if pid_filter and pid != pid_filter:
            continue
        # No point selecting a mapping whose memory cannot then be read.
        if not os.access(f"/proc/{pid}/mem", os.R_OK):
            continue
        try:
            with open(m) as fh:
                for line in fh:
                    fields = line.split()
                    if len(fields) < 5 or "x" not in fields[1]:
                        continue
                    yield pid, fields, line.strip()
        except OSError:
            continue


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pid", help="restrict to one process")
    ap.add_argument("--lib", default=DEFAULT_LIB, help="the engine the process loaded")
    ap.add_argument(
        "--tolerance",
        type=int,
        default=0x200000,
        help="how far a mapping's size may sit from the segment's, in bytes",
    )
    args = ap.parse_args()

    seg_off, seg_size = exec_segment(args.lib)
    print(f"reference {args.lib}: executable PT_LOAD at 0x{seg_off:x}, {seg_size} bytes")

    found = None
    for pid, fields, line in candidates(args.pid):
        lo, hi = (int(x, 16) for x in fields[0].split("-"))
        if abs((hi - lo) - seg_size) <= args.tolerance:
            found = (pid, lo, hi, line)
            break

    if not found:
        print(
            "no readable process has a mapping matching that segment.\n"
            "Sober's engine lives in the Flatpak PID namespace and needs\n"
            "`sudo flatpak enter` -- see the module docstring."
        )
        return 1

    pid, lo, hi, line = found
    size = min(hi - lo, seg_size)
    print(f"pid {pid}  {fields_of(line)}")

    with open(f"/proc/{pid}/mem", "rb", 0) as mem, open(args.lib, "rb") as f:
        mem.seek(lo)
        live = mem.read(size)
        f.seek(seg_off)
        disk = f.read(size)

    n = min(len(live), len(disk))
    diffs = [i for i in range(n) if live[i] != disk[i]]
    print(f"compared {n} bytes of executable text")
    print(f"DIFFERING BYTES: {len(diffs)}")
    for i in diffs[:12]:
        print(f"  +0x{seg_off + i:x}: file {disk[i]:#04x} -> memory {live[i]:#04x}")
    if len(diffs) > 12:
        print(f"  ... and {len(diffs) - 12} more")
    return 0


def fields_of(line):
    parts = line.split()
    return f"{parts[0]} {parts[1]}"


if __name__ == "__main__":
    sys.exit(main())
