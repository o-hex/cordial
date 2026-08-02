//! The index a plugin is published in.
//!
//! An index is one JSON file listing what is available. It is a static file on
//! purpose: served straight out of a git repository, it is auditable (every
//! change is a diff somebody can read), forkable (a user who dislikes what is
//! in it can point Cordial at their own), and costs nothing to host. That is
//! Homebrew's tap arrangement rather than a package server, and the reason is
//! the same — a registry that is a service is a registry only its operator can
//! check.
//!
//! ```json
//! {
//!   "format": 1,
//!   "plugins": [
//!     {
//!       "id": "flag-inspector",
//!       "name": "Flag Inspector",
//!       "version": "1.0.0",
//!       "capabilities": ["flags.read", "log"],
//!       "dependencies": {},
//!       "url": "https://example.invalid/flag-inspector-1.0.0.tar.zst",
//!       "hash": "sha256:0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"
//!     }
//!   ]
//! }
//! ```
//!
//! `capabilities` and `dependencies` are repeated here rather than left to the
//! archive because the user has to be shown what an install will do *before*
//! anything is downloaded, and a plan cannot be built out of files that have
//! not been fetched yet. That duplication is only safe because it is checked:
//! [`crate::unpack`] compares the extracted `plugin.json` against the entry it
//! was installed as and refuses a mismatch, so an index cannot advertise one
//! set of capabilities and ship another.
//!
//! **Who hosts an index, and who decides what goes in one, is not settled.**
//! Nothing here names a URL, and nothing here assumes there is only one. A
//! curated list maintained by whoever maintains Cordial and a self-hosted index
//! a user points at themselves are the same file in the same format, and
//! [`Index::merged`] refuses rather than picks when two of them disagree —
//! which is how this module avoids deciding a policy question that is the
//! project owner's to decide.
//!
//! ## Signing is not implemented, and this module says so in its function names
//!
//! The intended scheme is a detached minisign signature (Ed25519) beside the
//! index as `index.json.minisig`, verified against a key shipped with Cordial
//! before the JSON is parsed at all. It is chosen because it is boring: one
//! small well-specified format, an existing Rust implementation, and a key that
//! fits on a line — as against OpenPGP, which brings a keyring and a trust
//! model nobody wants, and Sigstore, which brings a network dependency to an
//! operation whose whole appeal is that it is a static file.
//!
//! None of that exists yet. The only entry point is
//! [`Index::parse_unverified`], named so that no caller can pass an index to
//! something trusting without the word "unverified" being in the line they
//! wrote. Until the signature check lands, an index is exactly as trustworthy
//! as the transport it arrived over, and the content hash in each entry
//! protects the *download* against a tampered mirror and nothing else — it
//! cannot protect against a tampered index, because a tampered index would
//! carry a matching hash.

use crate::capability::Capability;
use crate::manifest::{Dependency, Requirement};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The only index format this Cordial understands.
///
/// Refused rather than best-guessed when it is anything else, for the reason
/// `manifest::parse` refuses an unknown capability: an installer that skips the
/// parts of a newer index it does not recognise is an installer that will
/// cheerfully install a plugin whose declared capabilities it could not read.
pub const FORMAT: u32 = 1;

/// A `sha256:` content hash of a distribution archive.
///
/// The algorithm is part of the string rather than assumed. A field called
/// `hash` holding sixty-four hex characters is a field that means whatever the
/// publisher's script happened to run, and the first time that becomes a
/// question is the first time it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// The hash of some bytes, as [`crate::unpack`] computes it over a
    /// downloaded archive.
    pub fn of(bytes: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(bytes);
        let out = h.finalize();
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&out);
        ContentHash(digest)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let hex = text.strip_prefix("sha256:").ok_or_else(|| {
            format!("{text:?} does not name its algorithm; write \"sha256:<64 hex digits>\"")
        })?;
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(format!("{text:?} is not 64 lower-case hex digits after \"sha256:\""));
        }
        let mut digest = [0u8; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|e| format!("{text:?}: {e}"))?;
        }
        Ok(ContentHash(digest))
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// One published version of one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub capabilities: BTreeSet<Capability>,
    pub dependencies: Vec<Dependency>,
    pub url: String,
    pub hash: ContentHash,
}

impl Entry {
    pub fn requirement_on(&self, id: &str) -> Option<&Requirement> {
        self.dependencies.iter().find(|d| d.id == id).map(|d| &d.req)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawIndex {
    format: u32,
    #[serde(default)]
    plugins: Vec<RawEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawEntry {
    id: String,
    #[serde(default)]
    name: String,
    version: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    url: String,
    hash: String,
}

/// Everything one index file offers.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub entries: Vec<Entry>,
}

impl Index {
    /// Parse an index whose signature has **not** been checked, because no
    /// signature check exists yet. See the module note; the name is the whole
    /// of the warning and it is deliberately impossible to call this without
    /// writing it down.
    pub fn parse_unverified(text: &str) -> Result<Index, String> {
        let raw: RawIndex = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if raw.format != FORMAT {
            return Err(format!(
                "index format {} is not {FORMAT}; this Cordial cannot read it, and reading the \
                 parts it recognises would mean installing a plugin whose declarations it could \
                 not",
                raw.format
            ));
        }
        let mut entries = Vec::new();
        for r in raw.plugins {
            entries.push(entry_from(r)?);
        }
        let mut seen = BTreeSet::new();
        for e in &entries {
            if !seen.insert((e.id.clone(), e.version.clone())) {
                return Err(format!(
                    "{} {} appears twice in one index, with no way to tell which is meant",
                    e.id, e.version
                ));
            }
        }
        // Sorted so that "the newest version that matches" is a question about
        // the plugins on offer rather than about the order somebody happened
        // to write them down in.
        entries.sort_by(|a, b| a.id.cmp(&b.id).then(a.version.cmp(&b.version)));
        Ok(Index { entries })
    }

    pub fn get(&self, id: &str, version: &Version) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id && &e.version == version)
    }

    /// Every published version of `id`, oldest first.
    ///
    /// The id is copied into the returned iterator rather than borrowed, so a
    /// caller can hold the result across whatever it likes; the alternative
    /// ties the borrow of a plugin name to the borrow of the whole index, and
    /// the resolver holds one while it is still collecting the other.
    pub fn offers<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a Entry> + 'a {
        let id = id.to_string();
        self.entries.iter().filter(move |e| e.id == id)
    }

    /// The newest version of `id` satisfying every requirement placed on it.
    pub fn best_match<'a>(&'a self, id: &str, reqs: &[&Requirement]) -> Option<&'a Entry> {
        self.offers(id)
            .filter(|e| reqs.iter().all(|r| r.matches(&e.version)))
            .max_by(|a, b| a.version.cmp(&b.version))
    }

    /// One catalogue out of several indexes.
    ///
    /// A curated list and an index a user hosts themselves are the same file in
    /// the same format, so combining them is just concatenation — except when
    /// two of them publish the same id and version pointing at different bytes.
    /// That is refused rather than resolved by precedence, because precedence
    /// *is* the policy question this project has not answered: whichever order
    /// were picked, an index a user added for one plugin would silently be
    /// deciding what a different plugin's download is. Refusing keeps the
    /// disagreement where somebody can look at it.
    pub fn merged(indexes: &[Index]) -> Result<Index, String> {
        let mut by_key: BTreeMap<(String, Version), &Entry> = BTreeMap::new();
        for index in indexes {
            for e in &index.entries {
                let key = (e.id.clone(), e.version.clone());
                if let Some(prior) = by_key.get(&key) {
                    if prior.hash != e.hash || prior.url != e.url {
                        return Err(format!(
                            "two indexes publish {} {} differently ({} against {}); nothing here \
                             decides which one wins",
                            e.id, e.version, prior.url, e.url
                        ));
                    }
                    continue;
                }
                by_key.insert(key, e);
            }
        }
        Ok(Index { entries: by_key.into_values().cloned().collect() })
    }
}

fn entry_from(r: RawEntry) -> Result<Entry, String> {
    if !crate::manifest::is_valid_id(&r.id) {
        return Err(format!(
            "index entry id {:?} may only contain letters, digits, dashes and underscores",
            r.id
        ));
    }
    let version = Version::parse(&r.version)
        .map_err(|e| format!("{}: version {:?} is not a semantic version ({e})", r.id, r.version))?;
    let mut capabilities = BTreeSet::new();
    for name in &r.capabilities {
        match Capability::parse(name) {
            Some(c) => {
                capabilities.insert(c);
            }
            // The same refusal `manifest::parse` makes, and for a sharper
            // reason here: a capability name this Cordial cannot read is one it
            // cannot show the user before they approve the install, and an
            // approval given without seeing what was asked for is not one.
            None => return Err(format!("{}: unknown capability {name:?}", r.id)),
        }
    }
    let mut dependencies = Vec::new();
    for (id, req) in &r.dependencies {
        dependencies.push(Dependency::new(id, req).map_err(|e| format!("{}: {e}", r.id))?);
    }
    // Not because the hash would be weaker over plain HTTP — it would not, the
    // hash is in the index and the index is the thing being trusted — but
    // because a URL is the one field here that a fetcher will act on, and the
    // set of schemes it can be talked into is worth being a short list. `file:`
    // in particular has no business in a published index.
    if !r.url.starts_with("https://") {
        return Err(format!("{}: download url {:?} is not https", r.id, r.url));
    }
    let hash = ContentHash::parse(&r.hash).map_err(|e| format!("{}: {e}", r.id))?;
    Ok(Entry { id: r.id, name: r.name, version, capabilities, dependencies, url: r.url, hash })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn index(json: &str) -> Index {
        Index::parse_unverified(json).unwrap()
    }

    const ONE: &str = r#"{
      "format": 1,
      "plugins": [
        {
          "id": "flag-inspector",
          "name": "Flag Inspector",
          "version": "1.0.0",
          "capabilities": ["flags.read", "log"],
          "dependencies": {},
          "url": "https://example.invalid/flag-inspector-1.0.0.tar.zst",
          "hash": "sha256:0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"
        }
      ]
    }"#;

    #[test]
    fn an_index_parses() {
        let i = index(ONE);
        let e = &i.entries[0];
        assert_eq!(e.id, "flag-inspector");
        assert_eq!(e.version, Version::new(1, 0, 0));
        assert!(e.capabilities.contains(&Capability::FlagsRead));
        assert_eq!(e.hash.to_string().len(), "sha256:".len() + 64);
    }

    #[test]
    fn a_hash_must_name_its_algorithm() {
        assert!(ContentHash::parse(
            "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"
        )
        .is_err());
        assert!(ContentHash::parse("sha256:nothex").is_err());
        assert!(ContentHash::parse("sha256:0E5751C026E543B2E8AB2EB06099DAA1D1E5DF47778F7787FAAB45CDF12FE3A8").is_err());
        assert!(ContentHash::parse(
            "sha256:0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"
        )
        .is_ok());
    }

    #[test]
    fn a_hash_round_trips_through_its_text_form() {
        let h = ContentHash::of(b"a");
        assert_eq!(ContentHash::parse(&h.to_string()).unwrap(), h);
        assert_ne!(ContentHash::of(b"a"), ContentHash::of(b"b"));
    }

    #[test]
    fn an_unreadable_index_format_is_refused_rather_than_partly_read() {
        let text = ONE.replace("\"format\": 1", "\"format\": 2");
        assert!(Index::parse_unverified(&text).is_err());
    }

    #[test]
    fn an_unknown_capability_in_an_index_is_refused() {
        // The user has to be shown what they are approving, and a name this
        // Cordial cannot read cannot be shown.
        let text = ONE.replace("\"flags.read\"", "\"process.spawn\"");
        assert!(Index::parse_unverified(&text).is_err());
    }

    #[test]
    fn a_non_https_download_url_is_refused() {
        for bad in ["http://example.invalid/x.tar.zst", "file:///etc/shadow", "/etc/shadow"] {
            let text = ONE.replace("https://example.invalid/flag-inspector-1.0.0.tar.zst", bad);
            assert!(Index::parse_unverified(&text).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn one_id_and_version_twice_in_an_index_is_refused() {
        let text = ONE.replace(
            "\"plugins\": [",
            "\"plugins\": [
        {
          \"id\": \"flag-inspector\",
          \"version\": \"1.0.0\",
          \"url\": \"https://example.invalid/other.tar.zst\",
          \"hash\": \"sha256:1111111111111111111111111111111111111111111111111111111111111111\"
        },",
        );
        assert!(Index::parse_unverified(&text).is_err());
    }

    #[test]
    fn two_indexes_disagreeing_about_one_release_are_refused_rather_than_ranked() {
        // Precedence between a curated index and one a user added is a policy
        // question nobody has answered. Picking one silently would let an index
        // added for a single plugin decide where a different plugin's bytes
        // come from.
        let mine = index(&ONE.replace(
            "https://example.invalid/flag-inspector-1.0.0.tar.zst",
            "https://elsewhere.invalid/flag-inspector-1.0.0.tar.zst",
        ));
        let theirs = index(ONE);
        assert!(Index::merged(&[theirs.clone(), mine]).is_err());
        // The same release from two indexes is not a disagreement.
        assert_eq!(Index::merged(&[theirs.clone(), theirs]).unwrap().entries.len(), 1);
    }

    #[test]
    fn the_best_match_is_the_newest_version_satisfying_every_requirement() {
        let text = r#"{"format":1,"plugins":[
          {"id":"b","version":"1.0.0","url":"https://x.invalid/b1.tar.zst","hash":"sha256:1111111111111111111111111111111111111111111111111111111111111111"},
          {"id":"b","version":"1.4.0","url":"https://x.invalid/b2.tar.zst","hash":"sha256:2222222222222222222222222222222222222222222222222222222222222222"},
          {"id":"b","version":"2.0.0","url":"https://x.invalid/b3.tar.zst","hash":"sha256:3333333333333333333333333333333333333333333333333333333333333333"}
        ]}"#;
        let i = index(text);
        let caret_one = Requirement::parse("^1.0.0").unwrap();
        let exact = Requirement::parse("=1.0.0").unwrap();
        assert_eq!(i.best_match("b", &[&caret_one]).unwrap().version, Version::new(1, 4, 0));
        assert_eq!(i.best_match("b", &[&exact]).unwrap().version, Version::new(1, 0, 0));
        let caret_two = Requirement::parse("^2.0.0").unwrap();
        assert!(i.best_match("b", &[&caret_one, &caret_two]).is_none());
    }
}
