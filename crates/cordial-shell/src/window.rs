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
    toasts.set_child(Some(&clamp));

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
            config.clone(),
            config_path.clone(),
            flags_path.clone(),
        )
        .present();
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
        Outcome::ProfileBusy(name, holder) => profile_busy(window, toasts, config, &name, holder),
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
        Err(profile::Error::Busy(name, holder)) => return Outcome::ProfileBusy(name, holder),
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
    dialog.connect_response(None, move |_, response| match response {
        // The switcher is the combo row above the launch button; activating the
        // window's action rather than building a second chooser here keeps one
        // construction site.
        "profile" => {
            let _ = parent_window.activate_action("win.profile", None);
        }
        "stop" => {
            if let Some(h) = ours.clone() {
                close_then_launch(parent_window.clone(), toasts.clone(), config.clone(), h);
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
    holder: profile::Holder,
) {
    if let Err(message) = holder.ask_to_stop() {
        alert(&window, "Could not close the other client", &message);
        return;
    }
    let waited = Cell::new(Duration::ZERO);
    glib::timeout_add_local(STOP_POLL, move || {
        if holder.has_exited() {
            activate_roblox(&window, &toasts, &config);
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
