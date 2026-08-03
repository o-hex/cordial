//! The Roblox-build button in the header bar, and the settings that govern it.
//!
//! `docs/design/updating-roblox.md` specifies a button whose icon says which of
//! three states it is in, and all three are built here. What is **not** built,
//! because it does not exist to build, is the fetch behind them: Roblox
//! publishes no Android download. `client-version/AndroidApp` answers HTTP 500
//! while `WindowsPlayer` answers 200, `setup.rbxcdn.com/android/DeployHistory.txt`
//! is 403, and `roblox.com/download` offers Google Play and the Amazon Appstore
//! and no file. [`cordial_update::download::Source::official`] is a refusal for
//! that reason, and ADR-015 accepted it.
//!
//! So every control here stops short of claiming a download. The Update button
//! exists, in the state that earns it, and what it opens says where the build
//! comes from — because a control that looked live and could never fire would be
//! AGENTS.md's stub-that-lies wearing a widget: something proceeds on an answer
//! that is not true, except here the something is a person. This project has
//! already shipped a settings page describing plugins nobody had installed,
//! twice.
//!
//! ## One question, one button, and no second APK picker
//!
//! The window behind the header-bar button offered the APK picker as well, and
//! that picker is gone rather than kept in two places. `profile_switcher.rs`
//! states the rule it broke: *two ways to set one value drift, and the one that
//! drifts is the one nobody is looking at.* Choosing a build is configuration
//! and lives on the Roblox page in Settings beside the engine directory it has
//! to agree with; this window answers the one question a header-bar button can
//! answer on its own — which build am I on, and has Roblox published a newer
//! engine — and offers the single control that state earns.
//!
//! The settings themselves moved too, off the Roblox page and onto one of their
//! own. That page was answering "where is the build" and "when does the build
//! change" at once, which is six lines of description and a warning row on top
//! of the two rows somebody opened it for.
//!
//! ## How "there is an update" is established without a version endpoint
//!
//! [`cordial_update::changelog`] is the half that works: Roblox's release notes
//! are titled `Release Notes for NNN`, and `NNN` is the engine major — the `732`
//! in `0.732.23.7321040` and the `Version=732` the client logs about itself. So
//! the newest release-notes major is the newest engine Roblox has shipped, and
//! comparing it against the version of the build *here* is a real comparison.
//!
//! The catch is the other operand. Cordial knows the installed version only for
//! a build it fetched itself ([`cordial_update::cache::recorded_version`]), and
//! it has fetched none — an APK somebody obtained elsewhere carries no version
//! this can read without parsing Android's binary manifest. So the ordinary
//! state is **not knowing**, and the button says exactly that rather than
//! rounding it to "up to date". Telling somebody they are current while Roblox
//! refuses their build server-side is the failure this whole feature exists to
//! prevent.
//!
//! ## Always present, immediately left of Settings
//!
//! The design's argument, unchanged and for its original reason: a control that
//! comes and goes is hard to find at the moment you want it, and it moves
//! everything beside it when it arrives. The version-and-changelog view is
//! useful on its own, which is why the nothing-newer state is not a dead end.
//!
//! The check runs on a thread of its own, started once the window is up. That is
//! the one hard requirement in the design rather than a preference: `http`'s
//! timeout is twenty seconds, and twenty seconds of a frozen launcher is what a
//! call on the GTK thread buys.

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use cordial_update::cache;
use cordial_update::changelog::{self, Notes, Release};
use cordial_update::download::{Refusal, Source, URL_ENV};
use cordial_update::metered::{self, Metered};
use cordial_update::settings::{Automatic, DownloadOn, Plan, UpdateSettings, NEVER_DOWNLOADS};
use cordial_update::version;
use cordial_update::Unreachable;

use crate::install;
use crate::settings::persist;
use crate::shell_config::ShellConfig;

/// The resting icon: a build is a package, and most of the time there is
/// nothing waiting.
///
/// Worn whenever no update is known — which is Manual until somebody presses
/// the button, every mode between launch and the answer arriving, and the
/// ordinary steady state in all three. `system-software-install-symbolic`
/// before, which is the same idea in the legacy icon set.
const PACKAGE_ICON: &str = "package-x-generic-symbolic";

/// The icon that *means* an update is waiting, worn only when one actually is.
///
/// This is the distinction the earlier version of this comment was reaching for
/// and then applied to the wrong state: an arrow-in-a-star permanently on
/// screen is the attention state drawn rather than written, but on a build that
/// really has been superseded it is exactly the right icon. Swapping between
/// the two is what makes the header bar answer "is there anything new" without
/// being opened.
///
/// `suggested-action` still rides alongside it, so the difference is carried by
/// colour as well as shape.
const UPDATE_ICON: &str = "software-update-available-symbolic";

/// How long after the window is up before the check starts.
///
/// The design is explicit that the check must not be what `activate` waits on,
/// and a thread already satisfies that. This exists for the smaller reason: the
/// first paint and a TLS handshake competing for the same few milliseconds is a
/// launcher that appears fractionally later for nothing gained.
const AFTER_WINDOW: Duration = Duration::from_millis(250);

/// How often the main loop looks for the check having finished.
///
/// A `std::sync::mpsc` receiver polled from a timeout rather than anything
/// asynchronous: this crate has no async runtime and deliberately does not gain
/// one for a single request, and a GTK widget is not `Send`, so the answer has
/// to be collected on the thread that owns the widgets whatever carries it.
const POLL: Duration = Duration::from_millis(100);

/// What one check found out.
#[derive(Debug, Clone)]
pub struct Checked {
    metered: Metered,
    /// The version of the build installed here, when Cordial has any business
    /// claiming to know it. `None` is the ordinary case.
    installed: Option<String>,
    release: Result<(Release, Notes), Unreachable>,
}

impl Checked {
    /// The newest engine major Roblox has published, if the DevForum answered.
    fn latest_major(&self) -> Option<u32> {
        self.release.as_ref().ok().map(|(r, _)| r.major)
    }

    /// Whether a newer build is *established*, not assumed.
    ///
    /// Both operands or nothing. An unknown installed version is not an old one,
    /// and a DevForum that did not answer is not a Roblox that published
    /// nothing.
    fn update_available(&self) -> bool {
        match (self.installed.as_deref().and_then(version::major_of), self.latest_major()) {
            (Some(here), Some(latest)) => here < latest,
            _ => false,
        }
    }
}

/// The header-bar button, wired to whatever the settings say about checking.
///
/// It takes no config path, unlike everything else in the shell that opens a
/// window: nothing this button leads to writes a setting any more. The APK
/// picker it used to carry is on the Roblox page in Settings and nowhere else.
pub fn header_button(
    parent: &impl IsA<gtk::Window>,
    config: Rc<RefCell<ShellConfig>>,
) -> gtk::Button {
    let parent = parent.as_ref().clone();
    let button = gtk::Button::from_icon_name(PACKAGE_ICON);
    let last: Rc<RefCell<Option<Checked>>> = Rc::new(RefCell::new(None));

    let automatic = config.borrow().automatic_updates;
    dress(&button, &last.borrow(), automatic);

    // Manual does no background request of any kind — that is the whole of what
    // the setting says — so nothing is started here and the button is a refresh
    // control until somebody presses it.
    if automatic != Automatic::Manual {
        let button = button.clone();
        let last = last.clone();
        let parent = parent.clone();
        let config = config.clone();
        glib::timeout_add_local_once(AFTER_WINDOW, move || {
            let apk = install::effective_apk(&config.borrow().roblox).map(|(p, _)| p);
            check(apk, move |checked| {
                let available = checked.update_available();
                // The plan is asked for even though only one branch of it can be
                // acted on today, because the settings are what decide it and a
                // launch that never consults them is a settings page governing
                // nothing at all. What it governs right now is which line gets
                // printed, and that line is the truthful one either way.
                let plan = update_settings(&config.borrow()).plan(checked.metered);
                *last.borrow_mut() = Some(checked);
                dress(&button, &last.borrow(), automatic);
                if !available {
                    return;
                }
                match plan {
                    // Background would fetch here, and cannot: there is no
                    // source to fetch from. The refusal is what says so, rather
                    // than a silent no-op that looks like a download nobody can
                    // find.
                    Plan::CheckAndDownload => match Source::configured() {
                        Ok(source) => println!(
                            "[update] a newer Roblox build has been published, and {} is set to \
                             {} — the shell does not stream a file yet",
                            URL_ENV, source.url
                        ),
                        Err(refusal) => {
                            println!("[update] a newer Roblox build has been published, and {refusal}")
                        }
                    },
                    Plan::CheckAndAsk { why } => println!(
                        "[update] a newer Roblox build has been published; not downloading: {}",
                        why.unwrap_or_else(|| "Auto update is Ask".into())
                    ),
                    Plan::DoNotCheck => {}
                }
                // Ask means somebody is asked. A badge is what the other modes
                // leave behind; this one opens the changelog with an Update
                // button on it, on launch, and that dialog is the whole
                // difference between the two settings.
                if automatic == Automatic::Ask {
                    present(&parent, config.clone(), last.clone(), button.clone());
                }
            });
        });
    }

    {
        let last = last.clone();
        let parent = parent.clone();
        let config = config.clone();
        let button_for_click = button.clone();
        button.connect_clicked(move |_| {
            present(&parent, config.clone(), last.clone(), button_for_click.clone());
        });
    }

    button
}

/// Put the button in the state its knowledge earns.
///
/// Called after every check rather than once, because the manual mode's button
/// genuinely changes shape when a check lands: the design says a check that
/// finds an update turns the refresh control into the update control, and a
/// button that kept its arrow would be hiding the answer it just fetched.
/// Which icon, tooltip and attention state the knowledge earns.
///
/// Split out of [`dress`] because it is the part worth pinning and a
/// `gtk::Button` cannot be constructed without `gtk::init`, so a test that
/// reaches for one panics in the widget constructor rather than testing
/// anything. Same arrangement, and the same reason, as `window::busy_body`.
///
/// Two icons and one rule: the arrow means an update is waiting, and nothing
/// else does. Manual is deliberately not a case — a mode you cannot see from
/// the header bar has no business changing what the header bar looks like, and
/// Manual with nothing found is simply "no update known", the same as every
/// other mode before its check lands. Press the button, let a check find
/// something, and Manual gets the arrow like the rest. The refresh arrow this
/// used to wear in Manual is gone for exactly that reason: it made one button
/// two shapes depending on a setting elsewhere.
fn dressing(last: &Option<Checked>, automatic: Automatic) -> (&'static str, &'static str, bool) {
    match last {
        Some(checked) if checked.update_available() => {
            (UPDATE_ICON, "Roblox has published a newer build", true)
        }
        Some(_) => (PACKAGE_ICON, "Roblox build", false),
        None if automatic == Automatic::Manual => {
            (PACKAGE_ICON, "Check for a Roblox update", false)
        }
        None => (PACKAGE_ICON, "Roblox build", false),
    }
}

fn dress(button: &gtk::Button, last: &Option<Checked>, automatic: Automatic) {
    let (icon, tooltip, attention) = dressing(last, automatic);
    button.set_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    if attention {
        button.add_css_class("suggested-action");
    } else {
        button.remove_css_class("suggested-action");
    }
}

/// Ask the network and the cache, off the GTK thread, and hand the answer back
/// on it.
fn check(apk: Option<PathBuf>, then: impl Fn(Checked) + 'static) {
    // `apk` is taken here rather than read on the worker so that nothing off the
    // main thread touches config the main thread owns.
    let _ = apk;
    on_worker(
        || {
            let metered = metered::current();
            let installed = cache::recorded_version(&install::engine_cache());
            let release = changelog::latest().and_then(|r| changelog::notes(&r).map(|n| (r, n)));
            Checked { metered, installed, release }
        },
        then,
    )
}

/// The plumbing, with the work as an argument.
///
/// Split from [`check`] so that the hand-back can be tested without a network:
/// this is a worker thread, a channel and a main-loop source, and every one of
/// those three has a way of silently never delivering. A test that drives the
/// main context and asserts the answer arrives is the only thing that tells the
/// difference between "the request is slow" and "the answer had nowhere to go",
/// and the first version of this shipped looking exactly like the second.
///
/// Generic over what comes back because there are two of these now: the
/// changelog request, and the NetworkManager property the Updates page shows.
/// `metered::query` is a blocking D-Bus call with zbus's own timeout in front of
/// it, so building a settings page must not be what waits on it.
fn on_worker<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    then: impl Fn(T) + 'static,
) {
    let (tx, rx) = mpsc::channel::<T>();
    std::thread::spawn(move || {
        // The window may already be closed; nobody is owed this answer.
        let _ = tx.send(work());
    });

    glib::timeout_add_local(POLL, move || match rx.try_recv() {
        Ok(checked) => {
            then(checked);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        // Only reachable if the worker panicked. Left as a Break rather than
        // spinning forever on a channel nothing will ever send to.
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

/// Both switches and the mode, as `cordial_update` wants them.
pub fn update_settings(config: &ShellConfig) -> UpdateSettings {
    UpdateSettings { automatic: config.automatic_updates, download_on: config.download_on }
}

/// The window behind the button: which build is here, what Roblox has published,
/// and one control.
///
/// One window for all three states, with the top group saying which. Three
/// windows would be three places for the same sentence about where a build comes
/// from, and they would drift.
///
/// **Three groups and no fourth.** This had the APK picker, the extracted-engine
/// cache row, the download source, the connection NetworkManager reported and
/// the update settings on it as well, which made a header-bar button open a
/// second settings window. The settings are a tab in the real one now, the
/// picker is on the Roblox page, and what is left is the question the button can
/// answer: which build am I on, and is there a newer engine.
pub fn present(
    parent: &gtk::Window,
    config: Rc<RefCell<ShellConfig>>,
    last: Rc<RefCell<Option<Checked>>>,
    button: gtk::Button,
) {
    let automatic = config.borrow().automatic_updates;

    // Two things in this window: the changelog, and the one control that acts
    // on it. It used to open with three `PreferencesGroup`s — a status row, the
    // installed build, then the notes last and below the fold. The changelog is
    // the only reason anyone opens this, and it was the thing you had to scroll
    // past two paragraphs of caveat to reach.
    //
    // The caveats did not become untrue, so they are not gone: the one that
    // matters is a dim line above the button, and the rest is its tooltip. What
    // went is the framing that gave a permanently-unavailable version number
    // the same weight as the text people came to read.
    let notes_title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-1"])
        .label("Fetching…")
        .build();

    // Selectable so a line can be quoted into an issue without a screenshot,
    // and the full text rather than `summarise`'s eight lines: this pane has
    // the whole window to fill and a scrollbar for the rest.
    let notes_body = gtk::Label::builder()
        .xalign(0.0)
        .yalign(0.0)
        .wrap(true)
        .selectable(true)
        .label("Roblox's release notes, from the DevForum.")
        .build();

    let notes_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    notes_box.set_margin_top(24);
    notes_box.set_margin_bottom(12);
    notes_box.set_margin_start(24);
    notes_box.set_margin_end(24);
    notes_box.append(&notes_title);
    notes_box.append(&notes_body);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&notes_box)
        .build();

    // The DevForum link, out of the notes row and into the header bar. It is a
    // second way to read what is already on screen, which does not deserve to
    // sit beside the button that does something.
    let open = gtk::LinkButton::with_label("https://devforum.roblox.com/c/updates/release-notes", "Read");
    open.set_visible(false);

    // One button, relabelled by state rather than two swapped by visibility. A
    // manual check has to be able to turn this window from one state into the
    // other while it is open — the design's "the refresh button becomes the
    // update button" — and a single button that changes its word is that
    // sentence taken literally.
    //
    // It is not labelled `Install`, at any size. There is nothing here to
    // install — Roblox publishes the Android build through Google Play and the
    // Amazon Appstore and no file — so a full-width Install button would be the
    // interface version of the stub that reports success. See `UPDATE`.
    let action = gtk::Button::with_label(CHECK);
    action.add_css_class("pill");
    action.set_hexpand(true);
    action.set_height_request(48);

    // The caveat that used to be a whole group, reduced to its first line. It
    // cannot be dropped: without it a window showing only a changelog and a
    // button implies "up to date", which is the one thing this check is unable
    // to establish for the Android build.
    let status_line = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build();

    let footer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    footer.set_margin_top(12);
    footer.set_margin_bottom(24);
    footer.set_margin_start(24);
    footer.set_margin_end(24);
    footer.append(&status_line);
    footer.append(&action);

    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Roblox Build")
        .default_width(660)
        // 720 while there were six groups here, then 660 for three. Now there is
        // one pane and one button, and the height is the changelog's: this is
        // how much release note is worth having on screen before a scrollbar.
        .default_height(660)
        .build();

    // Everything the answer touches, in one closure, so a check started from
    // this dialog and one started at launch put the window into the same state.
    let paint = {
        let status_line = status_line.clone();
        let notes_title = notes_title.clone();
        let notes_body = notes_body.clone();
        let open = open.clone();
        let action = action.clone();
        let button = button.clone();
        Rc::new(move |checked: &Option<Checked>| {
            // The headline of the caveat on screen, the whole of it on hover.
            // The long form names the endpoint that answers 500 and why "up to
            // date" would be a guess, which is worth keeping reachable and is
            // not worth four lines above a changelog.
            let (title, subtitle) = status_lines(checked, automatic);
            status_line.set_label(&title);
            let installed = match checked {
                Some(checked) => version_line(checked.installed.clone()),
                None => version_line(cache::recorded_version(&install::engine_cache())),
            };
            action.set_tooltip_text(Some(&format!("{subtitle}\n\n{installed}\n\n{INSTALLED_DESCRIPTION}")));
            let available = checked.as_ref().is_some_and(Checked::update_available);
            action.set_label(action_label(checked));
            action.set_sensitive(true);
            // The attention the header-bar button wears, on the control that
            // acts on it. Removed again in the other direction because a check
            // can turn this window back into the nothing-newer state while it
            // is open, and a suggested-action Check reads as an update waiting.
            if available {
                action.add_css_class("suggested-action");
            } else {
                action.remove_css_class("suggested-action");
            }

            match checked {
                Some(checked) => {
                    let (title, body) = release_lines_full(&checked.release);
                    notes_title.set_label(&title);
                    notes_body.set_label(&body);
                    if let Ok((release, _)) = &checked.release {
                        open.set_uri(&release.web_url());
                        open.set_visible(true);
                    }
                }
                None => {
                    notes_title.set_label("Fetching…");
                    notes_body.set_label("Roblox's release notes, from the DevForum.");
                    open.set_visible(false);
                }
            }
            dress(&button, checked, automatic);
        })
    };
    paint(&last.borrow());

    // Manual's whole promise: this checks once, now, and if it finds something
    // the button and this window both become the update one. The same closure
    // serves the Check Now button and the window opening with nothing known
    // yet, so a manual check and a first look cannot behave differently.
    let run_check: Rc<dyn Fn()> = {
        let last = last.clone();
        let paint = paint.clone();
        let config = config.clone();
        let status_line = status_line.clone();
        let action = action.clone();
        Rc::new(move || {
            action.set_sensitive(false);
            status_line.set_label("Checking…");
            let apk = install::effective_apk(&config.borrow().roblox).map(|(p, _)| p);
            let last = last.clone();
            let paint = paint.clone();
            check(apk, move |checked| {
                *last.borrow_mut() = Some(checked);
                paint(&last.borrow());
            });
        })
    };
    {
        // The whole of what the one button does, and which branch it takes is
        // the state rather than a mode the user has to have selected. Check
        // re-asks the DevForum; Update… opens what Cordial knows about getting
        // a build it cannot fetch.
        let run_check = run_check.clone();
        let last_for_click = last.clone();
        let window_for_update = window.clone();
        action.connect_clicked(move |_| {
            if last_for_click.borrow().as_ref().is_some_and(Checked::update_available) {
                update_dialog(&window_for_update);
            } else {
                run_check();
            }
        });
    }
    // Opening this window is a check in every mode, which is what makes the
    // refresh button in Manual do what its icon says. It costs a second request
    // if the launch check is still in flight, and that is one small GET against
    // a forum rather than a reason to build a way for two dialogs to share one
    // in-flight answer.
    if last.borrow().is_none() {
        run_check();
    }

    let header = adw::HeaderBar::new();
    header.pack_end(&open);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroller));
    // A bottom bar rather than the last child of the scrolled pane: the button
    // has to stay put while a long changelog moves under it, or the control
    // this window exists to offer scrolls off the bottom of it.
    toolbar.add_bottom_bar(&footer);
    window.set_content(Some(&toolbar));
    window.present();
}

/// What the one button says before a check, and once one has landed.
///
/// **`Update…` and not `Download`, when there is nothing to download.** Roblox
/// publishes the Android build through Google Play and the Amazon Appstore and
/// no file, and a button labelled Download that opens a panel saying so is the
/// interface version of the stub that reports success. The ellipsis is load
/// bearing rather than decoration: GNOME's convention is that `…` means *this
/// opens something rather than doing it*, which is exactly what this button
/// does in that state.
const UPDATE: &str = "Update…";

/// The label the moment a fetch is genuinely possible — see [`DOWNLOAD`]'s
/// condition in [`action_label`].
///
/// This is the state the `UPDATE` comment always described as the day the
/// ellipsis drops. It arrives when `CORDIAL_UPDATE_URL` and its hash are set,
/// because then [`Source::configured`] returns a source and the button really
/// would do the thing its label names.
const DOWNLOAD: &str = "Download";

/// The resting label, in every mode including Automatic. Roblox publishes no
/// Android version number this can compare against, so "nothing newer" is the
/// near-permanent answer and this is the near-permanent word on the button.
const CHECK: &str = "Check for Updates";

/// The word the button wears, decided by state rather than by the update mode.
///
/// **`Download` is gated on a source existing, not on an update existing.** The
/// two come apart in the ordinary case: Roblox can publish a newer engine that
/// Cordial has no way to fetch, and labelling that Download would promise a file
/// that never arrives.
fn action_label(checked: &Option<Checked>) -> &'static str {
    match checked {
        Some(checked) if checked.update_available() => {
            if Source::configured().is_ok() {
                DOWNLOAD
            } else {
                UPDATE
            }
        }
        _ => CHECK,
    }
}

/// What `Update…` opens, and the one thing it must not do is imply a fetch.
///
/// It reports what [`Source::configured`] decided — which today is a refusal
/// naming Google Play and the Amazon Appstore — and stops there. It used to
/// offer the APK picker as its suggested response, which made this the second
/// place a build could be chosen; that is now the Roblox page in Settings, and
/// the text says so rather than growing a shortcut back to it.
fn update_dialog(parent: &adw::Window) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading("Cordial cannot download this build")
        .body(update_body(&Source::configured()))
        .build();
    dialog.add_response("close", "Close");
    dialog.present();
}

/// "Updates" — a page of its own, and what it says about a download it cannot
/// perform.
///
/// It was a group on the Roblox page, which left that page answering two
/// questions at once: *where is the build* and *when does the build change*.
/// Between them they put six lines of description, a dropdown, two switches and
/// a conditional warning row on top of the two path rows somebody opens that
/// page for. Splitting them costs one more tab in a window that gets its tabs
/// from libadwaita for free.
///
/// `system-software-install-symbolic` rather than
/// `software-update-available-symbolic` for the same reason the header-bar
/// button wears it: the second icon *means* an update is waiting, and this page
/// exists whether or not one is.
pub fn build_update_page(
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Updates")
        // The name `open_on_start` addresses this page by. Without it
        // libadwaita answers `settings=updates` with "Child name 'updates' not
        // found in AdwViewStack" and shows the first page instead, which is a
        // screenshot captioned as something it is not.
        .name("updates")
        .icon_name("system-software-install-symbolic")
        .build();
    page.add(&build_update_group(config, config_path));
    page.add(&build_source_group());
    page
}

/// Where a newer build comes from, and what this connection counts as.
///
/// Both rows were on the header-bar button's window, which is not where they
/// belong: neither answers "which build am I on". They explain the two switches
/// directly above them — an update that does not download because the desktop
/// guesses metered is the confusion this row exists to pre-empt.
fn build_source_group() -> adw::PreferencesGroup {
    let group =
        adw::PreferencesGroup::builder().title("Getting a newer build").description(STORES).build();

    let source_row = adw::ActionRow::builder()
        .title("Download source")
        .subtitle(source_line(&Source::configured()))
        .build();
    source_row.set_subtitle_lines(8);
    group.add(&source_row);

    let connection_row =
        adw::ActionRow::builder().title("This connection").subtitle("Asking NetworkManager…").build();
    connection_row.set_subtitle_lines(4);
    group.add(&connection_row);

    // Off the GTK thread, because `metered::query` is a blocking D-Bus call with
    // zbus's own timeout in front of it. A settings window that opened after a
    // pause on a machine where NetworkManager is wedged would be this crate
    // repeating the mistake the changelog check is already written to avoid.
    {
        let connection_row = connection_row.clone();
        on_worker(metered::current, move |m| connection_row.set_subtitle(&connection_line(m)));
    }

    group
}

/// The dropdown and the two switches.
///
/// `build_appearance_page`'s shape for the dropdown, to the letter: a
/// `StringList` in the order the enum's `index` defines, `selected` from the
/// saved value, and one `connect_selected_notify` that writes the config and
/// persists it. The switches follow `build_performance_group`'s.
fn build_update_group(
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> adw::PreferencesGroup {
    // Not "Updates" — the page is called that now, and a group repeating its own
    // page's title is a heading that says nothing.
    let group = adw::PreferencesGroup::builder()
        .title("Automatic updates")
        .description(SETTINGS_DESCRIPTION)
        .build();

    // Order has to match Automatic::index/from_index; the labels come from the
    // enum so the wording lives in one file rather than two.
    let model = gtk::StringList::new(&[
        Automatic::Background.label(),
        Automatic::Ask.label(),
        Automatic::Manual.label(),
    ]);
    let automatic = adw::ComboRow::builder()
        .title("Auto update")
        .subtitle(AUTO_UPDATE_SUBTITLE)
        .model(&model)
        .selected(config.borrow().automatic_updates.index())
        .build();
    automatic.set_subtitle_lines(6);
    {
        let config = config.clone();
        let config_path = config_path.clone();
        automatic.connect_selected_notify(move |row| {
            config.borrow_mut().automatic_updates = Automatic::from_index(row.selected());
            persist(&config, &config_path);
        });
    }
    group.add(&automatic);

    let wifi = adw::SwitchRow::builder()
        .title("Download on Wi-Fi")
        .subtitle(
            "Cordial cannot see whether a link is wireless. It asks NetworkManager whether the \
             connection is metered, and this switch governs every connection that is not — a \
             wired desktop included.",
        )
        .active(config.borrow().download_on.wifi)
        .build();
    wifi.set_subtitle_lines(4);

    let metered_row = adw::SwitchRow::builder()
        .title("Download on metered connection")
        .subtitle(
            "Off by default. Only an explicit unmetered answer counts as unmetered: both of \
             NetworkManager's guesses are treated as metered, and an ordinary desktop on a LAN \
             guesses, so this switch is reached more often than it looks.",
        )
        .active(config.borrow().download_on.metered)
        .build();
    metered_row.set_subtitle_lines(4);

    // The contradiction the design document warned two dropdowns would prevent.
    // It is expressible now, so it is said out loud instead — the objection was
    // to a settings page that can express a contradiction *silently*, and a row
    // that appears exactly when both switches are off is the answer to it.
    let warning = adw::ActionRow::builder().title("Nothing will download").subtitle(NEVER_DOWNLOADS).build();
    warning.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    warning.add_css_class("warning");
    warning.set_subtitle_lines(5);
    warning.set_visible(config.borrow().download_on.never_downloads());

    let restate = {
        let warning = warning.clone();
        let wifi = wifi.clone();
        let metered_row = metered_row.clone();
        let config = config.clone();
        let config_path = config_path.clone();
        Rc::new(move || {
            let on = DownloadOn { wifi: wifi.is_active(), metered: metered_row.is_active() };
            config.borrow_mut().download_on = on;
            persist(&config, &config_path);
            warning.set_visible(on.never_downloads());
        })
    };
    {
        let restate = restate.clone();
        wifi.connect_active_notify(move |_| restate());
    }
    {
        let restate = restate.clone();
        metered_row.connect_active_notify(move |_| restate());
    }

    group.add(&wifi);
    group.add(&metered_row);
    group.add(&warning);
    group
}

/// What the dropdown means, in the three sentences the modes differ by.
const AUTO_UPDATE_SUBTITLE: &str =
    "Update in background checks and fetches. Ask checks, and opens the changelog when Cordial \
     starts with an update waiting. Manual makes no request until you press the button in the \
     header bar, which then checks once.";

/// Said on the settings group rather than left for somebody to discover.
///
/// A setting that cannot take effect has to say so. All three of these are real
/// and none of them causes a download today, because there is nothing to
/// download from. Leaving that out would put live-looking controls in front of
/// somebody and let them conclude their updates were handled.
pub const SETTINGS_DESCRIPTION: &str =
    "These govern checking for a new Roblox build and downloading it once there is a source to \
     download from. There is none today: Roblox distributes the Android build through Google \
     Play and the Amazon Appstore and publishes no file, so nothing here causes a download at \
     present. Checking still works — Roblox's release notes are published — and so does \
     choosing a build you obtained yourself.";

/// Where the build is obtainable, named plainly.
///
/// Somebody meeting a refusal needs to learn the file exists and where from. A
/// message that only says Cordial cannot reads as Cordial being broken, which is
/// exactly what ADR-015 says the refusal must not do.
const STORES: &str =
    "Roblox publishes no Android build outside app stores: the application comes from Google \
     Play and the Amazon Appstore, and Roblox's own deployment CDN carries the Windows and Mac \
     clients only. So there is a file to obtain, and Cordial is not the thing that can fetch it \
     for you — it will not sign in to a store on your behalf, and it will not take the file from \
     a mirror and call that Roblox. Obtain an APK and choose it on the Roblox page.";

/// Said on the installed-build group, where the picker used to be.
///
/// It names where the picker went. A group that lost its only control and says
/// nothing about it reads as a control that failed to appear.
const INSTALLED_DESCRIPTION: &str =
    "Cordial ships no Roblox code and never will, so this is a build you already have. The APK \
     is chosen on the Roblox page in Settings, which is the only place it is set.";

/// The top row: which of the three states this is, and why.
fn status_lines(checked: &Option<Checked>, automatic: Automatic) -> (String, String) {
    let Some(checked) = checked else {
        // Opening this window is itself a check, in every mode — including
        // Manual, whose whole promise is that the button checks once, now. So
        // there is no "not checked" state to show here; that state lives on the
        // button, as the circular arrow.
        return (
            "Checking…".to_string(),
            format!(
                "Asking the DevForum what Roblox has published. Auto update is {}; this request \
                 is one you asked for by opening this window.",
                automatic.label()
            ),
        );
    };

    if checked.update_available() {
        let here = checked.installed.clone().unwrap_or_default();
        let latest = checked.latest_major().unwrap_or_default();
        return (
            format!("Roblox has published engine {latest}"),
            format!(
                "The build here is {here}. Update opens what Cordial knows about getting a newer \
                 one; it cannot fetch this build itself, because Roblox publishes none to fetch."
            ),
        );
    }

    match (&checked.installed, checked.latest_major()) {
        // The honest ordinary case, and the one that must not be rounded to "up
        // to date": Roblox serves no version for the Android build, so there is
        // no second operand to compare against what is installed.
        (None, Some(latest)) => (
            "Whether this build is current cannot be established".to_string(),
            format!(
                "Roblox's newest release notes are for engine {latest}. Cordial cannot tell which \
                 engine the APK here is: {}/AndroidApp answers HTTP 500 while WindowsPlayer \
                 answers 200, and an APK you obtained yourself carries no version this can read. \
                 Saying \"up to date\" on that would be a guess.",
                version::ENDPOINT.trim_end_matches('/')
            ),
        ),
        (Some(here), Some(latest)) => (
            "Nothing newer".to_string(),
            format!("This build is engine {here}, and Roblox's newest release notes are for {latest}."),
        ),
        // The DevForum did not answer. Naming it beats a blank row, which reads
        // as Roblox having published nothing.
        (_, None) => (
            "Could not check".to_string(),
            match &checked.release {
                Err(why) => why.to_string(),
                Ok(_) => "The release notes carried no engine number.".to_string(),
            },
        ),
    }
}

/// What the Update button's dialog says.
fn update_body(configured: &Result<Source, Refusal>) -> String {
    match configured {
        Ok(source) => format!(
            "{URL_ENV} points Cordial at {}.\n\nDownloading from this window is not built: \
             nothing in the shell streams a file yet, and pretending otherwise with a progress \
             bar that never fills would be worse than saying so. Fetch it yourself and choose \
             it on the Roblox page in Settings.",
            source.url
        ),
        Err(refusal) => format!("{refusal}\n\n{STORES}"),
    }
}

/// Which Roblox version this is, when that is knowable at all.
///
/// It is not, in the ordinary case, and the row says why rather than showing
/// nothing. An APK somebody obtained themselves carries no version Cordial can
/// read without parsing Android's binary manifest, and a guessed number in front
/// of a user is a number nothing established.
fn version_line(recorded: Option<String>) -> String {
    match recorded {
        Some(version) => format!("Roblox {version}, recorded when Cordial fetched this build."),
        None => "Not known. Cordial only records a version for a build it fetched itself, and it \
                 has fetched none — an APK you obtained elsewhere carries no version this can \
                 read, and it will not guess one."
            .to_string(),
    }
}

/// What the extracted engine is, and whether it still matches the APK above.
///
/// The stamp is shown verbatim because it is the thing that decides: `install`
/// re-extracts when it stops matching, and somebody looking at a Cordial that
/// re-extracts 115 MB every launch has no other way to see what it is comparing.
///
/// Said on the Roblox page's engine-directory row now rather than in the
/// header-bar button's window. It followed the engine directory, which is what
/// it is about; the alternative on offer was deleting it with the row it used to
/// sit in, and this is the only place the re-extraction is visible at all.
pub(crate) fn cache_line(engine: bool, stamp: Option<String>, current: bool) -> String {
    if !engine {
        return "None yet. Cordial takes lib/x86_64/libroblox.so out of the APK the first time \
                you launch, into its own cache."
            .to_string();
    }
    match stamp {
        Some(stamp) if current => format!("Extracted from the APK above.\n{stamp}"),
        // The case the stamp exists for: a new build at the same path used to
        // leave the old engine in place, and Cordial ran it against the new
        // APK's assets.
        Some(stamp) => {
            format!("Extracted from a different build, so the next launch extracts again.\n{stamp}")
        }
        None => "An engine is cached and nothing records which APK it came from, so the next \
                 launch extracts again."
            .to_string(),
    }
}

/// What NetworkManager said, and what the switches make of it.
///
/// Surfaced rather than left to be discovered when a download silently does not
/// happen. An ordinary desktop on a LAN answers `GUESS_NO`, which is a guess and
/// therefore metered, and somebody told only "metered" answers "no it isn't".
fn connection_line(metered: Metered) -> String {
    let verdict = if metered.is_metered() {
        "Treated as metered, so Download on metered connection is the switch that governs it. \
         Only an explicit unmetered answer takes the other branch, because reading a guess the \
         cheap way is how a data allowance pays for 115 MB nobody asked for."
    } else {
        "Treated as unmetered, so Download on Wi-Fi is the switch that governs it."
    };
    format!("{}.\n{verdict}", metered.describe())
}

/// What the download-source row says.
///
/// [`Source::configured`] is the one place that decides, so this asks it rather
/// than restating its conclusion — including the two half-configured cases,
/// where a URL without a hash is refused as a URL being trusted.
fn source_line(configured: &Result<Source, Refusal>) -> String {
    match configured {
        Ok(source) => format!(
            "{URL_ENV} points Cordial at a build you chose:\n{}\nThis window does not fetch it; \
             it shows what is installed and where a build comes from.",
            source.url
        ),
        Err(refusal) => refusal.to_string(),
    }
}

/// The release-notes row's title and subtitle.
/// The same heading, and the release notes **whole**.
///
/// [`release_lines`] exists for anywhere the notes have to fit in a row; this
/// window is a pane with a scrollbar, and truncating a changelog someone opened
/// a window to read is the shape of bug that makes people go to the DevForum
/// instead. The two share `heading` so they cannot title the same release
/// differently.
fn release_lines_full(release: &Result<(Release, Notes), Unreachable>) -> (String, String) {
    match release {
        Ok((release, notes)) => (heading(release, notes), notes.text().trim().to_string()),
        Err(why) => ("Could not fetch the release notes".to_string(), why.to_string()),
    }
}

fn heading(release: &Release, notes: &Notes) -> String {
    let when = release.created_at.split('T').next().unwrap_or_default();
    if when.is_empty() {
        notes.title.clone()
    } else {
        format!("{} — {when}", notes.title)
    }
}

// `release_lines` and `summarise` lived here, clipping the notes to eight lines
// and 600 characters because they had to fit in an `AdwActionRow`. Nothing has
// to fit in a row any more — the notes are the window — so they went rather than
// stay as a second, shorter truth about the same release that no caller wanted.

#[cfg(test)]
mod tests {
    use super::*;
    use cordial_update::Sha256Hash;

    fn release(major: u32) -> Release {
        Release {
            major,
            title: format!("Release Notes for {major}"),
            id: 4763851,
            slug: format!("release-notes-for-{major}"),
            created_at: "2026-07-29T18:44:52.923Z".into(),
        }
    }

    fn notes(major: u32) -> Notes {
        Notes {
            title: format!("Release Notes for {major}"),
            created_at: "2026-07-29T18:44:52.923Z".into(),
            html: "<p>Hi all,<br>\nPleased to announce that it has landed.</p>".into(),
        }
    }

    fn checked(installed: Option<&str>, latest: Option<u32>) -> Checked {
        Checked {
            metered: Metered::GuessNo,
            installed: installed.map(str::to_string),
            release: match latest {
                Some(major) => Ok((release(major), notes(major))),
                None => Err(Unreachable::Transport {
                    url: changelog::CATEGORY.into(),
                    why: "no route to host".into(),
                }),
            },
        }
    }

    #[test]
    fn an_unknown_installed_version_is_not_an_old_one_nor_a_current_one() {
        // The ordinary state, and the one everything here turns on. Roblox
        // serves no version for the Android build, so there is nothing to
        // compare — and both roundings are wrong: claiming an update invents
        // one, and claiming "up to date" tells somebody they are current while
        // Roblox may be refusing their build server-side.
        let c = checked(None, Some(732));
        assert!(!c.update_available());
        let (title, body) = status_lines(&Some(c), Automatic::Background);
        assert!(title.contains("cannot be established"), "{title}");
        assert!(body.contains("HTTP 500"), "{body}");
        assert!(!body.contains("up to date") || body.contains("would be a guess"), "{body}");
    }

    #[test]
    fn an_older_installed_engine_than_the_release_notes_is_an_update() {
        // The one honest route to the attention state: the changelog half works,
        // and an installed version Cordial recorded itself is a real operand.
        let c = checked(Some("0.700.1.7001000"), Some(732));
        assert!(c.update_available());
        let (title, body) = status_lines(&Some(c), Automatic::Ask);
        assert!(title.contains("732"), "{title}");
        assert!(body.contains("0.700.1.7001000"), "{body}");
        // And it still refuses to imply a fetch it cannot perform.
        assert!(body.contains("cannot fetch this build itself"), "{body}");
    }

    #[test]
    fn a_current_engine_is_reported_as_nothing_newer() {
        let c = checked(Some("0.732.23.7321040"), Some(732));
        assert!(!c.update_available());
        assert_eq!(status_lines(&Some(c), Automatic::Background).0, "Nothing newer");
    }

    #[test]
    fn a_devforum_that_did_not_answer_is_not_read_as_nothing_published() {
        // Silence here would read as Roblox having published nothing, which is
        // the shape ADR-015 rules out for the version check and is no better for
        // the half that does work.
        let c = checked(Some("0.700.1.7001000"), None);
        assert!(!c.update_available(), "an unreachable forum is not evidence of anything");
        let (title, body) = status_lines(&Some(c), Automatic::Background);
        assert_eq!(title, "Could not check");
        assert!(body.contains("devforum.roblox.com"), "{body}");
    }

    #[test]
    fn opening_the_window_is_itself_the_check_manual_mode_promises() {
        // Turning off automatic checking is a statement about background network
        // use, not a refusal to ever know. So the window opened from the refresh
        // button is already asking, and says which mode it is asking under
        // rather than reporting that nothing has happened.
        let (title, body) = status_lines(&None, Automatic::Manual);
        assert_eq!(title, "Checking…");
        assert!(body.contains("Manual"), "{body}");
        assert!(body.contains("one you asked for"), "{body}");
    }

    #[test]
    fn the_settings_description_says_the_settings_cannot_act_yet() {
        // The requirement that makes this page honest rather than decorative.
        // Controls that look live and govern nothing are the interface version
        // of a stub returning success.
        assert!(SETTINGS_DESCRIPTION.contains("There is none today"), "{SETTINGS_DESCRIPTION}");
        assert!(SETTINGS_DESCRIPTION.contains("Google Play"), "{SETTINGS_DESCRIPTION}");
        assert!(SETTINGS_DESCRIPTION.contains("Amazon Appstore"), "{SETTINGS_DESCRIPTION}");
        // And it does not overstate the outage either: checking genuinely works.
        assert!(SETTINGS_DESCRIPTION.contains("Checking still works"), "{SETTINGS_DESCRIPTION}");
    }

    #[test]
    fn the_dropdown_subtitle_distinguishes_the_three_modes() {
        // Ask and Update in background differ by one behaviour — the dialog on
        // launch — and a dropdown whose options are three words is where that
        // difference goes missing.
        assert!(AUTO_UPDATE_SUBTITLE.contains("opens the changelog"), "{AUTO_UPDATE_SUBTITLE}");
        assert!(AUTO_UPDATE_SUBTITLE.contains("checks once"), "{AUTO_UPDATE_SUBTITLE}");
    }

    #[test]
    fn the_update_button_names_the_stores_rather_than_implying_a_fetch() {
        // The Update button is the one control here that could be read as "and
        // now it downloads". What it opens has to be the honest answer, which is
        // where the build actually comes from.
        let body = update_body(&Source::configured());
        assert!(body.contains("Google Play"), "{body}");
        assert!(body.contains("Amazon Appstore"), "{body}");
        // And it points at the one place a build is chosen, rather than
        // offering a second picker of its own.
        assert!(body.contains("Roblox page"), "{body}");

        let hash = Sha256Hash::parse(&format!("sha256:{}", "ab".repeat(32))).unwrap();
        let with_source = update_body(&Source::new("https://example.invalid/base.apk", hash));
        assert!(with_source.contains("not built"), "{with_source}");
        assert!(with_source.contains("Roblox page in Settings"), "{with_source}");
    }

    #[test]
    fn the_button_says_download_only_when_there_is_something_to_download() {
        // The label is the promise. With no source configured nothing can be
        // downloaded — Roblox publishes no Android artefact — so a button that
        // said Download would be AGENTS.md's stub-that-reports-success with a
        // widget around it, and the panel behind it saying "get this from Google
        // Play" would be the failure arriving somewhere unrelated to the cause.
        //
        // This test runs with no `CORDIAL_UPDATE_URL` in the environment, which
        // is the state every user is in until they deliberately leave it, so the
        // update case here is the refusal case.
        assert_eq!(action_label(&None), CHECK);
        assert_eq!(action_label(&Some(checked(None, Some(732)))), CHECK);
        assert_eq!(action_label(&Some(checked(Some("0.732.23.7321040"), Some(732)))), CHECK);

        let available = checked(Some("0.700.1.7001000"), Some(732));
        assert!(available.update_available());
        assert!(Source::configured().is_err(), "a source leaked into the test environment");
        assert_eq!(action_label(&Some(available)), UPDATE);

        // The word only becomes Download alongside a source, never alongside a
        // mere update. Those two come apart in the ordinary case and that is the
        // whole point of the gate.
        assert!(!CHECK.contains("Download"), "{CHECK}");
        assert!(!UPDATE.contains("Download"), "{UPDATE}");
        // The ellipsis is the GNOME convention for "opens something rather than
        // doing it", and it is the difference between this button and a lie.
        assert!(UPDATE.ends_with('…'), "{UPDATE}");
        assert!(!DOWNLOAD.ends_with('…'), "{DOWNLOAD}");
    }

    #[test]
    fn the_header_icon_is_the_arrow_only_while_an_update_is_actually_waiting() {
        // The arrow-in-a-star means "an update is waiting" to every GNOME user,
        // so wearing it in the resting state would be the attention state drawn
        // rather than written. Manual is deliberately not a case here: a mode
        // you cannot see from the header bar must not change what it looks like.
        assert_ne!(PACKAGE_ICON, UPDATE_ICON);
        for automatic in [Automatic::Background, Automatic::Ask, Automatic::Manual] {
            let (icon, _, attention) = dressing(&None, automatic);
            assert_eq!(icon, PACKAGE_ICON, "{automatic:?} resting");
            assert!(!attention, "{automatic:?} resting");

            let (icon, _, attention) = dressing(&Some(checked(Some("0.732.23.7321040"), Some(732))), automatic);
            assert_eq!(icon, PACKAGE_ICON, "{automatic:?} nothing newer");
            assert!(!attention, "{automatic:?} nothing newer");

            let (icon, _, attention) = dressing(&Some(checked(Some("0.700.1.7001000"), Some(732))), automatic);
            assert_eq!(icon, UPDATE_ICON, "{automatic:?} update waiting");
            assert!(attention, "{automatic:?} update waiting");
        }
    }

    #[test]
    fn the_installed_group_says_where_the_apk_is_chosen_now_that_it_is_not_chosen_here() {
        // The picker was here and in Settings at once. `profile_switcher.rs`
        // wrote down what that costs: two ways to set one value drift, and the
        // one that drifts is the one nobody is looking at. What is left has to
        // say where the other one is, or a group with no control in it reads as
        // a control that failed to appear.
        assert!(INSTALLED_DESCRIPTION.contains("Roblox page in Settings"), "{INSTALLED_DESCRIPTION}");
        assert!(INSTALLED_DESCRIPTION.contains("only place"), "{INSTALLED_DESCRIPTION}");
    }

    #[test]
    fn the_refusal_names_both_stores_rather_than_only_refusing() {
        assert!(STORES.contains("Google Play"), "{STORES}");
        assert!(STORES.contains("Amazon Appstore"), "{STORES}");
        assert!(STORES.contains("mirror"), "{STORES}");
    }

    #[test]
    fn an_unknown_roblox_version_says_why_rather_than_guessing() {
        let line = version_line(None);
        assert!(line.contains("Not known"), "{line}");
        assert!(line.contains("will not guess"), "{line}");
        assert!(version_line(Some("0.732.23.7321040".into())).contains("0.732.23.7321040"));
    }

    #[test]
    fn a_cache_from_another_build_says_it_will_be_extracted_again() {
        // The defect the stamp exists for, in the one place a user can see it:
        // a new build at the same path used to leave the old engine in place.
        let stale = cache_line(true, Some("10 1754000000 /a/base.apk".into()), false);
        assert!(stale.contains("different build"), "{stale}");
        assert!(stale.contains("/a/base.apk"), "the stamp is what decides, so it is shown");

        let fresh = cache_line(true, Some("10 1754000000 /a/base.apk".into()), true);
        assert!(fresh.contains("Extracted from the APK above"), "{fresh}");

        assert!(cache_line(false, None, false).contains("None yet"));
        assert!(cache_line(true, None, false).contains("nothing records"));
    }

    #[test]
    fn the_ordinary_desktops_guess_is_shown_as_metered_and_explained() {
        // The surprise this row exists for. This machine answers GUESS_NO on an
        // ordinary LAN, which the rules treat as metered, and somebody who is
        // only told "metered" will disagree with it.
        let line = connection_line(Metered::GuessNo);
        assert!(line.contains("guess"), "{line}");
        assert!(line.contains("Treated as metered"), "{line}");
        assert!(line.contains("Download on metered connection"), "{line}");

        let plain = connection_line(Metered::No);
        assert!(plain.contains("Treated as unmetered"), "{plain}");
        assert!(plain.contains("Download on Wi-Fi"), "{plain}");
    }

    #[test]
    fn the_source_row_reports_the_refusal_it_was_given() {
        // Not a second copy of the reasoning: whatever `Source::configured`
        // decides is what the row says, including the half-configured cases.
        let refused = source_line(&Source::configured());
        assert!(refused.contains("Google Play"), "{refused}");

        let hash = Sha256Hash::parse(&format!("sha256:{}", "ab".repeat(32))).unwrap();
        let configured = source_line(&Source::new("https://example.invalid/base.apk", hash));
        assert!(configured.contains("https://example.invalid/base.apk"), "{configured}");
        assert!(configured.contains("does not fetch"), "{configured}");
    }

    #[test]
    fn a_fetched_changelog_shows_its_title_date_and_opening() {
        let (title, body) = release_lines_full(&Ok((release(732), notes(732))));
        assert_eq!(title, "Release Notes for 732 — 2026-07-29");
        assert!(body.starts_with("Hi all,"), "{body}");
    }

    #[test]
    fn a_long_set_of_notes_arrives_whole_now_that_it_has_a_window() {
        // The inverse of the test that used to be here. Roblox's release notes
        // run to thousands of words and this used to clip them to eight lines,
        // because they had to fit in a row. They are the window now, so the
        // thing worth pinning is that nothing is dropped: a changelog someone
        // opened a window to read, truncated, is the bug that sends them to the
        // DevForum instead.
        let long: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let mut release = notes(732);
        release.html = long.clone();
        let (_, body) = release_lines_full(&Ok((self::release(732), release)));
        assert!(body.contains("line 199"), "the last line was dropped");
        assert!(!body.ends_with('…'), "{body}");
    }

    #[test]
    fn an_answer_from_the_worker_thread_reaches_the_main_loop() {
        // Three things between the request and the row — a thread, a channel and
        // a main-loop source — and each can silently never deliver. The first
        // version of this window sat on "Checking…" indefinitely, and from the
        // outside that is indistinguishable from a slow CDN, which is why this
        // is a test rather than another launch with a stopwatch.
        use std::cell::Cell;
        use std::time::Instant;

        let context = glib::MainContext::default();
        let _guard = context.acquire().expect("the test thread can own the default context");
        let landed = Rc::new(Cell::new(false));
        on_worker(
            || checked(Some("0.700.1.7001000"), Some(732)),
            {
                let landed = landed.clone();
                move |checked: Checked| {
                    assert!(checked.update_available());
                    landed.set(true);
                }
            },
        );

        let started = Instant::now();
        while !landed.get() && started.elapsed() < Duration::from_secs(5) {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(landed.get(), "the answer never reached the main loop");
    }

    #[test]
    fn the_shell_config_becomes_the_pair_the_update_crate_plans_from() {
        // The seam between the three rows and `UpdateSettings::plan`. If these
        // stopped agreeing, the settings page would be governing nothing and
        // there would be nothing on screen to say so.
        use cordial_update::settings::Plan;
        let config = ShellConfig {
            automatic_updates: Automatic::Background,
            download_on: DownloadOn { wifi: false, metered: false },
            ..Default::default()
        };
        match update_settings(&config).plan(Metered::No) {
            Plan::CheckAndAsk { why: Some(why) } => assert_eq!(why, NEVER_DOWNLOADS),
            other => panic!("both switches off must not plan a download: {other:?}"),
        }
    }
}
