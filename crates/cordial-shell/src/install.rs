//! Where the Roblox build is, and how a launch gets it.
//!
//! **A launch resolves to a store entry, or to an explicit developer override,
//! and to nothing else** ([ADR-054](../../../docs/adr/ADR-054-cordial-owns-its-roblox-builds.md)).
//! It used to look in four places -- the environment, a path chosen in
//! Settings, Cordial's own download, and Sober's package directory -- and
//! follow whichever answered, so a machine with Sober silently ran whatever
//! Sober last fetched and never met the first-run screen. The sources now are:
//!
//! 1. `CORDIAL_APK`, the developer override: extracted into the old single
//!    slot, never filed, never updated, and labelled as such.
//! 2. The profile's pin, if it has one: that entry, or a refusal.
//! 3. **Latest**: the newest complete, signature-recorded entry in the store.
//!
//! Sober's directory and a file chosen in Settings are *imports* now (see
//! `cordial_update::provider::import` and [`crate::migration`]): the user asks,
//! the archives are verified and copied into the store, and the original is
//! forgotten.
//!
//! **Detection is a filesystem check every time, never a remembered answer.**
//! A stored "yes, it is installed" goes stale the moment the user deletes the
//! build, and a launcher that then fails with a path error is worse than one
//! that simply looks again. The store is read from disk at each launch.
//!
//! The override path keeps the old behaviour of stamping the extracted engine
//! with the APK it came from, because presence alone used to be the whole test
//! and a new build then left the old engine in the cache --
//! [`cordial_update::cache`] owns that and writes the same stamp `justfile`'s
//! `client` recipe does.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use cordial_update::store::{self, Choice, Entry, Refusal, Resolved};

/// The engine object. `--lib-dir` names the directory holding it.
pub const LIBRARY: &str = "libroblox.so";

/// Points this run at one APK without touching the saved settings or the
/// store. Set by `just dev --apk <path>`.
pub const APK_OVERRIDE: &str = "CORDIAL_APK";

/// Its path inside whichever APK carries it.
///
/// Taken from `cordial-update` rather than declared again here. It was declared
/// twice, privately, and two copies of one ABI string is how a port ends up
/// half done: the crate that extracts the engine and the crate that looks for
/// it would disagree, and nothing would say so.
use cordial_update::apk::LIBRARY_IN_APK;

/// The APK that used to be chosen in Settings.
///
/// **Legacy, and read exactly once.** It was a launch source that outranked
/// everything else; it is now an import, so [`crate::migration`] files the
/// named file in the store, pins every profile that had no pin to the result
/// (somebody who chose a file meant that file) and clears this field. Nothing
/// else reads it and nothing writes it: a Cordial that still found a value here
/// after the migration would be a launch source by another name.
///
/// There used to be a second field, `lib_dir`, and it is gone on purpose. An
/// existing `shell.json` that still carries `"lib_dir"` loads without
/// complaint (serde skips keys it does not know), and the next save drops it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RobloxInstall {
    pub apk: Option<PathBuf>,
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
    /// Nothing in the store. The first-run screen is the answer, not an error
    /// dialog -- this is a state rather than a fault, and it has a one-press way
    /// out.
    NoBuild,
    /// Something is there and cannot be run, or the override named a file that
    /// is unusable. Carries something specific enough to act on.
    Unusable(String),
}

/// Sober's copy of the official Android build, for the import.
///
/// Named rather than searched for, because the point is to be able to tell the
/// user exactly where Cordial looked. It is another application's private
/// directory, read by an import the user asked for and never written.
pub fn sober_apk() -> PathBuf {
    sober_apk_under(&std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir))
}

/// Split out from [`sober_apk`] so the path itself can be pinned by a test
/// without a test having to write to `HOME`, which is process-wide and would
/// interleave with every other test in this crate that reads it.
fn sober_apk_under(home: &Path) -> PathBuf {
    // Sober names this directory segment after the same Android ABI string
    // `cordial_update::apk::HOST_ABI` spells on x86_64: "x86_64". Not verified
    // for aarch64 -- Sober is a project this codebase may observe running but
    // never inspect (AGENTS.md) -- so this is INFERRED from the x86_64 naming
    // pattern, not confirmed against a real Sober install on ARM. See the same
    // caveat in `cordial_update::provider::import`.
    home.join(format!(
        ".var/app/org.vinegarhq.Sober/data/sober/packages/{}/com.roblox.client/base.apk",
        cordial_update::apk::HOST_ABI
    ))
}

/// Where Cordial keeps the engine it extracted for the override and for
/// `just client`: the old single slot. Same path `just dev` uses, so the two
/// never make each other re-extract 115 MB. A launch from the store does not
/// read it.
pub fn engine_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        // Must stay in step with `cordial_update::install::engine_dir`, which
        // is ABI-named for the same reason: two builds for two architectures
        // sharing one home directory must not share one engine cache.
        .join("cordial/lib")
        .join(cordial_update::apk::HOST_ABI)
}

/// The APK `CORDIAL_APK` names for this run, if it is set.
pub fn override_apk() -> Option<PathBuf> {
    std::env::var_os(APK_OVERRIDE).map(PathBuf::from)
}

/// Why updates are off, or `None` when they are on.
///
/// The one case left: `CORDIAL_APK` names the build for this run, so a download
/// would fill the store with a build the launch then declines to use -- a
/// success that changes nothing, which is worse than a failure that says so.
/// Everything else downloads into the store, and a profile on Latest follows it.
pub fn updates_blocked() -> Option<&'static str> {
    override_apk().map(|_| "CORDIAL_APK names the build for this run, so a download would not be used.")
}

/// The version a profile on Latest launches: the newest entry a launch may run.
/// `None` for an empty store, and while `CORDIAL_APK` overrides it, because the
/// store then says nothing about what runs.
pub fn installed_version() -> Option<String> {
    if override_apk().is_some() {
        return None;
    }
    store::latest(&store::list()).map(|e| e.version.clone())
}

/// What a profile's choice comes to on this machine, as a build.
///
/// `CORDIAL_APK` first, then the profile's pin, else Latest. Every entry a
/// launch may run has been checked for who signed it: filed by a route that
/// verified it, or checked here once, now, for an entry that predates the
/// records ([`cordial_update::store::ensure_verified`]).
pub fn resolve(profile_dir: &Path) -> Result<Build, NotFound> {
    if let Some(apk) = override_apk() {
        return locate_override(&apk);
    }
    verify_store();
    let entries = store::list();
    let pinned = cordial_shell::profile::pinned_version(profile_dir);
    let choice = match pinned.as_deref() {
        Some(v) => Choice::Pinned(v),
        None => Choice::Latest,
    };
    pick(&entries, choice)
}

/// [`resolve`]'s decision, over a given store listing so it can be tested
/// without a store.
fn pick(entries: &[Entry], choice: Choice<'_>) -> Result<Build, NotFound> {
    match store::resolve(choice, entries) {
        Resolved::Entry(version) => {
            let entry = entries.iter().find(|e| e.version == version).expect("resolved from this list");
            let apk = entry
                .base_apk()
                .ok_or_else(|| NotFound::Unusable(Refusal::Incomplete(version.clone()).to_string()))?;
            Ok(Build { apk, lib_dir: entry.dir.clone() })
        }
        Resolved::Refused(Refusal::Empty) => Err(NotFound::NoBuild),
        Resolved::Refused(refusal) => Err(NotFound::Unusable(refusal.to_string())),
    }
}

/// Check, once, every entry that has no record of who signed it.
///
/// The first launch after ADR-054 meets entries the old Version page and the
/// old updater filed without a record. Each is verified against the pinned
/// certificates and the answer is written down, so this costs one pass over a
/// build's archives (about half a second at 230 MB) once, on the main thread,
/// the same trade the old launch made for its extraction. An entry that fails
/// stays unrecorded and is never launched; the reason is said here.
fn verify_store() {
    for (version, verdict) in store::ensure_verified(&store::root(), &cordial_update::apk_signature::pinned()) {
        match verdict {
            Ok(fingerprint) => println!("  shell: checked Roblox {version}: signed by {fingerprint}"),
            Err(why) => println!("  shell: Roblox {version} was not launchable: {why}"),
        }
    }
}

// ---- the developer override ---------------------------------------------

/// Establish that Roblox signed this archive, at most once per build.
///
/// **The check belongs here because the override is the one launch path that
/// reaches an archive nothing else verified.** The store's entries were checked
/// when they were filed; `CORDIAL_APK` is a file somebody pointed at, and a
/// substituted archive would otherwise launch exactly like a genuine one.
/// Reported by @kanqz; issue #51.
///
/// **Recorded rather than repeated**, because verifying digests the whole
/// archive and `cordial_update::cache`'s own header argues against paying that
/// on every launch.
///
/// A refusal names which of the two failures happened, because
/// [`cordial_update::apk_signature::Refusal`] distinguishes "somebody changed
/// this file" from "this is intact and is not Roblox's", and collapsing them
/// into one shrug is what that type exists to prevent.
///
/// Returns the fingerprint when it had to verify, for [`locate_with`] to
/// record once the engine directory has settled. Recording it here, before an
/// extraction replaces that directory, wrote it into the build being replaced.
/// The fingerprint comes back with the archive's stamp as it was *before* the
/// digest, so what is recorded describes the bytes that were checked.
fn verified_once(apk: &Path, cache: &Path) -> Result<Option<(String, String)>, NotFound> {
    let pinned = cordial_update::apk_signature::pinned();
    if let Some(known) = cordial_update::cache::recorded_signer(cache, apk) {
        if pinned.iter().any(|p| p.eq_ignore_ascii_case(&known)) {
            return Ok(None);
        }
    }
    let stamp = cordial_update::cache::stamp_for(apk);
    match cordial_update::apk_signature::verify_signed_by(apk, &pinned) {
        Ok(signer) => Ok(stamp.map(|s| (signer.certificate_sha256, s))),
        Err(e) => Err(NotFound::Unusable(format!(
            "Cordial will not run {}: {e}.\n\nThis is the archive CORDIAL_APK named, not one \
             Cordial downloaded. Unset CORDIAL_APK to launch from Cordial's own store.",
            apk.display()
        ))),
    }
}

/// Run the archive `CORDIAL_APK` names: extract its engine into the old slot
/// and launch from there. **Never filed in the store** and never updated.
pub fn locate_override(apk: &Path) -> Result<Build, NotFound> {
    locate_with(apk, verified_once)
}

/// [`locate_override`], with the signature check injected.
///
/// **The seam exists so the extraction tests remain testable.** They drive real
/// behaviour worth keeping -- that a new Roblox build at the same path
/// re-extracts, and that an unchanged one does not -- against archives built by
/// `apk_holding`, which are plain zips. Gating unconditionally would make every
/// one of them unrunnable, and the way out is not to fabricate a signed
/// archive: `cordial_update::apk_signature`'s own tests explain that one
/// signed by an invented key exercises the parser and proves nothing about
/// whether Roblox's real build is accepted.
///
/// So tests inject a verifier that accepts, and a separate test drives the real
/// entry point above to prove an unsigned archive is refused.
fn locate_with(
    apk: &Path,
    verify: impl FnOnce(&Path, &Path) -> Result<Option<(String, String)>, NotFound>,
) -> Result<Build, NotFound> {
    if !apk.is_file() {
        return Err(NotFound::Unusable(format!(
            "CORDIAL_APK names {}, and there is no file there.",
            apk.display()
        )));
    }
    let fresh_signer = verify(apk, &engine_cache())?;
    let build = locate_verified(apk.to_path_buf())?;
    // Not fatal if it cannot be written: the cost is verifying again next
    // launch, which is slow rather than wrong.
    if let Some((fingerprint, stamp)) = fresh_signer {
        if let Err(e) = cordial_update::cache::record_signer_stamped(&engine_cache(), &fingerprint, &stamp) {
            println!("  shell: verified {} but could not record it: {e}", build.apk.display());
        }
    }
    Ok(build)
}

/// The rest of [`locate_with`], once the archive's signature is established.
fn locate_verified(apk: PathBuf) -> Result<Build, NotFound> {
    // Beside the APK, which is where it lands if you unzip in place.
    if let Some(beside) = apk.parent().map(|d| d.join("lib").join(cordial_update::apk::HOST_ABI)) {
        if beside.join(LIBRARY).is_file() {
            return Ok(Build { apk, lib_dir: beside });
        }
    }

    // The cache is the only location here whose contents Cordial put there, so
    // it is the only one it can vouch for -- and only against the APK it was
    // extracted from. An unstamped cache counts as stale.
    //
    // The stale engine is deliberately *not* deleted first. Extraction writes a
    // temporary and renames over it, so there is nothing to clear, and deleting
    // up front would leave a user with no engine at all if the extraction then
    // failed.
    let cache = engine_cache();
    let stale = cache.join(LIBRARY).is_file() && !cordial_update::cache::is_current(&cache, &apk);
    if stale {
        println!(
            "  shell: {} was extracted from a different {}; re-extracting",
            cache.display(),
            apk.display()
        );
    } else if cache.join(LIBRARY).is_file() {
        return Ok(Build { apk, lib_dir: cache });
    }

    // **Detached first, and not filed.** The cache path is a symlink into the
    // store whenever an update has pointed it at a kept build, and extracting
    // "into the cache" through it would write inside whichever build that is.
    // `detach` breaks the link and leaves a real, empty directory. It does
    // *not* key the outgoing engine into the store, which the launch used to
    // do: the override is for one run and leaves no entry behind.
    if let Err(e) = store::detach(&cache) {
        return Err(NotFound::Unusable(format!("{}: {e}", cache.display())));
    }

    match extract_engine(&apk, &cache) {
        Ok(from) => {
            // Stamped only once the engine is on disk. A stamp written first
            // and an extraction that then failed would claim a cache that is
            // not there, which is the same class of lie in the other direction.
            if let Err(e) = cordial_update::cache::write_stamp(&cache, &apk) {
                println!("  shell: extracted {LIBRARY} but could not stamp the cache: {e}");
            }
            // The extracted engine, not the archive `from` names: scanning the
            // archive finds nothing -- measured, `split_config.x86_64.apk`
            // gives `None` where the engine out of it gives `2.738.0.1397`.
            match cordial_update::engine::version_of(&cache.join(LIBRARY)) {
                Some(version) => {
                    if let Err(e) = cordial_update::cache::record_version(&cache, &version) {
                        println!("  shell: extracted {LIBRARY} but could not record its version: {e}");
                    }
                }
                None => println!("  shell: could not read a version out of the extracted {LIBRARY}"),
            }
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

/// Pull [`LIBRARY_IN_APK`] (`lib/x86_64/libroblox.so` on x86-64,
/// `lib/arm64-v8a/libroblox.so` on aarch64) out of the first archive that has
/// it.
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
         On a split build the engine is in {}, not base.apk, so that file has to sit \
         in the same folder as base.apk.",
        apk.display(),
        tried.len(),
        cordial_update::install::SPLIT_APK
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

    /// `CORDIAL_APK` is process-wide, so the tests that care about it have to be
    /// kept apart from each other. Same reasoning as `profile`'s own ENV mutex,
    /// and the same reason: cargo runs these as threads of one process.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// An entry a launch may run, written to disk so `base_apk` finds a file.
    fn kept(root: &Path, version: &str, signed: bool, with_apk: bool) -> Entry {
        let dir = root.join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LIBRARY), version).unwrap();
        if with_apk {
            std::fs::write(dir.join("base.apk"), format!("apk {version}")).unwrap();
            if signed {
                store::record_signer(&dir, &"ab".repeat(32)).unwrap();
            }
        }
        store::list_in(root).into_iter().find(|e| e.version == version).unwrap()
    }

    /// The decision a launch makes, over entries on disk: Latest is the newest
    /// build a launch may run, and a build that is newer but unchecked or
    /// without its archive does not take its place.
    #[test]
    fn latest_is_the_newest_build_that_can_be_run_and_the_pin_is_exact() {
        let dir = scratch("pick");
        let root = dir.join("builds");
        let entries = vec![
            kept(&root, "2.740.0.5", false, true),
            kept(&root, "2.739.0.1", true, false),
            kept(&root, "2.738.0.1397", true, true),
            kept(&root, "2.736.0.1408", true, true),
        ];
        let latest = pick(&entries, Choice::Latest).unwrap();
        assert_eq!(latest.lib_dir, root.join("2.738.0.1397"));
        assert_eq!(latest.apk, root.join("2.738.0.1397/base.apk"));
        let pinned = pick(&entries, Choice::Pinned("2.736.0.1408")).unwrap();
        assert_eq!(pinned.lib_dir, root.join("2.736.0.1408"));

        // A pin refuses and names itself; it never becomes Latest.
        for (pin, needle) in [("2.700.0.1", "not in Cordial's store"), ("2.739.0.1", "without the APK"), ("2.740.0.5", "who signed")] {
            match pick(&entries, Choice::Pinned(pin)) {
                Err(NotFound::Unusable(msg)) => assert!(msg.contains(needle), "{pin}: {msg}"),
                other => panic!("{pin}: expected a refusal, got {other:?}"),
            }
        }
    }

    /// An empty store is the first-run state and not an error.
    #[test]
    fn an_empty_store_is_the_first_run_screen_and_an_unusable_one_is_a_message() {
        assert!(matches!(pick(&[], Choice::Latest), Err(NotFound::NoBuild)));
        let dir = scratch("pick-unusable");
        let root = dir.join("builds");
        let entries = vec![kept(&root, "2.740.0.5", false, true)];
        assert!(matches!(pick(&entries, Choice::Latest), Err(NotFound::Unusable(_))));
    }

    /// **The point of ADR-054, as a test.** Nothing but the store and
    /// `CORDIAL_APK` is a launch source: with Sober's file in its real place
    /// and a Settings APK saved, a launch still finds an empty store and says
    /// so.
    #[test]
    fn sober_and_a_saved_apk_are_not_launch_sources() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("not-sources");
        let (data, home) = (dir.join("data"), dir.join("home"));
        let sober = sober_apk_under(&home);
        std::fs::create_dir_all(sober.parent().unwrap()).unwrap();
        std::fs::write(&sober, b"sober's base.apk").unwrap();
        let previous = (std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"));
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("HOME", &home);
        let outcome = resolve(&data.join("cordial/profiles/default"));
        for (k, v) in [("XDG_DATA_HOME", previous.0), ("HOME", previous.1)] {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        assert!(matches!(outcome, Err(NotFound::NoBuild)), "{outcome:?}");
        assert_eq!(std::fs::read(&sober).unwrap(), b"sober's base.apk", "and it was not touched");
    }

    #[test]
    fn only_the_environment_override_turns_updates_off() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        assert!(updates_blocked().is_none());
        std::env::set_var(APK_OVERRIDE, "/from/env/base.apk");
        let why = updates_blocked();
        let installed = installed_version();
        std::env::remove_var(APK_OVERRIDE);
        assert!(why.unwrap().contains("CORDIAL_APK"));
        assert_eq!(installed, None, "the store says nothing about what an override runs");
    }

    /// An old `shell.json` that still names an engine directory must load, and
    /// the value must go nowhere. See [`RobloxInstall`].
    #[test]
    fn a_saved_engine_directory_is_ignored_and_does_not_break_loading() {
        let old = r#"{"apk": "/chosen/base.apk", "lib_dir": "/old/lib/x86_64"}"#;
        let loaded: RobloxInstall = serde_json::from_str(old).unwrap();
        assert_eq!(loaded.apk, Some(PathBuf::from("/chosen/base.apk")));
        assert!(!serde_json::to_string(&loaded).unwrap().contains("lib_dir"));
    }

    #[test]
    fn the_split_apk_is_tried_after_the_one_it_was_given() {
        // The engine is not in base.apk on a split build. Asserting otherwise
        // is the mistake this ordering exists to stop, so the order is pinned.
        let dir = scratch("candidates");
        let split_name = cordial_update::install::SPLIT_APK;
        for name in ["base.apk", split_name, "split_config.en.apk"] {
            std::fs::write(dir.join(name), b"not really a zip").unwrap();
        }
        let candidates = engine_candidates(&dir.join("base.apk"));
        assert_eq!(candidates[0], dir.join("base.apk"));
        assert!(candidates.contains(&dir.join(split_name)));
    }

    #[test]
    fn the_detected_location_is_the_one_the_justfile_documents() {
        // `just dev` prints this path to anyone who has no build, and the two
        // must not drift: a user told to look in one place while the shell
        // looks in another has no way to tell which is wrong.
        let p = sober_apk_under(Path::new("/home/someone"));
        assert_eq!(
            p,
            Path::new(&format!(
                "/home/someone/.var/app/org.vinegarhq.Sober/data/sober/packages/{}/com.roblox.client/base.apk",
                cordial_update::apk::HOST_ABI
            ))
        );
    }

    #[test]
    fn an_override_that_has_gone_away_is_reported_rather_than_ignored() {
        let dir = scratch("stale");
        match locate_override(&dir.join("gone.apk")) {
            Err(NotFound::Unusable(msg)) => assert!(msg.contains("CORDIAL_APK"), "{msg}"),
            other => panic!("expected a usable message, got {other:?}"),
        }
    }

    /// An APK-shaped zip whose engine is `engine`.
    fn apk_holding(engine: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file(LIBRARY_IN_APK, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(engine).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn a_new_roblox_build_re_extracts_rather_than_running_the_old_engine() {
        // The defect this fixes, end to end, on the override path. Presence
        // alone was the whole test, so a new build left the OLD engine in the
        // cache and Cordial ran it against the new APK's assets. Delete the
        // `is_current` call in `locate_verified` and the second assertion fails.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("restamp");
        let previous = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));

        let apk = dir.join("base.apk");
        std::fs::write(&apk, apk_holding(b"the old engine")).unwrap();
        let first = locate_with(&apk, |_, _| Ok(None)).unwrap();
        assert_eq!(std::fs::read(first.lib_dir.join(LIBRARY)).unwrap(), b"the old engine");

        std::fs::write(&apk, apk_holding(b"the new engine, which is longer")).unwrap();
        let second = locate_with(&apk, |_, _| Ok(None)).unwrap();
        let got = std::fs::read(second.lib_dir.join(LIBRARY)).unwrap();

        match previous {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        assert_eq!(got, b"the new engine, which is longer", "the cache must follow the APK it was extracted from");
    }

    #[test]
    fn an_unchanged_apk_does_not_re_extract() {
        // The control for the test above. Re-extracting every launch would be
        // 115 MB of pointless work.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("unchanged");
        let previous = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));

        let apk = dir.join("base.apk");
        std::fs::write(&apk, apk_holding(b"the engine")).unwrap();
        let build = locate_with(&apk, |_, _| Ok(None)).unwrap();
        // Something no extraction would ever produce, so its survival is proof
        // the second call did not extract.
        std::fs::write(build.lib_dir.join(LIBRARY), b"left alone").unwrap();
        let again = locate_with(&apk, |_, _| Ok(None)).unwrap();
        let got = std::fs::read(again.lib_dir.join(LIBRARY)).unwrap();

        match previous {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        assert_eq!(got, b"left alone", "an unchanged APK must not re-extract");
    }

    /// **The override is never filed.** Extracting through the slot while it is
    /// a link into a kept build must break the link and leave that build
    /// alone, and the store must have no new entry afterwards.
    #[test]
    fn the_override_extracts_beside_the_store_and_files_nothing() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("override-unfiled");
        let (cache, data) = (dir.join("cache"), dir.join("data"));
        let previous = (std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("XDG_DATA_HOME"));
        std::env::set_var("XDG_CACHE_HOME", &cache);
        std::env::set_var("XDG_DATA_HOME", &data);

        let root = store::root();
        let entry = kept(&root, "2.738.0.1397", true, true);
        store::point_current_at(&engine_cache(), &entry.dir).unwrap();

        let apk = dir.join("base.apk");
        std::fs::write(&apk, apk_holding(b"an engine for this run only")).unwrap();
        let build = locate_with(&apk, |_, _| Ok(None)).unwrap();

        let after = store::list();
        let kept_bytes = std::fs::read(entry.dir.join(LIBRARY)).unwrap();
        let slot_is_link = std::fs::symlink_metadata(engine_cache()).unwrap().file_type().is_symlink();
        for (k, v) in [("XDG_CACHE_HOME", previous.0), ("XDG_DATA_HOME", previous.1)] {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        assert_eq!(std::fs::read(build.lib_dir.join(LIBRARY)).unwrap(), b"an engine for this run only");
        assert_eq!(after.len(), 1, "nothing was filed");
        assert_eq!(kept_bytes, b"2.738.0.1397", "the kept build was not written through the slot");
        assert!(!slot_is_link, "the link was broken, not followed");
    }

    #[test]
    fn an_archive_without_the_engine_says_where_it_looked() {
        let dir = scratch("noengine");
        std::fs::write(dir.join("base.apk"), b"not really a zip").unwrap();
        let err = extract_engine(&dir.join("base.apk"), &dir.join("cache")).unwrap_err();
        assert!(err.contains("split_config"), "{err}");
    }

    /// **The regression guard for issue #51**, reported by @kanqz: a build the
    /// launcher was merely pointed at used to reach the loader without anything
    /// asking whose signature was on it. On the override path that is still a
    /// file somebody named, so the real entry point must refuse an unsigned one.
    ///
    /// It drives the real entry point rather than the seam, so deleting the
    /// check fails here. `apk_holding` produces a plain zip with no signing
    /// block, exactly the shape of a substituted APK.
    #[test]
    fn an_unsigned_override_is_refused_by_the_real_entry_point() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("unsigned");
        let previous = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));

        let apk = dir.join("base.apk");
        std::fs::write(&apk, apk_holding(b"an engine nobody signed")).unwrap();
        let refused = locate_override(&apk);

        match previous {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        match refused {
            Err(NotFound::Unusable(msg)) => assert!(msg.contains("no APK signing block"), "{msg}"),
            other => panic!("an unsigned archive must not reach the loader, got {other:?}"),
        }
    }
}
