//! Does a WebKitGTK user script reach a nested frame, and can that frame use
//! `window.webkit.messageHandlers`?
//!
//! `crates/cordial-shell/src/webview.rs` injects the Roblox bridge shim with
//! `UserContentInjectedFrames::TopFrame`. The gir documents `all_frames` as
//! the *default* and `top_frame` as inserting "only in the top-level frame
//! loaded by the web view, and not in the nested frames" -- so Cordial has
//! deliberately opted out of reaching subframes.
//!
//! The open question is whether that matters, and it has two halves that a
//! reading of the documentation cannot separate:
//!
//! 1. Does the script get injected into a nested frame under `AllFrames`?
//! 2. Is `window.webkit.messageHandlers.<name>` even *present* in a nested
//!    frame? The handler is documented as registered "in script world", with
//!    no frame qualifier -- which suggests yes, and suggesting is not knowing.
//!
//! **If the answer to (2) is no, switching to `AllFrames` is a no-op** and any
//! fix for a bridge-in-an-iframe bug has to be something else entirely. That is
//! why this exists rather than a one-line change: flipping the flag blind would
//! have been indistinguishable from fixing it, right up until it did not.
//!
//! ```bash
//! cargo run -p cordial-shell --example frame_scope_probe
//! ```

use libadwaita as adw;
use libadwaita::gtk;
use libadwaita::prelude::*;
use webkit6::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const HANDLER: &str = "probe";

/// A top frame with one nested frame in it. `srcdoc` keeps the child
/// same-origin, which is the friendly case -- if the bridge cannot reach a
/// same-origin child, it certainly cannot reach a cross-origin one.
const PAGE: &str = r#"<!doctype html><meta charset=utf-8>
<title>frame scope probe</title>
<iframe srcdoc="<!doctype html><meta charset=utf-8><p>nested</p>"></iframe>
<p>top</p>"#;

/// Reports which frame it is in, and whether it can see the handler at all.
///
/// Posting is the only channel back, so a frame that cannot see the handler
/// cannot report its own absence -- which is exactly why this runs twice and
/// compares counts rather than trusting one run to be self-describing.
const SCRIPT: &str = r#"(() => {
  const top = (window.top === window);
  const h = window.webkit && window.webkit.messageHandlers
            && window.webkit.messageHandlers.probe;
  if (!h) { return; }
  h.postMessage(top ? "top" : "nested");
})();"#;

fn probe(frames: webkit6::UserContentInjectedFrames, label: &str) -> Vec<String> {
    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let manager = webkit6::UserContentManager::new();
    // Connect before registering, which is what the gir explicitly recommends
    // to avoid a race between registration and the first message.
    let collected = seen.clone();
    manager.connect_script_message_received(Some(HANDLER), move |_, value| {
        collected.borrow_mut().push(value.to_str().to_string());
    });
    assert!(
        manager.register_script_message_handler(HANDLER, None),
        "the handler must register or the probe proves nothing"
    );
    manager.add_script(&webkit6::UserScript::new(
        SCRIPT,
        frames,
        webkit6::UserScriptInjectionTime::Start,
        &[],
        &[],
    ));

    let view = webkit6::WebView::builder().user_content_manager(&manager).build();
    let window = gtk::Window::builder().default_width(200).default_height(120).child(&view).build();
    window.present();
    view.load_html(PAGE, None);

    // Pump until the loads settle. Crude and adequate: this is a local
    // document with one srcdoc child and nothing to fetch.
    let context = gtk::glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    while std::time::Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    window.close();

    let out = seen.borrow().clone();
    println!("{label:<10} -> {out:?}");
    out
}

fn main() {
    adw::init().expect("libadwaita starts");

    println!("Which frames receive the script, and can they reach the handler?\n");
    let top = probe(webkit6::UserContentInjectedFrames::TopFrame, "TopFrame");
    let all = probe(webkit6::UserContentInjectedFrames::AllFrames, "AllFrames");

    println!();
    println!("TopFrame  frames reporting: {}", top.len());
    println!("AllFrames frames reporting: {}", all.len());
    println!();
    if all.iter().any(|s| s == "nested") {
        println!(
            "ANSWER: a nested frame both receives the script and can reach the\n\
             handler. Cordial's TopFrame choice is therefore what keeps the\n\
             bridge out of iframes, and AllFrames would put it there."
        );
    } else if all.len() == top.len() {
        println!(
            "ANSWER: AllFrames changes nothing here. Either the script does not\n\
             reach the nested frame, or the frame cannot see the handler.\n\
             Switching webview.rs to AllFrames would be a no-op, so an\n\
             iframe-shaped bridge bug needs a different fix."
        );
    } else {
        println!("ANSWER: inconclusive -- counts differ but no nested frame reported.");
    }
}
