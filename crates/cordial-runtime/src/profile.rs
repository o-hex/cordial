//! Profiles: one account's storage, and the lock that keeps one window on it.
//!
//! See [ADR-012](../../../docs/adr/ADR-012-profiles-and-instances.md) for the
//! vocabulary, which matters here. An *instance* is a running Cordial process —
//! a window, which is what Roblox means by the word. A *profile* is the
//! directory this module resolves: one account's Roblox storage, plugin set and
//! flag overrides. An instance runs a profile.
//!
//! Nothing structurally prevents two Cordial processes opening the same profile.
//! Unlike Fishstrap on Windows there is no singleton mutex to defeat, because
//! each Cordial process is genuinely independent — which is what makes
//! multi-instance nearly free here. That same freedom is the hazard: two
//! instances on one profile are two processes writing one `appData` and one
//! cookie store, and Roblox's storage is not built for it. The failure does not
//! look like "you did something unsupported"; it looks like Cordial corrupting a
//! login.
//!
//! A profile now holds configuration as well as storage — the user's
//! `flags.json`, `plugin-grants.json`, `plugins/<id>/settings.json` for each
//! plugin that keeps anything, and, since the engine turned out never to write
//! its cookies anywhere, the session itself in `cookies` at `0600`
//! (`cookies.rs`). See
//! [ADR-013](../../../docs/adr/ADR-013-per-profile-configuration.md), which
//! extends ADR-012 and records why grants in particular had to stop being
//! global: a plugin approved in a throwaway profile was silently approved in
//! the profile someone plays on. Only the profile *directory* is decided here;
//! what goes in it is resolved by `flags.rs` and by `cordial_plugins`, both of
//! which take the directory rather than looking it up for themselves.

use std::ffi::c_int;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

/// `LOCK_EX | LOCK_NB` from `sys/file.h`. Non-blocking on purpose: a second
/// launch should say so immediately rather than hang waiting for the first to
/// exit, which reads as the client failing to start.
const LOCK_EX_NB: c_int = 2 | 4;

/// Where profiles live. `$XDG_DATA_HOME/cordial/profiles`, falling back the way
/// the rest of the tree does.
pub fn root() -> PathBuf {
    std::env::var_os("CORDIAL_PROFILE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
                .unwrap_or_else(std::env::temp_dir)
                .join("cordial/profiles")
        })
}

/// The profile an instance runs when it was told nothing else.
///
/// Not arbitrary: `migrate_legacy_layout` lands pre-existing storage here, so
/// picking any other name would present as being logged out.
pub const DEFAULT_NAME: &str = "default";

/// The profile this instance is running, once something has said which.
///
/// One process runs one profile for its whole life — that is ADR-012's
/// definition of an instance, and the `flock` in [`acquire`] is what makes it
/// true rather than a convention — so this is a fact about the process and is
/// recorded once as one.
static ACTIVE: OnceLock<PathBuf> = OnceLock::new();

/// Record which profile this instance runs.
///
/// **The profile arrives as a command-line argument, and everything else lives
/// underneath it.** Flag overrides, plugin grants and each plugin's settings
/// are all resolved from this one directory, so a second argument naming any
/// of them would be a second source of truth for something already decided.
/// Settings in particular must not be passed in on the command line: they are
/// read from the profile while the client runs, and the `DFFlag`/`DFInt`/
/// `DFString` families exist precisely so that a value can change mid-session
/// (ADR-005). An argument is fixed at exec and could never express that.
///
/// Refuses a second, different answer rather than taking it. Changing profile
/// under a running engine would mean two `appData` directories in one session,
/// which is the corruption ADR-012's lock exists to prevent — arriving by a
/// different door.
pub fn set_active(dir: PathBuf) -> Result<(), String> {
    // Create and tighten here as well as in `acquire`, because they are not the
    // same door. The launcher calls `acquire`, which does both; a hand-started
    // `cordial-run --profile <name>` calls only this, and so ran against a
    // directory `create_dir_all` had left at the umask's `0755`. That was
    // survivable while the profile only held Roblox's own storage. It is not
    // now that Cordial writes a session token into it — see `cookies.rs` and
    // ADR-012 — so the mode is applied wherever a profile is chosen, not only
    // where it is locked.
    let _ = std::fs::create_dir_all(&dir);
    restrict_to_owner(&dir);
    match ACTIVE.set(dir.clone()) {
        Ok(()) => Ok(()),
        Err(_) if ACTIVE.get() == Some(&dir) => Ok(()),
        Err(_) => Err(format!(
            "this instance already runs {}; a profile cannot be changed while the client is up",
            ACTIVE.get().expect("set failed, so it is set").display()
        )),
    }
}

/// The profile directory everything else in this process hangs off.
///
/// Falls back to [`DEFAULT_NAME`] for a `cordial-run` started by hand, which
/// has been told no profile and must not therefore write somewhere new — that
/// would look exactly like being logged out.
pub fn active() -> PathBuf {
    ACTIVE.get().cloned().unwrap_or_else(|| root().join(DEFAULT_NAME))
}

/// A profile name that cannot escape the profile root.
///
/// Names reach this from a command line and, later, from the launcher's own UI,
/// so `../` and absolute paths are refused rather than sanitised — quietly
/// rewriting a name would mean the profile a user selected is not the one they
/// get. Same reasoning as the zip-slip defence in `android::asset`.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn dir(name: &str) -> Result<PathBuf, String> {
    if !is_valid_name(name) {
        return Err(format!(
            "{name:?} is not a usable profile name; use letters, digits, - and _"
        ));
    }
    Ok(root().join(name))
}

/// An instance's claim on a profile, released when this is dropped.
///
/// The file is deliberately held open: `flock` is tied to the open description,
/// so closing the handle releases the lock. Storing it here means the lock lives
/// exactly as long as the value, and the process exiting — cleanly or not —
/// releases it, which a lock file containing a PID would not.
#[derive(Debug)]
pub struct Lock {
    _file: File,
    path: PathBuf,
}

impl Lock {
    pub fn profile_dir(&self) -> &Path {
        &self.path
    }
}

/// Take this instance's claim on `name`, creating the profile if it is new.
///
/// Fails immediately if another instance holds it. **Advisory, and honestly so:**
/// `flock` does not stop a process that never asks. It stops Cordial doing this
/// by accident, which is the actual failure mode — someone launching twice, not
/// someone defeating a lock.
pub fn acquire(name: &str) -> Result<Lock, String> {
    let path = dir(name)?;
    std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    restrict_to_owner(&path);

    let lock_path = path.join(".lock");
    let file = File::create(&lock_path).map_err(|e| format!("{}: {e}", lock_path.display()))?;

    // SAFETY: `file` is open for the duration of the call and outlives it.
    if unsafe { flock(file.as_raw_fd(), LOCK_EX_NB) } != 0 {
        return Err(format!(
            "profile {name:?} is already open in another Cordial window; \
             use a different profile, or close that one first"
        ));
    }
    Ok(Lock { _file: file, path })
}

/// Make a profile directory readable only by its owner.
///
/// The profile holds a live session, so `create_dir_all` applying the process
/// umask — `0755` on a normal desktop — would let any other account on the
/// machine take it.
///
/// **The paragraph that used to stand here was wrong, and the correction is the
/// point of this comment.** It said Cordial "never reads or handles" a session
/// token, that the engine reads its cookie from a file at startup, and rejected
/// a keyring partly on that basis. The engine does no such thing: a complete
/// `CORDIAL_TRACE_PATHS=1` inventory of every non-system file it opens contains
/// no cookie jar, and `grep -rl ROBLOSECURITY` over a real profile tree finds
/// nothing. The engine keeps its cookies in memory and expects the Java side of
/// the app to persist them, which on Android it does and under Cordial nothing
/// did — that is the whole of why signing in and restarting presented as being
/// logged out. Cordial now reads the jar out of the engine and writes it here,
/// so it *is* the custodian of a session token. See `cookies.rs` and ADR-012,
/// which records the reversal rather than quietly dropping the old reasoning.
///
/// The keyring is still rejected, but for the reason that survives: the token
/// has to be handed to the engine in plaintext on every launch, so a keyring
/// would encrypt it only while nothing is using it, in exchange for an unlock
/// prompt on every start. Permissions defend against the case that is actually
/// reachable, and the store itself is `0600` inside this `0700` directory.
///
/// Best-effort: a filesystem without Unix permissions is not a reason to refuse
/// to launch, and the failure is reported by the launch continuing rather than
/// by a panic.
fn restrict_to_owner(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.mode() & 0o077 != 0 {
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

/// Move a pre-ADR-012 layout into place, once.
///
/// Storage used to live at `cordial/instances/default/run`, which named a window
/// and contained a login. Renaming without moving would present as being logged
/// out for no reason, which is the class of failure this project keeps a list
/// of, so the directory is moved rather than abandoned.
///
/// Runs only when the old path exists and the new one does not, so it cannot
/// clobber a profile someone has already used.
pub fn migrate_legacy_layout() -> Option<PathBuf> {
    let legacy = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/instances/default");
    let target = root().join("default");
    if !legacy.is_dir() || target.exists() {
        return None;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    match std::fs::rename(&legacy, &target) {
        Ok(()) => {
            // The legacy directory was created with the old umask, so tighten it
            // on the way in rather than inheriting a world-readable cookie.
            restrict_to_owner(&target);
            println!(
                "  profiles: moved {} to {} (ADR-012)",
                legacy.display(),
                target.display()
            );
            Some(target)
        }
        // A cross-device rename is the one plausible failure. Leaving the old
        // directory in place and saying so beats a half-copied login.
        Err(e) => {
            println!(
                "  profiles: could not move {} to {} ({e}); the old location is untouched",
                legacy.display(),
                target.display()
            );
            None
        }
    }
}

/// Profiles that exist, for the launcher's switcher.
pub fn list() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_valid_name(n))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `CORDIAL_PROFILE_ROOT` is process-wide and cargo runs tests in parallel
    /// threads of one process, so two tests pointing it at different scratch
    /// directories will interleave and read each other's. They passed anyway on
    /// the first run, which is exactly how a one-in-three flake gets committed —
    /// this project already has one of those in its history. Serialised instead.
    static ENV: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let p = std::env::temp_dir().join(format!("cordial-profile-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        std::env::set_var("CORDIAL_PROFILE_ROOT", &p);
        (p, guard)
    }

    #[test]
    fn a_name_cannot_escape_the_profile_root() {
        // A profile name reaches this from a command line and from the
        // launcher's UI. Sanitising rather than refusing would mean the profile
        // someone selected is not the one they get.
        assert!(!is_valid_name("../../etc"));
        assert!(!is_valid_name("/absolute"));
        assert!(!is_valid_name("has/slash"));
        assert!(!is_valid_name(""));
        assert!(is_valid_name("default"));
        assert!(is_valid_name("alt_account-2"));
    }

    #[test]
    fn a_second_instance_cannot_take_a_held_profile() {
        // The whole point of ADR-012's lock: two instances on one profile are
        // two processes writing one cookie store, and the corruption that
        // follows presents as a Cordial bug rather than as unsupported use.
        let (_root, _g) = scratch("held");
        let first = acquire("default").expect("first instance takes the profile");
        let second = acquire("default");
        assert!(second.is_err(), "a second instance must be refused");
        assert!(second.unwrap_err().contains("already open"));
        drop(first);
    }

    #[test]
    fn releasing_a_profile_lets_the_next_instance_have_it() {
        // flock is tied to the open file description, so dropping the Lock must
        // actually release it — including when the process died rather than
        // exited, which a PID file would not handle.
        let (_root, _g) = scratch("released");
        let first = acquire("default").unwrap();
        drop(first);
        assert!(acquire("default").is_ok(), "the profile must be reusable");
    }

    #[test]
    fn a_profile_is_not_readable_by_other_users() {
        // Roblox keeps its session cookie in here. create_dir_all applies the
        // umask, which on a normal desktop gives 0755 — another account on the
        // machine could take the session. This is the whole of Cordial's
        // credential protection, so it is worth a test.
        let (_root, _g) = scratch("perms");
        let lock = acquire("default").unwrap();
        let mode = std::fs::metadata(lock.profile_dir())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "profile must not be group- or world-accessible");
    }

    #[test]
    fn the_active_profile_is_decided_once_and_defaults_to_the_migrated_one() {
        // One test rather than three, because `ACTIVE` is a `OnceLock` and the
        // fallback can only be observed before anything has set it. Written as
        // a sequence for that reason, not for brevity.
        let (_root, _g) = scratch("active");
        assert_eq!(
            active(),
            root().join(DEFAULT_NAME),
            "a client told nothing must use the profile the migration lands storage in, \
             or a hand-started run presents as being logged out"
        );

        let chosen = dir("alt_account").unwrap();
        set_active(chosen.clone()).unwrap();
        assert_eq!(active(), chosen);

        // Saying yes to the same answer twice is not a conflict; the launcher
        // and the client both resolving the same argument is ordinary.
        assert!(set_active(chosen.clone()).is_ok());

        // A different answer is. Two profiles in one session means two appData
        // directories, which is the corruption the lock exists to prevent
        // arriving by another door.
        let refused = set_active(dir("main").unwrap());
        assert!(refused.is_err(), "a second, different profile must be refused");
        assert_eq!(active(), chosen, "and the first answer must still stand");
    }

    #[test]
    fn different_profiles_do_not_contend() {
        // Multi-instance is the point: two windows on two profiles is the
        // supported shape and must not block.
        let (_root, _g) = scratch("distinct");
        let a = acquire("main").unwrap();
        let b = acquire("alt").unwrap();
        assert_ne!(a.profile_dir(), b.profile_dir());
    }
}
