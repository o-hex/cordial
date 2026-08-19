//! Running plugins alongside the client.
//!
//! Discovery, grants, spawning and the broker all live in `cordial-plugins`.
//! This is the join: it serves the methods those plugins call, backed by
//! Cordial's real state rather than a stand-in.
//!
//! One thread per plugin, each blocking on its own plugin's stdout. Plugins are
//! separate processes that mostly sit idle, so a thread each is the simple
//! correct thing. The broker's own decisions are per plugin and made before
//! dispatch, so they need nothing shared — but two effects genuinely are
//! shared across every running plugin in this process, and [`Shared`] is
//! where that state actually lives:
//!
//! * the event registry (ADR-006), because declaring, publishing and
//!   subscribing all have to agree about the same namespaces regardless of
//!   which plugin's thread is asking; and
//! * every running plugin's writable stdin, because delivering a published
//!   event to a subscriber means writing into a *different* plugin's pipe
//!   from the thread serving the publisher's `events.publish` call —
//!   `cordial_plugins::host::Writer` is what makes that safe without also
//!   sharing the read half, which stays owned by the one thread that reads
//!   it.

use cordial_plugins::broker::Broker;
use cordial_plugins::events::EventRegistry;
use cordial_plugins::host::{authorise, Plugin as PluginProc, Writer};
use cordial_plugins::presence::{DiscordPresence, PresencePayload};
use cordial_plugins::protocol::{Push, Request, Response};
use cordial_plugins::settings::{self, Store};
use cordial_plugins::{enablement, grants, manifest, notify, urlopen};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// State shared by every plugin's serving thread within one Cordial run.
///
/// Fresh every launch, the same as `Broker` always is — nothing here persists
/// across a restart, and nothing here is visible outside this process.
#[derive(Clone)]
struct Shared {
    events: Arc<Mutex<EventRegistry>>,
    /// Every currently-running plugin's stdin, keyed by id, so a publisher's
    /// thread can push into a subscriber's pipe without becoming the thread
    /// that reads that subscriber's own stdout.
    writers: Arc<Mutex<BTreeMap<String, Writer>>>,
}

impl Shared {
    fn new() -> Self {
        Shared { events: Arc::new(Mutex::new(EventRegistry::new())), writers: Arc::new(Mutex::new(BTreeMap::new())) }
    }
}

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
    let shared = Shared::new();
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

                // Registered before the process is handed to its own thread:
                // another plugin's `events.publish` has to be able to find
                // this writer immediately, not only once this thread gets
                // around to inserting it, which would be a race against
                // whichever plugin started first getting to publish first.
                shared.writers.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), proc.writer());

                let mut broker = Broker::new();
                broker.grant(&id, granted);
                let store = store.clone();
                let shared = shared.clone();
                let plugin_dir = plugin.dir.clone();
                std::thread::Builder::new()
                    .name(format!("plugin:{id}"))
                    .spawn(move || serve(proc, broker, store, shared, plugin_dir))
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

fn serve(mut proc: PluginProc, mut broker: Broker, store: Store, shared: Shared, plugin_dir: PathBuf) {
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
            Ok(()) => dispatch(&id, &req, &store, &mut presence, &shared, &plugin_dir),
        };
        if proc.reply(&response).is_err() {
            break;
        }
    }
    // The plugin is gone; nothing should still be able to reach it. A
    // publish arriving after this looks up an id `writers` no longer has and
    // simply has one fewer subscriber to deliver to, and an asset overlay it
    // registered stops being consulted — falling straight back to whatever
    // would have resolved without it, because nothing was ever written to
    // undo (ADR-010). `unregister_plugin_root` is a no-op if this plugin
    // never registered one, so calling it unconditionally costs nothing.
    shared.writers.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    crate::android::asset::unregister_plugin_root(&id);
    proc.kill();
}

/// Serve one authorised request. The broker has already decided this may
/// proceed, so this only has to do the work.
fn dispatch(
    id: &str,
    req: &Request,
    store: &Store,
    presence: &mut DiscordPresence,
    shared: &Shared,
    plugin_dir: &Path,
) -> Response {
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
        // ADR-007's other two brokered effects: the plugin sends a payload,
        // Cordial owns the D-Bus connection. Neither call learns anything
        // about the bus it went over.
        "notify.send" => {
            let summary = req.params.get("summary").and_then(|v| v.as_str());
            let body = req.params.get("body").and_then(|v| v.as_str()).unwrap_or("");
            match summary {
                Some(summary) => respond(req.id, notify::send(summary, body)),
                None => Response::Error { id: req.id, message: "notify.send needs a summary".into() },
            }
        }
        "url.open" => match req.params.get("url").and_then(|v| v.as_str()) {
            Some(url) => respond(req.id, urlopen::open(url)),
            None => Response::Error { id: req.id, message: "url.open needs a url".into() },
        },
        // ADR-010: a subdirectory of the plugin's own installed directory,
        // never an arbitrary path — `resolve_within` refuses anything that
        // would name somewhere else, the same treatment `manifest::Plugin`
        // gives a manifest's `entry`. Registration only takes effect for as
        // long as this plugin's own thread is alive; `serve` unregisters it
        // unconditionally when the process ends, so a disabled or removed
        // plugin's overlay never outlives it.
        "assets.override" => {
            if req.params.get("clear").and_then(|v| v.as_bool()) == Some(true) {
                crate::android::asset::unregister_plugin_root(id);
                return Response::Ok { id: req.id, result: serde_json::Value::Null };
            }
            let rel = req.params.get("dir").and_then(|v| v.as_str()).unwrap_or("overlay");
            match resolve_within(plugin_dir, rel) {
                Ok(resolved) => {
                    let shown = resolved.display().to_string();
                    crate::android::asset::register_plugin_root(id, resolved);
                    Response::Ok { id: req.id, result: serde_json::json!({"registered": shown}) }
                }
                Err(message) => Response::Error { id: req.id, message },
            }
        }
        // `flags.write`: a plugin's contribution to its own, machine-global
        // `flags.json` (ADR-013's open question — this file stays global
        // regardless of which profile granted the capability). Takes effect
        // at the next launch only; there is no live counterpart here because
        // `FFlag`/`FInt`/`FString` are read once at startup (ADR-005), which
        // is the entire reason `flags.write` and `flags.write.dynamic` are
        // two separate capabilities rather than one.
        "flags.set" => {
            let Some(values) = req.params.get("values").and_then(|v| v.as_object()) else {
                return Response::Error { id: req.id, message: "flags.set needs a values object".into() };
            };
            let flat: BTreeMap<String, String> = values
                .iter()
                .map(|(k, v)| {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), s)
                })
                .collect();
            respond(req.id, crate::flags::write_plugin_layer(id, &flat))
        }
        // ADR-006. `id` namespaces `declare` and gates `publish` the same way
        // it gates `settings.*` above: it is Cordial's own record of which
        // process is on the pipe, never a field the request could set.
        "events.declare" => match req.params.get("name").and_then(|v| v.as_str()) {
            Some(name) => {
                let mut events = shared.events.lock().unwrap_or_else(|e| e.into_inner());
                match events.declare(id, name) {
                    Ok(event_type) => Response::Ok { id: req.id, result: serde_json::json!({"type": event_type}) },
                    Err(message) => Response::Error { id: req.id, message },
                }
            }
            None => Response::Error { id: req.id, message: "events.declare needs a name".into() },
        },
        "events.subscribe" => match req.params.get("type").and_then(|v| v.as_str()) {
            Some(event_type) => {
                let mut events = shared.events.lock().unwrap_or_else(|e| e.into_inner());
                match events.subscribe(id, event_type) {
                    Ok(()) => Response::Ok { id: req.id, result: serde_json::Value::Null },
                    Err(message) => Response::Error { id: req.id, message },
                }
            }
            None => Response::Error { id: req.id, message: "events.subscribe needs a type".into() },
        },
        "events.publish" => {
            let Some(event_type) = req.params.get("type").and_then(|v| v.as_str()) else {
                return Response::Error { id: req.id, message: "events.publish needs a type".into() };
            };
            let subscribers = {
                let events = shared.events.lock().unwrap_or_else(|e| e.into_inner());
                if !events.may_publish(id, event_type) {
                    return Response::Error {
                        id: req.id,
                        message: format!(
                            "{id:?} may not publish on {event_type:?}; it must declare that type before publishing on it"
                        ),
                    };
                }
                events.subscribers(event_type).into_iter().map(str::to_string).collect::<Vec<_>>()
            };
            let payload = req.params.get("payload").cloned().unwrap_or(serde_json::Value::Null);
            let writers = shared.writers.lock().unwrap_or_else(|e| e.into_inner());
            for subscriber in subscribers {
                if let Some(writer) = writers.get(&subscriber) {
                    // Best effort, the same as every other push in this
                    // project: a subscriber that has already died is not a
                    // reason to fail the publisher's call, and the write
                    // error here would only repeat what that subscriber's own
                    // thread is about to discover reading its closed stdout.
                    let _ = writer.push(&Push { event: event_type.to_string(), payload: payload.clone() });
                }
            }
            Response::Ok { id: req.id, result: serde_json::Value::Null }
        }
        // Authorised but not implemented yet. Distinct from `denied`, which
        // would send an author looking for a permission that was never the
        // problem. `flags.setDynamic` lands here permanently rather than
        // temporarily: it needs a live write into the running engine's own
        // `DFFlag` table, and nothing in this project has ever reached into
        // the engine process to do that — ADR-001 and ADR-003 rule out the
        // in-process access that would take, so this is not a gap waiting to
        // be filled, it is a capability whose effect has nowhere to live.
        other => Response::Error {
            id: req.id,
            message: format!("{other} is not implemented yet"),
        },
    }
}

/// A subdirectory of `base`, refusing anything that would name somewhere
/// else. `rel` comes from a plugin's own `assets.override` call and is
/// treated as attacker-controlled the same way `manifest::Plugin::entry_path`
/// treats a manifest's `entry` — both cross a trust boundary from a process
/// Cordial does not control, and both get the same refusal rather than a
/// path that is quietly rewritten into something safe.
fn resolve_within(base: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel_path.components().any(|c| c.as_os_str() == "..") {
        return Err(format!("{rel:?} must be a path inside the plugin's own directory"));
    }
    Ok(base.join(rel_path))
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

    /// A plugin's own installed directory, standing in for `plugin.dir` —
    /// `assets.override` resolves relative to this.
    fn scratch_plugin_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-plugin-host-dir-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("presence-set");
        let req = call(
            "presence.set",
            serde_json::json!({"client_id": "1234567890123456", "details": "Playing Baseplate"}),
        );
        let res = dispatch("discord-presence", &req, &store, &mut presence, &shared, &plugin_dir);
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
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("presence-clear");
        let req = call("presence.clear", serde_json::Value::Null);
        let res = dispatch("discord-presence", &req, &store, &mut presence, &shared, &plugin_dir);
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_presence_payload_is_an_error_not_a_panic() {
        let store = scratch_store("presence-bad-payload");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("presence-bad-payload");
        let req = call("presence.set", serde_json::json!({"client_id": "not-a-snowflake"}));
        let res = dispatch("discord-presence", &req, &store, &mut presence, &shared, &plugin_dir);
        match res {
            Response::Error { message, .. } => assert!(message.contains("snowflake"), "{message}"),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_subscribe_acknowledges_the_capability() {
        let store = scratch_store("lifecycle-subscribe");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("lifecycle-subscribe");
        let req = call("lifecycle.subscribe", serde_json::Value::Null);
        let res = dispatch("some-plugin", &req, &store, &mut presence, &shared, &plugin_dir);
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");
    }

    #[test]
    fn an_unimplemented_method_still_says_so_rather_than_pretending() {
        // The catch-all this change carves presence and lifecycle.subscribe
        // out of must still hold for everything else `Session` answers that
        // this host does not yet wire up.
        let store = scratch_store("unimplemented");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("unimplemented");
        // `flags.setDynamic` is the one capability with nowhere to route to:
        // it would need a live write into the running engine's own `DFFlag`
        // table, which nothing in this project reaches into (ADR-001,
        // ADR-003). Every other method this test used to check here —
        // notify.send, url.open, events.*, assets.override, flags.set — is
        // wired for real below and is no longer a stand-in for "not written
        // yet".
        let req = call("flags.setDynamic", serde_json::json!({"key": "DFFlagX", "value": "true"}));
        let res = dispatch("some-plugin", &req, &store, &mut presence, &shared, &plugin_dir);
        match res {
            Response::Error { message, .. } => assert!(message.contains("not implemented yet"), "{message}"),
            other => panic!("expected the not-implemented-yet stub, got {other:?}"),
        }
    }

    #[test]
    fn notify_send_without_a_summary_is_refused_before_touching_the_bus() {
        // The one part of `notify.send` this file can check without a real
        // session bus — the shape check happens before `notify::send` ever
        // opens a connection, matching `notify.rs`'s own coverage of the
        // same rule.
        let store = scratch_store("notify-no-summary");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("notify-no-summary");
        let req = call("notify.send", serde_json::json!({"body": "no summary here"}));
        let res = dispatch("some-plugin", &req, &store, &mut presence, &shared, &plugin_dir);
        match res {
            Response::Error { message, .. } => assert!(message.contains("summary"), "{message}"),
            other => panic!("expected a shape refusal, got {other:?}"),
        }
    }

    #[test]
    fn url_open_refuses_a_non_http_scheme_through_the_real_dispatch() {
        // The exact case ADR-007's doc comment on `UrlOpen` calls out, proven
        // past the capability gate this time — a granted-but-malicious call
        // reaching the real host's dispatch, not only `urlopen.rs`'s own
        // unit tests.
        let store = scratch_store("url-open-bad-scheme");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("url-open-bad-scheme");
        let req = call("url.open", serde_json::json!({"url": "file:///etc/passwd"}));
        let res = dispatch("some-plugin", &req, &store, &mut presence, &shared, &plugin_dir);
        match res {
            Response::Error { message, .. } => assert!(message.contains("refused"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn assets_override_registers_a_root_inside_the_plugins_own_directory() {
        let store = scratch_store("assets-register");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("assets-register");
        std::fs::create_dir_all(plugin_dir.join("overlay/textures")).unwrap();
        std::fs::write(plugin_dir.join("overlay/textures/wood.png"), b"fake texture bytes").unwrap();

        let req = call("assets.override", serde_json::json!({}));
        let res = dispatch("themer", &req, &store, &mut presence, &shared, &plugin_dir);
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");

        assert_eq!(
            crate::android::asset::explain("textures/wood.png"),
            Some("plugin:themer".to_string()),
            "the registered root should now be consulted ahead of the APK"
        );

        // And clearing it falls straight back to nothing being overlaid —
        // there was never a write to undo (ADR-010).
        let clear = call("assets.override", serde_json::json!({"clear": true}));
        let res = dispatch("themer", &clear, &store, &mut presence, &shared, &plugin_dir);
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");
        assert_eq!(crate::android::asset::explain("textures/wood.png"), None);
    }

    #[test]
    fn assets_override_refuses_a_directory_that_would_escape_the_plugin() {
        let store = scratch_store("assets-escape");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("assets-escape");

        for bad in ["../../etc", "/etc"] {
            let req = call("assets.override", serde_json::json!({"dir": bad}));
            let res = dispatch("themer", &req, &store, &mut presence, &shared, &plugin_dir);
            match res {
                Response::Error { message, .. } => assert!(message.contains("inside"), "{bad}: {message}"),
                other => panic!("{bad:?} should have been refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_plugin_removed_from_the_writer_map_stops_receiving_its_overlay() {
        // `serve` unregisters unconditionally when a plugin's process ends;
        // this exercises the same call `serve` makes, proving the overlay
        // genuinely stops resolving rather than lingering because nothing
        // ever tore it down.
        let store = scratch_store("assets-teardown");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("assets-teardown");
        std::fs::create_dir_all(plugin_dir.join("overlay")).unwrap();
        std::fs::write(plugin_dir.join("overlay/sound.ogg"), b"fake sound bytes").unwrap();

        let req = call("assets.override", serde_json::json!({}));
        dispatch("sound-pack", &req, &store, &mut presence, &shared, &plugin_dir);
        assert!(crate::android::asset::explain("sound.ogg").is_some());

        crate::android::asset::unregister_plugin_root("sound-pack");
        assert_eq!(crate::android::asset::explain("sound.ogg"), None);
    }

    #[test]
    fn flags_set_writes_the_plugins_own_global_flags_layer() {
        let store = scratch_store("flags-set");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("flags-set");
        let root = std::env::temp_dir().join("cordial-plugin-host-flags-set-plugindir");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // The same lock `flags.rs`'s own tests take before touching this
        // process-wide variable — see that module's note on why a
        // module-local mutex would not actually exclude this one.
        let _guard = crate::flags::tests::ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CORDIAL_PLUGIN_DIR", &root);

        let req = call("flags.set", serde_json::json!({"values": {"FFlagFoo": "true", "FIntBar": 3}}));
        let res = dispatch("tuner", &req, &store, &mut presence, &shared, &plugin_dir);
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");

        let layer = crate::flags::read_layer(
            &root.join("tuner/flags.json"),
            crate::flags::Source::Plugin("tuner".into()),
        )
        .expect("the written file should read back");
        assert_eq!(layer.values["FFlagFoo"], "true");
        assert_eq!(layer.values["FIntBar"], "3");
    }

    #[test]
    fn events_publish_is_refused_before_a_declare_through_the_real_dispatch() {
        // The same refusal `cordial_plugins::host::Session` proves in its own
        // tests, checked here against the real host's `dispatch` and its
        // shared registry, not the test-only `Session` construct.
        let store = scratch_store("events-undeclared");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("events-undeclared");
        let req = call(
            "events.publish",
            serde_json::json!({"type": "flag-manager/profile-changed", "payload": {}}),
        );
        let res = dispatch("evil", &req, &store, &mut presence, &shared, &plugin_dir);
        match res {
            Response::Error { message, .. } => assert!(message.contains("may not publish"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_plugin_may_declare_then_publish_on_its_own_type_through_the_real_dispatch() {
        let store = scratch_store("events-declare-publish");
        let mut presence = DiscordPresence::new();
        let shared = Shared::new();
        let plugin_dir = scratch_plugin_dir("events-declare-publish");

        let declared = dispatch(
            "flag-manager",
            &call("events.declare", serde_json::json!({"name": "profile-changed"})),
            &store,
            &mut presence,
            &shared,
            &plugin_dir,
        );
        let event_type = match declared {
            Response::Ok { result, .. } => result["type"].as_str().unwrap().to_string(),
            other => panic!("expected declare to succeed, got {other:?}"),
        };
        assert_eq!(event_type, "flag-manager/profile-changed");

        let published = dispatch(
            "flag-manager",
            &call("events.publish", serde_json::json!({"type": event_type, "payload": {"slot": 2}})),
            &store,
            &mut presence,
            &shared,
            &plugin_dir,
        );
        // No subscriber is registered in `shared.writers` at all here, and
        // that must not be an error: publishing to nobody is exactly what a
        // plugin does before anything has subscribed yet.
        assert!(matches!(published, Response::Ok { .. }), "{published:?}");
    }

    /// The property the `Shared`/`Writer` refactor exists for, proven against
    /// two real Deno processes and the exact `dispatch` a running Cordial
    /// calls — not `cordial_plugins::host::Session`, which is a separate,
    /// test-only construct that never runs inside the real client. If this
    /// regressed, `events.publish` would answer `Ok` while silently reaching
    /// nobody: exactly the "recorded but not enforced" shape this file's
    /// wiring exists to close.
    #[test]
    fn a_published_event_reaches_a_real_subscriber_through_the_shared_writer_map() {
        if std::process::Command::new("deno").arg("--version").output().is_err() {
            eprintln!("skipping: deno is not installed");
            return;
        }

        let shared = Shared::new();
        let store = scratch_store("events-cross-process");
        let plugin_dir = scratch_plugin_dir("events-cross-process");

        // The publisher is simulated from the Rust side — declaring and
        // publishing are pure `dispatch` calls with no process behind
        // them — the same choice `cordial-plugins`' own
        // `events_integration.rs` makes, and for the same reason: a second
        // Deno process that only ever declares and publishes would test
        // nothing this file does not already exercise by calling `dispatch`
        // directly.
        let mut publisher_presence = DiscordPresence::new();
        let declared = dispatch(
            "flag-manager",
            &call("events.declare", serde_json::json!({"name": "profile-changed"})),
            &store,
            &mut publisher_presence,
            &shared,
            &plugin_dir,
        );
        let event_type = match declared {
            Response::Ok { result, .. } => result["type"].as_str().unwrap().to_string(),
            other => panic!("flag-manager should have been able to declare its own type, got {other:?}"),
        };

        // The subscriber has to be a real process: receiving a push over
        // stdio, from a thread that made no request for it, is the part that
        // cannot be faked without a genuine second pipe on the other end.
        // Reuses `cordial-plugins`' own fixture rather than a copy of it —
        // it declares nothing this crate does, only what a subscriber-only
        // plugin does.
        let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../cordial-plugins/tests/fixtures/events_subscriber.ts");
        let mut launcher = PluginProc::spawn("launcher", &entry).expect("deno should start");
        shared.writers.lock().unwrap().insert("launcher".to_string(), launcher.writer());

        let mut launcher_presence = DiscordPresence::new();
        let mut logs: Vec<String> = Vec::new();
        let mut published = false;
        while let Some(Ok(req)) = launcher.next_request() {
            if req.method == "log.write" {
                let message = req.params["message"].as_str().unwrap_or_default().to_string();
                launcher.reply(&Response::Ok { id: req.id, result: serde_json::Value::Null }).unwrap();
                logs.push(message);
                if logs.len() >= 2 {
                    break;
                }
                continue;
            }

            let res = dispatch("launcher", &req, &store, &mut launcher_presence, &shared, &plugin_dir);
            let subscribed_ok = req.method == "events.subscribe" && matches!(res, Response::Ok { .. });
            launcher.reply(&res).unwrap();

            if subscribed_ok && !published {
                published = true;
                // Now that the subscriber is actually registered, publish —
                // this is the call that should write a `Push` into
                // `launcher`'s stdin from a completely different thread's
                // point of view than the one reading it here.
                let pub_res = dispatch(
                    "flag-manager",
                    &call(
                        "events.publish",
                        serde_json::json!({"type": event_type, "payload": {"slot": 3}}),
                    ),
                    &store,
                    &mut publisher_presence,
                    &shared,
                    &plugin_dir,
                );
                assert!(matches!(pub_res, Response::Ok { .. }), "publish should succeed: {pub_res:?}");
            }
        }
        launcher.kill();

        assert!(published, "the test should have reached the point of publishing");
        let joined = logs.join("\n");
        assert!(joined.contains("subscribed: ok"), "got:\n{joined}");
        assert!(joined.contains("push: flag-manager/profile-changed"), "got:\n{joined}");
        assert!(joined.contains(r#""slot":3"#), "got:\n{joined}");
    }

    #[test]
    fn resolve_within_refuses_a_path_that_would_escape_the_base() {
        let base = std::env::temp_dir().join("cordial-plugin-host-resolve-within-test");
        for bad in ["..", "../elsewhere", "/etc/passwd", "a/../../b"] {
            assert!(resolve_within(&base, bad).is_err(), "{bad:?} should have been refused");
        }
        assert_eq!(resolve_within(&base, "overlay").unwrap(), base.join("overlay"));
        assert_eq!(resolve_within(&base, "a/b").unwrap(), base.join("a/b"));
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
