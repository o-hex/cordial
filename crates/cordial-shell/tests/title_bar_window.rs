use cordial_shell::{host_window::HostWindow, title_bar::TitleBar};
use libadwaita::prelude::*;
use std::time::Duration;

#[test]
#[ignore = "requires a Wayland display and CORDIAL_TITLE_BAR=hidden"]
fn hidden_title_bar_only_changes_mapped_game_window() {
    // Given real launcher and game windows in a process launched with Hidden.
    libadwaita::init().unwrap();
    let choice = TitleBar::from_env();
    assert_eq!(choice, TitleBar::Hidden, "launch this test with CORDIAL_TITLE_BAR=hidden");
    let launcher_content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let launcher = HostWindow::new("Cordial launcher fixture", 640, 480, &launcher_content);
    let game = HostWindow::with_canvas("Cordial game fixture", 640, 480);

    // When both windows are mapped normally, without asking for fullscreen.
    launcher.present();
    game.present();
    launcher.wait_until_mapped(Duration::from_secs(5)).unwrap();
    game.wait_until_mapped(Duration::from_secs(5)).unwrap();

    // Then Hidden removes only game chrome; launcher controls remain available.
    assert!(!launcher.window().is_fullscreen());
    assert!(!game.window().is_fullscreen());
    assert!(launcher.toolbar().reveals_top_bars());
    assert!(launcher.toolbar().top_bar_height() > 0);
    assert!(!game.toolbar().reveals_top_bars());
    assert_eq!(game.toolbar().top_bar_height(), 0);
    println!(
        "title-bar fixture: mode={choice:?}, launcher_top_bar_height={}, game_top_bar_height={}",
        launcher.toolbar().top_bar_height(),
        game.toolbar().top_bar_height()
    );
    launcher.window().close();
    game.window().close();
}
