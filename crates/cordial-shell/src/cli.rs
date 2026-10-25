//! The command line that starts a game with nobody at the keyboard.
//!
//! [ADR-058](../../../docs/adr/ADR-058-partydeck-and-gamescope.md) is why this
//! exists. A launcher that runs several copies of a game at once -- PartyDeck is
//! the one this was written for -- can pass each copy an argument vector and a
//! working directory and nothing else. It cannot press the Roblox button, and it
//! cannot forward the second copy to the first, which is what a `GApplication`
//! with a fixed id does with every later invocation. So the shell takes
//! its instructions from `argv` and, when they are an instruction to *play*,
//! stops being a launcher:
//!
//! ```text
//! cordial --profile P2 --play --place-id 1818 --fullscreen --set present_mode=fifo
//! ```
//!
//! **What is parsed here is what a process was told, and nothing about the
//! machine.** The grammar is a pure function over the arguments so each rule has
//! a test that needs no display, the shape `deep_link::accept` and `flag_import`
//! already take.
//!
//! Nothing a game was asked to do is written back. `--profile` and `--set` apply
//! to this process only and `shell_config::save` is a no-op under them
//! ([`crate::shell_config::hold_unsaved`]): a second instance that picked profile
//! `P2` for a session must not leave `P2` in the user's `shell.json` as the
//! profile their next ordinary launch runs.

use std::sync::OnceLock;

/// Everything the command line asked for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cli {
    /// `--profile NAME`: the profile this process runs, for this process only.
    pub profile: Option<String>,
    /// `--play`, or implied by `--place-id`: start the game at once and never
    /// show the launcher.
    pub play: bool,
    /// `--place-id N`: join this experience on start.
    pub place_id: Option<u64>,
    /// `--set KEY=VALUE`, in order, for this process only.
    pub settings: Vec<(String, String)>,
    /// `--fullscreen`: the client covers the output it opens on.
    pub fullscreen: bool,
    /// A `roblox-player:` or `roblox:` link, which is the shell's original
    /// job and is checked by `deep_link::accept`, not here.
    pub links: Vec<String>,
}

impl Cli {
    /// Whether this invocation is a game of its own rather than a request to the
    /// launcher that may already be open.
    ///
    /// **This is what decides single-instance behaviour**, and it is deliberately
    /// the *presence* of an instruction rather than the profile differing from
    /// the one a running launcher has: the shell cannot ask a process it has not
    /// forwarded to, and "run this profile" is meaningless as a request to
    /// somebody else's window. A bare `cordial` or one that carries only a link
    /// is still forwarded, which is how a click on the website reaches the
    /// launcher that is open.
    pub fn is_instance(&self) -> bool {
        self.play || self.profile.is_some()
    }

    /// The link `--place-id` stands for, in the one shape the engine has been
    /// measured to act on. See `cordial_runtime::deeplink`: a `roblox-player:`
    /// link is translated into exactly this, and a `placeId` on its own starts a
    /// new server rather than joining one.
    pub fn place_link(&self) -> Option<String> {
        self.place_id.map(|id| format!("roblox://experiences/start?placeId={id}"))
    }
}

/// The text `--help` carries for these options, in the same layout as the rest
/// of it: option at column 2, description from column 18.
pub const HELP: &str = "  --profile NAME  Run this profile in this window only, as a Cordial of its own:
                  it does not hand over to one already open, and it is not
                  remembered as your profile. Letters, digits, - and _.
  --play          Start Roblox at once, with no launcher window. Exits when
                  the game does, and non-zero if it could not start.
  --place-id N    Join experience N instead of the home screen (implies --play).
  --fullscreen    Make the game fill the display it opens on, which is what a
                  gamescope window wants (see docs/partydeck.md).
  --set KEY=VALUE
                  Override one setting from Settings for this run, such as
                  --set present_mode=fifo or --set mangohud=true. Nothing is
                  saved. An unknown KEY lists the valid ones.
";

/// Read the arguments after the program name.
///
/// Anything this does not recognise that begins with `-` is an error rather than
/// being handed on as a link, because the alternative is `--palay` being
/// reported as "ignoring the scheme" by a process that then opens a launcher the
/// caller never wanted to see.
pub fn parse(args: &[String]) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            match inline.clone().or_else(|| it.next().cloned()) {
                Some(v) if !v.is_empty() => Ok(v),
                _ => Err(format!("{flag} needs {what}")),
            }
        };
        match flag {
            "--profile" => {
                let name = value("a profile name")?;
                if !cordial_shell::profile::is_valid_name(&name) {
                    return Err(format!(
                        "{name:?} is not a usable profile name; use letters, digits, - and _"
                    ));
                }
                cli.profile = Some(name);
            }
            "--play" | "--fullscreen" if inline.is_some() => {
                return Err(format!("{flag} takes no value"));
            }
            "--play" => cli.play = true,
            "--fullscreen" => cli.fullscreen = true,
            "--place-id" => {
                let raw = value("a number")?;
                // Digits only, and a real number: this ends up in a URL that is
                // handed to the engine, so it is checked as an id and not merely
                // as printable text.
                let id: u64 = raw
                    .parse()
                    .ok()
                    .filter(|n| *n > 0 && raw.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| format!("--place-id wants a positive number, not {raw:?}"))?;
                cli.place_id = Some(id);
                cli.play = true;
            }
            "--set" => {
                let pair = value("KEY=VALUE")?;
                let Some((key, val)) = pair.split_once('=') else {
                    return Err(format!("--set wants KEY=VALUE, not {pair:?}"));
                };
                if key.is_empty() {
                    return Err(format!("--set wants KEY=VALUE, not {pair:?}"));
                }
                cli.settings.push((key.to_string(), val.to_string()));
            }
            // GApplication's own switches, which reach the process whether or
            // not it declared them and are not ours to refuse.
            f if f.starts_with("--gapplication-") => {}
            f if f.starts_with('-') && f.len() > 1 => {
                return Err(format!("unknown option {f}; see cordial --help"));
            }
            _ => cli.links.push(arg.clone()),
        }
    }
    Ok(cli)
}

/// What this process was started with, for the parts of the shell that are not
/// handed it: the window's autostart and the launch outcomes that have to
/// leave through the exit status.
static ACTIVE: OnceLock<Cli> = OnceLock::new();

/// Record the command line. Once; a second call is ignored, which is only ever a
/// test.
pub fn install(cli: Cli) {
    let _ = ACTIVE.set(cli);
}

pub fn active() -> Option<&'static Cli> {
    ACTIVE.get()
}

/// Whether this process was told to play: no launcher window, and a failure to
/// start is an exit status rather than a dialog.
pub fn playing() -> bool {
    active().is_some_and(|c| c.play)
}

/// The status the process exits with. `0` until something sets it.
static STATUS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Record why the process is going to exit non-zero. The first wins, which is
/// the cause rather than the consequence.
pub fn set_status(code: u8) {
    let _ = STATUS.compare_exchange(0, code, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed);
}

pub fn status() -> u8 {
    STATUS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn nothing_is_a_launcher_and_a_link_is_a_link() {
        let c = p(&[]).unwrap();
        assert!(!c.is_instance() && !c.play && c.links.is_empty());
        let c = p(&["roblox-player:1+launchmode:play"]).unwrap();
        assert_eq!(c.links, ["roblox-player:1+launchmode:play"]);
        assert!(!c.is_instance(), "a link is forwarded to the open launcher, not run as a game of its own");
    }

    #[test]
    fn the_partydeck_shape_parses() {
        let c = p(&["--profile", "P2", "--play", "--fullscreen", "--set", "present_mode=fifo"]).unwrap();
        assert_eq!(c.profile.as_deref(), Some("P2"));
        assert!(c.play && c.fullscreen && c.is_instance());
        assert_eq!(c.settings, [("present_mode".to_string(), "fifo".to_string())]);
    }

    #[test]
    fn equals_forms_are_the_same_as_two_words() {
        let a = p(&["--profile=P2", "--place-id=1818", "--set=mangohud=true"]).unwrap();
        let b = p(&["--profile", "P2", "--place-id", "1818", "--set", "mangohud=true"]).unwrap();
        assert_eq!(a, b);
        // The value of --set keeps its own '=': only the first one splits.
        let c = p(&["--set", "audio_output=alsa_output.pci-0000:00:1f.3=x"]).unwrap();
        assert_eq!(c.settings[0].0, "audio_output");
        assert_eq!(c.settings[0].1, "alsa_output.pci-0000:00:1f.3=x");
    }

    #[test]
    fn a_profile_may_not_climb_out_of_the_profile_root() {
        for bad in ["../x", "a/b", ".hidden", "with space", "", &"x".repeat(65)] {
            assert!(p(&["--profile", bad]).is_err(), "{bad:?}");
        }
        // PartyDeck's guest profiles start with a dot, and are refused for the
        // same reason a dot-dot is: ADR-012 refuses a name rather than
        // rewriting it, and docs/partydeck.md says to name profiles.
        assert!(p(&["--profile", ".Blinky"]).is_err());
        assert!(p(&["--profile", "P-2_b"]).is_ok());
    }

    #[test]
    fn a_place_id_is_a_number_and_means_play() {
        let c = p(&["--place-id", "1818"]).unwrap();
        assert_eq!(c.place_id, Some(1818));
        assert!(c.play && c.is_instance(), "a place id with no --play still plays");
        assert_eq!(c.place_link().as_deref(), Some("roblox://experiences/start?placeId=1818"));
        for bad in ["0", "-3", "12a", "1 2", "", "99999999999999999999999", "+5"] {
            assert!(p(&["--place-id", bad]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_missing_value_is_named() {
        assert_eq!(p(&["--profile"]).unwrap_err(), "--profile needs a profile name");
        assert_eq!(p(&["--set"]).unwrap_err(), "--set needs KEY=VALUE");
        assert!(p(&["--set", "novalue"]).unwrap_err().contains("KEY=VALUE"));
        assert!(p(&["--set", "=x"]).is_err());
    }

    #[test]
    fn an_unknown_option_is_refused_and_a_misspelling_is_not_a_link() {
        let e = p(&["--palay"]).unwrap_err();
        assert!(e.contains("--palay"), "{e}");
        // A flag that takes no value is not allowed one.
        assert!(p(&["--play=yes"]).is_err());
        // GApplication's own switches are not ours to refuse.
        assert!(p(&["--gapplication-service"]).is_ok());
    }

    #[test]
    fn the_help_mentions_every_option_the_parser_takes() {
        for flag in ["--profile", "--play", "--place-id", "--fullscreen", "--set"] {
            assert!(HELP.contains(flag), "--help does not mention {flag}");
        }
    }
}
