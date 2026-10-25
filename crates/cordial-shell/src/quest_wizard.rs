//! "Get it from your Quest": the step-by-step pages that copy Roblox off the
//! user's own headset over USB (ADR-053).
//!
//! Subpages of the Settings dialog, pushed one after another, the way a
//! plugin's own page is (`plugin_preferences::push`): the dialog's back button
//! goes back a step and nothing floats free of Settings. Each step checks for
//! itself rather than trusting the one before -- a cable can be pulled between
//! two clicks -- and says what is wrong and what to do about it.
//!
//! Every `adb` call runs on a worker; the first one starts adb's server and
//! takes a second or two, and a headset that is slow to answer must not freeze
//! the window. Nothing is ever changed on the headset: see `quest_adb`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use libadwaita as adw;
use libadwaita::glib;
use libadwaita::gtk;
use libadwaita::prelude::*;

use cordial_shell::quest_adb::{self, Adb, Device, Freshness, Headset, Roblox, SystemAdb};
use cordial_update::quest;

/// How often the Connect step asks `adb devices` while it is on screen.
const POLL: Duration = Duration::from_millis(1500);

struct Wizard {
    dialog: adw::PreferencesDialog,
    adb: RefCell<Option<Arc<dyn Adb>>>,
    device: RefCell<Option<Device>>,
    on_imported: Rc<dyn Fn(String)>,
}

/// Open the steps, or, when this computer is already allowed and the headset
/// is plugged in, go straight to the Roblox step: an update is then one click.
pub fn open(dialog: &adw::PreferencesDialog, on_imported: Rc<dyn Fn(String)>) {
    let w = Rc::new(Wizard {
        dialog: dialog.clone(),
        adb: RefCell::new(find_adb()),
        device: RefCell::new(None),
        on_imported,
    });
    let Some(adb) = w.adb.borrow().clone() else {
        push_need(&w);
        return;
    };
    let w2 = w.clone();
    crate::updater::on_worker_reporting(
        move |_: &dyn Fn(())| quest_adb::probe(adb.as_ref()),
        |_| {},
        move |probed| match probed {
            Ok(Headset::Ready(device)) => {
                *w2.device.borrow_mut() = Some(device);
                push_roblox(&w2);
            }
            _ => push_need(&w2),
        },
    );
}

/// Open at a named step, for `CORDIAL_SHELL_PRESENT=settings=vr,vr-pull=<step>`:
/// the same photograph seam `window::open_on_start` describes, one level
/// further down. `need`, `developer`, `connect` or `roblox`.
pub fn open_at(dialog: &adw::PreferencesDialog, on_imported: Rc<dyn Fn(String)>, at: &str) {
    let w = Rc::new(Wizard {
        dialog: dialog.clone(),
        adb: RefCell::new(find_adb()),
        device: RefCell::new(None),
        on_imported,
    });
    match at {
        "developer" => push_developer_mode(&w),
        "connect" => push_connect(&w),
        "roblox" => {
            if let Some(adb) = w.adb.borrow().clone() {
                if let Ok(Headset::Ready(d)) = quest_adb::probe(adb.as_ref()) {
                    *w.device.borrow_mut() = Some(d);
                }
            }
            push_roblox(&w)
        }
        _ => push_need(&w),
    }
}

fn find_adb() -> Option<Arc<dyn Adb>> {
    SystemAdb::find().map(|a| Arc::new(a) as Arc<dyn Adb>)
}

/// A step: a header with the dialog's back button, the content, and one main
/// action at the bottom.
fn step(
    title: &str,
    content: &adw::PreferencesPage,
    action: Option<&gtk::Button>,
) -> adw::NavigationPage {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(content));
    if let Some(button) = action {
        button.add_css_class("pill");
        button.add_css_class("suggested-action");
        button.set_halign(gtk::Align::Center);
        button.set_margin_top(12);
        button.set_margin_bottom(18);
        view.add_bottom_bar(button);
    }
    adw::NavigationPage::new(&view, title)
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .subtitle_selectable(true)
        .build()
}

// --- 1. What you need -----------------------------------------------------

fn push_need(w: &Rc<Wizard>) {
    let page = adw::PreferencesPage::builder()
        .description(
            "Cordial copies Roblox from your own Quest over a USB cable. It never downloads it from \
             anywhere else.",
        )
        .build();
    let group = adw::PreferencesGroup::builder().title("You need").build();
    group.add(&row(
        "A Meta Quest with Roblox installed",
        "From the Meta Horizon Store, on the headset.",
    ));
    group.add(&row(
        "A USB-C cable that carries data",
        "A charge-only cable is the most common reason a headset never shows up.",
    ));
    let adb_row = row("adb on this computer", "");
    let check = gtk::Button::builder()
        .label("Check Again")
        .valign(gtk::Align::Center)
        .build();
    adb_row.add_suffix(&check);
    group.add(&adb_row);
    page.add(&group);

    let next = gtk::Button::with_label("Continue");
    let show = {
        let adb_row = adb_row.clone();
        let next = next.clone();
        let w = w.clone();
        move || {
            *w.adb.borrow_mut() = find_adb();
            let found = w.adb.borrow().is_some();
            adb_row.set_subtitle(&if found {
                "Found. adb is Android's tool for talking to a headset over USB.".to_string()
            } else {
                format!(
                    "Not found. adb is Android's tool for talking to a headset over USB. {}",
                    crate::launch::adb_install_hint()
                )
            });
            next.set_sensitive(found);
        }
    };
    show();
    check.connect_clicked(move |_| show());
    let w2 = w.clone();
    next.connect_clicked(move |_| push_developer_mode(&w2));
    w.dialog
        .push_subpage(&step("Get Roblox from Your Quest", &page, Some(&next)));
}

// --- 2. Developer mode ----------------------------------------------------

fn push_developer_mode(w: &Rc<Wizard>) {
    let page = adw::PreferencesPage::builder()
        .description("A Quest only lets a computer copy apps off it once developer mode is on.")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("In the Meta Horizon app on your phone")
        .build();
    group.add(&row("1. Open Devices, then your headset", ""));
    group.add(&row("2. Open Headset settings, then Developer mode", ""));
    group.add(&row(
        "3. Turn Developer mode on",
        "Restart the headset if the setting does not seem to take.",
    ));
    page.add(&group);

    let help = adw::PreferencesGroup::new();
    let missing = row(
        "No Developer mode switch?",
        "Meta shows it only to accounts that belong to a developer organisation. Creating one is free.",
    );
    let link = gtk::LinkButton::builder()
        .label("Meta's Guide")
        .uri(quest_adb::DEVELOPER_MODE_DOCS)
        .valign(gtk::Align::Center)
        .build();
    missing.add_suffix(&link);
    help.add(&missing);
    page.add(&help);

    let next = gtk::Button::with_label("Continue");
    let w2 = w.clone();
    next.connect_clicked(move |_| push_connect(&w2));
    w.dialog
        .push_subpage(&step("Turn On Developer Mode", &page, Some(&next)));
}

// --- 3. Connect -----------------------------------------------------------

/// The status row's words for each state the headset can be in.
fn describe(state: &Result<Headset, String>) -> (String, String) {
    match state {
        Ok(Headset::NotFound) => (
            "No headset found".into(),
            "Plug the headset into this computer and wake it by putting it on or pressing the power \
             button."
                .into(),
        ),
        Ok(Headset::Unauthorized(_)) => (
            "Waiting for you to allow this computer".into(),
            "Put the headset on. Choose Allow on \"Allow USB debugging?\", and tick \"Always allow \
             from this computer\" so you are not asked again."
                .into(),
        ),
        Ok(Headset::NotReady(d)) => (
            format!("The headset is connected but {}", d.state),
            "Unplug it and plug it back in. If it says \"no permissions\", your system needs udev \
             rules for Android devices (often a package called android-udev-rules)."
                .into(),
        ),
        Ok(Headset::Several(devices)) => (
            format!("{} Android devices are connected", devices.len()),
            "Unplug the ones that are not your Quest, so Cordial copies from the right one.".into(),
        ),
        Ok(Headset::Ready(d)) => (
            format!("{} is ready", d.model.as_deref().unwrap_or("The headset")),
            "This computer is allowed. Continue.".into(),
        ),
        Err(e) => ("adb could not be asked".into(), e.clone()),
    }
}

fn push_connect(w: &Rc<Wizard>) {
    let page = adw::PreferencesPage::builder()
        .description("Plug the headset into this computer with the cable and put it on.")
        .build();
    let group = adw::PreferencesGroup::new();
    let status = row("Looking for the headset…", "");
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .valign(gtk::Align::Center)
        .build();
    status.add_prefix(&spinner);
    group.add(&status);
    page.add(&group);

    // What fixed a missing prompt on the machine this was built on, in the
    // order worth trying.
    let trouble = adw::PreferencesGroup::builder()
        .title("No prompt in the headset?")
        .description("Try these in order. The status above updates by itself.")
        .build();
    let restart = row("Restart adb", "Clears a stuck connection: adb kill-server.");
    let restart_button = gtk::Button::builder()
        .label("Restart")
        .valign(gtk::Align::Center)
        .build();
    restart.add_suffix(&restart_button);
    trouble.add(&restart);
    trouble.add(&row("Unplug the cable and plug it back in", ""));
    trouble.add(&row(
        "Revoke and ask again",
        "In the headset: Settings, System, Developer, revoke USB debugging authorisations. Then replug.",
    ));
    trouble.add(&row(
        "Check developer mode is still on",
        "It can turn itself off after a headset update.",
    ));
    trouble.add(&row(
        "Use a port on the computer itself",
        "Not a hub or a monitor's USB port.",
    ));
    page.add(&trouble);

    let next = gtk::Button::with_label("Continue");
    next.set_sensitive(false);

    let alive = Rc::new(Cell::new(false));
    let asking = Rc::new(Cell::new(false));
    let ask: Rc<dyn Fn()> = {
        let w = w.clone();
        let status = status.clone();
        let spinner = spinner.clone();
        let next = next.clone();
        let trouble = trouble.clone();
        let asking = asking.clone();
        Rc::new(move || {
            let Some(adb) = w.adb.borrow().clone() else {
                return;
            };
            if asking.replace(true) {
                return;
            }
            let w = w.clone();
            let status = status.clone();
            let spinner = spinner.clone();
            let next = next.clone();
            let trouble = trouble.clone();
            let asking = asking.clone();
            crate::updater::on_worker_reporting(
                move |_: &dyn Fn(())| quest_adb::probe(adb.as_ref()),
                |_| {},
                move |state| {
                    asking.set(false);
                    let (title, subtitle) = describe(&state);
                    status.set_title(&title);
                    status.set_subtitle(&subtitle);
                    let ready = matches!(&state, Ok(Headset::Ready(_)));
                    spinner.set_visible(!ready);
                    trouble.set_visible(!ready);
                    next.set_sensitive(ready);
                    *w.device.borrow_mut() = match state {
                        Ok(Headset::Ready(d)) => Some(d),
                        _ => None,
                    };
                },
            );
        })
    };
    {
        let w = w.clone();
        let ask = ask.clone();
        restart_button.connect_clicked(move |b| {
            let Some(adb) = w.adb.borrow().clone() else {
                return;
            };
            b.set_sensitive(false);
            let b = b.clone();
            let ask = ask.clone();
            crate::updater::on_worker_reporting(
                move |_: &dyn Fn(())| adb.run(&["kill-server"]),
                |_| {},
                move |_| {
                    b.set_sensitive(true);
                    ask();
                },
            );
        });
    }
    let w2 = w.clone();
    next.connect_clicked(move |_| push_roblox(&w2));

    let nav = step("Connect the Headset", &page, Some(&next));
    // Asked while the step is on screen and not otherwise: polling a USB bus
    // behind somebody's back once they have moved on is a cost with no reader.
    {
        let alive = alive.clone();
        let ask = ask.clone();
        nav.connect_shown(move |_| {
            if alive.replace(true) {
                return;
            }
            ask();
            let alive = alive.clone();
            let ask = ask.clone();
            glib::timeout_add_local(POLL, move || {
                if !alive.get() {
                    return glib::ControlFlow::Break;
                }
                ask();
                glib::ControlFlow::Continue
            });
        });
    }
    nav.connect_hidden(move |_| alive.set(false));
    w.dialog.push_subpage(&nav);
}

// --- 4. Roblox on the headset ---------------------------------------------

fn push_roblox(w: &Rc<Wizard>) {
    let page = adw::PreferencesPage::builder()
        .description(
            "Cordial looks for Roblox on the headset and copies the version installed there.",
        )
        .build();
    let group = adw::PreferencesGroup::new();
    let status = row("Looking for Roblox on the headset…", "");
    let check = gtk::Button::builder()
        .label("Check Again")
        .valign(gtk::Align::Center)
        .build();
    status.add_suffix(&check);
    group.add(&status);
    page.add(&group);

    let copy = gtk::Button::with_label("Copy to This Computer");
    copy.set_sensitive(false);
    let found: Rc<RefCell<Vec<String>>> = Rc::default();

    let look: Rc<dyn Fn()> = {
        let w = w.clone();
        let status = status.clone();
        let copy = copy.clone();
        let found = found.clone();
        Rc::new(move || {
            let (Some(adb), Some(device)) = (w.adb.borrow().clone(), w.device.borrow().clone())
            else {
                status.set_title("The headset is not connected");
                status.set_subtitle("Go back a step and connect it.");
                return;
            };
            status.set_title("Looking for Roblox on the headset…");
            status.set_subtitle("");
            copy.set_sensitive(false);
            let status = status.clone();
            let copy = copy.clone();
            let found = found.clone();
            crate::updater::on_worker_reporting(
                move |_: &dyn Fn(())| quest_adb::roblox(adb.as_ref(), &device.serial),
                |_| {},
                move |answer| match answer {
                    Ok(Roblox::Installed { version, paths }) => {
                        status.set_title(&match &version {
                            Some(v) => format!("Roblox {v} is on your headset"),
                            None => "Roblox is on your headset".into(),
                        });
                        let kept = quest::current().map(|e| e.version);
                        let files = format!(
                            "{} file{} to copy, about 150 MB.",
                            paths.len(),
                            if paths.len() == 1 { "" } else { "s" }
                        );
                        // Compared before copying, because the commonest
                        // reason to be here is Roblox refusing an old build,
                        // and copying the same old build again fixes nothing.
                        let (subtitle, worth_copying) = match quest_adb::freshness(kept.as_deref(), version.as_deref()) {
                            Freshness::Same => (
                                format!(
                                    "Cordial already has this version. If Roblox said it is out of date, \
                                     update Roblox on the headset from the Meta Horizon Store first, then \
                                     check again."
                                ),
                                false,
                            ),
                            Freshness::HeadsetNewer => (
                                format!("Newer than the {} Cordial has. {files}", kept.unwrap_or_default()),
                                true,
                            ),
                            Freshness::HeadsetOlder => (
                                format!(
                                    "Older than the {} Cordial already has. Update Roblox on the headset first.",
                                    kept.unwrap_or_default()
                                ),
                                false,
                            ),
                            Freshness::NothingKept | Freshness::Unknown => (files, true),
                        };
                        status.set_subtitle(&subtitle);
                        *found.borrow_mut() = paths;
                        copy.set_sensitive(worth_copying);
                    }
                    Ok(Roblox::NotInstalled) => {
                        status.set_title("Roblox is not installed on this headset");
                        status.set_subtitle("Install it from the Meta Horizon Store on the headset, then check again.");
                    }
                    Err(e) => {
                        status.set_title("The headset did not answer");
                        status
                            .set_subtitle(&format!("{e}\nCheck it is still plugged in and awake."));
                    }
                },
            );
        })
    };
    {
        let look = look.clone();
        check.connect_clicked(move |_| look());
    }
    {
        let w = w.clone();
        copy.connect_clicked(move |_| push_copy(&w, found.borrow().clone()));
    }
    let nav = step("Roblox on Your Quest", &page, Some(&copy));
    nav.connect_shown(move |_| look());
    w.dialog.push_subpage(&nav);
}

// --- 5. Copy --------------------------------------------------------------

fn push_copy(w: &Rc<Wizard>, paths: Vec<String>) {
    let page = adw::PreferencesPage::builder()
        .description("Keep the headset plugged in until this finishes.")
        .build();
    let group = adw::PreferencesGroup::new();
    let status = row("Copying from the headset…", "");
    group.add(&status);
    let bar = gtk::ProgressBar::builder().margin_top(12).build();
    group.add(&bar);
    page.add(&group);
    let retry = gtk::Button::with_label("Try Again");
    retry.set_visible(false);

    let run: Rc<dyn Fn()> = {
        let w = w.clone();
        let status = status.clone();
        let bar = bar.clone();
        let retry = retry.clone();
        Rc::new(move || {
            let (Some(adb), Some(device)) = (w.adb.borrow().clone(), w.device.borrow().clone())
            else {
                return;
            };
            retry.set_visible(false);
            status.set_title("Copying from the headset…");
            status.set_subtitle("");
            bar.set_fraction(0.0);
            let paths = paths.clone();
            let status_p = status.clone();
            let bar_p = bar.clone();
            let status_d = status.clone();
            let retry = retry.clone();
            let w = w.clone();
            crate::updater::on_worker_reporting(
                move |report: &dyn Fn((f64, String))| {
                    let staging = quest::root().join(format!(".pulling.{}", std::process::id()));
                    let _ = std::fs::remove_dir_all(&staging);
                    let result = quest_adb::pull(
                        adb.as_ref(),
                        &device.serial,
                        &paths,
                        &staging,
                        &|done, of| {
                            // The copy is most of the time; checking and filing the rest.
                            report((
                                0.8 * done as f64 / of.max(1) as f64,
                                format!("Copying file {} of {of}…", (done + 1).min(of)),
                            ))
                        },
                    )
                    .and_then(|files| {
                        report((
                            0.85,
                            "Checking Roblox's signature and filing the build…".into(),
                        ));
                        quest::import_files(
                            &files,
                            &quest::root(),
                            &cordial_update::apk_signature::pinned_quest(),
                        )
                        .map_err(|e| e.to_string())
                    });
                    let _ = std::fs::remove_dir_all(&staging);
                    result
                },
                move |(fraction, line): (f64, String)| {
                    bar_p.set_fraction(fraction);
                    status_p.set_title(&line);
                },
                move |result| match result {
                    Ok(version) => {
                        (w.on_imported)(version.clone());
                        push_done(&w, &version);
                    }
                    Err(e) => {
                        status_d.set_title("The copy did not finish");
                        status_d.set_subtitle(&format!(
                            "{e}\nNothing was changed. Check the cable and try again."
                        ));
                        retry.set_visible(true);
                    }
                },
            );
        })
    };
    {
        let run = run.clone();
        retry.connect_clicked(move |_| run());
    }
    let nav = step("Copying Roblox", &page, Some(&retry));
    w.dialog.push_subpage(&nav);
    run();
}

// --- 6. Done --------------------------------------------------------------

fn push_done(w: &Rc<Wizard>, version: &str) {
    let page = adw::PreferencesPage::builder().build();
    let group = adw::PreferencesGroup::new();
    group.add(&row(
        &format!("Roblox {version} is ready for Play in VR"),
        "You can unplug the headset.",
    ));
    group.add(&row(
        "When Roblox updates on your Quest",
        "Come back to Settings, VR, Get It from Your Quest. Once this computer is allowed, updating \
         is one click.",
    ));
    page.add(&group);
    let done = gtk::Button::with_label("Done");
    let dialog = w.dialog.clone();
    done.connect_clicked(move |_| while dialog.pop_subpage() {});
    w.dialog.push_subpage(&step("Done", &page, Some(&done)));
}
