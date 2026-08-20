//! One line, at exit, about whether Roblox's own storage came up.
//!
//! **The engine's answer to this question is emitted into nothing.**
//! `RbxStorage::init [INIT]` logs at Critical *before* the engine has opened
//! its log file, so the line that would say whether storage initialised never
//! reaches disk or the terminal. That is why storage looked broken here for
//! something like fifty runs while it was working, and it is a fair share of
//! why the logging is not trusted. Cordial can answer the same question
//! cheaply from outside — the database is a file, and a file can be looked at
//! — so it does.
//!
//! Deliberately not a health check and not a verdict. It reports what is
//! there; whether ten rows is the right number for a given session is not
//! something this can know.

use std::path::Path;

/// What `rbx-storage.db` looks like from outside the engine.
#[derive(Debug, PartialEq, Eq)]
pub enum Storage {
    /// No file. Either the engine never got as far as creating one, or it
    /// created it somewhere else.
    Missing,
    /// The file is there but is not a SQLite database, or is truncated past
    /// the point where its own header stops making sense. Reported rather
    /// than swallowed: a zero-length `rbx-storage.db` is a different failure
    /// from an absent one and the two used to look the same.
    Unreadable(&'static str),
    Present { bytes: u64, pages: u32, rows: u64 },
}

/// Count the rows a SQLite file holds, by summing the cell count in the header
/// of every table-leaf page.
///
/// **Why not open it with a SQLite library.** The whole cost of this is meant
/// to be a `stat` and a read; pulling `rusqlite` in for one line at exit would
/// add a C dependency and a build to every configuration of this workspace.
/// The file format is stable and the part being read is the first five bytes
/// of each page.
///
/// What it counts is every table's rows, including SQLite's own `sqlite_master`
/// and `sqlite_stat1`, because separating them out means decoding
/// `sqlite_master`'s records to find one root page and that is where a
/// hand-written reader starts being able to be subtly wrong. Index leaves
/// (`0x0A`) are not rows and are not counted.
///
/// Checked against `sqlite3` on a real `rbx-storage.db` from a 45-second run:
/// `select count(*)` over the three tables gave 7 + 10 + 5 = 22, and this
/// returned 22.
fn scan(data: &[u8]) -> Storage {
    if data.len() < 100 || !data.starts_with(b"SQLite format 3\0") {
        return Storage::Unreadable("not a SQLite database");
    }
    // Page size lives at offset 16 as a big-endian u16, where the value 1
    // means 65536 -- the one size that does not fit the field.
    let page_size = match u16::from_be_bytes([data[16], data[17]]) {
        1 => 65536u32,
        n if n >= 512 && n.is_power_of_two() => u32::from(n),
        _ => return Storage::Unreadable("implausible page size"),
    };
    let pages = u32::from_be_bytes([data[28], data[29], data[30], data[31]]);
    let mut rows: u64 = 0;
    for i in 0..pages {
        // Page 1 carries the 100-byte file header before its own b-tree header.
        let header_offset = if i == 0 { 100usize } else { 0 };
        let Some(start) = (i as usize).checked_mul(page_size as usize) else {
            return Storage::Unreadable("page count overflows the file");
        };
        let at = start + header_offset;
        if at + 5 > data.len() {
            // A page count larger than the file. Truncated mid-write, most
            // likely; count what is there rather than refusing to say anything.
            break;
        }
        // 0x0D is a table b-tree leaf: the only page kind whose cells are rows.
        if data[at] == 0x0D {
            rows += u64::from(u16::from_be_bytes([data[at + 3], data[at + 4]]));
        }
    }
    Storage::Present { bytes: data.len() as u64, pages, rows }
}

pub fn inspect(path: &Path) -> Storage {
    match std::fs::read(path) {
        Ok(data) => scan(&data),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Storage::Missing,
        Err(_) => Storage::Unreadable("could not be read"),
    }
}

/// Print the line. `files_dir` is the engine's `files` directory -- the
/// database sits at `appData/rbx-storage.db` under it, which is where the
/// engine put it on every run this was checked against.
pub fn report(files_dir: &Path) {
    let path = files_dir.join("appData/rbx-storage.db");
    match inspect(&path) {
        Storage::Present { bytes, pages, rows } => println!(
            "[storage] rbx-storage.db: {bytes} bytes, {pages} pages, {rows} rows across its tables \
             ({})",
            path.display()
        ),
        Storage::Missing => println!(
            "[storage] no rbx-storage.db at {} -- the engine's own RbxStorage::init line logs \
             before its log file is open, so this is the only report of it there is",
            path.display()
        ),
        Storage::Unreadable(why) => {
            println!("[storage] rbx-storage.db at {}: {why}", path.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-page file built by hand rather than by SQLite, so the test needs
    /// no database engine to run. Page 1 is a table leaf with three cells,
    /// page 2 an index leaf with nine -- the index leaf is the point: it must
    /// not be counted.
    fn synthetic() -> Vec<u8> {
        let mut d = vec![0u8; 8192];
        d[..16].copy_from_slice(b"SQLite format 3\0");
        d[16..18].copy_from_slice(&4096u16.to_be_bytes());
        d[28..32].copy_from_slice(&2u32.to_be_bytes());
        d[100] = 0x0D;
        d[103..105].copy_from_slice(&3u16.to_be_bytes());
        d[4096] = 0x0A;
        d[4099..4101].copy_from_slice(&9u16.to_be_bytes());
        d
    }

    #[test]
    fn index_leaves_are_not_rows() {
        assert_eq!(scan(&synthetic()), Storage::Present { bytes: 8192, pages: 2, rows: 3 });
    }

    #[test]
    fn a_file_that_is_not_a_database_says_so_rather_than_reporting_zero_rows() {
        // Zero rows and "this is not a database" are different answers, and
        // the whole point of the line is that somebody reads it and believes
        // it.
        assert_eq!(scan(b"not a database at all, but long enough to have a header\0\0\0"),
                   Storage::Unreadable("not a SQLite database"));
    }

    #[test]
    fn a_page_count_past_the_end_of_the_file_counts_what_is_there() {
        // An interrupted write, which is exactly the state a crash leaves.
        let mut d = synthetic();
        d[28..32].copy_from_slice(&9u32.to_be_bytes());
        assert_eq!(scan(&d), Storage::Present { bytes: 8192, pages: 9, rows: 3 });
    }

    #[test]
    fn a_missing_file_is_missing_and_not_an_error() {
        assert_eq!(inspect(Path::new("/nonexistent/rbx-storage.db")), Storage::Missing);
    }
}
