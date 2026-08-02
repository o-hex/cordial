//! Proves the settings broker against a real, separately spawned plugin
//! process: the handshake really carries what was on disk, a save really
//! lands in the profile, and a plugin asking for another plugin's document
//! gets its own.
//!
//! That last one is the test worth having. The unit tests in `settings.rs`
//! check the same property against `serve` directly; this one drives it from
//! the other side of a pipe, through `Session::handle`, from a process that
//! chose the words of the request itself. Delete the id check — or grow a
//! `plugin` parameter on `settings.get` — and `neighbour`'s secret appears in
//! what this plugin reports back.
//!
//! Each side names its own fields. The fixture reports under
//! `handshakeSaw`/`askingForNeighbourReturned`/`afterSaving` and this file
//! asserts on those names and on the file it can read for itself, so a
//! mismatch between the two halves fails rather than passing through
//! unexamined — this crate has a note about a round-trip test that passed with
//! both sides deliberately disagreeing.

use cordial_plugins::capability::Capability;
use cordial_plugins::host::{Plugin, Session};
use cordial_plugins::protocol::Response;
use cordial_plugins::settings::Store;
use std::path::PathBuf;

#[test]
fn a_real_plugin_gets_its_own_settings_and_only_its_own() {
    if std::process::Command::new("deno").arg("--version").output().is_err() {
        eprintln!("skipping: deno is not installed");
        return;
    }

    // A scratch profile. Not `CORDIAL_PROFILE_ROOT` and not an environment
    // variable of any kind: a `Store` is handed the directory it works in, the
    // same way the client will be handed the profile it was launched with.
    let profile = std::env::temp_dir().join(format!("cordial-settings-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile).unwrap();

    let store = Store::new(&profile);
    store.write("settings-demo", &serde_json::json!({"panel": "presence", "opened": 1})).unwrap();
    store.write("neighbour", &serde_json::json!({"secret": "cookie"})).unwrap();

    let mut session = Session::with_profile(&profile);
    session.broker.grant(
        "settings-demo",
        [Capability::SettingsRead, Capability::SettingsWrite, Capability::Log],
    );

    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/settings.ts");
    let proc = Plugin::spawn("settings-demo", &entry).expect("deno should start");
    // Grants first, then adopt: the handshake is built from the grant, so
    // adopting first would hand the plugin a handshake saying it holds nothing.
    session.add_plugin(proc);

    let mut report = None;
    while let Some(req) = session.plugin_mut("settings-demo").unwrap().next_request() {
        let Ok(req) = req else { break };
        if req.method == "log.write" {
            // Session has no broker for log.write — out of that module's scope
            // — so the test plays host for it, the way flag_inspector.rs does.
            report = Some(req.params["message"].as_str().unwrap_or_default().to_string());
            session
                .plugin_mut("settings-demo")
                .unwrap()
                .reply(&Response::Ok { id: req.id, result: serde_json::Value::Null })
                .unwrap();
            break;
        }
        let res = session.handle("settings-demo", &req);
        session.plugin_mut("settings-demo").unwrap().reply(&res).unwrap();
    }
    session.plugin_mut("settings-demo").unwrap().kill();

    let report = report.expect("the plugin should have reported what it saw");
    let report: serde_json::Value =
        serde_json::from_str(&report).unwrap_or_else(|e| panic!("{e}; got: {report}"));

    // The handshake, so the common case costs no round trip.
    assert_eq!(
        report["handshakeSaw"],
        serde_json::json!({"panel": "presence", "opened": 1}),
        "the handshake should have carried what was on disk; got {report}"
    );

    // The escape attempt. `neighbour`'s document is on disk and the plugin
    // asked for it by name three different ways.
    let nosey = &report["askingForNeighbourReturned"];
    assert!(
        nosey.get("secret").is_none(),
        "a plugin read another plugin's settings: {report}"
    );
    assert_eq!(
        nosey["panel"], "presence",
        "asking for another plugin should return the caller's own document; got {report}"
    );
    assert_eq!(
        store.read("neighbour").unwrap()["secret"],
        "cookie",
        "and the neighbour's document must be untouched"
    );

    // The save, checked both as the plugin read it back and as it sits on
    // disk, in the profile, under the plugin's own id.
    assert_eq!(report["afterSaving"], serde_json::json!({"panel": "flags", "opened": 4}));
    let on_disk: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(profile.join("plugins/settings-demo/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk, serde_json::json!({"panel": "flags", "opened": 4}));

    let _ = std::fs::remove_dir_all(&profile);
}
