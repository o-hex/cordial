//! Turning an index source into an install, without a second install path.
//!
//! This is the piece [ADR-014](../../../docs/adr/ADR-014-plugin-registry-and-unpacking.md)
//! describes and this crate did not yet have: given somewhere an index comes
//! from ([`crate::source`]) and, if there is one, a key to check it against
//! ([`crate::sign`]), resolve what the user asked for ([`crate::resolve`]),
//! fetch exactly the archives the plan needs, and install each one through
//! [`crate::unpack::install`] — the same hardened unpacker `install_local`
//! already goes through for a file picked off disk. Nothing here parses an
//! archive, checks a path, or writes a file itself; every one of those
//! belongs to `unpack` and stays there; a marketplace that reimplemented any
//! part of it, however small, would be the "new extraction path" this module
//! exists to not be.
//!
//! ## Verification is required, not offered
//!
//! [`open`] will parse an index with no key configured — [`Trust::Unconfigured`]
//! — because a UI has to be able to show what a source offers before a key
//! exists for it, the same way [`crate::resolve::resolve`] builds a plan
//! before grants are consulted so a user can be shown it. [`install`] is
//! where the difference is enforced: it refuses outright, before fetching a
//! single byte, if the index it was given was opened without a verified
//! signature. ADR-014's own words are the reason — "a marketplace UI...
//! [must] show what a plugin is asking for and let the user decide, rather
//! than presenting listing in an index as an endorsement" — and an installer
//! that unpacked an index nobody vouched for, on the strength of a content
//! hash the same unverified index also supplied, would be exactly the
//! endorsement-by-appearance ADR-014 warns against. See `crate::sign` for why
//! no key is configured by default.

use crate::capability::Capability;
use crate::manifest::{self, Dependency};
use crate::registry::Index;
use crate::resolve::{self, Plan, Refusal as ResolveRefusal};
use crate::sign::{self, Refusal as SignRefusal};
use crate::source::{IndexSource, Refusal as SourceRefusal};
use crate::unpack::{self, Refusal as UnpackRefusal};
use minisign_verify::PublicKey;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// What to check an opened index's signature against, if anything.
///
/// `Unconfigured` is the seam ADR-014 leaves open on purpose: Cordial ships no
/// default key, because shipping one means picking whose index is trusted out
/// of the box, and that is the hosting decision ADR-014 declines to make.
/// Nothing upgrades an `Unconfigured` source into a trusted one by itself —
/// see [`install`].
#[derive(Debug, Clone, Copy)]
pub enum Trust<'a> {
    Unconfigured,
    Key(&'a PublicKey),
}

/// An index, and whether it is trusted.
#[derive(Debug)]
pub struct Opened {
    pub index: Index,
    pub verified: bool,
    /// [`IndexSource::describe`], kept alongside the index so a UI showing an
    /// entry can still say where it came from after the source itself has
    /// gone out of scope.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRefusal {
    Source(String),
    /// A key was configured and the signature did not check out, or there was
    /// none to check. Distinct from a plain source failure: this is the index
    /// actively failing the one check that exists for it, not merely being
    /// unreachable.
    Signature(String),
    Parse(String),
}

impl fmt::Display for OpenRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenRefusal::Source(m) => write!(f, "could not fetch the index: {m}"),
            OpenRefusal::Signature(m) => write!(f, "index signature check failed: {m}"),
            OpenRefusal::Parse(m) => write!(f, "index could not be read: {m}"),
        }
    }
}

/// Fetch an index from `source` and, when `trust` names a key, check its
/// signature before a byte of it is parsed — the order [`sign::verify`]
/// documents and ADR-014 specifies.
///
/// With [`Trust::Unconfigured`] the index is still parsed — with
/// [`Index::parse_unverified`], named that way for the same reason it always
/// has been — so a source can be *browsed* with no key at all. [`install`] is
/// what refuses to act on the result; this function's job is only to say,
/// truthfully, whether what it returns was checked.
pub fn open(source: &dyn IndexSource, trust: Trust<'_>) -> Result<Opened, OpenRefusal> {
    let fetched = source.fetch_index().map_err(|e: SourceRefusal| OpenRefusal::Source(e.to_string()))?;
    match trust {
        Trust::Key(key) => {
            let signature = fetched.signature.as_deref().ok_or_else(|| {
                OpenRefusal::Signature(format!(
                    "{} offered no signature, and a key is configured for it",
                    source.describe()
                ))
            })?;
            let index = sign::verify(&fetched.json, signature, key)
                .map_err(|e: SignRefusal| OpenRefusal::Signature(e.to_string()))?;
            Ok(Opened { index, verified: true, source: source.describe() })
        }
        Trust::Unconfigured => {
            let index =
                Index::parse_unverified(&fetched.json).map_err(OpenRefusal::Parse)?;
            Ok(Opened { index, verified: false, source: source.describe() })
        }
    }
}

/// Why a plan built from an opened index was not installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallRefusal {
    /// [`open`] returned an index whose signature was never checked. Named
    /// apart from every refusal below because this one is decided before
    /// either of them is even reached — nothing is resolved and nothing is
    /// fetched.
    Unverified,
    Resolve(ResolveRefusal),
    Fetch { id: String, reason: String },
    Unpack { id: String, reason: String },
}

impl fmt::Display for InstallRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallRefusal::Unverified => f.write_str(
                "this index's signature has not been checked against a configured key; \
                 nothing will be installed from it",
            ),
            InstallRefusal::Resolve(r) => write!(f, "{r}"),
            InstallRefusal::Fetch { id, reason } => write!(f, "could not fetch {id}: {reason}"),
            InstallRefusal::Unpack { id, reason } => write!(f, "could not install {id}: {reason}"),
        }
    }
}

/// Resolve `wanted` against `opened.index`, refuse anything the plan asks for
/// that `granted` does not cover, fetch every step [`Plan::pending`] says is
/// missing from `plugin_root`, and install each one with
/// [`crate::unpack::install`] — never a second extraction path.
///
/// `plugin_root` is the machine-wide plugin directory
/// ([`crate::manifest::plugin_root`] in production), not anything scoped to a
/// profile: installing is a machine-level act exactly as it already is for
/// `install_local` (ADR-013's "installing is a machine-level act; approving
/// is an account-level one"). This function grants nothing — the returned
/// plugins are on disk and, per ADR-003, still hold no capabilities until a
/// user approves them in some profile.
pub fn install(
    source: &dyn IndexSource,
    opened: &Opened,
    wanted: &[Dependency],
    granted: &BTreeMap<String, BTreeSet<Capability>>,
    plugin_root: &Path,
) -> Result<Vec<PathBuf>, InstallRefusal> {
    if !opened.verified {
        return Err(InstallRefusal::Unverified);
    }
    let plan: Plan<'_> =
        resolve::plan(&opened.index, wanted, granted).map_err(InstallRefusal::Resolve)?;
    let installed = installed_versions(plugin_root);
    let mut out = Vec::new();
    for step in plan.pending(&installed) {
        let bytes = source
            .fetch_archive(step)
            .map_err(|e: SourceRefusal| InstallRefusal::Fetch { id: step.id.clone(), reason: e.to_string() })?;
        let dir = unpack::install(&bytes, step, plugin_root)
            .map_err(|e: UnpackRefusal| InstallRefusal::Unpack { id: step.id.clone(), reason: e.to_string() })?;
        out.push(dir);
    }
    Ok(out)
}

fn installed_versions(root: &Path) -> BTreeMap<String, semver::Version> {
    manifest::discover(root).into_iter().filter_map(|p| p.version.map(|v| (p.manifest.id, v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ContentHash;
    use crate::sign::fixtures;
    use crate::source::LocalFileSource;
    use std::io;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-marketplace-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tar_zst(manifest: &str) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        let mut append = |path: &str, data: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, path, data).unwrap();
        };
        append("plugin.json", manifest.as_bytes());
        append("main.ts", b"console.log('hi');\n");
        let tar = b.into_inner().unwrap();
        zstd::stream::encode_all(io::Cursor::new(tar), 3).unwrap()
    }

    const MANIFEST: &str =
        r#"{"id":"demo","name":"Demo","version":"1.0.0","entry":"main.ts","capabilities":["log"]}"#;

    /// A directory that is a complete, self-consistent local index: an
    /// `index.json` naming one plugin, its archive at the matching content
    /// hash, and — when `key` is given — a valid signature over the exact
    /// index bytes.
    fn built_source(tag: &str, key: Option<&fixtures::Keypair>) -> (PathBuf, LocalFileSource) {
        let dir = scratch(tag);
        std::fs::create_dir_all(dir.join("archives")).unwrap();
        let archive = tar_zst(MANIFEST);
        std::fs::write(dir.join("archives/demo-1.0.0.tar.zst"), &archive).unwrap();
        let index_json = format!(
            r#"{{"format":1,"plugins":[{{"id":"demo","name":"Demo","version":"1.0.0",
               "capabilities":["log"],"dependencies":{{}},
               "url":"https://example.invalid/demo-1.0.0.tar.zst","hash":"{}"}}]}}"#,
            ContentHash::of(&archive)
        );
        std::fs::write(dir.join("index.json"), &index_json).unwrap();
        if let Some(key) = key {
            std::fs::write(dir.join("index.json.minisig"), key.sign(index_json.as_bytes())).unwrap();
        }
        (dir.clone(), LocalFileSource::new(&dir))
    }

    fn want(id: &str) -> Vec<Dependency> {
        vec![Dependency::new(id, "^1.0.0").unwrap()]
    }

    fn granting(id: &str, caps: &[&str]) -> BTreeMap<String, BTreeSet<Capability>> {
        [(id.to_string(), caps.iter().map(|c| Capability::parse(c).unwrap()).collect())]
            .into_iter()
            .collect()
    }

    #[test]
    fn opening_with_no_key_parses_but_is_not_verified() {
        let (dir, source) = built_source("open-unconfigured", None);
        let opened = open(&source, Trust::Unconfigured).unwrap();
        assert!(!opened.verified);
        assert_eq!(opened.index.entries.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_with_a_correct_key_verifies() {
        let signer = fixtures::keypair(9);
        let (dir, source) = built_source("open-verified", Some(&signer));
        let key = sign::parse_key(&signer.public_base64()).unwrap();
        let opened = open(&source, Trust::Key(&key)).unwrap();
        assert!(opened.verified);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_with_the_wrong_key_is_refused_not_downgraded_to_unverified() {
        // A configured key that fails to verify must be a hard refusal, not a
        // silent fall-through to "unverified" — that would make configuring a
        // key strictly weaker than not configuring one.
        let signer = fixtures::keypair(9);
        let (dir, source) = built_source("open-wrong-key", Some(&signer));
        let other = fixtures::keypair(10);
        let wrong_key = sign::parse_key(&other.public_base64()).unwrap();
        let e = open(&source, Trust::Key(&wrong_key)).unwrap_err();
        assert!(matches!(e, OpenRefusal::Signature(_)), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_configured_key_with_no_signature_present_is_refused() {
        let signer = fixtures::keypair(9);
        let (dir, source) = built_source("open-no-sig", None);
        let key = sign::parse_key(&signer.public_base64()).unwrap();
        let e = open(&source, Trust::Key(&key)).unwrap_err();
        assert!(matches!(e, OpenRefusal::Signature(_)), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_refuses_an_unverified_index_before_fetching_or_unpacking_anything() {
        // The refusal this whole module exists for: a real archive, a
        // matching content hash, a satisfiable plan and a full grant are all
        // present, and it still must not install, because nothing has
        // checked that the index itself is what it claims to be.
        let (dir, source) = built_source("install-unverified", None);
        let opened = open(&source, Trust::Unconfigured).unwrap();
        let root = scratch("install-unverified-root");
        let granted = granting("demo", &["log"]);
        let e = install(&source, &opened, &want("demo"), &granted, &root).unwrap_err();
        assert_eq!(e, InstallRefusal::Unverified, "{e}");
        assert!(manifest::discover(&root).is_empty(), "nothing should have reached disk");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_refuses_a_verified_index_if_the_capability_was_not_granted() {
        // Verification proves the bytes; it says nothing about whether the
        // user approved what they ask for. `resolve::plan` still has to run.
        let signer = fixtures::keypair(9);
        let (dir, source) = built_source("install-ungranted", Some(&signer));
        let key = sign::parse_key(&signer.public_base64()).unwrap();
        let opened = open(&source, Trust::Key(&key)).unwrap();
        let root = scratch("install-ungranted-root");
        let e = install(&source, &opened, &want("demo"), &BTreeMap::new(), &root).unwrap_err();
        assert!(matches!(e, InstallRefusal::Resolve(ResolveRefusal::Ungranted { .. })), "{e}");
        assert!(manifest::discover(&root).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_verified_and_granted_plan_installs_through_the_hardened_unpacker() {
        let signer = fixtures::keypair(9);
        let (dir, source) = built_source("install-happy", Some(&signer));
        let key = sign::parse_key(&signer.public_base64()).unwrap();
        let opened = open(&source, Trust::Key(&key)).unwrap();
        let root = scratch("install-happy-root");
        let granted = granting("demo", &["log"]);
        let installed = install(&source, &opened, &want("demo"), &granted, &root).unwrap();
        assert_eq!(installed.len(), 1);
        assert!(root.join("demo/plugin.json").is_file());
        assert!(root.join("demo/main.ts").is_file());
        let found = manifest::discover(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.id, "demo");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_already_installed_version_is_not_fetched_again() {
        let signer = fixtures::keypair(9);
        let (dir, source) = built_source("install-idempotent", Some(&signer));
        let key = sign::parse_key(&signer.public_base64()).unwrap();
        let opened = open(&source, Trust::Key(&key)).unwrap();
        let root = scratch("install-idempotent-root");
        let granted = granting("demo", &["log"]);
        assert_eq!(install(&source, &opened, &want("demo"), &granted, &root).unwrap().len(), 1);
        // Installed once; asking again must not error and must not need the
        // archive a second time — delete the mirror's copy and confirm the
        // second call still succeeds by doing nothing.
        std::fs::remove_file(dir.join("archives/demo-1.0.0.tar.zst")).unwrap();
        assert_eq!(install(&source, &opened, &want("demo"), &granted, &root).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }
}
