//! Shared title-bar preference for the launcher settings and game window.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleBar {
    #[default]
    Default,
    Compact,
    Hidden,
}

impl TitleBar {
    pub const LABELS: &'static [&'static str] = &["Default", "Compact", "Hidden"];

    pub const fn index(self) -> u32 {
        match self {
            Self::Default => 0,
            Self::Compact => 1,
            Self::Hidden => 2,
        }
    }

    pub const fn from_index(index: u32) -> Self {
        match index {
            1 => Self::Compact,
            2 => Self::Hidden,
            _ => Self::Default,
        }
    }

    pub const fn env_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Compact => Some("compact"),
            Self::Hidden => Some("hidden"),
        }
    }

    pub fn from_env() -> Self {
        match std::env::var("CORDIAL_TITLE_BAR").as_deref() {
            Ok("compact") => Self::Compact,
            Ok("hidden") => Self::Hidden,
            _ => Self::Default,
        }
    }

    /// Leaving fullscreen must not reveal chrome the user explicitly hid.
    pub const fn revealed(self, fullscreen: bool) -> bool {
        match self {
            Self::Hidden => false,
            Self::Default | Self::Compact => !fullscreen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_selection_round_trips_through_config_and_launch_environment() {
        // Given the third choice in the settings row.
        let choice = TitleBar::from_index(2);
        // When saving and restoring its value.
        let stored = serde_json::to_string(&choice).unwrap();
        let restored: TitleBar = serde_json::from_str(&stored).unwrap();
        // Then the next game receives the hidden preference.
        assert_eq!(stored, "\"hidden\"");
        assert_eq!(restored.index(), 2);
        assert_eq!(restored.env_value(), Some("hidden"));
    }

    #[test]
    fn each_mode_controls_windowed_chrome() {
        // Given each named preference.
        for (choice, expected) in [
            (TitleBar::Default, true),
            (TitleBar::Compact, true),
            (TitleBar::Hidden, false),
        ] {
            // When requesting its windowed presentation, then only Hidden removes chrome.
            assert_eq!(choice.revealed(false), expected);
        }
    }

    #[test]
    fn fullscreen_hides_chrome_in_every_mode() {
        // Given each named preference.
        for choice in [TitleBar::Default, TitleBar::Compact, TitleBar::Hidden] {
            // When requesting its fullscreen presentation, then no mode reveals chrome.
            assert!(!choice.revealed(true));
        }
    }

    #[test]
    fn existing_preferences_keep_their_values_and_default() {
        // Given old configurations and out-of-range settings indices.
        for (index, expected) in [
            (0, TitleBar::Default),
            (1, TitleBar::Compact),
            (99, TitleBar::Default),
        ] {
            // When reading the preference, then old choices retain their meaning.
            assert_eq!(TitleBar::from_index(index), expected);
        }
        assert_eq!(TitleBar::default(), TitleBar::Default);
    }
}
