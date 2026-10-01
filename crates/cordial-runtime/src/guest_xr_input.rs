//! The input log, `CORDIAL_XR_INPUT_LOG=1`: what the engine reads of its
//! controllers, frame by frame, beside the haptics it asks for.
//!
//! Under WiVRn a hover over one UI element buzzed continuously, where the
//! same hover on the Quest's own runtime gives one tick, and the bridge log
//! showed the engine itself calling `xrApplyHapticFeedback` 20 to 36 times a
//! second while hovering. So something the engine reads changes every frame
//! or two -- an action state, a pose's flags, the interaction profile -- and
//! this names which. Every line is timestamped from one clock so the read
//! that precedes each pulse is visible beside it.
//!
//! Off, nothing here runs: `hook` is asked once per command when its stub is
//! made, and answers `None`. On, it logs only changes, with a one-second
//! summary of everything counted, so a still controller produces a quiet log
//! and a jittering one a loud, specific one.

use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::{host_export, off, say, Observe};

const XR_TYPE_INTERACTION_PROFILE_STATE: u32 = 53;
const XR_TYPE_EVENT_DATA_INTERACTION_PROFILE_CHANGED: u32 = 52;

/// Float changes under this are counted, not logged: trigger and grip noise
/// would otherwise drown the log, and the count still shows it.
const FLOAT_STEP: f32 = 0.02;
/// Lines one action may log in a second before the rest are only counted.
const LINES_PER_KEY: u32 = 8;

pub(super) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("CORDIAL_XR_INPUT_LOG").is_some_and(|v| v != "0");
        if on {
            say("CORDIAL_XR_INPUT_LOG is set: input reads, poses, events and haptics are logged on change".into());
        }
        on
    })
}

/// Milliseconds since the log's first line, so lines from different
/// commands interleave on one clock.
fn ms() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

fn log(s: String) {
    say(format!("input +{:.1}ms {s}", ms()));
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct ActionState {
    /// currentState: 0/1 for a boolean, the bits of an f32 for a float, the
    /// two f32s of a vector2f; empty for a pose.
    pub value: [u32; 2],
    pub changed: u32,
    pub last_change: i64,
    pub active: u32,
}

#[derive(Default)]
struct Window {
    syncs: u32,
    gets: HashMap<&'static str, u32>,
    /// Per (action, subaction): changes seen this second, and lines logged.
    changes: HashMap<(u64, u64), (u32, u32)>,
    locates: u32,
    /// Per space: locates, flag changes, largest position step (m) and
    /// largest rotation step (degrees) between consecutive locates.
    spaces: HashMap<u64, (u32, u32, f32, f32)>,
    applies: u32,
    stops: u32,
    events: u32,
}

#[derive(Default)]
pub(super) struct Input {
    paths: HashMap<u64, String>,
    actions: HashMap<u64, String>,
    spaces: HashMap<u64, String>,
    states: HashMap<(u64, u64), ActionState>,
    /// Per space: flags and pose (qx qy qz qw px py pz) at the last locate.
    poses: HashMap<u64, (u64, [f32; 7])>,
    view_flags: Option<u64>,
    profiles: HashMap<u64, u64>,
    instance: u64,
    session: u64,
    window: Window,
    since: Option<Instant>,
}

static INPUT: std::sync::LazyLock<Mutex<Input>> = std::sync::LazyLock::new(|| Mutex::new(Input::default()));

fn with<R>(f: impl FnOnce(&mut Input) -> R) -> R {
    let mut g = INPUT.lock().unwrap_or_else(|e| e.into_inner());
    let r = f(&mut g);
    let summary = g.tick();
    drop(g);
    if let Some((s, check)) = summary {
        log(s);
        if check {
            check_profiles();
        }
    }
    r
}

impl Input {
    fn path(&self, p: u64) -> String {
        match p {
            0 => "XR_NULL_PATH".into(),
            _ => self.paths.get(&p).cloned().unwrap_or_else(|| format!("{p:#x}")),
        }
    }
    fn action(&self, a: u64) -> String {
        self.actions.get(&a).map_or_else(|| format!("{a:#x}"), |n| format!("'{n}'"))
    }
    fn space(&self, s: u64) -> String {
        self.spaces.get(&s).cloned().unwrap_or_else(|| format!("{s:#x}"))
    }

    /// One action-state read: a line when anything the engine could act on
    /// differs from the last read of the same (action, subaction path).
    pub(super) fn state(&mut self, kind: &'static str, action: u64, sub: u64, s: ActionState) -> Option<String> {
        *self.window.gets.entry(kind).or_default() += 1;
        let key = (action, sub);
        let old = self.states.insert(key, s);
        if old == Some(s) {
            return None;
        }
        let w = self.window.changes.entry(key).or_default();
        w.0 += 1;
        if let (Some(o), "float") = (old, kind) {
            let (a, b) = (f32::from_bits(o.value[0]), f32::from_bits(s.value[0]));
            let same_flags = o.changed == s.changed && o.active == s.active;
            if same_flags && (a - b).abs() < FLOAT_STEP && (a == 0.0) == (b == 0.0) {
                return None;
            }
        }
        w.1 += 1;
        if w.1 > LINES_PER_KEY {
            return None;
        }
        let value = match kind {
            "boolean" => format!("{}", s.value[0]),
            "float" => format!("{:.3}", f32::from_bits(s.value[0])),
            "vector2f" => format!("({:.3}, {:.3})", f32::from_bits(s.value[0]), f32::from_bits(s.value[1])),
            _ => "-".into(),
        };
        let since = match old {
            Some(o) if o.last_change != s.last_change && o.last_change != 0 => {
                format!(" (+{:.1}ms since the previous change)", (s.last_change - o.last_change) as f64 / 1e6)
            }
            _ => String::new(),
        };
        Some(format!("{kind} {} {}: state {value} changedSinceLastSync {} isActive {} lastChangeTime {}{since}{}",
                     self.action(action), self.path(sub), s.changed, s.active, s.last_change,
                     if old.is_none() { " (first read)" } else { "" }))
    }

    /// One `xrLocateSpace` result: a line when the flags change, and the
    /// step from the last pose counted for the summary.
    pub(super) fn locate(&mut self, space: u64, base: u64, time: i64, flags: u64, pose: [f32; 7]) -> Option<String> {
        self.window.locates += 1;
        let old = self.poses.insert(space, (flags, pose));
        let w = self.window.spaces.entry(space).or_default();
        w.0 += 1;
        if let Some((_, p)) = old {
            let d = ((pose[4] - p[4]).powi(2) + (pose[5] - p[5]).powi(2) + (pose[6] - p[6]).powi(2)).sqrt();
            let dot = (pose[0] * p[0] + pose[1] * p[1] + pose[2] * p[2] + pose[3] * p[3]).abs().min(1.0);
            let deg = 2.0 * dot.acos().to_degrees();
            w.2 = w.2.max(d);
            w.3 = w.3.max(deg);
        }
        if old.is_some_and(|o| o.0 == flags) {
            return None;
        }
        w.1 += 1;
        Some(format!("xrLocateSpace {} in {} at {time}: flags {flags:#x} [{}] (was {}) pos ({:.3}, {:.3}, {:.3})",
                     self.space(space), self.space(base), flag_names(flags),
                     old.map_or("first".into(), |o| format!("{:#x}", o.0)), pose[4], pose[5], pose[6]))
    }

    /// Once a second, the counts, and whether to look at the interaction
    /// profiles again.
    fn tick(&mut self) -> Option<(String, bool)> {
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since).as_secs_f32() < 1.0 {
            return None;
        }
        self.since = Some(now);
        let w = std::mem::take(&mut self.window);
        let mut s = format!("1s: syncs {} haptic applies {} stops {} events {} locates {}", w.syncs, w.applies,
                            w.stops, w.events, w.locates);
        let mut gets: Vec<_> = w.gets.iter().collect();
        gets.sort();
        for (k, n) in gets {
            s += &format!(", {k} reads {n}");
        }
        let mut changes: Vec<_> = w.changes.iter().filter(|(_, c)| c.0 > 0).collect();
        changes.sort_by_key(|(k, _)| **k);
        for ((a, p), (n, lines)) in changes {
            s += &format!("; {} {} changed {n}x", self.action(*a), self.path(*p));
            if *lines > LINES_PER_KEY {
                s += &format!(" ({} lines not shown)", lines - LINES_PER_KEY);
            }
        }
        let mut spaces: Vec<_> = w.spaces.iter().collect();
        spaces.sort_by_key(|(k, _)| **k);
        for (sp, (n, fl, d, deg)) in spaces {
            s += &format!("; space {} located {n}x, flag changes {fl}, largest step {:.1} mm {:.2} deg",
                          self.space(*sp), d * 1000.0, deg);
        }
        Some((s, self.session != 0 && self.instance != 0))
    }
}

/// Space location flag names, from XrSpaceLocationFlagBits.
fn flag_names(f: u64) -> String {
    let names = [(1, "ORIENTATION_VALID"), (2, "POSITION_VALID"), (4, "ORIENTATION_TRACKED"), (8, "POSITION_TRACKED")];
    let v: Vec<&str> = names.iter().filter(|(b, _)| f & b != 0).map(|(_, n)| *n).collect();
    if v.is_empty() { "none".into() } else { v.join("|") }
}

fn event_name(ty: u32) -> String {
    match ty {
        16 => "EVENT_DATA_EVENTS_LOST".into(),
        17 => "INSTANCE_LOSS_PENDING".into(),
        18 => "SESSION_STATE_CHANGED".into(),
        40 => "REFERENCE_SPACE_CHANGE_PENDING".into(),
        52 => "INTERACTION_PROFILE_CHANGED".into(),
        1_000_101_000 => "DISPLAY_REFRESH_RATE_CHANGED_FB".into(),
        _ => format!("type {ty}"),
    }
}

/// # Safety
///
/// `p` points at a NUL-terminated string the guest passed.
unsafe fn cstr(p: usize) -> String {
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(p as *const c_char) }.to_string_lossy().into_owned()
}

/// The host's xrPathToString for a path the engine never named: the
/// subaction paths a runtime hands back, and the profile it chose.
fn path_string(instance: u64, path: u64) -> Option<String> {
    type F = unsafe extern "C" fn(u64, u64, u32, *mut u32, *mut c_char) -> i32;
    static PTS: OnceLock<Option<usize>> = OnceLock::new();
    let f = (*PTS.get_or_init(|| host_export("xrPathToString")))?;
    let mut buf = [0 as c_char; 256];
    let mut n = 0u32;
    // SAFETY: the loader's xrPathToString, with a host buffer.
    let r = unsafe { std::mem::transmute::<usize, F>(f)(instance, path, buf.len() as u32, &mut n, buf.as_mut_ptr()) };
    // SAFETY: the runtime wrote a NUL-terminated string on success.
    (r == 0).then(|| unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
}

fn name_path(instance: u64, path: u64) -> String {
    if path == 0 {
        return "XR_NULL_PATH".into();
    }
    if let Some(n) = INPUT.lock().unwrap_or_else(|e| e.into_inner()).paths.get(&path) {
        return n.clone();
    }
    match path_string(instance, path) {
        Some(n) => {
            INPUT.lock().unwrap_or_else(|e| e.into_inner()).paths.insert(path, n.clone());
            n
        }
        None => format!("{path:#x}"),
    }
}

/// Asks the runtime which interaction profile each hand has, and logs any
/// that differ from the last answer. The engine never calls
/// `xrGetCurrentInteractionProfile` itself, so a profile flip that came
/// without an event would otherwise be invisible.
fn check_profiles() {
    type F = unsafe extern "C" fn(u64, u64, *mut u8) -> i32;
    type S = unsafe extern "C" fn(u64, *const c_char, *mut u64) -> i32;
    static GET: OnceLock<Option<(usize, usize)>> = OnceLock::new();
    let Some((get, s2p)) = *GET.get_or_init(|| Some((host_export("xrGetCurrentInteractionProfile")?,
                                                      host_export("xrStringToPath")?))) else { return };
    let (instance, session) = {
        let g = INPUT.lock().unwrap_or_else(|e| e.into_inner());
        (g.instance, g.session)
    };
    for hand in [c"/user/hand/left", c"/user/hand/right"] {
        let mut top = 0u64;
        // SAFETY: the loader's xrStringToPath, a literal and a host out-pointer.
        if unsafe { std::mem::transmute::<usize, S>(s2p)(instance, hand.as_ptr(), &mut top) } != 0 {
            continue;
        }
        // XrInteractionProfileState: type, next, interactionProfile at 16.
        let mut st = [0u8; 24];
        st[..4].copy_from_slice(&XR_TYPE_INTERACTION_PROFILE_STATE.to_ne_bytes());
        // SAFETY: the loader's xrGetCurrentInteractionProfile, a host struct.
        let r = unsafe { std::mem::transmute::<usize, F>(get)(session, top, st.as_mut_ptr()) };
        let prof = if r == 0 {
            u64::from_ne_bytes(st[off("XrInteractionProfileState", "interactionProfile")..][..8].try_into().unwrap())
        } else {
            u64::MAX
        };
        let old = INPUT.lock().unwrap_or_else(|e| e.into_inner()).profiles.insert(top, prof);
        if old != Some(prof) {
            let what = if r != 0 { format!("error {r}") } else { name_path(instance, prof) };
            log(format!("current interaction profile for {}: {what} (asked by the bridge)", hand.to_string_lossy()));
        }
    }
}

/// Reads the four action-state structs' common fields.
///
/// # Safety
///
/// `p` is an XrActionState* of `kind` the runtime just filled.
unsafe fn read_state(kind: &str, p: usize) -> ActionState {
    let s = match kind {
        "boolean" => "XrActionStateBoolean",
        "float" => "XrActionStateFloat",
        "vector2f" => "XrActionStateVector2f",
        _ => "XrActionStatePose",
    };
    let at = |m: &str| p + off(s, m);
    // SAFETY: as the caller promises; the offsets are generated.
    unsafe {
        if kind == "pose" {
            return ActionState { value: [0; 2], changed: 0, last_change: 0, active: *(at("isActive") as *const u32) };
        }
        let v = at("currentState");
        ActionState {
            value: [*(v as *const u32), if kind == "vector2f" { *((v + 4) as *const u32) } else { 0 }],
            changed: *(at("changedSinceLastSync") as *const u32),
            last_change: *(at("lastChangeTime") as *const i64),
            active: *(at("isActive") as *const u32),
        }
    }
}

fn state_hook(kind: &'static str) -> Observe {
    Box::new(move |v, r| {
        if r != 0 || v[1] == 0 || v[2] == 0 {
            return;
        }
        let g = |m: &str| v[1] as usize + off("XrActionStateGetInfo", m);
        // SAFETY: the guest's XrActionStateGetInfo and the state the runtime
        // just wrote.
        let (action, sub, s) = unsafe { (*(g("action") as *const u64), *(g("subactionPath") as *const u64),
                                         read_state(kind, v[2] as usize)) };
        if let Some(line) = with(|i| i.state(kind, action, sub, s)) {
            log(line);
        }
    })
}

/// The after-hook for `name`, when the input log is on and the command is
/// one it watches.
pub(super) fn hook(name: &str) -> Option<Observe> {
    if !enabled() {
        return None;
    }
    Some(match name {
        "xrStringToPath" => Box::new(|v, r| {
            if r == 0 && v[1] != 0 && v[2] != 0 {
                // SAFETY: the guest's path string and the XrPath just written.
                let (s, p) = unsafe { (cstr(v[1] as usize), *(v[2] as *const u64)) };
                log(format!("xrStringToPath \"{s}\" = {p:#x}"));
                with(|i| {
                    i.instance = v[0];
                    i.paths.insert(p, s);
                });
            }
        }),
        "xrPathToString" => Box::new(|v, r| {
            if r == 0 && v[4] != 0 {
                // SAFETY: the buffer the runtime just filled.
                let s = unsafe { cstr(v[4] as usize) };
                log(format!("xrPathToString {:#x} = \"{s}\"", v[1]));
                with(|i| i.paths.insert(v[1], s));
            }
        }),
        "xrCreateActionSet" => Box::new(|v, r| {
            if r == 0 && v[1] != 0 && v[2] != 0 {
                // SAFETY: the guest's XrActionSetCreateInfo and handle.
                let (n, h) = unsafe {
                    (cstr(v[1] as usize + off("XrActionSetCreateInfo", "actionSetName")), *(v[2] as *const u64))
                };
                log(format!("xrCreateActionSet '{n}' = {h:#x}"));
            }
        }),
        "xrCreateAction" => Box::new(|v, r| {
            if r != 0 || v[1] == 0 || v[2] == 0 {
                return;
            }
            let g = |m: &str| v[1] as usize + off("XrActionCreateInfo", m);
            // SAFETY: the guest's XrActionCreateInfo and the handle just written.
            let (n, ty, count, subs, h) = unsafe {
                (cstr(g("actionName")), *(g("actionType") as *const i32), *(g("countSubactionPaths") as *const u32),
                 *(g("subactionPaths") as *const u64), *(v[2] as *const u64))
            };
            let ty = match ty { 1 => "boolean", 2 => "float", 3 => "vector2f", 4 => "pose", 100 => "vibration", _ => "?" };
            let paths: Vec<String> = (0..count as usize).map(|k| {
                // SAFETY: `count` XrPaths at `subs`.
                let p = unsafe { *((subs as usize + k * 8) as *const u64) };
                with(|i| i.path(p))
            }).collect();
            log(format!("xrCreateAction '{n}' ({ty}) = {h:#x}, subaction paths [{}]", paths.join(", ")));
            with(|i| i.actions.insert(h, n));
        }),
        "xrSuggestInteractionProfileBindings" => Box::new(|v, r| {
            if v[1] == 0 {
                return;
            }
            let g = |m: &str| v[1] as usize + off("XrInteractionProfileSuggestedBinding", m);
            // SAFETY: the guest's XrInteractionProfileSuggestedBinding.
            let (prof, n, arr) = unsafe {
                (*(g("interactionProfile") as *const u64), *(g("countSuggestedBindings") as *const u32),
                 *(g("suggestedBindings") as *const u64))
            };
            let pairs: Vec<String> = (0..n as usize).map(|k| {
                let e = arr as usize + k * 16;
                // SAFETY: `n` XrActionSuggestedBinding of 16 bytes.
                let (a, b) = unsafe {
                    (*((e + off("XrActionSuggestedBinding", "action")) as *const u64),
                     *((e + off("XrActionSuggestedBinding", "binding")) as *const u64))
                };
                with(|i| format!("{} -> {}", i.action(a), i.path(b)))
            }).collect();
            log(format!("xrSuggestInteractionProfileBindings {} -> result {r}: {}", with(|i| i.path(prof)),
                        pairs.join(", ")));
        }),
        "xrCreateActionSpace" => Box::new(|v, r| {
            if r != 0 || v[1] == 0 || v[2] == 0 {
                return;
            }
            let g = |m: &str| v[1] as usize + off("XrActionSpaceCreateInfo", m);
            // SAFETY: the guest's XrActionSpaceCreateInfo and the handle.
            let (a, p, h) = unsafe { (*(g("action") as *const u64), *(g("subactionPath") as *const u64), *(v[2] as *const u64)) };
            let name = with(|i| {
                i.session = v[0];
                format!("{} {}", i.action(a), i.path(p))
            });
            log(format!("xrCreateActionSpace {name} = {h:#x}"));
            with(|i| i.spaces.insert(h, format!("[{name}]")));
        }),
        "xrCreateReferenceSpace" => Box::new(|v, r| {
            if r != 0 || v[1] == 0 || v[2] == 0 {
                return;
            }
            // SAFETY: the guest's XrReferenceSpaceCreateInfo and the handle.
            let (t, h) = unsafe {
                (*((v[1] as usize + off("XrReferenceSpaceCreateInfo", "referenceSpaceType")) as *const i32),
                 *(v[2] as *const u64))
            };
            let t = match t { 1 => "VIEW".to_owned(), 2 => "LOCAL".into(), 3 => "STAGE".into(),
                              1_000_426_000 => "LOCAL_FLOOR".into(), t => format!("type {t}") };
            log(format!("xrCreateReferenceSpace {t} = {h:#x}"));
            with(|i| {
                i.session = v[0];
                i.spaces.insert(h, t);
            });
        }),
        "xrSyncActions" => Box::new(|v, r| {
            let n = if v[1] != 0 {
                // SAFETY: the guest's XrActionsSyncInfo.
                unsafe { *((v[1] as usize + off("XrActionsSyncInfo", "countActiveActionSets")) as *const u32) }
            } else {
                0
            };
            with(|i| {
                i.window.syncs += 1;
                i.session = v[0];
            });
            if r != 0 {
                static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
                    log(format!("xrSyncActions ({n} sets) -> {r}"));
                }
            }
        }),
        "xrGetActionStateBoolean" => state_hook("boolean"),
        "xrGetActionStateFloat" => state_hook("float"),
        "xrGetActionStateVector2f" => state_hook("vector2f"),
        "xrGetActionStatePose" => state_hook("pose"),
        "xrLocateSpace" => Box::new(|v, r| {
            if r != 0 || v[3] == 0 {
                return;
            }
            let p = v[3] as usize;
            // SAFETY: the XrSpaceLocation the runtime just filled: flags,
            // then an XrPosef of seven floats.
            let (flags, pose) = unsafe {
                let q = (p + off("XrSpaceLocation", "pose")) as *const f32;
                (*((p + off("XrSpaceLocation", "locationFlags")) as *const u64),
                 std::array::from_fn(|k| *q.add(k)))
            };
            if let Some(line) = with(|i| i.locate(v[0], v[1], v[2] as i64, flags, pose)) {
                log(line);
            }
        }),
        "xrLocateViews" => Box::new(|v, r| {
            if r != 0 || v[2] == 0 {
                return;
            }
            // SAFETY: the XrViewState the runtime just filled.
            let f = unsafe { *((v[2] as usize + off("XrViewState", "viewStateFlags")) as *const u64) };
            if let Some(old) = with(|i| {
                let o = i.view_flags.replace(f);
                (o != Some(f)).then_some(o)
            }) {
                log(format!("xrLocateViews: view state flags {f:#x} [{}] (was {})", flag_names(f),
                            old.map_or("first".into(), |o| format!("{o:#x}"))));
            }
        }),
        "xrGetCurrentInteractionProfile" => Box::new(|v, r| {
            let prof = if r == 0 && v[2] != 0 {
                // SAFETY: the XrInteractionProfileState just filled.
                unsafe { *((v[2] as usize + off("XrInteractionProfileState", "interactionProfile")) as *const u64) }
            } else {
                0
            };
            let inst = with(|i| i.instance);
            log(format!("xrGetCurrentInteractionProfile {} (engine) -> result {r}, {}", name_path(inst, v[1]),
                        name_path(inst, prof)));
        }),
        "xrPollEvent" => Box::new(|v, r| {
            if r != 0 || v[1] == 0 {
                return;
            }
            // SAFETY: the XrEventDataBuffer the runtime just filled.
            let ty = unsafe { *(v[1] as *const u32) };
            let session = with(|i| {
                i.window.events += 1;
                i.instance = v[0];
                i.session
            });
            log(format!("xrPollEvent: {}", event_name(ty)));
            if ty == XR_TYPE_EVENT_DATA_INTERACTION_PROFILE_CHANGED && session != 0 {
                check_profiles();
            }
        }),
        _ => return None,
    })
}

/// Each haptic call, in the same log. `forwarded` is false for a repeat the
/// bridge answered itself (`apply_haptic`'s coalescing).
pub(super) fn haptic(apply: Option<(i64, f32, f32)>, action: u64, path: u64, forwarded: bool) {
    let (a, p) = with(|i| {
        match apply {
            Some(_) => i.window.applies += 1,
            None => i.window.stops += 1,
        }
        (i.action(action), i.path(path))
    });
    match apply {
        Some((d, f, amp)) => log(format!("haptic apply {a} {p}: {d} ns, {f} Hz, amplitude {amp}{}",
                                         if forwarded { "" } else { " (answered here as a repeat)" })),
        None => log(format!("haptic stop {a} {p}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(v: u32, changed: u32, t: i64) -> ActionState {
        ActionState { value: [v, 0], changed, last_change: t, active: 1 }
    }

    /// A boolean read the same way twice logs once; a change logs again and
    /// says how long since the last change.
    #[test]
    fn boolean_logs_on_change_only() {
        let mut i = Input::default();
        i.paths.insert(2, "/user/hand/right".into());
        i.actions.insert(7, "select".into());
        let first = i.state("boolean", 7, 2, b(0, 0, 100)).unwrap();
        assert!(first.contains("'select' /user/hand/right") && first.contains("first read"), "{first}");
        assert_eq!(i.state("boolean", 7, 2, b(0, 0, 100)), None);
        let press = i.state("boolean", 7, 2, b(1, 1, 5_000_100)).unwrap();
        assert!(press.contains("state 1 changedSinceLastSync 1") && press.contains("+5.0ms"), "{press}");
        assert!(i.state("boolean", 7, 2, b(1, 0, 5_000_100)).is_some(), "changedSinceLastSync clearing is a change");
    }

    /// Float noise under the step is counted and not logged; a step over it,
    /// or a flag change, is logged.
    #[test]
    fn float_noise_is_counted_not_logged() {
        let mut i = Input::default();
        let f = |x: f32, c: u32| ActionState { value: [x.to_bits(), 0], changed: c, last_change: 0, active: 1 };
        assert!(i.state("float", 1, 0, f(0.5, 0)).is_some());
        assert_eq!(i.state("float", 1, 0, f(0.505, 0)), None);
        assert!(i.state("float", 1, 0, f(0.6, 0)).is_some());
        assert!(i.state("float", 1, 0, f(0.6, 1)).is_some());
        assert_eq!(i.window.changes[&(1, 0)].0, 4, "the first read and three changes");
    }

    /// One key logs at most LINES_PER_KEY lines in a window.
    #[test]
    fn lines_per_key_are_capped() {
        let mut i = Input::default();
        let logged = (0..20u32).filter(|k| i.state("boolean", 1, 0, b(k % 2, 1, *k as i64)).is_some()).count();
        assert_eq!(logged, LINES_PER_KEY as usize);
        assert_eq!(i.window.changes[&(1, 0)], (20, 20));
    }

    /// Location flags are logged when they change, and pose steps measured.
    #[test]
    fn locate_flags_and_steps() {
        let mut i = Input::default();
        let id = [0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0];
        assert!(i.locate(9, 1, 0, 0xf, id).unwrap().contains("ORIENTATION_VALID|POSITION_VALID|ORIENTATION_TRACKED|POSITION_TRACKED"));
        let moved = [0.0, 0.0, 0.0, 1.0, 0.003, 1.0, 0.004];
        assert_eq!(i.locate(9, 1, 0, 0xf, moved), None);
        let dropped = i.locate(9, 1, 0, 0x3, moved).unwrap();
        assert!(dropped.contains("[ORIENTATION_VALID|POSITION_VALID] (was 0xf)"), "{dropped}");
        let w = i.window.spaces[&9];
        assert_eq!((w.0, w.1), (3, 2));
        assert!((w.2 - 0.005).abs() < 1e-6, "{}", w.2);
    }
}
