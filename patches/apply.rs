// Builds a patched view of a submodule for the compiler, leaving the
// submodule itself untouched. `include!`d by crates/cordial-linker-sys/build.rs
// (0005 and 0006, on mcpelauncher-linker's nested bionic) and
// crates/cordial-guest/build.rs (0007 and 0008, on dynarmic), so every route
// that runs `cargo build` -- a checkout, `just`, CI, the Flatpak, the
// AppImage, the AUR, rpm, deb and Nix builds -- compiles the same loader and
// translator. Until this existed only tools/vr/build-aarch64.sh applied them,
// and a plain `cargo build` produced a client that could not link the Quest
// build at all.
//
// **Not in place.** The first version of this patched the submodules' own
// working trees, as the script had. Every build then left
// `third_party/dynarmic` and `third_party/mcpelauncher-linker` modified, and
// crates/cordial-shell/build.rs stamps `-dirty` on any tree whose `git status`
// is non-empty, so every x86-64 build from a clean checkout would have called
// itself dirty -- the provenance AGENTS.md relies on, lost to the build's own
// bookkeeping. So the overlay is a directory under OUT_DIR mirroring the
// submodule: a real directory for each of its directories, a symlink for each
// file, and a real, patched copy of each file a patch touches. CMake is
// pointed at the overlay. A source file includes its neighbours by relative
// path (mcpelauncher-linker's `src/linker.cpp` does
// `#include "../bionic/linker/linker_soinfo.h"`), and those paths resolve
// inside the overlay because its directories are real; a symlinked directory
// would have sent `..` back into the unpatched tree.
//
// A patched copy is rewritten only when its content changes, so a build
// script that reruns does not make CMake recompile the loader, and an
// unchanged symlink is left alone.
//
// Each patch must apply to the pristine file, or already be in it (a working
// tree where somebody applied it by hand, as tools/vr/build-aarch64.sh used
// to); anything else stops the build by name, because a patch that silently
// failed to take produces a binary indistinguishable from one without it
// (patches/README.md). `git apply` first and GNU `patch` if there is no git,
// since a Nix build has `patch` and no `git` and the Fedora CI container the
// reverse. GIT_CEILING_DIRECTORIES keeps git from treating the staging
// directory, which sits under `target/`, as part of the enclosing repository:
// there `git apply` would resolve the patch's paths against that
// repository's root and skip, without an error, every hunk outside the
// current directory.

/// `under` is the directory inside `src` the patches' paths are relative to:
/// `bionic` for the loader's, which are made against that nested submodule.
#[allow(dead_code)]
fn patched_overlay(
    src: &std::path::Path,
    under: &str,
    overlay: &std::path::Path,
    patches: &std::path::Path,
    names: &[&str],
) {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    // Absolute, since every symlink in the overlay points at it.
    let src = &src.canonicalize().unwrap_or_else(|e| panic!("{}: {e}", src.display()));

    // The files each patch touches, from its `+++ b/` lines.
    let mut touched = BTreeSet::new();
    let mut files = Vec::new();
    for name in names {
        let file = patches.join(format!("{name}.patch"));
        println!("cargo:rerun-if-changed={}", file.display());
        let text = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                touched.insert(Path::new(under).join(path.trim_end()));
            }
        }
        files.push((name, file));
    }

    // Stage the pristine files and apply the patches there.
    let stage = overlay.with_extension("stage");
    let _ = fs::remove_dir_all(&stage);
    for rel in &touched {
        let to = stage.join(rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(src.join(rel), &to).unwrap_or_else(|e| panic!("{}: {e}", src.join(rel).display()));
    }
    let have_git = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let run = |args: &[&str], file: &Path| -> bool {
        let mut cmd = if have_git {
            let mut c = Command::new("git");
            c.env("GIT_CEILING_DIRECTORIES", stage.parent().unwrap()).arg("apply").args(args).arg(file);
            c
        } else {
            // `--force` because without it `patch` guesses at reversed
            // patches, and in batch mode would answer the "already applied?"
            // dry run by applying forwards instead; with it each run tests
            // exactly the one direction asked.
            let mut c = Command::new("patch");
            c.args(["-p1", "--force", "--silent", "--no-backup-if-mismatch"]);
            for a in args {
                match *a {
                    "--check" => c.arg("--dry-run"),
                    "--reverse" => c.arg("--reverse"),
                    _ => unreachable!(),
                };
            }
            c.arg("-i").arg(file);
            c
        };
        cmd.current_dir(stage.join(under))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    for (name, file) in &files {
        if run(&["--check"], file) && run(&[], file) {
            continue;
        }
        if run(&["--check", "--reverse"], file) {
            println!("cargo:warning=patches/{name}.patch is already in {}; building it as it is", src.display());
            continue;
        }
        panic!(
            "{} neither applies to {} nor is already in it.\n\
             The submodule is probably at a different commit, or carries other edits.\n\
             `git submodule update --init --recursive` and a clean submodule tree fix the usual case.",
            file.display(),
            src.display()
        );
    }

    // Mirror the tree. `.git` is a directory in a clone and a file in a
    // submodule; neither belongs in the overlay.
    fn mirror(src: &Path, dst: &Path, rel: &Path, touched: &BTreeSet<PathBuf>) {
        fs::create_dir_all(dst.join(rel)).unwrap();
        for entry in fs::read_dir(src.join(rel)).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let rel = rel.join(entry.file_name());
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                mirror(src, dst, &rel, touched);
                continue;
            }
            if touched.contains(&rel) {
                continue;
            }
            // A symlink in the source is reproduced with its own target, so a
            // relative one keeps resolving inside the overlay; a file becomes
            // a symlink to it.
            let target = if kind.is_symlink() { fs::read_link(src.join(&rel)).unwrap() } else { src.join(&rel) };
            let link = dst.join(&rel);
            match fs::read_link(&link) {
                Ok(t) if t == target => continue,
                Ok(_) => fs::remove_file(&link).unwrap(),
                Err(_) if fs::symlink_metadata(&link).is_ok() => fs::remove_file(&link).unwrap(),
                Err(_) => {}
            }
            std::os::unix::fs::symlink(&target, &link).unwrap();
        }
    }
    mirror(src, overlay, Path::new(""), &touched);
    for rel in &touched {
        let patched = fs::read(stage.join(rel)).unwrap();
        let out = overlay.join(rel);
        let real = fs::symlink_metadata(&out).map(|m| m.file_type().is_file()).unwrap_or(false);
        if real && fs::read(&out).ok().as_deref() == Some(patched.as_slice()) {
            continue;
        }
        let _ = fs::remove_file(&out);
        fs::write(&out, patched).unwrap();
    }
    // Drop links left behind by files the submodule no longer has.
    fn prune(dir: &Path) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                prune(&path);
            } else if kind.is_symlink() && fs::metadata(&path).is_err() {
                let _ = fs::remove_file(&path);
            }
        }
    }
    prune(overlay);
    let _ = fs::remove_dir_all(&stage);
}
