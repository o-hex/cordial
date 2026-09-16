use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{
    CALL_TIMEOUT, CONTENT_TYPE, ENCODED_PREFIX, IFACE_COLLECTION, IFACE_ITEM, IFACE_SERVICE,
    SERVICE, SERVICE_PATH,
};

/// Set the first time a call times out, and never cleared.
///
/// After a timeout the worker is still inside the call that timed out, so every
/// later request queues behind a thread that is not coming back. Asking again
/// would spend five seconds per flush for the rest of the session and change
/// nothing.
static WEDGED: AtomicBool = AtomicBool::new(false);

pub(super) fn read_keyring(attrs: &HashMap<String, String>) -> Answer {
    match ask(Ask::Read(attrs.clone()), CALL_TIMEOUT)? {
        Some(stored) if stored.starts_with(ENCODED_PREFIX) => decode_keyring(&stored)
            .map(Some)
            .ok_or_else(|| "the stored session has an invalid encoding".to_string()),
        // Older releases wrote the body directly. Keep those values readable
        // (identity is a single-line JSON value); cookie stores that were
        // truncated by the service will simply parse as empty and be replaced
        // after the next sign-in.
        Some(stored) => Ok(Some(stored)),
        None => Ok(None),
    }
}

pub(super) fn encode_keyring(body: &str) -> String {
    let mut encoded = String::with_capacity(ENCODED_PREFIX.len() + body.len() * 2);
    encoded.push_str(ENCODED_PREFIX);
    for byte in body.as_bytes() {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_keyring(stored: &str) -> Option<String> {
    let hex = stored.strip_prefix(ENCODED_PREFIX)?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let mut chars = hex.bytes();
    while let (Some(hi), Some(lo)) = (chars.next(), chars.next()) {
        let nibble = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        };
        bytes.push((nibble(hi)? << 4) | nibble(lo)?);
    }
    String::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------------------
// The service itself, on a thread of its own.
// ---------------------------------------------------------------------------

pub(super) enum Ask {
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
type Request = (Ask, SyncSender<Answer>);
type Worker = Mutex<Sender<Request>>;

/// Every D-Bus call in this module happens on one thread that nothing waits on
/// for longer than it is prepared to.
///
/// The alternative was calling `zbus` from the looper thread and from the
/// engine's notification thread directly. `zbus`'s blocking API has no
/// per-call timeout, so that arrangement makes a wedged keyring daemon into a
/// wedged client, and the symptom — the window stops drawing thirty seconds
/// after sign-in — has no visible relationship to its cause.
fn worker() -> &'static Worker {
    static HANDLE: OnceLock<Worker> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Request>();
        // Unbounded on the request side on purpose. A rendezvous channel would
        // make `send` itself block on a worker that is stuck, which is the
        // deadlock this whole arrangement exists to avoid.
        let spawned = std::thread::Builder::new()
            .name("cordial-secrets".to_string())
            .spawn(move || serve(rx));
        if let Err(e) = &spawned {
            println!(
                "  [secrets] no thread for the secret service ({e}); sessions will not be saved"
            );
        }
        Mutex::new(tx)
    })
}

pub(super) fn ask(request: Ask, timeout: Duration) -> Answer {
    if WEDGED.load(Ordering::Acquire) {
        return Err("the secret service stopped answering earlier this session".to_string());
    }
    let (tx, rx) = sync_channel::<Answer>(1);
    let sent = worker()
        .lock()
        .map_err(|_| "the secret service worker panicked".to_string())
        .and_then(|h| {
            h.send((request, tx))
                .map_err(|_| "the secret service worker is gone".to_string())
        });
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

fn serve(rx: Receiver<Request>) {
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

        Ok(Keyring {
            conn,
            service,
            session,
            collection,
        })
    }

    fn proxy(
        &self,
        path: &OwnedObjectPath,
        interface: &'static str,
    ) -> Result<Proxy<'static>, String> {
        Proxy::new_owned(
            self.conn.clone(),
            SERVICE,
            path.clone().into_inner(),
            interface,
        )
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
            Err(e) => Err(format!(
                "the keyring would not say whether it is locked ({e})"
            )),
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

    fn write(
        &self,
        attrs: &HashMap<String, String>,
        label: &str,
        body: String,
    ) -> Result<(), String> {
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
