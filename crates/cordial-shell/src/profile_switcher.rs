//! Choosing the profile the next instance runs.
//!
//! An `AdwAvatar` in the shell's header bar, opening a popover over
//! [`profile::list`]. ADR-012 makes a profile a directory and an instance a
//! window; this is where one is picked for the other.
//!
//! **It is in the shell because it cannot be in a client.** A running client
//! cannot change profile: `cordial_runtime::profile::set_active` refuses a
//! second, different directory outright — "a profile cannot be changed while
//! the client is up" — the `flock` is held for the lifetime of that process,
//! and the engine's storage root is resolved before the first frame. A
//! switcher in the engine's window would therefore be a control that cannot do
//! what it looks like it does, which is the interface version of the stub that
//! reports success AGENTS.md rules out. In the shell it decides what the *next*
//! launch runs, and that is a thing it can actually do. Running a second
//! profile alongside the first is the same gesture: pick another one and press
//! Roblox, which is all "two accounts at once" has ever been.
//!
//! There used to be a text entry for this in Settings and it is gone rather
//! than kept beside this. Two ways to set one value drift, and the one that
//! drifts is the one nobody is looking at.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use crate::settings::persist;
use crate::shell_config::ShellConfig;
use cordial_shell::profile;

/// Header-bar avatars are drawn at 24px across GNOME; matching it is what keeps
/// the header bar from growing a row taller than every other application's.
const HEADER_AVATAR: i32 = 24;

/// Whether a profile can be handed to a new instance.
///
/// Answered by taking ADR-012's claim and dropping it again, rather than by a
/// liveness check of this module's own. The `flock` is the only thing that
/// actually decides, so a second opinion — a PID file, a scan of `/proc` — could
/// disagree with it, and the disagreement the user would meet is the worst
/// direction: a row offered as free, pressed, and then refused by `try_launch`
/// a moment later for a reason the popover had just said did not apply.
///
/// The cost is honest and small. The probe really does hold each profile's lock
/// for as long as it takes to release it, so a launch racing the popover being
/// opened could be refused when it would otherwise have been allowed. That is
/// the same refusal a second launch produces, it names the profile, and pressing
/// again succeeds — which is a better failure than marks that are guesswork.
#[derive(Debug, PartialEq, Eq)]
pub enum Availability {
    Free,
    /// Held by another instance. Not a fault: it is the lock doing its job.
    Running,
    /// The directory is there and cannot be used — permissions, most likely.
    /// Kept apart from `Running` because the answer to it is completely
    /// different, and `profile::Error` already draws that line for the same
    /// reason.
    Unusable(String),
}

fn availability(name: &str) -> Availability {
    match profile::acquire(name) {
        Ok(claim) => {
            // Released immediately and explicitly. The launcher hands its claim
            // to the client it spawns; this one belongs to nobody, and holding
            // it a line longer than needed would mean the switcher itself was
            // the instance keeping a profile busy.
            drop(claim);
            Availability::Free
        }
        Err(profile::Error::Busy(_)) => Availability::Running,
        Err(profile::Error::Unusable(message)) => Availability::Unusable(message),
    }
}

/// What the create dialog makes of what has been typed so far.
#[derive(Debug, PartialEq, Eq)]
pub enum NameCheck {
    /// Nothing typed yet. No complaint to make, and nothing to create either.
    Empty,
    /// [`profile::dir`]'s own refusal, verbatim.
    ///
    /// Quoted rather than reworded so that the sentence a user meets when they
    /// type a slash here is the same sentence they would meet anywhere else the
    /// name is resolved. Refused rather than sanitised, which is `profile`'s
    /// decision and not this module's: silently rewriting a name would mean the
    /// profile someone asked for is not the one they get.
    Invalid(String),
    /// A profile by that name is already there, so "create" is really "switch
    /// to". Said out loud rather than left to look like a no-op.
    Existing,
    New,
}

pub fn check_name(name: &str, existing: &[String]) -> NameCheck {
    if name.is_empty() {
        return NameCheck::Empty;
    }
    if let Err(message) = profile::dir(name) {
        return NameCheck::Invalid(message);
    }
    if existing.iter().any(|e| e == name) {
        NameCheck::Existing
    } else {
        NameCheck::New
    }
}

/// The names the popover offers.
///
/// The profiles on disk, plus the chosen one when it has never been launched and
/// so has no directory yet. Without that second half the current profile would
/// be the one entry missing from a list of profiles, on exactly the fresh
/// install where it is the only one there is.
fn offered(current: &str, mut existing: Vec<String>) -> Vec<String> {
    if !existing.iter().any(|e| e == current) {
        existing.push(current.to_string());
        existing.sort();
    }
    existing
}

/// The config, where it is saved, and the header-bar widgets that have to show
/// the answer. Cloned into every popover row, which is why the button is held
/// weakly: the button owns the popover, the popover owns the row, and a strong
/// reference back would be a cycle that GTK's refcounting cannot break — one
/// leaked popover per time the avatar is pressed.
#[derive(Clone)]
struct Switcher {
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
    avatar: adw::Avatar,
    button: glib::WeakRef<gtk::MenuButton>,
}

impl Switcher {
    fn current(&self) -> String {
        self.config.borrow().profile.clone()
    }

    /// Persisted, not only shown. `ShellConfig.profile` is what `try_launch`
    /// reads, so a choice this window forgets is a choice that never happened.
    fn choose(&self, name: &str) {
        self.config.borrow_mut().profile = name.to_string();
        persist(&self.config, &self.config_path);
        self.show(name);
    }

    fn show(&self, name: &str) {
        self.avatar.set_text(Some(name));
        if let Some(button) = self.button.upgrade() {
            // The avatar draws initials and nothing else, so the whole of the
            // name has to be somewhere a pointer or a screen reader can reach
            // it. ADR-012 is deliberate that this selects a directory and knows
            // nothing about accounts, and the wording follows it.
            button.set_tooltip_text(Some(&format!("Profile: {name}")));
        }
    }
}

/// The header-bar control.
///
/// `set_create_popup_func` rather than a popover built once: whether a profile
/// is running changes underneath this window every time a client starts or
/// exits, and a list assembled at startup would be confidently wrong by the
/// second launch.
pub fn build(config: Rc<RefCell<ShellConfig>>, config_path: Rc<PathBuf>) -> gtk::MenuButton {
    let avatar = adw::Avatar::new(HEADER_AVATAR, Some(&config.borrow().profile), true);
    let button = gtk::MenuButton::builder().child(&avatar).build();

    let switcher =
        Switcher { config, config_path, avatar, button: button.downgrade() };
    switcher.show(&switcher.current());

    button.set_create_popup_func({
        let switcher = switcher.clone();
        move |button| button.set_popover(Some(&popover(&switcher)))
    });

    // TEMPORARY: screenshot harness. Removed before this change is finished.
    if let Ok(demo) = std::env::var("CORDIAL_SHELL_DEMO") {
        let s = switcher.clone();
        let b = button.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || match demo.as_str()
        {
            "popover" => b.popup(),
            "create" => {
                if let Some(window) = b.root().and_downcast::<gtk::Window>() {
                    create(&window, &s);
                }
            }
            _ => {}
        });
    }

    button
}

fn popover(switcher: &Switcher) -> gtk::Popover {
    let current = switcher.current();
    let list = gtk::Box::new(gtk::Orientation::Vertical, 3);
    list.set_size_request(240, -1);

    let heading = gtk::Label::new(Some("Profile"));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    heading.set_margin_start(6);
    heading.set_margin_bottom(3);
    list.append(&heading);

    let existing = profile::list();
    for name in offered(&current, existing.clone()) {
        // Only what is on disk can be held by anything, and probing a name that
        // is not there would create the directory as a side effect of opening a
        // menu.
        let state = if existing.iter().any(|e| *e == name) {
            availability(&name)
        } else {
            Availability::Free
        };
        list.append(&row(switcher, &name, name == current, state));
    }

    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.set_margin_top(3);
    separator.set_margin_bottom(3);
    list.append(&separator);

    let content = adw::ButtonContent::builder().icon_name("list-add-symbolic").label("New profile…").build();
    content.set_halign(gtk::Align::Start);
    let new = gtk::Button::builder().child(&content).css_classes(["flat"]).build();
    {
        let switcher = switcher.clone();
        new.connect_clicked(move |button| {
            let window = button.root().and_downcast::<gtk::Window>();
            popdown(&switcher);
            if let Some(window) = window {
                create(&window, &switcher);
            }
        });
    }
    list.append(&new);

    // Bounded rather than left to grow: nothing stops someone having twenty
    // profiles, and a popover taller than the monitor is clipped by the
    // compositor rather than scrolled by GTK.
    let scroller = gtk::ScrolledWindow::builder()
        .child(&list)
        .propagate_natural_height(true)
        .propagate_natural_width(true)
        .max_content_height(360)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();

    gtk::Popover::builder().child(&scroller).build()
}

fn row(switcher: &Switcher, name: &str, current: bool, state: Availability) -> gtk::Button {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.append(&adw::Avatar::new(HEADER_AVATAR, Some(name), true));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
    labels.set_hexpand(true);
    labels.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(Some(name));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    labels.append(&title);

    let note = match &state {
        Availability::Free => None,
        Availability::Running => Some("Open in another window".to_string()),
        Availability::Unusable(message) => Some(message.clone()),
    };
    if let Some(note) = &note {
        let subtitle = gtk::Label::new(Some(note));
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.add_css_class("caption");
        subtitle.add_css_class("dim-label");
        labels.append(&subtitle);
    }
    line.append(&labels);

    if current {
        line.append(&gtk::Image::from_icon_name("object-select-symbolic"));
    }

    let button = gtk::Button::builder().child(&line).css_classes(["flat"]).build();

    // Shown and refused here rather than offered and refused at launch. ADR-012's
    // lock is what decides either way; the difference is whether the user finds
    // out before or after pressing Roblox.
    if !matches!(state, Availability::Free) {
        button.set_sensitive(false);
        if let Some(note) = note {
            button.set_tooltip_text(Some(&note));
        }
    }

    let switcher = switcher.clone();
    let name = name.to_string();
    button.connect_clicked(move |_| {
        switcher.choose(&name);
        popdown(&switcher);
    });
    button
}

fn popdown(switcher: &Switcher) {
    if let Some(button) = switcher.button.upgrade() {
        button.popdown();
    }
}

/// Make a profile, or say why not.
///
/// Creation goes through [`profile::acquire`] — the same door a launch uses —
/// so a name that cannot be made into a usable directory fails here with the
/// message it would have failed with there, and the directory comes out `0700`
/// because that is where the mode is applied. The claim is dropped at once; this
/// window is not an instance.
fn create(parent: &gtk::Window, switcher: &Switcher) {
    let entry = adw::EntryRow::builder().title("Name").build();
    let group = adw::PreferencesGroup::new();
    group.add(&entry);

    let hint = gtk::Label::new(None);
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("caption");
    hint.add_css_class("dim-label");

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.append(&group);
    body.append(&hint);

    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading("New profile")
        .body(
            "A profile is one account's Roblox storage: its own session, settings, flag \
             overrides and plugin grants. Creating one does not sign you in — Cordial \
             selects a directory and never sees a password (ADR-012).",
        )
        .extra_child(&body)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");
    // Nothing typed is nothing to create. The button follows what has been
    // typed rather than accepting it and reporting afterwards, because a
    // refusal after the dialog has closed is a refusal with nowhere to correct
    // it.
    dialog.set_response_enabled("create", false);

    {
        let dialog = dialog.clone();
        let hint = hint.clone();
        entry.connect_changed(move |entry| {
            let name = entry.text().to_string();
            match check_name(&name, &profile::list()) {
                NameCheck::Empty => {
                    entry.remove_css_class("error");
                    hint.set_text("");
                    dialog.set_response_enabled("create", false);
                }
                NameCheck::Invalid(message) => {
                    entry.add_css_class("error");
                    hint.set_text(&message);
                    dialog.set_response_enabled("create", false);
                }
                NameCheck::Existing => {
                    entry.remove_css_class("error");
                    hint.set_text("That profile already exists; this will switch to it.");
                    dialog.set_response_enabled("create", true);
                }
                NameCheck::New => {
                    entry.remove_css_class("error");
                    hint.set_text("");
                    dialog.set_response_enabled("create", true);
                }
            }
        });
    }

    let switcher = switcher.clone();
    let parent = parent.clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "create" {
            return;
        }
        let name = typed_name(dialog);
        match profile::acquire(&name) {
            Ok(claim) => {
                drop(claim);
                switcher.choose(&name);
            }
            // Both remaining cases already have a sentence written for them on
            // `profile::Error`, and this is not the place to write a second one.
            Err(e) => crate::window::alert(&parent, "Cordial could not open that profile", &e.to_string()),
        }
    });

    dialog.present();
}

/// Read the name back out of the dialog's own widget tree.
///
/// The entry is reachable from the dialog rather than captured, so that the
/// value acted on is the one the dialog is showing at the moment Create is
/// pressed — a captured clone and a validated string are two things that can
/// disagree, and the one that would win is the one nobody validated.
fn typed_name(dialog: &adw::MessageDialog) -> String {
    dialog
        .extra_child()
        .and_downcast::<gtk::Box>()
        .and_then(|body| body.first_child())
        .and_downcast::<adw::PreferencesGroup>()
        .and_then(|group| find_entry_row(group.upcast()))
        .map(|row| row.text().to_string())
        .unwrap_or_default()
}

/// `AdwPreferencesGroup` wraps its rows in boxes and a list box of its own, so
/// the entry is several levels down and the depth is libadwaita's business
/// rather than something to hard-code.
fn find_entry_row(widget: gtk::Widget) -> Option<adw::EntryRow> {
    if let Ok(row) = widget.clone().downcast::<adw::EntryRow>() {
        return Some(row);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_entry_row(current.clone()) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `CORDIAL_PROFILE_ROOT` is process-wide and cargo runs a test binary's
    /// tests in parallel threads, so two tests pointing it at different scratch
    /// directories interleave and read each other's. Same guard, and the same
    /// reason, as `cordial_shell::profile`'s own tests.
    static ENV: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let p = std::env::temp_dir().join(format!("cordial-switcher-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        std::env::set_var("CORDIAL_PROFILE_ROOT", &p);
        (p, guard)
    }

    #[test]
    fn a_running_profile_is_shown_as_unavailable_rather_than_offered() {
        // The whole reason the probe is `acquire` and not a check of this
        // module's own: what the popover says has to be what the launch will
        // do, and only the lock knows that.
        let (_root, _g) = scratch("running");
        let held = profile::acquire("main").expect("a fresh profile is free");
        assert_eq!(availability("main"), Availability::Running);
        drop(held);
        assert_eq!(availability("main"), Availability::Free);
    }

    #[test]
    fn probing_does_not_leave_the_profile_held() {
        // The hazard in answering the question by taking the lock. If the probe
        // kept it, opening the menu would make every profile in it unlaunchable
        // — the switcher would be the instance holding them.
        let (_root, _g) = scratch("release");
        assert_eq!(availability("main"), Availability::Free);
        profile::acquire("main").expect("the probe must have let go again");
    }

    #[test]
    fn an_impossible_name_is_refused_in_profiles_own_words() {
        // Not "matches an error", but "is the same sentence". A second wording
        // for the same refusal is how a user ends up believing there are two
        // different rules.
        let (_root, _g) = scratch("names");
        let expected = profile::dir("has/slash").unwrap_err();
        assert_eq!(check_name("has/slash", &[]), NameCheck::Invalid(expected));
        assert_eq!(check_name("../escape", &[]), NameCheck::Invalid(profile::dir("../escape").unwrap_err()));
    }

    #[test]
    fn nothing_typed_is_not_an_error_and_is_not_creatable_either() {
        let (_root, _g) = scratch("empty");
        assert_eq!(check_name("", &[]), NameCheck::Empty);
    }

    #[test]
    fn a_name_that_already_exists_is_a_switch_rather_than_a_create() {
        let (_root, _g) = scratch("existing");
        let existing = vec!["default".to_string()];
        assert_eq!(check_name("default", &existing), NameCheck::Existing);
        assert_eq!(check_name("alt_account-2", &existing), NameCheck::New);
    }

    #[test]
    fn the_chosen_profile_is_offered_even_before_it_has_a_directory() {
        // The fresh-install case. `ShellConfig` defaults to `default` and
        // nothing has created it yet, so `profile::list` is empty and a popover
        // built from that alone would offer no profiles at all while claiming
        // one was selected.
        assert_eq!(offered("default", Vec::new()), vec!["default".to_string()]);
        assert_eq!(
            offered("default", vec!["alt".to_string()]),
            vec!["alt".to_string(), "default".to_string()]
        );
        // And it must not appear twice once it does exist.
        assert_eq!(offered("default", vec!["default".to_string()]), vec!["default".to_string()]);
    }
}
