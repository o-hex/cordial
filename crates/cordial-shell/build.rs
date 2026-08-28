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
    // So that gap stays, and the workaround for it is
    // `touch crates/cordial-shell/build.rs` before a build whose version string
    // anyone is going to quote. **The separate and worse gap below -- commits
    // not being noticed at all -- is fixed rather than documented**, because a
    // stamp that is stale by thirteen commits is not a caveat, it is wrong.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../.git/refs");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
    println!("cargo:rerun-if-env-changed=CORDIAL_GIT_SHA");

    // **`.git/refs` above does not catch a commit, and the three lines before
    // this one were measured not catching thirteen of them.** A window titled
    // `Cordial 0.6.0-126-gb735207` was screenshotted out of a binary built from
    // `v0.7.0-12-gd52e528`, which is precisely the plausible-looking lie the
    // module comment says an explicit stamp exists to prevent -- arriving
    // through the mechanism meant to prevent it.
    //
    // Why the existing watches miss it: a commit does not rewrite `.git/HEAD`,
    // which still reads `ref: refs/heads/main`; and rewriting
    // `.git/refs/heads/main` does not change the mtime of the `.git/refs`
    // directory that is being watched, because the file is replaced inside a
    // subdirectory of it. `.git/index` does change, which is why some commits
    // appear to work and the failure is intermittent -- the worst kind to have
    // in a version string.
    //
    // So watch the file HEAD actually points at. This is one `read_to_string`
    // of a file under 64 bytes at build-script time and it costs nothing;
    // unlike forcing a rerun every build, which the comment above rejects on
    // measured grounds that still hold.
    if let Ok(head) = std::fs::read_to_string("../../.git/HEAD") {
        if let Some(reference) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=../../.git/{reference}");
        }
    }

    // **`Cargo.toml` is the version. `git` is provenance. They are not
    // alternatives, and treating them as alternatives was the bug.**
    //
    // This used to stamp `git describe --tags --always --dirty` as *the*
    // version, so a tree whose manifest said 0.11.0 displayed `0.10.0-26-g...`
    // -- the previous release -- and was reported as "wheres 0.11.0". The first
    // repair here was worse: it synthesised `0.11.0-dev-26-g...`, which is a
    // version-shaped string that is not a version, and anything wanting to
    // order two of them would need a parser for Cordial's own build metadata.
    //
    // The binary was already carrying the contradiction. `cordial-update`'s
    // User-Agent has always been `concat!("Cordial/", env!("CARGO_PKG_VERSION"))`
    // -- so one build announced itself to a mirror as 0.11.0 while its title
    // bar read 0.10.0-26-g571e69b-dirty. Two numbers that can disagree
    // eventually do.
    //
    // So: `CARGO_PKG_VERSION` is the version, read directly by
    // `crate::version::VERSION` and never computed here. It survives a tarball,
    // an AUR source package, a `cargo publish` and the Flatpak's `type: dir`
    // source -- none of which has a usable `.git` -- and it compares as semver,
    // which is what anything asking "is the remote newer" needs.
    //
    // This build script emits only the short commit, and only when there is
    // one. `option_env!` on the reading side means no git is not a build
    // failure, which is the property the old scheme lacked.
    // The manifest's version, needed below to tell "this is the release" from
    // "this is somewhere after it".
    let crate_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();

    let sha = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Kept, because AGENTS.md leans on it: a `-dirty` build came from a tree
    // with uncommitted changes and is otherwise indistinguishable from a
    // committed one, which cost an afternoon of chasing an input regression
    // nobody could attribute to a tree. It rides on the *provenance* string
    // now rather than on the version, which is where it always belonged.
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    // A packager with no `.git` may supply this instead; the three native
    // package scripts do. Nothing may supply the *version* any more -- that is
    // the manifest's, and a CI gate checks the tag against it.
    if let Ok(supplied) = std::env::var("CORDIAL_GIT_SHA") {
        let supplied = supplied.trim();
        if !supplied.is_empty() {
            println!("cargo:rustc-env=CORDIAL_GIT_SHA={supplied}");
            return;
        }
    }
    // **A release shows no commit, and that is not a style choice -- it is what
    // this did before and I broke it.** The old scheme stamped `git describe
    // --tags --always --dirty`, which on an exact tag is just `0.2.0`, so a
    // release read `Cordial 0.2.0` and only a development build carried a hash.
    // Replacing it with an unconditional sha made every release read
    // `Cordial 0.11.0 (0fdbb4425)`, and it was spotted immediately: "why did
    // you put the version in ( ), thats new".
    //
    // On a clean checkout of a tag matching the manifest, the version already
    // identifies the build exactly -- the tag names the commit -- so the hash
    // adds nothing and puts build plumbing in front of every ordinary user. Off
    // a tag, or with uncommitted changes, it is the only thing that says which
    // build this is, and AGENTS.md leans on it.
    let exact_release = !dirty
        && Command::new("git")
            .args(["describe", "--tags", "--exact-match", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|t| t.trim().trim_start_matches('v').to_string())
            .is_some_and(|t| t == crate_version);

    if exact_release {
        return;
    }
    if let Some(sha) = sha {
        let suffix = if dirty { "-dirty" } else { "" };
        println!("cargo:rustc-env=CORDIAL_GIT_SHA={sha}{suffix}");
    }
}
