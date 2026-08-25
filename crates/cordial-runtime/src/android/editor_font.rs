//! Draw the text editor in Roblox's own font.
//!
//! The editor is a `gtk::Text` placed on top of a box the engine drew, so the
//! moment it appears the string is re-rendered by a different text stack. Get
//! the family wrong and the characters change shape and weight under the
//! user's cursor even though the size is right -- reported as "the text shifts
//! a lot when you select the text box, like gtk doesnt match the text size as
//! good". Matching the size was never going to be enough; Pango was drawing
//! the desktop's UI font against the engine's own.
//!
//! Roblox ships that font in the APK: `assets/content/fonts/BuilderSans-Regular.otf`,
//! which fontconfig reports as family "Builder Sans". Registering it with the
//! process's fontconfig makes it available to Pango by name, and then the
//! editor and the engine are drawing the same glyphs.
//!
//! **Nothing here vendors a Roblox asset and nothing may.** AGENTS.md rules
//! out committing an APK or anything out of one. This reads the font from the
//! APK the *user* supplied, which is the same posture as every other asset:
//! Cordial ships none and works from the copy on the machine. The extracted
//! file lands in the cache beside the extracted libraries, and is rewritten
//! only when its size differs, so a launch does not rewrite it every time.
//!
//! `FcConfigAppFontAddFile` is reached through `dlsym` rather than linked.
//! fontconfig is already in the process -- GTK cannot render without it -- and
//! dlsym keeps it from becoming a build-time dependency of this crate for one
//! call. The same pattern `vulkan.rs` uses for the loader.

use std::ffi::{c_char, c_int, c_void, CString};
use std::path::PathBuf;

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// What fontconfig calls it, and therefore what Pango wants. Confirmed with
/// `fc-query` against the shipped file rather than assumed from the filename:
/// the file is `BuilderSans-Regular.otf` and the family is `Builder Sans`.
pub const FAMILY: &str = "Builder Sans";

const ASSET: &str = "content/fonts/BuilderSans-Regular.otf";

/// Register Roblox's UI font with the process, returning the family name for
/// the editor to ask for.
///
/// `None` on any failure, and every caller must treat that as "use whatever
/// Pango would have used". A missing font is a cosmetic mismatch; refusing to
/// draw an editor over it would make typing invisible, which is the bug this
/// whole path exists to fix.
pub fn install() -> Option<&'static str> {
    let bytes = super::asset::read_asset(ASSET)?;
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    // Size is a weak comparison and deliberately so: this is a versioned
    // asset out of a signed APK, not something a user edits, and hashing
    // 77KB on every launch to catch a same-size replacement is not worth the
    // startup cost.
    let stale = std::fs::metadata(&path).map(|m| m.len() != bytes.len() as u64).unwrap_or(true);
    if stale {
        std::fs::write(&path, bytes).ok()?;
    }

    let c_path = CString::new(path.to_str()?).ok()?;
    // SAFETY: RTLD_DEFAULT is NULL on glibc; the name is a NUL-terminated
    // literal. A null result means fontconfig is not in the process, which is
    // handled rather than assumed away.
    let sym = unsafe { dlsym(std::ptr::null_mut(), c"FcConfigAppFontAddFile".as_ptr()) };
    if sym.is_null() {
        return None;
    }
    type AddFile = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
    // SAFETY: the signature matches fontconfig's public declaration, and a
    // null config means "the current configuration", which is what GTK built.
    let add: AddFile = unsafe { std::mem::transmute::<*mut c_void, AddFile>(sym) };
    let added = unsafe { add(std::ptr::null_mut(), c_path.as_ptr()) };
    if added == 0 {
        return None;
    }
    println!("[android] editor font: registered {FAMILY} from the APK");
    Some(FAMILY)
}

/// Beside the extracted libraries, for the same reason they are there: it came
/// out of the APK and is reproducible from it, so it belongs in a cache the
/// user can delete rather than in their data directory.
fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("cordial").join("fonts").join("BuilderSans-Regular.otf"))
}
