use std::path::Path;

use super::file::{read_file, shred};
use super::keyring::{ask, encode_keyring, read_keyring, Ask};
use super::{attributes, Kind, CALL_TIMEOUT};

/// Take a plaintext store into the service and destroy the file.
///
/// Returns the body when there was one, so the caller can use it for this
/// launch as well — a migration that signed somebody out on the way past would
/// be a worse bug than the one it is fixing.
///
/// **The shred happens only after the service has been asked for the body back
/// and returned it byte for byte.** Deleting on the strength of a write that
/// reported success is how a migration eats a session: `CreateItem` returning
/// an object path says the daemon accepted the call, not that the item is
/// there to be read. Nothing is compared by printing; the two bodies are
/// compared in memory and only their equality is ever reported.
pub(super) fn adopt_file(dir: &Path, kind: Kind) -> Option<String> {
    let path = dir.join(kind.name());
    let body = read_file(&path)?;

    let write = ask(
        Ask::Write {
            attrs: attributes(dir, kind),
            label: kind.label(dir),
            body: encode_keyring(&body),
        },
        CALL_TIMEOUT,
    );
    if let Err(why) = write {
        println!(
            "  [secrets] {}: {} bytes are still in plaintext at {} ({why}); \
             it was left alone rather than half-moved",
            kind.name(),
            body.len(),
            path.display()
        );
        return Some(body);
    }
    match read_keyring(&attributes(dir, kind)) {
        Ok(Some(back)) if back == body => match shred(&path) {
            Ok(()) => {
                println!(
                    "  [secrets] {}: {} bytes moved into the secret service; \
                     the plaintext file was overwritten and removed",
                    kind.name(),
                    body.len()
                );
                Some(body)
            }
            Err(e) => {
                println!(
                    "  [secrets] {}: moved into the secret service, but {} could not be \
                     destroyed ({e}); delete it by hand",
                    kind.name(),
                    path.display()
                );
                Some(body)
            }
        },
        _ => {
            println!(
                "  [secrets] {}: the secret service did not hand back what it was given, \
                 so {} was left alone",
                kind.name(),
                path.display()
            );
            Some(body)
        }
    }
}
