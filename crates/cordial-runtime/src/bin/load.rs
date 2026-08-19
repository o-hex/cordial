//! `cordial-load` — load `libroblox.so` with the bionic linker.
//!
//! This does not run Roblox. It proves the loader, the relocations and the TLS
//! layout work against the real 116 MB object, and turns
//! docs/framework-api-inventory.md into a prioritised list of what to implement.

use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Instant;

use cordial_linker_sys as linker;
use cordial_runtime::{stubs, symtab};
// `ListModelExt` (`n_items`/`item`) and `Cast` (`downcast`), for
// `refresh_outputs` walking `gdk::Display::monitors()`. The same `gtk4` this
// crate already depends on for `instr_close_window` -- see that dependency's
// own comment in Cargo.toml for why a second gtk4-rs version is not an option.
use gtk4::prelude::*;

struct Options {
    lib_dir: String,
    library: String,
    apk: Option<String>,
    read_asset: Option<String>,
    client_settings: Option<String>,
    flag_overrides: Option<String>,
    gl_probe: bool,
    window_seconds: Option<u64>,
    game_activity: bool,
    join_url: Option<cordial_runtime::deeplink::JoinUrl>,
    run_seconds: u64,
    host_libc: bool,
    jni_onload: bool,
    dump_classes: Option<String>,
    verbose: bool,
}

const USAGE: &str = "\
usage: cordial-load --lib-dir <dir> [options]

  --lib-dir <dir>   directory holding the APK's lib/x86_64/ objects
  --library <name>  object to load (default: libroblox.so)
  --apk <path>      APK to serve assets from; without it AAssetManager_open fails
  --read-asset <p>  read one asset through the AAsset API and report its size
  --client-settings <f>  newline-free list of flag names to pre-cache.
                    NOT the ClientSettings document — the engine loads values itself
  --flag-overrides <f>  JSON passed to nativePreloadFlagOverrides. DIAGNOSTIC
                    ONLY: that native does nothing observable despite its name,
                    tested with several document shapes. To actually set a flag,
                    use ~/.config/cordial/flags.json (see CONTRIBUTING.md)
  --gl-probe        bring up GLES2 through the symbol table and read a pixel back
  --window <secs>   GL PROBE ONLY: open a window and draw a gradient for <secs>.
                    This is Cordial's own test pattern, not Roblox rendering.
  --host-libc       also resolve libc from the host (ABI-unsafe; diagnostic only)
  --jni-onload      stand up a JavaVM and call JNI_OnLoad
  --game-activity   implies --jni-onload; bring Roblox up and hand it a surface
  --join-url <url>  a roblox-player:// or roblox:// link from a browser click,
                    handed to the engine during bring-up. Rejected unless it is
                    one of those two schemes, printable ASCII, and under 2 kB.
                    A roblox-player: link in the desktop launcher's format is
                    rewritten into the roblox:// form this engine matches; its
                    one-time gameinfo ticket is dropped and never printed
  --run <secs>      how long to let Roblox run after handover (default 15).
                    0 means no timer: run until the window is closed or the
                    process is sent SIGTERM/SIGINT. Closing the window ends the
                    process either way — the timer is a backstop for headless
                    and scripted runs, not the way a session is meant to end
  --dump-classes <f>  implies --jni-onload; write the Java classes Roblox asked
                    for to <f> — the observed Phase 2 backlog
  -v, --verbose     list every symbol and how it resolved

env:
  MCPELAUNCHER_LINKER_VERBOSITY=<n>  bionic linker tracing (try 1 or 2)
  CORDIAL_STUB_ABORT=1               abort on the first unimplemented call
  CORDIAL_STUB_QUIET=1               do not report stub hits as they happen
  CORDIAL_TRACE=1                    log libc calls (WARNING: wraps variadic
                                     functions with fixed-arity ones, which is
                                     not ABI-safe — it changes behaviour)
  CORDIAL_ANDROID_TRACE=1            log Android API calls (safe; no variadics)
  CORDIAL_MONITOR=<n>                open the window on the nth monitor (0 is
                                     the first), instead of the primary one
  CORDIAL_WINDOW_POS=<x>,<y>         explicit window position; wins over
                                     CORDIAL_MONITOR
  CORDIAL_FULLSCREEN=1               cover the chosen monitor and ask the
                                     window manager for fullscreen
  CORDIAL_RESOLUTION=<w>x<h>         render resolution (default 1280x720);
                                     CORDIAL_FULLSCREEN overrides it
  CORDIAL_DPI_SCALE=<f>              UI density Roblox lays out against.
                                     1.0 is a low-density phone; try 1.5-2
  CORDIAL_PLATFORM_NAME=<name>       what Cordial answers when the engine asks
                                     which platform it is on. Defaults to Linux,
                                     one of the engine's own Enum.Platform
                                     names; =Android is the control run. See
                                     docs/analysis/platform-identity.md
  CORDIAL_WHEEL_SCALE=<f>            scroll wheel detents per notch (default 1);
                                     negative inverts the direction
  CORDIAL_TRACE_WHEEL=1              log every wheel event and the arguments
                                     nativePassMouseWheel received
  CORDIAL_NO_POINTER_LOCK=1          never capture the pointer, whatever the
                                     engine or the mouse asks for. The control
                                     for the capture path; it still polls and
                                     traces the engine's own request, so a
                                     control run says what it would have done
  CORDIAL_NO_DRAG_LOCK=1             capture only when the engine asks, not
                                     while a right/middle button is held
  CORDIAL_NO_CLOSE_EXIT=1            closing the window does not end the
                                     process — the old behaviour, kept as the
                                     control for the close path. SIGTERM and
                                     --run are unaffected
  CORDIAL_SIGNIN_PROBE=1             ask the engine whether login is Lua-rendered
  CORDIAL_DEEPLINK_PROBE=1           with --join-url, print the linking
                                     protocol's own message and field names,
                                     read out of the running engine
  CORDIAL_DEEPLINK_NO_TRANSLATE=1    hand a roblox-player:// desktop link to the
                                     engine as it arrived, instead of rewriting
                                     it into the roblox:// form the engine's own
                                     pattern matches. The control for the
                                     translation; the engine does not act on the
                                     untranslated link, which is the point
  CORDIAL_NO_VULKAN=1                make the host look like it has no Vulkan
                                     loader, forcing the GLES2/EGL fallback
                                     path Roblox uses when dlopen(libvulkan)
                                     fails
  CORDIAL_PRESENT_MODE=<m>           swapchain present mode: auto (the default;
                                     MAILBOX when the driver advertises it),
                                     off (forward the engine's own choice, which
                                     is FIFO — this is the control for a frame
                                     rate measurement), or one of mailbox,
                                     immediate, fifo, fifo-relaxed. FIFO is the
                                     only mode the spec guarantees, so anything
                                     the driver does not advertise falls back to
                                     what the engine asked for
  CORDIAL_GAMEMODE=0                 do not ask Feral GameMode to raise the CPU
                                     governor and priority for this process.
                                     On by default; a machine without gamemoded
                                     says so once and carries on
  CORDIAL_COUNT_GL=1                 count eglCreateWindowSurface/MakeCurrent/
                                     SwapBuffers/glClear/Draw*/CompileShader
                                     calls and report them after --run
  CORDIAL_SWAP_TIMES=1               with CORDIAL_COUNT_GL=1, also print how
                                     long each real eglSwapBuffers call blocked
";

fn parse() -> Result<Options, String> {
    let mut opt = Options {
        lib_dir: String::new(),
        library: "libroblox.so".into(),
        apk: None,
        read_asset: None,
        client_settings: None,
        flag_overrides: None,
        gl_probe: false,
        window_seconds: None,
        game_activity: false,
        join_url: None,
        run_seconds: 15,
        host_libc: false,
        jni_onload: false,
        dump_classes: None,
        verbose: false,
    };
    // Before anything can latch a profile. ADR-012's move used to be driven only
    // from the shell's `main`, so a client started any other way — `just client`,
    // or a hand-typed command — silently kept writing to the pre-ADR-012
    // `instances/default` while a shell-started one used `profiles/default`.
    // Signing in through one and restarting through the other then looked exactly
    // like the session being dropped. This is a no-op once the move has happened.
    cordial_runtime::profile::migrate_legacy_layout();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lib-dir" => opt.lib_dir = args.next().ok_or("--lib-dir needs a value")?,
            "--library" => opt.library = args.next().ok_or("--library needs a value")?,
            "--apk" => opt.apk = Some(args.next().ok_or("--apk needs a path")?),
            // Which profile's storage this instance runs against. The profile is
            // an argument and the settings inside it are not, deliberately: one
            // value decides where everything else lives, and a setting passed on
            // a command line cannot change while the client runs — which the
            // dynamic DFFlag families exist precisely to do (ADR-013).
            //
            // This has to be resolved before anything reads the profile, because
            // `profile::active()` latches on first use. Without it the client
            // wrote to `instances/default` while a shell-started one wrote to
            // `profiles/<name>`, and signing in through one and restarting
            // through the other looked exactly like the session being lost.
            "--profile" => {
                let name = args.next().ok_or("--profile needs a name")?;
                let dir = cordial_runtime::profile::dir(&name)?;
                cordial_runtime::profile::set_active(dir)?;
            }
            "--read-asset" => {
                opt.read_asset = Some(args.next().ok_or("--read-asset needs a name")?)
            }
            "--flag-overrides" => {
                let p = args.next().ok_or("--flag-overrides needs a path")?;
                opt.flag_overrides = Some(
                    std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?,
                );
            }
            "--client-settings" => {
                opt.client_settings =
                    Some(args.next().ok_or("--client-settings needs a path")?)
            }
            "--gl-probe" => opt.gl_probe = true,
            "--window" => {
                let v = args.next().ok_or("--window needs a duration in seconds")?;
                opt.window_seconds = Some(v.parse().map_err(|_| "--window wants a number")?);
            }
            "--host-libc" => opt.host_libc = true,
            "--jni-onload" => opt.jni_onload = true,
            "--run" => {
                let v = args.next().ok_or("--run needs a duration in seconds")?;
                opt.run_seconds = v.parse().map_err(|_| "--run wants a number")?;
            }
            "--game-activity" => {
                opt.jni_onload = true;
                opt.game_activity = true;
            }
            // The URL a browser click produced, forwarded by the shell. It is
            // validated here, at the edge, rather than anywhere further in:
            // this is the process boundary the value crosses, and a bad one
            // should end the launch with a sentence rather than travel.
            "--join-url" => {
                let raw = args.next().ok_or("--join-url needs a URL")?;
                opt.join_url = Some(cordial_runtime::deeplink::validate(&raw)?);
            }
            "--dump-classes" => {
                opt.jni_onload = true;
                opt.dump_classes = Some(args.next().ok_or("--dump-classes needs a path")?);
            }
            "-v" | "--verbose" => opt.verbose = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unrecognised argument: {other}")),
        }
    }
    if opt.lib_dir.is_empty() {
        return Err("--lib-dir is required".into());
    }
    Ok(opt)
}

/// The directory the engine should treat as its asset folder.
///
/// Not the APK. The engine's HTTP stack is curl, and curl's `CURLOPT_CAINFO`
/// needs a real filesystem path for `assets/ssl/cacert.pem`; handing it the
/// `.apk` names a file inside a file. So the APK's assets are unpacked once to
/// a cache directory and that is what `assetFolderPath` points at.
///
/// It points at the `content` **subdirectory**, not the unpack root. The
/// Waydroid capture is explicit about this:
///
/// ```text
/// [FLog::Output] setAssetFolder      /data/user/0/com.roblox.client/app_assets/content
/// [FLog::Output] setExtraAssetFolder /data/user/0/com.roblox.client/app_assets/ExtraContent
/// ```
///
/// The engine echoes back exactly the path it is given, and resolves its
/// siblings — `android/`, `ssl/`, `fonts/` — relative to the *parent*. Passing
/// the unpack root therefore sends every one of those lookups a level too high.
/// Cordial did that, and the engine's own log named the consequence:
///
/// ```text
/// [FLog::CreatorError] Error: boost::filesystem::canonical:
///     No such file or directory: ".../.cache/cordial/android"
/// ```
///
/// That throw aborts `SingleSurfaceApp` initialisation before it reaches
/// `setStage: (stage:Native)` and before it instantiates its controllers, which
/// is why `initializeLuaAppWithLoggedInUser` then ran at `(stage:None)` and
/// dereferenced a controller that was never built.
///
/// Falls back to the APK path if extraction fails, which keeps the old
/// behaviour rather than refusing to start over an asset folder — the loader
/// and asset paths still work without it.
fn asset_folder(apk: &Option<String>) -> String {
    let Some(apk) = apk else { return String::new() };
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/assets");
    match cordial_runtime::android::asset::extract_to(&base) {
        Ok(dir) => dir.join("content").to_string_lossy().into_owned(),
        Err(e) => {
            println!("  asset extraction failed ({e}); using the APK path");
            apk.clone()
        }
    }
}

/// The directory the engine runs *in*, and why it needs one of its own.
///
/// Roblox builds several paths from a root it was never given and resolves them
/// against the working directory: `./exe/cacert.pem`, `http/`, `sounds/`,
/// `cache/` and a `ContentProvider_<pid>` per launch. Two consequences, both
/// real:
///
/// * curl is handed `./exe/cacert.pem` as its trust store, does not find it, and
///   every HTTPS request fails — `error adding trust anchors from file`. The CA
///   bundle exists; it ships in the APK at `assets/ssl/cacert.pem`.
/// * whatever directory you launched from fills up with the engine's scratch
///   files. Running from a checkout littered this repository.
///
/// An Android app's working directory is its own sandbox, so giving the process
/// one is the faithful behaviour rather than a workaround. `--lib-dir` and
/// `--apk` are made absolute first, because they are the caller's paths and are
/// allowed to be relative to the caller's directory.
///
/// Never fatal: a client that starts in the wrong directory is more useful than
/// one that refuses to start.
fn enter_run_dir(opt: &mut Options) {
    for p in [&mut opt.lib_dir] {
        if let Ok(abs) = std::fs::canonicalize(&*p) {
            *p = abs.to_string_lossy().into_owned();
        }
    }
    if let Some(apk) = opt.apk.as_mut() {
        if let Ok(abs) = std::fs::canonicalize(&*apk) {
            *apk = abs.to_string_lossy().into_owned();
        }
    }

    // The engine's working directory, inside whichever profile this instance was
    // given. This used to compute `instances/default` by hand while the rest of
    // the process had moved to `profiles/<name>`, which put the run directory and
    // the data directory in different trees.
    let root = cordial_runtime::profile::active().join("run");
    if let Err(e) = std::fs::create_dir_all(root.join("exe")) {
        println!("  could not create {}: {e}", root.display());
        return;
    }

    // The trust store, from the APK's own copy. Linked rather than copied so a
    // re-extracted bundle is picked up without a stale duplicate.
    let ca = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/assets/ssl/cacert.pem");
    let dest = root.join("exe/cacert.pem");
    if ca.exists() && std::fs::read_link(&dest).ok().as_deref() != Some(ca.as_path()) {
        let _ = std::fs::remove_file(&dest);
        let _ = std::os::unix::fs::symlink(&ca, &dest);
    }

    if let Err(e) = std::env::set_current_dir(&root) {
        println!("  could not enter {}: {e}", root.display());
    }
}

/// The render resolution, and why it is not simply the window size.
///
/// Roblox sizes its framebuffers and picks UI asset resolutions from what the
/// surface reports, so 1280x720 is not just a small window — it is the whole
/// pipeline running at 720p. On a 1920x1200 panel that is the difference
/// between a native image and an upscaled one.
///
/// `CORDIAL_RESOLUTION=<w>x<h>`; defaults to 1280x720, and `CORDIAL_FULLSCREEN`
/// overrides both with the monitor's own size.
/// What `GameActivity.bootstrapTheApp()` runs, and whether it ran.
///
/// On Android this is Kotlin: the app fetches its client settings and its flag
/// set and hands both to the engine, and the engine calls it from inside
/// `initializeNativeCode`. Cordial is the host application, so this is Cordial's
/// job — it was simply being done in the wrong place. The delivery below used to
/// happen after `initializeNativeCode` returned, and a traced run shows the
/// engine calling `bootstrapTheApp`, getting an unresolved placeholder, and
/// reporting `gameActivity_onFlagsFailed` on the very next line. Nothing
/// delivered afterwards could have changed that verdict.
///
/// Function pointers rather than anything borrowed because the callback crosses
/// into C++ and back on the engine's own thread, with no lifetime to speak of.
struct BootstrapPlan {
    settings_native: usize,
    post_native: usize,
    flags_native: usize,
    settings: String,
    flag_names: String,
}

static BOOTSTRAP: std::sync::OnceLock<BootstrapPlan> = std::sync::OnceLock::new();
static BOOTSTRAP_RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Deliver settings and flags, from inside the engine's own bootstrap call.
///
/// Prints rather than returning a result because there is nobody to return one
/// to: the caller is the engine, three frames into `initializeNativeCode`.
extern "C" fn run_bootstrap() {
    let Some(plan) = BOOTSTRAP.get() else {
        eprintln!("  bootstrapTheApp: nothing planned");
        return;
    };
    // The engine calls `bootstrapTheApp` twice per launch -- the trace shows two
    // `Call Member Function ... bootstrapTheApp ()V` -- and delivering on both
    // registered two flag providers where Sober registers one. Deliver once.
    if BOOTSTRAP_RAN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        println!("  bootstrapTheApp: already delivered");
        return;
    }
    println!("  bootstrapTheApp: delivering settings and flags");
    if plan.settings_native != 0 {
        match linker::game_activity::init_client_settings(
            plan.settings_native as *mut std::ffi::c_void,
            &plan.settings,
            "",
            "",
        ) {
            Ok(code) => println!("    nativeInitClientSettings -> {code}"),
            Err(e) => println!("    nativeInitClientSettings failed: {e}"),
        }
    }
    // `post` immediately after `settings`, and the flag names last.
    //
    // That is Sober's order, read off its own log rather than guessed:
    // nativeInitClientSettings at 3.700s, nativePostClientSettingsLoaded
    // Initialization3 at 3.796s, RbxStorage::init at 3.820s, and
    // nativeInitializeNativeFlags only later. The first arrangement here put the
    // flag names between the two and the 139-name list takes long enough that
    // `post` landed after the verdict had already been reported.
    if plan.post_native != 0 {
        match linker::game_activity::post_client_settings_loaded(
            plan.post_native as *mut std::ffi::c_void,
        ) {
            Ok(()) => println!("    postClientSettingsLoadedInitialization3 ok"),
            Err(e) => println!("    postClientSettingsLoadedInitialization3 failed: {e}"),
        }
    }
    if plan.flags_native != 0 {
        match linker::game_activity::init_flags(
            plan.flags_native as *mut std::ffi::c_void,
            &plan.flag_names,
        ) {
            Ok(()) => println!("    flags initialised"),
            Err(e) => println!("    flag init failed: {e}"),
        }
    }
}

fn requested_resolution() -> (u32, u32) {
    let Ok(v) = std::env::var("CORDIAL_RESOLUTION") else {
        return (1280, 720);
    };
    let mut parts = v.split(['x', 'X']).map(str::trim);
    if let (Some(Ok(w)), Some(Ok(h))) = (
        parts.next().map(str::parse::<u32>),
        parts.next().map(str::parse::<u32>),
    ) {
        if w >= 320 && h >= 240 && w <= 7680 && h <= 4320 {
            return (w, h);
        }
    }
    println!("  CORDIAL_RESOLUTION={v:?} is not <w>x<h> within reason; using 1280x720");
    (1280, 720)
}

/// Every monitor GDK currently knows about, as `cordial_runtime::refresh::Output`.
///
/// This is `cordial_shell::refresh_watch::outputs` in spirit but not in fact:
/// that function marks an output `current` by asking `gdk::Display::
/// monitor_at_surface` about the specific `gtk::Window` the caller passes it,
/// and there is no such window reachable from here. The engine's own host
/// window is built and kept entirely inside `android::wayland::WaylandWindow`
/// -- a private field (`HostWindowCell`) with no accessor -- and `android/**`
/// was out of scope for the change that wired this up. So every `Output` below
/// carries `current: false`; see `wire_refresh_rate` for what that means for
/// the rate actually reported as current.
///
/// `gdk::Display::default()` answers regardless of that gap, because GTK/GDK
/// is initialised once, process-wide, as a side effect of the engine's window
/// opening (`cordial_shell::host_window::init_wayland`, called from inside
/// `wayland::open`) -- it does not itself need the window object, only that
/// something in the process has already brought GDK up. Empty before that has
/// happened, which `refresh::supported_from`/`current_for` already treat as
/// "nothing plausible is known" rather than a fault.
fn refresh_outputs() -> Vec<cordial_runtime::refresh::Output> {
    let Some(display) = gtk4::gdk::Display::default() else { return Vec::new() };
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i))
        .filter_map(|obj| obj.downcast::<gtk4::gdk::Monitor>().ok())
        .map(|m| cordial_runtime::refresh::Output {
            // Reusing `refresh::hz_from_millihertz` rather than repeating its
            // one-line body: `refresh_watch.rs` only re-derives that division
            // because the Cargo cycle noted in its header leaves it no other
            // choice, and load.rs is on the correct side of that edge.
            hz: cordial_runtime::refresh::hz_from_millihertz(m.refresh_rate()),
            current: false,
        })
        .collect()
}

/// Tell the engine what the display can do, and keep it told.
///
/// `NativeGLInterface.nativePassSupportedRefreshRates`/
/// `nativePassCurrentDisplayRefreshRate` are exported by every build this
/// project has looked at and neither had ever been called -- see
/// `cordial_runtime::refresh` for the policy this follows.
///
/// **What this does not achieve.** The design in `refresh.rs` and
/// `refresh_watch.rs` reports "current" as the output the engine's own window
/// is *mostly on*, tracked as the window moves and re-announced through
/// `worth_announcing`. That needs a live `gtk::Window` to call `watch` on, and
/// -- see `refresh_outputs` -- none is reachable from this file. What this
/// does instead: send the real supported-rate list at startup and on every
/// hotplug, and send a "current" rate chosen by `current_for`'s own documented
/// fallback (the first plausible rate, when nothing is marked current) rather
/// than inventing a second heuristic here. On a single-monitor machine that
/// fallback is exact, because there is only one candidate. On a multi-monitor
/// one -- this is true of the machine this was tested on -- it is a real rate
/// of a real attached output, not a fabricated number, but it is **not**
/// verified to be the output the window actually landed on, and must not be
/// read as though it were.
///
/// Window-crosses-a-boundary tracking specifically -- the case
/// `refresh_watch.rs`'s own header calls out -- is therefore not wired by this
/// change. It needs a `pub fn` on `android::wayland::WaylandWindow` (or on
/// `android::WindowHandle`) handing back the `adw::Window`
/// `cordial_shell::host_window::HostWindow::window()` already exposes;
/// `android/**` was off limits to the change that added this function, so
/// that accessor does not exist yet.
fn wire_refresh_rate(lib: linker::Library) {
    let supported_native = lib.symbol(
        "Java_com_roblox_engine_jni_NativeGLInterface_nativePassSupportedRefreshRates",
    );
    let current_native = lib.symbol(
        "Java_com_roblox_engine_jni_NativeGLInterface_nativePassCurrentDisplayRefreshRate",
    );
    println!(
        "  refresh: nativePassSupportedRefreshRates {}",
        if supported_native.is_some() { "resolved" } else { "NOT exported" }
    );
    println!(
        "  refresh: nativePassCurrentDisplayRefreshRate {}",
        if current_native.is_some() { "resolved" } else { "NOT exported" }
    );
    let (Some(supported_native), Some(current_native)) = (supported_native, current_native) else {
        return;
    };

    // Shared between the startup call below and the hotplug callback, so a
    // hotplug that leaves the rate unchanged does not re-announce -- see
    // `refresh::worth_announcing`'s own reasoning for why that matters.
    let previous_current: Rc<Cell<Option<f32>>> = Rc::new(Cell::new(None));
    let announce = {
        let previous_current = previous_current.clone();
        move || {
            let outputs = refresh_outputs();
            let supported = cordial_runtime::refresh::supported_from(&outputs);
            if supported.is_empty() {
                println!("  refresh: no plausible output to report yet");
            } else {
                match linker::game_activity::pass_supported_refresh_rates(supported_native, &supported) {
                    Ok(()) => println!("  refresh: nativePassSupportedRefreshRates {supported:?}"),
                    Err(e) => println!("  refresh: nativePassSupportedRefreshRates failed: {e}"),
                }
            }
            // Only when there is no ambiguity about which output that is.
            //
            // Nothing reachable from here holds the engine's window, so no
            // `Output` built above can carry `current: true`, and
            // `current_for`'s fallback picks the first plausible rate -- which
            // is GDK's enumeration order, not where the window is. On the
            // machine this was written on that is a coin flip between 49.998
            // and 60.002 Hz.
            //
            // Sending it anyway would be telling the engine something specific
            // and unverified, in the one area AGENTS.md is most emphatic about:
            // with input flowing the frame rate is a hard FIFO vsync lock to
            // the output's refresh, so a client that names the wrong output has
            // asked the engine to schedule against a display it is not on. The
            // supported list above is complete and true whatever the window is
            // doing, and goes regardless; this one waits.
            //
            // What unblocks it is small and named: an accessor on
            // `android::wayland::WaylandWindow` handing back the `adw::Window`
            // that `cordial_shell::host_window::HostWindow::window()` already
            // exposes, so `monitor_at_surface` can answer properly.
            let unambiguous = supported.len() == 1;
            let current = if unambiguous {
                cordial_runtime::refresh::current_for(&outputs)
            } else {
                None
            };
            if cordial_runtime::refresh::worth_announcing(previous_current.get(), current) {
                if let Some(hz) = current {
                    match linker::game_activity::pass_current_refresh_rate(current_native, hz) {
                        Ok(()) => println!("  refresh: nativePassCurrentDisplayRefreshRate {hz}"),
                        Err(e) => println!("  refresh: nativePassCurrentDisplayRefreshRate failed: {e}"),
                    }
                }
            } else if !unambiguous && previous_current.get().is_none() {
                println!(
                    "  refresh: {} outputs differ and nothing here knows which the window is on; \
                     not naming a current rate",
                    supported.len()
                );
            }
            previous_current.set(current);
        }
    };

    announce();

    // Hotplug only -- a monitor appearing or disappearing changes
    // `display.monitors()` regardless of where the window is, so this needs
    // no window reference either. GDK's `items-changed` fires from whichever
    // code pumps the process's one `glib::MainContext`, which
    // `android::wayland`'s own pump already does on every tick; nothing here
    // has to add a second pump loop.
    if let Some(display) = gtk4::gdk::Display::default() {
        display.monitors().connect_items_changed(move |_, _, _, _| announce());
    }
}

/// The engine's own version, read out of `libroblox.so` rather than hardcoded.
///
/// This existed as a hardcoded `"2.732.0.1043"` with a comment claiming it was
/// "the engine's own answer rather than a guess". It was neither: the engine in
/// the APK on this machine is **2.730.0.790**, which is what it stamps on every
/// log file it writes, so Cordial was telling the server one version while the
/// client was another. A build that misreports its own version is exactly the
/// shape of thing a server-side check rejects, and the value had gone stale
/// silently across an APK update with nothing to catch it.
///
/// Reading it back out is a plain string search, not disassembly: the version
/// is stored as an ASCII literal and `strings` finds exactly one match for the
/// four-part shape. Returning `None` when that is not true is deliberate —
/// skipping the call is honest, and inventing a version is what caused this.
fn engine_version(lib_dir: &str) -> Option<String> {
    let bytes = std::fs::read(std::path::Path::new(lib_dir).join("libroblox.so")).ok()?;
    let mut found: Option<String> = None;
    let mut run = Vec::new();
    for &b in bytes.iter().chain(std::iter::once(&0u8)) {
        if b.is_ascii_digit() || b == b'.' {
            run.push(b);
            continue;
        }
        if run.len() >= 9 && run.len() <= 20 {
            let s = String::from_utf8_lossy(&run).to_string();
            let parts: Vec<&str> = s.split('.').collect();
            if parts.len() == 4
                && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
                && parts[0] == "2"
            {
                // More than one distinct candidate means the shape is not
                // unique in this build and the assumption behind reading it
                // has stopped holding. Say so rather than pick one.
                match &found {
                    Some(prev) if *prev != s => return None,
                    _ => found = Some(s),
                }
            }
        }
        run.clear();
    }
    found
}

// `native/local_storage.cpp`'s two exported callers. Declared directly here
// rather than through `cordial_linker_sys::game_activity` -- that module is
// the usual home for a wrapper like this, and it was off limits to the task
// that added these two, on the reasoning that a crate several agents rely on
// as a stable interface should not gain new surface mid-session. The symbols
// still link in exactly the same way: `native/CMakeLists.txt` compiles
// `local_storage.cpp` into the same `libcordial_jni_shim.a`
// `cordial-linker-sys`'s `build.rs` already tells `cordial-run` to link, so a
// bare `extern "C"` here resolves at the same final link step every other
// wrapper in that crate does.
extern "C" {
    fn cordial_local_storage_set_platform_impl(
        f: *mut std::ffi::c_void,
        err: *mut std::os::raw::c_char,
        err_len: usize,
    ) -> std::os::raw::c_int;
    fn cordial_update_screen_orientation(
        f: *mut std::ffi::c_void,
        width: std::os::raw::c_int,
        height: std::os::raw::c_int,
        err: *mut std::os::raw::c_char,
        err_len: usize,
    ) -> std::os::raw::c_int;
}

fn take_c_err(err: Vec<u8>) -> String {
    let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
    String::from_utf8_lossy(&err[..end]).into_owned()
}

/// `ILocalStorageHandlerCore.setPlatformImpl(IPlatformLocalStorageHandler)`.
/// See `native/local_storage.cpp` for what the object handed over answers and
/// why the call is believed to be static.
fn local_storage_set_platform_impl(f: *mut std::ffi::c_void) -> Result<(), String> {
    let mut err = vec![0u8; 512];
    // SAFETY: `f` is the exported JNI native the caller resolved by name;
    // `err` is a live buffer for the duration of this call.
    let rc = unsafe {
        cordial_local_storage_set_platform_impl(f, err.as_mut_ptr() as *mut std::os::raw::c_char, err.len())
    };
    if rc == 0 { Ok(()) } else { Err(take_c_err(err)) }
}

/// `NativeInputInterface.nativeUpdateScreenOrientation(I)V` -- the one call
/// `docs/analysis/flag-init.md` §16 found mocktail makes between
/// `initializeNativeCode` and the settings handshake that Cordial did not.
fn update_screen_orientation(f: *mut std::ffi::c_void, width: i32, height: i32) -> Result<(), String> {
    let mut err = vec![0u8; 512];
    // SAFETY: as above.
    let rc = unsafe {
        cordial_update_screen_orientation(
            f,
            width,
            height,
            err.as_mut_ptr() as *mut std::os::raw::c_char,
            err.len(),
        )
    };
    if rc == 0 { Ok(()) } else { Err(take_c_err(err)) }
}

fn main() -> ExitCode {
    let mut opt = match parse() {
        Ok(o) => o,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}\n");
            }
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    // Before anything this profile might do reaches a network, including the
    // client-settings fetch further down -- which is a real HTTP request over
    // `ureq`, made by Cordial itself, and would otherwise go out whatever
    // route this instance happens to have. `--profile` (or its absence) has
    // just been resolved by `parse()`, above, so `profile::active()` is
    // settled and this is the earliest point this can be checked.
    //
    // This duplicates the same call `cordial-shell`'s `launch.rs` makes
    // before it ever spawns this process -- deliberately, not by accident.
    // AGENTS.md documents running `cordial-run` directly, without the shell,
    // as a fully supported path (`cargo run --release --bin cordial-run --
    // ...`), and a gate that only lived in the shell would be a `vpn-required`
    // profile that silently stopped meaning anything the moment somebody
    // started the client the other documented way. See
    // `cordial_shell::network`'s own doc for what this does and does not
    // guarantee.
    if let Err(refusal) =
        cordial_shell::network::ensure_launchable(&cordial_runtime::profile::active())
    {
        eprintln!("error: {refusal}");
        return ExitCode::FAILURE;
    }

    // Which backend, and who asked for it, before the engine has had a chance to
    // `dlopen` anything. Said out loud on every run: the questions it answers are
    // "why is this slow" and "why does this look different from yesterday", and
    // those get asked from a support thread rather than from a terminal somebody
    // is willing to re-run with a trace variable set.
    cordial_runtime::graphics::report();

    // Before anything can resolve a path: Android's `/system`, served from a
    // directory Cordial builds out of the host's fonts. Roblox asks for
    // `/system/fonts/NotoSansCJK-Regular.ttc` during app startup and turns the
    // miss into an empty path and an unhandled exception.
    cordial_runtime::android::system::install();

    if let Some(apk) = &opt.apk {
        match cordial_runtime::android::asset::set_apk(std::path::Path::new(apk)) {
            Ok(()) => println!("assets: {apk}"),
            Err(e) => {
                eprintln!("bad --apk: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // After the APK is registered (so the CA bundle can be extracted) and before
    // anything asks the engine to resolve a path.
    if opt.apk.is_some() {
        let _ = asset_folder(&opt.apk);
    }
    enter_run_dir(&mut opt);

    if let Some(name) = &opt.read_asset {
        match cordial_runtime::android::asset::probe(name) {
            Ok(len) => println!("asset {name}: {len} bytes"),
            Err(e) => {
                eprintln!("asset {name}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    cordial_runtime::android::set_trace(std::env::var_os("CORDIAL_ANDROID_TRACE").is_some());

    // Who is signed in, before anything can ask.
    //
    // This is deliberately the earliest thing after the profile is settled, and
    // it can be: unlike the cookie restore below, it calls into no engine
    // symbol at all. `NativeUserJavaInterface` and `StartAppParams` live in
    // Cordial's own framework layer, so there is nothing to be too early for —
    // whereas being *late* is a real failure with two shapes, because
    // `StartAppParams` copies four of these fields once inside
    // `nativeAppBridgeV2StartAppWithParams` and the engine can query the other
    // mirror at any moment before that.
    //
    // The cookie alone was never enough: with a real session restored and the
    // engine confirmed holding it, `PlatformAccountRouter` still routed to
    // Landing, because it asks these mirrors and they said user 0. See
    // `cordial_runtime::identity` and docs/design/sign-in.md §9.
    cordial_runtime::identity::listen();
    cordial_runtime::identity::restore();

    // Started this early, before `JNI_OnLoad`, so the AT-SPI bus connection
    // (a D-Bus round trip) has as much time as possible to finish before the
    // engine's first `AccessibilityManager.isEnabled()` check — the whole
    // point of `native/accessibility.cpp` reading a plain atomic there rather
    // than blocking on D-Bus is wasted if this is started too late for the
    // atomic to have flipped by the time it matters. Not a hard ordering
    // guarantee (the bridge thread and the engine's own load sequence race),
    // but every millisecond of head start narrows that race rather than
    // widening it.
    cordial_runtime::android::accessibility::start();

    // Before the engine loads, so the governor is already up when the shader
    // compiles and the asset cache warms — the part of a launch most obviously
    // bound by a CPU that has not been asked to hurry yet.
    gamemode::register();

    let table = symtab::build(opt.host_libc);
    let totals = table.totals();

    println!(
        "symbol table: {} cordial, {} host, {} stub, {} total across {} libraries",
        totals.cordial,
        totals.host,
        totals.stub,
        totals.cordial + totals.host + totals.stub,
        table.libraries.len()
    );
    for (lib, s) in &table.stats {
        println!(
            "  {lib:<20} cordial={:<4} host={:<5} stub={}",
            s.cordial, s.host, s.stub
        );
    }
    for missing in &table.missing_host_libs {
        println!("  warning: host {missing} unavailable; its symbols are stubbed");
    }

    if opt.verbose {
        for (lib, entries) in &table.libraries {
            for e in entries {
                println!(
                    "  {lib:<20} {:<44} {}",
                    e.symbol,
                    e.source.label()
                );
            }
        }
    }

    if let Some(secs) = opt.window_seconds {
        match cordial_runtime::android::gl::probe_window(&table, secs) {
            Ok(r) => {
                println!("\nGL probe rendered into a real window (this is a test pattern, not Roblox):");
                println!("  renderer  {}", r.renderer);
                println!("  version   {}", r.version);
                println!("  readback  {:02x?}", r.pixel);
            }
            Err(e) => {
                eprintln!("\nwindow render failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if opt.gl_probe {
        match cordial_runtime::android::gl::probe(&table) {
            Ok(r) => {
                println!("\nGLES2 context is live:");
                println!("  vendor    {}", r.vendor);
                println!("  renderer  {}", r.renderer);
                println!("  version   {}", r.version);
                println!("  readback  {:02x?} — drew and read it back", r.pixel);
            }
            Err(e) => {
                eprintln!("\nGL probe failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!("\ninitialising bionic linker...");
    linker::init();

    for (name, entries) in &table.libraries {
        let symbols: Vec<(String, *mut std::ffi::c_void)> = entries
            .iter()
            .map(|e| (e.symbol.to_string(), e.address))
            .collect();
        if let Err(e) = linker::register(name, &symbols) {
            eprintln!("failed to register {name}: {e}");
            return ExitCode::FAILURE;
        }
    }
    println!("registered {} virtual libraries", table.libraries.len());

    if let Err(e) = linker::set_library_path(&opt.lib_dir) {
        eprintln!("bad --lib-dir: {e}");
        return ExitCode::FAILURE;
    }
    println!("search path: {}", opt.lib_dir);

    println!("\nloading {} ...", opt.library);

    let start = Instant::now();
    let result = linker::dlopen(&opt.library, linker::RTLD_NOW);
    let elapsed = start.elapsed();

    let lib = match result {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("\nLOAD FAILED after {:.0?}: {e}", elapsed);
            stubs::report();
            return ExitCode::FAILURE;
        }
    };

    let (code_base, code_size) = lib.code_region();
    println!("\nLOADED in {:.0?}", elapsed);
    println!("  base       {:#x}", lib.base());
    println!(
        "  code       {code_base:#x} + {code_size} bytes ({:.1} MB)",
        code_size as f64 / (1024.0 * 1024.0)
    );

    match lib.symbol("JNI_OnLoad") {
        Some(p) => println!("  JNI_OnLoad {p:p}"),
        None => println!("  JNI_OnLoad not found"),
    }

    if opt.jni_onload {
        if let Some(p) = lib.symbol("JNI_OnLoad") {
            let Some(vm) = linker::jni::create_vm() else {
                eprintln!("could not create a JavaVM");
                return ExitCode::FAILURE;
            };
            println!("\nJavaVM at {vm:p}; calling JNI_OnLoad");

            match linker::jni::call_on_load(p) {
                // JNI versions are 0x000M_000m; 0x00010006 is JNI_VERSION_1_6.
                Ok(rc) => {
                    println!("JNI_OnLoad returned {rc:#x} = JNI {}.{}", rc >> 16, rc & 0xffff);
                }
                Err(e) => {
                    println!("JNI_OnLoad failed: {e}");
                    println!(
                        "\n  Roblox expects the Android bring-up sequence, not a bare JNI_OnLoad:\n                           a JavaVM, then GameActivity.initializeNativeCode called from Java with a\n                           real Activity. See docs/framework-api-inventory.md §3.3."
                    );
                }
            }

            if opt.game_activity {
                let skip_agdk = std::env::var_os("CORDIAL_SKIP_AGDK").is_some();
                let native = if skip_agdk {
                    // ActivityNativeMain, the manifest's real launch target, does
                    // not extend GameActivity. Driving both bring-ups at once
                    // creates AGDK's game thread, which then reads app-bridge
                    // state that only Cordial's calling thread ever touched.
                    println!("\nskipping AGDK; driving the app bridge alone");
                    None
                } else {
                    lib.symbol(
                        "Java_com_google_androidgamesdk_GameActivity_initializeNativeCode",
                    )
                };
                if skip_agdk {
                    // The bridge sequence, without a handle and without AGDK.
                    let (rw, rh) = requested_resolution();
                    match cordial_runtime::android::open_window(
                        rw, rh, &cordial_shell::host_window::title(),
                    ) {
                        Err(e) => println!("  no window: {e}"),
                        Ok(w) => {
                            let (width, height, _) = w.geometry();
                            cordial_runtime::android::config::set_screen(width, height);
                            let apk_path = asset_folder(&opt.apk);
                            // Order taken from a Waydroid capture of the real
                            // Android client (docs/traces/render-bringup-sequence.log),
                            // which logs:
                            //   nativeAppBridgeAppStart
                            //   nativeAppBridgeV2Init
                            //   nativeAppBridgeStartLuaAppDM
                            //   nativeAppBridgeV2StartApp
                            // StartLuaAppDM comes BEFORE StartApp. An earlier
                            // experiment here swapped them the other way on the
                            // strength of a crash appearing to move; the capture
                            // says that was backwards.
                            //
                            // Superseded note: the engine spawns its
                            // own 'Main' thread inside nativeGameGlobalInit,
                            // which independently races through the same
                            // StartLuaAppDM machinery our own explicit call
                            // drives. Calling StartAppWithParams — which
                            // delivers the surface — *before* StartLuaAppDM
                            // lets that background thread's own progress get
                            // substantially further (from dying during
                            // InitParams reflection to dying during
                            // StartAppParams/surface reflection) before it
                            // still crashes. Skipping our own StartLuaAppDM
                            // call entirely changes nothing, since the engine
                            // calls it on that background thread regardless
                            // — so it stays here, last, for parity with the
                            // engine's own onCreate order, but is provably
                            // redundant for this particular crash.
                            for (name, run) in [
                                ("nativeGameGlobalInit", 0),
                                ("nativeUpdateAdapterInit", 0),
                                ("nativeAppBridgeV2InitWithParams", 1),
                                ("nativeAppBridgeStartLuaAppDM", 0),
                                ("nativeAppBridgeV2StartAppWithParams", 2),
                            ] {
                                let sym = format!(
                                    "Java_com_roblox_engine_jni_NativeGLInterface_{name}"
                                );
                                let Some(f) = lib.symbol(&sym) else {
                                    println!("  {name} not exported");
                                    continue;
                                };
                                let r = match run {
                                    1 => linker::game_activity::appbridge_init(
                                        f, &apk_path, width, height,
                                    ),
                                    2 => linker::game_activity::appbridge_start_app(
                                        f, &apk_path, width, height,
                                    ),
                                    _ => linker::game_activity::appbridge_call_bare(f),
                                };
                                match r {
                                    Ok(()) => println!("  {name} ok"),
                                    Err(e) => println!("  {name} failed: {e}"),
                                }
                            }
                            println!("  pumping for {}s", opt.run_seconds);
                            // No AGDK handle on this path — it drives the app
                            // bridge directly and never calls
                            // initializeNativeCode, so onTouchEventNative etc.
                            // are never registered to deliver input to.
                            cordial_runtime::android::looper::pump(
                                std::time::Duration::from_secs(opt.run_seconds),
                                None,
                            );
                        }
                    }
                }

                match native {
                    None if !skip_agdk => eprintln!("  initializeNativeCode is not exported"),
                    None => {}
                    Some(f) => {
                        // `initStorageManagerNativeV3` takes *two different*
                        // directories. The Waydroid capture shows the real
                        // client using `<app>/files` and `<app>/cache`, with the
                        // engine putting `cache/flag_cache.dat` and
                        // `cache/tombstone.dat` under the second one. Cordial was
                        // passing a single path twice — and one that had never
                        // been created, since nothing here calls `mkdir`. An
                        // Android app's `files` and `cache` dirs always exist by
                        // the time any app code runs, so the engine is entitled
                        // to assume it.
                        // `profile::active()` rather than a second hand-rolled
                        // path: this used to compute `instances/default` here
                        // while everything else in the process had moved to
                        // `profiles/<name>`, so the engine's own storage ended up
                        // in a directory nothing else looked at.
                        let root = std::env::var("CORDIAL_FILES_DIR").unwrap_or_else(|_| {
                            format!("{}/data", cordial_runtime::profile::active().display())
                        });
                        let files = format!("{root}/files");
                        let cache = format!("{root}/cache");
                        for d in [&files, &cache] {
                            if let Err(e) = std::fs::create_dir_all(d) {
                                println!("  could not create {d}: {e}");
                            }
                        }
                        // Android's framework prepares the UI thread's looper
                        // before any app code runs, and AGDK's
                        // initializeNativeCode bails out with a zero handle if
                        // ALooper_forThread returns null. Nothing else prepares
                        // one here.
                        if !cordial_runtime::android::looper::prepare_for_current_thread() {
                            eprintln!("  could not prepare a looper for this thread");
                            return ExitCode::FAILURE;
                        }

                        // Client settings before initializeNativeCode.
                        // The engine's flags verdict is reported from a thread
                        // that initializeNativeCode starts, and it was arriving
                        // before any later delivery could possibly matter --
                        // every ordering tried downstream of this point still
                        // lost the race, because the decision had already been
                        // made. This is the last position that is actually
                        // earlier than the decision.
                        // Skipped when `bootstrapTheApp` is going to do the
                        // delivery, which is the default. Both running meant the
                        // engine registered a flag provider per call: Cordial
                        // logged `Registered Flag Provider ID from Java:` 0, 1
                        // and 2 on one launch where Sober logs 0 and nothing
                        // else. Whether repeated registration is harmful is not
                        // established, but matching the real client costs
                        // nothing and an unnecessary difference on the path
                        // being investigated is worth removing.
                        if let Some(f) = lib
                            .symbol(
                                "Java_com_roblox_engine_jni_NativeGLInterface_nativeInitClientSettings",
                            )
                            // `CORDIAL_EARLY_SETTINGS=1` decouples this from
                            // the bootstrap switch, which is a combination
                            // nobody has run.
                            //
                            // The early call was added because the first
                            // `flags FAILED` was seen arriving before
                            // `nativeInitClientSettings` had been called at
                            // all — the settings were being delivered after
                            // the decision they were meant to inform. But it
                            // was wired behind `CORDIAL_NO_BOOTSTRAP`, so
                            // "settings before `initializeNativeCode`" and
                            // "no bootstrap" have only ever been true together,
                            // and the useful half has never been tested alone.
                            //
                            // §12 measured the verdict being reached inside
                            // `initializeNativeCode`, before any settings call.
                            // If the engine wants its flags already present when
                            // that runs, this is the shape of the fix and the
                            // coupling is why it looked like it had been ruled
                            // out. Off by default: it is an experiment, and
                            // shipping an inference as a default is a mistake
                            // this file has made once already.
                            .filter(|_| {
                                (std::env::var_os("CORDIAL_NO_BOOTSTRAP").is_some()
                                    || std::env::var_os("CORDIAL_EARLY_SETTINGS").is_some())
                                    && std::env::var_os("CORDIAL_LATE_SETTINGS").is_none()
                            })
                        {
                            let settings = cordial_runtime::client_settings::load(
                                opt.client_settings.as_deref(),
                            )
                            .unwrap_or_default();
                            match linker::game_activity::init_client_settings(
                                f, &settings, "", "",
                            ) {
                                Ok(code) => {
                                    println!("  early client settings ({} bytes) -> {code}", settings.len())
                                }
                                Err(e) => println!("  early client settings failed: {e}"),
                            }
                        }

                        // Install the bootstrap before the engine can call it.
                        // `initializeNativeCode` calls `bootstrapTheApp` and
                        // reads the flags verdict immediately after, so this is
                        // the last line at which it can be installed at all.
                        //
                        // `CORDIAL_NO_BOOTSTRAP=1` is the control: it leaves the
                        // hook installed but with nothing behind it, which
                        // reproduces the old behaviour in the same session.
                        // `CORDIAL_LATE_SETTINGS=1` moves the whole handshake
                        // to after the app bridge, where Sober does it. Sober's
                        // FLog file has nativeAppBridgeV2Init at 3.901, 200ms
                        // AFTER RbxStorage::init; Cordial's has it at 1.781 as
                        // the first line in the file. See flag-init.md §11.7.
                        let late = std::env::var_os("CORDIAL_LATE_SETTINGS").is_some();
                        if std::env::var_os("CORDIAL_NO_BOOTSTRAP").is_none() && !late {
                            const FLAG_NAMES: &str = include_str!("../native-flag-names.txt");
                            let plan = BootstrapPlan {
                                settings_native: lib
                                    .symbol("Java_com_roblox_engine_jni_NativeGLInterface_nativeInitClientSettings")
                                    .map_or(0, |p| p as usize),
                                post_native: lib
                                    .symbol("Java_com_roblox_engine_jni_NativeGLInterface_nativePostClientSettingsLoadedInitialization3")
                                    .map_or(0, |p| p as usize),
                                flags_native: lib
                                    .symbol("Java_com_roblox_client_flags_FlagJniInterface_nativeInitializeNativeFlags")
                                    .map_or(0, |p| p as usize),
                                settings: cordial_runtime::client_settings::load(
                                    opt.client_settings.as_deref(),
                                )
                                .unwrap_or_default(),
                                flag_names: FLAG_NAMES.to_string(),
                            };
                            let _ = BOOTSTRAP.set(plan);
                            linker::game_activity::set_bootstrap(Some(run_bootstrap));
                            println!("  bootstrapTheApp installed");
                        } else {
                            println!("  bootstrapTheApp NOT installed (CORDIAL_NO_BOOTSTRAP)");
                        }

                        println!("\ncalling GameActivity.initializeNativeCode");
                        match linker::game_activity::initialize(f, &files, &files, &files) {
                            Ok(handle) => {
                                println!("  native handle {handle:#x}");

                                // The engine renders into an ANativeWindow, so
                                // there has to be a real one before the surface
                                // callbacks arrive.
                                let (rw, rh) = requested_resolution();
                                match cordial_runtime::android::open_window(
                                    rw, rh, &cordial_shell::host_window::title(),
                                ) {
                                    Err(e) => println!("  no window: {e}"),
                                    Ok(w) => {
                                        let (width, height, format) = w.geometry();
                                        cordial_runtime::android::config::set_screen(width, height);
                                        println!("  window {width}x{height}");
                                        cordial_runtime::android::config::set_screen(width, height);

                                        // The engine's own init sequence, in the
                                        // order MainGameActivity.onCreate runs it.
                                        // Without the asset manager the engine
                                        // cannot read its own content, which is
                                        // why nothing downstream ever starts —
                                        // no app shell, no network, no frame.
                                        // `NativeSettingsInterface` — where the
                                        // app tells the engine which directories
                                        // it owns. Nothing here called these, so
                                        // the engine resolved every path it built
                                        // from them against the working
                                        // directory: `./appData`, `cache`,
                                        // `http`, `sounds`. The capture shows the
                                        // real client using absolute paths under
                                        // its own storage for all of them.
                                        // Signatures read out of the shipping
                                        // APK's dex.
                                        const SETTINGS: &str =
                                            "com/roblox/engine/jni/NativeSettingsInterface";
                                        let external = format!("{root}/external");
                                        let _ = std::fs::create_dir_all(&external);
                                        let dirs: &[(&str, Vec<&str>)] = &[
                                            (
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetFilesDirectory",
                                                vec![files.as_str()],
                                            ),
                                            (
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetCacheDirectory",
                                                vec![cache.as_str()],
                                            ),
                                            (
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetExternalDirectory",
                                                vec![external.as_str()],
                                            ),
                                            (
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetBaseDataDirectories",
                                                vec![files.as_str(), cache.as_str()],
                                            ),
                                        ];
                                        let assets_now = asset_folder(&opt.apk);
                                        let engine_ver = engine_version(&opt.lib_dir)
                                            .unwrap_or_default();
                                        // Read by `build_user_agent` on the C++
                                        // side, which has no other route to it.
                                        if !engine_ver.is_empty() {
                                            std::env::set_var("CORDIAL_ENGINE_VERSION", &engine_ver);
                                        }
                                        if engine_ver.is_empty() {
                                            println!("  engine version not readable from libroblox.so; not setting one");
                                        } else {
                                            println!("  engine version {engine_ver} (read from the binary)");
                                        }
                                        // The preferences file. `INFERRED`: no
                                        // capture line names it, unlike the app
                                        // policy below. The path is where the
                                        // engine already writes
                                        // `GlobalBasicSettings_13.xml` of its own
                                        // accord, so this tells it the name it
                                        // had picked anyway rather than moving
                                        // anything. If it turns out to change
                                        // nothing, say so and delete it — issue
                                        // #5 asks for that answer, not for the
                                        // call.
                                        let prefs =
                                            format!("{files}/appData/GlobalBasicSettings_13.xml");
                                        let dirs2: &[(&str, &str, Vec<&str>)] = &[
                                            (
                                                // **The one difference from Sober
                                                // that is established rather than
                                                // suspected.** Sober's own log
                                                // reports
                                                //
                                                //   rbx.JNIRobloxSettings: Setting
                                                //   default app policy file:
                                                //   content/guac/defaultConfigs/
                                                //   GuacDefaultPolicy-GlobalDist.json
                                                //
                                                // and `docs/traces/` shows the real
                                                // Android client logging that exact
                                                // line. Cordial never called this,
                                                // so the engine ran with no app
                                                // policy at all.
                                                //
                                                // Relative, not absolute, because
                                                // both the capture and Sober log it
                                                // relative — it resolves under the
                                                // asset root that
                                                // `nativeSetAssetPath` sets, and
                                                // the APK carries the file at
                                                // `assets/content/guac/...`.
                                                //
                                                // GlobalDist of the three the APK
                                                // ships (CJVDist and VNGGamesDist
                                                // are the other two) because that
                                                // is the one the capture uses and
                                                // the one named in the real
                                                // client's User-Agent as
                                                // `(GlobalDist; GooglePlayStore)`.
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetDefaultAppPolicyFile",
                                                SETTINGS,
                                                vec![
                                                    "content/guac/defaultConfigs/GuacDefaultPolicy-GlobalDist.json",
                                                ],
                                            ),
                                            (
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetPreferencesFile",
                                                SETTINGS,
                                                vec![prefs.as_str()],
                                            ),
                                            (
                                                "Java_com_roblox_client_startup_MainGameActivity_nativeSetAssetPath",
                                                "com/roblox/client/startup/MainGameActivity",
                                                vec![assets_now.as_str()],
                                            ),
                                            (
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetRobloxVersion",
                                                SETTINGS,
                                                // Read out of the binary by
                                                // `engine_version`. See there for
                                                // why this is no longer a literal.
                                                vec![engine_ver.as_str()],
                                            ),
                                            (
                                                // The engine fetches its own
                                                // settings from
                                                // `clientsettingscdn.roblox.com/v2/
                                                // settings-compressed/application/
                                                // <name>.zst` and was asking for
                                                // `application/.zst` -- an EMPTY
                                                // name -- then taking the 403 and
                                                // reporting `Could not fetch
                                                // settings`. It does not know what
                                                // application it is because
                                                // nothing told it. `AndroidApp` is
                                                // not a guess: it is the name
                                                // `client_settings.rs` established
                                                // by experiment, where
                                                // AndroidClient, AndroidPlayer,
                                                // AndroidClientSettings and
                                                // AndroidAppSettings all return
                                                // HTTP 400 "The application name is
                                                // invalid" and this one returns the
                                                // real document. Verified again
                                                // here: that URL with `AndroidApp`
                                                // serves 302080 bytes.
                                                // ...and the reasoning above,
                                                // which is preserved because it
                                                // is still true, belongs to a
                                                // different question. That URL
                                                // is where the *settings
                                                // document* is fetched from and
                                                // `AndroidApp` is the right
                                                // application name for it. This
                                                // call is not that. It tells the
                                                // engine which channel platform
                                                // the *application* is, and the
                                                // two got conflated.
                                                //
                                                // `GoogleAndroidApp` is what the
                                                // real app passes, read out of
                                                // the dex rather than guessed:
                                                // the literal appears twice
                                                // there and zero times in
                                                // `libroblox.so`, while
                                                // `AndroidApp` appears three
                                                // times in the engine and zero
                                                // in the dex. Two strings, two
                                                // jobs, and this one had the
                                                // other's value.
                                                //
                                                // mocktail passes
                                                // `GoogleAndroidApp` here and
                                                // reaches `RbxStorage::init`;
                                                // Cordial passed `AndroidApp`
                                                // and does not. Whether that is
                                                // why is **not** established --
                                                // see the run recorded in the
                                                // commit, which changed nothing
                                                // measurable.
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeOverrideChannelPlatformName",
                                                SETTINGS,
                                                vec!["GoogleAndroidApp"],
                                            ),
                                            (
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetRobloxChannel",
                                                SETTINGS,
                                                // "the live channel is the empty
                                                // one" was wrong. Sober's engine
                                                // log says `The channel is
                                                // production` on the same APK, and
                                                // with the empty string the engine
                                                // wrote a `channel` preference with
                                                // an empty value and logged no
                                                // `ClientRunInfo` at all.
                                                vec![std::env::var("CORDIAL_CHANNEL").unwrap_or_else(|_| "production".into()).leak()],
                                                // `nativeSetBaseUrl` is exported
                                                // and still not called. The dex
                                                // settles its prototype --
                                                // `(Ljava/lang/String;Ljava/lang/
                                                // String;)V`, which is why an
                                                // earlier one-string guess killed
                                                // the process -- but calling it
                                                // with the same origin twice makes
                                                // the engine stop considering
                                                // itself signed in: the deeplink
                                                // join then refuses with "Signing
                                                // in is required before a join can
                                                // proceed". So the second argument
                                                // is not a second copy of the
                                                // first, and until somebody knows
                                                // what it is, not calling this is
                                                // better than calling it wrong.
                                            ),
                                        ];
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetDeviceInfo",
                                        ) {
                                            match linker::game_activity::set_device_info(
                                                f, width, height,
                                            ) {
                                                Ok(()) => println!("  device info set"),
                                                Err(e) => println!("  nativeSetDeviceInfo failed: {e}"),
                                            }
                                        }

                                        // What the display can do, alongside
                                        // the device info just above -- see
                                        // `wire_refresh_rate` for what this
                                        // does and does not establish.
                                        wire_refresh_rate(lib);

                                        // The content store, after the
                                        // directories above are set and before
                                        // anything asks for an asset. The engine
                                        // reports "RbxStorage is not initialized"
                                        // on every run without this.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_client_LocalStorageManager_initStorageManagerNativeV3",
                                        ) {
                                            match linker::game_activity::init_storage_manager(
                                                f, &files, &cache,
                                            ) {
                                                Ok(()) => println!("  storage manager initialised"),
                                                Err(e) => println!("  initStorageManagerNativeV3 failed: {e}"),
                                            }
                                        } else {
                                            println!("  initStorageManagerNativeV3 not exported");
                                        }

                                        for (name, cls, args) in dirs2 {
                                            match lib.symbol(name) {
                                                None => println!("  {name} not exported"),
                                                Some(f) => match linker::game_activity::call_static_strings(
                                                    f, cls, args,
                                                ) {
                                                    Ok(()) => println!(
                                                        "  {} ok",
                                                        name.rsplit('_').next().unwrap_or(name)
                                                    ),
                                                    Err(e) => println!("  {name} failed: {e}"),
                                                },
                                            }
                                        }

                                        for (name, args) in dirs {
                                            match lib.symbol(name) {
                                                None => println!("  {name} not exported"),
                                                Some(f) => match linker::game_activity::call_static_strings(
                                                    f, SETTINGS, args,
                                                ) {
                                                    Ok(()) => println!(
                                                        "  {} ok",
                                                        name.rsplit('_').next().unwrap_or(name)
                                                    ),
                                                    Err(e) => println!("  {name} failed: {e}"),
                                                },
                                            }
                                        }

                                        // The cookie natives, resolved here
                                        // and used later.
                                        //
                                        // The engine keeps its cookie jar in
                                        // memory only — measured, not assumed:
                                        // a full `CORDIAL_TRACE_PATHS=1`
                                        // inventory of every file it opens has
                                        // no cookie jar in it. On Android the
                                        // Java side persists them and hands
                                        // them back at startup, and Cordial has
                                        // no Java side, which is the whole of
                                        // why signing in and restarting
                                        // presented as being logged out.
                                        //
                                        // The handler is registered *here*, as
                                        // early as it resolves, because it only
                                        // reports changes: one registered after
                                        // a `Set-Cookie` has already been dealt
                                        // with never hears about that cookie.
                                        // Restoring has to wait, and the call
                                        // that does it sits after the app
                                        // bridge with the measurement that put
                                        // it there.
                                        if cordial_runtime::cookies::enabled() {
                                            match lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetMultipleCookies",
                                            ) {
                                                None => println!(
                                                    "  [cookies] nativeSetMultipleCookies not exported; a saved session cannot be restored"
                                                ),
                                                // SAFETY: the symbol resolved
                                                // under its own name, so it is
                                                // the static native this
                                                // signature describes.
                                                Some(f) => unsafe {
                                                    cordial_runtime::cookies::set_push(f)
                                                },
                                            }
                                            match lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeGetCookiesForDomain",
                                            ) {
                                                None => println!(
                                                    "  [cookies] nativeGetCookiesForDomain not exported; a session cannot be saved"
                                                ),
                                                // SAFETY: as above.
                                                Some(f) => unsafe {
                                                    cordial_runtime::cookies::set_pull(f)
                                                },
                                            }

                                            // The engine's own notification that
                                            // its jar changed. Verified firing
                                            // four times in the Waydroid capture
                                            // on a logged-out start — the device
                                            // and tracking cookies exercise the
                                            // identical plumbing an auth cookie
                                            // does.
                                            match lib.symbol(
                                                "Java_com_roblox_universalapp_cookie_JNICookieProtocol_updateOnSetCookieHandler",
                                            ) {
                                                None => println!(
                                                    "  [cookies] updateOnSetCookieHandler not exported; cookie changes will not be noticed"
                                                ),
                                                Some(f) => match linker::game_activity::cookies_register_handler(
                                                    f,
                                                    cordial_runtime::cookies::observe_host,
                                                ) {
                                                    Ok(()) => println!("  [cookies] OnSetCookieHandler registered"),
                                                    Err(e) => println!("  [cookies] updateOnSetCookieHandler failed: {e}"),
                                                },
                                            }
                                        } else {
                                            println!("  [cookies] persistence off (CORDIAL_SKIP_COOKIES)");
                                        }

                                        let files = files.clone();
                                        let cache = cache.clone();
                                        let steps: Vec<(&str, Box<dyn Fn(*mut std::ffi::c_void) -> Result<(), String>>)> = vec![
                                            (
                                                "Java_com_roblox_client_JNIAAssetManagerSetup_initNative",
                                                Box::new(linker::game_activity::asset_manager_init),
                                            ),
                                            (
                                                "Java_com_roblox_client_LocalStorageManager_initStorageManagerNativeV3",
                                                Box::new(move |f| {
                                                    linker::game_activity::storage_init(f, &files, &cache)
                                                }),
                                            ),
                                        ];
                                        for (name, run) in steps {
                                            match lib.symbol(name) {
                                                None => println!("  {name} not exported"),
                                                Some(f) => match run(f) {
                                                    Ok(()) => println!(
                                                        "  {} ok",
                                                        name.rsplit('_').next().unwrap_or(name)
                                                    ),
                                                    Err(e) => println!("  {name} failed: {e}"),
                                                },
                                            }
                                        }

                                        // `ILocalStorageHandlerCore.setPlatformImpl`.
                                        // **Measured to crash, and left off by
                                        // default because of it.** The call
                                        // itself returns cleanly -- "setPlatformImpl
                                        // ok" prints -- but the engine's own
                                        // djinni glue starts throwing
                                        // `[JNIVM] Exception ... djinni
                                        // (djinni_support.cpp:529): weakRef`
                                        // immediately afterwards, a dozen
                                        // times in one run, and the process
                                        // goes on to SIGSEGV inside libc's
                                        // `_IO_fflush` a few calls later --
                                        // the same crash SIGNATURE
                                        // docs/analysis/flag-init.md §7.4/§15
                                        // records for a different native, a
                                        // fault at address 0x8, which reads as
                                        // heap corruption rather than a clean
                                        // null-check failure. A control run
                                        // with this call skipped and every
                                        // other change in this commit intact
                                        // reached `app ready: Landing` and
                                        // exited 0; the identical run with
                                        // only this call re-enabled crashed
                                        // under `lldb` inside `nativeRetryInit`,
                                        // which this call precedes but does
                                        // not touch. That is exactly the
                                        // asymmetry mocktail's own
                                        // `MOCKTAIL_LOCAL_STORAGE_SET_PLATFORM_IMPL`
                                        // defaulting *off* already warned
                                        // about (see `native/local_storage.cpp`'s
                                        // header) -- confirmed here rather
                                        // than taken on trust.
                                        //
                                        // `IPlatformLocalStorageHandler` and
                                        // `ILocalStorageHandlerCore` are
                                        // djinni-generated (the `$CppProxy`
                                        // siblings in the dex are the
                                        // giveaway), and djinni's own runtime
                                        // wraps a Java-side interface
                                        // implementation in machinery that
                                        // needs working weak global
                                        // references -- `NewWeakGlobalRef`
                                        // and friends -- which is INFERRED to
                                        // be what libjnivm does not fully
                                        // provide, since the exception names
                                        // `weakRef` specifically and starts
                                        // firing the moment the engine has
                                        // the object in hand. Not confirmed
                                        // by reading djinni_support.cpp,
                                        // which is engine code this project
                                        // does not disassemble past what
                                        // AGENTS.md allows.
                                        //
                                        // The C++ side (`PlatformLocalStorageHandler`
                                        // in `native/local_storage.cpp`) is
                                        // left in place and registered either
                                        // way -- registering a class costs
                                        // nothing until something calls a
                                        // method on it -- so
                                        // `CORDIAL_LOCAL_STORAGE_SET_PLATFORM_IMPL=1`
                                        // is enough to pick the investigation
                                        // back up without touching code.
                                        if std::env::var_os(
                                            "CORDIAL_LOCAL_STORAGE_SET_PLATFORM_IMPL",
                                        )
                                        .is_some()
                                        {
                                            match lib.symbol(
                                                "Java_com_roblox_protocols_localstorageplatforminterface_generated_ILocalStorageHandlerCore_setPlatformImpl",
                                            ) {
                                                None => println!(
                                                    "  setPlatformImpl not exported"
                                                ),
                                                Some(f) => match local_storage_set_platform_impl(f) {
                                                    Ok(()) => println!("  setPlatformImpl ok"),
                                                    Err(e) => {
                                                        println!("  setPlatformImpl failed: {e}")
                                                    }
                                                },
                                            }
                                        } else {
                                            println!(
                                                "  setPlatformImpl skipped (measured to crash \
                                                 the process a few calls later; set \
                                                 CORDIAL_LOCAL_STORAGE_SET_PLATFORM_IMPL=1 to \
                                                 try it anyway)"
                                            );
                                        }

                                        if let Some(p) = lib.symbol(
                                            "Java_com_roblox_client_startup_MainGameActivity_nativeAppBridgeSetInitParams",
                                        ) {
                                            // `PlatformParams.assetFolderPath`,
                                            // which is the same field the app
                                            // bridge gets — so it takes the same
                                            // unpacked `content` directory, not
                                            // the `.apk`. Naming the archive here
                                            // made every path the engine built
                                            // from it a file inside a file.
                                            match linker::game_activity::set_init_params(
                                                p,
                                                &asset_folder(&opt.apk),
                                                width,
                                                height,
                                            ) {
                                                Ok(()) => println!("  init params set"),
                                                Err(e) => println!("  init params failed: {e}"),
                                            }
                                        }

                                        // `NativeInputInterface.nativeUpdateScreenOrientation(I)V`.
                                        // docs/analysis/flag-init.md §16: the
                                        // one call mocktail makes between
                                        // `initializeNativeCode` and the
                                        // settings handshake that Cordial
                                        // did not. Cordial already knows the
                                        // window size and, from it, whether
                                        // the window is landscape -- the same
                                        // comparison `Configuration::Create`
                                        // in init_params.cpp already makes
                                        // for `getResources().getConfiguration()`,
                                        // so this tells the engine the same
                                        // thing through its own dedicated
                                        // entry point rather than leaving it
                                        // to infer the answer from a class it
                                        // may not read this early.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeInputInterface_nativeUpdateScreenOrientation",
                                        ) {
                                            match update_screen_orientation(f, width, height) {
                                                Ok(()) => println!("  screen orientation set"),
                                                Err(e) => println!(
                                                    "  nativeUpdateScreenOrientation failed: {e}"
                                                ),
                                            }
                                        } else {
                                            println!("  nativeUpdateScreenOrientation not exported");
                                        }

                                        // Client settings BEFORE the flag
                                        // calls. The engine reports its flags
                                        // verdict once, early, and the first
                                        // "flags FAILED" was arriving before
                                        // nativeInitClientSettings had been
                                        // called at all -- the settings were
                                        // being delivered after the decision
                                        // they were supposed to inform.
                                        // The network counterpart, called the
                                        // way the real app calls it: on
                                        // Android, Cordial's role (the host
                                        // app) is to fetch client settings and
                                        // hand the response to the engine —
                                        // the engine does not fetch its own.
                                        // `--client-settings` supplies that
                                        // real response body; the other two
                                        // string arguments' roles were not
                                        // pinned down with confidence, so
                                        // empty strings are the honest
                                        // starting point. The `int` the
                                        // engine returns is logged directly —
                                        // it is a far more reliable signal
                                        // than the onFlagsFailed/onFlagsLoaded
                                        // print, which comes from an
                                        // unrelated async path.
                                        // Only when bootstrapTheApp has not
                                        // already delivered. Running both meant
                                        // three registered flag providers on one
                                        // launch -- Cordial logged `Registered
                                        // Flag Provider ID from Java:` 0, 1 and 2
                                        // where Sober logs 0 and nothing else.
                                        // Whether repeated registration harms
                                        // anything is not established; matching
                                        // the real client costs nothing, and an
                                        // unnecessary difference on the path
                                        // under investigation is worth removing.
                                        let already = BOOTSTRAP_RAN
                                            .load(std::sync::atomic::Ordering::SeqCst)
                                            || std::env::var_os("CORDIAL_LATE_SETTINGS").is_some();
                                        if already {
                                            println!("  settings and flags already delivered by bootstrapTheApp");
                                        }
                                        if let Some(f) = lib
                                            .symbol(
                                                "Java_com_roblox_engine_jni_NativeGLInterface_nativeInitClientSettings",
                                            )
                                            .filter(|_| !already)
                                        {
                                            // Cordial is the host app, so
                                            // Cordial does the fetch the app
                                            // would do. Cached on disk, so a
                                            // repeat launch is not a repeat
                                            // request.
                                            let settings =
                                                cordial_runtime::client_settings::load(
                                                    opt.client_settings.as_deref(),
                                                )
                                                .unwrap_or_default();
                                            println!(
                                                "  client settings: {} bytes",
                                                settings.len()
                                            );
                                            // Which of the three strings is the
                                            // settings document is not
                                            // established — the descriptor is
                                            // (String,String,String)I and the
                                            // engine's only clue is a
                                            // "ParseFailure on overrides" log
                                            // string, so one of the others is
                                            // an overrides document. Selectable
                                            // rather than guessed, so the
                                            // question can be settled by
                                            // running it.
                                            let pos = std::env::var("CORDIAL_CS_POS")
                                                .ok()
                                                .and_then(|v| v.parse::<u8>().ok())
                                                .unwrap_or(0);
                                            let (a, b, c) = match pos {
                                                1 => ("", settings.as_str(), ""),
                                                2 => ("", "", settings.as_str()),
                                                // Established by experiment:
                                                // the document goes first, and
                                                // 0 comes back. See
                                                // client_settings.rs.
                                                _ => (settings.as_str(), "", ""),
                                            };
                                            match linker::game_activity::init_client_settings(
                                                f, a, b, c,
                                            ) {
                                                Ok(code) => println!(
                                                    "  nativeInitClientSettings -> {code}"
                                                ),
                                                Err(e) => println!(
                                                    "  nativeInitClientSettings failed: {e}"
                                                ),
                                            }
                                        }
                                        // NOT called by default: passing an
                                        // empty `ArrayList` here reproducibly
                                        // crashes synchronously, on this
                                        // thread, inside libc's `_IO_fflush`
                                        // (fault address 0x8 — a near-null
                                        // pointer a small struct offset in),
                                        // verified live under lldb. That is
                                        // worse than the pre-existing
                                        // asynchronous crash this session set
                                        // out to leave alone, so this call is
                                        // wired but disabled pending a real
                                        // list argument. See the report for
                                        // detail. It is unconditional now:
                                        // that crash was a CONSEQUENCE of the
                                        // settings not being accepted, and with
                                        // nativeInitClientSettings returning 0
                                        // this call succeeds.
                                            if let Some(f) = lib
                                                .symbol(
                                                    "Java_com_roblox_engine_jni_NativeGLInterface_nativePostClientSettingsLoadedInitialization3",
                                                )
                                                .filter(|_| !already)
                                            {
                                                match linker::game_activity::post_client_settings_loaded(f) {
                                                    Ok(()) => println!(
                                                        "  postClientSettingsLoadedInitialization3 ok"
                                                    ),
                                                    Err(e) => println!(
                                                        "  postClientSettingsLoadedInitialization3 failed: {e}"
                                                    ),
                                                }
                                            }

                                        // Flags before anything else asks for
                                        // them: bootstrapTheApp's whole job is to
                                        // reach this, and the engine reports
                                        // onFlagsFailed without it.
                                        if let Some(f) = lib
                                            .symbol(
                                                "Java_com_roblox_client_flags_FlagJniInterface_nativeInitializeNativeFlags",
                                            )
                                            .filter(|_| !already)
                                        {
                                            // Flag NAMES, not the settings
                                            // document — the engine loads
                                            // values itself. Feeding the
                                            // document here was a bug once
                                            // already, so it is deliberately
                                            // NOT client_settings::load().
                                            //
                                            // The list is built in because the
                                            // real client always passes it: a
                                            // Waydroid capture of this APK logs
                                            // "flagCount = 139" and names each
                                            // one. See docs/traces/README.md.
                                            // An explicit --client-settings file
                                            // still overrides it, for
                                            // experimenting with other lists.
                                            const FLAG_NAMES: &str = include_str!(
                                                "../native-flag-names.txt"
                                            );
                                            let settings = opt
                                                .client_settings
                                                .as_deref()
                                                .and_then(|p| std::fs::read_to_string(p).ok())
                                                .unwrap_or_else(|| FLAG_NAMES.to_string());
                                            println!(
                                                "  flag names: {}",
                                                settings.lines().filter(|l| !l.trim().is_empty()).count()
                                            );
                                            match linker::game_activity::init_flags(f, &settings) {
                                                Ok(()) => println!("  flags initialised"),
                                                Err(e) => println!("  flag init failed: {e}"),
                                            }
                                        }

                                        // `--flag-overrides <f>`: JSON handed
                                        // straight through to
                                        // nativePreloadFlagOverrides, so
                                        // candidate payload shapes can be
                                        // compared against their effect on the
                                        // flags verdict and JNI trace. This was
                                        // previously parsed but never actually
                                        // wired to a call — the "no extra
                                        // logging" result recorded earlier in
                                        // docs/analysis/flag-init.md was
                                        // therefore not a real negative
                                        // result; nothing was ever invoked.
                                        if let Some(json) = opt.flag_overrides.as_deref() {
                                            // `opt.flag_overrides` already holds the
                                            // *file contents* (read at argument-parsing
                                            // time, below) — not a path. An earlier
                                            // version of this call re-read it as if it
                                            // were a path, which silently failed and
                                            // passed an empty string through; that is
                                            // almost certainly why the FLog-channel
                                            // experiment recorded in
                                            // docs/analysis/flag-init.md produced no
                                            // extra logging. Fixed here.
                                            if let Some(f) = lib.symbol(
                                                "Java_com_roblox_client_startup_MainGameActivity_nativePreloadFlagOverrides",
                                            ) {
                                                match linker::game_activity::preload_flag_overrides(
                                                    f, json,
                                                ) {
                                                    Ok(()) => println!(
                                                        "  flag overrides preloaded ({} bytes)",
                                                        json.len()
                                                    ),
                                                    Err(e) => println!(
                                                        "  nativePreloadFlagOverrides failed: {e}"
                                                    ),
                                                }
                                            } else {
                                                println!(
                                                    "  nativePreloadFlagOverrides not exported"
                                                );
                                            }
                                        }

                                        // The offline counterpart:
                                        // `readLocalFlags()` makes the engine
                                        // read whatever bundled/cached flag
                                        // defaults it has on disk, with no
                                        // network round trip and nothing
                                        // impersonating Roblox's servers.
                                        // Nothing on the `ActivityNativeMain`
                                        // chain calls this in the real app —
                                        // its only dex caller is a different
                                        // startup path — so it is otherwise
                                        // dead code here.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_readLocalFlags",
                                        ) {
                                            match linker::game_activity::read_local_flags(f) {
                                                Ok(()) => println!("  local flags read"),
                                                Err(e) => println!("  readLocalFlags failed: {e}"),
                                            }
                                        }

                                        // The in-experience web window's
                                        // protocol, read out of the engine
                                        // rather than guessed at. Account
                                        // settings and Robux both open one of
                                        // these, and with nobody answering they
                                        // do nothing at all -- no window, no
                                        // error, no log line.
                                        //
                                        // Reading only. Every name below is a
                                        // getter returning a constant the engine
                                        // already holds, so this changes no
                                        // state; what it produces is the
                                        // vocabulary the receiving half will
                                        // need, which is not yet written because
                                        // the message transport has not been
                                        // traced. See crates/cordial-runtime/
                                        // src/webview.rs for why that half is
                                        // absent rather than stubbed.
                                        {
                                            let v = cordial_runtime::webview::read_vocabulary(
                                                |name| lib.symbol(name),
                                            );
                                            cordial_runtime::webview::report(&v);
                                        }

                                        // The transport for that vocabulary:
                                        // `MessageBus.getMessageId` and
                                        // `MessageBus.doSubscribeRaw`, the two
                                        // natives `openWindow` needs. Resolved
                                        // and reported only — see
                                        // crates/cordial-runtime/src/webview.rs
                                        // for why `getMessageId` is not called
                                        // from here.
                                        {
                                            let n = cordial_runtime::webview::find_bus_natives(
                                                |name| lib.symbol(name),
                                            );
                                            cordial_runtime::webview::report_bus_natives(&n);
                                        }


                                        // Kicks the engine's initialisation once
                                        // everything it depends on is in place.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_client_startup_MainGameActivity_nativeRetryInit",
                                        ) {
                                            match linker::game_activity::call_bare(f) {
                                                Ok(()) => println!("  retryInit ok"),
                                                Err(e) => println!("  retryInit failed: {e}"),
                                            }
                                        }

                                        // Resolve the input path Roblox's own
                                        // interface reads. AGDK's
                                        // onTouchEventNative is accepted by the
                                        // engine and ignored by the Lua UI; this
                                        // is what actually moves anything.
                                        {
                                            let mv = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeInputInterface_nativePassMouseMove",
                                            ).unwrap_or(std::ptr::null_mut());
                                            let bt = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeInputInterface_nativePassMouseButton",
                                            ).unwrap_or(std::ptr::null_mut());
                                            let wh = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeInputInterface_nativePassMouseWheel",
                                            ).unwrap_or(std::ptr::null_mut());
                                            let ke = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeGLInterface_nativePassKeyEvent",
                                            ).unwrap_or(std::ptr::null_mut());
                                            let tx = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeGLInterface_nativePassText",
                                            ).unwrap_or(std::ptr::null_mut());
                                            let sy = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeGLInterface_syncTextboxTextAndCursorPosition2",
                                            ).unwrap_or(std::ptr::null_mut());
                                            let uk = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeGLInterface_updateKeyboardSize",
                                            ).unwrap_or(std::ptr::null_mut());
                                            cordial_runtime::android::input::set_input_natives(mv, bt, wh, ke, tx, sy, uk);
                                            if mv.is_null() || bt.is_null() {
                                                println!("  input: NativeInputInterface not fully exported; UI input will not work");
                                            }
                                            // Named separately from the pair
                                            // above, because a build that
                                            // exports move and button but not
                                            // wheel has a working pointer and a
                                            // dead scroll wheel — which is
                                            // exactly the report this line was
                                            // added chasing, and "UI input will
                                            // not work" would be the wrong
                                            // thing to print for it.
                                            println!(
                                                "  input: nativePassMouseWheel {}",
                                                if wh.is_null() { "NOT exported; the scroll wheel will do nothing" } else { "resolved" }
                                            );

                                            // The one native on this interface
                                            // Cordial reads rather than writes:
                                            // whether the engine wants the
                                            // pointer locked to the window
                                            // centre. Nothing had ever called
                                            // it, so a first-person camera had
                                            // no way to ask for the cursor.
                                            // See `input::engine_wants_pointer_lock`
                                            // for what is still INFERRED about
                                            // which direction it is meant to be
                                            // read in.
                                            let ml = lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeInputInterface_nativeGetMainWindowIsMouseLockedCenter",
                                            ).unwrap_or(std::ptr::null_mut());
                                            cordial_runtime::android::input::set_mouse_lock_native(ml);
                                            println!(
                                                "  input: nativeGetMainWindowIsMouseLockedCenter {}",
                                                if ml.is_null() {
                                                    "NOT exported; pointer capture falls back to the mouse button alone"
                                                } else {
                                                    "resolved"
                                                }
                                            );
                                        }

                                        // A read-only probe of the engine's own
                                        // verdict on whether login is rendered
                                        // by the Lua app shell rather than a
                                        // WebView — the question that decides
                                        // whether an embedded browser is needed
                                        // at all. See docs/design/sign-in.md.
                                        //
                                        // Behind a switch because it is a tool
                                        // for whoever is working on sign-in, not
                                        // something every launch should print.
                                        // It calls an exported boolean native
                                        // and prints the answer; it drives no UI
                                        // and enters no credentials.
                                        if std::env::var_os("CORDIAL_SIGNIN_PROBE").is_some() {
                                            match lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeIsLuaLoginEnabled",
                                            ) {
                                                None => println!(
                                                    "  [sign-in] nativeIsLuaLoginEnabled not exported"
                                                ),
                                                Some(f) => match linker::game_activity::call_static_bare_bool(
                                                    f, SETTINGS,
                                                ) {
                                                    Ok(v) => println!(
                                                        "  [sign-in] nativeIsLuaLoginEnabled() -> {v}"
                                                    ),
                                                    Err(e) => println!(
                                                        "  [sign-in] nativeIsLuaLoginEnabled() failed: {e}"
                                                    ),
                                                },
                                            }
                                        }

                                        // Android's Application.ActivityLifecycleCallbacks
                                        // order. The engine stores per-Activity
                                        // context as these fire, and nothing was
                                        // driving them — which is why it held a
                                        // null JNIEnv on the game thread.
                                        {
                                            const PREFIX: &str =
                                                "Java_com_roblox_universalapp_activitylifecyclecallbacks_JNIActivityLifecycleCallbacks_";
                                            let activity = "com.roblox.client.ActivityNativeMain";
                                            let mut fired = 0;
                                            for stage in [
                                                "nativeOnPreCreated", "nativeOnCreated",
                                                "nativeOnPostCreated", "nativeOnPreStarted",
                                                "nativeOnStarted", "nativeOnPostStarted",
                                                "nativeOnPreResumed", "nativeOnResumed",
                                                "nativeOnPostResumed",
                                            ] {
                                                if let Some(f) =
                                                    lib.symbol(&format!("{PREFIX}{stage}"))
                                                {
                                                    match linker::game_activity::activity_lifecycle(
                                                        f, activity,
                                                    ) {
                                                        Ok(()) => fired += 1,
                                                        Err(e) => {
                                                            println!("  {stage} failed: {e}")
                                                        }
                                                    }
                                                }
                                            }
                                            println!("  activity lifecycle: {fired}/9 fired");
                                        }

                                        // Globals first. Disassembly of the
                                        // ActivityNativeMain chain gives this
                                        // order, and calling StartLuaAppDM
                                        // without them crashes on a null JNIEnv
                                        // the engine expects to have been stored
                                        // by the globals init.
                                        for name in [
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeGameGlobalInit",
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeUpdateAdapterInit",
                                        ] {
                                            match lib.symbol(name) {
                                                None => println!("  {name} not exported"),
                                                Some(f) => match linker::game_activity::appbridge_call_bare(f) {
                                                    Ok(()) => println!(
                                                        "  {} ok",
                                                        name.rsplit('_').next().unwrap_or(name)
                                                    ),
                                                    Err(e) => println!("  {name} failed: {e}"),
                                                },
                                            }
                                        }

                                        // The app bridge proper. ActivitySplash —
                                        // the only launcher Activity — defaults
                                        // to ActivityNativeMain, not the AGDK
                                        // MainGameActivity, and this is the chain
                                        // that actually brings the client up.
                                        let apk_path = asset_folder(&opt.apk);
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2InitWithParams",
                                        ) {
                                            match linker::game_activity::appbridge_init(
                                                f, &apk_path, width, height,
                                            ) {
                                                Ok(()) => println!("  app bridge initialised"),
                                                Err(e) => println!("  app bridge init failed: {e}"),
                                            }
                                        }

                                        // The §11.7 experiment, kept because it
                                        // has a result and somebody will want to
                                        // re-run it: the handshake in Sober's
                                        // position, after the bridge.
                                        //
                                        // **It never gets here.** With the
                                        // handshake moved out of
                                        // `initializeNativeCode` the engine takes
                                        // a SIGSEGV before the app bridge is
                                        // reached -- twice out of two, against a
                                        // default run and a `CORDIAL_NO_BOOTSTRAP=1`
                                        // run in the same session, neither of
                                        // which crashes and both of which reach
                                        // the bridge. So Cordial cannot simply
                                        // adopt Sober's ordering: Sober's engine
                                        // sits idle for 2.05s waiting for the
                                        // Kotlin activity to hand it settings,
                                        // and Cordial, driving the natives
                                        // directly, has already advanced past the
                                        // point where they can arrive.
                                        if std::env::var_os("CORDIAL_LATE_SETTINGS").is_some() {
                                            println!("  late settings: delivering after the app bridge");
                                            run_bootstrap();
                                        }

                                        // The saved session goes back in here,
                                        // and this position was measured rather
                                        // than chosen.
                                        //
                                        // docs/design/sign-in.md §5.2 said to
                                        // call `nativeSetMultipleCookies`
                                        // before `nativeAppBridgeSetInitParams`,
                                        // reasoning that the cookie must be in
                                        // place before the engine starts hitting
                                        // `authenticated/*`. The reasoning is
                                        // right and the position was wrong:
                                        // called that early the native returns
                                        // cleanly and does nothing at all.
                                        // `CORDIAL_COOKIE_PROBE=1` sets a marker
                                        // and reads it straight back at four
                                        // points in this sequence, and the
                                        // answer is 0 bytes at startup, 0 after
                                        // init params, and 51 from here onwards
                                        // — the engine's cookie jar does not
                                        // exist until `nativeAppBridgeV2InitWithParams`
                                        // has built it. That document has been
                                        // corrected.
                                        //
                                        // Still before `StartLuaAppDM` below,
                                        // which is what actually sets the app
                                        // shell running and produces the first
                                        // `authenticated/*` request, so the
                                        // ordering the design doc wanted is
                                        // preserved.
                                        if cordial_runtime::cookies::enabled() {
                                            match lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetMultipleCookies",
                                            ) {
                                                None => {}
                                                Some(f) => {
                                                    let n = cordial_runtime::cookies::restore(f);
                                                    println!(
                                                        "  [cookies] restored {n} domain(s) from {}",
                                                        cordial_runtime::cookies::where_kept()
                                                    );
                                                    // Whether the two natives
                                                    // agree on what a domain is,
                                                    // and whether they are up
                                                    // yet. Off by default: it
                                                    // puts a marker cookie in
                                                    // the engine's jar, which is
                                                    // fine for a diagnostic run
                                                    // and not for an ordinary
                                                    // launch.
                                                    if std::env::var_os("CORDIAL_COOKIE_PROBE").is_some() {
                                                        if let Some(g) = lib.symbol(
                                                            "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeGetCookiesForDomain",
                                                        ) {
                                                            cordial_runtime::cookies::probe(f, g, "restore");
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // And the engine's own copy of who is
                                        // signed in, which is a third place
                                        // and not a duplicate of the two
                                        // mirrors `identity::restore` fills.
                                        //
                                        // Measured, because filling the mirrors
                                        // in was not enough on its own:
                                        // `CORDIAL_TRACE_IDENTITY=1` shows the
                                        // engine asking all six of
                                        // `NativeUserJavaInterface`'s methods
                                        // four times each, being told a real
                                        // user every time, and still reaching
                                        // `app ready: Landing`. The mirrors are
                                        // what Cordial answers when asked; this
                                        // is what the engine keeps for itself.
                                        //
                                        // Here rather than earlier for the same
                                        // reason as the cookie restore above:
                                        // this class's natives return cleanly
                                        // and do nothing until
                                        // `nativeAppBridgeV2InitWithParams` has
                                        // built what they write into. Still
                                        // before `StartLuaAppDM`, which is what
                                        // starts the app shell that routes.
                                        if cordial_runtime::identity::enabled() {
                                            match lib.symbol(
                                                "Java_com_roblox_engine_jni_NativeSettingsInterface_nativeSetUserId",
                                            ) {
                                                None => println!(
                                                    "  [identity] nativeSetUserId not exported; the engine will not know who is signed in"
                                                ),
                                                Some(f) => {
                                                    if cordial_runtime::identity::push_user_id(f) {
                                                        println!("  [identity] the engine has been told which user is signed in");
                                                    }
                                                }
                                            }
                                        }

                                        // The deep link, if this launch is one.
                                        //
                                        // Here because these are the engine's
                                        // *cold start* URL natives and this is
                                        // the cold-start moment: after
                                        // `nativeAppBridgeV2InitWithParams`,
                                        // which is what builds the protocol
                                        // machinery they talk to — the same
                                        // ordering constraint the cookie
                                        // restore above is placed by — and
                                        // before `StartLuaAppDM`, which is
                                        // where `ActivityNativeMain` consults
                                        // `isColdStartDeeplinkToGame()` on
                                        // Android.
                                        if let Some(url) = &opt.join_url {
                                            let outcome =
                                                cordial_runtime::deeplink::deliver(lib, url);
                                            println!("[deeplink] outcome: {outcome:?}");
                                        }

                                        if std::env::var_os("CORDIAL_SKIP_LUA_DM").is_none() {
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeStartLuaAppDM",
                                        ) {
                                            match linker::game_activity::appbridge_call_bare(f) {
                                                Ok(()) => println!("  Lua app DataModel started"),
                                                Err(e) => println!("  StartLuaAppDM failed: {e}"),
                                            }
                                        }
                                        }

                                        // The capture puts this immediately
                                        // before nativeAppBridgeV2StartApp:
                                        //   setTaskSchedulerBackgroundMode()
                                        //     enable:false context:ASMA.start
                                        // A task scheduler still in background
                                        // mode is one that has been told not to
                                        // render.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_setTaskSchedulerBackgroundMode",
                                        ) {
                                            match linker::game_activity::call_static_bool_string(
                                                f,
                                                "com/roblox/engine/jni/NativeGLInterface",
                                                false,
                                                "ASMA.start",
                                            ) {
                                                Ok(()) => println!("  task scheduler foregrounded"),
                                                Err(e) => {
                                                    println!("  setTaskSchedulerBackgroundMode failed: {e}")
                                                }
                                            }
                                        }

                                        // And the call that delivers the surface.
                                        if let Some(f) = lib.symbol(
                                            "Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2StartAppWithParams",
                                        ) {
                                            match linker::game_activity::appbridge_start_app(
                                                f, &apk_path, width, height,
                                            ) {
                                                Ok(()) => println!("  app started with surface"),
                                                Err(e) => println!("  StartApp failed: {e}"),
                                            }
                                        }

                                        // The two `UpdateSurface...WithPlatformParams` calls,
                                        // here because this is where Sober makes them — at about
                                        // 3.79s, immediately after StartApp and before any join.
                                        //
                                        // Sober makes 87 `JNIAppBridge` calls in a session and
                                        // Cordial made 3; these were two of the missing ones, and
                                        // neither was referenced anywhere in this tree. Whether
                                        // they are what stops the server sending disconnect
                                        // reason 304 sixty-one seconds into a join is **not
                                        // established** — this is the largest measured difference
                                        // between a client that stays connected and one that does
                                        // not, which earns an experiment rather than a claim.
                                        //
                                        // `CORDIAL_SKIP_UPDATE_SURFACE=1` is the control: it
                                        // restores exactly the previous behaviour, so a session
                                        // that survives can be shown to survive *because* of
                                        // these rather than because something else moved.
                                        if std::env::var_os("CORDIAL_SKIP_UPDATE_SURFACE").is_none() {
                                            for (native, game) in [
                                                ("Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams", false),
                                                ("Java_com_roblox_engine_jni_NativeGLInterface_nativeAppBridgeV2UpdateSurfaceGameWithPlatformParams", true),
                                            ] {
                                                let which = if game { "game" } else { "app" };
                                                match lib.symbol(native) {
                                                    Some(f) => match linker::game_activity::appbridge_update_surface(
                                                        f, &apk_path, width, height, game,
                                                    ) {
                                                        Ok(()) => println!("  surface+platform params delivered ({which})"),
                                                        Err(e) => println!("  UpdateSurface {which} failed: {e}"),
                                                    },
                                                    // Not a warning to swallow: the export list is
                                                    // per build, and a rename is exactly the kind
                                                    // of change that would make this quietly stop.
                                                    None => {
                                                        println!("  UpdateSurface {which}: the engine does not export it");
                                                        cordial_runtime::unimplemented::placeholder(
                                                            &format!("nativeAppBridgeV2UpdateSurface{which}WithPlatformParams"),
                                                            "not exported by this build; not called",
                                                        );
                                                    }
                                                }
                                            }
                                        }

                                        match linker::game_activity::start(
                                            handle, width, height, format,
                                        ) {
                                            Ok(()) => {
                                                println!("  surface handed to the engine");
                                                // `setInputConnectionNative`: on real
                                                // Android this is Java calling native
                                                // code from inside
                                                // `onCreateInputConnection`, which
                                                // Cordial has no view system to
                                                // trigger — driven directly, once,
                                                // here, so the engine has somewhere
                                                // to send `setState`/
                                                // `setSoftKeyboardActive`/
                                                // `restartInput` before it ever tries.
                                                // `Ok(None)` means the native was not
                                                // registered yet, the same
                                                // not-yet-vs-failed distinction the
                                                // other AGDK natives use.
                                                match linker::game_activity::set_input_connection(handle) {
                                                    Ok(Some(())) => println!("  InputConnection registered with the engine"),
                                                    Ok(None) => println!("  setInputConnectionNative not registered yet; IME state will not reach Cordial"),
                                                    Err(e) => println!("  setInputConnectionNative failed: {e}"),
                                                }
                                                let secs = opt.run_seconds;
                                                if secs == 0 {
                                                    println!(
                                                        "  pumping the looper until the window is closed (no --run timer)"
                                                    );
                                                } else {
                                                    println!("  pumping the looper for {secs}s");
                                                }
                                                // Android's UI thread runs the
                                                // message loop; AGDK put its
                                                // pipes on this thread's looper.
                                                // The same loop also drains
                                                // host mouse/keyboard input and
                                                // delivers it through this
                                                // handle's onTouchEventNative /
                                                // onKeyDownNative/UpNative.
                                                // Plugins run alongside the
                                                // client, in their own
                                                // processes. Started here
                                                // rather than earlier so they
                                                // observe a client that is
                                                // already up, and so a plugin
                                                // that misbehaves cannot
                                                // interfere with bring-up.
                                                let n = cordial_runtime::plugin_host::start_all();
                                                if n > 0 {
                                                    println!("  {n} plugin(s) running");
                                                }

                                                // Subscribe to the engine's
                                                // openWindow before the pump
                                                // starts, the same point
                                                // `android::clipboard::arm`
                                                // is called from inside that
                                                // pump: the message bus has
                                                // to exist first, and by now
                                                // the app bridge has started.
                                                // This module cannot reach
                                                // `looper::pump` to add
                                                // itself there (off limits
                                                // for this change), so it is
                                                // called from here instead,
                                                // one call earlier than
                                                // clipboard's but after the
                                                // same precondition holds.
                                                cordial_runtime::webview::arm(|name| lib.symbol(name));

                                                cordial_runtime::android::looper::pump(
                                                    std::time::Duration::from_secs(secs),
                                                    Some(handle),
                                                );
                                                if std::env::var_os("CORDIAL_COUNT_GL").is_some() {
                                                    // What each thread is blocked on. A game thread
                                                    // waiting on a socket, a futex, or nothing at all
                                                    // are three different problems.
                                                    println!("\n  threads:");
                                                    if let Ok(dir) = std::fs::read_dir("/proc/self/task") {
                                                        for e in dir.flatten() {
                                                            let p = e.path();
                                                            let name = std::fs::read_to_string(p.join("comm"))
                                                                .unwrap_or_default().trim().to_string();
                                                            let wchan = std::fs::read_to_string(p.join("wchan"))
                                                                .unwrap_or_default().trim().to_string();
                                                            let state = std::fs::read_to_string(p.join("stat"))
                                                                .ok()
                                                                .and_then(|s| s.rsplit(')').next()
                                                                    .and_then(|r| r.split_whitespace().next())
                                                                    .map(str::to_string))
                                                                .unwrap_or_default();
                                                            println!("    {name:<18} state={state:<2} wchan={wchan}");
                                                        }
                                                    }
                                                    println!(
                                                        "  looper polls: {}",
                                                        cordial_runtime::android::looper::POLLS
                                                            .load(std::sync::atomic::Ordering::Relaxed)
                                                    );
                                                    println!("\n  graphics calls Roblox made:");
                                                    for (name, n) in
                                                        cordial_runtime::android::glcount::report()
                                                    {
                                                        println!("    {name:<24} {n}");
                                                    }
                                                }
                                            }
                                            Err(e) => println!("  lifecycle failed: {e}"),
                                        }
                                    }
                                }
                            }
                            Err(e) => println!("  failed: {e}"),
                        }
                    }
                }
            }

            if let Some(path) = &opt.dump_classes {
                match linker::jni::dump_classes(path) {
                    Ok(()) => println!("  Java classes Roblox reached for -> {path}"),
                    Err(e) => eprintln!("  class dump failed: {e}"),
                }
            }
        }
    }

    stubs::report();

    // Everything the engine asked for that Cordial could not answer, in one
    // table: JNI classes and methods libjnivm never had, libc stubs, AGDK
    // natives called while unregistered, and framework calls that returned
    // something invented. Printed and written beside the engine's own logs,
    // because the question after a failure is "what did we fail to tell it"
    // and the answer used to be spread across four kinds of line.
    cordial_runtime::unimplemented::report();

    // Before `_exit`, which runs nothing. gamemoded would notice the process
    // was gone on its own — it reaps clients whose pid has vanished — but that
    // is a poll, so leaving it implicit means the governor stays raised for
    // however long the sweep takes after a session ends.
    gamemode::unregister();

    // Leave via _exit rather than returning.
    //
    // Roblox's static initialisers registered atexit handlers and DT_FINI_ARRAY
    // destructors that expect a live Android process — a JavaVM, a working
    // looper, its own stdio. Running them here segfaults during teardown, long
    // after the load this tool exists to verify has already succeeded. Clean
    // shutdown belongs with instance lifecycle in core; until then, reporting a
    // teardown crash as a load failure would be actively misleading.
    //
    // SAFETY: _exit is async-signal-safe and terminates without running any
    // handler. Nothing here owns a resource the kernel will not reclaim.
    unsafe { libc_exit(0) }
}

extern "C" {
    #[link_name = "_exit"]
    fn libc_exit(status: std::ffi::c_int) -> !;
}

/// Feral Interactive's GameMode, asked for over D-Bus.
///
/// GameMode is a request rather than a wrapper. There is nothing to link and
/// nothing to `LD_PRELOAD`: `gamemoded` owns `com.feralinteractive.GameMode` on
/// the session bus and takes `RegisterGame(i pid)` / `UnregisterGame(i pid)`.
/// While a client is registered it puts the CPU governor in performance, raises
/// the process's I/O and scheduling priority, puts the GPU in its performance
/// profile and inhibits the screensaver. That last one is not a footnote for a
/// game the user plays with a controller and does not touch the keyboard for.
///
/// **Absence is the ordinary case and must not fail a launch.** Most machines
/// do not have gamemoded, and this is an optimisation rather than a dependency
/// — a client that refused to start because a performance daemon was missing
/// would be a far worse bug than the frame it was trying to save. Every failure
/// here is reported in one line and stepped over.
///
/// On by default, which is what Sober does. `CORDIAL_GAMEMODE=0` turns it off,
/// and that is the control: it is the only way to show, in the same session,
/// that a timing difference came from this and not from something else.
mod gamemode {
    use std::sync::OnceLock;

    const SERVICE: &str = "com.feralinteractive.GameMode";
    const OBJECT: &str = "/com/feralinteractive/GameMode";

    /// Held for the life of the process rather than opened per call. Not because
    /// `RegisterGame` needs it — it registers a pid, and gamemoded watches that
    /// pid rather than this connection — but because [`unregister`] runs during
    /// teardown, and opening a bus connection is the wrong thing to be doing at
    /// the point where the engine's own destructors are already known to be
    /// unsafe to run.
    static CONNECTION: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();

    /// Whether [`register`] actually got a yes, so [`unregister`] does not send
    /// an `UnregisterGame` for a registration that never happened.
    static REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn enabled() -> bool {
        !matches!(
            std::env::var("CORDIAL_GAMEMODE").unwrap_or_default().trim(),
            "0" | "off" | "false" | "no"
        )
    }

    fn connection() -> Option<&'static zbus::blocking::Connection> {
        CONNECTION.get_or_init(|| zbus::blocking::Connection::session().ok()).as_ref()
    }

    /// `RegisterGame`/`UnregisterGame` both answer `0` for success and a
    /// negative number for a refusal, so the reply has to be read rather than
    /// just checked for not being a D-Bus error — gamemoded returns `-1` for a
    /// pid it will not accept and `-2` for one already registered, over a
    /// perfectly successful method call.
    fn call(method: &str) -> Result<i32, String> {
        let conn = connection().ok_or_else(|| "no session bus".to_string())?;
        let pid = std::process::id() as i32;
        let reply = conn
            .call_method(Some(SERVICE), OBJECT, Some(SERVICE), method, &(pid,))
            .map_err(|e| e.to_string())?;
        reply.body().deserialize::<i32>().map_err(|e| e.to_string())
    }

    pub fn register() {
        if !enabled() {
            println!("[gamemode] off (CORDIAL_GAMEMODE=0)");
            return;
        }
        match call("RegisterGame") {
            Ok(0) => {
                REGISTERED.store(true, std::sync::atomic::Ordering::Relaxed);
                println!(
                    "[gamemode] registered pid {}: performance governor, raised priority, \
                     GPU performance profile, screensaver inhibited",
                    std::process::id()
                );
            }
            // Said plainly rather than folded into the error path below. A
            // daemon that answered and declined is a different situation from
            // one that is not there, and only the second is the ordinary case.
            Ok(rc) => println!("[gamemode] gamemoded declined to register this process (rc {rc})"),
            Err(e) => println!("[gamemode] not available, continuing without it: {e}"),
        }
    }

    pub fn unregister() {
        if !REGISTERED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        match call("UnregisterGame") {
            Ok(0) => println!("[gamemode] unregistered"),
            Ok(rc) => println!("[gamemode] UnregisterGame returned {rc}"),
            Err(e) => println!("[gamemode] UnregisterGame failed: {e}"),
        }
    }
}

/// The store behind `native/local_storage.cpp`'s `PlatformLocalStorageHandler`
/// -- `ILocalStorageHandlerCore.setPlatformImpl`'s per-user, per-key secure
/// values, which is a different thing from `RbxStorage` (the content cache)
/// and from `LocalStorageManager`'s own `initStorageManagerNativeV3`. See that
/// file's header for the full account of what the interface is and how it was
/// confirmed; this module is the half of it the task that added it could not
/// put in `secrets.rs`.
///
/// **Why this is not a third `secrets::Kind`.** `secrets.rs` is this project's
/// settled answer for where a per-profile secret goes -- the desktop Secret
/// Service first, an announced `0600` file second, never a reason startup
/// fails -- and the right move would have been to add a variant and call it.
/// Two things stopped that. First, the task this module was written under
/// left `secrets.rs` off limits to edit, on the reasoning that a file several
/// agents have been relying on as a fixed reference should not move under
/// them mid-session. Second, and the reason a variant would not have been
/// enough even without that restriction: `Kind::load`/`save` hold exactly one
/// document per profile, and what this interface asks for is an arbitrary
/// number of small values keyed by an account id *and* a name the engine
/// picks — `getSecureValue`, `setSecureValueForUser`, `deleteUserValues`, all
/// of them shaped around a key that is not fixed at compile time the way
/// `"cookies"` and `"identity"` are. So this reuses `secrets::active()` --
/// the same environment variable, the same keyring-vs-file-vs-none decision,
/// decided once and shared with the cookie jar and the identity mirror rather
/// than asked a second time -- and carries its own small read/write/remove
/// against the same `org.freedesktop.secrets` interface under its own schema,
/// because that half of `secrets.rs` is `HashMap<String,String>`-shaped for
/// one document and cannot be reused as-is for many.
///
/// **The same restraint on printing.** Nothing below prints a stored value or
/// a user id at any verbosity, matching `secrets.rs`'s own header and
/// AGENTS.md's rule that this project's stubs answer honestly rather than
/// pretending. Key *names* are printed, the same way `secrets.rs` prints
/// `"cookies"`/`"identity"` -- they identify which field failed, not whose
/// account or what the field held.
///
/// **Why a fresh connection per call rather than `secrets.rs`'s worker
/// thread.** That thread exists because `secrets.rs` is called on a flush
/// cadence and a stuck keyring daemon must not freeze whichever thread asks
/// next. Local storage's calls are a handful of account-scoped values, not a
/// periodic save, so the simpler shape here — connect, ask, time out, drop
/// the connection — is enough: a wedged daemon leaks one thread for the one
/// call that hit it rather than jamming every later call behind a single
/// stuck worker the way a shared thread would.
mod local_storage_secrets {
    use std::collections::HashMap;
    use std::ffi::CStr;
    use std::io::Write as _;
    use std::os::raw::{c_char, c_int, c_longlong};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::PathBuf;
    use std::time::Duration;

    use cordial_runtime::secrets::{self, Store};
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const SERVICE: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const IFACE_SERVICE: &str = "org.freedesktop.Secret.Service";
    const IFACE_ITEM: &str = "org.freedesktop.Secret.Item";
    /// A schema of its own, distinct from `secrets.rs`'s `org.cordial.Session`
    /// -- so `secret-tool`/Seahorse show the two families separately, and so a
    /// search for one can never turn up an item that belongs to the other.
    const SCHEMA: &str = "org.cordial.LocalStorageSecureValue";
    const CONTENT_TYPE: &str = "text/plain; charset=utf8";
    const CALL_TIMEOUT: Duration = Duration::from_secs(3);
    const FILE_NAME: &str = "local-storage-secrets.json";

    fn profile_dir() -> PathBuf {
        cordial_runtime::profile::active()
    }

    /// Keyed by profile path (never by name — see `secrets.rs`'s own
    /// `attributes()` for why two profiles both called `default` must not
    /// share an item) plus the account id and, for a single value, the name
    /// the engine gave it. Omitting `key` widens a search to every value held
    /// for that account, which `delete_user` below relies on.
    fn attrs(user_id: i64, key: Option<&str>) -> HashMap<String, String> {
        let mut m = HashMap::from([
            ("xdg:schema".to_string(), SCHEMA.to_string()),
            ("application".to_string(), "cordial".to_string()),
            ("profile".to_string(), profile_dir().display().to_string()),
            ("user".to_string(), user_id.to_string()),
        ]);
        if let Some(k) = key {
            m.insert("key".to_string(), k.to_string());
        }
        m
    }

    fn with_timeout<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<T, String>>(1);
        if std::thread::Builder::new()
            .name("cordial-ls-secret".to_string())
            .spawn(move || {
                let _ = tx.send(f());
            })
            .is_err()
        {
            return Err("could not start a worker thread".to_string());
        }
        rx.recv_timeout(CALL_TIMEOUT).unwrap_or_else(|_| {
            Err(format!(
                "the secret service did not answer within {} seconds",
                CALL_TIMEOUT.as_secs()
            ))
        })
    }

    fn session() -> Result<(Connection, Proxy<'static>, OwnedObjectPath, OwnedObjectPath), String> {
        let conn = Connection::session().map_err(|_| "there is no session bus".to_string())?;
        let service = Proxy::new_owned(conn.clone(), SERVICE, SERVICE_PATH, IFACE_SERVICE)
            .map_err(|e| format!("the secret service could not be addressed ({e})"))?;
        let (_output, open_session): (OwnedValue, OwnedObjectPath) = service
            .call("OpenSession", &("plain", Value::from("")))
            .map_err(|_| "there is no secret service on the session bus".to_string())?;
        let collection: OwnedObjectPath = service
            .call("ReadAlias", &("default",))
            .map_err(|e| format!("the secret service has no default collection ({e})"))?;
        if collection.as_str() == "/" {
            return Err("the secret service has no default collection".to_string());
        }
        Ok((conn, service, open_session, collection))
    }

    fn proxy(conn: &Connection, path: &OwnedObjectPath, iface: &'static str) -> Result<Proxy<'static>, String> {
        Proxy::new_owned(conn.clone(), SERVICE, path.clone().into_inner(), iface)
            .map_err(|e| format!("{path} could not be addressed ({e})"))
    }

    fn keyring_read(request_attrs: HashMap<String, String>) -> Result<Option<String>, String> {
        with_timeout(move || {
            let (conn, service, item_session, _collection) = session()?;
            let (unlocked, _locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service
                .call("SearchItems", &(request_attrs,))
                .map_err(|e| format!("the keyring could not be searched ({e})"))?;
            let Some(item) = unlocked.into_iter().next() else {
                return Ok(None);
            };
            let (_session, _parameters, value, _content): (
                OwnedObjectPath,
                Vec<u8>,
                Vec<u8>,
                String,
            ) = proxy(&conn, &item, IFACE_ITEM)?
                .call("GetSecret", &(&item_session,))
                .map_err(|e| format!("the stored value could not be read ({e})"))?;
            String::from_utf8(value)
                .map(Some)
                .map_err(|_| "the stored value is not text".to_string())
        })
    }

    fn keyring_write(
        request_attrs: HashMap<String, String>,
        label: String,
        body: String,
    ) -> Result<(), String> {
        with_timeout(move || {
            let (conn, _service, item_session, collection) = session()?;
            let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
            properties.insert("org.freedesktop.Secret.Item.Label", Value::from(label.as_str()));
            properties.insert(
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(request_attrs),
            );
            let secret = (item_session, Vec::<u8>::new(), body.into_bytes(), CONTENT_TYPE);
            let (_item, prompt): (OwnedObjectPath, OwnedObjectPath) =
                proxy(&conn, &collection, "org.freedesktop.Secret.Collection")?
                    .call("CreateItem", &(properties, secret, true))
                    .map_err(|e| format!("the value could not be stored ({e})"))?;
            if prompt.as_str() != "/" {
                return Err("storing the value would have needed a prompt".to_string());
            }
            Ok(())
        })
    }

    fn keyring_remove(request_attrs: HashMap<String, String>) -> Result<(), String> {
        with_timeout(move || {
            let (conn, service, _item_session, _collection) = session()?;
            let (unlocked, _locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service
                .call("SearchItems", &(request_attrs,))
                .map_err(|e| format!("the keyring could not be searched ({e})"))?;
            for item in unlocked {
                let _prompt: OwnedObjectPath = proxy(&conn, &item, IFACE_ITEM)?
                    .call("Delete", &())
                    .map_err(|e| format!("a stored value could not be removed ({e})"))?;
            }
            Ok(())
        })
    }

    // -----------------------------------------------------------------
    // The file backend: one JSON document per profile rather than one file
    // per value, for the same reason `secrets.rs`'s file store is one
    // document rather than one file per cookie — a directory full of
    // ad-hoc-named files in a profile is a worse audit surface than one
    // named store, and `write_file` below is the same temp-then-rename
    // shape `secrets.rs`'s `write_private` uses, for the same reason: a
    // reader must see the old body or the new one, never half of either.
    // -----------------------------------------------------------------

    type FileMap = HashMap<String, HashMap<String, String>>;

    fn file_path() -> PathBuf {
        profile_dir().join(FILE_NAME)
    }

    fn file_load() -> FileMap {
        std::fs::read_to_string(file_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn file_save(map: &FileMap) -> std::io::Result<()> {
        let final_path = file_path();
        let tmp = profile_dir().join(format!("{FILE_NAME}.new"));
        let body = serde_json::to_string(map).map_err(std::io::Error::other)?;

        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &final_path)
    }

    // -----------------------------------------------------------------
    // The three operations, dispatched on the same `Store` cookies and
    // identity already settled on this launch.
    // -----------------------------------------------------------------

    fn get(user_id: i64, key: &str) -> Option<String> {
        match secrets::active() {
            Store::None => None,
            Store::File => file_load().get(&user_id.to_string()).and_then(|m| m.get(key)).cloned(),
            Store::Keyring => match keyring_read(attrs(user_id, Some(key))) {
                Ok(v) => v,
                Err(why) => {
                    println!("  [local-storage] {key}: not read back ({why})");
                    None
                }
            },
        }
    }

    fn set(user_id: i64, key: &str, value: &str) -> bool {
        match secrets::active() {
            // Matches secrets.rs's own `Store::None` save: accepted and
            // discarded rather than refused, so a user who opted out of
            // storage entirely is not additionally punished with a JNI
            // `false` the engine has no way to explain to anyone.
            Store::None => true,
            Store::File => {
                let mut map = file_load();
                map.entry(user_id.to_string())
                    .or_default()
                    .insert(key.to_string(), value.to_string());
                match file_save(&map) {
                    Ok(()) => true,
                    Err(e) => {
                        println!("  [local-storage] {key}: not saved ({e})");
                        false
                    }
                }
            }
            Store::Keyring => {
                let label = format!(
                    "Cordial: Roblox local storage ({key}) for profile {:?}",
                    profile_dir().file_name().map(|n| n.to_string_lossy().into_owned())
                );
                match keyring_write(attrs(user_id, Some(key)), label, value.to_string()) {
                    Ok(()) => true,
                    Err(why) => {
                        println!("  [local-storage] {key}: not saved ({why})");
                        false
                    }
                }
            }
        }
    }

    fn delete(user_id: i64, key: &str) -> bool {
        match secrets::active() {
            Store::None => true,
            Store::File => {
                let mut map = file_load();
                if let Some(m) = map.get_mut(&user_id.to_string()) {
                    m.remove(key);
                }
                match file_save(&map) {
                    Ok(()) => true,
                    Err(e) => {
                        println!("  [local-storage] {key}: not removed ({e})");
                        false
                    }
                }
            }
            Store::Keyring => match keyring_remove(attrs(user_id, Some(key))) {
                Ok(()) => true,
                Err(why) => {
                    println!("  [local-storage] {key}: not removed ({why})");
                    false
                }
            },
        }
    }

    fn delete_user(user_id: i64) -> bool {
        match secrets::active() {
            Store::None => true,
            Store::File => {
                let mut map = file_load();
                map.remove(&user_id.to_string());
                match file_save(&map) {
                    Ok(()) => true,
                    Err(e) => {
                        println!("  [local-storage] account values: not removed ({e})");
                        false
                    }
                }
            }
            // No "key" attribute: every item this profile holds for the
            // account, not one value of it.
            Store::Keyring => match keyring_remove(attrs(user_id, None)) {
                Ok(()) => true,
                Err(why) => {
                    println!("  [local-storage] account values: not removed ({why})");
                    false
                }
            },
        }
    }

    // -----------------------------------------------------------------
    // The C boundary. `native/local_storage.cpp` declares these four
    // directly against these symbol names — see that file's header for why
    // there is no generated binding for them.
    // -----------------------------------------------------------------

    unsafe fn borrow_str<'a>(p: *const c_char) -> Option<&'a str> {
        if p.is_null() {
            return None;
        }
        // SAFETY: the caller (native/local_storage.cpp) passes a
        // NUL-terminated buffer it owns for the duration of this call.
        unsafe { CStr::from_ptr(p) }.to_str().ok()
    }

    /// Returns `0` on an ordinary call, whether or not anything was found;
    /// `*found` and `*out_len` carry the actual answer. `-1` means the call
    /// itself could not be made (a bad key, a null buffer) rather than
    /// anything about whether a value exists.
    #[no_mangle]
    pub extern "C" fn cordial_local_storage_get(
        user_id: c_longlong,
        key: *const c_char,
        out: *mut c_char,
        out_cap: usize,
        found: *mut c_int,
        out_len: *mut usize,
    ) -> c_int {
        // SAFETY: `key` is a NUL-terminated C string owned by the caller for
        // the duration of this call; `out`/`found`/`out_len` are live
        // buffers the caller sized and will read back afterwards.
        let Some(key) = (unsafe { borrow_str(key) }) else {
            return -1;
        };
        if out.is_null() || found.is_null() || out_len.is_null() {
            return -1;
        }
        let value = get(user_id as i64, key);
        // SAFETY: pointers were just checked non-null; `out` has `out_cap`
        // bytes per the caller's own contract in local_storage.cpp.
        unsafe {
            match value {
                None => {
                    *found = 0;
                    *out_len = 0;
                }
                Some(v) => {
                    let bytes = v.as_bytes();
                    // `>=` rather than `>`: a byte of the cap is reserved for
                    // the NUL the C++ side reads the string through.
                    if bytes.len() >= out_cap {
                        println!(
                            "  [local-storage] {key}: {} bytes does not fit the platform \
                             buffer; treated as absent rather than truncated",
                            bytes.len()
                        );
                        *found = 0;
                        *out_len = bytes.len();
                    } else {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, bytes.len());
                        *out.add(bytes.len()) = 0;
                        *found = 1;
                        *out_len = bytes.len();
                    }
                }
            }
        }
        0
    }

    #[no_mangle]
    pub extern "C" fn cordial_local_storage_set(
        user_id: c_longlong,
        key: *const c_char,
        value: *const c_char,
        value_len: usize,
    ) -> c_int {
        // SAFETY: as above; `value` points to `value_len` bytes the caller
        // owns for the duration of this call.
        let Some(key) = (unsafe { borrow_str(key) }) else {
            return -1;
        };
        if value.is_null() {
            return -1;
        }
        let bytes = unsafe { std::slice::from_raw_parts(value as *const u8, value_len) };
        let Ok(value) = std::str::from_utf8(bytes) else {
            println!("  [local-storage] {key}: value is not UTF-8; refused rather than stored");
            return -1;
        };
        if set(user_id as i64, key, value) { 0 } else { -1 }
    }

    #[no_mangle]
    pub extern "C" fn cordial_local_storage_delete(user_id: c_longlong, key: *const c_char) -> c_int {
        let Some(key) = (unsafe { borrow_str(key) }) else {
            return -1;
        };
        if delete(user_id as i64, key) { 0 } else { -1 }
    }

    #[no_mangle]
    pub extern "C" fn cordial_local_storage_delete_user(user_id: c_longlong) -> c_int {
        if delete_user(user_id as i64) { 0 } else { -1 }
    }
}
