use libadwaita as adw;
use libadwaita::glib;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::browser_account::{self, LaunchTicket, ProfileMatch};
use crate::deep_link;
use crate::shell_config::ShellConfig;

/// A desktop join held until the browser account resolves or the user launches.
///
/// The banner is not decoration either. A link that vanished into a variable
/// would be indistinguishable from a link that was dropped, and "it ignored my
/// click" is the report that follows.
#[derive(Clone)]
pub struct PendingJoin {
    url: Rc<RefCell<Option<Rc<String>>>>,
    profile_match: Rc<RefCell<Option<ProfileMatch>>>,
    banner: adw::Banner,
}

impl PendingJoin {
    pub(super) fn new() -> Self {
        let banner = adw::Banner::builder()
            .revealed(false)
            .button_label("Discard")
            .build();
        let url = Rc::new(RefCell::new(None));
        let profile_match = Rc::new(RefCell::new(None));
        {
            // A queued join the user cannot get rid of is a trap: the next
            // launch would carry a link they have changed their mind about, and
            // the only way out would be closing the launcher.
            let url = url.clone();
            let profile_match = profile_match.clone();
            banner.connect_button_clicked(move |banner| {
                *url.borrow_mut() = None;
                *profile_match.borrow_mut() = None;
                banner.set_revealed(false);
            });
        }
        PendingJoin {
            url,
            profile_match,
            banner,
        }
    }

    /// Show a link as waiting. Replaces whatever was queued: two links means the
    /// second click is the one the user is looking at.
    pub fn queue(&self, url: String) {
        self.banner.set_title(&banner_line(&url));
        self.banner.set_revealed(true);
        *self.url.borrow_mut() = Some(Rc::new(url));
        *self.profile_match.borrow_mut() = None;
    }

    /// What the next launch should carry, without consuming it: a launch that
    /// fails must leave the link where it was, or a busy profile would cost the
    /// user the link as well as the launch.
    pub(super) fn peek(&self) -> Option<String> {
        self.url.borrow().as_ref().map(|url| url.as_ref().clone())
    }

    pub(super) fn clear(&self) {
        *self.url.borrow_mut() = None;
        *self.profile_match.borrow_mut() = None;
        self.banner.set_revealed(false);
    }

    pub(super) fn invalidate_lookup(&self, selected_profile: &str) {
        if let Some(url) = self.peek() {
            *self.url.borrow_mut() = Some(Rc::new(url));
        }
        if self
            .profile_match
            .borrow()
            .as_ref()
            .is_some_and(|profile_match| profile_match.name() != selected_profile)
        {
            *self.profile_match.borrow_mut() = None;
        }
    }

    pub(super) fn set_profile_match(&self, profile_match: ProfileMatch) {
        *self.profile_match.borrow_mut() = Some(profile_match);
    }

    pub(super) fn matched_profile_is_current(
        &self,
        name: &str,
        profile_dir: &std::path::Path,
    ) -> bool {
        match self.profile_match.borrow().as_ref() {
            Some(profile_match) => profile_match.still_matches(name, profile_dir),
            None => true,
        }
    }

    pub(super) fn clear_profile_match(&self) {
        *self.profile_match.borrow_mut() = None;
    }

    pub(super) fn banner(&self) -> &adw::Banner {
        &self.banner
    }
}

/// What the banner says about a waiting link.
///
/// Pure, and separate from the widget, for the reason `busy_body` is: a string
/// that can only be inspected by building a window and photographing it is a
/// string that drifts. It also has one genuine hazard in it — `AdwBanner`'s
/// title is Pango markup and this text came from a browser, so an unescaped `&`
/// in a query string is enough to make GTK drop the label, and anything sharper
/// is worse than that.
pub(super) fn banner_line(url: &str) -> String {
    let shown = glib::markup_escape_text(&deep_link::summarise(url));
    format!("Roblox link waiting: {shown} — press Roblox to join")
}

/// The running shell, for the handful of things that happen to it from outside.
///
/// `main.rs` holds one so that a second invocation carrying a link — which is
/// what the desktop does when a browser opens `roblox-player://` while Cordial
/// is up — reaches the window that already exists rather than starting another.
pub struct Shell {
    pub(super) window: adw::Window,
    pub(super) join: PendingJoin,
    pub(super) config: Rc<RefCell<ShellConfig>>,
    pub(super) config_path: Rc<PathBuf>,
    pub(super) refresh_profiles: Rc<dyn Fn()>,
}

impl Shell {
    /// Resolve browser joins without opening the picker unless a choice is needed.
    pub fn queue_join(&self, url: String) {
        let _task = self.queue_with_lookup(url, |ticket| {
            browser_account::resolve(ticket).map(Into::into)
        });
    }

    pub(super) fn queue_with_lookup(
        &self,
        url: String,
        lookup: impl FnOnce(LaunchTicket) -> Option<ResolvedProfile> + Send + 'static,
    ) -> Option<glib::JoinHandle<()>> {
        // Reading files while the runtime uses a keyring could match a stale
        // pre-migration identity. This adapter is for the explicit file backend.
        let enabled = std::env::var("CORDIAL_SECRET_STORE").as_deref() == Ok("file")
            && std::env::var("CORDIAL_BROWSER_ACCOUNT_ROUTING").as_deref() != Ok("0");
        let ticket = enabled.then(|| LaunchTicket::parse(&url)).flatten();
        match ticket {
            None => {
                self.join.queue(url);
                self.window.present();
                None
            }
            Some(ticket) => {
                // Strip before sending anything: a timeout cannot tell us whether
                // Roblox consumed the ticket, so even fallback must not reuse it.
                self.join.queue(ticket.join_url.clone());
                self.join.banner.set_title("Checking browser account…");
                let request = self.join.url.borrow().clone();
                let join = self.join.clone();
                let config = self.config.clone();
                let chosen = config.borrow().profile.clone();
                let path = self.config_path.clone();
                let refresh = self.refresh_profiles.clone();
                let window = self.window.downgrade();
                Some(glib::MainContext::default().spawn_local(async move {
                    let result = adw::gtk::gio::spawn_blocking(move || lookup(ticket)).await;
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    let current = join.url.borrow().clone();
                    // Discard, manual launch and a newer browser click invalidate
                    // this exact request, including two links to the same place.
                    let active = match (&request, &current) {
                        (Some(request), Some(current)) => Rc::ptr_eq(request, current),
                        (None, _) | (_, None) => false,
                    };
                    if !active {
                        return;
                    }
                    let selected = match result {
                        Ok(selected) => selected,
                        Err(_) => None,
                    };
                    match selected {
                        Some(profile) if config.borrow().profile == chosen => {
                            config.borrow_mut().profile = profile.name.clone();
                            if let Some(evidence) = profile.evidence {
                                join.set_profile_match(evidence);
                            }
                            crate::settings::persist(&config, &path);
                            refresh();
                            println!("  shell: browser account matched; launching saved profile");
                            if window.activate_action("win.launch", None).is_err() {
                                join.banner
                                    .set_title("Roblox link waiting — press Roblox to join");
                                window.present();
                            }
                        }
                        Some(_) | None => {
                            println!("  shell: browser account routing needs a profile choice");
                            join.banner.set_title(
                                "Roblox link waiting — choose a profile and press Roblox",
                            );
                            window.present();
                        }
                    }
                }))
            }
        }
    }

    /// Bring the launcher forward, for a second invocation carrying nothing.
    pub fn present(&self) {
        self.window.present();
    }
}

pub(super) struct ResolvedProfile {
    name: String,
    evidence: Option<ProfileMatch>,
}

impl From<ProfileMatch> for ResolvedProfile {
    fn from(profile_match: ProfileMatch) -> Self {
        ResolvedProfile {
            name: profile_match.name().to_string(),
            evidence: Some(profile_match),
        }
    }
}

#[cfg(test)]
impl From<&str> for ResolvedProfile {
    fn from(name: &str) -> Self {
        ResolvedProfile {
            name: name.to_string(),
            evidence: None,
        }
    }
}

#[cfg(test)]
#[path = "browser_join_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "browser_visibility_tests.rs"]
mod visibility_tests;
