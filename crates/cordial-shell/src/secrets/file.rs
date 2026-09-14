use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Write a file in a profile at `0600`, atomically.
///
/// Temp file plus rename rather than truncate-and-write. An interrupted write
/// to the real path would leave a jar cut off mid-value, and half a cookie
/// still parses as a cookie — the engine would take it on the next launch and
/// fail authentication for a reason with no visible relationship to a power
/// cut. `rename` within a directory is atomic, so a reader sees the old file or
/// the new one.
///
/// The mode is set on the temp file *before* anything is written to it, not
/// after: a `0644` window with a live session in it is still a window, and it
/// is the one an attacker with a loop would use.
///
/// **One writer, for both stores and both kinds.** This began in `cookies.rs`
/// and was shared with `identity.rs` rather than copied, because a second
/// writer is a second chance to get the mode or the rename wrong — including
/// later, when only one of the two gets a fix. It moved here when the file
/// stopped being the only place a session can live; it is still the only thing
/// in Cordial that writes one to disk.
pub(super) fn write_private(dir: &Path, name: &str, body: &str) -> std::io::Result<()> {
    let final_path = dir.join(name);
    let tmp = dir.join(format!("{name}.new"));

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    // `mode` only applies when the call creates the file, so a leftover temp
    // from an interrupted run would keep whatever mode it had.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    drop(f);

    std::fs::rename(&tmp, &final_path)
}

pub(super) fn read_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub(super) enum BoundedRead {
    Present(Vec<u8>),
    Unavailable,
    TooLarge,
}

pub(super) fn read_bounded(path: &Path, max_bytes: usize) -> BoundedRead {
    use std::io::Read as _;

    let Ok(file) = std::fs::File::open(path) else {
        return BoundedRead::Unavailable;
    };
    let mut bytes = Vec::new();
    let limit = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    if file
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .is_err()
    {
        return BoundedRead::Unavailable;
    }
    if bytes.len() > max_bytes {
        BoundedRead::TooLarge
    } else if std::str::from_utf8(&bytes).is_err() {
        BoundedRead::Unavailable
    } else {
        BoundedRead::Present(bytes)
    }
}

/// Overwrite a file's bytes, then unlink it.
///
/// `remove_file` unlinks; it does not erase. The blocks stay on the device with
/// a live session token in them until something else happens to allocate them,
/// which on a half-empty disk can be never, and undelete is a normal thing for
/// a filesystem to support. Overwriting first is what makes "the plaintext copy
/// is gone" mean anything at all.
///
/// **What this does not do**, said plainly rather than left for somebody to
/// assume: on a copy-on-write filesystem — btrfs, which is Fedora's default and
/// is what this was written on — a rewrite may land in new blocks and leave the
/// originals intact, and no user-space overwrite touches a snapshot, an SSD's
/// remapped blocks, or a backup that was taken yesterday. This is a floor, not
/// a guarantee, and the honest instruction after a migration is still to change
/// your password if the file was ever somewhere it should not have been.
pub(super) fn shred(path: &Path) -> std::io::Result<()> {
    let len = std::fs::metadata(path)?.len();
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
        let zeros = [0u8; 4096];
        let mut left = len;
        while left > 0 {
            let n = std::cmp::min(left, zeros.len() as u64) as usize;
            f.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        f.sync_all()?;
    }
    std::fs::remove_file(path)
}
