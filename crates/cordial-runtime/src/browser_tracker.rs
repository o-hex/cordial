//! The device's tracker identity — `RBXEventTrackerV2`.
//!
//! Roblox's clients identify the *device* separately from the account, through a
//! cookie called `RBXEventTrackerV2` obtained from
//! `apis.roblox.com/browser-tracker-api/device/initialize`. Cordial has never
//! asked for one, and has therefore always run without a device identity.
//!
//! ## Why this is here now
//!
//! Not from reading the binary. mocktail — which plays for four minutes on the
//! place Cordial is disconnected from at sixty seconds — bootstraps this before
//! its engine starts, and Cordial does not. §13 of `docs/analysis/flag-init.md`
//! has the measurement.
//!
//! Two things in this repository already pointed at it and were never joined up:
//! `docs/analysis/webview-surface.md` records that `libroblox.so` carries the
//! string `BrowserTrackerIdRequest: No RBXEventTrackerV2 in cookie.`, so the
//! engine has a path that notices the cookie missing; and `docs/design/sign-in.md`
//! captured this exact endpoint being called on real Android.
//!
//! **It is `INFERRED` that this has anything to do with the 304.** What is
//! established is that a working client does it and Cordial does not. That is a
//! reason to try it with a control, not a diagnosis.
//!
//! ## Scope
//!
//! This is an ordinary HTTPS request that the real client makes, answered by
//! Roblox's own server, and the value kept is the one Roblox returns. Nothing is
//! forged, replayed, or asserted on the client's behalf — the suggestion in the
//! query string is exactly that, and the server is free to ignore it and hand
//! back something else. That is the same footing as fetching client settings in
//! [`crate::client_settings`], which is also work the host application is
//! supposed to do.

use std::path::{Path, PathBuf};

/// Roblox's device-initialisation endpoint.
const URL: &str = "https://apis.roblox.com/browser-tracker-api/device/initialize";

/// The cookie the endpoint answers with.
pub const COOKIE: &str = "RBXEventTrackerV2";

/// Where the suggested id is kept between launches.
///
/// Inside the profile rather than beside the binary, because a *device* identity
/// that changed whenever somebody switched profile would not be a device
/// identity. ADR-012 makes the profile the unit that owns per-installation
/// state.
fn id_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("browser-tracker-id")
}

/// Whether a string is shaped like a tracker id.
///
/// A decimal integer, non-zero, no leading zero, at most twenty digits — which
/// is `u64`'s range. Worth checking rather than trusting: a truncated or
/// half-written file would otherwise be sent as a suggestion and the failure
/// would appear as a server-side rejection with nothing local to look at.
pub fn is_valid_id(value: &str) -> bool {
    if value.is_empty() || value.len() > 20 || value.starts_with('0') {
        return false;
    }
    matches!(value.parse::<u64>(), Ok(n) if n != 0)
}

/// Read the stored id, if there is a usable one.
pub fn stored_id(profile_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(id_path(profile_dir)).ok()?;
    let trimmed = raw.trim().to_string();
    is_valid_id(&trimmed).then_some(trimmed)
}

/// Keep an id for next launch.
pub fn store_id(profile_dir: &Path, id: &str) -> std::io::Result<()> {
    if !is_valid_id(id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a tracker id",
        ));
    }
    if let Some(parent) = id_path(profile_dir).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(id_path(profile_dir), id)
}

/// Pull `RBXEventTrackerV2`'s value out of a `Set-Cookie` header.
///
/// Deliberately not a general cookie parser. It answers one question — did this
/// response carry the tracker cookie, and what is it — and a wrong answer here
/// is a device identity silently not being kept, which would look exactly like
/// the endpoint having failed.
pub fn tracker_from_set_cookie(header: &str) -> Option<String> {
    for part in header.split(';') {
        let part = part.trim();
        let (name, value) = part.split_once('=')?;
        if name.trim() == COOKIE {
            let v = value.trim();
            return (!v.is_empty()).then(|| v.to_string());
        }
        // Only the first pair is the cookie itself; everything after is
        // attributes (Path, Domain, Max-Age). Stop rather than match an
        // attribute that happens to share the name.
        break;
    }
    None
}

/// What a bootstrap attempt produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The server answered and this is the cookie value it set.
    Initialised(String),
    /// The request was made and carried no tracker cookie back.
    NoCookie,
    /// The request could not be made.
    Failed(String),
}

/// Ask Roblox to initialise this device, suggesting `id`.
///
/// The suggestion is a suggestion: the server decides, and whatever it sets is
/// what gets kept. Returning [`Outcome`] rather than a bare `Option` so that
/// "the endpoint said no cookie" and "the network was down" stay distinguishable
/// in the log — they need different responses and conflating them has cost this
/// project time before.
pub fn initialise(id: &str) -> Outcome {
    let url = format!("{URL}?suggestedBrowserTrackerId={id}");
    match ureq::get(&url).call() {
        Ok(response) => {
            let set_cookie = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .find_map(tracker_from_set_cookie);
            match set_cookie {
                Some(v) => Outcome::Initialised(v),
                None => Outcome::NoCookie,
            }
        }
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_a_nonzero_decimal_without_a_leading_zero() {
        assert!(is_valid_id("1"));
        assert!(is_valid_id("18446744073709551615")); // u64::MAX, twenty digits
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("0"));
        assert!(!is_valid_id("0123"));
        assert!(!is_valid_id("12a4"));
        assert!(!is_valid_id("-5"));
        // Twenty-one digits is past u64 and past the length rule.
        assert!(!is_valid_id("123456789012345678901"));
    }

    #[test]
    fn the_tracker_cookie_is_read_out_of_a_set_cookie_header() {
        assert_eq!(
            tracker_from_set_cookie("RBXEventTrackerV2=CreateDate=1/1/2026; Path=/; Domain=.roblox.com")
                .as_deref(),
            Some("CreateDate=1/1/2026")
        );
    }

    /// A `Set-Cookie` for something else must not be mistaken for ours, and an
    /// attribute that happens to be named like the cookie must not either.
    #[test]
    fn another_cookie_is_not_mistaken_for_the_tracker() {
        assert_eq!(tracker_from_set_cookie("RBXSource=abc; Path=/"), None);
        assert_eq!(
            tracker_from_set_cookie("SomethingElse=x; RBXEventTrackerV2=sneaky"),
            None
        );
    }

    #[test]
    fn an_empty_value_is_not_an_identity() {
        assert_eq!(tracker_from_set_cookie("RBXEventTrackerV2=; Path=/"), None);
    }

    #[test]
    fn an_id_survives_a_round_trip_through_the_profile() {
        let dir = std::env::temp_dir().join(format!("cordial-bt-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(stored_id(&dir), None);
        store_id(&dir, "1234567890").unwrap();
        assert_eq!(stored_id(&dir).as_deref(), Some("1234567890"));
        // A file that got corrupted must read as absent rather than be sent.
        std::fs::write(id_path(&dir), "not-an-id").unwrap();
        assert_eq!(stored_id(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rubbish_id_is_refused_rather_than_written() {
        let dir = std::env::temp_dir().join(format!("cordial-bt-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(store_id(&dir, "0").is_err());
        assert!(!id_path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
