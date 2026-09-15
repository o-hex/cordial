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
//!
//! **A popup is not exempt from any of this.** Until this file also answered
//! WebKit's `create` signal (see [`install_popup_handling`]), a page that
//! opened a second window with `window.open()` got one with no bridge, no
//! navigation policy and no log line -- which is this module's own opening
//! sentence, describing a gap in this file rather than in the engine. Every
//! popup now gets the identical registration, policy check and origin
//! re-check a top-level window does, via the same functions [`open`] itself
//! calls.

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

#[derive(Clone, Copy)]
enum BridgeHandler {
    ExecuteRoblox,
    RobloxWkHybrid,
}

impl BridgeHandler {
    /// The registered handler name this variant answers for, so a rejection
    /// log can say which of the two contracts a malformed message arrived
    /// on without printing the message itself.
    fn name(self) -> &'static str {
        match self {
            BridgeHandler::ExecuteRoblox => BRIDGE_EXECUTE_ROBLOX,
            BridgeHandler::RobloxWkHybrid => BRIDGE_ROBLOX_WK_HYBRID,
        }
    }
}

/// A one-word JS type name for `value`, for the rejection log below -- never
/// the value itself, which may be whatever the page and the engine are
/// mid-conversation about. `object` covers the case a page posted an object
/// with no `command` string property, which reads identically to an
/// `is_object()` failure in the log unless something distinguishes them; this
/// does not distinguish that further because doing so would mean walking the
/// object's own properties, and this function's whole job is to say what
/// arrived without looking at what it says.
fn describe_value_shape(value: &webkit6::javascriptcore::Value) -> &'static str {
    if value.is_undefined() {
        "undefined"
    } else if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "a boolean"
    } else if value.is_number() {
        "a number"
    } else if value.is_string() {
        "a string"
    } else if value.is_array() {
        "an array"
    } else if value.is_function() {
        "a function"
    } else if value.is_object() {
        "an object"
    } else {
        "an unrecognised JS value"
    }
}

fn forward_script_message(
    view: &webkit6::WebView,
    value: &webkit6::javascriptcore::Value,
    handler: BridgeHandler,
) {
    // Match mocktail's two distinct handler contracts. executeRoblox posts an
    // object whose complete JSON representation is the command; RobloxWKHybrid
    // posts an envelope and the native receives only its string `command`.
    // Treating both signals as the former was subtle: both handlers existed,
    // the callback ran, and the engine still received the wrong bytes for Join.
    let command = match handler {
        BridgeHandler::ExecuteRoblox if value.is_object() => {
            value.to_json(0).map(|s| s.to_string())
        }
        BridgeHandler::RobloxWkHybrid if value.is_object() => value
            .object_get_property("command")
            .filter(|property| property.is_string())
            .and_then(|property| property.to_string_as_bytes())
            .and_then(|bytes| std::str::from_utf8(bytes.as_ref()).ok().map(str::to_owned)),
        _ => None,
    };
    // Named by handler and shaped by JS type, rather than a bare "malformed":
    // this is the one place that distinguishes "a message arrived on this
    // handler but was not the shape expected" from the silence a page
    // opening a *second* window produces instead (see
    // `install_popup_handling`'s doc) -- two failure modes that otherwise
    // both read as "pressing the button did nothing". The type name is
    // logged, never the value -- a JS type says nothing about what the page
    // and the engine were mid-conversation about.
    let Some(command) = command.filter(|command| !command.is_empty()) else {
        eprintln!(
            "[webview] rejected a malformed {} bridge command (arrived as {})",
            handler.name(),
            describe_value_shape(value),
        );
        return;
    };

    let current_uri = view.uri().map(|u| u.to_string()).unwrap_or_default();
    let verdict = webview_policy::evaluate(&current_uri);
    if let Err(reason) = webview_policy::bridge_message_acceptable(&verdict, command.len()) {
        eprintln!("[webview] rejected a bridge message (host {}): {reason}", verdict.host);
        return;
    }
    match BRIDGE_SINK.get() {
        Some(sink) => sink(&command),
        None => eprintln!(
            "[webview] a bridge message passed policy (host {}, {} bytes), but no sink is \
             installed to forward it -- see set_bridge_sink's doc",
            verdict.host,
            command.len()
        ),
    }
}

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
    /// Deliberately not something this module fetches for itself. The runtime
    /// owns the live cookie jar; the shared secret backend only knows its saved
    /// form. `cordial_runtime::webview::extract_roblosecurity` does
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
pub fn open(parent: &(impl IsA<gtk4::Widget> + Clone), request: &WindowRequest) -> Option<adw::Dialog> {
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
    //
    // Done through [`register_bridge`] rather than inline, because a page
    // that opens a second window from inside this one -- `window.open()`, or
    // a `target="_blank"` link -- needs the identical registration on that
    // window too. See [`install_popup_handling`]'s doc for the gap that used
    // to leave a popup with none of this at all.
    let user_content = webkit6::UserContentManager::new();
    register_bridge(&user_content);

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
    wire_bridge_messages(&user_content, &view);

    // The policy is applied again on every navigation, not just the first. A
    // page that is allowed to load may redirect, and the address that matters
    // for the bridge is wherever it ended up rather than where it started.
    install_navigation_policy(&view);

    // **A page opening a second window got none of the above at all, until
    // now.** `window.open()` and a `target="_blank"` link both fire WebKit's
    // `create` signal, and an unanswered `create` returns nothing by
    // WebKitGTK's own default -- no view, no error, nothing reaching this
    // process to log. See [`install_popup_handling`]'s doc for why this was
    // never exercised: the only window this module used to build was the one
    // `openWindow` itself asked for, and nothing inside that page had ever
    // been observed to open a second one.
    // `Clone::clone(parent)`, not `parent.clone()` -- `parent` is already a
    // reference, and `&T` is unconditionally `Clone` in its own right (a
    // trivial pointer copy), so `.clone()` on it resolves to *that* impl
    // before ever considering `T`'s. The explicit trait form pins `Self = T`
    // from the argument type instead, giving the owned widget `upcast` needs.
    install_popup_handling(&view, Clone::clone(parent).upcast(), request.user_agent.clone());

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

            // **`CORDIAL_WEBVIEW_BRIDGE_TEST=1`: make the page call the bridge,
            // so the JS-to-engine direction can be seen working.**
            //
            // Everything else about this window is observable from outside --
            // the dialog opens or it does not, the page renders or it does not.
            // The bridge is the one part that is invisible until a real Roblox
            // page decides to use it, which needs signed-in UI and a click
            // AGENTS.md's rule against synthesising input rules out. So there
            // was no way to tell a working bridge from a broken one except by
            // waiting for somebody to press Join and report back.
            //
            // This skips the page's own decision to call the bridge and skips
            // nothing else: the message goes through the same handler, the same
            // origin and size policy, the same sink and the same
            // `signalJavascriptCallback`. A run with this set prints
            // `forwarded a bridge message` from `cordial_runtime::webview` if
            // and only if the whole chain works.
            //
            // The page's own world, deliberately, unlike the probe above: the
            // bridge global is injected there and a named world would not see
            // it -- which would make this report "absent" on a perfectly
            // healthy build.
            if std::env::var_os("CORDIAL_WEBVIEW_BRIDGE_TEST").is_some() {
                const SELF_TEST: &str = r#"(() => {
  const bridge = window.__globalRobloxAndroidBridge__;
  if (!bridge || typeof bridge.executeRoblox !== "function") { return "no bridge on this page"; }
  bridge.executeRoblox(JSON.stringify({ cordial: "bridge-self-test" }));
  return "called";
})()"#;
                v.evaluate_javascript(
                    SELF_TEST,
                    None,
                    None,
                    gtk4::gio::Cancellable::NONE,
                    |r| match r {
                        Ok(value) => eprintln!(
                            "[webview] bridge self-test: {} (a `forwarded a bridge message` line \
                             below is the round trip; its absence is the finding)",
                            value.to_str()
                        ),
                        Err(e) => eprintln!("[webview] bridge self-test could not run: {e}"),
                    },
                );
            }
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

/// The script every view this module builds needs, so `window.__globalRobloxAndroidBridge__`
/// is present in a popup exactly as it is in [`open`]'s own top-level window.
///
/// **The global the page actually looks for is `__globalRobloxAndroidBridge__`.**
///
/// `register_script_message_handler` (in [`register_bridge`]) exposes the handler as
/// `window.webkit.messageHandlers.executeRoblox.postMessage(...)`, which is
/// WebKitGTK's only shape. The page never touches that directly. It looks
/// for a global object holding an `executeRoblox` **function that takes a
/// JSON string**, and calls it as
///
///     window.__globalRobloxAndroidBridge__.executeRoblox(jsonString)
///
/// Two earlier attempts missed for different reasons and both are worth
/// recording. The first shipped the message handler alone and nothing ever
/// called it, which read as a bridge failing to deliver when in fact the
/// page had concluded it was not inside an app and used an ordinary link --
/// reported as "pressing Join opens the game's detail page instead of
/// joining". The second guessed the Android `addJavascriptInterface` shape,
/// `window.executeRoblox.someMethod(args)`, and was wrong in both the name
/// and the calling convention: it is one function taking a string, not an
/// object with methods.
///
/// The shape is adapted from mocktail (`src/webview/webview_helper_policy.cc`,
/// Apache-2.0), which is the third implementation this project has learned
/// the platform's own vocabulary from rather than inferring it. Guessing at
/// a JS calling convention is exactly as unreliable as guessing at a JNI
/// descriptor, and this file now has two instances of each.
///
/// `writable: false, configurable: false` so a page-defined object can never
/// replace the native bridge, and the whole thing is a no-op when the
/// handler is absent rather than installing a global that silently swallows
/// calls. Policy is still re-checked against the live uri on every message,
/// because this necessarily runs in the page's own world.
///
/// **Never log the payload** -- it carries whatever the page and the engine
/// are mid-conversation about.
/// The early return below (`if (!handler) return;`) never retries, which
/// looks like a race waiting to happen: if `window.webkit.messageHandlers`
/// were not populated when a document-start script runs, the bridge would
/// silently never install.
///
/// **It cannot happen, and the reason is structural rather than lucky.**
/// `WebKitWebView`'s `user-content-manager` property is construct-only, so
/// a manager cannot be attached after the view exists -- handlers and
/// scripts are necessarily registered before there is any document to run
/// them in. Checked in `WebKit-6.0.gir`; this file's own ordering
/// (handlers, then script, then `WebView::builder()`, then `load_uri`)
/// satisfies it regardless.
///
/// Recorded because it is an appealing theory that costs a day: it explains
/// an intermittent symptom neatly and is wrong.
fn bridge_shim() -> String {
    format!(
        r#"(() => {{
  "use strict";
  const handlers = window.webkit && window.webkit.messageHandlers;
  const handler = handlers && handlers.{a};
  if (!handler) {{ return; }}
  const bridge = {{}};
  Object.defineProperty(bridge, "{a}", {{
    value: (query) => handler.postMessage(JSON.parse(query)),
    enumerable: true, writable: false, configurable: false
  }});
  try {{
    Object.defineProperty(window, "__globalRobloxAndroidBridge__", {{
      value: bridge, enumerable: true, writable: false, configurable: false
    }});
  }} catch (_) {{ /* a page may not replace the native bridge */ }}
}})();"#,
        a = BRIDGE_EXECUTE_ROBLOX,
    )
}

/// Register the two bridge handler names and inject [`bridge_shim`] into
/// `user_content`. Shared between [`open`]'s own top-level view and
/// [`create_popup_view`], because a popup with no bridge handlers registered
/// is a popup a Roblox sign-in or payment flow cannot talk to -- the same
/// silent gap this whole module exists to end, one level down.
fn register_bridge(user_content: &webkit6::UserContentManager) {
    for handler in [BRIDGE_EXECUTE_ROBLOX, BRIDGE_ROBLOX_WK_HYBRID] {
        if !user_content.register_script_message_handler(handler, None) {
            eprintln!("[webview] could not register the {handler} bridge handler");
        }
    }

    // **`TopFrame` is load-bearing, not a leftover.** It pairs with the origin
    // check in `forward_script_message`, and changing one without the other
    // opens a hole. Written down because the pairing is invisible from either
    // end and the obvious "fix" for a bridge-in-an-iframe bug is to flip this
    // constant.
    //
    // Measured on this machine with `cargo run -p cordial-shell --example
    // frame_scope_probe`, against a page with one same-origin `srcdoc` child:
    //
    //     TopFrame   -> ["top"]
    //     AllFrames  -> ["top", "nested"]
    //
    // So a nested frame does receive the script under `AllFrames`, and it can
    // reach `window.webkit.messageHandlers` -- the handler is registered per
    // script *world*, not per frame. `AllFrames` is in fact WebKitGTK's
    // documented default; this is a deliberate narrowing away from it.
    //
    // The narrowing is what makes the policy check correct. Messages arrive
    // through `script-message-received`, which does not say which frame sent
    // them, and `forward_script_message` therefore evaluates
    // `webview_policy::evaluate(view.uri())` -- **the top-level document's
    // address**. With `TopFrame` that is the sender's address by construction.
    // With `AllFrames` it would not be: any nested frame, including a
    // third-party one embedded in a roblox.com page, could post a bridge
    // command and have it judged against roblox.com rather than against
    // itself.
    //
    // So if the Join control ever does turn out to live in an iframe, the fix
    // is **not** one constant. It is `AllFrames` plus per-frame origin
    // attribution, and this API does not obviously offer the latter.
    //
    // **That hypothesis is now the less likely of the two live in this file.**
    // Checked directly against mocktail's own `CreateSurface`
    // (`third_party/mocktail-webview/mocktail_webview_helper.cc`), a working
    // reference implementation of this exact button: it also injects its
    // bridge shim with `WEBKIT_USER_CONTENT_INJECT_TOP_FRAME`, not
    // `ALL_FRAMES`. A reference that solves this problem and still narrows to
    // the top frame is evidence the Join control is not hiding in an iframe --
    // see [`install_popup_handling`] for the gap that same reference
    // implementation *does* answer and this file, until now, did not.
    user_content.add_script(&webkit6::UserScript::new(
        &bridge_shim(),
        webkit6::UserContentInjectedFrames::TopFrame,
        webkit6::UserScriptInjectionTime::Start,
        &[],
        &[],
    ));
}

/// Wire `script-message-received` on `user_content` to [`forward_script_message`]
/// for both handler names, reporting bridge messages that arrive on `view`.
/// Split out from [`register_bridge`] because this half needs `view` itself
/// (to know which page a message came from), which does not exist yet when
/// `register_bridge` has to run -- `user-content-manager` is a construct-only
/// property, so the manager and its handlers must be finished before
/// `WebView::builder().build()`, and this half can only run after it.
fn wire_bridge_messages(user_content: &webkit6::UserContentManager, view: &webkit6::WebView) {
    {
        let bridge_view = view.clone();
        user_content.connect_script_message_received(Some(BRIDGE_EXECUTE_ROBLOX), move |_manager, value| {
            forward_script_message(&bridge_view, value, BridgeHandler::ExecuteRoblox);
        });
    }
    {
        let bridge_view = view.clone();
        user_content.connect_script_message_received(Some(BRIDGE_ROBLOX_WK_HYBRID), move |_manager, value| {
            forward_script_message(&bridge_view, value, BridgeHandler::RobloxWkHybrid);
        });
    }
}

/// Approve or refuse a navigation, or a request to open a new window, against
/// [`webview_policy::evaluate`]. Wired onto every view this module builds --
/// [`open`]'s own top-level view and every popup [`create_popup_view`] makes
/// in answer to WebKit's `create` signal -- because a page that redirects
/// itself, or opens `window.open()` at an address the policy would have
/// refused as a first load, must be judged exactly as strictly the second
/// time.
///
/// **`NewWindowAction` used to go unanswered here.** `INFERRED`: WebKitGTK's
/// own documentation for `WebKitWebView::decide-policy` describes an
/// unhandled decision as defaulting to `webkit_policy_decision_use()`, which
/// would mean a blocked-scheme popup was already reaching `create` regardless
/// of what this function did with it -- but nothing in this session ran a
/// build to confirm that default rather than reading it off webkitgtk.org, so
/// treat it as documented rather than measured here. What the checked-in
/// change to this function does not depend on that either way:
/// [`install_popup_handling`] refuses inside `create` itself regardless, and
/// checking the type here too, with its own log line, is the same
/// belt-and-braces this file already applies to bridge messages -- re-checked
/// at the point of use rather than trusted from an earlier decision, because
/// "the window did not open" must never again be indistinguishable from
/// "nobody was listening".
fn install_navigation_policy(view: &webkit6::WebView) {
    view.connect_decide_policy(|_, decision, kind| {
        if kind != webkit6::PolicyDecisionType::NavigationAction
            && kind != webkit6::PolicyDecisionType::NewWindowAction
        {
            return false;
        }
        let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
            return false;
        };
        let uri = nav
            .navigation_action()
            .and_then(|a| a.request())
            .and_then(|r| r.uri())
            .map(|u| u.to_string())
            .unwrap_or_default();
        let verdict = webview_policy::evaluate(&uri);
        if verdict.allowed {
            decision.use_();
        } else {
            let what = if kind == webkit6::PolicyDecisionType::NewWindowAction {
                "opening a new window"
            } else {
                "navigation"
            };
            eprintln!(
                "[webview] blocked {what} to scheme {} host {}",
                verdict.scheme, verdict.host
            );
            decision.ignore();
        }
        true
    });
}

/// Answer WebKit's `create` signal on `view`, fired when a page inside it
/// calls `window.open()` or follows a `target="_blank"` link.
///
/// **Nothing answered this before, and that is a real gap rather than a
/// theory.** This module's own opening line says what an unanswered Android
/// expectation looks like: "no window, no error, no log line". An unanswered
/// `create` is WebKitGTK's own equivalent -- the default return is nothing,
/// so a page that opens a window this way gets exactly that: no view is
/// built, nothing is logged, and the click that triggered it is
/// indistinguishable from one that did nothing at all. That is a plausible
/// account of issue #40 ("pressing Join in the Servers list does nothing")
/// distinct from the iframe hypothesis [`register_bridge`] records and
/// weighs against: it does not require the Join control to be in a nested
/// frame at all, only that whatever it does to join a server opens a second
/// window rather than calling the existing bridge.
///
/// **Still not established which of the two this is, or whether it is
/// either.** Nothing in this session ran a signed-in client to watch which
/// signal actually fires when Join is pressed (AGENTS.md's rule on test
/// accounts). What is established is that this path was entirely unhandled,
/// which on its own is a bug worth fixing regardless of whether it explains
/// that issue: a `broken_feature` exactly as this module's doc describes one.
///
/// Modelled on mocktail's `OnCreatePopup`/`CreateSurface`
/// (`third_party/mocktail-webview/mocktail_webview_helper.cc`, Apache-2.0),
/// which wires `create`, `ready-to-show` and `close` together and reuses its
/// own view-construction function for a popup rather than building one with
/// less on it than a top-level window gets -- the same reuse
/// [`create_popup_view`] does here via [`register_bridge`],
/// [`wire_bridge_messages`] and [`install_navigation_policy`].
///
/// `chrome_parent` is the widget [`open`] itself presents its dialog
/// against, threaded through so a popup presents alongside it. `AdwDialog`
/// has no transient-for requirement of its own -- libadwaita already stacks
/// multiple dialogs presented against the same parent correctly, which is
/// simpler than mocktail's own `gtk_window_set_transient_for` bookkeeping and
/// needs none of it because Cordial's dialogs were never separate
/// `GtkWindow`s to begin with (see this module's own doc on why).
fn install_popup_handling(view: &webkit6::WebView, chrome_parent: gtk4::Widget, user_agent: Option<String>) {
    view.connect_create(move |parent_view, action| {
        let uri = action
            .request()
            .and_then(|r| r.uri())
            .map(|u| u.to_string())
            .unwrap_or_default();
        let verdict = webview_policy::evaluate(&uri);
        eprintln!(
            "[webview] the page asked to open a new window (host {}, scheme {}); {}",
            verdict.host,
            verdict.scheme,
            if verdict.allowed { "creating one" } else { "refusing" },
        );
        if !verdict.allowed {
            return None;
        }
        Some(create_popup_view(parent_view, chrome_parent.clone(), user_agent.clone()).upcast())
    });
}

/// Build the WebKitWebView, dialog chrome and bridge for one popup a page
/// opened with `window.open()`, and present it once WebKit says it is ready.
///
/// `related_view` rather than a fresh `NetworkSession` -- unlike [`open`]'s
/// own ephemeral session, a popup must share the parent's cookie jar to be
/// usable for anything Roblox would open one for (a sign-in step, a payment
/// confirmation). mocktail's `CreateSurface` sets a new view up the same way:
/// `related-view` when a related view exists, `network-session` only when it
/// does not, never both, and asserts afterwards that the result carries the
/// same `NetworkSession` as the app's one shared view in either case -- which
/// is this crate's evidence that `related-view` inherits the session rather
/// than a WebKitGTK API guarantee read off documentation. This crate's own
/// `open()` creates a fresh ephemeral session specifically because Cordial
/// only ever has one web window in flight at a time (see
/// `cordial_runtime::webview::report_window_closed`'s doc); a popup is that
/// same window's child, not a second unrelated one, so it inherits rather
/// than repeating that choice.
///
/// The user agent is still applied explicitly, to the view's own live
/// `Settings` fetched after construction, rather than assumed to be
/// inherited through `related-view` -- mocktail's `CreateSurface` re-applies
/// its settings to every view it builds regardless of relation, which reads
/// as the same caution rather than an oversight, and nothing here has
/// measured whether WebKitGTK actually would inherit them if left unset.
///
/// Not presented immediately: WebKit hands this view back from `create`
/// before it has navigated anywhere or been given a size, and presenting an
/// empty, zero-sized dialog would be worse than the wait. `ready-to-show` is
/// WebKit's own signal that it is safe to, the same point mocktail's
/// `OnReadyToShow` waits for before its equivalent of `present`.
fn create_popup_view(
    parent_view: &webkit6::WebView,
    chrome_parent: gtk4::Widget,
    user_agent: Option<String>,
) -> webkit6::WebView {
    let user_content = webkit6::UserContentManager::new();
    register_bridge(&user_content);

    // No `.settings(&settings)` construct property here, unlike [`open`]'s own
    // view -- mocktail's `CreateSurface` never passes one at construction
    // either, for a related or an unrelated view; it applies its equivalent
    // (`ConfigureWebView`) to `webkit_web_view_get_settings()` afterwards in
    // both cases. Matched here rather than risk an untested interaction
    // between `related-view` and a `settings` construct property this crate
    // never had reason to try before.
    let view = webkit6::WebView::builder()
        .related_view(parent_view)
        .user_content_manager(&user_content)
        .hexpand(true)
        .vexpand(true)
        .build();
    // Fully qualified, because `settings()` is ambiguous on a `WebView`: GTK's
    // `WidgetExt` offers one returning `gtk::Settings` and WebKit's `WebViewExt`
    // offers this one returning `Option<webkit6::Settings>`. Left bare it fails
    // to compile with E0034, and the `Some(..)` pattern below only makes sense
    // for the WebKit one.
    if let (Some(ua), Some(settings)) =
        (&user_agent, webkit6::prelude::WebViewExt::settings(&view))
    {
        settings.set_user_agent(Some(ua));
    }

    wire_bridge_messages(&user_content, &view);
    install_navigation_policy(&view);
    install_popup_handling(&view, chrome_parent.clone(), user_agent);

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&view));

    let dialog = adw::Dialog::new();
    dialog.set_child(Some(&toolbar));
    dialog.set_content_width(900);
    dialog.set_content_height(700);

    {
        let dialog = dialog.clone();
        let shown_view = view.clone();
        view.connect_ready_to_show(move |_| {
            let host =
                webview_policy::evaluate(&shown_view.uri().map(|u| u.to_string()).unwrap_or_default())
                    .host;
            header.set_title_widget(Some(&adw::WindowTitle::new(&host, "")));
            dialog.set_title(&host);
            dialog.present(Some(&chrome_parent));
        });
    }
    // The counterpart to `create`: a page calling `window.close()` on itself.
    // Closing the dialog rather than leaving an unresponsive window up with
    // nothing left in it to show anything.
    {
        let dialog = dialog.clone();
        // `AdwDialog::close` returns whether it accepted the request, which
        // `connect_close`'s own signature has no room for -- WebKit's `close`
        // signal wants a plain reaction, not a verdict.
        view.connect_close(move |_| {
            let _ = dialog.close();
        });
    }

    // **Untested: whether this leaks a closed popup.** `dialog` holds `view`
    // (through `toolbar`), and `view`'s own signal connections above hold
    // `dialog` back -- a reference cycle, if GTK's own teardown of a closed
    // dialog does not break it by dropping the content widget (and with it
    // the signal closures) before the cycle matters. This crate could not
    // build to check, so it is written down rather than assumed away; a weak
    // reference via `glib::clone!` would be the fix if a live run ever shows
    // popup dialogs surviving their own closure.
    view
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

    /// `forward_script_message`'s malformed-command log names a handler by
    /// this, so a typo here would make the log lie about which contract a
    /// bad message arrived on -- pinned the same way the constants above are.
    #[test]
    fn bridge_handler_name_matches_its_registered_handler() {
        assert_eq!(BridgeHandler::ExecuteRoblox.name(), BRIDGE_EXECUTE_ROBLOX);
        assert_eq!(BridgeHandler::RobloxWkHybrid.name(), BRIDGE_ROBLOX_WK_HYBRID);
    }

    /// Pure-function coverage for the one part of [`register_bridge`] that
    /// does not need a live display: the shim text itself still installs the
    /// global the page actually calls, under the handler name WebKitGTK's
    /// `postMessage` is registered against.
    #[test]
    fn the_bridge_shim_installs_the_android_bridge_global() {
        let shim = bridge_shim();
        assert!(shim.contains("__globalRobloxAndroidBridge__"));
        assert!(shim.contains(BRIDGE_EXECUTE_ROBLOX));
    }
}
