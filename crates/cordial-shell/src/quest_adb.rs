//! Getting the Quest build off the user's own headset, over `adb`.
//!
//! Each step the "Get it from your Quest" pages walk through is one function
//! here, and each answers in a shape the page can say something useful about:
//! no headset, a headset that has not allowed this computer, several devices,
//! Roblox missing, and so on. `adb` itself is behind [`Adb`] so every one of
//! those states is tested with canned output rather than with whatever happens
//! to be plugged in (ADR-053).
//!
//! Read-only on the headset: `devices`, `pm list packages`, `dumpsys package`,
//! `pm path` and `pull`. Nothing is installed, changed or removed on it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The package Roblox installs as, on the Quest as on a phone.
pub const PACKAGE: &str = "com.roblox.client";

/// Meta's own page on turning on developer mode. Checked to exist and to
/// describe the Meta Horizon app's toggle and the developer-organisation
/// requirement, 2026-10-01.
pub const DEVELOPER_MODE_DOCS: &str =
    "https://developers.meta.com/horizon/documentation/native/android/mobile-device-setup/";

/// Something that runs `adb` with arguments: the real one, or a test's.
pub trait Adb: Send + Sync {
    /// Standard output on success, or why it failed.
    fn run(&self, args: &[&str]) -> Result<String, String>;
}

/// The `adb` on `PATH`.
pub struct SystemAdb {
    pub path: PathBuf,
}

impl SystemAdb {
    pub fn find() -> Option<Self> {
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths)
            .map(|d| d.join("adb"))
            .find(|p| p.is_file())
            .map(|path| SystemAdb { path })
    }
}

impl Adb for SystemAdb {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new(&self.path)
            .args(args)
            .output()
            .map_err(|e| format!("could not run adb: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() {
                format!("adb {} failed", args.join(" "))
            } else {
                err
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// One line of `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    /// `device` when usable; `unauthorized` until the headset's prompt is
    /// accepted; `offline`, `no permissions` and others otherwise.
    pub state: String,
    /// `model:` from `-l`, with underscores as spaces: "Quest 3".
    pub model: Option<String>,
}

pub fn parse_devices(out: &str) -> Vec<Device> {
    out.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let serial = words.next()?.to_string();
            let rest: Vec<&str> = words.collect();
            // "no permissions (...)" is two words before the key:value pairs.
            let state: Vec<&str> = rest
                .iter()
                .take_while(|w| !w.contains(':'))
                .copied()
                .collect();
            if state.is_empty() {
                return None;
            }
            let model = rest
                .iter()
                .find_map(|w| w.strip_prefix("model:"))
                .map(|m| m.replace('_', " "));
            Some(Device {
                serial,
                state: state.join(" "),
                model,
            })
        })
        .collect()
}

/// Where the headset is, as far as the "Connect" step cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Headset {
    NotFound,
    /// Connected, and the "Allow USB debugging" prompt has not been accepted.
    Unauthorized(Device),
    /// Connected in some other state: `offline`, `no permissions`.
    NotReady(Device),
    /// More than one usable device and no single Quest among them.
    Several(Vec<Device>),
    Ready(Device),
}

/// `adb devices -l`, read into a [`Headset`]. Starting the adb server is part
/// of the first call and takes a second or two; that is the slow case.
pub fn probe(adb: &dyn Adb) -> Result<Headset, String> {
    Ok(headset_from(parse_devices(&adb.run(&["devices", "-l"])?)))
}

pub fn headset_from(devices: Vec<Device>) -> Headset {
    let ready: Vec<&Device> = devices.iter().filter(|d| d.state == "device").collect();
    match ready.as_slice() {
        [one] => return Headset::Ready((*one).clone()),
        [] => {}
        several => {
            // A phone and a Quest both plugged in is ordinary; pick the Quest
            // when there is exactly one, rather than making somebody unplug.
            let quests: Vec<&&Device> = several
                .iter()
                .filter(|d| d.model.as_deref().is_some_and(|m| m.contains("Quest")))
                .collect();
            if let [one] = quests.as_slice() {
                return Headset::Ready((**one).clone());
            }
            return Headset::Several(several.iter().map(|d| (*d).clone()).collect());
        }
    }
    if let Some(d) = devices.iter().find(|d| d.state == "unauthorized") {
        return Headset::Unauthorized(d.clone());
    }
    match devices.into_iter().next() {
        Some(d) => Headset::NotReady(d),
        None => Headset::NotFound,
    }
}

/// Roblox on the headset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Roblox {
    NotInstalled,
    Installed {
        /// `versionName` from `dumpsys package`, if it said one.
        version: Option<String>,
        /// Every APK `pm path` lists: the base and any splits.
        paths: Vec<String>,
    },
}

/// `pm list packages`, `dumpsys package` and `pm path`, on `serial`.
pub fn roblox(adb: &dyn Adb, serial: &str) -> Result<Roblox, String> {
    let listed = adb.run(&["-s", serial, "shell", "pm", "list", "packages", PACKAGE])?;
    // `pm list packages X` is a substring filter, so `com.roblox.client.vr`
    // would match too; only the exact name counts.
    if !listed
        .lines()
        .any(|l| l.trim() == format!("package:{PACKAGE}"))
    {
        return Ok(Roblox::NotInstalled);
    }
    let version = adb
        .run(&["-s", serial, "shell", "dumpsys", "package", PACKAGE])
        .ok()
        .and_then(|d| parse_version_name(&d));
    let paths = parse_package_paths(&adb.run(&["-s", serial, "shell", "pm", "path", PACKAGE])?);
    if paths.is_empty() {
        return Ok(Roblox::NotInstalled);
    }
    Ok(Roblox::Installed { version, paths })
}

pub fn parse_version_name(dumpsys: &str) -> Option<String> {
    dumpsys
        .lines()
        .find_map(|l| l.trim().strip_prefix("versionName="))
        .map(|v| v.trim().to_string())
}

pub fn parse_package_paths(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.trim().strip_prefix("package:"))
        .map(str::to_string)
        .collect()
}

/// Copy every APK in `paths` into `staging`, under each one's own file name
/// (`base.apk`, `split_config.arm64_v8a.apk`), reporting `(done, of)`.
pub fn pull(
    adb: &dyn Adb,
    serial: &str,
    paths: &[String],
    staging: &Path,
    progress: &dyn Fn(usize, usize),
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let mut local = Vec::new();
    for (i, remote) in paths.iter().enumerate() {
        progress(i, paths.len());
        let name = Path::new(remote)
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| n.ends_with(".apk") && !n.starts_with('.'))
            .ok_or_else(|| format!("the headset listed {remote:?}, which is not an APK"))?;
        let to = staging.join(name);
        adb.run(&["-s", serial, "pull", remote, &to.to_string_lossy()])?;
        local.push(to);
    }
    progress(paths.len(), paths.len());
    Ok(local)
}

/// The headset's Roblox against the one Cordial already holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// Cordial holds no Quest build yet.
    NothingKept,
    /// The headset has a newer Roblox than Cordial's: copy it.
    HeadsetNewer,
    /// The same version. If Roblox has said this version is too old, the
    /// headset needs updating from the Meta Horizon Store first; copying it
    /// again would change nothing.
    Same,
    /// The headset has an older Roblox than Cordial's.
    HeadsetOlder,
    /// The headset did not say its version.
    Unknown,
}

/// Compare `versionName` from the headset (`2.740.927`) with the store's key,
/// read out of the engine (`2.740.0.927`). The two spell one build
/// differently, the same way the mirror's `2.738.1397` and the store's
/// `2.738.0.1397` do (ADR-033), so a zero third component is dropped first.
pub fn freshness(kept: Option<&str>, headset: Option<&str>) -> Freshness {
    let Some(kept) = kept else {
        return Freshness::NothingKept;
    };
    let Some(headset) = headset else {
        return Freshness::Unknown;
    };
    let parts = |v: &str| -> Vec<u64> {
        let mut p: Vec<u64> = v.split('.').map(|c| c.parse().unwrap_or(0)).collect();
        if p.len() == 4 && p[2] == 0 {
            p.remove(2);
        }
        p
    };
    match parts(headset).cmp(&parts(kept)) {
        std::cmp::Ordering::Greater => Freshness::HeadsetNewer,
        std::cmp::Ordering::Equal => Freshness::Same,
        std::cmp::Ordering::Less => Freshness::HeadsetOlder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Canned `adb`: each argument line maps to an output, and `pull` writes
    /// the named file so the import that follows has something to read.
    struct Fake {
        answers: HashMap<String, Result<String, String>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(answers: &[(&str, Result<&str, &str>)]) -> Self {
            Fake {
                answers: answers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.map(str::to_string).map_err(str::to_string)))
                    .collect(),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl Adb for Fake {
        fn run(&self, args: &[&str]) -> Result<String, String> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            if args.get(2) == Some(&"pull") {
                std::fs::write(args[4], b"apk").unwrap();
                return Ok(String::new());
            }
            self.answers
                .get(&line)
                .cloned()
                .unwrap_or_else(|| Err(format!("unexpected: adb {line}")))
        }
    }

    const HEAD: &str = "List of devices attached\n";

    #[test]
    fn no_headset_is_not_found() {
        let adb = Fake::new(&[(
            "devices -l",
            Ok("* daemon started successfully\nList of devices attached\n\n"),
        )]);
        assert_eq!(probe(&adb).unwrap(), Headset::NotFound);
    }

    #[test]
    fn an_unaccepted_prompt_is_unauthorized() {
        let adb = Fake::new(&[(
            "devices -l",
            Ok(&format!(
                "{HEAD}2G0YC000000000  unauthorized usb:1-4 transport_id:3\n"
            )),
        )]);
        match probe(&adb).unwrap() {
            Headset::Unauthorized(d) => assert_eq!(d.serial, "2G0YC000000000"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_ready_quest_is_found_with_its_model() {
        let adb = Fake::new(&[(
            "devices -l",
            Ok(&format!("{HEAD}2G0YC000000000  device usb:1-4 product:eureka model:Quest_3 device:eureka transport_id:3\n")),
        )]);
        match probe(&adb).unwrap() {
            Headset::Ready(d) => assert_eq!(d.model.as_deref(), Some("Quest 3")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn several_devices_pick_the_one_quest_or_ask() {
        let phone = "R58M123 device usb:1-1 product:a52 model:SM_A525F device:a52 transport_id:1\n";
        let quest =
            "2G0YC1 device usb:1-4 product:eureka model:Quest_3 device:eureka transport_id:3\n";
        let other_phone = "ZY22 device usb:1-2 product:x model:Pixel_7 device:p transport_id:2\n";
        match headset_from(parse_devices(&format!("{HEAD}{phone}{quest}"))) {
            Headset::Ready(d) => assert_eq!(d.serial, "2G0YC1"),
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(headset_from(parse_devices(&format!("{HEAD}{phone}{other_phone}"))), Headset::Several(v) if v.len() == 2)
        );
    }

    #[test]
    fn no_permissions_is_not_ready_rather_than_lost() {
        let out = format!("{HEAD}2G0YC1 no permissions (missing udev rules? user is in the plugdev group); see [http://developer.android.com/tools/device.html] usb:1-4 transport_id:5\n");
        match headset_from(parse_devices(&out)) {
            Headset::NotReady(d) => assert!(d.state.starts_with("no permissions"), "{}", d.state),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn roblox_missing_is_said_and_a_lookalike_package_does_not_count() {
        let adb = Fake::new(&[(
            "-s Q shell pm list packages com.roblox.client",
            Ok("package:com.roblox.client.beta\n"),
        )]);
        assert_eq!(roblox(&adb, "Q").unwrap(), Roblox::NotInstalled);
    }

    #[test]
    fn roblox_present_gives_its_version_and_every_apk_and_pull_takes_them_all() {
        let adb = Fake::new(&[
            ("-s Q shell pm list packages com.roblox.client", Ok("package:com.roblox.client\n")),
            ("-s Q shell dumpsys package com.roblox.client", Ok("Packages:\n  Package [com.roblox.client]\n    versionCode=2740927 minSdk=29\n    versionName=2.740.927\n")),
            (
                "-s Q shell pm path com.roblox.client",
                Ok("package:/data/app/~~a==/com.roblox.client-b==/base.apk\npackage:/data/app/~~a==/com.roblox.client-b==/split_config.arm64_v8a.apk\n"),
            ),
        ]);
        let Roblox::Installed { version, paths } = roblox(&adb, "Q").unwrap() else {
            panic!()
        };
        assert_eq!(version.as_deref(), Some("2.740.927"));
        assert_eq!(paths.len(), 2);

        let staging =
            std::env::temp_dir().join(format!("cordial-quest-adb-pull-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        let seen = Mutex::new(Vec::new());
        let local = pull(&adb, "Q", &paths, &staging, &|d, n| {
            seen.lock().unwrap().push((d, n))
        })
        .unwrap();
        let names: Vec<_> = local
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["base.apk", "split_config.arm64_v8a.apk"]);
        assert_eq!(seen.into_inner().unwrap(), [(0, 2), (1, 2), (2, 2)]);
        // Read-only on the headset: nothing but queries and pulls was asked.
        for call in adb.calls.lock().unwrap().iter() {
            assert!(
                [
                    "devices",
                    "shell pm list",
                    "shell dumpsys",
                    "shell pm path",
                    "pull"
                ]
                .iter()
                .any(|ok| call.contains(ok)),
                "{call}"
            );
        }
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn a_path_that_is_not_an_apk_is_refused_before_it_lands_anywhere() {
        let adb = Fake::new(&[]);
        let staging =
            std::env::temp_dir().join(format!("cordial-quest-adb-odd-{}", std::process::id()));
        let err = pull(
            &adb,
            "Q",
            &["/data/app/x/../../.bashrc".into()],
            &staging,
            &|_, _| {},
        )
        .unwrap_err();
        assert!(err.contains("not an APK"), "{err}");
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn the_headset_and_the_store_spell_one_build_two_ways() {
        assert_eq!(
            freshness(Some("2.740.0.927"), Some("2.740.927")),
            Freshness::Same
        );
        assert_eq!(
            freshness(Some("2.740.0.927"), Some("2.742.611")),
            Freshness::HeadsetNewer
        );
        assert_eq!(
            freshness(Some("2.740.0.927"), Some("2.739.900")),
            Freshness::HeadsetOlder
        );
        assert_eq!(freshness(None, Some("2.740.927")), Freshness::NothingKept);
        assert_eq!(freshness(Some("2.740.0.927"), None), Freshness::Unknown);
    }
}
