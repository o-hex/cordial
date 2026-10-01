#!/usr/bin/env python3
"""Generates crates/cordial-runtime/src/guest_gl_table.rs from Khronos's
gl.xml and egl.xml.

Under dynarmic the arm64 engine's EGL and GLES calls go to the host's real
libEGL and libGLESv2 (docs/vr/dynarmic-design.md §3.2, §9.3), each through a
stub that moves its arguments from AAPCS64 registers into a SysV call and so
needs the command's C signature. The engine imports about a hundred of them
and reaches the rest through eglGetProcAddress, so the table covers every EGL
command and every command the GLES 2/3 API or a GLES extension names. Types
the call builder cannot move -- a function pointer the host would call back
(GLDEBUGPROC and the blob-cache callbacks), or anything else not classified
below -- leave the command out, and the script says which on stderr: a
command not in the table stops when imported and is not handed out by
eglGetProcAddress.

    tools/vr/gen-guest-gl.py gl.xml egl.xml > crates/cordial-runtime/src/guest_gl_table.rs

The XML is Khronos's (OpenGL-Registry and EGL-Registry on GitHub, Apache-2.0);
pass the commits it came from as GL_REGISTRY_COMMIT and EGL_REGISTRY_COMMIT so
the header records them.
"""
import os
import sys
import xml.etree.ElementTree as ET

INT = {
    # GL
    "GLenum": "U32", "GLuint": "U32", "GLint": "I32", "GLsizei": "I32", "GLbitfield": "U32",
    "GLboolean": "U8", "GLbyte": "I8", "GLubyte": "U8", "GLshort": "I16", "GLushort": "U16",
    "GLfixed": "I32", "GLclampx": "I32", "GLhalf": "U16", "GLhalfNV": "U16", "GLchar": "I8",
    "GLintptr": "I64", "GLsizeiptr": "I64", "GLintptrARB": "I64", "GLsizeiptrARB": "I64",
    "GLint64": "I64", "GLuint64": "U64", "GLint64EXT": "I64", "GLuint64EXT": "U64",
    "GLsync": "Ptr", "GLeglImageOES": "Ptr", "GLeglClientBufferEXT": "Ptr", "GLhandleARB": "U32",
    "GLvdpauSurfaceNV": "I64",
    # EGL
    "EGLint": "I32", "EGLBoolean": "U32", "EGLenum": "U32", "EGLAttrib": "I64", "EGLAttribKHR": "I64",
    "EGLTime": "U64", "EGLTimeKHR": "U64", "EGLTimeNV": "U64", "EGLuint64KHR": "U64",
    "EGLuint64NV": "U64", "EGLnsecsANDROID": "I64", "EGLsizeiANDROID": "I64",
    "EGLNativeFileDescriptorKHR": "I32",
    "EGLDisplay": "Ptr", "EGLSurface": "Ptr", "EGLContext": "Ptr", "EGLConfig": "Ptr",
    "EGLClientBuffer": "Ptr", "EGLImage": "Ptr", "EGLImageKHR": "Ptr", "EGLSync": "Ptr",
    "EGLSyncKHR": "Ptr", "EGLSyncNV": "Ptr", "EGLStreamKHR": "Ptr", "EGLDeviceEXT": "Ptr",
    "EGLOutputLayerEXT": "Ptr", "EGLOutputPortEXT": "Ptr", "EGLLabelKHR": "Ptr",
    "EGLObjectKHR": "Ptr", "EGLNativeDisplayType": "Ptr", "EGLNativeWindowType": "Ptr",
    "EGLNativePixmapType": "Ptr", "EGLClientPixmapHI": "Ptr",
    # Android's EGLNativeWindowType is an ANativeWindow*; the Cordial override
    # of eglCreateWindowSurface substitutes the real window either way.
    "int": "I32", "void": None,
}
FLOAT = {"GLfloat": "F32", "GLclampf": "F32", "GLdouble": "F64", "GLclampd": "F64"}
# Function pointers the host would call back into guest code.
CALLBACKS = {"GLDEBUGPROC", "GLDEBUGPROCKHR", "GLDEBUGPROCARB", "GLDEBUGPROCAMD", "GLVULKANPROCNV",
             "EGLSetBlobFuncANDROID", "EGLGetBlobFuncANDROID", "EGLDEBUGPROCKHR",
             "__eglMustCastToProperFunctionPointerType", "__GLXextFuncPtr"}


def ctype(elem):
    """The C type text of a <proto> or <param>, without its name."""
    parts = [elem.text or ""]
    for child in elem:
        if child.tag == "name":
            parts.append(child.tail or "")
            continue
        parts.append((child.text or "") + (child.tail or ""))
    return " ".join("".join(parts).split())


def base_type(elem):
    p = elem.find("ptype")
    return p.text if p is not None else None


def classify(elem):
    t = ctype(elem)
    b = base_type(elem)
    if b in CALLBACKS:
        raise ValueError(f"callback type {b}")
    if "*" in t:
        return "Ptr"
    key = b or t.replace("const", "").strip()
    if key in FLOAT:
        return FLOAT[key]
    if key in INT:
        return INT[key]
    raise ValueError(f"unclassified type {t!r}")


def ret(c):
    return {None: "Ret::Void", "F32": "Ret::F32", "F64": "Ret::F64"}.get(c, f"Ret::Int({c})")


def commands(root):
    out = {}
    for cmd in root.findall("commands/command"):
        proto = cmd.find("proto")
        name = proto.find("name").text
        out[name] = cmd
    return out


def gles_names(root):
    names = set()
    for f in root.iter("feature"):
        if f.get("api") == "gles2":
            for r in f.iter("require"):
                if r.get("api") not in (None, "gles2"):
                    continue
                names.update(c.get("name") for c in r.iter("command"))
    for e in root.iter("extension"):
        if "gles2" in (e.get("supported") or "").split("|"):
            for r in e.iter("require"):
                if r.get("api") not in (None, "gles2"):
                    continue
                names.update(c.get("name") for c in r.iter("command"))
    return names


def main():
    gl_root = ET.parse(sys.argv[1]).getroot()
    egl_root = ET.parse(sys.argv[2]).getroot()
    gl_cmds = commands(gl_root)
    egl_cmds = commands(egl_root)
    wanted = [(n, gl_cmds[n]) for n in sorted(gles_names(gl_root))]
    wanted += [(n, c) for n, c in sorted(egl_cmds.items())]
    rows, refused = [], []
    for name, cmd in wanted:
        try:
            r = classify(cmd.find("proto")) if name != "eglGetProcAddress" else "Ptr"
            args = [classify(p) for p in cmd.findall("param")]
            if None in args:
                args = [a for a in args if a is not None]
        except ValueError as e:
            refused.append((name, str(e)))
            continue
        rows.append((name, args, r))
    for n, why in refused:
        print(f"gen-guest-gl: {n} left out: {why}", file=sys.stderr)
    print(f"gen-guest-gl: {len(rows)} commands, {len(refused)} left out", file=sys.stderr)
    gl_commit = os.environ.get("GL_REGISTRY_COMMIT", "unrecorded")
    egl_commit = os.environ.get("EGL_REGISTRY_COMMIT", "unrecorded")
    print("// Generated by tools/vr/gen-guest-gl.py from Khronos's gl.xml (OpenGL-Registry")
    print(f"// {gl_commit}) and egl.xml (EGL-Registry {egl_commit}). Do not edit; rerun the")
    print("// script.")
    print()
    print("use cordial_guest::{Ret, Ty::*};")
    print()
    print("/// Every EGL command, and every command GLES 2/3 or a GLES extension names,")
    print("/// whose arguments the call builder can move: name, argument types, return.")
    print("#[rustfmt::skip]")
    print(f"pub(crate) static GL: [(&str, &[cordial_guest::Ty], Ret); {len(rows)}] = [")
    for name, args, r in rows:
        print(f'    ("{name}", &[{", ".join(args)}], {ret(r)}),')
    print("];")
    print()
    print("/// Left out, with why: the call builder cannot move these arguments.")
    print(f"pub(crate) static REFUSED: [(&str, &str); {len(refused)}] = [")
    for n, why in refused:
        print(f'    ("{n}", "{why}"),')
    print("];")


if __name__ == "__main__":
    main()
