//! The one HTTP client this crate uses, and the two things it is configured for.
//!
//! **Status codes are not errors.** ureq turns 4xx and 5xx into `Err` by
//! default and throws the body away with them, and the body is where Roblox
//! says what went wrong: `AndroidApp` answers
//! `{"errors":[{"code":3,"message":"Error while fetching version information."}]}`
//! with an HTTP 500, and `AndroidPlayer` answers `"Invalid binaryType."` with a
//! 400. Those two are a Roblox outage and a name Cordial got wrong, and a
//! fetcher that cannot tell them apart makes whoever maintains this guess.
//!
//! **Cordial says it is Cordial.** ADR-015 is explicit that this never pretends
//! to be the official client, and a request with no user agent at all is a
//! smaller lie than a copied one but is still not an answer. There is nothing
//! to gain by hiding: the file is public and the endpoints are public.

use crate::Unreachable;
use std::time::Duration;

/// Identifies this as Cordial, truthfully. ADR-015: never pretends to be the
/// official client.
pub const USER_AGENT: &str = concat!("Cordial/", env!("CARGO_PKG_VERSION"), " (Linux)");

/// Long enough for a CDN having a slow morning, short enough that the
/// background check after launch does not hold a thread for a minute. Nothing
/// waits on this: the check runs after the window is up.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Roblox's settings CDN answers a bad application name with a well-formed JSON
/// error body of a few dozen bytes, and its release-notes listing is a few
/// hundred kilobytes. Anything larger than this from a *metadata* endpoint is
/// not metadata, and reading it into a `String` would be the fetcher's own
/// denial of service. The APK download does not come through here — see
/// [`crate::download`], which streams.
const MAX_BODY: u64 = 8 * 1024 * 1024;

fn agent() -> ureq::Agent {
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .timeout_global(Some(TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

/// GET `url` and return its body, or why there is no body.
///
/// A non-2xx answer comes back as [`Unreachable::Status`] carrying the body
/// rather than as a bare code, because that body is the difference between "the
/// endpoint moved" and "Roblox is having a bad day".
pub fn get_text(url: &str) -> Result<String, Unreachable> {
    let mut response = agent()
        .get(url)
        .call()
        .map_err(|e| Unreachable::Transport { url: url.to_string(), why: e.to_string() })?;

    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .map_err(|e| Unreachable::Transport { url: url.to_string(), why: e.to_string() })?;

    if !(200..300).contains(&status) {
        return Err(Unreachable::Status { url: url.to_string(), status, body });
    }
    Ok(body)
}

/// GET `url` and parse the body as JSON.
///
/// Separate from [`get_text`] only so the "answered 200 with a shape this
/// Cordial cannot read" case gets its own name. Roblox changing a field is the
/// failure ADR-015 says must not look like "no update available", and folding it
/// into a transport error would do exactly that.
pub fn get_json(url: &str) -> Result<serde_json::Value, Unreachable> {
    let body = get_text(url)?;
    serde_json::from_str(&body)
        .map_err(|e| Unreachable::Malformed { url: url.to_string(), why: e.to_string() })
}
