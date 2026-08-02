//! Stamp the build with what git says it is.
//!
//! The version read `0.0.1` from the day the repository was created until well
//! after sign-in worked, because a number in `Cargo.toml` only changes when
//! somebody remembers. Worse, every build looked identical in the title bar, so
//! a binary built from a working tree that four agents were editing was
//! indistinguishable from a committed one — which cost an afternoon when input
//! regressed and nobody could say which tree the broken build came from.
//!
//! `git describe --tags --always --dirty` answers both. On a tagged commit it is
//! the tag and nothing else, so a release says `0.2.0`. Off a tag it appends the
//! distance and the hash, so a development build says `0.2.0-14-g8db7100` and
//! still sorts. With uncommitted changes it appends `-dirty`, which is the part
//! that would have caught the broken build.
//!
//! Falls back to the Cargo version when git is unavailable, because a Flatpak
//! builds from a tarball with no `.git` at all and must not fail for it.

use std::process::Command;

fn main() {
    // Without these the stamp is baked once and then lies for the rest of the
    // session — the failure mode this file exists to prevent.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../.git/refs");

    let described = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        // Tags are `v0.2.0` by convention, but "Cordial v0.2.0" reads as a typo
        // in a title bar and the `v` carries nothing the word "Cordial" does not
        // already imply.
        .map(|s| s.strip_prefix('v').unwrap_or(&s).to_string())
        .filter(|s| !s.is_empty());

    // `CARGO_PKG_VERSION` rather than a literal: a tarball build has no git and
    // should still report the version the crate was cut at.
    let stamp = described.unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=CORDIAL_BUILD_VERSION={stamp}");
}
