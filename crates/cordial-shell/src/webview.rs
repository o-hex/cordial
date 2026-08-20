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
//! **This choice held up, but needed a partner fix to actually be visible.**
//! An `AdwDialog` "draws inside its parent's own surface" — true, and still
//! true below — but that surface is the same `wl_surface` the engine's own
//! canvas is a `wl_subsurface` of, per ADR-011, and a subsurface's default
//! stacking is *above* its parent. So a dialog opened here rendered correctly
//! from the moment this file was written, and was invisible from the same
//! moment, painted into a surface the engine was compositing over every
//! frame — reported as "the whole window goes white/blank", which reads as
//! this window failing to open at all rather than as a stacking order one
//! layer up from it. `crates/cordial-runtime/src/android/wayland.rs`'s
//! `WaylandWindow::webview_dialog_opened`/`webview_dialog_closed` is the fix:
//! it lowers the engine's subsurface behind `parent_surface` for as long as a
//! dialog from [`open`] is up. Nothing in *this* file changed for it, because
//! the stacking is entirely a property of the engine's own subsurface, which
//! this crate has no handle to — see that module's own doc for the mechanism
//! and why a nested-compositor screenshot missed it the first time.
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

/// Where a policy-approved bridge message goes once it has passed
/// [`webview_policy::bridge_message_acceptable`] against the page's *current*
/// address.
///
/// This crate cannot call into `cordial_runtime` directly to reach the
/// engine's JNI natives — `cordial-runtime` depends on `cordial-shell` for
/// `host_window`, not the reverse, and adding the opposite edge would be a
/// cycle. The presenter side of this same problem (`cordial_runtime::webview
/// ::set_presenter`) solved it with a callback installed once at startup;
/// this is that shape, in the other direction. `load.rs`'s
/// `install_webview_presenter` installs a sink that forwards straight to
/// `cordial_runtime::webview::forward_bridge_message`, which is where the
/// actual `WebViewProtocol.signalJavascriptCallback` call lives — see that
/// function's doc for the native, its declared signature, and what remains
/// unestablished about what happens on the engine's side of it.
type BridgeSink = dyn Fn(&str) + Send + Sync;
static BRIDGE_SINK: std::sync::OnceLock<std::sync::Arc<BridgeSink>> = std::sync::OnceLock::new();

/// Install the sink [`open`]'s bridge handler forwards an approved message
/// to. Only the first call takes effect — see `cordial_runtime::webview::
/// set_presenter`'s doc for why two installations racing is a bug worth
/// seeing rather than one that resolves itself silently.
pub fn set_bridge_sink(f: impl Fn(&str) + Send + Sync + 'static) {
    if BRIDGE_SINK.set(std::sync::Arc::new(f)).is_err() {
        eprintln!("[webview] set_bridge_sink called twice; keeping the first sink installed");
    }
}

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
    /// The `User-Agent` this window's `WebKitWebView` should send, or `None`
    /// to leave WebKitGTK's own default in place.
    ///
    /// Why this exists at all: with no User-Agent set, WebKitGTK sends its
    /// own, and roblox.com — reading a browser UA rather than the app's own
    /// — serves the full desktop site, complete with the site's own
    /// navigation bar (Search, Charts, Marketplace, Create, Robux, avatar,
    /// notifications) stacked above whatever page the engine asked this
    /// window to open. On Android the same URL renders the embedded in-app
    /// layout, because the app sends its own User-Agent and the site branches
    /// on it. Cordial already computes that string for the engine's own HTTP
    /// client — see `InitParams.userAgent`, built by `native/init_params.cpp`'s
    /// `build_user_agent` — and `cordial_runtime::webview::user_agent` hands
    /// the identical bytes back out, so this field is never a second,
    /// independently-typed copy of that string. Whether the site's own
    /// navigation bar actually disappears once this is set is the thing this
    /// change expects but a maintainer with a live window needs to confirm —
    /// see this crate's caller for what was and was not run.
    pub user_agent: Option<String>,
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

    // **Both bridge shapes, because the page is an Android app's page and
    // WebKitGTK only offers the WebKit one.**
    //
    // `register_script_message_handler` above exposes these as
    // `window.webkit.messageHandlers.<name>.postMessage(...)`. That is the
    // shape WKWebView gives an iOS app -- `RobloxWKHybrid` is named for it --
    // and it is the only shape WebKitGTK has. An Android WebView instead
    // injects a *named object* through `addJavascriptInterface`, which the page
    // reaches as `window.executeRoblox.someMethod(...)`.
    //
    // `executeRoblox` is a real string in the shipping dex, so the name is
    // right. What was missing is the object: a page probing for the Android
    // shape found nothing, concluded it was not inside an app, and fell back to
    // ordinary navigation. Reported as "pressing Join in the Servers window
    // opens the game's detail page instead of joining", which read as a bridge
    // that was not delivering when in fact nothing ever called it.
    //
    // The shim is a `Proxy`, not a fixed method list, because **nothing here
    // knows which methods the page calls.** Enumerating guesses would be a
    // stub that lies about what it accepts; a proxy forwards whatever arrives
    // and says so. `method` travels with the payload so the receiving side can
    // tell `join` from anything else once somebody has observed one.
    //
    // It runs in the page's own world, not an isolated one -- unavoidable,
    // since the point is for the page's own scripts to find it. That is why
    // `webview_policy::bridge_message_acceptable` is re-checked against the
    // *live* uri on every message rather than trusted from here.
    //
    // **Never log the payload**, only that a call happened and its method name.
    // A bridge command carries whatever the page and the engine are
    // mid-conversation about; `docs/analysis/webview-surface.md` §4's rule
    // about one-time tickets applies to this direction too.
    let shim = format!(
        r#"(function () {{
  var post = function (name, method, args) {{
    try {{
      var h = window.webkit && window.webkit.messageHandlers
              && window.webkit.messageHandlers[name];
      if (!h) {{ return undefined; }}
      return h.postMessage({{ bridge: name, method: method, args: args }});
    }} catch (e) {{ return undefined; }}
  }};
  ['{a}', '{b}'].forEach(function (name) {{
    if (window[name]) {{ return; }}
    try {{
      window[name] = new Proxy({{}}, {{
        get: function (_t, prop) {{
          if (typeof prop !== 'string') {{ return undefined; }}
          return function () {{
            return post(name, prop, Array.prototype.slice.call(arguments));
          }};
        }}
      }});
    }} catch (e) {{ /* no Proxy: leave the WebKit shape as the only one */ }}
  }});
}})();"#,
        a = BRIDGE_EXECUTE_ROBLOX,
        b = BRIDGE_ROBLOX_WK_HYBRID,
    );
    user_content.add_script(&webkit6::UserScript::new(
        &shim,
        webkit6::UserContentInjectedFrames::TopFrame,
        webkit6::UserScriptInjectionTime::Start,
        &[],
        &[],
    ));

    // With no User-Agent set, WebKitGTK sends its own, and roblox.com reads
    // that as an ordinary desktop browser rather than the app it is -- and
    // serves the full desktop site, complete with the site's own navigation
    // bar (Search, Charts, Marketplace, Create, Robux, avatar, notifications)
    // stacked above whatever page the engine actually asked this window to
    // open. See `WindowRequest::user_agent`'s own doc for where the string
    // comes from; `webkit_settings_set_user_agent` (via `Settings::builder`)
    // is the API this crate has for the equivalent of Android's
    // `WebSettings.setUserAgentString`. Built as its own `Settings` and
    // handed to the builder below, rather than fetched from the view and
    // mutated afterwards, so the very first navigation -- `load_uri`, at the
    // end of this function -- already carries it; nothing here has
    // established whether an in-flight load's headers can be changed after
    // the fact, and there is no reason to find out when the ordering below
    // never needs to.
    let settings = webkit6::Settings::new();
    if let Some(ua) = &request.user_agent {
        settings.set_user_agent(Some(ua));
    }

    let view = webkit6::WebView::builder()
        .network_session(&network_session)
        .user_content_manager(&user_content)
        .settings(&settings)
        .hexpand(true)
        .vexpand(true)
        .build();

    // **Corrected**: this used to say nothing here forwards to the engine,
    // with the missing receiver left as follow-up work. That silence is what
    // made "pressing Join navigates to the game's detail page instead of
    // joining" look like a WebKit bug rather than what it was -- the page's
    // JS calling `executeRoblox.postMessage`/`RobloxWKHybrid`, getting no
    // answer, and falling back to plain navigation. [`set_bridge_sink`]
    // above is the receiver; `load.rs`'s `install_webview_presenter` wires it
    // to `cordial_runtime::webview::forward_bridge_message`, which calls
    // `WebViewProtocol.signalJavascriptCallback` -- see that function's own
    // doc for the native and what calling it does and does not establish.
    // AGENTS.md's rule against a stub that lies is still the reason a
    // message that fails policy, or arrives with no sink installed, is
    // reported rather than silently dropped: a page that believes its
    // command was delivered when nothing on this end received it is exactly
    // the lie that rule exists to rule out.
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
            // Never the payload itself, here or in anything this is handed
            // to -- a bridge message can carry whatever the page and the
            // engine are mid-conversation about, which is exactly the kind
            // of session-scoped value `extract_roblosecurity` already
            // refuses to print. Length and the fact that it passed policy
            // are the whole of what is worth a log line.
            match BRIDGE_SINK.get() {
                Some(sink) => sink(&rendered),
                None => eprintln!(
                    "[webview] a bridge message passed policy (host {}, {} bytes), but no sink is \
                     installed to forward it -- see set_bridge_sink's doc",
                    verdict.host,
                    rendered.len()
                ),
            }
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

    // Whether this WebKitGTK build has WebAuthn at all, asked of a live page,
    // once per process. A Roblox account with a passkey enrolled cannot finish
    // sign-in without `navigator.credentials.get`, and when the binding is
    // absent the site's passkey button throws `undefined is not an object`
    // somewhere inside minified JS, in a window with no console and no error
    // path back to Cordial -- the silent shape this whole module exists to end.
    //
    // Asked rather than read off the library, because `ENABLE_WEB_AUTHN` is a
    // property of whichever libwebkitgtk the machine happens to carry and
    // upstream's default answers only for upstream. What was measured, and the
    // reason it is not a passing detail, is in
    // `docs/analysis/webview-surface.md` section 9.
    //
    // Nothing is polyfilled here and nothing should be. A shim standing in for
    // `navigator.credentials.get` would be a stub that lies: the page would
    // carry on believing an authenticator had been asked and had refused,
    // when nothing was asked at all. Reporting the gap leaves it where
    // somebody can find it, which is the whole of `native/opensles.cpp`'s
    // reasoning applied one layer up.
    view.connect_load_changed(|v, event| {
        if event != webkit6::LoadEvent::Finished {
            return;
        }
        static PROBED: std::sync::Once = std::sync::Once::new();
        PROBED.call_once(|| {
            // Two type names and a boolean, and deliberately nothing else. A
            // bare `typeof` cannot carry a session cookie, a one-time ticket
            // or an address, which matters because the page this runs on is
            // usually the sign-in page.
            const PROBE: &str = "[typeof PublicKeyCredential, typeof navigator.credentials, \
                String(window.isSecureContext)].join(' ')";
            // Run in a named script world rather than the page's own. A world
            // sees the real IDL bindings but not properties the page defined,
            // so `window.PublicKeyCredential = function () {}` in the page
            // cannot talk this diagnostic into reporting a capability the
            // build does not have. The distinction costs one argument and the
            // page in question is a sign-in page.
            const WORLD: &str = "cordial-capability-probe";
            v.evaluate_javascript(PROBE, Some(WORLD), None, gtk4::gio::Cancellable::NONE, |r| {
                match r {
                    Ok(value) => {
                        let answer = value.to_str();
                        if answer.starts_with("undefined") {
                            eprintln!(
                                "[webview] this WebKitGTK build has no WebAuthn \
                                 (PublicKeyCredential/navigator.credentials/isSecureContext = \
                                 {answer}) -- a passkey sign-in cannot complete in this window; \
                                 see docs/analysis/webview-surface.md section 9"
                            );
                        } else {
                            eprintln!("[webview] WebAuthn is present in this build ({answer})");
                        }
                    }
                    Err(e) => eprintln!("[webview] could not ask the page about WebAuthn: {e}"),
                }
            });
        });
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
        // Same reasoning: no invented User-Agent, only one actually computed
        // and threaded through by the caller.
        assert!(r.user_agent.is_none());
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
