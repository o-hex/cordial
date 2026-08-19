//! `AAssetManager` — Roblox's assets, read out of the APK.
//!
//! This is the gate on rendering: the engine loads every shader, font and
//! texture through it, so nothing appears on screen until it works.
//!
//! Roblox uses the whole-buffer half of the API (`AAsset_getBuffer` and
//! `AAsset_getLength`) rather than streaming `AAsset_read`, which makes the
//! implementation much simpler than the general case — an asset is just its
//! decompressed bytes, held until closed.
//!
//! Android serves assets from `assets/` inside the APK. That is a plain zip, and
//! the paths Roblox passes are relative to it.
//!
//! Before the zip is ever consulted, every lookup is checked against the
//! overlay stack — see the `overlay` section below. That is
//! [ADR-010](../../../../docs/adr/ADR-010-plugin-asset-overlays.md): plugins
//! and the user may provide files that resolve in place of the APK's own,
//! without Cordial ever writing into the APK or into anything extracted from
//! it.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// One opened asset. The pointer handed back to Roblox is a `Box::into_raw` of
/// this, which is what an `AAsset*` is as far as the engine is concerned.
struct Asset {
    /// Borrowed from the cache, not copied. Roblox opens the same assets
    /// repeatedly; copying each time would double the memory for no benefit,
    /// since the cached bytes already live for the process.
    bytes: &'static [u8],
}

struct Manager {
    apk: PathBuf,
    /// Decompressed contents, keyed by path inside `assets/`.
    ///
    /// Roblox reopens the same assets repeatedly during startup, and zip
    /// decompression is not free. Caching also means `AAsset_getBuffer` can hand
    /// out a pointer that stays valid after the asset is closed, which the API
    /// does not promise but callers routinely assume.
    cache: Mutex<HashMap<String, &'static [u8]>>,
}

static MANAGER: OnceLock<Manager> = OnceLock::new();

/// Point the asset manager at an APK. Must be called before Roblox asks for
/// anything; `AAssetManager_open` fails cleanly until it is.
pub fn set_apk(path: &Path) -> Result<(), String> {
    // Fail now, with a path in the message, rather than at the first asset.
    File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    MANAGER
        .set(Manager {
            apk: path.to_path_buf(),
            cache: Mutex::new(HashMap::new()),
        })
        .map_err(|_| "an APK is already set".to_string())
}

pub fn is_configured() -> bool {
    MANAGER.get().is_some()
}

/// `CORDIAL_TRACE_ASSETS=1` is the one env var this module's tracing is
/// documented to key off; `CORDIAL_TRACE` is accepted too where it was
/// already wired in, but never required — `CORDIAL_TRACE=1` wraps variadic
/// functions with fixed-arity declarations and aborts the engine
/// (AGENTS.md), so a trace line gated on it alone is not reachable without
/// killing the client. That was true of the miss line below until this
/// change: it checked `CORDIAL_TRACE` only, so a miss — the empty-string
/// signature this investigation is chasing — could never actually be
/// observed. See docs/analysis/flag-init.md §34, "What is left after
/// twenty-four".
fn trace_assets_enabled() -> bool {
    std::env::var_os("CORDIAL_TRACE_ASSETS").is_some() || std::env::var_os("CORDIAL_TRACE").is_some()
}

impl Manager {
    /// Read `assets/<name>`, consulting the overlay stack before the APK, and
    /// caching whichever bytes win. Every outcome — cache, overlay, APK, or
    /// miss — is traced under `CORDIAL_TRACE_ASSETS=1` so a miss is visible,
    /// not just a hit.
    fn read(&self, name: &str) -> Option<&'static [u8]> {
        if let Some(bytes) = self.cache.lock().ok()?.get(name) {
            if trace_assets_enabled() {
                eprintln!("[asset] hit (cache): {name}");
            }
            return Some(bytes);
        }

        if let Some((source, path)) = resolve_overlay(name) {
            // A file that resolved a moment ago and is gone now (removed
            // mid-registration-change, races with `unregister_plugin_root`) is
            // not a reason to fail a lookup that has a perfectly good
            // fallback — fall through to the APK exactly as if the overlay
            // had never had it.
            if let Ok(bytes) = std::fs::read(&path) {
                // This line was the only way to observe that an overlay had
                // applied, and it was gated on a flag AGENTS.md tells people
                // never to set: `CORDIAL_TRACE=1` wraps variadic functions with
                // fixed-arity declarations and aborts the engine. So the one
                // diagnostic for ADR-010 could not be reached without killing
                // the client, which means nobody could confirm an overlay was
                // working except by looking at the screen.
                if trace_assets_enabled() {
                    eprintln!("[asset] hit (overlay): {name} from {}", source.describe());
                }
                let leaked: &'static [u8] = Vec::leak(bytes);
                self.cache.lock().ok()?.insert(name.to_string(), leaked);
                return Some(leaked);
            }
        }

        let Some(bytes) = (|| -> Option<Vec<u8>> {
            let file = File::open(&self.apk).ok()?;
            let mut zip = zip::ZipArchive::new(file).ok()?;
            let mut entry = zip.by_name(&format!("assets/{name}")).ok()?;
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes).ok()?;
            Some(bytes)
        })() else {
            if trace_assets_enabled() {
                eprintln!("[asset] miss: {name}");
            }
            return None;
        };

        if trace_assets_enabled() {
            eprintln!("[asset] hit (apk): {name}");
        }

        // Deliberately leaked. Asset lifetimes in this API are the caller's to
        // manage and Roblox is not careful about them; an asset outliving its
        // AAsset handle is far cheaper than a dangling buffer, and the total is
        // bounded by what the APK contains.
        let leaked: &'static [u8] = Vec::leak(bytes);
        self.cache.lock().ok()?.insert(name.to_string(), leaked);
        Some(leaked)
    }
}

// ---------------------------------------------------------------- the overlay
//
// ADR-010: a plugin, or the user directly, may provide a file that resolves in
// place of the APK's own for the same name. The whole mechanism is a lookup
// interposed ahead of the zip read above — nothing is ever copied into the
// APK, nothing is ever copied into `extract_to`'s output, and removing a root
// is exactly as simple as no longer consulting it. There is no cleanup step
// because there was never a write to undo.
//
// One thing this does *not* attempt: if an asset is already cached (served
// once, from either the overlay or the APK), it stays cached for the rest of
// the process — the same "cached forever" contract `Manager::read` already
// gives the APK path, because Roblox is handed a pointer that has to remain
// valid. Changing the overlay stack after an asset has already been served
// does not retroactively change what a held pointer points at. In practice
// this does not bite: plugins register their roots at startup, before Roblox
// asks for anything.

/// Where an overlaid file came from. Mirrors [`crate::flags::Source`] — naming
/// the origin is what makes "why did this asset change" answerable after the
/// fact, and what lets removing one root put back exactly what it replaced and
/// nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlaySource {
    /// The user's own overlay root — see [`user_root`].
    User,
    /// A plugin's overlay root, named by the plugin's id.
    Plugin(String),
}

impl OverlaySource {
    pub fn describe(&self) -> String {
        match self {
            OverlaySource::User => "user".into(),
            OverlaySource::Plugin(id) => format!("plugin:{id}"),
        }
    }
}

/// One overlay root and who owns it: a directory that mirrors the APK's
/// `assets/` layout.
#[derive(Debug, Clone)]
struct OverlayLayer {
    source: OverlaySource,
    root: PathBuf,
}

/// Every plugin-registered overlay root, in registration order. The user's
/// root is not kept here — it has nowhere to be "registered" from, it is
/// always whatever `user_root` resolves to — so this only ever holds plugins.
static PLUGIN_OVERLAYS: Mutex<Vec<OverlayLayer>> = Mutex::new(Vec::new());

/// The user's own overlay root: `$XDG_CONFIG_HOME/cordial/overlay`, falling
/// back to `$HOME/.config/cordial/overlay`, overridable with `CORDIAL_OVERLAY`.
/// Mirrors the APK's `assets/` layout exactly, the same shape Sober's
/// `asset_overlay` uses.
///
/// Not required to exist. A missing directory simply never matches anything —
/// see `safe_join`, which fails closed rather than needing a separate
/// existence check here.
pub fn user_root() -> PathBuf {
    std::env::var_os("CORDIAL_OVERLAY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .unwrap_or_else(std::env::temp_dir)
                .join("cordial/overlay")
        })
}

/// Register (or re-register) a plugin's overlay root.
///
/// Re-registering an id already on the stack moves it to the end rather than
/// leaving a stale entry in its old position, so "last registered wins" also
/// covers a plugin that changes its own root mid-session rather than only
/// covering the order plugins first started in.
pub fn register_plugin_root(id: &str, root: PathBuf) {
    let source = OverlaySource::Plugin(id.to_string());
    if let Ok(mut stack) = PLUGIN_OVERLAYS.lock() {
        stack.retain(|l| l.source != source);
        stack.push(OverlayLayer { source, root });
    }
}

/// Remove a plugin's overlay root. Everything it was serving falls straight
/// back to whatever would have resolved without it — a lower-priority overlay,
/// or the APK — because nothing was ever written into either to begin with.
pub fn unregister_plugin_root(id: &str) {
    let source = OverlaySource::Plugin(id.to_string());
    if let Ok(mut stack) = PLUGIN_OVERLAYS.lock() {
        stack.retain(|l| l.source != source);
    }
}

/// Join `name` onto `root`, refusing anything that would resolve outside it.
///
/// `name` is not written by Cordial — for the overlay it is ultimately
/// whatever path Roblox asked `AAssetManager` for — so it gets the same
/// treatment as a zip entry in `extract_to`: a `..` or absolute component is
/// rejected outright, and a symlink that resolves outside `root` is rejected
/// even when the name itself looks clean, by canonicalising and checking the
/// real path is still a descendant. A name that is rejected and a name that
/// simply is not present both return `None`, because from the caller's point
/// of view they are the same instruction: keep looking elsewhere.
fn safe_join(root: &Path, name: &str) -> Option<PathBuf> {
    let rel = Path::new(name);
    if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let candidate = root.join(rel).canonicalize().ok()?;
    candidate.starts_with(&root).then_some(candidate)
}

/// Resolve `name` against a stack of overlay layers, in precedence order
/// **lowest first** — a later layer in the slice overrides an earlier one for
/// the same name, exactly like [`crate::flags::resolve`]. Callers build the
/// slice so plugins come first (in registration order, so the last-registered
/// plugin is last in the slice and therefore wins among plugins) and the user
/// comes last (so the user beats every plugin).
///
/// Pure and side-effect-free: it only reads `root`'s filesystem entries to
/// decide what exists, which is what makes it directly testable without
/// touching the global stack or the process's real overlay directories.
fn resolve_stack(layers: &[OverlayLayer], name: &str) -> Option<(OverlaySource, PathBuf)> {
    let mut found = None;
    for layer in layers {
        if let Some(path) = safe_join(&layer.root, name) {
            if path.is_file() {
                found = Some((layer.source.clone(), path));
            }
        }
    }
    found
}

/// The live resolution `Manager::read` actually uses: the registered plugin
/// stack, with the user's root appended last so it always wins.
fn resolve_overlay(name: &str) -> Option<(OverlaySource, PathBuf)> {
    let mut layers = PLUGIN_OVERLAYS.lock().ok()?.clone();
    layers.push(OverlayLayer { source: OverlaySource::User, root: user_root() });
    resolve_stack(&layers, name)
}

/// Which layer currently serves `name` — `"user"`, `"plugin:<id>"`, or `None`
/// meaning the APK itself. Diagnostic only; it recomputes the answer fresh each
/// call rather than remembering what was actually served, so it can disagree
/// with an asset that was cached before the stack last changed. That is the
/// same page the running process has been serving since, which is the
/// question worth answering here.
pub fn explain(name: &str) -> Option<String> {
    resolve_overlay(name).map(|(source, _)| source.describe())
}

/// Extract `assets/` to a real directory and return its path.
///
/// Not everything in the engine reads assets through `AAssetManager`. Its HTTP
/// stack is curl, and curl's `CURLOPT_CAINFO` takes a **filesystem path** — it
/// cannot be handed a zip entry or a pointer. Roblox ships its CA bundle as
/// `assets/ssl/cacert.pem` precisely because the Android app extracts assets to
/// a real directory and gives the engine that directory.
///
/// Cordial was passing the `.apk` file itself as `assetFolderPath`, so every
/// path the engine built from it (`<assetFolder>/ssl/cacert.pem`) named a file
/// inside a file and could not be opened. TLS verification then has no roots,
/// which fails the client-settings fetch, which fails the flag set — three
/// layers away from anything that mentions certificates.
///
/// Extraction is skipped when the destination is already populated, so repeat
/// launches pay for it once.
pub fn extract_to(dir: &Path) -> Result<PathBuf, String> {
    let manager = MANAGER.get().ok_or("no APK is set")?;
    let file = File::open(&manager.apk).map_err(|e| format!("{}: {e}", manager.apk.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            // `enclosed_name` is None for entries that escape the destination
            // (`../`, absolute paths). Skipping them is the whole defence
            // against a zip-slip write outside `dir`.
            continue;
        };
        let Ok(rel) = name.strip_prefix("assets") else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let out = dir.join(rel);
        if out.exists() {
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        std::fs::write(&out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(dir.to_path_buf())
}

/// SAFETY: `p` is null or a NUL-terminated C string, per the API contract.
unsafe fn cstr(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

// ------------------------------------------------------------------- the API

extern "C" fn asset_manager_from_java(_env: *mut c_void, _obj: *mut c_void) -> *mut c_void {
    super::trace(format_args!("AAssetManager_fromJava"));
    // There is one asset manager per process and the Java object is Cordial's
    // own, so there is nothing to look up. Returning a non-null token matters:
    // Roblox checks it.
    MANAGER.get().map_or(std::ptr::null_mut(), |m| m as *const Manager as *mut c_void)
}

extern "C" fn asset_manager_open(
    _mgr: *mut c_void,
    filename: *const c_char,
    _mode: c_int,
) -> *mut c_void {
    let Some(manager) = MANAGER.get() else {
        eprintln!("[asset] open before an APK was set — pass --apk");
        return std::ptr::null_mut();
    };
    // SAFETY: the API contract is a NUL-terminated path.
    let Some(name) = (unsafe { cstr(filename) }) else {
        return std::ptr::null_mut();
    };

    match manager.read(&name) {
        Some(bytes) => Box::into_raw(Box::new(Asset { bytes })) as *mut c_void,
        // Not an error worth failing on: Android's asset manager returns null
        // for a missing asset and callers probe for optional files. `read`
        // already traced the miss (under `CORDIAL_TRACE_ASSETS=1`) with the
        // name; nothing more to add here.
        None => std::ptr::null_mut(),
    }
}

extern "C" fn asset_get_buffer(asset: *mut c_void) -> *const c_void {
    if asset.is_null() {
        return std::ptr::null();
    }
    // SAFETY: `asset` came from asset_manager_open and has not been closed.
    let a = unsafe { &*(asset as *const Asset) };
    a.bytes.as_ptr() as *const c_void
}

extern "C" fn asset_get_length(asset: *mut c_void) -> i64 {
    if asset.is_null() {
        return 0;
    }
    // SAFETY: as above.
    let a = unsafe { &*(asset as *const Asset) };
    a.bytes.len() as i64
}

extern "C" fn asset_close(asset: *mut c_void) {
    if asset.is_null() {
        return;
    }
    // SAFETY: `asset` came from Box::into_raw in asset_manager_open and is closed
    // exactly once, per the API contract.
    drop(unsafe { Box::from_raw(asset as *mut Asset) });
}

/// Some callers want a file descriptor rather than a buffer — Android can give
/// one because assets are stored uncompressed at a known offset in the APK.
/// Cordial's are decompressed in memory, so this hands back a sealed `memfd`
/// holding the same bytes, which behaves the same for reading.
extern "C" fn asset_open_file_descriptor(
    asset: *mut c_void,
    out_start: *mut i64,
    out_length: *mut i64,
) -> c_int {
    if asset.is_null() {
        return -1;
    }
    // SAFETY: `asset` came from asset_manager_open.
    let a = unsafe { &*(asset as *const Asset) };

    extern "C" {
        fn memfd_create(name: *const c_char, flags: u32) -> c_int;
        fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
        fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
    }

    // SAFETY: a literal name and no flags; the fd is returned to the caller.
    let fd = unsafe { memfd_create(c"cordial-asset".as_ptr(), 0) };
    if fd < 0 {
        return -1;
    }
    // SAFETY: writing exactly the bytes we own to a fresh fd.
    let written = unsafe { write(fd, a.bytes.as_ptr() as *const c_void, a.bytes.len()) };
    if written < 0 || written as usize != a.bytes.len() {
        return -1;
    }
    // SAFETY: rewinding the fd we just wrote.
    unsafe { lseek(fd, 0, 0) };

    if !out_start.is_null() {
        // SAFETY: caller-provided out-parameter.
        unsafe { *out_start = 0 };
    }
    if !out_length.is_null() {
        // SAFETY: caller-provided out-parameter.
        unsafe { *out_length = a.bytes.len() as i64 };
    }
    fd
}

/// Everything this module provides, for the symbol table.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        f!("AAssetManager_fromJava", asset_manager_from_java),
        f!("AAssetManager_open", asset_manager_open),
        f!("AAsset_getBuffer", asset_get_buffer),
        f!("AAsset_getLength", asset_get_length),
        f!("AAsset_close", asset_close),
        f!("AAsset_openFileDescriptor", asset_open_file_descriptor),
    ]
}

/// Open an asset through the same entry points Roblox uses and report its size.
///
/// Exercising the C API rather than `Manager::read` is the point: it is the
/// pointer handling either side of the FFI boundary that is worth proving, not
/// the zip lookup.
pub fn probe(name: &str) -> Result<usize, String> {
    let c = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
    let mgr = asset_manager_from_java(std::ptr::null_mut(), std::ptr::null_mut());
    if mgr.is_null() {
        return Err("no APK configured".into());
    }
    let asset = asset_manager_open(mgr, c.as_ptr(), 0);
    if asset.is_null() {
        return Err(format!("asset not found: {name}"));
    }
    let len = asset_get_length(asset) as usize;
    let buf = asset_get_buffer(asset);
    if buf.is_null() {
        asset_close(asset);
        return Err("asset opened but its buffer is null".into());
    }
    // Touch the first and last byte: a length without readable memory behind it
    // is exactly the failure this probe exists to catch.
    // SAFETY: `buf` points at `len` bytes owned by the asset, still open.
    let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, len) };
    let checksum = bytes.first().copied().unwrap_or(0) as usize
        + bytes.last().copied().unwrap_or(0) as usize;
    std::hint::black_box(checksum);
    asset_close(asset);
    Ok(len)
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    /// Write `contents` at `dir/rel`, creating whatever directories are needed
    /// to mirror the APK's own tree structure.
    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn layer(source: OverlaySource, root: &Path) -> OverlayLayer {
        OverlayLayer { source, root: root.to_path_buf() }
    }

    #[test]
    fn the_user_root_beats_a_plugin_root() {
        // The user always wins over anything they installed to do something
        // else — the same rule flags.rs already enforces for FastFlags, and
        // for the same reason: a plugin quietly overriding a choice the user
        // made on purpose would make "the user's own overlay" a polite
        // fiction rather than a real one.
        let dir = std::env::temp_dir().join("cordial-overlay-test-user-beats-plugin");
        let plugin_dir = dir.join("plugin");
        let user_dir = dir.join("user");
        write(&plugin_dir, "textures/wood.png", b"plugin-version");
        write(&user_dir, "textures/wood.png", b"user-version");

        let layers = vec![
            layer(OverlaySource::Plugin("themer".into()), &plugin_dir),
            layer(OverlaySource::User, &user_dir),
        ];
        let (source, path) = resolve_stack(&layers, "textures/wood.png").unwrap();
        assert_eq!(source, OverlaySource::User);
        assert_eq!(std::fs::read(path).unwrap(), b"user-version");
    }

    #[test]
    fn the_last_registered_plugin_wins() {
        // Two plugins wanting the same asset is a real disagreement between
        // them, and resolving it by directory-iteration order would make the
        // outcome depend on something nobody chose. Registration order is at
        // least a fact — deterministic, and one a user can be told.
        let dir = std::env::temp_dir().join("cordial-overlay-test-last-plugin-wins");
        let a = dir.join("a");
        let b = dir.join("b");
        write(&a, "sounds/click.ogg", b"a");
        write(&b, "sounds/click.ogg", b"b");

        // Registration order is a-then-b, so b — the later registration — is
        // later in the slice and wins, per resolve_stack's contract.
        let layers = vec![
            layer(OverlaySource::Plugin("a".into()), &a),
            layer(OverlaySource::Plugin("b".into()), &b),
        ];
        let (source, path) = resolve_stack(&layers, "sounds/click.ogg").unwrap();
        assert_eq!(source, OverlaySource::Plugin("b".into()));
        assert_eq!(std::fs::read(path).unwrap(), b"b");
    }

    #[test]
    fn removing_a_root_restores_the_original_with_no_cleanup_step() {
        // This is the whole point of an overlay that only ever reads:
        // uninstalling a plugin is "stop consulting its directory", full
        // stop. There is nothing on disk to undo, because the asset the
        // overlay was standing in for was never touched.
        let dir = std::env::temp_dir().join("cordial-overlay-test-removal");
        write(&dir, "fonts/custom.ttf", b"overlay-font");

        register_plugin_root("overlay-removal-test", dir.clone());
        assert_eq!(
            resolve_overlay("fonts/custom.ttf").map(|(s, _)| s),
            Some(OverlaySource::Plugin("overlay-removal-test".into()))
        );

        unregister_plugin_root("overlay-removal-test");
        // Nothing else provides fonts/custom.ttf, so this is exactly what a
        // real lookup would see: no overlay hit, fall through to the APK.
        assert!(resolve_overlay("fonts/custom.ttf").is_none());
    }

    #[test]
    fn an_escape_attempt_is_rejected() {
        // The name being resolved did not come from Cordial — for the overlay
        // it is ultimately whatever path Roblox asked for. Trusting it would
        // turn an overlay root into a way to read anything the process can
        // read, which is a much bigger grant than "a directory of assets".
        let dir = std::env::temp_dir().join("cordial-overlay-test-escape");
        write(&dir, "marker", b"present");

        assert!(safe_join(&dir, "../escape").is_none());
        assert!(safe_join(&dir, "/etc/passwd").is_none());
        assert!(safe_join(&dir, "a/../../escape").is_none());
        // The defence is about leaving the root, not about refusing
        // everything — an ordinary name still has to resolve normally.
        assert!(safe_join(&dir, "marker").is_some());
    }

    #[test]
    fn a_symlink_leaving_the_root_is_rejected() {
        // A name with no ".." in it can still escape if a path component is a
        // symlink — the component check alone would miss this entirely, which
        // is why safe_join also canonicalises and checks the real path stayed
        // under the root, the same rigour extract_to's zip-slip defence uses.
        let dir = std::env::temp_dir().join("cordial-overlay-test-symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let outside = std::env::temp_dir().join("cordial-overlay-test-symlink-target");
        std::fs::write(&outside, b"outside").unwrap();
        let link = dir.join("escape.png");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        assert!(safe_join(&dir, "escape.png").is_none());
    }

    #[test]
    fn a_missing_overlay_file_falls_through_to_the_apk() {
        // Most assets are not overlaid. The common case has to behave as
        // "nothing here, keep looking" rather than "nothing here, fail the
        // lookup" — resolve_stack returning None is exactly what lets
        // Manager::read carry on to the zip.
        let dir = std::env::temp_dir().join("cordial-overlay-test-miss");
        write(&dir, "textures/other.png", b"unrelated");

        let layers = vec![layer(OverlaySource::Plugin("x".into()), &dir)];
        assert!(resolve_stack(&layers, "textures/wood.png").is_none());
    }
}
