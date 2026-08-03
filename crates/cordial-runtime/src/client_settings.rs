//! Roblox's client settings — the FastFlag set the engine runs on.
//!
//! On Android the *application* fetches this document and hands it to the engine
//! through `NativeGLInterface.nativeInitClientSettings`. The engine never
//! fetches it itself: breakpoints on `getaddrinfo`, `connect` and `SSL_connect`
//! are never hit during startup, so there is no request of the engine's own to
//! fix or proxy. Cordial is the host application here, so doing the fetch is the
//! job, not a workaround.
//!
//! ## The call contract, established by experiment
//!
//! `nativeInitClientSettings(String, String, String)` returns an `int`. Feeding
//! it known-good and known-bad documents settles what it means:
//!
//! (An earlier version of this note called that return value "the only
//! trustworthy signal, since the engine's own `FLog` output is not routed
//! anywhere visible in this build". The second half was wrong. `FLog` is routed,
//! and always has been — the engine writes it to `appData/logs/*.log`, relative
//! to the working directory. Read that file; it is the best diagnostic Cordial
//! has.)
//!
//! | first argument | result |
//! |---|---|
//! | the real document, `{"applicationSettings": {...}}` | `0` |
//! | `{"applicationSettings":{"FFlagNotARealFlag":"True"}}` | `0` |
//! | `this is not json at all` | `1` |
//! | `{}` — valid JSON, no `applicationSettings` key | `1` |
//!
//! So **`0` is success**, the document goes in the *first* argument, and the
//! `applicationSettings` wrapper must be kept rather than unwrapped. Passing the
//! document in either of the other two positions returns `1` regardless of
//! whether it is valid, which is what "this argument is not the settings" looks
//! like. One of those two is an overrides document — the engine has a
//! `ParseFailure on overrides` log string — but which is still unestablished, so
//! both are left empty rather than guessed at.
//!
//! Those first readings were confounded: `--client-settings` fed *two* call
//! sites, so `nativeInitializeNativeFlags` was receiving the document too and
//! the result could not be attributed to this call alone. On the automatic path
//! the flag-names call gets nothing, and the discriminator survives cleanly —
//! empty string gives `1`, the real document gives `0`. The conclusion held, but
//! it had not actually been shown until the two were separated.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Roblox's settings CDN. The application name is `AndroidApp`; it is not a
/// guess — `AndroidClient`, `AndroidPlayer`, `AndroidClientSettings` and
/// `AndroidAppSettings` all return HTTP 400 "The application name is invalid",
/// and `AndroidApp` returns the real 1.2 MB document.
const URL: &str = "https://clientsettingscdn.roblox.com/v2/settings/application/AndroidApp";

/// How long a cached copy is used before refetching.
///
/// Roblox changes flags continuously, so this is not "cache forever". It is long
/// enough that ordinary repeat launches do not each hit the network, and short
/// enough that a machine left running picks up changes within a session or two.
const MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

fn cache_path() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/clientsettings.json")
}

fn fresh(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let age = SystemTime::now().duration_since(meta.modified().ok()?).ok()?;
    (age < MAX_AGE).then(|| std::fs::read_to_string(path).ok())?
}

/// Looks like a settings document rather than an error page.
///
/// Worth checking before caching: the CDN answers a bad application name with a
/// perfectly well-formed JSON error body, and caching that would produce six
/// hours of failures that look like a flag problem rather than a fetch problem.
fn plausible(body: &str) -> bool {
    body.contains("\"applicationSettings\"")
}

/// The client settings document, from cache when it is fresh and from Roblox
/// otherwise.
///
/// Returns `None` rather than failing the launch: the engine is given whatever
/// it can be given, and a client that starts without flags is more useful than
/// one that refuses to start because a CDN was unreachable.
pub fn load(explicit: Option<&str>) -> Option<String> {
    load_base(explicit).map(apply_overrides)
}

fn load_base(explicit: Option<&str>) -> Option<String> {
    if let Some(path) = explicit {
        return std::fs::read_to_string(path).ok();
    }
    let cache = cache_path();
    if let Some(body) = fresh(&cache) {
        return Some(body);
    }
    match fetch() {
        Some(body) => {
            if let Some(parent) = cache.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&cache, &body);
            Some(body)
        }
        // A stale copy beats nothing when the network is down.
        None => std::fs::read_to_string(&cache).ok(),
    }
}

/// Keys in the flag layers that belong to Cordial rather than to Roblox.
///
/// One prefix rather than a list, so a second Cordial-owned key does not need
/// this file edited to stay out of the engine's settings.
const CORDIAL_KEY_PREFIX: &str = "Cordial";

/// Whether a resolved key is Roblox's to receive.
fn is_roblox_flag(key: &str) -> bool {
    !key.starts_with(CORDIAL_KEY_PREFIX)
}

/// Merge every layer of flag overrides into the settings document.
///
/// The layering, precedence and provenance live in [`crate::flags`]; this only
/// applies the result. Splitting them is what lets a plugin contribute flags
/// without writing into the user's file.
///
/// This is the mechanism that demonstrably works. Verified with a control:
/// `DFFlagRbxTransportUseRtcioRna=false` removes
/// `Initialized RtcIoRna with 1 event loop threads` from the engine's own log,
/// and the same run without it has that line. `nativePreloadFlagOverrides` is
/// *not* the mechanism despite the name — it was tried with several document
/// shapes and changed nothing observable.
fn apply_overrides(doc: String) -> String {
    let resolved = crate::flags::resolve(crate::flags::collect());
    if resolved.is_empty() {
        return doc;
    }
    // Cordial's own keys ride the flag layering for its precedence and
    // provenance, and are not Roblox flags — `CordialGraphicsBackend` asks
    // Cordial whether to offer the engine a Vulkan loader, which is a question
    // the engine has no idea it is being asked. Handing them over would put
    // invented names in Roblox's settings document; the engine ignores what it
    // does not know, but a flag it silently ignores is exactly the thing this
    // project keeps mistaking for a flag that works.
    let overrides: serde_json::Map<String, serde_json::Value> = resolved
        .iter()
        .filter(|(k, _)| is_roblox_flag(k))
        .map(|(k, r)| (k.clone(), serde_json::Value::String(r.value.clone())))
        .collect();

    match merge(&doc, overrides) {
        Ok((merged, _)) => {
            crate::flags::report(&resolved);
            merged
        }
        Err(why) => {
            println!("  flags: {why}; ignoring overrides");
            doc
        }
    }
}

/// Merge overrides into `applicationSettings`, returning the document and how
/// many were applied. Split out from layer resolution so it can be tested.
fn merge(
    doc: &str,
    overrides: serde_json::Map<String, serde_json::Value>,
) -> Result<(String, usize), &'static str> {
    let mut parsed: serde_json::Value =
        serde_json::from_str(doc).map_err(|_| "the settings document did not parse")?;
    let app = parsed
        .get_mut("applicationSettings")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("no applicationSettings object")?;

    let mut applied = 0usize;
    for (k, v) in overrides {
        let as_string = match v {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        };
        app.insert(k, serde_json::Value::String(as_string));
        applied += 1;
    }
    let out = serde_json::to_string(&parsed).map_err(|_| "the merged document did not serialise")?;
    Ok((out, applied))
}

fn fetch() -> Option<String> {
    let body = ureq::get(URL)
        .call()
        .ok()?
        .body_mut()
        // The document is ~1.2 MB and ureq's default read limit is smaller, so
        // an unset limit here is the difference between the real settings and a
        // silent truncation that would parse as valid JSON.
        .read_to_string()
        .ok()?;
    plausible(&body).then_some(body)
}

#[cfg(test)]
mod tests {

    #[test]
    fn cordial_owned_keys_do_not_reach_roblox_settings() {
        // `CordialGraphicsBackend` asks Cordial whether to offer the engine a
        // Vulkan loader, which is a question the engine has no idea it is being
        // asked. It rides the flag layering for that machinery's precedence and
        // provenance, not because it is a FastFlag.
        assert!(!is_roblox_flag("CordialGraphicsBackend"));
        assert!(!is_roblox_flag(crate::graphics::KEY));
        // Roblox's own prefixes are untouched. `DFFlag...` matters most: it is
        // the one this file has a measured control for.
        for key in ["DFFlagRbxTransportUseRtcioRna", "FFlagDebugDisplayFPS", "FStringTest"] {
            assert!(is_roblox_flag(key), "{key}");
        }
    }
    use super::*;

    fn map(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn an_override_replaces_an_existing_flag() {
        let doc = r#"{"applicationSettings":{"DFFlagX":"True","FFlagY":"False"}}"#;
        let (out, n) = merge(doc, map(&[("DFFlagX", serde_json::json!(false))])).unwrap();
        assert_eq!(n, 1);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["applicationSettings"]["DFFlagX"], "false");
        // untouched flags survive
        assert_eq!(v["applicationSettings"]["FFlagY"], "False");
    }

    #[test]
    fn non_string_values_are_converted_rather_than_rejected() {
        // Roblox stores every value as a string, so a config file written with
        // a bare `7` or `true` has to work rather than be a silent no-op.
        let doc = r#"{"applicationSettings":{}}"#;
        let (out, _) = merge(
            doc,
            map(&[("FIntA", serde_json::json!(7)), ("FFlagB", serde_json::json!(true))]),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["applicationSettings"]["FIntA"], "7");
        assert_eq!(v["applicationSettings"]["FFlagB"], "true");
    }

    #[test]
    fn a_document_without_application_settings_is_refused() {
        assert!(merge(r#"{"nope":{}}"#, map(&[("FFlagX", serde_json::json!(1))])).is_err());
    }

    #[test]
    fn an_error_body_is_not_mistaken_for_settings() {
        // What the CDN actually returns for a bad application name. It is valid
        // JSON, so only the shape check distinguishes it.
        let err = r#"{"errors":[{"code":1,"message":"The application name is invalid."}]}"#;
        assert!(!plausible(err));
        assert!(plausible(r#"{"applicationSettings":{"FFlagX":"True"}}"#));
    }

    /// Exercises `load_base` rather than `load` on purpose. `load` merges the
    /// user's own overrides — since ADR-013 that is `<profile>/flags.json`, and
    /// before it `~/.config/cordial/flags.json` — so going through it would
    /// make this test read the developer's real profile and fail for anyone who
    /// has overrides, which is exactly what it did once one existed. The
    /// behaviour under test is path-versus-network, and that is `load_base`.
    #[test]
    fn an_explicit_path_bypasses_the_network() {
        let dir = std::env::temp_dir().join("cordial-cs-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, r#"{"applicationSettings":{}}"#).unwrap();
        assert_eq!(
            load_base(Some(p.to_str().unwrap())).as_deref(),
            Some(r#"{"applicationSettings":{}}"#)
        );
    }
}
