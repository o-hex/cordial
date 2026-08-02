//! What the fetcher gets from the outside world, right now, on this machine.
//!
//! ```bash
//! cargo run -p cordial-update --example update_probe
//! ```
//!
//! Every endpoint this crate talks to, asked once, with the answer printed
//! rather than summarised. It exists because ADR-015 accepts that distribution
//! URLs and version endpoints are Roblox's to change without notice, and the
//! first question anyone will have when this stops working is "what does it
//! actually say now" — which should be one command rather than a debugging
//! session. Sober's own habit of checking every launch is the same idea.
//!
//! It writes nothing and downloads nothing. The APK is the one thing this does
//! not fetch, deliberately: a probe that pulls 115 MB is a probe nobody runs
//! twice.

use cordial_update::{changelog, download, http, metered, settings, version};

fn main() {
    println!("== NetworkManager");
    match metered::query() {
        Ok(m) => println!("  Metered = {m} ({})", m.describe()),
        Err(why) => println!("  {why}"),
    }
    let now = metered::current();
    println!("  treated as metered: {}", now.is_metered());
    let s = settings::UpdateSettings::default();
    println!("  default settings would: {:?}", s.plan(now));

    println!("== version endpoint");
    // WindowsPlayer alongside AndroidApp on purpose. Without a control, an
    // AndroidApp failure looks like Cordial being broken; with one, it is
    // visibly the endpoint answering for some platforms and not others.
    for binary_type in [version::ANDROID, "WindowsPlayer"] {
        match version::check_binary_type(binary_type) {
            Ok(v) => println!("  {binary_type}: {} (major {:?}, {})", v.version, v.major(), v.upload),
            Err(e) => println!("  {binary_type}: {e}"),
        }
    }

    println!("== release notes");
    match changelog::latest() {
        Ok(release) => {
            println!("  newest: {} ({})", release.title, release.created_at);
            println!("  {}", release.web_url());
            match changelog::notes(&release) {
                Ok(notes) => {
                    let text = notes.text();
                    println!("  {}", text.lines().take(3).collect::<Vec<_>>().join("\n  "));
                }
                Err(e) => println!("  {e}"),
            }
        }
        Err(e) => println!("  {e}"),
    }

    println!("== roblox's deployment cdn");
    // The claim `Source::official` rests on is that Roblox publishes no Android
    // build, and that claim is worth one request rather than an afternoon. The
    // desktop path is the control in the same run: if both fail, the network is
    // the story, and if only the Android one fails, the absence is real.
    for (what, url) in
        [("desktop", download::DEPLOY_HISTORY), ("android", download::ANDROID_DEPLOY_HISTORY)]
    {
        match http::get_text(url) {
            // The body is hundreds of kilobytes of deployment lines, so this
            // prints what it looked for rather than what it got. `android` and
            // `apk` occurring nowhere in the desktop history is the finding.
            Ok(body) => {
                let lower = body.to_lowercase();
                println!(
                    "  {what}: {url} answered HTTP 200, {} lines, mentions android/apk: {}",
                    body.lines().count(),
                    lower.contains("android") || lower.contains("apk")
                );
            }
            Err(e) => println!("  {what}: {e}"),
        }
    }

    println!("== download source");
    match download::Source::configured() {
        Ok(source) => {
            println!("  {} ({})", source.url, source.hash);
            // Only when asked, and only into a directory the caller named. This
            // is how somebody who has just filled in a distribution URL checks
            // that it streams and hashes as published, without having to wire
            // the whole shell up first.
            match std::env::var_os(INTO) {
                Some(into) => fetch(&source, std::path::Path::new(&into)),
                None => println!("  set {INTO}=<directory> to actually fetch it"),
            }
        }
        Err(e) => println!("  {e}"),
    }
}

const INTO: &str = "CORDIAL_UPDATE_PROBE_INTO";

fn fetch(source: &download::Source, into: &std::path::Path) {
    let mut last = 0u64;
    let mut progress = |so_far: u64, total: Option<u64>| {
        // Every megabyte rather than every buffer, so a 115 MB download does
        // not print five hundred lines.
        if so_far - last >= 1024 * 1024 || Some(so_far) == total {
            last = so_far;
            match total {
                Some(t) => println!("  {so_far} / {t} bytes"),
                None => println!("  {so_far} bytes"),
            }
        }
    };
    match download::fetch(source, into, &mut progress) {
        Ok(path) => println!("  verified and kept at {}", path.display()),
        Err(e) => println!("  {e}"),
    }
}
