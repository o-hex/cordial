//! The shell window: [`cordial_shell::host_window`]'s window with the chooser
//! in its content slot.
//!
//! ADR-002 calls the core shell "a bridge measured in milliseconds, not a
//! product" — window, chooser, and an escape hatch to settings. Nothing here
//! should grow features; anything richer belongs to the UI plugin that takes
//! over at T3.
//!
//! What this file owns beyond the widgets is the *decision*: ADR-002's
//! correction says a plugin declares what and core decides how, and this is
//! core deciding. The chooser hands over an id; everything between that id and
//! a running client — finding a build, claiming a profile, refusing when the
//! profile is taken, saying so when there is nothing installed — happens here,
//! and every branch of it ends in something the user can see.

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crate::chooser;
use crate::deep_link;
use crate::install::{self, NotFound};
use crate::instructions;
use crate::launch;
use crate::profile_switcher;
use crate::refresh_watch;
use crate::settings;
use crate::shell_config::ShellConfig;
use crate::updater;
use cordial_shell::host_window::HostWindow;
use cordial_shell::profile;

/// A `roblox-player://` link the desktop handed over, held until the user
/// presses Roblox.
///
/// **Deliberately not a launch.** A link could start the client outright, and
/// that is what a browser handler usually does; here it would skip the two
/// decisions the launcher exists to take — which profile, and against which
/// build — and it would do it in response to a click in another application.
/// So the link opens the launcher, the launcher says a join is waiting, and the
/// user presses Roblox as they otherwise would.
///
/// The banner is not decoration either. A link that vanished into a variable
/// would be indistinguishable from a link that was dropped, and "it ignored my
/// click" is the report that follows.
#[derive(Clone)]
pub struct PendingJoin {
    url: Rc<RefCell<Option<String>>>,
    banner: adw::Banner,
}

impl PendingJoin {
    fn new() -> Self {
        let banner = adw::Banner::builder().revealed(false).button_label("Discard").build();
        let url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        {
            // A queued join the user cannot get rid of is a trap: the next
            // launch would carry a link they have changed their mind about, and
            // the only way out would be closing the launcher.
            let url = url.clone();
            banner.connect_button_clicked(move |banner| {
                *url.borrow_mut() = None;
                banner.set_revealed(false);
            });
        }
        PendingJoin { url, banner }
    }

    /// Show a link as waiting. Replaces whatever was queued: two links means the
    /// second click is the one the user is looking at.
    pub fn queue(&self, url: String) {
        self.banner.set_title(&banner_line(&url));
        self.banner.set_revealed(true);
        *self.url.borrow_mut() = Some(url);
    }

    /// What the next launch should carry, without consuming it: a launch that
    /// fails must leave the link where it was, or a busy profile would cost the
    /// user the link as well as the launch.
    fn peek(&self) -> Option<String> {
        self.url.borrow().clone()
    }

    fn clear(&self) {
        *self.url.borrow_mut() = None;
        self.banner.set_revealed(false);
    }

    fn banner(&self) -> &adw::Banner {
        &self.banner
    }
}

/// What the banner says about a waiting link.
///
/// Pure, and separate from the widget, for the reason `busy_body` is: a string
/// that can only be inspected by building a window and photographing it is a
/// string that drifts. It also has one genuine hazard in it — `AdwBanner`'s
/// title is Pango markup and this text came from a browser, so an unescaped `&`
/// in a query string is enough to make GTK drop the label, and anything sharper
/// is worse than that.
fn banner_line(url: &str) -> String {
    let shown = glib::markup_escape_text(&deep_link::summarise(url));
    format!("Roblox link waiting: {shown} — press Roblox to join")
}

/// The running shell, for the handful of things that happen to it from outside.
///
/// `main.rs` holds one so that a second invocation carrying a link — which is
/// what the desktop does when a browser opens `roblox-player://` while Cordial
/// is up — reaches the window that already exists rather than starting another.
pub struct Shell {
    window: adw::Window,
    join: PendingJoin,
}

impl Shell {
    /// Bring the launcher forward and show the link as waiting.
    pub fn queue_join(&self, url: String) {
        self.join.queue(url);
        self.window.present();
    }

    /// Bring the launcher forward, for a second invocation carrying nothing.
    pub fn present(&self) {
        self.window.present();
    }
}

/// How long to wait before deciding a client that has already exited failed.
///
/// `cordial-run` reaching its first frame takes seconds; failing at load takes
/// a fraction of one. Anything gone by the time this fires never started, and
/// the launcher has to say so — the alternative is a toast saying Roblox is
/// starting and then nothing at all, which is exactly the silent no-op this
/// whole change exists to remove.
const EARLY_EXIT_CHECK: std::time::Duration = std::time::Duration::from_secs(3);

/// The dialog shown while the client starts.
///
/// Roblox takes a while to get from a launch to a window of its own, and until
/// now that gap was a toast and then nothing -- the launcher looked idle while
/// the most failure-prone part of the whole program was happening. This says
/// "working" for as long as that lasts.
///
/// **The progress bar is indeterminate on purpose.** Nothing here knows how far
/// along the client is: the shell holds a pid, not a progress channel, and a
/// bar that filled at a made-up rate would be inventing a measurement, which is
/// the one thing this project is strict about. A pulsing bar claims only that
/// something is still happening, which is true and is all that is known.
///
/// The icon is Cordial's own. It is deliberately the only customisable part and
/// it is customised by pointing at a file, never by shipping alternatives:
/// Roblox's own icons are their assets, and AGENTS.md rules out vendoring any
/// of them however small.
fn starting_dialog(parent: &gtk::Window, profile: &str, joining: bool) -> gtk::Window {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_top(28);
    content.set_margin_bottom(24);
    content.set_margin_start(36);
    content.set_margin_end(36);

    let icon = gtk::Image::from_icon_name("io.github.luohoa97.Cordial");
    icon.set_pixel_size(72);
    content.append(&icon);

    let title = gtk::Label::new(Some(&match joining {
        true => format!("Starting Roblox on {profile}, joining the link"),
        false => format!("Starting Roblox on {profile}"),
    }));
    title.add_css_class("title-4");
    title.set_wrap(true);
    title.set_justify(gtk::Justification::Center);
    title.set_max_width_chars(32);
    content.append(&title);

    let bar = gtk::ProgressBar::new();
    bar.set_width_request(260);
    content.append(&bar);

    // 12 pulses a second is the shell's own animation and costs nothing; it is
    // not tied to anything the client is doing and must not be read as though
    // it were.
    let pulse = glib::timeout_add_local(Duration::from_millis(80), {
        let bar = bar.clone();
        move || {
            bar.pulse();
            glib::ControlFlow::Continue
        }
    });

    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .deletable(false)
        .titlebar(&{
            let h = adw::HeaderBar::new();
            h.set_show_end_title_buttons(false);
            h.add_css_class("flat");
            h
        })
        .child(&content)
        .build();

    // Stop the pulse whenever the dialog goes, however it goes -- closed here
    // when the client is up, or closed by the early-exit path when it is not.
    // A timeout left running against a dropped widget is a warning per frame
    // for the rest of the session.
    let pulse = std::cell::RefCell::new(Some(pulse));
    dialog.connect_close_request(move |_| {
        if let Some(id) = pulse.borrow_mut().take() {
            id.remove();
        }
        glib::Propagation::Proceed
    });

    dialog.present();
    dialog
}



pub fn build(
    app: &adw::Application,
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> Shell {
    // T1: the chooser core paints before the plugin host is up. One entry, and
    // it starts the client — see `chooser.rs` on why the second one went.
    let source = chooser::CordialSource;

    // The toast overlay has to exist before the chooser, because the chooser's
    // activate handler reports through it.
    let toasts = adw::ToastOverlay::new();

    // Empty and hidden until the desktop hands over a link, which on most runs
    // is never; `AdwBanner` takes no space while it is not revealed.
    let join = PendingJoin::new();

    let chooser_widget = {
        let toasts = toasts.clone();
        let config = config.clone();
        let join = join.clone();
        chooser::build(&source, move |id| {
            let Some(window) = toasts.root().and_downcast::<gtk::Window>() else { return };
            match id {
                chooser::ROBLOX => activate_roblox(&window, &toasts, &config, &join),
                // Unreachable while there is one entry, and deliberately loud
                // rather than ignored: the moment plugin-contributed entries
                // exist, an id core does not know how to launch is a bug in the
                // registration path, and a silently dead row is how that bug
                // gets shipped.
                other => alert(&window, "Cordial does not know how to launch that", other),
            }
        })
    };

    // Directly above the Launch button, and that position is the whole argument:
    // the profile is a launch parameter — which of these do I start — rather
    // than an ambient identity, so it belongs beside the button it governs. It
    // was an avatar in the top-right corner first; see `profile_switcher.rs` for
    // why that was a browser convention borrowed into an application that is not
    // one.
    let profile_row = profile_switcher::build(config.clone(), config_path.clone());

    // Two controls, centred, and the empty space around them is the point.
    //
    // This was a stack of two `AdwPreferencesGroup`s pinned to the top of the
    // window, which left the bottom two thirds blank and read as a form that had
    // run out of fields. Nothing was added to fill it: the same two controls
    // sitting in the middle of the window is a composition rather than a
    // remainder, and the width clamp is tighter than the 480 the groups used
    // because a boxed list and a pill button stretched to a launcher's full
    // width look like a preferences page whatever is in them.
    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
    column.set_valign(gtk::Align::Center);
    column.append(&profile_row);
    column.append(&chooser_widget);

    let clamp = adw::Clamp::builder().maximum_size(360).child(&column).build();
    clamp.set_margin_top(24);
    clamp.set_margin_bottom(24);
    clamp.set_margin_start(24);
    clamp.set_margin_end(24);
    clamp.set_vexpand(true);

    // The banner sits above the centred column rather than inside it, so that a
    // queued join reads as a message about the window instead of a third
    // control in the composition. It expands nothing while it is hidden, which
    // is why the launcher looks the same as it always did on every run where no
    // link arrives.
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(join.banner());
    body.append(&clamp);
    toasts.set_child(Some(&body));

    // --- seam for the engine's surface -----------------------------------
    // This window is the same definition `cordial-runtime` builds to host the
    // engine's Wayland surface — see `host_window`, and ADR-011 on why there
    // must not be two. What differs between the two callers is only what goes
    // in the content slot: the chooser here, the engine's `wl_subsurface`
    // there. The two processes are still separate, so nothing is handed over
    // between them yet; what is shared today is the window, its header bar
    // and its theming.
    //
    // When the T3 handoff does exist, the shape is still a second child under
    // the toolbar view — a `gtk::Stack` page swapped in — rather than tearing
    // this window down and building another. ADR-002's open question about a
    // UI-plugin crash is why: the shell has to stay retained and hidden rather
    // than destroyed, so the window, header bar and theme survive the handoff
    // instead of flickering through a second window creation.
    // -----------------------------------------------------------------------

    // 540x340 was 720x480 while the content was two preference groups pinned to
    // the top of it, which left the lower two thirds empty. This is a launcher's
    // size rather than a settings window's: wide enough that a development
    // build's title — `git describe` output, beside two header-bar buttons —
    // does not ellipsise, and tall enough that the centred column has room
    // around it without being adrift in it. Only the *default*; the window
    // resizes, and the runtime passes its own size to this same constructor for
    // the engine's canvas.
    let host = HostWindow::new(&cordial_shell::host_window::title(), 540, 340, &toasts);

    let settings_button = gtk::Button::from_icon_name("preferences-system-symbolic");
    settings_button.set_tooltip_text(Some("Settings"));
    host.header().pack_end(&settings_button);

    let window = host.window().clone();
    // `HostWindow` is deliberately application-less — the runtime has no
    // `GApplication` — so the shell binary attaches its own here, which is
    // what makes the window keep `app` alive and quit with it.
    window.set_application(Some(app));

    // Immediately left of Settings, which is what packing it second at the end
    // produces, and always present. A control that comes and goes is hard to
    // find at the moment you want it and moves everything beside it when it
    // arrives — and this one answers "which Roblox build am I on", which is
    // worth a button whether or not anything newer exists.
    let update_button = updater::header_button(&window, config.clone());
    host.header().pack_end(&update_button);


    // An action rather than only a button handler, because the header bar is no
    // longer the only way in: a launch refused for a busy profile offers to
    // open settings, and that offer must land on the same window this button
    // opens. `AdwWindow` is not a `GtkApplicationWindow` and so carries no
    // action map of its own, hence the explicit group.
    //
    // It takes a string, which is the name of the page to open on and is empty
    // for "whichever libadwaita shows first". That parameter exists for the
    // photograph seam below: Settings has five tabs now and only the first one
    // can be seen without pressing something, which under ADR-011's Wayland is
    // not a thing an agent may do — see `open_on_start`. It goes through the
    // same action the button does rather than a second way in.
    let settings_action =
        gtk::gio::SimpleAction::new("settings", Some(glib::VariantTy::STRING));
    let window_for_settings = window.clone();
    settings_action.connect_activate(move |_, page| {
        let settings = settings::build_preferences_window(
            &window_for_settings,
            config.clone(),
            config_path.clone(),
        );
        if let Some(name) = page.and_then(|p| p.str()).filter(|n| !n.is_empty()) {
            settings.set_visible_page_name(name);
        }
        settings.present();
    });
    // The same arrangement for the profile row: a launch refused because the
    // profile is busy has to be able to reach the control that chooses another
    // one. It is in this window rather than behind a button now, so the action
    // only has to move the focus there.
    let profile_action = gtk::gio::SimpleAction::new("profile", None);
    let profile_row_for_action = profile_row.clone();
    profile_action.connect_activate(move |_, _| {
        profile_row_for_action.grab_focus();
    });

    let actions = gtk::gio::SimpleActionGroup::new();
    actions.add_action(&settings_action);
    actions.add_action(&profile_action);
    window.insert_action_group("win", Some(&actions));
    // The target before the name, and in that order deliberately: GTK's action
    // helper re-checks the pair every time either changes, and naming a
    // string-taking action while the target is still unset logs "can't be
    // activated due to parameter type mismatch" on every startup. The empty
    // string is "open on whatever page libadwaita shows first", which is what a
    // press of this button has always meant.
    settings_button.set_action_target_value(Some(&"".to_variant()));
    settings_button.set_action_name(Some("win.settings"));

    // Focus starts on the thing the window is for, so that a keyboard launches
    // with Return and nothing else. Left alone it lands on the profile row,
    // which is the first focusable widget and the one control here that is not
    // the point. GTK only *draws* a focus ring once a key has been pressed, so
    // this costs nothing visually on a window nobody types into.
    // Spelled out because `GtkWindowExt` and `RootExt` both offer `set_focus`
    // and neither is more obviously meant than the other.
    GtkWindowExt::set_focus(&window, Some(&chooser_widget));

    window.present();
    open_on_start(&window, &update_button);

    // This is the shell's own launcher window, not the engine's -- the two
    // are the same *definition* per ADR-011 but separate processes with
    // separate `HostWindow`s, and the one hosting the engine's subsurface is
    // built by `cordial-run`, out of reach from here. Watching this one is
    // what proves `refresh_watch` observes a real display correctly; the
    // callback is empty because there is nothing on this side of the process
    // boundary to tell -- see `refresh_watch.rs` for where the JNI wiring
    // belongs instead.
    refresh_watch::watch(&window, |_outputs| {});

    Shell { window, join }
}

/// `CORDIAL_SHELL_PRESENT=settings,settings=updates,update` opens those windows
/// at startup.
///
/// A test seam, and it exists because there is no other way to photograph these
/// windows. AGENTS.md forbids synthesising input at the compositor — it lands on
/// whatever has focus, which is the developer's session, and it has hijacked a
/// cursor here once — and Wayland has no window-targeted injection to fall back
/// on, so "click Settings and take a screenshot" is not an available sentence.
/// Asking the shell to open its own window is, and it goes through exactly the
/// same action and the same button handler a click does rather than a second
/// construction path that could differ from the real one.
///
/// **`settings=<page>` opens Settings on a named page**, which is the same
/// problem one level down: the window has five tabs and a photograph of it shows
/// one. The names are the `name` each `AdwPreferencesPage` is built with —
/// `roblox`, `updates`, `appearance`, `general`, `plugins` — and an unknown one
/// is libadwaita's warning to answer, not this function's, because a name that
/// silently fell back to the first page would produce a screenshot captioned as
/// something it is not.
///
/// Same shape as `CORDIAL_SHELL_RUN_SECONDS` above: an environment variable that
/// nothing sets in normal use, doing something a person could do by hand.
fn open_on_start(window: &adw::Window, update_button: &gtk::Button) {
    let Some(want) = std::env::var("CORDIAL_SHELL_PRESENT").ok().filter(|s| !s.is_empty()) else {
        return;
    };
    let window = window.clone();
    let update_button = update_button.clone();
    // After a beat, so the main window is mapped and photographable underneath
    // whatever this opens on top of it.
    glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
        for one in want.split(',').map(str::trim) {
            // `settings` and `settings=updates` are the same action with and
            // without a page, so they are split rather than matched separately.
            let (what, page) = one.split_once('=').unwrap_or((one, ""));
            match what {
                "settings" => {
                    let _ = window.activate_action("win.settings", Some(&page.to_variant()));
                }
                "update" => update_button.emit_clicked(),
                other => println!("  shell: CORDIAL_SHELL_PRESENT does not know {other:?}"),
            }
        }
    });
}

/// The whole of what pressing Roblox does.
///
/// Split out so the instructions window's "check again" button can call exactly
/// the same thing rather than a second, similar-looking path — the failure that
/// arrangement produces is a retry button that succeeds where the row does not,
/// or the reverse, and neither is discoverable from the outside.
fn activate_roblox(
    window: &gtk::Window,
    toasts: &adw::ToastOverlay,
    config: &Rc<RefCell<ShellConfig>>,
    join: &PendingJoin,
) {
    match try_launch(window, config, join) {
        Outcome::Started => {}
        Outcome::Failed(message) => alert(window, "Roblox could not start", &message),
        Outcome::ProfileBusy(name, holder) => {
            profile_busy(window, toasts, config, join, &name, holder)
        }
        Outcome::NoBuild => {
            let window = window.clone();
            let config = config.clone();
            let join = join.clone();
            instructions::present(&window.clone(), move || {
                // Returning whether it worked is what lets the instructions
                // window stay up while the user is still following them.
                matches!(try_launch(&window, &config, &join), Outcome::Started)
            });
        }
    }
}

enum Outcome {
    Started,
    NoBuild,
    /// The profile is held by another instance. Separated from `Failed` because
    /// it is not a fault and the answer is not "close this dialog": people
    /// reach it by double-clicking launch, and what they need is a way to run
    /// against a different profile.
    ///
    /// Carries the holder so the dialog can name it. Said "another window" once
    /// and that was the misleading part — the holder frequently has no window,
    /// which is exactly what makes this hard to recover from unaided.
    ProfileBusy(String, Option<profile::Holder>),
    Failed(String),
}

/// No `ToastOverlay` any more: the "Starting Roblox" toast this used to raise
/// is now the loading dialog, which says the same thing and stays up for as
/// long as it is true.
fn try_launch(
    window: &gtk::Window,
    config: &Rc<RefCell<ShellConfig>>,
    join: &PendingJoin,
) -> Outcome {
    let (roblox, profile_name) = {
        let config = config.borrow();
        (config.roblox.clone(), config.profile.clone())
    };

    let build = match install::locate(&roblox) {
        Ok(build) => build,
        Err(NotFound::NoBuild) => return Outcome::NoBuild,
        Err(NotFound::Unusable(message)) => return Outcome::Failed(message),
    };

    // ADR-012's claim, taken before the process exists so that a refusal
    // costs nothing. A second window on one profile is two processes writing
    // one cookie store; the message names the profile because "already open"
    // on its own does not tell anyone which one to close.
    let claim = match profile::acquire(&profile_name) {
        Ok(claim) => claim,
        Err(profile::Error::Busy(name, holder)) => return Outcome::ProfileBusy(name, holder),
        Err(e) => return Outcome::Failed(e.to_string()),
    };

    // Read rather than taken: everything above this can still refuse, and a
    // busy profile that also cost the user their link would be two failures for
    // one press. It is cleared below, once there is a process holding it.
    let url = join.peek();
    let mut instance = match launch::spawn(&build, claim, run_seconds_override(), url.as_deref()) {
        Ok(instance) => instance,
        Err(message) => return Outcome::Failed(message),
    };
    join.clear();

    let starting = starting_dialog(&window, &profile_name, url.is_some());

    // A client that is already gone a moment later never started. Checked
    // rather than assumed, because the shell may well have been started from a
    // desktop icon with nowhere for the loader's own output to go.
    let window = window.clone();
    glib::timeout_add_local_once(EARLY_EXIT_CHECK, move || {
        starting.close();
        if let Some(status) = instance.exited() {
            alert(
                &window,
                "Roblox stopped as soon as it started",
                &format!(
                    "cordial-run exited with {status}. Running this in a terminal will show \
                     what it printed:\n\n{}",
                    instance.command_line
                ),
            );
        }
    });

    Outcome::Started
}

/// Shorten a session, for testing. `--run` is a hard timer in `cordial-run` and
/// the launcher's own default is a day, which is unhelpful when what is being
/// checked is that a launch happens at all.
fn run_seconds_override() -> Option<u64> {
    std::env::var("CORDIAL_SHELL_RUN_SECONDS").ok().and_then(|v| v.parse().ok())
}

/// The profile is already running, and here is what to do about it.
///
/// Deliberately not phrased as an error. ADR-012's lock refusing a second
/// instance is the design working, and the ordinary way to reach it is
/// double-clicking the launcher — so the dialog names the profile, says what is
/// true, and offers the only action that helps: choosing a different one.
///
/// It used to point at a text field in Settings. It now moves the focus to the
/// profile row above the Launch button, which already shows the profile this
/// dialog is about as unavailable.
/// The refusal a held profile earns, written so somebody can act on it.
///
/// **The old wording said "close the window that already has it" and that was
/// the actively misleading part.** The holder very often has no window: closing
/// the engine's window does not end `cordial-run` yet, and the launcher's
/// `--run` default is a day, so a client outlives the window that started it,
/// is reparented to `systemd --user`, and keeps the profile for the rest of its
/// timer. This developer met exactly that — a client 31 minutes into a 24-hour
/// timer holding `default` with nothing on screen to close — and the interface
/// told them to close a window that did not exist.
///
/// So the dialog names the process and offers to stop it. Someone who cannot
/// find a window is not confused; they are looking at a correct description of
/// a situation the previous text could not express.
fn profile_busy(
    parent: &gtk::Window,
    toasts: &adw::ToastOverlay,
    config: &Rc<RefCell<ShellConfig>>,
    join: &PendingJoin,
    name: &str,
    holder: Option<profile::Holder>,
) {
    let ours = holder.as_ref().filter(|h| h.is_cordial()).cloned();
    let body = busy_body(holder.as_ref());

    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading(format!("Profile {name} is already in use"))
        .body(body)
        .build();
    dialog.add_response("close", "Close");
    dialog.add_response("profile", "Choose a Profile");
    if ours.is_some() {
        dialog.add_response("stop", "Close It and Launch");
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("stop"));
    } else {
        dialog.set_response_appearance("profile", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("profile"));
    }

    let parent_window = parent.clone();
    let toasts = toasts.clone();
    let config = config.clone();
    let join = join.clone();
    dialog.connect_response(None, move |_, response| match response {
        // The switcher is the combo row above the launch button; activating the
        // window's action rather than building a second chooser here keeps one
        // construction site.
        "profile" => {
            let _ = parent_window.activate_action("win.profile", None);
        }
        "stop" => {
            if let Some(h) = ours.clone() {
                close_then_launch(
                    parent_window.clone(),
                    toasts.clone(),
                    config.clone(),
                    join.clone(),
                    h,
                );
            }
        }
        _ => {}
    });
    dialog.present();
}

/// What the refusal says, given who is holding the profile.
///
/// Pulled out of the dialog and made pure so the wording can be pinned by
/// tests. These three paragraphs are the entire recovery path for somebody who
/// cannot find a window to close, and the previous version of this text was
/// wrong in a way nobody noticed for months — a widget that has to be built and
/// clicked to inspect is a widget whose text drifts.
fn busy_body(holder: Option<&profile::Holder>) -> String {
    // The reason is the same in every branch, so it is written once.
    const WHY: &str = "A profile opens in one client at a time. Roblox keeps its session and \
                       its storage in there, and two clients writing to it at once corrupts \
                       both.";

    match holder {
        Some(h) if h.is_cordial() => {
            let started =
                h.running_for_text().map(|t| format!(", running for {t}")).unwrap_or_default();
            format!(
                "Cordial is already running on this profile as process {}{started}.\n\n{WHY}\n\n\
                 If you cannot find a Cordial window anywhere, that is expected rather than \
                 strange. Closing the Roblox window does not end the client yet, so one left \
                 over from an earlier session keeps running — and keeps this profile — until \
                 its own timer runs out. Closing it here is safe.",
                h.pid
            )
        }
        Some(h) => format!(
            "Process {} has this profile's lock file open, and it is not a Cordial client:\n\n\
             {}\n\n{WHY}\n\nCordial will not close a process it did not start. Close it \
             yourself, or launch against a different profile.",
            h.pid, h.command
        ),
        None => format!(
            "{WHY}\n\nCordial could not tell which process is holding it. That usually means \
             the other client belongs to a different user account, or is running inside a \
             container this one cannot see into. Launch against a different profile."
        ),
    }
}

/// Stop the client holding a profile, then launch once it is actually gone.
///
/// Waiting on [`profile::Holder::has_exited`] rather than sleeping a fixed
/// interval, because the thing being waited for is a Roblox client flushing
/// storage and there is no interval that is both short enough to feel like a
/// button and long enough to be true.
///
/// Gives up out loud. A recovery path that silently does nothing is worse than
/// the refusal it was reached from, since the user has now been told the
/// problem was handled.
fn close_then_launch(
    window: gtk::Window,
    toasts: adw::ToastOverlay,
    config: Rc<RefCell<ShellConfig>>,
    join: PendingJoin,
    holder: profile::Holder,
) {
    if let Err(message) = holder.ask_to_stop() {
        alert(&window, "Could not close the other client", &message);
        return;
    }
    let waited = Cell::new(Duration::ZERO);
    glib::timeout_add_local(STOP_POLL, move || {
        if holder.has_exited() {
            activate_roblox(&window, &toasts, &config, &join);
            return glib::ControlFlow::Break;
        }
        waited.set(waited.get() + STOP_POLL);
        if waited.get() < STOP_GIVE_UP {
            return glib::ControlFlow::Continue;
        }
        alert(
            &window,
            "The other client is still running",
            &format!(
                "Process {} was asked to close and has not stopped after {} seconds. It may be \
                 busy saving. Wait a moment and press Roblox again, or close it yourself.",
                holder.pid,
                STOP_GIVE_UP.as_secs()
            ),
        );
        glib::ControlFlow::Break
    });
}

/// How often to look for the old client having gone, and how long to keep
/// looking. Ten seconds is past any shutdown observed here and short enough
/// that a wedged client still produces an answer rather than a spinner.
const STOP_POLL: Duration = Duration::from_millis(250);
const STOP_GIVE_UP: Duration = Duration::from_secs(10);

/// Say something went wrong, in a dialog the user has to acknowledge.
///
/// A toast would be wrong for these: every one of them needs an action from the
/// user, and a message that fades after four seconds is barely better than no
/// message. The toast is for the good case.
pub fn alert(parent: &gtk::Window, heading: &str, body: &str) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading(heading)
        .body(body)
        .build();
    dialog.add_response("ok", "Close");
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn holder(command: &str) -> profile::Holder {
        profile::Holder {
            pid: 649889,
            command: command.into(),
            running_for: Some(Duration::from_secs(31 * 60)),
        }
    }

    #[test]
    fn the_no_window_case_is_explained_rather_than_left_a_mystery() {
        // The sentence this pins is the whole point of the dialog. Somebody
        // hunting for a Cordial window that does not exist needs to be told
        // that is expected, and told which process to close instead -- the old
        // text sent them looking for a window and had nothing else to offer.
        let body = busy_body(Some(&holder("/app/bin/cordial-run --profile default")));
        assert!(body.contains("process 649889"), "{body}");
        assert!(body.contains("running for 31 minutes"), "{body}");
        assert!(body.contains("cannot find a Cordial window"), "{body}");
        assert!(!body.contains("close the window that already has it"), "{body}");
    }

    #[test]
    fn a_stranger_holding_the_lock_is_named_but_not_offered_up_for_killing() {
        // is_cordial gates the "Close It and Launch" response, so the body for
        // a process Cordial did not start must not imply it can be closed from
        // here. It says what has the file open and stops.
        let body = busy_body(Some(&holder("/usr/bin/grep -r something")));
        assert!(body.contains("not a Cordial client"), "{body}");
        assert!(body.contains("/usr/bin/grep"), "{body}");
        assert!(body.contains("will not close a process it did not start"), "{body}");
    }

    #[test]
    fn the_waiting_link_is_shown_escaped_and_short() {
        // Both hazards in one line of text. The `&` is not hypothetical —
        // Roblox's own links join their fields with one — and an `AdwBanner`
        // fed raw markup drops the label entirely, so the queued join would
        // become a blank grey bar: the exact "it ignored my click" this banner
        // exists to prevent, wearing the banner as a disguise.
        // The banner no longer shows the payload at all -- see
        // `deep_link::summarise`, which used to truncate to 64 characters and
        // put 24 characters of a one-time auth ticket on screen. So the first
        // thing to assert is absence.
        let line = banner_line("roblox-player://placeId=1818&launchData=<x>");
        assert!(!line.contains("placeId"), "{line}");
        assert!(!line.contains("launchData"), "{line}");
        assert!(line.contains("roblox-player:"), "{line}");
        assert!(line.contains("press Roblox"), "{line}");

        // The escaping stays even though nothing reaching it should now need
        // escaping. It is one call, and the failure it prevents is total: an
        // `AdwBanner` fed raw markup drops the label entirely, so the queued
        // join becomes a blank grey bar -- the exact "it ignored my click" this
        // banner exists to prevent, wearing the banner as a disguise. Belt and
        // braces on a label built from attacker-influenced input is the correct
        // amount of paranoia.
        assert!(!banner_line("roblox-player://<b>x</b>").contains("<b>"));

        // And it cannot stretch the window, whatever length Roblox's payload
        // happens to be this year.
        let long = banner_line(&format!("roblox-player://{}", "a".repeat(4000)));
        assert!(long.chars().count() < 120, "{}", long.chars().count());
    }

    #[test]
    fn an_unidentifiable_holder_says_so_instead_of_inventing_one() {
        // `holder_of` returns None for "could not tell", never for "nobody
        // holds it" -- the flock already proved somebody does. The body has to
        // preserve that distinction or it becomes a message claiming the
        // profile is both taken and free.
        let body = busy_body(None);
        assert!(body.contains("could not tell which process"), "{body}");
        assert!(!body.contains("process 649889"), "{body}");
    }
}
