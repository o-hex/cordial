//! Which graphics backend the engine is given, and who asked for it.
//!
//! ## This is not a FastFlag, and the measurement that says so
//!
//! There is an obvious-looking way to do this and it does not work.
//! `FStringDebugGraphicsPreferredBackend` is a real Roblox flag, the shell used
//! to expose it as the Renderer row, and `flags_file.rs` described `"Vulkan"` as
//! "the only value confirmed anywhere in this repository". **It changes
//! nothing.** Measured 2026-08-03 on 2.730.0.790, one launch each, reading the
//! engine's own `Loaded N shaders from pack <name>` line:
//!
//! ```text
//! (no flag)                     -> pack vulkan_mobile
//! "CordialProbeNotABackend"     -> pack vulkan_mobile
//! "OpenGL" "GLES" "GLES3"       -> pack vulkan_mobile
//! "OpenGLES" "glsles3"          -> pack vulkan_mobile
//! ```
//!
//! Deliberate rubbish is accepted in silence, which is the important half: a
//! wrong value is indistinguishable from no value, so a settings row built on
//! this flag would have looked like it worked and done nothing. That was the
//! exact risk `flags_file.rs` named when it refused to guess a GLES spelling,
//! and it was right — but its own "confirmed" value is confirmed only as a
//! string that appears in our README. `"Vulkan"` produces output identical to
//! setting nothing, because Vulkan is what the engine takes anyway.
//!
//! ## What does work
//!
//! `libroblox.so` links no Vulkan at all — it is not in `DT_NEEDED`, and the
//! engine `dlopen`s `libvulkan.so` and then `libvulkan.so.1` (see
//! [`crate::android::vulkan`]). So the backend is chosen by whether that
//! `dlopen` finds anything, and Cordial is what answers it. Withhold the
//! virtual soname and the engine takes its own documented fall-through to
//! GLES3. Measured, same day, with the Vulkan ICD made unavailable:
//!
//! ```text
//! [FLog::Graphics] Trying to choose EGL config r8 g8 b8 vsync0
//! [FLog::Graphics] Initialized EGL context ... with renderbuffer 1280x720
//! Loaded ... from pack glsles3 variant default
//! Compiled 618 shaders in 75 ms
//! -> onDataModelNotification: Received type(APP_READY, 10), data(Landing)
//! ```
//!
//! The APK ships exactly two shader packs — `shaders_vulkan_mobile.pack` and
//! `shaders_glsles3.pack` — so that log line names the backend unambiguously
//! and is the way to check any claim made here.
//!
//! That run forced it the blunt way, by making the Vulkan ICD unavailable to
//! the whole process. **Withholding the virtual soname was then shown to be
//! equivalent**, through this module rather than around it:
//!
//! ```text
//! CORDIAL_GRAPHICS=gles     -> from pack glsles3
//! CORDIAL_GRAPHICS=vulkan   -> from pack vulkan_mobile
//! (unset)                   -> from pack vulkan_mobile
//! CordialGraphicsBackend=gles in a flag layer, no env -> from pack glsles3
//! ```
//!
//! The last two are the control: the same binary, the same profile, the switch
//! being the only difference.
//!
//! **Still not established:** that GLES3 is *stable*. One launch to Landing is
//! not a stability result, this project has a bug that reproduced on roughly one
//! launch in three, and nothing here has been signed in or run inside an
//! experience. The setting is offered because withholding Vulkan demonstrably
//! selects the backend, not because the backend is known to be good.
//!
//! ## Precedence, which is the user's and then the plugins'
//!
//! The same rule [`crate::flags`] already applies to everything else: an
//! explicit setting is the one thing that must not be quietly overridden, and a
//! plugin gets its say only where the user has not had one. `Automatic` is not
//! a third opinion competing with those two — it is the absence of a user
//! opinion, which is exactly what leaves the door open for a plugin.
//!
//! A plugin asks by writing [`KEY`] into its own flag layer. That key is
//! Cordial's rather than Roblox's, so it never reaches the engine's settings —
//! see `client_settings.rs`, which drops the `Cordial` prefix before applying.

use std::sync::OnceLock;

/// The Cordial-owned key a plugin writes to ask for a backend.
///
/// Deliberately not a Roblox flag name. It rides the flag layering because that
/// machinery already carries precedence and provenance, not because the engine
/// has any idea what it means.
pub const KEY: &str = "CordialGraphicsBackend";

/// The environment variable the shell sets from the Graphics row.
///
/// Set only when the user has chosen something other than Automatic: an absent
/// variable and `automatic` mean the same thing, and the shell sends the
/// variable rather than a file because the backend has to be known before the
/// first `dlopen`, which is well before anything reads a profile.
pub const ENV: &str = "CORDIAL_GRAPHICS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Let the engine choose: Vulkan if Cordial can offer it, GLES3 otherwise.
    Automatic,
    /// Offer Vulkan. **Not a promise that Vulkan is used** — if the engine's own
    /// initialisation fails it still falls through to GLES3, which is its
    /// behaviour and not something Cordial could prevent.
    Vulkan,
    /// Withhold Vulkan, so the engine takes GLES3.
    ///
    /// There is no fallback from here. Withholding happens before the engine has
    /// tried anything, so if GLES3 fails there is nothing left in the process to
    /// fall back *to*; recovering would mean relaunching with Vulkan restored.
    /// The asymmetry is the engine's: Vulkan-then-GLES is a path it implements
    /// and GLES-then-Vulkan is not.
    GlEs,
}

impl Backend {
    pub fn parse(text: &str) -> Option<Backend> {
        match text.trim().to_ascii_lowercase().as_str() {
            "automatic" | "auto" | "" => Some(Backend::Automatic),
            "vulkan" => Some(Backend::Vulkan),
            "gles" | "gles3" | "glsles3" | "opengl" | "opengles" => Some(Backend::GlEs),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Backend::Automatic => "automatic",
            Backend::Vulkan => "vulkan",
            Backend::GlEs => "gles",
        }
    }

    /// Whether Cordial should offer the engine a Vulkan loader.
    pub fn offers_vulkan(self) -> bool {
        !matches!(self, Backend::GlEs)
    }
}

/// The backend in force, and the words for who asked.
#[derive(Debug, Clone)]
pub struct Choice {
    pub backend: Backend,
    /// Human-readable provenance — `"the Graphics setting"`, `"plugin:foo"`.
    ///
    /// Carried rather than derived because the whole point is that a plugin
    /// silently changing somebody's renderer must be diagnosable: "my game got
    /// slow after installing a plugin" should be one line in a log, not an
    /// afternoon.
    pub source: String,
}

/// Resolve once, per process.
pub fn choice() -> &'static Choice {
    static CHOICE: OnceLock<Choice> = OnceLock::new();
    CHOICE.get_or_init(|| resolve(std::env::var(ENV).ok(), plugin_request()))
}

/// The decision itself, with both inputs passed in so it can be tested.
pub fn resolve(from_env: Option<String>, from_plugin: Option<(String, String)>) -> Choice {
    // The user first, and an unparseable value is reported rather than
    // silently treated as Automatic: a Graphics row that does nothing is the
    // failure this whole module exists because of.
    if let Some(text) = from_env.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        match Backend::parse(text) {
            Some(Backend::Automatic) => {}
            Some(backend) => {
                return Choice { backend, source: "the Graphics setting".into() };
            }
            None => {
                eprintln!(
                    "[graphics] {ENV}={text:?} is not a backend; using Automatic. \
                     Known: automatic, vulkan, gles"
                );
                return Choice { backend: Backend::Automatic, source: "Automatic (after a bad {ENV})".into() };
            }
        }
    }

    // Then a plugin, which only gets here because the user said Automatic.
    if let Some((id, text)) = from_plugin {
        match Backend::parse(&text) {
            Some(Backend::Automatic) | None => {
                eprintln!(
                    "[graphics] {id} asked for {KEY}={text:?}, which is not a backend; ignoring"
                );
            }
            Some(backend) => return Choice { backend, source: id },
        }
    }

    Choice { backend: Backend::Automatic, source: "Automatic".into() }
}

/// What the flag layers say, if anything, and who said it.
fn plugin_request() -> Option<(String, String)> {
    let resolved = crate::flags::resolve(crate::flags::collect());
    let entry = resolved.get(KEY)?;
    Some((entry.source.describe(), entry.value.clone()))
}

/// Say which backend is in force and why, once, at startup.
///
/// Printed unconditionally rather than behind a trace switch. It is one line,
/// and the question it answers — "why is this slow" / "why does this look
/// different from yesterday" — is asked from a support thread where nobody is
/// going to be asked to reproduce with an environment variable set.
pub fn report() {
    let choice = choice();
    match choice.backend {
        Backend::Automatic => println!(
            "[graphics] backend: automatic (Vulkan if available, else GLES3), from {}",
            choice.source
        ),
        Backend::Vulkan => {
            println!("[graphics] backend: Vulkan, from {}", choice.source)
        }
        Backend::GlEs => println!(
            "[graphics] backend: GLES3 — Vulkan is being withheld deliberately, from {}",
            choice.source
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spellings_that_are_accepted_are_the_ones_a_person_would_type() {
        for text in ["gles", "GLES", " GlEs3 ", "opengl", "glsles3"] {
            assert_eq!(Backend::parse(text), Some(Backend::GlEs), "{text}");
        }
        assert_eq!(Backend::parse("Vulkan"), Some(Backend::Vulkan));
        assert_eq!(Backend::parse(""), Some(Backend::Automatic));
        assert_eq!(Backend::parse("metal"), None);
    }

    #[test]
    fn the_user_beats_a_plugin_and_automatic_is_not_a_veto() {
        // The rule `flags.rs` states and this has to match: an explicit setting
        // wins, and Automatic is the absence of one rather than a third opinion.
        let plugin = || Some(("plugin:shiny".to_string(), "gles".to_string()));

        let explicit = resolve(Some("vulkan".into()), plugin());
        assert_eq!(explicit.backend, Backend::Vulkan);
        assert_eq!(explicit.source, "the Graphics setting");

        let deferred = resolve(Some("automatic".into()), plugin());
        assert_eq!(deferred.backend, Backend::GlEs);
        assert_eq!(deferred.source, "plugin:shiny", "Automatic must let a plugin through");

        let unset = resolve(None, plugin());
        assert_eq!(unset.backend, Backend::GlEs, "an absent variable is Automatic");
    }

    #[test]
    fn nothing_asking_means_automatic() {
        let none = resolve(None, None);
        assert_eq!(none.backend, Backend::Automatic);
        assert!(none.source.contains("Automatic"), "{}", none.source);
    }

    #[test]
    fn a_value_nobody_understands_falls_back_rather_than_guessing() {
        // Both directions, because the failure being avoided is the one the
        // FastFlag had: a value that is not understood must not look like it
        // worked. Automatic is the safe landing, and it is announced.
        let bad_env = resolve(Some("mantle".into()), None);
        assert_eq!(bad_env.backend, Backend::Automatic);

        let bad_plugin = resolve(None, Some(("plugin:x".into(), "mantle".into())));
        assert_eq!(bad_plugin.backend, Backend::Automatic);
        assert!(bad_plugin.source.contains("Automatic"), "{}", bad_plugin.source);
    }

    #[test]
    fn only_gles_withholds_vulkan() {
        assert!(Backend::Automatic.offers_vulkan());
        assert!(Backend::Vulkan.offers_vulkan());
        assert!(!Backend::GlEs.offers_vulkan(), "GLES is the whole point of the switch");
    }
}
