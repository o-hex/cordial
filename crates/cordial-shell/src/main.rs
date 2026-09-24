//! `cordial-shell` — the core shell binary.
//!
//! [ADR-002](../../../docs/adr/ADR-002-core-shell-and-ui-handoff.md) draws the
//! line this crate has to stay inside: core owns a window, the chooser that
//! paints at T1, and a minimal settings fallback narrow enough to disable a
//! broken plugin. Everything richer — real settings, themes, plugin-contributed
//! chooser entries, instance management — belongs to the UI plugin that takes
//! over at T3. This binary does not link the plugin host or the engine at all;
//! it is built standalone on purpose, so the window/chooser/settings shape can
//! be proven before either of those exist. See `window.rs` for the seam where
//! the engine's Wayland surface will eventually be embedded.
//!
//! [ADR-011](../../../docs/adr/ADR-011-wayland-and-libadwaita.md) is why this
//! is libadwaita rather than bare GTK: `AdwStyleManager` tracks
//! `org.freedesktop.appearance color-scheme` on its own, live, which is what
//! keeps the area behind the engine's canvas the desktop's actual background
//! colour instead of a flash of white while a resize catches up.

// This binary compiles its own copies of `audio_devices.rs` and
// `root_warning.rs` (see the `mod` lines below) rather than depending on the
// `cordial_shell` lib crate for them, so `[lints] workspace = true`'s
// `unsafe_code = "deny"` applies to this crate root independently of the
// `#![allow(unsafe_code)]` on `lib.rs` -- see that file's comment, and
// [ADR-036](../../../docs/adr/ADR-036-unsafe-is-a-boundary-not-a-convention.md),
// for why cordial-shell carries the allow at all.
#![allow(unsafe_code)]
#![allow(dead_code)]

mod audio_devices;
mod chooser;
mod crash;
mod deep_link;
mod browser_account;
mod diagnostics;
mod download_progress;
mod install;
mod instructions;
mod launch;
mod multi_instance_warning;
mod profile_switcher;
mod refresh_watch;
mod roblox_versions;
mod root_warning;
mod settings;
mod shell_config;
mod updater;
// The window itself needs webkitgtk6.0-devel, which an immutable host does not
// have; the policy beside it needs nothing and is always compiled, because it is
// the part that has to be right and it should be under test everywhere.
#[cfg(feature = "webview")]
mod webview;
mod webview_policy;
mod window;
use cordial_shell::window_state;

/// Guards `CORDIAL_PROFILE_ROOT` across every test in this binary that points
/// it at a scratch directory.
///
/// **Shared rather than one per file, and that distinction is load-bearing.**
/// `profile_switcher.rs` and `launch.rs` each used to keep their own private
/// mutex for this, on the reasonable-looking assumption that a mutex local to
/// a file's own `mod tests` was enough to stop its own tests interleaving.
/// It stops that, and does nothing at all about a *different* file's tests
/// setting the same process-wide variable at the same moment — two locks
/// guarding one variable serialise nothing against each other. Measured, not
/// assumed: adding `launch.rs`'s vpn-gate test surfaced this by actually
/// failing `profile_switcher::tests::the_list_offers_no_profile_that_does_not_exist`
/// on one run out of several, reading back
/// `["CordialTest", "evr_l", "main"]` where only `["alt", "main"]` should have
/// existed — another test's scratch directory, torn into view mid-assertion.
/// It did not reproduce every run, which is exactly the "one-in-three flake"
/// shape `profile.rs`'s own tests already warn about, and exactly why a fix
/// that "seemed to work" on a single clean run would not have been evidence of
/// anything.
#[cfg(test)]
pub(crate) static PROFILE_ROOT_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());


/// Must match `packaging/io.github.luohoa97.Cordial.desktop`'s file name and
/// `StartupWMClass`. GNOME Shell uses the application id to find the desktop
/// entry for window-to-launcher matching; let the two drift and the taskbar
/// icon and startup notification silently stop matching up rather than erroring.
///
/// It is also what makes Cordial single-instance, which the deep-link handler
/// depends on rather than works around: a `GApplication` with a fixed id
/// registers on the session bus, and a second invocation carrying a URL hands
/// that URL to the process already registered and exits, which is why clicking
/// a link on a website wakes the launcher instead of starting a second one.
const APP_ID: &str = "io.github.luohoa97.Cordial";

use std::io::Write;

fn installed_roblox_version() -> Option<String> {
    let engine_dir = cordial_update::install::engine_dir();
    cordial_update::cache::recorded_version(&engine_dir)
        .or_else(|| cordial_update::engine::version_of(&engine_dir.join(cordial_update::engine::LIBRARY)))
}

fn run_cli_install(is_update: bool, force: bool) -> bool {
    let installed = installed_roblox_version();

    if is_update && !force {
        if let Some(ref current) = installed {
            print!("Checking for updates... ");
            let _ = std::io::stdout().flush();
            match cordial_update::provider::newest_obtainable() {
                Some(newest) => {
                    if !cordial_update::version::is_newer(&newest, current) {
                        println!("up to date.");
                        println!("Roblox is already at the latest available version ({current}).");
                        println!("Use 'cordial update --force' to force re-download.");
                        return true;
                    }
                    println!("update available ({current} -> {newest}).");
                }
                None => {
                    println!("provider unreachable.");
                    eprintln!("Warning: could not query update provider. Proceeding with update attempt...");
                }
            }
        }
    } else if !is_update && !force {
        if let Some(ref current) = installed {
            println!("Roblox is already installed (version {current}).");
            println!("Use 'cordial update' to check for updates or 'cordial install --force' to reinstall.");
            return true;
        }
    }

    println!("Connecting to update provider (apkpure)...");
    let cancel = cordial_update::provider::Cancel::new();
    let want = if is_update {
        cordial_update::provider::Want::Newest
    } else {
        cordial_update::provider::Want::Any
    };
    let store = cordial_update::install::Store::live(cordial_shell::profile::all_pinned_versions());

    let result = cordial_update::provider::obtain_and_install(
        None,
        want,
        Some(&store),
        &cancel,
        &mut |p| match p {
            cordial_update::provider::Progress::Asking { provider } => {
                println!("  Querying {provider}...");
            }
            cordial_update::provider::Progress::Fetching { file, done, total } => {
                let msg = if let Some(total) = total {
                    let percent = (done as f64 / total as f64) * 100.0;
                    format!(
                        "  Fetching {file}: {:.1} MB / {:.1} MB ({:.1}%)",
                        done as f64 / 1_000_000.0,
                        total as f64 / 1_000_000.0,
                        percent
                    )
                } else {
                    format!("  Fetching {file}: {:.1} MB", done as f64 / 1_000_000.0)
                };
                print!("\r{msg}\x1b[K");
                let _ = std::io::stdout().flush();
            }
            cordial_update::provider::Progress::Verifying { file } => {
                println!("\n  Verifying signature of {file}...");
            }
        },
    );

    match result {
        Ok((obtained, installed)) => {
            println!("\nInstalled Roblox version {}.", obtained.version.name);
            println!("  APK: {}", installed.base.display());
            println!("  Engine: {}", installed.engine.display());
            true
        }
        Err(e) => {
            eprintln!("\nFailed to install Roblox: {e}");
            false
        }
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--diagnostics" || a == "diagnostics") {
        print!("{}", diagnostics::report());
        return std::process::ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "--help" || a == "-h" || a == "help") {
        println!(
            "cordial {version}\n\
             \n\
             Usage: cordial [COMMAND | ROBLOX-LINK] [OPTIONS]\n\
             \n\
             Commands:\n\
               run [LINK]          Launch Roblox Player directly (default)\n\
               install [--force]   Download, verify and install Roblox APK and engine\n\
               update [--force]    Check for updates and install the newest Roblox build\n\
               diagnostics         Print diagnostic information\n\
               help, -h            Show this help message\n\
             \n\
             Options:\n\
               --force, -f         Force re-download even if already up to date\n\
               --apk <PATH>        Override Roblox APK path\n\
               --profile <NAME>    Roblox profile to use (default: \"default\")\n\
             \n\
             `cordial-run` is the engine loader. Issues: https://github.com/luohoa97/cordial/issues\n\
             \n\
             {notice}",
            version = cordial_shell::version::full(),
            notice = cordial_shell::version::NOTICE,
        );
        return std::process::ExitCode::SUCCESS;
    }

    let force = args.iter().any(|a| a == "--force" || a == "-f");

    if args.iter().any(|a| a == "install" || a == "--install") {
        return if run_cli_install(false, force) {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }

    if args.iter().any(|a| a == "update" || a == "--update") {
        return if run_cli_install(true, force) {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }

    let mut link = None;
    let mut profile = "default".to_string();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "run" {
            continue;
        } else if arg == "--profile" {
            if let Some(p) = iter.next() {
                profile = p;
            }
        } else if arg == "--apk" {
            if let Some(apk_path) = iter.next() {
                std::env::set_var(install::APK_OVERRIDE, apk_path);
            }
        } else if arg.starts_with("roblox-player:")
            || arg.starts_with("roblox:")
            || arg.starts_with("http:")
            || arg.starts_with("https:")
        {
            match deep_link::accept(&arg) {
                Ok(u) => link = Some(u),
                Err(why) => {
                    eprintln!("Warning: ignoring malformed link ({why})");
                }
            }
        } else if !arg.starts_with('-') && link.is_none() {
            match deep_link::accept(&arg) {
                Ok(u) => link = Some(u),
                Err(_) => link = Some(arg),
            }
        }
    }

    cordial_shell::profile::migrate_legacy_layout();
    let config_path = shell_config::path();
    let config = shell_config::load(&config_path);

    let build = match install::locate(&config.roblox) {
        Ok(b) => b,
        Err(install::NotFound::NoBuild) => {
            println!("No Roblox build found.");
            print!("Would you like to download and install Roblox now? [Y/n]: ");
            let _ = std::io::stdout().flush();
            let mut answer = String::new();
            let _ = std::io::stdin().read_line(&mut answer);
            let trimmed = answer.trim();
            if trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("y")
                || trimmed.eq_ignore_ascii_case("yes")
            {
                if !run_cli_install(false, false) {
                    return std::process::ExitCode::FAILURE;
                }
                match install::locate(&config.roblox) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("Failed to locate newly installed build: {e:?}");
                        return std::process::ExitCode::FAILURE;
                    }
                }
            } else {
                eprintln!("Aborted. Run 'cordial install' when you are ready to install Roblox.");
                return std::process::ExitCode::FAILURE;
            }
        }
        Err(install::NotFound::Unusable(why)) => {
            eprintln!("Cannot use Roblox build: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let claim = match cordial_shell::profile::claim_for_instance(&profile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Profile '{profile}' could not be claimed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    println!("Starting Roblox Player (profile: {profile})...");
    let req = launch::LaunchRequest {
        run_seconds: None,
        join_url: link.as_deref(),
        secret_store: None,
    };

    let mut instance = match launch::spawn(&build, claim, req) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Launch failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    match instance.wait() {
        Ok(status) => {
            if let Some(code) = status.code() {
                if code == 0 {
                    std::process::ExitCode::SUCCESS
                } else {
                    std::process::ExitCode::from(code as u8)
                }
            } else {
                std::process::ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("Error waiting for client: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
