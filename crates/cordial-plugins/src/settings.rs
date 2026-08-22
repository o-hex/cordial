//! A plugin's own configuration, held by Cordial and never handed over.
//!
//! A plugin had nowhere to keep anything. It runs as a Deno process with no
//! file access at all (ADR-003), so nothing it learned survived being
//! restarted, and the obvious fix — give it a path — is the one ADR-007 rules
//! out: a plugin never receives a socket, a descriptor or a filename. So
//! Cordial owns the file and the plugin exchanges a document. The effect, not
//! the channel, the same shape `presence.set` already has.
//!
//! Settings live inside the profile, at `<profile>/plugins/<id>/settings.json`,
//! and deliberately not beside the plugin's installed code. Installing a plugin
//! once is right; carrying what it remembered about one account into the
//! account someone else plays on is not, and a settings document is exactly
//! where a plugin would record a username, a server, or a webhook.
//!
//! **The plugin id is never read out of a request.** Every function here takes
//! the id Cordial already holds for the process on the other end of the pipe,
//! and [`serve`] ignores any `plugin` field a request carries rather than
//! honouring it. This is `events.rs`'s defence applied to a directory instead
//! of a namespace, and for the same reason: a field a plugin can set is a field
//! it can set to somebody else's name.

use crate::capability::Capability;
use crate::manifest;
use crate::protocol::{Push, Request, Response};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The one message a plugin receives before it has asked for anything.
///
/// Namespaced under `cordial/` because `events.rs` reserves that prefix for
/// Cordial's own types and refuses to let any plugin declare inside it, so a
/// line naming it cannot have been minted by another plugin.
pub const INIT_EVENT: &str = "cordial/init";

/// How large a settings document may be.
///
/// `settings.write` is the only capability that lets a plugin consume the
/// user's disk, and it does so inside a directory the user did not choose and
/// does not watch. A plugin appending on every launch — an ordinary bug, not an
/// attack — would otherwise fill it silently, and the first symptom would be
/// somewhere else entirely. A megabyte is far more than configuration needs;
/// a plugin wanting more than this wants a data store, and that is a design
/// conversation rather than a constant to raise quietly.
const MAX_BYTES: usize = 1024 * 1024;

/// Cordial's hold on every plugin's settings within one profile.
///
/// Cheap to clone because the runtime hands one to each plugin's serving
/// thread; there is no shared state to keep in step, only a directory name.
#[derive(Debug, Clone)]
pub struct Store {
    profile_dir: PathBuf,
}

impl Store {
    pub fn new(profile_dir: impl Into<PathBuf>) -> Self {
        Store { profile_dir: profile_dir.into() }
    }

    pub fn profile_dir(&self) -> &Path {
        &self.profile_dir
    }

    /// Where `plugin_id`'s settings live, refusing an id that could name
    /// anything else.
    ///
    /// The id only ever arrives from a manifest on disk or from Cordial's own
    /// record of which process is talking, and both are already restricted —
    /// but it is checked again here, because this is the function that turns a
    /// string into a path, and a check kept somewhere upstream is a check a
    /// later caller can skip without noticing. `..`, `a/b` and `/etc` all fail
    /// `manifest::is_valid_id`, so there is nothing to sanitise and nothing is
    /// quietly rewritten.
    pub fn path_for(&self, plugin_id: &str) -> Result<PathBuf, String> {
        if !manifest::is_valid_id(plugin_id) {
            return Err(format!(
                "{plugin_id:?} is not a usable plugin id, so it has no settings"
            ));
        }
        Ok(self.profile_dir.join("plugins").join(plugin_id).join("settings.json"))
    }

    /// This plugin's saved document, or an empty one.
    pub fn read(&self, plugin_id: &str) -> Result<serde_json::Value, String> {
        let path = self.path_for(plugin_id)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            // Nothing saved yet is the ordinary state of a first launch rather
            // than a failure, and an empty object says so in the same shape as
            // having settings, so a plugin needs no second code path for it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(serde_json::json!({}))
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) if v.is_object() => Ok(v),
            // Reported rather than answered as "you have nothing saved".
            // Telling a plugin its settings are empty when the file is merely
            // unreadable invites it to write a fresh document straight over
            // whatever the user actually had.
            Ok(_) => Err(format!("{} is not a JSON object", path.display())),
            Err(e) => Err(format!("{} is not usable ({e})", path.display())),
        }
    }

    /// Replace this plugin's document with `document`.
    ///
    /// A whole-document replace rather than a merge: the plugin is the only
    /// writer of its own settings, so it always knows the state it means to
    /// leave behind, and a merge would give it no way to remove a key it had
    /// stopped using.
    pub fn write(&self, plugin_id: &str, document: &serde_json::Value) -> Result<(), String> {
        // An object, not any JSON value, because a settings page has to render
        // this. A bare array or number leaves a UI nothing to show, and finding
        // that out when the page is drawn is much later than finding it out
        // here.
        if !document.is_object() {
            return Err("settings must be a JSON object".into());
        }
        let text = serde_json::to_string_pretty(document).map_err(|e| e.to_string())?;
        if text.len() > MAX_BYTES {
            return Err(format!(
                "settings are {} bytes; the limit is {MAX_BYTES}",
                text.len()
            ));
        }

        let path = self.path_for(plugin_id)?;
        let dir = path.parent().expect("path_for always joins at least one component");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

        // Written alongside and renamed rather than truncated in place. A
        // plugin killed mid-write would otherwise leave a half-document that
        // reads back as malformed, costing the user every setting they had
        // rather than the one they were changing. `rename` is atomic within a
        // directory, which is why the temporary file sits in the same one.
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Answer one already-authorised `settings.*` call.
///
/// `plugin_id` is Cordial's own record of which process is on the other end of
/// the pipe. There is deliberately no id parameter on these methods: a request
/// naming another plugin reads and writes the caller's own document, because
/// the namespace is not something this call takes an argument for. That is the
/// whole of the isolation between one plugin's settings and another's, and it
/// is why it is expressed as an absent parameter rather than as a check that
/// could be reordered away.
pub fn serve(store: Option<&Store>, plugin_id: &str, req: &Request) -> Response {
    let Some(store) = store else {
        // No profile means no file, and saying so beats answering `Ok` with
        // nothing behind it — the engine-facing half of this project has a
        // list of afternoons lost to a stub that reported success.
        return Response::Error {
            id: req.id,
            message: format!(
                "{} needs an open profile; this Cordial has no settings store",
                req.method
            ),
        };
    };
    match req.method.as_str() {
        "settings.get" => match store.read(plugin_id) {
            Ok(document) => Response::Ok { id: req.id, result: document },
            Err(message) => Response::Error { id: req.id, message },
        },
        "settings.set" => match req.params.get("settings") {
            Some(document) => match store.write(plugin_id, document) {
                Ok(()) => Response::Ok { id: req.id, result: serde_json::Value::Null },
                Err(message) => Response::Error { id: req.id, message },
            },
            None => Response::Error {
                id: req.id,
                message: "settings.set needs a settings object".into(),
            },
        },
        other => Response::Error {
            id: req.id,
            message: format!("{other:?} is not a settings method"),
        },
    }
}

/// The handshake line, carrying this plugin's settings so the common case
/// costs no round trip.
///
/// `settings` is `null` when the plugin does not hold `settings.read` or when
/// there is no profile, and `{}` when it holds the capability and has saved
/// nothing yet. The two are worth telling apart: the first means "ask the user
/// for the capability", the second means "you are new here".
///
/// Delivering the document to a plugin that was never granted `settings.read`
/// would be routing around the broker on the one path where the plugin made no
/// request to check.
pub fn init_push(
    store: Option<&Store>,
    fields: &[crate::preferences::Declaration],
    plugin_id: &str,
    granted: &BTreeSet<Capability>,
) -> Push {
    let settings = match (granted.contains(&Capability::SettingsRead), store) {
        (true, Some(store)) => match store.read(plugin_id) {
            Ok(document) => Some(document),
            // Said out loud here, because a handshake carrying `null` is
            // otherwise indistinguishable from a plugin that was never granted
            // the capability, and the author would go looking for the wrong
            // problem.
            Err(e) => {
                println!("  plugin {plugin_id}: settings not delivered ({e})");
                None
            }
        },
        _ => None,
    };
    // The user's answers to the questions this plugin's own manifest asked,
    // complete and already validated (ADR-020). Rooted at the same profile
    // directory, so it is derived from the settings store rather than passed
    // separately -- two parameters that must name the same profile is a pair
    // that can be handed different ones.
    //
    // `null` and `{}` are told apart here exactly as they are for `settings`
    // above, and there is a third case: a plugin that declares no preferences
    // has no page and gets `{}`, which is the truthful answer -- it has no
    // answers because it asked nothing, not because it was refused.
    let preferences = match (granted.contains(&Capability::SettingsRead), store) {
        (true, Some(store)) => {
            let prefs = crate::preferences::Store::new(store.profile_dir());
            match prefs.effective_for(plugin_id, fields) {
                Ok(values) => Some(serde_json::to_value(values).unwrap_or_default()),
                Err(e) => {
                    println!("  plugin {plugin_id}: preferences not delivered ({e})");
                    None
                }
            }
        }
        _ => None,
    };
    Push {
        event: INIT_EVENT.to_string(),
        payload: serde_json::json!({ "settings": settings, "preferences": preferences }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("cordial-settings-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Store::new(dir)
    }

    fn call(method: &str, params: serde_json::Value) -> Request {
        Request { id: 1, method: method.into(), params }
    }

    #[test]
    fn settings_land_inside_the_profile_under_the_plugins_own_id() {
        let store = scratch("layout");
        let path = store.path_for("flag-inspector").unwrap();
        assert_eq!(path, store.profile_dir().join("plugins/flag-inspector/settings.json"));
    }

    #[test]
    fn an_id_that_is_a_path_is_refused_rather_than_sanitised() {
        // This is the check that keeps one plugin's document inside its own
        // directory. Remove `manifest::is_valid_id` from `path_for` and every
        // one of these resolves to somewhere in — or above — the profile.
        let store = scratch("escape");
        for bad in ["..", "../../etc", "a/b", "/etc/passwd", ".", ""] {
            let refused = store.path_for(bad);
            assert!(refused.is_err(), "{bad:?} should not resolve to a settings path");
        }
    }

    #[test]
    fn a_document_survives_being_written_and_read_back() {
        let store = scratch("roundtrip");
        store
            .write("themer", &serde_json::json!({"accent": "teal", "panels": [1, 2]}))
            .unwrap();
        let back = store.read("themer").unwrap();
        assert_eq!(back["accent"], "teal");
        assert_eq!(back["panels"], serde_json::json!([1, 2]));
    }

    #[test]
    fn a_write_replaces_rather_than_merges() {
        // A plugin is the only writer of its own document, so it must be able
        // to remove a key it has stopped using. A merge would leave that key
        // there forever with nothing able to delete it.
        let store = scratch("replace");
        store.write("themer", &serde_json::json!({"accent": "teal", "stale": true})).unwrap();
        store.write("themer", &serde_json::json!({"accent": "rose"})).unwrap();
        let back = store.read("themer").unwrap();
        assert_eq!(back["accent"], "rose");
        assert!(back.get("stale").is_none(), "the removed key should be gone: {back}");
    }

    #[test]
    fn a_plugin_with_nothing_saved_gets_an_empty_document_not_an_error() {
        let store = scratch("first-run");
        assert_eq!(store.read("brand-new").unwrap(), serde_json::json!({}));
    }

    #[test]
    fn an_unreadable_document_is_reported_rather_than_answered_as_empty() {
        // Answering "you have no settings" for a file that is merely corrupt
        // invites the plugin to write a fresh document over what the user had.
        let store = scratch("malformed");
        let path = store.path_for("themer").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert!(store.read("themer").is_err());
    }

    #[test]
    fn a_settings_document_that_is_not_an_object_is_refused() {
        let store = scratch("shape");
        assert!(store.write("themer", &serde_json::json!([1, 2, 3])).is_err());
        assert!(store.write("themer", &serde_json::json!("hello")).is_err());
        assert!(store.write("themer", &serde_json::json!({})).is_ok());
    }

    #[test]
    fn an_oversized_document_is_refused_and_leaves_the_old_one_alone() {
        let store = scratch("oversized");
        store.write("hoarder", &serde_json::json!({"keep": "this"})).unwrap();
        let huge = serde_json::json!({ "blob": "x".repeat(MAX_BYTES + 1) });
        assert!(store.write("hoarder", &huge).is_err());
        assert_eq!(store.read("hoarder").unwrap()["keep"], "this");
    }

    #[test]
    fn naming_another_plugin_in_the_request_reads_your_own_settings() {
        // The proof that a plugin cannot read another plugin's settings. The
        // request below asks for `victim` by every field name a settings API
        // might plausibly have used; the answer must be the caller's own
        // document, because the id is not a parameter. If `serve` ever grew
        // one, `secret` would appear here.
        let store = scratch("namespace");
        store.write("victim", &serde_json::json!({"secret": "cookie"})).unwrap();
        store.write("thief", &serde_json::json!({"mine": true})).unwrap();

        let res = serve(
            Some(&store),
            "thief",
            &call(
                "settings.get",
                serde_json::json!({"plugin": "victim", "id": "victim", "plugin_id": "victim"}),
            ),
        );
        match res {
            Response::Ok { result, .. } => {
                assert_eq!(result["mine"], true, "should have read its own document");
                assert!(result.get("secret").is_none(), "read another plugin's settings: {result}");
            }
            other => panic!("expected the caller's own settings, got {other:?}"),
        }
    }

    #[test]
    fn naming_another_plugin_in_the_request_writes_your_own_settings() {
        // The other half, and the more damaging one: a plugin that could
        // address another plugin's document could not just read a secret but
        // overwrite a configuration its author never wrote.
        let store = scratch("namespace-write");
        store.write("victim", &serde_json::json!({"secret": "cookie"})).unwrap();

        let res = serve(
            Some(&store),
            "thief",
            &call(
                "settings.set",
                serde_json::json!({"plugin": "victim", "settings": {"secret": "stolen"}}),
            ),
        );
        assert!(matches!(res, Response::Ok { .. }), "{res:?}");
        assert_eq!(
            store.read("victim").unwrap()["secret"],
            "cookie",
            "the victim's document must be untouched"
        );
        assert_eq!(store.read("thief").unwrap()["secret"], "stolen");
    }

    #[test]
    fn settings_without_a_profile_fail_loudly_rather_than_pretending_to_save() {
        // A save that reports success and writes nowhere is the stub that lies
        // — the plugin believes the user's choice was kept and it was not.
        let res = serve(None, "themer", &call("settings.set", serde_json::json!({"settings": {}})));
        match res {
            Response::Error { message, .. } => assert!(message.contains("profile"), "{message}"),
            other => panic!("expected an explicit failure, got {other:?}"),
        }
    }

    #[test]
    fn a_set_without_a_document_is_an_error_not_an_empty_save() {
        // Treating a missing `settings` field as `{}` would erase everything
        // the plugin had on a malformed call.
        let store = scratch("no-document");
        store.write("themer", &serde_json::json!({"accent": "teal"})).unwrap();
        let res = serve(Some(&store), "themer", &call("settings.set", serde_json::json!({})));
        assert!(matches!(res, Response::Error { .. }), "{res:?}");
        assert_eq!(store.read("themer").unwrap()["accent"], "teal");
    }

    #[test]
    fn the_handshake_carries_settings_only_to_a_plugin_granted_the_capability() {
        // Delivering the document unasked to a plugin without settings.read
        // would route around the broker on the one path where the plugin makes
        // no request for the broker to check.
        let store = scratch("handshake");
        store.write("themer", &serde_json::json!({"accent": "teal"})).unwrap();

        let granted: BTreeSet<Capability> = [Capability::SettingsRead].into_iter().collect();
        let push = init_push(Some(&store), &[], "themer", &granted);
        assert_eq!(push.event, INIT_EVENT);
        assert_eq!(push.payload["settings"]["accent"], "teal");

        let ungranted: BTreeSet<Capability> = [Capability::Log].into_iter().collect();
        let push = init_push(Some(&store), &[], "themer", &ungranted);
        assert!(push.payload["settings"].is_null(), "{}", push.payload);
    }

    #[test]
    fn a_granted_plugin_with_nothing_saved_is_told_that_rather_than_nothing() {
        // `{}` means "you are new here" and `null` means "you were not granted
        // this"; collapsing them would leave an author unable to tell a
        // first launch from a missing capability.
        let store = scratch("handshake-empty");
        let granted: BTreeSet<Capability> = [Capability::SettingsRead].into_iter().collect();
        let push = init_push(Some(&store), &[], "brand-new", &granted);
        assert_eq!(push.payload["settings"], serde_json::json!({}));
    }
}
