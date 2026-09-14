//! Where a session is kept, once Cordial became the thing that keeps it.
//!
//! Cordial's cookie store made it the custodian of a live `.ROBLOSECURITY` —
//! a bearer token that is whole-account access — and its identity store added
//! the username and user id beside it. Both went into the profile directory as
//! plaintext at `0600`, and the argument for that being enough is written down
//! in [ADR-012](../../../docs/adr/ADR-012-profiles-and-instances.md).
//!
//! **That argument was wrong, and it was mine.** It said a keyring "adds an
//! unlock prompt to every launch and protects against nothing extra, because
//! the token has to be handed to the engine in plaintext regardless". The
//! second half is true and does not lead where I took it: the token being in
//! the clear *inside a running process* says nothing about it being in the
//! clear *on disk for ever*, and it is the disk copy that a backup, a sync
//! client, a container mount, a second application running as the same user, or
//! somebody reading over a shoulder actually reaches. The first half was
//! false on this platform, measured here: `org.freedesktop.secrets` is up and
//! answering `DBus.Peer.Ping`, and its default collection reports
//! `Locked = false` without anything being typed, because the login keyring is
//! unlocked by the session's own login.
//!
//! **Secret Service, not "GNOME keyring".** `org.freedesktop.secrets` is the
//! interface; `gnome-keyring-daemon` implements it on GNOME, KWallet and
//! KeePassXC implement it elsewhere, and libsecret is a client library for it.
//! Targeting the interface is what makes this work off GNOME, so this module
//! speaks the interface.
//!
//! **Why not libsecret.** libsecret is a C client for the D-Bus API below, and
//! Cordial already has a D-Bus client: `zbus` is a dependency of this crate,
//! and `android::accessibility` already hand-rolls `org.a11y.atspi` over it for
//! the same reason. Linking libsecret would add glib, gobject and a build-time
//! `libsecret-devel` that is *not installed on the machine this was written
//! on*, where `pkg-config --libs libsecret-1` resolves instead to a Homebrew
//! prefix under `/home/linuxbrew` — a release binary linked against that runs
//! on exactly one computer. The API is the same either way; only the client
//! differs.
//!
//! **A stored session is a convenience and never a prerequisite.** Every way
//! this can fail ends with a working client on the landing page and one line in
//! the log saying why: no service on the bus, a collection that is locked, a
//! read that comes back unusable, a service that stops answering mid-session.
//! Nothing here blocks startup on a keyring, and **nothing here ever asks the
//! service to unlock anything** — the collection's `Locked` property is read,
//! and a locked collection is treated as "not available", not as an error to
//! propagate and not as a reason to put a password dialog in front of somebody
//! who wanted to play Roblox. A user with auto-login never types the password
//! that would unlock the login keyring, so locked is the ordinary case on those
//! machines rather than an edge one.
//!
//! **What happens when it is not available** is [`Store::File`] — the same
//! `0600` file as before, with the warning printed every launch. That is not a
//! silent degradation: it is announced, it names the file, and
//! `CORDIAL_SECRET_STORE=keyring` refuses it for anyone who would rather have
//! no saved session than a plaintext one. The reasoning for the default is that
//! a user without a Secret Service is not made safer by being signed out — they
//! sign in again every launch *and* the next tool they use writes a token to
//! their disk anyway. Being told is worth more than being protected by
//! accident.
//!
//! **Nothing in this module prints a secret at any verbosity.** Bodies are
//! passed as strings and never formatted into a message; where a length is
//! useful, a length is what is reported. `String::from_utf8` failures drop the
//! bytes rather than carrying them into the error. The one thing not claimed is
//! memory hygiene: the body is an ordinary `String` and is not scrubbed on
//! drop, which would be theatre while the engine holds the same bytes in its
//! own heap for the life of the process.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

mod file;
mod keyring;
mod migration;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;

use file::{read_bounded, read_file, shred, write_private, BoundedRead};
use keyring::{ask, encode_keyring, read_keyring, Ask};
use migration::adopt_file;

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const IFACE_SERVICE: &str = "org.freedesktop.Secret.Service";
const IFACE_COLLECTION: &str = "org.freedesktop.Secret.Collection";
const IFACE_ITEM: &str = "org.freedesktop.Secret.Item";

/// The `xdg:schema` attribute libsecret-based tools key on, so that
/// `secret-tool` and Seahorse see these items as one family rather than as
/// loose rows. Not a security boundary — attributes are a search key.
const SCHEMA: &str = "org.cordial.Session";

/// What a stored body is, as far as the service is concerned.
const CONTENT_TYPE: &str = "text/plain; charset=utf8";

/// Keep Secret Service values to one ASCII line. The implementation used on
/// this desktop truncates text values containing cookie-style separators
/// (newline, tab and `=`), returning only the first comment line on read-back.
/// Hex encoding makes the service carry the value opaquely while leaving the
/// file backend and callers' formats unchanged.
const ENCODED_PREFIX: &str = "cordial-secret-hex-v1:";

/// How long the first question is allowed to take.
///
/// Deliberately short and deliberately on the startup path: if the answer is
/// "there is no secret service here", that answer has to arrive before the
/// user notices a launcher hesitating. A bus that is not there fails much
/// faster than this; the budget exists for the case where the name is
/// D-Bus-activatable and something has to be started.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long any later read or write is allowed to take.
///
/// A save runs on the looper thread, once per flush. An unbounded D-Bus call
/// there would not present as "the keyring is slow", it would present as the
/// client freezing mid-game, which is the worst possible way to learn that a
/// keyring daemon has wedged.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Which of the two stores a body belongs to.
///
/// The name doubles as the file name in [`Store::File`] and as the `store`
/// attribute in the service, so the two backends cannot drift into disagreeing
/// about what a thing is called.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Cookies,
    Identity,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Cookies => "cookies",
            Kind::Identity => "identity",
        }
    }

    /// What a person reading their keyring in Seahorse should see. Never
    /// contains a value; a label is displayed by other people's software.
    fn label(self, dir: &Path) -> String {
        let profile = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        match profile {
            Some(p) => format!("Cordial: Roblox {} for profile {p:?}", self.name()),
            None => format!("Cordial: Roblox {}", self.name()),
        }
    }
}

/// Where this instance keeps a session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Store {
    /// `org.freedesktop.secrets`, keyed per profile.
    Keyring,
    /// A `0600` file in the profile, in plaintext. Announced, never silent.
    File,
    /// Nothing is kept. Reached only by asking for `keyring` on a machine that
    /// has none: the session works, it is simply not saved.
    None,
}

impl Store {
    /// Explicit setting that makes a child process keep this selection.
    pub const fn setting_value(self) -> &'static str {
        match self {
            Store::Keyring | Store::None => "keyring",
            Store::File => "file",
        }
    }
}

/// One bounded, read-only view of a profile secret.
#[derive(Clone, Copy)]
pub struct SnapshotRequest<'a> {
    pub store: Store,
    pub profile_dir: &'a Path,
    pub kind: Kind,
    pub max_bytes: usize,
}

/// The setting, as an environment variable.
///
/// `auto` (the default) prefers the service and falls back to the file. `keyring`
/// refuses the fallback. `file` skips the service outright, which is also how
/// the tests get a deterministic backend without touching anybody's keyring.
///
/// The shell reports this choice in settings. Browser routing also pins the
/// resolved backend in the environment `launch.rs` builds for the client, so
/// two processes cannot resolve `auto` differently for one matched launch.
const SETTING: &str = "CORDIAL_SECRET_STORE";

/// Resolve the configured store and announce the result.
///
/// Entry points latch this result because it is a fact about the process and
/// the announcement must not repeat on every flush or browser request.
pub fn configured(fallback_file: &Path) -> Store {
    let requested = std::env::var(SETTING).unwrap_or_default();
    let (store, line) = decide(&requested, fallback_file, usable);
    println!("{line}");
    store
}

/// The choice, split out from the printing and from the bus.
///
/// The probe is a closure rather than a value for two reasons, and both are
/// about honesty. It lets the `file` answer skip the bus entirely, so a user
/// who has said they do not want a keyring does not pay a D-Bus round trip on
/// the startup path to be told so. And it lets the branch that matters most —
/// a service that is present but *locked*, which is the ordinary state on a
/// machine with auto-login — be tested for real rather than described, on a
/// developer machine whose own keyring must not be locked to find out.
fn decide(
    requested: &str,
    fallback_file: &Path,
    probe: impl FnOnce() -> Result<(), String>,
) -> (Store, String) {
    match requested {
        "file" => (
            Store::File,
            format!(
                "  [secrets] {SETTING}=file: the session is kept in plaintext at {}, by request",
                fallback_file.display()
            ),
        ),
        "keyring" | "auto" | "" => match probe() {
            Ok(()) => (
                Store::Keyring,
                "  [secrets] the session is kept in the desktop secret service \
                 (org.freedesktop.secrets); nothing is written to the profile"
                    .to_string(),
            ),
            Err(why) if requested == "keyring" => (
                Store::None,
                format!(
                    "  [secrets] {SETTING}=keyring, and {why}. This session will not be saved; \
                     you will sign in again next launch."
                ),
            ),
            Err(why) => (
                Store::File,
                format!(
                    "  [secrets] {why}, so the session falls back to a 0600 file at {}. \
                     Anything that can read your files can take the account. \
                     Set {SETTING}=keyring to refuse this and stay signed out instead.",
                    fallback_file.display()
                ),
            ),
        },
        other => (
            Store::File,
            format!(
                "  [secrets] {SETTING}={other:?} is not one of auto, keyring, file; \
                 treating it as file, which keeps the session in plaintext at {}",
                fallback_file.display()
            ),
        ),
    }
}

/// Whether the service is there *and* open, without asking it to open.
pub fn usable() -> Result<(), String> {
    ask(Ask::Usable, PROBE_TIMEOUT).map(|_| ())
}

/// Read a body back, or `None` for "nothing saved", which is the honest answer
/// for missing, locked, unreadable and malformed alike.
///
/// Every failure here is a signed-out launch and not a failed one. That is the
/// rule the whole module is built to: losing the stored session degrades to
/// "sign in again", never to "the client will not start".
pub fn load(store: Store, dir: &Path, kind: Kind) -> Option<String> {
    match store {
        // Not read, and not destroyed either. Somebody who asked for
        // keyring-or-nothing on a machine that turned out to have no keyring
        // has asked to be signed out, not to have a file they may still want
        // deleted out from under them. Saying it is there is the difference
        // between a decision and an accident.
        Store::None => {
            let path = dir.join(kind.name());
            if path.exists() {
                println!(
                    "  [secrets] {} is present in plaintext at {} and is being ignored, \
                     because {SETTING}=keyring. Delete it if you no longer want it there.",
                    kind.name(),
                    path.display()
                );
            }
            None
        }
        Store::File => read_file(&dir.join(kind.name())),
        Store::Keyring => {
            // A plaintext store in the profile is, by construction, newer than
            // anything in the service: the keyring path shreds the file the
            // moment it has taken it in, so a file that still exists is one the
            // file backend wrote after the last keyring write. Taking it in
            // here rather than in a separate migration step means the move
            // happens on the first launch after the upgrade, without anybody
            // having to run anything.
            if let Some(body) = adopt_file(dir, kind) {
                return Some(body);
            }
            match read_keyring(&attributes(dir, kind)) {
                Ok(body) => body,
                Err(why) => {
                    println!(
                        "  [secrets] {}: not read back ({why}); signed out",
                        kind.name()
                    );
                    None
                }
            }
        }
    }
}

/// Read exactly what a runtime load would consume, without changing storage.
///
/// The keyring runtime adopts a readable plaintext file before consulting the
/// service. Routing follows that precedence but deliberately leaves the file
/// and keyring untouched. Missing, locked, malformed and oversized values all
/// fail closed.
pub fn snapshot(request: SnapshotRequest<'_>) -> Option<Vec<u8>> {
    snapshot_with(request, read_keyring)
}

fn snapshot_with(
    request: SnapshotRequest<'_>,
    keyring_read: impl FnOnce(&HashMap<String, String>) -> Result<Option<String>, String>,
) -> Option<Vec<u8>> {
    let file = || {
        read_bounded(
            &request.profile_dir.join(request.kind.name()),
            request.max_bytes,
        )
    };
    match request.store {
        Store::None => None,
        Store::File => match file() {
            BoundedRead::Present(bytes) => Some(bytes),
            BoundedRead::Unavailable | BoundedRead::TooLarge => None,
        },
        Store::Keyring => match file() {
            BoundedRead::Present(bytes) => Some(bytes),
            BoundedRead::TooLarge => None,
            BoundedRead::Unavailable => {
                let body = keyring_read(&attributes(request.profile_dir, request.kind)).ok()??;
                (body.len() <= request.max_bytes).then(|| body.into_bytes())
            }
        },
    }
}

/// Write a body, or say plainly that it was not written.
///
/// The caller reports success; this reports its own failures, because a session
/// that was not saved is something the user has to be told about at the moment
/// it happens. Silence would leave them assuming they are signed in and
/// discovering otherwise at the next launch.
pub fn save(store: Store, dir: &Path, kind: Kind, body: &str) -> std::io::Result<()> {
    match store {
        Store::None => Ok(()),
        Store::File => write_private(dir, kind.name(), body),
        Store::Keyring => match ask(
            Ask::Write {
                attrs: attributes(dir, kind),
                label: kind.label(dir),
                body: encode_keyring(body),
            },
            CALL_TIMEOUT,
        ) {
            Ok(_) => Ok(()),
            Err(why) => Err(std::io::Error::other(why)),
        },
    }
}

/// Remove a body. Absent is success: the state being asked for is "nothing
/// saved", and that is already true.
pub fn erase(store: Store, dir: &Path, kind: Kind) -> std::io::Result<()> {
    // Both backends, whichever is active. A profile that has been through the
    // file backend can still have a file, and a logout that leaves a stale
    // identity behind is the failure `identity::observe_logout` exists to
    // prevent: a shell that looks signed in for an account whose cookie the
    // server has already thrown away.
    let file = dir.join(kind.name());
    if file.exists() {
        shred(&file)?;
    }
    if store == Store::Keyring {
        if let Err(why) = ask(Ask::Remove(attributes(dir, kind)), CALL_TIMEOUT) {
            return Err(std::io::Error::other(why));
        }
    }
    Ok(())
}

/// Where a session is kept, for a startup line to name.
pub fn where_kept(store: Store, dir: &Path, kind: Kind) -> String {
    match store {
        Store::Keyring => "the desktop secret service".to_string(),
        Store::File => dir.join(kind.name()).display().to_string(),
        Store::None => "nowhere; this session is not being saved".to_string(),
    }
}

/// How the service tells one profile's session from another's.
///
/// **Keyed by the profile's full path, not by its name, and that is not
/// fussiness.** Every agent and every test in this repository is told to run
/// under its own `XDG_DATA_HOME`, and every one of those roots contains a
/// profile called `default`. Keying on the name would have a scratch profile
/// read, overwrite and delete the session of the profile somebody actually
/// plays on, which is the one thing here that cannot be rebuilt by re-running
/// something.
fn attributes(dir: &Path, kind: Kind) -> HashMap<String, String> {
    HashMap::from([
        ("xdg:schema".to_string(), SCHEMA.to_string()),
        ("application".to_string(), "cordial".to_string()),
        ("profile".to_string(), dir.display().to_string()),
        ("store".to_string(), kind.name().to_string()),
    ])
}
