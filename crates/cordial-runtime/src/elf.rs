//! Read the undefined symbols out of an ELF64 shared object.
//!
//! **This exists so symbol resolution asks the library what it needs instead of
//! asking a file somebody remembered to update.** `docs/analysis/undefined-symbols.tsv`
//! is a snapshot of what one build imported, and twice now a Roblox update has
//! added an import that was not in it and stopped the client loading outright:
//! `hypotf` in 2.734.0.917 and `getpwuid_r` in 2.738.0.1393. Both are ordinary C
//! library functions the host has always provided. See issue #15.
//!
//! Only `.dynsym` is read, and only the `SHN_UNDEF` entries with a name -- that
//! is exactly the set `readelf --dyn-syms -W | awk '$7=="UND"'` prints, which is
//! the recipe AGENTS.md already gives for this question.
//!
//! Section headers rather than `PT_DYNAMIC`: the entry count then comes from
//! `sh_size / sh_entsize` and is exact. Walking `DT_SYMTAB` instead means
//! recovering the count from `DT_HASH`'s `nchain` or by walking `DT_GNU_HASH`'s
//! bucket chains, which is more code and more ways to be subtly wrong. If a
//! future build ships with its section headers stripped this returns an error
//! naming that, rather than silently finding nothing -- a silent empty answer
//! here would look exactly like "this build imports nothing new".
//!
//! The file is read in pieces rather than slurped. `libroblox.so` is 118 MB and
//! the three regions this needs total well under a megabyte.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const SHT_DYNSYM: u32 = 11;
const SHN_UNDEF: u16 = 0;
const STB_WEAK: u8 = 2;
const EHDR_LEN: usize = 64;
const SHDR_LEN: usize = 64;
const SYM_LEN: usize = 24;

/// Whether the linker must resolve an import or may leave it null.
///
/// This distinction is not decoration. `libroblox.so` imports eight symbols
/// weakly -- `__gcov_dump`, `__gcov_flush`, `getentropy`,
/// `__cxa_thread_atexit_impl` and four `ZSTD_trace_*` hooks -- and a weak
/// import nothing provides is resolved to zero rather than being an error.
/// Reporting those as "the load will fail" would be a false alarm on every
/// single launch, which is the fastest way to teach everyone to ignore the
/// warning that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Must resolve, or the load fails.
    Strong,
    /// May resolve to nothing.
    Weak,
}

/// Every symbol an object imports, and whether it has to be answered.
pub type Imports = BTreeMap<String, Binding>;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Read exactly `len` bytes at `off`, and say which region ran off the end.
///
/// `read_exact`'s own message is "failed to fill whole buffer", which names
/// neither the file nor what was being read -- and the first test written
/// against this hit exactly that, on a 46-byte file that is simply not an ELF
/// object. A truncated or half-downloaded `libroblox.so` is a real thing to
/// meet, and the error should say so.
fn read_at(f: &mut File, off: u64, len: usize, what: &str) -> io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            bad(format!(
                "truncated: {what} wants {len} bytes at offset {off}, past the end of the file"
            ))
        } else {
            e
        }
    })?;
    Ok(buf)
}

/// Every symbol the object imports: `.dynsym` entries with `st_shndx == SHN_UNDEF`
/// and a non-empty name, each with its binding.
///
/// Version suffixes are not stripped because `.dynsym` does not carry them --
/// `readelf`'s `hypotf@GLIBC_2.2.5` spelling comes from `.gnu.version`, which is
/// a parallel table this deliberately does not read. The bare name is what the
/// linker resolves on and what the host lookup wants.
pub fn undefined_symbols(path: &Path) -> io::Result<Imports> {
    let mut f = File::open(path)?;

    // Length first, so a file too short to hold a header is reported as not
    // being an ELF object rather than as a truncated one -- the common case is
    // that something else entirely was handed in.
    if f.metadata()?.len() < EHDR_LEN as u64 {
        return Err(bad(format!(
            "{} is {} bytes, too short to be an ELF object",
            path.display(),
            f.metadata()?.len()
        )));
    }

    let ehdr = read_at(&mut f, 0, EHDR_LEN, "the ELF header")?;
    if &ehdr[0..4] != b"\x7fELF" {
        return Err(bad(format!("{} is not an ELF file", path.display())));
    }
    // ELFCLASS64 and ELFDATA2LSB. Roblox's Android x86-64 build is both, and a
    // mismatch here means the wrong file was handed in rather than a format
    // worth supporting.
    if ehdr[4] != 2 || ehdr[5] != 1 {
        return Err(bad(format!(
            "{} is not 64-bit little-endian ELF",
            path.display()
        )));
    }

    let e_shoff = u64le(&ehdr, 0x28);
    let e_shentsize = u16le(&ehdr, 0x3a) as usize;
    let mut e_shnum = u16le(&ehdr, 0x3c) as u64;

    if e_shoff == 0 || e_shnum == 0 && e_shentsize == 0 {
        return Err(bad(format!(
            "{} has no section headers, so .dynsym cannot be located; \
             it was probably stripped with --strip-sections",
            path.display()
        )));
    }
    if e_shentsize != SHDR_LEN {
        return Err(bad(format!(
            "{} has {e_shentsize}-byte section headers, expected {SHDR_LEN}",
            path.display()
        )));
    }

    // e_shnum == 0 with a section header table present means the real count did
    // not fit in 16 bits and lives in section 0's sh_size. Rare, but cheap to
    // honour and it costs one read.
    if e_shnum == 0 {
        let zero = read_at(&mut f, e_shoff, SHDR_LEN, "section header 0")?;
        e_shnum = u64le(&zero, 0x20);
    }

    let shdrs = read_at(
        &mut f,
        e_shoff,
        (e_shnum as usize)
            .checked_mul(SHDR_LEN)
            .ok_or_else(|| bad("section header table size overflows"))?,
        "the section header table",
    )?;

    let mut found = None;
    for i in 0..e_shnum as usize {
        let s = &shdrs[i * SHDR_LEN..(i + 1) * SHDR_LEN];
        if u32le(s, 0x04) == SHT_DYNSYM {
            found = Some((
                u64le(s, 0x18), // sh_offset
                u64le(s, 0x20), // sh_size
                u32le(s, 0x28), // sh_link -> the string table
                u64le(s, 0x38), // sh_entsize
            ));
            break;
        }
    }
    let (sym_off, sym_size, strtab_idx, entsize) = found.ok_or_else(|| {
        bad(format!(
            "{} has section headers but no SHT_DYNSYM among them",
            path.display()
        ))
    })?;

    let entsize = if entsize == 0 { SYM_LEN as u64 } else { entsize };
    if entsize != SYM_LEN as u64 {
        return Err(bad(format!(
            "{} has {entsize}-byte .dynsym entries, expected {SYM_LEN}",
            path.display()
        )));
    }
    if strtab_idx as u64 >= e_shnum {
        return Err(bad(format!(
            "{}'s .dynsym links to section {strtab_idx}, past the end of the table",
            path.display()
        )));
    }

    let st = &shdrs[strtab_idx as usize * SHDR_LEN..(strtab_idx as usize + 1) * SHDR_LEN];
    let strtab = read_at(&mut f, u64le(st, 0x18), u64le(st, 0x20) as usize, ".dynstr")?;
    let syms = read_at(&mut f, sym_off, sym_size as usize, ".dynsym")?;

    let mut out = Imports::new();
    for chunk in syms.chunks_exact(SYM_LEN) {
        if u16le(chunk, 0x06) != SHN_UNDEF {
            continue;
        }
        let name_off = u32le(chunk, 0x00) as usize;
        if name_off == 0 || name_off >= strtab.len() {
            continue;
        }
        let end = strtab[name_off..]
            .iter()
            .position(|&c| c == 0)
            .map(|n| name_off + n)
            .unwrap_or(strtab.len());
        if end == name_off {
            continue;
        }
        let Ok(name) = std::str::from_utf8(&strtab[name_off..end]) else {
            continue;
        };
        let binding = if chunk[0x04] >> 4 == STB_WEAK {
            Binding::Weak
        } else {
            Binding::Strong
        };
        // One name can appear more than once across the objects in a directory,
        // and strong wins: if anything requires it, it is required.
        match out.entry(name.to_string()) {
            std::collections::btree_map::Entry::Occupied(mut e) => {
                if binding == Binding::Strong {
                    e.insert(Binding::Strong);
                }
            }
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(binding);
            }
        }
    }
    Ok(out)
}

/// Every `*.so` directly inside `dir`, unioned.
///
/// A directory rather than one file because the TSV's first column records
/// eleven consumer libraries, not one -- `libbacktrace-native.so` alone
/// contributes 312 of its rows. Only `libroblox.so` is extracted here today,
/// but a build that starts shipping a second library should not need this
/// function changed.
///
/// A file that cannot be parsed is reported and skipped rather than fatal: one
/// unreadable object in the directory should not stop the client, and the
/// caller prints what was skipped.
pub fn undefined_symbols_in_dir(dir: &Path) -> (Imports, Vec<(String, String)>) {
    let mut all = Imports::new();
    let mut skipped = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (all, vec![(dir.display().to_string(), "unreadable".into())]);
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".so") || n.contains(".so."))
        })
        .collect();
    paths.sort();
    for p in paths {
        match undefined_symbols(&p) {
            Ok(found) => {
                for (name, binding) in found {
                    let slot = all.entry(name).or_insert(binding);
                    if binding == Binding::Strong {
                        *slot = Binding::Strong;
                    }
                }
            }
            Err(e) => skipped.push((p.display().to_string(), e.to_string())),
        }
    }
    (all, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// A file that deletes itself, so these tests need no dependency. Adding
    /// `tempfile` for four tests would be the workspace's first use of it.
    struct Scratch(PathBuf);

    impl Scratch {
        fn dir(tag: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("cordial-elf-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("scratch directory");
            Scratch(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A hand-built ELF64 with one `.dynsym` holding four entries: the
    /// mandatory null one, a defined symbol, a strong undefined one and a weak
    /// undefined one. Synthetic rather than a fixture from the host, because
    /// the host's libraries differ between the machines this runs on and a test
    /// that reads one asserts on whatever glibc happens to export.
    fn synthetic(tag: &str, strip_sections: bool) -> (Scratch, PathBuf, &'static str) {
        // Layout: ehdr | strtab | dynsym | shdrs
        let strtab: Vec<u8> = {
            let mut v = vec![0u8]; // index 0 is the empty name
            v.extend_from_slice(b"defined\0");
            v.extend_from_slice(b"wanted_from_host\0");
            v.extend_from_slice(b"optional_hook\0");
            v
        };
        let str_defined = 1u32;
        let str_wanted = 1 + b"defined\0".len() as u32;
        let str_weak = str_wanted + b"wanted_from_host\0".len() as u32;

        let mut dynsym = vec![0u8; SYM_LEN]; // null entry
        let sym = |name: u32, shndx: u16, bind: u8| {
            let mut e = vec![0u8; SYM_LEN];
            e[0..4].copy_from_slice(&name.to_le_bytes());
            e[4] = (bind << 4) | 0x2; // STT_FUNC
            e[6..8].copy_from_slice(&shndx.to_le_bytes());
            e
        };
        dynsym.extend(sym(str_defined, 1, 1)); // defined in some section
        dynsym.extend(sym(str_wanted, SHN_UNDEF, 1));
        dynsym.extend(sym(str_weak, SHN_UNDEF, STB_WEAK));

        let strtab_off = EHDR_LEN;
        let dynsym_off = strtab_off + strtab.len();
        let shdrs_off = dynsym_off + dynsym.len();

        let shdr = |ty: u32, off: usize, size: usize, link: u32, entsize: u64| {
            let mut s = vec![0u8; SHDR_LEN];
            s[0x04..0x08].copy_from_slice(&ty.to_le_bytes());
            s[0x18..0x20].copy_from_slice(&(off as u64).to_le_bytes());
            s[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
            s[0x28..0x2c].copy_from_slice(&link.to_le_bytes());
            s[0x38..0x40].copy_from_slice(&entsize.to_le_bytes());
            s
        };
        let mut shdrs = shdr(0, 0, 0, 0, 0); // SHN_UNDEF section
        shdrs.extend(shdr(3, strtab_off, strtab.len(), 0, 0)); // SHT_STRTAB, index 1
        shdrs.extend(shdr(SHT_DYNSYM, dynsym_off, dynsym.len(), 1, SYM_LEN as u64));

        let mut ehdr = vec![0u8; EHDR_LEN];
        ehdr[0..4].copy_from_slice(b"\x7fELF");
        ehdr[4] = 2; // ELFCLASS64
        ehdr[5] = 1; // ELFDATA2LSB
        ehdr[6] = 1; // EV_CURRENT
        if !strip_sections {
            ehdr[0x28..0x30].copy_from_slice(&(shdrs_off as u64).to_le_bytes());
            ehdr[0x3a..0x3c].copy_from_slice(&(SHDR_LEN as u16).to_le_bytes());
            ehdr[0x3c..0x3e].copy_from_slice(&3u16.to_le_bytes());
        }

        let dir = Scratch::dir(tag);
        let path = dir.path().join("libsynthetic.so");
        let mut f = std::fs::File::create(&path).expect("scratch file");
        f.write_all(&ehdr).unwrap();
        f.write_all(&strtab).unwrap();
        f.write_all(&dynsym).unwrap();
        f.write_all(&shdrs).unwrap();
        f.flush().unwrap();
        (dir, path, "wanted_from_host")
    }

    #[test]
    fn an_undefined_symbol_is_reported_and_a_defined_one_is_not() {
        let (_d, path, wanted) = synthetic("und", false);
        let got = undefined_symbols(&path).expect("parses");
        assert_eq!(got.get(wanted), Some(&Binding::Strong), "{got:?}");
        assert!(
            !got.contains_key("defined"),
            "a defined symbol is not an import: {got:?}"
        );
        assert_eq!(got.len(), 2, "the null entry must not be counted: {got:?}");
    }

    /// The eight weak imports in `libroblox.so` are the reason this is tracked
    /// at all: unresolved, they are answered with zero and the load proceeds.
    #[test]
    fn a_weak_import_is_marked_weak() {
        let (_d, path, _) = synthetic("weak", false);
        let got = undefined_symbols(&path).expect("parses");
        assert_eq!(got.get("optional_hook"), Some(&Binding::Weak), "{got:?}");
    }

    /// The failure this must never turn into a silent empty answer: an object
    /// with no section headers reads as "imports nothing", which is
    /// indistinguishable from a build that genuinely added no symbol.
    #[test]
    fn a_stripped_object_is_an_error_rather_than_an_empty_set() {
        let (_d, path, _) = synthetic("stripped", true);
        let err = undefined_symbols(&path).expect_err("must not succeed");
        assert!(
            err.to_string().contains("no section headers"),
            "unhelpful error: {err}"
        );
    }

    #[test]
    fn a_file_that_is_not_elf_is_refused_by_name() {
        let d = Scratch::dir("notelf");
        let path = d.path().join("notelf.so");
        std::fs::write(&path, b"this is not an ELF file at all, not even close").unwrap();
        let err = undefined_symbols(&path).expect_err("must not succeed");
        assert!(err.to_string().contains("too short to be an ELF"), "{err}");

        // Long enough for a header, still not one.
        let long = d.path().join("notelf-long.so");
        std::fs::write(&long, vec![b'x'; 4096]).unwrap();
        let err = undefined_symbols(&long).expect_err("must not succeed");
        assert!(err.to_string().contains("not an ELF"), "{err}");
    }

    #[test]
    fn a_directory_walk_unions_and_reports_what_it_skipped() {
        let dir = Scratch::dir("walk");
        let (_d, good, wanted) = synthetic("walk-src", false);
        std::fs::copy(&good, dir.path().join("libgood.so")).unwrap();
        std::fs::write(dir.path().join("libbad.so"), b"not elf").unwrap();
        // Not a shared object, so not even looked at.
        std::fs::write(dir.path().join("notes.txt"), b"ignored").unwrap();

        let (syms, skipped) = undefined_symbols_in_dir(dir.path());
        assert_eq!(syms.get(wanted), Some(&Binding::Strong), "{syms:?}");
        assert_eq!(skipped.len(), 1, "expected one skip, got {skipped:?}");
        assert!(skipped[0].0.ends_with("libbad.so"), "{skipped:?}");
    }
}
