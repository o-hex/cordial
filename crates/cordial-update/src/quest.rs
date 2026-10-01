//! The Quest build, kept in the store beside the phone build.
//!
//! The VR mode runs Roblox's Meta Quest build, an arm64 `libroblox.so`, inside
//! the x86-64 client under the in-process translator
//! ([ADR-053](../../../docs/adr/ADR-053-vr-is-a-mode-of-the-android-runtime.md)).
//! Cordial ships no Roblox code and downloads no Quest build from anywhere: the
//! user supplies the APK from their own headset, and this files it.
//!
//! **Keyed by ABI and version, not by version alone.** ADR-043 named the
//! collision: the store was keyed by version, so an arm64 build at the same
//! version as an x86-64 one would land in the same directory and one would
//! silently replace the other. The phone build stays at `builds/<version>/`,
//! untouched, and the Quest build goes under `builds/arm64-v8a/<version>/`.
//! [`store::list_in`] skips a directory whose name is not a version, so the
//! phone build's listing and pruning never see the Quest entries, and the Quest
//! store has its own `.store.lock` because the two never share an entry.
//!
//! An entry holds every `lib/arm64-v8a/*.so` rather than the engine alone. The
//! translator links `libroblox.so` and the libraries it needs from the same
//! directory -- `libovrplatformloader.so` is one, and it is the one whose
//! constructor needed patches/0005 -- so a directory with the engine alone would
//! fail at link time with a name nothing in the store could answer.

use std::path::{Path, PathBuf};

use crate::install::{Failed, BASE_APK};
use crate::{apk, apk_signature, cache, engine, store};

/// The Quest build's ABI, and the store subdirectory it is kept under.
pub const ABI: &str = "arm64-v8a";

/// The engine inside a Quest APK.
pub const LIBRARY_IN_APK: &str = "lib/arm64-v8a/libroblox.so";

/// A library only the Quest build carries: Meta's platform loader.
///
/// What tells the Quest build from Roblox's arm64 *phone* build, which has the
/// same `lib/arm64-v8a/libroblox.so` and is not a VR client. Measured on
/// 2.740.927: the Quest APK carries it beside `libopenxr_loader.so`.
pub const QUEST_MARKER: &str = "lib/arm64-v8a/libovrplatformloader.so";

/// `~/.cache/cordial/builds/arm64-v8a`.
pub fn root() -> PathBuf {
    root_in(&store::root())
}

/// The Quest store under a given build store root.
pub fn root_in(builds: &Path) -> PathBuf {
    builds.join(ABI)
}

/// What the Quest store holds, newest first.
pub fn list_in(root: &Path) -> Vec<store::Entry> {
    store::list_in(root)
}

/// The build a VR launch uses: the newest entry that has its APK.
///
/// There is no pin and no single-slot link for the Quest build. Both exist for
/// the phone build because an update can replace it underneath a profile; the
/// Quest build arrives only when the user imports one, so the newest is the one
/// they last chose.
pub fn current_in(root: &Path) -> Option<store::Entry> {
    list_in(root).into_iter().find(|e| e.complete)
}

pub fn current() -> Option<store::Entry> {
    current_in(&root())
}

/// Why an APK was not filed.
#[derive(Debug)]
pub enum Refused {
    /// The archive's signature does not verify, or verifies to a certificate
    /// that is not Roblox's. ADR-033's rule: there is no "use this APK
    /// unchecked" path, for an import any more than for a download.
    Signature(apk_signature::Refusal),
    /// Not an arm64 build at all: most likely the phone build for this PC.
    NotArm64,
    /// An arm64 build without Meta's platform loader: the phone build, not
    /// the Quest one.
    PhoneBuild,
    Failed(Failed),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Signature(r) => write!(f, "{r}"),
            Refused::NotArm64 => write!(
                f,
                "This APK has no {LIBRARY_IN_APK}, so it is not the Quest build. Pull the APK \
                 from your headset, not from a phone or a mirror."
            ),
            Refused::PhoneBuild => write!(
                f,
                "This is Roblox's arm64 phone build, not the Quest build: it has no \
                 {QUEST_MARKER}. Pull the APK from your headset."
            ),
            Refused::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl From<Failed> for Refused {
    fn from(e: Failed) -> Self {
        Refused::Failed(e)
    }
}

impl From<apk::Refusal> for Refused {
    fn from(e: apk::Refusal) -> Self {
        Refused::Failed(Failed::Archive(e))
    }
}

/// File a Quest APK into `root`, verified against `trusted`, and return its
/// version.
///
/// The APK is copied, never moved: it is the user's file, wherever they put it
/// (the same rule [`crate::install::adopt`] follows for a file outside
/// Cordial's directories). Already holding that version is not an error; the
/// entry is left as it is and the version returned.
pub fn import(apk_path: &Path, root: &Path, trusted: &[String]) -> Result<String, Refused> {
    import_files(&[apk_path.to_path_buf()], root, trusted)
}

/// [`import`] for an install that is several APKs, as `pm path` lists them:
/// a `base.apk` and splits. Every one must verify; the one holding the engine
/// gives the libraries, and `base.apk` -- or the single file, whatever it is
/// called -- is kept as the entry's `base.apk`, where the assets are. A Quest
/// install has been seen as one APK only; splits are handled the way the
/// phone build's are rather than refused, because a store update could
/// change that without notice.
pub fn import_files(files: &[PathBuf], root: &Path, trusted: &[String]) -> Result<String, Refused> {
    let mut signer = None;
    for f in files {
        apk::inspect(f, apk::Limits::default())?;
        signer = Some(apk_signature::verify_signed_by(f, trusted).map_err(Refused::Signature)?);
    }
    let Some(signer) = signer else {
        return Err(Refused::NotArm64);
    };
    let mut carrier = None;
    for f in files {
        if apk::holds(f, LIBRARY_IN_APK)? {
            carrier = Some(f.clone());
            break;
        }
    }
    let Some(carrier) = carrier else {
        return Err(Refused::NotArm64);
    };
    if !apk::holds(&carrier, QUEST_MARKER)? {
        return Err(Refused::PhoneBuild);
    }
    let base = match files {
        [one] => one.clone(),
        _ => files
            .iter()
            .find(|f| f.file_name().is_some_and(|n| n == BASE_APK))
            .cloned()
            .ok_or_else(|| Failed::Io {
                path: carrier.display().to_string(),
                why: "no base.apk among the files".into(),
            })?,
    };
    let io = |path: &Path, e: std::io::Error| Failed::Io {
        path: path.display().to_string(),
        why: e.to_string(),
    };

    std::fs::create_dir_all(root).map_err(|e| io(root, e))?;
    let gathering = root.join(format!(".filing.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&gathering);
    std::fs::create_dir_all(&gathering).map_err(|e| io(&gathering, e))?;

    let result = (|| -> Result<String, Refused> {
        for name in libraries_in(&carrier)? {
            apk::extract(&carrier, &name, &gathering)?;
        }
        let engine = gathering.join(engine::LIBRARY);
        let version = engine::version_of(&engine)
            .filter(|v| store::is_valid_version(v))
            .ok_or_else(|| Failed::Io {
                path: engine.display().to_string(),
                why:
                    "the engine carries no version Cordial can read, so it cannot be kept under one"
                        .into(),
            })?;
        let entry = store::entry_dir_in(root, &version).expect("checked by is_valid_version above");

        let _lock = store::lock(root).map_err(|e| io(root, e))?;
        if entry.join(engine::LIBRARY).is_file() && entry.join(BASE_APK).is_file() {
            return Ok(version);
        }
        std::fs::copy(&base, gathering.join(BASE_APK)).map_err(|e| io(&gathering, e))?;
        for f in files.iter().filter(|f| **f != base) {
            if let Some(name) = f.file_name() {
                std::fs::copy(f, gathering.join(name)).map_err(|e| io(&gathering, e))?;
            }
        }
        cache::record_version(&gathering, &version).map_err(|e| io(&gathering, e))?;
        cache::record_signer(
            &gathering,
            &signer.certificate_sha256,
            &gathering.join(BASE_APK),
        )
        .map_err(|e| io(&gathering, e))?;
        let _ = std::fs::remove_dir_all(&entry);
        std::fs::rename(&gathering, &entry).map_err(|e| io(&entry, e))?;
        store::ensure_content_hash(&entry);
        Ok(version)
    })();
    let _ = std::fs::remove_dir_all(&gathering);
    let version = result?;
    // Bounded like the phone store, and nothing to protect: there is no pin,
    // and the newest entry -- the one just filed or found -- is always kept.
    store::prune_in(root, store::KEEP, &[]);
    Ok(version)
}

/// Every shared library directly under `lib/arm64-v8a/`.
fn libraries_in(apk_path: &Path) -> Result<Vec<String>, Refused> {
    let file = std::fs::File::open(apk_path).map_err(|e| Failed::Io {
        path: apk_path.display().to_string(),
        why: e.to_string(),
    })?;
    let archive =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| apk::Refusal::NotAZip {
            path: apk_path.display().to_string(),
            why: e.to_string(),
        })?;
    let prefix = format!("lib/{ABI}/");
    Ok(archive
        .file_names()
        .filter(|n| {
            n.strip_prefix(&prefix)
                .is_some_and(|leaf| leaf.ends_with(".so") && !leaf.contains('/'))
        })
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cordial-update-quest-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn zip_at(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }

    /// The Quest APK a developer pulled from their own headset, if they say
    /// where it is. Cordial ships none, so the test that needs a genuine
    /// signed archive is skipped rather than faked without it -- the same rule
    /// as `apk_signature`'s `shipping_apk`.
    fn quest_apk() -> Option<PathBuf> {
        let p = PathBuf::from(std::env::var_os("CORDIAL_TEST_QUEST_APK")?);
        p.is_file().then_some(p)
    }

    #[test]
    fn the_quest_store_sits_inside_the_phone_store_and_is_invisible_to_it() {
        let dir = scratch("nested");
        let builds = dir.join("builds");
        let quest = root_in(&builds);
        for (root, v) in [(&builds, "2.737.0.1"), (&quest, "2.740.0.927")] {
            let e = root.join(v);
            std::fs::create_dir_all(&e).unwrap();
            std::fs::write(e.join(engine::LIBRARY), b"x").unwrap();
            std::fs::write(e.join(BASE_APK), b"x").unwrap();
        }
        let phone: Vec<_> = store::list_in(&builds)
            .into_iter()
            .map(|e| e.version)
            .collect();
        assert_eq!(
            phone,
            ["2.737.0.1"],
            "the phone listing must not offer the Quest build"
        );
        // And pruning the phone store to nothing leaves the Quest store alone.
        store::prune_in(&builds, 0, &[]);
        assert!(quest.join("2.740.0.927").join(engine::LIBRARY).is_file());
        assert_eq!(
            current_in(&quest).map(|e| e.version).as_deref(),
            Some("2.740.0.927")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_without_its_apk_is_not_the_current_quest_build() {
        let dir = scratch("incomplete");
        let e = dir.join("2.740.0.927");
        std::fs::create_dir_all(&e).unwrap();
        std::fs::write(e.join(engine::LIBRARY), b"x").unwrap();
        assert!(current_in(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unsigned_archive_is_refused_before_anything_is_extracted() {
        let dir = scratch("unsigned");
        let apk_path = dir.join("fake.apk");
        zip_at(
            &apk_path,
            &[
                (LIBRARY_IN_APK, b"\0engine 2.740.0.927\0"),
                (QUEST_MARKER, b"x"),
            ],
        );
        let root = dir.join("store");
        match import(&apk_path, &root, &["00".repeat(32)]) {
            Err(Refused::Signature(_)) => {}
            other => panic!("expected a signature refusal, got {other:?}"),
        }
        assert!(list_in(&root).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_quest_key_does_not_widen_what_an_update_accepts() {
        let phone = apk_signature::pinned();
        let quest = apk_signature::pinned_quest();
        assert!(!quest.is_empty(), "the built-in list carries the Quest key");
        assert!(
            quest.iter().all(|q| !phone.contains(q)),
            "a Quest digest leaked into the phone list"
        );
    }

    #[test]
    fn libraries_are_every_so_directly_under_the_abi_directory() {
        let dir = scratch("libs");
        let apk_path = dir.join("a.apk");
        zip_at(
            &apk_path,
            &[
                (LIBRARY_IN_APK, b"x"),
                (QUEST_MARKER, b"x"),
                ("lib/arm64-v8a/notes.txt", b"x"),
                ("lib/arm64-v8a/sub/libnested.so", b"x"),
                ("lib/x86_64/libroblox.so", b"x"),
            ],
        );
        let mut libs = libraries_in(&apk_path).unwrap();
        libs.sort();
        assert_eq!(libs, [QUEST_MARKER, LIBRARY_IN_APK]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_quest_apk_is_filed_whole_and_a_second_import_is_a_no_op() {
        let Some(apk_path) = quest_apk() else {
            eprintln!("skipped: set CORDIAL_TEST_QUEST_APK to a Quest APK pulled from a headset");
            return;
        };
        let dir = scratch("real");
        let root = dir.join("store");
        let version = import(&apk_path, &root, &apk_signature::pinned_quest())
            .expect("the Quest APK should file");
        let entry = root.join(&version);
        assert!(entry.join(engine::LIBRARY).is_file());
        assert!(entry.join("libovrplatformloader.so").is_file());
        assert!(entry.join(BASE_APK).is_file());
        assert!(store::content_hash(&entry).is_some());
        assert!(
            apk_path.is_file(),
            "the user's APK must be copied, not moved"
        );
        let current = current_in(&root).expect("a filed build is the current one");
        assert_eq!(current.version, version);
        assert_eq!(
            import(&apk_path, &root, &apk_signature::pinned_quest()).unwrap(),
            version
        );
        eprintln!("filed Quest build {version}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_import_leaves_the_build_already_kept() {
        let dir = scratch("keep-old");
        let root = dir.join("store");
        let old = root.join("2.740.0.927");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(engine::LIBRARY), b"x").unwrap();
        std::fs::write(old.join(BASE_APK), b"x").unwrap();
        let broken = dir.join("half-pulled.apk");
        std::fs::write(&broken, b"PK\x03\x04 truncated").unwrap();
        assert!(import(&broken, &root, &apk_signature::pinned_quest()).is_err());
        assert_eq!(
            current_in(&root).map(|e| e.version).as_deref(),
            Some("2.740.0.927")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
