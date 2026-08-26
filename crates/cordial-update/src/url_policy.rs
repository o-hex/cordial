//! Which URLs Cordial will connect to, and how a redirect is followed.
//!
//! [ADR-025](../../../docs/adr/ADR-025-fetching-from-a-third-party-mirror.md)
//! permits fetching the Roblox build from a distributor that is not Roblox, on
//! one condition: the bytes are worthless until
//! [`crate::apk_signature`] says Roblox signed them. That condition is what
//! makes the mirror acceptable, and it is also what makes *this* file
//! necessary, because a signature check answers "are these the right bytes"
//! and answers nothing at all about where Cordial's process just made a
//! connection to, what it sent, or how long it waited.
//!
//! So the URLs a provider hands back are treated as what they are: strings
//! chosen by somebody else.
//!
//! ## Redirects are followed by hand, and that is the point
//!
//! An HTTP client that follows redirects for you validates the first URL and
//! then goes wherever that URL's owner sends it. Every check in [`check`] would
//! apply to exactly one hop and none of the others, which is the same as not
//! having them: the mirror would only need to serve one `302` to have Cordial
//! fetch from anywhere it liked.
//!
//! [`walk`] therefore turns redirect following off in the client and does it in
//! this file, re-checking every target in full against the same rules as the
//! first. It is more code than `.call()` and it is the difference between an
//! allow-list and a decoration.
//!
//! ## Why the host list has three names on it
//!
//! `winudf.com` looks out of place beside the two APKPure names and it is
//! where the bytes actually come from -- APKPure's CDN serves from it, so a
//! list without it refuses every real download. It is a third organisation to
//! trust with the transport, and naming it here rather than quietly widening
//! the rule is the honest version of that.

use crate::Unreachable;
use http::Uri;
use std::time::Duration;

/// Which hosts a URL may name, and whether subdomains of them count.
///
/// **This used to be a fixed pair of lists compiled into this file**, and it
/// stopped being one when the mirror became configurable. The lists are now
/// built by whoever is about to make the request -- see
/// [`crate::provider::mirror::Mirror`] -- because a user who points Cordial at
/// a different endpoint has to be able to point it at that endpoint's hosts
/// too, and a check that consulted a hardcoded list would refuse the very
/// thing they configured.
///
/// That does not weaken the check. It moves who writes the list from this file
/// to the user, and it is still never the mirror: a `302` from an allowed host
/// to an unlisted one is refused exactly as before, which is the property the
/// allow-list exists for.
#[derive(Debug, Clone)]
pub struct Allowed {
    pub hosts: Vec<String>,
    /// Whether a subdomain of an allowed name is also allowed.
    ///
    /// True for downloads, because the bytes come off a CDN whose hostnames are
    /// not enumerable in advance. False for metadata: that is one endpoint, and
    /// the version list is the one answer in this whole path with no
    /// cryptographic check behind it -- an archive's signature says Roblox made
    /// it, and nothing says the version list was honest -- so the host that
    /// gives it is kept exact.
    pub subdomains: bool,
}

impl Allowed {
    pub fn exactly(host: impl Into<String>) -> Self {
        Allowed { hosts: vec![host.into()], subdomains: false }
    }

    pub fn any_of(hosts: impl IntoIterator<Item = String>) -> Self {
        Allowed { hosts: hosts.into_iter().collect(), subdomains: true }
    }
}

/// At most five hops, so at most six requests. A chain longer than this is
/// either a loop or a provider doing something Cordial should not be following
/// blind, and both want the same answer.
const MAX_REDIRECTS: usize = 5;

/// Why a URL was not connected to.
///
/// Each variant is a separate sentence to a user because they mean genuinely
/// different things: a plain-HTTP URL is a provider doing something wrong, an
/// unknown host is a provider doing something Cordial deliberately will not
/// follow, and a redirect loop is usually an outage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    Unparseable { url: String },
    NotHttps { url: String, scheme: String },
    HostNotAllowed { url: String, host: String },
    UnexpectedPort { url: String, port: u16 },
    CarriesCredentials { url: String },
    TooManyRedirects { url: String },
    RelativeRedirect { url: String, target: String },
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejected::Unparseable { url } => {
                write!(f, "the provider offered something that is not a URL: {url}")
            }
            Rejected::NotHttps { url, scheme } => write!(
                f,
                "the provider offered a {scheme} URL and Cordial only fetches over https: {url}"
            ),
            Rejected::HostNotAllowed { url, host } => write!(
                f,
                "the provider tried to send Cordial to {host}, which is not a host it downloads \
                 from: {url}"
            ),
            Rejected::UnexpectedPort { url, port } => write!(
                f,
                "the provider named port {port} rather than 443, so Cordial did not connect: {url}"
            ),
            Rejected::CarriesCredentials { url } => write!(
                f,
                "the provider offered a URL carrying credentials in it, which Cordial refuses \
                 rather than sends: {url}"
            ),
            Rejected::TooManyRedirects { url } => write!(
                f,
                "fetching {url} redirected more than {MAX_REDIRECTS} times without arriving \
                 anywhere"
            ),
            Rejected::RelativeRedirect { url, target } => write!(
                f,
                "fetching {url} was redirected to {target}, which Cordial cannot resolve safely \
                 and will not guess at"
            ),
        }
    }
}

impl From<Rejected> for Unreachable {
    fn from(r: Rejected) -> Self {
        Unreachable::Malformed { url: r.url().to_string(), why: r.to_string() }
    }
}

impl Rejected {
    fn url(&self) -> &str {
        match self {
            Rejected::Unparseable { url }
            | Rejected::NotHttps { url, .. }
            | Rejected::HostNotAllowed { url, .. }
            | Rejected::UnexpectedPort { url, .. }
            | Rejected::CarriesCredentials { url }
            | Rejected::TooManyRedirects { url }
            | Rejected::RelativeRedirect { url, .. } => url,
        }
    }
}

/// Does `candidate` name the same host as `allowed`, or one under it?
///
/// **This is the check a substring match gets wrong**, and getting it wrong is
/// not subtle: `evilapkpure.com` contains `apkpure.com`, and a rule written
/// with `contains` hands an attacker the whole allow-list for the price of a
/// domain registration. A label boundary is required, so only `apkpure.com`
/// itself and things ending `.apkpure.com` match.
fn host_matches(candidate: &str, allowed: &str, subdomains: bool) -> bool {
    let c = candidate.trim_end_matches('.').to_ascii_lowercase();
    let a = allowed.to_ascii_lowercase();
    c == a || (subdomains && c.ends_with(&format!(".{a}")))
}

/// Check one URL against the rules for `purpose`, and return its host.
///
/// The parse is done by [`http::Uri`] rather than by looking for the allowed
/// name in the string, because the two disagree on exactly the input that
/// matters. `https://api.pureapk.com@evil.example/` reads to a human as the
/// allowed host and its actual host is `evil.example`; the userinfo before the
/// `@` is a username. A real parser says so, and this refuses userinfo outright
/// as well, since no URL this project fetches has any business carrying it.
pub fn check(url: &str, allowed: &Allowed) -> Result<String, Rejected> {
    let uri: Uri = url.parse().map_err(|_| Rejected::Unparseable { url: url.to_string() })?;

    let scheme = uri.scheme_str().unwrap_or("").to_ascii_lowercase();
    if scheme != "https" {
        return Err(Rejected::NotHttps {
            url: url.to_string(),
            scheme: if scheme.is_empty() { "relative".into() } else { scheme },
        });
    }

    let authority =
        uri.authority().ok_or_else(|| Rejected::Unparseable { url: url.to_string() })?;
    if authority.as_str().contains('@') {
        return Err(Rejected::CarriesCredentials { url: url.to_string() });
    }

    if let Some(port) = uri.port_u16() {
        if port != 443 {
            return Err(Rejected::UnexpectedPort { url: url.to_string(), port });
        }
    }

    let host = uri.host().ok_or_else(|| Rejected::Unparseable { url: url.to_string() })?;
    if !allowed.hosts.iter().any(|a| host_matches(host, a, allowed.subdomains)) {
        return Err(Rejected::HostNotAllowed { url: url.to_string(), host: host.to_string() });
    }

    Ok(host.to_ascii_lowercase())
}

/// The client used for everything in this module.
///
/// Three settings do the work. Redirects are off, because [`walk`] follows them
/// itself and a client that also followed them would defeat the checks. The
/// protocol is restricted to https at the transport as well as in [`check`], so
/// a bug in one is not the only thing standing between Cordial and a
/// plain-text fetch. And status codes are not errors, because a `302` is not a
/// failure here and a `503` needs its body read to say what the host said.
pub fn agent(connect: Duration, transfer: Duration) -> ureq::Agent {
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .https_only(true)
        .user_agent(crate::http::USER_AGENT)
        .timeout_connect(Some(connect))
        .timeout_global(Some(transfer))
        .build();
    ureq::Agent::new_with_config(config)
}

/// GET `url`, following redirects by hand and re-checking every one.
///
/// Returns the response at the end of the chain, whatever its status: a 4xx or
/// 5xx comes back as a response and not an error, so the caller can name the
/// host that refused. That name is the single most useful thing in a first-run
/// failure message, because "the mirror is down" and "your network is broken"
/// look identical from inside the process and only one of them is the user's
/// problem.
pub fn walk(
    agent: &ureq::Agent,
    url: &str,
    allowed: &Allowed,
    headers: &[(&str, &str)],
) -> Result<(String, ureq::http::Response<ureq::Body>), Unreachable> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        check(&current, allowed)?;

        let mut request = agent.get(&current);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request
            .call()
            .map_err(|e| Unreachable::Transport { url: current.clone(), why: e.to_string() })?;

        let status = response.status().as_u16();
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Ok((current, response));
        }

        let target = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Unreachable::Malformed {
                url: current.clone(),
                why: format!("the host answered {status} with no location to redirect to"),
            })?
            .to_string();

        // An origin-relative target is resolved against the hop it came from,
        // which is the only form worth supporting: CDNs use it and it cannot
        // move Cordial to another host by construction. Anything else is
        // refused rather than guessed at.
        current = if target.starts_with("https://") {
            target
        } else if let Some(rest) = target.strip_prefix('/') {
            let base: Uri = current.parse().map_err(|_| Unreachable::Malformed {
                url: current.clone(),
                why: "this hop stopped being a URL".into(),
            })?;
            format!("https://{}/{rest}", base.authority().map(|a| a.as_str()).unwrap_or_default())
        } else {
            return Err(Rejected::RelativeRedirect { url: current, target }.into());
        };
    }
    Err(Rejected::TooManyRedirects { url: url.to_string() }.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Allowed {
        Allowed::exactly("api.pureapk.com")
    }

    fn downloads() -> Allowed {
        Allowed::any_of(
            ["pureapk.com", "apkpure.com", "winudf.com"].map(String::from),
        )
    }

    /// The two lists are not the same list, and the metadata one is much
    /// narrower: one host, no subdomains. A CDN is trusted to serve an archive
    /// whose signature is then checked, and is not trusted to say what versions
    /// exist -- that answer is not verifiable against anything.
    #[test]
    fn the_download_cdn_is_not_trusted_to_answer_the_metadata_question() {
        assert!(check("https://api.pureapk.com/m/v3/cms/app_version", &metadata()).is_ok());
        assert!(matches!(
            check("https://download.winudf.com/m/v3/cms/app_version", &metadata()),
            Err(Rejected::HostNotAllowed { host, .. }) if host == "download.winudf.com"
        ));
        // The list is exact, so even a subdomain of the metadata host is not a
        // second metadata host.
        assert!(matches!(
            check("https://cdn.api.pureapk.com/x", &metadata()),
            Err(Rejected::HostNotAllowed { .. })
        ));
    }

    #[test]
    fn the_cdn_is_allowed_for_downloads() {
        assert!(check("https://d.cdnpure.com/b/APK/x", &downloads()).is_err());
        assert!(check("https://download.winudf.com/x.apk", &downloads()).is_ok());
        assert!(check("https://apkpure.com/x.apk", &downloads()).is_ok());
    }

    /// **The one a substring match gets wrong.** Registering `evilapkpure.com`
    /// costs about ten pounds, and against a `contains` check it buys the
    /// entire download allow-list.
    #[test]
    fn a_host_that_merely_ends_with_an_allowed_name_is_not_a_subdomain_of_it() {
        assert!(matches!(
            check("https://evilapkpure.com/x.apk", &downloads()),
            Err(Rejected::HostNotAllowed { host, .. }) if host == "evilapkpure.com"
        ));
        assert!(matches!(
            check("https://apkpure.com.evil.example/x.apk", &downloads()),
            Err(Rejected::HostNotAllowed { .. })
        ));
    }

    /// **The one a human reading the string gets wrong.** The host here is
    /// `evil.example`; everything before the `@` is a username.
    #[test]
    fn userinfo_that_looks_like_an_allowed_host_is_refused() {
        let u = "https://api.pureapk.com@evil.example/m/v3/cms/app_version";
        assert!(matches!(check(u, &metadata()), Err(Rejected::CarriesCredentials { .. })));
    }

    #[test]
    fn a_trailing_root_dot_does_not_smuggle_a_host_past_the_list() {
        assert!(check("https://api.pureapk.com./x", &metadata()).is_ok());
        assert!(check("https://API.PUREAPK.COM/x", &metadata()).is_ok());
    }

    #[test]
    fn plain_http_and_odd_ports_are_refused_separately() {
        assert!(matches!(
            check("http://api.pureapk.com/x", &metadata()),
            Err(Rejected::NotHttps { .. })
        ));
        assert!(matches!(
            check("https://api.pureapk.com:8443/x", &metadata()),
            Err(Rejected::UnexpectedPort { port: 8443, .. })
        ));
        // 443 stated explicitly is the same as not stating it.
        assert!(check("https://api.pureapk.com:443/x", &metadata()).is_ok());
    }

    #[test]
    fn a_refusal_says_which_host_it_refused() {
        let e = check("https://evil.example/x.apk", &downloads()).unwrap_err();
        assert!(e.to_string().contains("evil.example"), "{e}");
    }

    /// **A user-configured list is still a list.** The point of making the
    /// hosts a value rather than a constant was to let somebody name their own
    /// endpoint -- not to let whatever they named send Cordial anywhere else.
    #[test]
    fn a_custom_list_still_refuses_everything_not_on_it() {
        let mine = Allowed::any_of(["mirror.example".to_string()]);
        assert!(check("https://mirror.example/x.apk", &mine).is_ok());
        assert!(check("https://cdn.mirror.example/x.apk", &mine).is_ok());
        assert!(matches!(
            check("https://apkpure.com/x.apk", &mine),
            Err(Rejected::HostNotAllowed { .. })
        ));
    }
}
