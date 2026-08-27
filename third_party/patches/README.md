# Local changes to vendored third-party source

`third_party/mcpelauncher-linker` and the `bionic` tree nested inside it are
other people's repositories. Changes to them cannot be committed as a submodule
pointer bump, because there is nowhere to push the commit the pointer would
name. They live here instead, as patches, and this file exists because the
alternative was what was actually happening: **132 lines of local edits to the
AOSP linker sitting uncommitted in a working tree for an unknown length of
time, in the three files `crates/cordial-linker-sys/build.rs` compiles.**

That is the failure mode AGENTS.md keeps returning to, in a new shape. The
edits were behaviourally inert -- every one is gated on an environment variable
that defaults off -- so a release built from a fresh checkout would have run
identically and nobody would have noticed anything was missing. What would have
been lost is the instrument, and with it the ability to reproduce any
measurement taken using it.

## Applying them

    git -C third_party/mcpelauncher-linker/bionic apply \
        ../../patches/0001-cordial-linker-tracing.patch

Nothing applies these automatically. A build without them is a correct build
and differs only in that the trace variables below do nothing.

## What is in them

`0001-cordial-linker-tracing.patch` adds two traces to the guest linker, both
off unless asked for:

- `CORDIAL_TRACE_DLSYM=1` prints every `dlopen` and every `dlsym` the guest
  makes through the virtual `libdl.so`, symbol names unfiltered. Unfiltered on
  purpose: deciding in advance which names are interesting is the guessing the
  trace exists to replace. It is what established that the `mi_option_*`
  strings in `libroblox.so` are mimalloc option names for its own log line
  rather than symbols anything resolves.
- `CORDIAL_TRACE_DLADDR=1` prints every `dladdr` and `dl_iterate_phdr`. Those
  are the two ways guest code can ask the linker where it is loaded from, and
  `docs/analysis/flag-init.md` §31 asks whether the engine finds its private
  data directory by walking up from its own library path the way an Android app
  does. `/proc/self/maps`, the third route, is a plain file read and already
  visible under `CORDIAL_TRACE_PATHS=1`, where it has never once appeared.
