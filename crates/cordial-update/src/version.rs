//! Asking Roblox what the current build is.
//!
//! One GET against `clientsettingscdn.roblox.com/v2/client-version/<binaryType>`.
//! It costs nothing and Sober doing it every launch is not the waste it looks
//! like; the mistake would be doing it *synchronously*, where a slow or absent
//! network delays the window rather than the answer.
//!
//! ## What this endpoint actually answers, measured 2026-08-02
//!
//! | binaryType | answer |
//! |---|---|
//! | `WindowsPlayer` | 200 `{"version":"0.732.23.7321040","clientVersionUpload":"version-145f189a6a974303","bootstrapperVersion":""}` |
//! | `MacPlayer` | 200, same shape |
//! | `WindowsStudio64` | 200, same shape |
//! | `AndroidApp` | **500** `{"errors":[{"code":3,"message":"Error while fetching version information."}]}` |
//! | `iOSApp` | 500, the same error |
//! | `UWPApp` | 500, the same error |
//! | `AndroidPlayer`, `AndroidStudio` | 400 `{"errors":[{"code":2,"message":"Invalid binaryType."}]}` |
//!
//! Two things follow, and the first one contradicts what
//! `docs/design/updating-roblox.md` assumed when it was written.
//!
//! **`AndroidApp` is the right name and Roblox serves no version for it.** The
//! 400/500 split is the discriminator: an unrecognised name is refused as a bad
//! name, and `AndroidApp` is not refused — it is accepted and then fails to
//! produce a version, exactly as the other two store-distributed platforms do.
//! So this is not a name to go hunting for; it is an endpoint that does not
//! cover the platforms Roblox ships through an app store. `AndroidApp` is also
//! the name `client_settings` already established for the settings document on
//! the same host, by the same 400-versus-200 experiment.
//!
//! **The version check for the Android build therefore fails today**, and it
//! fails saying so. That is the required behaviour rather than a gap: ADR-015
//! accepts that version endpoints are Roblox's to change without notice and
//! requires a message naming what could not be reached instead of something that
//! appears to work. A check that quietly reported "up to date" would be the
//! stub-that-lies AGENTS.md is about — the user would be told they were current
//! while Roblox refused their build server-side.
//!
//! [`changelog`](crate::changelog) is the half that does work for Android:
//! Roblox's release notes name the engine major, and the engine major is the
//! number the client reports about itself.
//!
//! **Re-measured 2026-08-20.** `AndroidApp` still answers 500 with the same
//! body; `WindowsPlayer` now answers `0.735.0.7351131`, so the endpoint is live
//! and this is not an outage that happened to be caught twice.
//!
//! What has changed is the *other* operand. [`engine`](crate::engine) reads the
//! installed engine's version out of `libroblox.so`, so "which build is here"
//! no longer depends on this endpoint answering — on the build in this cache it
//! reads 2.734.0.917 against release notes for 734. This endpoint would still be
//! the better answer if it worked, because it names what Roblox is serving
//! rather than what is installed, and it is left wired for the day it does.

use crate::http;
use crate::Unreachable;

/// Roblox's settings CDN. The same host `client_settings` fetches the flag
/// document from.
pub const ENDPOINT: &str = "https://clientsettingscdn.roblox.com/v2/client-version/";

/// The Android application's binary type. Established, not guessed: see the
/// table above — every other Android spelling is refused as an invalid
/// `binaryType`, and this one is accepted.
pub const ANDROID: &str = "AndroidApp";

/// What the endpoint says about a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientVersion {
    /// `0.732.23.7321040`.
    pub version: String,
    /// `version-145f189a6a974303`. Roblox's own identifier for the upload, and
    /// the one that appears in deployment paths.
    pub upload: String,
}

impl ClientVersion {
    /// The engine major — `732` out of `0.732.23.7321040`.
    ///
    /// This is the number worth carrying around: it is what Roblox's release
    /// notes are titled by, and it is what the engine reports about itself
    /// (`Version=732` in `docs/traces/waydroid-roblox-startup.log.gz`). The
    /// fourth component is a build number and the first has been `0` for the
    /// whole life of this project.
    pub fn major(&self) -> Option<u32> {
        major_of(&self.version)
    }
}

/// `0.732.23.7321040` -> `732`.
/// A version as comparable numbers.
///
/// **String comparison is wrong here and quietly so**: lexically `"2.734.9"`
/// beats `"2.734.10"`, and the sources disagree on shape anyway -- the engine
/// records `2.734.0.917` while a mirror says `2.734.917`. Anything numeric is
/// compared and anything else ignored, rather than guessed at.
pub fn numeric(version: &str) -> Vec<u64> {
    version.split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse().ok()).collect()
}

/// The two numbers that identify a Roblox build: its major, and its build.
///
/// **Comparing the components element-wise is wrong**, and quietly so, because
/// the two sources that have to be compared do not agree on how many there
/// are. The engine records `2.734.0.917` and the mirror says `2.734.917` for
/// the same build; element-wise that reads 0 against 917 at the third position
/// and calls the identical build an update. Measured against those exact two
/// strings, which is how it was found.
///
/// The major is the second component in both shapes and the build is the last
/// in both, so those are the two that carry meaning. Everything between them
/// is padding that differs by source.
pub(crate) fn major_and_build(version: &str) -> Option<(u64, u64)> {
    let parts = numeric(version);
    match parts.len() {
        0 | 1 => None,
        _ => Some((parts[1], *parts.last().expect("len >= 2"))),
    }
}

/// Is `candidate` a newer build than `installed`?
///
/// Both or nothing: an unknown installed version is not an old one, and a
/// version neither shape can be read from is not a comparison.
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    match (major_and_build(candidate), major_and_build(installed)) {
        (Some(there), Some(here)) => there > here,
        _ => false,
    }
}

pub fn major_of(version: &str) -> Option<u32> {
    version.split('.').nth(1)?.parse().ok()
}

/// Ask about the Android build.
///
/// Expected to fail at the time of writing — see the module documentation. It
/// is written and wired anyway rather than left out, because the endpoint is
/// the one Roblox would serve an Android version from if it served one, and a
/// fetcher that has to be built the day it starts working is a fetcher nobody
/// has ever run.
pub fn check() -> Result<ClientVersion, Unreachable> {
    check_binary_type(ANDROID)
}

pub fn check_binary_type(binary_type: &str) -> Result<ClientVersion, Unreachable> {
    let url = format!("{ENDPOINT}{binary_type}");
    let body = http::get_text(&url)?;
    parse(&body).map_err(|why| Unreachable::Malformed { url, why })
}

/// Read the endpoint's success body.
///
/// Separate from the request so the shape can be tested against the bodies in
/// the table above without a network, which is the only way this parser stays
/// pinned to something observed rather than to something remembered.
pub fn parse(body: &str) -> Result<ClientVersion, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    // Roblox's error bodies are also valid JSON with a 200-shaped content type,
    // so an absent `version` is named as such rather than reported as a parse
    // failure.
    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or("no \"version\" string in the reply")?;
    let upload = value
        .get("clientVersionUpload")
        .and_then(|v| v.as_str())
        .ok_or("no \"clientVersionUpload\" string in the reply")?;
    if version.is_empty() {
        return Err("\"version\" is empty".into());
    }
    Ok(ClientVersion { version: version.to_string(), upload: upload.to_string() })
}

#[cfg(test)]
mod tests {
    /// **The pair that produced the bug.** The engine writes four components
    /// and the mirror three, for the same build.
    #[test]
    fn the_two_version_shapes_compare_equal_for_one_build() {
        assert!(!super::is_newer("2.734.917", "2.734.0.917"), "same build, different shapes");
        assert!(!super::is_newer("2.734.0.917", "2.734.917"));
        // And a genuinely newer major still reads as newer, either way round.
        assert!(super::is_newer("2.735.1138", "2.734.0.917"));
        assert!(!super::is_newer("2.734.0.917", "2.735.1138"));
        // A newer build within the same major.
        assert!(super::is_newer("2.734.918", "2.734.0.917"));
        // Nonsense compares as nothing rather than as newer.
        assert!(!super::is_newer("", "2.734.0.917"));
        assert!(!super::is_newer("2.734.0.917", ""));
    }

    use super::*;

    /// Copied from the wire on 2026-08-02, byte for byte.
    const WINDOWS_PLAYER: &str = r#"{"version":"0.732.23.7321040","clientVersionUpload":"version-145f189a6a974303","bootstrapperVersion":""}"#;
    const ANDROID_APP_500: &str =
        r#"{"errors":[{"code":3,"message":"Error while fetching version information."}]}"#;

    #[test]
    fn the_success_shape_is_the_one_that_was_observed() {
        let v = parse(WINDOWS_PLAYER).unwrap();
        assert_eq!(v.version, "0.732.23.7321040");
        assert_eq!(v.upload, "version-145f189a6a974303");
        assert_eq!(v.major(), Some(732));
    }

    #[test]
    fn an_error_body_is_not_read_as_a_version() {
        // It is valid JSON, so "did it parse" is not the question. This is the
        // body `AndroidApp` actually returns, and reading it as a version is
        // how a fetcher reports success on an outage.
        let e = parse(ANDROID_APP_500).unwrap_err();
        assert!(e.contains("version"), "{e}");
    }

    #[test]
    fn an_empty_version_is_refused() {
        let e = parse(r#"{"version":"","clientVersionUpload":"x"}"#).unwrap_err();
        assert!(e.contains("empty"), "{e}");
    }

    #[test]
    fn a_failure_names_the_url_it_could_not_get_an_answer_from() {
        // ADR-015 requires this in as many words. The assertion is on the
        // rendered message rather than the variant, because the message is what
        // reaches whoever has to fix the endpoint.
        let e = Unreachable::Status {
            url: format!("{ENDPOINT}{ANDROID}"),
            status: 500,
            body: ANDROID_APP_500.into(),
        };
        let shown = e.to_string();
        assert!(shown.contains("clientsettingscdn.roblox.com"), "{shown}");
        assert!(shown.contains("AndroidApp"), "{shown}");
        assert!(shown.contains("500"), "{shown}");
    }

    #[test]
    fn the_major_is_the_second_component() {
        assert_eq!(major_of("0.732.23.7321040"), Some(732));
        assert_eq!(major_of("0.732.0.7321040"), Some(732));
        assert_eq!(major_of("nonsense"), None);
        assert_eq!(major_of(""), None);
    }
}
