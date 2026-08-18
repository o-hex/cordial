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
//! mocktail runs its web view in a separate process with its own `main()`, for
//! isolation. That is sound and Cordial does not copy it, because **WebKitGTK is
//! already multi-process**: a `WebKitWebView` is a widget, and the page itself
//! runs in `WebKitWebProcess` and `WebKitNetworkProcess` under WebKit's own
//! sandbox. A hand-rolled helper adds a second IPC layer for isolation that is
//! already there, maintained by people who do it full time — and it costs the
//! attached dialog, because a separate process cannot be one.
//!
//! What is *not* skipped is the origin check. This window is where a user signs
//! in and where payment happens, so a page able to post arbitrary commands at
//! the engine is the whole security boundary. Every navigation goes through
//! [`crate::webview_policy::evaluate`] before it is allowed.

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;
use webkit6::prelude::*;

use crate::webview_policy;

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

    let view = webkit6::WebView::new();
    view.set_vexpand(true);
    view.set_hexpand(true);

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
    }
}
