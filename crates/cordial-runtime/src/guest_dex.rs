//! The JNI descriptors of an APK's declared `native` methods, read from its
//! dex files (docs/vr/dynarmic-design.md §3.3).
//!
//! The arm64 engine's `Java_*` exports are guest code, and a host caller --
//! `cordial-run`'s bring-up, libjnivm, the web view and permission bridges --
//! calls them with a SysV frame. The host entry that translates that frame
//! needs the method's argument types, and the export's name does not carry
//! them unless the method is overloaded. The dex does: every `native` method
//! is declared there with its prototype, which is the same question `javap -s`
//! or `dexdump` answers.
//!
//! Declaration metadata only: the string, type, proto and method tables and
//! each class's method list with its access flags. No code item is read.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

const ACC_NATIVE: u64 = 0x100;

fn u16_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

fn uleb(b: &[u8], o: &mut usize) -> Option<u64> {
    let (mut r, mut s) = (0u64, 0);
    loop {
        let x = *b.get(*o)?;
        *o += 1;
        r |= ((x & 0x7f) as u64) << s;
        if x & 0x80 == 0 {
            return Some(r);
        }
        s += 7;
        if s > 35 {
            return None;
        }
    }
}

struct Dex<'a> {
    b: &'a [u8],
    strings: usize,
    types: usize,
    protos: usize,
    methods: usize,
}

impl Dex<'_> {
    /// A string_data_item as UTF-8. The names that matter here are ASCII;
    /// MUTF-8's differences (NUL and supplementary characters) only affect
    /// the mangled `_0xxxx` escapes of names nobody exports.
    fn string(&self, i: usize) -> Option<String> {
        let mut o = u32_at(self.b, self.strings + 4 * i)?;
        uleb(self.b, &mut o)?;
        let end = o + self.b.get(o..)?.iter().position(|&c| c == 0)?;
        Some(String::from_utf8_lossy(&self.b[o..end]).into_owned())
    }

    fn type_name(&self, i: usize) -> Option<String> {
        self.string(u32_at(self.b, self.types + 4 * i)?)
    }

    /// `(args)ret` for proto `i`.
    fn proto(&self, i: usize) -> Option<String> {
        let p = self.protos + 12 * i;
        let ret = self.type_name(u32_at(self.b, p + 4)?)?;
        let params = u32_at(self.b, p + 8)?;
        let mut s = String::from("(");
        if params != 0 {
            let n = u32_at(self.b, params)?;
            for k in 0..n {
                s.push_str(&self.type_name(u16_at(self.b, params + 4 + 2 * k)?)?);
            }
        }
        s.push(')');
        s.push_str(&ret);
        Some(s)
    }

    /// (class descriptor, method name, prototype) of method_id `i`.
    fn method(&self, i: usize) -> Option<(String, String, String)> {
        let m = self.methods + 8 * i;
        Some((
            self.type_name(u16_at(self.b, m)?)?,
            self.string(u32_at(self.b, m + 4)?)?,
            self.proto(u16_at(self.b, m + 2)?)?,
        ))
    }
}

/// Every `native` method one dex declares: (class descriptor, name, prototype).
pub fn natives_in(b: &[u8]) -> Option<Vec<(String, String, String)>> {
    if !b.starts_with(b"dex\n") {
        return None;
    }
    let d = Dex {
        b,
        strings: u32_at(b, 0x3c)?,
        types: u32_at(b, 0x44)?,
        protos: u32_at(b, 0x4c)?,
        methods: u32_at(b, 0x5c)?,
    };
    let (n_classes, classes) = (u32_at(b, 0x60)?, u32_at(b, 0x64)?);
    let mut out = Vec::new();
    for c in 0..n_classes {
        let data = u32_at(b, classes + 32 * c + 24)?;
        if data == 0 {
            continue;
        }
        let mut o = data;
        let sf = uleb(b, &mut o)?;
        let inf = uleb(b, &mut o)?;
        let dm = uleb(b, &mut o)?;
        let vm = uleb(b, &mut o)?;
        for _ in 0..(sf + inf) * 2 {
            uleb(b, &mut o)?;
        }
        for count in [dm, vm] {
            let mut idx = 0u64;
            for _ in 0..count {
                idx += uleb(b, &mut o)?;
                let flags = uleb(b, &mut o)?;
                uleb(b, &mut o)?;
                if flags & ACC_NATIVE != 0 {
                    out.push(d.method(idx as usize)?);
                }
            }
        }
    }
    Some(out)
}

/// JNI's name mangling (the JNI specification, "Resolving Native Method
/// Names"), for a class or method name with `/` separators, or for the
/// argument part of a descriptor.
pub fn mangle(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for u in s.encode_utf16() {
        match u {
            0x2f => o.push('_'),
            0x5f => o.push_str("_1"),
            0x3b => o.push_str("_2"),
            0x5b => o.push_str("_3"),
            u if u < 0x80 && (u as u8).is_ascii_alphanumeric() => o.push(u as u8 as char),
            u => o.push_str(&format!("_0{u:04x}")),
        }
    }
    o
}

/// The short and long `Java_*` names a native would be exported under.
pub fn jni_names(class: &str, name: &str, proto: &str) -> (String, String) {
    let class = class.strip_prefix('L').and_then(|c| c.strip_suffix(';')).unwrap_or(class);
    let short = format!("Java_{}_{}", mangle(class), mangle(name));
    let args = proto.strip_prefix('(').and_then(|p| p.split(')').next()).unwrap_or("");
    let long = format!("{short}__{}", mangle(args));
    (short, long)
}

/// `Java_*` export name -> JNI descriptor, for every native the APK's dex
/// files declare. A short name shared by overloads is left out, since only
/// the long names can tell them apart and the export would be the long one.
pub fn native_descriptors(apk: &Path) -> Result<HashMap<String, String>, String> {
    let f = std::fs::File::open(apk).map_err(|e| format!("{}: {e}", apk.display()))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", apk.display()))?;
    let mut map: HashMap<String, String> = HashMap::new();
    let mut ambiguous = Vec::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).map_err(|e| e.to_string())?;
        let n = e.name().to_owned();
        if !(n.starts_with("classes") && n.ends_with(".dex") && !n.contains('/')) {
            continue;
        }
        let mut b = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut b).map_err(|e| format!("{n}: {e}"))?;
        let natives = natives_in(&b).ok_or_else(|| format!("{n}: not a dex this parser can read"))?;
        for (class, name, proto) in natives {
            let (short, long) = jni_names(&class, &name, &proto);
            if let Some(prev) = map.insert(short.clone(), proto.clone()) {
                if prev != proto {
                    ambiguous.push(short);
                }
            }
            map.insert(long, proto);
        }
    }
    for s in ambiguous {
        map.remove(&s);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangling_follows_the_jni_specification() {
        let (s, l) = jni_names("Lcom/roblox/engine/jni/NativeGLInterface;", "nativeSet_Thing",
                               "(Ljava/lang/String;[IJ)V");
        assert_eq!(s, "Java_com_roblox_engine_jni_NativeGLInterface_nativeSet_1Thing");
        assert_eq!(l, format!("{s}__Ljava_lang_String_2_3IJ"));
        assert_eq!(mangle("a\u{e9}"), "a_000e9");
    }

    /// A hand-built dex with one class, one native and one ordinary method.
    #[test]
    fn natives_are_read_from_the_class_data() {
        let strings = ["LFoo;", "V", "I", "nat", "plain", "VI"];
        let mut b = vec![0u8; 0x70];
        b[..8].copy_from_slice(b"dex\n035\0");
        let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        // string data, then ids
        let mut offs = Vec::new();
        for s in strings {
            offs.push(b.len() as u32);
            b.push(s.len() as u8);
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        while b.len() % 4 != 0 {
            b.push(0);
        }
        let sid = b.len();
        for o in &offs {
            b.extend_from_slice(&o.to_le_bytes());
        }
        let tid = b.len();
        for s in [0u32, 1, 2] {
            b.extend_from_slice(&s.to_le_bytes()); // LFoo; V I
        }
        let tl = b.len();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        let pid = b.len();
        for v in [5u32, 1, tl as u32] {
            b.extend_from_slice(&v.to_le_bytes()); // (I)V
        }
        let mid = b.len();
        for (name, _) in [(3u32, ()), (4, ())] {
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&name.to_le_bytes());
        }
        let cdata = b.len();
        b.extend_from_slice(&[0, 0, 2, 0, 0, 0x81, 0x02, 0, 1, 0x01, 0]);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        let cdef = b.len();
        b.extend_from_slice(&[0u8; 32]);
        put(&mut b, cdef + 24, cdata as u32);
        put(&mut b, 0x3c, sid as u32);
        put(&mut b, 0x44, tid as u32);
        put(&mut b, 0x4c, pid as u32);
        put(&mut b, 0x5c, mid as u32);
        put(&mut b, 0x60, 1);
        put(&mut b, 0x64, cdef as u32);
        let n = natives_in(&b).unwrap();
        assert_eq!(n, vec![("LFoo;".to_string(), "nat".to_string(), "(I)V".to_string())]);
    }
}
