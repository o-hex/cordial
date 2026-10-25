package.nix: draft of pkgs/by-name/co/cordial/package.nix for nixpkgs.
It is not submitted anywhere. Written 2026-10-01.

What was run
  Built with nixpkgs b4fd65b198c5 (the revision in the cordial repo's flake.lock)
  as `callPackage ./package.nix { }`, on x86_64-linux, in a store under
  $HOME because /nix is read-only on the developer's host. src hash and
  cargoHash in the file are the ones that build reported. Not run: a game, a rebuild after the final one-line fix that adds LICENSE to the installed notices (the other 24 files matched the build),
  `cargo test`, nixpkgs-review, nixfmt, nixpkgs-hammer.

What reviewers will ask

1. Submodules. third_party/mcpelauncher-linker (which has two nested
   submodules, bionic and core, both from github.com/minecraft-linux) and
   third_party/libjnivm (github.com/ChristopherHX/libjnivm) are git submodules,
   and the build panics without them. fetchFromGitHub is used with
   fetchSubmodules = true, which is recursive; the src hash covers all of them.
   They are pinned by the commit the v0.23.2 tag records. A reviewer may prefer
   them as separate fetchFromGitHub inputs with the tree assembled in postUnpack;
   upstream has no preference recorded.

2. The vendored C/C++ build. crates/cordial-linker-sys/build.rs compiles the
   ported AOSP bionic linker, libjnivm and native/ with CMake, and all of it is
   statically linked into the two Rust binaries (the generated stub count is
   printed as a cargo warning; that is normal). AOSP bionic does not compile with GCC
   (C11 _Atomic in C++ headers), so the package uses clangStdenv via
   `buildRustPackage.override { stdenv = clangStdenv; }`. build.rs also passes
   the literal names `clang` and `clang++` to CMake. Whether the override alone
   puts those on PATH was not tested separately; the flake lists `clang` in
   nativeBuildInputs as well and this draft does not. It built, so it works
   here. native/CMakeLists.txt probes for pipewire, ALSA, PulseAudio and
   WebKitGTK headers and silently compiles an "unavailable" stub when they are
   missing, so they are buildInputs and not optional. `buildFeatures` enables
   the web view in both crates, and postInstall fails the build if
   cordial-run does not link WebKitGTK.

3. Licence. Cordial is GPL-3.0-or-later (Cargo.toml, LICENSE). Statically
   linked third-party code: mcpelauncher-linker, libjnivm and libbadcpu are MIT,
   third_party/mcpelauncher-linker/core carries an AOSP NOTICE, and
   third_party/mocktail-webview is Apache-2.0 (see NOTICE and
   THIRD-PARTY-NOTICES.md in the repository). meta.license lists gpl3Plus, mit
   and asl20; a reviewer may want it narrowed, or the AOSP code
   (see third_party/mcpelauncher-linker/core/NOTICE) spelled out. The notices are installed under
   share/licenses/cordial. NOTICE says libbadcpu may not reach a shipped binary
   ("UNVERIFIED" in the file itself); it is installed anyway.

4. No Roblox code. The package contains none and the build fetches none: the
   only network access is the crates and the git submodules above, in
   fixed-output derivations. THE INSTALLED PROGRAM CAN DOWNLOAD ROBLOX'S
   ANDROID APK AT RUN TIME, from a third-party mirror, when the user asks it to
   on first launch; it installs it only if Roblox's own signing certificate
   signed it. It can instead use a copy Sober unpacked, or one the user points
   it at. The package therefore ships no Roblox code, but the program is a way
   to obtain and run it. Say so in the PR rather than leaving a reviewer to
   find it. Roblox's terms of service are not something this note assesses.

5. Other questions to expect.
   - Maintainer: meta.maintainers is empty. A nixpkgs maintainer is needed.
     The upstream author has not agreed to be one.
   - AI-assisted code: upstream's docs/install.md says the project contains a
     great deal of AI-generated or AI-assisted code and records it in
     Co-Authored-By trailers. Check nixpkgs' current policy before submitting.
   - Tests: doCheck = false. Upstream skips three tests that need a session
     bus; the others were not run here.
   - Plugins: first-party plugins are Deno programs and Cordial bundles no
     runtime, so deno is put on the wrapper PATH.
   - Version stamp: observed from this build, with no .git and no
     CORDIAL_GIT_SHA, `cordial --help` and `--diagnostics` print the bare
     version ("0.23.2"). The cordial repo's own flake instead passes the git
     revision through CORDIAL_GIT_SHA; this package does not.
   - `cordial --diagnostics` reports "Install unknown" under Nix, because
     nothing in crates/cordial-shell recognises a /nix/store path.
   - platforms is x86_64-linux only: the engine is Roblox's x86-64 Android
     build, run without emulation.
   - The wrapper puts vulkan-loader, pipewire, libpulseaudio and alsa-lib on
     LD_LIBRARY_PATH because they are dlopen()ed. Nothing tested confirms that
     audio or the GPU path works from this package.
