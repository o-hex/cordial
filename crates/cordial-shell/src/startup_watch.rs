//! Says what the launcher was doing when its window did not appear.
//!
//! **Why this exists.** Several reports (#38, #88) describe the launcher
//! starting, using a full core and never mapping a window, on COSMIC, KWin and
//! Mango sessions, with nothing on the terminal. A WAYLAND_DEBUG trace from one
//! of them stops right after GTK binds `xdg_wm_base`, which is where a healthy
//! run goes on to build the launcher and create the toplevel. Five nested
//! compositors here (sway, KWin, weston, cage, Hyprland) all mapped it, so the
//! cause is something about those machines, and the only way to learn what is
//! to make the stall describe itself.
//!
//! What it prints, once, if no window has mapped after [`LIMIT`]: the last
//! step `start()` recorded, whether the main thread was burning CPU or
//! waiting (a spin and a block look the same in a backtrace), and the
//! variables that choose a display backend and renderer. It runs on its own
//! thread precisely because the main thread is the one that is stuck.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// How long the launcher may take to map. A cold start on a slow disk is a few
/// seconds; twenty is a stall.
pub const LIMIT: Duration = Duration::from_secs(20);

/// [`LIMIT`], unless `CORDIAL_STARTUP_WATCH_SECS` says otherwise. The override
/// exists so the report can be provoked in a test session without waiting.
pub fn limit() -> Duration {
    limit_from(std::env::var("CORDIAL_STARTUP_WATCH_SECS").ok().as_deref())
}

fn limit_from(value: Option<&str>) -> Duration {
    value.and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs).unwrap_or(LIMIT)
}

static MAPPED: AtomicBool = AtomicBool::new(false);
static STAGE: Mutex<&'static str> = Mutex::new("before the shell started");
static ARMED: OnceLock<()> = OnceLock::new();

/// Record the step the shell has reached. Silent: it only matters if the
/// window never appears.
pub fn stage(name: &'static str) {
    *STAGE.lock().unwrap_or_else(|p| p.into_inner()) = name;
}

/// The window has mapped; the watchdog has nothing to say.
pub fn mapped() {
    MAPPED.store(true, Ordering::Relaxed);
}

pub fn is_mapped() -> bool {
    MAPPED.load(Ordering::Relaxed)
}

/// User plus system CPU ticks out of a `/proc/<pid>/task/<tid>/stat` line.
///
/// The command name sits in parentheses and may itself contain spaces or a
/// closing bracket, so the fields are counted from the last `)`.
pub fn cpu_ticks(stat: &str) -> Option<u64> {
    let rest = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After the name the first field is the state (field 3 of the file), so
    // utime (14) and stime (15) are at 11 and 12.
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

/// The words printed when the window has not appeared. A pure function of what
/// was measured, so the wording is tested rather than discovered in a bug
/// report.
pub fn stalled_report(
    stage: &str,
    waited: Duration,
    busy_percent: Option<u64>,
    env: &[(&str, Option<String>)],
) -> String {
    let busy = match busy_percent {
        Some(p) if p >= 50 => format!("The main thread was {p}% busy over the last second, so it is spinning rather than waiting."),
        Some(p) => format!("The main thread was {p}% busy over the last second, so it is waiting on something rather than spinning."),
        None => "The main thread's CPU use could not be read.".to_string(),
    };
    let vars = env
        .iter()
        .map(|(k, v)| format!("{k}={}", v.as_deref().unwrap_or("(unset)")))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "shell: the launcher window has not appeared after {} s. Last step reached: {stage}. {busy} \
         Environment: {vars}. If you are reporting this, include this paragraph. \
         `GSK_RENDERER=cairo cordial` starts GTK without its GPU renderer and is worth one try.",
        waited.as_secs()
    )
}

/// Start the watchdog. Call from the main thread, once the window is going to
/// be shown; a second call does nothing.
pub fn arm() {
    ARMED.get_or_init(|| {
        // SAFETY: `gettid` takes no arguments and cannot fail.
        let main_tid = unsafe { libc::syscall(libc::SYS_gettid) } as i64;
        let spawned = std::thread::Builder::new().name("startup-watch".into()).spawn(move || watch(main_tid));
        if let Err(e) = spawned {
            eprintln!("shell: could not start the startup watchdog: {e}");
        }
    });
}

fn ticks_of(tid: i64) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat")).ok()?;
    cpu_ticks(&stat)
}

fn watch(main_tid: i64) {
    let step = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    let limit = limit();
    while waited < limit {
        if is_mapped() {
            return;
        }
        std::thread::sleep(step);
        waited += step;
    }
    // SAFETY: `sysconf` with a valid name has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    let before = ticks_of(main_tid);
    std::thread::sleep(Duration::from_secs(1));
    if is_mapped() {
        return;
    }
    let busy = match (before, ticks_of(main_tid)) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a) * 100 / hz),
        _ => None,
    };
    let stage = *STAGE.lock().unwrap_or_else(|p| p.into_inner());
    let env: Vec<(&str, Option<String>)> = ["GDK_BACKEND", "GSK_RENDERER", "WAYLAND_DISPLAY", "DISPLAY", "XDG_SESSION_TYPE"]
        .into_iter()
        .map(|k| (k, std::env::var(k).ok()))
        .collect();
    eprintln!("{}", stalled_report(stage, waited + Duration::from_secs(1), busy, &env));
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real line, command name with a space and a bracket in it, which is
    // what a naive split on whitespace gets wrong.
    const STAT: &str = "4242 (cordial (shell)) R 1 4242 4242 0 -1 4194560 100 0 0 0 750 250 0 0 20 0 50 0 12345 0 0";

    #[test]
    fn ticks_are_counted_from_the_last_bracket() {
        assert_eq!(cpu_ticks(STAT), Some(1000));
    }

    #[test]
    fn a_truncated_line_is_none_rather_than_a_guess() {
        assert_eq!(cpu_ticks("4242 (x) R 1 2"), None);
        assert_eq!(cpu_ticks("garbage"), None);
    }

    #[test]
    fn the_report_names_the_stage_and_says_spinning_when_busy() {
        let r = stalled_report("building the window", Duration::from_secs(21), Some(99), &[("GDK_BACKEND", None)]);
        assert!(r.contains("building the window"), "{r}");
        assert!(r.contains("99% busy") && r.contains("spinning"), "{r}");
        assert!(r.contains("GDK_BACKEND=(unset)"), "{r}");
        assert!(r.contains("GSK_RENDERER=cairo"), "{r}");
    }

    #[test]
    fn a_quiet_main_thread_is_called_waiting() {
        let r = stalled_report("x", Duration::from_secs(21), Some(1), &[]);
        assert!(r.contains("waiting on something"), "{r}");
        assert!(!r.contains("spinning rather"), "{r}");
    }

    #[test]
    fn the_limit_is_twenty_seconds_unless_overridden() {
        assert_eq!(limit_from(None), LIMIT);
        assert_eq!(limit_from(Some(" 3 ")), Duration::from_secs(3));
        assert_eq!(limit_from(Some("soon")), LIMIT);
    }

    #[test]
    fn stage_is_recorded_and_mapped_silences_it() {
        stage("reading settings");
        assert_eq!(*STAGE.lock().unwrap(), "reading settings");
        mapped();
        assert!(is_mapped());
    }
}
