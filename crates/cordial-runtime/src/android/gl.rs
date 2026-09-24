//! Proves the graphics path Roblox will use.
//!
//! Every `egl*` and `gl*` symbol Roblox imports resolves to the host's Mesa
//! ([`crate::symtab`]), but "the symbol resolved" and "the graphics stack works"
//! are different claims, and only the second matters. This brings up a real
//! GLES2 context through *the same function pointers Roblox will be given*,
//! draws, and reads the result back.
//!
//! Going through the symbol table rather than calling `libEGL` directly is the
//! point. It exercises the plumbing — classification, host lookup, the
//! descriptor the linker will hand over — not just Mesa.
//!
//! Roblox imports `eglCreatePbufferSurface` alongside `eglCreateWindowSurface`,
//! so an offscreen surface is a path the engine itself uses, not a shortcut
//! invented here.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};

use crate::symtab;

type Display = *mut c_void;
type Config = *mut c_void;
type Surface = *mut c_void;
type Context = *mut c_void;

// EGL constants, from eglplatform.h / egl.h.
const EGL_DEFAULT_DISPLAY: *mut c_void = std::ptr::null_mut();
const EGL_NO_CONTEXT: Context = std::ptr::null_mut();
const EGL_NONE: i32 = 0x3038;
const EGL_WIDTH: i32 = 0x3057;
const EGL_HEIGHT: i32 = 0x3056;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_PBUFFER_BIT: i32 = 0x0001;
const EGL_WINDOW_BIT: i32 = 0x0004;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES2_BIT: i32 = 0x0004;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_ALPHA_SIZE: i32 = 0x3021;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;

// GLES2 constants.
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_VERSION: u32 = 0x1F02;
const GL_RENDERER: u32 = 0x1F01;
const GL_VENDOR: u32 = 0x1F00;

/// The subset of the graphics API this probe needs, resolved from the symbol
/// table Roblox will be handed.
struct Api {
    get_display: extern "C" fn(*mut c_void) -> Display,
    initialize: extern "C" fn(Display, *mut i32, *mut i32) -> u32,
    choose_config: extern "C" fn(Display, *const i32, *mut Config, i32, *mut i32) -> u32,
    create_pbuffer: extern "C" fn(Display, Config, *const i32) -> Surface,
    create_window_surface: extern "C" fn(Display, Config, u64, *const i32) -> Surface,
    swap_buffers: extern "C" fn(Display, Surface) -> u32,
    create_context: extern "C" fn(Display, Config, Context, *const i32) -> Context,
    make_current: extern "C" fn(Display, Surface, Surface, Context) -> u32,
    get_error: extern "C" fn() -> i32,
    terminate: extern "C" fn(Display) -> u32,

    clear_color: extern "C" fn(f32, f32, f32, f32),
    clear: extern "C" fn(u32),
    read_pixels: extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void),
    viewport: extern "C" fn(i32, i32, i32, i32),
    // `c_char`, not a hardcoded `i8`: `glGetString` returns `const GLubyte*`,
    // and `CStr::from_ptr` wants whatever this platform's own `char` is --
    // signed on x86_64 (System V ABI), unsigned on aarch64 (AAPCS64). A bare
    // `i8` here compiled and ran on x86_64 by coincidence and failed to
    // *compile at all* for aarch64 with `CStr::from_ptr` below expecting
    // `*const u8`, which is the honest outcome: a real ABI difference this
    // signature was not tracking, not a bug that could stay silent.
    gl_get_string: extern "C" fn(u32) -> *const c_char,
}

fn address(table: &HashMap<&str, *mut c_void>, name: &str) -> Result<*mut c_void, String> {
    match table.get(name) {
        Some(&addr) if !addr.is_null() => Ok(addr),
        _ => Err(format!("{name} is not in the symbol table")),
    }
}

impl Api {
    /// Resolve from a built symbol table, refusing anything that only resolved to
    /// a stub — a stub would "succeed" at everything and prove nothing.
    fn from_table(table: &symtab::SymbolTable) -> Result<Self, String> {
        let mut resolved: HashMap<&str, *mut c_void> = HashMap::new();
        for lib in ["libEGL.so", "libGLESv2.so"] {
            let Some(entries) = table.libraries.get(lib) else {
                return Err(format!("{lib} is not in the symbol table"));
            };
            for e in entries {
                if e.source == symtab::Source::Stub {
                    continue;
                }
                resolved.insert(e.symbol, e.address);
            }
        }

        macro_rules! get {
            ($name:literal) => {
                // SAFETY: the address came from dlsym on the host's libEGL or
                // libGLESv2 for exactly this name, so the signature is the one
                // Khronos specifies.
                unsafe { std::mem::transmute(address(&resolved, $name)?) }
            };
        }

        Ok(Api {
            get_display: get!("eglGetDisplay"),
            initialize: get!("eglInitialize"),
            choose_config: get!("eglChooseConfig"),
            create_pbuffer: get!("eglCreatePbufferSurface"),
            create_window_surface: get!("eglCreateWindowSurface"),
            swap_buffers: get!("eglSwapBuffers"),
            create_context: get!("eglCreateContext"),
            make_current: get!("eglMakeCurrent"),
            get_error: get!("eglGetError"),
            terminate: get!("eglTerminate"),
            clear_color: get!("glClearColor"),
            clear: get!("glClear"),
            read_pixels: get!("glReadPixels"),
            viewport: get!("glViewport"),
            gl_get_string: get!("glGetString"),
        })
    }

    fn string(&self, name: u32) -> String {
        let p = (self.gl_get_string)(name);
        if p.is_null() {
            return "(null)".into();
        }
        // SAFETY: glGetString returns a NUL-terminated string owned by the driver.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

pub struct Report {
    pub vendor: String,
    pub renderer: String,
    pub version: String,
    /// The pixel read back after clearing, as RGBA.
    pub pixel: [u8; 4],
}

/// Bring up GLES2, clear to a known colour, and read a pixel back.
///
/// The readback is the whole point. Every call up to `glClear` can succeed
/// against a driver that never rasterises anything; only reading the framebuffer
/// proves something was actually drawn.
pub fn probe(table: &symtab::SymbolTable) -> Result<Report, String> {
    const W: i32 = 64;
    const H: i32 = 64;
    // A colour with four distinct channels, so a wrong component order shows up
    // as an obviously wrong answer rather than a plausible one.
    const WANT: [u8; 4] = [0x33, 0x66, 0x99, 0xFF];

    let api = Api::from_table(table)?;

    let display = (api.get_display)(EGL_DEFAULT_DISPLAY);
    if display.is_null() {
        return Err("eglGetDisplay returned no display".into());
    }

    let (mut major, mut minor) = (0, 0);
    if (api.initialize)(display, &mut major, &mut minor) == 0 {
        return Err(format!("eglInitialize failed ({:#x})", (api.get_error)()));
    }

    let config_attribs = [
        EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_NONE,
    ];
    let mut config: Config = std::ptr::null_mut();
    let mut count = 0;
    if (api.choose_config)(display, config_attribs.as_ptr(), &mut config, 1, &mut count) == 0
        || count == 0
    {
        (api.terminate)(display);
        return Err("no EGL config with a GLES2-capable pbuffer".into());
    }

    let surface_attribs = [EGL_WIDTH, W, EGL_HEIGHT, H, EGL_NONE];
    let surface = (api.create_pbuffer)(display, config, surface_attribs.as_ptr());
    if surface.is_null() {
        (api.terminate)(display);
        return Err(format!("eglCreatePbufferSurface failed ({:#x})", (api.get_error)()));
    }

    let context_attribs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
    let context = (api.create_context)(display, config, EGL_NO_CONTEXT, context_attribs.as_ptr());
    if context.is_null() {
        (api.terminate)(display);
        return Err(format!("eglCreateContext failed ({:#x})", (api.get_error)()));
    }

    if (api.make_current)(display, surface, surface, context) == 0 {
        (api.terminate)(display);
        return Err(format!("eglMakeCurrent failed ({:#x})", (api.get_error)()));
    }

    let report_strings = (
        api.string(GL_VENDOR),
        api.string(GL_RENDERER),
        api.string(GL_VERSION),
    );

    (api.viewport)(0, 0, W, H);
    (api.clear_color)(
        WANT[0] as f32 / 255.0,
        WANT[1] as f32 / 255.0,
        WANT[2] as f32 / 255.0,
        WANT[3] as f32 / 255.0,
    );
    (api.clear)(GL_COLOR_BUFFER_BIT);

    let mut pixel = [0u8; 4];
    (api.read_pixels)(
        W / 2,
        H / 2,
        1,
        1,
        GL_RGBA,
        GL_UNSIGNED_BYTE,
        pixel.as_mut_ptr() as *mut c_void,
    );

    (api.terminate)(display);

    // Allow one LSB of slack per channel: the config may not be exactly 8 bits
    // per component, and an off-by-one from quantisation is not a failure.
    let matches = pixel
        .iter()
        .zip(WANT.iter())
        .all(|(got, want)| got.abs_diff(*want) <= 1);
    if !matches {
        return Err(format!(
            "cleared to {WANT:02x?} but read back {pixel:02x?} — the context is live but not rasterising as expected"
        ));
    }

    Ok(Report {
        vendor: report_strings.0,
        renderer: report_strings.1,
        version: report_strings.2,
        pixel,
    })
}

/// Open a host window and render into it through `eglCreateWindowSurface`.
///
/// This is the same path Roblox takes — `ANativeWindow_fromSurface` returns the
/// window created here, and the engine's EGL surface is built on it. The pbuffer
/// probe proves the GL stack; this proves the *window system* half, which is the
/// part that cannot be tested headless.
pub fn probe_window(
    table: &symtab::SymbolTable,
    seconds: u64,
) -> Result<Report, String> {
    use crate::android::window;

    let host = window::open(1280, 720, &crate::window_title("GL probe"))?;
    let (w, h, _) = host.geometry();
    let api = Api::from_table(table)?;

    // The X display, not EGL_DEFAULT_DISPLAY: the surface has to belong to the
    // same connection as the window it is created on.
    let display = (api.get_display)(host.egl_native_display());
    if display.is_null() {
        return Err("eglGetDisplay rejected the X display".into());
    }

    let (mut major, mut minor) = (0, 0);
    if (api.initialize)(display, &mut major, &mut minor) == 0 {
        return Err(format!("eglInitialize failed ({:#x})", (api.get_error)()));
    }

    let config_attribs = [
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_NONE,
    ];
    let mut config: Config = std::ptr::null_mut();
    let mut count = 0;
    if (api.choose_config)(display, config_attribs.as_ptr(), &mut config, 1, &mut count) == 0
        || count == 0
    {
        return Err("no EGL config with a GLES2-capable window surface".into());
    }

    let surface = (api.create_window_surface)(
        display,
        config,
        host.egl_native_window(),
        std::ptr::null(),
    );
    if surface.is_null() {
        return Err(format!("eglCreateWindowSurface failed ({:#x})", (api.get_error)()));
    }

    let context_attribs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
    let context = (api.create_context)(display, config, EGL_NO_CONTEXT, context_attribs.as_ptr());
    if context.is_null() {
        return Err(format!("eglCreateContext failed ({:#x})", (api.get_error)()));
    }
    if (api.make_current)(display, surface, surface, context) == 0 {
        return Err(format!("eglMakeCurrent failed ({:#x})", (api.get_error)()));
    }

    let strings = (
        api.string(GL_VENDOR),
        api.string(GL_RENDERER),
        api.string(GL_VERSION),
    );

    // Animate rather than clearing once. A static colour cannot be told apart
    // from a window the compositor painted itself; a changing one can.
    const FRAMES_PER_SECOND: u64 = 60;
    let total = seconds * FRAMES_PER_SECOND;
    let mut pixel = [0u8; 4];
    (api.viewport)(0, 0, w, h);

    for frame in 0..total.max(1) {
        let t = frame as f32 / FRAMES_PER_SECOND as f32;
        (api.clear_color)(0.2 + 0.2 * t.sin(), 0.4, 0.6 + 0.2 * t.cos(), 1.0);
        (api.clear)(GL_COLOR_BUFFER_BIT);

        if frame == 0 {
            (api.read_pixels)(
                w / 2, h / 2, 1, 1,
                GL_RGBA, GL_UNSIGNED_BYTE,
                pixel.as_mut_ptr() as *mut c_void,
            );
        }
        if (api.swap_buffers)(display, surface) == 0 {
            return Err(format!("eglSwapBuffers failed ({:#x})", (api.get_error)()));
        }
        std::thread::sleep(std::time::Duration::from_millis(1000 / FRAMES_PER_SECOND));
    }

    (api.terminate)(display);
    host.close();

    Ok(Report {
        vendor: strings.0,
        renderer: strings.1,
        version: strings.2,
        pixel,
    })
}
