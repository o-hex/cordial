//! Keeping more than one Roblox build, and choosing between them.
//!
//! There was one slot. `~/.cache/cordial/lib/<abi>` held one `libroblox.so`
//! and a stamp naming the APK it came out of, and a new build overwrote the
//! old one. That is fine until a Roblox build regresses, and then it is the
//! whole problem: the engine is the one component nobody here controls, and a
//! user whose game stopped working had no way back short of finding an APK
//! themselves.
//!
//! So the slot becomes a store. `$XDG_DATA_HOME/cordial/builds/<version>/` holds
//! the extracted library, the archives it came from and what proves them (who
//! signed them, where they came from, which bytes), and a launch resolves to an
//! entry and to nothing else ([ADR-054](../../../docs/adr/ADR-054-cordial-owns-its-roblox-builds.md);
//! it was `~/.cache/cordial/builds` until then). The old single-slot path
//! `~/.cache/cordial/lib/<abi>` is kept as a symlink to the newest entry so
//! that `justfile` and every `--lib-dir` anybody has typed keep working, but a
//! launch no longer reads it.
//! [ADR-033](../../../docs/adr/ADR-033-roblox-versions-are-a-keyed-store.md)
//! records the original decision and what it deliberately leaves out.
//!
//! **The version is a directory name, and it comes from scanning a binary.**
//! [`crate::engine::scan`] finds it by looking for a plausible run of digits
//! and dots in `libroblox.so`, which is a heuristic over bytes Cordial did not
//! write. Anything derived that way and then joined onto a path is a directory
//! traversal waiting to be reported as one, so [`is_valid_version`] is checked
//! at every entry point here rather than trusted to a caller -- and it is a
//! whitelist of digits and dots, not a blacklist of `..`.
//!
//! **Ordering is component-wise and numeric, never lexicographic.** Roblox's
//! versions look like `2.738.0.1393`, and sorted as strings `2.99` comes after
//! `2.738` -- so a store sorted the obvious way offers the wrong build as the
//! newest, and prunes the right one. That is a one-line bug with a
//! months-later symptom.

use crate::sha256::{Hasher, Sha256Hash};
use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

/// Where the entries live, under the cache root.
pub const BUILDS: &str = "builds";

/// The Cordial version that last loaded this entry, written beside it.
///
/// A compatibility record and not a decoration. Cordial's own shim is
/// versioned too: an old Roblox build can import a symbol the current shim
/// does not answer, and that fails at `dlopen` with `cannot locate symbol`
/// before any window appears. A picker that offers a build nothing here has
/// ever loaded is offering a crash, so an entry says whether it has been run
/// and by what -- and an entry with no record is offered *with that said*,
/// never hidden.
pub const LOADED_BY: &str = ".loaded-by";

/// A SHA-256 of the entry's own `libroblox.so`, written beside it.
///
/// Recorded once, by [`ensure_content_hash`], and trusted after that rather
/// than recomputed on every launch. It hashes the engine rather than the APK
/// it came out of because the APK is not a stable name for one build: ADR-025
/// measured Google Play's split bundle and APKPure's monolithic archive
/// carrying the same signed engine in containers 150 MB and 229 MB — a hash of
/// either container would call those two downloads different builds, which is
/// exactly backwards. [ADR-037](../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md)
/// records why the directory itself stays named by version rather than by this.
pub const CONTENT_SHA256: &str = ".content-sha256";

/// Who signed the entry's `base.apk`, and which bytes that was checked against.
///
/// Same file name as [`crate::cache::SIGNER`], which records the same fact for
/// the old single slot, but a different second line: the slot's stamp names an
/// archive by size, mtime *and path*, and an entry has to survive being moved
/// (the store itself moves from the cache to the data directory) without
/// every build in it becoming unverified. So an entry's record is the
/// fingerprint and the archive's size and mtime, nothing about where it sits.
/// A record in the slot's format fails to parse as this one and reads as "not
/// checked", which costs one verification and is the safe direction.
/// [ADR-054](../../../docs/adr/ADR-054-cordial-owns-its-roblox-builds.md).
pub const SIGNER: &str = crate::cache::SIGNER;

/// Where the entry came from: `mirror`, `sober`, `file` or `legacy`, then the
/// time it was filed in seconds since the epoch. Provenance for the interface
/// to quote, so that "imported from Sober" is a fact on disk and not a guess.
pub const SOURCE: &str = ".source";

/// The mirror's `versionCode`, when the source supplied one. Display only: a
/// source that cannot always supply a key is not a key.
pub const VERSION_CODE: &str = ".version-code";

/// Held shared, for as long as it runs, by every client running this entry.
/// Garbage collection takes it exclusively and non-blocking, so a build in use
/// is told apart from one that merely is not pinned.
pub const IN_USE: &str = ".in-use";

/// How many builds besides the newest, the pinned and the running a garbage
/// collection keeps: one, so that the build before the newest stays. The reason
/// the store exists (ADR-033) is that an update regresses and the user wants
/// yesterday's build two clicks away; zero would be the literal "newest only".
pub const SPARE: usize = 1;

/// How many builds the Quest store keeps, newest first: the newest and two
/// spares, which is the bound the phone store used before [`gc_plan`] replaced
/// it. The Quest store has no pins and no running-client lock (ADR-053 keeps it
/// out of ADR-054's), so a count is what it has.
pub const QUEST_SPARE: usize = 2;

/// `$XDG_DATA_HOME/cordial`, or `~/.local/share/cordial`: where profiles live,
/// and now where the builds do.
pub fn data_root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial")
}

/// `$XDG_DATA_HOME/cordial/builds`.
///
/// **Data, not cache.** It was `~/.cache/cordial/builds` until ADR-054, and a
/// cache is what cleaners delete: a pinned build may not be obtainable again
/// (the mirror keeps old versions today and nobody promises it will), so the
/// place that holds somebody's only copy of Roblox 2.730 must not be the place
/// `bleachbit` empties. In a Flatpak `XDG_DATA_HOME` is the sandbox's own
/// `~/.var/app/<id>/data`, so nothing is shared with a host install.
pub fn root() -> PathBuf {
    data_root().join(BUILDS)
}

/// Where the store was before ADR-054, for [`relocate`] to move out of.
pub fn cache_store() -> PathBuf {
    crate::install::cache_root().join(BUILDS)
}

/// Move what a pre-ADR-054 store holds from `from` to `to`, once.
///
/// Entry by entry (and the Quest store as one directory, which is not a
/// version and is skipped by [`list_in`]), never overwriting: a name already
/// under `to` is left where it is and in `from`, because two entries claiming
/// one version is for [`crate::install::file_into_store`] to adjudicate by hash
/// and not for a move to settle by order. A rename where both are on one
/// filesystem, which is a metadata change; otherwise a copy to a hidden name
/// and a rename, so a killed move leaves a name [`list_in`] skips rather than
/// an entry holding half a build. Returns what moved.
///
/// Archives hard-linked between `build/<abi>` and an entry become two copies
/// after a copy move. Nothing breaks by it; it costs the disk a link saved.
pub fn relocate(from: &Path, to: &Path) -> io::Result<Vec<String>> {
    let Ok(read) = std::fs::read_dir(from) else { return Ok(Vec::new()) };
    let mut candidates: Vec<(String, PathBuf)> = read
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.file_name().to_str()?.to_string(), e.path())))
        .filter(|(name, _)| is_valid_version(name) || name == crate::quest::ABI)
        .collect();
    candidates.sort();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(to)?;
    let _lock = lock(to)?;
    let mut moved = Vec::new();
    for (name, source) in candidates {
        let dest = to.join(&name);
        if dest.exists() {
            continue;
        }
        match std::fs::rename(&source, &dest) {
            Ok(()) => {}
            Err(_) => {
                let hidden = to.join(format!(".moving.{}.{name}", std::process::id()));
                let _ = std::fs::remove_dir_all(&hidden);
                let copied = copy_tree(&source, &hidden).and_then(|()| std::fs::rename(&hidden, &dest));
                if let Err(e) = copied {
                    let _ = std::fs::remove_dir_all(&hidden);
                    return Err(e);
                }
                std::fs::remove_dir_all(&source)?;
            }
        }
        moved.push(name);
    }
    // Nothing left that is an entry: the lock files and staging names that
    // remain are this store's own and go with it. Anything else stays.
    let leftover = std::fs::read_dir(from)
        .map(|r| r.flatten().any(|e| !e.file_name().to_string_lossy().starts_with('.')))
        .unwrap_or(false);
    if !leftover {
        let _ = std::fs::remove_dir_all(from);
    }
    Ok(moved)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Whether `version` may be used as a directory name.
///
/// Digits and dots, nothing else, and neither end may be a dot. That rejects
/// `..` and `/` without naming them, which is the point: a blacklist of the
/// traversal spellings somebody thought of is how the next spelling gets
/// through. The length cap is the same one [`crate::engine`] applies to a run
/// of digits when it scans, so nothing this accepts is longer than something
/// that could have been scanned.
pub fn is_valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 32
        && !version.starts_with('.')
        && !version.ends_with('.')
        && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Order two Roblox version strings, oldest first.
///
/// Component-wise and numeric. A component that does not parse -- which
/// [`is_valid_version`] should already have excluded, and which an empty
/// component from `1..2` would produce -- compares as zero rather than
/// panicking, because the ordering of a string that should not exist is not
/// worth an unwrap.
pub fn compare(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (l, r) => {
                let l = l.unwrap_or("").parse::<u64>().unwrap_or(0);
                let r = r.unwrap_or("").parse::<u64>().unwrap_or(0);
                match l.cmp(&r) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
        }
    }
}

/// Where a build in the store came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Downloaded by Cordial from the mirror.
    Mirror,
    /// Copied out of Sober's package directory by an import.
    Sober,
    /// A file the user chose.
    File,
    /// Found on disk by the migration from before the store was the only
    /// source of a launch, with nothing recording where it came from.
    Legacy,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Mirror => "mirror",
            Source::Sober => "sober",
            Source::File => "file",
            Source::Legacy => "legacy",
        }
    }

    pub fn parse(text: &str) -> Option<Source> {
        match text {
            "mirror" => Some(Source::Mirror),
            "sober" => Some(Source::Sober),
            "file" => Some(Source::File),
            "legacy" => Some(Source::Legacy),
            _ => None,
        }
    }

    /// What an archive at `path` is, judged by where it sits: Sober's package
    /// directory is Sober's, anything else is a file somebody chose.
    pub fn for_path(path: &Path) -> Source {
        let sober = path.components().any(|c| c.as_os_str() == "org.vinegarhq.Sober")
            || path.to_string_lossy().contains("sober/packages/");
        if sober {
            Source::Sober
        } else {
            Source::File
        }
    }
}

/// `4 Oct 2026`, for a time in seconds since the epoch. Pure and in UTC, so a
/// filing is described by the day it happened and not by the viewer's zone.
pub fn format_date(secs: u64) -> String {
    // Civil-from-days (Hinnant). Days since 1970-01-01 to y/m/d.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// [`Source`] and when it was filed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    pub source: Source,
    /// Seconds since the epoch, if it was recorded.
    pub at: Option<u64>,
}

/// One build in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub version: String,
    pub dir: PathBuf,
    /// The Cordial version that last loaded this build, if one ever has.
    ///
    /// `None` is the honest answer for an entry that has been fetched and
    /// never launched, and for every entry that existed before this was
    /// recorded. It is not "incompatible" and must not be presented as one.
    pub loaded_by: Option<String>,
    /// What the entry occupies, so a picker can say what deleting it frees.
    ///
    /// Hard-linked archives are counted here even though they share their
    /// inode with `build/<abi>`, so this over-reports the current entry by the
    /// size of the APKs. Deliberately: the number a user wants is "how big is
    /// this build", and explaining inode sharing in a picker helps nobody.
    pub bytes: u64,
    /// Whether this entry holds the APKs as well as the engine, and can
    /// therefore be launched on its own.
    ///
    /// False for every entry keyed before the archives were kept, and for one
    /// whose linking failed. Such an entry is not offered as a pin -- an engine
    /// paired with another version's assets is the mismatch
    /// [`crate::cache`] exists to prevent -- but it is still listed, because
    /// "you have this build and cannot select it" is information and hiding it
    /// is not.
    pub complete: bool,
    /// A SHA-256 of this entry's `libroblox.so`, if one has been recorded.
    ///
    /// `None` for an entry kept before [ADR-037] shipped, and for one whose
    /// hash could not be computed -- neither is fatal to launching it. See
    /// [`CONTENT_SHA256`].
    ///
    /// [ADR-037]: ../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md
    pub content_hash: Option<Sha256Hash>,
    /// The certificate this entry's `base.apk` was verified against, if it was
    /// and the file is still the one that was checked. `None` is "nobody has
    /// checked", which is not "refused": a build that fails verification is
    /// never filed. See [`SIGNER`].
    pub signer: Option<String>,
    /// Where it came from, if that was recorded. See [`SOURCE`].
    pub provenance: Option<Provenance>,
    /// The mirror's `versionCode`, when there was one. See [`VERSION_CODE`].
    pub version_code: Option<u64>,
}

impl Entry {
    /// Whether a launch may run this entry: it holds its own archives, and
    /// somebody has established who signed them.
    pub fn launchable(&self) -> bool {
        self.complete && self.signer.is_some()
    }

    /// The `base.apk` in this entry, if it has one.
    pub fn base_apk(&self) -> Option<PathBuf> {
        let base = self.dir.join(crate::install::BASE_APK);
        base.is_file().then_some(base)
    }
}

/// The directory an entry would occupy, or `None` if the version is not a name
/// this will write.
pub fn entry_dir_in(root: &Path, version: &str) -> Option<PathBuf> {
    is_valid_version(version).then(|| root.join(version))
}

pub fn entry_dir(version: &str) -> Option<PathBuf> {
    entry_dir_in(&root(), version)
}

/// What the store holds, newest first.
///
/// A directory whose name is not a version is skipped rather than reported.
/// The cache is a place users and packagers poke at, and a stray directory
/// there is not an error worth surfacing -- but it is also not something to
/// offer as a build, and it is emphatically not something [`prune_in`] should
/// feel free to delete.
pub fn list_in(root: &Path) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<Entry> = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(version) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_valid_version(version) {
            continue;
        }
        // An entry with no engine in it is not a build. That is what a killed
        // install leaves behind, and offering it would be offering a launch
        // that fails on a missing file.
        if !dir.join(crate::engine::LIBRARY).is_file() {
            continue;
        }
        found.push(Entry {
            version: version.to_string(),
            loaded_by: loaded_by(&dir),
            bytes: bytes_in(&dir),
            complete: dir.join(crate::install::BASE_APK).is_file(),
            content_hash: content_hash(&dir),
            signer: signer_of(&dir),
            provenance: provenance_of(&dir),
            version_code: version_code_of(&dir),
            dir,
        });
    }
    found.sort_by(|a, b| compare(&b.version, &a.version));
    found
}

pub fn list() -> Vec<Entry> {
    list_in(&root())
}

/// One level, not a walk: an entry holds a handful of files and no
/// subdirectories, and a recursive size of a cache directory is a way to spend
/// a second of somebody's launch on a number shown in a picker.
fn bytes_in(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// Everything under `dir`, files only, for a store view that wants to say what
/// removing an entry frees. A walk, unlike [`list_in`]'s one-level count: an
/// entry's extracted assets are most of its disk after the first launch, and a
/// number that left them out would under-report what a Remove gives back.
pub fn tree_bytes(dir: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else { return 0 };
    read.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => tree_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// The Cordial version that last loaded the build in `dir`.
pub fn loaded_by(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(LOADED_BY)).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The content hash recorded for the entry in `dir`, if there is one.
pub fn content_hash(dir: &Path) -> Option<Sha256Hash> {
    let text = std::fs::read_to_string(dir.join(CONTENT_SHA256)).ok()?;
    Sha256Hash::parse(text.trim()).ok()
}

/// What a filing route knows about a build it is putting in the store.
///
/// Every route that creates an entry writes all of it, so that an entry proves
/// who signed it, says how it arrived and (when the source knew) which
/// `versionCode` it was. ADR-054.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filing {
    pub source: Source,
    /// The certificate the archives verified against, lowercase hex.
    pub signer: String,
    pub version_code: Option<u64>,
}

/// Write [`Filing`]'s records into the entry at `dir`. The signer is recorded
/// against the `base.apk` that is in `dir` now, so call it after the archives
/// have landed.
pub fn write_records(dir: &Path, filing: &Filing) -> io::Result<()> {
    record_signer(dir, &filing.signer)?;
    record_source(dir, filing.source)?;
    if let Some(code) = filing.version_code {
        record_version_code(dir, code)?;
    }
    Ok(())
}

/// "size mtime", the identity of an archive that survives being moved.
fn archive_identity(path: &Path) -> Option<String> {
    use std::time::UNIX_EPOCH;
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{} {}", meta.len(), mtime))
}

/// The certificate fingerprint recorded for the entry in `dir`, if the record
/// still describes the `base.apk` that is there now.
pub fn signer_of(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(SIGNER)).ok()?;
    let mut lines = text.lines();
    let fingerprint = lines.next()?.trim();
    let vouched_for = lines.next()?.trim();
    let now = archive_identity(&dir.join(crate::install::BASE_APK))?;
    (now == vouched_for && !fingerprint.is_empty()).then(|| fingerprint.to_ascii_lowercase())
}

/// Record that the `base.apk` in `dir` verified against `fingerprint`, as it is
/// right now. Call it on the archive that is kept, after the check, never on
/// the one that was checked: a copy does not keep its mtime.
pub fn record_signer(dir: &Path, fingerprint: &str) -> io::Result<()> {
    let identity = archive_identity(&dir.join(crate::install::BASE_APK)).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, format!("{} has no base.apk to record a signer for", dir.display()))
    })?;
    std::fs::write(dir.join(SIGNER), format!("{}\n{identity}\n", fingerprint.trim().to_ascii_lowercase()))
}

pub fn provenance_of(dir: &Path) -> Option<Provenance> {
    let text = std::fs::read_to_string(dir.join(SOURCE)).ok()?;
    let mut words = text.split_whitespace();
    let source = Source::parse(words.next()?)?;
    Some(Provenance { source, at: words.next().and_then(|w| w.parse().ok()) })
}

/// Record where the entry in `dir` came from, and now. An existing record is
/// kept: the first filing is the one that says how it arrived.
pub fn record_source(dir: &Path, source: Source) -> io::Result<()> {
    if provenance_of(dir).is_some() {
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    std::fs::write(dir.join(SOURCE), format!("{} {now}\n", source.as_str()))
}

pub fn version_code_of(dir: &Path) -> Option<u64> {
    std::fs::read_to_string(dir.join(VERSION_CODE)).ok()?.trim().parse().ok()
}

pub fn record_version_code(dir: &Path, code: u64) -> io::Result<()> {
    std::fs::write(dir.join(VERSION_CODE), code.to_string())
}

/// Record `hash` as `dir`'s content hash.
fn record_content_hash(dir: &Path, hash: &Sha256Hash) -> io::Result<()> {
    std::fs::write(dir.join(CONTENT_SHA256), hash.to_string())
}

/// A streamed SHA-256 of `path`, read a block at a time so a 100+ MB engine is
/// never held whole -- the same reason [`crate::sha256::Hasher`] exists rather
/// than `Sha256Hash::of` being used directly.
pub(crate) fn hash_file(path: &Path) -> io::Result<Sha256Hash> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finish())
}

/// Make sure `entry` has a recorded content hash, computing one only if it
/// does not already have one.
///
/// Called by [`crate::install::file_into_store`] once an entry exists at its
/// final name. See
/// [ADR-037](../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md).
///
/// **The recorded-hash path is what makes this safe to call wherever a build is
/// filed**, rather than hashing a 100+ MB file each time: it reads
/// `.content-sha256` first and returns it unhashed if it parses. An entry filed
/// before ADR-037 has none until something asks, which is a filing of the same
/// engine again.
///
/// Failure is reported to the caller and is not fatal -- an entry with no
/// recorded hash is exactly what one predating this feature looks like, and it
/// still launches.
pub fn ensure_content_hash(entry: &Path) -> Option<Sha256Hash> {
    if let Some(existing) = content_hash(entry) {
        return Some(existing);
    }
    let library = entry.join(crate::engine::LIBRARY);
    if !library.is_file() {
        return None;
    }
    let hash = hash_file(&library).ok()?;
    record_content_hash(entry, &hash).ok()?;
    Some(hash)
}

/// The entry under `root` whose recorded content hash is `hash`, if any.
///
/// [`crate::install::file_into_store`] uses it to recognise that an incoming
/// engine is byte-identical to one already kept, whatever either is called, and
/// to file into that entry rather than duplicate it (ADR-054 wired what ADR-037
/// left open).
pub fn find_by_content_hash(root: &Path, hash: &Sha256Hash) -> Option<Entry> {
    list_in(root).into_iter().find(|e| e.content_hash.as_ref() == Some(hash))
}

/// An advisory lock over every write to the store at `root`, held for the
/// duration of one mutation.
///
/// Callers used to key or prune a build from three places, two of them taking
/// different lock files and one taking none. Everything now reaches the store
/// only through [`crate::install::file_into_store`], [`gc_in`],
/// [`remove_in`] or [`relocate`], so the lock lives inside those rather than at
/// each caller. See
/// [ADR-037](../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md).
///
/// **Blocking, unlike [`crate::provider::exclusive`].** That lock guards a
/// network fetch a user is watching progress for, so it refuses a second
/// attempt instantly rather than queue it invisibly. This one guards a handful
/// of local renames and, at most, one pass over a 100+ MB file with no network
/// in it -- so a second caller waiting a fraction of a second for the first to
/// finish is the honest behaviour, and refusing an ordinary launch's keying
/// step because a Version-page download happened to be mid-rename would fail
/// for a reason nobody watching it could act on.
pub(crate) fn lock(root: &Path) -> io::Result<std::fs::File> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".store.lock"))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).map_err(io::Error::from)?;
    Ok(file)
}

/// The store entry `path` resolves to, if it is one.
///
/// Takes the symlink into account, because in the ordinary case the caller has
/// `~/.cache/cordial/lib/<abi>` and the entry is what that points at. A pinned
/// launch passes the entry directly and this returns it unchanged.
///
/// `None` for anything outside the store -- an engine beside the APK, a
/// hand-set `--lib-dir`, a build of unknown version still in the old slot. All
/// ordinary, none of them things to record against.
pub fn entry_at(path: &Path) -> Option<PathBuf> {
    entry_at_in(&root(), path)
}

/// [`entry_at`] against a given store root, so the answer can be tested
/// without the machine's real store.
pub fn entry_at_in(root: &Path, path: &Path) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(path).ok()?;
    let root = std::fs::canonicalize(root).ok()?;
    let name = resolved.file_name()?.to_str()?;
    (resolved.parent() == Some(root.as_path()) && is_valid_version(name)).then_some(resolved)
}

/// Record that this Cordial loaded the build in `dir`.
///
/// Called after a load has succeeded, never before it is attempted: the whole
/// value of the record is that it distinguishes a build somebody has run from
/// one nobody has, and a record written on the way in would say the same thing
/// about both.
pub fn record_loaded_by(dir: &Path, cordial: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(LOADED_BY), cordial.trim())
}

/// Point `live` at the entry for `version`.
///
/// `live` is the old single-slot path, and after this it is a symlink. The
/// swap is a `symlink` to a temporary name followed by a `rename` over the
/// old one, because `symlink` itself refuses to replace anything: the
/// alternative is unlink-then-symlink, which leaves a window in which there is
/// no engine at all, and a launch in that window fails with a missing file
/// rather than waiting.
///
/// **It refuses when `live` is a real directory**, rather than deleting one.
/// That directory is an engine somebody may be running -- the `CORDIAL_APK`
/// override's, or `just client`'s -- and this will not delete it to make room
/// for a link.
pub fn point_current_at(live: &Path, entry: &Path) -> io::Result<()> {
    use std::os::unix::fs::symlink;

    if let Ok(meta) = std::fs::symlink_metadata(live) {
        if meta.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} is a directory holding an engine, not a link into the store",
                    live.display()
                ),
            ));
        }
    }
    if let Some(parent) = live.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = live.with_file_name(format!(
        ".{}.linking.{}",
        live.file_name().and_then(|n| n.to_str()).unwrap_or("current"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&temporary);
    symlink(entry, &temporary)?;
    match std::fs::rename(&temporary, live) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temporary);
            Err(e)
        }
    }
}

/// Which entry `live` currently points at, if it points at one at all.
pub fn current_in(root: &Path, live: &Path) -> Option<String> {
    let target = std::fs::read_link(live).ok()?;
    let resolved = if target.is_absolute() {
        target
    } else {
        live.parent()?.join(target)
    };
    // Compared by name under the store root rather than by canonicalising
    // both: `canonicalize` needs the target to exist, and an entry that has
    // been pruned out from under a stale link is exactly the case worth
    // answering rather than erroring on.
    let name = resolved.file_name()?.to_str()?.to_string();
    (resolved.parent() == Some(root) && is_valid_version(&name)).then_some(name)
}

/// Break the link at `live` and leave an empty directory in its place.
///
/// Called before an install, and this is the ordering the whole store turns
/// on. Once `live` is a symlink into an entry, *everything that writes to it
/// writes into that entry* -- so an extraction that renames a new
/// `libroblox.so` onto `live/libroblox.so` would land inside the build the
/// user is keeping, overwrite it, and leave the store holding one entry with
/// two versions' worth of claim on it. The first draft did exactly that and
/// the test below is what caught it.
///
/// Breaking the link costs nothing: the entry it pointed at is untouched and
/// stays in the store, which is the build somebody rolls back to.
pub fn detach(live: &Path) -> io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(live) {
        if meta.file_type().is_symlink() {
            std::fs::remove_file(live)?;
        }
    }
    std::fs::create_dir_all(live)
}

impl Provenance {
    /// What the interface says about how the entry arrived: a fact read off the
    /// disk, never a guess.
    pub fn label(&self) -> String {
        let on = self.at.map(|t| format!(" on {}", format_date(t))).unwrap_or_default();
        match self.source {
            Source::Mirror => format!("Downloaded{on}"),
            Source::Sober => format!("Imported from Sober{on}"),
            Source::File => format!("Imported from a file{on}"),
            Source::Legacy => format!("Found on disk by an earlier Cordial, filed{on}"),
        }
    }
}

/// Where an entry's extracted assets live.
///
/// Inside the entry, so every build has its own tree and two profiles on two
/// versions never re-extract over each other, and so removing an entry takes
/// its assets with it. The runtime derives it from [`entry_at`] of its
/// `--lib-dir`; anything outside the store keeps the shared tree it always
/// had. ADR-054.
pub const ASSETS: &str = "assets";

pub fn assets_dir(entry: &Path) -> PathBuf {
    entry.join(ASSETS)
}

/// Mark the entry at `dir` as in use, for as long as the returned file lives.
///
/// A shared, non-blocking `flock` on [`IN_USE`]: any number of clients may run
/// one build, and none of them can have it removed from under them, because
/// [`gc_in`] and [`remove_in`] ask for the same file exclusively. The lock
/// belongs to the open file description, so a client that crashes or is killed
/// releases it, which a file holding a pid would not.
///
/// Refuses (rather than waits) when the entry is being removed right now: the
/// directory is about to disappear and starting a client in it would only fail
/// later and elsewhere.
pub fn hold_in_use(dir: &Path) -> io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(IN_USE))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockShared).map_err(|_| {
        io::Error::new(io::ErrorKind::WouldBlock, format!("{} is being removed", dir.display()))
    })?;
    Ok(file)
}

/// Take the entry exclusively, which succeeds only when no client holds it.
/// The caller keeps the file for as long as it needs the answer to stay true.
fn claim_unused(dir: &Path) -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(IN_USE))
        .ok()?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).ok()?;
    Some(file)
}

/// Whether any client is running the entry at `dir` right now.
pub fn is_in_use(dir: &Path) -> bool {
    claim_unused(dir).is_none() && dir.is_dir()
}

/// Remove what [`gc_plan`] says nothing needs, and return what went.
///
/// Each victim is claimed exclusively before it is touched and held while it is
/// deleted, so a client that started between the plan and the delete is not
/// failed under: it holds the entry, the claim is refused and the entry stays.
/// Run after a filing and from the store view, never at launch with a window
/// open. [ADR-054](../../../docs/adr/ADR-054-cordial-owns-its-roblox-builds.md).
pub fn gc_in(root: &Path, pins: &[String], spare: usize) -> Vec<String> {
    let Ok(_lock) = lock(root) else { return Vec::new() };
    let entries = list_in(root);
    let running: Vec<String> =
        entries.iter().filter(|e| is_in_use(&e.dir)).map(|e| e.version.clone()).collect();
    let mut removed = Vec::new();
    for version in gc_plan(&entries, pins, &running, spare) {
        let Some(dir) = entry_dir_in(root, &version) else { continue };
        let Some(_held) = claim_unused(&dir) else { continue };
        if std::fs::remove_dir_all(&dir).is_ok() {
            removed.push(version);
        }
    }
    removed
}

/// Remove one entry because somebody asked to, from the store view.
///
/// Refuses the newest launchable entry (Latest, which would leave a profile
/// nothing to launch), anything in `protect` (a profile's pin turned into a
/// launch failure in a profile nobody touched), and an entry a client is
/// running. Removing an entry only drops its own copies, so a Sober directory
/// an import was made from is never touched.
pub fn remove_in(root: &Path, version: &str, protect: &[String]) -> Result<(), String> {
    let Some(dir) = entry_dir_in(root, version) else {
        return Err(format!("{version:?} is not a Roblox version"));
    };
    let _lock = lock(root).map_err(|e| format!("could not lock the build store: {e}"))?;
    if latest(&list_in(root)).is_some_and(|e| e.version == version) {
        return Err(format!(
            "Roblox {version} is the newest build, which profiles on Latest launch. Removing it would leave them nothing."
        ));
    }
    if protect.iter().any(|p| p == version) {
        return Err(format!("A profile is pinned to Roblox {version}. Clear that pin first."));
    }
    let Some(_held) = claim_unused(&dir) else {
        return Err(format!("Roblox {version} is running in a client. Close it first."));
    };
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Check the archives an entry holds against `trusted`, as a filing would have.
///
/// For an entry that predates the records ADR-054 added, so it has none: the
/// first launch checks it once and writes the answer down. Every archive in the
/// entry is checked and they must share one certificate, the same rule
/// [`crate::provider`] applies to a download, for the same reason: two halves
/// signed by different keys are not two halves of one build.
pub fn verify_entry(dir: &Path, trusted: &[String]) -> Result<String, String> {
    let mut found: Vec<PathBuf> = Vec::new();
    for name in [crate::install::BASE_APK, crate::install::SPLIT_APK] {
        let path = dir.join(name);
        if path.is_file() {
            found.push(path);
        }
    }
    if found.is_empty() {
        return Err("it holds no archive to check".into());
    }
    let mut certificate: Option<String> = None;
    for path in &found {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("archive");
        let signer = crate::apk_signature::verify_signed_by(path, trusted).map_err(|e| format!("{name}: {e}"))?;
        match &certificate {
            None => certificate = Some(signer.certificate_sha256),
            Some(first) if *first != signer.certificate_sha256 => {
                return Err(format!("{name}: the two halves of this build were signed by different certificates"));
            }
            Some(_) => {}
        }
    }
    certificate.ok_or_else(|| "nothing was checked".into())
}

/// Check every complete entry that has no signer record, and record the ones
/// that pass. Returns what was checked and how each came out, for the caller
/// to say: an entry that fails stays unrecorded and is never launched.
pub fn ensure_verified(root: &Path, trusted: &[String]) -> Vec<(String, Result<String, String>)> {
    let mut out = Vec::new();
    for entry in list_in(root) {
        if !entry.complete || entry.signer.is_some() {
            continue;
        }
        let verdict = verify_entry(&entry.dir, trusted).and_then(|fingerprint| {
            record_signer(&entry.dir, &fingerprint).map_err(|e| format!("could not record it: {e}"))?;
            let _ = record_source(&entry.dir, Source::Legacy);
            Ok(fingerprint)
        });
        out.push((entry.version, verdict));
    }
    out
}

// ---- planning ----------------------------------------------------------
//
// Everything below is a pure function of what is on disk, handed in. A launch,
// a garbage collection and the first-run migration each decide something
// destructive or something that refuses to start a client, and a decision that
// can only be exercised by building a store is a decision nobody tests twice.
// ADR-054.

/// What a profile asks of the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice<'a> {
    /// The newest build in the store, whatever it is when the client starts.
    Latest,
    /// One build, by the engine's version.
    Pinned(&'a str),
}

/// Why a launch cannot take a build from the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing in the store. The first-run screen is the answer, not an error.
    Empty,
    /// Something is there and none of it can be run: kept without its
    /// archives, or never checked for a signature.
    NothingUsable { incomplete: Vec<String>, unsigned: Vec<String> },
    Missing(String),
    Incomplete(String),
    Unsigned(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Empty => write!(f, "Cordial has no Roblox build yet."),
            Refusal::NothingUsable { incomplete, unsigned } => {
                write!(f, "Cordial's store holds Roblox builds it cannot run.")?;
                if !unsigned.is_empty() {
                    write!(
                        f,
                        " Cordial could not establish who signed {}, so it will not run {}.",
                        unsigned.join(", "),
                        if unsigned.len() == 1 { "it" } else { "them" }
                    )?;
                }
                if !incomplete.is_empty() {
                    write!(
                        f,
                        " {} kept without the APK it came from, so its assets are gone.",
                        incomplete.join(", ")
                    )?;
                }
                write!(f, " Download Roblox again from Settings.")
            }
            Refusal::Missing(v) => write!(
                f,
                "This profile is pinned to Roblox {v}, and that build is not in Cordial's store. \
                 Open Settings and choose another version, or choose Latest."
            ),
            Refusal::Incomplete(v) => write!(
                f,
                "This profile is pinned to Roblox {v}, and Cordial kept that build's engine \
                 without the APK it came from, so its assets are gone and it cannot be run on its \
                 own. Choose Latest, or pin a build Cordial has downloaded since."
            ),
            Refusal::Unsigned(v) => write!(
                f,
                "This profile is pinned to Roblox {v}, and Cordial has not established who signed \
                 that build, so it will not run it. Choose Latest, or download the build again."
            ),
        }
    }
}

/// The outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Entry(String),
    Refused(Refusal),
}

/// The newest entry a launch may run: complete, and with a recorded signer.
///
/// Ordered by [`compare`] rather than trusting the caller's order, so a list
/// built by hand in a test and one from [`list_in`] answer the same.
pub fn latest(entries: &[Entry]) -> Option<&Entry> {
    entries.iter().filter(|e| e.launchable()).max_by(|a, b| compare(&a.version, &b.version))
}

/// What a profile's choice comes to, given the store.
///
/// **A pin refuses and never falls back.** Falling back to Latest would run the
/// very build the user pinned away from and say nothing, which is the one
/// outcome that makes a pin worse than not having one (ADR-033).
pub fn resolve(choice: Choice<'_>, entries: &[Entry]) -> Resolved {
    match choice {
        Choice::Latest => match latest(entries) {
            Some(entry) => Resolved::Entry(entry.version.clone()),
            None if entries.is_empty() => Resolved::Refused(Refusal::Empty),
            None => Resolved::Refused(Refusal::NothingUsable {
                incomplete: entries.iter().filter(|e| !e.complete).map(|e| e.version.clone()).collect(),
                unsigned: entries
                    .iter()
                    .filter(|e| e.complete && e.signer.is_none())
                    .map(|e| e.version.clone())
                    .collect(),
            }),
        },
        Choice::Pinned(version) => match entries.iter().find(|e| e.version == version) {
            None => Resolved::Refused(Refusal::Missing(version.to_string())),
            Some(e) if !e.complete => Resolved::Refused(Refusal::Incomplete(version.to_string())),
            Some(e) if e.signer.is_none() => Resolved::Refused(Refusal::Unsigned(version.to_string())),
            Some(e) => Resolved::Entry(e.version.clone()),
        },
    }
}

/// Which builds a garbage collection removes.
///
/// Kept: the newest entry, every pinned one, every one in use, and the `spare`
/// newest of whatever is left. **A pinned build does not count against
/// `spare`**: the bound it replaces ([`KEEP`]) counted pins towards the limit,
/// so two pins evicted the build before the newest, which is the one the
/// store exists to keep.
///
/// Returns only names that are versions and are in `entries`, so nothing it
/// returns can be a path outside the store, and applying the answer and asking
/// again returns nothing.
pub fn gc_plan(entries: &[Entry], pins: &[String], in_use: &[String], spare: usize) -> Vec<String> {
    let mut by_age: Vec<&Entry> = entries.iter().filter(|e| is_valid_version(&e.version)).collect();
    by_age.sort_by(|a, b| compare(&b.version, &a.version));
    let newest = by_age.first().map(|e| e.version.clone());
    let mut spared = 0usize;
    let mut doomed = Vec::new();
    for entry in by_age {
        let v = &entry.version;
        if Some(v) == newest.as_ref() || pins.contains(v) || in_use.contains(v) {
            continue;
        }
        if spared < spare {
            spared += 1;
            continue;
        }
        doomed.push(v.clone());
    }
    doomed
}

/// What the slot at `lib/<abi>` was before this release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Absent,
    /// A link into the store: the build it names is an entry already.
    LinkIntoStore,
    /// A real directory holding an engine, which no entry corresponds to.
    Directory,
}

/// What the first launch after the upgrade finds on disk.
#[derive(Debug, Clone)]
pub struct Legacy {
    pub store_has_complete_entry: bool,
    pub slot: Slot,
    /// The archive the slot's `.from` stamp names, if it is still there.
    pub slot_archive: Option<PathBuf>,
    /// The archive named by the `.from` stamp of the newest entry the store
    /// holds **without its archives**, if it is still there, else Sober's when
    /// such an entry exists.
    ///
    /// 0.24.1's Flatpak keeps a build it extracted from Sober's directory as an
    /// entry of the cache store with the engine and no `base.apk`, because the
    /// hard link from Sober's read-only mount fails with `EXDEV` (measured, in
    /// the Flatpak, 2026-10-05). `relocate` then moves that entry and leaves the
    /// slot link dangling, so the slot reads as `Absent` and nothing else in the
    /// plan names the archives the entry was missing.
    pub orphan_archive: Option<PathBuf>,
    /// `build/<abi>/base.apk`, which only a build Cordial installed has.
    pub managed_apk: Option<PathBuf>,
    pub sober_apk: Option<PathBuf>,
    /// The APK chosen in Settings, which was a launch source before.
    pub settings_apk: Option<PathBuf>,
}

/// One thing the migration does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Verify these archives, copy them into the store and file them.
    File { apk: PathBuf, source: Source },
    /// The same for a file the user chose, and every profile with no pin is
    /// pinned to the result: somebody who chose a file meant that file, and
    /// Latest would otherwise move them off it.
    ImportChosen { apk: PathBuf },
}

/// What the migration does, in order. `CORDIAL_APK` is not an input: it is a
/// per-run override and nothing here may read it as a build.
///
/// **Sober's directory alone is not a reason to file anything.** A machine
/// that has Sober and has never run Cordial has no slot, so it gets the
/// first-run screen with both buttons, and so does one whose cache was wiped
/// while Sober stayed installed: nothing here re-imports silently. Sober's
/// build is filed only when it is what the old launch was running, which
/// always left a slot behind -- a real directory at `lib/<abi>` -- and there it
/// is the fallback after the archive the slot's own stamp names.
///
/// A slot already linked into the store, or a store that already holds a
/// complete build, has nothing to copy. A Settings APK is imported whether or
/// not the store is empty, because it is the one legacy launch source that
/// outranked everything else and the user chose it.
pub fn migration_plan(legacy: &Legacy) -> Vec<Step> {
    if let Some(apk) = &legacy.settings_apk {
        return vec![Step::ImportChosen { apk: apk.clone() }];
    }
    if legacy.store_has_complete_entry {
        return Vec::new();
    }
    let candidate = match legacy.slot {
        Slot::Directory => legacy
            .slot_archive
            .as_ref()
            .or(legacy.managed_apk.as_ref())
            .or(legacy.sober_apk.as_ref()),
        // No slot: the legacy evidence left is a build Cordial installed itself
        // into its own directory, or an entry the old Flatpak kept without its
        // archives, which is the old launch's own build with half of it
        // missing. Either is something the old launch was running; Sober's
        // directory alone, with no such entry, still is not.
        _ => legacy.managed_apk.as_ref().or(legacy.orphan_archive.as_ref()),
    };
    match candidate {
        Some(apk) => {
            let source = match Source::for_path(apk) {
                Source::Sober => Source::Sober,
                _ => Source::Legacy,
            };
            vec![Step::File { apk: apk.clone(), source }]
        }
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_refuses_the_newest_build_and_a_pinned_one_and_takes_the_rest() {
        let scratch = Scratch::new("remove");
        let root = scratch.path().join(BUILDS);
        for v in ["1.0", "2.0", "3.0"] {
            launchable(&root, v);
        }
        let pins = vec!["2.0".to_string()];

        assert!(remove_in(&root, "3.0", &pins).is_err(), "the newest build, which Latest launches");
        assert!(remove_in(&root, "2.0", &pins).is_err(), "a pinned build");
        assert!(remove_in(&root, "../lib", &pins).is_err(), "not a version");
        remove_in(&root, "1.0", &pins).unwrap();
        let left: Vec<String> = list_in(&root).into_iter().map(|e| e.version).collect();
        assert_eq!(left, ["3.0", "2.0"]);
    }

    /// An entry a launch may run: engine, its own archive, and a signer.
    fn launchable(root: &Path, version: &str) -> PathBuf {
        let dir = root.join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(crate::engine::LIBRARY), version).unwrap();
        std::fs::write(dir.join(crate::install::BASE_APK), format!("apk {version}")).unwrap();
        record_signer(&dir, &"ab".repeat(32)).unwrap();
        dir
    }

    /// A scratch directory that deletes itself, so these tests need no
    /// dependency the workspace does not already have.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("cordial-store-test-{}-{}", tag, std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn build(root: &Path, version: &str) -> PathBuf {
        let dir = root.join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(crate::engine::LIBRARY), b"not really an engine").unwrap();
        dir
    }

    #[test]
    fn a_version_is_only_ever_digits_and_dots() {
        assert!(is_valid_version("2.738.0.1393"));
        assert!(is_valid_version("2"));
        assert!(!is_valid_version(""));
        assert!(!is_valid_version(".."));
        assert!(!is_valid_version("../../etc"));
        assert!(!is_valid_version("2.738/0"));
        assert!(!is_valid_version("2.738.0."));
        assert!(!is_valid_version(".2.738"));
        assert!(!is_valid_version("2.738.0-beta"));
        assert!(!is_valid_version(&"1".repeat(33)));
    }

    /// The one this exists for. Sorted as strings, `2.99` beats `2.738`, and a
    /// store that believes that offers the wrong build as newest and prunes the
    /// right one.
    #[test]
    fn versions_compare_numerically_rather_than_as_text() {
        assert_eq!(compare("2.738.0.1393", "2.99.0.1"), Ordering::Greater);
        assert!("2.738.0.1393" < "2.99.0.1", "the string ordering this corrects");
        assert_eq!(compare("2.738.0.1393", "2.738.0.1393"), Ordering::Equal);
        assert_eq!(compare("2.738", "2.738.0"), Ordering::Equal);
        assert_eq!(compare("2.734.0.917", "2.738.0.1393"), Ordering::Less);
    }

    #[test]
    fn the_store_lists_newest_first_and_skips_what_is_not_a_build() {
        let scratch = Scratch::new("list");
        let root = scratch.path();
        build(root, "2.734.0.917");
        build(root, "2.738.0.1393");
        build(root, "2.99.0.1");
        // A directory with no engine in it: what a killed install leaves.
        std::fs::create_dir_all(root.join("2.740.0.1")).unwrap();
        // And something that is not a version at all.
        std::fs::create_dir_all(root.join("scratch")).unwrap();

        let listed: Vec<String> = list_in(root).into_iter().map(|e| e.version).collect();
        assert_eq!(listed, vec!["2.738.0.1393", "2.734.0.917", "2.99.0.1"]);
    }

    #[test]
    fn the_current_link_names_the_entry_it_points_at() {
        let scratch = Scratch::new("link");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&root).unwrap();
        let entry = build(&root, "2.738.0.1393");

        point_current_at(&live, &entry).unwrap();
        assert_eq!(current_in(&root, &live).as_deref(), Some("2.738.0.1393"));
        assert!(live.join(crate::engine::LIBRARY).is_file(), "reads through the link");

        // And it re-points rather than refusing, which is what an update does.
        let newer = build(&root, "2.740.0.5");
        point_current_at(&live, &newer).unwrap();
        assert_eq!(current_in(&root, &live).as_deref(), Some("2.740.0.5"));
    }

    /// The engine somebody may be running is not deleted to make room for a
    /// link.
    #[test]
    fn a_real_directory_is_never_replaced_by_a_link_behind_your_back() {
        let scratch = Scratch::new("refuse");
        let root = scratch.path().join("builds");
        std::fs::create_dir_all(&root).unwrap();
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(crate::engine::LIBRARY), b"the only engine there is").unwrap();
        let entry = build(&root, "2.738.0.1393");

        let refused = point_current_at(&live, &entry).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        assert!(live.join(crate::engine::LIBRARY).is_file(), "and it is still there");
    }

    /// What `prune_in(.., KEEP = 3, ..)` got wrong, on disk: with the pin
    /// counted against the bound, one pinned old build left room for only one
    /// of the unreferenced others, and an old unreferenced build the bound did
    /// not reach stayed for ever. `gc_in` keeps the newest, the pin and one
    /// spare, and removes the rest.
    #[test]
    fn gc_keeps_the_newest_the_pin_and_one_spare_on_disk() {
        let scratch = Scratch::new("gc-disk");
        let root = scratch.path();
        for v in ["2.730.0.1", "2.734.0.917", "2.738.0.1393", "2.740.0.5", "2.742.0.9"] {
            build(root, v);
        }
        let removed = gc_in(root, &["2.730.0.1".to_string()], SPARE);
        assert_eq!(removed, vec!["2.738.0.1393", "2.734.0.917"]);

        let left: Vec<String> = list_in(root).into_iter().map(|e| e.version).collect();
        assert_eq!(left, vec!["2.742.0.9", "2.740.0.5", "2.730.0.1"]);
        assert!(gc_in(root, &["2.730.0.1".to_string()], SPARE).is_empty(), "idempotent on disk too");
    }

    #[test]
    fn an_entry_says_whether_anything_has_ever_loaded_it() {
        let scratch = Scratch::new("loaded");
        let root = scratch.path();
        let dir = build(root, "2.738.0.1393");
        assert_eq!(list_in(root)[0].loaded_by, None, "fetched and never launched");

        record_loaded_by(&dir, "0.14.0").unwrap();
        assert_eq!(list_in(root)[0].loaded_by.as_deref(), Some("0.14.0"));
    }

    /// The bug this ordering exists for: with `live` a link into an entry, a
    /// write to `live/libroblox.so` lands *inside* the build being kept.
    #[test]
    fn detaching_leaves_the_entry_alone_and_the_slot_writable() {
        let scratch = Scratch::new("detach");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&root).unwrap();
        let entry = build(&root, "2.738.0.1393");
        std::fs::write(entry.join(crate::engine::LIBRARY), b"the build being kept").unwrap();
        point_current_at(&live, &entry).unwrap();

        detach(&live).unwrap();
        assert!(live.is_dir() && !live.is_symlink());
        assert!(!live.join(crate::engine::LIBRARY).exists(), "an empty slot to extract into");

        // Writing the next build into the slot does not reach the old entry.
        std::fs::write(live.join(crate::engine::LIBRARY), b"the next build").unwrap();
        assert_eq!(
            std::fs::read(entry.join(crate::engine::LIBRARY)).unwrap(),
            b"the build being kept"
        );
    }

    #[test]
    fn a_content_hash_round_trips_through_its_file() {
        let scratch = Scratch::new("hash-roundtrip");
        let dir = build(scratch.path(), "2.738.0.1393");
        assert_eq!(content_hash(&dir), None, "nothing recorded yet");

        let hash = Sha256Hash::of(b"an engine's bytes, in this test");
        record_content_hash(&dir, &hash).unwrap();
        assert_eq!(content_hash(&dir), Some(hash));
    }

    /// The property `ensure_content_hash` exists for: called on an entry with
    /// no recorded hash it computes and records one from the real file: called
    /// again it must return that *same* value even if the file underneath has
    /// since changed, because the whole point of recording it once is not to
    /// pay for a fresh pass over 100+ MB of engine on every ordinary launch.
    #[test]
    fn ensure_content_hash_computes_once_and_trusts_what_it_recorded() {
        let scratch = Scratch::new("hash-ensure");
        let dir = build(scratch.path(), "2.738.0.1393");
        std::fs::write(dir.join(crate::engine::LIBRARY), b"the real engine bytes").unwrap();

        let first = ensure_content_hash(&dir).expect("a library is there to hash");
        assert_eq!(first, Sha256Hash::of(b"the real engine bytes"));

        // The file changes underneath -- corruption, or a bug elsewhere -- but
        // nothing here re-reads it, because a hash was already recorded.
        std::fs::write(dir.join(crate::engine::LIBRARY), b"different bytes entirely").unwrap();
        let second = ensure_content_hash(&dir).unwrap();
        assert_eq!(second, first, "the stale recorded hash, not a fresh one");
    }

    #[test]
    fn find_by_content_hash_locates_the_matching_entry() {
        let scratch = Scratch::new("hash-find");
        let root = scratch.path();
        let a = build(root, "2.734.0.917");
        let b = build(root, "2.738.0.1393");
        std::fs::write(a.join(crate::engine::LIBRARY), b"engine A").unwrap();
        std::fs::write(b.join(crate::engine::LIBRARY), b"engine B").unwrap();
        ensure_content_hash(&a);
        let hash_b = ensure_content_hash(&b).unwrap();

        let found = find_by_content_hash(root, &hash_b).expect("engine B is in the store");
        assert_eq!(found.version, "2.738.0.1393");
        assert!(find_by_content_hash(root, &Sha256Hash::of(b"nothing kept this")).is_none());
    }

    /// The race this store lock exists to close: three code paths mutate one
    /// directory and, before ADR-037, two of them used different lock files
    /// and one used none. Holding the lock externally and measuring how long a
    /// blocked mutator waits is the only way to observe "serialised" rather
    /// than merely "did not corrupt anything on this run".
    #[test]
    fn concurrent_mutations_serialize_on_the_store_lock() {
        let scratch = Scratch::new("concurrent");
        let root = scratch.path().join("builds");
        std::fs::create_dir_all(&root).unwrap();
        build(&root, "1.0");

        let held = lock(&root).unwrap();
        let waited_root = root.clone();
        let start = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            // Must block until the lock taken above is released below.
            gc_in(&waited_root, &[], 5);
            start.elapsed()
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(held);
        let elapsed = handle.join().unwrap();
        assert!(elapsed >= std::time::Duration::from_millis(180), "gc_in ran concurrently: {elapsed:?}");
    }

    // ---- planning ----

    fn fake(version: &str, complete: bool, signed: bool) -> Entry {
        Entry {
            version: version.into(),
            dir: PathBuf::from("/nonexistent").join(version),
            loaded_by: None,
            bytes: 0,
            complete,
            content_hash: None,
            signer: signed.then(|| "ab".repeat(32)),
            provenance: None,
            version_code: None,
        }
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.version.as_str()).collect()
    }

    /// Sorted as text `2.99` beats `2.738`, and Latest would pick the wrong
    /// build in whatever order the directory listing happened to come back.
    #[test]
    fn latest_is_the_numerically_newest_whatever_order_it_is_given() {
        let all = [fake("2.99.0.1", true, true), fake("2.738.0.1397", true, true), fake("2.734.0.917", true, true)];
        assert_eq!(latest(&all).unwrap().version, "2.738.0.1397");
        let reversed: Vec<Entry> = all.iter().rev().cloned().collect();
        assert_eq!(latest(&reversed).unwrap().version, "2.738.0.1397");
        assert_eq!(names(&all), ["2.99.0.1", "2.738.0.1397", "2.734.0.917"]);
    }

    #[test]
    fn latest_ignores_what_cannot_be_launched() {
        let all = [
            fake("2.740.0.5", true, false),
            fake("2.739.0.1", false, true),
            fake("2.738.0.1397", true, true),
        ];
        assert_eq!(resolve(Choice::Latest, &all), Resolved::Entry("2.738.0.1397".into()));
    }

    #[test]
    fn latest_of_an_empty_store_is_the_first_run_state_and_of_an_unusable_one_a_refusal() {
        assert_eq!(resolve(Choice::Latest, &[]), Resolved::Refused(Refusal::Empty));
        let unusable = [fake("2.740.0.5", true, false), fake("2.739.0.1", false, true)];
        let Resolved::Refused(Refusal::NothingUsable { incomplete, unsigned }) = resolve(Choice::Latest, &unusable)
        else {
            panic!("expected a refusal naming what is unusable");
        };
        assert_eq!(incomplete, ["2.739.0.1"]);
        assert_eq!(unsigned, ["2.740.0.5"]);
        // The sentence names both builds, because a user has to be able to
        // tell which one is the problem.
        let said = Refusal::NothingUsable { incomplete, unsigned }.to_string();
        assert!(said.contains("2.740.0.5") && said.contains("2.739.0.1"), "{said}");
    }

    /// A pin is never turned into Latest: that would run the build the user
    /// pinned away from, silently.
    #[test]
    fn a_pin_refuses_when_its_entry_is_missing_incomplete_or_unsigned() {
        let all = [
            fake("2.738.0.1397", true, true),
            fake("2.734.0.917", false, true),
            fake("2.730.0.1", true, false),
        ];
        assert_eq!(resolve(Choice::Pinned("2.738.0.1397"), &all), Resolved::Entry("2.738.0.1397".into()));
        assert_eq!(
            resolve(Choice::Pinned("2.700.0.1"), &all),
            Resolved::Refused(Refusal::Missing("2.700.0.1".into()))
        );
        assert_eq!(
            resolve(Choice::Pinned("2.734.0.917"), &all),
            Resolved::Refused(Refusal::Incomplete("2.734.0.917".into()))
        );
        assert_eq!(
            resolve(Choice::Pinned("2.730.0.1"), &all),
            Resolved::Refused(Refusal::Unsigned("2.730.0.1".into()))
        );
        for refusal in [
            Refusal::Missing("2.700.0.1".into()),
            Refusal::Incomplete("2.734.0.917".into()),
            Refusal::Unsigned("2.730.0.1".into()),
        ] {
            assert!(refusal.to_string().contains("pinned to Roblox"), "{refusal}");
        }
    }

    fn store_of(versions: &[&str]) -> Vec<Entry> {
        versions.iter().map(|v| fake(v, true, true)).collect()
    }

    #[test]
    fn gc_keeps_the_newest_the_pinned_the_in_use_and_the_spare() {
        let all = store_of(&["2.742.0.9", "2.740.0.5", "2.738.0.1397", "2.734.0.917", "2.730.0.1", "2.700.0.1"]);
        // Newest, one spare (2.740), a pin (2.700) and one in use (2.734).
        let doomed = gc_plan(&all, &["2.700.0.1".into()], &["2.734.0.917".into()], 1);
        assert_eq!(doomed, ["2.738.0.1397", "2.730.0.1"]);
        // Spare zero is the literal "newest only".
        assert_eq!(gc_plan(&all, &[], &[], 0), ["2.740.0.5", "2.738.0.1397", "2.734.0.917", "2.730.0.1", "2.700.0.1"]);
    }

    /// The reason `KEEP = 3` is gone: it counted a pin towards the bound, so
    /// two pinned old builds pushed the one before the newest out.
    #[test]
    fn pinned_builds_do_not_count_against_the_spare() {
        let all = store_of(&["2.742.0.9", "2.740.0.5", "2.738.0.1397", "2.700.0.1", "2.690.0.1"]);
        let doomed = gc_plan(&all, &["2.700.0.1".into(), "2.690.0.1".into()], &[], 1);
        assert_eq!(doomed, ["2.738.0.1397"], "2.740 stays as the spare although two older builds are pinned");
    }

    #[test]
    fn gc_never_returns_a_name_that_is_not_a_version_and_is_idempotent() {
        let mut all = store_of(&["2.742.0.9", "2.740.0.5", "2.738.0.1397"]);
        all.push(fake("../../etc", true, true));
        all.push(fake("scratch", true, true));
        let doomed = gc_plan(&all, &[], &[], 0);
        assert!(doomed.iter().all(|v| is_valid_version(v)), "{doomed:?}");
        assert_eq!(doomed, ["2.740.0.5", "2.738.0.1397"]);
        let remaining: Vec<Entry> = all.into_iter().filter(|e| !doomed.contains(&e.version)).collect();
        assert!(gc_plan(&remaining, &[], &[], 0).is_empty(), "applying the plan and asking again changes nothing");
        assert!(gc_plan(&[], &[], &[], 1).is_empty());
    }

    #[test]
    fn gc_with_one_build_removes_nothing() {
        assert!(gc_plan(&store_of(&["2.742.0.9"]), &[], &[], 0).is_empty());
    }

    fn legacy() -> Legacy {
        Legacy {
            store_has_complete_entry: false,
            slot: Slot::Absent,
            slot_archive: None,
            orphan_archive: None,
            managed_apk: None,
            sober_apk: None,
            settings_apk: None,
        }
    }

    #[test]
    fn a_slot_linked_into_the_store_or_a_store_with_a_build_has_nothing_to_migrate() {
        let sober = PathBuf::from("/home/u/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk");
        assert!(migration_plan(&Legacy { slot: Slot::LinkIntoStore, ..legacy() }).is_empty());
        assert!(migration_plan(&Legacy { store_has_complete_entry: true, sober_apk: Some(sober), ..legacy() })
            .is_empty());
    }

    #[test]
    fn an_unkeyed_slot_is_filed_from_the_archive_its_stamp_names() {
        let sober = PathBuf::from("/home/u/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk");
        let plan = migration_plan(&Legacy {
            slot: Slot::Directory,
            slot_archive: Some(sober.clone()),
            ..legacy()
        });
        assert_eq!(plan, [Step::File { apk: sober, source: Source::Sober }]);

        let managed = PathBuf::from("/home/u/.cache/cordial/build/x86_64/base.apk");
        let plan = migration_plan(&Legacy {
            slot: Slot::Directory,
            slot_archive: None,
            managed_apk: Some(managed.clone()),
            ..legacy()
        });
        assert_eq!(plan, [Step::File { apk: managed, source: Source::Legacy }]);
    }

    /// Sober was what the old launch ran when the slot is a real directory that
    /// names nothing else: the fallback after the stamp's own archive.
    #[test]
    fn a_slot_with_no_archive_of_its_own_falls_back_to_sober() {
        let sober = PathBuf::from("/home/u/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk");
        let plan = migration_plan(&Legacy { slot: Slot::Directory, sober_apk: Some(sober.clone()), ..legacy() });
        assert_eq!(plan, [Step::File { apk: sober, source: Source::Sober }]);
    }

    /// **The case the ADR is most careful about.** Sober installed, Cordial
    /// never run (or its cache wiped): no slot, so nothing is filed silently and
    /// the first-run screen offers the download and the copy.
    #[test]
    fn sober_alone_is_never_imported_without_being_asked() {
        let sober = PathBuf::from("/home/u/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk");
        assert!(migration_plan(&Legacy { sober_apk: Some(sober.clone()), ..legacy() }).is_empty());
        assert!(migration_plan(&Legacy { slot: Slot::LinkIntoStore, sober_apk: Some(sober), ..legacy() }).is_empty());
        assert!(migration_plan(&legacy()).is_empty());
    }

    /// The 0.24.1 Flatpak's shape: an engine-only entry the old launch kept
    /// from Sober's directory, a slot that no longer resolves, and nothing else.
    /// Without this the first launch after the upgrade refuses every launch
    /// ("kept without the APK it came from") and files nothing.
    #[test]
    fn an_entry_kept_without_its_archives_is_completed_from_the_archive_it_names() {
        let sober = PathBuf::from("/home/u/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk");
        let plan = migration_plan(&Legacy { orphan_archive: Some(sober.clone()), ..legacy() });
        assert_eq!(plan, [Step::File { apk: sober, source: Source::Sober }]);
        // Not a Sober path: filed as the old layout's own, like a managed build.
        let kept = PathBuf::from("/home/u/Downloads/roblox/base.apk");
        let plan = migration_plan(&Legacy { orphan_archive: Some(kept.clone()), ..legacy() });
        assert_eq!(plan, [Step::File { apk: kept, source: Source::Legacy }]);
        // A store that already has a complete build has nothing to complete.
        assert!(migration_plan(&Legacy {
            store_has_complete_entry: true,
            orphan_archive: Some(PathBuf::from("/x/base.apk")),
            ..legacy()
        })
        .is_empty());
    }

    #[test]
    fn a_settings_apk_is_imported_even_when_the_store_has_builds() {
        let chosen = PathBuf::from("/home/u/Downloads/roblox.apk");
        let plan = migration_plan(&Legacy {
            store_has_complete_entry: true,
            settings_apk: Some(chosen.clone()),
            sober_apk: Some(PathBuf::from("/x/sober/packages/x86_64/com.roblox.client/base.apk")),
            ..legacy()
        });
        assert_eq!(plan, [Step::ImportChosen { apk: chosen }]);
    }

    #[test]
    fn provenance_and_signer_round_trip_and_a_replaced_archive_unsigns_the_entry() {
        let scratch = Scratch::new("records");
        let dir = build(scratch.path(), "2.738.0.1393");
        std::fs::write(dir.join(crate::install::BASE_APK), b"the archive").unwrap();
        assert_eq!(signer_of(&dir), None);
        record_signer(&dir, "ABCD").unwrap();
        assert_eq!(signer_of(&dir).as_deref(), Some("abcd"));
        // The record names the bytes, not the place: moving the entry keeps it.
        let moved = scratch.path().join("moved");
        std::fs::rename(&dir, &moved).unwrap();
        assert_eq!(signer_of(&moved).as_deref(), Some("abcd"));
        // A replaced archive is a different archive.
        std::fs::write(moved.join(crate::install::BASE_APK), b"a different archive entirely").unwrap();
        assert_eq!(signer_of(&moved), None);
        // The slot's record format (three fields on line two) vouches for nothing.
        std::fs::write(moved.join(SIGNER), "abcd\n27 1700000000 /somewhere/base.apk\n").unwrap();
        assert_eq!(signer_of(&moved), None);

        assert_eq!(provenance_of(&moved), None);
        record_source(&moved, Source::Sober).unwrap();
        let first = provenance_of(&moved).unwrap();
        assert_eq!(first.source, Source::Sober);
        assert!(first.at.unwrap() > 1_700_000_000);
        record_source(&moved, Source::Mirror).unwrap();
        assert_eq!(provenance_of(&moved).unwrap().source, Source::Sober, "the first filing is the record");
    }

    #[test]
    fn a_source_is_told_by_where_the_archive_sits() {
        let sober = Path::new("/home/u/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk");
        assert_eq!(Source::for_path(sober), Source::Sober);
        assert_eq!(Source::for_path(Path::new("/home/u/.local/share/sober/packages/x86_64/c/base.apk")), Source::Sober);
        assert_eq!(Source::for_path(Path::new("/home/u/Downloads/roblox.apk")), Source::File);
        for s in [Source::Mirror, Source::Sober, Source::File, Source::Legacy] {
            assert_eq!(Source::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn gc_skips_a_build_a_client_holds_and_takes_it_once_the_client_is_gone() {
        let scratch = Scratch::new("gc-in-use");
        let root = scratch.path();
        for v in ["2.742.0.9", "2.738.0.1", "2.734.0.1"] {
            build(root, v);
        }
        let running = hold_in_use(&root.join("2.734.0.1")).unwrap();
        // Spare zero: 2.738 and 2.734 are both candidates; only the idle one goes.
        assert_eq!(gc_in(root, &[], 0), ["2.738.0.1"]);
        assert!(root.join("2.734.0.1").is_dir(), "held by a client");
        assert!(is_in_use(&root.join("2.734.0.1")));

        drop(running);
        assert_eq!(gc_in(root, &[], 0), ["2.734.0.1"]);
        assert_eq!(list_in(root).len(), 1, "the newest is never removed");
    }

    #[test]
    fn two_clients_may_share_a_build_and_removal_refuses_it_by_name() {
        let scratch = Scratch::new("in-use-shared");
        let root = scratch.path().join("builds");
        let dir = launchable(&root, "2.738.0.1");
        launchable(&root, "2.742.0.9");
        let a = hold_in_use(&dir).unwrap();
        let b = hold_in_use(&dir).expect("a second client on the same build is fine");
        let refused = remove_in(&root, "2.738.0.1", &[]).unwrap_err();
        assert!(refused.contains("running"), "{refused}");
        drop((a, b));
        remove_in(&root, "2.738.0.1", &[]).unwrap();
    }

    #[test]
    fn a_build_that_is_being_removed_cannot_be_started() {
        let scratch = Scratch::new("in-use-removing");
        let dir = build(scratch.path(), "2.738.0.1");
        let claim = claim_unused(&dir).expect("nothing holds it");
        let refused = hold_in_use(&dir).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        drop(claim);
        hold_in_use(&dir).unwrap();
    }

    #[test]
    fn two_entries_have_two_asset_trees_and_removing_one_takes_its_own() {
        let scratch = Scratch::new("assets");
        let root = scratch.path();
        let a = build(root, "2.738.0.1");
        let b = build(root, "2.734.0.1");
        assert_ne!(assets_dir(&a), assets_dir(&b));
        assert_eq!(assets_dir(&a), a.join("assets"));
        std::fs::create_dir_all(assets_dir(&b).join("content")).unwrap();
        std::fs::write(assets_dir(&b).join("content/x"), b"x").unwrap();
        assert_eq!(gc_in(root, &[], 0), ["2.734.0.1"]);
        assert!(!b.exists(), "its assets went with it");
        assert!(a.is_dir());
    }

    /// What the runtime does with `--lib-dir`: a store entry gets its own tree,
    /// through the slot's link as well as directly, and anything else has none.
    #[test]
    fn the_assets_root_follows_the_entry_and_a_foreign_lib_dir_has_none() {
        let scratch = Scratch::new("assets-root");
        let root = scratch.path().join("builds");
        let a = build(&root, "2.738.0.1");
        let b = build(&root, "2.734.0.1");
        let live = scratch.path().join("lib/x86_64");
        point_current_at(&live, &a).unwrap();

        let of = |p: &Path| entry_at_in(&root, p).map(|e| assets_dir(&e));
        let (ra, rb) = (of(&a).unwrap(), of(&b).unwrap());
        assert_ne!(ra, rb, "two versions, two trees");
        assert_eq!(of(&live), Some(ra), "through the slot's link it is the same entry's tree");

        let foreign = scratch.path().join("somewhere/lib");
        std::fs::create_dir_all(&foreign).unwrap();
        assert_eq!(of(&foreign), None, "a hand-typed --lib-dir keeps the shared tree");
        assert_eq!(of(&root.join("missing")), None);
    }

    #[test]
    fn the_store_is_data_and_the_old_one_moves_in_whole_once() {
        let scratch = Scratch::new("relocate");
        let (from, to) = (scratch.path().join("cache/builds"), scratch.path().join("data/builds"));
        let a = launchable(&from, "2.738.0.1397");
        std::fs::write(a.join("marker"), b"travels with it").unwrap();
        launchable(&from, "2.736.0.1408");
        // The Quest store rides along as one directory, and a stray stays put.
        let quest = from.join("arm64-v8a/2.740.0.927");
        std::fs::create_dir_all(&quest).unwrap();
        std::fs::write(quest.join(crate::engine::LIBRARY), b"q").unwrap();
        std::fs::create_dir_all(from.join("not-a-version")).unwrap();
        std::fs::write(from.join(".store.lock"), b"").unwrap();

        let moved = relocate(&from, &to).unwrap();
        assert_eq!(moved, ["2.736.0.1408", "2.738.0.1397", "arm64-v8a"]);
        assert_eq!(std::fs::read(to.join("2.738.0.1397/marker")).unwrap(), b"travels with it");
        assert!(to.join("arm64-v8a/2.740.0.927").join(crate::engine::LIBRARY).is_file());
        assert!(from.join("not-a-version").is_dir(), "what is not an entry is left alone");
        assert!(!from.join("2.738.0.1397").exists());
        // A moved entry is still signed: the record names the bytes, not the place.
        assert!(list_in(&to).iter().all(|e| e.signer.is_some() && e.launchable()));

        // Once: a second run finds nothing to move and nothing to disturb.
        assert!(relocate(&from, &to).unwrap().is_empty());
        // And a name already at the destination is not overwritten.
        launchable(&from, "2.738.0.1397");
        std::fs::write(from.join("2.738.0.1397/marker"), b"the other one").unwrap();
        assert!(relocate(&from, &to).unwrap().is_empty());
        assert_eq!(std::fs::read(to.join("2.738.0.1397/marker")).unwrap(), b"travels with it");
        assert!(from.join("2.738.0.1397").is_dir());
    }

    #[test]
    fn an_absent_cache_store_moves_nothing_and_creates_nothing() {
        let scratch = Scratch::new("relocate-none");
        let to = scratch.path().join("data/builds");
        assert!(relocate(&scratch.path().join("nope"), &to).unwrap().is_empty());
        assert!(!to.exists());
    }

    #[test]
    fn dates_are_the_utc_day_of_the_filing() {
        assert_eq!(format_date(0), "1 Jan 1970");
        assert_eq!(format_date(1_759_579_200), "4 Oct 2025");
        assert_eq!(format_date(1_791_081_816), "4 Oct 2026");
        assert_eq!(format_date(951_782_400), "29 Feb 2000");
        let p = Provenance { source: Source::Sober, at: Some(1_791_081_816) };
        assert_eq!(p.label(), "Imported from Sober on 4 Oct 2026");
        assert_eq!(Provenance { source: Source::Mirror, at: None }.label(), "Downloaded");
        assert!(Provenance { source: Source::Legacy, at: Some(0) }.label().contains("earlier Cordial"));
    }

    #[test]
    fn an_entry_that_fails_verification_stays_unrecorded_and_is_not_launchable() {
        let scratch = Scratch::new("verify");
        let root = scratch.path().join("builds");
        let dir = build(&root, "2.738.0.1393");
        std::fs::write(dir.join(crate::install::BASE_APK), b"not an apk at all").unwrap();
        let trusted = vec!["44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477".to_string()];
        let checked = ensure_verified(&root, &trusted);
        assert_eq!(checked.len(), 1);
        assert!(checked[0].1.is_err(), "{:?}", checked[0]);
        assert!(!list_in(&root)[0].launchable());
        // Nothing to check about an entry that is already recorded, or has no archive.
        launchable(&root, "2.740.0.1");
        build(&root, "2.730.0.1");
        assert_eq!(ensure_verified(&root, &trusted).len(), 1, "the failing one is retried; the rest are not");
    }
}
