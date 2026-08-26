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
//! The swap itself is renames within one filesystem. It is not one atomic
//! operation and cannot be; what makes the gap harmless is that the cache stamp
//! is cleared *first*, so a process killed mid-swap leaves a cache that
//! re-extracts on the next launch rather than an engine paired with the wrong
//! assets. That pairing is the bug [`cache`](crate::cache) exists for and it is
//! not worth reintroducing at the one moment it is easiest to hit.

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
pub fn install(parts: &Parts, progress: Progress<'_>) -> Result<Installed, Failed> {
    install_into(parts, &build_dir(), &engine_dir(), progress)
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
    progress: Progress<'_>,
) -> Result<Installed, Failed> {
    let staging = build.join(STAGING);
    let incoming = engine_into.join(STAGING);
    let result = staged(parts, build, engine_into, &staging, &incoming, progress);
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

    adopt(&fetched, build, engine_into, incoming, progress)
}

/// Take archives that are already on disk and make them the build in use.
///
/// Split out of [`staged`] so that [`crate::provider`] can reach the same
/// sequence with files it obtained and verified itself, rather than through
/// [`Parts`] and a URL. Everything after "the bytes are here" is identical and
/// having two copies of it would be how the two paths drift.
///
/// **A file that is not already in the staging area is copied, not moved.** The
/// local provider hands back the archive at the path it already occupies, which
/// on most machines is Sober's own package directory -- renaming it into
/// Cordial's cache would take Roblox out from underneath Sober and break a
/// working program to install this one. Inside staging a rename is correct and
/// is what happens, because those files were fetched for this.
/// Written beside a build Cordial installed, so it can tell its own work from
/// somebody else's.
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
    progress: Progress<'_>,
) -> Result<Installed, Failed> {
    let _ = progress;

    // **Before anything is unpacked, not after.** Checking once the engine has
    // been extracted would mean refusing after several minutes of work, and the
    // point of the refusal is that the user's file is still there.
    if !ours_to_write(build) {
        return Err(Failed::NotOurs { path: build.display().to_string() });
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
    let mut base = build.join(BASE_APK);
    let mut carrier_live = build.join(carrier_name);
    for (name, path) in fetched {
        let live = build.join(name);
        if path == &live {
            // Already where it belongs. Renaming a file onto itself is not an
            // error but copying one onto itself truncates it, so this case is
            // checked rather than discovered.
        } else if path.starts_with(build) {
            std::fs::rename(path, &live)
                .map_err(|e| Failed::Io { path: live.display().to_string(), why: e.to_string() })?;
        } else {
            std::fs::copy(path, &live)
                .map_err(|e| Failed::Io { path: live.display().to_string(), why: e.to_string() })?;
        }
        if *name == BASE_APK {
            base = live.clone();
        }
        if *name == carrier_name {
            carrier_live = live;
        }
    }


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
            install_into(&parts, &build, &engine_into, &mut silent()).expect("install");

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
        let e = install_into(&parts, &dir.join("build"), &dir.join("lib"), &mut silent())
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
            install_into(&parts, &dir.join("build"), &dir.join("lib"), &mut silent()).unwrap();
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
}
