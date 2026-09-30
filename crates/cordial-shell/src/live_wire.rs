//! The message set the shell uses to change a running client's settings.
//!
//! **A fixed vocabulary, not a command channel.** Each [`Update`] is one
//! setting that the client reads on a hot path and can therefore change without
//! a restart (ADR-044 has the classification and the reasons). The shell sends
//! `{"set":{"pointer_acceleration":"unlocked"}}`; the client answers with a
//! [`Reply`]. There is no verb that runs anything, reads a file, or reaches the
//! engine, and an unknown key is reported back rather than acted on. A plugin
//! never sees this socket (ADR-003, ADR-007): it lives in a `0700` directory in
//! the profile, which the plugin sandbox does not bind.
//!
//! This module is pure -- no sockets, no GTK -- because both ends need to agree
//! on it and `cordial-runtime` depends on `cordial-shell`, not the reverse.
//! Values travel as the same words the launch environment already uses
//! (`CORDIAL_POINTER_ACCEL=unlocked`, `CORDIAL_THROTTLE=off`), so a setting has
//! one spelling whether it arrives at spawn or afterwards.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directory inside the profile that holds the socket.
///
/// A directory rather than a bare socket file so the permission is on the
/// directory: a socket takes the process umask at `bind`, and closing the gap
/// between `bind` and a `chmod` is easier done by making the path unreachable
/// to anyone else from the start.
pub const SOCKET_DIR: &str = "live";
pub const SOCKET_NAME: &str = "settings.sock";

/// Longest request line either side will read. Generous for four keys and
/// small enough that a peer sending noise cannot make the client buffer it.
pub const MAX_LINE: usize = 1024;

/// Where a profile's client listens.
pub fn socket_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(SOCKET_DIR).join(SOCKET_NAME)
}

/// The camera-acceleration choice, in the words `CORDIAL_POINTER_ACCEL` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accel {
    Unlocked,
    Always,
}

impl Accel {
    pub fn as_str(self) -> &'static str {
        match self {
            Accel::Unlocked => "unlocked",
            Accel::Always => "always",
        }
    }
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "unlocked" => Some(Accel::Unlocked),
            "always" => Some(Accel::Always),
            _ => None,
        }
    }
}

/// When the keepalive stops, in the words `CORDIAL_THROTTLE` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Throttle {
    Visible,
    Unfocused,
    Off,
}

impl Throttle {
    pub fn as_str(self) -> &'static str {
        match self {
            Throttle::Visible => "visible",
            Throttle::Unfocused => "unfocused",
            Throttle::Off => "off",
        }
    }
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "visible" => Some(Throttle::Visible),
            "unfocused" => Some(Throttle::Unfocused),
            "off" => Some(Throttle::Off),
            _ => None,
        }
    }
}

/// One setting a running client can change. Adding a variant here is the whole
/// of "making a setting live" on the wire; the client must also read it from a
/// place that can change (see `cordial_runtime::live_settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Update {
    PointerAcceleration(Accel),
    Throttle(Throttle),
    CloseOnLeave(bool),
    CarryLaunchTicket(bool),
}

/// The keys [`Update`] can carry, which are also the `shell.json` field names.
pub const KEYS: [&str; 4] =
    ["pointer_acceleration", "throttle", "close_on_leave", "carry_launch_ticket"];

impl Update {
    pub fn key(&self) -> &'static str {
        match self {
            Update::PointerAcceleration(_) => "pointer_acceleration",
            Update::Throttle(_) => "throttle",
            Update::CloseOnLeave(_) => "close_on_leave",
            Update::CarryLaunchTicket(_) => "carry_launch_ticket",
        }
    }

    fn value(&self) -> Value {
        match self {
            Update::PointerAcceleration(a) => Value::from(a.as_str()),
            Update::Throttle(t) => Value::from(t.as_str()),
            Update::CloseOnLeave(b) | Update::CarryLaunchTicket(b) => Value::from(*b),
        }
    }

    /// A known key with its value, or why the value is unusable. `Ok(None)` is
    /// not returned: the caller has already split unknown keys off.
    fn from_pair(key: &str, value: &Value) -> Result<Self, String> {
        let bad = || format!("{key}: {value} is not a value this setting takes");
        match key {
            "pointer_acceleration" => value
                .as_str()
                .and_then(Accel::parse)
                .map(Update::PointerAcceleration)
                .ok_or_else(bad),
            "throttle" => {
                value.as_str().and_then(Throttle::parse).map(Update::Throttle).ok_or_else(bad)
            }
            "close_on_leave" => value.as_bool().map(Update::CloseOnLeave).ok_or_else(bad),
            "carry_launch_ticket" => value.as_bool().map(Update::CarryLaunchTicket).ok_or_else(bad),
            _ => Err(format!("{key}: not a live setting")),
        }
    }
}

/// What a client is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Change these. `ignored` names keys this client does not know, which are
    /// reported back rather than failing the whole message, so a newer shell
    /// talking to an older client still gets its known keys applied.
    Set { updates: Vec<Update>, ignored: Vec<String> },
    /// Report the values in force now.
    Get,
}

/// One line, newline-terminated.
pub fn encode_set(updates: &[Update]) -> String {
    let mut set = Map::new();
    for u in updates {
        set.insert(u.key().to_string(), u.value());
    }
    let mut line = Value::Object(Map::from_iter([("set".to_string(), Value::Object(set))])).to_string();
    line.push('\n');
    line
}

pub fn encode_get() -> String {
    "{\"get\":true}\n".to_string()
}

pub fn decode(line: &str) -> Result<Request, String> {
    if line.len() > MAX_LINE {
        return Err(format!("request longer than {MAX_LINE} bytes"));
    }
    let value: Value = serde_json::from_str(line.trim()).map_err(|e| format!("not JSON: {e}"))?;
    let Value::Object(top) = value else {
        return Err("a request is a JSON object".to_string());
    };
    if top.len() != 1 {
        return Err("a request has exactly one verb".to_string());
    }
    let (verb, body) = top.into_iter().next().expect("length checked above");
    match (verb.as_str(), body) {
        ("get", Value::Bool(true)) => Ok(Request::Get),
        ("set", Value::Object(map)) => {
            let mut updates = Vec::new();
            let mut ignored = Vec::new();
            for (key, value) in &map {
                if KEYS.contains(&key.as_str()) {
                    updates.push(Update::from_pair(key, value)?);
                } else {
                    ignored.push(key.clone());
                }
            }
            Ok(Request::Set { updates, ignored })
        }
        (other, _) => Err(format!("unknown request {other:?}")),
    }
}

/// A client's answer. `values` is filled for `get` and for a `set` (the state
/// after the change), so the shell can log what is actually in force rather
/// than what it asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    pub fn failure(why: impl Into<String>) -> Self {
        Reply { ok: false, error: Some(why.into()), ..Reply::default() }
    }
    pub fn encode(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a Reply always serialises");
        line.push('\n');
        line
    }
    pub fn decode(line: &str) -> Result<Self, String> {
        serde_json::from_str(line.trim()).map_err(|e| format!("unreadable reply: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Update> {
        vec![
            Update::PointerAcceleration(Accel::Unlocked),
            Update::Throttle(Throttle::Off),
            Update::CloseOnLeave(true),
            Update::CarryLaunchTicket(false),
        ]
    }

    #[test]
    fn every_update_round_trips_through_the_wire() {
        let line = encode_set(&all());
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "one line");
        let Request::Set { mut updates, ignored } = decode(&line).unwrap() else {
            panic!("a set encodes to a set")
        };
        assert!(ignored.is_empty());
        let mut want = all();
        // JSON objects are unordered; compare as sets of keys.
        updates.sort_by_key(|u| u.key());
        want.sort_by_key(|u| u.key());
        assert_eq!(updates, want);
    }

    #[test]
    fn the_key_list_matches_what_update_can_carry() {
        let mut keys: Vec<_> = all().iter().map(Update::key).collect();
        keys.sort_unstable();
        let mut listed = KEYS.to_vec();
        listed.sort_unstable();
        assert_eq!(keys, listed, "KEYS and Update::key drifted apart");
    }

    #[test]
    fn get_round_trips() {
        assert_eq!(decode(&encode_get()).unwrap(), Request::Get);
    }

    #[test]
    fn an_unknown_key_is_ignored_and_named_and_known_keys_still_apply() {
        let r = decode(r#"{"set":{"throttle":"off","warp_drive":"on"}}"#).unwrap();
        assert_eq!(
            r,
            Request::Set {
                updates: vec![Update::Throttle(Throttle::Off)],
                ignored: vec!["warp_drive".to_string()],
            }
        );
    }

    #[test]
    fn a_known_key_with_a_bad_value_refuses_the_whole_message() {
        // Half-applying a message would leave the shell believing one thing and
        // the client another, so a bad value is an error, not a skip.
        for bad in [
            r#"{"set":{"throttle":"sometimes"}}"#,
            r#"{"set":{"pointer_acceleration":true}}"#,
            r#"{"set":{"close_on_leave":"yes"}}"#,
            r#"{"set":{"throttle":"off","carry_launch_ticket":1}}"#,
        ] {
            assert!(decode(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn nothing_but_the_two_verbs_is_understood() {
        for bad in [
            "",
            "hello",
            "[]",
            r#"{}"#,
            r#"{"get":false}"#,
            r#"{"run":"ls"}"#,
            r#"{"exec":{"cmd":"ls"}}"#,
            r#"{"set":{},"get":true}"#,
            r#"{"set":"throttle"}"#,
        ] {
            assert!(decode(bad).is_err(), "{bad:?} must not decode");
        }
    }

    #[test]
    fn an_oversized_line_is_refused_before_it_is_parsed() {
        let big = format!(r#"{{"set":{{"x":"{}"}}}}"#, "a".repeat(MAX_LINE));
        assert!(decode(&big).unwrap_err().contains("longer than"));
    }

    #[test]
    fn a_reply_round_trips_and_a_failure_says_why() {
        let mut ok = Reply { ok: true, applied: vec!["throttle".into()], ..Reply::default() };
        ok.values.insert("throttle".into(), Value::from("off"));
        assert_eq!(Reply::decode(&ok.encode()).unwrap(), ok);
        let bad = Reply::failure("nope");
        let back = Reply::decode(&bad.encode()).unwrap();
        assert!(!back.ok);
        assert_eq!(back.error.as_deref(), Some("nope"));
    }

    #[test]
    fn the_socket_lives_in_a_directory_of_its_own_inside_the_profile() {
        assert_eq!(
            socket_path(Path::new("/p/default")),
            PathBuf::from("/p/default/live/settings.sock")
        );
    }
}
