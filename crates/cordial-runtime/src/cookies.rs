//! The session, kept across a restart, because the engine will not keep it.
//!
//! **The bug.** Sign in, quit, restart against the same profile, and you are on
//! the landing page again. Reported twice, and reproduced on a single profile,
//! so it is not the `flock` in [`crate::profile`] handing out a different
//! directory on the second launch.
//!
//! **What was actually measured**, because the diagnosis is not the obvious one
//! and the obvious one wastes a day. A complete `CORDIAL_TRACE_PATHS=1`
//! inventory of every non-system file the engine opens contains no cookie jar
//! and no credential store, and `grep -rl ROBLOSECURITY` over a real profile
//! tree finds nothing. The engine holds its cookies in memory for the life of
//! the process. On Android the *Java* side persists them — the Waydroid capture
//! has `OnSetCookieHandlerImpl.b(): Updated WebViewCookieHandler with Cookies
//! from URL ...` and eight other cookie lines — and Cordial has no Java side.
//!
//! That rules out the fix everyone reaches for first. A shutdown hook cannot
//! flush a file that is never written. The graceful teardown descent in
//! `android::looper` exists and works, and alternating killed and graceful runs
//! over two passes produced no file created or updated at shutdown that a
//! killed run does not also produce. Teardown was never the missing piece; the
//! Java side was.
//!
//! So Cordial becomes the Java side, and with it the custodian of a session
//! token — which reverses a decision written down in
//! [ADR-012](../../../docs/adr/ADR-012-profiles-and-instances.md) and in
//! `profile.rs`, both of which said Cordial "never reads or handles" one. That
//! reasoning rested on the engine reading its cookie from a file, which the
//! measurement above disproves. The ADR records the reversal rather than
//! quietly contradicting it.
//!
//! **The cookie is necessary and is not sufficient**, which cost a second round
//! of this. With the store restored and the engine confirmed holding five
//! cookies for each of four domains, the client still reached `app ready:
//! Landing`, because `PlatformAccountRouter` asks who is signed in rather than
//! what cookies are held. [`crate::identity`] is the other half, and neither
//! half alone signs anybody in.
//!
//! **Where it goes, and why not the keyring.** The profile directory, which is
//! already `0700` and already holds everything else about that account, with
//! the file itself at `0600`. A keyring adds an unlock prompt to every launch
//! and protects against nothing extra here, because the token has to be handed
//! to the engine in plaintext on every start regardless — so the window it
//! would encrypt is the window in which the file is not being read anyway.
//!
//! **Two things about the engine's contract that were measured here and are
//! not what the design document assumed**, both of which produced a working
//! call and a silently empty jar before they were found:
//!
//! The cookie natives do nothing until the app bridge exists. Called before
//! `nativeAppBridgeSetInitParams`, where `docs/design/sign-in.md` §5.2 said to
//! put them, `nativeSetMultipleCookies` returns cleanly and stores nothing.
//! `CORDIAL_COOKIE_PROBE=1` sets a marker and reads it straight back at four
//! points in the startup sequence: 0 bytes at startup, 0 after init params, 51
//! from `nativeAppBridgeV2InitWithParams` onwards.
//!
//! The getter and the setter do not speak the same language.
//! `nativeGetCookiesForDomain` returns Netscape `cookies.txt` records joined by
//! `"; "`; `nativeSetMultipleCookies` wants `name=value` pairs. Handing the
//! getter's output straight back writes 51 bytes and leaves the engine holding
//! zero. [`to_settable`] is the conversion, and the store holds the converted
//! form so a restore is a straight hand-over.
//!
//! **Nothing in this module logs a cookie value.** The type carrying one is
//! [`Jar`], whose `Debug` prints a length; getting at the bytes takes a
//! deliberate `expose()`, and the places that do it are the write to disk, the
//! hand-off to the engine, and the separator counting in [`shape`]. Every
//! diagnostic here reports a count or a size.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

pub use cordial_linker_sys::game_activity::Jar;

/// The file inside the profile. No extension: it is not a document anybody
/// should open, and `.txt` invites exactly that.
const FILE: &str = "cookies";

const HEADER: &str = "# cordial cookie store v1 -- a live Roblox session. Keep this file at 0600.";

/// Domains asked for even before anything has been observed.
///
/// The first launch after a sign-in has an engine full of cookies and an empty
/// set of observed hosts, because the handler only fires on *changes*. Seeding
/// the hosts the capture shows Roblox actually setting cookies on means that
/// launch still saves something.
const SEED_HOSTS: [&str; 4] = ["roblox.com", ".roblox.com", "apis.roblox.com", "auth.roblox.com"];

const SETTINGS: &str = "com/roblox/engine/jni/NativeSettingsInterface";

/// `nativeGetCookiesForDomain`, as an address, once `load.rs` has resolved it.
///
/// An `AtomicUsize` rather than a `*mut c_void` because the pull happens on the
/// looper thread while the engine's callback that marks it needed arrives on
/// the engine's own HTTP thread, and a raw pointer is not `Send`.
static PULL: AtomicUsize = AtomicUsize::new(0);

/// `nativeSetMultipleCookies`, kept for the same reason as `PULL`. Needed after
/// startup as well as during it, because the probe that establishes whether the
/// two natives round-trip has to be able to run late, once the engine is fully
/// up, to tell a timing problem from a format one.
static PUSH: AtomicUsize = AtomicUsize::new(0);

/// Hosts the engine has told us its jar changed for.
static OBSERVED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Set by the engine's callback, cleared by the flush. The callback itself does
/// no work beyond this: it runs on the engine's HTTP thread, inside the
/// engine's own `Set-Cookie` handling, and calling back into the engine to read
/// the jar from there would be re-entering it on its own thread.
static DIRTY: AtomicBool = AtomicBool::new(false);

/// Whether persistence is on at all. The control for every measurement in this
/// area: same binary, same profile, one thing different.
pub fn enabled() -> bool {
    std::env::var_os("CORDIAL_SKIP_COOKIES").is_none()
}

/// Where the store lives for the profile this instance runs.
pub fn path() -> PathBuf {
    crate::profile::active().join(FILE)
}

/// The sink `native/cookies.cpp` calls with each host whose cookies changed.
///
/// Deliberately tiny, and deliberately not doing the read here — see `DIRTY`.
pub extern "C" fn observe_host(host: *const std::ffi::c_char) {
    if host.is_null() {
        return;
    }
    // SAFETY: the C side passes a nul-terminated buffer that outlives the call.
    let host = unsafe { std::ffi::CStr::from_ptr(host) };
    let Ok(host) = host.to_str() else { return };
    if host.is_empty() {
        return;
    }
    if let Ok(mut set) = OBSERVED.lock() {
        set.insert(host.to_string());
    }
    DIRTY.store(true, Ordering::Release);
}

/// Record the address of `nativeGetCookiesForDomain` so the flush can use it.
///
/// # Safety
///
/// `native` must be the exported `nativeGetCookiesForDomain`, which is a static
/// native taking one `String` and returning one. Calling anything else through
/// this signature is undefined.
pub unsafe fn set_pull(native: *mut std::ffi::c_void) {
    PULL.store(native as usize, Ordering::Release);
}

/// Record the address of `nativeSetMultipleCookies`.
///
/// # Safety
///
/// `native` must be the exported `nativeSetMultipleCookies`, a static native
/// taking two `String`s.
pub unsafe fn set_push(native: *mut std::ffi::c_void) {
    PUSH.store(native as usize, Ordering::Release);
}

/// Turn what the engine hands out into what the engine will take back.
///
/// **The two natives are not mirrors, and assuming they were cost a whole
/// round of this.** `nativeSetMultipleCookies` accepts `name=value` pairs — a
/// `Cookie:` header — and reading the jar back afterwards shows it took them.
/// Feeding it what `nativeGetCookiesForDomain` returns, unchanged, is accepted
/// just as quietly and leaves the jar holding **nothing**: 51 bytes written,
/// 0 bytes in the engine.
///
/// The shape of the getter's output, measured with `CORDIAL_COOKIE_PROBE=1` on
/// two cookies planted under known names and lengths, and never by printing a
/// jar. Three cookies come back as nineteen tab-separated fields with widths
/// `[21, 4, 1, 5, 1, 4, 27, 4, 1, 5, 1, 6, 29, 4, 1, 5, 1, 12, 1]`, which
/// resolves to Netscape `cookies.txt` records —
///
/// ```text
/// #HttpOnly_.roblox.com <TAB> TRUE <TAB> / <TAB> FALSE <TAB> 0 <TAB> NAME <TAB> VALUE
/// ```
///
/// — joined by `"; "` rather than by a newline. That accounts for every number:
/// the 21-wide first field is `#HttpOnly_.roblox.com`, the `4/1/5/1` run is
/// `TRUE`, `/`, `FALSE`, `0`, the 4-, 6- and 12-wide fields are the three
/// planted names, and the 27- and 29-wide fields are a value glued to the next
/// record's domain across the two-character separator (4+2+21 and 6+2+21).
///
/// So: split on the record separator, take the last two tab-separated fields of
/// each, and rebuild the header form. A record that does not have at least the
/// seven fields is skipped rather than guessed at.
fn to_settable(jar: &Jar) -> String {
    let mut pairs: Vec<String> = Vec::new();
    for record in jar.expose().split("; ") {
        let fields: Vec<&str> = record.split('\t').collect();
        if fields.len() < 7 {
            continue;
        }
        let name = fields[fields.len() - 2];
        let value = fields[fields.len() - 1];
        if name.is_empty() {
            continue;
        }
        pairs.push(format!("{name}={value}"));
    }
    pairs.join("; ")
}

/// Make a jar survive being one line of a tab-separated file.
///
/// **This is not decoration, and leaving it out silently loses the session.**
/// `nativeGetCookiesForDomain` does not return the `name=value; name=value`
/// shape a `Cookie:` header has. Measured with `CORDIAL_COOKIE_PROBE=1`, which
/// counts a jar's separators without printing any of it: two cookies come back
/// as 136 bytes containing twelve tabs, no newlines, and no `=` at all — six
/// tabs per cookie, which is the Netscape `cookies.txt` field layout (domain,
/// flag, path, secure, expiry, name, value) with the records run together
/// rather than newline-separated.
///
/// The first version of this store wrote `host<TAB>jar` and dropped any record
/// containing a tab, which meant it faithfully wrote a file containing nothing
/// but its own header and reported success. Escaping rather than rejecting is
/// the fix; the rejection is kept underneath it for anything still anomalous.
fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 8);
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            // An unknown escape is kept verbatim rather than dropped: losing a
            // byte in the middle of a token is the failure this whole module
            // is about.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Read the store back. Missing, unreadable and malformed all mean "no session
/// saved", which is the honest answer and the one that presents as a normal
/// signed-out launch rather than as a failure to start.
pub fn load(dir: &Path) -> Vec<(String, Jar)> {
    let Ok(text) = std::fs::read_to_string(dir.join(FILE)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stale = 0;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let Some((host, jar)) = line.split_once('\t') else { continue };
        let jar = unescape(jar);
        // Reject a record that is not in the form the engine will take back.
        //
        // This is not defensive programming, it is a bug that reached a real
        // account. Before `to_settable` existed the store held the engine's own
        // Netscape output, and `nativeSetMultipleCookies` accepts that and
        // discards it — so a session restored from such a file reported five
        // domains restored and left the engine holding nothing. It reproduces
        // exactly: plant a Netscape-form store and every domain says "wrote 6
        // cookie(s) but the engine holds 0".
        //
        // Detected by shape rather than by a version number in the header,
        // because the header did say v1 the whole time and the format changed
        // underneath it. A settable jar is `name=value` pairs: it has an `=`
        // and cannot contain a tab.
        if jar.contains('\t') || !jar.contains('=') {
            stale += 1;
            continue;
        }
        out.push((host.to_string(), Jar::from_stored(jar)));
    }
    if stale > 0 {
        println!(
            "  [cookies] discarded {stale} record(s) in an old on-disk format; \
             they would have been accepted by the engine and silently ignored. \
             Signing in again will write the store in the current format."
        );
    }
    out
}

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
/// **Shared with [`crate::identity`] rather than copied into it.** The identity
/// store beside this one holds a username and a user id for the same account,
/// and a second writer would be a second chance to get the mode or the rename
/// wrong — including later, when only one of the two gets a fix. Every property
/// this function has is a property that store needs too.
pub(crate) fn write_private(dir: &Path, name: &str, body: &str) -> std::io::Result<()> {
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

/// Write the store, at `0600`, atomically. See [`write_private`].
pub fn save(dir: &Path, records: &[(String, Jar)]) -> std::io::Result<()> {
    let mut body = String::new();
    writeln!(body, "{HEADER}").expect("writing to a String cannot fail");
    for (host, jar) in records {
        // The jar is escaped, because it genuinely does contain tabs — see
        // `escape`. The *host* is not, and a host containing a separator is
        // not something the engine produces, so it is refused rather than
        // encoded: a record whose key had to be mangled to fit is a record
        // that will not be found again.
        if host.contains(['\t', '\n']) {
            continue;
        }
        writeln!(body, "{host}\t{}", escape(jar.expose())).expect("writing to a String cannot fail");
    }
    write_private(dir, FILE, &body)
}

/// How often the jar is written out even if nothing has announced a change.
///
/// A timer rather than pure event-driven, because the event is not trusted. The
/// engine's `onSetCookie` callback is registered and *ought* to fire, but it has
/// never been seen to under Cordial — no response in a logged-out run carries a
/// `Set-Cookie`, and the capture's cookie traffic comes from requests Roblox's
/// Java code makes and Cordial does not. Depending on it would mean a session
/// established mid-session is written only at a clean exit, and lost entirely if
/// the client is killed. Thirty seconds costs one pull of a small string.
const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Pull and write when something has changed, and periodically regardless.
///
/// The hot path: called once per looper poll, so both cheap checks come before
/// anything that touches the engine or the disk.
pub fn flush_if_dirty() {
    if DIRTY.swap(false, Ordering::AcqRel) {
        flush("changed");
        return;
    }
    // `Mutex<Instant>` rather than an atomic: this runs on the looper thread
    // only, and the lock is uncontended.
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let due = {
        let Ok(mut last) = LAST.lock() else { return };
        let now = std::time::Instant::now();
        match *last {
            Some(t) if now.duration_since(t) < FLUSH_INTERVAL => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    };
    if due {
        flush("periodic");
    }
}

/// Pull the jar for every host worth asking about and write it out.
///
/// Unconditional, because the dirty bit is a *notification* and not the state.
/// The engine only calls back on a `Set-Cookie`, so a session established
/// before the handler was registered — or refreshed by a path that does not go
/// through that callback — would never set the bit, and waiting for it would
/// mean saving nothing on exactly the launch that has something to save. That
/// mistake is what the first run of this code actually did.
///
/// Called from the looper thread, including once during teardown while the
/// engine is still up: the jar lives in the engine, and after
/// `terminateNativeCode` there is nothing left to read it out of.
pub fn flush(reason: &str) {
    if !enabled() {
        return;
    }
    let native = PULL.load(Ordering::Acquire);
    if native == 0 {
        return;
    }

    // Run late as well as early. The startup probe says nothing round-trips
    // before the app bridge exists, and the only way to tell "too early" from
    // "wrong spelling" is to ask again once the engine is fully up.
    let push = PUSH.load(Ordering::Acquire);
    if push != 0 && std::env::var_os("CORDIAL_COOKIE_PROBE").is_some() {
        probe(push as *mut std::ffi::c_void, native as *mut std::ffi::c_void, reason);
    }

    let mut hosts: BTreeSet<String> = SEED_HOSTS.iter().map(|h| h.to_string()).collect();
    let observed = if let Ok(set) = OBSERVED.lock() {
        hosts.extend(set.iter().cloned());
        set.len()
    } else {
        0
    };

    let mut records: Vec<(String, Jar)> = Vec::new();
    for host in hosts {
        match cordial_linker_sys::game_activity::cookies_for_domain(
            native as *mut std::ffi::c_void,
            SETTINGS,
            &host,
        ) {
            // Converted on the way out, not on the way in. The store holds the
            // form the engine will accept back, so a restore is a straight
            // hand-over and the one place that has to understand the engine's
            // output format is this one.
            Ok(jar) if !jar.is_empty() => {
                let settable = to_settable(&jar);
                if !settable.is_empty() {
                    records.push((host, Jar::from_stored(settable)));
                }
            }
            Ok(_) => {}
            // Hosts and sizes are safe to name and are the only way anyone
            // diagnoses a jar that will not round-trip. The value never is.
            Err(e) => eprintln!("[cookies] {host}: {e}"),
        }
    }
    if records.is_empty() {
        // Said out loud rather than passed over in silence. "The engine had
        // nothing" and "Cordial never asked" are different failures, and the
        // first version of this printed neither, which cost a run to work out.
        // Not on the periodic tick, though: a signed-out session would repeat
        // it every thirty seconds for the life of the client.
        if reason != "periodic" {
            println!(
                "  [cookies] {reason}: the engine's jar is empty for all \
                 {} host(s) tried ({observed} observed); nothing saved",
                SEED_HOSTS.len() + observed
            );
        }
        return;
    }

    let dir = crate::profile::active();
    match save(&dir, &records) {
        Ok(()) => {
            let bytes: usize = records.iter().map(|(_, j)| j.len()).sum();
            println!(
                "  [cookies] {reason}: saved {} domain(s), {bytes} bytes to {}",
                records.len(),
                dir.join(FILE).display()
            );
        }
        Err(e) => eprintln!("[cookies] could not save to {}: {e}", dir.display()),
    }
}

/// A jar's punctuation, and never its content.
///
/// Which format the engine hands back decides how the store has to be written,
/// and the first version of this code assumed a single line of
/// `name=value; name=value` and silently dropped every record that was not.
/// Counting separators settles it without printing a byte of anybody's session:
/// tabs and newlines mean the Netscape `cookies.txt` shape, semicolons without
/// them mean a `Cookie:` header.
fn shape(jar: &Jar) -> String {
    let v = jar.expose();
    let count = |c: char| v.matches(c).count();
    let widths: Vec<usize> = v.split('\t').map(|f| f.len()).collect();
    format!(
        "{} newline(s), {} tab(s), {} semicolon(s), {} equals; {} field(s) widths {:?}",
        count('\n'),
        count('\t'),
        count(';'),
        count('='),
        widths.len(),
        widths
    )
}

/// Set a marker through `nativeSetMultipleCookies` and immediately read it back
/// through `nativeGetCookiesForDomain`, for a range of domain spellings.
///
/// This exists because the first attempt at this feature restored a cookie
/// without error and then read an empty jar back, and there was no way to tell
/// which of three things was true: the set was a silent no-op, the two natives
/// key on different spellings of a domain, or the engine's cookie subsystem was
/// not up yet at the point in the startup sequence the restore happens. Naming
/// the sizes each spelling round-trips answers all three at once.
///
/// The marker is a fixed, obviously-fake value, so this never handles a real
/// session, and only byte counts are ever printed.
pub fn probe(set_native: *mut std::ffi::c_void, get_native: *mut std::ffi::c_void, when: &str) {
    const MARKER: &str = "CORDIALPROBE=1";
    println!("  [cookies] probe at {when}: set the marker, then read it back");
    for domain in [
        "roblox.com",
        ".roblox.com",
        "www.roblox.com",
        "https://roblox.com",
        "https://www.roblox.com/",
    ] {
        let set = cordial_linker_sys::game_activity::call_static_strings(
            set_native,
            SETTINGS,
            &[domain, MARKER],
        );
        let got = cordial_linker_sys::game_activity::cookies_for_domain(
            get_native, SETTINGS, domain,
        );
        match (set, got) {
            (Ok(()), Ok(jar)) => println!(
                "    {domain:<26} set ok, read back {} bytes, {}",
                jar.len(),
                shape(&jar)
            ),
            (Ok(()), Err(e)) => println!("    {domain:<26} set ok, read failed: {e}"),
            (Err(e), _) => println!("    {domain:<26} set failed: {e}"),
        }
    }
}

/// Hand a saved session back to the engine.
///
/// Returns how many domains were restored, for the caller to report. Must run
/// before the app-bridge sequence starts: the engine begins hitting
/// `authenticated/*` endpoints as soon as that chain runs, and a cookie that
/// arrives after the first 401 is a cookie that arrived too late.
pub fn restore(set_native: *mut std::ffi::c_void) -> usize {
    if !enabled() {
        return 0;
    }
    let dir = crate::profile::active();
    let records = load(&dir);

    // Every host in the store is a host worth asking about at save time.
    //
    // Without this the save only ever pulls `SEED_HOSTS` plus whatever the
    // engine's callback happened to report, so a domain that reached the store
    // once — `friends.roblox.com`, say — is restored, never re-pulled, and
    // dropped from the file at the next save. Observed doing exactly that: five
    // domains in, four out, on every launch. A session decaying by one domain
    // at a time is the kind of fault that gets blamed on Roblox.
    if let Ok(mut set) = OBSERVED.lock() {
        for (host, _) in &records {
            set.insert(host.clone());
        }
    }

    let mut restored = 0;
    for (host, jar) in &records {
        match cordial_linker_sys::game_activity::call_static_strings(
            set_native,
            SETTINGS,
            &[host.as_str(), jar.expose()],
        ) {
            Ok(()) => restored += 1,
            Err(e) => eprintln!("[cookies] {host}: nativeSetMultipleCookies failed: {e}"),
        }
    }

    // Read it straight back, because `nativeSetMultipleCookies` returns `void`
    // and returned cleanly for the entire time it was being called too early to
    // do anything at all. "The call succeeded" is not evidence the engine took
    // the cookie; the only evidence is the jar afterwards. Sizes only — this
    // runs on every launch that has a session, and a session must never reach a
    // log.
    let pull = PULL.load(Ordering::Acquire);
    if restored > 0 && pull != 0 {
        for (host, jar) in &records {
            // Cookies counted rather than bytes compared: the jar comes back in
            // the engine's own Netscape-ish shape, which is longer than the
            // header form that went in, so a byte comparison would say "bigger,
            // fine" for a jar that had actually thrown the session away.
            let wanted = jar.expose().split("; ").filter(|p| !p.is_empty()).count();
            match cordial_linker_sys::game_activity::cookies_for_domain(
                pull as *mut std::ffi::c_void,
                SETTINGS,
                host,
            ) {
                Ok(back) => {
                    let got = to_settable(&back).split("; ").filter(|p| !p.is_empty()).count();
                    if got >= wanted {
                        println!("  [cookies] {host}: engine holds {got} cookie(s) after restore");
                    } else {
                        println!(
                            "  [cookies] {host}: wrote {wanted} cookie(s) but the engine holds \
                             {got}; the session did not take"
                        );
                    }
                }
                Err(e) => println!("  [cookies] {host}: could not read back: {e}"),
            }
        }
    }
    restored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-cookie-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_saved_session_is_not_readable_by_other_users() {
        // The profile directory is already 0700, so this is the second lock on
        // the same door. It is worth having because a profile directory can be
        // copied, archived or synced by something that does not preserve the
        // directory's mode, and the file's own mode travels with it.
        let dir = scratch("perms");
        save(&dir, &[("roblox.com".into(), Jar::from_stored("a=b".into()))]).unwrap();
        let mode = std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o177, 0, "a session must not be readable by anyone else");
    }

    #[test]
    fn a_session_survives_the_round_trip() {
        // The whole bug in one assertion: what was saved is what comes back.
        let dir = scratch("roundtrip");
        let written = vec![
            ("roblox.com".to_string(), Jar::from_stored(".ROBLOSECURITY=xxx; path=/".into())),
            (".roblox.com".to_string(), Jar::from_stored("GuestData=UserID=-1".into())),
        ];
        save(&dir, &written).unwrap();
        let read = load(&dir);
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].0, "roblox.com");
        assert_eq!(read[0].1.expose(), ".ROBLOSECURITY=xxx; path=/");
        assert_eq!(read[1].1.expose(), "GuestData=UserID=-1");
    }

    #[test]
    fn no_session_saved_reads_as_signed_out_rather_than_as_a_failure() {
        // A missing or corrupt store must present as an ordinary signed-out
        // launch. Returning an error here would turn "you were logged out" into
        // "the client would not start", which is strictly worse.
        let dir = scratch("absent");
        assert!(load(&dir).is_empty());
        std::fs::write(dir.join(FILE), "not a store at all\n\n#\n").unwrap();
        assert!(load(&dir).is_empty());
    }

    #[test]
    fn an_interrupted_write_cannot_leave_a_half_token() {
        // The temp file plus rename, observed rather than assumed: after a
        // save, the temp name must not exist, so nothing can later be mistaken
        // for a store. A reader sees the whole old file or the whole new one.
        let dir = scratch("atomic");
        save(&dir, &[("roblox.com".into(), Jar::from_stored("a=1".into()))]).unwrap();
        save(&dir, &[("roblox.com".into(), Jar::from_stored("a=2".into()))]).unwrap();
        assert!(!dir.join(format!("{FILE}.new")).exists(), "the temp file must not survive");
        assert_eq!(load(&dir)[0].1.expose(), "a=2", "the second save must win whole");
    }

    #[test]
    fn one_record_per_line_whatever_the_jar_contains() {
        // This test used to assert that a tab-separated Netscape jar survived
        // the file, which was right when the store held the engine's raw output
        // and became wrong the moment `to_settable` landed — the store holds the
        // settable form now, and `load` rejects anything else on purpose. Kept,
        // rewritten, because the property it was really guarding still matters:
        // whatever a value contains, the writer must not turn one record into
        // two.
        let dir = scratch("layout");
        let jar = ".ROBLOSECURITY=a-value-with-a-\\-backslash; other=1";
        save(&dir, &[("roblox.com".into(), Jar::from_stored(jar.into()))]).unwrap();

        let raw = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(raw.lines().count(), 2, "header plus exactly one record: {raw:?}");

        let read = load(&dir);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].1.expose(), jar, "and it must come back byte for byte");
    }

    #[test]
    fn escaping_round_trips_every_separator_it_has_to() {
        // Including a literal backslash, which is what makes the escape
        // reversible rather than merely usually reversible.
        for original in [
            "a\tb",
            "a\nb",
            "a\\b",
            "a\\tb",
            "trailing\\",
            ".roblox.com\tTRUE\t/\tTRUE\t0\tname\tvalue",
        ] {
            assert_eq!(unescape(&escape(original)), original, "on {original:?}");
        }
    }

    #[test]
    fn a_store_in_the_pre_conversion_format_is_discarded_not_replayed() {
        // This one reached a real account. Before `to_settable` existed the
        // store held the engine's own Netscape output; `nativeSetMultipleCookies`
        // accepts that and discards it, so the restore reported five domains
        // restored and left the engine holding nothing. Feeding it to the engine
        // is strictly worse than treating the file as absent, because the user
        // is signed out either way and only one of the two says why.
        let dir = scratch("stale-format");
        let netscape = "#HttpOnly_.roblox.com\tTRUE\t/\tFALSE\t0\t.ROBLOSECURITY\tvalue";
        save(
            &dir,
            &[
                ("roblox.com".into(), Jar::from_stored(netscape.into())),
                ("apis.roblox.com".into(), Jar::from_stored("a=1; b=2".into())),
            ],
        )
        .unwrap();
        let read = load(&dir);
        assert_eq!(read.len(), 1, "the Netscape-form record must not be replayed");
        assert_eq!(read[0].0, "apis.roblox.com", "and the settable one must survive");
    }

    #[test]
    fn a_host_that_would_split_a_record_is_refused() {
        // The jar is escaped because it really does contain tabs; the host is
        // not, because a host holding a separator is not something the engine
        // produces and a mangled key is a record nobody finds again.
        let dir = scratch("bad-host");
        save(
            &dir,
            &[
                ("roblox.com".into(), Jar::from_stored("good=1".into())),
                ("evil\tcom".into(), Jar::from_stored("a=1".into())),
            ],
        )
        .unwrap();
        let read = load(&dir);
        assert_eq!(read.len(), 1, "only the well-formed record survives");
        assert_eq!(read[0].0, "roblox.com");
    }

    #[test]
    fn a_jar_does_not_print_its_contents() {
        // The guard that matters most, because the leak this prevents is a
        // one-line diagnostic somebody adds while debugging something else.
        let jar = Jar::from_stored(".ROBLOSECURITY=super-secret-value".into());
        let shown = format!("{jar:?}");
        assert!(!shown.contains("secret"), "Debug must not reveal a session");
        assert!(shown.contains("33 bytes"), "but it must still be useful: {shown}");
    }
}
