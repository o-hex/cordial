//! `url.open` — open a URL in the user's browser, through the portal.
//!
//! The narrowest useful form of "leave the application". Cordial validates
//! the scheme before the URL ever reaches `org.freedesktop.portal.OpenURI`;
//! without that check this capability would be indistinguishable from
//! handing a plugin a way to trigger `file://` traversal or hijack whatever
//! handler the desktop has registered for an arbitrary scheme (`mailto:`,
//! a browser extension's own scheme, or worse — some desktops register
//! schemes that map to local applications with their own command-line
//! parsing). `http` and `https` only, per the doc comment on
//! `Capability::UrlOpen`.

use zbus::blocking::Connection;
use zbus::zvariant::Value;
use std::collections::HashMap;

/// Check the scheme without pulling in a full URL-parsing dependency for one
/// prefix check. Deliberately case-insensitive and deliberately strict about
/// the `://` — `http:example.com` or `javascript:...` styled to merely
/// *contain* "http" must not slip past a looser check.
fn validate_scheme(raw: &str) -> Result<(), String> {
    match raw.split_once("://") {
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") => Ok(()),
        Some((scheme, _)) => Err(format!("scheme {scheme:?} is refused; only http and https may be opened")),
        None => Err("not an absolute http or https URL".into()),
    }
}

/// Open `raw` in the user's browser via the portal. Refuses anything that is
/// not `http://` or `https://` before Cordial's D-Bus connection is even
/// touched, so a refusal never depends on whether a session bus happens to
/// be reachable.
pub fn open(raw: &str) -> Result<(), String> {
    validate_scheme(raw)?;
    let conn = Connection::session().map_err(|e| format!("could not reach the session bus: {e}"))?;
    let options: HashMap<&str, Value> = HashMap::new();
    // The first argument is a portal "parent window" handle; Cordial has no
    // portal-aware window identifier to offer here, and the portal accepts
    // an empty one and opens without trying to be modal to a parent.
    conn.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.OpenURI"),
        "OpenURI",
        &("", raw, options),
    )
    .map_err(|e| format!("the OpenURI portal refused the call: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_session_bus() -> bool {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
    }

    #[test]
    fn http_and_https_are_accepted() {
        assert!(validate_scheme("http://example.com").is_ok());
        assert!(validate_scheme("https://example.com/path?q=1").is_ok());
        // Scheme comparison is case-insensitive, matching RFC 3986 §3.1;
        // rejecting "HTTPS://" on casing alone would be a spurious refusal.
        assert!(validate_scheme("HTTPS://example.com").is_ok());
    }

    #[test]
    fn a_non_http_scheme_is_refused() {
        // The exact case ADR-007's doc comment calls out: this must not
        // become file:// traversal or a handler hijack for some other
        // registered scheme.
        for scheme in ["file:///etc/passwd", "javascript:alert(1)", "mailto:a@b.com", "ftp://host/x", "discord://x"] {
            assert!(url_open_refuses(scheme), "{scheme} should have been refused");
        }
    }

    fn url_open_refuses(raw: &str) -> bool {
        validate_scheme(raw).is_err()
    }

    #[test]
    fn a_bare_string_with_no_scheme_is_refused() {
        assert!(validate_scheme("example.com").is_err());
        assert!(validate_scheme("").is_err());
    }

    #[test]
    fn opening_a_disallowed_scheme_never_touches_the_bus() {
        // Refused before any D-Bus call, so this holds even where there is
        // no session bus at all to reach.
        let err = open("file:///etc/passwd").unwrap_err();
        assert!(err.contains("refused"), "{err}");
    }

    /// `#[ignore]`, and it has to stay that way.
    ///
    /// This drives the real portal against the real session bus, which means it
    /// really does open a browser on the machine running it. `cargo test
    /// --workspace` is run constantly here — by every contributor, by every
    /// agent, and by anything watching the tree — so a test with that side
    /// effect spawned tens of browser tabs at a developer who was trying to use
    /// their desktop. A test is allowed to be slow or to need a fixture; it is
    /// not allowed to take over the machine it runs on.
    ///
    /// Run it deliberately when changing the portal call:
    ///
    /// ```text
    /// cargo test -p cordial-plugins -- --ignored urlopen
    /// ```
    ///
    /// The scheme validation above is what actually guards the capability, and
    /// that is covered by ordinary tests that open nothing.
    #[test]
    #[ignore = "opens a real browser window; run with --ignored"]
    fn an_http_url_round_trips_through_the_real_portal_when_one_is_reachable() {
        if !have_session_bus() {
            eprintln!("skipping: no DBUS_SESSION_BUS_ADDRESS in this environment");
            return;
        }
        match open("https://example.com") {
            Ok(()) => {}
            Err(e) => panic!("the portal call failed against a real session bus: {e}"),
        }
    }
}
