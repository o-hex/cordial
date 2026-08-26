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

/// Why a build is being obtained, which decides how the sources are ordered.
///
/// **These are not the same question and treating them as one was a bug.**
/// `all()` returns the free source first, and for a first run that is exactly
/// right: most machines that will run Cordial already have this file, and
/// fetching a second copy from a mirror would be slower, more exposed and no
/// more trustworthy. But the same ordering applied to an update means the local
/// copy always wins, so **anybody with Sober installed could never receive a
/// newer build** -- "Download Roblox" would reinstall the build they already
/// had, for ever, and report success.
///
/// So an update asks every source what it has and takes the newest, and only
/// falls back to order when the versions cannot be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Any usable build. Cheapest first.
    Any,
    /// The newest available. Compares across sources.
    Newest,
}

/// Every source, in the order they should be tried.
///
/// Zero-network first. See [`Want`] for when that ordering is wrong.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![Box::new(local::OnThisMachine::default()), Box::new(mirror::ApkPure::default())]
}

/// The source named `name`, for a user who has chosen one.
pub fn named(name: &str) -> Option<Box<dyn Provider>> {
    all().into_iter().find(|p| p.name().eq_ignore_ascii_case(name))
}

/// Sort the sources newest-first, leaving ones that cannot answer at the back.
///
/// A source that fails to answer is not dropped here, only deprioritised: it
/// may still be the only one that can actually deliver, and `obtain` reports
/// what each one said if none of them can.
fn order_by_version(
    sources: Vec<Box<dyn Provider>>,
    progress: &mut dyn FnMut(Progress),
    absent: &mut Vec<String>,
) -> Vec<Box<dyn Provider>> {
    let mut scored: Vec<(Option<Vec<u64>>, usize, Box<dyn Provider>)> = Vec::new();
    for (position, source) in sources.into_iter().enumerate() {
        let version = match source.newest(progress) {
            Ok(v) => Some(numeric(&v.name)),
            Err(Unreachable::NoSource { why }) => {
                absent.push(format!("{}: {why}", source.name()));
                None
            }
            // Not fatal here. A mirror being down must not stop a local build
            // from being found, and the real attempt below will report it if
            // it turns out to matter.
            Err(e) => {
                absent.push(format!("{}: {e}", source.name()));
                None
            }
        };
        scored.push((version, position, source));
    }
    // Newest first; ties and unanswerables keep `all()`'s order, which puts the
    // free source ahead of the paid one.
    scored.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => y.cmp(x).then(a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    scored.into_iter().map(|(_, _, s)| s).collect()
}

/// A version as comparable numbers.
///
/// **String comparison is wrong here and quietly so**: `2.734.917` sorts below
/// `2.734.916` only if you compare component by component as numbers, and
/// lexically `"2.734.9"` beats `"2.734.10"`. The two sources also disagree on
/// shape -- the engine reads `2.734.0.917` and the mirror says `2.734.917` --
/// so anything numeric is compared and anything else is ignored rather than
/// guessed at.
fn numeric(version: &str) -> Vec<u64> {
    version.split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse().ok()).collect()
}

/// A build that was obtained and checked.
#[derive(Debug, Clone)]
pub struct Obtained {
    pub archives: Archives,
    pub version: Available,
    /// Which source it came from, for the message that follows.
    pub provider: &'static str,
    /// The certificate that signed it, so a caller can say whose build this is
    /// rather than merely that a check passed.
    pub certificate_sha256: String,
}

/// Get the build, from the first source that has it, and verify it.
///
/// **The signature check is here rather than left to the caller**, and that is
/// the point of the function. [ADR-025](../../../docs/adr/ADR-025-fetching-from-a-third-party-mirror.md)
/// makes a non-Roblox source acceptable only because the archive is verified,
/// and a fetch API that returned unverified paths would make forgetting the
/// check the easy thing to do. There is nothing here that hands back bytes
/// nobody has checked.
///
/// ## Which failures move to the next source and which stop everything
///
/// [`Unreachable::NoSource`] means a source has nothing -- no local build, an
/// empty directory -- and the next one is tried. **Anything else stops.** In
/// particular a signature refusal is never a reason to quietly try somewhere
/// else: an archive that fails verification is the one event this whole design
/// exists to notice, and moving on to the next provider would turn the loudest
/// possible signal into a slightly slower success.
pub fn obtain(
    preferred: Option<&str>,
    want: Want,
    into: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<Obtained, Unreachable> {
    let sources = match preferred {
        Some(name) => vec![named(name).ok_or_else(|| Unreachable::NoSource {
            why: format!(
                "there is no source called {name}. Cordial knows: {}",
                all().iter().map(|p| p.name()).collect::<Vec<_>>().join(", ")
            ),
        })?],
        None => all(),
    };

    let trusted = crate::apk_signature::pinned();
    let mut absent: Vec<String> = Vec::new();

    // For Newest, ask everybody first and put the winner in front. Asking is
    // one small request per source and no bytes; the alternative is the pin
    // described on `Want`.
    let sources = match want {
        Want::Any => sources,
        Want::Newest => order_by_version(sources, progress, &mut absent),
    };

    for source in sources {
        let version = match source.newest(progress) {
            Ok(v) => v,
            Err(Unreachable::NoSource { why }) => {
                absent.push(format!("{}: {why}", source.name()));
                continue;
            }
            Err(e) => return Err(e),
        };

        let archives = match source.fetch(&version, into, progress) {
            Ok(a) => a,
            Err(Unreachable::NoSource { why }) => {
                absent.push(format!("{}: {why}", source.name()));
                continue;
            }
            Err(e) => return Err(e),
        };

        // Every distinct file, and only once each: a monolithic APK is not
        // hashed twice to satisfy the shape of the rule.
        let mut certificate: Option<String> = None;
        for file in archives.distinct() {
            progress(Progress::Verifying {
                file: file.file_name().unwrap_or_default().to_string_lossy().to_string(),
            });
            let signer = crate::apk_signature::verify_signed_by(file, &trusted).map_err(|e| {
                Unreachable::Malformed { url: file.display().to_string(), why: e.to_string() }
            })?;
            // **The two halves must share a certificate.** Each being signed by
            // some pinned key is not enough: two archives signed by different
            // pinned keys would each pass on its own, and pairing them installs
            // an engine from one release beside assets from another. Android's
            // installer enforces the same rule for the same reason.
            match &certificate {
                None => certificate = Some(signer.certificate_sha256),
                Some(first) if *first != signer.certificate_sha256 => {
                    return Err(Unreachable::Malformed {
                        url: file.display().to_string(),
                        why: "the two halves of this build were signed by different \
                              certificates, so they are not two halves of one build"
                            .into(),
                    })
                }
                Some(_) => {}
            }
        }
        let certificate = certificate.ok_or_else(|| Unreachable::NoSource {
            why: "the source returned no archives at all".into(),
        })?;

        return Ok(Obtained {
            archives,
            version,
            provider: source.name(),
            certificate_sha256: certificate,
        });
    }

    Err(Unreachable::NoSource {
        why: format!("no source had a Roblox build.\n  {}", absent.join("\n  ")),
    })
}

/// Obtain a build and make it the one Cordial launches.
///
/// The whole thing, and the one call the shell needs: choose a source, fetch,
/// verify against the pinned certificates, apply ADR-014's extraction refusals,
/// extract the engine, and swap it in in an order that keeps a working build
/// working if any step fails.
///
/// `staging` is where a downloading source writes. It is emptied first, because
/// anything an interrupted attempt left there was never verified and is exactly
/// the file the ordering exists to keep away from the live build.
pub fn obtain_and_install(
    preferred: Option<&str>,
    want: Want,
    progress: &mut dyn FnMut(Progress),
) -> Result<(Obtained, crate::install::Installed), Unreachable> {
    let staging = crate::install::build_dir().join(".fetching");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;

    let outcome = (|| {
        let obtained = obtain(preferred, want, &staging, progress)?;
        let mut named: Vec<(&'static str, PathBuf)> =
            vec![(crate::install::BASE_APK, obtained.archives.base.clone())];
        // A monolithic archive is one file and must be named once. Handing the
        // same path in twice would have `adopt` copy it to both names and then
        // look for the engine in whichever it found first, which works by
        // accident and doubles the disk it takes.
        if obtained.archives.split != obtained.archives.base {
            named.push((crate::install::SPLIT_APK, obtained.archives.split.clone()));
        }

        let installed = crate::install::adopt(
            &named,
            &crate::install::build_dir(),
            &crate::install::engine_dir(),
            &crate::install::engine_dir().join(".incoming"),
            &mut |_, _, _| {},
        )
        .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;
        Ok((obtained, installed))
    })();

    let _ = std::fs::remove_dir_all(&staging);
    outcome
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

    /// Naming a source that does not exist must not silently fall back to one
    /// that does. A typo in a setting should say so, not quietly use the
    /// network when the user asked for the local build.
    /// **The comparison a string sort gets wrong, both ways.**
    #[test]
    fn versions_compare_as_numbers_and_not_as_text() {
        assert!(numeric("2.734.917") > numeric("2.734.916"));
        // Lexically "2.734.9" beats "2.734.10". Numerically it does not.
        assert!(numeric("2.734.10") > numeric("2.734.9"));
        // The two sources disagree on shape: the engine reads four components
        // and the mirror says three. Comparing what is there is the honest
        // answer; inventing the missing one is not.
        assert_eq!(numeric("2.734.0.917"), vec![2, 734, 0, 917]);
        assert_eq!(numeric("2.734.917"), vec![2, 734, 917]);
    }

    #[test]
    fn an_unknown_preferred_source_is_an_error_and_not_a_fallback() {
        let dir = std::env::temp_dir().join("cordial-obtain-unknown");
        std::fs::create_dir_all(&dir).expect("scratch");
        let mut noise = |_: Progress| {};
        let e = obtain(Some("no-such-source"), Want::Any, &dir, &mut noise).unwrap_err();
        assert!(e.to_string().contains("no-such-source"), "{e}");
        assert!(e.to_string().contains("local"), "the error should list what does exist: {e}");
    }
}
