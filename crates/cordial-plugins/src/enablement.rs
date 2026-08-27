//! Whether a plugin runs at all, kept separately from what it may do.
//!
//! ```json
//! { "flag-inspector": false }
//! ```
//!
//! **Why this is not the grants file.** Before this existed, the only way to
//! stop a plugin was to take its capabilities away, and that conflates two
//! questions the user answers separately: *is this thing running* and *what is
//! it allowed to do*. Revoking to disable makes turning it back on cost every
//! approval decision already made, which is a price nobody should pay for
//! switching something off for an afternoon — and the likely response to that
//! price is to leave a suspect plugin enabled. Grants stay untouched here, so
//! disabling and re-enabling is free and the approvals survive it.
//!
//! **Default enabled, and absence means enabled.** A plugin the user has never
//! had an opinion about is not off; it is simply subject to its grants, which
//! under [ADR-003](../../../docs/adr/ADR-003-plugin-isolation.md) start at
//! nothing. That means an installed plugin with no grants does not run either,
//! but for a different reason, and the two must not be shown as the same state:
//! one is "you have not decided what to allow", the other is "you switched it
//! off".
//!
//! **Per profile, for the same reason as `grants`.** A profile made to test
//! something untrusted must not carry its decisions into the profile someone
//! plays on ([ADR-013](../../../docs/adr/ADR-013-per-profile-configuration.md)).
//! The file sits beside `plugin-grants.json` inside the profile directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// This profile's enablement file.
pub fn path_in(profile_dir: &Path) -> PathBuf {
    profile_dir.join("plugin-enabled.json")
}

/// Everything the user has an opinion about, by plugin id.
///
/// A missing or malformed file reads as "no opinions recorded", the same way
/// `grants::load` treats one. The consequence differs in direction and is
/// deliberate: a grants file that will not parse denies everything, and an
/// enablement file that will not parse falls back to each plugin's default —
/// which is on for anything the user installed and **off** for anything in
/// [`SHIPS_DISABLED`]. This paragraph said "enables everything" until
/// 2026-08-28, which was true until `SHIPS_DISABLED` existed and has been wrong
/// since; a plugin that ships switched off stays off through an unreadable
/// file, and that is the direction it should stay. Neither is a
/// fail-open — the grants file is still the thing that decides what a plugin can
/// do, and it fails closed.
pub fn load(path: &Path) -> BTreeMap<String, bool> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    match serde_json::from_str(&text) {
        Ok(map) => map,
        Err(e) => {
            // Says what actually happens rather than what this used to do.
            // "Every plugin as enabled" would send somebody whose fps-flex did
            // not start looking at a file that is not the reason.
            println!(
                "  plugins: {} is not usable ({e}); no opinions recorded, so each plugin \
                 falls back to its own default",
                path.display()
            );
            BTreeMap::new()
        }
    }
}

/// The key under which the master switch is recorded.
///
/// **`*` cannot collide with a plugin.** `manifest::is_valid_id` allows only
/// ASCII alphanumerics, `-` and `_`, so no plugin can ever be called this and
/// the one document keeps holding one kind of thing: a map from a name to
/// whether it runs. The alternative was a second file beside this one, and two
/// files answering "do plugins run here" is the arrangement that ends with a
/// profile where they disagree.
pub const ALL: &str = "*";

/// Whether plugins run at all in this profile -- the master switch the settings
/// window shows above the two lists.
///
/// Absent means yes, the same as every other entry, so no existing profile
/// changes behaviour by this key appearing.
pub fn plugins_allowed(profile_dir: &Path) -> bool {
    load(&path_in(profile_dir)).get(ALL).copied().unwrap_or(true)
}

/// Record whether plugins run at all in this profile.
pub fn set_plugins_allowed(profile_dir: &Path, on: bool) -> std::io::Result<()> {
    set_enabled(profile_dir, ALL, on)
}

/// Whether `id` runs in this profile. Absent means yes; see the module docs.
///
/// **The master switch is checked here rather than at every call site**, which
/// is what makes it honest without a second mechanism: `plugin_host::start_all`
/// and `flags::collect` already ask this question per plugin, so a master
/// switch that is a special case of the same answer is one nobody can forget to
/// consult. A switch labelled "Use Plugins" that left plugin processes running
/// would be the interface version of a stub returning success.
pub fn is_enabled(profile_dir: &Path, id: &str) -> bool {
    let opinions = load(&path_in(profile_dir));
    if !opinions.get(ALL).copied().unwrap_or(true) {
        return false;
    }
    opinions.get(id).copied().unwrap_or_else(|| default_for(id))
}

/// Plugins that ship with Cordial and start switched **off**.
///
/// A plugin the user went and installed is one they asked for, so the default
/// for anything not named here is on. A plugin that arrives because they
/// installed Cordial is a different thing: nobody chose it, and it should not
/// change how their machine behaves until somebody does.
///
/// `fps-flex` uncaps presentation, which makes the GPU render frames as fast as
/// the driver allows. On a desktop that is the point; on a laptop it is heat and
/// battery spent on frames nobody asked for. Listing it here is what makes the
/// settings entry an offer rather than a change.
///
/// Deliberately a list of ids and not a manifest field. A plugin declaring its
/// own default would let any plugin declare itself on, and the question this
/// answers -- what may run without being asked for -- is Cordial's to answer.
const SHIPS_DISABLED: &[&str] = &["fps-flex"];

/// What [`is_enabled`] answers for a plugin nobody has an opinion about.
pub fn default_for(id: &str) -> bool {
    !SHIPS_DISABLED.contains(&id)
}

/// Record that `id` should or should not run in this profile.
///
/// Written alongside and renamed rather than truncated in place, copying
/// `settings::Store::write` for the reason recorded there: a process killed
/// mid-write would otherwise leave a half-document that reads back as malformed,
/// and here that would silently re-enable every plugin the user had turned off.
pub fn set_enabled(profile_dir: &Path, id: &str, on: bool) -> std::io::Result<()> {
    let path = path_in(profile_dir);
    let mut map = load(&path);
    // Only the exceptions are stored. Writing `true` for everything would make
    // the file grow a permanent entry for every plugin ever installed, and
    // "absent means the default" would stop being true of a file anyone
    // inspects.
    //
    // **The exception is measured against this plugin's own default, not
    // against `true`.** This removed the entry whenever `on` was set, which was
    // right while every plugin defaulted on and silently wrong the moment one
    // shipped disabled: enabling `fps-flex` deleted the only record that it had
    // been enabled, `is_enabled` fell back to the shipped default, and the
    // switch appeared to revert itself on the next launch. Caught by a test
    // rather than by a user, which is the only reason it is not a bug report.
    if on == default_for(id) {
        map.remove(id);
    } else {
        map.insert(id.to_string(), on);
    }

    std::fs::create_dir_all(profile_dir)?;
    let text = serde_json::to_string_pretty(&map)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.new");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    /// A plugin that ships with Cordial must not run until somebody says so.
    ///
    /// The failure this catches: `is_enabled` defaults to true, which is right
    /// for a plugin the user went and installed and wrong for one that arrived
    /// because they installed Cordial. If `fps-flex` ever defaults on, every
    /// user gets uncapped presentation -- their GPU rendering as fast as the
    /// driver allows, on a laptop, without having asked -- and the first they
    /// hear of it is the fan.
    #[test]
    fn a_built_in_that_changes_the_machine_ships_off() {
        let dir = scratch("ships-disabled");
        assert!(!super::is_enabled(&dir, "fps-flex"), "fps-flex must ship disabled");
        assert!(super::is_enabled(&dir, "some-plugin-a-user-installed"));
    }

    /// Turning it on has to stick, and turning it off again has to stick too.
    ///
    /// A default-off plugin whose "on" is not recorded would appear to enable
    /// and revert on the next launch, which reads as the switch not working.
    #[test]
    fn the_user_can_overrule_the_shipped_default_both_ways() {
        let dir = scratch("ships-disabled-override");
        super::set_enabled(&dir, "fps-flex", true).unwrap();
        assert!(super::is_enabled(&dir, "fps-flex"));
        super::set_enabled(&dir, "fps-flex", false).unwrap();
        assert!(!super::is_enabled(&dir, "fps-flex"));
    }

    /// The master switch still wins over a shipped default.
    #[test]
    fn turning_plugins_off_beats_an_enabled_built_in() {
        let dir = scratch("ships-disabled-master");
        super::set_enabled(&dir, "fps-flex", true).unwrap();
        super::set_plugins_allowed(&dir, false).unwrap();
        assert!(!super::is_enabled(&dir, "fps-flex"));
    }

    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-enablement-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_plugin_nobody_has_an_opinion_about_is_enabled() {
        // Absence is not "off". An installed plugin with no grants still does
        // nothing, but for a reason the user has to be able to tell apart.
        let dir = scratch("default-on");
        assert!(is_enabled(&dir, "flag-inspector"));
    }

    #[test]
    fn turning_one_off_and_on_again_round_trips() {
        let dir = scratch("roundtrip");
        set_enabled(&dir, "flag-inspector", false).unwrap();
        assert!(!is_enabled(&dir, "flag-inspector"));
        assert!(is_enabled(&dir, "themer"), "one plugin's state must not decide another's");
        set_enabled(&dir, "flag-inspector", true).unwrap();
        assert!(is_enabled(&dir, "flag-inspector"));
    }

    #[test]
    fn re_enabling_leaves_no_entry_behind() {
        // The file records exceptions only, so that "absent means enabled" is
        // true of the file on disk and not merely of this module's reading of
        // it.
        let dir = scratch("exceptions");
        set_enabled(&dir, "a", false).unwrap();
        set_enabled(&dir, "a", true).unwrap();
        let text = std::fs::read_to_string(path_in(&dir)).unwrap();
        assert!(!text.contains("\"a\""), "{text}");
    }

    #[test]
    fn disabling_is_not_stored_where_grants_are() {
        // The whole point of the module. Turning a plugin off must leave its
        // approvals exactly where they were, or re-enabling costs the user
        // every decision again.
        let dir = scratch("separate");
        let grants = crate::grants::path_in(&dir);
        std::fs::write(&grants, r#"{"flag-inspector":["flags.read"]}"#).unwrap();
        set_enabled(&dir, "flag-inspector", false).unwrap();
        assert_ne!(path_in(&dir), grants);
        let still = crate::grants::load(&grants);
        assert!(
            still.get("flag-inspector").is_some_and(|c| !c.is_empty()),
            "disabling must not touch the grants file"
        );
    }

    #[test]
    fn the_master_switch_stops_every_plugin_without_touching_their_own_entries() {
        // What "Use Plugins" off has to mean: nothing runs, and turning it back
        // on restores exactly the per-plugin choices that were there before --
        // including one plugin the user had separately switched off.
        let dir = scratch("master");
        set_enabled(&dir, "off-anyway", false).unwrap();
        assert!(is_enabled(&dir, "ordinary"));

        set_plugins_allowed(&dir, false).unwrap();
        assert!(!plugins_allowed(&dir));
        assert!(!is_enabled(&dir, "ordinary"), "the master switch must stop a plugin nobody disabled");
        assert!(!is_enabled(&dir, "off-anyway"));

        set_plugins_allowed(&dir, true).unwrap();
        assert!(is_enabled(&dir, "ordinary"));
        assert!(!is_enabled(&dir, "off-anyway"), "the master switch must not clear an individual choice");
    }

    #[test]
    fn the_master_key_cannot_be_mistaken_for_a_plugin() {
        // The whole reason one document can hold both. If `is_valid_id` ever
        // grew to allow `*`, a plugin called that would silently become the
        // master switch.
        assert!(!crate::manifest::is_valid_id(ALL));
    }

    #[test]
    fn a_malformed_file_does_not_silently_disable_everything() {
        let dir = scratch("malformed");
        std::fs::write(path_in(&dir), "{not json").unwrap();
        assert!(is_enabled(&dir, "anything"));
    }
}
