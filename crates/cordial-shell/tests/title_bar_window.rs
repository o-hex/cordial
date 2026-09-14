use cordial_shell::{host_window::HostWindow, title_bar::TitleBar};
use libadwaita::prelude::*;
use std::time::Duration;

#[test]
#[ignore = "requires a Wayland display; run once per CORDIAL_TITLE_BAR value"]
fn title_bar_preference_controls_real_game_window_chrome() {
    // Given the real game-window widgets with a launch-time preference.
    libadwaita::init().unwrap();
    let choice = TitleBar::from_env();
    let host = HostWindow::with_canvas("Cordial title-bar fixture", 640, 480);
    // When the window is mapped normally, without asking for fullscreen.
    host.present();
    host.wait_until_mapped(Duration::from_secs(5)).unwrap();
    // Then toolbar space follows the preference while the window stays windowed.
    assert!(!host.window().is_fullscreen());
    assert_eq!(host.toolbar().reveals_top_bars(), choice.revealed(false));
    if choice == TitleBar::Hidden {
        assert_eq!(host.toolbar().top_bar_height(), 0);
    } else {
        assert!(host.toolbar().top_bar_height() > 0);
    }
    // Re-deliver the notification used on leaving fullscreen. The saved Hidden
    // choice must still win when the window reports its windowed state.
    host.window().notify("fullscreened");
    assert_eq!(host.toolbar().reveals_top_bars(), choice.revealed(false));
    println!(
        "title-bar fixture: mode={choice:?}, top_bar_height={}, fullscreen={}",
        host.toolbar().top_bar_height(),
        host.window().is_fullscreen()
    );
    host.window().close();
}
