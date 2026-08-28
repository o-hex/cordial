//! The build directory Cordial owns, and filling it.
//!
//! Until now the only build Cordial could run was one somebody else had put
//! somewhere: in practice Sober's, at
//! `~/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/`.
//! That works and it is not going away as a *place to read from*, but it means
//! Cordial only runs if a different Roblox launcher is installed and has
//! already downloaded something. This module is the other half: one directory
//! Cordial owns, that Cordial can fill.
//!
//! ## Read anywhere, write in one place
//!
//! [`build_dir`] is under the user's cache and is the only directory anything
//! here writes an APK into. A user may point Cordial at a build wherever they
//! like and it will be read; if they point it inside another application's
//! Flatpak data, a download **diverts** to [`build_dir`] rather than writing
//! there ([`Destination`]). Writing into somebody else's install is not ours to
//! do, and quietly replacing Sober's copy would break Sober.
//!
//! ADR-015 is the other half of that sentence: what lands here came from the
//! user's own request, stays on the user's own machine, and is never re-served.
//!
//! ## Both halves, and the trap
//!
//! **`base.apk` does not contain the engine.** On a split build `libroblox.so`
//! is in `split_config.x86_64.apk` beside it, and anything that fetches "the
//! APK" and stops has fetched the half without the engine in it. Both names are
//! constants here, [`Parts`](crate::download::Parts) carries both, and
//! [`install_into`] refuses if no part it fetched holds
//! [`apk::LIBRARY_IN_APK`] — naming the split, because that refusal is the one
//! somebody will meet and the sentence they need is which file is missing.
//!
//! Assets come out of `base.apk` at runtime and the engine comes out of the
//! split, so both are kept rather than the engine being extracted and the
//! archives thrown away.
//!
//! ## The extracted engine stays where it is
//!
//! [`engine_dir`] is `~/.cache/cordial/lib/x86_64`, unchanged and deliberately
//! not moved under [`build_dir`]. Three things already agree on that path —
//! `justfile`'s recipes, `cordial-shell`'s `install::engine_cache`, and every
//! working install on a contributor's machine — and moving it would buy tidiness
//! at the cost of invalidating all of them at once. They are siblings under one
//! cache root, which is what the swap below needs: a rename between them stays
//! on one filesystem.
//!
//! ## Nothing replaces a working build until the replacement is complete
//!
//! The worst failure this feature can produce is somebody losing the client they
//! had because a download was interrupted. So the order is: fetch every part
//! into a staging directory, verify each against its published hash, check each
//! as a zip against ADR-014's refusals, extract the engine out of the staged
//! archive — and only then move anything into place. Every failure before that
//! last step leaves the previous build exactly as it was.
//!
//! Two things move into place, separately, and each is made safe for a
//! different reason.
//!
//! **The engine** is a single rename from the directory it was extracted into
//! onto [`engine_dir`], so it is atomic on its own. The cache stamp is cleared
//! *before* that rename rather than after, so a process killed on either side
//! of it leaves a cache that re-extracts on the next launch rather than an
//! engine claiming to belong to archives it does not match — the pairing bug
//! [`cache`](crate::cache) exists for.
//!
//! **The two archives are not one rename**, and an earlier version of this
//! swapped them one after the other with nothing to undo the first if the
//! second failed — a process killed, or simply a copy that ran out of disk,
//! between the two could leave an engine's carrier from one build sitting
//! beside assets from another, on disk, with the marker and the (by-then
//! cleared) stamp giving no sign anything was wrong. `adopt`'s `swap_archives`
//! step fixes that by preparing every archive fully inside a directory of its
//! own, off to the side of anything live, before touching `build` at all, and
//! then promoting the whole set with each promoted file's previous content
//! kept until every other file has landed too — so a failure partway through
//! puts back what had already been swapped, and `build` ends up wholly the old
//! pair or wholly the new one. What that cannot close is a kill landing in the
//! gap between one rename syscall and the next during promotion itself; on one
//! filesystem, without a further directory indirection this crate does not
//! add, no set of plain renames can.

use crate::apk;
use crate::cache;
use crate::download::{self, Parts, Refusal as DownloadRefusal};
use crate::engine;
use std::fmt;
use std::path::{Path, PathBuf};

/// The archive the assets come out of.
pub const BASE_APK: &str = "base.apk";

/// The archive the engine comes out of on a split build, which is every build
/// this has been run against.
///
/// **Underscore, not hyphen.** Play names the split for an ABI with the ABI's
/// characters normalised -- `split_config.arm64_v8a.apk` -- while the directory
/// inside the archive keeps the hyphen, `lib/arm64-v8a/`. The two spellings of
/// one ABI sit four lines apart in this crate for that reason.
#[cfg(target_arch = "x86_64")]
pub const SPLIT_APK: &str = "split_config.x86_64.apk";
#[cfg(target_arch = "aarch64")]
pub const SPLIT_APK: &str = "split_config.arm64_v8a.apk";

/// Where the staged download lives while it is being checked. Inside
/// [`build_dir`] so the move into place is a rename rather than a copy, and
/// dot-prefixed so nothing looking for an APK finds a half-written one.
const STAGING: &str = ".incoming";

/// `$XDG_CACHE_HOME/cordial`, or `~/.cache/cordial`.
pub fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial")
}

/// The build Cordial manages: `~/.cache/cordial/build/<abi>`.
///
/// Named by ABI rather than fixed, so a machine that has run both an x86-64 and
/// an aarch64 build against the same home directory does not have them collide
/// in one cache. That costs nothing today and is the kind of thing nobody
/// notices is missing until two builds have already overwritten each other.
pub fn build_dir() -> PathBuf {
    cache_root().join("build").join(crate::apk::HOST_ABI)
}

/// The extracted engine: `~/.cache/cordial/lib/<abi>`.
///
/// `cordial-shell`'s `install::engine_cache` computes the same path and
/// `justfile` writes the same string into it. Three copies of one path is two
/// too many; this is the one the crate that owns the stamp offers, so the others
/// can delegate.
pub fn engine_dir() -> PathBuf {
    cache_root().join("lib").join(crate::apk::HOST_ABI)
}

/// The managed `base.apk`, if Cordial has one.
///
/// What a launcher should prefer over anything it detects elsewhere: a build
/// Cordial fetched is one it knows the version of and can replace, and a build
/// found in another application's directory is neither.
pub fn managed_base() -> Option<PathBuf> {
    managed_base_in(&build_dir())
}

pub fn managed_base_in(dir: &Path) -> Option<PathBuf> {
    let base = dir.join(BASE_APK);
    base.is_file().then_some(base)
}

/// Whether Cordial may write to `path`: it is inside the cache root.
///
/// Lexical rather than canonical, for [`apk`]'s reason — `canonicalize` answers
/// about files that exist and follows symlinks to do it, and this is asked about
/// a directory that may not have been created yet.
pub fn ours(path: &Path) -> bool {
    path.starts_with(cache_root())
}

/// Where a download may land, given where the user has pointed Cordial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// The directory the download will be written into.
    pub dir: PathBuf,
    /// Set when the user's own location was not somewhere Cordial may write, so
    /// the download went to [`build_dir`] instead.
    pub diverted_from: Option<PathBuf>,
}

impl Destination {
    /// One line for the user, or nothing when there is nothing to say.
    pub fn note(&self) -> Option<String> {
        self.diverted_from.as_ref().map(|from| {
            format!(
                "{} belongs to another app, so the download goes to {} instead.",
                from.display(),
                self.dir.display()
            )
        })
    }
}

/// Decide where to put a download.
///
/// A user who has pointed `--apk` at a build inside Cordial's own directory gets
/// it updated in place. A user who has pointed it at Sober's copy — the ordinary
/// case today — gets the download in Cordial's directory and a line saying so,
/// rather than Cordial writing into another application's storage.
pub fn destination(user_apk: Option<&Path>) -> Destination {
    destination_under(user_apk, &build_dir())
}

pub fn destination_under(user_apk: Option<&Path>, managed: &Path) -> Destination {
    match user_apk.and_then(Path::parent) {
        Some(dir) if ours(dir) => Destination { dir: dir.to_path_buf(), diverted_from: None },
        Some(dir) => {
            Destination { dir: managed.to_path_buf(), diverted_from: Some(dir.to_path_buf()) }
        }
        None => Destination { dir: managed.to_path_buf(), diverted_from: None },
    }
}

/// What a completed install left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The APK a launcher should be pointed at.
    pub base: PathBuf,
    /// The archive the engine came out of — the split, on every build seen.
    pub carrier: PathBuf,
    pub engine: PathBuf,
    /// Read back out of the engine that was just written, so it describes what
    /// is on disk rather than what was expected to arrive.
    pub version: Option<String>,
}

/// Why an install did not finish. Every one of these leaves the previous build
/// in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failed {
    /// Nothing was fetched: no source, a refusal, a bad hash, a dead URL.
    Download(DownloadRefusal),
    /// Something arrived and was not an archive Cordial will unpack.
    Archive(apk::Refusal),
    /// The download succeeded and the engine is not in any of it. The one that
    /// catches the two-APK trap.
    NoEngine { fetched: Vec<String> },
    Io { path: String, why: String },
    /// The managed directory holds a build Cordial did not put there.
    ///
    /// **This is the foot-gun the whole ownership rule exists for.** Somebody
    /// drops an APK into Cordial's own build directory, an update arrives, and
    /// the file they chose is silently replaced by one from a mirror. Cordial
    /// writes only what it installed, so an unmarked directory stops the
    /// install rather than overwriting it.
    NotOurs { path: String },
    /// The caller asked to stop, and was still owed an answer for it.
    ///
    /// Only reachable before the swap begins -- see the `Cancel` checks in
    /// [`adopt`]. Once a live file has started being replaced, stopping is no
    /// longer offered: the alternative to finishing a rename is a build left
    /// half swapped in, which is worse than a cancel button that occasionally
    /// finishes what it was asked to stop.
    Cancelled,
}

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failed::Download(r) => write!(f, "{r}"),
            Failed::Archive(r) => write!(f, "{r}"),
            Failed::NoEngine { fetched } => write!(
                f,
                "None of the downloaded files contains the Roblox engine ({}). \
                 It lives in {SPLIT_APK}, not {BASE_APK}.",
                fetched.join(", ")
            ),
            Failed::NotOurs { path } => write!(
                f,
                "{path} holds a Roblox build Cordial did not install, so it will not be \
                 overwritten. Choose that APK on the Roblox page in Settings and Cordial will \
                 use it and leave it alone, or delete the directory to let Cordial manage one."
            ),
            Failed::Io { path, why } => write!(f, "{path}: {why}"),
            Failed::Cancelled => write!(f, "the install was stopped before anything was replaced"),
        }
    }
}

impl std::error::Error for Failed {}

impl From<DownloadRefusal> for Failed {
    fn from(r: DownloadRefusal) -> Self {
        Failed::Download(r)
    }
}

impl From<apk::Refusal> for Failed {
    fn from(r: apk::Refusal) -> Self {
        Failed::Archive(r)
    }
}

/// How far along the whole install is: which file, bytes so far, and the total
/// if the server said.
///
/// The name is in it because there are two files and a bar that restarts with no
/// explanation reads as a download that failed and began again.
pub type Progress<'a> = &'a mut dyn FnMut(&str, u64, Option<u64>);

/// Fetch a build into the directory Cordial owns and make it the one in use.
pub fn install(
    parts: &Parts,
    cancel: &crate::provider::Cancel,
    progress: Progress<'_>,
) -> Result<Installed, Failed> {
    install_into(parts, &build_dir(), &engine_dir(), cancel, progress)
}

/// The whole sequence, with both directories named so it can be tested without
/// going anywhere near the cache somebody is using to launch.
///
/// The staging directories are cleared here rather than at each failure inside
/// [`staged`], so there is one statement of "nothing half-fetched is left
/// behind" instead of one per `return`. The first version of this had them
/// inline and missed the earliest one, which is the failure most likely to
/// happen: a download that dies part way through.
pub fn install_into(
    parts: &Parts,
    build: &Path,
    engine_into: &Path,
    cancel: &crate::provider::Cancel,
    progress: Progress<'_>,
) -> Result<Installed, Failed> {
    let staging = build.join(STAGING);
    let incoming = engine_into.join(STAGING);
    let result = staged(parts, build, engine_into, &staging, &incoming, cancel, progress);
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&incoming);
    result
}

fn staged(
    parts: &Parts,
    build: &Path,
    engine_into: &Path,
    staging: &Path,
    incoming: &Path,
    cancel: &crate::provider::Cancel,
    progress: Progress<'_>,
) -> Result<Installed, Failed> {
    // Anything left by an interrupted attempt is not something to resume. It was
    // never verified, and a `.partial` from a previous run is exactly the file
    // this whole ordering exists to keep away from the live build.
    let _ = std::fs::remove_dir_all(staging);
    let _ = std::fs::remove_dir_all(incoming);
    std::fs::create_dir_all(staging)
        .map_err(|e| Failed::Io { path: staging.display().to_string(), why: e.to_string() })?;

    let mut fetched: Vec<(&'static str, PathBuf)> = Vec::new();
    for (name, source) in parts.named() {
        let path = download::fetch_named(source, staging, name, &mut |n, total| {
            progress(name, n, total)
        })?;
        fetched.push((name, path));
    }

    adopt(&fetched, build, engine_into, incoming, cancel, progress)
}

/// Take archives that are already on disk and make them the build in use.
///
/// Split out of [`staged`] so that [`crate::provider`] can reach the same
/// sequence with files it obtained and verified itself, rather than through
/// [`Parts`] and a URL. Everything after "the bytes are here" is identical and
/// having two copies of it would be how the two paths drift.
///
/// **A file that is not already inside `build` is copied, not moved.** The
/// local provider hands back the archive at the path it already occupies,
/// which on most machines is Sober's own package directory -- renaming it
/// into Cordial's cache would take Roblox out from underneath Sober and break
/// a working program to install this one.
///
/// **Neither kind of file ever lands on `live` directly, and neither lands
/// alone.** Streaming a copy straight onto the live path was tried first and
/// was wrong: `fs::copy` opens its destination with truncation before a byte
/// moves, and `live` is often the *previous* build, so a process killed
/// mid-copy left an archive exactly as long as the copy had reached, and no
/// further. `swap_archives` instead copies or renames every fetched archive
/// into a `prepared` directory first, entirely off to the side of `build`,
/// and only then promotes the whole set -- each promoted file's previous
/// content parked in a `displaced` directory until every other file in the
/// set has landed too, so one failing does not leave the rest half swapped.
/// Both directories live inside [`STAGING`], which gets cleared with
/// everything else fetched for this attempt; see `swap_archives` for why the
/// promotion step itself is nothing but renames.
///
/// This constant's own file is written beside a build Cordial installed, so
/// it can tell its own work from somebody else's.
///
/// The contents do not matter and are not parsed; **its presence is the whole
/// signal.** Anything richer would be a format to keep compatible for the sake
/// of a question with a yes/no answer.
pub const OURS: &str = ".cordial-managed";

/// Whether `build` is a directory Cordial installed into, or is empty.
///
/// An empty or absent directory is ours to take. One with archives in it and no
/// marker belongs to whoever put them there.
pub fn ours_to_write(build: &Path) -> bool {
    if build.join(OURS).is_file() {
        return true;
    }
    let occupied = [BASE_APK, SPLIT_APK].iter().any(|n| build.join(n).is_file());
    !occupied
}

pub fn adopt(
    fetched: &[(&'static str, PathBuf)],
    build: &Path,
    engine_into: &Path,
    incoming: &Path,
    cancel: &crate::provider::Cancel,
    progress: Progress<'_>,
) -> Result<Installed, Failed> {
    let _ = progress;

    // **Before anything is unpacked, not after.** Checking once the engine has
    // been extracted would mean refusing after several minutes of work, and the
    // point of the refusal is that the user's file is still there.
    if !ours_to_write(build) {
        return Err(Failed::NotOurs { path: build.display().to_string() });
    }

    // **Checked here and once more below, and nowhere else.** The slow,
    // network-bound part of an install -- fetching and verifying a signature --
    // already happened in `provider::obtain` before this was ever called, and
    // its own loop checks `cancel` between chunks. What is left here is
    // inspecting and extracting archives already on disk, which on a large
    // local build is still real seconds of hashing and unzipping and is worth
    // being able to stop. Past the second check, below, everything replaces a
    // live file, and a cancel honoured mid-replacement would trade a clean stop
    // for a build half swapped in -- worse than letting a few renames finish.
    if cancel.stopped() {
        return Err(Failed::Cancelled);
    }

    // Every refusal in ADR-014's list, against each archive, before anything is
    // taken out of any of them.
    for (_, path) in fetched {
        apk::inspect(path, apk::Limits::default())?;
    }

    // Which half has the engine. Asked rather than assumed: a universal APK
    // carries it in `base.apk` and a split build does not, and assuming either
    // is the mistake this refuses on behalf of.
    let mut carrier = None;
    for (name, path) in fetched {
        if apk::holds(path, apk::LIBRARY_IN_APK)? {
            carrier = Some((*name, path.clone()));
            break;
        }
    }
    let Some((carrier_name, carrier)) = carrier else {
        return Err(Failed::NoEngine {
            fetched: fetched.iter().map(|(name, _)| name.to_string()).collect(),
        });
    };

    // Out of the *staged* archive and into a directory beside the live engine,
    // so the last thing that can fail happens while the old engine is still the
    // one on disk. `engine_into` is a sibling of `build` under one cache root,
    // which is what makes the final rename a rename.
    let staged_engine = apk::extract(&carrier, apk::LIBRARY_IN_APK, incoming)?;
    let version = engine::version_of(&staged_engine);

    // The last point a cancel is honoured -- see the comment on the first
    // check, above. Everything from here replaces the previous build.
    if cancel.stopped() {
        return Err(Failed::Cancelled);
    }

    // From here the previous build is being replaced. The stamp goes first: a
    // process killed between the renames below leaves a cache that re-extracts,
    // rather than the old engine claiming to belong to the new archives.
    cache::clear_stamp(engine_into);

    std::fs::create_dir_all(build)
        .map_err(|e| Failed::Io { path: build.display().to_string(), why: e.to_string() })?;
    // **Before the archives land, not after.** Written afterwards there is a
    // window -- the whole rename loop -- in which a killed process leaves a
    // directory full of Cordial's own archives with no marker beside them.
    // `ours_to_write` would then read that as somebody else's build and refuse
    // to install over it for ever, which is the ownership rule turned against
    // the only program it exists to serve. The marker has no contents, so
    // writing it early costs nothing and closes the window entirely.
    let _ = std::fs::write(build.join(OURS), b"Installed by Cordial. Delete to disown.\n");

    swap_archives(fetched, build)?;
    // Every name in `fetched` lands at `build.join(name)` or is refused before
    // this point, so these are not read back out of the swap -- they are what
    // it promises.
    let base = build.join(BASE_APK);
    let carrier_live = build.join(carrier_name);

    let engine_live = engine_into.join(engine::LIBRARY);
    std::fs::rename(&staged_engine, &engine_live)
        .map_err(|e| Failed::Io { path: engine_live.display().to_string(), why: e.to_string() })?;

    // Stamped last and only now, for the reason `cache` gives: a stamp written
    // before the engine is on disk claims a cache that is not there.
    if let Err(e) = cache::write_stamp(engine_into, &base) {
        // Not fatal. An unstamped cache re-extracts next launch, which is slow
        // rather than wrong, and losing a working install over a failed write of
        // a 120-byte file would be the wrong trade.
        println!("[update] installed the build but could not stamp the engine cache: {e}");
    }
    if let Some(version) = &version {
        let _ = cache::record_version(engine_into, version);
    }

    Ok(Installed { base, carrier: carrier_live, engine: engine_live, version })
}

/// Land every archive in `fetched` under `build`, as one unit: either all of
/// them replace what was there, or none do.
///
/// **Two passes, not one.** The first -- "prepare" -- copies or moves each
/// archive to a file inside [`STAGING`] named for its final slot, and touches
/// nothing under `build` itself. A failure here, including a real one forced
/// mid-copy with `ulimit -f`, leaves the previous build exactly as it was,
/// because nothing live has been opened for writing yet. The second --
/// "promote" -- does nothing but rename: every rename in it is a metadata
/// change on one filesystem, not a write, so there is no data left for a size
/// limit or a full disk to interrupt partway through. What is left is the gap
/// between one rename syscall and the next, and that is what keeping the
/// displaced originals is for: each promoted file's previous content is
/// renamed aside rather than deleted, and kept until every file in the set has
/// been promoted, so a later failure can put back everything that already
/// landed. This is the fix for the bug this pair of passes replaced: an
/// earlier version promoted each file as soon as it was ready, so a failure on
/// the second of two files left the first one swapped and the second one not
/// -- an engine's carrier from one build sitting beside assets from another,
/// silently, because the marker and the (already-cleared) stamp gave no sign
/// anything had gone wrong.
fn swap_archives(fetched: &[(&'static str, PathBuf)], build: &Path) -> Result<(), Failed> {
    let staging = build.join(STAGING);
    let prepared_dir = staging.join("prepared");
    let displaced_dir = staging.join("displaced");
    // Cleared here rather than trusted to a caller. `install_into` clears the
    // whole of `staging` before and after every attempt, but `adopt` -- and
    // therefore this -- is also reachable directly from `provider`, which has
    // no such wrapper, and a directory only one of two callers ever empties is
    // exactly the shape of the bug that put a `.incoming` file outside every
    // directory anything cleared.
    let _ = std::fs::remove_dir_all(&prepared_dir);
    let _ = std::fs::remove_dir_all(&displaced_dir);

    let result = (|| -> Result<(), Failed> {
        std::fs::create_dir_all(&prepared_dir).map_err(|e| Failed::Io {
            path: prepared_dir.display().to_string(),
            why: e.to_string(),
        })?;
        std::fs::create_dir_all(&displaced_dir).map_err(|e| Failed::Io {
            path: displaced_dir.display().to_string(),
            why: e.to_string(),
        })?;

        // Prepare: get every archive into `prepared_dir`, or note that it is
        // already where it belongs. What is already inside `build` -- in
        // practice `staging` itself, where a network fetch lands -- is moved
        // rather than copied, because it was fetched for this install and
        // nothing else needs it afterward; that is a rename, not a write of
        // 100+ MB, and nothing that only limits bytes written can catch it
        // mid-way. What is from outside `build` -- the local provider's
        // shape, Sober's package directory in practice -- is copied, per the
        // doc comment on `OURS`: it is not this install's to move away from
        // whoever else is using it.
        let mut ready: Vec<(&'static str, Option<PathBuf>)> = Vec::with_capacity(fetched.len());
        for (name, path) in fetched {
            let name = *name;
            let live = build.join(name);
            if path == &live {
                // Already where it belongs -- the local provider returns this
                // shape when `CORDIAL_APK_DIR` points at `build` itself.
                // Nothing to prepare and nothing to promote.
                ready.push((name, None));
                continue;
            }
            let target = prepared_dir.join(name);
            if path.starts_with(build) {
                std::fs::rename(path, &target).map_err(|e| Failed::Io {
                    path: target.display().to_string(),
                    why: e.to_string(),
                })?;
            } else if let Err(e) = std::fs::copy(path, &target) {
                let _ = std::fs::remove_file(&target);
                return Err(Failed::Io { path: target.display().to_string(), why: e.to_string() });
            }
            ready.push((name, Some(target)));
        }

        // Promote: rename every prepared file into `build`, keeping enough
        // behind to undo it. `applied` records, for each file already
        // promoted, where its previous content went -- `None` when there was
        // none, which a name fetched for the first time hits.
        let mut applied: Vec<(PathBuf, Option<PathBuf>)> = Vec::with_capacity(ready.len());
        for (name, target) in &ready {
            let Some(target) = target else { continue };
            let live = build.join(name);
            let displaced = displaced_dir.join(name);
            let had_previous = live.is_file();
            if had_previous {
                if let Err(e) = std::fs::rename(&live, &displaced) {
                    unwind(&applied);
                    return Err(Failed::Io {
                        path: displaced.display().to_string(),
                        why: e.to_string(),
                    });
                }
            }
            if let Err(e) = std::fs::rename(target, &live) {
                if had_previous {
                    let _ = std::fs::rename(&displaced, &live);
                }
                unwind(&applied);
                return Err(Failed::Io { path: live.display().to_string(), why: e.to_string() });
            }
            applied.push((live, had_previous.then_some(displaced)));
        }
        Ok(())
    })();

    // Whatever `prepare` left unmoved on failure, and whatever `promote` left
    // behind on success, is scratch this attempt owned start to finish.
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Put back whatever [`swap_archives`] had already promoted.
///
/// Each entry names a different file, so nothing here depends on the order
/// they are undone in -- reverse is just the shape a rollback conventionally
/// takes, not a correctness requirement the way it would be if one entry
/// could depend on another.
fn unwind(applied: &[(PathBuf, Option<PathBuf>)]) {
    for (live, displaced) in applied.iter().rev() {
        match displaced {
            Some(displaced) => {
                let _ = std::fs::rename(displaced, live);
            }
            None => {
                let _ = std::fs::remove_file(live);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::Source;
    use crate::sha256::Sha256Hash;
    use std::io::{BufRead, Write};
    use std::net::TcpListener;

    /// **The foot-gun this rule exists for, exercised.**
    ///
    /// Somebody drops their own APK into Cordial's build directory. Without the
    /// marker check, the next update replaces it with one from a mirror and
    /// says nothing -- the user's deliberate choice quietly undone by a feature
    /// they may not have known was on.
    #[test]
    fn a_build_cordial_did_not_install_is_never_overwritten() {
        let root = scratch("adopt-not-ours");
        let build = root.join("build");
        std::fs::create_dir_all(&build).unwrap();
        // Their file, with no marker beside it.
        std::fs::write(build.join(BASE_APK), b"the APK I chose myself").unwrap();

        let incoming = root.join("incoming");
        std::fs::create_dir_all(&incoming).unwrap();
        let fresh = incoming.join("base.apk");
        std::fs::write(&fresh, zip_of(&[(apk::LIBRARY_IN_APK, b"\x7fELF newer 9.9.9")])).unwrap();

        let err = adopt(
            &[(BASE_APK, fresh)],
            &build,
            &root.join("engine"),
            &root.join("engine").join(STAGING),
            &crate::provider::Cancel::new(),
            &mut |_, _, _| {},
        )
        .expect_err("a directory Cordial did not fill must not be written into");
        assert!(matches!(err, Failed::NotOurs { .. }), "{err:?}");

        // Untouched, byte for byte. This is the assertion that matters.
        assert_eq!(std::fs::read(build.join(BASE_APK)).unwrap(), b"the APK I chose myself");
        // And the message says both ways out rather than only refusing.
        let said = err.to_string();
        assert!(said.contains("Settings"), "{said}");
        assert!(said.contains("delete"), "{said}");
    }

    /// The other half: once Cordial has installed there, it may install again.
    /// A rule that refused its own directory after the first write would break
    /// every update after the first one.
    #[test]
    fn a_directory_cordial_installed_into_is_one_it_may_write_again() {
        let root = scratch("adopt-ours-again");
        let build = root.join("build");
        std::fs::create_dir_all(&build).unwrap();
        assert!(ours_to_write(&build), "an empty directory is free to take");

        std::fs::write(build.join(BASE_APK), b"whatever").unwrap();
        assert!(!ours_to_write(&build), "an occupied one without the marker is not");

        std::fs::write(build.join(OURS), b"x").unwrap();
        assert!(ours_to_write(&build), "the marker is what makes it ours again");
    }

    /// **A source outside the staging area must be copied and left alone.**
    ///
    /// The local provider hands back the archive where it already is, and on
    /// most machines that is Sober's package directory. Renaming it into
    /// Cordial's cache would take Roblox out from underneath Sober -- breaking
    /// a working program in order to install this one, silently, on the happy
    /// path. This is the test that says it does not.
    #[test]
    fn adopting_a_build_from_elsewhere_does_not_move_it_out_of_elsewhere() {
        let root = scratch("adopt-elsewhere");
        let theirs = root.join("someone-elses");
        std::fs::create_dir_all(&theirs).unwrap();
        let their_apk = theirs.join("base.apk");
        std::fs::write(&their_apk, zip_of(&[(apk::LIBRARY_IN_APK, b"\x7fELF fake engine 1.2.3")]))
            .unwrap();

        let build = root.join("build");
        let engine_into = root.join("engine");
        std::fs::create_dir_all(&engine_into).unwrap();

        let installed = adopt(
            &[(BASE_APK, their_apk.clone())],
            &build,
            &engine_into,
            &engine_into.join(STAGING),
            &crate::provider::Cancel::new(),
            &mut |_, _, _| {},
        )
        .expect("a universal APK carrying the engine installs");

        assert!(their_apk.is_file(), "the source archive was moved rather than copied");
        assert!(installed.base.starts_with(&build), "the live copy is not under the build dir");
        assert_ne!(installed.base, their_apk);
        assert!(installed.engine.is_file());
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-update-install-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A zip holding whatever entries are named. Not an APK and not a Roblox
    /// byte: the entry path is what this code acts on, and the contents are
    /// whatever this test wrote.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// One request, one body, on loopback. Same shape as `download`'s server and
    /// for the same reason: the streaming path is what is being exercised.
    fn serve(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                line.clear();
            }
            let mut out = std::io::BufWriter::new(&stream);
            let _ = write!(
                out,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/vnd.android.package-archive\r\n\r\n",
                body.len()
            );
            let _ = out.write_all(&body);
            let _ = out.flush();
        });
        format!("http://127.0.0.1:{port}/x.apk")
    }

    fn source_for(body: &[u8]) -> Source {
        Source::for_test(serve(body.to_vec()), Sha256Hash::of(body))
    }

    fn silent() -> impl FnMut(&str, u64, Option<u64>) {
        |_, _, _| {}
    }

    /// A `Cancel` nobody has stopped, for tests that are not about stopping.
    fn no_cancel() -> crate::provider::Cancel {
        crate::provider::Cancel::new()
    }

    /// The engine literal this crate's own scanner looks for, in a file this
    /// test wrote. Nothing here comes from Roblox.
    fn engine_bytes(version: &str) -> Vec<u8> {
        let mut v = b"\0not an engine, just the shape of one\0".to_vec();
        v.extend_from_slice(version.as_bytes());
        v.push(0);
        v
    }

    #[test]
    fn a_split_build_installs_both_halves_and_the_engine_out_of_the_split() {
        // The control for everything below, and the trap stated as a test: the
        // engine is in the split and `base.apk` does not have it.
        let dir = scratch("split");
        let build = dir.join("build");
        let engine_into = dir.join("lib");
        let base = zip_of(&[("assets/content/fonts/x.json", b"{}")]);
        let split = zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917"))]);
        let parts =
            Parts { base: source_for(&base), split: Some(source_for(&split)) };

        let installed =
            install_into(&parts, &build, &engine_into, &no_cancel(), &mut silent()).expect("install");

        assert_eq!(installed.base, build.join(BASE_APK));
        assert_eq!(installed.carrier, build.join(SPLIT_APK));
        assert_eq!(installed.engine, engine_into.join(engine::LIBRARY));
        assert!(installed.engine.is_file());
        // Read back out of what landed, which is the point of reading it there
        // rather than trusting what was expected.
        assert_eq!(installed.version.as_deref(), Some("2.734.0.917"));
        assert_eq!(cache::recorded_version(&engine_into).as_deref(), Some("2.734.0.917"));
        assert!(cache::is_current(&engine_into, &installed.base));
        // Both halves kept: the assets come out of `base.apk` at runtime.
        assert!(build.join(BASE_APK).is_file());
        assert!(build.join(SPLIT_APK).is_file());
        assert!(!build.join(STAGING).exists(), "staging is not left behind");
        assert!(!engine_into.join(STAGING).exists());
    }

    #[test]
    fn fetching_only_base_apk_is_refused_and_the_refusal_names_the_split() {
        // The mistake this module exists to make impossible. `base.apk` alone
        // downloads perfectly, verifies perfectly, and has no engine in it.
        let dir = scratch("halfway");
        let base = zip_of(&[("assets/content/fonts/x.json", b"{}")]);
        let parts = Parts { base: source_for(&base), split: None };
        let e = install_into(&parts, &dir.join("build"), &dir.join("lib"), &no_cancel(), &mut silent())
            .unwrap_err();
        assert!(matches!(e, Failed::NoEngine { .. }), "{e}");
        let shown = e.to_string();
        assert!(shown.contains(SPLIT_APK), "{shown}");
    }

    #[test]
    fn a_universal_apk_that_does_carry_the_engine_is_accepted() {
        // The other side of the same check: the engine's location is asked
        // rather than assumed, so a build with everything in one archive works
        // without a split being invented for it.
        let dir = scratch("universal");
        let one = zip_of(&[
            ("assets/content/fonts/x.json", b"{}" as &[u8]),
            (apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917")),
        ]);
        let parts = Parts { base: source_for(&one), split: None };
        let installed =
            install_into(&parts, &dir.join("build"), &dir.join("lib"), &no_cancel(), &mut silent())
                .unwrap();
        assert_eq!(installed.carrier, installed.base);
    }

    #[test]
    fn a_failed_download_leaves_the_working_build_exactly_as_it_was() {
        // The worst thing this feature can do, asserted against. The hash is
        // wrong, so the second part never verifies — and the build already
        // installed has to survive it byte for byte.
        let dir = scratch("survives");
        let build = dir.join("build");
        let engine_into = dir.join("lib");
        let base = zip_of(&[("assets/content/fonts/x.json", b"{}")]);
        let split = zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917"))]);
        install_into(
            &Parts { base: source_for(&base), split: Some(source_for(&split)) },
            &build,
            &engine_into,
            &no_cancel(),
            &mut silent(),
        )
        .unwrap();
        let engine_before = std::fs::read(engine_into.join(engine::LIBRARY)).unwrap();
        let stamp_before = cache::stamp_of(&engine_into);

        let newer = zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.999.0.1"))]);
        let lying = Source::for_test(serve(newer), Sha256Hash::of(b"not what arrived"));
        let e = install_into(
            &Parts { base: source_for(&base), split: Some(lying) },
            &build,
            &engine_into,
            &no_cancel(),
            &mut silent(),
        )
        .unwrap_err();
        assert!(matches!(e, Failed::Download(DownloadRefusal::HashMismatch { .. })), "{e}");

        assert_eq!(std::fs::read(engine_into.join(engine::LIBRARY)).unwrap(), engine_before);
        assert_eq!(cache::stamp_of(&engine_into), stamp_before, "the stamp survives too");
        assert!(cache::is_current(&engine_into, &build.join(BASE_APK)));
        assert!(!build.join(STAGING).exists());
    }

    #[test]
    fn an_archive_that_breaks_adr_014_is_refused_before_anything_is_swapped() {
        let dir = scratch("hostile");
        let build = dir.join("build");
        let engine_into = dir.join("lib");
        let hostile = zip_of(&[("../../.bashrc", b"pwned")]);
        let e = install_into(
            &Parts { base: source_for(&hostile), split: None },
            &build,
            &engine_into,
            &no_cancel(),
            &mut silent(),
        )
        .unwrap_err();
        assert!(matches!(e, Failed::Archive(apk::Refusal::ParentTraversal(_))), "{e}");
        assert!(!build.join(BASE_APK).exists(), "nothing reached the live directory");
    }

    #[test]
    fn the_parts_land_under_the_names_cordial_uses_whatever_the_url_said() {
        // The URLs in these tests all end `/x.apk`. What the directory has to
        // hold is `base.apk` and `split_config.x86_64.apk`, because that is what
        // the launcher and the split-detection look for.
        let dir = scratch("names");
        let build = dir.join("build");
        let base = zip_of(&[("assets/x", b"{}")]);
        let split = zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917"))]);
        install_into(
            &Parts { base: source_for(&base), split: Some(source_for(&split)) },
            &build,
            &dir.join("lib"),
            &no_cancel(),
            &mut silent(),
        )
        .unwrap();
        // Dotfiles excluded: this asserts which *archives* landed and under
        // what names, and Cordial's own bookkeeping beside them -- the
        // `OURS` marker, a stamp -- is not what the test is about.
        let mut names: Vec<String> = std::fs::read_dir(&build)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort();
        assert_eq!(names, vec![BASE_APK.to_string(), SPLIT_APK.to_string()]);
    }

    #[test]
    fn progress_names_the_file_it_is_about() {
        let dir = scratch("progress");
        let base = zip_of(&[("assets/x", b"{}")]);
        let split = zip_of(&[(apk::LIBRARY_IN_APK, &engine_bytes("2.734.0.917"))]);
        let mut seen: Vec<String> = Vec::new();
        install_into(
            &Parts { base: source_for(&base), split: Some(source_for(&split)) },
            &dir.join("build"),
            &dir.join("lib"),
            &no_cancel(),
            &mut |name, _, _| {
                if seen.last().map(String::as_str) != Some(name) {
                    seen.push(name.to_string());
                }
            },
        )
        .unwrap();
        assert_eq!(seen, vec![BASE_APK.to_string(), SPLIT_APK.to_string()]);
    }

    #[test]
    fn a_download_never_writes_into_another_application_s_directory() {
        // The rule, as a test. Sober's copy is a perfectly good build to read
        // and its directory is not ours to write into; the download diverts and
        // says which directory it used.
        let sober = PathBuf::from(
            "/home/someone/.var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk",
        );
        let managed = PathBuf::from("/home/someone/.cache/cordial/build/x86_64");
        let d = destination_under(Some(&sober), &managed);
        assert_eq!(d.dir, managed);
        assert_eq!(d.diverted_from.as_deref(), sober.parent());
        let note = d.note().unwrap();
        assert!(note.contains("another app"), "{note}");
    }

    #[test]
    fn a_build_already_in_cordials_own_directory_is_updated_in_place() {
        let managed = build_dir();
        let mine = managed.join(BASE_APK);
        let d = destination_under(Some(&mine), &managed);
        assert_eq!(d.dir, managed);
        assert_eq!(d.diverted_from, None);
        assert_eq!(d.note(), None);
    }

    #[test]
    fn with_nowhere_chosen_the_download_goes_to_the_managed_directory() {
        let managed = PathBuf::from("/home/someone/.cache/cordial/build/x86_64");
        assert_eq!(
            destination_under(None, &managed),
            Destination { dir: managed, diverted_from: None }
        );
    }

    #[test]
    fn the_engine_cache_and_the_managed_build_share_one_root() {
        // What makes the swap a rename rather than a copy across filesystems,
        // and the reason the engine directory was not moved under the build one.
        assert!(build_dir().starts_with(cache_root()));
        assert!(engine_dir().starts_with(cache_root()));
        assert!(ours(&build_dir()));
        assert!(!ours(Path::new("/home/someone/.var/app/org.vinegarhq.Sober")));
    }

    /// **A cancel asked for before any work starts must be honoured.** This is
    /// the state a press of Stop leaves things in the moment verification has
    /// just finished and `adopt` has not yet been called: nothing has been
    /// touched, so there is nothing to finish.
    #[test]
    fn a_cancel_already_asked_for_is_honoured_before_anything_is_touched() {
        let root = scratch("adopt-cancelled");
        let build = root.join("build");
        let engine_into = root.join("engine");
        let elsewhere = root.join("someone-elses");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let external = elsewhere.join("base.apk");
        std::fs::write(&external, zip_of(&[(apk::LIBRARY_IN_APK, b"\x7fELF engine")])).unwrap();

        let cancel = crate::provider::Cancel::new();
        cancel.stop();
        let err = adopt(
            &[(BASE_APK, external.clone())],
            &build,
            &engine_into,
            &engine_into.join(STAGING),
            &cancel,
            &mut |_, _, _| {},
        )
        .expect_err("a cancel asked for up front must be honoured, not raced past");
        assert!(matches!(err, Failed::Cancelled), "{err:?}");
        assert!(!build.exists(), "nothing was created for a cancel this early");
        assert!(external.is_file(), "the source is untouched");
    }

    /// **The path finding 1 is about, run to completion.** An update landing
    /// from outside `build` -- the local provider's shape, Sober's package
    /// directory in practice -- over a build already installed there. The
    /// control for the interrupted case: this is what a *successful* run
    /// through the copy-then-rename path leaves behind.
    #[test]
    fn updating_from_elsewhere_over_an_existing_install_lands_the_new_content() {
        let root = scratch("adopt-update-elsewhere");
        let build = root.join("build");
        let engine_into = root.join("engine");

        // An existing install, landed the ordinary way -- staged, then
        // renamed -- exactly as a previous run of this module would leave one.
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let old_src = staging.join("base.apk");
        std::fs::write(&old_src, zip_of(&[(apk::LIBRARY_IN_APK, b"\x7fELF old engine 1.0.0")]))
            .unwrap();
        adopt(
            &[(BASE_APK, old_src)],
            &build,
            &engine_into,
            &engine_into.join(STAGING),
            &no_cancel(),
            &mut |_, _, _| {},
        )
        .expect("the first install lands");
        let old_bytes = std::fs::read(build.join(BASE_APK)).unwrap();

        // Now update it from a path outside `build`, which is the copy-then-
        // rename branch finding 1 fixed rather than the plain rename above.
        let elsewhere = root.join("someone-elses");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let new_src = elsewhere.join("base.apk");
        let new_bytes = zip_of(&[(apk::LIBRARY_IN_APK, b"\x7fELF new engine 2.0.0")]);
        std::fs::write(&new_src, &new_bytes).unwrap();

        let installed = adopt(
            &[(BASE_APK, new_src.clone())],
            &build,
            &engine_into,
            &engine_into.join(STAGING),
            &no_cancel(),
            &mut |_, _, _| {},
        )
        .expect("updating over an existing install from outside `build` must still work");

        assert_eq!(std::fs::read(&installed.base).unwrap(), new_bytes);
        assert_ne!(std::fs::read(&installed.base).unwrap(), old_bytes, "the old content is gone");
        assert!(new_src.is_file(), "the source is copied, not moved");
        // No leftover temporary file beside the two archives on the happy path.
        let stray: Vec<String> = std::fs::read_dir(&build)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".incoming"))
            .collect();
        assert!(stray.is_empty(), "leftover temporary file(s): {stray:?}");
    }
}
