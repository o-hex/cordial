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
//!
//! A packager may override the whole thing by setting `CORDIAL_BUILD_VERSION`,
//! and should, because git answers badly for a tarball in two measured ways.
//! With no `.git` in reach it fails and the fallback stamps a bare `0.6.0`,
//! while the RPM built from that same tarball is numbered
//! `0.6.0-1.108.20260822git9d9c980` — so `rpm -q` and the title bar disagree
//! about which build is installed. Worse, if the tarball is unpacked anywhere
//! beneath *someone else's* repository, `git describe` walks up out of the
//! source tree and reports that repository's tag instead: a scratch repo
//! tagged `v9.9.9` produced a Cordial that called itself `Cordial 9.9.9`. The
//! first is merely useless, the second is a plausible-looking lie, and both
//! are why an explicit stamp outranks git rather than filling in behind it.

use std::process::Command;

fn main() {
    // These catch a commit, a checkout and a staging operation, which is most of
    // what moves the stamp.
    //
    // **They do not catch an unstaged edit, and that is the case this file cares
    // about most.** Measured: editing a tracked file flips `git describe --dirty`
    // from `v1.0.0` to `v1.0.0-dirty` while leaving HEAD, refs and `.git/index`
    // all untouched — so cargo keeps the previous build script output and the
    // binary goes on claiming a clean tree it was not built from. That is exactly
    // the confusion the module comment above describes, surviving the fix for it.
    //
    // Forcing a re-run every build does fix it and is not worth what it costs:
    // re-running the script invalidates the crate whether or not the stamp
    // actually changed, and three consecutive no-op builds measured here took
    // 14 s, 16 s and 21 s instead of 0.3 s. Every developer and every agent would
    // pay a relink on every build to keep one string honest.
    //
    // So the gap stays, and the workaround is `touch crates/cordial-shell/build.rs`
    // before a build whose version string anyone is going to quote.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../.git/refs");
    println!("cargo:rerun-if-env-changed=CORDIAL_BUILD_VERSION");

    // Read before the `cargo:rustc-env` line below is emitted, so this is the
    // packager's value from the ambient environment and never our own output;
    // `rustc-env` applies to the rustc invocation, not to this process.
    let supplied = std::env::var("CORDIAL_BUILD_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let described = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // `CARGO_PKG_VERSION` rather than a literal: a tarball build has no git and
    // should still report the version the crate was cut at.
    let stamp = supplied
        .or(described)
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());

    // Tags are `v0.2.0` by convention, but "Cordial v0.2.0" reads as a typo in a
    // title bar and the `v` carries nothing the word "Cordial" does not already
    // imply. Stripped after the choice rather than inside it, so a packager who
    // passes the tag through verbatim gets the same treatment git's answer does.
    let stamp = stamp.strip_prefix('v').unwrap_or(&stamp).to_string();
    println!("cargo:rustc-env=CORDIAL_BUILD_VERSION={stamp}");
}
