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
//! comes from and offers the file picker — because a control that looked live
//! and could never fire would be AGENTS.md's stub-that-lies wearing a widget:
//! something proceeds on an answer that is not true, except here the something
//! is a person. This project has already shipped a settings page describing
//! plugins nobody had installed, twice.
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

use crate::install::{self, Origin};
use crate::settings::{choose_file, persist};
use crate::shell_config::ShellConfig;

/// The three states, and the icon each one wears.
///
/// `system-software-install-symbolic` is the download icon here rather than
/// `software-update-available-symbolic`, which is the icon that *means* an
/// update is waiting. Wearing that permanently would be the attention state
/// drawn instead of written, and it is the same icon `instructions.rs` already
/// uses for the same subject: obtaining a Roblox build.
const DOWNLOAD_ICON: &str = "system-software-install-symbolic";
const REFRESH_ICON: &str = "view-refresh-symbolic";

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
pub fn header_button(
    parent: &impl IsA<gtk::Window>,
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> gtk::Button {
    let parent = parent.as_ref().clone();
    let button = gtk::Button::from_icon_name(DOWNLOAD_ICON);
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
        let config_path = config_path.clone();
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
                    present(&parent, config.clone(), config_path.clone(), last.clone(), button.clone());
                }
            });
        });
    }

    {
        let last = last.clone();
        let parent = parent.clone();
        let config = config.clone();
        let config_path = config_path.clone();
        let button_for_click = button.clone();
        button.connect_clicked(move |_| {
            present(&parent, config.clone(), config_path.clone(), last.clone(), button_for_click.clone());
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
fn dress(button: &gtk::Button, last: &Option<Checked>, automatic: Automatic) {
    let (icon, tooltip, attention) = match last {
        Some(checked) if checked.update_available() => (
            DOWNLOAD_ICON,
            "Roblox has published a newer build",
            true,
        ),
        Some(_) => (DOWNLOAD_ICON, "Roblox build", false),
        // Nothing checked yet. In manual that is the resting state and the
        // circular arrow is the offer; in the other two it is the second or so
        // before the answer arrives, and the download icon is where it will stay.
        None if automatic == Automatic::Manual => {
            (REFRESH_ICON, "Check for a Roblox update", false)
        }
        None => (DOWNLOAD_ICON, "Roblox build", false),
    };
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
    check_with(
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
fn check_with(
    work: impl FnOnce() -> Checked + Send + 'static,
    then: impl Fn(Checked) + 'static,
) {
    let (tx, rx) = mpsc::channel::<Checked>();
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

/// The dialog behind the button.
///
/// One dialog for all three states, with the top group saying which. Three
/// dialogs would be three places for the same sentence about where a build comes
/// from, and they would drift.
pub fn present(
    parent: &gtk::Window,
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
    last: Rc<RefCell<Option<Checked>>>,
    button: gtk::Button,
) {
    let automatic = config.borrow().automatic_updates;
    let page = adw::PreferencesPage::new();

    // --- what is known, and what to do about it ----------------------------
    let status_group = adw::PreferencesGroup::new();
    let status_row = adw::ActionRow::new();
    status_row.set_subtitle_lines(6);
    status_group.add(&status_row);

    // Both suffixes are built once and shown by state, rather than rebuilt: a
    // manual check has to be able to turn this dialog from one into the other
    // while it is open, which is the state the design describes as the refresh
    // button becoming the update button.
    let update = gtk::Button::with_label("Update…");
    update.add_css_class("suggested-action");
    update.set_valign(gtk::Align::Center);
    update.set_visible(false);
    status_row.add_suffix(&update);

    let check_now = gtk::Button::with_label("Check Now");
    check_now.set_valign(gtk::Align::Center);
    status_row.add_suffix(&check_now);
    page.add(&status_group);

    // --- what is installed -------------------------------------------------
    let installed_group = adw::PreferencesGroup::builder()
        .title("Installed build")
        .description(
            "Cordial ships no Roblox code and never will, so this is a build you already have. \
             Choosing a file here is the same setting as the APK row in Settings.",
        )
        .build();

    let apk_row = adw::ActionRow::builder().title("APK").build();
    apk_row.set_subtitle_lines(3);
    let version_row = adw::ActionRow::builder().title("Roblox version").build();
    version_row.set_subtitle_lines(4);
    let cache_row = adw::ActionRow::builder().title("Extracted engine").build();
    cache_row.set_subtitle_lines(4);

    let refresh_paths = {
        let config = config.clone();
        let apk_row = apk_row.clone();
        let version_row = version_row.clone();
        let cache_row = cache_row.clone();
        // Recomputed rather than remembered, for the same reason `install::locate`
        // looks every time: the APK can move, and a stale "yes it is there" is
        // the sentence that sends somebody debugging the wrong thing.
        Rc::new(move || {
            let effective = install::effective_apk(&config.borrow().roblox);
            let apk = effective.as_ref().map(|(path, _)| path.clone());
            apk_row.set_subtitle(&apk_line(effective));
            let engine_dir = install::engine_cache();
            version_row.set_subtitle(&version_line(cache::recorded_version(&engine_dir)));
            cache_row.set_subtitle(&cache_line(
                engine_dir.join(install::LIBRARY).is_file(),
                cache::stamp_of(&engine_dir),
                apk.as_deref().is_some_and(|apk| cache::is_current(&engine_dir, apk)),
            ));
        })
    };
    refresh_paths();

    let choose = gtk::Button::with_label("Choose…");
    choose.set_valign(gtk::Align::Center);
    {
        // The same picker the Roblox settings page uses, called the same way. A
        // second chooser of its own would be a second thing to keep agreeing
        // with `RobloxInstall`, and the two would drift the first time one of
        // them learned something about split APKs.
        let parent = parent.clone();
        let config = config.clone();
        let config_path = config_path.clone();
        let refresh_paths = refresh_paths.clone();
        choose.connect_clicked(move |_| {
            let config = config.clone();
            let config_path = config_path.clone();
            let refresh_paths = refresh_paths.clone();
            choose_file(&parent, "Choose the Roblox APK", false, move |path| {
                config.borrow_mut().roblox.apk = Some(path);
                persist(&config, &config_path);
                refresh_paths();
            });
        });
    }
    apk_row.add_suffix(&choose);

    installed_group.add(&apk_row);
    installed_group.add(&version_row);
    installed_group.add(&cache_row);
    page.add(&installed_group);

    // --- what Roblox has published ----------------------------------------
    let notes_group = adw::PreferencesGroup::builder()
        .title("Roblox release notes")
        .description(
            "From Roblox's own DevForum. The number in the title is the engine major — the 732 \
             in 0.732.23.7321040, and the same number the client reports about itself.",
        )
        .build();

    let notes_row = adw::ActionRow::builder().title("Not checked").build();
    notes_row.set_subtitle_lines(10);
    let open = gtk::LinkButton::with_label("https://devforum.roblox.com/c/updates/release-notes", "Read");
    open.set_valign(gtk::Align::Center);
    open.set_visible(false);
    notes_row.add_suffix(&open);
    notes_group.add(&notes_row);
    page.add(&notes_group);

    // --- where a newer one comes from --------------------------------------
    let source_group =
        adw::PreferencesGroup::builder().title("Getting a newer build").description(STORES).build();

    let source_row = adw::ActionRow::builder()
        .title("Download source")
        .subtitle(source_line(&Source::configured()))
        .build();
    source_row.set_subtitle_lines(8);
    source_group.add(&source_row);

    let connection_row =
        adw::ActionRow::builder().title("This connection").subtitle("Not checked").build();
    connection_row.set_subtitle_lines(4);
    source_group.add(&connection_row);
    page.add(&source_group);

    page.add(&build_update_group(config.clone(), config_path.clone()));

    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Roblox Build")
        .default_width(660)
        .default_height(720)
        .build();

    // Everything the answer touches, in one closure, so a check started from
    // this dialog and one started at launch put the window into the same state.
    let paint = {
        let status_row = status_row.clone();
        let notes_row = notes_row.clone();
        let connection_row = connection_row.clone();
        let open = open.clone();
        let update = update.clone();
        let check_now = check_now.clone();
        let button = button.clone();
        Rc::new(move |checked: &Option<Checked>| {
            let (title, subtitle) = status_lines(checked, automatic);
            status_row.set_title(&title);
            status_row.set_subtitle(&subtitle);
            let available = checked.as_ref().is_some_and(Checked::update_available);
            update.set_visible(available);
            check_now.set_sensitive(true);

            match checked {
                Some(checked) => {
                    connection_row.set_subtitle(&connection_line(checked.metered));
                    let (title, body) = release_lines(&checked.release);
                    notes_row.set_title(&title);
                    notes_row.set_subtitle(&body);
                    if let Ok((release, _)) = &checked.release {
                        open.set_uri(&release.web_url());
                        open.set_visible(true);
                    }
                }
                None => {
                    connection_row.set_subtitle("Asking NetworkManager…");
                    notes_row.set_title("Fetching…");
                    notes_row.set_subtitle("Roblox's release notes, from the DevForum.");
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
        let status_row = status_row.clone();
        let check_now = check_now.clone();
        Rc::new(move || {
            check_now.set_sensitive(false);
            status_row.set_title("Checking…");
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
        let run_check = run_check.clone();
        check_now.connect_clicked(move |_| run_check());
    }
    // Opening this window is a check in every mode, which is what makes the
    // refresh button in Manual do what its icon says. It costs a second request
    // if the launch check is still in flight, and that is one small GET against
    // a forum rather than a reason to build a way for two dialogs to share one
    // in-flight answer.
    if last.borrow().is_none() {
        run_check();
    }

    {
        // The Update button, and the one thing it must not do is imply a fetch.
        // It reports what `Source::configured` decided — which today is a
        // refusal naming Google Play and the Amazon Appstore — and hands over
        // the file picker, which is the part that actually gets somebody a newer
        // build.
        let window_for_update = window.clone();
        let config = config.clone();
        let config_path = config_path.clone();
        let refresh_paths = refresh_paths.clone();
        update.connect_clicked(move |_| {
            let dialog = adw::MessageDialog::builder()
                .transient_for(&window_for_update)
                .modal(true)
                .heading("Cordial cannot download this build")
                .body(update_body(&Source::configured()))
                .build();
            dialog.add_response("close", "Close");
            dialog.add_response("choose", "Choose an APK…");
            dialog.set_response_appearance("choose", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("choose"));

            let window_for_choose = window_for_update.clone();
            let config = config.clone();
            let config_path = config_path.clone();
            let refresh_paths = refresh_paths.clone();
            dialog.connect_response(None, move |_, response| {
                if response != "choose" {
                    return;
                }
                let config = config.clone();
                let config_path = config_path.clone();
                let refresh_paths = refresh_paths.clone();
                choose_file(
                    window_for_choose.upcast_ref::<gtk::Window>(),
                    "Choose the Roblox APK",
                    false,
                    move |path| {
                        config.borrow_mut().roblox.apk = Some(path);
                        persist(&config, &config_path);
                        refresh_paths();
                    },
                );
            });
            dialog.present();
        });
    }

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&page));
    window.set_content(Some(&toolbar));
    window.present();
}

/// The dropdown and the two switches, for the Roblox settings page.
///
/// `build_appearance_page`'s shape for the dropdown, to the letter: a
/// `StringList` in the order the enum's `index` defines, `selected` from the
/// saved value, and one `connect_selected_notify` that writes the config and
/// persists it. The switches follow `build_performance_group`'s.
pub fn build_update_group(
    config: Rc<RefCell<ShellConfig>>,
    config_path: Rc<PathBuf>,
) -> adw::PreferencesGroup {
    let group =
        adw::PreferencesGroup::builder().title("Updates").description(SETTINGS_DESCRIPTION).build();

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
     a mirror and call that Roblox. Obtain an APK and choose it above.";

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
             it here.",
            source.url
        ),
        Err(refusal) => format!("{refusal}\n\n{STORES}"),
    }
}

/// What the APK row says.
fn apk_line(effective: Option<(PathBuf, Origin)>) -> String {
    match effective {
        Some((path, origin)) => format!("{}\n{}", path.display(), origin.describe()),
        None => "No build found. Choose an APK, or press Roblox in the launcher for how to get \
                 one."
            .to_string(),
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
fn cache_line(engine: bool, stamp: Option<String>, current: bool) -> String {
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
fn release_lines(release: &Result<(Release, Notes), Unreachable>) -> (String, String) {
    match release {
        Ok((release, notes)) => {
            let when = release.created_at.split('T').next().unwrap_or_default();
            let head = if when.is_empty() {
                notes.title.clone()
            } else {
                format!("{} — {when}", notes.title)
            };
            (head, summarise(&notes.text()))
        }
        Err(why) => ("Could not fetch the release notes".to_string(), why.to_string()),
    }
}

/// How much of a set of release notes goes in a row.
const SUMMARY_LINES: usize = 8;
const SUMMARY_CHARS: usize = 600;

/// The opening of the notes, and no more. The whole thing is a long document and
/// a row is not where it gets read; the Read button is.
fn summarise(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(SUMMARY_LINES) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim());
        if out.chars().count() >= SUMMARY_CHARS {
            break;
        }
    }
    match out.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}…", out[..cut].trim_end()),
        None => out,
    }
}

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

        let hash = Sha256Hash::parse(&format!("sha256:{}", "ab".repeat(32))).unwrap();
        let with_source = update_body(&Source::new("https://example.invalid/base.apk", hash));
        assert!(with_source.contains("not built"), "{with_source}");
    }

    #[test]
    fn the_refusal_names_both_stores_rather_than_only_refusing() {
        assert!(STORES.contains("Google Play"), "{STORES}");
        assert!(STORES.contains("Amazon Appstore"), "{STORES}");
        assert!(STORES.contains("mirror"), "{STORES}");
    }

    #[test]
    fn the_apk_row_says_where_the_build_came_from() {
        // Provenance is not decoration: the build usually lives in another
        // application's private directory, and somebody who does not know that
        // cannot understand why removing Sober broke Cordial.
        let line = apk_line(Some((PathBuf::from("/home/someone/base.apk"), Origin::Sober)));
        assert!(line.contains("/home/someone/base.apk"), "{line}");
        assert!(line.contains("org.vinegarhq.Sober"), "{line}");
        assert!(apk_line(None).contains("Choose an APK"));
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
        let (title, body) = release_lines(&Ok((release(732), notes(732))));
        assert_eq!(title, "Release Notes for 732 — 2026-07-29");
        assert!(body.starts_with("Hi all,"), "{body}");
    }

    #[test]
    fn a_long_set_of_notes_is_cut_rather_than_pasted_whole() {
        // Roblox's release notes run to thousands of words. A row that takes all
        // of them is a window nobody can see the rest of.
        let long = "line\n".repeat(200);
        let cut = summarise(&long);
        assert!(cut.lines().count() <= SUMMARY_LINES, "{cut}");
        let wide = "x".repeat(5000);
        assert!(summarise(&wide).chars().count() <= SUMMARY_CHARS + 1);
        assert!(summarise(&wide).ends_with('…'));
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
        check_with(
            || checked(Some("0.700.1.7001000"), Some(732)),
            {
                let landed = landed.clone();
                move |checked| {
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
