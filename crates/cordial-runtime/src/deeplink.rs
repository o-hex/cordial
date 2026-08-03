//! `roblox-player://` and `roblox://` — a URL from a browser click, into the
//! engine.
//!
//! The shell registers Cordial as the handler for those two schemes and hands
//! the client whatever the browser passed, as `cordial-run --join-url <url>`.
//! Everything after that argument is here.
//!
//! **What the engine asks for: nothing.** This was the first question, because
//! a protocol handler that accepts a click and drops it is worse than none at
//! all. The engine never asks for an `Intent`, a `Uri` or a URL — Roblox's own
//! Java receives it on Android, and the URL crosses into `libroblox.so` only
//! because Java *calls inward*. Cordial is the Java side here, so those inward
//! calls are the interface. `docs/analysis/deep-links.md` records how that was
//! established; `native/deeplink.cpp` holds the calls themselves.
//!
//! **What works, measured.** Publishing `{"url": …}` on the engine's own
//! `Linking.detectURL` message makes the app shell answer with a `Game.launch`
//! naming the place from the link:
//!
//! ```text
//! [deeplink] (cold start) published Linking.detectURL
//! [deeplink] (app ready) Game.launch is:
//!     Some("{\"placeId\":1818,\"referralPage\":\"DeepLink\",\"joinAttemptId\":…}")
//! ```
//!
//! Twice, on two consecutive runs, with `roblox://experiences/start?placeId=1818`.
//! The control is `CORDIAL_DEEPLINK_NO_PUBLISH=1`: same link, same launch,
//! publish suppressed, and `Game.launch` stays empty with
//! `isColdStartDeeplinkToGame()` false at both sampling points.
//!
//! **What does not work, also measured.** A `roblox-player://` link produces no
//! `Game.launch` at all. The engine's own pattern for a game link —
//! `FStringGameLaunchLinkURL`, a client setting — admits `roblox://` and
//! `robloxmobile://` and nothing else. That is the scheme roblox.com's desktop
//! play button emits, so the handler Cordial is taking over from Sober will
//! receive links this engine does not understand. Cordial says so on the way
//! past rather than waiting for the silence to speak.
//!
//! **Not verified: an actual join.** Reaching `Game.launch` is the engine
//! asking to launch an experience; whether it then joins one needs a signed-in
//! account, and no account was used here. Every run above ends at
//! `app ready: Landing`, which is where a signed-out client belongs.
//!
//! **The URL is hostile input.** It arrives from a browser, which got it from a
//! page, which got it from anywhere. It is length-capped, checked for one of
//! the two schemes Cordial claims, and restricted to printable ASCII before
//! anything else sees it — the last of those specifically so a URL cannot carry
//! a newline into the log and forge a line. It is never interpolated into a
//! shell command, never used to build a filesystem path, and never used as a
//! format string. It goes to exactly one place: a `String` argument of a JNI
//! native.
//!
//! **Its contents are not printed.** A `roblox://` link is almost entirely
//! query, and a Roblox query can carry a private-server `accessCode` or
//! `linkCode` — a capability, not a preference. Cordial elides web-view URLs'
//! queries for exactly this reason (`native/android_classes.cpp`), so this
//! module reports the scheme, the parameter *names*, and the length, and never
//! a value.

use cordial_linker_sys as linker;

/// `com.roblox.universalapp.linking.JNIBaseUrlProtocol`.
const BASE_URL: &str = "com/roblox/universalapp/linking/JNIBaseUrlProtocol";
/// `com.roblox.universalapp.linking.JNIWebLoginProtocol`.
const WEB_LOGIN: &str = "com/roblox/universalapp/linking/JNIWebLoginProtocol";
/// `com.roblox.universalapp.linking.JNILinkingProtocol`.
const LINKING: &str = "com/roblox/universalapp/linking/JNILinkingProtocol";
/// `com.roblox.universalapp.messagebus.MessageBus`.
const BUS: &str = "com/roblox/universalapp/messagebus/MessageBus";

/// The engine's own names for the messages a URL travels on, read out of a
/// running engine by [`probe`] rather than guessed:
///
/// ```text
/// getProtocolName      -> "Linking"
/// getDetectURLId       -> "Linking.detectURL"
/// getPendingURLId      -> "Linking.pendingURL"
/// getHandleLuaURLId    -> "Linking.handleLuaURL"
/// getHandlePlatformURLId -> "Linking.handlePlatformURL"
/// getUrlKey            -> "url"
/// ```
///
/// `Linking.detectURL` is the one Cordial publishes on. Measured: with the app
/// shell up, publishing a game link on it produced a `Game.launch` message
/// synchronously, and the three siblings published straight afterwards produced
/// no further one. The engine's own `isColdStartDeeplinkToGame()` goes from
/// false to true across the same publish, and stays false when the publish is
/// suppressed and nothing else changes.
const DETECT_URL: &str = "Linking.detectURL";

/// `JNIExperienceProtocol.getLaunchId()` answers with this, read from a running
/// engine. It is the message the app shell publishes when it wants an
/// experience launched, so it is the observable that says a link was understood.
const GAME_LAUNCH: &str = "Game.launch";

/// The longest URL Cordial will carry.
///
/// A `roblox://` link's `launchData` is developer-supplied and can be long;
/// Roblox's own documented ceiling for it is 200 characters, and the rest of a
/// join link is short. 2048 is well clear of any real link and well under the
/// ~8 kB a browser would hand over, which is the point: the cap exists so that
/// a megabyte of query cannot be walked into a JNI `String` on the strength of
/// somebody else's web page.
const MAX_LEN: usize = 2048;

/// The two schemes Cordial claims as a handler.
///
/// Both, because Roblox's own site emits both: `roblox-player://` from the web
/// site's play button and `roblox://` from older links and from the mobile
/// deep-link surface. A handler that took only one would leave half the links
/// on the machine pointing at nothing.
const SCHEMES: [&str; 2] = ["roblox-player", "roblox"];

/// A URL that has passed [`validate`]. Construct it no other way.
#[derive(Clone, PartialEq, Eq)]
pub struct JoinUrl {
    raw: String,
    scheme: &'static str,
}

impl JoinUrl {
    /// The URL itself, for the one caller that hands it to the engine.
    ///
    /// Named rather than a `Deref`, and separate from `Display`, so that every
    /// place the raw value escapes is greppable — the same shape
    /// [`crate::cookies::Jar::expose`] uses for the same reason.
    pub fn expose(&self) -> &str {
        &self.raw
    }

    /// Which of the two schemes this arrived on.
    pub fn scheme(&self) -> &'static str {
        self.scheme
    }

    /// What is safe to print: the scheme, the parameter names, and the length.
    ///
    /// Never a parameter value. `accessCode` and `linkCode` are private-server
    /// capabilities and `launchData` is arbitrary developer payload; a log line
    /// that carried them would hand a shoulder-surfer or a pasted terminal
    /// transcript a working join link.
    pub fn describe(&self) -> String {
        let query = self
            .raw
            .split_once(':')
            .map(|(_, rest)| rest.trim_start_matches('/'))
            .unwrap_or_default();
        let mut names: Vec<&str> = query
            .split(['&', '?'])
            .filter_map(|p| p.split('=').next())
            .filter(|p| !p.is_empty())
            .collect();
        names.dedup();
        if names.is_empty() {
            return format!("{}:// ({} bytes, no parameters)", self.scheme, self.raw.len());
        }
        format!(
            "{}:// with {} ({} bytes; values not shown)",
            self.scheme,
            names.join(", "),
            self.raw.len()
        )
    }
}

impl std::fmt::Debug for JoinUrl {
    /// Reports the shape, never the value — so a stray `{:?}` in a future
    /// caller cannot leak a join link into a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JoinUrl({})", self.describe())
    }
}

/// Accept a URL, or say precisely why not.
///
/// Deliberately strict and deliberately dull. Every rejection is a string the
/// user sees, because the alternative — a handler that starts, says nothing and
/// lands on the home page — is the failure this whole path exists to avoid.
pub fn validate(raw: &str) -> Result<JoinUrl, String> {
    if raw.is_empty() {
        return Err("--join-url was given an empty string".into());
    }
    if raw.len() > MAX_LEN {
        return Err(format!(
            "--join-url is {} bytes and the limit is {MAX_LEN}",
            raw.len()
        ));
    }
    // Printable ASCII only. Rejecting control characters is what stops a URL
    // forging a log line, and rejecting spaces and non-ASCII bytes is what a
    // browser would already have percent-encoded — so anything else here did
    // not come from a browser.
    if let Some(bad) = raw.bytes().position(|b| !(0x21..=0x7e).contains(&b)) {
        return Err(format!(
            "--join-url has a character that is not printable ASCII at byte {bad}"
        ));
    }
    let (scheme, _) = raw
        .split_once(':')
        .ok_or_else(|| "--join-url has no scheme".to_string())?;
    // Schemes are case-insensitive (RFC 3986 §3.1), and a browser is entitled
    // to hand over `Roblox-Player://`.
    let lower = scheme.to_ascii_lowercase();
    let scheme = SCHEMES
        .iter()
        .find(|s| **s == lower)
        .ok_or_else(|| {
            format!(
                "--join-url has scheme {lower:?}; Cordial handles {}",
                SCHEMES.join(" and ")
            )
        })?;
    Ok(JoinUrl {
        raw: raw.to_string(),
        scheme,
    })
}

/// What was *done* with the link at cold start.
///
/// Deliberately not a verdict on whether the link worked. Nothing at this point
/// in a launch knows that: the app shell does not exist yet, and the answer
/// arrives at `APP_READY`, which is [`tick`]'s job to report. An enum that said
/// "not handled" here would be a lie with a thirty-second head start on the
/// truth, and that is exactly the shape of report this path exists to avoid.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A `maybeHandleColdStartProtocolLaunch` claimed the URL outright. Nothing
    /// further is published and nothing further is expected.
    Claimed(&'static str),
    /// On the engine's message bus. Whether the app shell acts on it is
    /// reported later, by [`tick`].
    Published,
    /// The build exports none of it — a Roblox update moved the natives, which
    /// is a different problem from a link nobody wanted.
    NoSurface,
}

/// Hand the URL to the engine.
///
/// Called during bring-up rather than after the app shell settles. That is
/// measured rather than assumed: publishing at cold start produces nothing
/// immediately and a `Game.launch` by the first `APP_READY`, so the bus holds
/// the message until there is something to act on it. Publishing again after
/// the shell is up produces a *second* `Game.launch` with a second
/// `joinAttemptId`, so it is done once, here.
///
/// Placed after `nativeAppBridgeV2InitWithParams` for the same reason the
/// cookie restore is: the protocol machinery it talks to does not exist until
/// that call has built it.
pub fn deliver(lib: linker::Library, url: &JoinUrl) -> Outcome {
    println!("[deeplink] delivering {}", url.describe());

    // Said up front rather than after thirty seconds of nothing happening.
    //
    // The engine carries its own pattern for what a game link looks like, as
    // the client setting `FStringGameLaunchLinkURL`, and that pattern admits
    // `roblox://` and `robloxmobile://` and no other scheme. Measured: the same
    // link published under `roblox-player://` produces no `Game.launch` at all,
    // where under `roblox://` it produces one naming the place. This is the
    // scheme roblox.com's own play button emits on desktop, so it is the case
    // that matters most and the one Cordial cannot yet act on.
    if url.scheme() == "roblox-player" {
        println!(
            "[deeplink] warning: this engine's own link pattern (FStringGameLaunchLinkURL) \
             matches roblox:// and robloxmobile:// only, so a roblox-player:// link is not \
             expected to reach an experience"
        );
    }

    if std::env::var_os("CORDIAL_DEEPLINK_PROBE").is_some() {
        probe(lib);
    }

    // `init(Context)` first on each class that has one. Whether it is required
    // before `maybeHandleColdStartProtocolLaunch` is not established; it is
    // driven because a native that needs setting up and did not get it is the
    // kind of silence this path cannot afford, and because a failure here is
    // printed rather than assumed away.
    let mut any_surface = false;
    for (class, tag) in [(BASE_URL, "JNIBaseUrlProtocol"), (WEB_LOGIN, "JNIWebLoginProtocol")] {
        let sym = format!("Java_{}_init", class.replace('/', "_"));
        if let Some(f) = lib.symbol(&sym) {
            match linker::game_activity::protocol_init(f, class) {
                Ok(()) => println!("[deeplink] {tag}.init ok"),
                Err(e) => println!("[deeplink] {tag}.init failed: {e}"),
            }
        }

        let sym = format!(
            "Java_{}_maybeHandleColdStartProtocolLaunch",
            class.replace('/', "_")
        );
        let Some(f) = lib.symbol(&sym) else {
            println!("[deeplink] {tag}.maybeHandleColdStartProtocolLaunch is not exported");
            continue;
        };
        any_surface = true;
        match linker::game_activity::cold_start_protocol_launch(f, class, url.expose()) {
            Ok(true) => {
                println!("[deeplink] {tag} took the link");
                return Outcome::Claimed(tag);
            }
            Ok(false) => println!("[deeplink] {tag} did not claim this link"),
            Err(e) => println!("[deeplink] {tag} failed: {e}"),
        }
    }

    // Neither claimed it, so it goes on the message bus, which is where the
    // engine's own deep-link handling lives. The engine holds the pattern it
    // matches game links against as a client setting — `FStringGameLaunchLinkURL`
    // accepts `roblox://` and `robloxmobile://`, with or without
    // `experiences/start?`, carrying `placeid`, `linkCode`, `accessCode`,
    // `launchData` and the rest — so the URL is parsed inside the engine and
    // Cordial does not have to understand it.
    if publish_url(lib, url, "cold start") {
        any_surface = true;
    }

    if !any_surface {
        return Outcome::NoSurface;
    }

    // Reading the result back is deferred, not the publish. The bus takes the
    // message at cold start and the app shell acts on it when it comes up, so
    // the answer does not exist yet at this point in the sequence; [`tick`]
    // reports it once `APP_READY` arrives.
    arm(lib, url);
    println!("[deeplink] handed to the engine; the app shell will act on it when it starts");
    Outcome::Published
}

/// The link, waiting for something to confirm the engine acted on it.
///
/// A `Mutex` rather than a channel because there is exactly one of these per
/// process and it is consumed once: the looper takes it, reports, and leaves
/// `None` behind, so the second and third `APP_READY` of an ordinary launch
/// (`PlatformAccountRouter`, `Startup`, `Landing` — all three fire) cannot
/// report the same link three times.
static ARMED: std::sync::Mutex<Option<(linker::Library, JoinUrl)>> = std::sync::Mutex::new(None);

/// Set by the engine's own `APP_READY`, read by the looper thread.
static APP_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The engine's thread calls this from inside its own notification callback, so
/// it does nothing but raise a flag. Every JNI call this module makes is made
/// from the looper thread, which is where [`tick`] runs.
extern "C" fn on_app_ready(_state: *const std::ffi::c_char) {
    APP_READY.store(true, std::sync::atomic::Ordering::Release);
}

fn arm(lib: linker::Library, url: &JoinUrl) {
    *ARMED.lock().expect("no other thread panics holding this") = Some((lib, url.clone()));
    linker::game_activity::app_ready_set_sink(Some(on_app_ready));
}

/// Called from the looper each pass. Does nothing at all until the engine has
/// reported `APP_READY` and a link is waiting.
///
/// This reports; it does not re-deliver. Publishing a second time produces a
/// second `Game.launch` with a second `joinAttemptId` — measured — and two join
/// attempts for one click is a worse failure than a slow one.
pub fn tick() {
    if !APP_READY.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let taken = ARMED.lock().expect("no other thread panics holding this").take();
    let Some((lib, _url)) = taken else { return };

    let launched = lib
        .symbol("Java_com_roblox_universalapp_messagebus_MessageBus_getLastRaw")
        .and_then(|f| read_last(f, GAME_LAUNCH));
    // The payload names a place and carries a join attempt id the engine
    // minted, so it is diagnostic output rather than something every launch
    // should print.
    if std::env::var_os("CORDIAL_DEEPLINK_PROBE").is_some() {
        println!("[deeplink] (app ready) {GAME_LAUNCH} is: {launched:?}");
    }
    match (launched, cold_start_flag(lib)) {
        (Some(_), _) => println!(
            "[deeplink] the app shell asked to launch an experience; the link reached the engine"
        ),
        (None, Some(true)) => println!(
            "[deeplink] the engine registered a deep link, but has not asked to launch an \
             experience"
        ),
        (None, _) => println!(
            "[deeplink] the app shell is up and nothing asked to launch an experience — this \
             link did not reach an experience. Signing in is required before a join can proceed"
        ),
    }
}

/// Put the URL on the engine's message bus.
///
/// Returns whether the bus was reachable at all — not whether the link worked.
/// The difference matters: a build that does not export `publishRaw` is a
/// Roblox update to chase, and a bus that took the message and did nothing is
/// a link nobody claimed.
///
/// `CORDIAL_DEEPLINK_NO_PUBLISH=1` suppresses the publish and is the control:
/// with it set, and everything else identical, `Game.launch` stays empty and
/// `isColdStartDeeplinkToGame()` stays false. That is what establishes that
/// this publish, and not something else in the launch, is what carries the link.
fn publish_url(lib: linker::Library, url: &JoinUrl, phase: &str) -> bool {
    let Some(publish) = lib.symbol("Java_com_roblox_universalapp_messagebus_MessageBus_publishRaw")
    else {
        println!("[deeplink] MessageBus.publishRaw is not exported by this build");
        return false;
    };
    let last = lib.symbol("Java_com_roblox_universalapp_messagebus_MessageBus_getLastRaw");

    // `getUrlKey()` answers `"url"`, read from the running engine rather than
    // guessed. Built by hand rather than with a JSON library because there is
    // exactly one field — but escaped, because [`validate`] admits every
    // printable ASCII byte including `"` and `\`, and this payload is built
    // from text somebody else's web page chose.
    let payload = format!("{{\"url\":\"{}\"}}", escape_json(url.expose()));

    let verbose = std::env::var_os("CORDIAL_DEEPLINK_PROBE").is_some();
    if verbose {
        let before = last.and_then(|f| read_last(f, GAME_LAUNCH));
        println!("[deeplink] ({phase}) {GAME_LAUNCH} before publishing: {before:?}");
        println!(
            "[deeplink] ({phase}) isColdStartDeeplinkToGame before publishing: {:?}",
            cold_start_flag(lib)
        );
    }

    if std::env::var_os("CORDIAL_DEEPLINK_NO_PUBLISH").is_some() {
        println!("[deeplink] ({phase}) not publishing (CORDIAL_DEEPLINK_NO_PUBLISH)");
        return true;
    }
    match linker::game_activity::call_static_strings(publish, BUS, &[DETECT_URL, &payload]) {
        Ok(()) => println!("[deeplink] ({phase}) published {DETECT_URL}"),
        Err(e) => {
            println!("[deeplink] ({phase}) publishing {DETECT_URL} failed: {e}");
            return false;
        }
    }
    if verbose {
        if let Some(f) = last {
            println!(
                "[deeplink] ({phase}) {GAME_LAUNCH} after publishing: {:?}",
                read_last(f, GAME_LAUNCH)
            );
        }
    }
    true
}

/// `MessageBus.getLastRaw(id)` — the last payload published on a message id,
/// or `None` when there has never been one.
fn read_last(f: *mut std::ffi::c_void, id: &str) -> Option<String> {
    match linker::game_activity::call_static_string_ret_string(f, BUS, id) {
        Ok(v) if v.is_empty() => None,
        Ok(v) => Some(v),
        Err(e) => {
            println!("[deeplink] getLastRaw({id}) failed: {e}");
            None
        }
    }
}

/// The two characters that would break out of a JSON string literal.
///
/// [`validate`] admits every printable ASCII byte, which includes `"` and `\`,
/// so a URL is entitled to carry both and the payload above is built from
/// attacker-influenced text. Escaping is cheaper than a JSON dependency and
/// exact for this one field, because nothing else in printable ASCII needs it.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `NativeGLInterface.isColdStartDeeplinkToGame()` — the engine's own answer to
/// "was this launch a deep link into an experience".
///
/// An eleven-byte tail call to an internal getter, so it reads engine state and
/// decides nothing. On Android it is what `ActivityNativeMain` consults between
/// initialising the app bridge and starting the Lua app shell, which places it
/// exactly where Cordial hands the URL over.
fn cold_start_flag(lib: linker::Library) -> Option<bool> {
    let f = lib.symbol("Java_com_roblox_engine_jni_NativeGLInterface_isColdStartDeeplinkToGame")?;
    linker::game_activity::call_static_bare_bool(f, "com/roblox/engine/jni/NativeGLInterface").ok()
}

/// Read the linking protocol's own vocabulary out of the running engine.
///
/// Every one of these is a zero-argument `String` getter on
/// `JNILinkingProtocol` — the message names and JSON field names the engine
/// uses on its own MessageBus. Reading them is how the protocol is learned from
/// a running engine rather than guessed at from symbol names, which is the
/// mistake this project has paid for nine times over (AGENTS.md).
///
/// `CORDIAL_DEEPLINK_PROBE=1`. Diagnostic only: it passes nothing in.
fn probe(lib: linker::Library) {
    const GETTERS: [&str; 18] = [
        "getProtocolName",
        "getOpenURLId",
        "getOpenURLRequestId",
        "getOpenURLResponseId",
        "getDetectURLId",
        "getPendingURLId",
        "getRegisterURLId",
        "getIsURLRegisteredId",
        "getIsURLRegisteredRequestId",
        "getIsURLRegisteredResponseId",
        "getHandleEngineURLId",
        "getHandleLuaURLId",
        "getHandlePlatformURLId",
        "getUrlKey",
        "getMatchedUrlKey",
        "getAttributionUrlKey",
        "getIsRegisteredKey",
        "getSuccessKey",
    ];
    let prefix = format!("Java_{}_", LINKING.replace('/', "_"));
    for name in GETTERS {
        match lib.symbol(&format!("{prefix}{name}")) {
            None => println!("[deeplink probe] {name}: not exported"),
            Some(f) => match linker::game_activity::call_static_ret_string(f, LINKING) {
                Ok(v) => println!("[deeplink probe] {name} -> {v:?}"),
                Err(e) => println!("[deeplink probe] {name} failed: {e}"),
            },
        }
    }
    match lib.symbol("Java_com_roblox_universalapp_experience_JNIExperienceProtocol_getLaunchId") {
        None => println!("[deeplink probe] JNIExperienceProtocol.getLaunchId: not exported"),
        Some(f) => match linker::game_activity::call_static_ret_string(
            f,
            "com/roblox/universalapp/experience/JNIExperienceProtocol",
        ) {
            Ok(v) => println!("[deeplink probe] JNIExperienceProtocol.getLaunchId -> {v:?}"),
            Err(e) => println!("[deeplink probe] JNIExperienceProtocol.getLaunchId failed: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link of the shape the web site emits, with a place that belongs to
    /// nobody. No real account, server or access code appears in this file.
    const SAMPLE: &str = "roblox-player://placeId=1818&launchData=hello";

    #[test]
    fn accepts_both_schemes() {
        assert_eq!(validate(SAMPLE).unwrap().scheme(), "roblox-player");
        assert_eq!(validate("roblox://placeId=1818").unwrap().scheme(), "roblox");
    }

    #[test]
    fn scheme_is_case_insensitive() {
        // RFC 3986 §3.1. A browser is entitled to hand over any case, and a
        // handler that only took the lowercase one would drop real links.
        assert_eq!(validate("ROBLOX-PLAYER://placeId=1").unwrap().scheme(), "roblox-player");
    }

    #[test]
    fn rejects_other_schemes() {
        for bad in ["http://roblox.com", "file:///etc/passwd", "javascript:alert(1)"] {
            assert!(validate(bad).is_err(), "{bad} should not be accepted");
        }
    }

    #[test]
    fn rejects_a_missing_scheme() {
        assert!(validate("placeId=1818").is_err());
    }

    /// The reason the character check exists. A URL carrying a newline could
    /// otherwise write its own line into Cordial's log, and the line above it
    /// would be indistinguishable from one Cordial wrote.
    #[test]
    fn rejects_control_characters() {
        assert!(validate("roblox://placeId=1\n[deeplink] joined").is_err());
        assert!(validate("roblox://place\0id=1").is_err());
        assert!(validate("roblox://placeId=1 2").is_err());
    }

    #[test]
    fn rejects_an_overlong_url() {
        let long = format!("roblox://launchData={}", "a".repeat(MAX_LEN));
        assert!(validate(&long).is_err());
    }

    #[test]
    fn rejects_an_empty_url() {
        assert!(validate("").is_err());
    }

    /// The privacy rule, as a test rather than a comment: a description names
    /// the parameters and never their values.
    #[test]
    fn describe_never_shows_a_value() {
        let u = validate("roblox-player://placeId=1818&accessCode=SECRETVALUE").unwrap();
        let d = u.describe();
        assert!(d.contains("placeId"), "{d}");
        assert!(d.contains("accessCode"), "{d}");
        assert!(!d.contains("SECRETVALUE"), "{d}");
        assert!(!d.contains("1818"), "{d}");
        assert!(!format!("{u:?}").contains("SECRETVALUE"));
    }

    #[test]
    fn expose_returns_the_url_unchanged() {
        assert_eq!(validate(SAMPLE).unwrap().expose(), SAMPLE);
    }

    /// The bus payload is one JSON field built from a URL somebody else's web
    /// page chose, and a quote is a printable ASCII character that [`validate`]
    /// admits. Without this, `roblox://a=","x":"` would add a field to a message
    /// going into the engine.
    #[test]
    fn a_quote_cannot_add_a_field_to_the_payload() {
        let url = validate(r#"roblox://placeId=1","evil":"yes"#).unwrap();
        let payload = format!("{{\"url\":\"{}\"}}", escape_json(url.expose()));
        assert!(!payload.contains(r#","evil":"yes""#), "{payload}");
        assert!(payload.contains(r#"\"evil\""#), "{payload}");
        assert_eq!(payload.matches("\":\"").count(), 1, "{payload}");
    }

    #[test]
    fn a_backslash_cannot_escape_the_closing_quote() {
        assert_eq!(escape_json(r"a\"), r"a\\");
    }
}
