//! Layered FastFlag overrides, with provenance.
//!
//! Flags come from more than one place: Roblox's own settings document, the
//! user, and — once plugins exist — any plugin that needs a flag set to do its
//! job. Merging those into one file would be the obvious thing and it is wrong
//! in three specific ways:
//!
//! * a plugin could silently overwrite a value the user chose deliberately;
//! * uninstalling a plugin would leave its flags behind, because nothing records
//!   which line belonged to whom;
//! * "why is this flag set to that?" would have no answer.
//!
//! So each source owns its own file and they are resolved in a fixed order,
//! recording where every effective value came from.
//!
//! ```text
//! <profile>/flags.json                                user      (wins)
//! ~/.local/share/cordial/plugins/<id>/flags.json      plugin
//! the client-settings document from Roblox            base
//! ```
//!
//! **The user always wins.** An explicit setting is the one thing that must not
//! be overridable by software the user installed to do something else.
//!
//! **The user's file is per profile** (ADR-013). It used to be one file at
//! `~/.config/cordial/flags.json` for the whole machine, which meant a flag set
//! while debugging something on a test account was silently still set on the
//! account someone actually plays — and flags are exactly the setting people
//! change temporarily and forget. `migrate_legacy_user_file` moves an existing
//! one in rather than leaving it to be ignored, because a launch that quietly
//! stopped honouring overrides is indistinguishable from the overrides never
//! having worked.
//!
//! A plugin's own `flags.json` stays beside its installed code, which is global
//! — a plugin is installed once. That is a real asymmetry and it is recorded in
//! ADR-013's open question, because an installed plugin currently contributes
//! its flags in every profile whether or not it was granted anything there.
//!
//! **Plugin-against-plugin conflicts are reported, not resolved.** Two plugins
//! wanting the same flag set differently is a real disagreement, and picking one
//! by filesystem order would hide it. Both are named and the first-loaded wins,
//! so the behaviour is at least deterministic and visible.
//!
//! One property matters to anything built on this, and it is not obvious:
//! `FFlag`, `FInt` and `FString` are read once during engine startup, so a
//! plugin can only influence them if its flags are resolved *before the process
//! starts*. Only the `DFFlag`/`DFInt`/`DFString` family is re-read while the
//! client runs. A plugin loaded on demand, part-way through a session, cannot
//! change a startup flag no matter what it writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where an effective flag value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The user's own `flags.json`.
    User,
    /// A plugin, by its directory name.
    Plugin(String),
    /// Cordial's own defaults, below every other layer.
    Builtin,
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::User => "user".into(),
            Source::Plugin(id) => format!("plugin:{id}"),
            Source::Builtin => "built-in".into(),
        }
    }
}

/// One resolved override: the value that will be applied, and who asked for it.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub value: String,
    pub source: Source,
    /// Other sources that wanted this flag and lost, in the order they were read.
    pub overridden: Vec<(Source, String)>,
}

/// A set of overrides read from one file.
pub struct Layer {
    pub source: Source,
    pub values: BTreeMap<String, String>,
}

/// Read a flat JSON object of flag -> value.
///
/// Roblox stores every setting as a string, so booleans and numbers are
/// converted rather than refused — a config file should not require knowing
/// that. Returns `None` when the file is absent, and reports and skips when it
/// is present but malformed: a typo should not stop the client from starting.
pub fn read_layer(path: &Path, source: Source) -> Option<Layer> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            println!("  flags: {} is not valid JSON ({e}); ignoring", path.display());
            return None;
        }
    };
    let Some(obj) = parsed.as_object() else {
        println!("  flags: {} is not a JSON object; ignoring", path.display());
        return None;
    };
    let values = obj
        .iter()
        .map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect();
    Some(Layer { source, values })
}

/// Resolve layers into one set of effective values.
///
/// `layers` is in precedence order, lowest first, so the user's layer goes last.
/// Everything a losing layer wanted is kept on the winner rather than discarded,
/// which is what makes the result explainable afterwards.
pub fn resolve(layers: Vec<Layer>) -> BTreeMap<String, Resolved> {
    let mut out: BTreeMap<String, Resolved> = BTreeMap::new();
    for layer in layers {
        for (key, value) in layer.values {
            match out.get_mut(&key) {
                Some(existing) => {
                    let previous = std::mem::replace(&mut existing.value, value);
                    let previous_source =
                        std::mem::replace(&mut existing.source, layer.source.clone());
                    existing.overridden.push((previous_source, previous));
                }
                None => {
                    out.insert(
                        key,
                        Resolved { value, source: layer.source.clone(), overridden: Vec::new() },
                    );
                }
            }
        }
    }
    out
}

/// The user's overrides file, inside `profile_dir`.
///
/// `CORDIAL_FLAGS` still overrides it outright, which is how a one-off
/// experiment points at a file of its own. That override is global by nature —
/// it makes one file serve every profile — so it is a development switch, not a
/// supported arrangement.
pub fn user_path_in(profile_dir: &Path) -> PathBuf {
    std::env::var_os("CORDIAL_FLAGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| profile_dir.join("flags.json"))
}

/// The user's overrides file for the profile this instance is running.
pub fn user_path() -> PathBuf {
    user_path_in(&crate::profile::active())
}

/// Where the user's overrides lived before they were per profile.
pub fn legacy_user_path() -> PathBuf {
    config_dir().join("cordial/flags.json")
}

/// Move a pre-existing global overrides file into `profile_dir`, once.
///
/// Same guard and the same reasoning as `profile::migrate_legacy_layout` and
/// `cordial_plugins::grants::migrate_legacy_into`: only when the old file
/// exists and the new one does not, so it can never overwrite overrides already
/// made in this profile. Moved rather than copied, and into whichever profile
/// first goes looking, because there is no record of which profile a global
/// file was meant for — it was meant for all of them, which is the thing being
/// fixed. In practice that is `default`, where ADR-012's migration lands
/// existing storage.
pub fn migrate_legacy_user_file(profile_dir: &Path) -> Option<PathBuf> {
    let legacy = legacy_user_path();
    let target = user_path_in(profile_dir);
    if !legacy.is_file() || target.exists() {
        return None;
    }
    std::fs::create_dir_all(profile_dir).ok()?;
    match std::fs::rename(&legacy, &target) {
        Ok(()) => {
            println!(
                "  flags: moved {} to {} (ADR-013)",
                legacy.display(),
                target.display()
            );
            Some(target)
        }
        // A cross-device rename is the plausible failure. Leaving the old file
        // alone and saying so beats a half-written overrides document, which
        // would be reported as invalid JSON and ignored entirely.
        Err(e) => {
            println!(
                "  flags: could not move {} to {} ({e}); the old location is untouched",
                legacy.display(),
                target.display()
            );
            None
        }
    }
}

/// The directory each plugin's own overrides live under, one subdirectory per
/// plugin id. A plugin owns its file and nothing else writes to it, so removing
/// the plugin removes its flags.
pub fn plugin_dir() -> PathBuf {
    std::env::var_os("CORDIAL_PLUGIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir().join("cordial/plugins"))
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir)
}

fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
}

/// Flags Cordial sets for itself, at the bottom of the stack.
///
/// The bar for anything here is a setting whose default is wrong specifically
/// *because* the engine is an Android build running on a desktop, and which the
/// settings document does not carry — so there is no value from Roblox being
/// overridden, only a compiled-in default being replaced.
///
/// `FStringGraphicsTextureManager2DenyPattern2` is absent from the document
/// entirely. TextureManager2 picks a streaming tier from hardware it recognises,
/// the document's sibling `...DenyPattern` already denies tiers 1 to 3, and a
/// desktop GPU behind this stack matches nothing it knows — so it settles on low
/// residency and textures load blurred. `.*` denies every pattern, which takes
/// the engine off TextureManager2 and onto the legacy streaming path that
/// `FFlagTextureManager2SupportLegacyStreaming2` keeps alive.
///
/// **`INFERRED`.** The flag's absence from the document and its effect on
/// mocktail are established; the tier-matching story above is the reading that
/// fits, not something measured here. It is at the bottom layer precisely
/// because it is a guess a user should be able to overrule with one line.
const BUILTIN: &[(&str, &str)] = &[("FStringGraphicsTextureManager2DenyPattern2", ".*")];

fn builtin_layer() -> Layer {
    Layer {
        source: Source::Builtin,
        values: BUILTIN.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

/// Every layer, in precedence order: Cordial's own defaults first so anything
/// can overrule them, then plugins (alphabetically, so the outcome does not
/// depend on directory iteration order), then the user.
pub fn collect() -> Vec<Layer> {
    // The move happens here because this is the one path every reader of the
    // user's overrides goes through, and the runtime has no single startup hook
    // that a launcher-started client and a hand-started one both pass. Once per
    // process: `collect` runs again for every `flags.list` a plugin makes, and
    // re-stating a file that has already moved on each of those is noise in a
    // log people read.
    static MIGRATED: std::sync::Once = std::sync::Once::new();
    MIGRATED.call_once(|| {
        migrate_legacy_user_file(&crate::profile::active());
    });

    let mut layers = vec![builtin_layer()];

    let mut ids: Vec<String> = std::fs::read_dir(plugin_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();

    for id in ids {
        let path = plugin_dir().join(&id).join("flags.json");
        if let Some(layer) = read_layer(&path, Source::Plugin(id)) {
            layers.push(layer);
        }
    }

    if let Some(layer) = read_layer(&user_path(), Source::User) {
        layers.push(layer);
    }
    layers
}

/// Report what was resolved, naming every conflict.
///
/// Printed rather than silent because a flag doing nothing — because something
/// else set it — is otherwise indistinguishable from a flag that had no effect.
pub fn report(resolved: &BTreeMap<String, Resolved>) {
    if resolved.is_empty() {
        return;
    }
    let conflicts: Vec<_> = resolved.iter().filter(|(_, r)| !r.overridden.is_empty()).collect();
    println!("  flags: {} override(s) applied", resolved.len());
    for (key, r) in conflicts {
        let losers = r
            .overridden
            .iter()
            .map(|(s, v)| format!("{}={v}", s.describe()))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  flags: {key} = {} from {} (overrides {losers})",
            r.value,
            r.source.describe()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(source: Source, pairs: &[(&str, &str)]) -> Layer {
        Layer {
            source,
            values: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn the_user_beats_a_plugin() {
        let r = resolve(vec![
            layer(Source::Plugin("fps".into()), &[("DFFlagX", "false")]),
            layer(Source::User, &[("DFFlagX", "true")]),
        ]);
        assert_eq!(r["DFFlagX"].value, "true");
        assert_eq!(r["DFFlagX"].source, Source::User);
    }

    #[test]
    fn a_losing_value_is_kept_so_the_conflict_can_be_explained() {
        let r = resolve(vec![
            layer(Source::Plugin("fps".into()), &[("DFFlagX", "false")]),
            layer(Source::User, &[("DFFlagX", "true")]),
        ]);
        assert_eq!(
            r["DFFlagX"].overridden,
            vec![(Source::Plugin("fps".into()), "false".to_string())]
        );
    }

    #[test]
    fn two_plugins_disagreeing_is_recorded_not_hidden() {
        let r = resolve(vec![
            layer(Source::Plugin("a".into()), &[("FIntQ", "1")]),
            layer(Source::Plugin("b".into()), &[("FIntQ", "2")]),
        ]);
        // Deterministic — later layer wins — but the disagreement survives.
        assert_eq!(r["FIntQ"].value, "2");
        assert_eq!(r["FIntQ"].overridden.len(), 1);
        assert_eq!(r["FIntQ"].overridden[0].0, Source::Plugin("a".into()));
    }

    #[test]
    fn untouched_flags_carry_no_conflict() {
        let r = resolve(vec![layer(Source::User, &[("FFlagY", "true")])]);
        assert!(r["FFlagY"].overridden.is_empty());
        assert_eq!(r["FFlagY"].source, Source::User);
    }

    /// `XDG_CONFIG_HOME` and `CORDIAL_FLAGS` are process-wide, and cargo runs
    /// these in parallel threads of one process. Same reasoning as
    /// `profile`'s own test mutex, which records that the unserialised version
    /// passed anyway on its first run.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn scratch(tag: &str) -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("cordial-flags-test-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::remove_var("CORDIAL_FLAGS");
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        (root, guard)
    }

    #[test]
    fn one_profiles_overrides_are_not_another_profiles() {
        // A flag set while debugging on a test account used to be set on the
        // account someone plays, because there was one file for the machine.
        let (root, _g) = scratch("isolation");
        let alt = root.join("profiles/alt");
        let main = root.join("profiles/main");
        std::fs::create_dir_all(&alt).unwrap();
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(user_path_in(&alt), r#"{"DFFlagDebugSomething":"true"}"#).unwrap();

        assert!(read_layer(&user_path_in(&alt), Source::User).is_some());
        assert!(
            read_layer(&user_path_in(&main), Source::User).is_none(),
            "an override made in another profile must not apply here"
        );
    }

    #[test]
    fn a_pre_existing_global_overrides_file_is_moved_into_the_profile() {
        // Leaving it to be ignored would look exactly like overrides never
        // having worked, which is a bug report about the wrong thing.
        let (root, _g) = scratch("migrate");
        let profile = root.join("profiles/default");
        std::fs::create_dir_all(&profile).unwrap();
        let legacy = legacy_user_path();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"FIntTaskSchedulerAutoThreadLimit":"8"}"#).unwrap();

        let moved = migrate_legacy_user_file(&profile).expect("the legacy file should have moved");
        assert_eq!(moved, user_path_in(&profile));
        assert!(!legacy.exists(), "the old file should be gone, not copied");
        let layer = read_layer(&moved, Source::User).expect("the moved file should read");
        assert_eq!(layer.values["FIntTaskSchedulerAutoThreadLimit"], "8");
    }

    #[test]
    fn migration_never_overwrites_overrides_this_profile_already_has() {
        let (root, _g) = scratch("migrate-existing");
        let profile = root.join("profiles/alt");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(user_path_in(&profile), r#"{"FIntQ":"1"}"#).unwrap();

        let legacy = legacy_user_path();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"FIntQ":"2"}"#).unwrap();

        assert!(migrate_legacy_user_file(&profile).is_none());
        let layer = read_layer(&user_path_in(&profile), Source::User).unwrap();
        assert_eq!(layer.values["FIntQ"], "1", "the profile's own file must win");
        assert!(legacy.exists(), "and the old file is left untouched");
    }
}
