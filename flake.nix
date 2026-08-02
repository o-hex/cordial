{
  # A shell that can BUILD Cordial. Nothing more.
  #
  # This exists because the build quietly compiles different code depending on
  # what happens to be installed: `native/CMakeLists.txt` probes for
  # libpipewire-0.3 and libspa-0.2 and omits the audio backend when they are
  # absent, and WebKitGTK will be the same story. Two people on the same commit
  # can therefore produce binaries with different features and neither has any
  # way to tell. For a project whose whole method is "verify by running", a
  # measurement is worth what you can say about the thing you measured, so the
  # toolchain being pinned matters more here than it would elsewhere.
  #
  # It is also the difference between a shell and a reboot on an immutable base
  # like Fedora Silverblue, where `dnf install` means layering onto the host
  # image for one project's build dependency.
  #
  # Deliberately out of scope:
  #
  #   * Running the client. Testing is done by running the binary on the host —
  #     `just dev` — because the engine's behaviour depends on the host's real
  #     graphics stack, compositor and glibc, and `--host-libc` makes that
  #     explicit rather than incidental. A hermetic runtime would be measuring
  #     something nobody ships.
  #   * Shipping. Users install the Flatpak. This is a contributor's shell.
  #   * Roblox. Cordial ships no Roblox code and never will. This pins the
  #     toolchain, not the input; you still supply an APK yourself, and
  #     CONTRIBUTING.md explains the least fiddly way to obtain one.

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in {
        devShells.default = pkgs.mkShell {
          # Clang, not GCC, and this is not a preference: the vendored AOSP
          # bionic linker does not build with GCC at all.
          stdenv = pkgs.clangStdenv;

          nativeBuildInputs = with pkgs; [
            clang
            cmake
            pkg-config
            rustc
            cargo
            just
          ];

          buildInputs = with pkgs; [
            # gtk4-sys and libadwaita-sys link against the system libraries
            # rather than vendoring them, so these have to be present to link at
            # all, not merely to run. ADR-011 and ADR-002.
            gtk4
            libadwaita
            glib
            gdk-pixbuf
            cairo
            pango
            graphene
            wayland
            libxkbcommon

            # Optional at build time, and that is exactly the problem this shell
            # solves: without them the tree still compiles, it just silently
            # loses the audio backend and the web views. Including them means
            # everyone builds the same Cordial.
            pipewire
            webkitgtk_6_0
          ];

          shellHook = ''
            echo "cordial: clang $(clang --version | head -1 | grep -o '[0-9.]*' | head -1), rust $(rustc --version | cut -d' ' -f2)"
            echo "  pipewire  $(pkg-config --modversion libpipewire-0.3 2>/dev/null || echo MISSING)"
            echo "  webkitgtk $(pkg-config --modversion webkitgtk-6.0 2>/dev/null || echo MISSING)"
            echo "  gtk4      $(pkg-config --modversion gtk4 2>/dev/null || echo MISSING)"
            echo "  libadwaita $(pkg-config --modversion libadwaita-1 2>/dev/null || echo MISSING)"
            echo
            echo "This shell builds Cordial. Run it on the host: just dev"
          '';
        };
      });
}
