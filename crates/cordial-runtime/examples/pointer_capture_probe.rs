//! Opens the Wayland window and drives the pointer-capture path with no
//! Roblox APK, no engine and no `libjnivm` — the same shape as
//! `accessibility_probe.rs`, and for the same reason.
//!
//! Pointer capture is four new hand-written protocol messages
//! (`zwp_pointer_constraints_v1.lock_pointer`,
//! `zwp_locked_pointer_v1.set_cursor_position_hint`, its `destroy`, and
//! `zwp_relative_pointer_manager_v1.get_relative_pointer`), and a
//! hand-written signature that is wrong by one argument corrupts the wire
//! rather than failing cleanly — `wayland.rs`'s module doc records that
//! family of crash twice. The riskiest part of the change is therefore the
//! part a full client run is *worst* at exercising, because reaching it
//! needs a person holding a mouse button.
//!
//! It also exists because the honest place to run a lock test is a nested
//! headless compositor on its own `WAYLAND_DISPLAY`, and the Roblox engine
//! does not survive in one — measured, on an unmodified tree as a control:
//! `mutter --headless --virtual-monitor 1280x800` dies within a second of
//! the engine starting to present, and takes the client with it
//! (`Gdk-Message: Error 32 (Broken pipe)`). An ordinary GTK client in the
//! same nested compositor runs indefinitely. So the choice was between
//! testing the lock against the developer's own session — where a bug in it
//! captures their real cursor, which is precisely the accident this is
//! supposed to prevent — and testing it without the engine. This is the
//! second option.
//!
//! ```text
//! mutter --headless --wayland --wayland-display=probe --virtual-monitor 1280x800 &
//! WAYLAND_DISPLAY=probe CORDIAL_FORCE_POINTER_LOCK=1 CORDIAL_TRACE_MOUSE=1 \
//!     cargo run --release --example pointer_capture_probe -- 8
//! ```
//!
//! What it cannot show: that a lock, once granted, actually moves the
//! camera. A headless compositor has no pointer device, so the compositor
//! has nothing to lock and never sends `locked`. That half needs a person
//! with a mouse in an experience and is not something this binary should
//! pretend to have done.

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);

    println!(
        "pointer capture probe: no engine is involved here. Nothing this prints is a \n\
         claim about what Roblox does with a locked pointer — only about what Cordial \n\
         and the compositor say to each other."
    );

    let window = match cordial_runtime::android::wayland::open(960, 540, "Cordial pointer probe") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("no Wayland window: {e}");
            std::process::exit(1);
        }
    };
    let (w, h, _) = window.geometry();
    println!("window up at {w}x{h}; pumping for {seconds}s");

    // `CORDIAL_PROBE_FULLSCREEN=1` — the only way to get the pointer over the
    // canvas without moving somebody's mouse for them.
    //
    // A compositor grants a lock when the surface has pointer focus, and a
    // client cannot place its own window under the cursor on Wayland. It can
    // ask to cover the whole output, which puts the cursor over the canvas
    // wherever the cursor already is, and `gtk_window_fullscreen` is a request
    // this client makes about its own window — the same call
    // `looper.rs`'s `CORDIAL_SCRIPT` timeline uses, and nothing that touches
    // anyone else's session.
    if std::env::var_os("CORDIAL_PROBE_FULLSCREEN").is_some() {
        println!("going fullscreen so that the pointer is over the canvas");
        cordial_runtime::android::wayland::instr_set_fullscreen(true);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        // `0` is "no GameActivity handle", which is the case this whole binary
        // is: the AGDK touch path is skipped and only the Wayland side runs.
        cordial_runtime::android::wayland::pump_input_events(0);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    println!("done");
}
