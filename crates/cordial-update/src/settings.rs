//! One dropdown and two toggles, and why it is no longer two dropdowns.
//!
//! **Auto update** — update in the background, ask, or manual. **Download on
//! Wi-Fi** and **Download on metered connection** — one switch each.
//!
//! ## The two-dropdown shape this replaces, and why it went
//!
//! This module used to be two enums, and its header argued the case at length:
//! three controls that can each be set on their own produce combinations with no
//! defined meaning, and "update in the background, but never download" is a
//! setting that has to either lie or explain itself. That reasoning is kept here
//! rather than deleted, because it was not wrong and because the combination it
//! warned about is now reachable: **both connection switches off, with Auto
//! update on background, can never download anything.**
//!
//! The owner specified the dropdown-plus-two-toggles shape, twice, and it is
//! their call. What the old argument was really objecting to was expressing a
//! contradiction *silently*, so the contradiction is now something this module
//! names — [`DownloadOn::never_downloads`] is the whole of it, the shell puts
//! that sentence on the settings page as a warning, and
//! [`UpdateSettings::may_download`] gives the same reason back when a download is
//! held. A user who sets it that way is told what they have set. That is a
//! different thing from a settings page that quietly means nothing.
//!
//! [`UpdateSettings::plan`] is still total, which is the property the old shape
//! was chosen for: every combination of the three controls and every one of
//! NetworkManager's four answers maps to exactly one [`Plan`], and
//! `every_setting_combination_has_exactly_one_plan` is the test that would have
//! to be told what a fourth control meant.
//!
//! ## Wi-Fi is not a thing Cordial can see, and the row says so
//!
//! There is no radio in this. `org.freedesktop.NetworkManager`'s `Metered`
//! property is the only question [`crate::metered`] asks, and it is a statement
//! about who pays for the bytes rather than about the link layer. So *Download
//! on Wi-Fi* governs the not-metered case and *Download on metered connection*
//! governs the metered one; between them they cover every connection, which is
//! exactly what makes "both off" mean "never".
//!
//! A wired desktop therefore takes the Wi-Fi switch's branch. That is the right
//! answer to the question the switch is really asking — may this download over a
//! connection nobody is charging by the megabyte — and a poor reading of its
//! name, so the shell's row carries the explanation rather than leaving somebody
//! to find out by unplugging something.
//!
//! Nothing here is GTK. The shell binds an `AdwComboRow` to [`Automatic::index`],
//! the same seam `shell_config::AppearanceScheme` already uses, so the position
//! in the model is the one thing that has to agree between the two files rather
//! than a name-keyed lookup that can drift from the model's actual contents.

use crate::metered::Metered;
use serde::{Deserialize, Serialize};

/// What Cordial does about updates without being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Automatic {
    /// Check, and fetch what it finds, subject to [`DownloadOn`].
    Background,
    /// Check, and when Cordial starts with an update waiting, open the changelog
    /// and wait to be told. A dialog on launch rather than a badge: the point of
    /// asking is that somebody is asked.
    Ask,
    /// No check of any kind without a button being pressed.
    ///
    /// **Manual, not "disabled".** It turns off every automatic behaviour and it
    /// does not turn the feature off, because the header-bar button still checks
    /// on demand — it becomes a refresh control. Turning off automatic checking
    /// is a statement about background network use, not a refusal to ever know,
    /// and "disabled" says the second thing.
    Manual,
}

impl Default for Automatic {
    /// Background, because a stale client is not a working client: Roblox
    /// refuses old builds server-side, so this is not a convenience setting and
    /// the default that leaves somebody broken on Roblox's schedule is the
    /// wrong one. It is a safe default only because [`DownloadOn`] leaves the
    /// metered switch off — the pair is the decision, not either half.
    fn default() -> Self {
        Automatic::Background
    }
}

impl Automatic {
    /// Order matches the `AdwComboRow` model in the shell.
    pub fn index(self) -> u32 {
        match self {
            Automatic::Background => 0,
            Automatic::Ask => 1,
            Automatic::Manual => 2,
        }
    }

    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Automatic::Background,
            1 => Automatic::Ask,
            _ => Automatic::Manual,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Automatic::Background => "Update in background",
            Automatic::Ask => "Ask",
            Automatic::Manual => "Manual",
        }
    }
}

/// Which connections a download may use. One switch each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DownloadOn {
    /// *Download on Wi-Fi*, which is really "on a connection nobody is charging
    /// by the megabyte" — see the module header. Only an explicit `NO` from
    /// NetworkManager takes this branch; every guess is metered.
    pub wifi: bool,
    /// *Download on metered connection.* Off by default: reading a guess as
    /// cheap is how somebody's data allowance pays for a 115 MB download they
    /// never asked for, and the cost of being wrong the other way is a button
    /// press.
    pub metered: bool,
}

impl Default for DownloadOn {
    fn default() -> Self {
        DownloadOn { wifi: true, metered: false }
    }
}

/// The sentence the settings page shows when both switches are off.
///
/// A constant rather than a formatted string, so the shell's warning row and the
/// refusal a held download reports are demonstrably the same words.
pub const NEVER_DOWNLOADS: &str =
    "With both connection switches off, nothing is ever downloaded on its own, whatever Auto \
     update is set to above — Wi-Fi and metered are every connection there is. Pressing Update \
     yourself still works: these switches govern what happens without being asked.";

impl DownloadOn {
    /// Whether this pair rules out every connection.
    ///
    /// The contradiction the two-dropdown shape existed to make unexpressible.
    /// It is expressible now, so it is named instead — expressing it is fine,
    /// and expressing it silently is what the old design was objecting to.
    pub fn never_downloads(self) -> bool {
        !self.wifi && !self.metered
    }
}

/// All three controls. Serialised into whatever document the shell keeps its own
/// preferences in; nothing here reads or writes a file, because where Cordial's
/// settings live is `shell_config`'s answer and ADR-013's, not this crate's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct UpdateSettings {
    pub automatic: Automatic,
    pub download_on: DownloadOn,
}

/// What should happen on launch, given the settings and the connection.
///
/// Total by construction: every combination of settings values maps to exactly
/// one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Check in the background and fetch whatever it finds.
    CheckAndDownload,
    /// Check in the background, then wait to be asked.
    ///
    /// `why` is `Some` when the reason for waiting is the connection or the
    /// switches rather than the mode, so the button can say which — "waiting
    /// because you asked to be asked", "waiting because you are on a hotspot"
    /// and "waiting because you turned both connections off" are three different
    /// things to be looking at.
    CheckAndAsk { why: Option<String> },
    /// No request of any kind. The button offers a manual check.
    DoNotCheck,
}

impl UpdateSettings {
    pub fn plan(self, metered: Metered) -> Plan {
        match self.automatic {
            Automatic::Manual => Plan::DoNotCheck,
            Automatic::Ask => Plan::CheckAndAsk { why: None },
            Automatic::Background => match self.may_download(metered) {
                Ok(()) => Plan::CheckAndDownload,
                Err(why) => Plan::CheckAndAsk { why: Some(why) },
            },
        }
    }

    /// Whether the expensive half may run right now, or why not.
    ///
    /// The refusal is a sentence rather than a flag because it ends up in front
    /// of somebody wondering why their update is sitting there, and "metered"
    /// on its own invites "no it isn't" — [`Metered::describe`] says which of
    /// NetworkManager's four answers it was.
    pub fn may_download(self, metered: Metered) -> Result<(), String> {
        if self.download_on.never_downloads() {
            return Err(NEVER_DOWNLOADS.to_string());
        }
        let allowed =
            if metered.is_metered() { self.download_on.metered } else { self.download_on.wifi };
        if allowed {
            return Ok(());
        }
        Err(format!(
            "{}, and {} is off",
            metered.describe(),
            if metered.is_metered() { "Download on metered connection" } else { "Download on Wi-Fi" }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_ones_the_design_names() {
        let s = UpdateSettings::default();
        assert!(s.download_on.wifi);
        assert!(!s.download_on.metered, "a data allowance is not the default to spend");
        assert_eq!(s.automatic, Automatic::Background);
    }

    #[test]
    fn manual_means_no_request_whatever_the_network_is() {
        // Manual is about background network use, so a fast unmetered connection
        // must not talk it out of the setting. What it does not mean is "never
        // know": the header-bar button still checks on demand, which is the
        // whole reason the option is not called Disabled.
        let s = UpdateSettings {
            automatic: Automatic::Manual,
            download_on: DownloadOn { wifi: true, metered: true },
        };
        assert_eq!(s.plan(Metered::No), Plan::DoNotCheck);
        assert_eq!(s.plan(Metered::Yes), Plan::DoNotCheck);
    }

    #[test]
    fn background_over_a_metered_connection_waits_and_says_why() {
        // Still checking, not yet downloading, and naming which of the four
        // answers it got.
        let s = UpdateSettings::default();
        match s.plan(Metered::GuessYes) {
            Plan::CheckAndAsk { why: Some(why) } => {
                assert!(why.contains("Download on metered connection"), "{why}");
                assert!(why.contains("guess"), "{why}");
            }
            other => panic!("expected a reason, got {other:?}"),
        }
    }

    #[test]
    fn both_switches_off_is_a_contradiction_that_is_stated_rather_than_hidden() {
        // The combination the old two-dropdown shape existed to make
        // unexpressible. It is expressible now, so the requirement moved: it has
        // to be said out loud, in the same words, wherever it is met.
        let s = UpdateSettings {
            automatic: Automatic::Background,
            download_on: DownloadOn { wifi: false, metered: false },
        };
        assert!(s.download_on.never_downloads());
        for metered in [Metered::No, Metered::Yes, Metered::GuessNo, Metered::Unknown] {
            assert_eq!(s.may_download(metered), Err(NEVER_DOWNLOADS.to_string()));
        }
        assert!(NEVER_DOWNLOADS.contains("nothing is ever downloaded"), "{NEVER_DOWNLOADS}");
        assert!(NEVER_DOWNLOADS.contains("whatever Auto update is set to"), "{NEVER_DOWNLOADS}");
        // And it does not overstate itself: an Update the user presses is not
        // what these switches are about, and saying otherwise would be a second
        // wrong sentence rather than a fix for the first.
        assert!(NEVER_DOWNLOADS.contains("Pressing Update yourself still works"), "{NEVER_DOWNLOADS}");
    }

    #[test]
    fn background_over_an_unmetered_connection_downloads() {
        assert_eq!(UpdateSettings::default().plan(Metered::No), Plan::CheckAndDownload);
    }

    #[test]
    fn a_metered_connection_is_allowed_only_by_its_own_switch() {
        // The switches are not interchangeable: turning Wi-Fi on must not pay
        // for a hotspot, and turning metered on must not be needed for a LAN.
        let hotspot_only =
            UpdateSettings { automatic: Automatic::Background, download_on: DownloadOn { wifi: false, metered: true } };
        assert_eq!(hotspot_only.plan(Metered::Yes), Plan::CheckAndDownload);
        assert!(hotspot_only.may_download(Metered::No).is_err());

        let both = UpdateSettings {
            automatic: Automatic::Background,
            download_on: DownloadOn { wifi: true, metered: true },
        };
        assert_eq!(both.plan(Metered::Yes), Plan::CheckAndDownload);
        assert_eq!(both.plan(Metered::Unknown), Plan::CheckAndDownload);
    }

    #[test]
    fn a_guess_takes_the_metered_switch_rather_than_the_wifi_one() {
        // The consequence nobody expects: an ordinary desktop on a LAN answers
        // guess-no, which is metered, so the default settings hold the download
        // there. Deleting this is how "guesses are metered" quietly becomes
        // "guesses go the way they lean".
        let s = UpdateSettings::default();
        assert!(s.may_download(Metered::GuessNo).is_err());
    }

    #[test]
    fn asking_first_is_not_overridden_by_a_cheap_connection() {
        let s = UpdateSettings {
            automatic: Automatic::Ask,
            download_on: DownloadOn { wifi: true, metered: true },
        };
        assert_eq!(s.plan(Metered::No), Plan::CheckAndAsk { why: None });
    }

    #[test]
    fn every_setting_combination_has_exactly_one_plan() {
        // The property the old two-dropdown shape was chosen for, kept through
        // the change to a dropdown and two switches. If a fourth control were
        // ever added, this is the test that would have to be told what the new
        // combinations mean.
        for automatic in [Automatic::Background, Automatic::Ask, Automatic::Manual] {
            for wifi in [true, false] {
                for metered_switch in [true, false] {
                    for metered in [Metered::No, Metered::Yes, Metered::GuessNo, Metered::Unknown] {
                        let s = UpdateSettings {
                            automatic,
                            download_on: DownloadOn { wifi, metered: metered_switch },
                        };
                        // Total: no panic, no "undefined" arm to fall into.
                        let _ = s.plan(metered);
                    }
                }
            }
        }
    }

    #[test]
    fn the_combo_indices_round_trip() {
        for a in [Automatic::Background, Automatic::Ask, Automatic::Manual] {
            assert_eq!(Automatic::from_index(a.index()), a);
        }
    }

    #[test]
    fn the_third_option_is_named_manual_rather_than_disabled() {
        // The word is the setting's meaning. Manual turns off every automatic
        // behaviour and leaves the header-bar button checking on demand;
        // "Disabled" would claim the feature was gone.
        assert_eq!(Automatic::Manual.label(), "Manual");
        assert_eq!(Automatic::Background.label(), "Update in background");
        assert_eq!(Automatic::Ask.label(), "Ask");
    }

    #[test]
    fn the_stored_form_survives_a_round_trip_and_an_absent_key() {
        let s = UpdateSettings {
            automatic: Automatic::Ask,
            download_on: DownloadOn { wifi: false, metered: true },
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"ask\""), "{text}");
        assert_eq!(serde_json::from_str::<UpdateSettings>(&text).unwrap(), s);
        // A settings file written before this existed must not refuse to load.
        assert_eq!(serde_json::from_str::<UpdateSettings>("{}").unwrap(), UpdateSettings::default());
        // Nor one written while the switches were still a single enum.
        assert_eq!(
            serde_json::from_str::<UpdateSettings>(r#"{"automatic":"manual"}"#).unwrap().automatic,
            Automatic::Manual
        );
    }
}
