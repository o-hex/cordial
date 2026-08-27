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
//! ## One font, and games use more than one
//!
//! **Builder Sans is a default and a fallback, not an answer.** It is Roblox's
//! own UI font, so it is right for the login screen, for settings, and for any
//! box a game did not restyle. It is wrong the moment a game sets a TextBox's
//! `FontFace` to something else, and then this module reproduces the bug it
//! exists to fix -- the characters change shape under the cursor -- with a
//! different wrong font instead of the desktop's UI one.
//!
//! That matters more here than it would elsewhere, because the editor is what
//! the player actually sees. The engine stops drawing the box's own text while
//! it is focused and resumes on blur, so during editing these glyphs are the
//! visible text rather than an overlay on top of it.
//!
//! **The engine does name the font, and the mapping ships in the APK.**
//! `com.roblox.engine.jni.model.NativeTextBoxInfo` -- the styling spec handed
//! to `showKeyboard` -- declares an `int` field called exactly `font`, read out
//! of `classes2.dex` on 2026-08-27. And `assets/android/fonts/font-mappings.json`
//! in the same APK is a 48-entry table from that integer to a font file, with
//! `46` naming `BuilderSans-Regular.otf`; `assets/content/fonts/families/*.json`
//! then gives the family string Pango wants. So a per-box font needs no
//! hand-maintained table that would rot across Roblox builds -- the
//! authoritative one is already on disk, in the archive the user supplied, and
//! can be read the same way this module already reads the OTF.
//!
//! **What blocks it is which constructor slot carries the id.**
//! `native/android_classes.cpp` guesses slot 9, and that guess is weak in a
//! specific way: it rests on one capture of two Login-screen boxes where slot 9
//! read 46 on both and never varied. Slot 6 read 0 on both, and
//! `Enum.Font.Legacy` is 0 -- a real font value -- so slot 6 fits the evidence
//! exactly as well. Only `textColor` is genuinely pinned, by a packed ARGB
//! value nothing else could be.
//!
//! The experiment is one capture, and it needs a person rather than a change:
//! `CORDIAL_TRACE_TEXT=1`, focus a TextBox in a game that restyled its font,
//! and see which of slots 6, 7, 9, 10 and 11 moves. Whichever varies with the
//! visible glyphs is the field. Every capture this project holds was taken on
//! the login screen, which is exactly why one observation could not separate
//! them.
//!
//! Reading `TextBox.FontFace` out of the DataModel would answer it directly and
//! is permanently out of scope: that is in-process introspection of the engine,
//! which ADR-001 and ADR-003 rule out. Disassembling the constructor's `iput`
//! order would also answer it, and is declined on the licence line in
//! AGENTS.md -- declared shapes and call order are observation, the body of a
//! method is how it implements something.
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
