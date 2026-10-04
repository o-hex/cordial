//! Filing a Roblox build into the store, and where the old layout's pieces are.
//!
//! [ADR-054](../../../docs/adr/ADR-054-cordial-owns-its-roblox-builds.md) made
//! the store the only place a launch gets a build from, and everything that
//! brings a build in -- the mirror download, an import of Sober's copy or a
//! chosen file, the first-launch migration -- files it with
//! [`file_into_store`]. This module used to be the *other* road, which swapped
//! archives into `build/<abi>/` and the engine into the single slot
//! `lib/<abi>` and then keyed whatever it had made current into the store
//! afterwards. That path (`adopt`) and the ownership marker that guarded it are
//! gone; what is left is the part both roads shared and the paths the old
//! layout used, which the migration still reads.
//!
//! ## Both halves, and the trap
//!
//! **`base.apk` does not contain the engine.** On a split build `libroblox.so`
//! is in `split_config.x86_64.apk` beside it, and anything that fetches "the
//! APK" and stops has fetched the half without the engine in it. Both names are
//! constants here, and [`file_into_store`] refuses if no archive it was given
//! holds [`apk::LIBRARY_IN_APK`] -- naming the split, because that refusal is
//! the one somebody will meet and the sentence they need is which file is
//! missing. Assets come out of `base.apk` at runtime and the engine comes out
//! of the split, so both are kept rather than the engine being extracted and
//! the archives thrown away.
//!
//! ## Nothing enters the store until it is whole
//!
//! The worst failure this feature can produce is somebody losing the client
//! they had because a download was interrupted. So the engine and the archives
//! are gathered in a hidden directory, every archive is checked against
//! ADR-014's refusals first, and the directory is renamed to the version only
//! when it is complete. A killed filing leaves a name `store::list_in` skips,
//! never an entry holding half a build, and no existing entry is touched.

use crate::apk;
use crate::cache;
use crate::engine;
use crate::store;
use std::fmt;
use std::path::{Path, PathBuf};

/// The archive the assets come out of.
pub const BASE_APK: &str = "base.apk";

/// The archive the engine comes out of on a split build, which is every build
/// this has been run against.
///
/// **Underscore, not hyphen.** Play names the split for an ABI with the ABI's
/// characters normalised -- `split_config.arm64_v8a.apk` -- while the directory
/// inside the archive keeps the hyphen, `lib/arm64-v8a/`. The two spellings of
/// one ABI sit four lines apart in this crate for that reason.
#[cfg(target_arch = "x86_64")]
pub const SPLIT_APK: &str = "split_config.x86_64.apk";
#[cfg(target_arch = "aarch64")]
pub const SPLIT_APK: &str = "split_config.arm64_v8a.apk";

/// Where the store is, and what a garbage collection after an update must keep.
///
/// Owned rather than borrowed, which is the whole reason this is a struct and
/// not two parameters: every caller is a closure moved onto a worker thread,
/// and borrowed arguments would each need a binding outside the closure and a
/// `move` that captured it. One owned value is a line.
///
/// **`protect` is the caller's job because the caller is the only one who can
/// know it.** Which versions are pinned lives in the profiles, under
/// `$XDG_DATA_HOME`, and this crate has no business reading them --
/// `cordial-shell` collects them and hands them over. A collection that took a
/// pinned build would turn somebody's deliberate choice into a launch failure
/// with nothing to explain it.
#[derive(Debug, Clone)]
pub struct Store {
    /// `$XDG_DATA_HOME/cordial/builds` in production.
    pub root: PathBuf,
    /// Versions no collection may take, whatever their age.
    pub protect: Vec<String>,
}

impl Store {
    /// The real store, holding `protect` safe from a collection.
    pub fn live(protect: Vec<String>) -> Self {
        Store { root: crate::store::root(), protect }
    }
}

/// `$XDG_CACHE_HOME/cordial`, or `~/.cache/cordial`.
pub fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial")
}

/// The directory the old layout installed a download into:
/// `~/.cache/cordial/build/<abi>`. Legacy: see [`managed_base`].
pub fn build_dir() -> PathBuf {
    cache_root().join("build").join(crate::apk::HOST_ABI)
}

/// The old single slot: `~/.cache/cordial/lib/<abi>`.
///
/// A launch no longer reads it. It is kept as a link to the newest entry for
/// `just client` and hand-typed `--lib-dir`, and holds the engine the
/// `CORDIAL_APK` override extracts. `cordial-shell`'s `install::engine_cache`
/// computes the same path and `justfile` writes the same string into it.
pub fn engine_dir() -> PathBuf {
    cache_root().join("lib").join(crate::apk::HOST_ABI)
}

/// The managed `base.apk` of the old layout, if there is one.
///
/// **Legacy, read by the migration only.** Before ADR-054 a Cordial download
/// was installed into `build/<abi>/` and keyed into the store afterwards; a
/// build there that no entry corresponds to is something the migration files.
/// Nothing is written here any more.
pub fn managed_base() -> Option<PathBuf> {
    managed_base_in(&build_dir())
}

pub fn managed_base_in(dir: &Path) -> Option<PathBuf> {
    let base = dir.join(BASE_APK);
    base.is_file().then_some(base)
}

/// Why filing a build did not finish. Every one of these leaves the store as
/// it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failed {
    /// Something arrived and was not an archive Cordial will unpack.
    Archive(apk::Refusal),
    /// The download succeeded and the engine is not in any of it. The one that
    /// catches the two-APK trap.
    NoEngine { fetched: Vec<String> },
    Io { path: String, why: String },
    /// The store already keeps this version and its engine is not these bytes.
    ///
    /// Two byte-different engines claiming one version is the one thing the
    /// store must not paper over (ADR-054): whichever was kept first would be
    /// silently replaced, or silently kept in preference to the one just
    /// verified.
    Conflict { version: String },
    /// The caller asked to stop, and was still owed an answer for it.
    ///
    /// Only reachable before the entry is renamed into place: once it is, the
    /// alternative to finishing is an entry half there.
    Cancelled,
}

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failed::Archive(r) => write!(f, "{r}"),
            Failed::NoEngine { fetched } => write!(
                f,
                "None of the downloaded files contains the Roblox engine ({}). \
                 It lives in {SPLIT_APK}, not {BASE_APK}.",
                fetched.join(", ")
            ),
            Failed::Conflict { version } => write!(
                f,
                "Cordial already keeps Roblox {version}, and the engine in this download is not \
                 the same bytes. It will not keep two different engines under one version. If \
                 you want to replace the kept one, remove it in Settings first."
            ),
            Failed::Io { path, why } => write!(f, "{path}: {why}"),
            Failed::Cancelled => write!(f, "the install was stopped before anything was replaced"),
        }
    }
}

impl std::error::Error for Failed {}

impl From<apk::Refusal> for Failed {
    fn from(r: apk::Refusal) -> Self {
        Failed::Archive(r)
    }
}

/// Keep a build in the store without making it the build in use.
///
/// The Version page's download. [`adopt`] is the wrong tool for it, because
/// everything there replaces the live build, and somebody fetching an older
/// Roblox to try it wants the one they launch every day left where it is. So
/// nothing here writes outside `root`. The engine and the archives are gathered
/// in a hidden directory and that directory is renamed to the version, so a
/// download killed part way leaves a name `store::list_in` skips rather than an
/// entry holding half a build.
///
/// `fetched` must already have passed the signature check; `provider` is the
/// caller and does that first. An archive already under `root` -- the mirror's
/// staging directory -- is moved, and anything else is copied, for `adopt`'s
/// reason: a file outside Cordial's directories belongs to somebody else.
///
/// Returns the version read out of the engine, which is the store's key.
pub fn file_into_store(
    fetched: &[(&'static str, PathBuf)],
    root: &Path,
    filing: &store::Filing,
    cancel: &crate::provider::Cancel,
) -> Result<String, Failed> {
    for (_, path) in fetched {
        apk::inspect(path, apk::Limits::default())?;
    }
    let mut carrier = None;
    for (_, path) in fetched {
        if apk::holds(path, apk::LIBRARY_IN_APK)? {
            carrier = Some(path.clone());
            break;
        }
    }
    let Some(carrier) = carrier else {
        return Err(Failed::NoEngine { fetched: fetched.iter().map(|(n, _)| n.to_string()).collect() });
    };
    if cancel.stopped() {
        return Err(Failed::Cancelled);
    }

    let io = |path: &Path, e: std::io::Error| Failed::Io { path: path.display().to_string(), why: e.to_string() };
    let land = |from: &Path, to: &Path| -> Result<(), Failed> {
        if from.starts_with(root) {
            std::fs::rename(from, to).map_err(|e| io(to, e))
        } else {
            std::fs::copy(from, to).map(|_| ()).map_err(|e| io(to, e))
        }
    };

    let gathering = root.join(format!(".filing.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&gathering);
    std::fs::create_dir_all(&gathering).map_err(|e| io(&gathering, e))?;

    let result = (|| -> Result<String, Failed> {
        let engine = apk::extract(&carrier, apk::LIBRARY_IN_APK, &gathering)?;
        let version = engine::version_of(&engine).filter(|v| store::is_valid_version(v)).ok_or_else(|| {
            Failed::Io {
                path: engine.display().to_string(),
                why: "the engine carries no version Cordial can read, so it cannot be kept under one".into(),
            }
        })?;
        if cancel.stopped() {
            return Err(Failed::Cancelled);
        }
        let incoming = store::hash_file(&engine).map_err(|e| io(&engine, e))?;

        // This does not go through `store::adopt_current`, so it is the one
        // caller that takes the store lock directly rather than getting it for
        // free -- see [ADR-037](../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md).
        // Held from here so both branches below are covered.
        let _lock = store::lock(root).map_err(|e| io(root, e))?;

        // **The engine's bytes decide which entry this is, then the version.**
        // An entry with these exact bytes is this build whatever it is called
        // (ADR-037's left-open `find_by_content_hash`); otherwise the name is
        // the version the engine reports, and an entry already under that name
        // with different bytes is refused rather than replaced or kept in
        // preference (ADR-054).
        let entry = match store::find_by_content_hash(root, &incoming) {
            Some(kept) => kept.dir,
            None => store::entry_dir_in(root, &version).expect("checked by is_valid_version above"),
        };
        let version = entry.file_name().and_then(|n| n.to_str()).unwrap_or(&version).to_string();

        if entry.join(engine::LIBRARY).is_file() {
            if store::ensure_content_hash(&entry).as_ref() != Some(&incoming) {
                return Err(Failed::Conflict { version });
            }
            // Already kept, and byte for byte this engine. Only the archives an
            // entry kept without them is missing are added; an entry that
            // already holds its own `base.apk` is not re-vouched for with a
            // signature taken from a different file.
            let had_base = entry.join(BASE_APK).is_file();
            for (name, path) in fetched {
                let target = entry.join(name);
                if !target.exists() {
                    land(path, &target)?;
                }
            }
            if !had_base {
                store::write_records(&entry, filing).map_err(|e| io(&entry, e))?;
            } else {
                let _ = store::record_source(&entry, filing.source);
            }
            return Ok(version);
        }

        for (name, path) in fetched {
            land(path, &gathering.join(name))?;
        }
        cache::record_version(&gathering, &version).map_err(|e| io(&gathering, e))?;
        // Records go in with the entry rather than after it, so there is no
        // moment at which an entry exists that a launch would call unchecked.
        // Written against the kept `base.apk` in `gathering`; a rename does
        // not change size or mtime, so the record survives it.
        store::write_records(&gathering, filing).map_err(|e| io(&gathering, e))?;
        // A directory under the version's name with no engine in it is what a
        // killed install leaves, and `list_in` already ignores it; it is in
        // the way of the rename and holds nothing worth keeping.
        let _ = std::fs::remove_dir_all(&entry);
        std::fs::rename(&gathering, &entry).map_err(|e| io(&entry, e))?;
        store::ensure_content_hash(&entry);
        Ok(version)
    })();
    let _ = std::fs::remove_dir_all(&gathering);
    result
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-update-install-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A zip holding whatever entries are named. Not an APK and not a Roblox
    /// byte: the entry path is what this code acts on, and the contents are
    /// whatever this test wrote.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn filing() -> store::Filing {
        store::Filing { source: store::Source::Mirror, signer: "ab".repeat(32), version_code: Some(7) }
    }

    /// A `Cancel` nobody has stopped, for tests that are not about stopping.
    fn no_cancel() -> crate::provider::Cancel {
        crate::provider::Cancel::new()
    }

    /// The engine literal this crate's own scanner looks for, in a file this
    /// test wrote. Nothing here comes from Roblox.
    fn engine_bytes(version: &str) -> Vec<u8> {
        let mut v = b"\0not an engine, just the shape of one\0".to_vec();
        v.extend_from_slice(version.as_bytes());
        v.push(0);
        v
    }

    /// An engine that scans as `version` and differs from `engine_bytes`
    /// by a salt: the same version, other bytes.
    fn engine_with(version: &str, salt: &[u8]) -> Vec<u8> {
        let mut v = engine_bytes(version);
        v.extend_from_slice(salt);
        v
    }

    fn stage(root: &Path, name: &str, engine: &[u8], extra: &[u8]) -> PathBuf {
        let staging = root.join(".fetching");
        std::fs::create_dir_all(&staging).unwrap();
        let path = staging.join(name);
        std::fs::write(&path, zip_of(&[("assets/x.json", extra), (apk::LIBRARY_IN_APK, engine)])).unwrap();
        path
    }

    /// A build filed beside the one already kept: it comes out whole, the one
    /// kept is byte for byte as it was, and no gathering directory is left.
    #[test]
    fn filing_a_build_keeps_it_beside_the_one_in_use_and_touches_nothing_else() {
        let dir = scratch("file-into-store");
        let root = dir.join("builds");
        let staging = root.join(".fetching");
        std::fs::create_dir_all(&staging).unwrap();
        let in_use = root.join("2.738.0.1397");
        std::fs::create_dir_all(&in_use).unwrap();
        std::fs::write(in_use.join(engine::LIBRARY), b"the build in use").unwrap();

        let base = staging.join("candidate-0.apk");
        let engine = engine_bytes("2.730.0.790");
        std::fs::write(
            &base,
            zip_of(&[("assets/x.json", b"{}" as &[u8]), (apk::LIBRARY_IN_APK, engine.as_slice())]),
        )
        .unwrap();

        let version = file_into_store(&[(BASE_APK, base.clone())], &root, &filing(), &no_cancel()).expect("filed");
        assert_eq!(version, "2.730.0.790");
        let entries = store::list_in(&root);
        let names: Vec<&str> = entries.iter().map(|e| e.version.as_str()).collect();
        assert_eq!(names, ["2.738.0.1397", "2.730.0.790"]);
        assert!(entries[1].complete, "kept with its APK, so it can be chosen");
        assert_eq!(std::fs::read(in_use.join(engine::LIBRARY)).unwrap(), b"the build in use");
        assert!(!base.exists(), "moved out of staging rather than copied");
        let stray = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(".filing"));
        assert!(!stray, "the gathering directory is not left behind");
    }

    /// Every filing route writes the proof; this is the mirror's.
    #[test]
    fn a_filed_entry_proves_who_signed_it_and_how_it_arrived() {
        let dir = scratch("file-records");
        let root = dir.join("builds");
        let base = stage(&root, "a.apk", &engine_bytes("2.730.0.790"), b"{}");
        file_into_store(&[(BASE_APK, base)], &root, &filing(), &no_cancel()).unwrap();
        let entry = &store::list_in(&root)[0];
        assert_eq!(entry.signer.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(entry.provenance.map(|p| p.source), Some(store::Source::Mirror));
        assert_eq!(entry.version_code, Some(7));
        assert!(entry.launchable());
        assert!(entry.content_hash.is_some());
    }

    /// The control is the next test: the same engine in another container is
    /// fine, so this refusal is about the bytes and not about the version.
    #[test]
    fn a_second_engine_under_one_version_is_refused_by_name_and_the_first_is_kept() {
        let dir = scratch("file-conflict");
        let root = dir.join("builds");
        let first = stage(&root, "a.apk", &engine_bytes("2.730.0.790"), b"{}");
        file_into_store(&[(BASE_APK, first)], &root, &filing(), &no_cancel()).unwrap();
        let before = store::list_in(&root)[0].content_hash.clone();

        let other = stage(&root, "b.apk", &engine_with("2.730.0.790", b"tampered"), b"{}");
        let refused = file_into_store(&[(BASE_APK, other)], &root, &filing(), &no_cancel()).unwrap_err();
        assert!(matches!(&refused, Failed::Conflict { version } if version == "2.730.0.790"), "{refused:?}");
        assert!(refused.to_string().contains("2.730.0.790"));
        let after = store::list_in(&root);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].content_hash, before, "the kept engine is untouched");
    }

    #[test]
    fn the_same_engine_in_another_container_is_the_same_entry() {
        let dir = scratch("file-same");
        let root = dir.join("builds");
        let engine = engine_bytes("2.730.0.790");
        let first = stage(&root, "a.apk", &engine, b"{}");
        file_into_store(&[(BASE_APK, first)], &root, &filing(), &no_cancel()).unwrap();
        let signer = store::list_in(&root)[0].signer.clone();

        // A monolithic archive from another distributor: other bytes outside
        // the engine, the identical engine inside.
        let second = stage(&root, "b.apk", &engine, b"{\"another\": \"container\"}");
        let mut other = filing();
        other.signer = "cd".repeat(32);
        assert_eq!(file_into_store(&[(BASE_APK, second)], &root, &other, &no_cancel()).unwrap(), "2.730.0.790");
        let entries = store::list_in(&root);
        assert_eq!(entries.len(), 1, "linked, not duplicated");
        assert_eq!(entries[0].signer, signer, "an entry that holds its own archive is not re-vouched for");
    }

    /// The mistake this module exists to make impossible. `base.apk` alone
    /// downloads perfectly, verifies perfectly, and has no engine in it.
    #[test]
    fn filing_only_base_apk_is_refused_and_the_refusal_names_the_split() {
        let dir = scratch("halfway");
        let root = dir.join("builds");
        let base = stage_without_engine(&root);
        let e = file_into_store(&[(BASE_APK, base)], &root, &filing(), &no_cancel()).unwrap_err();
        assert!(matches!(e, Failed::NoEngine { .. }), "{e}");
        assert!(e.to_string().contains(SPLIT_APK), "{e}");
        assert!(store::list_in(&root).is_empty());
    }

    fn stage_without_engine(root: &Path) -> PathBuf {
        let staging = root.join(".fetching");
        std::fs::create_dir_all(&staging).unwrap();
        let path = staging.join("base.apk");
        std::fs::write(&path, zip_of(&[("assets/content/fonts/x.json", b"{}")])).unwrap();
        path
    }

    /// A split build files both halves, the engine out of the split.
    #[test]
    fn a_split_build_files_both_halves_and_reads_the_engine_out_of_the_split() {
        let dir = scratch("split");
        let root = dir.join("builds");
        let staging = root.join(".fetching");
        std::fs::create_dir_all(&staging).unwrap();
        let base = staging.join("base.apk");
        let split = staging.join("split.apk");
        std::fs::write(&base, zip_of(&[("assets/content/fonts/x.json", b"{}")])).unwrap();
        std::fs::write(&split, zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917"))])).unwrap();

        let version =
            file_into_store(&[(BASE_APK, base), (SPLIT_APK, split)], &root, &filing(), &no_cancel()).unwrap();
        assert_eq!(version, "2.734.0.917");
        let entry = root.join("2.734.0.917");
        assert!(entry.join(BASE_APK).is_file());
        assert!(entry.join(SPLIT_APK).is_file());
        assert!(entry.join(engine::LIBRARY).is_file());
    }

    #[test]
    fn an_archive_that_breaks_adr_014_is_refused_before_anything_is_filed() {
        let dir = scratch("hostile");
        let root = dir.join("builds");
        let staging = root.join(".fetching");
        std::fs::create_dir_all(&staging).unwrap();
        let hostile = staging.join("base.apk");
        std::fs::write(&hostile, zip_of(&[("../../.bashrc", b"pwned")])).unwrap();
        let e = file_into_store(&[(BASE_APK, hostile)], &root, &filing(), &no_cancel()).unwrap_err();
        assert!(matches!(e, Failed::Archive(apk::Refusal::ParentTraversal(_))), "{e}");
        assert!(store::list_in(&root).is_empty(), "nothing reached the store");
    }

    /// A cancel asked for before any work starts is honoured before anything
    /// is touched, and the source is left where it was.
    #[test]
    fn a_cancel_already_asked_for_is_honoured_before_anything_is_touched() {
        let dir = scratch("cancelled");
        let root = dir.join("builds");
        let elsewhere = dir.join("someone-elses");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let external = elsewhere.join("base.apk");
        std::fs::write(&external, zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917"))])).unwrap();

        let cancel = crate::provider::Cancel::new();
        cancel.stop();
        let err = file_into_store(&[(BASE_APK, external.clone())], &root, &filing(), &cancel)
            .expect_err("a cancel asked for up front must be honoured, not raced past");
        assert!(matches!(err, Failed::Cancelled), "{err:?}");
        assert!(!root.exists(), "nothing was created for a cancel this early");
        assert!(external.is_file(), "the source is untouched");
    }

    /// **A source outside the store is copied and left alone.** The import's
    /// archives are Sober's or the user's; filing one must not take the file
    /// out from under whoever else is using it.
    #[test]
    fn filing_a_build_from_elsewhere_copies_it_and_leaves_the_original() {
        let dir = scratch("elsewhere");
        let root = dir.join("builds");
        let elsewhere = dir.join("sober/packages");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let theirs = elsewhere.join("base.apk");
        std::fs::write(&theirs, zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917"))])).unwrap();
        let before = std::fs::read(&theirs).unwrap();

        file_into_store(&[(BASE_APK, theirs.clone())], &root, &filing(), &no_cancel()).unwrap();
        assert_eq!(std::fs::read(&theirs).unwrap(), before, "bytes unchanged");
        assert!(theirs.is_file(), "not moved");
        assert_eq!(std::fs::read(root.join("2.734.0.917").join(BASE_APK)).unwrap(), before);
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 1, "nothing was added beside it");
    }

    #[test]
    fn the_old_slot_and_the_old_build_directory_share_the_cache_root() {
        assert!(build_dir().starts_with(cache_root()));
        assert!(engine_dir().starts_with(cache_root()));
    }
}
