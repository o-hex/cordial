//! A development control surface: a socket that drives Cordial's own input and
//! captures its own framebuffer, so an agent can test the client without a
//! human at the keyboard.
//!
//! This exists because of what debugging Cordial actually costs. On 2026-08-21
//! a rendering bug took most of a session and every wrong turn in it was an
//! instrument failure rather than a reasoning failure: `mainWorkCallback` was
//! read as a per-frame heartbeat when it fires exactly twice in healthy runs
//! too, `onFlagsLoaded`'s byte count looked like a delivery readout and is a
//! constant, and present counts collapse to 1.0/s from the idle throttle
//! whenever input is not being driven. Meanwhile nothing here could screenshot
//! a Wayland surface at all -- five methods were tried and all five were
//! refused by the compositor or the kernel -- so every visual check ended with
//! a human being asked to look at the window and describe it.
//!
//! **Two rules shape the whole design, and both come from AGENTS.md.**
//!
//! Input is delivered by calling Cordial's own `input::pass_*` entry points,
//! never by synthesising events at the compositor. `XTestFake*`, `ydotool`,
//! `wlr-virtual-keyboard` and the RemoteDesktop portal all land on whatever
//! has focus, which is the developer's session, and one of them has already
//! hijacked a cursor mid-session. Cordial *is* the client, so there is nothing
//! to send through: the calls go straight in.
//!
//! And nothing here reads Roblox's internal state. The engine publishes no
//! accessibility tree -- measured on 2026-08-21, four ways, all negative; see
//! `native/accessibility.cpp` -- so there is no semantic element list to offer
//! and obtaining one would mean engine introspection, which ADR-001 and
//! ADR-003 place permanently out of scope. This surface works in coordinates
//! and pixels, which is exactly what a human tester has.
//!
//! Off unless asked for. The socket is created only when `CORDIAL_DEV_CONTROL`
//! is set, it lives inside the profile directory so ADR-012's one-instance
//! rule covers it, and no plugin can reach it: ADR-007 gives plugins effects
//! rather than channels, and a socket that drives input is precisely the
//! channel ADR-003 exists to prevent.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// One queued instruction, drained by the pump.
///
/// Queued rather than executed on the socket thread because the engine's input
/// natives are called from the pump everywhere else in this codebase --
/// `CORDIAL_SCRIPT` included -- and an engine that has only ever been handed
/// input from one thread is not somewhere to discover thread-safety from.
#[derive(Debug, Clone)]
pub enum Cmd {
    Move { x: f32, y: f32 },
    Button { x: f32, y: f32, down: bool, button: i32 },
    Key { down: bool, evdev: i32, modifiers: i32 },
    Text(String),
    Scroll { x: f32, y: f32, detents: f32 },
    /// Fullscreen the window, or restore it.
    ///
    /// Here because a resize is the one condition that has ever produced the
    /// render stall this surface was built to investigate, and until now the
    /// only way to drive one was `CORDIAL_SCRIPT`, which is fixed at launch --
    /// so provoking it meant restarting the client, which loses the state you
    /// were trying to provoke it in.
    Fullscreen(bool),
}

static QUEUE: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());

/// Commands accepted since start, so `info` can show the surface is live even
/// when the thing being driven is not.
static ACCEPTED: AtomicU64 = AtomicU64::new(0);

fn push(c: Cmd) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push(c);
    }
    ACCEPTED.fetch_add(1, Ordering::Relaxed);
}

/// Take everything queued. Called once per pump tick.
pub fn drain() -> Vec<Cmd> {
    match QUEUE.lock() {
        Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
        _ => Vec::new(),
    }
}

/// Where the socket lives for this profile.
///
/// Inside the profile directory rather than `XDG_RUNTIME_DIR` so that the
/// profile lock already decides who owns it: two instances cannot share a
/// profile, so they cannot collide on this either.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CORDIAL_DEV_CONTROL_SOCKET") {
        return PathBuf::from(p);
    }
    crate::profile::active().join("devctl.sock")
}

/// Whether the surface was asked for.
pub fn enabled() -> bool {
    matches!(std::env::var("CORDIAL_DEV_CONTROL").as_deref(), Ok(v) if v != "0" && !v.is_empty())
}

/// Start the listener, if it was asked for.
///
/// Failure to bind is reported and then ignored: a development aid that
/// refuses to launch the client would be worse than one that is absent, and
/// the caller has no better answer than carrying on without it.
pub fn start() {
    if !enabled() {
        return;
    }
    let path = socket_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // A stale socket from a killed instance would otherwise make every later
    // run fail to bind. Safe because the profile lock has already established
    // that no other instance owns this directory.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            println!("  devctl: could not bind {} ({e}); continuing without it", path.display());
            return;
        }
    };
    println!("  devctl: listening on {}", path.display());
    std::thread::Builder::new()
        .name("cordial-devctl".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                // One connection at a time, handled inline. The clients are
                // test harnesses issuing a command and reading one reply;
                // concurrency here would buy nothing and would make the
                // ordering of queued input ambiguous, which is the one
                // property a test harness actually needs.
                serve(stream);
            }
        })
        .ok();
}

fn serve(stream: UnixStream) {
    let reader = match stream.try_clone() {
        Ok(s) => BufReader::new(s),
        Err(_) => return,
    };
    let mut out = stream;
    for line in reader.lines().map_while(Result::ok) {
        let reply = handle(line.trim());
        if writeln!(out, "{reply}").is_err() {
            return;
        }
        let _ = out.flush();
    }
}

/// The whole protocol: one line in, one line out, `ok` or `err <why>`.
///
/// Line-oriented and textual rather than a framed binary format because every
/// client of this is either a test script or an agent, and both benefit far
/// more from being drivable by `socat` in a pinch than from an efficient
/// encoding. Screenshots are the exception and are written to a path rather
/// than returned inline, so the reply stays one short line.
fn handle(line: &str) -> String {
    let mut it = line.split_whitespace();
    let Some(verb) = it.next() else { return "ok".into() };
    let num = |s: Option<&str>| s.and_then(|v| v.parse::<f32>().ok());
    match verb {
        "ping" => "ok".into(),
        "info" => info_line(),
        "move" => match (num(it.next()), num(it.next())) {
            (Some(x), Some(y)) => {
                push(Cmd::Move { x, y });
                "ok".into()
            }
            _ => "err move <x> <y>".into(),
        },
        // A click is a press and a release with a move in front of it, because
        // the engine tracks the pointer position separately from the button
        // and a button arriving at a stale position lands somewhere else.
        "click" => match (num(it.next()), num(it.next())) {
            (Some(x), Some(y)) => {
                let button = it.next().and_then(|b| b.parse::<i32>().ok()).unwrap_or(1);
                push(Cmd::Move { x, y });
                push(Cmd::Button { x, y, down: true, button });
                push(Cmd::Button { x, y, down: false, button });
                "ok".into()
            }
            _ => "err click <x> <y> [button]".into(),
        },
        "down" | "up" => match (num(it.next()), num(it.next())) {
            (Some(x), Some(y)) => {
                let button = it.next().and_then(|b| b.parse::<i32>().ok()).unwrap_or(1);
                push(Cmd::Button { x, y, down: verb == "down", button });
                "ok".into()
            }
            _ => format!("err {verb} <x> <y> [button]"),
        },
        // Keys are evdev codes, matching `input::pass_key_event` and every
        // other caller in the tree. Naming them would mean a second keymap to
        // keep correct, and the one that already exists is the compositor's.
        "key" => {
            let down = matches!(it.next(), Some("down"));
            match it.next().and_then(|v| v.parse::<i32>().ok()) {
                Some(evdev) => {
                    let modifiers = it.next().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
                    push(Cmd::Key { down, evdev, modifiers });
                    "ok".into()
                }
                None => "err key <down|up> <evdev-code> [modifiers]".into(),
            }
        }
        "tap" => match it.next().and_then(|v| v.parse::<i32>().ok()) {
            Some(evdev) => {
                let modifiers = it.next().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
                push(Cmd::Key { down: true, evdev, modifiers });
                push(Cmd::Key { down: false, evdev, modifiers });
                "ok".into()
            }
            None => "err tap <evdev-code> [modifiers]".into(),
        },
        "text" => {
            let rest = line.splitn(2, char::is_whitespace).nth(1).unwrap_or("").trim();
            if rest.is_empty() {
                return "err text <string>".into();
            }
            push(Cmd::Text(rest.to_string()));
            "ok".into()
        }
        "fullscreen" | "windowed" => {
            push(Cmd::Fullscreen(verb == "fullscreen"));
            "ok".into()
        }
        "scroll" => match (num(it.next()), num(it.next()), num(it.next())) {
            (Some(x), Some(y), Some(d)) => {
                push(Cmd::Scroll { x, y, detents: d });
                "ok".into()
            }
            _ => "err scroll <x> <y> <detents>".into(),
        },
        // The capture the compositor would not give us. See `vulkan::capture`.
        "screenshot" => match it.next() {
            Some(path) => match crate::android::vulkan::request_capture(path) {
                Ok(desc) => format!("ok {desc}"),
                Err(e) => format!("err {e}"),
            },
            None => "err screenshot <path>".into(),
        },
        _ => format!("err unknown verb {verb:?}"),
    }
}

/// Counters worth having in one place, so a harness can tell a frozen client
/// from a slow one without a debugger.
///
/// `presents` is the number that actually distinguishes them: a wedged engine
/// leaves it fixed while `polls` keeps climbing, which is exactly the shape the
/// 2026-08-21 freeze turned out to have -- 42 presents and 74 million polls.
fn info_line() -> String {
    let presents =
        crate::android::glcount::QUEUE_PRESENT.load(Ordering::Relaxed);
    let (w, h) = crate::android::vulkan::last_extent();
    format!(
        "ok presents={presents} accepted={} extent={w}x{h} pid={}",
        ACCEPTED.load(Ordering::Relaxed),
        std::process::id(),
    )
}

/// Monotonic milliseconds since the surface first ran, for the one input path
/// that wants an event time. Started lazily rather than at load so that a run
/// which never enables this pays nothing for it.
fn now_ms() -> i64 {
    static CLOCK: OnceLock<std::time::Instant> = OnceLock::new();
    CLOCK.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64
}

/// Apply everything queued. Called from the pump, once per tick.
///
/// The handle is the pump's own `game_activity_handle`; the wheel path is the
/// only one that needs it, because it goes through AGDK's motion queue rather
/// than through a bare native the way the others do.
pub fn apply_queued(handle: i64) {
    for cmd in drain() {
        match cmd {
            Cmd::Move { x, y } => crate::android::input::pass_mouse_move(x, y),
            Cmd::Button { x, y, down, button } => {
                crate::android::input::pass_mouse_button(x, y, down, button)
            }
            Cmd::Key { down, evdev, modifiers } => {
                crate::android::input::pass_key_event(down, evdev, modifiers)
            }
            Cmd::Text(s) => {
                // **Per character, through the same path a keystroke takes.**
                //
                // This used to call `pass_text` once with the whole string,
                // which reaches the engine but never touches Cordial's own text
                // buffer -- and the editor draws from that buffer. So the MCP
                // could type into a box, the engine would act on it (search
                // suggestions updated), and the editor showed an empty field
                // with a caret. That is not a bug in the editor; it is this
                // command bypassing the thing the editor reads.
                //
                // It also meant `cordial_text` could not exercise the
                // per-keystroke path at all, so an agent testing text entry
                // through the MCP was testing a paste and reporting on typing.
                // `handle` here is the **GameActivity** handle, which is what
                // `script_type` wants -- the same thing `looper.rs` passes it
                // from `game_activity_handle`. Handing it the *focused textbox*
                // handle instead segfaulted the client on the first keystroke,
                // silently, with no panic and the log ending mid-`pass_key_event`.
                crate::android::input::script_type(handle, &s, now_ms());
            }
            Cmd::Scroll { x, y, detents } => {
                crate::android::input::wheel(handle, x, y, 0.0, detents, now_ms())
            }
            Cmd::Fullscreen(on) => crate::android::backend_set_fullscreen(on),
        }
    }
}
