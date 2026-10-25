//! What the Plugins page lists, worked out without a display.
//!
//! The page used to show installed plugins in one group and the folders loaded
//! for development in another, as bare paths that said nothing about whether
//! the plugin in them loaded. A development plugin *is* an installed plugin as
//! far as the person using it is concerned -- it has a switch, permissions and
//! a health line -- so the page lists them together, and this module decides
//! what goes in that list. It takes no GTK and reads only the folders it is
//! handed, which is what lets the merge and the removal be tested.
//!
//! The rules below are the runtime's, restated rather than shared: in
//! `cordial_runtime::plugin_host::start_all` an unpacked folder whose id is
//! already taken by a built-in or an installed plugin is not used, and a
//! second folder with an id an earlier one already has is not used either. A
//! row that offered a switch for a plugin the client will never start would be
//! a control that does nothing.

use std::collections::BTreeSet;
use std::path::Path;

use cordial_plugins::manifest::{self, Plugin};

/// One folder from the Developing a plugin list, and what became of it.
#[derive(Debug)]
pub struct DevEntry {
    /// The folder as it is written in `shell.json`. Kept as text, not a path,
    /// because that exact string is what removal has to match.
    pub dir: String,
    pub state: DevState,
}

#[derive(Debug)]
pub enum DevState {
    /// Parsed, and nothing else claims its id: the client will start it.
    Loaded(Plugin),
    /// The folder is missing, has no readable `plugin.json`, or the manifest
    /// does not parse. Carries the reason in a form fit for a subtitle.
    Unusable(String),
    /// Parsed, but an earlier plugin has the same id, so the client skips it.
    Shadowed { id: String, by: Shadowing },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Shadowing {
    /// A built-in or installed plugin.
    Installed,
    /// An earlier folder in the same list.
    EarlierFolder,
}

/// An entry on the Installed list.
#[derive(Debug)]
pub enum Listed {
    Installed(Plugin),
    Development(DevEntry),
}

/// Read the plugin in `dir`, with an error a person can act on.
pub fn read_folder(dir: &str) -> Result<Plugin, String> {
    let path = Path::new(dir).join("plugin.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} could not be read ({e})", path.display()))?;
    manifest::parse(&text, Path::new(dir)).map_err(|e| format!("plugin.json is not usable ({e})"))
}

/// Classify every folder, in list order. `taken` is the ids of the built-in and
/// installed plugins.
pub fn dev_entries(dirs: &[String], taken: &BTreeSet<String>) -> Vec<DevEntry> {
    dev_entries_with(dirs, taken, read_folder)
}

fn dev_entries_with(
    dirs: &[String],
    taken: &BTreeSet<String>,
    read: impl Fn(&str) -> Result<Plugin, String>,
) -> Vec<DevEntry> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let state = match read(dir) {
            Err(why) => DevState::Unusable(why),
            Ok(plugin) => {
                let id = plugin.manifest.id.clone();
                if taken.contains(&id) {
                    DevState::Shadowed { id, by: Shadowing::Installed }
                } else if !seen.insert(id.clone()) {
                    DevState::Shadowed { id, by: Shadowing::EarlierFolder }
                } else {
                    DevState::Loaded(plugin)
                }
            }
        };
        out.push(DevEntry { dir: dir.clone(), state });
    }
    out
}

/// Installed plugins first, in the order discovery found them, then the
/// development folders in the order they were added.
pub fn merge(installed: Vec<Plugin>, development: Vec<DevEntry>) -> Vec<Listed> {
    installed
        .into_iter()
        .map(Listed::Installed)
        .chain(development.into_iter().map(Listed::Development))
        .collect()
}

/// Take `dir` out of the list. Returns whether it was there.
///
/// This is the whole of "remove": the entry in Cordial's own list, and
/// nothing on disk. The folder belongs to the person who wrote the plugin.
pub fn remove_folder(list: &mut Vec<String>, dir: &str) -> bool {
    let before = list.len();
    list.retain(|d| d != dir);
    list.len() != before
}

/// The subtitle of a development row: the folder, then the status line.
pub fn dev_subtitle(dir: &str, status: &str) -> String {
    format!("{dir}\n{status}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(id: &str, dir: &str) -> Plugin {
        manifest::parse(&format!(r#"{{"id":"{id}","name":"{id}","entry":"main.ts"}}"#), Path::new(dir))
            .expect("well-formed test manifest")
    }

    fn reader(known: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Result<Plugin, String> {
        move |dir| {
            known
                .iter()
                .find(|(d, _)| *d == dir)
                .map(|(d, id)| plugin(id, d))
                .ok_or_else(|| format!("{dir}/plugin.json could not be read (no such file)"))
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_readable_folder_is_loaded_and_a_missing_one_says_why() {
        let taken = BTreeSet::new();
        let got = dev_entries_with(&strings(&["/a", "/gone"]), &taken, reader(&[("/a", "alpha")]));
        assert!(matches!(&got[0].state, DevState::Loaded(p) if p.manifest.id == "alpha"));
        assert!(matches!(&got[1].state, DevState::Unusable(why) if why.contains("/gone")));
    }

    #[test]
    fn an_id_an_installed_plugin_has_is_marked_as_not_used() {
        let taken: BTreeSet<String> = ["alpha".to_string()].into();
        let got = dev_entries_with(&strings(&["/a"]), &taken, reader(&[("/a", "alpha")]));
        assert!(matches!(&got[0].state, DevState::Shadowed { by: Shadowing::Installed, .. }));
    }

    #[test]
    fn the_second_folder_with_one_id_is_the_one_marked_as_not_used() {
        let got = dev_entries_with(
            &strings(&["/a", "/b"]),
            &BTreeSet::new(),
            reader(&[("/a", "same"), ("/b", "same")]),
        );
        assert!(matches!(&got[0].state, DevState::Loaded(_)));
        assert!(matches!(&got[1].state, DevState::Shadowed { by: Shadowing::EarlierFolder, .. }));
    }

    #[test]
    fn the_list_has_installed_plugins_then_development_folders_in_order() {
        let dev = dev_entries_with(
            &strings(&["/z", "/y"]),
            &BTreeSet::new(),
            reader(&[("/z", "zed"), ("/y", "why")]),
        );
        let merged = merge(vec![plugin("one", "/i/one"), plugin("two", "/i/two")], dev);
        let names: Vec<String> = merged
            .iter()
            .map(|l| match l {
                Listed::Installed(p) => format!("installed:{}", p.manifest.id),
                Listed::Development(d) => format!("development:{}", d.dir),
            })
            .collect();
        assert_eq!(names, ["installed:one", "installed:two", "development:/z", "development:/y"]);
    }

    #[test]
    fn an_empty_development_list_leaves_installed_plugins_alone() {
        assert_eq!(merge(vec![plugin("one", "/i/one")], Vec::new()).len(), 1);
    }

    #[test]
    fn removing_a_folder_takes_only_that_entry_out_of_the_list() {
        let mut list = strings(&["/a", "/b", "/c"]);
        assert!(remove_folder(&mut list, "/b"));
        assert_eq!(list, ["/a", "/c"]);
        assert!(!remove_folder(&mut list, "/b"), "removing twice reports that nothing was there");
        assert_eq!(list, ["/a", "/c"]);
    }

    #[test]
    fn removing_from_the_list_does_not_touch_the_folder() {
        let dir = std::env::temp_dir().join(format!("cordial-listing-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("plugin.json");
        std::fs::write(&manifest, r#"{"id":"kept","name":"Kept","entry":"main.ts"}"#).unwrap();
        let text = dir.display().to_string();

        let mut list = vec![text.clone()];
        assert!(remove_folder(&mut list, &text));
        assert!(list.is_empty());
        assert!(manifest.is_file(), "the source folder must survive removal from the list");

        std::fs::remove_file(&manifest).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn a_real_folder_reads_back_through_the_real_reader() {
        let dir = std::env::temp_dir().join(format!("cordial-listing-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), r#"{"id":"real","name":"Real","entry":"main.ts"}"#).unwrap();
        let text = dir.display().to_string();
        let got = dev_entries(&[text.clone()], &BTreeSet::new());
        assert!(matches!(&got[0].state, DevState::Loaded(p) if p.manifest.id == "real"));
        std::fs::remove_file(dir.join("plugin.json")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert!(matches!(dev_entries(&[text], &BTreeSet::new())[0].state, DevState::Unusable(_)));
    }
}
