//! Holds `plugins/index.example.json` to what it documents.
//!
//! The example index is the only description of the format anybody is likely to
//! copy, so it has to parse, it has to resolve, and it has to agree with the
//! manifests of the plugins that actually ship in this repository. An example
//! that has quietly stopped matching the code is worse than no example: it
//! looks authoritative, and the person following it has no reason to doubt it.

use cordial_plugins::manifest::{self, Dependency};
use cordial_plugins::registry::Index;
use cordial_plugins::resolve;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn plugins_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
}

fn example() -> Index {
    let text = std::fs::read_to_string(plugins_dir().join("index.example.json"))
        .expect("the example index should be in plugins/");
    Index::parse_unverified(&text).expect("the example index should parse")
}

#[test]
fn the_example_index_agrees_with_the_plugins_that_ship_here() {
    // The two first-party plugins are in the example, so the example is a
    // place the real manifests can drift away from. Anyone adding a capability
    // to `discord-presence` and not to its index entry finds out here rather
    // than at install time, where it presents as a `ManifestMismatch` from a
    // published archive nobody can reproduce.
    let index = example();
    for plugin in manifest::discover(&plugins_dir()) {
        let Some(published) = index.entries.iter().find(|e| e.id == plugin.manifest.id) else {
            continue;
        };
        assert_eq!(
            plugin.version.as_ref(),
            Some(&published.version),
            "{} declares a different version than the example index publishes",
            plugin.manifest.id
        );
        assert_eq!(
            plugin.requested, published.capabilities,
            "{} requests different capabilities than the example index publishes",
            plugin.manifest.id
        );
        assert_eq!(
            plugin.dependencies, published.dependencies,
            "{} declares different dependencies than the example index publishes",
            plugin.manifest.id
        );
    }
}

#[test]
fn the_example_index_resolves_a_dependency_into_an_install_order() {
    let index = example();
    let wanted = vec![Dependency::new("flag-profiles", "^0.3.0").unwrap()];
    let plan = resolve::resolve(&index, &wanted).unwrap();
    let order: Vec<&str> = plan.steps.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        order,
        ["flag-inspector", "flag-profiles"],
        "a dependency installs, and starts, before the plugin that needs it"
    );
}

#[test]
fn resolving_the_example_refuses_an_install_the_user_has_not_approved() {
    // `flag-profiles` depends on `flag-inspector`, which asks for `flags.read`
    // and `log`. Approving the first must not silently approve the second.
    let index = example();
    let wanted = vec![Dependency::new("flag-profiles", "^0.3.0").unwrap()];
    let mut granted: BTreeMap<String, BTreeSet<_>> = BTreeMap::new();
    granted.insert(
        "flag-profiles".into(),
        index
            .entries
            .iter()
            .find(|e| e.id == "flag-profiles")
            .unwrap()
            .capabilities
            .clone(),
    );

    let e = resolve::plan(&index, &wanted, &granted).unwrap_err();
    let message = e.to_string();
    assert!(
        message.contains("flag-inspector") && message.contains("flag-profiles"),
        "the refusal should say which plugin pulled the other in: {message}"
    );
}
