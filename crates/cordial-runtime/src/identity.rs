//! Who is signed in, kept across a restart, because the cookie is not enough.
//!
//! **The bug this finishes.** [`crate::cookies`] made the session survive a
//! restart and the user was still on the landing page. That was measured, not
//! assumed: with a real signed-in store restored, the engine reports five
//! cookies held for each of four domains and the run still ends
//! `[roblox] app ready: Landing`.
//!
//! **Why.** Roblox asks Cordial who is signed in, through two Java mirrors that
//! were hardcoded to nobody — `NativeUserJavaInterface.getUserId` returning 0
//! with an empty `getUsername`, and `StartAppParams`' `appUserId`/`username`/
//! `isUnder13`/`membershipType` all zeroed. `PlatformAccountRouter` runs after
//! the cookie restore, asks the mirrors, is told user 0 with an empty name, and
//! routes to the landing page. It never gets as far as the network, which is
//! why the cookie being correct changed nothing.
//!
//! `docs/design/sign-in.md` §1.3 said these mirrors were presentation-layer
//! only and made redundant by the cookie. **That was wrong**, and §9 of that
//! document now says so; this module is the correction in code.
//!
//! **Where the identity comes from — the engine hands it over.** Its own
//! DataModel notification carries exactly the fields the mirrors want. From the
//! owner's own sign-in, in the engine's FastLog, values elided here as they are
//! everywhere else in this module:
//!
//! ```text
//! onDataModelNotification: Received type(DID_LOG_IN, 28), data({"username":…,
//!   "membershipType":0,"isUnder13":false,"hasRobloxSubscription":false,
//!   "countryCode":…,"userId":…,"displayName":…})
//! onDataModelNotification: Received type(APP_READY, 10), data(Home)
//! ```
//!
//! Cordial received that on `NativeGLJavaInterface.onDataModelNotificationCallback`,
//! printed it — username and user id and all — and dropped it. So this module
//! catches it, and `native/android_classes.cpp` stopped printing it.
//!
//! **The bootstrap is two launches, and that is inherent rather than a defect.**
//! `DID_LOG_IN` fires when a login happens, not when a session is restored: a
//! run with a restored cookie and no identity reaches `Landing` and emits no
//! notification at all, measured. So the launch that signs in is the launch that
//! captures the identity, and every launch after it restores one. Same shape as
//! the cookie, and for the same reason — the engine keeps neither on disk.
//!
//! **Privacy.** A username and a user id identify a real person, so nothing here
//! prints either at any verbosity. [`Identity`]'s `Debug` reports lengths;
//! reaching a value takes a named accessor, and the only callers are the write
//! to the store and the hand-off to the mirrors. The store itself is
//! [`crate::secrets`], the same one the cookie goes into: an item in
//! `org.freedesktop.secrets` keyed by profile path, falling back to the old
//! `0600` file with a warning where there is no secret service. It used to be
//! that file unconditionally, and ADR-012 records why that was wrong and whose
//! argument made it so.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::secrets::{self, Kind, Store};

/// What the store is called. No extension, for the same reason the cookie
/// store has none: it is not a document anybody should open.
const KIND: Kind = Kind::Identity;

/// Bumped if the field set ever stops being what the engine's own notification
/// carries. A store from a future version is refused rather than half-read,
/// because a half-read identity is a client claiming to be somebody it cannot
/// authenticate as.
const SCHEMA: u64 = 1;

/// The identity the mirrors answer from, once something has established one.
static CURRENT: Mutex<Option<Identity>> = Mutex::new(None);

/// Who is signed in, in the engine's own field names.
///
/// The names are `DID_LOG_IN`'s, deliberately: the store holds what the engine
/// said, so the same extraction reads a notification and a saved file and there
/// is only one place that can get a field name wrong.
#[derive(Clone, PartialEq)]
pub struct Identity {
    user_id: i64,
    username: String,
    display_name: String,
    membership_type: i64,
    is_under13: bool,
    has_subscription: bool,
    country_code: String,
}

/// Lengths, never values. The leak this guards against is a one-line diagnostic
/// somebody adds while debugging something else — the same reasoning as
/// `cookies::Jar`, and worth the same guard, because a username is a person.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Identity(id present, username {} bytes, display name {} bytes, membership {}, \
             under13 {}, subscription {})",
            self.username.len(),
            self.display_name.len(),
            self.membership_type,
            self.is_under13,
            self.has_subscription
        )
    }
}

impl Identity {
    pub fn user_id(&self) -> i64 {
        self.user_id
    }
    pub fn username(&self) -> &str {
        &self.username
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn membership_type(&self) -> i64 {
        self.membership_type
    }
    pub fn is_under13(&self) -> bool {
        self.is_under13
    }
    pub fn has_subscription(&self) -> bool {
        self.has_subscription
    }
    pub fn country_code(&self) -> &str {
        &self.country_code
    }
}

/// Whether the identity is restored and captured at all.
///
/// The control for every measurement in this area: same binary, same profile,
/// one thing different — shaped after `CORDIAL_SKIP_COOKIES` because a reader
/// who knows one should not have to learn a second convention. Deliberately
/// separate from it, so that "the cookie is restored and the router still says
/// Landing" stays reproducible on a signed-in profile.
pub fn enabled() -> bool {
    std::env::var_os("CORDIAL_SKIP_IDENTITY").is_none()
}

/// Where the store would live as a file, for the profile this instance runs.
///
/// Kept because a `Store::File` fallback still puts one there. Use
/// [`where_kept`] for anything a person reads.
pub fn path() -> PathBuf {
    crate::profile::active().join(KIND.name())
}

/// Where this instance is actually keeping the identity, in words.
pub fn where_kept() -> String {
    secrets::where_kept(secrets::active(), &crate::profile::active(), KIND)
}

/// Pull an identity out of the engine's field names.
///
/// Shared by the notification path and the on-disk path on purpose: the store
/// holds the same names, so a field renamed in one place fails in both rather
/// than in whichever was forgotten.
///
/// **A zero user id is refused.** Zero is exactly what the mirrors said before
/// this module existed, so storing it would persist "nobody is signed in" as
/// though it were an account and hand it back on every launch thereafter. The
/// same goes for an empty username: an identity that cannot name itself is not
/// one the app shell can render, and the honest state is no identity at all.
fn from_fields(v: &serde_json::Value) -> Option<Identity> {
    let user_id = v.get("userId")?.as_i64()?;
    let username = v.get("username")?.as_str()?.to_string();
    if user_id == 0 || username.is_empty() {
        return None;
    }
    Some(Identity {
        user_id,
        username,
        display_name: v
            .get("displayName")
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .to_string(),
        // Absent rather than zero if the engine ever stops sending it: these
        // three are what a signed-out client already claims, so defaulting to
        // them cannot invent a membership or a subscription.
        membership_type: v.get("membershipType").and_then(|m| m.as_i64()).unwrap_or(0),
        is_under13: v.get("isUnder13").and_then(|u| u.as_bool()).unwrap_or(false),
        has_subscription: v
            .get("hasRobloxSubscription")
            .and_then(|s| s.as_bool())
            .unwrap_or(false),
        country_code: v
            .get("countryCode")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// Parse a `DID_LOG_IN`-shaped notification payload.
pub fn parse(data: &str) -> Option<Identity> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    from_fields(&v)
}

/// Read the store back.
///
/// Missing, unreadable, malformed and future-schema all mean "nobody is signed
/// in", which is the honest answer and the one that presents as an ordinary
/// signed-out launch rather than as a failure to start. Refusing to launch over
/// a damaged identity file would turn "you were logged out" into "the client
/// will not open", which is strictly worse.
pub fn load(dir: &Path) -> Option<Identity> {
    load_from(secrets::active(), dir)
}

/// The same, against a named store rather than this instance's, so the tests
/// can exercise the parser without reaching into anybody's keyring.
fn load_from(store: Store, dir: &Path) -> Option<Identity> {
    let text = secrets::load(store, dir, KIND)?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    if v.get("schema").and_then(|s| s.as_u64()) != Some(SCHEMA) {
        return None;
    }
    from_fields(&v)
}

/// Write the store. See [`crate::secrets`] for where it ends up.
pub fn save(dir: &Path, id: &Identity) -> std::io::Result<()> {
    save_to(secrets::active(), dir, id)
}

fn save_to(store: Store, dir: &Path, id: &Identity) -> std::io::Result<()> {
    let body = serde_json::json!({
        "schema": SCHEMA,
        // Not decoration: this is a live account identity, and where it falls
        // back to a file it lands in a directory people copy around. The one
        // thing that stops it being pasted into an issue is somebody reading
        // this line first. It used to say "keep this file at 0600", which is
        // advice that only means anything in one of the two places it can be.
        "warning": "a live Roblox identity; treat it as personal data and keep it out of any bug report",
        "userId": id.user_id,
        "username": id.username,
        "displayName": id.display_name,
        "membershipType": id.membership_type,
        "isUnder13": id.is_under13,
        "hasRobloxSubscription": id.has_subscription,
        "countryCode": id.country_code,
    });
    secrets::save(store, dir, KIND, &format!("{body}\n"))
}

/// Remove the store.
///
/// Absent is success: there was nothing signed in, which is the state this is
/// trying to reach. [`crate::secrets::erase`] removes the item *and* any
/// plaintext file left over from a machine that once had no secret service, and
/// it overwrites that file before unlinking it — an identity nobody signed out
/// of is the failure `observe_logout` exists to prevent.
pub fn erase(dir: &Path) -> std::io::Result<()> {
    erase_from(secrets::active(), dir)
}

fn erase_from(store: Store, dir: &Path) -> std::io::Result<()> {
    secrets::erase(store, dir, KIND)
}

/// Push an identity into the mirrors `native/android_classes.cpp` answers from.
fn publish(id: &Identity) {
    cordial_linker_sys::game_activity::identity_publish(
        id.user_id,
        &id.username,
        &id.display_name,
        id.membership_type,
        id.is_under13,
        id.has_subscription,
    );
}

/// The sink `native/android_classes.cpp` calls with a `DID_LOG_IN` payload.
///
/// Runs on whichever thread the engine delivered the notification on, so it
/// does the parse and the write here rather than setting a flag for the looper:
/// unlike the cookie jar, none of this calls back into the engine, and a login
/// that is not written before the process dies is a login the user has to do
/// again.
pub extern "C" fn observe_login(data: *const std::ffi::c_char) {
    if data.is_null() || !enabled() {
        return;
    }
    // SAFETY: the C side passes a nul-terminated buffer that outlives the call.
    let data = unsafe { std::ffi::CStr::from_ptr(data) };
    let Ok(data) = data.to_str() else { return };

    let Some(id) = parse(data) else {
        // Said out loud, with a size rather than the payload. A notification
        // that arrived and did not parse is a different failure from one that
        // never arrived, and only one of them is Cordial's to fix.
        println!(
            "  [identity] a login notification of {} bytes did not carry a usable identity",
            data.len()
        );
        return;
    };

    {
        let Ok(mut current) = CURRENT.lock() else { return };
        *current = Some(id.clone());
    }
    publish(&id);

    // Written every time, including when it matches what was already restored.
    // Skipping the identical case was tried and is wrong twice over: the store
    // can have been removed or damaged since the launch that read it, and — the
    // reason it was noticed — "nothing was written because nothing changed" is
    // indistinguishable from "the notification never reached the sink", which
    // is the one thing this whole path needs to be able to demonstrate. A
    // handful of small writes per session buys that.
    let dir = crate::profile::active();
    match save(&dir, &id) {
        Ok(()) => println!(
            "  [identity] signed in; saved to {} (username {} bytes)",
            where_kept(),
            id.username().len()
        ),
        // Named at the moment it happens rather than left to be discovered next
        // launch: "you are still signed out tomorrow" is a surprise, and this
        // is the only place that knows why.
        Err(e) => eprintln!("[identity] the identity was not saved: {e}"),
    }
}

/// The sink called on `DID_LOG_OUT`.
///
/// **A stale identity that outlives a logout is worse than none.** It would
/// make the next launch present a signed-in shell for an account whose cookie
/// the server has already invalidated — a client that looks logged in and
/// cannot fetch anything, which is the hardest state to diagnose from a user's
/// description of it.
pub extern "C" fn observe_logout() {
    if !enabled() {
        return;
    }
    let had = CURRENT.lock().map(|mut c| c.take().is_some()).unwrap_or(false);
    cordial_linker_sys::game_activity::identity_clear();
    let dir = crate::profile::active();
    match erase(&dir) {
        Ok(()) if had => println!("  [identity] signed out; the saved identity is gone"),
        Ok(()) => {}
        Err(e) => eprintln!("[identity] could not remove the saved identity: {e}"),
    }
}

/// Hand a saved identity to the mirrors, and say whether there was one.
///
/// **Must run before `StartAppParams` is built**, which happens inside
/// `nativeAppBridgeV2StartAppWithParams` — those four fields are copied out of
/// the mirrors at construction and never asked for again, so an identity that
/// arrives afterwards reaches `NativeUserJavaInterface` and not the app-start
/// parameters, and the two then disagree about who is signed in.
///
/// Unlike the cookie restore this has no ordering constraint against the engine
/// at all, because it touches none of it: the mirrors live in Cordial's own
/// framework layer, not in `libroblox.so`. That is why it can run before the
/// library is even loaded, and it does.
pub fn restore() -> bool {
    if !enabled() {
        println!("  [identity] off (CORDIAL_SKIP_IDENTITY); the mirrors will report nobody");
        return false;
    }
    let dir = crate::profile::active();
    let Some(id) = load(&dir) else { return false };
    publish(&id);
    println!(
        "  [identity] restored a signed-in user from {} (username {} bytes)",
        where_kept(),
        id.username().len()
    );
    if let Ok(mut current) = CURRENT.lock() {
        *current = Some(id);
    }
    true
}

/// `NativeSettingsInterface.nativeSetUserId(String)` — the engine's own copy.
///
/// **A third place identity has to be threaded through, and the mirrors alone
/// are not enough without it.** With the mirrors filled in and this one not
/// called, `CORDIAL_TRACE_IDENTITY=1` shows the engine asking all six of
/// `NativeUserJavaInterface`'s methods four times each and being told a real
/// user, and the run still ends `app ready: Landing`. The mirrors are what
/// Cordial's Java side answers; this is what the engine keeps for itself.
///
/// A `String`, not a number — that is the exported signature, read out of the
/// dex in `docs/design/sign-in.md` §2.1, where it was recorded as never called
/// by anything in this repository.
///
/// Called from `load.rs` beside the cookie restore, for the reason recorded
/// there: the natives on this class do nothing until
/// `nativeAppBridgeV2InitWithParams` has run.
pub fn push_user_id(set_native: *mut std::ffi::c_void) -> bool {
    let Some(id) = CURRENT.lock().ok().and_then(|c| c.clone()) else {
        return false;
    };
    match cordial_linker_sys::game_activity::call_static_strings(
        set_native,
        "com/roblox/engine/jni/NativeSettingsInterface",
        &[&id.user_id.to_string()],
    ) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[identity] nativeSetUserId failed: {e}");
            false
        }
    }
}

/// Install the sinks the notification handler reports through.
///
/// Separate from anything that registers with the engine, so that the control
/// run — same binary, `CORDIAL_SKIP_IDENTITY=1` — differs in exactly whether
/// anything is listening, rather than in whether a class exists.
pub fn listen() {
    cordial_linker_sys::game_activity::identity_set_sinks(observe_login, observe_logout);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The name the file store uses, for the tests that look at the file.
    const FILE: &str = KIND.name();

    /// Pinned to the file store rather than run against whatever
    /// `secrets::active()` resolves to on this machine — a unit test that
    /// reached into a developer's real keyring would behave differently on the
    /// one machine where behaviour matters, and would leave rows in it. The
    /// keyring path is tested as itself, in `secrets.rs`.
    fn save(dir: &Path, id: &Identity) -> std::io::Result<()> {
        save_to(Store::File, dir, id)
    }

    fn load(dir: &Path) -> Option<Identity> {
        load_from(Store::File, dir)
    }

    fn erase(dir: &Path) -> std::io::Result<()> {
        erase_from(Store::File, dir)
    }

    /// A payload of the shape the engine sends, with values that belong to
    /// nobody. **No real account's fields appear in this file or any other**,
    /// which is why the id is a small number no live account would have.
    const SAMPLE: &str = r#"{"username":"testuser","membershipType":2,"isUnder13":false,
        "hasRobloxSubscription":true,"countryCode":"AU","userId":12345,"displayName":"Test User"}"#;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-identity-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_notification_the_engine_sends_yields_an_identity() {
        // The field names are the engine's, read off a real `DID_LOG_IN` line
        // in the engine's own FastLog. Getting one of them wrong is silent —
        // the parse returns None and the client is simply logged out again —
        // so the names are worth pinning down in a test.
        let id = parse(SAMPLE).expect("a real DID_LOG_IN payload must parse");
        assert_eq!(id.user_id(), 12345);
        assert_eq!(id.username(), "testuser");
        assert_eq!(id.display_name(), "Test User");
        assert_eq!(id.membership_type(), 2);
        assert!(!id.is_under13());
        assert!(id.has_subscription());
        assert_eq!(id.country_code(), "AU");
    }

    #[test]
    fn nobody_signed_in_is_not_an_identity() {
        // Zero is exactly what the mirrors answered before this module existed.
        // Persisting it would hand "nobody" back on every launch afterwards as
        // though it were an account, and the restore would report success.
        assert!(parse(r#"{"userId":0,"username":""}"#).is_none());
        assert!(parse(r#"{"userId":0,"username":"someone"}"#).is_none());
        assert!(parse(r#"{"userId":12345,"username":""}"#).is_none());
        assert!(parse("not json at all").is_none());
        assert!(parse("{}").is_none());
    }

    #[test]
    fn an_identity_survives_the_round_trip() {
        // The whole bug in one assertion: what was captured at sign-in is what
        // the next launch answers the mirrors with.
        let dir = scratch("roundtrip");
        let id = parse(SAMPLE).unwrap();
        save(&dir, &id).unwrap();
        let read = load(&dir).expect("a saved identity must come back");
        assert_eq!(read, id);
    }

    #[test]
    fn a_saved_identity_is_not_readable_by_other_users() {
        // A username and a user id identify a person, and the profile
        // directory can be copied or synced by something that does not
        // preserve its mode. The file's own mode travels with it.
        let dir = scratch("perms");
        save(&dir, &parse(SAMPLE).unwrap()).unwrap();
        let mode = std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o177, 0, "an identity must not be readable by anyone else");
    }

    #[test]
    fn no_identity_saved_reads_as_signed_out_rather_than_as_a_failure() {
        let dir = scratch("absent");
        assert!(load(&dir).is_none());
        std::fs::write(dir.join(FILE), "not a store at all").unwrap();
        assert!(load(&dir).is_none());
        // A file from a schema this build does not know is refused whole. Half
        // an identity is a client claiming to be somebody it cannot
        // authenticate as.
        std::fs::write(dir.join(FILE), r#"{"schema":99,"userId":1,"username":"x"}"#).unwrap();
        assert!(load(&dir).is_none());
    }

    #[test]
    fn a_logout_leaves_nothing_behind() {
        // The failure this prevents: a signed-in shell for an account whose
        // cookie the server has already invalidated.
        let dir = scratch("erase");
        save(&dir, &parse(SAMPLE).unwrap()).unwrap();
        erase(&dir).unwrap();
        assert!(load(&dir).is_none());
        assert!(!dir.join(FILE).exists());
        // And erasing what is not there is success, not an error to report.
        assert!(erase(&dir).is_ok());
    }

    #[test]
    fn an_interrupted_write_cannot_leave_half_an_identity() {
        // Temp file plus rename, observed rather than assumed — the same
        // property the cookie store needs, which is why both go through one
        // writer.
        let dir = scratch("atomic");
        save(&dir, &parse(SAMPLE).unwrap()).unwrap();
        assert!(!dir.join(format!("{FILE}.new")).exists(), "the temp file must not survive");
    }

    #[test]
    fn an_identity_does_not_print_its_owner() {
        // The guard that matters most, because this type is one `{:?}` away
        // from putting a real person's username in a log.
        let id = parse(SAMPLE).unwrap();
        let shown = format!("{id:?}");
        assert!(!shown.contains("testuser"), "Debug must not reveal a username: {shown}");
        assert!(!shown.contains("Test User"), "nor a display name: {shown}");
        assert!(!shown.contains("12345"), "nor a user id: {shown}");
        assert!(shown.contains("8 bytes"), "but it must still be useful: {shown}");
    }
}
