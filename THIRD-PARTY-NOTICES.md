# Third-party notices

Cordial as a whole is GPL-3.0-or-later (see [`LICENSE`](LICENSE)). It
incorporates the components below, each under its own licence, and each of those
licences requires its notice to travel with the software — in source *and* in
binary form. This file exists so that obligation is met by anyone redistributing
Cordial, including from the Flatpak, which installs this file to
`/app/share/licenses/cordial/`.

MIT and Apache-2.0 are both compatible with GPL-3.0 in this direction: their
terms are satisfied while the combined work is offered under the GPL. Preserving
these notices is a condition of that, not a courtesy.

---

## libbadcpu — MIT

x86-64 CPU *feature* emulator. Vendored at
[`third_party/libbadcpu/`](third_party/libbadcpu), from
[`Z3ki/sober-oss`](https://github.com/Z3ki/sober-oss) at commit
`e48a905efdffa1ad49a3ebb873895bcff73aa935`. Cordial vendors only
`src/libbadcpu/`, `include/badcpu.h` and the test.

Upstream licence text is preserved verbatim at
[`third_party/libbadcpu/LICENSE.upstream`](third_party/libbadcpu/LICENSE.upstream).

```
MIT License

Copyright (c) 2026 Sober OSS Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## mcpelauncher-linker — MIT

The host wrapper around the AOSP bionic linker. Submodule at
[`third_party/mcpelauncher-linker/`](third_party/mcpelauncher-linker), from
[`minecraft-linux/mcpelauncher-linker`](https://github.com/minecraft-linux/mcpelauncher-linker).

```
MIT License

Copyright (c) 2024 ChristopherHX and MCMrARM
```

Full text: `third_party/mcpelauncher-linker/LICENSE`.

---

## Android Open Source Project (bionic) — Apache-2.0 and BSD

The bionic linker and headers carried inside `mcpelauncher-linker`. The NOTICE
file required by Apache-2.0 §4(d) is preserved at
`third_party/mcpelauncher-linker/core/NOTICE`.

```
Android Code
Copyright 2005-2008 The Android Open Source Project
```

Portions of bionic are under BSD licences from the original authors, whose
notices are preserved in the individual source files as required.

---

## libjnivm — MIT

The JNI virtual machine that stands in for Android's. Submodule at
[`third_party/libjnivm/`](third_party/libjnivm), from
[`ChristopherHX/libjnivm`](https://github.com/ChristopherHX/libjnivm).

```
MIT License

Copyright (c) 2019 ChristopherHX
```

Full text: `third_party/libjnivm/LICENSE`.

---

## mocktail — Apache-2.0

Both vendored and read, which is why this entry is longer than the others.

**Vendored.** [`third_party/mocktail-webview/`](third_party/mocktail-webview)
holds unmodified copies of mocktail's implementation of Roblox's web-view
protocol, kept as the reference for Cordial's own web window. They are not
compiled — nothing in the build reads that directory.

```
Copyright 2026 komaruworld

Licensed under the Apache License, Version 2.0
```

Full text: `third_party/mocktail-webview/LICENSE`.

**Derived and adapted.** Cordial's `crates/cordial-shell/src/webview_policy.rs`
is derived from mocktail's `webview_helper_policy.cc`;
`crates/cordial-runtime/src/permissions.rs` follows the discovery pattern in its
`roblox_permissions_bridge.cc`; and the performance tables in
`crates/cordial-shell/src/shell_config.rs` are adapted from its own. Those are
adaptations of Apache-2.0 work and are named as such in each file.

**Read, and the reason several things here are right rather than guessed.**
The field order of Roblox's `NativeTextBoxInfo` — including which slot carries
`textWrapped` and the `xAlign`/`yAlign` pair — came from mocktail's constructor
rather than from a stripped binary. So did several thread-count and pipeline
flag values, the platform identity string Cordial reports, and a number of
engine behaviours confirmed by watching mocktail run against the same build.
`docs/analysis/flag-init.md` cites it throughout.

This project reads mocktail deliberately and says so. The line it holds is that
ideas, call orders, field layouts and documented shapes may be taken with
credit, and implementations may not be transcribed — see
[CLAUDE.md](CLAUDE.md).

---

## Android Game Development Kit — Apache-2.0

Not vendored. AGDK's `GameActivity` source was **read** to get the
`onTouchEventNative` argument packing and the surface/lifecycle callback
contract right rather than guessing at them. No AGDK code is copied into this
repository; the reference is recorded here because it is the reason those
signatures are correct.

---

## What is *not* here

**Roblox.** Cordial contains no Roblox code, APK, asset or decompiled material,
and never will. It loads the official Android client that the user supplies from
their own installation. Roblox is a trademark of Roblox Corporation, which does
not endorse this project.
