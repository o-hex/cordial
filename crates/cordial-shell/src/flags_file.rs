//! The one corner of the FastFlag override contract this crate touches.
//!
//! `crates/cordial-runtime/src/flags.rs` owns the real thing: layered
//! overrides with provenance, `~/.config/cordial/flags.json` (or
//! `$CORDIAL_FLAGS`) as the user's own layer, which always wins. This crate
//! does not depend on cordial-runtime — the core shell has to build and run
//! before the engine exists at all, per ADR-002 — so rather than link the
//! runtime crate just to touch one file both already agree on the shape of,
//! this reimplements only the path contract and the single read/write
//! operation the General settings page needs.
//!
//! The renderer preference is the only General-page row backed by this,
//! because it is the only one with somewhere real to land: `flags.json` is
//! read by `cordial-run` regardless of which process wrote it. Window
//! placement and resolution (`CORDIAL_MONITOR`, `CORDIAL_RESOLUTION`,
//! `CORDIAL_FULLSCREEN`, `CORDIAL_WINDOW_POS`) are read from `cordial-run`'s
//! own process environment rather than from a file.
//!
//! *Corrected:* the reason given here used to be that the shell does not
//! launch that process, so there was nothing for such a setting to write to.
//! It does now — see `launch.rs`, which spawns it and sets the environment it
//! runs with — so that argument no longer holds and the honest one is
//! narrower: those four are worth exposing and nobody has, and two of them
//! (`CORDIAL_MONITOR`, `CORDIAL_FULLSCREEN`) are read only by the X11 backend
//! and do nothing on the Wayland one the launcher now asks for. Adding a row
//! for a variable that silently does nothing would be the switch-that-changes-
//! nothing this paragraph warns about, so the warning stands and the reason
//! has moved.

use std::path::{Path, PathBuf};

/// `FStringDebugGraphicsPreferredBackend` is the only flag exposed here, and
/// "Vulkan" is the only value confirmed anywhere in this repository — it is
/// README's own worked example. There is no second confirmed spelling for
/// forcing GLES2: grepped `docs/traces/` and `docs/analysis/`, found nothing.
/// AGENTS.md's rule against conclusions drawn without a trace to back them
/// applies to a value typed into a settings row exactly as much as to
/// something read out of the stripped binary, so this offers "Automatic"
/// (no override — Roblox's own dlopen-Vulkan-then-GLES2-fallback) alongside
/// the confirmed "Vulkan", rather than alongside a guessed GLES2 string that
/// might silently do nothing.
pub const RENDERER_BACKEND_FLAG: &str = "FStringDebugGraphicsPreferredBackend";

pub fn user_flags_path() -> PathBuf {
    std::env::var_os("CORDIAL_FLAGS").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(std::env::temp_dir)
            .join("cordial/flags.json")
    })
}

/// The current value of one string flag. `None` covers "unset", "file
/// absent" and "file doesn't parse" alike — every caller here falls back to
/// the same thing (the UI's own default) regardless of which of the three it
/// was, so there is no reason to distinguish them further up.
pub fn read_string_flag(path: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    match value.as_object()?.get(name)? {
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Set, or with `value: None` clear, one string flag in the user's
/// `flags.json`, preserving every other key already there.
///
/// Refuses to touch a file that exists but is not a JSON object, rather than
/// overwriting it wholesale — the same reasoning as
/// `cordial_runtime::flags::read_layer` skipping a file it cannot parse
/// instead of guessing at it. The difference here is that a write is
/// destructive in a way a skipped read is not, so this returns an error for
/// the caller to surface rather than silently starting from an empty object.
pub fn set_string_flag(path: &Path, name: &str, value: Option<&str>) -> Result<(), String> {
    let mut obj = match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(obj)) => obj,
            Ok(_) => return Err(format!("{} is not a JSON object; leaving it alone", path.display())),
            Err(e) => return Err(format!("{} is not valid JSON ({e}); leaving it alone", path.display())),
        },
        Err(_) => serde_json::Map::new(),
    };

    match value {
        Some(v) => {
            obj.insert(name.to_string(), serde_json::Value::String(v.to_string()));
        }
        None => {
            obj.remove(name);
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(obj)).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("cordial-shell-flags-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn a_missing_file_reads_as_unset() {
        let p = scratch("missing.json");
        let _ = std::fs::remove_file(&p);
        assert_eq!(read_string_flag(&p, RENDERER_BACKEND_FLAG), None);
    }

    #[test]
    fn setting_a_flag_creates_the_file_and_round_trips() {
        let p = scratch("create.json");
        let _ = std::fs::remove_file(&p);
        set_string_flag(&p, RENDERER_BACKEND_FLAG, Some("Vulkan")).unwrap();
        assert_eq!(read_string_flag(&p, RENDERER_BACKEND_FLAG).as_deref(), Some("Vulkan"));
    }

    #[test]
    fn setting_a_flag_preserves_other_keys_already_in_the_file() {
        let p = scratch("preserve.json");
        std::fs::write(&p, r#"{"DFFlagSomethingElse": true}"#).unwrap();
        set_string_flag(&p, RENDERER_BACKEND_FLAG, Some("Vulkan")).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("DFFlagSomethingElse"));
        assert!(text.contains("Vulkan"));
    }

    #[test]
    fn clearing_a_flag_removes_only_that_key() {
        let p = scratch("clear.json");
        std::fs::write(&p, r#"{"FStringDebugGraphicsPreferredBackend": "Vulkan", "DFFlagKeep": true}"#).unwrap();
        set_string_flag(&p, RENDERER_BACKEND_FLAG, None).unwrap();
        assert_eq!(read_string_flag(&p, RENDERER_BACKEND_FLAG), None);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("DFFlagKeep"));
    }

    #[test]
    fn a_file_that_is_not_a_json_object_is_left_alone() {
        let p = scratch("not-object.json");
        std::fs::write(&p, "[1,2,3]").unwrap();
        assert!(set_string_flag(&p, RENDERER_BACKEND_FLAG, Some("Vulkan")).is_err());
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, "[1,2,3]");
    }
}
