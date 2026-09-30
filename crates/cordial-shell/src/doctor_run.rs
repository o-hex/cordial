//! The half of the doctor that needs the launcher's own state, and the
//! `cordial --doctor` command.
//!
//! The machine checks live in `cordial_shell::doctor`, because the report
//! screen and this command both show them and the argv path runs before any
//! `GApplication` exists. What is here reads things only this binary holds:
//! where `cordial-run` is, the shell configuration, and which Roblox archive
//! the launcher would use. **It never calls `install::locate`**, which
//! verifies a signature and may extract 116 MB: a report screen that froze for
//! a minute to say whether the engine was present would be worse than the
//! problem it was opened for. It reads the version Cordial recorded, or scans
//! the extracted library, exactly as `diagnostics::roblox` does.

use crate::install::{self, Origin};
use cordial_shell::doctor::{self, check, Check, Inputs, Level};
use std::time::Duration;

/// Every check, in the order a reader wants them: can it run at all, then the
/// Roblox build, then the machine. `offline` skips the one network question,
/// whether a newer build is on offer.
pub fn checks(offline: bool) -> Vec<Check> {
    let config = crate::shell_config::load(&crate::shell_config::path());
    let mut out = vec![loader()];
    out.extend(roblox(&config.roblox, offline));
    out.extend(doctor::machine(&Inputs { gamemode: config.gamemode, probe_vulkan: true }));
    out
}

/// `cordial --doctor [--offline]`. Returns the exit status: 1 only when a
/// check failed, so a script can tell "will not start" from "worth a look".
pub fn run(args: &[String]) -> u8 {
    let offline = args.iter().any(|a| a == "--offline");
    if let Some(other) = args.iter().find(|a| !matches!(a.as_str(), "--offline" | "--doctor")) {
        eprintln!("cordial: unexpected argument {other:?}; --doctor takes only --offline");
        return 2;
    }
    let checks = checks(offline);
    print!("{}", doctor::render(&checks));
    doctor::exit_status(&checks)
}

fn loader() -> Check {
    match crate::launch::loader_path() {
        Ok(path) => check(Level::Ok, format!("cordial-run is at {}", path.display()), ""),
        Err(why) => check(
            Level::Fail,
            "cordial-run, the program that runs Roblox, is missing",
            format!(
                "{why}\nInstall both binaries into one directory, e.g.\n\
                 install -Dm755 target/release/cordial-shell target/release/cordial-run -t ~/.local/bin/"
            ),
        ),
    }
}

fn roblox(configured: &install::RobloxInstall, offline: bool) -> Vec<Check> {
    let Some((apk, origin)) = install::effective_apk(configured) else {
        return vec![check(
            Level::Info,
            "Roblox is not installed yet",
            "Press Download Roblox in the launcher. It fetches a build once, about 230 MB.",
        )];
    };
    if !apk.is_file() {
        return vec![check(
            Level::Fail,
            format!("the Roblox archive {} does not exist", apk.display()),
            match origin {
                Origin::Environment => "CORDIAL_APK points at a file that is not there; unset it or correct it.",
                _ => "Clear the APK on Settings, Roblox to let Cordial find or download a build.",
            },
        )];
    }
    let cache = install::engine_cache();
    let version = cordial_update::cache::recorded_version(&cache)
        .or_else(|| cordial_update::engine::installed_version(&cache));
    let Some(installed) = version else {
        return vec![check(
            Level::Info,
            format!("a Roblox archive is present ({}) and has not been unpacked yet", origin.describe()),
            "The first launch unpacks it, about 116 MB, and checks who signed it.",
        )];
    };
    let mut out = vec![check(Level::Ok, format!("Roblox {installed} ({})", origin.describe()), "")];
    if offline || !origin.updatable() {
        return out;
    }
    match newest_online(Duration::from_secs(5)) {
        Some(newest) if cordial_update::version::is_newer(&newest, &installed) => out.push(check(
            Level::Info,
            format!("Roblox {newest} is available"),
            "Settings, Updates installs it.",
        )),
        Some(_) => out.push(check(Level::Ok, "Roblox is the newest build on offer", "")),
        None => out.push(check(
            Level::Warn,
            "could not reach the download mirror to check for updates",
            "Roblox refuses clients that are too old. If joining fails with an update message, \
             check the connection and update from Settings.",
        )),
    }
    out
}

/// The newest build any source can supply, or `None` if nothing answered in
/// `limit`. The request runs on its own thread, which is left to finish or die
/// with the process if it overruns: `--doctor` is about to exit and the report
/// screen never asks.
fn newest_online(limit: Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(cordial_update::provider::newest_obtainable());
    });
    rx.recv_timeout(limit).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `DESKTOP_ID` lives in the library and `APP_ID` in `main.rs`; the Play
    /// button check compares against the first, so a rename that changed only
    /// the second would report a healthy registration as somebody else's.
    #[test]
    fn the_desktop_id_is_the_application_id() {
        assert_eq!(doctor::DESKTOP_ID, format!("{}.desktop", crate::APP_ID));
    }

    #[test]
    fn an_unknown_argument_is_refused_rather_than_ignored() {
        assert_eq!(run(&["--doctor".into(), "--bogus".into()]), 2);
    }

    #[test]
    fn a_missing_archive_is_a_failure_with_a_fix() {
        if std::env::var_os(install::APK_OVERRIDE).is_some() {
            return; // Takes precedence over the setting this test sets.
        }
        let dir = tempfile::tempdir().unwrap();
        let configured = install::RobloxInstall {
            apk: Some(dir.path().join("not-there.apk")),
            lib_dir: None,
        };
        let out = roblox(&configured, true);
        // `CORDIAL_APK` set in the environment takes precedence over the
        // setting and is reported the same way.
        assert_eq!(out[0].level, Level::Fail, "{out:?}");
        assert!(!out[0].fix.is_empty());
    }
}
