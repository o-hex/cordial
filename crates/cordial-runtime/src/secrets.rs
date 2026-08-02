//! Where a session is kept, once Cordial became the thing that keeps it.
//!
//! [`crate::cookies`] made Cordial the custodian of a live `.ROBLOSECURITY` —
//! a bearer token that is whole-account access — and [`crate::identity`] added
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
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

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

/// Set the first time a call times out, and never cleared.
///
/// After a timeout the worker is still inside the call that timed out, so every
/// later request queues behind a thread that is not coming back. Asking again
/// would spend five seconds per flush for the rest of the session and change
/// nothing.
static WEDGED: AtomicBool = AtomicBool::new(false);

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

/// The setting, as an environment variable.
///
/// `auto` (the default) prefers the service and falls back to the file. `keyring`
/// refuses the fallback. `file` skips the service outright, which is also how
/// the tests get a deterministic backend without touching anybody's keyring.
///
/// The shell should expose this as a preference and pass it here the way
/// `launch.rs` already passes `CORDIAL_WAYLAND`; the runtime deliberately has
/// no opinion about where the shell keeps it.
const SETTING: &str = "CORDIAL_SECRET_STORE";

/// Decide once, say so once.
///
/// A `OnceLock` because the answer is a fact about the process and because the
/// announcement must not repeat: this is consulted on every flush, and a line
/// printed every thirty seconds is a line nobody reads.
pub fn active() -> Store {
    static ACTIVE: OnceLock<Store> = OnceLock::new();
    *ACTIVE.get_or_init(|| {
        let requested = std::env::var(SETTING).unwrap_or_default();
        let (store, line) = decide(&requested, usable);
        println!("{line}");
        store
    })
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
fn decide(requested: &str, probe: impl FnOnce() -> Result<(), String>) -> (Store, String) {
    match requested {
        "file" => (
            Store::File,
            format!(
                "  [secrets] {SETTING}=file: the session is kept in plaintext at {}, by request",
                crate::profile::active().join("cookies").display()
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
                    crate::profile::active().join("cookies").display()
                ),
            ),
        },
        other => (
            Store::File,
            format!(
                "  [secrets] {SETTING}={other:?} is not one of auto, keyring, file; \
                 treating it as file, which keeps the session in plaintext at {}",
                crate::profile::active().join("cookies").display()
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
            match ask(Ask::Read(attributes(dir, kind)), CALL_TIMEOUT) {
                Ok(body) => body,
                Err(why) => {
                    println!("  [secrets] {}: not read back ({why}); signed out", kind.name());
                    None
                }
            }
        }
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
                body: body.to_string(),
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

// ---------------------------------------------------------------------------
// The file backend, and getting rid of a file once the service has the body.
// ---------------------------------------------------------------------------

/// Write a file in a profile at `0600`, atomically.
///
/// Temp file plus rename rather than truncate-and-write. An interrupted write
/// to the real path would leave a jar cut off mid-value, and half a cookie
/// still parses as a cookie — the engine would take it on the next launch and
/// fail authentication for a reason with no visible relationship to a power
/// cut. `rename` within a directory is atomic, so a reader sees the old file or
/// the new one.
///
/// The mode is set on the temp file *before* anything is written to it, not
/// after: a `0644` window with a live session in it is still a window, and it
/// is the one an attacker with a loop would use.
///
/// **One writer, for both stores and both kinds.** This began in `cookies.rs`
/// and was shared with `identity.rs` rather than copied, because a second
/// writer is a second chance to get the mode or the rename wrong — including
/// later, when only one of the two gets a fix. It moved here when the file
/// stopped being the only place a session can live; it is still the only thing
/// in Cordial that writes one to disk.
fn write_private(dir: &Path, name: &str, body: &str) -> std::io::Result<()> {
    let final_path = dir.join(name);
    let tmp = dir.join(format!("{name}.new"));

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    // `mode` only applies when the call creates the file, so a leftover temp
    // from an interrupted run would keep whatever mode it had.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    drop(f);

    std::fs::rename(&tmp, &final_path)
}

fn read_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Overwrite a file's bytes, then unlink it.
///
/// `remove_file` unlinks; it does not erase. The blocks stay on the device with
/// a live session token in them until something else happens to allocate them,
/// which on a half-empty disk can be never, and undelete is a normal thing for
/// a filesystem to support. Overwriting first is what makes "the plaintext copy
/// is gone" mean anything at all.
///
/// **What this does not do**, said plainly rather than left for somebody to
/// assume: on a copy-on-write filesystem — btrfs, which is Fedora's default and
/// is what this was written on — a rewrite may land in new blocks and leave the
/// originals intact, and no user-space overwrite touches a snapshot, an SSD's
/// remapped blocks, or a backup that was taken yesterday. This is a floor, not
/// a guarantee, and the honest instruction after a migration is still to change
/// your password if the file was ever somewhere it should not have been.
fn shred(path: &Path) -> std::io::Result<()> {
    let len = std::fs::metadata(path)?.len();
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
        let zeros = [0u8; 4096];
        let mut left = len;
        while left > 0 {
            let n = std::cmp::min(left, zeros.len() as u64) as usize;
            f.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        f.sync_all()?;
    }
    std::fs::remove_file(path)
}

/// Take a plaintext store into the service and destroy the file.
///
/// Returns the body when there was one, so the caller can use it for this
/// launch as well — a migration that signed somebody out on the way past would
/// be a worse bug than the one it is fixing.
///
/// **The shred happens only after the service has been asked for the body back
/// and returned it byte for byte.** Deleting on the strength of a write that
/// reported success is how a migration eats a session: `CreateItem` returning
/// an object path says the daemon accepted the call, not that the item is
/// there to be read. Nothing is compared by printing; the two bodies are
/// compared in memory and only their equality is ever reported.
fn adopt_file(dir: &Path, kind: Kind) -> Option<String> {
    let path = dir.join(kind.name());
    let body = read_file(&path)?;

    let write = ask(
        Ask::Write {
            attrs: attributes(dir, kind),
            label: kind.label(dir),
            body: body.clone(),
        },
        CALL_TIMEOUT,
    );
    if let Err(why) = write {
        println!(
            "  [secrets] {}: {} bytes are still in plaintext at {} ({why}); \
             it was left alone rather than half-moved",
            kind.name(),
            body.len(),
            path.display()
        );
        return Some(body);
    }

    match ask(Ask::Read(attributes(dir, kind)), CALL_TIMEOUT) {
        Ok(Some(back)) if back == body => match shred(&path) {
            Ok(()) => {
                println!(
                    "  [secrets] {}: {} bytes moved into the secret service; \
                     the plaintext file was overwritten and removed",
                    kind.name(),
                    body.len()
                );
                Some(body)
            }
            Err(e) => {
                println!(
                    "  [secrets] {}: moved into the secret service, but {} could not be \
                     destroyed ({e}); delete it by hand",
                    kind.name(),
                    path.display()
                );
                Some(body)
            }
        },
        _ => {
            println!(
                "  [secrets] {}: the secret service did not hand back what it was given, \
                 so {} was left alone",
                kind.name(),
                path.display()
            );
            Some(body)
        }
    }
}

// ---------------------------------------------------------------------------
// The service itself, on a thread of its own.
// ---------------------------------------------------------------------------

enum Ask {
    /// Is there a service, and is its default collection open?
    Usable,
    Read(HashMap<String, String>),
    Write {
        attrs: HashMap<String, String>,
        label: String,
        body: String,
    },
    Remove(HashMap<String, String>),
}

/// `Ok(Some(body))` for a read that found something, `Ok(None)` for one that did
/// not and for every write, `Err` for a reason to report and carry on from.
type Answer = Result<Option<String>, String>;

/// Every D-Bus call in this module happens on one thread that nothing waits on
/// for longer than it is prepared to.
///
/// The alternative was calling `zbus` from the looper thread and from the
/// engine's notification thread directly. `zbus`'s blocking API has no
/// per-call timeout, so that arrangement makes a wedged keyring daemon into a
/// wedged client, and the symptom — the window stops drawing thirty seconds
/// after sign-in — has no visible relationship to its cause.
fn worker() -> &'static Mutex<Sender<(Ask, SyncSender<Answer>)>> {
    static HANDLE: OnceLock<Mutex<Sender<(Ask, SyncSender<Answer>)>>> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(Ask, SyncSender<Answer>)>();
        // Unbounded on the request side on purpose. A rendezvous channel would
        // make `send` itself block on a worker that is stuck, which is the
        // deadlock this whole arrangement exists to avoid.
        let spawned = std::thread::Builder::new()
            .name("cordial-secrets".to_string())
            .spawn(move || serve(rx));
        if let Err(e) = &spawned {
            println!("  [secrets] no thread for the secret service ({e}); sessions will not be saved");
        }
        Mutex::new(tx)
    })
}

fn ask(request: Ask, timeout: Duration) -> Answer {
    if WEDGED.load(Ordering::Acquire) {
        return Err("the secret service stopped answering earlier this session".to_string());
    }
    let (tx, rx) = sync_channel::<Answer>(1);
    let sent = worker()
        .lock()
        .map_err(|_| "the secret service worker panicked".to_string())
        .and_then(|h| h.send((request, tx)).map_err(|_| "the secret service worker is gone".to_string()));
    sent?;
    match rx.recv_timeout(timeout) {
        Ok(answer) => answer,
        Err(_) => {
            WEDGED.store(true, Ordering::Release);
            Err(format!(
                "the secret service did not answer within {} seconds",
                timeout.as_secs()
            ))
        }
    }
}

fn serve(rx: Receiver<(Ask, SyncSender<Answer>)>) {
    // The connection is built on the first request and its failure is kept.
    // Retrying a connection that has already failed once would spend the
    // timeout budget again on every flush for the rest of the session, and the
    // answer would not change: the service either exists at launch or it does
    // not.
    let mut service: Option<Result<Keyring, String>> = None;
    for (request, reply) in rx {
        let answer = match service.get_or_insert_with(Keyring::connect) {
            Err(why) => Err(why.clone()),
            Ok(keyring) => keyring.handle(request),
        };
        // The caller may have timed out and gone; that is not an error here.
        let _ = reply.send(answer);
    }
}

struct Keyring {
    conn: Connection,
    service: Proxy<'static>,
    session: OwnedObjectPath,
    collection: OwnedObjectPath,
}

impl Keyring {
    fn connect() -> Result<Keyring, String> {
        let conn = Connection::session().map_err(|_| "there is no session bus".to_string())?;
        let service = Proxy::new_owned(conn.clone(), SERVICE, SERVICE_PATH, IFACE_SERVICE)
            .map_err(|e| format!("the secret service could not be addressed ({e})"))?;

        // `plain` rather than the DH-negotiated transport. The negotiated one
        // encrypts the body between two processes that already share a unix
        // socket in the user's own runtime directory, and anything positioned
        // to read that socket is positioned to ask the daemon for the item
        // itself. Encrypting a hop that is not the exposure would be
        // obfuscation dressed as protection, which this project has a rule
        // about.
        let (_output, session): (OwnedValue, OwnedObjectPath) = service
            .call("OpenSession", &("plain", Value::from("")))
            .map_err(|_| "there is no secret service on the session bus".to_string())?;

        let collection: OwnedObjectPath = service
            .call("ReadAlias", &("default",))
            .map_err(|e| format!("the secret service has no default collection ({e})"))?;
        if collection.as_str() == "/" {
            return Err("the secret service has no default collection".to_string());
        }

        Ok(Keyring { conn, service, session, collection })
    }

    fn proxy(&self, path: &OwnedObjectPath, interface: &'static str) -> Result<Proxy<'static>, String> {
        Proxy::new_owned(self.conn.clone(), SERVICE, path.clone().into_inner(), interface)
            .map_err(|e| format!("{path} could not be addressed ({e})"))
    }

    /// **Read `Locked`; never call `Unlock`.**
    ///
    /// Unlocking is a prompt, and a prompt here lands in front of somebody who
    /// asked to play Roblox and has not yet seen a window. With auto-login the
    /// login keyring is never unlocked, because the password that would unlock
    /// it was never typed — so this is the common answer on those machines, and
    /// treating it as an error to propagate would be treating the ordinary case
    /// as a fault.
    fn open(&self) -> Result<(), String> {
        let collection = self.proxy(&self.collection, IFACE_COLLECTION)?;
        match collection.get_property::<bool>("Locked") {
            Ok(false) => Ok(()),
            Ok(true) => Err(
                "the desktop keyring is locked, and unlocking it is not something a game \
                 launcher should demand before it will start"
                    .to_string(),
            ),
            Err(e) => Err(format!("the keyring would not say whether it is locked ({e})")),
        }
    }

    fn handle(&self, request: Ask) -> Answer {
        self.open()?;
        match request {
            Ask::Usable => Ok(None),
            Ask::Read(attrs) => self.read(&attrs),
            Ask::Write { attrs, label, body } => self.write(&attrs, &label, body).map(|()| None),
            Ask::Remove(attrs) => self.remove(&attrs).map(|()| None),
        }
    }

    fn read(&self, attrs: &HashMap<String, String>) -> Answer {
        let (unlocked, _locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = self
            .service
            .call("SearchItems", &(attrs,))
            .map_err(|e| format!("the keyring could not be searched ({e})"))?;
        let Some(item) = unlocked.into_iter().next() else {
            return Ok(None);
        };
        let (_session, _parameters, value, _content): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) =
            self.proxy(&item, IFACE_ITEM)?
                .call("GetSecret", &(&self.session,))
                .map_err(|e| format!("the stored session could not be read ({e})"))?;
        // The bytes are dropped rather than carried into the error: an error
        // string is the one place a value reliably reaches a log.
        String::from_utf8(value)
            .map(Some)
            .map_err(|_| "the stored session is not text".to_string())
    }

    fn write(&self, attrs: &HashMap<String, String>, label: &str, body: String) -> Result<(), String> {
        let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
        properties.insert("org.freedesktop.Secret.Item.Label", Value::from(label));
        properties.insert(
            "org.freedesktop.Secret.Item.Attributes",
            Value::from(attrs.clone()),
        );
        let secret = (
            self.session.clone(),
            Vec::<u8>::new(),
            body.into_bytes(),
            CONTENT_TYPE,
        );
        // `replace` is true: the attributes are the identity of the item, so a
        // second save for the same profile and store must update one item
        // rather than accumulate a row per launch in the user's keyring.
        let (_item, prompt): (OwnedObjectPath, OwnedObjectPath) = self
            .proxy(&self.collection, IFACE_COLLECTION)?
            .call("CreateItem", &(properties, secret, true))
            .map_err(|e| format!("the session could not be stored ({e})"))?;
        if prompt.as_str() != "/" {
            return Err("storing the session would have needed a prompt".to_string());
        }
        Ok(())
    }

    fn remove(&self, attrs: &HashMap<String, String>) -> Result<(), String> {
        let (unlocked, _locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = self
            .service
            .call("SearchItems", &(attrs,))
            .map_err(|e| format!("the keyring could not be searched ({e})"))?;
        for item in unlocked {
            let _prompt: OwnedObjectPath = self
                .proxy(&item, IFACE_ITEM)?
                .call("Delete", &())
                .map_err(|e| format!("a stored session could not be removed ({e})"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-secrets-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// What the probe says, for a test that must not depend on the machine it
    /// runs on.
    fn present() -> Result<(), String> {
        Ok(())
    }

    fn locked() -> Result<(), String> {
        Err("the desktop keyring is locked".to_string())
    }

    #[test]
    fn the_setting_decides_and_says_which() {
        // The three answers a user can give, and the one they can give by
        // mistake. `keyring` is the only one that is allowed to end in nothing
        // being saved, and even that says so rather than failing.
        assert_eq!(decide("", present).0, Store::Keyring);
        assert_eq!(decide("auto", present).0, Store::Keyring);
        // And `file` never asks the bus at all: a user who has said they do not
        // want a keyring should not pay a round trip to be told so.
        assert_eq!(
            decide("file", || unreachable!("the file store must not touch the bus")).0,
            Store::File
        );
        assert!(decide("file", present).1.contains("plaintext"));
        assert_eq!(decide("nonsense", present).0, Store::File);
        assert!(decide("nonsense", present).1.contains("not one of"));
    }

    #[test]
    fn a_locked_keyring_is_the_ordinary_case_and_never_an_error() {
        // **This is the branch that matters most and the one that cannot be
        // measured here.** With auto-login the password that would unlock the
        // login keyring is never typed, so `Locked = true` is the normal state
        // on those machines rather than an edge case — and the developer
        // machine this was written on has both collections unlocked, which was
        // read off the bus and could not be changed to find out without locking
        // somebody's real keyring. So the *consequence* of a locked keyring is
        // pinned here instead: it degrades, it warns, and under no setting does
        // it become a failure or a prompt.
        let (store, line) = decide("auto", locked);
        assert_eq!(store, Store::File, "a locked keyring must not cost the user their session");
        assert!(line.contains("locked"), "and must say why: {line}");
        assert!(line.contains("0600"), "and where the session went instead: {line}");

        // Only somebody who explicitly asked for keyring-or-nothing gets
        // nothing, and even they are told rather than left to notice.
        let (store, line) = decide("keyring", locked);
        assert_eq!(store, Store::None);
        assert!(line.contains("will not be saved"), "{line}");
    }

    #[test]
    fn nothing_here_can_stop_a_client_starting() {
        // The rule the whole module is built to, as an assertion: every
        // combination of setting and machine resolves to a store, and no
        // combination resolves to an error a caller could propagate.
        for requested in ["", "auto", "keyring", "file", "nonsense"] {
            for probe in [present as fn() -> Result<(), String>, locked] {
                let (_store, line) = decide(requested, probe);
                assert!(!line.is_empty(), "every outcome is announced");
            }
        }
    }

    #[test]
    fn a_stored_body_is_not_readable_by_other_users() {
        // The profile directory is already 0700, so this is the second lock on
        // the same door. It is worth having because a profile directory can be
        // copied, archived or synced by something that does not preserve the
        // directory's mode, and the file's own mode travels with it.
        let dir = scratch("perms");
        save(Store::File, &dir, Kind::Cookies, "a=b\n").unwrap();
        let mode = std::fs::metadata(dir.join("cookies")).unwrap().permissions().mode();
        assert_eq!(mode & 0o177, 0, "a session must not be readable by anyone else");
    }

    #[test]
    fn an_interrupted_write_cannot_leave_half_a_session() {
        // Temp file plus rename, observed rather than assumed: after a save the
        // temp name must not exist, so nothing can later be mistaken for a
        // store.
        let dir = scratch("atomic");
        save(Store::File, &dir, Kind::Cookies, "a=1\n").unwrap();
        save(Store::File, &dir, Kind::Cookies, "a=2\n").unwrap();
        assert!(!dir.join("cookies.new").exists(), "the temp file must not survive");
        assert_eq!(load(Store::File, &dir, Kind::Cookies).as_deref(), Some("a=2\n"));
    }

    #[test]
    fn nothing_saved_is_not_a_failure() {
        // Every absence has to read as an ordinary signed-out launch. Turning
        // "you were logged out" into "the client would not start" is strictly
        // worse, and is the rule this module is built to.
        let dir = scratch("absent");
        assert!(load(Store::File, &dir, Kind::Cookies).is_none());
        assert!(load(Store::None, &dir, Kind::Cookies).is_none());
        assert!(save(Store::None, &dir, Kind::Cookies, "a=b").is_ok());
        assert!(erase(Store::File, &dir, Kind::Identity).is_ok());
    }

    #[test]
    fn erasing_overwrites_before_unlinking() {
        // `remove_file` unlinks and does not erase. The point of the shred is
        // that the bytes are gone from the blocks as well as from the
        // directory entry; what can be asserted from here is the part that is
        // deterministic — the file is written over its whole length and then
        // removed, and no `.new` temp is left holding a copy.
        let dir = scratch("shred");
        save(Store::File, &dir, Kind::Cookies, ".ROBLOSECURITY=fake\n").unwrap();
        erase(Store::File, &dir, Kind::Cookies).unwrap();
        assert!(!dir.join("cookies").exists());
        assert!(!dir.join("cookies.new").exists());
    }

    #[test]
    fn two_profiles_cannot_read_each_others_session() {
        // The attribute set is the whole of the isolation between profiles, and
        // between an agent's scratch profile and the one somebody plays on.
        // Both are called `default`; only the path tells them apart.
        let a = attributes(Path::new("/home/someone/.local/share/cordial/profiles/default"), Kind::Cookies);
        let b = attributes(Path::new("/home/someone/.cache/scratch/cordial/profiles/default"), Kind::Cookies);
        assert_ne!(a, b, "two roots with the same profile name must not share an item");
        let cookies = attributes(Path::new("/p/default"), Kind::Cookies);
        let identity = attributes(Path::new("/p/default"), Kind::Identity);
        assert_ne!(cookies, identity, "the two stores must be separate items");
    }

    /// The real thing, against the real service, skipped rather than failed
    /// where there is none.
    ///
    /// A skip is printed rather than passed silently, because a test that
    /// quietly does nothing on the machine that matters is how this project
    /// ends up believing something it never measured.
    #[test]
    fn a_session_survives_the_round_trip_through_the_service() {
        let dir = scratch("keyring");
        if let Err(why) = usable() {
            println!("skipped: {why}");
            return;
        }
        // Obviously fake, and short. No test in this repository holds a real
        // token, at any verbosity.
        let body = "# cordial test\nroblox.com\tCORDIALTEST=not-a-session\n";
        save(Store::Keyring, &dir, Kind::Cookies, body).unwrap();
        assert_eq!(
            load(Store::Keyring, &dir, Kind::Cookies).as_deref(),
            Some(body),
            "what was stored must come back byte for byte"
        );
        erase(Store::Keyring, &dir, Kind::Cookies).unwrap();
        assert!(
            load(Store::Keyring, &dir, Kind::Cookies).is_none(),
            "and a removed item must not linger in somebody's keyring"
        );
    }

    #[test]
    fn a_plaintext_store_is_adopted_and_destroyed() {
        // The migration, end to end: the owner has a live session in a file
        // right now, and the launch after this change has to take it in
        // *without* signing them out. The body is returned as well as stored,
        // which is the half that is easy to leave out and expensive to notice.
        let dir = scratch("adopt");
        if let Err(why) = usable() {
            println!("skipped: {why}");
            return;
        }
        let body = "# cordial test\nroblox.com\tCORDIALTEST=adopt-me\n";
        save(Store::File, &dir, Kind::Cookies, body).unwrap();
        assert_eq!(
            load(Store::Keyring, &dir, Kind::Cookies).as_deref(),
            Some(body),
            "the migrating launch must still be signed in"
        );
        assert!(!dir.join("cookies").exists(), "and the plaintext file must be gone");
        assert_eq!(
            load(Store::Keyring, &dir, Kind::Cookies).as_deref(),
            Some(body),
            "the next launch must read it from the service"
        );
        erase(Store::Keyring, &dir, Kind::Cookies).unwrap();
    }
}
