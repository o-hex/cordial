//! Keeping the profile's flag overrides in force after the engine's own
//! settings refresh. [ADR-051](../../../docs/adr/ADR-051-overrides-are-reapplied-after-the-engines-refresh.md).
//!
//! The engine runs a `DynamicFastVariableReloader`. It fetches Roblox's settings
//! document again and applies it over whatever is in force, which puts every
//! `DF*` key that document contains back to Roblox's value (`client_settings.rs`,
//! `docs/analysis/flag-init.md` section 47). A flag Cordial merged in at launch
//! therefore holds until the next refresh and then quietly stops: a user who set
//! `DFIntTaskSchedulerTargetFps` saw 144 for a couple of minutes and 60 after.
//!
//! The cure is to hand the engine the same document again, with the overrides
//! merged in, once the refresh has landed. `nativeInitClientSettings` takes a
//! second call, answers `0` and the new values take effect (measured 2026-09-01,
//! `client_settings.rs`), and it is the entry point the engine already exports
//! and Cordial already calls at launch, so nothing new reaches into the process
//! (ADR-001).
//!
//! **When** is the part that was open. The engine says when it has finished, in
//! its own log, which `game_log` already tails from the pump:
//!
//! ```text
//! 120.525764 [FLog::DynamicFastVariableReloader] DynamicFastVariableReloader finished flag fetch. Tombstone status: invalid
//! ```
//!
//! That line closes the refresh (it follows the flag-cache write), and it is in
//! every earlier log that ran past two minutes, signed in or out, at about 120,
//! 241 and 362 seconds. So the re-apply is triggered by it, and not by a timer
//! that has to guess. `game_log::poll` calls [`note_engine_refresh`]; a worker
//! thread does the work, because building the document means reading and
//! re-serialising about 1.3 MB of JSON and the call hands the engine the same
//! amount, and none of that belongs on the pump.
//!
//! **A timer exists only as a net.** If a process has gone [`FALLBACK_AFTER`]
//! without ever seeing the refresh line, the line is evidently not reaching us
//! (a future build logging it differently, an unreadable log directory), and the
//! worker re-applies on that interval instead. One refresh line seen switches
//! the net off for good: a profile that is offline would otherwise repeat a
//! pointless call every interval, but the trade is one call per [`FALLBACK_AFTER`]
//! and only for a client whose trigger has never fired.
//!
//! **With nothing to keep, nothing happens.** The check is made when the
//! trigger fires, against the flags as they are then, so a flag added to
//! `flags.json` while the client runs is kept from the next refresh on.

use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::flags::{Resolved, Source};

/// How long after the refresh line to wait before re-applying. The line is
/// written once the reloader has finished, so this is slack for the engine to be
/// done with its own bookkeeping and for a burst of lines to coalesce, not a
/// delay the mechanism depends on.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// The shortest gap between two re-applies, whatever asks. Bounds the log to a
/// line every couple of seconds at worst and the engine to one document per gap
/// if somebody drags a live setting across a combo row.
pub const MIN_GAP: Duration = Duration::from_secs(2);

/// How long a client that has never seen the refresh line goes between
/// re-applies. The refresh has been measured at 120.1 to 121.5 s apart across
/// logs, so 150 s is past it with room to spare.
pub const FALLBACK_AFTER: Duration = Duration::from_secs(150);

/// Whether the resolved flags include anything worth keeping in force.
///
/// "Worth keeping" is any Roblox flag a layer above Cordial's own built-in
/// default asked for: the user's file, a plugin, the performance mode, the frame
/// rate limit. The built-in default is an `FFlag`, the family the reloader does
/// not revert, and a client that carries nothing else has no override to defend.
/// Cordial's own keys (`Cordial...`) are never sent to the engine, so they are
/// not overrides either.
///
/// Every family counts, not just `DF*`. Only the dynamic family is measured to
/// be reverted, so a profile whose overrides are all `FFlag`s pays for a call it
/// may not need; that was chosen over teaching this function which names the
/// reloader touches, a list that would have to be right on every Roblox build.
pub fn needs_reapply(resolved: &BTreeMap<String, Resolved>) -> bool {
    resolved
        .iter()
        .any(|(key, r)| crate::client_settings::is_roblox_flag(key) && r.source != Source::Builtin)
}

/// Why a re-apply is happening, for the log line and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The engine's own log said its settings refresh finished.
    Refresh,
    /// A live setting changed what is wanted, so the engine is told now.
    Requested,
    /// No refresh line has ever been seen and the interval ran out.
    Fallback,
}

impl Reason {
    fn describe(self) -> &'static str {
        match self {
            Reason::Refresh => "engine settings refresh seen",
            Reason::Requested => "live change",
            Reason::Fallback => "no refresh line seen, timer",
        }
    }
}

/// When to re-apply, as a pure state machine so the timing is testable without
/// a clock or an engine. The caller supplies `now`.
#[derive(Debug, Clone)]
pub struct Trigger {
    debounce: Duration,
    min_gap: Duration,
    fallback: Duration,
    started: Instant,
    refresh_pending: Option<Instant>,
    requested: bool,
    refreshes_seen: u64,
    last_applied: Option<Instant>,
}

impl Trigger {
    pub fn new(now: Instant) -> Self {
        Self::with_timing(now, DEBOUNCE, MIN_GAP, FALLBACK_AFTER)
    }

    pub fn with_timing(now: Instant, debounce: Duration, min_gap: Duration, fallback: Duration) -> Self {
        Trigger {
            debounce,
            min_gap,
            fallback,
            started: now,
            refresh_pending: None,
            requested: false,
            refreshes_seen: 0,
            last_applied: None,
        }
    }

    /// The engine's log reported a finished refresh.
    pub fn refresh_seen(&mut self, now: Instant) {
        self.refreshes_seen += 1;
        self.refresh_pending.get_or_insert(now);
    }

    /// Something changed what is wanted and the engine should hear it at once.
    pub fn request(&mut self) {
        self.requested = true;
    }

    /// Whether, and why, a re-apply is due at `now`.
    pub fn due(&self, now: Instant) -> Option<Reason> {
        if let Some(last) = self.last_applied {
            if now.saturating_duration_since(last) < self.min_gap {
                return None;
            }
        }
        if self.requested {
            return Some(Reason::Requested);
        }
        if let Some(seen) = self.refresh_pending {
            return (now.saturating_duration_since(seen) >= self.debounce).then_some(Reason::Refresh);
        }
        if self.refreshes_seen == 0 {
            let since = self.last_applied.unwrap_or(self.started);
            if now.saturating_duration_since(since) >= self.fallback {
                return Some(Reason::Fallback);
            }
        }
        None
    }

    /// The re-apply is starting. Clears what asked for it, so a refresh that
    /// lands while it runs is still pending afterwards and gets its own.
    pub fn begin(&mut self, now: Instant) {
        self.requested = false;
        self.refresh_pending = None;
        self.last_applied = Some(now);
    }

    /// The next moment [`Trigger::due`] could change its answer, or `None` if
    /// only a new event can.
    pub fn next_deadline(&self) -> Option<Instant> {
        let gap_end = self.last_applied.map(|l| l + self.min_gap);
        let later = |t: Instant| gap_end.map_or(t, |g| g.max(t));
        if self.requested {
            return Some(later(self.started));
        }
        if let Some(seen) = self.refresh_pending {
            return Some(later(seen + self.debounce));
        }
        if self.refreshes_seen == 0 {
            return Some(later(self.last_applied.unwrap_or(self.started) + self.fallback));
        }
        None
    }
}

/// What a worker needs from the outside, as closures so a test can supply an
/// engine that is not there.
pub struct Hooks {
    /// Hands the engine a settings document; the result is its return code.
    pub call: Box<dyn Fn(&str) -> Result<i32, String> + Send>,
    /// Builds the document to hand over, or says there is nothing to keep.
    pub document: Box<dyn Fn(bool) -> Result<Option<crate::client_settings::Reapply>, String> + Send>,
}

struct Shared {
    trigger: Mutex<Trigger>,
    wake: Condvar,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared { trigger: Mutex::new(Trigger::new(Instant::now())), wake: Condvar::new() })
}

/// `CORDIAL_NO_FLAG_REDELIVERY=1` turns the mechanism off. Not a setting: it
/// exists so a measurement can show the flag reverting with the re-apply
/// withdrawn, in the same build, which is the control this fix needs. The name
/// is the one a contributor's pull request (73) gave the same switch.
pub fn enabled() -> bool {
    !std::env::var_os("CORDIAL_NO_FLAG_REDELIVERY").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Called from the pump when the engine's log reports a finished refresh. One
/// lock and a notify; the work happens on the worker.
pub fn note_engine_refresh() {
    let s = shared();
    s.trigger.lock().unwrap_or_else(|e| e.into_inner()).refresh_seen(Instant::now());
    s.wake.notify_all();
}

/// Asks for a re-apply now, because a live setting changed what is wanted.
pub fn request_apply() {
    let s = shared();
    s.trigger.lock().unwrap_or_else(|e| e.into_inner()).request();
    s.wake.notify_all();
}

/// Starts the worker. Called once, after the launch-time `nativeInitClientSettings`
/// has been answered; that call counts as the first apply, so the fallback timer
/// runs from here and a live request made before now is not repeated.
pub fn start(hooks: Hooks) {
    if !enabled() {
        println!("  [reapply] CORDIAL_NO_FLAG_REDELIVERY is set: overrides are not re-applied after the engine's settings refresh");
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        shared().trigger.lock().unwrap_or_else(|e| e.into_inner()).begin(Instant::now());
        std::thread::Builder::new()
            .name("flag-reapply".into())
            .spawn(move || worker(shared(), hooks))
            .expect("the re-apply worker thread starts");
    });
}

fn worker(s: &'static Shared, hooks: Hooks) {
    loop {
        let reason = {
            let mut t = s.trigger.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let now = Instant::now();
                if let Some(reason) = t.due(now) {
                    t.begin(now);
                    break reason;
                }
                t = match t.next_deadline() {
                    Some(at) => s
                        .wake
                        .wait_timeout(t, at.saturating_duration_since(now).max(Duration::from_millis(1)))
                        .unwrap_or_else(|e| e.into_inner())
                        .0,
                    None => s.wake.wait(t).unwrap_or_else(|e| e.into_inner()),
                };
            }
        };
        run_once(reason, &hooks);
    }
}

/// One re-apply, with its cost printed. One line when it happened, none when
/// there was nothing to keep.
fn run_once(reason: Reason, hooks: &Hooks) {
    let doc = match (hooks.document)(reason == Reason::Requested) {
        Ok(Some(doc)) => doc,
        Ok(None) => return,
        Err(why) => {
            println!("  [reapply] {}: not re-applied: {why}", reason.describe());
            return;
        }
    };
    let t = Instant::now();
    let outcome = (hooks.call)(&doc.body);
    let call = t.elapsed();
    let result = match outcome {
        Ok(code) => format!("nativeInitClientSettings -> {code}"),
        Err(e) => format!("nativeInitClientSettings failed: {e}"),
    };
    println!(
        "  [reapply] {}: {result} ({} override(s), {} bytes; base {}, load {} ms, merge {} ms, call {} ms)",
        reason.describe(),
        doc.overrides,
        doc.body.len(),
        doc.source,
        doc.load.as_millis(),
        doc.merge.as_millis(),
        call.as_millis(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::{resolve, Layer};

    fn layer(source: Source, pairs: &[(&str, &str)]) -> Layer {
        Layer {
            source,
            values: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    // ---- the decision: is there anything to keep? ----------------------------

    #[test]
    fn a_profile_with_no_overrides_has_nothing_to_reapply() {
        assert!(!needs_reapply(&resolve(vec![])));
    }

    #[test]
    fn cordials_own_default_is_not_an_override() {
        let r = resolve(vec![layer(Source::Builtin, &[("FFlagUserLaunchedWithBloxstrap", "True")])]);
        assert!(!needs_reapply(&r));
    }

    #[test]
    fn a_cordial_key_is_never_sent_so_it_is_not_one_either() {
        let r = resolve(vec![layer(Source::User, &[("CordialFrameRateLimit", "144")])]);
        assert!(!needs_reapply(&r));
    }

    #[test]
    fn any_user_flag_is_worth_keeping_not_just_the_frame_rate() {
        for key in ["DFIntTaskSchedulerTargetFps", "DFFlagDebugPauseVoxelizer", "DFLogHttpTrace", "FFlagFoo"] {
            let r = resolve(vec![
                layer(Source::Builtin, &[("FFlagUserLaunchedWithBloxstrap", "True")]),
                layer(Source::User, &[(key, "1")]),
            ]);
            assert!(needs_reapply(&r), "{key}");
        }
    }

    #[test]
    fn a_plugin_or_a_chosen_mode_counts_the_same_as_the_users_file() {
        for source in [
            Source::Plugin("hide-gui".into()),
            Source::Performance,
            Source::FrameRateLimit,
        ] {
            let r = resolve(vec![layer(source.clone(), &[("DFIntTaskSchedulerTargetFps", "144")])]);
            assert!(needs_reapply(&r), "{source:?}");
        }
    }

    #[test]
    fn a_user_value_over_the_builtin_one_still_counts() {
        let r = resolve(vec![
            layer(Source::Builtin, &[("FFlagUserLaunchedWithBloxstrap", "True")]),
            layer(Source::User, &[("FFlagUserLaunchedWithBloxstrap", "False")]),
        ]);
        assert!(needs_reapply(&r), "the winner's source is the user, not the default it replaced");
    }

    // ---- the trigger ---------------------------------------------------------

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn trigger(base: Instant) -> Trigger {
        Trigger::with_timing(
            base,
            Duration::from_millis(250),
            Duration::from_secs(2),
            Duration::from_secs(150),
        )
    }

    #[test]
    fn nothing_is_due_until_something_happens() {
        let t0 = Instant::now();
        let t = trigger(t0);
        assert_eq!(t.due(at(t0, 0)), None);
        assert_eq!(t.due(at(t0, 120_000)), None, "no timer fires before the fallback interval");
    }

    #[test]
    fn a_refresh_is_acted_on_after_the_debounce_and_not_before() {
        let t0 = Instant::now();
        let mut t = trigger(t0);
        t.refresh_seen(at(t0, 120_500));
        assert_eq!(t.due(at(t0, 120_600)), None);
        assert_eq!(t.due(at(t0, 120_750)), Some(Reason::Refresh));
    }

    #[test]
    fn a_burst_of_refresh_lines_is_one_apply() {
        let t0 = Instant::now();
        let mut t = trigger(t0);
        t.refresh_seen(at(t0, 120_000));
        t.refresh_seen(at(t0, 120_100));
        t.begin(at(t0, 120_250));
        assert_eq!(t.due(at(t0, 125_000)), None, "begin consumed both");
    }

    #[test]
    fn a_refresh_that_lands_during_an_apply_gets_its_own() {
        let t0 = Instant::now();
        let mut t = trigger(t0);
        t.refresh_seen(at(t0, 1_000));
        t.begin(at(t0, 1_250));
        t.refresh_seen(at(t0, 1_300));
        assert_eq!(t.due(at(t0, 1_600)), None, "inside the minimum gap");
        assert_eq!(t.due(at(t0, 3_300)), Some(Reason::Refresh));
    }

    #[test]
    fn applies_are_never_closer_than_the_minimum_gap() {
        let t0 = Instant::now();
        let mut t = trigger(t0);
        t.begin(at(t0, 10_000));
        t.request();
        assert_eq!(t.due(at(t0, 10_500)), None);
        assert_eq!(t.due(at(t0, 11_999)), None);
        assert_eq!(t.due(at(t0, 12_000)), Some(Reason::Requested));
    }

    #[test]
    fn a_live_request_is_immediate_and_wins_over_a_pending_refresh() {
        let t0 = Instant::now();
        let mut t = trigger(t0);
        t.refresh_seen(at(t0, 5_000));
        t.request();
        assert_eq!(t.due(at(t0, 5_000)), Some(Reason::Requested));
        t.begin(at(t0, 5_000));
        assert_eq!(t.due(at(t0, 60_000)), None, "the request also consumed the refresh");
    }

    #[test]
    fn the_timer_fires_only_for_a_client_that_has_never_seen_a_refresh() {
        let t0 = Instant::now();
        let mut t = trigger(t0);
        assert_eq!(t.due(at(t0, 149_999)), None);
        assert_eq!(t.due(at(t0, 150_000)), Some(Reason::Fallback));
        t.begin(at(t0, 150_000));
        assert_eq!(t.due(at(t0, 299_999)), None);
        assert_eq!(t.due(at(t0, 300_000)), Some(Reason::Fallback), "and it repeats");

        // One refresh line proves the trigger works; the net comes down.
        let mut t = trigger(t0);
        t.refresh_seen(at(t0, 120_500));
        t.begin(at(t0, 120_750));
        assert_eq!(t.due(at(t0, 900_000)), None);
    }

    #[test]
    fn the_next_deadline_follows_what_is_pending() {
        let t0 = Instant::now();
        let mut t = trigger(t0);
        assert_eq!(t.next_deadline(), Some(at(t0, 150_000)), "the net, for a client with no evidence");
        t.refresh_seen(at(t0, 1_000));
        assert_eq!(t.next_deadline(), Some(at(t0, 1_250)));
        t.begin(at(t0, 1_250));
        assert_eq!(t.next_deadline(), None, "a client whose trigger works waits for the next line");
        t.request();
        assert_eq!(t.next_deadline(), Some(at(t0, 3_250)), "a request waits out the gap");
    }

    // ---- the worker's one apply ---------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn doc(overrides: usize) -> crate::client_settings::Reapply {
        crate::client_settings::Reapply {
            body: "{\"applicationSettings\":{}}".into(),
            overrides,
            source: crate::client_settings::Source::EngineDocumentCached,
            load: Duration::from_millis(1),
            merge: Duration::from_millis(2),
        }
    }

    #[test]
    fn an_apply_hands_the_engine_the_document_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let hooks = Hooks {
            call: Box::new(move |body| {
                assert!(body.contains("applicationSettings"));
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }),
            document: Box::new(|_| Ok(Some(doc(3)))),
        };
        run_once(Reason::Refresh, &hooks);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn with_nothing_to_keep_the_engine_is_not_called() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let hooks = Hooks {
            call: Box::new(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }),
            document: Box::new(|_| Ok(None)),
        };
        run_once(Reason::Refresh, &hooks);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_document_that_could_not_be_built_is_reported_and_not_sent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let hooks = Hooks {
            call: Box::new(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }),
            document: Box::new(|_| Err("no base document".into())),
        };
        run_once(Reason::Fallback, &hooks);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn only_a_live_request_forces_a_document_when_nothing_is_overridden() {
        let forced = Arc::new(Mutex::new(Vec::new()));
        let log = forced.clone();
        let hooks = Hooks {
            call: Box::new(|_| Ok(0)),
            document: Box::new(move |force| {
                log.lock().unwrap().push(force);
                Ok(None)
            }),
        };
        run_once(Reason::Refresh, &hooks);
        run_once(Reason::Fallback, &hooks);
        run_once(Reason::Requested, &hooks);
        assert_eq!(*forced.lock().unwrap(), vec![false, false, true]);
    }

    #[test]
    fn the_worker_applies_after_a_refresh_and_stays_quiet_otherwise() {
        // The shared singleton would make this order-dependent against anything
        // else that touches it, so it drives its own `Shared` and a worker.
        let s: &'static Shared = Box::leak(Box::new(Shared {
            trigger: Mutex::new(Trigger::with_timing(
                Instant::now(),
                Duration::from_millis(20),
                Duration::from_millis(30),
                Duration::from_secs(3600),
            )),
            wake: Condvar::new(),
        }));
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let hooks = Hooks {
            call: Box::new(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }),
            document: Box::new(|_| Ok(Some(doc(1)))),
        };
        std::thread::spawn(move || worker(s, hooks));

        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no refresh, no call");

        s.trigger.lock().unwrap().refresh_seen(Instant::now());
        s.wake.notify_all();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one refresh, one call");

        s.trigger.lock().unwrap().request();
        s.wake.notify_all();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "a live request is another");
    }
}
