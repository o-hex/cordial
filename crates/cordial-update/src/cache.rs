//! Stamping the extracted engine with the APK it came from.
//!
//! `~/.cache/cordial/lib/x86_64` holds `libroblox.so`, pulled out of whichever
//! APK the user has. **Presence alone used to be the whole test**, which meant a
//! new Roblox build left the *old* engine in place and Cordial ran it against
//! the new APK's assets — a silent version mismatch, which is worse than the
//! cold start the cache exists to avoid, because nothing about it presents as a
//! caching problem.
//!
//! `justfile`'s `client` recipe had exactly this bug and had it fixed; the shell
//! did not, and ran the stale engine. This is the fix in code, and it writes
//! **the same stamp string the recipe writes**, deliberately:
//!
//! ```text
//! stamp="$cache/.from"
//! want="$(stat -c '%s %Y %n' "$apk")"
//! ```
//!
//! Size, mtime in seconds, path — separated by single spaces, no trailing
//! newline. Not because either has to read the other's file, but because they
//! share one cache directory, and two formats in one file means `just client`
//! and the shell each see the other's stamp as a change and re-extract 115 MB in
//! turn, for as long as somebody alternates between them. Where they can still
//! disagree is the path: `stat`'s `%n` prints the path as it was given, so a
//! relative `--apk base.apk` stamps differently from the shell's absolute path.
//! The cost of that is one extra extraction, which is the safe direction.
//!
//! **The mtime is deliberately not read off the extracted file.** Zip preserves
//! the timestamp stored in the archive, so an engine extracted this morning has
//! an mtime somewhere in 1981 and comparing it to anything is meaningless.
//!
//! Size and mtime rather than a hash of the APK: hashing 115 MB on every launch
//! to answer "has this changed" costs about half a second of a cold start to
//! detect a case — same size, same mtime, different contents — that means
//! somebody deliberately forged a timestamp. [`crate::download`] hashes what it
//! fetched, which is where a hash actually protects something.

use std::path::Path;
use std::time::UNIX_EPOCH;

/// The stamp file, beside the engine it describes. Dot-prefixed so it is not
/// mistaken for something the loader wants, and the same name `justfile` uses.
pub const STAMP: &str = ".from";

/// What the cache should be stamped with for this APK, or `None` if the APK
/// cannot be looked at.
///
/// `None` is not "unchanged". [`is_current`] treats it as a mismatch, because a
/// build that cannot be stat'd is a build nothing should be claiming to have
/// extracted from.
pub fn stamp_for(apk: &Path) -> Option<String> {
    let meta = std::fs::metadata(apk).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{} {} {}", meta.len(), mtime, apk.display()))
}

/// What the cache says it was extracted from.
pub fn stamp_of(cache_dir: &Path) -> Option<String> {
    std::fs::read_to_string(cache_dir.join(STAMP)).ok()
}

/// Whether what is in `cache_dir` came from `apk` as it is right now.
///
/// An unstamped cache is **not** current. Every cache extracted before this
/// existed is unstamped, so this re-extracts once for everybody who upgrades —
/// which is 0.6 s on the machine this was measured on, once, and is the correct
/// answer for a directory whose provenance genuinely is unknown.
pub fn is_current(cache_dir: &Path, apk: &Path) -> bool {
    match (stamp_of(cache_dir), stamp_for(apk)) {
        (Some(have), Some(want)) => have == want,
        _ => false,
    }
}

/// Record that what is in `cache_dir` came from `apk`.
///
/// Called after the extraction has landed, never before: a stamp written first
/// and an extraction that then failed would claim a cache that is not there.
pub fn write_stamp(cache_dir: &Path, apk: &Path) -> std::io::Result<()> {
    let want = stamp_for(apk).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("cannot stat {} to stamp the engine cache", apk.display()),
        )
    })?;
    std::fs::create_dir_all(cache_dir)?;
    std::fs::write(cache_dir.join(STAMP), want)
}

/// Forget the stamp, so the next launch re-extracts.
///
/// Used when the engine is removed for any reason. A stamp left behind pointing
/// at an engine that is gone is the mirror image of the bug this module exists
/// for.
pub fn clear_stamp(cache_dir: &Path) {
    let _ = std::fs::remove_file(cache_dir.join(STAMP));
}

/// The Roblox version an extracted engine is known to be, if Cordial fetched it
/// and therefore knows.
///
/// `None` is the ordinary case today and is honest: an APK the user obtained
/// themselves — through Sober, or by hand — carries no version Cordial can read
/// without parsing Android's binary manifest, and guessing one would put a
/// number in front of the user that nothing established.
pub fn recorded_version(cache_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(cache_dir.join(".version")).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Record the version of a build Cordial fetched itself.
pub fn record_version(cache_dir: &Path, version: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    std::fs::write(cache_dir.join(".version"), version.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-update-cache-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_stamp_is_the_string_the_justfile_writes() {
        // `stat -c '%s %Y %n'`. If this drifts, `just client` and the shell
        // re-extract 115 MB in turn every time somebody alternates between
        // them, and neither of them looks wrong on its own.
        let dir = scratch("format");
        let apk = dir.join("base.apk");
        std::fs::write(&apk, b"0123456789").unwrap();
        let stamp = stamp_for(&apk).unwrap();
        let mut fields = stamp.split(' ');
        assert_eq!(fields.next().unwrap(), "10");
        assert!(fields.next().unwrap().parse::<u64>().unwrap() > 1_500_000_000);
        assert_eq!(fields.next().unwrap(), apk.to_str().unwrap());
        assert_eq!(fields.next(), None);
        assert!(!stamp.ends_with('\n'), "stat prints no newline and printf writes none");
    }

    #[test]
    fn an_unstamped_cache_is_not_current() {
        // The whole defect: presence alone. Every cache from before this
        // existed lands here, and re-extracting once is the right answer for a
        // directory nobody can vouch for.
        let dir = scratch("unstamped");
        let apk = dir.join("base.apk");
        std::fs::write(&apk, b"apk").unwrap();
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("libroblox.so"), b"engine").unwrap();
        assert!(!is_current(&cache, &apk));
    }

    #[test]
    fn a_stamped_cache_is_current_until_the_apk_changes() {
        let dir = scratch("changed");
        let apk = dir.join("base.apk");
        std::fs::write(&apk, b"the old build").unwrap();
        let cache = dir.join("cache");
        write_stamp(&cache, &apk).unwrap();
        assert!(is_current(&cache, &apk), "nothing changed, so nothing should re-extract");

        // A new Roblox build arrives at the same path. This is the case that
        // used to leave the old engine in place and run it against the new
        // APK's assets.
        std::fs::write(&apk, b"a different build, and a different length").unwrap();
        assert!(!is_current(&cache, &apk));
    }

    #[test]
    fn a_different_apk_at_a_different_path_is_not_current() {
        // The user pointing Settings at another build has to re-extract even if
        // the two files happen to be the same size.
        let dir = scratch("path");
        let a = dir.join("a.apk");
        let b = dir.join("b.apk");
        std::fs::write(&a, b"same length!").unwrap();
        std::fs::write(&b, b"same length!").unwrap();
        let cache = dir.join("cache");
        write_stamp(&cache, &a).unwrap();
        assert!(is_current(&cache, &a));
        assert!(!is_current(&cache, &b));
    }

    #[test]
    fn an_apk_that_has_gone_away_is_not_current() {
        let dir = scratch("gone");
        let apk = dir.join("base.apk");
        std::fs::write(&apk, b"apk").unwrap();
        let cache = dir.join("cache");
        write_stamp(&cache, &apk).unwrap();
        std::fs::remove_file(&apk).unwrap();
        assert!(!is_current(&cache, &apk));
        assert!(write_stamp(&cache, &apk).is_err(), "and it cannot be stamped either");
    }

    #[test]
    fn a_cleared_stamp_forces_a_re_extraction() {
        let dir = scratch("cleared");
        let apk = dir.join("base.apk");
        std::fs::write(&apk, b"apk").unwrap();
        let cache = dir.join("cache");
        write_stamp(&cache, &apk).unwrap();
        clear_stamp(&cache);
        assert!(!is_current(&cache, &apk));
    }

    #[test]
    fn the_recorded_version_is_absent_rather_than_guessed() {
        let dir = scratch("version");
        assert_eq!(recorded_version(&dir), None);
        record_version(&dir, "0.732.23.7321040\n").unwrap();
        assert_eq!(recorded_version(&dir).as_deref(), Some("0.732.23.7321040"));
    }
}
