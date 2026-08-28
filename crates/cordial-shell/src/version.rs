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
}
