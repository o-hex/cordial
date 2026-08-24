//! The one window Cordial has: an `AdwWindow` carrying an `AdwToolbarView`,
//! a header bar, and a content slot.
//!
//! [ADR-002](../../../docs/adr/ADR-002-core-shell-and-ui-handoff.md) gives core
//! a shell — window, chooser, an escape hatch — and
//! [ADR-011](../../../docs/adr/ADR-011-wayland-and-libadwaita.md) says of that
//! shell and the engine's host window that they "are the same window", because
//! "building the engine's host window as a bare Wayland surface would mean
//! building the shell twice, and the second one would have to inherit the theme
//! anyway". This module is where that sentence stops being an intention: the
//! shell binary fills the content slot with the chooser, and `cordial-runtime`
//! fills it with the engine's Wayland subsurface. One window definition, two
//! callers.
//!
//! It stays deliberately thin. Anything that grows — settings, themes,
//! plugin-contributed views — belongs to the UI plugin, not here.

use gdk4_wayland::prelude::*;
use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;
use std::ffi::c_void;
use std::time::{Duration, Instant};

/// The `xdg_toplevel` app_id this window carries, and X11's `WM_CLASS` before
/// it. Must keep matching `StartupWMClass` in
/// `packaging/io.github.luohoa97.Cordial.desktop` for the reasons in ADR-009 — GNOME
/// Shell, the screen-cast portal's window picker and every capture tool match a
/// window to its desktop entry through this string, and a drift shows up as a
/// missing icon rather than as an error. Pinned by
/// `tests::app_id_matches_the_desktop_entry`.
pub const APP_ID: &str = "Cordial";

/// What the header bar says.
///
/// It used to name the graphics backend and the name it gave was "OpenGL ES",
/// which is false: the engine reaches its landing page through Vulkan on both
/// backends — 547, 548 and 550 `vkQueuePresentKHR` calls over three
/// consecutive 25-second runs, with every GLES counter at zero in the same
/// runs. A title bar is a poor place to report something that can change at
/// run time and an even poorer place to report it wrongly, so it now reports
/// nothing about graphics at all; `CORDIAL_COUNT_GL=1` answers the question
/// the backend suffix was trying to answer, and answers it with counts.
/// The version is what `git describe` says, not what `Cargo.toml` says — see
/// `build.rs`. On a tag that is the tag alone; off one it carries the distance
/// and the hash so a bug report identifies a commit rather than a range of
/// dozens; and with uncommitted changes it ends in `-dirty`, which is the case
/// that matters most here. A binary built from a working tree several agents
/// were editing looked exactly like a committed one, and an afternoon went into
/// a regression nobody could attribute to a tree.
pub fn title() -> String {
    // The name rather than a literal, so the twice-a-year joke reaches the one
    // place a user actually reads it. See `branding`: decided once, never
    // polled, and never applied to anything in the repository.
    format!("{} {}", crate::branding::current().name(), env!("CORDIAL_BUILD_VERSION"))
}

/// How much of a monitor to leave for whatever else is on it.
///
/// Wayland has no way to ask for a work area — a panel is just another client,
/// so `gdk_monitor_get_geometry` is the whole of what GDK can tell anyone and
/// there is no `get_workarea` to pair with it. The space a desktop shell
/// reserves therefore has to be allowed for rather than read. GNOME's top bar
/// is 37 logical pixels at scale 1; this is roughly twice that, so a window
/// clamped by [`fit_within`] still has somewhere to sit rather than being
/// placed with its bottom edge past the end of the screen.
const MONITOR_ALLOWANCE: i32 = 96;

/// Clamp a requested window size to something that fits on one monitor.
///
/// Kept pure and separate from the GDK lookups so the arithmetic is testable
/// without a display. The bug it exists for: on a dual-head desktop measuring
/// 5360x1440 in total — a 3440x1440 monitor with a 1920x1200 one beside it —
/// the window ran off the edge of the first screen, because nothing in this
/// file had ever asked how big a screen was. Sizes here are logical pixels,
/// which is what both `gdk_monitor_get_geometry` and
/// `gtk_window_set_default_size` speak, so no scale factor enters into it.
fn fit_within(requested: (i32, i32), monitor: (i32, i32)) -> (i32, i32) {
    // The floors matter more than they look: a monitor smaller than the
    // allowance would otherwise produce a zero or negative size, and GTK
    // treats that as "no default size at all" rather than as an error.
    let max_w = (monitor.0 - MONITOR_ALLOWANCE).max(320);
    let max_h = (monitor.1 - MONITOR_ALLOWANCE).max(240);
    (requested.0.min(max_w), requested.1.min(max_h))
}

/// The geometry of every monitor GDK currently lists.
fn monitor_geometries() -> Vec<(i32, i32)> {
    let Some(display) = gtk::gdk::Display::default() else {
        return Vec::new();
    };
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i))
        .filter_map(|m| m.downcast::<gtk::gdk::Monitor>().ok())
        .map(|m| {
            let g = m.geometry();
            (g.width(), g.height())
        })
        .collect()
}

/// The smallest monitor attached, which is the only safe guess before the
/// window exists.
///
/// Wayland does not let a client choose, or even learn in advance, which
/// output its toplevel will be mapped on — that is the compositor's decision
/// and it is communicated after the fact, through `wl_surface.enter`. So the
/// size passed to `gtk_window_set_default_size` has to fit *whichever* monitor
/// the window lands on, and the smallest one is the only bound that does.
///
/// A second pass was tried and dropped: once the window is mapped,
/// `gdk_display_get_monitor_at_surface` says which output it really landed on,
/// which would allow a tighter clamp. It buys nothing — the build-time bound
/// already fits every monitor — and it costs a second `set_default_size` on a
/// window the engine is about to take its geometry from. `content_rect` is
/// where the subsurface's position, every pointer coordinate and the IME's
/// cursor rectangle come from, so it is not somewhere to perturb for a bound
/// that is already sufficient.
///
/// **Measured, on a 3440x1440 monitor beside a 1920x1200 one.** Asking for
/// 5000x1300: without this clamp the same tree yields a 3440x1301 window —
/// the whole width of the first screen — and the binary built from the
/// commit before it yields 5000x1300, which is the reported bleed. With it,
/// 1824x1058, twice each. The default 1280x720 is unaffected either way.
fn smallest_monitor() -> Option<(i32, i32)> {
    monitor_geometries().into_iter().min_by_key(|(w, h)| (*w as i64) * (*h as i64))
}

/// Bring GTK up for a process that is hosting the engine's Wayland surface.
///
/// Two things here are not the defaults and both were paid for.
///
/// The backend is forced to Wayland because the engine's surface has to be a
/// subsurface of a *Wayland* surface or it cannot be a subsurface at all. That
/// is a requirement of the caller, not a preference.
///
/// It takes both a call and an environment variable, which is worth spelling
/// out because the obvious half does not work. This developer's session — an
/// ordinary GNOME Wayland one — exports `GDK_BACKEND=x11`. With
/// `gdk_set_allowed_backends("wayland")` alone and that variable set, GTK 4.22
/// opens *no display at all* and `gtk_init_check` returns false with nothing
/// printed: under `GDK_DEBUG=misc` the trace reads `Skipping x11 backend` — so
/// the allowed-backends call was honoured — and then never says a word about
/// wayland, which the environment variable had already excluded. Two filters,
/// and their intersection was empty. The symptom is `Failed to initialize GTK`
/// and no window, so anyone who hits it will not guess.
///
/// The variable is only overwritten when it is set to something else, so on a
/// session that does not export it nothing here touches the environment. It is
/// still a process-global write with the engine's threads already running,
/// which is why it is conditional rather than unconditional.
///
/// `glib::set_prgname` because a window with no `GApplication` takes its
/// `xdg_toplevel.app_id` from the program name, which would otherwise be
/// `cordial-run`. See [`APP_ID`].
pub fn init_wayland() -> Result<(), String> {
    if std::env::var("GDK_BACKEND").is_ok_and(|v| v != "wayland") {
        // SAFETY: `g_setenv` is not thread-safe against a concurrent
        // `getenv`, which is why the standard library marks its equivalent
        // unsafe. This runs on the thread that is about to initialise GTK,
        // before any GTK or GDK call, and the engine's own threads do not read
        // this variable — nothing in `libroblox.so` has heard of GDK.
        unsafe { glib::setenv("GDK_BACKEND", "wayland", true) }
            .map_err(|e| format!("could not force GDK_BACKEND=wayland: {e}"))?;
    }
    gtk::gdk::set_allowed_backends("wayland");
    glib::set_prgname(Some(APP_ID));
    adw::init().map_err(|e| format!("libadwaita would not initialise: {e}"))?;
    unmute_waylands_own_errors();
    Ok(())
}

/// Let libwayland's fatal messages reach the terminal again.
///
/// A session was lost with this as its entire epitaph:
///
/// ```text
/// Gdk-Message: 14:10:43.968: Error 71 (Protocol error) dispatching to Wayland display.
/// ```
///
/// GDK prints that and calls `_exit(1)`. It names an errno and nothing else,
/// and 71 is `EPROTO` — the compositor rejected something the client sent.
/// libwayland *does* say which object and why, but GTK4 calls
/// `wl_log_set_handler_client` with a handler that logs at
/// `G_LOG_LEVEL_DEBUG`, and debug is dropped unless `G_MESSAGES_DEBUG` names
/// the domain. So the one line that answers the question is discarded by
/// default, roughly 50ms before the process dies.
///
/// Measured, by binding a global name mutter never advertised. Without this,
/// the whole of the output is the `Gdk-Message` above. With
/// `G_MESSAGES_DEBUG=all`, and now with this:
///
/// ```text
/// wl_registry#107: error 0: global wl_compositor (999999) is unavailable
/// ```
///
/// Installing a `Gdk`-domain handler rather than setting `G_MESSAGES_DEBUG`
/// keeps the other ~122 debug lines GDK emits per launch (portal settings,
/// mostly) out of the way; a handler registered here is called whatever
/// `G_MESSAGES_DEBUG` says, because that filter lives in GLib's *default*
/// handler and this replaces it for one domain.
///
/// The substring test is the weak part and is deliberately small. These are
/// the shapes libwayland uses when a connection is finished: `<interface>#<id>:
/// error <code>: <reason>` for a compositor-sent `wl_display.error`, and
/// `interface '<name>' has no event <n>` for an opcode past the end of one of
/// the hand-written tables in `cordial_runtime::android::wayland`. Missing a
/// third shape costs a diagnostic, not correctness — everything still goes to
/// GDK's own handler as well.
fn unmute_waylands_own_errors() {
    glib::log_set_handler(
        Some("Gdk"),
        glib::LogLevels::LEVEL_DEBUG,
        false,
        false,
        |_domain, _level, message| {
            if message.contains(": error ") || message.contains("has no event") {
                eprintln!("[wayland] {message}");
            }
        },
    );
}

/// A built, not-yet-presented shell window.
///
/// Holds GTK objects, which are `Rc`-refcounted and must only ever be touched
/// from the thread that ran [`init_wayland`]. Nothing in this type is
/// `Send`/`Sync` and it must not be made so; the runtime keeps its copy behind
/// a wrapper whose own comment names the same rule.
pub struct HostWindow {
    window: adw::Window,
    header: adw::HeaderBar,
    toolbar: adw::ToolbarView,
    /// The widget the content occupies. Its allocation — not the window's — is
    /// what the engine's subsurface is sized and positioned from, so that the
    /// header bar's height never has to be assumed anywhere.
    content: gtk::Widget,
    text_layer: gtk::Fixed,
    text_label: gtk::Label,
}

/// The Android editor rectangle Roblox asks the platform to paint over its
/// surface while a TextBox has focus.
pub struct TextOverlay<'a> {
    pub text: &'a str,
    pub caret_chars: i32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub text_color: u32,
    pub password: bool,
}

fn displayed_editor_text(text: &str, caret_chars: i32, password: bool) -> String {
    let mut display = if password {
        "•".repeat(text.chars().count())
    } else {
        text.to_owned()
    };
    let caret = caret_chars.max(0) as usize;
    let byte = display
        .char_indices()
        .nth(caret)
        .map(|(at, _)| at)
        .unwrap_or(display.len());
    display.insert(byte, '│');
    display
}

impl HostWindow {
    /// Build the window with an empty canvas in the content slot.
    ///
    /// `width`/`height` are the *content* size the caller wants; the header bar
    /// is added on top of that, so the engine gets the resolution it asked for
    /// rather than that minus a titlebar.
    pub fn with_canvas(title: &str, width: i32, height: i32) -> Self {
        // A `GtkDrawingArea` with no draw function paints nothing at all, so
        // what shows through is the themed window background — which is
        // exactly what ADR-011 asks for behind the canvas ("the desktop's own
        // background colour, following light and dark mode, rather than a
        // flash of white"), with no CSS of Cordial's own involved.
        let canvas = gtk::DrawingArea::new();
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        let host = Self::new(title, width, height, &canvas);
        host.window.add_css_class("cordial-engine-host");
        let css = gtk::CssProvider::new();
        css.load_from_string(
            // **The window itself is not transparent, and must not be.**
            //
            // It was, briefly, and the result was a window nobody could see at
            // all -- not the canvas, not the header bar, not the background.
            // Reported as "its just an invisible window, i cant see it, alt tab
            // its just invisible, but its there".
            //
            // A transparent toplevel is only safe while the engine's subsurface
            // is both painting and stacked above it. Neither holds reliably
            // today: the canvas is lowered whenever a web-view dialog or the
            // text overlay is up, and there is an open bug where the engine
            // presents one frame and then stops. In either state a transparent
            // window shows the desktop behind it and there is nothing left to
            // click, read, or even find.
            //
            // The drawing area alone stays transparent, which is all the engine
            // needs: its subsurface covers that region when it is above, and
            // when it is below the window's own background is what should show
            // -- an occluded game, which is recoverable, rather than an absent
            // window, which is not.
            ".cordial-engine-host drawingarea { \
                 background-color: transparent; \
             } \
             .cordial-engine-host headerbar { \
                 background-color: @headerbar_bg_color; \
                 color: @headerbar_fg_color; \
                 background-image: none; \
                 min-height: 30px; \
                 padding: 0 6px; \
             } \
             .cordial-engine-host headerbar windowcontrols button { \
                 min-width: 24px; \
                 min-height: 24px; \
                 padding: 0; \
                 margin: 2px; \
             }",
        );
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&host.window),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        host
    }

    pub fn new(title: &str, width: i32, height: i32, content: &impl IsA<gtk::Widget>) -> Self {
        let header = adw::HeaderBar::new();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(content));
        let text_layer = gtk::Fixed::new();
        text_layer.set_can_target(false);
        text_layer.set_hexpand(true);
        text_layer.set_vexpand(true);
        let text_label = gtk::Label::new(None);
        text_label.set_can_target(false);
        text_label.set_visible(false);
        text_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text_label.set_overflow(gtk::Overflow::Hidden);
        text_layer.put(&text_label, 0.0, 0.0);
        overlay.add_overlay(&text_layer);
        toolbar.set_content(Some(&overlay));

        // Clamped before the window exists, because a default size larger than
        // the screen is not something the compositor will correct for you: it
        // maps the toplevel at the size asked for and the far edge simply ends
        // up past the end of the monitor. On a dual-head desktop that reads as
        // the window bleeding onto the second screen, which is how this was
        // first reported.
        let (w, h) = match smallest_monitor() {
            Some(monitor) => fit_within((width, height + header_height_hint()), monitor),
            None => (width, height + header_height_hint()),
        };

        let window = adw::Window::builder()
            .title(title)
            .default_width(w)
            .default_height(h)
            .content(&toolbar)
            .build();

        // Hide the header bar while fullscreen, and put it back afterwards.
        //
        // GTK does not do this for you and is right not to: it cannot know
        // whether a given toolbar is chrome the user wants gone or part of the
        // application. For a game it is chrome. Without this, fullscreening
        // leaves the title bar sitting across the top of the picture, which is
        // what "fullscreen still shows the titlebar" means and is not a
        // compositor problem — the window really is fullscreen, the bar is
        // simply still inside it.
        //
        // `set_reveal_top_bars` rather than hiding the header directly, because
        // ToolbarView owns the space it occupies: hiding the child leaves the
        // gap it was sitting in.
        {
            let toolbar_for_fs = toolbar.clone();
            window.connect_fullscreened_notify(move |w| {
                toolbar_for_fs.set_reveal_top_bars(!w.is_fullscreen());
            });
        }

        HostWindow {
            window,
            header,
            toolbar,
            content: content.as_ref().clone(),
            text_layer,
            text_label,
        }
    }

    pub fn window(&self) -> &adw::Window {
        &self.window
    }

    pub fn header(&self) -> &adw::HeaderBar {
        &self.header
    }

    pub fn toolbar(&self) -> &adw::ToolbarView {
        &self.toolbar
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Paint (or hide) the desktop equivalent of Android's transparent
    /// `EditText` over the engine canvas. The caller controls subsurface
    /// stacking; this method only updates GTK's parent surface.
    pub fn set_text_overlay(&self, overlay: Option<TextOverlay<'_>>) {
        let Some(overlay) = overlay else {
            self.text_label.set_text("");
            self.text_label.set_visible(false);
            self.window.queue_draw();
            return;
        };

        let display = displayed_editor_text(overlay.text, overlay.caret_chars, overlay.password);
        self.text_label.set_text(&display);
        self.text_label.set_size_request(
            overlay.width.max(1.0).round() as i32,
            overlay.height.max(1.0).round() as i32,
        );
        self.text_label.set_xalign(0.0);
        self.text_label.set_yalign(0.5);

        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrSize::new(
            (overlay.font_size.max(1.0) * gtk::pango::SCALE as f32).round() as i32,
        ));
        let color = overlay.text_color;
        attrs.insert(gtk::pango::AttrColor::new_foreground(
            (((color >> 16) & 0xff) * 257) as u16,
            (((color >> 8) & 0xff) * 257) as u16,
            ((color & 0xff) * 257) as u16,
        ));
        self.text_label.set_attributes(Some(&attrs));
        self.text_layer.move_(
            &self.text_label,
            overlay.x.round() as f64,
            overlay.y.round() as f64,
        );
        self.text_label.set_visible(true);
        self.window.queue_draw();
    }

    /// The `wl_display` GTK opened, as a raw pointer.
    ///
    /// Everything Cordial does natively — the engine's own `wl_surface`, the
    /// `wl_subsurface` that parents it here, Mesa's Vulkan WSI and its EGL
    /// winsys — has to be on *this* connection and no other. Wayland object
    /// ids are scoped to the connection that made them, so a second connection
    /// would produce buffers that can never be attached to this surface. That
    /// is why this is exposed rather than the runtime opening its own.
    pub fn wl_display(&self) -> Option<*mut c_void> {
        let display = WidgetExt::display(&self.window).downcast::<gdk4_wayland::WaylandDisplay>().ok()?;
        display.wl_display_raw().map(std::ptr::NonNull::as_ptr)
    }

    /// GDK's own `wl_pointer` for the default seat, borrowed as a raw pointer.
    ///
    /// A Wayland client may create more than one `wl_pointer` from one seat,
    /// but GDK's is the object that owns the desktop cursor for this GTK
    /// window. Pointer constraints attached to a second object can receive a
    /// `locked` event without constraining GDK's cursor, which leaves the host
    /// pointer free to cross the window edge while the engine's cursor stays
    /// centred. The runtime borrows this object for its relative-pointer and
    /// constraint requests; ownership and destruction remain with GDK.
    pub fn wl_pointer(&self) -> Option<*mut c_void> {
        let display = WidgetExt::display(&self.window);
        let pointer = display.default_seat()?.pointer()?;
        let pointer = pointer.downcast::<gdk4_wayland::WaylandDevice>().ok()?;
        pointer.wl_pointer_raw().map(std::ptr::NonNull::as_ptr)
    }

    /// The toplevel's own `wl_surface` — the parent the engine's surface is
    /// made a subsurface of. `None` until the window has been presented and
    /// GTK has realised it, which is what [`Self::wait_until_mapped`] waits
    /// for.
    pub fn wl_surface(&self) -> Option<*mut c_void> {
        let surface = self.window.surface()?;
        let surface = surface.downcast::<gdk4_wayland::WaylandSurface>().ok()?;
        surface.wl_surface_raw().map(std::ptr::NonNull::as_ptr)
    }

    /// Whether the compositor currently considers this window focused.
    ///
    /// `xdg_toplevel`'s `activated` state, which GDK reports as
    /// `GdkToplevelState::FOCUSED`. `None` means there is no toplevel yet, on
    /// the same footing as [`Self::visible`].
    pub fn focused(&self) -> Option<bool> {
        let surface = self.window.surface()?;
        let toplevel = surface.downcast::<gtk::gdk::Toplevel>().ok()?;
        Some(toplevel.state().contains(gtk::gdk::ToplevelState::FOCUSED))
    }

    /// Whether the compositor still considers this window visible.
    ///
    /// `None` means there is no toplevel to ask yet, which is not `Some(false)`
    /// — the caller throttles on this, and treating "not realised" as "not
    /// visible" would throttle a window during its own bring-up.
    ///
    /// `SUSPENDED` is the interesting half and it is a real protocol answer
    /// rather than a guess: `xdg_toplevel`'s `suspended` state, added in
    /// xdg-shell version 6, is the compositor saying the surface's content is
    /// not visible and the client may stop drawing it. GDK surfaces it as
    /// `GdkToplevelState::SUSPENDED`. `MINIMIZED` is included because a
    /// minimised window is not visible either and a compositor is free to
    /// report only that.
    ///
    /// **Measured on this desktop rather than taken from the protocol text** —
    /// see `looper::pump`'s `minimise` script action and the report that used
    /// it. What a compositor does for a *covered* window is its own choice and
    /// is not established here.
    pub fn visible(&self) -> Option<bool> {
        let surface = self.window.surface()?;
        let toplevel = surface.downcast::<gtk::gdk::Toplevel>().ok()?;
        let hidden = gtk::gdk::ToplevelState::MINIMIZED | gtk::gdk::ToplevelState::SUSPENDED;
        Some(!toplevel.state().intersects(hidden))
    }

    /// The toplevel's whole state, for the instrumented run that established
    /// what [`Self::visible`] can actually see. Not used by anything else.
    pub fn toplevel_state(&self) -> Option<gtk::gdk::ToplevelState> {
        let surface = self.window.surface()?;
        Some(surface.downcast::<gtk::gdk::Toplevel>().ok()?.state())
    }

    /// Minimise or restore the window from code, for the same reason
    /// [`Self::set_fullscreen`] exists and under the same rule: the visibility
    /// signal cannot be tested by clicking the real control, because every
    /// route that would do the clicking injects at the compositor and lands on
    /// the developer's session.
    pub fn set_minimised(&self, on: bool) {
        if on {
            self.window.minimize();
        } else {
            self.window.present();
        }
    }

    /// Where the content slot sits inside the toplevel's surface, in surface
    /// coordinates: `(x, y, width, height)`.
    ///
    /// The offset matters and is not the widget's own allocation. A libadwaita
    /// window draws its drop shadow and resize border *inside* its
    /// `wl_surface`, so the content starts some way in from the surface's
    /// origin; `gtk_native_get_surface_transform` is that inset, and
    /// `wl_subsurface.set_position` is expressed in the parent's surface
    /// coordinates. Adding the two is the difference between the engine
    /// landing under the header bar and landing under the shadow.
    pub fn content_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let bounds = self.content.compute_bounds(&self.window)?;
        let (dx, dy) = self.window.surface_transform();
        let w = bounds.width().round() as i32;
        let h = bounds.height().round() as i32;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(((bounds.x() as f64 + dx).round() as i32, (bounds.y() as f64 + dy).round() as i32, w, h))
    }

    /// Run whatever GTK has queued, without blocking.
    ///
    /// Bounded rather than "until nothing is pending": GTK's frame clock can
    /// keep a main context permanently ready while an animation runs, and this
    /// is called from inside the engine's own message pump, which must return.
    pub fn pump(&self) {
        let ctx = glib::MainContext::default();
        for _ in 0..32 {
            if !ctx.iteration(false) {
                break;
            }
        }
    }

    /// Iterate until the window actually exists on the compositor and has been
    /// laid out, or give up.
    ///
    /// Both conditions are needed before a subsurface can be created against
    /// it: `wl_surface` is null until GTK realises the surface, and the content
    /// allocation is zero until the first layout pass, which is what says how
    /// big the engine's surface should be.
    pub fn wait_until_mapped(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.wl_surface().is_some() && self.content_rect().is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("GTK never mapped the window (no wl_surface, or no content allocation)".into());
            }
            self.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Ask GTK to repaint and therefore to commit the toplevel.
    ///
    /// `wl_subsurface.set_position` is double-buffered *on the parent*: it
    /// does nothing at all until the parent surface is committed, and GTK only
    /// commits when it has drawn. Moving the engine's surface without this
    /// leaves it at its old position until something unrelated happens to
    /// repaint the window, which reads as a stuck or torn canvas.
    pub fn queue_commit(&self) {
        self.window.queue_draw();
    }

    /// Fullscreen the window from code, so the configure path can be exercised
    /// without a click.
    ///
    /// **This said "TEMPORARY INSTRUMENTATION -- not for commit" for several
    /// releases while being committed**, on the one window definition ADR-011
    /// makes shared between the shell and the runtime. It is not temporary, and
    /// a doc comment lying about its own status costs more than no comment: the
    /// next person to read it either deletes something load-bearing or learns
    /// to disregard the markers that do mean it.
    ///
    /// Why it cannot be replaced by clicking the real control. Fullscreening is
    /// how `dispatch_configure` and the swapchain recreate behind it get
    /// exercised, and a test cannot press Cordial's own fullscreen button:
    /// every compositor-level injection route — `XTestFake*`, `ydotool`,
    /// `wlr-virtual-keyboard`, the RemoteDesktop portal — lands on whatever has
    /// focus, which is the developer's session, and has already hijacked their
    /// cursor once mid-session. ADR-011 is Wayland, which has no
    /// window-targeted injection to fall back on. Asking GTK directly is what
    /// remains.
    ///
    /// Reached through `android::wayland::instr_set_fullscreen`, from
    /// `looper::pump`'s `CORDIAL_SCRIPT` timeline and from the probes under
    /// `crates/cordial-runtime/examples`.
    pub fn set_fullscreen(&self, on: bool) {
        if on {
            self.window.fullscreen();
        } else {
            self.window.unfullscreen();
        }
    }
}

/// A first guess at the header bar's height, used only to pick the window's
/// initial size so the *content* comes out at the requested resolution. The
/// real height is read back from the widget tree once there is a layout — see
/// [`HostWindow::content_rect`] — so being a few pixels out here costs a
/// resize at startup, not a wrong canvas forever.
fn header_height_hint() -> i32 {
    47
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_caret_counts_characters_and_passwords_never_reach_the_label() {
        assert_eq!(displayed_editor_text("aé中", 2, false), "aé│中");
        assert_eq!(displayed_editor_text("secret", 3, true), "•••│•••");
    }

    #[test]
    fn app_id_matches_the_desktop_entry() {
        // Moved here from `cordial-runtime`'s Wayland backend, which used to
        // set the app_id itself through `xdg_toplevel.set_app_id`. GTK owns
        // the toplevel now, so this constant is what reaches the wire, and the
        // test has to live beside it or it pins nothing. ADR-009 is why the
        // two must agree.
        let desktop = include_str!("../../../packaging/io.github.luohoa97.Cordial.desktop");
        let declared = desktop
            .lines()
            .find_map(|l| l.strip_prefix("StartupWMClass="))
            .expect("desktop entry declares StartupWMClass");
        assert_eq!(declared.trim(), APP_ID);
    }

    #[test]
    fn the_title_does_not_name_a_graphics_backend() {
        // It used to say "(OpenGL ES)" and the engine renders through Vulkan,
        // measured over three runs. A title bar that reports the wrong backend
        // is worse than one that reports none, and this is here so that a
        // future suffix has to be justified rather than pasted back.
        let t = title();
        // Whichever face today wears -- the version has to follow the name
        // either way, and on two days a year the name is not "Cordial".
        let name = crate::branding::current().name();
        assert!(t.starts_with(&format!("{name} ")), "{t}");
        for backend in ["OpenGL", "GLES", "Vulkan"] {
            assert!(!t.contains(backend), "{t} names a graphics backend");
        }
    }

    #[test]
    fn a_window_is_clamped_to_the_monitor_it_opens_on() {
        // The reported bug, in numbers: a 3440x1440 monitor with a 1920x1200
        // one beside it, a 5360x1440 union, and a window sized against nothing
        // at all. Whatever is asked for, the result has to fit the screen it
        // lands on, not the sum of the screens.
        let (w, h) = fit_within((5360, 1440), (1920, 1200));
        assert!(w <= 1920 && h <= 1200, "{w}x{h}");
        let (w, h) = fit_within((3440, 1440), (3440, 1440));
        assert!(w <= 3440 && h <= 1440, "{w}x{h}");
    }

    #[test]
    fn a_window_that_already_fits_is_left_exactly_as_asked() {
        // The clamp must not become a resize of every window. 1280x720 plus a
        // header bar is the runtime's default and has to survive untouched, or
        // the engine renders at a resolution nobody asked for.
        assert_eq!(fit_within((1280, 767), (3440, 1440)), (1280, 767));
    }

    #[test]
    fn a_monitor_smaller_than_the_allowance_still_yields_a_usable_size() {
        // Subtracting the panel allowance from a tiny monitor would otherwise
        // produce a zero or negative default size, which GTK reads as "no
        // default size" rather than as an error — a silently ignored clamp.
        let (w, h) = fit_within((1280, 767), (64, 48));
        assert!(w > 0 && h > 0, "{w}x{h}");
    }
}
