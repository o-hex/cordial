#!/usr/bin/env python3
"""Generates the guest's OpenXR tables from Khronos's xr.xml, and the probe
the OpenXR layout-diff gate compiles (docs/vr/dynarmic-design.md §3.2, M6).

The Quest engine imports its core OpenXR commands from libopenxr_loader.so
and fetches the rest through xrGetInstanceProcAddr. Under dynarmic each one
is a stub whose arguments move from AAPCS64 registers into a SysV call to
the host's own loader, which needs every command's C signature -- the same
shape as gen-guest-vk.py, and generated the same way:

* every command's argument and return types;
* every struct member that holds a function pointer the host would call
  (a PFN_ member): the debug-utils callback, and XR_KHR_vulkan_enable2's
  pfnGetInstanceProcAddr, which is a guest stub the runtime must never be
  handed;
* every struct member that points at a Vulkan struct, since a
  VkInstanceCreateInfo or VkDeviceCreateInfo reached through OpenXR has to be
  translated exactly as one reached through Vulkan;
* `crates/cordial-runtime/src/guest_xr_probe.c`: sizeof, alignof, the type
  value and every member's offsetof for every struct openxr.h and
  openxr_platform.h declare with XR_USE_PLATFORM_ANDROID and
  XR_USE_GRAPHICS_API_VULKAN -- the two platform parts the Quest build uses
  -- compiled for aarch64-linux-android and x86_64-linux-gnu by the unit test
  `guest_xr::tests::layout_gate`, and here, which refuses to write the table
  if the two differ anywhere.

    tools/vr/gen-guest-xr.py xr.xml third_party/openxr/include /usr/include \\
        crates/cordial-runtime/src/guest_xr_table.rs crates/cordial-runtime/src/guest_xr_probe.c

xr.xml is Khronos's (OpenXR-SDK-Source, specification/registry/xr.xml,
Apache-2.0), at the tag matching the host's loader; pass the tag and commit
as XR_TAG and XR_COMMIT so the output records them. The second argument is
the matching OpenXR-SDK `include/` (openxr/*.h), the third the Vulkan
headers openxr_platform.h's Vulkan part needs.
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
# Types from other APIs that appear by value in a command. Vulkan handles
# are pointers or u64 on LP64 either way; the Android ones are pointers.
FOREIGN_SCALARS = {
    "VkInstance": "Ptr", "VkPhysicalDevice": "Ptr", "VkDevice": "Ptr", "VkImage": "U64",
    "VkFormat": "I32", "VkResult": "I32", "jobject": "Ptr",
}
# The platform parts the probe compiles and the stubs cover. Everything
# else (Win32, D3D, Metal, desktop GL, EGL, timespec) is left out: the Quest
# build has no use for them and the host loader answers them as absent.
DEFINES = ["XR_USE_PLATFORM_ANDROID", "XR_USE_GRAPHICS_API_VULKAN"]


def text_of(elem):
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
        for types in root.iter("types"):
            for t in types.findall("type"):
                name = t.get("name") or (t.find("name").text if t.find("name") is not None else None)
                if name:
                    self.types[name] = t
        self.commands = {}
        aliases = []
        for cmds in root.iter("commands"):
            for c in cmds.findall("command"):
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
        name = self.resolve(name)
        if name in C_SCALARS:
            return C_SCALARS[name]
        if name in FOREIGN_SCALARS:
            return FOREIGN_SCALARS[name]
        t = self.types.get(name)
        if t is None:
            raise ValueError(f"unknown type {name}")
        cat = t.get("category")
        body = text_of(t)
        if cat == "handle":
            return "U64"
        if cat == "enum":
            return "I32"
        if cat == "bitmask":
            return "U64"  # XrFlags64, every OpenXR bitmask
        if cat == "basetype":
            if "XR_DEFINE_ATOM" in body or "XR_DEFINE_OPAQUE_64" in body:
                return "U64"
            inner = t.find("type")
            if inner is None:
                raise ValueError(f"opaque basetype {name}")
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
            return "Ptr"
        if base.startswith("PFN_"):
            raise ValueError(f"callback type {base}")
        return self.scalar(base)

    def members(self, sname):
        return self.structs[sname].findall("member")


def ret_rs(c):
    return {None: "Ret::Void", "F32": "Ret::F32", "F64": "Ret::F64"}.get(c, f"Ret::Int({c})")


def preprocess(include, vk_include, target):
    """What openxr.h and openxr_platform.h declare under DEFINES."""
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "p.c")
        with open(src, "w") as f:
            f.write(probe_prelude())
        out = subprocess.run(["clang", f"--target={target}", "-ffreestanding", "-nostdlibinc", "-E", "-P",
                              "-I", include, "-I", vk_link(d, vk_include), src],
                             check=True, capture_output=True, text=True).stdout
    return out


def vk_link(d, vk_include):
    inc = os.path.join(d, "vkinc")
    os.makedirs(inc, exist_ok=True)
    for sub in ("vulkan", "vk_video"):
        p = os.path.join(inc, sub)
        if not os.path.exists(p):
            os.symlink(os.path.join(vk_include, sub), p)
    return inc


def probe_prelude():
    lines = ["#include <stddef.h>", "#include <stdint.h>"]
    lines += [f"#define {d}" for d in DEFINES]
    lines += [
        "// openxr_platform.h's Android part names these and expects the application",
        "// to have declared them. Both are pointers to opaque types on either side, so",
        "// declaring them here, without jni.h, cannot change a layout.",
        "typedef void* jobject;",
        "#include <vulkan/vulkan.h>",
        "#include <openxr/openxr.h>",
        "#include <openxr/openxr_platform.h>",
    ]
    return "\n".join(lines) + "\n"


def write_probe(reg, names, api_version, path):
    lines = [
        "// Generated by tools/vr/gen-guest-xr.py from xr.xml. Do not edit; rerun the script.",
        "//",
        "// The OpenXR layout-diff gate (docs/vr/dynarmic-design.md §3.2, M6): every",
        "// struct and union openxr.h and openxr_platform.h declare for Android and",
        "// Vulkan, compiled once as the Quest engine sees it (aarch64-linux-android)",
        "// and once as the host does (x86_64-linux-gnu). The two objects' data must be",
        "// byte-identical. S() is sizeof, alignof and the type value (all ones for a",
        "// struct without one); M() one member's offset.",
    ]
    lines += probe_prelude().splitlines()
    lines += [
        "",
        f"_Static_assert(XR_CURRENT_API_VERSION == {api_version}ull, \"the probe was generated from another xr.xml\");",
        "",
        "#define S(T, st) sizeof(T), _Alignof(T), (uint64_t)(st),",
        "#define M(T, m) offsetof(T, m),",
        "",
        "const uint64_t cordial_xr_probe[] = {",
    ]
    for n in names:
        members = reg.members(n)
        stype = next((m.get("values").split(",")[0] for m in members
                      if m.find("name").text == "type" and m.get("values")), None)
        lines.append(f"    S({n}, {stype or '~0ull'})")
        for m in members:
            lines.append(f"        M({n}, {m.find('name').text})")
    lines.append("    0,")
    lines.append("};")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


def elf_symbols(path):
    data = open(path, "rb").read()
    shoff, = st.unpack_from("<Q", data, 0x28)
    shentsize, shnum, shstrndx = st.unpack_from("<HHH", data, 0x3a)
    secs = [st.unpack_from("<IIQQQQIIQQ", data, shoff + i * shentsize) for i in range(shnum)]
    out = {}
    for s in secs:
        if s[1] != 2:
            continue
        strtab = secs[s[6]]
        for k in range(s[5] // 24):
            name_off, info, other, shndx, value, size = st.unpack_from("<IBBHQQ", data, s[4] + k * 24)
            if shndx == 0 or shndx >= 0xff00 or size == 0:
                continue
            end = data.index(b"\0", strtab[4] + name_off)
            name = data[strtab[4] + name_off:end].decode()
            sec = secs[shndx]
            out[name] = bytes(size) if sec[1] == 8 else data[sec[4] + value:sec[4] + value + size]
    return out


def compile_probe(probe, include, vk_include, target):
    with tempfile.TemporaryDirectory() as d:
        obj = os.path.join(d, "probe.o")
        subprocess.run(["clang", f"--target={target}", "-ffreestanding", "-nostdlibinc", "-I", include,
                        "-I", vk_link(d, vk_include), "-std=c11", "-w", "-c", probe, "-o", obj], check=True)
        return elf_symbols(obj)


def probe_labels(probe):
    words = []
    for line in open(probe):
        s = line.strip()
        m = re.match(r"S\((\w+), ", s)
        if m:
            words += [f"sizeof({m.group(1)})", f"alignof({m.group(1)})", f"type of {m.group(1)}"]
            continue
        m = re.match(r"M\((\w+), (\w+)\)", s)
        if m:
            words.append(f"offsetof({m.group(1)}, {m.group(2)})")
    return words + ["terminator"]


def main():
    xml, include, vk_include, table_out, probe_out = sys.argv[1:6]
    reg = Registry(ET.parse(xml).getroot())
    tag = os.environ.get("XR_TAG", "unrecorded")
    commit = os.environ.get("XR_COMMIT", "unrecorded")

    pre = preprocess(include, vk_include, "x86_64-linux-gnu")
    hdr = open(os.path.join(include, "openxr", "openxr.h")).read()
    mv = re.search(r"#define XR_CURRENT_API_VERSION XR_MAKE_VERSION\((\d+), (\d+), (\d+)\)", hdr)
    major, minor, patch = (int(x) for x in mv.groups())
    api_version = (major << 48) | (minor << 32) | patch

    declared = set(re.findall(r"typedef\s+(?:struct|union)\s+(\w+)\s*\{", pre))
    pfns = set(re.findall(r"\(\s*\*\s*PFN_(xr\w+)\)", pre))
    names = sorted(n for n in reg.structs if n in declared)
    missing = sorted(n for n in declared if n.startswith("Xr") and n not in reg.structs)
    if missing:
        sys.exit(f"gen-guest-xr: the headers declare structs xr.xml does not: {missing[:10]}")

    rows, refused = [], []
    for name in sorted(reg.commands):
        if name not in pfns:
            continue  # another platform's, or not in these headers
        cmd = reg.commands[name]
        try:
            proto = cmd.find("proto")
            rt = reg.classify(proto)
            args = [reg.classify(p) for p in cmd.findall("param")]
        except ValueError as e:
            refused.append((name, str(e)))
            continue
        rows.append((name, args, rt))

    # Function pointers the host would call, and pointers to Vulkan structs.
    pfn_members, vk_members = [], []
    for n in names:
        for mem in reg.members(n):
            t = text_of(mem)
            base = mem.find("type").text
            mname = mem.find("name").text
            if base.startswith("PFN_") and "*" not in t:
                pfn_members.append((n, mname, base))
            elif base.startswith("Vk") and "*" in t and base not in FOREIGN_SCALARS:
                vk_members.append((n, mname, base))

    callbacks = []
    for n, t in sorted(reg.types.items()):
        if t.get("category") != "funcpointer" or n == "PFN_xrVoidFunction":
            continue
        if not any(p == n for _, _, p in pfn_members):
            continue
        proto_text = text_of(t)
        mret = re.match(r"typedef\s+(\w+)\s*\(", proto_text)
        rtype = reg.scalar(mret.group(1)) if mret else None
        args = []
        params = re.search(r"\)\s*\((.*)\)\s*;", proto_text).group(1)
        for p in params.split(","):
            p = p.strip()
            if not p or p == "void":
                continue
            ptype = p.replace("const ", "").split()[0]
            args.append("Ptr" if "*" in p else reg.scalar(ptype))
        callbacks.append((n, args, rtype))

    write_probe(reg, names, api_version, probe_out)
    labels = probe_labels(probe_out)
    objs = {t: compile_probe(probe_out, include, vk_include, t)
            for t in ("aarch64-linux-android26", "x86_64-linux-gnu")}
    pa, px = objs["aarch64-linux-android26"]["cordial_xr_probe"], objs["x86_64-linux-gnu"]["cordial_xr_probe"]
    wa = st.unpack(f"<{len(pa)//8}Q", pa)
    wx = st.unpack(f"<{len(px)//8}Q", px)
    if len(wa) != len(wx) or len(wa) != len(labels):
        sys.exit(f"gen-guest-xr: probe lengths differ: {len(wa)} / {len(wx)} / {len(labels)} labels")
    diffs = [f"{lab}: aarch64 {va}, x86-64 {vx}" for lab, va, vx in zip(labels, wa, wx) if va != vx]
    for d in diffs:
        print(f"gen-guest-xr: LAYOUT DIFFERS {d}", file=sys.stderr)
    if diffs:
        sys.exit(f"gen-guest-xr: {len(diffs)} layout differences; not writing the table")
    print(f"gen-guest-xr: layout gate: {len(names)} types, {len(wa) - 1} words, 0 differences", file=sys.stderr)

    sizes, sval, offs = {}, {}, {}
    it = iter(zip(labels, wx))
    for lab, v in it:
        mm = re.match(r"sizeof\((\w+)\)", lab)
        if mm:
            sizes[mm.group(1)] = v
            next(it)
            _, stv = next(it)
            if stv != (1 << 64) - 1:
                sval[mm.group(1)] = stv
    for lab, v in zip(labels, wx):
        mm = re.match(r"offsetof\((\w+), (\w+)\)", lab)
        if mm:
            offs[(mm.group(1), mm.group(2))] = v

    for n, why in refused:
        print(f"gen-guest-xr: {n} left out: {why}", file=sys.stderr)
    print(f"gen-guest-xr: {len(rows)} commands, {len(refused)} left out, {len(pfn_members)} function-pointer "
          f"members, {len(vk_members)} Vulkan-struct pointers, {len(callbacks)} callback types", file=sys.stderr)

    o = []
    o.append(f"// Generated by tools/vr/gen-guest-xr.py from Khronos's xr.xml (OpenXR-SDK-Source {tag},")
    o.append(f"// {commit}; XR_CURRENT_API_VERSION {major}.{minor}.{patch}). Do not edit; rerun the script.")
    o.append("")
    o.append("use cordial_guest::{Ret, Ty::*};")
    o.append("")
    o.append(f"pub(crate) const API_VERSION: u64 = {api_version:#x};")
    o.append("")
    o.append("/// Every OpenXR command the Android and Vulkan headers declare: name,")
    o.append("/// argument types, return.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static XR: [(&str, &[cordial_guest::Ty], Ret); {len(rows)}] = [")
    for name, args, rt in rows:
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
    o.append("/// Callback types the host may call, as it will call them.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static CALLBACKS: [(&str, &[cordial_guest::Ty], Ret); {len(callbacks)}] = [")
    for n, a_, r in callbacks:
        o.append(f'    ("{n}", &[{", ".join(a_)}], {ret_rs(r)}),')
    o.append("];")
    o.append("")
    o.append("/// Function-pointer members: struct, its type value (or None), member")
    o.append("/// offset, member, callback type. Offsets from the x86-64 probe.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static PFN_MEMBERS: [(&str, Option<u32>, usize, &str, &str); {len(pfn_members)}] = [")
    for s, mname, pfn in pfn_members:
        sv = sval.get(s)
        o.append(f'    ("{s}", {"None" if sv is None else f"Some({sv})"}, {offs[(s, mname)]}, "{mname}", "{pfn}"),')
    o.append("];")
    o.append("")
    o.append("/// Members pointing at a Vulkan struct: struct, member offset, member, Vulkan type.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static VK_MEMBERS: [(&str, usize, &str, &str); {len(vk_members)}] = [")
    for s, mname, vt in vk_members:
        o.append(f'    ("{s}", {offs[(s, mname)]}, "{mname}", "{vt}"),')
    o.append("];")
    o.append("")
    srows = sorted((sval[s], s, sizes[s]) for s in sval)
    o.append("/// Every typed struct's x86-64 size (identical on aarch64, by the gate), by")
    o.append("/// type value: what copying one link of a `next` chain needs.")
    o.append("#[rustfmt::skip]")
    o.append(f"pub(crate) static TYPE_SIZES: [(u32, &str, usize); {len(srows)}] = [")
    for v, s, z in srows:
        o.append(f'    ({v}, "{s}", {z}),')
    o.append("];")
    o.append("")
    o.append("/// Member offsets, from the x86-64 probe, for every struct the bridge reads")
    o.append("/// a field of: struct, member, offset.")
    o.append("#[rustfmt::skip]")
    wanted = ["XrInstanceCreateInfo", "XrApplicationInfo", "XrFrameEndInfo", "XrCompositionLayerBaseHeader",
              "XrCompositionLayerProjection", "XrCompositionLayerProjectionView", "XrSwapchainSubImage",
              "XrRect2Di", "XrOffset2Di", "XrExtent2Di", "XrFrameState", "XrEventDataSessionStateChanged", "XrSwapchainCreateInfo",
              "XrGraphicsBindingVulkanKHR", "XrVulkanInstanceCreateInfoKHR", "XrVulkanDeviceCreateInfoKHR",
              "XrLoaderInitInfoAndroidKHR", "XrInstanceCreateInfoAndroidKHR", "XrSwapchainImageVulkanKHR",
              "XrEventDataBaseHeader", "XrSystemProperties", "XrSystemGraphicsProperties",
              "XrEventDataInstanceLossPending", "XrDebugUtilsMessengerCreateInfoEXT",
              # The input log (guest_xr_input.rs).
              "XrActionSetCreateInfo", "XrActionCreateInfo", "XrActionSpaceCreateInfo",
              "XrReferenceSpaceCreateInfo", "XrInteractionProfileSuggestedBinding", "XrActionSuggestedBinding",
              "XrActionStateGetInfo", "XrActionStateBoolean", "XrActionStateFloat", "XrActionStateVector2f",
              "XrActionStatePose", "XrSpaceLocation", "XrViewState", "XrInteractionProfileState",
              "XrEventDataInteractionProfileChanged", "XrActionsSyncInfo", "XrActiveActionSet",
              "XrHapticActionInfo", "XrHapticVibration"]
    orows = [(s, mm, v) for (s, mm), v in sorted(offs.items()) if s in wanted]
    o.append(f"pub(crate) static OFFSETS: [(&str, &str, usize); {len(orows)}] = [")
    for s, mm, v in orows:
        o.append(f'    ("{s}", "{mm}", {v}),')
    o.append("];")
    with open(table_out, "w") as f:
        f.write("\n".join(o) + "\n")


if __name__ == "__main__":
    main()
