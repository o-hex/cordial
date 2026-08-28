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
//!
//! **Redirects are walked and re-checked, the same as a mirror's.** This used
//! to hand `url` straight to a plain `ureq::Agent` with `https_only` unset and
//! ureq's own default of ten redirects left in place -- no per-hop host check
//! at all, on every request this file makes, unlike [`crate::url_policy`],
//! which this same crate built precisely so a redirect could not walk a
//! request off the host it was sent to. Every URL here is a compile-time
//! constant naming `clientsettingscdn.roblox.com`, `devforum.roblox.com` or
//! `create.roblox.com`, so the request itself was never the risk; a redirect
//! is, and the fix is the same one `mirror` already needed: follow it by hand
//! and require every hop to stay on the host the request started on.

use crate::url_policy;
use crate::Unreachable;
use std::time::Duration;

/// Identifies this as Cordial, truthfully. ADR-015: never pretends to be the
/// official client.
pub const USER_AGENT: &str = concat!("Cordial/", env!("CARGO_PKG_VERSION"), " (Linux)");

/// How long to wait for the connection itself. `url_policy::agent`'s other
/// knob; `mirror` measured the same number and there is no reason for a
/// metadata request here to wait longer to find out nobody is answering.
const CONNECT: Duration = Duration::from_secs(10);

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

/// GET `url` and return its body, or why there is no body.
///
/// A non-2xx answer comes back as [`Unreachable::Status`] carrying the body
/// rather than as a bare code, because that body is the difference between "the
/// endpoint moved" and "Roblox is having a bad day".
///
/// Every hop of a redirect has to stay on `url`'s own host: nothing calling
/// this hands it anything but a Roblox endpoint, and a redirect leaving that
/// host is exactly the thing [`url_policy::walk`] exists to refuse rather than
/// follow.
pub fn get_text(url: &str) -> Result<String, Unreachable> {
    let host = url_policy::host_of(url)?;
    let agent = url_policy::agent(CONNECT, TIMEOUT);
    let (final_url, mut response) =
        url_policy::walk(&agent, url, &url_policy::Allowed::exactly(host), &[])?;

    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .map_err(|e| Unreachable::Transport { url: final_url.clone(), why: e.to_string() })?;

    if !(200..300).contains(&status) {
        return Err(Unreachable::Status { url: final_url, status, body });
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
