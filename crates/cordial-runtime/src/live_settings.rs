//! The client's end of the shell's live-settings channel (ADR-044).
//!
//! The shell used to hand every setting to the client once, in the launch
//! environment, so changing one in Settings did nothing to a game already
//! running while the row looked as if it had saved. This listens on a Unix
//! socket inside the profile and applies the few settings that are read on a
//! hot path and can safely change: each one is an atomic in the module that
//! uses it, so applying a message is a store from this thread and needs no
//! hand-off to the pump (unlike `devctl`, which calls engine natives). The one
//! exception is the audio sink, which is changed by the audio backend itself
//! (`cordial_audio_set_output`): it re-links the streams PipeWire already has
//! and never touches the engine's OpenSL ES or AAudio objects.
//!
//! **What this is not.** It is not `devctl`. That socket is opt-in, drives input
//! and captures frames, and must stay off for anyone who did not ask for it
//! (ADR-019); this one is always on, and can only set the values in
//! `cordial_shell::live_wire::Update`. Anything else in a message is refused or
//! reported as ignored. It also does not reach the engine, read files, or
//! start anything.
//!
//! Access is by directory: the socket sits in a `0700` directory inside the
//! profile, which ADR-012's `flock` already gives one owner and which the
//! plugin sandbox does not bind (ADR-003). There is deliberately no
//! credential-passing check on top; the only party that can open the path is
//! the user's own account, which could equally edit `shell.json`.

use cordial_shell::live_wire::{self, Accel, Reply, Request, Throttle, Update};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

/// How long a peer may sit silent before its connection is dropped. The listener
/// is single-threaded, so an idle peer must not be able to hold it.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Start listening for this profile. Failure is reported and ignored: a client
/// that cannot take live changes still runs, and the settings page only claims
/// "applies now" for what the socket accepted.
pub fn start() {
    let path = live_wire::socket_path(&crate::profile::active());
    match bind(&path) {
        Ok(listener) => {
            println!("  live: listening on {}", path.display());
            std::thread::Builder::new()
                .name("cordial-live".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        serve(stream);
                    }
                })
                .ok();
        }
        Err(e) => println!("  live: could not bind {} ({e}); settings apply at launch only", path.display()),
    }
}

/// Create the private directory and bind the socket in it.
pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // `recursive` leaves an existing directory as it found it; narrowing it
        // here is what makes the "same user only" claim true for one that a
        // previous run or a hand-made `mkdir` created wider.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // Stale after a killed client. Safe because the profile lock has already
    // established that no other instance owns this directory.
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path)
}

fn serve(stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let mut line = String::new();
    // One byte past the limit, so an over-long line is seen as over-long rather
    // than silently truncated into something that might parse.
    let limit = live_wire::MAX_LINE as u64 + 1;
    let read = match stream.try_clone() {
        Ok(s) => BufReader::new(s.take(limit)).read_line(&mut line),
        Err(e) => Err(e),
    };
    let reply = match read {
        Ok(0) => return,
        Ok(_) => handle(&line),
        Err(e) => Reply::failure(format!("could not read a request: {e}")),
    };
    // Whatever the peer sent past the limit is discarded before replying. Closing
    // a socket with unread bytes makes Linux reset the connection, and the peer
    // would then see a reset instead of the refusal that explains itself.
    if stream.set_nonblocking(true).is_ok() {
        let mut sink = [0u8; 4096];
        let mut left = 64 * 1024;
        while left > 0 {
            match (&stream).read(&mut sink) {
                Ok(n) if n > 0 => left -= n.min(left),
                _ => break,
            }
        }
        let _ = stream.set_nonblocking(false);
    }
    let mut out = stream;
    let _ = out.write_all(reply.encode().as_bytes());
    let _ = out.flush();
}

/// One request line to one reply. Pure apart from the stores in [`apply`].
pub fn handle(line: &str) -> Reply {
    match live_wire::decode(line) {
        Err(why) => Reply::failure(why),
        Ok(Request::Get) => Reply { ok: true, values: current(), ..Reply::default() },
        Ok(Request::Set { updates, ignored }) => {
            let mut applied = Vec::new();
            let mut notes = BTreeMap::new();
            for u in &updates {
                if let Some(note) = apply(u) {
                    notes.insert(u.key().to_string(), note);
                }
                applied.push(u.key().to_string());
            }
            Reply { ok: true, applied, ignored, values: current(), notes, error: None }
        }
    }
}

/// Applies one update. The `Some` is something worth telling the shell about
/// that is not a failure: a sink change with nothing playing has nothing to move.
fn apply(update: &Update) -> Option<String> {
    let note = match update {
        Update::PointerAcceleration(a) => {
            crate::android::wayland::set_pointer_acceleration(*a == Accel::Always);
            None
        }
        Update::Throttle(t) => {
            crate::android::input::set_throttle_policy(match t {
                Throttle::Visible => crate::android::input::ThrottleWhen::Visible,
                Throttle::Unfocused => crate::android::input::ThrottleWhen::Unfocused,
                Throttle::Off => crate::android::input::ThrottleWhen::Off,
            });
            None
        }
        Update::CloseOnLeave(b) => {
            crate::game_log::set_close_on_leave(*b);
            None
        }
        Update::CarryLaunchTicket(b) => {
            crate::deeplink::set_carry_ticket(*b);
            None
        }
        Update::AudioOutput(name) => {
            let (moved, note) = audio::set_output(name);
            println!("  live: audio_output: {moved} playing stream(s) moved");
            (!note.is_empty()).then_some(note)
        }
        // A request to gamemoded, made on the spot. The note is whatever the
        // daemon said that was not "yes", or that it has not answered yet.
        Update::Gamemode(on) => crate::gamemode::set_enabled(*on),
        // Stored here and acted on by the pump's next `gamepad::poll`, which is
        // the thread the engine's natives may be called from.
        Update::Gamepad(on) => {
            crate::android::gamepad::set_enabled(*on);
            None
        }
    };
    // Narrated because this project debugs by reading the client's output, and
    // "the setting reached the process" is the fact worth being able to see.
    println!("  live: {} -> {}", update.key(), value_word(update));
    note
}

fn value_word(update: &Update) -> String {
    match update {
        Update::PointerAcceleration(a) => a.as_str().to_string(),
        Update::Throttle(t) => t.as_str().to_string(),
        Update::CloseOnLeave(b) | Update::CarryLaunchTicket(b) | Update::Gamemode(b) | Update::Gamepad(b) => {
            b.to_string()
        }
        Update::AudioOutput(name) if name.is_empty() => "the system default".to_string(),
        Update::AudioOutput(name) => name.clone(),
    }
}

/// The audio backend's end of `audio_output`. Declared rather than wrapped in a
/// module of its own because it is two calls into `native/pipewire_backend.cpp`.
mod audio {
    use std::ffi::CString;
    use std::os::raw::c_char;

    extern "C" {
        fn cordial_audio_set_output(name: *const c_char, note: *mut c_char, note_len: usize) -> usize;
        fn cordial_audio_output(out: *mut c_char, out_len: usize) -> usize;
    }

    /// Aim playback at `name` (empty for the system default). Returns how many
    /// playing streams moved and a note when the answer is more than "all of
    /// them".
    pub fn set_output(name: &str) -> (usize, String) {
        // The wire has already refused control characters, which include NUL, so
        // this only fails if a caller skipped that check. Refused rather than
        // truncated at the NUL into a different sink.
        let Ok(c) = CString::new(name) else {
            return (0, "the sink name contains a NUL byte".to_string());
        };
        let mut note = vec![0u8; 512];
        // Safety: `c` is NUL-terminated and outlives the call; `note` is writable
        // for `note.len()` bytes and the callee NUL-terminates within it.
        let moved = unsafe {
            cordial_audio_set_output(c.as_ptr(), note.as_mut_ptr().cast::<c_char>(), note.len())
        };
        (moved, nul_terminated(&note))
    }

    /// The sink in force, empty for the system default.
    pub fn current() -> String {
        let mut buf = vec![0u8; 512];
        // Safety: as above.
        unsafe { cordial_audio_output(buf.as_mut_ptr().cast::<c_char>(), buf.len()) };
        nul_terminated(&buf)
    }

    fn nul_terminated(buf: &[u8]) -> String {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }
}

/// What is in force now, in the wire's own words.
fn current() -> BTreeMap<String, Value> {
    use crate::android::input::{throttle_policy, ThrottleWhen};
    let mut m = BTreeMap::new();
    m.insert(
        "pointer_acceleration".into(),
        Value::from(
            if crate::android::wayland::current_pointer_acceleration() {
                Accel::Always
            } else {
                Accel::Unlocked
            }
            .as_str(),
        ),
    );
    m.insert(
        "throttle".into(),
        Value::from(
            match throttle_policy() {
                ThrottleWhen::Visible => Throttle::Visible,
                ThrottleWhen::Unfocused => Throttle::Unfocused,
                ThrottleWhen::Off => Throttle::Off,
            }
            .as_str(),
        ),
    );
    m.insert("close_on_leave".into(), Value::from(crate::game_log::current_close_on_leave()));
    m.insert("carry_launch_ticket".into(), Value::from(crate::deeplink::carry_ticket()));
    m.insert("audio_output".into(), Value::from(audio::current()));
    m.insert("gamemode".into(), Value::from(crate::gamemode::current()));
    m.insert("gamepad".into(), Value::from(crate::android::gamepad::current_enabled()));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// A private scratch directory that removes itself. `tempfile` would be a
    /// new dependency for this crate, and four tests do not justify one.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("cordial-live-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The atomics are process-global, so tests that store to them take this
    /// lock and put the values back.
    static GLOBALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn talk(path: &Path, request: &str) -> Reply {
        let mut s = UnixStream::connect(path).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        BufReader::new(s).read_line(&mut reply).unwrap();
        Reply::decode(&reply).unwrap()
    }

    #[test]
    fn a_set_changes_what_the_client_reads_and_get_reports_it() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = Scratch::new("set");
        let path = live_wire::socket_path(dir.path());
        let listener = bind(&path).unwrap();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten().take(3) {
                serve(s);
            }
        });

        let before = talk(&path, &live_wire::encode_get());
        assert!(before.ok);

        let set = live_wire::encode_set(&[
            Update::PointerAcceleration(Accel::Unlocked),
            Update::Throttle(Throttle::Off),
        ]);
        let reply = talk(&path, &set);
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.values["pointer_acceleration"], "unlocked");
        assert_eq!(reply.values["throttle"], "off");
        assert!(!crate::android::wayland::current_pointer_acceleration());
        assert_eq!(
            crate::android::input::throttle_policy(),
            crate::android::input::ThrottleWhen::Off
        );

        // Control: the opposite message flips it back in the same process.
        let back = talk(
            &path,
            &live_wire::encode_set(&[
                Update::PointerAcceleration(Accel::Always),
                Update::Throttle(Throttle::Visible),
            ]),
        );
        assert_eq!(back.values["pointer_acceleration"], "always");
        assert!(crate::android::wayland::current_pointer_acceleration());
    }

    #[test]
    fn a_bad_or_foreign_request_changes_nothing_and_says_why() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let before = current();
        for bad in [
            "not json\n",
            "{\"exec\":\"ls\"}\n",
            "{\"set\":{\"throttle\":\"sometimes\"}}\n",
            "{\"set\":{\"audio_output\":true}}\n",
            "{\"set\":{\"gamemode\":\"yes\"}}\n",
            "{\"set\":{\"gamepad\":1}}\n",
            "{\"set\":{\"audio_output\":\"a\\u0000b\"}}\n",
        ] {
            let r = handle(bad);
            assert!(!r.ok, "{bad:?} should fail");
            assert!(r.error.is_some());
        }
        let r = handle("{\"set\":{\"unheard_of\":1}}\n");
        assert!(r.ok && r.applied.is_empty() && r.ignored == vec!["unheard_of".to_string()]);
        assert_eq!(current(), before, "nothing above may have moved a setting");
    }

    #[test]
    fn a_sink_change_reaches_the_audio_backend_and_get_reports_it() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let before = audio::current();

        let set = |name: &str| handle(&live_wire::encode_set(&[Update::AudioOutput(name.into())]));
        let r = set("a-sink-that-does-not-exist");
        assert!(r.ok, "{r:?}");
        assert_eq!(r.applied, vec!["audio_output".to_string()]);
        assert_eq!(r.values["audio_output"], "a-sink-that-does-not-exist");
        assert_eq!(audio::current(), "a-sink-that-does-not-exist");
        // No stream is playing in a unit test. The reply says so instead of
        // letting "applied" be read as "you will hear it".
        assert!(r.notes.contains_key("audio_output"), "{r:?}");

        // A get reports what is in force, and the opposite change flips it back;
        // empty is the system default.
        assert_eq!(handle(&live_wire::encode_get()).values["audio_output"], "a-sink-that-does-not-exist");
        let back = set("");
        assert_eq!(back.values["audio_output"], "");
        assert_eq!(audio::current(), "");

        set(&before);
    }

    #[test]
    fn a_gamemode_change_is_recorded_and_get_reports_it() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let before = crate::gamemode::current();
        let set = |on: bool| handle(&live_wire::encode_set(&[Update::Gamemode(on)]));

        // Startup registration has not run in a unit test, so this only records
        // the wish and there is nothing to tell the daemon. The daemon side is
        // exercised in `gamemode`'s own tests with a fake one.
        let r = set(!before);
        assert!(r.ok && r.applied == vec!["gamemode".to_string()], "{r:?}");
        assert_eq!(r.values["gamemode"], !before);
        assert!(!r.notes.contains_key("gamemode"), "{r:?}");
        assert_eq!(handle(&live_wire::encode_get()).values["gamemode"], !before);

        // Control: the opposite message flips it back.
        assert_eq!(set(before).values["gamemode"], before);
    }

    #[test]
    fn a_gamepad_change_reaches_the_switch_and_get_reports_it() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let before = crate::android::gamepad::current_enabled();
        let set = |on: bool| handle(&live_wire::encode_set(&[Update::Gamepad(on)]));

        let r = set(!before);
        assert!(r.ok && r.applied == vec!["gamepad".to_string()], "{r:?}");
        assert_eq!(r.values["gamepad"], !before);
        assert_eq!(crate::android::gamepad::current_enabled(), !before);

        // Control: the opposite message flips it back in the same process.
        assert_eq!(set(before).values["gamepad"], before);
        assert_eq!(crate::android::gamepad::current_enabled(), before);
    }

    #[test]
    fn a_sink_name_with_a_control_character_changes_nothing() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let before = audio::current();
        let r = handle("{\"set\":{\"audio_output\":\"two\\nlines\"}}\n");
        assert!(!r.ok);
        assert_eq!(audio::current(), before);
    }

    #[test]
    fn the_socket_directory_is_private_even_when_it_already_existed_wide() {
        let dir = Scratch::new("mode");
        let live = dir.path().join(live_wire::SOCKET_DIR);
        std::fs::create_dir(&live).unwrap();
        std::fs::set_permissions(&live, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _l = bind(&live_wire::socket_path(dir.path())).unwrap();
        let mode = std::fs::metadata(&live).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn an_oversized_request_is_refused_not_buffered() {
        let _g = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = Scratch::new("big");
        let path = live_wire::socket_path(dir.path());
        let listener = bind(&path).unwrap();
        std::thread::spawn(move || {
            if let Some(s) = listener.incoming().flatten().next() {
                serve(s);
            }
        });
        let big = format!("{}\n", "x".repeat(live_wire::MAX_LINE * 4));
        let reply = talk(&path, &big);
        assert!(!reply.ok);
    }
}
