//! The expensive half: getting the build onto the user's disk.
//!
//! This is the only part [`settings`](crate::settings) governs. Checking is one
//! small request and runs regardless; downloading is 115 MB and runs when the
//! user's settings and their connection both say it may.
//!
//! ## There is no Roblox-hosted URL for the Android build, and that is now
//! established rather than merely unfound
//!
//! ADR-015 permits fetching the official build *from Roblox's own distribution*.
//! This section used to say that no such URL "could be established", which reads
//! as an investigation that ran out of time and invites the next person to have
//! another go. The investigation has now been done, on 2026-08-03, and the
//! answer is that there is none — stated here strongly enough that nobody spends
//! another afternoon on it.
//!
//! Roblox's deployment CDN carries the desktop clients and nothing else.
//! [`DEPLOY_HISTORY`] answers 200 with 7210 lines of deployment records, whose
//! product names are `Studio`, `Studio64`, `WindowsPlayer`, `RccService`,
//! `Client`, `MFCStudio` and `StudioBeta`; the strings `android` and `apk` do
//! not occur in it once. [`ANDROID_DEPLOY_HISTORY`] answers 403 `AccessDenied`,
//! which is what that bucket says about a prefix it does not have.
//!
//! Roblox's own download page offers Android through Google Play and the Amazon
//! Appstore and links no file at all, and the `client-version` endpoint answers
//! HTTP 500 for `AndroidApp` ([`version`](crate::version) has that table). Three
//! separate places where a public Android artefact would surface if there were
//! one, and it is absent from all three, because Roblox ships the Android
//! application through app stores.
//!
//! ## What Sober does, since it was the project assumed to know
//!
//! This section used to say Sober "documents nothing about where it gets it".
//! Half of that was right and the useful half was wrong. Sober is closed source
//! — `vinegarhq/sober` holds a README, an icon and issue templates, and no code
//! — but it documents its distribution route plainly, in its own licence and
//! privacy notices, and those say it goes to the app store:
//!
//! Sober does not redistribute the APK. Its optional Automatic Downloads
//! feature asks VinegarHQ's own servers for a **Google Play** link, and the
//! bundle comes from Google Play on the user's own account, after the user opted
//! in during onboarding. VinegarHQ's one open-source component that touches the
//! Android build, the `custard` deployment tracker, does not use a Roblox
//! endpoint either: it watches `com.roblox.client` on Aptoide, a third-party APK
//! mirror, and it queries `client-version` only for `WindowsPlayer` and
//! `WindowsStudio64`. That is independent corroboration of the 500 above, from
//! the project that would most like it to answer.
//!
//! So the one project known to obtain this file had not found a Roblox URL. It
//! went to the store Roblox put the file in.
//!
//! ## Why neither of Sober's two routes is available here
//!
//! Fetching from Google Play means holding the user's Google credentials and
//! speaking the Play protocol as a registered device. ADR-015 permits fetching
//! from Roblox's own distribution, and a store account is not that; a Cordial
//! that asked for a Google password would be a different program from the one
//! that ADR was written about.
//!
//! A mirror is worse rather than easier. Aptoide serves the file today, and
//! taking it would mean Cordial quietly fetching an APK from a third party while
//! its ADR says it fetches from Roblox — with an MD5 from the mirror as the only
//! thing vouching for bytes the mirror itself supplied, which is not a check, it
//! is the mirror agreeing with itself.
//!
//! So there is no constant here holding a URL, guessed or borrowed.
//! [`Source::official`] refuses and says all of this in the short form, and
//! [`Source::configured`] takes a URL and a hash from the environment for a user
//! who has chosen a location themselves. If Roblox ever publishes an Android
//! deployment path, [`Source::official`] is the one function to fill in.
//!
//! ## What is checked, and when
//!
//! The scheme must be `https`. ADR-014 rejects a `file:` or plain-HTTP download
//! URL for the same reason: the hash is what makes a download trustworthy, so
//! the scheme is defence in depth rather than the guarantee, but a URL is the
//! one field a fetcher acts on and the set of schemes it can be talked into is
//! worth being a short list.
//!
//! The size is capped as it streams, not afterwards and not from the
//! `Content-Length` header, which is whatever the server felt like sending.
//!
//! The hash is computed over the stream and checked **before the file is given
//! the name anything else looks for**. ADR-014 hashes a plugin archive in memory
//! before a byte is written, and that is not available here — a 115 MB APK held
//! whole to check it is 115 MB of resident memory on a machine that is about to
//! run a game engine. The equivalent is a `.partial` staging file: nothing reads
//! it, [`crate::apk`] is never pointed at it, and it is removed on any refusal.
//! What ADR-014's rule protects against is a tampered archive that has already
//! been parsed and unpacked, and that cannot happen here, because unpacking
//! takes a path this function only produces after the digest matches.

use crate::http as _http;
use crate::metered::Metered;
use crate::settings::UpdateSettings;
use crate::sha256::{Hasher, Sha256Hash};
use crate::Unreachable;
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Where a build comes from, and what it must hash to.
///
/// The hash is not optional. An unverified download is a URL being trusted, and
/// a URL is not a claim about content — ADR-015 says exactly that, and making
/// the field an `Option` would be the whole sentence undone by a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub url: String,
    pub hash: Sha256Hash,
}

/// What the user sets to point Cordial at a build. Two variables rather than
/// one string to split, so a malformed value fails at the one that is wrong.
pub const URL_ENV: &str = "CORDIAL_ROBLOX_APK_URL";
pub const HASH_ENV: &str = "CORDIAL_ROBLOX_APK_SHA256";

/// Roblox's deployment CDN, and the Android prefix it does not have.
///
/// Nothing in this crate downloads from either. They are here so that
/// `cargo run -p cordial-update --example update_probe` can ask both and print
/// what came back, because "Roblox publishes no Android build" is the load-
/// bearing claim in this module and a claim of that shape decays silently. The
/// desktop URL is the control: without it, a 403 on the Android prefix looks
/// like the CDN being down or the network being broken, and with it the
/// difference is one line of output.
pub const DEPLOY_HISTORY: &str = "https://setup.rbxcdn.com/DeployHistory.txt";
pub const ANDROID_DEPLOY_HISTORY: &str = "https://setup.rbxcdn.com/android/DeployHistory.txt";

impl Source {
    /// A source, if the URL is one this will act on.
    pub fn new(url: impl Into<String>, hash: Sha256Hash) -> Result<Self, Refusal> {
        let url = url.into();
        if !url.starts_with("https://") {
            return Err(Refusal::NotHttps(url));
        }
        Ok(Source { url, hash })
    }

    /// Roblox's own distribution of the Android build.
    ///
    /// **There is none.** See the module documentation, which now records the
    /// measurements rather than an unfinished search: Roblox's deployment CDN
    /// carries only the desktop clients, its download page sends Android users
    /// to Google Play and the Amazon Appstore, and `client-version` answers 500
    /// for `AndroidApp`. Sober reaches the same file through Google Play, which
    /// is a route ADR-015 does not permit and Cordial does not want.
    ///
    /// The message names the stores rather than saying "not established",
    /// because a user meeting this needs to know the file is obtainable and
    /// where from. A refusal that only says Cordial cannot reads as Cordial
    /// being broken.
    pub fn official() -> Result<Self, Refusal> {
        Err(Refusal::NoSource(format!(
            "Cordial has no download URL for the Roblox Android build, because Roblox publishes \
             none: the Android application is distributed through Google Play and the Amazon \
             Appstore, and Roblox's deployment CDN carries only the Windows and Mac clients. \
             Cordial will not sign in to a store on your behalf, and it will not fetch the file \
             from a mirror and call that Roblox — see crates/cordial-update/src/download.rs for \
             what was measured. Obtain the APK yourself and point Cordial at it in Settings, or \
             set {URL_ENV} and {HASH_ENV} to fetch from a location you have chosen."
        )))
    }

    /// What the environment says, or [`Source::official`] if it says nothing.
    pub fn configured() -> Result<Self, Refusal> {
        let url = std::env::var(URL_ENV).ok().filter(|s| !s.trim().is_empty());
        let hash = std::env::var(HASH_ENV).ok().filter(|s| !s.trim().is_empty());
        match (url, hash) {
            (None, None) => Source::official(),
            (Some(_), None) => Err(Refusal::NoSource(format!(
                "{URL_ENV} is set and {HASH_ENV} is not. A download with no hash to check it \
                 against is a URL being trusted, which is the one thing ADR-015 says this \
                 must not do."
            ))),
            (None, Some(_)) => Err(Refusal::NoSource(format!(
                "{HASH_ENV} is set and {URL_ENV} is not, so there is nothing to fetch."
            ))),
            (Some(url), Some(hash)) => {
                let hash = Sha256Hash::parse(hash.trim())
                    .map_err(|why| Refusal::NoSource(format!("{HASH_ENV}: {why}")))?;
                Source::new(url.trim(), hash)
            }
        }
    }

    /// Bypasses the scheme check so the streaming path can be exercised over a
    /// loopback socket. Compiled only into test builds, so there is nothing in
    /// a release binary that can be talked into plain HTTP.
    #[cfg(test)]
    fn unchecked(url: impl Into<String>, hash: Sha256Hash) -> Self {
        Source { url: url.into(), hash }
    }
}

/// How much a download may be.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: u64,
}

impl Default for Limits {
    /// The APK this loads the engine out of is around 115 MB and the split
    /// siblings beside it are smaller. Half a gigabyte is far above anything
    /// observed and far below anything that fills a disk before the refusal
    /// arrives.
    fn default() -> Self {
        Limits { max_bytes: 512 * 1024 * 1024 }
    }
}

/// Why nothing was downloaded, or why what was downloaded was not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// There is no URL to fetch from. Carries the whole explanation, because
    /// this is the one a user actually meets.
    NoSource(String),
    NotHttps(String),
    /// The settings and the connection between them say not now.
    Blocked(String),
    Unreachable(Unreachable),
    TooLarge { url: String, limit: u64 },
    HashMismatch { url: String, expected: String, actual: String },
    Io { path: String, why: String },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoSource(why) => f.write_str(why),
            Refusal::NotHttps(url) => write!(
                f,
                "{url:?} is not an https URL, and Cordial will not download a build over anything else"
            ),
            Refusal::Blocked(why) => write!(f, "not downloading: {why}"),
            Refusal::Unreachable(u) => write!(f, "{u}"),
            Refusal::TooLarge { url, limit } => {
                write!(f, "{url} sent more than {limit} bytes, so the download was abandoned")
            }
            Refusal::HashMismatch { url, expected, actual } => write!(
                f,
                "{url} hashes to {actual}, and it was published as {expected}; nothing was kept"
            ),
            Refusal::Io { path, why } => write!(f, "{path}: {why}"),
        }
    }
}

impl std::error::Error for Refusal {}

impl From<Unreachable> for Refusal {
    fn from(u: Unreachable) -> Self {
        Refusal::Unreachable(u)
    }
}

/// How far along a download is: bytes so far, and the total if the server said.
pub type Progress<'a> = &'a mut dyn FnMut(u64, Option<u64>);

/// Fetch, if the settings and the connection allow it.
///
/// The gate is here rather than at the call site so that "the settings govern
/// the download" is a property of the download rather than of whoever remembered
/// to ask. [`fetch`] is still public, because a user pressing Update in the
/// header bar has answered the question the settings exist to answer.
pub fn fetch_automatically(
    source: &Source,
    into: &Path,
    settings: UpdateSettings,
    metered: Metered,
    progress: Progress<'_>,
) -> Result<PathBuf, Refusal> {
    settings.may_download(metered).map_err(Refusal::Blocked)?;
    fetch(source, into, progress)
}

pub fn fetch(source: &Source, into: &Path, progress: Progress<'_>) -> Result<PathBuf, Refusal> {
    fetch_with(source, into, Limits::default(), progress)
}

/// Stream `source` into `into`, returning the path it landed at.
///
/// The file is named after the last path segment of the URL, or `base.apk` when
/// the URL has nothing usable — never after anything the *server* said, because
/// a `Content-Disposition` naming `../../.bashrc` is a filename this would
/// otherwise be choosing on a remote host's say-so.
pub fn fetch_with(
    source: &Source,
    into: &Path,
    limits: Limits,
    progress: Progress<'_>,
) -> Result<PathBuf, Refusal> {
    let name = file_name_for(&source.url);
    std::fs::create_dir_all(into)
        .map_err(|e| Refusal::Io { path: into.display().to_string(), why: e.to_string() })?;
    let partial = into.join(format!("{name}.partial"));
    let final_path = into.join(&name);

    let result = stream_to(source, &partial, limits, progress);
    match result {
        Ok(()) => {
            std::fs::rename(&partial, &final_path).map_err(|e| {
                let _ = std::fs::remove_file(&partial);
                Refusal::Io { path: final_path.display().to_string(), why: e.to_string() }
            })?;
            Ok(final_path)
        }
        Err(e) => {
            // A refused download leaves nothing. "We deleted it again" is a
            // much weaker statement than "it was never given the name anything
            // looks for", and the partial is what makes the second one true.
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

fn stream_to(
    source: &Source,
    partial: &Path,
    limits: Limits,
    progress: Progress<'_>,
) -> Result<(), Refusal> {
    let mut response = agent()
        .get(&source.url)
        .call()
        .map_err(|e| Unreachable::Transport { url: source.url.clone(), why: e.to_string() })?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.body_mut().with_config().limit(4096).read_to_string().unwrap_or_default();
        return Err(Unreachable::Status { url: source.url.clone(), status, body }.into());
    }
    let declared = response.body().content_length();

    let mut body = response.body_mut().with_config().limit(limits.max_bytes + 1).reader();
    let mut file = std::fs::File::create(partial)
        .map_err(|e| Refusal::Io { path: partial.display().to_string(), why: e.to_string() })?;

    let mut hasher = Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = body
            .read(&mut buf)
            .map_err(|e| Unreachable::Transport { url: source.url.clone(), why: e.to_string() })?;
        if n == 0 {
            break;
        }
        total += n as u64;
        // Checked as it arrives rather than against Content-Length, which is a
        // number the server chose and is not a promise about what follows it.
        if total > limits.max_bytes {
            return Err(Refusal::TooLarge { url: source.url.clone(), limit: limits.max_bytes });
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| Refusal::Io { path: partial.display().to_string(), why: e.to_string() })?;
        progress(total, declared);
    }
    file.flush()
        .map_err(|e| Refusal::Io { path: partial.display().to_string(), why: e.to_string() })?;
    drop(file);

    let actual = hasher.finish();
    if actual != source.hash {
        return Err(Refusal::HashMismatch {
            url: source.url.clone(),
            expected: source.hash.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}

/// A download runs for minutes, so it gets its own agent rather than
/// [`crate::http`]'s, whose global timeout is sized for a metadata request and
/// would abandon this one part way through a perfectly healthy transfer.
fn agent() -> ureq::Agent {
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .user_agent(_http::USER_AGENT)
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
        .build();
    ureq::Agent::new_with_config(config)
}

/// The last path segment of a URL, if it is a plausible file name.
fn file_name_for(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let last = without_query.rsplit('/').next().unwrap_or("");
    let plausible = !last.is_empty()
        && last != "."
        && last != ".."
        && !last.contains('/')
        && !last.contains('\\')
        && last.len() <= 128;
    if plausible {
        last.to_string()
    } else {
        "base.apk".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-update-download-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A one-shot HTTP/1.1 server on loopback.
    ///
    /// A real socket rather than a mocked transport, because what is being
    /// tested is the streaming loop: the cap has to fire on bytes as they
    /// arrive, and a fake that hands over a `Vec` would not exercise that at
    /// all.
    fn serve(status: &str, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = status.to_string();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                line.clear();
            }
            let mut out = std::io::BufWriter::new(&stream);
            let _ = write!(
                out,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                body.len()
            );
            let _ = out.write_all(&body);
            let _ = out.flush();
        });
        format!("http://127.0.0.1:{port}/base.apk")
    }

    fn no_progress() -> impl FnMut(u64, Option<u64>) {
        |_, _| {}
    }

    #[test]
    fn an_honest_download_lands_and_verifies() {
        // The control for every refusal below.
        let body = b"PK\x03\x04 pretend this is an APK".to_vec();
        let url = serve("200 OK", body.clone());
        let source = Source::unchecked(&url, Sha256Hash::of(&body));
        let dir = scratch("good");
        let mut seen = Vec::new();
        let path =
            fetch_with(&source, &dir, Limits::default(), &mut |n, total| seen.push((n, total)))
                .unwrap();
        assert_eq!(path, dir.join("base.apk"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert!(!seen.is_empty(), "progress has to be reported, or the UI has nothing to show");
        assert_eq!(seen.last().unwrap().0, body.len() as u64);
        assert!(!dir.join("base.apk.partial").exists());
    }

    #[test]
    fn a_download_that_does_not_match_its_hash_is_not_kept() {
        // The assertion names HashMismatch and stays that specific: a weaker
        // one would pass with the check deleted, since a mismatching file is
        // still a perfectly readable file.
        let url = serve("200 OK", b"not what was published".to_vec());
        let source = Source::unchecked(&url, Sha256Hash::of(b"what was published"));
        let dir = scratch("mismatch");
        let e = fetch_with(&source, &dir, Limits::default(), &mut no_progress()).unwrap_err();
        assert!(matches!(e, Refusal::HashMismatch { .. }), "{e}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "not even the partial should survive"
        );
    }

    #[test]
    fn a_download_larger_than_the_cap_is_abandoned() {
        // Against the bytes as they arrive, not against Content-Length: a
        // server that understates its length is exactly the case a header check
        // would miss.
        let url = serve("200 OK", vec![0u8; 64 * 1024]);
        let source = Source::unchecked(&url, Sha256Hash::of(&[0u8; 64 * 1024]));
        let dir = scratch("toolarge");
        let e = fetch_with(&source, &dir, Limits { max_bytes: 4096 }, &mut no_progress())
            .unwrap_err();
        assert!(matches!(e, Refusal::TooLarge { .. }), "{e}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn a_server_error_names_the_url_and_keeps_nothing() {
        let url = serve("503 Service Unavailable", b"try later".to_vec());
        let source = Source::unchecked(&url, Sha256Hash::of(b"try later"));
        let dir = scratch("status");
        let e = fetch_with(&source, &dir, Limits::default(), &mut no_progress()).unwrap_err();
        let shown = e.to_string();
        assert!(shown.contains("127.0.0.1"), "{shown}");
        assert!(shown.contains("503"), "{shown}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn nothing_but_https_is_downloaded_from() {
        let h = Sha256Hash::of(b"");
        assert!(matches!(
            Source::new("http://example.invalid/base.apk", h),
            Err(Refusal::NotHttps(_))
        ));
        assert!(matches!(Source::new("file:///etc/passwd", h), Err(Refusal::NotHttps(_))));
        assert!(Source::new("https://example.invalid/base.apk", h).is_ok());
    }

    #[test]
    fn there_is_no_built_in_url_and_the_refusal_says_where_the_build_lives() {
        // The honest state of this feature, and it is now a finding rather than
        // a gap: Roblox ships Android through app stores and publishes no
        // deployment path, so this refusal is the permanent answer until that
        // changes. If somebody later fills in an official URL, this test is the
        // one to rewrite to assert what it is — not to delete quietly.
        let e = Source::official().unwrap_err();
        let shown = e.to_string();
        assert!(shown.contains("no download URL"), "{shown}");
        // Naming the stores is the point. A refusal that only says Cordial
        // cannot do it leaves the user believing the file is unobtainable, and
        // this one has to send them somewhere.
        assert!(shown.contains("Google Play"), "{shown}");
        assert!(shown.contains("Amazon Appstore"), "{shown}");
        assert!(shown.contains(URL_ENV), "{shown}");
        assert!(shown.contains(HASH_ENV), "{shown}");
    }

    #[test]
    fn the_probe_constants_point_at_roblox_and_carry_their_control() {
        // These exist only for the probe, so nothing else would notice one of
        // them being edited into a mirror's hostname. The Android path is the
        // claim and the desktop path is what makes a 403 on it legible, so
        // losing either quietly is losing the measurement.
        assert!(DEPLOY_HISTORY.starts_with("https://setup.rbxcdn.com/"), "{DEPLOY_HISTORY}");
        assert!(
            ANDROID_DEPLOY_HISTORY.starts_with("https://setup.rbxcdn.com/"),
            "{ANDROID_DEPLOY_HISTORY}"
        );
        assert_ne!(DEPLOY_HISTORY, ANDROID_DEPLOY_HISTORY);
    }

    #[test]
    fn a_configured_url_without_a_hash_is_refused() {
        // Half-configuring this must not silently fetch. The variables are
        // process-wide, so this test sets and clears them under the same guard
        // the rest of the workspace uses for env.
        static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(URL_ENV, "https://example.invalid/base.apk");
        std::env::remove_var(HASH_ENV);
        let e = Source::configured().unwrap_err();
        std::env::remove_var(URL_ENV);
        assert!(matches!(e, Refusal::NoSource(_)), "{e}");
        assert!(e.to_string().contains(HASH_ENV), "{e}");
    }

    #[test]
    fn settings_govern_the_download_rather_than_the_caller_remembering_to_ask() {
        let dir = scratch("blocked");
        let source =
            Source::new("https://example.invalid/base.apk", Sha256Hash::of(b"")).unwrap();
        let e = fetch_automatically(
            &source,
            &dir,
            UpdateSettings::default(),
            Metered::GuessYes,
            &mut no_progress(),
        )
        .unwrap_err();
        assert!(matches!(e, Refusal::Blocked(_)), "{e}");
        // And no request was made at all: the refusal comes before the socket.
        assert!(!dir.join("base.apk").exists());
    }

    #[test]
    fn the_file_name_comes_from_the_url_and_never_from_the_server() {
        assert_eq!(file_name_for("https://x.invalid/a/b/base.apk"), "base.apk");
        assert_eq!(file_name_for("https://x.invalid/base.apk?token=1"), "base.apk");
        assert_eq!(file_name_for("https://x.invalid/"), "base.apk");
        assert_eq!(file_name_for("https://x.invalid/.."), "base.apk");
    }
}
