//! The build that is already on this machine.
//!
//! **This is the source most users should end up on, and it is the cheapest
//! thing in the crate.** It makes no request, discloses nothing to anybody,
//! costs no bytes on a metered connection, and finishes in the time it takes to
//! read a version string out of a library. It exists because of an observation
//! that is easy to miss while designing a downloader: a large share of the
//! people who will run Cordial have already downloaded this exact file, and
//! the README told them to.
//!
//! Sober is why. It fetches the Android build through Google Play on the user's
//! own account and leaves it in a predictable place, Cordial's quickstart names
//! that place, and this crate has been reading `libroblox.so` out of it since
//! [`crate::engine`] was written. The file is already there. Fetching a second
//! copy of it from a third-party mirror would be worse in every dimension --
//! slower, more exposed, and no more trustworthy, since both copies get the
//! same signature check anyway.
//!
//! ## What this provider does not do
//!
//! It does not copy Sober's file into Cordial's cache and it does not read
//! anything of Sober's but this one archive, which is Roblox's rather than
//! Sober's. It takes no configuration from Sober, touches none of its state,
//! and is unaffected by whether Sober is running.
//!
//! It also does not trust the file for being local. Being on the disk already
//! is a statement about convenience and not about provenance -- the path is
//! under `$HOME` and anything running as the user could have written it -- so
//! the archive goes through exactly the same [`crate::apk_signature`] check as
//! one off a mirror. A local file that fails that check is refused with the
//! same words.
//!
//! ## Where it looks
//!
//! `CORDIAL_APK_DIR` first, so somebody who has an APK of their own can say so
//! without editing anything. Then Sober's package directory, in both the
//! Flatpak location and the native one, because Sober ships as both and the
//! two are not the same path.

use super::{Available, Progress, Provider};
use crate::Unreachable;
use std::path::{Path, PathBuf};

/// The engine, inside whichever archive carries it.
const ENGINE: &str = "lib/x86_64/libroblox.so";

#[derive(Debug, Default)]
pub struct OnThisMachine;

/// Every directory that might hold a base/split pair, in the order to try.
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(explicit) = std::env::var_os("CORDIAL_APK_DIR") {
        out.push(PathBuf::from(explicit));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let package = "sober/packages/x86_64/com.roblox.client";
        // Sober as a Flatpak, which is how VinegarHQ distributes it and how
        // this machine has it.
        out.push(home.join(".var/app/org.vinegarhq.Sober/data").join(package));
        // Sober installed natively, which follows the XDG data directory.
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        out.push(data.join(package));
    }
    out
}

/// The archives in `dir`, if it holds a usable pair.
///
/// A monolithic APK is accepted as both halves: what matters is that the assets
/// and the engine are both reachable, and one file carrying both satisfies that
/// as well as two do.
fn pair_in(dir: &Path) -> Option<super::Archives> {
    let base = dir.join("base.apk");
    let split = dir.join("split_config.x86_64.apk");
    if base.is_file() && split.is_file() {
        return Some(super::Archives { base, split });
    }
    // One file carrying everything. Only accepted if it actually holds the
    // engine, checked by opening it rather than by trusting the name.
    for name in ["base.apk", "com.roblox.client.apk", "roblox.apk"] {
        let one = dir.join(name);
        if one.is_file() && holds_engine(&one) {
            return Some(super::Archives { base: one.clone(), split: one });
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
/// provider answering in a moment and it answering in a minute.
fn version_in(archives: &super::Archives) -> Option<String> {
    let file = std::fs::File::open(&archives.split).ok()?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    let entry = archive.by_name(ENGINE).ok()?;
    crate::engine::scan(entry)
}

impl Provider for OnThisMachine {
    fn name(&self) -> &'static str {
        "local"
    }

    fn needs_network(&self) -> bool {
        false
    }

    fn newest(&self, progress: &mut dyn FnMut(Progress)) -> Result<Available, Unreachable> {
        progress(Progress::Asking { provider: self.name() });
        for dir in candidates() {
            let Some(archives) = pair_in(&dir) else { continue };
            let Some(version) = version_in(&archives) else { continue };
            // There is no version code in an extracted library, and inventing
            // one would put a fabricated number where the ordering logic reads
            // it. Zero is the honest answer and callers compare by name for
            // this source.
            return Ok(Available { name: version, code: 0 });
        }
        Err(Unreachable::NoSource {
            why: "no Roblox build was found on this machine. Cordial looked in \
                  $CORDIAL_APK_DIR and in Sober's package directory."
                .into(),
        })
    }

    /// "Fetching" here is establishing that the files are where they were.
    ///
    /// **Nothing is copied.** The archives are handed back at the paths they
    /// already occupy, and the caller verifies and extracts from there. Copying
    /// 150 MB inside the same filesystem to reach the same conclusion would be
    /// the one expensive step in an otherwise free source.
    fn fetch(
        &self,
        version: &Available,
        _into: &Path,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<super::Archives, Unreachable> {
        for dir in candidates() {
            let Some(archives) = pair_in(&dir) else { continue };
            let Some(found) = version_in(&archives) else { continue };
            if found != version.name {
                continue;
            }
            progress(Progress::Verifying {
                file: archives.base.display().to_string(),
            });
            return Ok(archives);
        }
        Err(Unreachable::NoSource {
            why: format!(
                "the build on this machine is no longer {}; it changed between being \
                 checked and being used",
                version.name
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory holding neither half is not a source, and saying so must not
    /// involve a network request or a panic.
    #[test]
    fn an_empty_directory_is_not_a_build() {
        let dir = std::env::temp_dir().join("cordial-local-empty");
        std::fs::create_dir_all(&dir).expect("scratch");
        assert!(pair_in(&dir).is_none());
    }

    /// **A file named like an APK and containing nothing is refused**, because
    /// the check opens it rather than reading its name. This is the shape of a
    /// truncated download and of an HTML error page saved with the wrong
    /// extension, and both have reached this project before.
    #[test]
    fn a_file_with_the_right_name_and_no_engine_is_not_a_build() {
        let dir = std::env::temp_dir().join("cordial-local-fake");
        std::fs::create_dir_all(&dir).expect("scratch");
        std::fs::write(dir.join("base.apk"), b"not a zip at all").expect("write");
        assert!(pair_in(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_source_never_reaches_the_network() {
        assert!(!OnThisMachine.needs_network());
    }

    /// The real one, on a machine that has the build. Skipped rather than
    /// faked elsewhere: a synthetic APK would prove the zip reader works and
    /// nothing about whether this finds the file people actually have.
    #[test]
    fn the_build_on_this_machine_is_found_and_named() {
        let mut noise = |_: Progress| {};
        match OnThisMachine.newest(&mut noise) {
            Ok(v) => {
                assert!(
                    v.name.split('.').count() >= 3,
                    "a version read out of the engine should look like one: {}",
                    v.name
                );
                eprintln!("local provider found {}", v.name);
            }
            Err(e) => eprintln!("skipped: no build on this machine ({e})"),
        }
    }
}
