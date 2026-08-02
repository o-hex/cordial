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
/// enablement file that will not parse enables everything. Neither is a
/// fail-open — the grants file is still the thing that decides what a plugin can
/// do, and it fails closed.
pub fn load(path: &Path) -> BTreeMap<String, bool> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    match serde_json::from_str(&text) {
        Ok(map) => map,
        Err(e) => {
            println!("  plugins: {} is not usable ({e}); treating every plugin as enabled", path.display());
            BTreeMap::new()
        }
    }
}

/// Whether `id` runs in this profile. Absent means yes; see the module docs.
pub fn is_enabled(profile_dir: &Path, id: &str) -> bool {
    load(&path_in(profile_dir)).get(id).copied().unwrap_or(true)
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
    // "absent means enabled" would stop being true of a file anyone inspects.
    if on {
        map.remove(id);
    } else {
        map.insert(id.to_string(), false);
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
    fn a_malformed_file_does_not_silently_disable_everything() {
        let dir = scratch("malformed");
        std::fs::write(path_in(&dir), "{not json").unwrap();
        assert!(is_enabled(&dir, "anything"));
    }
}
