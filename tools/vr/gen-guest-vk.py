#!/usr/bin/env python3
"""Generates the guest's Vulkan tables from Khronos's vk.xml, and the probe
the layout-diff gate compiles (docs/vr/dynarmic-design.md §3.2, M5).

Under dynarmic every Vulkan command the arm64 engine reaches through
vkGetInstanceProcAddr/vkGetDeviceProcAddr is handed back as a stub whose
arguments move from AAPCS64 registers into a SysV call, and that needs each
command's C signature -- the same shape as gen-guest-gl.py. Three things are
Vulkan's own and are generated alongside:

* the command parameters that carry a function pointer the host would call:
  `pAllocator` (VkAllocationCallbacks) directly, and any `const T*` whose
  struct, or a struct that may extend it through pNext, holds a PFN member
  (VkDebugUtilsMessengerCreateInfoEXT in a VkInstanceCreateInfo chain);
* the callback signatures (funcpointer types), for host-to-guest entries;
* `crates/cordial-runtime/src/guest_vk_probe.c`: sizeof, alignof, the sType
  value and every member's offsetof for every struct and union the engine
  could hand the host, plus a byte image per bitfield member. Compiled for
  aarch64-linux-android and x86_64-linux-gnu by the unit test
  `guest_vk::tests::layout_gate`, which diffs the two; and compiled here,
  for x86-64, to fill in the sizes and PFN member offsets the runtime needs
  to copy a pNext chain. The test checks those numbers against the headers
  too, so the table cannot drift from what it was generated from.

    tools/vr/gen-guest-vk.py vk.xml /usr/include \\
        crates/cordial-runtime/src/guest_vk_table.rs crates/cordial-runtime/src/guest_vk_probe.c

vk.xml is Khronos's (Vulkan-Headers, registry/vk.xml, Apache-2.0), at the tag
matching the host's installed headers (VK_HEADER_VERSION); pass the tag and
commit as VK_HEADERS_TAG and VK_HEADERS_COMMIT so the output records them.
The generator refuses to write if the two targets' layouts differ anywhere,
listing each difference: that is the gate, and the unit test re-runs it.

The idea of generating guest-arch Vulkan proxies from vk.xml is Berberis's
(AOSP, android_api/libvulkan, Apache-2.0); nothing of its code is used here.
"""
import os
import re
import struct as st
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

C_SCALARS = {
    "void": None, "char": "I8", "int8_t": "I8", "uint8_t": "U8", "int16_t": "I16", "uint16_t": "U16",
    "int32_t": "I32", "uint32_t": "U32", "int64_t": "I64", "uint64_t": "U64", "size_t": "U64",
    "int": "I32", "float": "F32", "double": "F64",
}
# Platform types a non-pointer parameter can have. Only the ones that are
# plain integers or pointers on LP64 Linux are listed; anything else refuses.
PLATFORM_SCALARS = {
    "VisualID": "U64", "Window": "U64", "RROutput": "U64", "xcb_window_t": "U32",
    "xcb_visualid_t": "U32", "zx_handle_t": "U32", "GgpStreamDescriptor": "U32",
    "GgpFrameToken": "U64", "HANDLE": "Ptr", "HINSTANCE": "Ptr", "HWND": "Ptr", "HMONITOR": "Ptr",
    "DWORD": "U32", "LPCWSTR": "Ptr", "MTLDevice_id": "Ptr", "MTLCommandQueue_id": "Ptr",
    "MTLBuffer_id": "Ptr", "MTLTexture_id": "Ptr", "MTLSharedEvent_id": "Ptr", "IOSurfaceRef": "Ptr",
    "_screen_buffer": None,
}
# Platforms whose header the probe includes. Android is the guest's own;
# Wayland is what Cordial's native layer turns the Android surface into.
PROBE_PLATFORMS = {None, "android", "wayland"}


def api_ok(e):
    a = e.get("api")
    return a is None or "vulkan" in a.split(",")


def text_of(elem):
    """The C text of a <proto>, <param> or <member>, name included."""
    parts = [elem.text or ""]
    for child in elem:
        if child.tag == "comment":
            parts.append(child.tail or "")
            continue
        parts.append((child.text or "") + (child.tail or ""))
    return " ".join("".join(parts).split())


class Registry:
    def __init__(self, root):
        self.root = root
        self.types = {}
        for t in root.find("types").findall("type"):
            if not api_ok(t):
                continue
            name = t.get("name") or (t.find("name").text if t.find("name") is not None else None)
            if name is None and t.find("proto") is not None:
                name = t.find("proto").find("name").text
            if name:
                self.types[name] = t
        self.enum_bits = {}
        for e in root.findall("enums"):
            if e.get("name"):
                self.enum_bits[e.get("name")] = int(e.get("bitwidth", "32"))
        self.commands = {}
        aliases = []
        for c in root.find("commands").findall("command"):
            if not api_ok(c):
                continue
            if c.get("alias"):
                aliases.append((c.get("name"), c.get("alias")))
                continue
            self.commands[c.find("proto").find("name").text] = c
        for name, target in aliases:
            if target in self.commands:
                self.commands[name] = self.commands[target]
        self.structs = {n: t for n, t in self.types.items()
                        if t.get("category") in ("struct", "union") and not t.get("alias")}

    def resolve(self, name):
        seen = set()
        while name in self.types and self.types[name].get("alias") and name not in seen:
            seen.add(name)
            name = self.types[name].get("alias")
        return name

    def scalar(self, name):
        """The Ty of a by-value parameter of type `name`, or raise."""
        name = self.resolve(name)
        if name in C_SCALARS:
            return C_SCALARS[name]
        if name in PLATFORM_SCALARS:
            if PLATFORM_SCALARS[name] is None:
                raise ValueError(f"by-value platform type {name}")
            return PLATFORM_SCALARS[name]
        t = self.types.get(name)
        if t is None:
            raise ValueError(f"unknown type {name}")
        cat = t.get("category")
        if cat == "handle":
            kind = t.find("type").text
            return "Ptr" if kind == "VK_DEFINE_HANDLE" else "U64"
        if cat == "enum":
            return "U64" if self.enum_bits.get(name, 32) == 64 else "I32"
        if cat == "bitmask":
            base = t.find("type").text
            return "U64" if base == "VkFlags64" else "U32"
        if cat == "basetype":
            inner = t.find("type")
            if inner is None:
                raise ValueError(f"opaque basetype {name}")
            if "*" in text_of(t):
                return "Ptr"
            return self.scalar(inner.text)
        if cat in ("struct", "union"):
            raise ValueError(f"by-value {cat} {name}")
        if cat == "funcpointer":
            raise ValueError(f"callback type {name}")
        raise ValueError(f"unclassified type {name} ({cat})")

    def classify(self, elem):
        t = text_of(elem)
        base = elem.find("type").text
        if "*" in t or "[" in t:
            if self.types.get(self.resolve(base), ET.Element("x")).get("category") == "funcpointer" \
                    and t.count("*") == 0:
                raise ValueError(f"callback type {base}")
            return "Ptr"
        return self.scalar(base)

    def members(self, sname):
        return [m for m in self.structs[sname].findall("member") if api_ok(m)]

    def pfn_members(self, sname):
        out = []
        for m in self.members(sname):
            b = self.resolve(m.find("type").text)
            if self.types.get(b) is not None and self.types[b].get("category") == "funcpointer" \
                    and "*" not in text_of(m):
                out.append((m.find("name").text, b))
        return out

    def extenders(self):
        ext = {}
        for n, t in self.structs.items():
            for parent in (t.get("structextends") or "").split(","):
                if parent:
                    ext.setdefault(self.resolve(parent), set()).add(n)
        return ext


def funcpointer_sig(reg, t):
    proto = t.find("proto")
    rtext = text_of(proto)
    rbase = proto.find("type").text
    if "*" in rtext:
        ret = "Ptr"
    else:
        ret = reg.scalar(rbase) if rbase != "PFN_vkVoidFunction" else "Ptr"
    args = [reg.classify(p) for p in t.findall("param")]
    return args, ret


def ret_rs(c):
    return {None: "Ret::Void", "F32": "Ret::F32", "F64": "Ret::F64"}.get(c, f"Ret::Int({c})")


def probe_types(reg):
    """Structs and unions the headers the probe includes declare."""
    wanted = set()
    for f in reg.root.findall("feature"):
        if not api_ok(f):
            continue
        for r in f.iter("require"):
            if api_ok(r):
                wanted.update(t.get("name") for t in r.iter("type"))
    for e in reg.root.find("extensions").findall("extension"):
        if "vulkan" not in (e.get("supported") or "").split(","):
            continue
        if e.get("platform") not in PROBE_PLATFORMS or e.get("provisional") == "true":
            continue
        for r in e.iter("require"):
            if api_ok(r):
                wanted.update(t.get("name") for t in r.iter("type"))
    # Structs named only as members of wanted structs are declared too.
    frontier = list(wanted)
    while frontier:
        n = reg.resolve(frontier.pop())
        if n in reg.structs:
            for m in reg.members(n):
                b = m.find("type").text
                if b not in wanted:
                    wanted.add(b)
                    frontier.append(b)
    return sorted(n for n in {reg.resolve(w) for w in wanted if w} if n in reg.structs)


def write_probe(reg, names, header_version, path):
    lines = [
        "// Generated by tools/vr/gen-guest-vk.py from vk.xml. Do not edit; rerun the script.",
        "//",
        "// The layout-diff gate (docs/vr/dynarmic-design.md §3.2, M5): every struct and",
        "// union below, compiled once as the Quest engine sees it (aarch64-linux-android)",
        "// and once as the host does (x86_64-linux-gnu). The two objects' data must be",
        "// byte-identical. S() is sizeof, alignof and the sType value (all ones for a",
        "// struct without one); M() one member's offset; BF() one bitfield member set",
        "// to all ones in an otherwise zero object, since a bitfield has no offsetof.",
        "#include <stddef.h>",
        "#include <stdint.h>",
        "#include <vulkan/vulkan_core.h>",
        "#include <vulkan/vulkan_android.h>",
        "#include <vulkan/vulkan_wayland.h>",
        "",
        f"_Static_assert(VK_HEADER_VERSION == {header_version}, \"the probe was generated from vk.xml {header_version}\");",
        "",
        "#define S(T, st) sizeof(T), _Alignof(T), (uint64_t)(st),",
        "#define M(T, m) offsetof(T, m),",
        "#define BF(i, T, m, w) const union { T s; unsigned char b[sizeof(T)]; } cordial_vk_bf_##i = "
        "{ .s = { .m = (1ull << (w)) - 1 } };",
        "",
        "const uint64_t cordial_vk_probe[] = {",
    ]
    bitfields = []
    for n in names:
        members = reg.members(n)
        stype = next((m.get("values").split(",")[0] for m in members
                      if m.find("name").text == "sType" and m.get("values")), None)
        lines.append(f"    S({n}, {stype or '~0ull'})")
        for m in members:
            mt = text_of(m)
            mname = m.find("name").text
            bf = re.search(r"\b" + re.escape(mname) + r"\s*:\s*(\d+)", mt)
            if bf:
                bitfields.append((n, mname, int(bf.group(1))))
            else:
                lines.append(f"        M({n}, {mname})")
    lines.append("    0,")
    lines.append("};")
    lines.append("")
    for i, (n, m, w) in enumerate(bitfields):
        lines.append(f"BF({i}, {n}, {m}, {w})")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


def elf_symbols(path):
    """{name: bytes} of every defined data symbol in an ELF64 LE relocatable."""
    data = open(path, "rb").read()
    shoff, = st.unpack_from("<Q", data, 0x28)
    shentsize, shnum, shstrndx = st.unpack_from("<HHH", data, 0x3a)
    secs = [st.unpack_from("<IIQQQQIIQQ", data, shoff + i * shentsize) for i in range(shnum)]
    out = {}
    for s in secs:
        if s[1] != 2:  # SHT_SYMTAB
            continue
        strtab = secs[s[6]]
        for k in range(s[5] // 24):
            name_off, info, other, shndx, value, size = st.unpack_from("<IBBHQQ", data, s[4] + k * 24)
            if shndx == 0 or shndx >= 0xff00 or size == 0:
                continue
            end = data.index(b"\0", strtab[4] + name_off)
            name = data[strtab[4] + name_off:end].decode()
            sec = secs[shndx]
            if sec[1] == 8:  # SHT_NOBITS
                out[name] = bytes(size)
            else:
                out[name] = data[sec[4] + value:sec[4] + value + size]
    return out


def compile_probe(probe, include, target):
    with tempfile.TemporaryDirectory() as d:
        inc = os.path.join(d, "inc")
        os.mkdir(inc)
        for sub in ("vulkan", "vk_video"):
            os.symlink(os.path.join(include, sub), os.path.join(inc, sub))
        obj = os.path.join(d, "probe.o")
        subprocess.run(["clang", f"--target={target}", "-ffreestanding", "-nostdlibinc", "-I", inc,
                        "-std=c11", "-w", "-c", probe, "-o", obj], check=True)
        return elf_symbols(obj)


def probe_labels(probe):
    """What each word of cordial_vk_probe and each cordial_vk_bf_N is."""
    words, bfs = [], []
    for line in open(probe):
        s = line.strip()
        m = re.match(r"S\((\w+), ", s)
        if m:
            words += [f"sizeof({m.group(1)})", f"alignof({m.group(1)})", f"sType of {m.group(1)}"]
            continue
        m = re.match(r"M\((\w+), (\w+)\)", s)
        if m:
            words.append(f"offsetof({m.group(1)}, {m.group(2)})")
            continue
        m = re.match(r"BF\((\d+), (\w+), (\w+), (\d+)\)", s)
        if m:
            bfs.append(f"{m.group(2)}.{m.group(3)}:{m.group(4)}")
    return words + ["terminator"], bfs


def main():
    xml, include, table_out, probe_out = sys.argv[1:5]
    reg = Registry(ET.parse(xml).getroot())
    hv = None
    for t in reg.root.find("types").findall("type"):
        n = t.find("name")
        if n is not None and n.text == "VK_HEADER_VERSION" and api_ok(t):
            hv = int(n.tail.split()[0])
    tag = os.environ.get("VK_HEADERS_TAG", "unrecorded")
    commit = os.environ.get("VK_HEADERS_COMMIT", "unrecorded")

    # Commands.
    rows, refused, alloc_args = [], [], []
    for name in sorted(reg.commands):
        cmd = reg.commands[name]
        try:
            proto = cmd.find("proto")
            rt = "Ptr" if proto.find("type").text == "PFN_vkVoidFunction" else reg.classify(proto)
            params = [p for p in cmd.findall("param") if api_ok(p)]
            args = [reg.classify(p) for p in params]
        except ValueError as e:
            refused.append((name, str(e)))
            continue
        rows.append((name, args, rt, params))

    # What may carry a function pointer the host would call, and how.
    ext = reg.extenders()
    direct = {n for n in reg.structs if reg.pfn_members(n)}
    # Reaching a PFN only through a pointer member (VkDirectDriverLoadingListLUNARG
    # -> pDrivers): the runtime refuses these by name rather than walking them.
    via_member = set()
    changed = True
    while changed:
        changed = False
        for n in reg.structs:
            if n in direct or n in via_member:
                continue
            for m in reg.members(n):
                b = reg.resolve(m.find("type").text)
                if m.find("name").text == "pNext":
                    continue
                if b in direct or b in via_member:
                    via_member.add(n)
                    changed = True
                    break
    carrying = direct | via_member

    def may_carry(sname):
        sname = reg.resolve(sname)
        return sname in carrying or any(e in carrying for e in ext.get(sname, ()))

    chain_args = []
    for name, args, rt, params in rows:
        for i, p in enumerate(params):
            b = reg.resolve(p.find("type").text)
            t = text_of(p)
            if b == "VkAllocationCallbacks":
                alloc_args.append((name, i))
            elif b in reg.structs and "*" in t and may_carry(b):
                if not t.startswith("const"):
                    refused.append((name, f"writes a {b} that may hold a function pointer"))
                elif p.get("len"):
                    chain_args.append((name, i, b, True))
                else:
                    chain_args.append((name, i, b, False))
    refused_names = {n for n, _ in refused}
    rows = [r for r in rows if r[0] not in refused_names]

    callbacks = []
    for n, t in sorted(reg.types.items()):
        if t.get("category") == "funcpointer" and n not in ("PFN_vkVoidFunction",):
            try:
                a, r = funcpointer_sig(reg, t)
                callbacks.append((n, a, r))
            except ValueError as e:
                print(f"gen-guest-vk: callback {n} left out: {e}", file=sys.stderr)

    # The probe, and the gate over it.
    names = probe_types(reg)
    write_probe(reg, names, hv, probe_out)
    labels, bf_labels = probe_labels(probe_out)
    objs = {t: compile_probe(probe_out, include, t) for t in ("aarch64-linux-android26", "x86_64-linux-gnu")}
    a, x = objs["aarch64-linux-android26"], objs["x86_64-linux-gnu"]
    diffs = []
    pa, px = a["cordial_vk_probe"], x["cordial_vk_probe"]
    wa = st.unpack(f"<{len(pa)//8}Q", pa)
    wx = st.unpack(f"<{len(px)//8}Q", px)
    if len(wa) != len(wx) or len(wa) != len(labels):
        sys.exit(f"gen-guest-vk: probe lengths differ: {len(wa)} / {len(wx)} / {len(labels)} labels")
    for lab, va, vx in zip(labels, wa, wx):
        if va != vx:
            diffs.append(f"{lab}: aarch64 {va}, x86-64 {vx}")
    for i, lab in enumerate(bf_labels):
        if a[f"cordial_vk_bf_{i}"] != x[f"cordial_vk_bf_{i}"]:
            diffs.append(f"bitfield {lab}: aarch64 {a[f'cordial_vk_bf_{i}'].hex()}, x86-64 {x[f'cordial_vk_bf_{i}'].hex()}")
    for d in diffs:
        print(f"gen-guest-vk: LAYOUT DIFFERS {d}", file=sys.stderr)
    if diffs:
        sys.exit(f"gen-guest-vk: {len(diffs)} layout differences; not writing the table")
    print(f"gen-guest-vk: layout gate: {len(names)} types, {len(wa) - 1} words, {len(bf_labels)} bitfields, "
          f"0 differences", file=sys.stderr)

    # x86-64 sizes of every sType-bearing struct, and offsets of PFN members.
    it = iter(zip(labels, wx))
    sizes, sval = {}, {}
    for lab, v in it:
        m = re.match(r"sizeof\((\w+)\)", lab)
        if m:
            sname = m.group(1)
            sizes[sname] = v
            next(it)
            _, stv = next(it)
            if stv != (1 << 64) - 1:
                sval[sname] = stv
    offs = {}
    for lab, v in zip(labels, wx):
        m = re.match(r"offsetof\((\w+), (\w+)\)", lab)
        if m:
            offs[(m.group(1), m.group(2))] = v

    for n, why in refused:
        print(f"gen-guest-vk: {n} left out: {why}", file=sys.stderr)
    print(f"gen-guest-vk: {len(rows)} commands, {len(refused)} left out, {len(alloc_args)} pAllocator "
          f"arguments, {len(chain_args)} chain arguments, {len(callbacks)} callback types", file=sys.stderr)

    o = []
    o.append(f"// Generated by tools/vr/gen-guest-vk.py from Khronos's vk.xml (Vulkan-Headers {tag},")
    o.append(f"// {commit}; VK_HEADER_VERSION {hv}). Do not edit; rerun the script.")
    o.append("")
    o.append("use cordial_guest::{Ret, Ty::*};")
    o.append("")
    o.append(f"pub(crate) const HEADER_VERSION: u32 = {hv};")
    o.append("")
    o.append("/// Every Vulkan command whose arguments the call builder can move: name,")
    o.append("/// argument types, return. Aliases carry their target's signature.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static VK: [(&str, &[cordial_guest::Ty], Ret); {len(rows)}] = [")
    for name, args, rt, _ in rows:
        o.append(f'    ("{name}", &[{", ".join(args)}], {ret_rs(rt)}),')
    o.append("];")
    o.append("")
    o.append("/// Left out, with why.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static REFUSED: [(&str, &str); {len(refused)}] = [")
    for n, why in refused:
        o.append(f'    ("{n}", "{why}"),')
    o.append("];")
    o.append("")
    o.append("/// Commands taking `const VkAllocationCallbacks*`, and which argument.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static ALLOCATOR_ARG: [(&str, usize); {len(alloc_args)}] = [")
    for n, i in alloc_args:
        o.append(f'    ("{n}", {i}),')
    o.append("];")
    o.append("")
    o.append("/// Commands with a `const T*` whose struct, or a pNext extension of it, may")
    o.append("/// hold a function pointer: command, argument, struct, whether it is an array.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static CHAIN_ARGS: [(&str, usize, &str, bool); {len(chain_args)}] = [")
    for n, i, s, arr in chain_args:
        o.append(f'    ("{n}", {i}, "{s}", {"true" if arr else "false"}),')
    o.append("];")
    o.append("")
    o.append("/// Callback types, as the host will call them: argument types, return.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static CALLBACKS: [(&str, &[cordial_guest::Ty], Ret); {len(callbacks)}] = [")
    for n, a_, r in callbacks:
        o.append(f'    ("{n}", &[{", ".join(a_)}], {ret_rs(r)}),')
    o.append("];")
    o.append("")
    pfn_rows = []
    for s in sorted(direct):
        for m, pfn in reg.pfn_members(s):
            if (s, m) in offs:
                pfn_rows.append((s, sval.get(s), offs[(s, m)], m, pfn))
    o.append("/// Function-pointer members, from the x86-64 probe: struct, its sType (or")
    o.append("/// None), member offset, member, callback type.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static PFN_MEMBERS: [(&str, Option<u32>, usize, &str, &str); {len(pfn_rows)}] = [")
    for s, sv, off, m, pfn in pfn_rows:
        svs = "None" if sv is None else f"Some({sv})"
        o.append(f'    ("{s}", {svs}, {off}, "{m}", "{pfn}"),')
    o.append("];")
    o.append("")
    vm = sorted((s, sval.get(s)) for s in via_member if s in sizes)
    o.append("/// Structs that reach a function pointer only through a pointer member; a")
    o.append("/// chain holding one is refused by name.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static PFN_VIA_MEMBER: [(&str, Option<u32>); {len(vm)}] = [")
    for s, sv in vm:
        o.append(f'    ("{s}", {"None" if sv is None else f"Some({sv})"}),')
    o.append("];")
    o.append("")
    srows = sorted((sval[s], s, sizes[s]) for s in sval)
    o.append("/// Every sType-bearing struct's x86-64 size (identical on aarch64, by the gate),")
    o.append("/// by sType value: what copying one link of a pNext chain needs.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static STYPE_SIZES: [(u32, &str, usize); {len(srows)}] = [")
    for v, s, z in srows:
        o.append(f'    ({v}, "{s}", {z}),')
    o.append("];")
    with open(table_out, "w") as f:
        f.write("\n".join(o) + "\n")


if __name__ == "__main__":
    main()
