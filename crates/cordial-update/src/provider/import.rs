//! A build that is already on this machine, taken into the store on request.
//!
//! **This used to be a provider, tried first, and it is not any more.** Sober
//! downloads the same Android build and leaves it in a predictable place, and
//! Cordial read it in place for as long as it could not download anything
//! itself: a launch followed whatever Sober last fetched, "Download Roblox"
//! copied Sober's archives and reported a download, and a machine with Sober
//! never met the first-run screen. Those were the right behaviours while the
//! mirror was a risk and Sober was the only road. They are the wrong ones now
//! that Cordial keeps its own store and downloads into it
//! ([ADR-054](../../../docs/adr/ADR-054-cordial-owns-its-roblox-builds.md)),
//! because Cordial's builds then change when Sober's app happens to.
//!
//! So this module finds, and the caller decides. It is an *action*: the user
//! asks for Sober's build to be copied in (first-run screen, Settings), the
//! archives are verified against the pinned certificate exactly as a download
//! is, filed in the store with `source=sober`, and Sober is then forgotten.
//! Nothing here writes to, moves or deletes anything of Sober's, and nothing
//! follows its later updates.
//!
//! Being on the disk already is a statement about convenience and not about
//! provenance -- the path is under `$HOME` and anything running as the user
//! could have written it -- so an import takes the same
//! [`crate::apk_signature`] check as a download.
//!
//! ## Where it looks
//!
//! Sober's package directory, in both the Flatpak location and the native
//! one, because Sober ships as both and the two are not the same path.
//! `CORDIAL_APK_DIR`, which used to be consulted first, is gone: it was
//! undocumented, and a directory somebody wants to import is what "Import from
//! a file" is for.

use super::Archives;
use std::path::{Path, PathBuf};

/// The engine, inside whichever archive carries it.
///
/// `crate::apk::LIBRARY_IN_APK` rather than a literal: this was hardcoded to
/// `"lib/x86_64/libroblox.so"` until the aarch64 port, which meant this
/// could never recognise a build on an aarch64 host even though `apk.rs`
/// already had the right path for it.
const ENGINE: &str = crate::apk::LIBRARY_IN_APK;

/// A build found on disk, and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The directory it was found in.
    pub dir: PathBuf,
    pub archives: Archives,
    /// The engine's version, as the store would key it.
    pub version: String,
    /// What the archives occupy, so the screen can say what a copy costs.
    pub bytes: u64,
}

/// Sober's package directories, in the order to try.
fn sober_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        // Sober names this directory segment after the same Android ABI
        // string `crate::apk::HOST_ABI` on x86_64: "x86_64". Not verified for
        // aarch64 -- Sober is a project this codebase may observe running but
        // not inspect (AGENTS.md) -- so this is INFERRED from the x86_64
        // naming pattern rather than confirmed against a real Sober install on
        // ARM. If Sober turns out to use a different segment there, this finds
        // nothing and the first-run screen offers the download alone, which
        // is a safe failure.
        let package = format!("sober/packages/{}/com.roblox.client", crate::apk::HOST_ABI);
        // Sober as a Flatpak, which is how VinegarHQ distributes it and how
        // this machine has it.
        out.push(home.join(".var/app/org.vinegarhq.Sober/data").join(&package));
        // Sober installed natively, which follows the XDG data directory.
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        out.push(data.join(&package));
    }
    out
}

/// Sober's build, if its package directory holds one.
pub fn detect_sober() -> Option<Found> {
    sober_candidates().iter().find_map(|dir| from_dir(dir).ok())
}

/// [`detect_sober`], unless the store already keeps that build (compared by
/// [`crate::version::same_build`], so the mirror's three-component spelling
/// and the engine's four are one build). `None` is "nothing worth offering".
pub fn sober_offer(kept: &[crate::store::Entry]) -> Option<Found> {
    unkept(detect_sober(), kept)
}

fn unkept(found: Option<Found>, kept: &[crate::store::Entry]) -> Option<Found> {
    found.filter(|f| !kept.iter().any(|e| e.complete && crate::version::same_build(&f.version, &e.version)))
}

/// The archives in `dir`, if it holds a usable pair, and the version they carry.
pub fn from_dir(dir: &Path) -> Result<Found, String> {
    let archives = pair_in(dir)
        .ok_or_else(|| format!("{} holds no Roblox build (base.apk and {})", dir.display(), crate::install::SPLIT_APK))?;
    finish(dir, archives)
}

/// A build named by its `base.apk` (or a monolithic APK).
///
/// A split build's engine is in the split beside `base.apk`, so a chosen
/// `base.apk` that does not carry the engine is paired with the sibling
/// [`crate::install::SPLIT_APK`] if there is one, and refused by name if there
/// is not: the sentence somebody needs is which file is missing.
pub fn from_file(apk: &Path) -> Result<Found, String> {
    if !apk.is_file() {
        return Err(format!("{} is not a file", apk.display()));
    }
    let dir = apk.parent().unwrap_or(Path::new("."));
    let archives = if holds_engine(apk) {
        Archives { base: apk.to_path_buf(), split: apk.to_path_buf() }
    } else {
        let split = dir.join(crate::install::SPLIT_APK);
        if split.is_file() && holds_engine(&split) {
            Archives { base: apk.to_path_buf(), split }
        } else {
            return Err(format!(
                "No {ENGINE} in {} or beside it. On a split build the engine is in {}, so \
                 that file has to sit in the same folder as the APK.",
                apk.display(),
                crate::install::SPLIT_APK
            ));
        }
    };
    finish(dir, archives)
}

fn finish(dir: &Path, archives: Archives) -> Result<Found, String> {
    let version = version_in(&archives)
        .filter(|v| crate::store::is_valid_version(v))
        .ok_or_else(|| format!("the engine in {} carries no version Cordial can read", archives.split.display()))?;
    let bytes = archives
        .distinct()
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    Ok(Found { dir: dir.to_path_buf(), archives, version, bytes })
}

/// The archives in `dir`, if it holds a usable pair.
///
/// A monolithic APK is accepted as both halves: what matters is that the assets
/// and the engine are both reachable, and one file carrying both satisfies that
/// as well as two do.
fn pair_in(dir: &Path) -> Option<Archives> {
    let base = dir.join("base.apk");
    let split = dir.join(crate::install::SPLIT_APK);
    if base.is_file() && split.is_file() {
        return Some(Archives { base, split });
    }
    // One file carrying everything. Only accepted if it actually holds the
    // engine, checked by opening it rather than by trusting the name.
    for name in ["base.apk", "com.roblox.client.apk", "roblox.apk"] {
        let one = dir.join(name);
        if one.is_file() && holds_engine(&one) {
            return Some(Archives { base: one.clone(), split: one });
        }
    }
    None
}

fn holds_engine(apk: &Path) -> bool {
    let Ok(file) = std::fs::File::open(apk) else { return false };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return false;
    };
    let held = archive.by_name(ENGINE).is_ok();
    held
}

/// The engine's version, read out of the archive without extracting it.
///
/// [`crate::engine::scan`] takes anything readable, and a zip entry is
/// readable, so the 116 MB library is streamed past the scanner rather than
/// written to a temporary file first. That is the difference between this
/// answering in a moment and it answering in a minute.
fn version_in(archives: &Archives) -> Option<String> {
    let file = std::fs::File::open(&archives.split).ok()?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    let entry = archive.by_name(ENGINE).ok()?;
    crate::engine::scan(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn zip_at(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }

    fn engine_bytes(version: &str) -> Vec<u8> {
        let mut v = b"\0not an engine, just the shape of one\0".to_vec();
        v.extend_from_slice(version.as_bytes());
        v.push(0);
        v
    }

    /// A directory holding neither half is not a build, and saying so must not
    /// involve a network request or a panic.
    #[test]
    fn an_empty_directory_is_not_a_build() {
        let dir = scratch("empty");
        assert!(pair_in(&dir).is_none());
        assert!(from_dir(&dir).is_err());
    }

    /// **A file named like an APK and containing nothing is refused**, because
    /// the check opens it rather than reading its name. This is the shape of a
    /// truncated download and of an HTML error page saved with the wrong
    /// extension, and both have reached this project before.
    #[test]
    fn a_file_with_the_right_name_and_no_engine_is_not_a_build() {
        let dir = scratch("fake");
        std::fs::write(dir.join("base.apk"), b"not a zip at all").expect("write");
        assert!(pair_in(&dir).is_none());
    }

    #[test]
    fn a_split_pair_is_read_for_its_version_and_size() {
        let dir = scratch("split");
        zip_at(&dir.join("base.apk"), &[("assets/x", b"{}")]);
        zip_at(&dir.join(crate::install::SPLIT_APK), &[(ENGINE, &engine_bytes("2.738.0.1397"))]);
        let found = from_dir(&dir).unwrap();
        assert_eq!(found.version, "2.738.0.1397");
        assert!(found.bytes > 0);
        assert_ne!(found.archives.base, found.archives.split);
    }

    /// A chosen `base.apk` with the engine in its sibling split, and the
    /// refusal that names the split when there is none.
    #[test]
    fn a_chosen_base_apk_borrows_its_split_and_is_refused_without_one() {
        let dir = scratch("file");
        zip_at(&dir.join("base.apk"), &[("assets/x", b"{}")]);
        let refused = from_file(&dir.join("base.apk")).unwrap_err();
        assert!(refused.contains(crate::install::SPLIT_APK), "{refused}");

        zip_at(&dir.join(crate::install::SPLIT_APK), &[(ENGINE, &engine_bytes("2.730.0.790"))]);
        let found = from_file(&dir.join("base.apk")).unwrap();
        assert_eq!(found.version, "2.730.0.790");
        assert_eq!(found.archives.split, dir.join(crate::install::SPLIT_APK));

        let mono = scratch("mono");
        zip_at(&mono.join("roblox.apk"), &[("assets/x", b"{}"), (ENGINE, &engine_bytes("2.730.0.790"))]);
        let found = from_file(&mono.join("roblox.apk")).unwrap();
        assert_eq!(found.archives.base, found.archives.split);
        assert!(from_file(&mono.join("missing.apk")).is_err());
    }

    #[test]
    fn sober_is_not_offered_when_the_store_already_keeps_that_build() {
        // `sober_offer` is `detect_sober` through `unkept`; the filter is what
        // is worth pinning, and it does not need Sober installed.
        let kept = |v: &str| crate::store::Entry {
            version: v.into(),
            dir: PathBuf::from("/x").join(v),
            loaded_by: None,
            bytes: 0,
            complete: true,
            content_hash: None,
            signer: Some("ab".repeat(32)),
            provenance: None,
            version_code: None,
        };
        let found = Found {
            dir: PathBuf::from("/s"),
            archives: Archives { base: "/s/base.apk".into(), split: "/s/split.apk".into() },
            version: "2.738.0.1397".into(),
            bytes: 1,
        };
        let offered = |entries: &[crate::store::Entry]| unkept(Some(found.clone()), entries);
        assert!(offered(&[kept("2.738.0.1397")]).is_none(), "kept: nothing to offer");
        assert!(offered(&[kept("2.736.0.1408")]).is_some(), "a different build: offer it");
        assert!(offered(&[]).is_some());
    }

    /// The real one, on a machine that has Sober. Skipped rather than faked
    /// elsewhere: a synthetic APK would prove the zip reader works and nothing
    /// about whether this finds the file people actually have.
    #[test]
    fn the_build_in_sobers_directory_is_found_and_named() {
        match detect_sober() {
            Some(f) => {
                assert!(
                    f.version.split('.').count() >= 3,
                    "a version read out of the engine should look like one: {}",
                    f.version
                );
                eprintln!("sober holds {} ({} bytes)", f.version, f.bytes);
            }
            None => eprintln!("skipped: no Sober build on this machine"),
        }
    }
}
