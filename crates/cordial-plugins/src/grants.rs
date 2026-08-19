//! What the user has actually approved.
//!
//! A manifest *requests* capabilities; this is what was *granted*. Keeping them
//! in separate files, owned by different parties, is the whole point — if
//! installing a plugin were enough to grant what it asked for, the manifest
//! would be a formality and the capability system would be decorative.
//!
//! ```json
//! {
//!   "flag-inspector": ["flags.read", "log"],
//!   "themer": ["log"]
//! }
//! ```
//!
//! Default deny. A plugin absent from this file gets nothing, and a capability
//! it requested but that is not listed here is refused at the point of use. That
//! is also why there is no "grant everything" entry: it would be the one line
//! anybody pastes from a forum.
//!
//! **The file lives inside the profile, and that is a security property rather
//! than tidiness.** Grants used to sit at `~/.config/cordial/plugin-grants.json`
//! — one list, every account. Approving a plugin in a throwaway profile while
//! trying it out then silently held in the profile someone actually plays on,
//! against the account with the purchases and the friends list, and nothing
//! about approving it there ever suggested it would apply here. ADR-003's
//! default deny is only worth anything if the thing being denied is the same
//! thing the user was asked about; a global allow-list wearing a profile's
//! clothes is not that. Per profile, an approval means what it looked like it
//! meant, and a profile made to test something untrusted stays untrusted.

use crate::capability::Capability;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// This profile's grants file.
///
/// `CORDIAL_PLUGIN_GRANTS` still overrides it outright, which is how the tests
/// and a side-by-side development setup point at a file of their own. Note that
/// the override is global by nature: setting it makes one grants file serve
/// every profile, which is the arrangement this function otherwise exists to
/// end, so it is a development switch and not a supported configuration.
pub fn path_in(profile_dir: &Path) -> PathBuf {
    std::env::var_os("CORDIAL_PLUGIN_GRANTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| profile_dir.join("plugin-grants.json"))
}

/// Where grants lived before they were per profile.
pub fn legacy_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/plugin-grants.json")
}

/// Move a pre-existing global grants file into `profile_dir`, once.
///
/// Follows `cordial_runtime::profile::migrate_legacy_layout` deliberately,
/// including its guard: it runs only when the old file exists and the new one
/// does not, so it cannot overwrite approvals someone has already made in this
/// profile. Leaving the old file to be ignored instead would present as every
/// plugin having silently lost its permissions, which is the class of failure
/// this project keeps a list of.
///
/// **Moved, not copied, and into whichever profile first goes looking.** There
/// is no record of which profile the old file was meant for, because it was
/// meant for all of them — that is the thing being fixed. Copying it into every
/// profile would faithfully reconstruct the global allow-list, so the grants
/// land in one profile and every other profile starts at default deny, which is
/// the outcome the change is for. In practice that profile is `default`, since
/// ADR-012's own migration lands existing storage there.
pub fn migrate_legacy_into(profile_dir: &Path) -> Option<PathBuf> {
    let legacy = legacy_path();
    let target = path_in(profile_dir);
    if !legacy.is_file() || target.exists() {
        return None;
    }
    std::fs::create_dir_all(profile_dir).ok()?;
    match std::fs::rename(&legacy, &target) {
        Ok(()) => {
            println!(
                "  plugin grants: moved {} to {}; other profiles start at default deny",
                legacy.display(),
                target.display()
            );
            Some(target)
        }
        // A cross-device rename is the plausible failure. Saying so and leaving
        // the old file alone beats a half-written grants document, which would
        // be read as granting nothing at all.
        Err(e) => {
            println!(
                "  plugin grants: could not move {} to {} ({e}); the old location is untouched",
                legacy.display(),
                target.display()
            );
            None
        }
    }
}

/// Parse a grants document. Unknown capability names are refused rather than
/// ignored: silently dropping one would grant less than the user believes they
/// granted, and they would have no way to tell.
pub fn parse(text: &str) -> Result<BTreeMap<String, BTreeSet<Capability>>, String> {
    let raw: BTreeMap<String, Vec<String>> =
        serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    for (plugin, names) in raw {
        let mut caps = BTreeSet::new();
        for n in names {
            match Capability::parse(&n) {
                Some(c) => {
                    caps.insert(c);
                }
                None => return Err(format!("unknown capability {n:?} granted to {plugin:?}")),
            }
        }
        out.insert(plugin, caps);
    }
    Ok(out)
}

/// Load grants, or nothing at all.
///
/// A missing file means no plugin has been approved, which is the correct
/// default rather than an error. A malformed file grants nothing and says so —
/// falling back to "grant what was requested" on a parse error would turn a typo
/// into a privilege escalation.
pub fn load(path: &Path) -> BTreeMap<String, BTreeSet<Capability>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    match parse(&text) {
        Ok(g) => g,
        Err(e) => {
            println!("  plugin grants: {} is not usable ({e}); granting nothing", path.display());
            BTreeMap::new()
        }
    }
}

/// Serialise grants to the JSON shape [`parse`] reads back, with plugin ids
/// and capability names in sorted order so writing the same grants twice
/// produces the same bytes rather than a spurious diff.
fn render(grants: &BTreeMap<String, BTreeSet<Capability>>) -> String {
    let raw: BTreeMap<&str, Vec<&str>> = grants
        .iter()
        .map(|(id, caps)| (id.as_str(), caps.iter().map(|c| c.name()).collect()))
        .collect();
    serde_json::to_string_pretty(&raw).expect("a set of capability names always serialises")
}

/// Replace the whole grants document at `path`.
///
/// Written to a sibling file and renamed in, the same atomic-write shape
/// `enablement::set_enabled` and `settings::Store::write` use — a process
/// killed mid-write must not leave a half-document that reads back as
/// malformed, which here would present as every plugin's approvals having
/// silently vanished.
pub fn save(path: &Path, grants: &BTreeMap<String, BTreeSet<Capability>>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, render(grants))?;
    std::fs::rename(&tmp, path)
}

/// Grant or revoke exactly one capability for one plugin, leaving every other
/// plugin's grants — and every other capability of this one — untouched.
///
/// This is the call a settings UI wants. Before it existed, this module could
/// only read a grants file; approving or withdrawing a single capability had
/// no way to happen except hand-editing `plugin-grants.json`, which is not a
/// UI a user should have to operate. `set` reads the current document, flips
/// one entry, and writes the whole thing back — there is no smaller unit to
/// touch, because the file's shape is "one plugin maps to one set", not a
/// list of individual grants that could be appended or removed in place.
pub fn set(path: &Path, plugin: &str, cap: Capability, on: bool) -> std::io::Result<()> {
    let mut grants = load(path);
    let entry = grants.entry(plugin.to_string()).or_default();
    if on {
        entry.insert(cap);
    } else {
        entry.remove(&cap);
        // An empty set and an absent key mean the same thing to `load` and to
        // the broker — nothing granted — so an entry a revoke has emptied is
        // dropped entirely rather than kept as `[]`. Otherwise the file would
        // grow a permanent line for every plugin anyone ever revoked a
        // capability from, the same reasoning `enablement::set_enabled` gives
        // for recording only the exceptions.
        if entry.is_empty() {
            grants.remove(plugin);
        }
    }
    save(path, &grants)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `XDG_CONFIG_HOME` and `CORDIAL_PLUGIN_GRANTS` are process-wide, and
    /// cargo runs these tests in parallel threads of one process. Copied from
    /// `cordial_runtime::profile`'s tests, where the note records that the
    /// unserialised version passed anyway on the first run — which is exactly
    /// how a one-in-three flake gets committed.
    static ENV: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("cordial-grants-test-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // The override would otherwise point every profile at one file, which
        // is the arrangement these tests exist to prove is gone.
        std::env::remove_var("CORDIAL_PLUGIN_GRANTS");
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        (root, guard)
    }

    #[test]
    fn grants_parse() {
        let g = parse(r#"{"a":["log","flags.read"],"b":[]}"#).unwrap();
        assert!(g["a"].contains(&Capability::Log));
        assert!(g["a"].contains(&Capability::FlagsRead));
        assert!(g["b"].is_empty());
    }

    #[test]
    fn an_unknown_capability_is_refused() {
        // Dropping it silently would grant less than the user thinks.
        assert!(parse(r#"{"a":["log","process.spawn"]}"#).is_err());
    }

    #[test]
    fn a_missing_file_grants_nothing() {
        assert!(load(Path::new("/nonexistent/plugin-grants.json")).is_empty());
    }

    #[test]
    fn a_malformed_file_grants_nothing_rather_than_everything() {
        let dir = std::env::temp_dir().join("cordial-grants-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.json");
        std::fs::write(&p, "{not json").unwrap();
        assert!(load(&p).is_empty());
    }

    #[test]
    fn approving_a_plugin_in_one_profile_grants_it_nothing_in_another() {
        // The security property the move exists for. A plugin trusted in a
        // throwaway profile must not silently hold that grant in the profile
        // someone actually plays on, and the only way to be sure of that is
        // for the two profiles to be reading different files.
        let (root, _g) = scratch("isolation");
        let throwaway = root.join("throwaway");
        let main = root.join("main");
        std::fs::create_dir_all(&throwaway).unwrap();
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(
            path_in(&throwaway),
            r#"{"sketchy-plugin":["presence.set","url.open"]}"#,
        )
        .unwrap();

        assert!(load(&path_in(&throwaway)).contains_key("sketchy-plugin"));
        assert!(
            load(&path_in(&main)).is_empty(),
            "a grant made in another profile must not follow the plugin here"
        );
    }

    #[test]
    fn a_pre_existing_global_grants_file_is_moved_into_the_profile() {
        // Ignoring it instead would present as every plugin having silently
        // lost its permissions, with the old file still sitting there looking
        // correct.
        let (root, _g) = scratch("migrate");
        let profile = root.join("profiles/default");
        std::fs::create_dir_all(&profile).unwrap();
        let legacy = legacy_path();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"flag-inspector":["flags.read","log"]}"#).unwrap();

        let moved = migrate_legacy_into(&profile).expect("the legacy file should have moved");
        assert_eq!(moved, path_in(&profile));
        assert!(!legacy.exists(), "the old file should be gone, not copied");
        assert!(load(&moved).contains_key("flag-inspector"));
    }

    #[test]
    fn migration_never_overwrites_grants_this_profile_already_has() {
        // Approvals made here are the user's most recent decision about this
        // profile; a stale global file must not be able to widen or replace
        // them.
        let (root, _g) = scratch("migrate-existing");
        let profile = root.join("profiles/alt");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(path_in(&profile), r#"{"flag-inspector":["log"]}"#).unwrap();

        let legacy = legacy_path();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"flag-inspector":["flags.read","flags.write","log"]}"#)
            .unwrap();

        assert!(migrate_legacy_into(&profile).is_none());
        let held = load(&path_in(&profile));
        assert_eq!(held["flag-inspector"].len(), 1, "the profile's own grants must win");
        assert!(legacy.exists(), "and the old file is left where it was, untouched");
    }

    #[test]
    fn migration_moves_into_one_profile_rather_than_seeding_every_one() {
        // Copying would faithfully rebuild the global allow-list this change
        // exists to remove: the second profile has to come up at default deny.
        let (root, _g) = scratch("migrate-once");
        let first = root.join("profiles/default");
        let second = root.join("profiles/alt");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let legacy = legacy_path();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"sketchy-plugin":["url.open"]}"#).unwrap();

        assert!(migrate_legacy_into(&first).is_some());
        assert!(migrate_legacy_into(&second).is_none());
        assert!(load(&path_in(&second)).is_empty());
    }

    #[test]
    fn granting_one_capability_persists_and_leaves_the_rest_of_the_file_alone() {
        // The call a settings UI wants: flip one checkbox, touch nothing else.
        let (root, _g) = scratch("set-grant");
        let dir = root.join("profile");
        std::fs::create_dir_all(&dir).unwrap();
        let path = path_in(&dir);
        std::fs::write(&path, r#"{"themer":["log"]}"#).unwrap();

        set(&path, "flag-inspector", Capability::FlagsRead, true).unwrap();

        let grants = load(&path);
        assert!(grants["flag-inspector"].contains(&Capability::FlagsRead));
        assert_eq!(grants["themer"], [Capability::Log].into_iter().collect(), "untouched");
    }

    #[test]
    fn revoking_one_capability_leaves_its_siblings_granted() {
        let (root, _g) = scratch("set-revoke");
        let dir = root.join("profile");
        std::fs::create_dir_all(&dir).unwrap();
        let path = path_in(&dir);
        std::fs::write(&path, r#"{"flag-inspector":["flags.read","log"]}"#).unwrap();

        set(&path, "flag-inspector", Capability::FlagsRead, false).unwrap();

        let grants = load(&path);
        assert!(!grants["flag-inspector"].contains(&Capability::FlagsRead));
        assert!(grants["flag-inspector"].contains(&Capability::Log), "log should still be granted");
    }

    #[test]
    fn revoking_the_last_capability_drops_the_plugin_from_the_file_rather_than_leaving_an_empty_list() {
        // An empty set and an absent key mean the same thing to `load`, so
        // the file should not accumulate a permanent `"id": []` for every
        // plugin anyone ever fully revoked — the same call `enablement.rs`
        // makes for re-enabling.
        let (root, _g) = scratch("set-revoke-last");
        let dir = root.join("profile");
        std::fs::create_dir_all(&dir).unwrap();
        let path = path_in(&dir);
        std::fs::write(&path, r#"{"flag-inspector":["log"]}"#).unwrap();

        set(&path, "flag-inspector", Capability::Log, false).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("flag-inspector"), "{text}");
        assert!(load(&path).is_empty());
    }

    #[test]
    fn granting_a_capability_to_a_plugin_nobody_has_approved_yet_creates_its_entry() {
        let (root, _g) = scratch("set-new");
        let dir = root.join("profile");
        std::fs::create_dir_all(&dir).unwrap();
        let path = path_in(&dir);
        assert!(!path.exists());

        set(&path, "brand-new", Capability::Log, true).unwrap();

        assert!(load(&path)["brand-new"].contains(&Capability::Log));
    }

    #[test]
    fn a_capability_granted_in_one_profile_is_not_granted_in_another() {
        // The property ADR-013 exists for, exercised through the write path
        // this time rather than only through `load`: two profiles reading
        // different files by construction is what makes an approval in one
        // mean nothing in the other.
        let (root, _g) = scratch("set-isolation");
        let throwaway = root.join("throwaway");
        let main = root.join("main");
        std::fs::create_dir_all(&throwaway).unwrap();
        std::fs::create_dir_all(&main).unwrap();

        set(&path_in(&throwaway), "sketchy-plugin", Capability::PresenceSet, true).unwrap();

        assert!(load(&path_in(&throwaway))["sketchy-plugin"].contains(&Capability::PresenceSet));
        assert!(
            load(&path_in(&main)).is_empty(),
            "a grant written in one profile must not be readable from another"
        );
    }

    #[test]
    fn saving_replaces_the_whole_document() {
        let (root, _g) = scratch("save-whole");
        let dir = root.join("profile");
        std::fs::create_dir_all(&dir).unwrap();
        let path = path_in(&dir);
        std::fs::write(&path, r#"{"stale-plugin":["log"]}"#).unwrap();

        let mut fresh = BTreeMap::new();
        fresh.insert("themer".to_string(), [Capability::Log].into_iter().collect());
        save(&path, &fresh).unwrap();

        let grants = load(&path);
        assert!(!grants.contains_key("stale-plugin"), "{grants:?}");
        assert!(grants["themer"].contains(&Capability::Log));
    }
}
