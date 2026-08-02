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
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::chooser;
use crate::flags_file;
use crate::install::{self, NotFound};
use crate::instructions;
use crate::launch;
use crate::profile_switcher;
use crate::settings;
use crate::shell_config::ShellConfig;
use cordial_shell::host_window::HostWindow;
use cordial_shell::profile;

/// How long to wait before deciding a client that has already exited failed.
///
/// `cordial-run` reaching its first frame takes seconds; failing at load takes
/// a fraction of one. Anything gone by the time this fires never started, and
/// the launcher has to say so — the alternative is a toast saying Roblox is
/// starting and then nothing at all, which is exactly the silent no-op this
/// whole change exists to remove.
const EARLY_EXIT_CHECK: std::time::Duration = std::time::Duration::from_secs(3);

pub fn build(app: &adw::Application, config: Rc<RefCell<ShellConfig>>, config_path: Rc<PathBuf>) {
    // T1: the chooser core paints before the plugin host is up. One entry, and
    // it starts the client — see `chooser.rs` on why the second one went.
    let source = chooser::CordialSource;

    // The toast overlay has to exist before the chooser, because the chooser's
    // activate handler reports through it.
    let toasts = adw::ToastOverlay::new();

    let chooser_widget = {
        let toasts = toasts.clone();
        let config = config.clone();
        chooser::build(&source, move |id| {
            let Some(window) = toasts.root().and_downcast::<gtk::Window>() else { return };
            match id {
                chooser::ROBLOX => activate_roblox(&window, &toasts, &config),
                // Unreachable while there is one entry, and deliberately loud
                // rather than ignored: the moment plugin-contributed entries
                // exist, an id core does not know how to launch is a bug in the
                // registration path, and a silently dead row is how that bug
                // gets shipped.
                other => alert(&window, "Cordial does not know how to launch that", other),
            }
        })
    };
    toasts.set_child(Some(&chooser_widget));

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
    let host = HostWindow::new(&cordial_shell::host_window::title(), 720, 480, &toasts);

    // Packed first, so it is the rightmost thing in the header bar. An avatar
    // is what libadwaita offers for representing a user and a `GtkMenuButton`
    // opening a popover is an ordinary GNOME header bar, so this fights nothing;
    // Fractal switches accounts the same way. What it is *not* is an account
    // switcher — ADR-012 is explicit that this selects a directory, does not
    // authenticate, and must not imply that it does.
    let switcher = profile_switcher::build(config.clone(), config_path.clone());
    host.header().pack_end(&switcher);

    let settings_button = gtk::Button::from_icon_name("preferences-system-symbolic");
    settings_button.set_tooltip_text(Some("Settings"));
    host.header().pack_end(&settings_button);

    let window = host.window().clone();
    // `HostWindow` is deliberately application-less — the runtime has no
    // `GApplication` — so the shell binary attaches its own here, which is
    // what makes the window keep `app` alive and quit with it.
    window.set_application(Some(app));

    // Demo data now; see settings.rs for what backs this once the plugin host
    // can actually answer "what's installed". Rc so every click on the
    // settings button reopens against the same (in-memory) state rather than
    // resetting it.
    let registry: Rc<dyn settings::PluginRegistry> = settings::DemoRegistry::installed();
    let flags_path = Rc::new(flags_file::user_flags_path());

    // An action rather than only a button handler, because the header bar is no
    // longer the only way in: a launch refused for a busy profile offers to
    // open settings, and that offer must land on the same window this button
    // opens. `AdwWindow` is not a `GtkApplicationWindow` and so carries no
    // action map of its own, hence the explicit group.
    let settings_action = gtk::gio::SimpleAction::new("settings", None);
    let window_for_settings = window.clone();
    settings_action.connect_activate(move |_, _| {
        settings::build_preferences_window(
            &window_for_settings,
            registry.clone(),
            config.clone(),
            config_path.clone(),
            flags_path.clone(),
        )
        .present();
    });
    // The same arrangement for the switcher, and for the same reason: a launch
    // refused because the profile is busy has to be able to open the thing that
    // chooses another one, and that is now the header bar's popover rather than
    // a settings page.
    let profile_action = gtk::gio::SimpleAction::new("profile", None);
    let switcher_for_action = switcher.clone();
    profile_action.connect_activate(move |_, _| switcher_for_action.popup());

    let actions = gtk::gio::SimpleActionGroup::new();
    actions.add_action(&settings_action);
    actions.add_action(&profile_action);
    window.insert_action_group("win", Some(&actions));
    settings_button.set_action_name(Some("win.settings"));

    window.present();

}

/// The whole of what pressing Roblox does.
///
/// Split out so the instructions window's "check again" button can call exactly
/// the same thing rather than a second, similar-looking path — the failure that
/// arrangement produces is a retry button that succeeds where the row does not,
/// or the reverse, and neither is discoverable from the outside.
fn activate_roblox(window: &gtk::Window, toasts: &adw::ToastOverlay, config: &Rc<RefCell<ShellConfig>>) {
    match try_launch(window, toasts, config) {
        Outcome::Started => {}
        Outcome::Failed(message) => alert(window, "Roblox could not start", &message),
        Outcome::ProfileBusy(name) => profile_busy(window, &name),
        Outcome::NoBuild => {
            let window = window.clone();
            let toasts = toasts.clone();
            let config = config.clone();
            instructions::present(&window.clone(), move || {
                // Returning whether it worked is what lets the instructions
                // window stay up while the user is still following them.
                matches!(try_launch(&window, &toasts, &config), Outcome::Started)
            });
        }
    }
}

enum Outcome {
    Started,
    NoBuild,
    /// The profile is held by another window. Separated from `Failed` because
    /// it is not a fault and the answer is not "close this dialog": people
    /// reach it by double-clicking launch, and what they need is a way to run
    /// against a different profile.
    ProfileBusy(String),
    Failed(String),
}

fn try_launch(window: &gtk::Window, toasts: &adw::ToastOverlay, config: &Rc<RefCell<ShellConfig>>) -> Outcome {
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
        Err(profile::Error::Busy(name)) => return Outcome::ProfileBusy(name),
        Err(e) => return Outcome::Failed(e.to_string()),
    };

    let mut instance = match launch::spawn(&build, claim, run_seconds_override()) {
        Ok(instance) => instance,
        Err(message) => return Outcome::Failed(message),
    };

    toasts.add_toast(adw::Toast::new(&format!("Starting Roblox on profile {profile_name}")));

    // A client that is already gone a moment later never started. Checked
    // rather than assumed, because the shell may well have been started from a
    // desktop icon with nowhere for the loader's own output to go.
    let window = window.clone();
    glib::timeout_add_local_once(EARLY_EXIT_CHECK, move || {
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
/// It used to point at a text field in Settings. It now opens the header bar's
/// switcher, which is the same door the avatar is, and which already shows the
/// profile this dialog is about as unavailable.
fn profile_busy(parent: &gtk::Window, name: &str) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading(format!("Profile {name} is already running"))
        .body(
            "One profile can only be open in one window at a time. Roblox keeps its \
             session and its storage in there, and two clients writing to it at once \
             corrupts both.\n\nStart this one against a different profile, or close the \
             window that already has it.",
        )
        .build();
    dialog.add_response("close", "Close");
    dialog.add_response("profile", "Choose a Profile");
    dialog.set_response_appearance("profile", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("profile"));

    let parent = parent.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "profile" {
            // The avatar in the header bar is the shell's switcher and this is
            // the same door; activating the window's action rather than
            // building a second chooser here keeps one construction site.
            let _ = parent.activate_action("win.profile", None);
        }
    });
    dialog.present();
}

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
