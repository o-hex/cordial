//! Running plugins alongside the client.
//!
//! Discovery, grants, spawning and the broker all live in `cordial-plugins`.
//! This is the join: it serves the methods those plugins call, backed by
//! Cordial's real state rather than a stand-in.
//!
//! One thread per plugin, each blocking on its own plugin's stdout. Plugins are
//! separate processes that mostly sit idle, so a thread each is the simple
//! correct thing; there is no shared mutable state between them because the
//! broker's decisions are per plugin and made before dispatch.

use cordial_plugins::broker::Broker;
use cordial_plugins::enablement;
use cordial_plugins::host::{authorise, Plugin as PluginProc};
use cordial_plugins::presence::{DiscordPresence, PresencePayload};
use cordial_plugins::protocol::{Request, Response};
use cordial_plugins::settings::{self, Store};
use cordial_plugins::{grants, manifest};
use std::path::PathBuf;

/// Start every approved plugin. Returns how many are running.
///
/// Never fails the launch. A plugin that will not start is reported and skipped:
/// the client working without a plugin is a much better outcome than a plugin
/// stopping the client.
pub fn start_all() -> usize {
    let root = manifest::plugin_root();
    let found = manifest::discover(&root);
    if found.is_empty() {
        return 0;
    }

    // Plugin *code* is installed once for the machine; what a plugin is
    // allowed to do, and anything it remembers, belong to the profile
    // (ADR-013). An approval given in a throwaway profile is not an approval
    // here, so the grants are read from this profile and nowhere else.
    let profile = crate::profile::active();
    grants::migrate_legacy_into(&profile);
    let approved = grants::load(&grants::path_in(&profile));
    let store = Store::new(&profile);
    let mut started = 0usize;

    for plugin in found {
        let id = plugin.manifest.id.clone();

        // Checked before anything about grants, and before the process is
        // spawned at all. The bug this fixes was `start_all` never reading
        // `plugin-enabled.json` in the first place, so Settings' switch wrote
        // a file nothing consulted; a plugin that started and then had every
        // request refused would still be the "stub that lies" shape AGENTS.md
        // warns about, just moved into a process. Absence in the file means
        // enabled — `enablement::is_enabled` already encodes that, and a
        // plugin nobody has an opinion about must keep running subject to its
        // grants, the same as before this change, or turning this bug off
        // would quietly turn a different one on.
        if !enabled_in_profile(&profile, &id) {
            println!("  plugin {id}: disabled in Settings, not started");
            continue;
        }

        let granted = approved.get(&id).cloned().unwrap_or_default();

        // Say what was withheld. A plugin silently doing less than it asked for
        // is otherwise indistinguishable from a plugin that is broken.
        let withheld: Vec<_> =
            plugin.requested.iter().filter(|c| !granted.contains(c)).copied().collect();
        if !withheld.is_empty() {
            let names: Vec<_> = withheld.iter().map(|c| c.name()).collect();
            println!("  plugin {id}: not granted {}", names.join(", "));
        }
        if granted.is_empty() {
            println!("  plugin {id}: no capabilities granted, not started");
            continue;
        }

        let entry = match plugin.entry_path() {
            Ok(e) => e,
            Err(e) => {
                println!("  plugin {id}: {e}");
                continue;
            }
        };
        match PluginProc::spawn(&id, &entry) {
            Ok(mut proc) => {
                // The handshake, before the plugin has asked for anything, so
                // that reading its own configuration — the first thing most
                // plugins do — costs no round trip. Best effort: a plugin that
                // died on startup is reported by its stdout closing, not here.
                let _ = proc.push(&settings::init_push(Some(&store), &id, &granted));

                let mut broker = Broker::new();
                broker.grant(&id, granted);
                let store = store.clone();
                std::thread::Builder::new()
                    .name(format!("plugin:{id}"))
                    .spawn(move || serve(proc, broker, store))
                    .ok();
                started += 1;
                println!("  plugin {id}: started");
            }
            Err(e) => println!("  plugin {id}: could not start ({e})"),
        }
    }
    started
}

/// Whether `id` is allowed to run in `profile_dir`, per Settings' plugin
/// toggle (`cordial_plugins::enablement`).
///
/// A thin wrapper rather than calling `enablement::is_enabled` straight from
/// `start_all`, so this file's own tests can exercise the decision on a
/// scratch profile directory without going through manifest discovery and
/// process spawning to do it.
fn enabled_in_profile(profile_dir: &std::path::Path, id: &str) -> bool {
    enablement::is_enabled(profile_dir, id)
}

fn serve(mut proc: PluginProc, mut broker: Broker, store: Store) {
    let id = proc.id.clone();
    // One Discord connection per plugin thread, held for the plugin's whole
    // run rather than opened fresh on every call — Session does the same for
    // the same reason: a plugin that calls presence.set on every tick must
    // not hand-shake with Discord that often.
    let mut presence = DiscordPresence::new();
    while let Some(req) = proc.next_request() {
        let req = match req {
            Ok(r) => r,
            Err(e) => {
                println!("  plugin {id}: sent something unreadable ({e})");
                break;
            }
        };
        let response = match authorise(&mut broker, &id, &req) {
            Err(refusal) => refusal,
            Ok(()) => dispatch(&id, &req, &store, &mut presence),
        };
        if proc.reply(&response).is_err() {
            break;
        }
    }
    proc.kill();
}

/// Serve one authorised request. The broker has already decided this may
/// proceed, so this only has to do the work.
fn dispatch(id: &str, req: &Request, store: &Store, presence: &mut DiscordPresence) -> Response {
    match req.method.as_str() {
        // `id` is this thread's own plugin — the process on the other end of
        // its pipe — and it is the only id the settings broker is given. A
        // plugin naming another one in its params reads and writes its own
        // document; see cordial_plugins::settings.
        "settings.get" | "settings.set" => settings::serve(Some(store), id, req),
        // ADR-007's worked example, finally reachable from the host the
        // client actually runs: cordial-plugins already speaks Discord's IPC
        // framing (presence.rs) and cordial_plugins::host::Session already
        // wires it up, but Session is only ever constructed in that crate's
        // own tests. This is the same wiring, once, for the real host —
        // reusing DiscordPresence rather than re-opening the socket search
        // here, so there is exactly one place that knows where Discord's IPC
        // socket might be.
        "presence.set" => match PresencePayload::parse(&req.params) {
            Ok(payload) => respond(req.id, presence.set(&payload)),
            Err(message) => Response::Error { id: req.id, message },
        },
        "presence.clear" => respond(req.id, presence.clear()),
        // Acknowledges the capability the way Session's does: there is
        // nothing to subscribe to yet, because delivering a lifecycle event
        // means the client's own run loop pushing one down this plugin's
        // stdin at the moment it happens, and nothing in this file's reach
        // owns that loop. Answering Ok here is honest about what it claims —
        // "you hold lifecycle.read" — and not about delivery, which stays
        // unimplemented rather than silently promised.
        "lifecycle.subscribe" => Response::Ok { id: req.id, result: serde_json::Value::Null },
        "flags.list" => {
            let resolved = crate::flags::resolve(crate::flags::collect());
            let list: Vec<_> = resolved
                .iter()
                .map(|(k, r)| {
                    serde_json::json!({
                        "key": k,
                        "value": r.value,
                        "source": r.source.describe(),
                    })
                })
                .collect();
            Response::Ok { id: req.id, result: serde_json::Value::Array(list) }
        }
        "flags.get" => {
            let key = req.params.get("key").and_then(|v| v.as_str()).unwrap_or_default();
            let resolved = crate::flags::resolve(crate::flags::collect());
            let value = resolved.get(key).map(|r| {
                serde_json::json!({ "value": r.value, "source": r.source.describe() })
            });
            Response::Ok { id: req.id, result: value.unwrap_or(serde_json::Value::Null) }
        }
        "log.write" => {
            let msg = req.params.get("message").and_then(|v| v.as_str()).unwrap_or("");
            println!("  [{id}] {msg}");
            Response::Ok { id: req.id, result: serde_json::Value::Null }
        }
        // Authorised but not implemented yet. Distinct from `denied`, which
        // would send an author looking for a permission that was never the
        // problem.
        other => Response::Error {
            id: req.id,
            message: format!("{other} is not implemented yet"),
        },
    }
}

/// Turn a broker effect's plain `Result` into the wire `Response` — the
/// success case carries nothing back, so this only exists to spell the
/// error case the same way every time. Copied from
/// `cordial_plugins::host::respond` rather than imported: it is three lines,
/// and pulling it in would mean making it `pub` in a crate whose own doc
/// comment says `Session` is the only thing with a real broker for this.
fn respond(id: u64, result: Result<(), String>) -> Response {
    match result {
        Ok(()) => Response::Ok { id, result: serde_json::Value::Null },
        Err(message) => Response::Error { id, message },
    }
}

/// Where plugins are installed, exposed so the loader can report it.
pub fn root() -> PathBuf {
    manifest::plugin_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str, params: serde_json::Value) -> Request {
        Request { id: 1, method: method.into(), params }
    }

    fn scratch_store(tag: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("cordial-plugin-host-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Store::new(dir)
    }

    // XDG_RUNTIME_DIR is process-wide, and cargo runs this file's tests on
    // multiple threads by default; presence.rs's own tests take the same
    // lock for the same reason. Held for as long as the env var points at a
    // scratch directory, so the two tests below cannot race each other's
    // socket search.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn presence_set_fails_honestly_when_discord_is_not_running() {
        // AGENTS.md: a stub must never claim success it did not have. With
        // no Discord IPC socket present, dispatch must answer Error, not Ok
        // — the exact failure this dispatch arm exists to reach past the
        // "not implemented yet" catch-all in dispatch's `other` arm, and the
        // failure it must still report honestly now that it does.
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir()
            .join(format!("cordial-plugin-host-no-discord-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);

        let store = scratch_store("presence-set");
        let mut presence = DiscordPresence::new();
        let req = call(
            "presence.set",
            serde_json::json!({"client_id": "1234567890123456", "details": "Playing Baseplate"}),
        );
        let res = dispatch("discord-presence", &req, &store, &mut presence);
        match res {
            Response::Error { message, .. } => assert!(message.contains("not running"), "{message}"),
            other => panic!("expected an honest failure with no Discord listening, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn presence_clear_is_a_quiet_no_op_when_nothing_was_ever_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir()
            .join(format!("cordial-plugin-host-clear-noop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);

        let store = scratch_store("presence-clear");
        let mut presence = DiscordPresence::new();
        let req = call("presence.clear", serde_json::Value::Null);
        let res = dispatch("discord-presence", &req, &store, &mut presence);
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_presence_payload_is_an_error_not_a_panic() {
        let store = scratch_store("presence-bad-payload");
        let mut presence = DiscordPresence::new();
        let req = call("presence.set", serde_json::json!({"client_id": "not-a-snowflake"}));
        let res = dispatch("discord-presence", &req, &store, &mut presence);
        match res {
            Response::Error { message, .. } => assert!(message.contains("snowflake"), "{message}"),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_subscribe_acknowledges_the_capability() {
        let store = scratch_store("lifecycle-subscribe");
        let mut presence = DiscordPresence::new();
        let req = call("lifecycle.subscribe", serde_json::Value::Null);
        let res = dispatch("some-plugin", &req, &store, &mut presence);
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");
    }

    #[test]
    fn an_unimplemented_method_still_says_so_rather_than_pretending() {
        // The catch-all this change carves presence and lifecycle.subscribe
        // out of must still hold for everything else `Session` answers that
        // this host does not yet wire up.
        let store = scratch_store("unimplemented");
        let mut presence = DiscordPresence::new();
        let req = call("notify.send", serde_json::json!({"summary": "hi"}));
        let res = dispatch("some-plugin", &req, &store, &mut presence);
        match res {
            Response::Error { message, .. } => assert!(message.contains("not implemented yet"), "{message}"),
            other => panic!("expected the not-implemented-yet stub, got {other:?}"),
        }
    }

    // The bug this file exists to fix: `start_all` discovered every plugin
    // with a nonempty grant and never once asked `enablement::is_enabled`,
    // so Settings' switch wrote `plugin-enabled.json` and nothing read it
    // back. These exercise the same decision `start_all` now makes —
    // `enabled_in_profile`, called before a plugin's grants are even looked
    // at — on a scratch profile directory, rather than against real
    // discovered plugins and spawned processes, which is what `start_all`
    // itself talks to and is not something a unit test should stand up.

    fn scratch_profile(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-plugin-host-enablement-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_plugin_explicitly_disabled_does_not_start() {
        let dir = scratch_profile("disabled");
        enablement::set_enabled(&dir, "flag-inspector", false).unwrap();
        assert!(
            !enabled_in_profile(&dir, "flag-inspector"),
            "Settings turned this off; start_all must not spawn it"
        );
    }

    #[test]
    fn a_plugin_explicitly_enabled_does_start() {
        let dir = scratch_profile("enabled");
        // Written explicitly rather than left absent, so this test is
        // distinct from the absence case below: this covers the entry
        // reading `true`, not merely "nobody wrote anything".
        enablement::set_enabled(&dir, "flag-inspector", false).unwrap();
        enablement::set_enabled(&dir, "flag-inspector", true).unwrap();
        assert!(enabled_in_profile(&dir, "flag-inspector"));
    }

    #[test]
    fn a_plugin_absent_from_the_file_defaults_to_enabled() {
        // The design question this change had to answer: an installed
        // plugin the user has never touched must not be silently disabled
        // by wiring `start_all` up to enablement. `enablement.rs`'s own
        // contract is "absence means enabled" (see its module docs and
        // `is_enabled`); this asserts that `start_all`'s call site actually
        // gets that answer, rather than assuming the wrapper forwards it
        // correctly.
        let dir = scratch_profile("absent");
        assert!(!std::path::Path::new(&enablement::path_in(&dir)).exists());
        assert!(enabled_in_profile(&dir, "a-plugin-nobody-has-an-opinion-about"));
    }
}
