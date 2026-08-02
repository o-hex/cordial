//! Where the Roblox build is, and how Cordial goes looking for one.
//!
//! Cordial ships no Roblox code and never will, so every launch depends on a
//! build the user already has. Nothing in this project used to record where
//! that was: the only way to run the client was a hand-typed `cordial-run`
//! command line carrying `--lib-dir` and `--apk`, which is why the chooser's
//! activate handler had nothing to call.
//!
//! The resolution order here is the same one `justfile`'s `dev` recipe uses,
//! deliberately and to the letter, because that recipe has been run end to end
//! and this had not. Two things in it are not guessable and both cost somebody
//! an afternoon to establish:
//!
//! The engine is **not in `base.apk`** on a split build. `libroblox.so` lives
//! in `split_config.x86_64.apk` beside it, so anything that assumes the APK it
//! was given contains the engine fails on the ordinary case. Each candidate is
//! tried in turn instead of one being asserted.
//!
//! And the extracted engine belongs in the cache rather than beside the APK,
//! because the APK is usually inside another application's data directory,
//! which Cordial has no business writing into.
//!
//! **Detection is a filesystem check every time, never a remembered answer.**
//! A stored "yes, it is installed" goes stale the moment the user deletes the
//! build or Sober replaces it, and a launcher that then fails with a path
//! error is worse than one that simply looks again.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The engine object. `--lib-dir` names the directory holding it.
pub const LIBRARY: &str = "libroblox.so";

/// Points this run at one APK without touching the saved settings. Set by
/// `just dev --apk <path>`.
pub const APK_OVERRIDE: &str = "CORDIAL_APK";

/// Its path inside whichever APK carries it.
const LIBRARY_IN_APK: &str = "lib/x86_64/libroblox.so";

/// What the user has pinned by hand, if anything.
///
/// Both are `None` on a fresh install and stay that way for anyone who lets
/// detection do its job — which is the intended case, not a degraded one. A
/// value here is an override, and [`locate`] honours it over anything it would
/// otherwise find, because a user who went to Settings and chose a file meant
/// that file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RobloxInstall {
    pub apk: Option<PathBuf>,
    pub lib_dir: Option<PathBuf>,
}

/// A build that has been found and checked: both of these exist right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub apk: PathBuf,
    pub lib_dir: PathBuf,
}

/// Why a launch cannot proceed, split by what the user can do about it.
#[derive(Debug)]
pub enum NotFound {
    /// No APK anywhere. The answer is the Sober instructions, not an error
    /// dialog — this is a first-run state rather than a fault, and it is the
    /// only one with a scripted way out.
    NoBuild,
    /// An APK was found or configured, and getting the engine out of it did not
    /// work. Carries something specific enough to act on.
    Unusable(String),
}

/// Sober's copy of the official Android build.
///
/// Named rather than searched for, because the point is to be able to tell the
/// user exactly where Cordial looked. Sober downloads the same official
/// x86-64 Android build this runtime loads and leaves it unpacked, which makes
/// it far and away the least painful way for someone to obtain one — but it is
/// another application's private directory, so Cordial *offers* what it finds
/// there and records the path only once the user has launched with it. It never
/// silently depends on Sober being installed.
pub fn sober_apk() -> PathBuf {
    sober_apk_under(&std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir))
}

/// Split out from [`sober_apk`] so the path itself can be pinned by a test
/// without a test having to write to `HOME`, which is process-wide and would
/// interleave with every other test in this crate that reads it.
fn sober_apk_under(home: &Path) -> PathBuf {
    home.join(".var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk")
}

/// Where Cordial keeps the engine it extracted. Same path `just dev` uses, so
/// the two never make each other re-extract 115 MB.
pub fn engine_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/lib/x86_64")
}

/// The APK Cordial would use, and whether the user picked it or Cordial found
/// it. `None` means there is nothing to launch and the instructions are the
/// answer.
pub fn effective_apk(configured: &RobloxInstall) -> Option<(PathBuf, Origin)> {
    // `CORDIAL_APK` overrides the saved setting outright, the same override
    // pattern `CORDIAL_SHELL_CONFIG`, `CORDIAL_FLAGS` and `CORDIAL_PROFILE_ROOT`
    // already use. `just dev --apk <path>` is what sets it: that recipe now
    // starts the shell rather than the engine, so the one path that genuinely
    // varies between contributors has to reach the shell somehow, and pointing
    // it at a build for one run must not overwrite what the user chose in
    // Settings.
    if let Some(apk) = std::env::var_os(APK_OVERRIDE) {
        return Some((PathBuf::from(apk), Origin::Environment));
    }
    if let Some(apk) = &configured.apk {
        return Some((apk.clone(), Origin::Chosen));
    }
    let sober = sober_apk();
    sober.is_file().then_some((sober, Origin::Sober))
}

/// Where a path came from, so the UI can say. A detected path that presents
/// itself as configuration is how a user ends up not knowing that deleting
/// another application will break this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Environment,
    Chosen,
    Sober,
}

impl Origin {
    pub fn describe(self) -> &'static str {
        match self {
            Origin::Environment => "Set by CORDIAL_APK for this run only",
            Origin::Chosen => "Chosen in Settings",
            Origin::Sober => "Found in Sober's download (org.vinegarhq.Sober), which Cordial does not manage",
        }
    }
}

/// Find a usable build, extracting the engine from the APK if that is what it
/// takes.
///
/// The extraction runs on the calling thread and the calling thread is the one
/// GTK draws on, so the window stops responding while it happens. Measured at
/// 0.6 s for the 115 MB object on this machine, once, on the first launch after
/// an install — which is a worse trade than a progress bar and a better one
/// than the two states and a worker thread that a progress bar costs. If that
/// stops being true, this is the place to move.
pub fn locate(configured: &RobloxInstall) -> Result<Build, NotFound> {
    let Some((apk, _)) = effective_apk(configured) else {
        return Err(NotFound::NoBuild);
    };
    if !apk.is_file() {
        return Err(NotFound::Unusable(format!(
            "No APK at {}. Open Settings and choose one, or clear it to let Cordial look again.",
            apk.display()
        )));
    }

    // An explicit --lib-dir wins and is not second-guessed: someone who set it
    // has a reason, and quietly extracting over the top of it would hide a
    // mismatch between the engine they meant to test and the one they got.
    if let Some(lib_dir) = &configured.lib_dir {
        return if lib_dir.join(LIBRARY).is_file() {
            Ok(Build { apk, lib_dir: lib_dir.clone() })
        } else {
            Err(NotFound::Unusable(format!(
                "No {LIBRARY} in {}. Open Settings and clear the engine directory to let Cordial extract one.",
                lib_dir.display()
            )))
        };
    }

    // Beside the APK, which is where it lands if you unzip in place.
    if let Some(beside) = apk.parent().map(|d| d.join("lib/x86_64")) {
        if beside.join(LIBRARY).is_file() {
            return Ok(Build { apk, lib_dir: beside });
        }
    }

    let cache = engine_cache();
    if cache.join(LIBRARY).is_file() {
        return Ok(Build { apk, lib_dir: cache });
    }

    match extract_engine(&apk, &cache) {
        Ok(from) => {
            println!("  shell: extracted {LIBRARY} from {} into {}", from.display(), cache.display());
            Ok(Build { apk, lib_dir: cache })
        }
        Err(e) => Err(NotFound::Unusable(e)),
    }
}

/// Candidate archives, in the order the justfile tries them: the APK named
/// first, then its `split_config*` siblings.
fn engine_candidates(apk: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![apk.to_path_buf()];
    if let Some(dir) = apk.parent() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut splits: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("split_config") && n.ends_with(".apk"))
                })
                .collect();
            splits.sort();
            candidates.extend(splits);
        }
    }
    candidates
}

/// Pull `lib/x86_64/libroblox.so` out of the first archive that has it.
///
/// Written to a temporary name and renamed into place, because a launch
/// interrupted halfway leaves a 40 MB file that looks exactly like a complete
/// one to the `is_file` check above, and the next launch would then hand the
/// loader a truncated engine. `rename` within one directory is atomic.
fn extract_engine(apk: &Path, into: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;

    let mut tried = Vec::new();
    for candidate in engine_candidates(apk) {
        let Ok(file) = std::fs::File::open(&candidate) else { continue };
        let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
            continue;
        };
        let Ok(mut entry) = archive.by_name(LIBRARY_IN_APK) else {
            tried.push(candidate);
            continue;
        };

        let partial = into.join(format!("{LIBRARY}.partial"));
        let mut out = std::fs::File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", partial.display()))?;
        drop(out);
        std::fs::rename(&partial, into.join(LIBRARY)).map_err(|e| format!("{}: {e}", into.display()))?;
        return Ok(candidate);
    }

    Err(format!(
        "No {LIBRARY_IN_APK} in {} or its split_config siblings ({} tried). \
         On a split build the engine is in split_config.x86_64.apk, not base.apk — \
         if it is somewhere else, set the engine directory in Settings.",
        apk.display(),
        tried.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-shell-install-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// `CORDIAL_APK` is process-wide, so the two tests that care about it have
    /// to be kept apart from each other. Same reasoning as `profile`'s own ENV
    /// mutex, and the same reason: cargo runs these as threads of one process.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_chosen_apk_wins_over_anything_detected() {
        // Someone who went to Settings and picked a file meant that file, even
        // if Sober's copy is sitting right there.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let install = RobloxInstall { apk: Some(PathBuf::from("/somewhere/base.apk")), lib_dir: None };
        let (apk, origin) = effective_apk(&install).unwrap();
        assert_eq!(apk, PathBuf::from("/somewhere/base.apk"));
        assert_eq!(origin, Origin::Chosen);
    }

    #[test]
    fn the_environment_override_wins_over_the_saved_setting() {
        // `just dev --apk <path>` has to be able to point one run at a build
        // without silently rewriting what the user chose in Settings.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(APK_OVERRIDE, "/from/env/base.apk");
        let install = RobloxInstall { apk: Some(PathBuf::from("/from/settings/base.apk")), lib_dir: None };
        let (apk, origin) = effective_apk(&install).unwrap();
        std::env::remove_var(APK_OVERRIDE);
        assert_eq!(apk, PathBuf::from("/from/env/base.apk"));
        assert_eq!(origin, Origin::Environment);
    }

    #[test]
    fn the_split_apk_is_tried_after_the_one_it_was_given() {
        // The engine is not in base.apk on a split build. Asserting otherwise
        // is the mistake this ordering exists to stop, so the order is pinned.
        let dir = scratch("candidates");
        for name in ["base.apk", "split_config.x86_64.apk", "split_config.en.apk"] {
            std::fs::write(dir.join(name), b"not really a zip").unwrap();
        }
        let candidates = engine_candidates(&dir.join("base.apk"));
        assert_eq!(candidates[0], dir.join("base.apk"));
        assert!(candidates.contains(&dir.join("split_config.x86_64.apk")));
    }

    #[test]
    fn the_detected_location_is_the_one_the_justfile_documents() {
        // `just dev` prints this path to anyone who has no build, and the two
        // must not drift: a user told to look in one place while the shell
        // looks in another has no way to tell which is wrong.
        let p = sober_apk_under(Path::new("/home/someone"));
        assert_eq!(
            p,
            Path::new(
                "/home/someone/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk"
            )
        );
    }

    #[test]
    fn a_configured_apk_that_has_gone_away_is_reported_rather_than_ignored() {
        // The stored path going stale is the ordinary way this breaks — the
        // user moves or deletes the build. Falling back to detection silently
        // would launch something other than what they chose.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("stale");
        let install = RobloxInstall { apk: Some(dir.join("gone.apk")), lib_dir: None };
        match locate(&install) {
            Err(NotFound::Unusable(msg)) => assert!(msg.contains("Settings"), "{msg}"),
            other => panic!("expected a usable message, got {other:?}"),
        }
    }

    #[test]
    fn an_archive_without_the_engine_says_where_it_looked() {
        let dir = scratch("noengine");
        std::fs::write(dir.join("base.apk"), b"not really a zip").unwrap();
        let err = extract_engine(&dir.join("base.apk"), &dir.join("cache")).unwrap_err();
        assert!(err.contains("split_config"), "{err}");
    }
}
