//! The in-experience web window, as a dialog attached to the client.
//!
//! Roblox keeps a browser inside itself. Account settings, buying Robux, the
//! sign-in flow and anything else the client does not render natively all open
//! one of these. Without it those buttons do nothing at all — no window, no
//! error, no log line.
//!
//! Informed by mocktail's implementation, Copyright 2026 komaruworld,
//! Apache-2.0 — see `NOTICE` and `third_party/mocktail-webview/`. The security
//! rules are theirs and live in [`crate::webview_policy`]; this file is Cordial's
//! own window and does not share their structure. Apache-2.0 section 4(b): a
//! derived work, saying so.
//!
//! ## Why a dialog rather than a window
//!
//! Because the protocol says so, once you read what it asks for. The engine
//! exports `getHideHeaderKey`, `getShowDomainAsTitleKey` and
//! `getBackButtonVisibleKey` — that is chrome for a view embedded in an app with
//! a header bar that can be suppressed, not for a browser window. On Android
//! these are in-app overlays and never separate tasks.
//!
//! An `AdwDialog` is the same thing on this desktop: attached to the client
//! window, adaptive, and dismissed by the gesture every other GNOME sheet uses.
//! A second top-level would land wherever the compositor decided, which this
//! project has already fought once over the engine canvas.
//!
//! ## Why in-process, when mocktail's helper is not
//!
//! Checked directly against `webview_helper_launcher.cc` before writing this,
//! because the filename is a real architectural claim and deserved reading
//! rather than assuming. What it shows: mocktail's helper is a second
//! `posix_spawn`ed binary talking to the first over a `SOCK_SEQPACKET`
//! control channel it hand-rolls (`MOCKTAIL-WEBVIEW 1`, `MWVC`/`MWVE` framed
//! packets) — real process isolation, built because mocktail's engine process
//! is C++ over SDL3 and carries **no GTK at all**. Adding a browser to that
//! process would mean linking GTK, WebKitGTK and libadwaita into a process
//! that currently has none of them, for a feature the engine process itself
//! never needs to touch.
//!
//! That reasoning does not transfer. [ADR-011](../../../docs/adr/ADR-011-wayland-and-libadwaita.md)
//! already put GTK4 and libadwaita in Cordial's engine process, unconditionally,
//! for a reason that has nothing to do with web views: the engine's `wl_surface`
//! is a `wl_subsurface` of the GTK toplevel, and a Wayland subsurface cannot
//! parent across a process boundary, so "one connection, therefore one
//! process" was decided before this module existed. Cordial does not choose
//! between one GTK process and none — it is already one GTK process, and a
//! `WebKitWebView` in that process is a widget, not a second toolkit.
//! `docs/analysis/webview-surface.md` §5 reaches the same conclusion by
//! reading the WebKitGTK API surface rather than mocktail's launcher, and is
//! the fuller writeup; this file's summary and that document should not be
//! allowed to drift apart.
//!
//! What actually isolates page content — the process boundary mocktail's
//! helper buys by being a separate binary — Cordial gets for free from
//! **WebKitGTK's own multi-process model**: a `WebKitWebView` is a widget, and
//! the page itself runs in `WebKitWebProcess` and `WebKitNetworkProcess` under
//! WebKit's own sandbox, maintained by people who do it full time. A
//! hand-rolled helper would add a second IPC layer on top of that for
//! isolation already present, and it would cost the attached dialog, because a
//! separate process cannot present one — `AdwDialog` is not a second
//! `xdg_toplevel` to begin with; it is drawn inside its parent's own surface
//! (libadwaita's dialog host, not a second `GdkSurface`), so there is no
//! second Wayland connection here for a helper process to avoid fighting over.
//!
//! **What would change this**, stated the way ADR-011 states its own
//! reversal condition: if WebKit's own process crashes started taking Cordial
//! down with them, that would be a measurement, not a prediction, and nothing
//! here has measured it — recorded in `docs/analysis/webview-surface.md` §5
//! and repeated here so it is not lost if this file is read on its own.
//!
//! What is *not* skipped, and is the one thing mocktail's shape and this one
//! agree on without qualification: **every bridge message is origin-checked**.
//! This window is where a user signs in and where payment happens, so a page
//! able to post arbitrary commands at the engine is the whole security
//! boundary. Every navigation goes through [`crate::webview_policy::evaluate`]
//! before it is allowed, and — see [`open`]'s bridge handler below — every
//! bridge message is checked again, against the page's *current* address, not
//! the one it was granted the bridge for; a page can navigate itself
//! unprivileged after loading privileged, and mocktail's own
//! `IsPrivilegedBridgeAllowed` re-reads `webkit_web_view_get_uri` for exactly
//! that reason. This file does the same, at the same point.

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;
use webkit6::prelude::*;

use crate::webview_policy;

/// The two message-handler names mocktail's bridge registers, kept identical
/// here so a page written against either app's bridge finds the same handler
/// name present (whether it gets an answer is the origin check, not this).
/// Named in `third_party/mocktail-webview/webview_helper_policy.h`, itself
/// citing `kExecuteRobloxHandler`/`kRobloxWkHybridHandler`.
const BRIDGE_EXECUTE_ROBLOX: &str = "executeRoblox";
const BRIDGE_ROBLOX_WK_HYBRID: &str = "RobloxWKHybrid";

/// How the engine asked for the window to look.
///
/// Field names deliberately match the protocol's own keys — `hideHeader`,
/// `showDomainAsTitle` — because the engine's `getHideHeaderKey()` and friends
/// are what fill them, and a rename here would make the correspondence something
/// a reader has to work out.
#[derive(Debug, Clone, Default)]
pub struct WindowRequest {
    pub url: String,
    pub title: Option<String>,
    pub hide_header: bool,
    pub show_domain_as_title: bool,
    pub back_button_visible: bool,
    /// A validated `.ROBLOSECURITY` value to seed this window's cookie jar
    /// with before the first load, or `None` to open signed out.
    ///
    /// Deliberately not something this module fetches for itself. ADR-012
    /// keeps a Roblox session in the desktop secret service, and the module
    /// that already knows how to ask it — `crate::cookies` plus
    /// `crate::secrets`, both in `cordial-runtime` — is not one this crate can
    /// depend on without a cycle (`cordial-runtime` depends on this crate for
    /// `host_window`). `cordial_runtime::webview::extract_roblosecurity` does
    /// the validation (RFC 6265 `cookie-octet`, a length bound) that has to
    /// happen before a value reaches a `Set-Cookie` header; this field only
    /// carries the already-checked result.
    pub roblox_session_cookie: Option<String>,
}

/// Open a web window for `request`, attached to `parent`.
///
/// Returns the dialog so a later `closeWindow` or `mutateWindow` can find it
/// again; the protocol has both and neither can be served by a window that was
/// opened and forgotten.
///
/// A request whose URL the policy refuses opens nothing and says why. Refusing
/// loudly matters more here than elsewhere: silence is the failure mode this
/// whole module exists to end, and "the window did not open" must never again be
/// indistinguishable from "nobody was listening".
pub fn open(parent: &impl IsA<gtk4::Widget>, request: &WindowRequest) -> Option<adw::Dialog> {
    let policy = webview_policy::evaluate(&request.url);
    if !policy.allowed {
        eprintln!(
            "[webview] refused to open {} (scheme {}, host {})",
            if request.url.len() > 120 { "a very long address" } else { &request.url },
            policy.scheme,
            policy.host,
        );
        return None;
    }

    // Ephemeral, deliberately: a `NetworkSession` built without a data
    // directory keeps its cookie jar in memory and writes nothing to disk.
    // ADR-012 already made Cordial's session store the desktop secret
    // service rather than a file; a `WebKitWebsiteDataManager`'s own cookie
    // jar on top of that would be a second copy of the same secret, on disk,
    // that this project spent a whole ADR getting *out* of a file. A fresh
    // session per `open()` call also settles the per-profile requirement
    // without any extra bookkeeping: Cordial runs one profile per process
    // (ADR-012's lock), so there is never a second window in this process for
    // one session to leak into.
    let network_session = webkit6::NetworkSession::new_ephemeral();
    if let Some(cookie_value) = &request.roblox_session_cookie {
        if let Some(cookie_manager) = network_session.cookie_manager() {
            let mut cookie =
                webkit6::soup::Cookie::new(".ROBLOSECURITY", cookie_value, ".roblox.com", "/", -1);
            cookie.set_secure(true);
            cookie.set_http_only(true);
            // The callback reports failure only, and reports it as a fact
            // with no value in it -- the one thing worth knowing here is
            // whether the session made it into the view, never what it was.
            cookie_manager.add_cookie(&cookie, gtk4::gio::Cancellable::NONE, |result| {
                if let Err(e) = result {
                    eprintln!("[webview] could not seed the session cookie: {e}");
                }
            });
        }
    }

    // Registered unconditionally, whatever `policy` said about the opening
    // url. What matters for the bridge is not the address this window was
    // asked to open with, but the address a message actually arrived from --
    // see the module doc's closing paragraph and mocktail's
    // `IsPrivilegedBridgeAllowed`, which this mirrors. A page that starts
    // privileged and navigates itself away must lose the bridge; a page that
    // starts unprivileged and is somehow later on a Roblox host still may not
    // get it retroactively without this being checked again at the point of
    // use, which is exactly what happens below.
    let user_content = webkit6::UserContentManager::new();
    for handler in [BRIDGE_EXECUTE_ROBLOX, BRIDGE_ROBLOX_WK_HYBRID] {
        if !user_content.register_script_message_handler(handler, None) {
            eprintln!("[webview] could not register the {handler} bridge handler");
        }
    }

    let view = webkit6::WebView::builder()
        .network_session(&network_session)
        .user_content_manager(&user_content)
        .hexpand(true)
        .vexpand(true)
        .build();

    // Not yet wired to the engine -- there is no receiver on the other end
    // yet (`MessageBus.publishRaw` is how a reply would leave this process;
    // see `cordial_runtime::webview` and
    // `docs/analysis/webview-surface.md` §3c), and connecting one is
    // follow-up work, not this change's. Printing a bounded, JSON-only
    // rendering rather than silently accepting the message is deliberate:
    // AGENTS.md's rule against a stub that lies applies here as much as it
    // does to `native/opensles.cpp` -- a page that believes its command was
    // delivered when nothing on this end received it is exactly the lie that
    // rule exists to rule out.
    {
        let bridge_view = view.clone();
        user_content.connect_script_message_received(None, move |_manager, value| {
            // The live address, not the one this window was opened with --
            // see the comment above `user_content`'s construction.
            let current_uri = bridge_view.uri().map(|u| u.to_string()).unwrap_or_default();
            let verdict = webview_policy::evaluate(&current_uri);
            let rendered = value.to_json(0).map(|s| s.to_string()).unwrap_or_default();
            if let Err(reason) = webview_policy::bridge_message_acceptable(&verdict, rendered.len()) {
                eprintln!("[webview] rejected a bridge message (host {}): {reason}", verdict.host);
                return;
            }
            eprintln!(
                "[webview] bridge message received and NOT forwarded to the engine (no receiver \
                 wired yet): {rendered}"
            );
        });
    }

    // The policy is applied again on every navigation, not just the first. A
    // page that is allowed to load may redirect, and the address that matters
    // for the bridge is wherever it ended up rather than where it started.
    view.connect_decide_policy(|_, decision, kind| {
        if kind != webkit6::PolicyDecisionType::NavigationAction {
            return false;
        }
        let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
            return false;
        };
        let uri = nav
            .navigation_action()
            .and_then(|mut a| a.request())
            .and_then(|r| r.uri())
            .map(|u| u.to_string())
            .unwrap_or_default();
        let verdict = webview_policy::evaluate(&uri);
        if verdict.allowed {
            decision.use_();
        } else {
            eprintln!(
                "[webview] blocked navigation to scheme {} host {}",
                verdict.scheme, verdict.host
            );
            decision.ignore();
        }
        true
    });

    let header = adw::HeaderBar::new();
    // `showDomainAsTitle` exists because a user in a payment flow needs to be
    // able to see who they are actually talking to. When the engine asks for it,
    // the host wins over whatever title the page would like to call itself.
    let title = if request.show_domain_as_title {
        policy.host.clone()
    } else {
        request.title.clone().unwrap_or_else(|| policy.host.clone())
    };
    header.set_title_widget(Some(&adw::WindowTitle::new(&title, "")));
    header.set_visible(!request.hide_header);
    header.set_show_start_title_buttons(request.back_button_visible);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&view));

    let dialog = adw::Dialog::new();
    dialog.set_child(Some(&toolbar));
    dialog.set_content_width(900);
    dialog.set_content_height(700);
    dialog.set_title(&title);

    view.load_uri(&request.url);
    dialog.present(Some(parent));
    Some(dialog)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The struct is filled from the engine's own keys, so a field quietly
    /// changing meaning would be a real bug. This only pins the defaults, which
    /// are the conservative ones: no chrome hidden, no domain forced, no back
    /// button, which is what an empty request should produce.
    #[test]
    fn a_default_request_hides_nothing_and_forces_nothing() {
        let r = WindowRequest::default();
        assert!(!r.hide_header);
        assert!(!r.show_domain_as_title);
        assert!(!r.back_button_visible);
        assert!(r.title.is_none());
        // The conservative default for a session, too: open signed out
        // rather than guess at a cookie nobody supplied.
        assert!(r.roblox_session_cookie.is_none());
    }

    /// `open()` itself needs a live GTK/Wayland display to construct a
    /// `WebKitWebView` and is not exercised here for the same reason no test
    /// in this file has ever called it: `cargo test` runs headless, with no
    /// compositor for `gtk::init` to attach to. The two bridge handler names
    /// are pinned on their own, because a typo here silently produces a page
    /// that can call `window.webkit.messageHandlers.executeRoblox` and get
    /// nothing back with no error anywhere -- the exact failure mode this
    /// whole module exists to end.
    #[test]
    fn the_bridge_handler_names_match_mocktails() {
        assert_eq!(BRIDGE_EXECUTE_ROBLOX, "executeRoblox");
        assert_eq!(BRIDGE_ROBLOX_WK_HYBRID, "RobloxWKHybrid");
    }
}
