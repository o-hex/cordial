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
//! Measured on 2026-08-02, on an ordinary desktop on wired/wireless LAN, this
//! answered `u 4` — `GUESS_NO` — and the paragraph here concluded that the
//! ordinary desktop case is a *guess* rather than a `NO`, so the background
//! download does not run there either.
//!
//! **Re-measured on 2026-08-05 on the same machine, it answers `u 2`:**
//!
//! ```text
//! $ busctl --system get-property org.freedesktop.NetworkManager \
//!     /org/freedesktop/NetworkManager org.freedesktop.NetworkManager Metered
//! u 2
//! ```
//!
//! Two is an explicit `NO`. So the earlier conclusion is not a property of
//! ordinary desktops; it was a property of that connection on that day. Both
//! readings are real and the value moves, which is the actual lesson: do not
//! reason about what "the ordinary case" reports without asking the machine in
//! front of you. The consequence is still written up in
//! `docs/design/updating-roblox.md`, and it still applies whenever the answer
//! is a guess.
//!
//! ## Inside a Flatpak
//!
//! There is no system bus unless the manifest grants
//! `--system-talk-name=org.freedesktop.NetworkManager`, and without it [`query`]
//! fails with `Could not connect` and [`current`] falls closed to
//! [`Metered::Unknown`]. The grant is in the manifest for that reason.
//!
//! **`org.freedesktop.portal.NetworkMonitor` is not the answer**, despite
//! needing no grant at all and being the route `notify.send` and `url.open`
//! take. It reports `metered` as a boolean, and the rule below is that only an
//! explicit `No` counts — a boolean cannot carry the difference between `No`,
//! `GuessNo` and `Unknown`. `INFERRED` that GLib folds all three to `false`:
//! this machine reports an explicit `No` today, so the portal and
//! NetworkManager agree here and the mapping could not be observed. If somebody
//! can get a machine to report `GUESS_NO` and asks both, that settles it.

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
    /// waiting.
    ///
    /// Plain, and deliberately does not say "NetworkManager" — every one of
    /// these used to, which put the name of a system service in front of
    /// somebody who wanted to know whether their update was going to cost them
    /// anything. The service is still the source and the source is still worth
    /// knowing: [`Display`](std::fmt::Display) is the diagnostic token, this is
    /// the sentence.
    ///
    /// The two guesses keep their hedge. "Metered" on its own invites "no it
    /// isn't", and a user who is told flatly that a wired desktop is metered
    /// concludes Cordial is wrong rather than that the answer is a guess.
    pub fn describe(self) -> String {
        match self {
            Metered::Unknown => "Cordial cannot tell whether this connection is metered".into(),
            Metered::Yes => "This connection is metered".into(),
            Metered::No => "This connection is not metered".into(),
            Metered::GuessYes => "This connection looks metered, such as a phone hotspot".into(),
            Metered::GuessNo => {
                "This connection is probably not metered, but that is a guess".into()
            }
            // Still carries the number. Somebody meeting this has hit a value
            // added to NetworkManager since this was written, and the number is
            // the whole of what they need to fix it.
            Metered::Unrecognised(n) => {
                format!("Cordial does not recognise what this connection reported ({n})")
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
        // And it says so without naming a system service at somebody who asked
        // about their data allowance.
        for m in [Metered::Unknown, Metered::Yes, Metered::No, Metered::GuessYes, Metered::GuessNo]
        {
            assert!(!m.describe().contains("NetworkManager"), "{}", m.describe());
        }
    }
}
