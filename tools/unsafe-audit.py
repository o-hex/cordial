#!/usr/bin/env python3
"""Track `unsafe` and its SAFETY comments per crate, over time.

[ADR-036](../docs/adr/ADR-036-unsafe-is-a-boundary-not-a-convention.md) makes
the two-crate boundary a lint rather than a convention: `cordial-runtime` and
`cordial-linker-sys` are where Roblox's ABI calls into Cordial with raw
pointers, and `unsafe_code` is denied everywhere else. This script is how to
watch the two counts that matter without waiting for a full release build --
whether the two allowed crates are growing relative to the rest of the
workspace, and whether the SAFETY-comment gap inside them is closing or
widening.

**Default mode is a grep, not a build**, so it costs nothing and CI could run
it on every push. It counts lines that use the `unsafe` keyword as code
(`unsafe fn`, `unsafe impl`, `unsafe trait`, `unsafe extern`, `unsafe {`, or a
bare `unsafe` opening a block on its own line) and separately counts
`SAFETY:` comments. That deliberately overcounts relative to
`clippy::undocumented_unsafe_blocks`, which counts unsafe *blocks* and knows
which SAFETY comment belongs to which block -- an `unsafe fn` declaration
counts here and does not there, and clippy will not be fooled by a SAFETY
comment sitting above the wrong block. Prose that mentions the word "unsafe"
in an ordinary comment is excluded (comment-only lines are skipped for the
unsafe count, though not for the SAFETY count, since a SAFETY comment is
itself a comment).

Pass --clippy for the accurate number -- nothing gates on it, because this
repository runs no clippy job in CI at all, so it is a number to watch rather
than one that will fail a build. It runs
`cargo clippy -W clippy::undocumented_unsafe_blocks` in the given
`CARGO_TARGET_DIR` and counts the real warnings, per crate. That needs a full
build, so it is not the default and is not fast.

Usage:
    tools/unsafe-audit.py                  # grep counts, all crates
    tools/unsafe-audit.py --crate cordial-shell
    tools/unsafe-audit.py --clippy --target-dir target-toolbox
"""

import argparse
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Kept in workspace-member order (root Cargo.toml), not alphabetical, so the
# table reads in the same order as the ADR's boundary list: the two allowed
# crates first, then the three that forbid unsafe outright.
CRATES = [
    "cordial-runtime",
    "cordial-linker-sys",
    "cordial-shell",
    "cordial-plugins",
    "cordial-update",
]

# A code line that actually opens unsafe, as opposed to a doc comment that
# merely says the word. `unsafe {` and `unsafe fn`/`impl`/`trait`/`extern`
# cover the declared forms; a bare `unsafe` at end of line covers a block
# opened on its own line before a `{` on the next.
UNSAFE_CODE = re.compile(r"\bunsafe\b\s*(\{|fn\b|impl\b|trait\b|extern\b|$)")
SAFETY_COMMENT = re.compile(r"SAFETY:")


def is_comment_only(line: str) -> bool:
    return line.strip().startswith("//")


def scan_crate(name: str) -> tuple[int, int]:
    src = os.path.join(ROOT, "crates", name, "src")
    unsafe_sites = 0
    safety_comments = 0
    for dirpath, _dirnames, filenames in os.walk(src):
        for fn in filenames:
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn)
            with open(path, encoding="utf-8", errors="replace") as f:
                for line in f:
                    if SAFETY_COMMENT.search(line):
                        safety_comments += 1
                    if is_comment_only(line):
                        continue
                    if UNSAFE_CODE.search(line):
                        unsafe_sites += 1
    return unsafe_sites, safety_comments


def grep_report(names: list[str]) -> int:
    rows = []
    for name in names:
        sites, comments = scan_crate(name)
        gap = max(sites - comments, 0)
        rows.append((name, sites, comments, gap))

    width = max(len(n) for n in names) + 2
    print(f"{'crate':<{width}}{'unsafe sites':>14}{'SAFETY comments':>18}{'gap':>6}")
    total_sites = total_comments = total_gap = 0
    for name, sites, comments, gap in rows:
        print(f"{name:<{width}}{sites:>14}{comments:>18}{gap:>6}")
        total_sites += sites
        total_comments += comments
        total_gap += gap
    print(f"{'total':<{width}}{total_sites:>14}{total_comments:>18}{total_gap:>6}")
    print()
    print("Heuristic textual count -- see this file's docstring for what it")
    print("does and does not match. Use --clippy for the accurate count.")
    return 0


def clippy_report(names: list[str], target_dir: str) -> int:
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = target_dir
    cmd = [
        "cargo",
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-W",
        "clippy::undocumented_unsafe_blocks",
    ]
    proc = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)
    warnings = re.findall(
        r"warning: unsafe block missing a safety comment\s*\n\s*-->\s*([^\s:]+):",
        proc.stdout + proc.stderr,
    )
    counts = {name: 0 for name in names}
    other = 0
    for path in warnings:
        matched = False
        for name in names:
            if path.startswith(f"crates/{name}/"):
                counts[name] += 1
                matched = True
                break
        if not matched:
            other += 1

    width = max(len(n) for n in names) + 2
    print(f"{'crate':<{width}}{'undocumented unsafe blocks':>28}")
    total = 0
    for name in names:
        print(f"{name:<{width}}{counts[name]:>28}")
        total += counts[name]
    if other:
        print(f"{'(unmatched path)':<{width}}{other:>28}")
        total += other
    print(f"{'total':<{width}}{total:>28}")
    print()
    print("Source: clippy::undocumented_unsafe_blocks, cargo clippy exit code",
          proc.returncode)
    if proc.returncode not in (0, 101):
        print("clippy did not run cleanly; the count above may be incomplete.",
              file=sys.stderr)
        print(proc.stderr[-4000:], file=sys.stderr)
        return 1
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--crate", action="append", dest="crates",
                     help="limit to this crate (repeatable); default is all five")
    ap.add_argument("--clippy", action="store_true",
                     help="run cargo clippy for the exact undocumented-unsafe-blocks count "
                          "instead of the grep heuristic (slow, needs a full build)")
    ap.add_argument("--target-dir", default="target-toolbox",
                     help="CARGO_TARGET_DIR for --clippy (default: target-toolbox, "
                          "matching 'just build toolbox')")
    args = ap.parse_args()

    names = args.crates or CRATES
    unknown = [n for n in names if n not in CRATES]
    if unknown:
        print(f"not a workspace member: {', '.join(unknown)}", file=sys.stderr)
        return 2

    if args.clippy:
        return clippy_report(names, args.target_dir)
    return grep_report(names)


if __name__ == "__main__":
    sys.exit(main())
