//! Launching a plugin and talking to it.
//!
//! A plugin is a Deno process. That gives two independent layers of containment
//! rather than one: Deno's own permission model, and Cordial's capability
//! broker. The Deno process is started with **no permissions at all** — no file,
//! network, environment or subprocess access — so a plugin cannot reach the
//! machine even if the broker had a hole in it. Everything it is allowed to do
//! arrives over stdio and is checked by the broker first.
//!
//! This is ADR-003 made concrete: plugins are isolated by process, and the only
//! channel is a named, brokered one.

use crate::broker::Broker;
use crate::capability::Capability;
use crate::events::EventRegistry;
use crate::presence::{DiscordPresence, PresencePayload};
use crate::protocol::{required_capability, Push, Request, Response};
use crate::settings::{self, Store};
use crate::{notify, urlopen};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Mutex};

/// A plugin's stdin, shareable across threads.
///
/// A real host runs one thread per plugin, blocking on that plugin's own
/// stdout (see `cordial-runtime`'s `plugin_host.rs`). Delivering a published
/// event to a *subscriber* means writing to a different plugin's stdin from
/// whichever thread is serving the publisher's `events.publish` call — a
/// write that has nothing to do with that subscriber's own request/response
/// cycle and must not have to wait for one. Splitting the writable half out
/// from `Plugin` and wrapping it in a mutex is what makes that possible
/// without also having to share the read half, which only ever needs to be
/// read from the one thread that owns the `Plugin` itself.
///
/// Cheap to clone: an `Arc` around one mutex, never a duplicated file
/// descriptor.
#[derive(Clone)]
pub struct Writer(Arc<Mutex<ChildStdin>>);

impl Writer {
    fn write_line(&self, line: &str) -> std::io::Result<()> {
        // A poisoned mutex means some other write already panicked mid-line;
        // recovering the guard rather than propagating the poison lets this
        // write still land cleanly rather than every subsequent push to this
        // plugin failing forever over an unrelated panic.
        let mut stdin = self.0.lock().unwrap_or_else(|e| e.into_inner());
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    /// Deliver a [`Push`] through this handle, from whichever thread holds
    /// it — the counterpart to [`Plugin::push`] for a caller that only has
    /// the writable half. `&self` rather than `&mut self`: the mutex inside
    /// is what serialises concurrent writers, not Rust's own borrow checker,
    /// because a `Writer` is meant to be called from a thread that does not
    /// own the `Plugin` at all.
    pub fn push(&self, push: &Push) -> std::io::Result<()> {
        self.write_line(&serde_json::to_string(push).expect("Push always serialises"))
    }
}

pub struct Plugin {
    pub id: String,
    child: Child,
    writer: Writer,
    stdout: BufReader<ChildStdout>,
}

impl Plugin {
    /// Start a plugin from an entry module.
    ///
    /// `--no-prompt` matters as much as the absence of allow flags: without it
    /// Deno would *ask* for a permission on first use, and a plugin host has
    /// nobody to ask. With it, an attempt to touch the filesystem fails
    /// immediately instead of hanging on a prompt nothing will answer.
    pub fn spawn(id: &str, entry: &Path) -> std::io::Result<Self> {
        // A third layer under the two above, when the host can enforce one. It
        // does not replace either: a sub-sandbox only ever subtracts from what
        // Cordial holds, so every effect is still performed by the broker. See
        // `crate::sandbox`, which says so at length because "we sandbox now" is
        // the argument someone will use to justify handing a plugin an fd.
        //
        // Absence is a downgrade rather than a hole -- the Deno process still
        // has no permissions at all -- so a missing `bwrap` does not stop a
        // plugin running. It is said out loud instead, because a layer nobody
        // can tell is missing is one nobody notices went away.
        let sandbox = crate::sandbox::available();
        println!("[plugin] {id}: {}", sandbox.describe());
        let mut child = crate::sandbox::command(sandbox, entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
        Ok(Plugin { id: id.to_string(), child, writer: Writer(Arc::new(Mutex::new(stdin))), stdout })
    }

    /// Read one request from the plugin. `None` at end of stream.
    pub fn next_request(&mut self) -> Option<Result<Request, String>> {
        let mut line = String::new();
        match self.stdout.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => Some(serde_json::from_str(line.trim()).map_err(|e| e.to_string())),
            Err(e) => Some(Err(e.to_string())),
        }
    }

    pub fn reply(&mut self, response: &Response) -> std::io::Result<()> {
        self.writer.write_line(&serde_json::to_string(response).expect("Response always serialises"))
    }

    /// Deliver a message the plugin did not ask for in this call — a
    /// lifecycle event, or another plugin's published event arriving for a
    /// subscriber. See [`Push`] for how a plugin tells this apart from a
    /// reply to one of its own requests.
    pub fn push(&mut self, push: &Push) -> std::io::Result<()> {
        self.writer.write_line(&serde_json::to_string(push).expect("Push always serialises"))
    }

    /// A cloneable handle to this plugin's stdin, for a host that wants to
    /// push to it from a thread other than the one reading its stdout — see
    /// [`Writer`].
    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Decide what a request gets, without performing any effect.
///
/// Kept separate from dispatch so the decision is testable on its own, and so
/// the check cannot be accidentally skipped by a future handler that forgets to
/// call it — a handler receives an already-authorised request or nothing.
pub fn authorise(broker: &mut Broker, plugin: &str, req: &Request) -> Result<(), Response> {
    match required_capability(&req.method) {
        None => Err(Response::Error {
            id: req.id,
            message: format!("unknown method {:?}", req.method),
        }),
        Some(cap) if !broker.allows(plugin, cap) => {
            Err(Response::Denied { id: req.id, capability: cap.name().to_string() })
        }
        Some(_) => Ok(()),
    }
}

/// Everything one running Cordial process needs to serve the capabilities
/// this crate actually performs an effect for: the grants, the event
/// registry, and the one live Discord connection. A `Session` is where
/// ADR-007 stops being a description and starts being true — it is the one
/// place that has both a plugin's authorised request and the host resource
/// the request wants to act on, and nothing upstream of it ever holds the
/// two together.
///
/// `Session` only ever answers the methods it has a real broker for —
/// `presence.*`, `notify.send`, `url.open`, `settings.*`, `events.*`, and the
/// `lifecycle.subscribe` acknowledgement paired with `push_lifecycle` below.
/// Everything else (`flags.*`, `log.write`, `assets.override`) is still only
/// `authorise`d here; a caller that wants those served has to do it itself,
/// the way `tests/flag_inspector.rs` already does. Falling through to an
/// explicit "no broker wired" error rather than silently returning `Ok`
/// keeps this file honest about what it does and does not implement — see
/// AGENTS.md on a stub never claiming success it did not have.
pub struct Session {
    pub broker: Broker,
    pub events: EventRegistry,
    presence: DiscordPresence,
    plugins: BTreeMap<String, Plugin>,
    /// Where every plugin's settings live, which is a property of the profile
    /// this instance is running rather than of the process. `None` is honest
    /// about a session with no profile behind it — `settings.*` then fails and
    /// says why, instead of reporting a save that went nowhere.
    settings: Option<Store>,
}

impl Session {
    pub fn new() -> Self {
        Session {
            broker: Broker::new(),
            events: EventRegistry::new(),
            presence: DiscordPresence::new(),
            plugins: BTreeMap::new(),
            settings: None,
        }
    }

    /// A session running `profile_dir`, so plugins have somewhere to keep
    /// their settings. Everything else about a session is per process; this is
    /// the one thing that belongs to the profile.
    pub fn with_profile(profile_dir: impl Into<PathBuf>) -> Self {
        Session { settings: Some(Store::new(profile_dir)), ..Session::new() }
    }

    /// Adopt a spawned plugin so it can receive pushes — lifecycle events and
    /// other plugins' published events — in addition to answering its own
    /// requests through [`Session::handle`].
    ///
    /// The plugin's first line is the handshake, carrying whatever it had
    /// saved. That is what keeps the common case — read your configuration,
    /// then start — free of a round trip a plugin would otherwise have to make
    /// before it could do anything. Grants must therefore be in place before
    /// this is called; adopting first and granting afterwards would hand a
    /// plugin a handshake saying it holds nothing.
    pub fn add_plugin(&mut self, mut plugin: Plugin) {
        let granted = self.broker.granted(&plugin.id);
        let push = settings::init_push(self.settings.as_ref(), &plugin.id, &granted);
        // Best effort, like every other push: a plugin that has already died
        // is not a reason to fail adopting it, and the write error would only
        // repeat what the next read of its stdout is about to say.
        let _ = plugin.push(&push);
        self.plugins.insert(plugin.id.clone(), plugin);
    }

    pub fn remove_plugin(&mut self, id: &str) -> Option<Plugin> {
        self.plugins.remove(id)
    }

    /// Access an adopted plugin directly — for reading its next request and
    /// replying, the way a caller drives any other `Plugin`. Needed because
    /// once a `Plugin` is adopted its stdio is also the channel `handle`
    /// pushes events down, so a caller cannot keep its own separate handle to
    /// the same process.
    pub fn plugin_mut(&mut self, id: &str) -> Option<&mut Plugin> {
        self.plugins.get_mut(id)
    }

    /// Deliver a client lifecycle event to every plugin holding
    /// `lifecycle.read`. There is no request to deny for a plugin that lacks
    /// the capability — only a push nobody asked to receive — so it is
    /// simply not sent rather than answered with a denial nobody is waiting
    /// for.
    pub fn push_lifecycle(&mut self, event: &str) {
        let recipients: Vec<String> = self
            .plugins
            .keys()
            .filter(|id| self.broker.granted(id).contains(&Capability::LifecycleRead))
            .cloned()
            .collect();
        for id in recipients {
            if let Some(plugin) = self.plugins.get_mut(&id) {
                let _ = plugin.push(&Push { event: event.to_string(), payload: serde_json::Value::Null });
            }
        }
    }

    /// Authorise, then perform, one call from `plugin_id`.
    pub fn handle(&mut self, plugin_id: &str, req: &Request) -> Response {
        if let Err(refusal) = authorise(&mut self.broker, plugin_id, req) {
            return refusal;
        }
        match req.method.as_str() {
            "presence.set" => match PresencePayload::parse(&req.params) {
                Ok(payload) => respond(req.id, self.presence.set(&payload)),
                Err(message) => Response::Error { id: req.id, message },
            },
            "presence.clear" => respond(req.id, self.presence.clear()),
            // Delivery for lifecycle events is capability-gated, not a
            // subscription list — see push_lifecycle — so this call has
            // nothing to record. It exists so a plugin gets a definite
            // acknowledgement that it holds lifecycle.read, the same way
            // events.subscribe acknowledges a subscription, rather than the
            // plugin having to infer that from the first push ever arriving.
            "lifecycle.subscribe" => Response::Ok { id: req.id, result: serde_json::Value::Null },
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
            "events.declare" => match req.params.get("name").and_then(|v| v.as_str()) {
                Some(name) => match self.events.declare(plugin_id, name) {
                    Ok(event_type) => Response::Ok { id: req.id, result: serde_json::json!({"type": event_type}) },
                    Err(message) => Response::Error { id: req.id, message },
                },
                None => Response::Error { id: req.id, message: "events.declare needs a name".into() },
            },
            // `plugin_id` is this session's own record of which process is on
            // the other end of the pipe, and it is the only id `serve` is
            // given — a plugin naming another one in its params reads and
            // writes its own document. See settings.rs.
            "settings.get" | "settings.set" => {
                settings::serve(self.settings.as_ref(), plugin_id, req)
            }
            "events.publish" => self.publish(plugin_id, req),
            "events.subscribe" => match req.params.get("type").and_then(|v| v.as_str()) {
                Some(event_type) => match self.events.subscribe(plugin_id, event_type) {
                    Ok(()) => Response::Ok { id: req.id, result: serde_json::Value::Null },
                    Err(message) => Response::Error { id: req.id, message },
                },
                None => Response::Error { id: req.id, message: "events.subscribe needs a type".into() },
            },
            other => Response::Error { id: req.id, message: format!("no broker wired for {other:?}") },
        }
    }

    fn publish(&mut self, plugin_id: &str, req: &Request) -> Response {
        let Some(event_type) = req.params.get("type").and_then(|v| v.as_str()) else {
            return Response::Error { id: req.id, message: "events.publish needs a type".into() };
        };
        if !self.events.may_publish(plugin_id, event_type) {
            return Response::Error {
                id: req.id,
                message: format!(
                    "{plugin_id:?} may not publish on {event_type:?}; it must declare that type before publishing on it"
                ),
            };
        }
        let payload = req.params.get("payload").cloned().unwrap_or(serde_json::Value::Null);
        for subscriber in self.events.subscribers(event_type) {
            let subscriber = subscriber.to_string();
            if let Some(plugin) = self.plugins.get_mut(&subscriber) {
                let _ = plugin.push(&Push { event: event_type.to_string(), payload: payload.clone() });
            }
        }
        Response::Ok { id: req.id, result: serde_json::Value::Null }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

fn respond(id: u64, result: Result<(), String>) -> Response {
    match result {
        Ok(()) => Response::Ok { id, result: serde_json::Value::Null },
        Err(message) => Response::Error { id, message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    fn req(method: &str) -> Request {
        Request { id: 1, method: method.into(), params: serde_json::Value::Null }
    }

    /// A cloned [`Writer`] really does deliver to the same process, from a
    /// thread that never reads that process's stdout at all.
    ///
    /// This is the property `cordial-runtime`'s real host depends on: one
    /// thread blocks reading a plugin's own requests, and a *different*
    /// plugin's publish has to be able to push into this one's stdin without
    /// waiting for that read loop to be between requests. Proven against a
    /// real Deno process rather than a mock, because the property in question
    /// is about `ChildStdin` actually being safe to write from two threads
    /// through one mutex — a unit test with a fake writer would not exercise
    /// that at all.
    #[test]
    fn a_cloned_writer_pushes_into_the_same_process_from_a_different_thread() {
        if std::process::Command::new("deno").arg("--version").output().is_err() {
            eprintln!("skipping: deno is not installed");
            return;
        }
        let entry = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/events_subscriber.ts");
        let mut plugin = Plugin::spawn("writer-clone-test", &entry).expect("deno should start");

        // The clone is what a publishing plugin's own serving thread would
        // hold — never the `Plugin` itself, which stays owned by the thread
        // reading this process's stdout below.
        let writer = plugin.writer();
        std::thread::spawn(move || {
            let _ = writer.write_line(
                &serde_json::to_string(&Push {
                    event: "cross-thread/proof".into(),
                    payload: serde_json::json!({"from": "another thread"}),
                })
                .unwrap(),
            );
        });

        let mut logs = Vec::new();
        while let Some(Ok(req)) = plugin.next_request() {
            if req.method == "log.write" {
                logs.push(req.params["message"].as_str().unwrap_or_default().to_string());
                plugin.reply(&Response::Ok { id: req.id, result: serde_json::Value::Null }).unwrap();
                break;
            }
            // The fixture's own `events.subscribe` call; not answering it is
            // fine, since the pushed message arrives on an independent code
            // path in the fixture's event loop and does not wait for a reply.
        }
        plugin.kill();

        let joined = logs.join("\n");
        assert!(joined.contains("push: cross-thread/proof"), "got:\n{joined}");
        assert!(joined.contains(r#""from":"another thread""#), "got:\n{joined}");
    }

    #[test]
    fn an_authorised_call_passes() {
        let mut b = Broker::new();
        b.grant("p", [Capability::FlagsRead]);
        assert!(authorise(&mut b, "p", &req("flags.list")).is_ok());
    }

    #[test]
    fn an_unauthorised_call_is_denied_by_name() {
        let mut b = Broker::new();
        b.grant("p", [Capability::FlagsRead]);
        match authorise(&mut b, "p", &req("flags.set")) {
            Err(Response::Denied { capability, .. }) => assert_eq!(capability, "flags.write"),
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_method_is_an_error_not_a_denial() {
        // A typo must not look like a missing permission, or the author goes
        // hunting for a capability that was never the problem.
        let mut b = Broker::new();
        b.grant("p", Capability::all().iter().copied());
        match authorise(&mut b, "p", &req("flags.nonsense")) {
            Err(Response::Error { message, .. }) => assert!(message.contains("unknown method")),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    fn call(method: &str, params: serde_json::Value) -> Request {
        Request { id: 1, method: method.into(), params }
    }

    #[test]
    fn session_denies_an_ungranted_brokered_capability_rather_than_erroring() {
        // The distinction protocol.rs draws between `denied` and `error` has
        // to survive contact with a real effect-performing broker, not just
        // the plain `authorise` check — a plugin without notify.send must see
        // a denial, not a message about the D-Bus call that never happened.
        let mut session = Session::new();
        let res = session.handle("p", &call("notify.send", serde_json::json!({"summary": "hi"})));
        match res {
            Response::Denied { capability, .. } => assert_eq!(capability, "notify.send"),
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn session_refuses_a_non_http_url_scheme_once_granted() {
        // ADR-007's doc comment on UrlOpen is explicit that this must not
        // become file:// traversal — checked here past the capability gate,
        // where a granted-but-malicious call would otherwise reach the portal.
        let mut session = Session::new();
        session.broker.grant("p", [Capability::UrlOpen]);
        let res = session.handle("p", &call("url.open", serde_json::json!({"url": "file:///etc/passwd"})));
        match res {
            Response::Error { message, .. } => assert!(message.contains("refused"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn session_refuses_a_publish_on_a_type_the_plugin_never_declared() {
        // events.publish alone is not enough — ADR-006 splits declare from
        // publish precisely so holding this capability cannot be used to
        // impersonate a type nobody gave this plugin.
        let mut session = Session::new();
        session.broker.grant("evil", [Capability::EventsPublish]);
        let res = session.handle(
            "evil",
            &call("events.publish", serde_json::json!({"type": "flag-manager/profile-changed", "payload": {}})),
        );
        match res {
            Response::Error { message, .. } => assert!(message.contains("may not publish"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn session_lets_a_plugin_declare_then_publish_on_its_own_type() {
        let mut session = Session::new();
        session.broker.grant("flag-manager", [Capability::EventsDeclare, Capability::EventsPublish]);
        let declared = session.handle("flag-manager", &call("events.declare", serde_json::json!({"name": "profile-changed"})));
        let event_type = match declared {
            Response::Ok { result, .. } => result["type"].as_str().unwrap().to_string(),
            other => panic!("expected declare to succeed, got {other:?}"),
        };
        assert_eq!(event_type, "flag-manager/profile-changed");

        let published = session.handle(
            "flag-manager",
            &call("events.publish", serde_json::json!({"type": event_type, "payload": {"slot": 2}})),
        );
        assert!(matches!(published, Response::Ok { .. }), "{published:?}");
    }

    #[test]
    fn lifecycle_subscribe_acknowledges_holding_the_capability() {
        let mut session = Session::new();
        session.broker.grant("p", [Capability::LifecycleRead]);
        let res = session.handle("p", &call("lifecycle.subscribe", serde_json::Value::Null));
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");
    }

    #[test]
    fn session_keeps_one_plugins_settings_out_of_anothers_reach() {
        // The escape this is guarding is not hypothetical: a settings API that
        // took the plugin id as a parameter is the obvious way to write one,
        // and it would have let any plugin holding settings.read address every
        // other plugin's document. `handle` passes its own record of who is
        // calling and nothing else, so the request below reads `thief`'s own
        // settings however it words the question. Remove that and `secret`
        // appears in the result.
        let dir = std::env::temp_dir().join("cordial-session-settings-namespace");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut session = Session::with_profile(&dir);
        session.broker.grant("victim", [Capability::SettingsWrite]);
        session.broker.grant("thief", [Capability::SettingsRead]);

        let stored = session.handle(
            "victim",
            &call("settings.set", serde_json::json!({"settings": {"secret": "cookie"}})),
        );
        assert!(matches!(stored, Response::Ok { .. }), "{stored:?}");

        let res = session.handle(
            "thief",
            &call("settings.get", serde_json::json!({"plugin": "victim"})),
        );
        match res {
            Response::Ok { result, .. } => {
                assert!(result.get("secret").is_none(), "read another plugin's settings: {result}");
                assert_eq!(result, serde_json::json!({}), "thief has saved nothing of its own");
            }
            other => panic!("expected the caller's own settings, got {other:?}"),
        }
    }

    #[test]
    fn reading_settings_does_not_carry_permission_to_replace_them() {
        let dir = std::env::temp_dir().join("cordial-session-settings-readonly");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut session = Session::with_profile(&dir);
        session.broker.grant("reader", [Capability::SettingsRead]);
        let res = session
            .handle("reader", &call("settings.set", serde_json::json!({"settings": {"a": 1}})));
        match res {
            Response::Denied { capability, .. } => assert_eq!(capability, "settings.write"),
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[test]
    fn a_session_with_no_profile_refuses_settings_rather_than_dropping_them() {
        // `Session::new` has nowhere to put a document. Answering Ok would
        // tell the plugin the user's choice was saved when it went nowhere.
        let mut session = Session::new();
        session.broker.grant("themer", [Capability::SettingsWrite]);
        let res = session
            .handle("themer", &call("settings.set", serde_json::json!({"settings": {}})));
        match res {
            Response::Error { message, .. } => assert!(message.contains("profile"), "{message}"),
            other => panic!("expected an explicit failure, got {other:?}"),
        }
    }

    #[test]
    fn session_has_no_broker_for_flags_and_says_so_rather_than_pretending() {
        // flags.* is deliberately out of this module's scope (see the
        // Session doc comment); it must fail loudly as "no broker wired"
        // rather than silently answering Ok with nothing behind it, which is
        // exactly the stub-that-lies AGENTS.md warns against.
        let mut session = Session::new();
        session.broker.grant("p", [Capability::FlagsRead]);
        let res = session.handle("p", &call("flags.list", serde_json::Value::Null));
        match res {
            Response::Error { message, .. } => assert!(message.contains("no broker wired"), "{message}"),
            other => panic!("expected an explicit no-broker error, got {other:?}"),
        }
    }
}
