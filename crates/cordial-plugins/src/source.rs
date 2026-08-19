//! Where an index's bytes come from — abstracted, so a local checkout today
//! and a future HTTPS fetch are the same shape to everything that reads one.
//!
//! [ADR-014](../../../docs/adr/ADR-014-plugin-registry-and-unpacking.md)
//! settles the *format* of an index and refuses to settle *who hosts one*:
//! "nothing in Cordial names a URL for one, and nothing assumes there is only
//! one." [`IndexSource`] is the seam that decision drops into later. It is
//! deliberately two calls — fetch the index document, fetch one entry's
//! archive — because those are the only two things anything downstream ever
//! needs moved over a network, and an HTTPS implementation is two `ureq`
//! calls behind the same trait once somebody decides which host the default
//! points at.
//!
//! **`cordial-plugins` still adds no HTTP client for this.** Doing that now
//! would itself be a form of the policy decision ADR-014 declines to make —
//! it would mean choosing a networking stack, and implicitly a transport
//! story, for a host nobody has named yet. [`LocalFileSource`] is not a stub
//! standing in for that decision. It is real and useful on its own: ADR-014
//! already designs an index to be "served straight out of a git repository",
//! and a local clone of one — `git clone` a tap, point Cordial at the
//! checkout — is that arrangement with the network step removed, not a
//! different one. It is also what makes every marketplace refusal in
//! [`crate::marketplace`] reachable from a test with no network, the same
//! property [`crate::resolve`] and [`crate::unpack`] already have.

use crate::registry::Entry;
use std::fmt;
use std::path::PathBuf;

/// Why a source could not produce what was asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Read the thing, but something about the filesystem or transport
    /// itself failed — not "it is not there", which is [`Refusal::NotFound`].
    Io(String),
    /// The index, the signature, or an archive is simply not present at this
    /// source. Distinct from `Io` because a missing `index.json.minisig` is
    /// routine — plenty of sources will have none — while other failures are
    /// not.
    NotFound(String),
    /// What was asked for could not even be turned into a request — an entry
    /// whose `url` has no usable file name, for instance.
    Invalid(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Io(m) => f.write_str(m),
            Refusal::Invalid(m) => f.write_str(m),
            Refusal::NotFound(m) => write!(f, "not found: {m}"),
        }
    }
}

/// An index document exactly as a source handed it over.
///
/// `json` is the raw text, unmodified — [`crate::sign::verify`] hashes these
/// exact bytes, so nothing here may re-encode or reformat them, and a source
/// implementation must not either. `signature` is the detached minisign
/// signature text beside it, when the source has one; `None` is not itself a
/// refusal; see the module note on [`IndexSource::fetch_index`].
#[derive(Debug, Clone)]
pub struct Fetched {
    pub json: String,
    pub signature: Option<String>,
}

/// Where an index, and the archives it lists, can be fetched from.
///
/// Nothing here does listing, search, or authentication — those are shaped by
/// whichever host eventually exists, and are not this project's to guess at.
/// What every future source needs, an HTTPS one included, is exactly these
/// two calls.
pub trait IndexSource: fmt::Debug {
    /// A short, human-readable name for this source, for a UI to show beside
    /// what it lists. ADR-014 is explicit that a marketplace UI must not let
    /// a listing read as an endorsement; saying *where* a plugin's index
    /// entry came from is part of that, and a source with no name at all
    /// invites skipping past the question.
    fn describe(&self) -> String;

    /// The index document, and its detached signature if one exists here.
    ///
    /// A source with no signature is not itself a refusal — [`LocalFileSource`]
    /// happily returns `signature: None` for a directory with no
    /// `index.json.minisig`. What an absent signature means for trust is a
    /// decision for whoever opens the index against a key
    /// ([`crate::marketplace::open`]), not for the source.
    fn fetch_index(&self) -> Result<Fetched, Refusal>;

    /// The bytes of `entry`'s distribution archive.
    ///
    /// Never trusted on its own: [`crate::unpack::install`] checks the result
    /// against `entry.hash` before anything is decompressed, exactly as it
    /// would for a download ADR-014 actually specifies. A source
    /// implementation has nothing extra to prove here — it only has to hand
    /// the bytes over.
    fn fetch_archive(&self, entry: &Entry) -> Result<Vec<u8>, Refusal>;
}

/// A directory on disk holding `index.json`, an optional
/// `index.json.minisig` beside it, and an `archives/` subdirectory holding
/// each entry's `.tar.zst` under the file name its `url` ends in.
///
/// Not a mock standing in for a future HTTPS source — see the module note.
/// It works fully offline and today, and is what a user who has `git clone`d
/// a tap, or been handed a directory by someone they trust, points Cordial
/// at.
#[derive(Debug, Clone)]
pub struct LocalFileSource {
    root: PathBuf,
}

impl LocalFileSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalFileSource { root: root.into() }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Where `entry`'s archive would live in this directory.
    ///
    /// Only the file name is ever taken out of `entry.url`, and only after
    /// this check — the same posture [`crate::unpack::extract_into`] takes
    /// with a path found inside an archive, applied here to a path built from
    /// a URL instead. A crafted `url` containing `..` or an embedded `/`
    /// cannot walk the result anywhere outside `archives/`, because nothing
    /// but the trailing segment is ever read from it.
    fn archive_path(&self, entry: &Entry) -> Result<PathBuf, Refusal> {
        let name = entry.url.rsplit('/').next().unwrap_or("");
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(Refusal::Invalid(format!(
                "{:?} has no file name a local mirror could look up",
                entry.url
            )));
        }
        Ok(self.root.join("archives").join(name))
    }
}

impl IndexSource for LocalFileSource {
    fn describe(&self) -> String {
        format!("local index at {}", self.root.display())
    }

    fn fetch_index(&self) -> Result<Fetched, Refusal> {
        let index_path = self.root.join("index.json");
        let json = std::fs::read_to_string(&index_path)
            .map_err(|e| Refusal::NotFound(format!("{}: {e}", index_path.display())))?;
        let sig_path = self.root.join("index.json.minisig");
        let signature = match std::fs::read_to_string(&sig_path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(Refusal::Io(format!("{}: {e}", sig_path.display()))),
        };
        Ok(Fetched { json, signature })
    }

    fn fetch_archive(&self, entry: &Entry) -> Result<Vec<u8>, Refusal> {
        let path = self.archive_path(entry)?;
        std::fs::read(&path).map_err(|e| Refusal::NotFound(format!("{}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{ContentHash, Index};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-source-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const ONE_ENTRY: &str = r#"{"format":1,"plugins":[{"id":"p","version":"1.0.0","capabilities":[],"dependencies":{},"url":"https://example.invalid/p-1.0.0.tar.zst","hash":"sha256:0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"}]}"#;

    #[test]
    fn a_local_source_reads_its_index_and_reports_no_signature_when_there_is_none() {
        let dir = tmp("index-only");
        std::fs::write(dir.join("index.json"), ONE_ENTRY).unwrap();
        let source = LocalFileSource::new(&dir);
        let fetched = source.fetch_index().unwrap();
        assert_eq!(fetched.json, ONE_ENTRY);
        assert!(fetched.signature.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_source_reads_a_signature_beside_the_index_when_one_exists() {
        let dir = tmp("index-and-sig");
        std::fs::write(dir.join("index.json"), ONE_ENTRY).unwrap();
        std::fs::write(dir.join("index.json.minisig"), "untrusted comment: x\nAAAA\ntrusted comment: y\nBBBB\n")
            .unwrap();
        let fetched = LocalFileSource::new(&dir).fetch_index().unwrap();
        assert!(fetched.signature.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_index_is_reported_as_not_found_rather_than_a_bare_io_error() {
        let dir = tmp("missing");
        let e = LocalFileSource::new(&dir).fetch_index().unwrap_err();
        assert!(matches!(e, Refusal::NotFound(_)), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_archive_is_looked_up_by_the_file_name_the_url_ends_in() {
        let dir = tmp("archive");
        std::fs::create_dir_all(dir.join("archives")).unwrap();
        std::fs::write(dir.join("archives/p-1.0.0.tar.zst"), b"pretend archive bytes").unwrap();
        let index = Index::parse_unverified(ONE_ENTRY).unwrap();
        let entry = &index.entries[0];
        let bytes = LocalFileSource::new(&dir).fetch_archive(entry).unwrap();
        assert_eq!(bytes, b"pretend archive bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_url_engineered_to_escape_the_archives_directory_is_refused_not_followed() {
        let dir = tmp("escape");
        std::fs::create_dir_all(&dir).unwrap();
        // Built by hand rather than through `Index::parse_unverified`, which
        // already refuses a non-https url; this proves the source's own
        // defence holds even if that earlier check were ever weakened or
        // bypassed by a different caller.
        let entry = Entry {
            id: "p".into(),
            name: "P".into(),
            version: semver::Version::new(1, 0, 0),
            capabilities: Default::default(),
            dependencies: Vec::new(),
            url: "https://example.invalid/../../../../etc/passwd".into(),
            hash: ContentHash::of(b"x"),
        };
        let e = LocalFileSource::new(&dir).fetch_archive(&entry).unwrap_err();
        assert!(matches!(e, Refusal::NotFound(_)), "{e}: took the last segment, did not escape");
        // And a url with no trailing segment at all is refused outright.
        let mut entry2 = entry.clone();
        entry2.url = "https://example.invalid/".into();
        assert!(matches!(LocalFileSource::new(&dir).fetch_archive(&entry2), Err(Refusal::Invalid(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
