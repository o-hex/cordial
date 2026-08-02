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
use cordial_plugins::host::{authorise, Plugin as PluginProc};
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

fn serve(mut proc: PluginProc, mut broker: Broker, store: Store) {
    let id = proc.id.clone();
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
            Ok(()) => dispatch(&id, &req, &store),
        };
        if proc.reply(&response).is_err() {
            break;
        }
    }
    proc.kill();
}

/// Serve one authorised request. The broker has already decided this may
/// proceed, so this only has to do the work.
fn dispatch(id: &str, req: &Request, store: &Store) -> Response {
    match req.method.as_str() {
        // `id` is this thread's own plugin — the process on the other end of
        // its pipe — and it is the only id the settings broker is given. A
        // plugin naming another one in its params reads and writes its own
        // document; see cordial_plugins::settings.
        "settings.get" | "settings.set" => settings::serve(Some(store), id, req),
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

/// Where plugins are installed, exposed so the loader can report it.
pub fn root() -> PathBuf {
    manifest::plugin_root()
}
