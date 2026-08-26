//! The bar and the line of status shown while Cordial fetches the Roblox build.
//!
//! Same primitives as `window::starting_dialog` -- a bar and a line under it --
//! because they are the same kind of moment and two different-looking "please
//! wait" treatments in one program is one too many.
//!
//! **It is a widget and not a window.** It began as a second modal dialog, and
//! that was one window too many in both places it appeared: the first-run
//! screen already has a button somebody just pressed and a line under it, and
//! the changelog window already has a status line above its button. Opening a
//! separate box in front of either says nothing the space already there could
//! not, and leaves the user with two windows to dismiss for one action. So the
//! button is replaced in place by the bar, and the screen the download was
//! started from is the screen that reports it.
//!
//! **The bar is determinate, and that is the difference that matters.** The
//! starting dialog pulses, and its comment is emphatic about why: the shell
//! holds a pid and not a progress channel, so a bar that filled at a made-up
//! rate would be inventing a measurement. Here the number is real. The provider
//! reports bytes as they arrive and the server's `Content-Length` when it gave
//! one, so this fills against something measured and says so in units somebody
//! can check against their own network.
//!
//! When the server did not declare a length the bar goes back to pulsing rather
//! than guessing a denominator. That case is rare on this path and it is the
//! same rule from the other side: show what is known, never a plausible number
//! that is not.
//!
//! The estimate is deliberately crude -- bytes so far over seconds so far, which
//! is an average and not a forecast. It is honest about being one by only
//! appearing once there is enough of a sample to be worth anything, and by
//! rounding to whole minutes past sixty seconds. A countdown that ticks
//! backwards precisely is a promise; this is an indication.

use libadwaita::gtk;
use libadwaita::gtk::glib;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use cordial_update::provider::Progress as Step;

const MIB: f64 = 1_048_576.0;

pub struct Meter {
    root: gtk::Box,
    bar: gtk::ProgressBar,
    status: gtk::Label,
    started: Instant,
    /// When the first byte arrived, so a slow provider lookup beforehand does
    /// not drag the average down and produce an estimate that is wrong in the
    /// pessimistic direction for the whole transfer.
    transfer_began: RefCell<Option<Instant>>,
    pulse: RefCell<Option<glib::SourceId>>,
}

/// `1.2 GB`, `229 MB`, `812 kB`. Decimal units, because that is what a browser
/// and an ISP quote and the point of this line is to be comparable with those.
fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.0} MB", n / 1e6)
    } else {
        format!("{:.0} kB", n / 1e3)
    }
}

fn remaining(seconds: f64) -> String {
    if seconds < 10.0 {
        "a moment".into()
    } else if seconds < 60.0 {
        format!("{} seconds", (seconds / 5.0).round() as u64 * 5)
    } else if seconds < 90.0 {
        "about a minute".into()
    } else {
        format!("about {} minutes", (seconds / 60.0).round() as u64)
    }
}

impl Meter {
    /// A bar and a line, hidden until [`Meter::start`] is called.
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.set_visible(false);

        let bar = gtk::ProgressBar::new();
        bar.set_hexpand(true);
        root.append(&bar);

        let status = gtk::Label::new(Some(""));
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.set_wrap(true);
        status.set_justify(gtk::Justification::Center);
        root.append(&status);

        Rc::new(Meter {
            root,
            bar,
            status,
            started: Instant::now(),
            transfer_began: RefCell::new(None),
            pulse: RefCell::new(None),
        })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Show it, and start pulsing.
    ///
    /// Pulsing until the first byte arrives, because looking for a build and
    /// opening a connection take a moment, and a bar frozen at zero during it
    /// reads as a download that failed to start.
    pub fn start(&self) {
        self.root.set_visible(true);
        self.status.set_label("Starting…");
        self.status.remove_css_class("error");
        if self.pulse.borrow().is_none() {
            let id = glib::timeout_add_local(std::time::Duration::from_millis(80), {
                let bar = self.bar.clone();
                move || {
                    bar.pulse();
                    glib::ControlFlow::Continue
                }
            });
            *self.pulse.borrow_mut() = Some(id);
        }
    }

    fn stop_pulsing(&self) {
        if let Some(id) = self.pulse.borrow_mut().take() {
            id.remove();
        }
    }

    pub fn step(&self, step: &Step) {
        match step {
            Step::Asking { .. } => {
                // The provider is not named. Which mirror answered is a fact
                // about Cordial's plumbing, and this line is being read by
                // somebody who wants their game.
                self.status.set_label("Looking for the latest build…");
            }
            Step::Fetching { done, total, .. } => {
                if self.transfer_began.borrow().is_none() {
                    *self.transfer_began.borrow_mut() = Some(Instant::now());
                }
                let elapsed = self
                    .transfer_began
                    .borrow()
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or_default();

                match total {
                    Some(total) if *total > 0 => {
                        self.stop_pulsing();
                        let fraction = (*done as f64 / *total as f64).clamp(0.0, 1.0);
                        self.bar.set_fraction(fraction);

                        let left = total.saturating_sub(*done);
                        let mut line = format!(
                            "{:.0}% — {} of {}, {} to go",
                            fraction * 100.0,
                            bytes(*done),
                            bytes(*total),
                            bytes(left)
                        );
                        // Only once there is a sample worth dividing by. Three
                        // seconds of a transfer that has not reached its steady
                        // rate produces an estimate off by an order of
                        // magnitude, and a wrong number here is worse than none.
                        if elapsed > 3.0 && *done > 0 {
                            let rate = *done as f64 / elapsed;
                            if rate > 1024.0 {
                                line.push_str(&format!(
                                    "\n{:.1} MB/s, {} remaining",
                                    rate / MIB,
                                    remaining(left as f64 / rate)
                                ));
                            }
                        }
                        self.status.set_label(&line);
                    }
                    // No declared length. Keep pulsing and count what has
                    // arrived, rather than inventing a denominator.
                    _ => self.status.set_label(&format!("{} downloaded", bytes(*done))),
                }
            }
            Step::Verifying { .. } => {
                self.stop_pulsing();
                self.bar.set_fraction(1.0);
                // The file name is not shown. "Checking split_config.x86_64.apk
                // is signed by Roblox" was on screen for one measured run, and
                // it is Cordial's vocabulary rather than anybody else's.
                self.status.set_label("Checking it was signed by Roblox…");
            }
        }
    }

    pub fn finish(&self, version: &str) {
        self.stop_pulsing();
        self.bar.set_fraction(1.0);
        self.status.set_label(&format!("Installed Roblox {version}"));
    }

    /// Stopped by the user. Not a failure, and deliberately not styled as one.
    ///
    /// The bar goes back to empty rather than staying where it got to, because
    /// a part-full bar next to an idle button reads as a download still in
    /// progress -- and nothing was kept: a cancelled fetch removes what it
    /// wrote, exactly as a failed one does.
    pub fn stopped(&self) {
        self.stop_pulsing();
        self.bar.set_fraction(0.0);
        self.status.remove_css_class("error");
        self.status.set_label("Download stopped.");
    }

    /// A failure, said in full and left on screen.
    pub fn failed(&self, why: &str) {
        self.stop_pulsing();
        self.bar.set_fraction(0.0);
        self.status.set_label(why);
        self.status.add_css_class("error");
    }

    pub fn elapsed_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_a_download_is_usually_quoted() {
        assert_eq!(bytes(229_140_095), "229 MB");
        assert_eq!(bytes(1_500_000_000), "1.5 GB");
        assert_eq!(bytes(812_000), "812 kB");
    }

    /// **No false precision.** An estimate that says "3 minutes 47 seconds"
    /// is claiming a forecast from an average, and it will be wrong in a way
    /// the user can watch happen.
    #[test]
    fn the_estimate_gets_vaguer_as_it_gets_longer() {
        assert_eq!(remaining(4.0), "a moment");
        assert_eq!(remaining(32.0), "30 seconds");
        assert_eq!(remaining(70.0), "about a minute");
        assert_eq!(remaining(400.0), "about 7 minutes");
    }
}
