#!/usr/bin/env python3
"""Generate packaging/cargo-sources.json from Cargo.lock.

Issue #3: the Flatpak build used to run `cargo build` with `--share=network`,
so the crate graph came off crates.io inside the build sandbox and the result
was not reproducible. flatpak-builder can fetch every crate itself, in the
download phase, from a source list with a checksum against each one — and then
the build step needs no network at all.

The upstream tool for this is flatpak-builder-tools' flatpak-cargo-generator.py,
which is a hard dependency to acquire (aiohttp, toml) and which goes to the
network to resolve git dependencies. Cordial has none: every entry in Cargo.lock
carries `source = "registry+https://github.com/rust-lang/crates.io-index"` and a
`checksum`, and that checksum *is* the sha256 of the .crate tarball. So this
reads the lock file and writes the source list, offline, out of the standard
library. If a git dependency is ever added, this exits rather than emitting a
list that quietly omits it.

    python3 packaging/cargo-sources.py

Regenerate whenever Cargo.lock changes; CI builds from the committed JSON, so a
stale one fails the build with `no matching package named ... found` rather than
reaching for the network behind your back.
"""

import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"

# Cargo resolves $CARGO_HOME/config.toml relative to nothing — the vendor
# directory has to be named absolutely. This is where flatpak-builder puts the
# module's build directory, and it is why the manifest sets CARGO_HOME to the
# same prefix. The two have to agree; if the module is ever renamed, both move.
BUILD_DIR = "/run/build/cordial"


def main() -> int:
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    sources = []

    for pkg in lock["package"]:
        source = pkg.get("source")
        if source is None:
            continue  # a workspace member, carried by the `dir` source
        if source != CRATES_IO:
            print(
                f"error: {pkg['name']} comes from {source}, which this script "
                "does not know how to vendor. Add it as a `git` source in the "
                "manifest by hand, or use flatpak-cargo-generator.py instead.",
                file=sys.stderr,
            )
            return 1

        name, version = pkg["name"], pkg["version"]
        dest = f"cargo/vendor/{name}-{version}"
        sources.append(
            {
                "type": "archive",
                # A .crate is a gzipped tar with an unfamiliar extension, so
                # flatpak-builder has to be told rather than left to guess.
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                "sha256": pkg["checksum"],
                "dest": dest,
            }
        )
        # Cargo refuses a vendored crate without this file. `files` is empty on
        # purpose: cargo then verifies the package hash and skips the per-file
        # comparison, which is what `cargo vendor` itself writes for a registry
        # crate it did not have to modify.
        sources.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": pkg["checksum"], "files": {}}),
                "dest": dest,
                "dest-filename": ".cargo-checksum.json",
            }
        )

    sources.append(
        {
            "type": "inline",
            "contents": (
                "[source.crates-io]\n"
                'replace-with = "vendored-sources"\n'
                "\n"
                "[source.vendored-sources]\n"
                f'directory = "{BUILD_DIR}/cargo/vendor"\n'
            ),
            "dest": "cargo",
            "dest-filename": "config.toml",
        }
    )

    out = ROOT / "packaging" / "cargo-sources.json"
    out.write_text(json.dumps(sources, indent=4) + "\n")
    crates = (len(sources) - 1) // 2
    print(f"wrote {out.relative_to(ROOT)}: {crates} crates, {len(sources)} sources")
    return 0


if __name__ == "__main__":
    sys.exit(main())
