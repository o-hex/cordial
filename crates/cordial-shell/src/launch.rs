//! Starting the client.
//!
//! The chooser used to print `no launch target wired into the standalone shell
//! yet` and return, which is the same failure as a stub that reports success:
//! the button looked live, nothing happened, and nothing said why. This module
//! is what it calls instead.
//!
//! **A separate process, not a thread.** ADR-012 makes an instance a window and
//! a window a process, and the practical half of that is crash isolation — the
//! engine bringing itself down must not take the launcher with it, because the
//! launcher is how the user gets back. It is also the shape Sober uses: its
//! engine process is separate from `sober_services`, the GTK4/libadwaita one.
//! Note that this is *not* the arrangement ADR-011 rules out; that paragraph is
//! about the engine's `wl_surface` needing to share a connection with the
//! window it is a subsurface of, and here each process builds its own window.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use cordial_shell::profile::Claim;

use crate::install::Build;

/// The binary this shell starts.
///
/// The Flatpak is a split app: `cordial-shell` is what a user starts and
/// `cordial-run` is what runs Roblox. Cargo's standard output layout is kept
/// deliberately for that reason — `target/release/cordial-shell` beside
/// `target/release/cordial-run` in a checkout is the same arrangement as
/// `/app/bin/cordial-shell` beside `/app/bin/cordial-run` in the package.
const LOADER: &str = "cordial-run";

/// How long the client is allowed to run.
///
/// `cordial-run` has no run-until-the-window-closes mode: `--run` is a hard
/// timer and the looper pumps until it expires, so some number has to be
/// chosen here. A day is far past any real session and well short of "never",
/// which matters because the profile claim is released when this process exits
/// — a client left running forever is a profile that can never be opened again
/// without finding and killing it.
///
/// The consequence worth knowing, and it is a real one: closing the engine's
/// window does not end the process today. Until `cordial-run` grows a close
/// path, quitting means the timer or the task manager.
const DEFAULT_RUN_SECONDS: u64 = 86_400;

/// Where `cordial-run` is.
///
/// The sibling of `current_exe`, and deliberately only that. One lookup covers
/// both layouts because both layouts are the same shape, which is the point of
/// keeping cargo's paths: a separate development branch that looked for
/// `target/release/` relative to the working directory would work in a
/// checkout and fail in the Flatpak, and nothing would notice until somebody
/// installed the package. `PATH` is the fallback for a deliberate install
/// elsewhere; there is no baked-in `/app/bin` and no configurable path, because
/// this binary is never separately installed.
pub fn loader_path() -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(sibling) = exe.parent().map(|d| d.join(LOADER)) {
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(LOADER);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(format!(
        "Cordial could not find {LOADER}, which should be installed beside the launcher. \
         This is a broken installation rather than a setting."
    ))
}

/// A client this launcher started.
pub struct Instance {
    child: Child,
    /// Kept so the command can be quoted back at the user if the process dies
    /// immediately — an exit code on its own says nothing about what was run.
    pub command_line: String,
}

impl Instance {
    /// Whether the client is already gone, and with what status.
    ///
    /// Polled a moment after launch rather than waited on. A `cordial-run`
    /// that exits within seconds has failed at load — a missing symbol, an APK
    /// it cannot read — and the launcher has to say so, because the only other
    /// evidence is on a stdout nobody is looking at when the shell was started
    /// from a desktop icon.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

/// Start the client on `build`, holding `claim`'s profile.
///
/// `claim` is consumed and handed to the child: ADR-012's lock belongs to the
/// instance, and the instance is the process being spawned here. See
/// [`Claim::hand_to`].
pub fn spawn(build: &Build, claim: Claim, run_seconds: Option<u64>) -> Result<Instance, String> {
    let loader = loader_path()?;
    let run = run_seconds.unwrap_or(DEFAULT_RUN_SECONDS).to_string();

    let mut command = Command::new(&loader);
    command
        .arg("--lib-dir")
        .arg(&build.lib_dir)
        .arg("--apk")
        .arg(&build.apk)
        // Both are what README's own worked example passes and what every run
        // this project has recorded as working passed. `--host-libc` is marked
        // diagnostic in cordial-run's usage text and dropping it is a separate
        // experiment, not something to fold into wiring up a button.
        .arg("--host-libc")
        .arg("--game-activity")
        .arg("--run")
        .arg(&run);

    // The profile stops being a directory name and starts meaning something
    // here: `CORDIAL_FILES_DIR` is where the engine puts `appData`, its cookie
    // store and its own logs. Without it every profile would share one storage
    // directory and the lock would be guarding nothing.
    //
    // **This is a bridge, and the argument is the destination.** `cordial-run`
    // hardcodes `cordial/instances/default/` off `XDG_DATA_HOME` and takes no
    // `--profile`; it rejects arguments it does not know, so passing one now
    // would fail every launch. What it should take is `--profile <name>`, the
    // profile *name* and nothing else — the owner's decision, and the right
    // one: settings live inside the profile directory and the client reads
    // them from the root it was given, so one argument fixes where everything
    // else lives. Passing settings on the command line would duplicate the
    // source of truth and could not reflect a change made while the client is
    // running, which is exactly what the dynamic DFFlag families exist for.
    // When that argument lands, this line becomes `--profile <name>` and the
    // engine resolves the directory itself.
    command.env("CORDIAL_FILES_DIR", claim.profile_dir().join("data"));

    // ADR-011 makes Wayland the display backend and says X11 "is not developed
    // further", but `cordial-run` still defaults to X11 and takes Wayland only
    // on `CORDIAL_WAYLAND=1` — deliberately, from when that backend was not
    // real yet. It is real now: it is what the engine reached the landing page
    // and a signed-in session on. A launcher that quietly started the
    // superseded backend would mean the window this crate builds, its header
    // bar and its monitor fitting were all bypassed. `backend()` still needs
    // `WAYLAND_DISPLAY` as well, so on a host without a compositor this asks
    // for nothing and X11 is used anyway.
    command.env("CORDIAL_WAYLAND", "1");

    // Inherited rather than captured, so that a shell started from a terminal
    // still narrates the load the way it always has. Nothing here parses it.
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());

    claim.hand_to(&mut command);

    let command_line = describe(&loader, &build.lib_dir, &build.apk, &run);
    let child = command
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}\n\n{command_line}", loader.display()))?;

    // Dropped explicitly rather than left to fall off the end of the function,
    // because the ordering is the whole mechanism: the child now holds the
    // flock through its inherited descriptor, and the launcher must let go or
    // quitting the shell would be the thing that released it.
    drop(claim);

    Ok(Instance { child, command_line })
}

fn describe(loader: &Path, lib_dir: &Path, apk: &Path, run: &str) -> String {
    format!(
        "{} --lib-dir {} --apk {} --host-libc --game-activity --run {run}",
        loader.display(),
        lib_dir.display(),
        apk.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loader_is_looked_for_beside_the_launcher_first() {
        // Under `cargo test` the test binary lives in target/debug/deps, so
        // this asserts the shape of the answer rather than a hit: either a
        // sibling or something on PATH, and a message naming the installation
        // rather than a setting when there is neither.
        match loader_path() {
            Ok(p) => assert!(p.ends_with(LOADER), "{}", p.display()),
            Err(e) => assert!(e.contains("broken installation"), "{e}"),
        }
    }

    #[test]
    fn the_quoted_command_line_is_one_someone_could_retype() {
        // It is shown when the client dies at once, which is the moment a user
        // most needs to be able to run the same thing in a terminal and read
        // what it printed.
        let line = describe(
            Path::new("/app/bin/cordial-run"),
            Path::new("/home/a/.cache/cordial/lib/x86_64"),
            Path::new("/home/a/base.apk"),
            "600",
        );
        assert!(line.contains("--lib-dir /home/a/.cache/cordial/lib/x86_64"), "{line}");
        assert!(line.contains("--apk /home/a/base.apk"), "{line}");
        assert!(line.contains("--run 600"), "{line}");
    }

    /// Everything the chooser row does, minus the click.
    ///
    /// `#[ignore]` because it starts the real 115 MB engine and needs a Roblox
    /// build, neither of which belongs in `cargo test --workspace`. It exists
    /// because the alternative evidence for "the launch button works" is
    /// somebody pressing it, and this project's rule is that a claim is worth
    /// what it was measured with — so the measurable part is written down and
    /// runnable rather than described.
    ///
    ///     cargo test --release --bin cordial-shell -- --ignored --nocapture
    ///
    /// Skips rather than fails when there is no build, and says so: a machine
    /// without one has nothing to disprove.
    #[test]
    #[ignore = "starts the real engine; needs a Roblox build"]
    fn a_launch_really_starts_the_client() {
        use crate::install;
        use cordial_shell::profile;

        // A test binary lives in `target/release/deps`, so `cordial-run` is not
        // its sibling and the production lookup correctly declines to find it.
        // Reaching it through the documented `PATH` fallback keeps that lookup
        // exactly as it ships rather than teaching it about test layouts.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(release) = exe.parent().and_then(|p| p.parent()) {
                let path = std::env::var_os("PATH").unwrap_or_default();
                let mut dirs = vec![release.to_path_buf()];
                dirs.extend(std::env::split_paths(&path));
                std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
            }
        }

        // The cache rather than `temp_dir`: the engine writes its whole asset
        // and shader cache into the profile, which on a distribution where
        // `/tmp` is tmpfs means hundreds of megabytes of RAM. Removed at the
        // end, and named so that a run killed halfway is obvious.
        let root = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("cordial-shell-launch-e2e");
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("CORDIAL_PROFILE_ROOT", &root);

        let build = match install::locate(&install::RobloxInstall::default()) {
            Ok(build) => build,
            Err(e) => {
                println!("no Roblox build on this machine, nothing to prove: {e:?}");
                return;
            }
        };
        println!("build: {} + {}", build.apk.display(), build.lib_dir.display());

        let claim = profile::acquire("e2e").expect("a fresh profile is free");
        let profile_dir = claim.profile_dir().to_path_buf();
        let mut instance = spawn(&build, claim, Some(40)).expect("the client starts");

        // The lock has to have moved to the child. Checked while it is running,
        // because that is the only moment the answer can be wrong.
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(
            profile::acquire("e2e").is_err(),
            "the spawned instance must hold the profile the launcher claimed"
        );

        // Long enough for the engine to get past loading and write something of
        // its own. Its log is the evidence that `CORDIAL_FILES_DIR` took —
        // without it the engine would be writing into the shared default and
        // the profile would be a directory name and nothing more.
        std::thread::sleep(std::time::Duration::from_secs(25));
        assert!(instance.exited().is_none(), "the client must still be up after 27 seconds");

        let logs = profile_dir.join("data/files/appData/logs");
        let wrote = std::fs::read_dir(&logs).map(|d| d.count()).unwrap_or(0);
        assert!(wrote > 0, "the engine wrote nothing to {}", logs.display());
        println!("engine wrote {wrote} log file(s) into {}", logs.display());

        instance.child.kill().ok();
        instance.child.wait().ok();
        let _ = std::fs::remove_dir_all(&root);
    }
}
