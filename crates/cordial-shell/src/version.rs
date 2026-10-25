//! What version this is, and which build of it.
//!
//! **Two facts, not one, and the mistake was treating them as alternatives.**
//! The version is what the manifest says and is the thing anything ordering two
//! releases compares. The commit is provenance: it answers "which exact build
//! is this" in a bug report and nothing else.
//!
//! Before this split, `build.rs` stamped `git describe --tags --always --dirty`
//! as the version, which meant a tree whose `Cargo.toml` said 0.11.0 displayed
//! `0.10.0-26-g571e69b-dirty` -- the *previous* release. Meanwhile
//! `cordial-update`'s User-Agent has always been built from
//! `CARGO_PKG_VERSION`, so the same binary told a mirror it was 0.11.0 while
//! its title bar said 0.10.0. Two numbers that can disagree eventually do.
//!
//! [`GIT_SHA`] is `option_env!` rather than `env!` deliberately: an AUR source
//! package, a `cargo publish`, a vendored build and the Flatpak's `type: dir`
//! source all arrive without a usable `.git`, and none of them should fail to
//! compile over it. They get a clean [`VERSION`] and no commit, which is the
//! correct answer rather than a degraded one.

/// The version. `Cargo.toml`'s, always, and the only thing that may be compared.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit this was built from, when there was a git to ask.
///
/// Carries `-dirty` when the tree had uncommitted changes. AGENTS.md asks for
/// the full string in any report for exactly that reason: a build made from a
/// working tree several agents were editing is otherwise indistinguishable from
/// a committed one.
pub const GIT_SHA: Option<&str> = option_env!("CORDIAL_GIT_SHA");

/// What this is, under what licence, and where it came from.
///
/// Compiled into every binary that links this crate -- `cordial-run` included,
/// since `cordial-runtime` depends on this one -- so that `strings` answers the
/// question for a build that arrived without its repository. An AUR source
/// package, a vendored tarball and a fork's own release all reach somebody that
/// way, and the licence Cordial is under is not a thing they should have to go
/// looking for.
///
/// GPL-3.0-or-later asks a modified version to carry its notices forward. A
/// notice that is already in the binary is more likely to survive that than one
/// somebody has to remember to copy across.
///
/// **This is a notice and deliberately not a watermark.** Nothing reads it
/// back, nothing checks whether it is still there, and stripping it breaks
/// nothing -- AGENTS.md rules out client-side integrity marks, and anything
/// that behaved differently when this string was missing would be one. It is
/// here to be read, including by somebody who has forked this and is deciding
/// what they owe.
pub const NOTICE: &str = concat!(
    "Cordial ",
    env!("CARGO_PKG_VERSION"),
    " — GPL-3.0-or-later — https://github.com/luohoa97/cordial"
);

/// Version and provenance together, for a title bar or a bug report.
///
/// `0.11.0 (0fdbb44a1)` with a git, `0.11.0` without. Never a third shape: the
/// parenthesis is what stops the commit reading as part of the number.
pub fn full() -> String {
    match GIT_SHA {
        Some(sha) => format!("{VERSION} ({sha})"),
        None => VERSION.to_string(),
    }
}

/// The repository the project's own releases are built from.
pub const UPSTREAM_REPO: &str = "luohoa97/cordial";

/// What the release workflows stamped: the repository that ran them. `None` on
/// any build that was not made by a workflow that sets it.
pub const OFFICIAL_BUILD: Option<&str> = option_env!("CORDIAL_OFFICIAL_BUILD");

/// The git remote the source was built from, credentials and local paths
/// removed by `build_remote::clean`, when there was one.
pub const BUILD_REMOTE: Option<&str> = option_env!("CORDIAL_BUILD_REMOTE");

/// Where a build came from, for triage. **A hint, not a check.** A fork can set
/// the same variable or edit this file, so nothing may ever read it back and
/// behave differently; it exists so a report says what was run. The same line
/// `NOTICE` draws, for the same reason (AGENTS.md rules out client-side
/// integrity marks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Stamped by the project's own release workflows.
    Official,
    /// Anything else. `remote` is where the source was cloned from, if known.
    Unofficial { remote: Option<String> },
}

/// [`origin`] over explicit inputs, so the rule is tested without a build.
pub fn origin_from(official: Option<&str>, remote: Option<&str>) -> Origin {
    match official.map(str::trim) {
        Some(slug) if slug.eq_ignore_ascii_case(UPSTREAM_REPO) => Origin::Official,
        // Another repository's own release workflow. Reported as that
        // repository, which is what the reader wants to know, and better than
        // the checkout's remote, which a CI runner reports as its own clone.
        Some(slug) if slug.split('/').count() == 2 && !slug.contains(char::is_whitespace) => {
            Origin::Unofficial { remote: Some(format!("https://github.com/{slug}")) }
        }
        _ => Origin::Unofficial { remote: remote.map(str::to_string) },
    }
}

pub fn origin() -> Origin {
    origin_from(OFFICIAL_BUILD, BUILD_REMOTE)
}

impl Origin {
    /// One line for the diagnostics block and the doctor.
    pub fn text(&self) -> String {
        match self {
            Origin::Official => "Official build".into(),
            Origin::Unofficial { remote: Some(remote) } => format!("Unofficial build from {remote}"),
            Origin::Unofficial { remote: None } => "Unofficial build".into(),
        }
    }

    pub fn is_official(&self) -> bool {
        matches!(self, Origin::Official)
    }

    /// `owner/repo` of the remote when it is a `https://github.com/` one.
    pub fn github_slug(&self) -> Option<String> {
        let Origin::Unofficial { remote: Some(remote) } = self else { return None };
        let path = remote.strip_prefix("https://github.com/")?;
        (path.split('/').count() == 2).then(|| path.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`VERSION` is a bare semver and stays one.**
    ///
    /// The whole point of the split is that this string can be compared against
    /// another release's. The moment it grows a `-dev` or a commit count, every
    /// comparison needs a parser for Cordial's own build metadata -- which is
    /// what the scheme this replaced was heading for.
    #[test]
    fn the_version_is_a_plain_semver_with_nothing_appended() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "not major.minor.patch: {VERSION}");
        for p in parts {
            assert!(
                p.chars().all(|c| c.is_ascii_digit()),
                "{VERSION} has a non-numeric component, so it is not orderable"
            );
        }
    }

    /// **The notice names this build, its licence and where it came from.**
    ///
    /// Asserted by content rather than against a copy of the string, because a
    /// test that repeats the literal only proves the literal was not edited by
    /// accident. What matters is that somebody holding the binary can answer
    /// all three questions from it, and that the version in it is this build's
    /// rather than one frozen when the constant was written.
    #[test]
    fn the_notice_carries_the_version_the_licence_and_the_project() {
        assert!(NOTICE.contains(VERSION), "{NOTICE} does not name this build");
        assert!(NOTICE.contains("GPL-3.0-or-later"), "{NOTICE} does not state the licence");
        assert!(
            NOTICE.contains("https://github.com/luohoa97/cordial"),
            "{NOTICE} does not say where this came from"
        );
    }

    /// The commit never leaks into the version half of the display string.
    #[test]
    fn full_keeps_the_commit_in_brackets() {
        let text = full();
        assert!(text.starts_with(VERSION), "{text} does not start with {VERSION}");
        match GIT_SHA {
            Some(sha) => {
                assert_eq!(text, format!("{VERSION} ({sha})"));
                assert!(text.contains('('), "the commit must be bracketed: {text}");
            }
            None => assert_eq!(text, VERSION),
        }
    }

    #[test]
    fn only_the_upstream_repository_is_official() {
        assert_eq!(origin_from(Some("luohoa97/cordial"), None), Origin::Official);
        // A fork's own release workflow stamps its own name.
        assert_eq!(
            origin_from(Some("somebody/cordial"), Some("https://github.com/runner/checkout")),
            Origin::Unofficial { remote: Some("https://github.com/somebody/cordial".into()) }
        );
        // A local build: no stamp, and the checkout's remote if it had one.
        assert_eq!(
            origin_from(None, Some("https://github.com/somebody/cordial")),
            Origin::Unofficial { remote: Some("https://github.com/somebody/cordial".into()) }
        );
        assert_eq!(origin_from(None, None), Origin::Unofficial { remote: None });
        // An empty or malformed stamp is no stamp.
        assert_eq!(origin_from(Some("  "), None), Origin::Unofficial { remote: None });
        assert_eq!(origin_from(Some("not a repo"), None), Origin::Unofficial { remote: None });
    }

    #[test]
    fn the_origin_reads_as_one_plain_line() {
        assert_eq!(Origin::Official.text(), "Official build");
        assert_eq!(Origin::Unofficial { remote: None }.text(), "Unofficial build");
        assert_eq!(
            Origin::Unofficial { remote: Some("https://github.com/a/b".into()) }.text(),
            "Unofficial build from https://github.com/a/b"
        );
    }

    #[test]
    fn a_github_remote_yields_its_slug_and_nothing_else_does() {
        let github = Origin::Unofficial { remote: Some("https://github.com/a/b".into()) };
        assert_eq!(github.github_slug().as_deref(), Some("a/b"));
        let deep = Origin::Unofficial { remote: Some("https://github.com/a/b/c".into()) };
        assert_eq!(deep.github_slug(), None);
        let elsewhere = Origin::Unofficial { remote: Some("https://codeberg.org/a/b".into()) };
        assert_eq!(elsewhere.github_slug(), None);
        assert_eq!(Origin::Official.github_slug(), None);
    }
}
