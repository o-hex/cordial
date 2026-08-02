//! Two dropdowns, and why it is two dropdowns.
//!
//! **Automatic updates** — download in the background, ask first, or never
//! check. **Download over** — any connection, or unmetered connections only,
//! which is the default.
//!
//! Not a mode plus two independent toggles. Three controls that can each be set
//! on their own produce combinations with no defined meaning: "update in the
//! background, but never download" is a setting that has to either lie or
//! explain itself, and a settings page that can express a contradiction will
//! eventually be asked to honour one. Two enums cannot express it — there is no
//! value of either that contradicts a value of the other, and
//! [`UpdateSettings::plan`] is total.
//!
//! Nothing here is GTK. The shell binds an `AdwComboRow` to [`Automatic::index`]
//! and [`DownloadOver::index`], the same seam `shell_config::AppearanceScheme`
//! already uses, so the position in the model is the one thing that has to agree
//! between the two files rather than a name-keyed lookup that can drift from the
//! model's actual contents.

use crate::metered::Metered;
use serde::{Deserialize, Serialize};

/// What Cordial does about updates without being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Automatic {
    /// Check, and fetch what it finds, subject to [`DownloadOver`].
    Background,
    /// Check, then raise the header-bar button's attention state and wait.
    Ask,
    /// No background request of any kind. The header-bar button becomes a
    /// manual refresh — turning off automatic checking is a statement about
    /// background network use, not a refusal to ever know.
    Never,
}

impl Default for Automatic {
    /// Background, because a stale client is not a working client: Roblox
    /// refuses old builds server-side, so this is not a convenience setting and
    /// the default that leaves somebody broken on Roblox's schedule is the
    /// wrong one. It is a safe default only because [`DownloadOver`] defaults to
    /// unmetered — the pair is the decision, not either half.
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
            Automatic::Never => 2,
        }
    }

    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Automatic::Background,
            1 => Automatic::Ask,
            _ => Automatic::Never,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Automatic::Background => "Download in the background",
            Automatic::Ask => "Ask first",
            Automatic::Never => "Never check",
        }
    }
}

/// Which connections a download may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DownloadOver {
    Any,
    UnmeteredOnly,
}

impl Default for DownloadOver {
    fn default() -> Self {
        DownloadOver::UnmeteredOnly
    }
}

impl DownloadOver {
    pub fn index(self) -> u32 {
        match self {
            DownloadOver::Any => 0,
            DownloadOver::UnmeteredOnly => 1,
        }
    }

    pub fn from_index(index: u32) -> Self {
        match index {
            0 => DownloadOver::Any,
            _ => DownloadOver::UnmeteredOnly,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DownloadOver::Any => "Any connection",
            DownloadOver::UnmeteredOnly => "Unmetered connections only",
        }
    }
}

/// Both dropdowns. Serialised into whatever document the shell keeps its own
/// preferences in; nothing here reads or writes a file, because where Cordial's
/// settings live is `shell_config`'s answer and ADR-013's, not this crate's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct UpdateSettings {
    pub automatic: Automatic,
    pub download_over: DownloadOver,
}

/// What should happen on launch, given the settings and the connection.
///
/// Total by construction: every pair of settings values maps to exactly one of
/// these, which is the property the two-dropdown shape was chosen for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Check in the background and fetch whatever it finds.
    CheckAndDownload,
    /// Check in the background, then wait to be asked.
    ///
    /// `why` is `Some` when the reason for waiting is the connection rather
    /// than the user's choice, so the button can say which — "waiting because
    /// you asked to be asked" and "waiting because you are on a hotspot" are
    /// different things to be looking at.
    CheckAndAsk { why: Option<String> },
    /// No request of any kind. The button offers a manual check.
    DoNotCheck,
}

impl UpdateSettings {
    pub fn plan(self, metered: Metered) -> Plan {
        match self.automatic {
            Automatic::Never => Plan::DoNotCheck,
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
        match self.download_over {
            DownloadOver::Any => Ok(()),
            DownloadOver::UnmeteredOnly if !metered.is_metered() => Ok(()),
            DownloadOver::UnmeteredOnly => Err(format!(
                "{}, and Download over is set to unmetered connections only",
                metered.describe()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_ones_the_design_names() {
        let s = UpdateSettings::default();
        assert_eq!(s.download_over, DownloadOver::UnmeteredOnly);
        assert_eq!(s.automatic, Automatic::Background);
    }

    #[test]
    fn never_check_means_no_request_whatever_the_network_is() {
        // "Never check" is about background network use, so a fast unmetered
        // connection must not talk it out of the setting.
        let s = UpdateSettings { automatic: Automatic::Never, download_over: DownloadOver::Any };
        assert_eq!(s.plan(Metered::No), Plan::DoNotCheck);
        assert_eq!(s.plan(Metered::Yes), Plan::DoNotCheck);
    }

    #[test]
    fn background_over_a_metered_connection_waits_and_says_why() {
        // The combination the two-dropdown shape exists to make expressible
        // without a contradiction: still checking, not yet downloading.
        let s = UpdateSettings::default();
        match s.plan(Metered::GuessYes) {
            Plan::CheckAndAsk { why: Some(why) } => {
                assert!(why.contains("unmetered connections only"), "{why}");
                assert!(why.contains("guess"), "{why}");
            }
            other => panic!("expected a reason, got {other:?}"),
        }
    }

    #[test]
    fn background_over_an_unmetered_connection_downloads() {
        assert_eq!(UpdateSettings::default().plan(Metered::No), Plan::CheckAndDownload);
    }

    #[test]
    fn any_connection_means_any() {
        let s = UpdateSettings {
            automatic: Automatic::Background,
            download_over: DownloadOver::Any,
        };
        assert_eq!(s.plan(Metered::Yes), Plan::CheckAndDownload);
        assert_eq!(s.plan(Metered::Unknown), Plan::CheckAndDownload);
    }

    #[test]
    fn asking_first_is_not_overridden_by_a_cheap_connection() {
        let s = UpdateSettings { automatic: Automatic::Ask, download_over: DownloadOver::Any };
        assert_eq!(s.plan(Metered::No), Plan::CheckAndAsk { why: None });
    }

    #[test]
    fn every_setting_pair_has_exactly_one_plan() {
        // The property two dropdowns were chosen for. If a third control were
        // ever added, this is the test that would have to be told what the new
        // combinations mean.
        for automatic in [Automatic::Background, Automatic::Ask, Automatic::Never] {
            for download_over in [DownloadOver::Any, DownloadOver::UnmeteredOnly] {
                for metered in [Metered::No, Metered::Yes, Metered::GuessNo, Metered::Unknown] {
                    let s = UpdateSettings { automatic, download_over };
                    // Total: no panic, no "undefined" arm to fall into.
                    let _ = s.plan(metered);
                }
            }
        }
    }

    #[test]
    fn the_combo_indices_round_trip() {
        for a in [Automatic::Background, Automatic::Ask, Automatic::Never] {
            assert_eq!(Automatic::from_index(a.index()), a);
        }
        for d in [DownloadOver::Any, DownloadOver::UnmeteredOnly] {
            assert_eq!(DownloadOver::from_index(d.index()), d);
        }
    }

    #[test]
    fn the_stored_form_survives_a_round_trip_and_an_absent_key() {
        let s = UpdateSettings { automatic: Automatic::Ask, download_over: DownloadOver::Any };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"ask\""), "{text}");
        assert_eq!(serde_json::from_str::<UpdateSettings>(&text).unwrap(), s);
        // A settings file written before this existed must not refuse to load.
        assert_eq!(
            serde_json::from_str::<UpdateSettings>("{}").unwrap(),
            UpdateSettings::default()
        );
    }
}
