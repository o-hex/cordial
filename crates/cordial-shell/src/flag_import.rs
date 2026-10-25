//! `cordial --import-flags`: bring a flag list from another launcher into a
//! profile without opening a window.
//!
//! The rules -- what is a valid value, which formats there are, what one bad
//! entry costs -- are `cordial_plugins::flag_document::import`, the same code
//! the Settings page's Import button runs. This only reads the source, picks
//! the profile and writes the result.
//!
//! Adapted in part from DamnShabu/stacked's `cli_flags.rs` (GPL-3.0-or-later):
//! the `--sober` lookup, and the merge-or-replace shape.

use cordial_plugins::flag_document as fd;
use cordial_shell::profile;
use std::collections::BTreeMap;
use std::path::Path;

const USAGE: &str = "usage: cordial --import-flags FILE|-|--sober [--profile NAME] [--replace]\n\
                     \n\
                     Reads a Bloxstrap or Fishstrap ClientAppSettings.json, or Sober's config.json,\n\
                     and merges its FastFlags into a profile's. - reads standard input; --sober\n\
                     finds Sober's own config. --replace discards the flags the profile had.";

/// Returns the exit status: 0 imported, 1 could not, 2 misuse.
pub fn run(args: &[String]) -> u8 {
    let mut source: Option<String> = None;
    let mut named: Option<String> = None;
    let mut replace = false;
    let mut it = args.iter().skip_while(|a| *a != "--import-flags").skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--replace" => replace = true,
            "--profile" => match it.next() {
                Some(name) => named = Some(name.clone()),
                None => return misuse("--profile needs a NAME"),
            },
            other if source.is_none() => source = Some(other.to_string()),
            other => return misuse(&format!("unexpected argument {other:?}")),
        }
    }
    let Some(source) = source else { return misuse("--import-flags needs a FILE, -, or --sober") };

    let (text, shown) = match read_source(&source) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let name = named.clone().unwrap_or_else(|| crate::shell_config::load(&crate::shell_config::path()).profile);
    let dir = match profile::dir(&name) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    // A typo in `--profile` would otherwise create a stray profile directory,
    // since writing creates it. The configured profile may not exist yet on a
    // machine that has not launched anything, and is written anyway, the way
    // the Settings page does.
    if named.is_some() && !profile::list().contains(&name) {
        return fail(&format!("there is no profile called {name:?}"));
    }
    let path = fd::path_in(&dir);
    match import_into(&path, &text, replace) {
        Ok(report) => {
            println!("{shown}: {}", report.summary);
            for line in &report.notes {
                println!("  {line}");
            }
            u8::from(report.nothing_taken)
        }
        Err(e) => fail(&format!("{shown}: {e}")),
    }
}

/// A path for a message, with the home directory as `~`.
fn shown(path: &Path) -> String {
    cordial_shell::doctor::private(&path.display().to_string())
}

fn misuse(message: &str) -> u8 {
    eprintln!("cordial: {message}\n{USAGE}");
    2
}

fn fail(message: &str) -> u8 {
    eprintln!("cordial: {message}");
    1
}

/// The text to import and a name for it in messages.
fn read_source(source: &str) -> Result<(String, String), String> {
    match source {
        "-" => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .map_err(|e| format!("reading standard input: {e}"))?;
            Ok((text, "standard input".into()))
        }
        "--sober" => {
            let candidates = fd::sober_config_candidates();
            let found = candidates.iter().find(|p| p.is_file()).ok_or_else(|| {
                let looked: Vec<String> = candidates.iter().map(|p| shown(p)).collect();
                format!("no Sober settings found. Looked in {}", looked.join(" and "))
            })?;
            let text = std::fs::read_to_string(found).map_err(|e| format!("{}: {e}", found.display()))?;
            Ok((text, shown(found)))
        }
        file => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            Ok((text, file.to_string()))
        }
    }
}

struct Report {
    summary: String,
    notes: Vec<String>,
    /// Entries were offered and none could be taken, so the status is a failure.
    nothing_taken: bool,
}

/// Merge (or replace) `text` into the flags document at `path`.
///
/// A document at `path` that exists and does not parse is an error and is not
/// touched: treating it as empty and writing into it would replace somebody's
/// hand-written list with the import.
fn import_into(path: &Path, text: &str, replace: bool) -> Result<Report, String> {
    let imported = fd::import(text)?;
    let mut flags: BTreeMap<String, String> = if replace {
        BTreeMap::new()
    } else {
        match std::fs::read_to_string(path) {
            Ok(existing) => fd::parse(&existing).map_err(|e| {
                format!("{} is not a usable flags file ({e}); it was not changed", path.display())
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    };
    let (added, changed) = fd::merge(&mut flags, &imported.flags);
    let nothing_taken = imported.flags.is_empty() && !imported.skipped.is_empty();
    if !nothing_taken {
        fd::write(path, &flags)?;
    }
    let mut notes = Vec::new();
    for s in &imported.skipped {
        notes.push(format!("skipped {}: {}", s.name, s.why));
    }
    if !imported.unrecognised.is_empty() {
        notes.push(format!(
            "these have no FastFlag prefix, so check the spelling (imported anyway): {}",
            imported.unrecognised.join(", ")
        ));
    }
    if nothing_taken {
        notes.push("nothing was written".into());
    } else {
        notes.push(format!("{} in total; applies the next time Roblox starts", flags.len()));
    }
    Ok(Report { summary: fd::summary(&imported, added, changed), notes, nothing_taken })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flags.json");
        (dir, path)
    }

    #[test]
    fn an_import_merges_into_what_the_profile_has() {
        let (_dir, path) = scratch();
        std::fs::write(&path, r#"{"FFlagKept": "True", "DFIntX": "1"}"#).unwrap();
        let report = import_into(&path, r#"{"DFIntX": 2, "FLogAudio": "Info"}"#, false).unwrap();
        assert!(report.summary.starts_with("Imported 2 flags: 1 new, 1 changed."), "{}", report.summary);
        let back = fd::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["FFlagKept"], "True");
        assert_eq!(back["DFIntX"], "2");
        assert_eq!(back["FLogAudio"], "Info");
    }

    #[test]
    fn replace_discards_what_was_there() {
        let (_dir, path) = scratch();
        std::fs::write(&path, r#"{"FFlagKept": "True"}"#).unwrap();
        import_into(&path, r#"{"FFlagNew": "False"}"#, true).unwrap();
        let back = fd::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.keys().collect::<Vec<_>>(), ["FFlagNew"]);
    }

    /// The fork refused a whole list over one FLog value.
    #[test]
    fn valid_log_values_and_one_bad_line_import_the_rest_and_name_the_bad_one() {
        let (_dir, path) = scratch();
        let text = r#"{"FLogAudio": "Info", "DFLogWebSocketTraceError": "Warning,6", "FFlagBad": "maybe", "DFIntOk": 3}"#;
        let report = import_into(&path, text, false).unwrap();
        assert!(!report.nothing_taken);
        assert!(report.notes.iter().any(|n| n.starts_with("skipped FFlagBad: ")), "{:?}", report.notes);
        let back = fd::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back["DFLogWebSocketTraceError"], "Warning,6");
    }

    #[test]
    fn a_sober_config_imports_its_fflags() {
        let (_dir, path) = scratch();
        let text = r#"{"use_opengl": false, "fflags": {"FFlagExample": true, "DFIntTaskSchedulerTargetFps": 144}}"#;
        import_into(&path, text, false).unwrap();
        let back = fd::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back["DFIntTaskSchedulerTargetFps"], "144");
    }

    #[test]
    fn a_list_where_nothing_can_be_taken_writes_nothing_and_fails() {
        let (_dir, path) = scratch();
        let report = import_into(&path, r#"{"FFlagA": "maybe"}"#, false).unwrap();
        assert!(report.nothing_taken);
        assert!(!path.exists(), "nothing imported means nothing written");
    }

    #[test]
    fn a_profile_whose_flags_do_not_parse_is_left_alone() {
        let (_dir, path) = scratch();
        std::fs::write(&path, "{\"FFlagA\": \"True\",}").unwrap();
        assert!(import_into(&path, r#"{"FFlagB": "True"}"#, false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"FFlagA\": \"True\",}");
    }

    #[test]
    fn a_document_that_is_not_json_is_an_error() {
        let (_dir, path) = scratch();
        assert!(import_into(&path, "not json", false).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn misuse_is_status_two() {
        assert_eq!(run(&["--import-flags".into()]), 2);
        assert_eq!(run(&["--import-flags".into(), "a.json".into(), "b.json".into()]), 2);
        assert_eq!(run(&["--import-flags".into(), "--profile".into()]), 2);
    }
}
