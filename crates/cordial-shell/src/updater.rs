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
use cordial_update::changelog::{self, Entry, Notes, Release};
use cordial_update::download::{Refusal, Source, URL_ENV};
use cordial_update::metered::{self, Metered};
use cordial_update::settings::{Automatic, DownloadOn, Plan, UpdateSettings};
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
    /// The newest build a source can actually supply **for this
    /// architecture**, which is not the same as the newest Roblox announced.
    ///
    /// **This is the difference between an honest badge and one that never goes
    /// out.** Measured on 2026-08-26: Roblox announced 735 and published
    /// 2.735.1138 as two XAPK bundles carrying `config.armeabi_v7a.apk` and
    /// `config.arm64_v8a.apk` and no `config.x86_64.apk` in either. So on this
    /// machine the newest runnable build was 2.734.917, and a comparison
    /// against the DevForum's number said "update available" for a build that
    /// could never be installed -- for ever, and with the accent to match.
    obtainable: Option<String>,
    /// What actually changed, from the Creator Hub table.
    ///
    /// Separate from `release` and allowed to fail on its own: the DevForum
    /// listing is what establishes the version number, and losing the changelog
    /// must not take the version comparison down with it. `None` here means the
    /// docs page did not answer, and the window falls back to the announcement
    /// post rather than showing nothing.
    entries: Option<Vec<Entry>>,
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
    /// Re-read the installed version from disk.
    ///
    /// **Called after an install, so the button stops asking for attention it
    /// has already been given.** `installed` is a snapshot taken when the check
    /// ran, and installing a build makes it stale immediately -- so the badge
    /// stayed lit after a download that had just resolved the thing it was
    /// pointing at, which reads as the update having failed.
    ///
    /// Re-read rather than assigned from the version the install reported,
    /// because `update_available` compares this against the DevForum's number
    /// and both operands have to come from the sources the original comparison
    /// used. A string from somewhere else that happens to look similar is how
    /// two version formats end up compared and neither is wrong-looking enough
    /// to notice.
    fn reread_installed(&mut self) {
        self.installed = cache::recorded_version(&install::engine_cache());
    }

    fn update_available(&self) -> bool {
        // **Against what can be installed, not against what was announced.**
        // See `obtainable`. Falling back to the announced major when no source
        // answered would put the old behaviour back on exactly the runs where
        // it is least defensible -- a network that failed is not evidence that
        // a newer build exists for this architecture.
        match (self.installed.as_deref(), self.obtainable.as_deref()) {
            (Some(here), Some(there)) => version::is_newer(there, here),
            _ => false,
        }
    }

    /// Roblox announced a build newer than anything installable here.
    ///
    /// Worth saying out loud rather than hiding: somebody who reads the
    /// release notes for 735 and is running 734 deserves to know why, and
    /// "your architecture has no 735 build" is a better answer than a button
    /// that does nothing.
    fn newer_announced_than_obtainable(&self) -> bool {
        match (self.obtainable.as_deref().and_then(version::major_of), self.latest_major()) {
            (Some(have), Some(announced)) => have < announced,
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
                    // **Background now downloads, which is what it says.** It
                    // used to print that there was no source to fetch from and
                    // stop, which was true when it was written and stopped
                    // being true with ADR-025 -- leaving the shipped default
                    // setting describing a behaviour the code did not have.
                    //
                    // Silent by design: no window, no dialog, no toast. The
                    // user chose "in the background" and the badge is how they
                    // find out, which is the whole difference between this
                    // mode and Ask. A failure prints and changes nothing --
                    // there is no user watching to tell, and the build they
                    // have is untouched.
                    //
                    // `metered` has already been consulted: `plan` is what
                    // turns Background into CheckAndAsk on a connection
                    // somebody pays for by the megabyte, so reaching this arm
                    // means the download is permitted.
                    Plan::CheckAndDownload => {
                        let button = button.clone();
                        let last = last.clone();
                        on_worker(
                            || {
                                let cancel = cordial_update::provider::Cancel::new();
                                cordial_update::provider::obtain_and_install(
                                    None,
                                    cordial_update::provider::Want::Newest,
                                    &cancel,
                                    &mut |_| {},
                                )
                                .map(|(got, _)| got.version.name)
                                .map_err(|e| e.to_string())
                            },
                            move |outcome| match outcome {
                                Ok(version) => {
                                    println!("[update] installed Roblox {version} in the background");
                                    // The badge has to go out, for the same
                                    // reason it does after the button: nothing
                                    // else re-runs the comparison, and a badge
                                    // pointing at an update already installed
                                    // is worse than no badge.
                                    if let Some(checked) = last.borrow_mut().as_mut() {
                                        checked.reread_installed();
                                    }
                                    dress(&button, &last.borrow(), automatic);
                                }
                                Err(why) => println!(
                                    "[update] a background update did not install: {why}"
                                ),
                            },
                        );
                    }
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

/// What the banner says, or nothing at all.
///
/// Split out for the same reason as [`dressing`]: it is the part worth pinning
/// in a test, and a `gtk::Banner` cannot be built without `gtk::init`.
///
/// Order matters. An update waiting outranks a standing setting -- somebody
/// with automatic updates off who has just been told a build is available does
/// not also need telling why nobody told them sooner.
fn banner_text(last: &Option<Checked>, automatic: Automatic) -> Option<(String, bool)> {
    if let Some(checked) = last {
        if checked.update_available() {
            return Some(("A newer Roblox build is available.".to_string(), false));
        }
        // Roblox published something this architecture has no build of. Said
        // plainly, because the alternative is a user reading release notes for
        // a version they cannot have and concluding Cordial is broken.
        if checked.newer_announced_than_obtainable() {
            if let Some(latest) = checked.latest_major() {
                return Some((
                    format!(
                        "Roblox has published {latest}, but there is no build of it for this \
                         computer's architecture yet. You are on the newest one there is."
                    ),
                    false,
                ));
            }
        }
    }
    if automatic == Automatic::Manual {
        return Some((
            "Automatic update checks are off. Cordial will not look for new Roblox builds \
             on its own."
                .to_string(),
            true,
        ));
    }
    None
}

fn dress_banner(banner: &adw::Banner, last: &Option<Checked>, automatic: Automatic) {
    match banner_text(last, automatic) {
        Some((text, actionable)) => {
            banner.set_title(&text);
            // A button only where there is somewhere to send them. When an
            // update is waiting the window already has a Download button below,
            // and a banner button restating it is clutter; when checks are off
            // the fix is in Settings, and a banner that names a problem without
            // a way to it is the half-pattern this file has been fixing all
            // day.
            banner.set_button_label(if actionable { Some("Settings") } else { None });
            banner.set_revealed(true);
        }
        None => banner.set_revealed(false),
    }
}

fn dress(button: &gtk::Button, last: &Option<Checked>, automatic: Automatic) {
    let (icon, tooltip, _attention) = dressing(last, automatic);
    button.set_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    // **No accent, deliberately.** `.suggested-action` is the interface
    // guidelines' style for the primary affirmative action -- the confirm
    // button in a dialog -- and not a way of indicating state. An
    // accent-filled icon button in a header bar is not a pattern the platform
    // uses for "something is available", and it read as one thing the
    // maintainer noticed before anybody else: why is the update button blue.
    //
    // The icon still changes, which is the quiet half of the signal, and the
    // loud half is now an `AdwBanner` -- which is what GNOME's own software
    // centres use to say exactly this.
    button.remove_css_class("suggested-action");
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
            // Two requests rather than one, on the same worker: the DevForum
            // gives the number, the Creator Hub gives the list. A failure here
            // is recorded as `None` and nothing else changes, because the
            // version comparison does not depend on it.
            let entries = release
                .as_ref()
                .ok()
                .and_then(|(r, _)| match changelog::docs_notes(r.major) {
                    Ok(entries) => Some(entries),
                    Err(why) => {
                        println!("[update] the release-notes table did not answer: {why}");
                        None
                    }
                });
            // One more small request, and it is what makes the answer true.
            // The version list is metadata-sized -- about 100 kB -- against the
            // two this already makes, and `metered` governs the download rather
            // than the check, which this file has always said.
            let obtainable = cordial_update::provider::newest_obtainable();
            Checked { metered, installed, obtainable, release, entries }
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
/// [`on_worker`], for work that has something to say while it runs.
///
/// The long fetch needed this and the plain version could not give it: it
/// delivers one value at the end, and a 229 MB download that says nothing for
/// four minutes is indistinguishable from a hung window. Two channels rather
/// than one, drained by the same timer, so progress and the result cannot
/// arrive out of order.
///
/// `report` is handed to the worker as a plain closure, so the code doing the
/// work does not have to know it is being watched.
///
/// Generic over what it reports rather than over `String`. It was `String`, and
/// that meant every caller formatted a sentence on the worker thread before
/// sending it -- so a widget that wanted the numbers rather than the words had
/// to parse them back out of prose. `provider::Progress` travels intact and the
/// widget decides what to say.
pub(crate) fn on_worker_reporting<T: Send + 'static, P: Send + 'static>(
    work: impl FnOnce(&dyn Fn(P)) -> T + Send + 'static,
    progress: impl Fn(P) + 'static,
    then: impl FnOnce(T) + 'static,
) {
    let (ptx, prx) = mpsc::channel::<P>();
    let (tx, rx) = mpsc::channel::<T>();
    std::thread::spawn(move || {
        let value = work(&|update| {
            // Nobody is owed this if the window has gone.
            let _ = ptx.send(update);
        });
        let _ = tx.send(value);
    });

    let mut then = Some(then);
    glib::timeout_add_local(POLL, move || {
        // Progress first and all of it, so the last line before a result is
        // the one shown rather than one the result raced past.
        while let Ok(update) = prx.try_recv() {
            progress(update);
        }
        match rx.try_recv() {
            Ok(value) => {
                if let Some(then) = then.take() {
                    then(value);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

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
thread_local! {
    /// The update window, if one is open.
    ///
    /// **Two callers can want this window at the same time and neither knows
    /// about the other.** The header button opens it on click, and the
    /// launch-time check opens it by itself when the mode is Ask and it finds
    /// something -- so pressing the button while that check is still in flight
    /// gave you a second window on top of the first, reported on 2026-08-28 as
    /// "you get another update window this time because you got an available
    /// update". Both callers were right to ask; nothing was deciding that one
    /// window is enough.
    ///
    /// Weak, so the window closing is what ends its own entry rather than a
    /// close handler that has to be kept in step with every way a window can go
    /// away. A dead weak reference and no reference at all mean the same thing
    /// here, which is the property that makes this safe to get wrong.
    static OPEN: RefCell<Option<glib::WeakRef<adw::Window>>> = const { RefCell::new(None) };
}

/// Raise the update window if it is already open.
///
/// Returns whether it did. Presenting an already-present `adw::Window` brings it
/// forward, which is the behaviour somebody pressing the button a second time is
/// asking for -- they want to see it, and they have no way of knowing a check
/// opened it a moment ago.
fn raise_if_open() -> bool {
    OPEN.with(|open| {
        let Some(existing) = open.borrow().as_ref().and_then(glib::WeakRef::upgrade) else {
            return false;
        };
        existing.present();
        true
    })
}

pub fn present(
    parent: &gtk::Window,
    config: Rc<RefCell<ShellConfig>>,
    last: Rc<RefCell<Option<Checked>>>,
    button: gtk::Button,
) {
    if raise_if_open() {
        return;
    }
    let automatic = config.borrow().automatic_updates;
    // The header-bar button, kept under a name of its own because `button` is
    // shadowed further down by this window's own action button and the two
    // being confused is how the badge would get cleared on the wrong widget.
    let button_for_badge = button.clone();
    let last_for_badge = last.clone();

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
    // `title-2` rather than `title-1`: at this window's width a full release
    // heading on one line is most of the card, and the larger size wrapped it
    // across two before the notes had said anything.
    let notes_title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-2"])
        .label("Fetching…")
        .build();

    // Selectable so a line can be quoted into an issue without a screenshot,
    // and the full text rather than the eight lines a row used to allow.
    // `use_markup` because `changelog::render_entries` returns Pango markup —
    // bold headings, `<tt>` for the API names Roblox writes in code spans. It
    // escapes everything it interpolates; see that function.
    let notes_body = gtk::Label::builder()
        .xalign(0.0)
        .yalign(0.0)
        .wrap(true)
        .selectable(true)
        .use_markup(true)
        .label("Roblox's release notes, from the DevForum.")
        .build();

    // A selectable `GtkLabel` takes focus when the window opens and selects its
    // whole contents, so the changelog arrived highlighted end to end and had to
    // be clicked off before it could be read. Selection is worth keeping — a
    // line goes into an issue without a screenshot — so what moves is the focus:
    // the button gets it, and the selection is cleared each time the text is
    // replaced, since setting a label's text re-selects it all over again.
    notes_body.set_can_focus(false);

    // The notes sit in a `card`, which is libadwaita's own container for "this
    // is one document" — the same shape a system updater puts its changelog in.
    // Loose text on the window background left the release notes and the status
    // caption looking like two halves of one column, when one is what Roblox
    // published and the other is what Cordial can tell you about it.
    //
    // `card` draws the background and the rounded corner and no padding, so the
    // inset is this inner box's margins.
    let notes_inner = gtk::Box::new(gtk::Orientation::Vertical, 12);
    notes_inner.set_margin_top(16);
    notes_inner.set_margin_bottom(16);
    notes_inner.set_margin_start(16);
    notes_inner.set_margin_end(16);
    notes_inner.append(&notes_title);
    notes_inner.append(&notes_body);

    let notes_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    notes_card.add_css_class("card");
    notes_card.append(&notes_inner);

    let notes_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    notes_box.set_margin_top(18);
    notes_box.set_margin_bottom(6);
    notes_box.set_margin_start(18);
    notes_box.set_margin_end(18);
    notes_box.append(&notes_card);

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
    footer.set_margin_bottom(18);
    // Level with the card above it: a full-width button inset differently from
    // the thing it sits under reads as belonging to a different window.
    footer.set_margin_start(18);
    footer.set_margin_end(18);
    let meter = crate::download_progress::Meter::new();
    footer.append(&status_line);
    footer.append(meter.widget());
    footer.append(&action);

    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Roblox Build")
        // 660 while this was three `PreferencesGroup`s that needed the room for
        // their rows. A changelog is a column of prose, and 660 stretched its
        // lines well past a comfortable measure while leaving the full-width
        // button looking like a stretched bar with a word in the middle. This is
        // roughly the same measure a system updater gives the same content.
        .default_width(460)
        // The height stayed: this is how much release note is worth having on
        // screen before the scrollbar takes over.
        .default_height(660)
        .build();

    // **A banner rather than an accent-filled header button.** The interface
    // guidelines reserve `.suggested-action` for the primary affirmative
    // action -- the confirm in a dialog -- not for indicating state, and an
    // accented icon button in a header bar is not a pattern the platform uses
    // for "something is available". `AdwBanner` is: a full-width strip with a
    // sentence and one button, which is what GNOME's own software centres use
    // to say this.
    //
    // Two things can want saying, and they are different in kind, so the
    // banner says whichever matters more:
    //
    //   - an update is waiting, which is news;
    //   - automatic updates are off, which is a standing condition and the
    //     reason no news will arrive by itself.
    //
    // The second is deliberately not an error. Manual is a legitimate choice
    // and a banner that nags about it would be the application arguing with a
    // setting the user made.
    let banner = adw::Banner::new("");
    banner.set_revealed(false);
    // The button goes to the setting rather than flipping it from here. This
    // window has no config path to persist through, and reaching for one would
    // mean threading it through two call sites so a banner could write a value
    // the Updates page already owns -- ADR-020's rule about one writer, applied
    // to Cordial's own settings for the same reason.
    {
        let banner_for_click = banner.clone();
        banner.connect_button_clicked(move |_| {
            let _ = banner_for_click.activate_action("win.settings", Some(&"updates".to_variant()));
        });
    }
    // Everything the answer touches, in one closure, so a check started from
    // this dialog and one started at launch put the window into the same state.
    let paint = {
        let status_line = status_line.clone();
        let notes_title = notes_title.clone();
        let notes_body = notes_body.clone();
        let open = open.clone();
        let action = action.clone();
        let button = button.clone();
        let banner_for_paint = banner.clone();
        Rc::new(move |checked: &Option<Checked>| {
            // **Which build is here, not a verdict on it.** This line used to
            // carry the outcome — "Up to date", or the longer "Whether this
            // build is current cannot be established" — and a verdict is the one
            // thing it is not entitled to give: Roblox serves no version for the
            // Android build, so there is nothing to compare against. Saying so
            // in four words above a changelog reads as a complaint, and saying
            // "Up to date" instead would be a guess.
            //
            // So the line answers the question it can: which build you have. The
            // reasoning behind the missing verdict is still one hover away on
            // the button, where somebody who wants it will look for it.
            dress_banner(&banner_for_paint, checked, automatic);
            let (_, subtitle) = status_lines(checked, automatic);
            let installed = match checked {
                Some(checked) => version_line(checked.installed.clone()),
                None => version_line(cache::recorded_version(&install::engine_cache())),
            };
            status_line.set_label(&installed);
            action.set_tooltip_text(Some(&format!("{subtitle}\n\n{INSTALLED_DESCRIPTION}")));
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
                    let (title, body) = release_lines_full(&checked.release, checked.entries.as_deref());
                    notes_title.set_label(&title);
                    notes_body.set_markup(&body);
                    // Setting the text of a selectable label selects all of it.
                    notes_body.select_region(0, 0);
                    if let Ok((release, _)) = &checked.release {
                        open.set_uri(&release.web_url());
                        open.set_visible(true);
                    }
                }
                None => {
                    notes_title.set_label("Fetching…");
                    notes_body.set_text("Roblox's release notes, from the DevForum.");
                    notes_body.select_region(0, 0);
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
        let action = action.clone();
        Rc::new(move || {
            // The progress goes on the control that started it, rather than on
            // a line above it that then has to be put back. One thing changes,
            // and it is the thing you just pressed.
            action.set_sensitive(false);
            action.set_label(CHECKING);
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
        // re-asks the DevForum; Download fetches the build.
        //
        // **The download happens here rather than in a dialog this opens.**
        // There was a second dialog, and before that a third, both of them
        // explaining that Cordial could not fetch a build. Once it could, a
        // dialog whose only content is an apology for a missing feature had
        // nothing left to say, and stacking a working button behind a window
        // that has to be opened first is two clicks for one action. This is
        // the window somebody opened to see what changed; the button under
        // the changelog is where they will press to get it.
        let run_check = run_check.clone();
        let last_for_click = last.clone();
        let status_line = status_line.clone();
        let action_for_click = action.clone();
        let meter = meter.clone();
        action.connect_clicked(move |_| {
            if !last_for_click.borrow().as_ref().is_some_and(Checked::update_available) {
                run_check();
                return;
            }

            // **Refused before a byte moves, not after.** `effective_apk`
            // prefers a chosen APK over a downloaded one, so fetching here
            // would spend a few hundred megabytes on a build the launcher
            // would then decline to use -- a success that changes nothing,
            // which is worse than a failure that says so.
            if let Some((_, origin)) = install::effective_apk(&install::RobloxInstall::default()) {
                if let Some(why) = origin.why_not_updatable() {
                    status_line.set_visible(true);
                    status_line.set_label(why);
                    status_line.add_css_class("error");
                    return;
                }
            }

            // Disabled for the duration. A 229 MB fetch pressed twice is two
            // downloads into one staging directory, and the second one refuses
            // -- correctly, and confusingly, since the first is still working.
            // The button goes and the bar takes its place, which is the same
            // treatment the first-run screen gives it -- see
            // `crate::download_progress`. A disabled button reading
            // "Downloading…" with a sentence under it was two widgets saying
            // one thing and neither of them a bar.
            // **The control stays, and becomes the one that stops it.** That
            // is the interface guideline and it is also the only way out of a
            // 229 MB fetch on a slow line that does not involve killing the
            // client. The first version of this hid the button entirely, which
            // is the half of the pattern that looks tidy and leaves the user
            // stuck.
            let cancel = std::sync::Arc::new(cordial_update::provider::Cancel::new());
            status_line.set_visible(false);
            meter.start();
            action_for_click.set_label("Cancel");
            action_for_click.remove_css_class("suggested-action");
            // **The handler has to outlive this block**, because whoever
            // finishes the download -- success, failure or cancel -- must
            // disconnect it. It did not, and the button stayed wired to
            // `cancel.stop()` afterwards: pressing what now read "Check for
            // updates" stopped a fetch that was no longer running.
            let cancel_handler: std::rc::Rc<std::cell::RefCell<Option<glib::SignalHandlerId>>> =
                std::rc::Rc::new(std::cell::RefCell::new(None));
            {
                let handler = cancel_handler.clone();
                let cancel = cancel.clone();
                let id = action_for_click.connect_clicked(move |b| {
                    cancel.stop();
                    b.set_sensitive(false);
                    b.set_label("Stopping...");
                    // One press is enough; a second would stop a fetch already
                    // unwinding.
                    if let Some(id) = handler.borrow_mut().take() {
                        b.disconnect(id);
                    }
                });
                *cancel_handler.borrow_mut() = Some(id);
            }

            let button = action_for_click.clone();
            let line = status_line.clone();
            let stepping = meter.clone();
            let finishing = meter.clone();
            // The header-bar button and the knowledge behind it, so a finished
            // download can put both right. `present` is handed them precisely
            // so this window can; before this they were only ever read.
            let done_button = button_for_badge.clone();
            let done_last = last_for_badge.clone();
            let done_paint = paint.clone();
            on_worker_reporting(
                {
                    let cancel = cancel.clone();
                    move |report: &dyn Fn(cordial_update::provider::Progress)| {
                    // `Newest`, and this is the button where that matters.
                    // With `Any` a local copy always wins, so anybody with
                    // Sober installed would press Update, watch it succeed,
                    // and receive the build they already had -- for ever.
                    cordial_update::provider::obtain_and_install(
                        None,
                        cordial_update::provider::Want::Newest,
                        &cancel,
                        &mut |p| report(p),
                    )
                        .map(|(got, _)| got.version.name)
                        .map_err(|e| e.to_string())
                }},
                move |step| stepping.step(&step),
                move |outcome| {
                    // **Whatever happened, the button stops being Cancel.**
                    // Leaving the handler attached wired the finished button to
                    // `stop()`; leaving it insensitive after a cancel blocked
                    // the only control on the window, which is how a stopped
                    // download became a dead dialog.
                    if let Some(id) = cancel_handler.borrow_mut().take() {
                        button.disconnect(id);
                    }
                    button.set_sensitive(true);
                    match outcome {
                    Ok(version) => {
                        finishing.finish(&version);
                        // **The badge is cleared here and nowhere else.**
                        // Nothing else re-runs the comparison: `last` holds the
                        // answer from a check that ran before this download
                        // existed, so without this the header button keeps its
                        // arrow and its accent for the rest of the session --
                        // still pointing at an update the user has just
                        // installed. Local and free: no request, just the
                        // version now on disk.
                        if let Some(checked) = done_last.borrow_mut().as_mut() {
                            checked.reread_installed();
                        }
                        dress(&done_button, &done_last.borrow(), automatic);
                        // **And this window, not only the header button.**
                        // `paint` is the one place that turns a `Checked` into
                        // what is on screen, so a refresh that skips it leaves
                        // the installed-build line reading the version that was
                        // there before the download -- the update visibly
                        // succeeds and the window goes on insisting nothing has
                        // changed. Reported exactly that way: Cordial does not
                        // get the memo the APK was updated.
                        done_paint(&done_last.borrow());
                    }
                    Err(why) if why == cordial_update::Unreachable::Cancelled.to_string() => {
                        // **A cancel is not a failure and must not look like
                        // one.** It used to take the error path: red text, and
                        // STORES underneath explaining where Roblox publishes
                        // its builds -- advice for somebody who could not
                        // download, shown to somebody who chose not to.
                        finishing.stopped();
                        button.set_label(DOWNLOAD);
                        button.add_css_class("suggested-action");
                        line.set_visible(true);
                    }
                    Err(why) => {
                        // Verbatim, then what to do about it. A mirror being
                        // down and a machine having no network look identical
                        // from in here and only one is the user's to fix.
                        finishing.failed(&format!("{why}\n\n{STORES}"));
                        button.set_label(DOWNLOAD);
                        button.add_css_class("suggested-action");
                        line.set_visible(true);
                    }
                }},
            );
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
    toolbar.add_top_bar(&banner);
    toolbar.set_content(Some(&scroller));
    // A bottom bar rather than the last child of the scrolled pane: the button
    // has to stay put while a long changelog moves under it, or the control
    // this window exists to offer scrolls off the bottom of it.
    toolbar.add_bottom_bar(&footer);
    window.set_content(Some(&toolbar));
    // The button, not the changelog — see `notes_body.set_can_focus(false)`.
    GtkWindowExt::set_focus(&window, Some(&action));
    OPEN.with(|open| *open.borrow_mut() = Some(window.downgrade()));
    window.present();
}

/// The label once a check says there is something newer.
///
/// **There used to be two labels here.** `Update…` was shown when no source
/// existed, its ellipsis carrying GNOME's meaning of "this opens something
/// rather than doing it" -- honest at the time, because what it opened was a
/// panel explaining that Cordial could not fetch a build. A source exists now,
/// so the second label and the panel behind it are both gone.
const DOWNLOAD: &str = "Download Roblox";

/// The resting label, in every mode including Automatic. Roblox publishes no
/// Android version number this can compare against, so "nothing newer" is the
/// near-permanent answer and this is the near-permanent word on the button.
const CHECK: &str = "Check for updates";

/// What the button says while the request is in flight.
///
/// On the button rather than on the line above it: the check is started by
/// pressing this, so this is where somebody is already looking, and a status
/// line that says "Checking…" and then has to be restored to something else is
/// two states to keep in step instead of one.
const CHECKING: &str = "Checking…";

/// The word the button wears, decided by state rather than by the update mode.
///
/// **`Download` is gated on a source existing, not on an update existing.** The
/// two come apart in the ordinary case: Roblox can publish a newer engine that
/// Cordial has no way to fetch, and labelling that Download would promise a file
/// that never arrives.
fn action_label(checked: &Option<Checked>) -> &'static str {
    match checked {
        // This used to ask whether a source existed and say `Update…` when
        // none did, with the ellipsis meaning "opens a panel that explains why
        // not". A source exists now, so the button does the thing it names.
        Some(checked) if checked.update_available() => DOWNLOAD,
        _ => CHECK,
    }
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
        // **This is the one place in the UI that names the mirror**, and it is
        // the right one: a settings page is where somebody goes to find out
        // how a thing works, unlike the first-run screen where they are trying
        // to get moving. Removing the sentence from there and keeping it here
        // is a trade about where the explanation lands, not about whether it
        // is made.
        adw::PreferencesGroup::builder()
            .title("Getting a newer build")
            .description(FETCH_EXPLAINS)
            .build();

    // Said where somebody looking at the update settings will see it, rather
    // than only when they press a button and are refused. A page of switches
    // that cannot take effect has to say so; that requirement is older than
    // this feature and it now has a second reason to exist.
    if let Some((_, origin)) = install::effective_apk(&install::RobloxInstall::default()) {
        if let Some(why) = origin.why_not_updatable() {
            let row = adw::ActionRow::builder().title("Updates are off for this build").subtitle(why).build();
            row.set_subtitle_lines(4);
            group.add(&row);
        }
    }

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

    // One switch, not two. **Download on Wi-Fi used to sit above this**, and its
    // own subtitle admitted the problem: Cordial cannot see whether a link is
    // wireless, so it asked NetworkManager the metered question and called the
    // not-metered answer Wi-Fi. Two names for one bit — and a wired desktop was
    // governed by the switch labelled Wi-Fi, which is where it stopped being a
    // wording quibble. Nobody has a reason to turn Wi-Fi off while leaving
    // metered on, so the question is asked once, in the direction people have an
    // opinion about.
    let metered_row = adw::SwitchRow::builder()
        .title("Download on metered connection")
        .subtitle(
            "A connection NetworkManager only guesses is unmetered counts as metered, which an \
             ordinary wired desktop often is.",
        )
        .active(config.borrow().download_on.metered)
        .build();
    metered_row.set_subtitle_lines(4);

    // The warning row that used to appear when both switches were off has gone
    // with the second switch. There is no longer a settings combination that
    // downloads nothing ever — off means "wait for an unmetered link", not
    // "never" — and the contradiction it existed to announce was an artefact of
    // asking one question twice. `NEVER_DOWNLOADS` still says what off means and
    // is still shown by the refusal a held download reports.
    {
        let config = config.clone();
        let config_path = config_path.clone();
        metered_row.connect_active_notify(move |row| {
            config.borrow_mut().download_on = DownloadOn { metered: row.is_active() };
            persist(&config, &config_path);
        });
    }

    group.add(&metered_row);
    group
}

/// What the dropdown means, in the three sentences the modes differ by.
const AUTO_UPDATE_SUBTITLE: &str =
    "Ask opens the changelog when an update is waiting. Manual checks only when you press the \
     header-bar button.";

/// Said on the settings group. One sentence, and both halves matter.
pub const SETTINGS_DESCRIPTION: &str =
    "Builds come from APKPure, and install only if Roblox's signing certificate signed them.";

/// What to do instead, when something went wrong.
///
/// **It used to name Google Play and the Amazon Appstore.** That was the right
/// thing to say while Cordial could not fetch anything and those were the only
/// two places the file existed -- a message that says only "Cordial cannot"
/// reads as Cordial being broken. It is the wrong thing to say now: neither is
/// a source Cordial can use, nobody reading this can act on either, and naming
/// them is telling somebody about doors they have no key to.
const STORES: &str = "Try again, or choose an APK yourself on the Roblox page in Settings.";

/// What the fetch does, wherever it needs saying.
///
/// One string and not two, because the second half is the whole of why the
/// first half is acceptable. "Cordial downloads from APKPure" alone reads as
/// Cordial trusting a mirror, which is the one thing it does not do.
const FETCH_EXPLAINS: &str =
    "Builds come from APKPure, and install only if Roblox's signing certificate signed them.";

/// Said on the installed-build group, where the picker used to be.
///
/// It names where the picker went. A group that lost its only control and says
/// nothing about it reads as a control that failed to appear.
const INSTALLED_DESCRIPTION: &str = "Choose your own APK on the Roblox page in Settings.";

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
        return format!(
            "None yet. Cordial takes {} out of the APK the first time you launch, \
             into its own cache.",
            cordial_update::apk::LIBRARY_IN_APK
        );
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
fn release_lines_full(
    release: &Result<(Release, Notes), Unreachable>,
    entries: Option<&[Entry]>,
) -> (String, String) {
    match release {
        // The Creator Hub table when it answered, the announcement post when it
        // did not. The post is a covering note — "732 has landed, enjoy" and a
        // set of links — so it is the fallback rather than the source; showing
        // it and calling it a changelog was the bug this replaced.
        Ok((release, notes)) => {
            let body = match entries {
                Some(entries) if !entries.is_empty() => changelog::render_entries(entries),
                _ => notes.text().trim().to_string(),
            };
            (heading(release, notes), body)
        }
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
            // Tests that predate `obtainable` describe a world where the
            // mirror offers whatever Roblox announced -- the ordinary case,
            // and the one that keeps their meaning. Where the installed build
            // is already at the announced major, the mirror offers exactly
            // what is installed, which is what "nothing newer" means.
            obtainable: match (installed, latest) {
                (Some(i), Some(m)) => Some(match version::major_of(i) {
                    Some(here) if here < m => format!("2.{m}.9999"),
                    _ => i.to_string(),
                }),
                (Some(i), None) => Some(i.to_string()),
                (None, Some(m)) => Some(format!("2.{m}.9999")),
                (None, None) => None,
            },
            release: match latest {
                Some(major) => Ok((release(major), notes(major))),
                None => Err(Unreachable::Transport {
                    url: changelog::CATEGORY.into(),
                    why: "no route to host".into(),
                }),
            },
            // The docs table is deliberately absent in these fixtures: every
            // test here is about the version comparison or the button, and the
            // fallback to the announcement post is the state that must keep
            // working when the Creator Hub is unreachable.
            entries: None,
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

    /// **This test used to assert the opposite** -- that the page said the
    /// settings could not act yet -- and it was right to, for as long as
    /// nothing here downloaded anything. Controls that look live and govern
    /// nothing are the interface version of a stub returning success.
    ///
    /// ADR-025 made the page able to act, so the invariant moves rather than
    /// disappears: **the fetch is never named without the sentence that makes
    /// it acceptable.** "Cordial downloads from APKPure" on its own is the
    /// interface version of a different lie -- one that sounds like Cordial
    /// trusts a mirror, which is exactly what it does not do.
    #[test]
    fn the_settings_description_names_the_mirror_and_the_check_together() {
        assert!(SETTINGS_DESCRIPTION.contains("APKPure"), "{SETTINGS_DESCRIPTION}");
        assert!(SETTINGS_DESCRIPTION.contains("signing certificate"), "{SETTINGS_DESCRIPTION}");
    }

    /// The same rule, applied to every string that offers the fetch. A new one
    /// that names the mirror and forgets the check fails here rather than in
    /// front of somebody deciding whether to press it.
    #[test]
    fn nothing_offers_the_download_without_saying_what_is_checked() {
        for text in [SETTINGS_DESCRIPTION, FETCH_EXPLAINS] {
            assert!(text.contains("APKPure"), "{text}");
            assert!(
                text.contains("signing certificate") || text.contains("signed it"),
                "names the mirror without naming the check: {text}"
            );
        }
    }

    #[test]
    fn the_dropdown_subtitle_distinguishes_the_three_modes() {
        // Ask and Update in background differ by one behaviour — the dialog on
        // launch — and a dropdown whose options are three words is where that
        // difference goes missing.
        assert!(AUTO_UPDATE_SUBTITLE.contains("opens the changelog"), "{AUTO_UPDATE_SUBTITLE}");
        assert!(AUTO_UPDATE_SUBTITLE.contains("Manual checks only"), "{AUTO_UPDATE_SUBTITLE}");
    }

    /// **This test used to require the Update panel to name Google Play and
    /// the Amazon Appstore.** The panel is gone -- the button downloads now --
    /// and so is the reason to name them: neither is a source Cordial can use,
    /// so telling somebody about them is describing doors they have no key to.
    ///
    /// What survives is the rule underneath: when something fails, say what to
    /// do next. A message that only reports a failure is the silent failure
    /// with extra steps.
    #[test]
    fn the_failure_text_says_what_to_do_rather_than_only_what_broke() {
        assert!(!STORES.contains("Google Play"), "{STORES}");
        assert!(!STORES.contains("Amazon"), "{STORES}");
        assert!(STORES.contains("Roblox page"), "{STORES}");
        assert!(STORES.contains("Try again"), "{STORES}");
    }

    #[test]
    fn the_button_says_download_only_when_there_is_something_to_download() {
        // The label is the promise. With no source configured nothing can be
        // downloaded — Roblox publishes no Android artefact — so a button that
        // **This test used to assert the opposite half of the same rule.** It
        // required the update label to be `Update…` and to not say Download,
        // because nothing could be downloaded and a Download button that opened
        // an explanation was AGENTS.md's stub-that-reports-success with a widget
        // around it.
        //
        // ADR-025 built the source, so the rule flips rather than relaxes: the
        // button says Download exactly when pressing it downloads. What must
        // still never happen is Check saying Download.
        assert_eq!(action_label(&None), CHECK);
        assert_eq!(action_label(&Some(checked(None, Some(732)))), CHECK);
        assert_eq!(action_label(&Some(checked(Some("0.732.23.7321040"), Some(732)))), CHECK);

        let available = checked(Some("0.700.1.7001000"), Some(732));
        assert!(available.update_available());
        assert_eq!(action_label(&Some(available)), DOWNLOAD);

        assert!(!CHECK.contains("Download"), "{CHECK}");
        // No ellipsis on either. GNOME's convention is that `…` means the
        // button opens something rather than doing it, and both of these now
        // do the thing they name.
        assert!(!DOWNLOAD.ends_with('…'), "{DOWNLOAD}");
        assert!(!CHECK.ends_with('…'), "{CHECK}");
    }

    /// **A build Roblox has not published for this architecture is not an
    /// update.** The case that produced the report: Roblox announced 735, and
    /// published 2.735.1138 for ARM only, so the newest build this machine
    /// could run stayed 2.734.917. Comparing against the announcement asked for
    /// attention that pressing the button could never satisfy.
    #[test]
    fn an_announced_build_with_no_obtainable_version_is_not_an_update() {
        let mut state = checked(Some("2.734.0.917"), Some(735));
        // What the mirror can actually supply for x86-64.
        state.obtainable = Some("2.734.917".to_string());

        assert!(
            !state.update_available(),
            "the newest obtainable build is installed, so nothing is waiting"
        );
        assert!(
            state.newer_announced_than_obtainable(),
            "and the window should still be able to explain why 735 is not on offer"
        );

        let (_, attention) = (0, dressing(&Some(state.clone()), Automatic::Ask).2);
        assert!(!attention, "no attention for an update that cannot be installed");

        // And once a 735 does appear for this architecture, it is an update again.
        state.obtainable = Some("2.735.1200".to_string());
        assert!(state.update_available());
    }

    /// The banner says the more urgent of two things, and says nothing when
    /// there is nothing to say.
    #[test]
    fn the_banner_prefers_news_over_a_standing_setting() {
        let waiting = checked(Some("2.700.1.7001000"), Some(732));
        let (text, has_button) = banner_text(&Some(waiting), Automatic::Manual).expect("a banner");
        assert!(text.contains("newer Roblox build is available"), "{text}");
        assert!(!has_button, "the window already has the Download button below it");

        // Nothing waiting, checks off: the standing condition surfaces, with
        // somewhere to go about it.
        let current = checked(Some("2.732.9999"), Some(732));
        let (text, has_button) =
            banner_text(&Some(current.clone()), Automatic::Manual).expect("a banner");
        assert!(text.contains("Automatic update checks are off"), "{text}");
        assert!(has_button, "a banner naming a setting must offer a way to it");

        // Nothing waiting, checks on: no banner at all.
        assert!(banner_text(&Some(current), Automatic::Ask).is_none());
    }

    /// **The badge must go out once the thing it points at is installed.**
    ///
    /// Reported from the running app: download an update, close the window,
    /// and the header button keeps its arrow and its accent -- still asking
    /// for attention it has already been given, which reads as the update
    /// having failed.
    ///
    /// The cause is that `Checked::installed` is a snapshot from before the
    /// download, and nothing re-ran the comparison afterwards. `dressing` was
    /// never wrong; it was being asked about stale knowledge. So this pins the
    /// invariant at the level that has no GTK in it: the moment `installed`
    /// catches up with what Roblox published, the attention state is false and
    /// the icon goes back to the package.
    #[test]
    fn installing_the_update_puts_the_badge_out() {
        let automatic = Automatic::Ask;

        // Before: behind by a major, so the button asks for attention.
        let mut state = checked(Some("0.700.1.7001000"), Some(732));
        let (icon, _, attention) = dressing(&Some(state.clone()), automatic);
        assert!(attention, "a build behind the published one must ask for attention");
        assert_eq!(icon, UPDATE_ICON);

        // After: `reread_installed` would have picked this up off disk.
        state.installed = Some("0.732.23.7321040".to_string());
        let (icon, _, attention) = dressing(&Some(state), automatic);
        assert!(!attention, "the badge must go out once the update is installed");
        assert_eq!(icon, PACKAGE_ICON, "and the icon goes back to the package");
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
        assert!(INSTALLED_DESCRIPTION.contains("your own APK"), "{INSTALLED_DESCRIPTION}");
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
        // It said "Google Play" while that was where a user had to go. The row
        // now reports where Cordial actually gets a build.
        assert!(refused.contains("APKPure"), "{refused}");

        let hash = Sha256Hash::parse(&format!("sha256:{}", "ab".repeat(32))).unwrap();
        let configured = source_line(&Source::new("https://example.invalid/base.apk", hash));
        assert!(configured.contains("https://example.invalid/base.apk"), "{configured}");
        assert!(configured.contains("does not fetch"), "{configured}");
    }

    #[test]
    fn a_fetched_changelog_shows_its_title_date_and_opening() {
        let (title, body) = release_lines_full(&Ok((release(732), notes(732))), None);
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
        let (_, body) = release_lines_full(&Ok((self::release(732), release)), None);
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
        // The seam between the rows and `UpdateSettings::plan`. If these stopped
        // agreeing, the settings page would be governing nothing and there would
        // be nothing on screen to say so.
        use cordial_update::settings::Plan;
        let config = ShellConfig {
            automatic_updates: Automatic::Background,
            download_on: DownloadOn { metered: false },
            ..Default::default()
        };
        // The switch off and the link metered: held, with the reason.
        match update_settings(&config).plan(Metered::GuessNo) {
            Plan::CheckAndAsk { why: Some(why) } => {
                assert!(why.contains("Download on metered connection"), "{why}");
            }
            other => panic!("a metered link with the switch off must not plan a download: {other:?}"),
        }
        // The same settings on an unmetered link download, which is what stops
        // "off" meaning "never" now that it is the only connection switch.
        assert_eq!(update_settings(&config).plan(Metered::No), Plan::CheckAndDownload);
    }
}
