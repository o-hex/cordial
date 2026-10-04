//! The half of the doctor that needs the launcher's own state, and the
//! `cordial --doctor` command.
//!
//! The machine checks live in `cordial_shell::doctor`, because the report
//! screen and this command both show them and the argv path runs before any
//! `GApplication` exists. What is here reads things only this binary holds:
//! where `cordial-run` is, the shell configuration, and which Roblox archive
//! the launcher would use. **It never calls `install::resolve`**, which may
//! verify an entry's signature: a report screen that stalled to say whether the
//! engine was present would be worse than the problem it was opened for. It
//! reads the store's entries and what each records, exactly as
//! `diagnostics::roblox` does.

use crate::install;
use cordial_update::store::{self, Entry};
use cordial_shell::doctor::{self, check, Check, Inputs, Level};
use std::time::Duration;

/// Every check, in the order a reader wants them: can it run at all, then the
/// Roblox build, then the machine. `offline` skips the one network question,
/// whether a newer build is on offer.
pub fn checks(offline: bool) -> Vec<Check> {
    let config = crate::shell_config::load(&crate::shell_config::path());
    let mut out = vec![loader()];
    out.extend(roblox(offline));
    out.extend(doctor::machine(&Inputs { gamemode: config.gamemode, probe_vulkan: true }));
    if cordial_shell::vr::HOST_SUPPORTED {
        let setting = config.vr_openxr_runtime.as_deref();
        out.extend(doctor::vr(&cordial_shell::vr::Readiness::gather(setting), setting.is_some()));
    }
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

fn roblox(offline: bool) -> Vec<Check> {
    let (mut out, installed) = store_checks(&store::list(), install::override_apk().as_deref());
    let Some(installed) = installed else { return out };
    if offline || install::updates_blocked().is_some() {
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

/// What the store says, with no network: the checks and the version a profile
/// on Latest would run, if there is one.
fn store_checks(entries: &[Entry], overridden: Option<&std::path::Path>) -> (Vec<Check>, Option<String>) {
    if let Some(apk) = overridden {
        return (
            vec![if apk.is_file() {
                check(
                    Level::Info,
                    format!("CORDIAL_APK names {} for this run", apk.display()),
                    "The store is not used while it is set, and nothing is downloaded for it.",
                )
            } else {
                check(
                    Level::Fail,
                    format!("CORDIAL_APK names {}, which does not exist", apk.display()),
                    "CORDIAL_APK points at a file that is not there; unset it or correct it.",
                )
            }],
            None,
        );
    }
    if entries.is_empty() {
        return (
            vec![check(
                Level::Info,
                "Roblox is not installed yet",
                "Press Download Roblox in the launcher. It fetches a build once, about 230 MB.",
            )],
            None,
        );
    }
    let Some(latest) = store::latest(entries) else {
        return (
            vec![check(
                Level::Fail,
                "Cordial's store holds Roblox builds it cannot run",
                "A build is only run once it holds its own APK and Cordial has checked who signed \
                 it. Download Roblox again from Settings.",
            )],
            None,
        );
    };
    let how = latest.provenance.map(|p| p.label()).unwrap_or_else(|| "source not recorded".into());
    (
        vec![check(Level::Ok, format!("Roblox {} ({how})", latest.version), "")],
        Some(latest.version.clone()),
    )
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

    fn entry(version: &str, launchable: bool) -> Entry {
        Entry {
            version: version.into(),
            dir: std::path::PathBuf::from("/x").join(version),
            loaded_by: None,
            bytes: 0,
            complete: launchable,
            content_hash: None,
            signer: launchable.then(|| "ab".repeat(32)),
            provenance: Some(store::Provenance { source: store::Source::Mirror, at: Some(1_791_081_816) }),
            version_code: None,
        }
    }

    #[test]
    fn an_empty_store_says_to_download_and_a_good_one_names_its_build_and_how_it_came() {
        let (out, installed) = store_checks(&[], None);
        assert_eq!(out[0].level, Level::Info);
        assert!(out[0].fix.contains("Download Roblox"));
        assert_eq!(installed, None);

        let (out, installed) = store_checks(&[entry("2.736.0.1408", true), entry("2.738.0.1397", true)], None);
        assert_eq!(out[0].level, Level::Ok);
        assert!(out[0].what.contains("2.738.0.1397") && out[0].what.contains("Downloaded on 4 Oct 2026"), "{:?}", out[0]);
        assert_eq!(installed.as_deref(), Some("2.738.0.1397"));
    }

    #[test]
    fn a_store_with_nothing_runnable_is_a_failure_with_a_fix() {
        let (out, installed) = store_checks(&[entry("2.738.0.1397", false)], None);
        assert_eq!(out[0].level, Level::Fail);
        assert!(!out[0].fix.is_empty());
        assert_eq!(installed, None);
    }

    #[test]
    fn a_missing_override_is_a_failure_with_a_fix_and_a_present_one_is_only_said() {
        let dir = tempfile::tempdir().unwrap();
        let (out, _) = store_checks(&[], Some(&dir.path().join("not-there.apk")));
        assert_eq!(out[0].level, Level::Fail, "{out:?}");
        assert!(!out[0].fix.is_empty());
        let there = dir.path().join("there.apk");
        std::fs::write(&there, b"x").unwrap();
        let (out, installed) = store_checks(&[entry("2.738.0.1397", true)], Some(&there));
        assert_eq!(out[0].level, Level::Info);
        assert_eq!(installed, None, "the store says nothing about what an override runs");
    }
}
