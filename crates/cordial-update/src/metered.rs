//! Whether this connection is one somebody pays for by the megabyte.
//!
//! `org.freedesktop.NetworkManager`'s `Metered` property, read from the system
//! bus. Same mechanism as GameMode and the Secret Service, so no new dependency.
//!
//! **It has four values, not two.** NetworkManager's `NMMetered` is `UNKNOWN`,
//! `YES`, `NO`, `GUESS_YES`, `GUESS_NO` — a guess being what it reports when
//! nobody has told it and it has worked something out from the device type. A
//! phone hotspot commonly reports *guess-yes*.
//!
//! **Both guesses are treated as metered, and so is unknown, and so is any
//! number this code does not recognise.** Reading a guess as "not metered" is
//! how somebody's data allowance pays for a 115 MB download they never asked
//! for, and there is no symmetrical harm on the other side: the cost of being
//! wrong in this direction is that an update waits for the user to press a
//! button. Only an explicit `NO` — somebody or something stating it — turns the
//! background download on.
//!
//! ## What this machine reports, and why it matters more than it looks
//!
//! Measured on 2026-08-02, on an ordinary desktop on wired/wireless LAN:
//!
//! ```text
//! $ busctl --system get-property org.freedesktop.NetworkManager \
//!     /org/freedesktop/NetworkManager org.freedesktop.NetworkManager Metered
//! u 4
//! ```
//!
//! Four is `GUESS_NO`. So the ordinary desktop case is a *guess*, not a `NO`,
//! and with the default *Unmetered connections only* the background download
//! does not run there either. That is the rule working as written rather than a
//! bug, and it is written up in `docs/design/updating-roblox.md` because it is
//! the sort of consequence that gets discovered later and read as one.

use std::fmt;

/// `NMMetered`, as NetworkManager defines it.
///
/// [`Metered::Unrecognised`] exists so a value added to the enum after this was
/// written cannot arrive as `No`. A `u32` from another process is not a promise
/// about this code's variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metered {
    Unknown,
    Yes,
    No,
    GuessYes,
    GuessNo,
    Unrecognised(u32),
}

impl Metered {
    /// NetworkManager's numbering, from `NMMetered` in `nm-dbus-interface.h`.
    pub fn from_nm(value: u32) -> Self {
        match value {
            0 => Metered::Unknown,
            1 => Metered::Yes,
            2 => Metered::No,
            3 => Metered::GuessYes,
            4 => Metered::GuessNo,
            other => Metered::Unrecognised(other),
        }
    }

    /// Whether a download should be held back.
    ///
    /// Written as "everything except `No`" rather than as a list of the metered
    /// cases, deliberately. The list form is the one that acquires a new
    /// variant and quietly defaults it to cheap.
    pub fn is_metered(self) -> bool {
        !matches!(self, Metered::No)
    }

    /// What to put in front of a user who is being told their download is
    /// waiting. "Metered" on its own invites "no it isn't".
    pub fn describe(self) -> String {
        match self {
            Metered::Unknown => "NetworkManager does not know whether this connection is metered".into(),
            Metered::Yes => "NetworkManager reports this connection as metered".into(),
            Metered::No => "NetworkManager reports this connection as unmetered".into(),
            Metered::GuessYes => {
                "NetworkManager guesses this connection is metered — a phone hotspot usually reads this way".into()
            }
            Metered::GuessNo => {
                "NetworkManager guesses this connection is unmetered, which is a guess rather than an answer".into()
            }
            Metered::Unrecognised(n) => {
                format!("NetworkManager reported Metered = {n}, which this Cordial does not recognise")
            }
        }
    }
}

impl fmt::Display for Metered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Metered::Unknown => "unknown",
            Metered::Yes => "yes",
            Metered::No => "no",
            Metered::GuessYes => "guess-yes",
            Metered::GuessNo => "guess-no",
            Metered::Unrecognised(_) => "unrecognised",
        })
    }
}

const SERVICE: &str = "org.freedesktop.NetworkManager";
const OBJECT: &str = "/org/freedesktop/NetworkManager";

/// Ask NetworkManager, naming what could not be reached if it does not answer.
///
/// The error is a string rather than a type because there is exactly one caller
/// that cares about the difference, [`current`], and it treats every failure the
/// same way. What the string is for is the log line: "no system bus" and
/// "NetworkManager is not running" are different machines to fix.
pub fn query() -> Result<Metered, String> {
    let connection = zbus::blocking::Connection::system()
        .map_err(|e| format!("no system bus, so {SERVICE} could not be asked: {e}"))?;
    let reply = connection
        .call_method(
            Some(SERVICE),
            OBJECT,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(SERVICE, "Metered"),
        )
        .map_err(|e| format!("{SERVICE} did not answer Get(Metered): {e}"))?;
    // The body has to outlive the `Value` borrowed out of it, which is why this
    // is two bindings rather than a chain.
    let body = reply.body();
    let value: zbus::zvariant::Value = body
        .deserialize()
        .map_err(|e| format!("{SERVICE} answered Get(Metered) with something unreadable: {e}"))?;
    match u32::try_from(&value) {
        Ok(n) => Ok(Metered::from_nm(n)),
        Err(e) => Err(format!("{SERVICE} answered Get(Metered) with {value:?}, not a u32: {e}")),
    }
}

/// What to act on: NetworkManager's answer, or metered.
///
/// A machine with no NetworkManager is not a machine with a free connection; it
/// is a machine nobody asked. Defaulting the absent case to unmetered would make
/// "no D-Bus" the way to get the background download on every system that does
/// not run NetworkManager, which is not a decision anybody made.
pub fn current() -> Metered {
    match query() {
        Ok(m) => m,
        Err(why) => {
            println!("[update] treating the connection as metered: {why}");
            Metered::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbering_is_networkmanagers() {
        assert_eq!(Metered::from_nm(0), Metered::Unknown);
        assert_eq!(Metered::from_nm(1), Metered::Yes);
        assert_eq!(Metered::from_nm(2), Metered::No);
        assert_eq!(Metered::from_nm(3), Metered::GuessYes);
        assert_eq!(Metered::from_nm(4), Metered::GuessNo);
        assert_eq!(Metered::from_nm(9), Metered::Unrecognised(9));
    }

    #[test]
    fn only_an_explicit_no_counts_as_unmetered() {
        // The rule the whole module exists for. A guess-yes read as cheap is
        // somebody's phone plan paying for 115 MB they did not ask for; the
        // symmetric mistake costs a button press.
        assert!(!Metered::No.is_metered());
        for m in [
            Metered::Unknown,
            Metered::Yes,
            Metered::GuessYes,
            Metered::GuessNo,
            Metered::Unrecognised(7),
        ] {
            assert!(m.is_metered(), "{m} must be treated as metered");
        }
    }

    #[test]
    fn a_guess_no_is_metered_and_that_is_the_ordinary_desktop() {
        // Called out on its own because it is the case that looks like a bug:
        // this very machine answers 4 (guess-no) on an ordinary LAN, so the
        // default settings hold the download there. Deleting this assertion is
        // how "guesses are metered" quietly becomes "guesses go the way they
        // lean".
        assert!(Metered::GuessNo.is_metered());
        assert!(Metered::GuessNo.describe().contains("guess"));
    }
}
