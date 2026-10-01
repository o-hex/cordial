//! The Frame rate limit choice, shared by the launcher's settings and the live-settings wire.

use serde::{Deserialize, Serialize};

/// The Frame rate limit row: what `DFIntTaskSchedulerTargetFps` is held at.
///
/// Display refresh sets nothing, which is the engine's own behaviour. A number
/// sets the flag to it, and `cordial_runtime::flag_reapply` puts it back after
/// each of the engine's own settings refreshes, which would otherwise revert it
/// to Roblox's value a couple of minutes in (ADR-051). The row is live: the
/// choice reaches a running client over the settings socket and is applied at
/// once (ADR-044).
///
/// **Separate from `PresentMode`, not a replacement for it.** `fastflags.md`
/// documents these as two levers -- Frame pacing is
/// `VkSwapchainCreateInfoKHR::presentMode`, this is the engine's own
/// scheduler target -- and `PresentMode::default()` has been `Mailbox`
/// since before this row existed. What was missing was a way to raise the
/// *engine's* own cap and have it stay raised.
///
/// **There is no "Unlimited", and nothing above 240.** An earlier draft sent
/// 9999, a number nothing had been measured near. A contributor then tried
/// raising the engine's frame-rate settings to 1000 on a 144 Hz monitor and the
/// frame rate still held at 240, so 240 is where the engine stops (reported in
/// the FPS Flex pull request; not reproduced here, this project has no monitor
/// that fast). A choice above 240 would be a row that does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameRateLimit {
    /// Sets nothing. The engine's own default, and what every Cordial before
    /// this row shipped.
    Display,
    Cap90,
    Cap120,
    Cap144,
    Cap165,
    /// `unlimited` is what a pre-release build wrote here, when the row had one;
    /// a config that names it still has to load, and 240 is where the engine
    /// stops anyway.
    #[serde(alias = "unlimited")]
    Cap240,
}

impl Default for FrameRateLimit {
    fn default() -> Self {
        FrameRateLimit::Display
    }
}

impl FrameRateLimit {
    pub const ALL: [FrameRateLimit; 6] = [
        FrameRateLimit::Display,
        FrameRateLimit::Cap90,
        FrameRateLimit::Cap120,
        FrameRateLimit::Cap144,
        FrameRateLimit::Cap165,
        FrameRateLimit::Cap240,
    ];

    /// Order matches the `AdwComboRow` model in `settings.rs`, as
    /// `PresentMode::index` does.
    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0) as u32
    }

    pub fn from_index(index: u32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or_default()
    }

    /// The word `cordial_runtime::flags::FrameRateLimit::parse` takes, out of
    /// `CORDIAL_FRAME_RATE_LIMIT` at launch and over the live socket afterwards,
    /// so a choice has one spelling either way.
    pub fn as_env(self) -> &'static str {
        match self {
            FrameRateLimit::Display => "display",
            FrameRateLimit::Cap90 => "90",
            FrameRateLimit::Cap120 => "120",
            FrameRateLimit::Cap144 => "144",
            FrameRateLimit::Cap165 => "165",
            FrameRateLimit::Cap240 => "240",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.as_env() == word)
    }

    pub fn row_label(self) -> &'static str {
        match self {
            FrameRateLimit::Display => "Display refresh",
            FrameRateLimit::Cap90 => "90 fps",
            FrameRateLimit::Cap120 => "120 fps",
            FrameRateLimit::Cap144 => "144 fps",
            FrameRateLimit::Cap165 => "165 fps",
            FrameRateLimit::Cap240 => "240 fps",
        }
    }
}
