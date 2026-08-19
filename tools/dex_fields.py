#!/usr/bin/env python3
"""List a class's declared fields: name, type, and static/instance.

`tools/dex_method.py` answers what a method's descriptor is; it does not touch
fields at all, and `libjnivm` binds a `HookInstance` field by name and
descriptor exactly as strictly as it binds a method — a mismatch there is the
same silent failure `dex_method.py`'s own docstring describes for methods. This
was written because `com.roblox.engine.jni.model.BatteryStatus` needed
answering: `dex_method.py --class` on it shows only a bare `<init>()V`, which
means every one of its fields is a name Cordial had no way to guess.

Declaration metadata only: `field_ids` and each class's `encoded_field` lists in
`class_data_item`. No code units are read, and this is the same question
`javap -p` answers — not a decompiler, and not covered by AGENTS.md's line
against transcribing decompiled implementation, which is about *how* something
works rather than what it is named and typed.

Two dex layout notes, recorded here because `class_def_item`'s field 6 is
`class_data_off` and it is easy to miscount past the four `uN` fields before it
(`interfaces_off` at field 3, `annotations_off` at field 5 — `dex_signature.py`
already wrote that one down): a class with no code at all (an interface, or a
framework type this dex only references) has `class_data_off == 0` and is
skipped outright, and `class_data_item` itself opens with four ULEB128 sizes —
static field count, instance field count, direct method count, virtual method
count — before the encoded fields begin. Each encoded field is a delta-encoded
index into `field_ids` (`field_idx_diff`, cumulative from zero) plus its
`access_flags`, in that order — get the accumulation wrong and every field
after the first prints under the wrong name.

Usage:
    tools/dex_fields.py <dir-of-dex> <class-substring>
"""
import struct
import sys
import pathlib


def uleb(b, o):
    r = s = 0
    while True:
        x = b[o]
        o += 1
        r |= (x & 0x7f) << s
        if not x & 0x80:
            return r, o
        s += 7


def go(path, want_cls):
    b = path.read_bytes()
    h = struct.unpack_from('<20I', b, 56)
    str_sz, str_off, typ_sz, typ_off, _pro_sz, _pro_off, fld_sz, fld_off, _mth_sz, _mth_off = h[0:10]
    cls_sz, cls_off = h[10], h[11]

    strings = []
    for i in range(str_sz):
        off = struct.unpack_from('<I', b, str_off + 4 * i)[0]
        n, q = uleb(b, off)
        strings.append(b[q:q + n].decode('utf-8', 'replace'))
    types = [strings[struct.unpack_from('<I', b, typ_off + 4 * i)[0]] for i in range(typ_sz)]

    fields = []
    for i in range(fld_sz):
        c, t, nm = struct.unpack_from('<HHI', b, fld_off + 8 * i)
        fields.append((types[c], types[t], strings[nm]))

    for i in range(cls_sz):
        cd = struct.unpack_from('<8I', b, cls_off + 32 * i)
        cls, class_data_off = types[cd[0]], cd[6]
        if want_cls not in cls or not class_data_off:
            continue
        o = class_data_off
        static_n, o = uleb(b, o)
        instance_n, o = uleb(b, o)
        _direct_n, o = uleb(b, o)
        _virtual_n, o = uleb(b, o)

        print(f'== {cls} ==')
        for label, count in (('static', static_n), ('instance', instance_n)):
            print(f'  {label} fields ({count}):')
            idx = 0
            for _ in range(count):
                diff, o = uleb(b, o)
                idx += diff
                access, o = uleb(b, o)
                _fc, ft, fn = fields[idx]
                print(f'    {fn} : {ft}  (access=0x{access:x})')


for p in sorted(pathlib.Path(sys.argv[1]).glob('*.dex')):
    try:
        go(p, sys.argv[2])
    except Exception as e:
        print(f'-- {p.name}: {e}', file=sys.stderr)
