#!/usr/bin/env python3
"""Find Java hooks that register but can never bind.

libjnivm builds a hook's JNI descriptor from its C++ types. If that descriptor
does not match what the engine looks up, the hook registers cleanly, the symbol
resolves, and it is simply never called -- there is no warning at build time or
run time, and `Constructed Unresolved symbol` names a method that is right there
in the source. Two live instances of this were found by hand in one evening
(`getAllocatableBytes` hooked as an instance method when it is static, and
`getWindowInsets` returning `Object` where the engine wants `Insets`), which is
what this exists to stop happening a third time.

It compares, for every hook in `native/*.cpp`:

  * static vs instance -- `Hook` against `HookInstanceFunction`, checked against
    the dex's `ACC_STATIC` bit
  * the full descriptor -- return type and parameters, derived from the C++
    signature, against the prototype the dex declares

Usage:
    tools/hook_descriptors.py <dir-of-dex>

Nothing here reads code, only declarations: the dex's method tables and the C++
function signatures. It is the same question `javap` answers.
"""

import pathlib
import re
import struct
import sys

ACC_STATIC = 0x8

PRIMITIVES = {
    'void': 'V', 'jint': 'I', 'jlong': 'J', 'jboolean': 'Z', 'jfloat': 'F',
    'jdouble': 'D', 'jshort': 'S', 'jbyte': 'B', 'jchar': 'C', 'int': 'I',
    'long': 'J', 'bool': 'Z', 'float': 'F', 'double': 'D',
}


def uleb(b, o):
    r = s = 0
    while True:
        x = b[o]
        o += 1
        r |= (x & 0x7f) << s
        if not x & 0x80:
            return r, o
        s += 7


def parse_dex(path):
    """-> {(class, method): (descriptor, is_static)}"""
    b = path.read_bytes()
    h = struct.unpack_from('<20I', b, 56)
    str_sz, str_off, typ_sz, typ_off, pro_sz, pro_off, _f_sz, _f_off, mth_sz, mth_off = h[0:10]
    cls_sz, cls_off = h[10], h[11]

    strings = []
    for i in range(str_sz):
        off = struct.unpack_from('<I', b, str_off + 4 * i)[0]
        n, p = uleb(b, off)
        strings.append(b[p:p + n].decode('utf-8', 'replace'))
    types = [strings[struct.unpack_from('<I', b, typ_off + 4 * i)[0]] for i in range(typ_sz)]

    protos = []
    for i in range(pro_sz):
        _sh, ret, params = struct.unpack_from('<3I', b, pro_off + 12 * i)
        args = []
        if params:
            n = struct.unpack_from('<I', b, params)[0]
            args = [types[struct.unpack_from('<H', b, params + 4 + 2 * j)[0]] for j in range(n)]
        protos.append('(' + ''.join(args) + ')' + types[ret])

    methods = []
    for i in range(mth_sz):
        c, pr, nm = struct.unpack_from('<HHI', b, mth_off + 8 * i)
        methods.append((types[c], strings[nm], protos[pr]))

    # Access flags live in class_data, reached through class_def.
    out = {}
    for i in range(cls_sz):
        cd = struct.unpack_from('<8I', b, cls_off + 32 * i)
        class_data_off = cd[6]
        if not class_data_off:
            continue
        o = class_data_off
        sf, o = uleb(b, o)
        inst_f, o = uleb(b, o)
        dm, o = uleb(b, o)
        vm, o = uleb(b, o)
        for _ in range(sf + inst_f):          # skip fields
            _, o = uleb(b, o)
            _, o = uleb(b, o)
        for kind, count in (('direct', dm), ('virtual', vm)):
            idx = 0
            for _ in range(count):
                d, o = uleb(b, o)
                flags, o = uleb(b, o)
                _code, o = uleb(b, o)
                idx += d
                if idx < len(methods):
                    cls, name, proto = methods[idx]
                    cls = cls[1:-1] if cls.startswith('L') else cls
                    out.setdefault((cls, name), (proto, bool(flags & ACC_STATIC)))
    return out


def cpp_type_to_jni(t, cpp_to_java):
    t = t.strip()
    t = re.sub(r'^(const|static)\s+', '', t).strip()
    if t in PRIMITIVES:
        return PRIMITIVES[t]
    m = re.match(r'std::shared_ptr<\s*(?:cordial::)?(\w+)\s*>', t)
    if m:
        name = m.group(1)
        if name == 'String':
            return 'Ljava/lang/String;'
        if name == 'Object':
            return 'Ljava/lang/Object;'
        if name == 'Class':
            return 'Ljava/lang/Class;'
        return 'L' + cpp_to_java.get(name, '?' + name) + ';'
    return '?' + t


def main():
    dexdir = pathlib.Path(sys.argv[1])
    dex = {}
    for f in sorted(dexdir.glob('*.dex')):
        for k, v in parse_dex(f).items():
            dex.setdefault(k, v)

    src = list(pathlib.Path('native').glob('*.cpp'))
    text = {p: p.read_text(errors='replace') for p in src}

    # C++ class -> Java name, from env->GetClass<Cpp>("java/name")
    cpp_to_java = {}
    for body in text.values():
        for m in re.finditer(r'GetClass<\s*(\w+)\s*>\s*\(\s*"([^"]+)"', body):
            cpp_to_java[m.group(1)] = m.group(2)

    findings, checked = [], 0
    for path, body in text.items():
        # Each Register() body names one Java class then hooks onto it.
        for reg in re.finditer(r'static void Register\s*\([^)]*\)\s*\{(.*?)\n    \}', body, re.S):
            blk = reg.group(1)
            jm = re.search(r'GetClass\s*(?:<\s*\w+\s*>)?\s*\(\s*"([^"]+)"', blk)
            if not jm:
                continue
            java = jm.group(1)
            for hm in re.finditer(
                    r'->(Hook|HookInstanceFunction)\s*\(\s*env\s*,\s*"([^"]+)"\s*,\s*&(\w+)::(\w+)', blk):
                kind, jname, cpp_cls, cpp_fn = hm.groups()
                key = (java, jname)
                if jname == '<init>':
                    # Constructors are hooked with `Hook` throughout this
                    # codebase -- libjnivm models them as static factories --
                    # so the dex's instance flag is not the right comparison
                    # and every one of them would report as a mismatch.
                    continue
                if key not in dex:
                    continue                      # not declared in this APK; nothing to check against
                want_desc, want_static = dex[key]
                checked += 1

                # Scoped to the owning class: two classes in one file can
                # declare the same method name, and searching the whole file
                # reported AudioRecord::startRecording's `void` against
                # WebRtcAudioRecord's descriptor.
                cls_m = re.search(r'\nclass\s+' + re.escape(cpp_cls) + r'\b.*?\n\};', body, re.S)
                scope = cls_m.group(0) if cls_m else body
                sig = re.search(
                    r'\n\s*(?:static\s+)?([\w:<>,\s\*&]+?)\s+' + re.escape(cpp_fn) + r'\s*\(([^)]*)\)',
                    scope)
                if not sig:
                    continue
                ret_cpp, params = sig.group(1), sig.group(2)
                parts = [p for p in re.split(r',(?![^<]*>)', params) if p.strip()]
                # ENV* and the receiver (Object*/Class*) are not JNI parameters.
                parts = [p for p in parts
                         if not re.search(r'\bENV\s*\*', p)
                         and not re.match(r'\s*(?:jnivm::)?(Object|Class)\s*\*', p.strip())]
                args = ''.join(cpp_type_to_jni(re.sub(r'\s*\w+\s*$', '', p) if not p.strip().endswith('*') else p,
                                               cpp_to_java) for p in parts)
                got_desc = '(' + args + ')' + cpp_type_to_jni(ret_cpp, cpp_to_java)
                got_static = (kind == 'Hook')

                if got_static != want_static:
                    findings.append((java, jname,
                                     f'registered {"static" if got_static else "instance"}, '
                                     f'dex says {"static" if want_static else "instance"}'))
                elif '?' not in got_desc and got_desc != want_desc:
                    findings.append((java, jname, f'descriptor {got_desc} != dex {want_desc}'))

    print(f'checked {checked} hooks that the dex declares\n')
    if not findings:
        print('no mismatches')
        return
    for java, name, why in sorted(findings):
        print(f'  {java}.{name}\n      {why}')
    print(f'\n{len(findings)} hook(s) that cannot bind')


main()
