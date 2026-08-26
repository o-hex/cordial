//! Where the Roblox build comes from, as a set of interchangeable sources.
//!
//! [ADR-015](../../../docs/adr/ADR-015-fetching-the-roblox-build.md) permits
//! Cordial to download the official build to the user's own machine and never
//! to ship one, and for a long time this crate could honour the first half only
//! in theory: Roblox publishes no Android download, so
//! [`crate::download::Source::official`] refuses in three plain sentences and
//! the user was left to find an APK themselves. That is the single largest
//! thing standing between somebody and a working client, and it is the reason
//! the quickstart's first instruction is to install a different program.
//!
//! [ADR-025](../../../docs/adr/ADR-025-fetching-from-a-third-party-mirror.md)
//! is what changed. It permits a distributor who is not Roblox, on one
//! condition, and the condition is the whole of it: the archive is worthless
//! until [`crate::apk_signature`] says Roblox's own signing certificate signed
//! those exact bytes. A mirror that alters a byte is caught; a mirror that
//! serves something else entirely is caught; a mirror that is compromised is
//! caught. What a mirror can still do is go down, lie about what versions
//! exist, or watch who asks -- which is why the choice of source is the user's
//! and why the one that needs no network is tried first.
//!
//! ## Why this is a trait and not a function with a match in it
//!
//! Because the two sources here are not variations on a theme. [`local`] reads
//! a file that is already on the disk and touches no network at all; [`mirror`]
//! speaks an undocumented binary protocol to a third party and validates every
//! URL it is handed. They share an output type and nothing else, and a single
//! function covering both would be a long `if` whose branches never meet.
//!
//! It also means the next one is additive. Roblox publishing an Android
//! deployment path is the source everyone would want, and the day that happens
//! it is a new file in this directory and a line in [`all`], not a rewrite.
//!
//! ## The order matters and is not alphabetical
//!
//! [`all`] returns the zero-network source first. On a machine that already has
//! the build -- which is most machines that have ever run Sober, and Sober is
//! what the README tells people to install -- the whole of [`mirror`] is
//! skipped: no request, no third party, no 150 MB, and nothing for a metered
//! connection to object to. Reaching the network to fetch a file that is
//! already present would be the worst version of this feature.

use crate::Unreachable;
use std::path::{Path, PathBuf};

pub mod local;
pub mod mirror;

/// A version a provider says it can supply.
///
/// The code is Android's monotonic `versionCode` and the name is what a person
/// recognises. Both are kept because they answer different questions: the code
/// orders two builds correctly and the name is the only one worth putting in a
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub name: String,
    pub code: u64,
}

/// The archives a build is made of, once they are on disk.
///
/// **`base` and `split` are frequently the same path**, and that is not a bug
/// to tidy away. Roblox's own distribution splits the assets from the engine;
/// mirrors often serve one monolithic APK carrying both. Callers care whether
/// `assets/` and `lib/x86_64/libroblox.so` are reachable, not how many files
/// that took, and [`crate::install`] already checks for the contents rather
/// than the names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archives {
    pub base: PathBuf,
    pub split: PathBuf,
}

impl Archives {
    /// The distinct files, so a caller verifying signatures does not verify a
    /// monolithic 150 MB archive twice to satisfy the shape of a rule.
    pub fn distinct(&self) -> Vec<&Path> {
        if self.base == self.split {
            vec![self.base.as_path()]
        } else {
            vec![self.base.as_path(), self.split.as_path()]
        }
    }
}

/// How far along a fetch is, for a progress bar that is not a lie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Asking a source what it has. Cheap, and worth showing because a
    /// provider being down looks like a hang otherwise.
    Asking { provider: &'static str },
    /// Bytes arriving. `total` is absent when the source did not say, which is
    /// common enough that a bar assuming otherwise shows nonsense.
    Fetching { file: String, done: u64, total: Option<u64> },
    /// Checking the signature. On a 150 MB archive this is seconds of full-tilt
    /// hashing with no network traffic, which reads as a stall unless it is
    /// named.
    Verifying { file: String },
}

/// A source of the Roblox Android build.
pub trait Provider {
    /// A short name, for messages. Appears in failures, so it is the word the
    /// user will search for.
    fn name(&self) -> &'static str;

    /// Whether using this source makes network requests.
    ///
    /// [`crate::metered`] asks NetworkManager whether somebody is paying by the
    /// megabyte for this connection, and a source that answers `false` here
    /// does not need to be asked at all.
    fn needs_network(&self) -> bool;

    /// The newest build this source can supply.
    fn newest(&self, progress: &mut dyn FnMut(Progress)) -> Result<Available, Unreachable>;

    /// Put the archives for `version` into `into`, and say where they landed.
    ///
    /// `into` must exist and be empty. **A provider that fails part way leaves
    /// nothing behind**: there is no such thing as most of a build, and a
    /// half-populated directory that a later run mistakes for a complete one is
    /// how a client ends up with assets from one release and an engine from
    /// another.
    fn fetch(
        &self,
        version: &Available,
        into: &Path,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Archives, Unreachable>;
}

/// Every source, in the order they should be tried.
///
/// Zero-network first. See the module header for why that ordering is the
/// feature rather than an implementation detail.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![Box::new(local::OnThisMachine::default()), Box::new(mirror::ApkPure::default())]
}

/// The source named `name`, for a user who has chosen one.
pub fn named(name: &str) -> Option<Box<dyn Provider>> {
    all().into_iter().find(|p| p.name().eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zero_network_source_is_tried_first() {
        let providers = all();
        assert!(
            !providers[0].needs_network(),
            "a source that needs no network must come before one that does, or Cordial \
             downloads a file it already has"
        );
    }

    #[test]
    fn every_source_has_a_distinct_name_and_can_be_asked_for_by_it() {
        let names: Vec<_> = all().iter().map(|p| p.name()).collect();
        for n in &names {
            assert!(named(n).is_some(), "{n} is listed and cannot be looked up");
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "two sources share a name: {names:?}");
    }

    #[test]
    fn an_unknown_source_is_none_rather_than_a_default() {
        assert!(named("whatever-the-user-typed").is_none());
    }

    #[test]
    fn a_monolithic_archive_is_verified_once_and_not_twice() {
        let one = Archives { base: "/x/a.apk".into(), split: "/x/a.apk".into() };
        assert_eq!(one.distinct().len(), 1);
        let two = Archives { base: "/x/a.apk".into(), split: "/x/b.apk".into() };
        assert_eq!(two.distinct().len(), 2);
    }
}
