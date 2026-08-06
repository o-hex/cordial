#!/usr/bin/env python3
"""Print declared generic signatures from `dalvik.annotation.Signature`.

`tools/dex_method.py` prints a method's erased descriptor, which is what the
method_ids table holds -- and for anything taking a collection that is not
enough. `nativePostClientSettingsLoadedInitialization3(Ljava/util/List;)V` was
recorded as unresolved for two sessions and treated as a place where Cordial had
to guess; the generic signature says

    (Ljava/util/List<Lcom/roblox/engine/jni/model/ApplicationExitInfoCpp;>;)V

which settles it. Reach for this whenever a descriptor says `List`, `Map` or
`Object` and the question is what of.

Declaration metadata only: the annotations directory and the string table. No
code units are read, and this is the same question `javap -s` answers.

Two layout details that cost real time, recorded so the next reader does not
re-find them: `annotations_off` is field 5 of `class_def_item` and field 3 is
`interfaces_off`, and the first word of `annotations_directory_item` is an
offset while the three after it are counts.

Usage:
    tools/dex_signature.py <dir-of-dex> <class-substring> <method-substring>

Empty strings match everything, so `'' ''` dumps the lot.
"""
import struct, sys, pathlib, re

def uleb(b,o):
    r=s=0
    while True:
        x=b[o]; o+=1; r|=(x&0x7f)<<s
        if not x&0x80: return r,o
        s+=7

def val(b,o,strings):
    """Minimal encoded_value: enough for arrays and strings."""
    arg=b[o]>>5; t=b[o]&0x1f; o+=1
    if t==0x17:                                   # VALUE_STRING
        idx=0
        for i in range(arg+1): idx|=b[o+i]<<(8*i)
        return strings[idx], o+arg+1
    if t==0x1c:                                   # VALUE_ARRAY
        n,o=uleb(b,o); out=[]
        for _ in range(n):
            v,o=val(b,o,strings); out.append(v)
        return out,o
    if t==0x1e: return None,o                     # VALUE_NULL
    if t in (0x00,):  return None,o+arg+1
    return None,o+arg+1

def go(path, want_cls, want_meth):
    b=path.read_bytes()
    h=struct.unpack_from('<20I',b,56)
    str_sz,str_off,typ_sz,typ_off,pro_sz,pro_off,_fs,_fo,mth_sz,mth_off=h[0:10]
    cls_sz,cls_off=h[10],h[11]
    strings=[]
    for i in range(str_sz):
        off=struct.unpack_from('<I',b,str_off+4*i)[0]
        n,q=uleb(b,off); strings.append(b[q:q+n].decode('utf-8','replace'))
    types=[strings[struct.unpack_from('<I',b,typ_off+4*i)[0]] for i in range(typ_sz)]
    methods=[]
    for i in range(mth_sz):
        c,pr,nm=struct.unpack_from('<HHI',b,mth_off+8*i)
        methods.append((types[c],strings[nm]))
    for i in range(cls_sz):
        cd=struct.unpack_from('<8I',b,cls_off+32*i)
        cls=types[cd[0]]; ann_off=cd[5]   # class_def field 5; cd[3] is interfaces_off
        if want_cls not in cls or not ann_off: continue
        # class_annotations_off, then three COUNTS. Reading the first as a
        # count is what sent the earlier version off into garbage offsets.
        _cls_ann_off,fields_n,methods_n,_params_n=struct.unpack_from('<4I',b,ann_off)
        o=ann_off+16
        o+=8*fields_n                             # skip field annotations
        mf=methods_n
        for _ in range(mf):
            midx,aset=struct.unpack_from('<2I',b,o); o+=8
            mc,mn=methods[midx]
            if want_meth not in mn: continue
            try:
                n=struct.unpack_from('<I',b,aset)[0]
            except Exception:
                continue
            for j in range(n):
                item=struct.unpack_from('<I',b,aset+4+4*j)[0]
                p=item+1                          # visibility byte
                tidx,p=uleb(b,p)
                size,p=uleb(b,p)
                if tidx>=len(types) or types[tidx]!='Ldalvik/annotation/Signature;':
                    continue
                try:
                  for _ in range(size):
                    nidx,p=uleb(b,p)
                    v,p=val(b,p,strings)
                    if isinstance(v,list):
                        print(f'{path.name}  {mc[1:-1]}.{mn}')
                        print('   ' + ''.join(x for x in v if x))
                    else:
                        print(f'   {strings[nidx]} = {v}')
                except Exception as e:
                    print(f'   <unparsed annotation: {e}>')

for p in sorted(pathlib.Path(sys.argv[1]).glob('*.dex')):
    try: go(p, sys.argv[2], sys.argv[3])
    except Exception as e: print(f'-- {p.name}: {e}', file=sys.stderr)
