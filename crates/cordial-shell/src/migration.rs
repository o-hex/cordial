//! The first launch after ADR-054: moving what the old layout held into the
//! store, once.
//!
//! Before this, a launch followed the slot `lib/<abi>`, which followed whichever
//! archive `effective_apk` had found: the environment, a file chosen in
//! Settings, Cordial's own download, or Sober's package directory. A machine
//! that had only ever used Sober therefore had *no entry in the store at all*
//! (measured on the maintainer's Flatpak: `lib/x86_64` a real directory naming
//! Sober's `base.apk`, `builds/` empty), and one that had chosen a file had that
//! file as its only launch source. Both must keep launching the same build, so
//! this runs before the window exists and files them.
//!
//! What it does, in order:
//!
//! 1. Moves a store left in `~/.cache/cordial/builds` to `$XDG_DATA_HOME/cordial/builds`
//!    (`cordial_update::store::relocate`), because a cache is what cleaners
//!    delete and a pinned build may not be obtainable again.
//! 2. Asks `cordial_update::store::migration_plan` what else is owed, from what
//!    is on disk, and does it: a slot or Sober build with nothing in the store is
//!    verified, **copied** (never linked: a Flatpak sees Sober's directory
//!    through a read-only grant) and filed with its provenance; a file chosen in
//!    Settings is filed, every profile with no pin is pinned to it, and the
//!    setting is cleared.
//! 3. Re-points the old slot at the newest entry, for `just client` and
//!    hand-typed `--lib-dir`.
//!
//! **Once, by construction.** After it the store holds a complete entry and the
//! Settings APK is cleared, so every condition that triggered it is gone; a wiped
//! cache with Sober still installed finds an empty store and gets the first-run
//! screen with both buttons, not a silent re-import. A failure (an archive that
//! does not verify) leaves the conditions in place and is tried again next time,
//! and is said, here and on the first-run screen's own Copy button.
//!
//! **Nothing of Sober's is deleted, moved or written to, in any path.**

use cordial_update::provider::{self, import};
use cordial_update::store::{self, Legacy, Slot, Source, Step};
use std::path::{Path, PathBuf};

use crate::shell_config::ShellConfig;

/// What a run did, for the log and for the one question it may owe the user.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Entries moved out of the cache store.
    pub moved: Vec<String>,
    /// Builds filed, with how they arrived.
    pub filed: Vec<(String, Source)>,
    /// Profiles pinned to a file the user had chosen.
    pub pinned_profiles: Vec<String>,
    /// `roblox.apk` was cleared, so the config needs saving.
    pub cleared_settings_apk: bool,
    /// Things that did not work, in words.
    pub failures: Vec<String>,
}

impl Report {
    pub fn config_changed(&self) -> bool {
        self.cleared_settings_apk
    }

    /// Whether Sober's build was just copied in. Its updates used to move with
    /// Sober's app; they now move with Cordial's, so this is the one launch
    /// that asks once whether to fetch the newest (ADR-054, decision 4).
    pub fn offer_newest(&self) -> bool {
        self.filed.iter().any(|(_, s)| *s == Source::Sober)
    }

    pub fn print(&self) {
        for v in &self.moved {
            println!("  migration: moved {v} to the data directory");
        }
        for (v, source) in &self.filed {
            println!("  migration: filed Roblox {v} ({})", source.as_str());
        }
        for p in &self.pinned_profiles {
            println!("  migration: pinned profile {p} to the file chosen in Settings");
        }
        if self.cleared_settings_apk {
            println!("  migration: cleared the APK path saved in Settings");
        }
        for f in &self.failures {
            println!("  migration: {f}");
        }
    }
}

/// Where things are, handed in so a test can run a migration without the
/// machine's real directories.
pub struct Env {
    pub cache_store: PathBuf,
    pub store_root: PathBuf,
    pub slot: PathBuf,
    pub managed_apk: Option<PathBuf>,
    pub sober_apk: Option<PathBuf>,
    /// Every profile directory, for pinning.
    pub profiles: Vec<PathBuf>,
    pub trusted: Vec<String>,
}

impl Env {
    pub fn live() -> Env {
        let sober = crate::install::sober_apk();
        Env {
            cache_store: store::cache_store(),
            store_root: store::root(),
            slot: crate::install::engine_cache(),
            managed_apk: cordial_update::install::managed_base(),
            sober_apk: sober.is_file().then_some(sober),
            profiles: cordial_shell::profile::list()
                .iter()
                .filter_map(|n| cordial_shell::profile::dir(n).ok())
                .collect(),
            trusted: cordial_update::apk_signature::pinned(),
        }
    }
}

pub fn run(config: &mut ShellConfig) -> Report {
    run_with(config, &Env::live())
}

pub fn run_with(config: &mut ShellConfig, env: &Env) -> Report {
    let mut report = Report::default();

    match store::relocate(&env.cache_store, &env.store_root) {
        Ok(moved) => report.moved = moved,
        Err(e) => report.failures.push(format!(
            "could not move the build store out of {}: {e}",
            env.cache_store.display()
        )),
    }

    let entries = store::list_in(&env.store_root);
    let legacy = Legacy {
        store_has_complete_entry: entries.iter().any(|e| e.complete),
        slot: slot_state(&env.slot, &env.store_root),
        slot_archive: slot_archive(&env.slot),
        orphan_archive: orphan_archive(&entries, env.sober_apk.as_deref()),
        managed_apk: env.managed_apk.clone(),
        sober_apk: env.sober_apk.clone(),
        settings_apk: config.roblox.apk.clone(),
    };
    for step in store::migration_plan(&legacy) {
        match step {
            Step::File { apk, source } => match file(&apk, source, env) {
                Ok(version) => report.filed.push((version, source)),
                Err(why) => report.failures.push(format!("could not file {}: {why}", apk.display())),
            },
            Step::ImportChosen { apk } => match file(&apk, Source::for_path(&apk), env) {
                Ok(version) => {
                    report.filed.push((version.clone(), Source::for_path(&apk)));
                    report.pinned_profiles = pin_unpinned(&env.profiles, &version);
                    config.roblox.apk = None;
                    report.cleared_settings_apk = true;
                }
                // Left in place: the file may be on a drive that is not mounted
                // right now, and clearing the only record of what the user chose
                // would turn "try again later" into "lost".
                Err(why) => report.failures.push(format!(
                    "could not import the APK chosen in Settings ({}): {why}",
                    apk.display()
                )),
            },
        }
    }

    repoint_slot(&env.slot, &env.store_root);
    report
}

/// Verify, copy and file one build.
fn file(apk: &Path, source: Source, env: &Env) -> Result<String, String> {
    let found = import::from_file(apk)?;
    provider::import_into_store_trusting(
        &found,
        source,
        &env.store_root,
        &env.trusted,
        &provider::Cancel::new(),
        &mut |_| {},
    )
    .map_err(|e| e.to_string())
}

/// Pin every profile that has no pin to `version`. Returns who was pinned.
///
/// A user who chose a file meant that file, and Latest would otherwise move
/// every such profile onto whatever the store holds newest.
pub fn pin_unpinned(profile_dirs: &[PathBuf], version: &str) -> Vec<String> {
    let mut pinned = Vec::new();
    for dir in profile_dirs {
        if cordial_shell::profile::pinned_version(dir).is_some() {
            continue;
        }
        if cordial_shell::profile::set_pinned_version(dir, Some(version)).is_ok() {
            if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                pinned.push(name.to_string());
            }
        }
    }
    pinned
}

/// What the old slot is.
pub fn slot_state(slot: &Path, store_root: &Path) -> Slot {
    match std::fs::symlink_metadata(slot) {
        Ok(m) if m.file_type().is_symlink() => {
            if store::current_in(store_root, slot).is_some() && slot.join(crate::install::LIBRARY).is_file() {
                Slot::LinkIntoStore
            } else {
                // Dangling, or a link to somewhere that is not the store.
                Slot::Absent
            }
        }
        Ok(m) if m.is_dir() && slot.join(crate::install::LIBRARY).is_file() => Slot::Directory,
        _ => Slot::Absent,
    }
}

/// The archive the slot's engine was extracted from, if it still exists.
///
/// The stamp is `size mtime path` (see `cordial_update::cache`), and a path may
/// hold spaces, so it is everything after the second field.
pub fn slot_archive(slot: &Path) -> Option<PathBuf> {
    let stamp = cordial_update::cache::stamp_of(slot)?;
    let mut fields = stamp.splitn(3, ' ');
    fields.next()?;
    fields.next()?;
    let path = PathBuf::from(fields.next()?.trim_end());
    path.is_file().then_some(path)
}

/// The archive an entry kept without its own archives was taken from.
///
/// The 0.24.1 Flatpak extracts Sober's engine into an entry and then fails to
/// hard link Sober's archives across the read-only mount (`EXDEV`), keeping the
/// entry "without its archives". The entry is moved with the rest of the cache
/// store, which is correct, and it is then the only record left of what the
/// old launch was running: the slot link into the cache now dangles. Its own
/// `.from` stamp names the archive, in the slot's format; Sober's directory is
/// the fallback for a stamp whose file has since been replaced or is unreadable,
/// and only because an entry like this exists, so Sober alone still files nothing.
pub fn orphan_archive(entries: &[store::Entry], sober: Option<&Path>) -> Option<PathBuf> {
    let newest = entries
        .iter()
        .filter(|e| !e.complete)
        .max_by(|a, b| store::compare(&a.version, &b.version))?;
    slot_archive(&newest.dir).or_else(|| sober.map(Path::to_path_buf))
}

/// Point the old slot at the newest entry, when it is free to be pointed:
/// absent, or a link (including a dangling one left by the move). A real
/// directory is somebody's engine -- the override's, or `just client`'s -- and
/// is left alone.
fn repoint_slot(slot: &Path, store_root: &Path) {
    let free = match std::fs::symlink_metadata(slot) {
        Ok(m) => m.file_type().is_symlink(),
        Err(_) => true,
    };
    if !free {
        return;
    }
    // The newest *complete* entry, not the newest launchable one: entries the
    // old layout left have not been checked yet (the first launch does that),
    // and the slot is for `just client` and hand-typed `--lib-dir`, which read
    // nothing but an engine and its archive.
    let newest = store::list_in(store_root)
        .into_iter()
        .filter(|e| e.complete)
        .max_by(|a, b| store::compare(&a.version, &b.version));
    if let Some(entry) = newest {
        if let Err(e) = store::point_current_at(slot, &entry.dir) {
            println!("  migration: could not point {} at Roblox {}: {e}", slot.display(), entry.version);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-shell-migration-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn env_in(dir: &Path) -> Env {
        Env {
            cache_store: dir.join("cache/cordial/builds"),
            store_root: dir.join("data/cordial/builds"),
            slot: dir.join("cache/cordial/lib/x86_64"),
            managed_apk: None,
            sober_apk: None,
            profiles: Vec::new(),
            // A key no archive made here was signed with: every filing is refused.
            trusted: vec!["44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477".into()],
        }
    }

    /// An APK-shaped zip whose engine scans as `version`, with no signature.
    fn unsigned_apk(path: &Path, version: &str) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        w.start_file(cordial_update::apk::LIBRARY_IN_APK, zip::write::SimpleFileOptions::default()).unwrap();
        let mut engine = b"\0not an engine\0".to_vec();
        engine.extend_from_slice(version.as_bytes());
        engine.push(0);
        w.write_all(&engine).unwrap();
        w.finish().unwrap();
    }

    fn launchable(root: &Path, v: &str) {
        let dir = root.join(v);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libroblox.so"), v).unwrap();
        std::fs::write(dir.join("base.apk"), format!("apk {v}")).unwrap();
        store::record_signer(&dir, &"ab".repeat(32)).unwrap();
    }

    /// Case 1 of the table: a store already holding a complete build, and the
    /// slot already a link into it, is left exactly as it is.
    #[test]
    fn a_slot_already_linked_into_the_store_has_nothing_to_migrate() {
        let dir = scratch("linked");
        let env = env_in(&dir);
        launchable(&env.store_root, "2.738.0.1397");
        store::point_current_at(&env.slot, &env.store_root.join("2.738.0.1397")).unwrap();
        let mut config = ShellConfig::default();
        let report = run_with(&mut config, &env);
        assert_eq!(report, Report::default());
        assert_eq!(store::list_in(&env.store_root).len(), 1);
    }

    /// The store left in the cache moves to the data directory, and the slot
    /// link that pointed into it is re-pointed rather than left dangling.
    #[test]
    fn a_cache_store_moves_and_the_slot_link_is_repaired() {
        let dir = scratch("moves");
        let env = env_in(&dir);
        launchable(&env.cache_store, "2.738.0.1397");
        launchable(&env.cache_store, "2.736.0.1408");
        store::point_current_at(&env.slot, &env.cache_store.join("2.738.0.1397")).unwrap();

        let report = run_with(&mut ShellConfig::default(), &env);
        assert_eq!(report.moved, ["2.736.0.1408", "2.738.0.1397"]);
        assert_eq!(store::list_in(&env.store_root).len(), 2);
        assert!(store::list_in(&env.store_root).iter().all(|e| e.launchable()), "records survive the move");
        assert!(!env.cache_store.exists());
        assert_eq!(
            store::current_in(&env.store_root, &env.slot).as_deref(),
            Some("2.738.0.1397"),
            "no dangling link into the cache"
        );
        assert!(env.slot.join("libroblox.so").is_file());
    }

    /// The entries the old layout left carry no record of who signed them: the
    /// first launch checks them. The slot must still be repaired by then, or
    /// it dangles into the cache until something else notices.
    #[test]
    fn the_slot_is_repaired_even_while_the_moved_entries_are_unchecked() {
        let dir = scratch("unchecked");
        let env = env_in(&dir);
        launchable(&env.cache_store, "2.738.0.1397");
        std::fs::remove_file(env.cache_store.join("2.738.0.1397/.signer")).unwrap();
        store::point_current_at(&env.slot, &env.cache_store.join("2.738.0.1397")).unwrap();

        run_with(&mut ShellConfig::default(), &env);
        assert!(!store::list_in(&env.store_root)[0].launchable(), "unchecked until it launches");
        assert_eq!(store::current_in(&env.store_root, &env.slot).as_deref(), Some("2.738.0.1397"));
        assert!(env.slot.join("libroblox.so").is_file());
    }

    /// An archive that does not verify is not filed, nothing of Sober's is
    /// written, and the failure is said. The conditions stay, so it is tried
    /// again next launch.
    #[test]
    fn an_unsigned_sober_build_is_refused_and_sober_is_untouched() {
        let dir = scratch("sober-unsigned");
        let mut env = env_in(&dir);
        let sober = dir.join("home/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client");
        std::fs::create_dir_all(&sober).unwrap();
        unsigned_apk(&sober.join("base.apk"), "2.738.0.1397");
        let before = std::fs::read(sober.join("base.apk")).unwrap();
        env.sober_apk = Some(sober.join("base.apk"));

        // Sober alone is not a reason to file anything: the first-run screen
        // offers it. What the old launch was *running* is, and that always
        // left a real directory in the slot.
        let alone = run_with(&mut ShellConfig::default(), &env);
        assert_eq!(alone, Report::default(), "no slot, so nothing silent");
        assert!(store::list_in(&env.store_root).is_empty());

        std::fs::create_dir_all(&env.slot).unwrap();
        std::fs::write(env.slot.join("libroblox.so"), b"an engine extracted from sober's apk").unwrap();
        let report = run_with(&mut ShellConfig::default(), &env);
        assert!(report.filed.is_empty());
        assert_eq!(report.failures.len(), 1, "{report:?}");
        assert!(store::list_in(&env.store_root).is_empty());
        assert_eq!(std::fs::read(sober.join("base.apk")).unwrap(), before);
        assert_eq!(std::fs::read_dir(&sober).unwrap().count(), 1, "nothing was added beside it");
        // And again next launch: still nothing filed, still said.
        assert_eq!(run_with(&mut ShellConfig::default(), &env).failures.len(), 1);
    }

    /// A Settings APK that cannot be imported stays saved, and nothing is
    /// pinned to a build that was never filed.
    #[test]
    fn a_settings_apk_that_cannot_be_imported_stays_set_and_pins_nothing() {
        let dir = scratch("chosen-fails");
        let mut env = env_in(&dir);
        let profile = dir.join("data/cordial/profiles/default");
        std::fs::create_dir_all(&profile).unwrap();
        env.profiles = vec![profile.clone()];
        let chosen = dir.join("Downloads/roblox.apk");
        std::fs::create_dir_all(chosen.parent().unwrap()).unwrap();
        unsigned_apk(&chosen, "2.730.0.790");
        let mut config = ShellConfig::default();
        config.roblox.apk = Some(chosen.clone());

        let report = run_with(&mut config, &env);
        assert_eq!(report.failures.len(), 1, "{report:?}");
        assert_eq!(config.roblox.apk, Some(chosen));
        assert!(!report.config_changed());
        assert_eq!(cordial_shell::profile::pinned_version(&profile), None);
    }

    /// Settings-APK-pins-every-profile, at the step that does the pinning: a
    /// profile with a pin keeps it, every other profile gets the file's build.
    #[test]
    fn every_profile_without_a_pin_is_pinned_to_the_imported_file() {
        let dir = scratch("pins");
        let mk = |n: &str| {
            let p = dir.join(n);
            std::fs::create_dir_all(&p).unwrap();
            p
        };
        let (a, b, c) = (mk("alpha"), mk("beta"), mk("gamma"));
        cordial_shell::profile::set_pinned_version(&b, Some("2.700.0.1")).unwrap();

        let pinned = pin_unpinned(&[a.clone(), b.clone(), c.clone()], "2.730.0.790");
        assert_eq!(pinned, ["alpha", "gamma"]);
        assert_eq!(cordial_shell::profile::pinned_version(&a).as_deref(), Some("2.730.0.790"));
        assert_eq!(cordial_shell::profile::pinned_version(&b).as_deref(), Some("2.700.0.1"), "an existing pin is kept");
        assert_eq!(cordial_shell::profile::pinned_version(&c).as_deref(), Some("2.730.0.790"));
        assert!(pin_unpinned(&[a, b, c], "2.730.0.790").is_empty(), "idempotent");
    }

    #[test]
    fn the_slot_is_read_for_what_it_is_and_what_it_was_extracted_from() {
        let dir = scratch("slot");
        let store_root = dir.join("builds");
        let slot = dir.join("lib/x86_64");
        assert_eq!(slot_state(&slot, &store_root), Slot::Absent);

        std::fs::create_dir_all(&slot).unwrap();
        std::fs::write(slot.join("libroblox.so"), b"e").unwrap();
        assert_eq!(slot_state(&slot, &store_root), Slot::Directory);

        // A stamp naming an archive with a space in its path, which exists.
        let apk = dir.join("with space/base.apk");
        std::fs::create_dir_all(apk.parent().unwrap()).unwrap();
        std::fs::write(&apk, b"apk").unwrap();
        cordial_update::cache::write_stamp(&slot, &apk).unwrap();
        assert_eq!(slot_archive(&slot), Some(apk.clone()));
        std::fs::remove_file(&apk).unwrap();
        assert_eq!(slot_archive(&slot), None, "an archive that is gone is not a source");

        std::fs::remove_dir_all(&slot).unwrap();
        launchable(&store_root, "2.738.0.1397");
        store::point_current_at(&slot, &store_root.join("2.738.0.1397")).unwrap();
        assert_eq!(slot_state(&slot, &store_root), Slot::LinkIntoStore);
        std::fs::remove_dir_all(store_root.join("2.738.0.1397")).unwrap();
        assert_eq!(slot_state(&slot, &store_root), Slot::Absent, "dangling");
    }

    /// What the 0.24.1 Flatpak leaves for a user whose Roblox came from Sober:
    /// an engine-only entry in the cache store (the hard link across Sober's
    /// read-only mount failed), a slot link into it, and Sober's archives where
    /// they always were. The first launch after the upgrade must complete that
    /// entry from them, say so, and offer the update once; it used to move the
    /// entry, file nothing, and refuse every launch.
    #[test]
    fn an_engine_only_entry_the_old_flatpak_kept_from_sober_is_completed_from_sober() {
        let dir = scratch("engine-only");
        let mut env = env_in(&dir);
        let sober = dir.join("home/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client");
        std::fs::create_dir_all(&sober).unwrap();
        unsigned_apk(&sober.join("base.apk"), "2.737.0.1584");
        std::fs::copy(sober.join("base.apk"), sober.join("split_config.x86_64.apk")).unwrap();
        env.sober_apk = Some(sober.join("base.apk"));
        // The fixture archive is unsigned and the filing checks the
        // certificate, so it is refused; that it was *tried* is the claim,
        // because before this the plan had nothing to try.
        let entry = env.cache_store.join("2.737.0.1584");
        std::fs::create_dir_all(&entry).unwrap();
        std::fs::write(entry.join("libroblox.so"), b"engine").unwrap();
        cordial_update::cache::write_stamp(&entry, &sober.join("base.apk")).unwrap();
        store::point_current_at(&env.slot, &entry).unwrap();

        let report = run_with(&mut ShellConfig::default(), &env);
        assert_eq!(report.moved, ["2.737.0.1584"]);
        // The plan reached for Sober's archive instead of doing nothing; the
        // refusal is the unsigned fixture's.
        assert_eq!(report.failures.len(), 1, "{report:?}");
        assert!(report.failures[0].contains("base.apk"), "{report:?}");
        assert_eq!(std::fs::read_dir(&sober).unwrap().count(), 2, "nothing was added beside Sober's files");
    }

    #[test]
    fn only_complete_entries_have_no_orphan_and_the_stamp_beats_sober() {
        let dir = scratch("orphan");
        let root = dir.join("builds");
        launchable(&root, "2.738.0.1397");
        let sober = dir.join("sober/base.apk");
        std::fs::create_dir_all(sober.parent().unwrap()).unwrap();
        std::fs::write(&sober, b"sober").unwrap();
        assert_eq!(orphan_archive(&store::list_in(&root), Some(&sober)), None, "nothing is missing its archives");

        let bare = root.join("2.737.0.1584");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(bare.join("libroblox.so"), b"engine").unwrap();
        // No stamp: Sober's directory, because an entry is missing its archives.
        assert_eq!(orphan_archive(&store::list_in(&root), Some(&sober)), Some(sober.clone()));
        assert_eq!(orphan_archive(&store::list_in(&root), None), None);
        // A stamp naming a file that exists is preferred to Sober.
        let chosen = dir.join("chosen/base.apk");
        std::fs::create_dir_all(chosen.parent().unwrap()).unwrap();
        std::fs::write(&chosen, b"apk").unwrap();
        cordial_update::cache::write_stamp(&bare, &chosen).unwrap();
        assert_eq!(orphan_archive(&store::list_in(&root), Some(&sober)), Some(chosen));
    }

    #[test]
    fn only_a_sober_filing_asks_whether_to_fetch_the_newest() {
        let sober = Report { filed: vec![("2.738.0.1397".into(), Source::Sober)], ..Default::default() };
        let file = Report { filed: vec![("2.738.0.1397".into(), Source::File)], ..Default::default() };
        assert!(sober.offer_newest());
        assert!(!file.offer_newest());
        assert!(!Report::default().offer_newest());
    }
}
