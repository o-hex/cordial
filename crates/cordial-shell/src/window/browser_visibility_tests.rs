use super::*;
use std::cell::Cell;

#[test]
#[ignore = "requires a GTK display; run alone"]
fn matching_browser_account_never_maps_profile_picker() {
    // Given an unopened launcher and a matching account.
    let (shell, launches, _root) = super::tests::fallback_shell();
    let maps = Rc::new(Cell::new(0));
    {
        let maps = maps.clone();
        shell.window.connect_map(move |_| maps.set(maps.get() + 1));
    }
    // When the browser join resolves and invokes the existing launch action.
    let task = shell
        .queue_with_lookup(
            "roblox-player:1+launchmode:play+gameinfo:FAKE+placelauncherurl:x".into(),
            |_| Some("browser-account".into()),
        )
        .unwrap();
    assert!(
        !shell.window.is_visible(),
        "lookup must not reveal the picker"
    );
    glib::MainContext::default().block_on(task).unwrap();
    // Then a player is launched without ever mapping the picker.
    assert_eq!(launches.get(), 1);
    assert_eq!(maps.get(), 0);
    assert!(!shell.window.is_visible());
    shell.window.close();
}

#[test]
#[ignore = "requires a GTK display; run alone"]
fn unmatched_browser_account_reveals_manual_picker() {
    // Given an unopened launcher with no matching saved account.
    let (shell, launches, _root) = super::tests::fallback_shell();
    // When lookup completes without a match.
    let task = shell
        .queue_with_lookup(
            "roblox-player:1+launchmode:play+gameinfo:FAKE+placelauncherurl:x".into(),
            |_| None,
        )
        .unwrap();
    assert!(!shell.window.is_visible());
    glib::MainContext::default().block_on(task).unwrap();
    // Then the user can choose a profile instead of losing the join.
    assert!(shell.window.is_visible());
    assert!(shell.join.peek().is_some());
    assert_eq!(launches.get(), 0);
    shell.window.close();
}

#[test]
#[ignore = "requires a GTK display; run alone with isolated XDG directories"]
fn shell_construction_stays_hidden_until_explicitly_opened() {
    // Given the real application and shell construction path.
    adw::init().unwrap();
    let app = adw::Application::builder()
        .application_id("org.cordial.BrowserVisibilityTest")
        .flags(adw::gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&adw::gtk::gio::Cancellable>).unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = Rc::new(RefCell::new(ShellConfig::default()));
    // When preparing a shell for a browser invocation.
    let shell = super::super::build(&app, config, Rc::new(root.path().join("shell.json")));
    // Then construction does not show it, but a desktop-icon open still does.
    assert!(!shell.window.is_visible());
    shell.present();
    assert!(shell.window.is_visible());
    shell.window.close();
}

#[test]
#[ignore = "requires a GTK display; run alone with isolated XDG directories"]
fn failed_hidden_launch_reveals_recovery_ui() {
    // Given a hidden shell whose selected APK no longer exists.
    let (shell, launches, root) = super::tests::fallback_shell();
    shell.config.borrow_mut().roblox.apk = Some(root.path().join("missing.apk"));
    let toasts = adw::ToastOverlay::new();
    // When the real launch path refuses that installation before spawning.
    super::super::launch_now(
        &shell.window.clone().upcast(),
        &toasts,
        &shell.config,
        &shell.join,
    );
    // Then the recovery dialog has a visible parent.
    assert!(shell.window.is_visible());
    assert_eq!(launches.get(), 0);
    shell.window.close();
}

#[test]
#[ignore = "requires a GTK display; run alone"]
fn hidden_browser_launch_does_not_show_starting_dialog() {
    // Given a hidden picker on the automatic launch path.
    adw::init().unwrap();
    let parent = adw::Window::new();
    // When the normal launch path creates its progress dialog.
    let dialog = super::super::starting_dialog(&parent.clone().upcast(), "fixture", true);
    // Then neither launcher surface appears ahead of the player.
    assert!(dialog.is_none());
    assert!(!parent.is_visible());
    parent.close();
}

#[test]
#[ignore = "requires a GTK display; run alone with isolated XDG directories"]
fn hidden_application_stays_alive_until_browser_launch_dispatch() {
    // Given the real application loop starting with a browser join.
    adw::init().unwrap();
    let root = tempfile::tempdir().unwrap();
    let app = adw::Application::builder()
        .application_id("org.cordial.HiddenLaunchTest")
        .flags(adw::gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let launched = Rc::new(Cell::new(false));
    let revealed = Rc::new(Cell::new(false));
    let maps = Rc::new(Cell::new(0));
    {
        let launched = launched.clone();
        let revealed = revealed.clone();
        let maps = maps.clone();
        let config_path = root.path().join("shell.json");
        app.connect_activate(move |app| {
            let config = Rc::new(RefCell::new(ShellConfig::default()));
            let shell = super::super::build(app, config, Rc::new(config_path.clone()));
            revealed.set(shell.window.is_visible());
            let maps = maps.clone();
            shell.window.connect_map(move |_| maps.set(maps.get() + 1));
            // Record at the player-spawn boundary: no proprietary engine is
            // needed to observe whether the application's picker was mapped.
            let actions = adw::gtk::gio::SimpleActionGroup::new();
            let launch = adw::gtk::gio::SimpleAction::new("launch", None);
            let app = app.clone();
            let launched = launched.clone();
            let revealed = revealed.clone();
            let window = shell.window.clone();
            launch.connect_activate(move |_, _| {
                launched.set(true);
                revealed.set(revealed.get() || window.is_visible());
                window.close();
                app.quit();
            });
            actions.add_action(&launch);
            shell.window.insert_action_group("win", Some(&actions));
            let _task = shell.queue_with_lookup(
                "roblox-player:1+launchmode:play+gameinfo:FAKE+placelauncherurl:x".into(),
                |_| Some("browser-account".into()),
            );
        });
    }
    let timed_out = Rc::new(Cell::new(false));
    let timeout = {
        let app = app.clone();
        let timed_out = timed_out.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || {
            timed_out.set(true);
            app.quit();
        })
    };
    // When GApplication runs normally rather than a manually pumped test context.
    app.run_with_args(&["cordial-hidden-launch-test"]);
    // Then the hidden window keeps it alive for dispatch without flashing a picker.
    assert!(!timed_out.get());
    timeout.remove();
    assert!(launched.get());
    assert!(!revealed.get());
    assert_eq!(maps.get(), 0);
}

#[test]
#[ignore = "requires a GTK display; run alone with isolated XDG directories"]
fn closing_visible_picker_cancels_pending_browser_launch() {
    // Given the full shell with an outstanding account lookup.
    adw::init().unwrap();
    let app = adw::Application::builder()
        .application_id("org.cordial.CloseLookupTest")
        .flags(adw::gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&adw::gtk::gio::Cancellable>).unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = Rc::new(RefCell::new(ShellConfig::default()));
    config.borrow_mut().profile = "last-used".into();
    config.borrow_mut().roblox.apk = Some(root.path().join("missing.apk"));
    let shell = super::super::build(&app, config, Rc::new(root.path().join("shell.json")));
    shell.present();
    let task = shell
        .queue_with_lookup(
            "roblox-player:1+launchmode:play+gameinfo:FAKE+placelauncherurl:x".into(),
            |_| Some("browser-account".into()),
        )
        .unwrap();
    // When the user closes the picker before lookup completion.
    shell.window.close();
    glib::MainContext::default().block_on(task).unwrap();
    // Then the late result neither selects a profile nor reopens the window.
    assert_eq!(shell.config.borrow().profile, "last-used");
    assert!(shell.join.peek().is_none());
    assert!(!shell.window.is_visible());
}
