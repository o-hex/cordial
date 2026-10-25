# Patches against the vendored loader

`third_party/mcpelauncher-linker` and its own nested `bionic` are submodules
pointing at `github.com/minecraft-linux`, which this project cannot push to. A
change made inside them can be committed locally, but the submodule pointer would
then name a commit nobody else can fetch, and a fresh clone would fail. So
changes to the loader live here as patches until somebody decides to fork.

**What is at stake is the instrument, not the behaviour.** 0002, 0003 and 0004
are gated on environment variables that default off, and 0001 only tightens a
mapping, so a release built from a fresh checkout behaves the same whether or
not any of them is applied. What a lost working tree destroys is the ability to
reproduce a measurement. That nearly happened: 132 lines of edits to the three
files `crates/cordial-linker-sys/build.rs` compiles sat uncommitted for an
unknown length of time before anything captured them.

A second copy of those same 132 lines used to live at `third_party/patches/`,
as one undifferentiated blob with a README describing only the two traces in it
and not the mapping change or the constructor split it also carried. It has been
removed: its content is exactly these four patches, checked line for line before
it went.

**0001 to 0004 are not applied automatically.** Applying them is a build-system change
nobody has made yet, and a patch that silently applies is worse than one that
does not: `crates/cordial-linker-sys/build.rs` did not watch the loader sources
until recently, so a loader change that failed to take produced a binary that
behaved exactly as though it had never been written. Apply by hand:

```bash
git -C third_party/mcpelauncher-linker/bionic apply ../../../patches/0001-map-engine-text-read-only.patch
```

## 0001 — map the engine's text read-only

`ElfReader::LoadSegments` mapped every `PT_LOAD` with `prot | PROT_WRITE`,
unconditionally, regardless of the segment's own `p_flags`, and nothing ever took
the write bit off again: `_phdr_table_set_load_prot` has its body wrapped in
`#if 0`, and both callers of `phdr_table_protect_segments` are inside
`#if !defined(__LP64__)`. So on x86-64 the engine's 106 MB of text sat `rwxp` for
the life of the process.

That matters because [ADR-001](../docs/adr/ADR-001-in-process-hooking.md) makes
in-process hooking *absent* rather than disabled, precisely so that a fork cannot
extract the primitive — and a fork is currently building a script executor on
Cordial. Writable text hands that fork a foothold that does not even need an
`mprotect` first. Sealing it does not make patching impossible; it stops Cordial
shipping the capability pre-armed.

Checked before changing it: `libroblox.so` has no `TEXTREL`/`DF_TEXTREL` and no
`.rela.dyn`; its only relocations are 546 `.rela.plt` entries, all landing in the
writable data segment. `libbadcpu`'s SIGILL emulator only reads instruction
operands and never writes into the faulting code.

Verified with `tools/engine-text-diff.py`, before and after, on the same build:

    before   7f99b8500000-7f99befa9000 rwxp    DIFFERING BYTES: 0
    after    7feb80500000-7feb86fa9000 r-xp    DIFFERING BYTES: 0

and the client still reaches `app ready` with no crash.

## 0002 — trace guest `dlopen`/`dlsym`

Inert unless `CORDIAL_TRACE_DLSYM=1`, matching the existing `CORDIAL_TRACE_DLOPEN`
convention. It exists because a question about what the engine looks up at runtime
had been answered by inference twice, and this answers it by observation.

What it established, over three identical runs to `app ready`: the engine makes
exactly five `dlopen` calls (`libc.so`, `libcamera2ndk.so`, `libmediandk.so`,
`libvulkan.so.1`, `libandroid.so`) and seven `dlsym` calls (`getauxval`,
`vkGetInstanceProcAddr`, five `AThermal_*`). **Nothing mimalloc-shaped is ever
looked up** — see `crates/cordial-runtime/src/mimalloc_lib.rs` for why that
matters and what it rules out.

## 0003 — split-phase `dlopen`, for testing whether libroblox.so's own
## constructors can be deferred past Cordial's directory setup

Inert unless something calls the two new exports it adds
(`mcpelauncher_defer_next_ctors`, `mcpelauncher_run_deferred_ctors`) — nothing
in the default load path does; `cordial-run` only reaches them behind
`CORDIAL_DEFER_CTORS=1` / `CORDIAL_DEFER_PAST_SETTINGS=1` in
`crates/cordial-runtime/src/bin/load.rs`. Exists to test the question
`docs/analysis/flag-init.md` §26.1 leaves open: can `RbxStorage::init`'s
constructor-time call be pushed past the point where Cordial has told the
engine anything, and does that change its outcome.

Splits `do_dlopen`'s existing two steps — `find_library` (map, relocate) then
`si->call_constructors()` — so a caller can run the first, do its own setup
against the mapped-and-relocated (but not yet constructed) object, then
explicitly trigger the second. `soinfo::call_constructors()` is already
idempotent (guarded by `constructors_called`), so `mcpelauncher_run_
deferred_ctors` calling it late is exactly as safe as bionic's own recursive
calls into it.

**What it established, §27 of `docs/analysis/flag-init.md` has the full
record:** deferring past Cordial's four `NativeSettingsInterface` directory
setters is coherent — no crash, three plain runs and one lldb-instrumented
run, all clean — but changes nothing: `RbxStorage::init`'s empty-path failure
reproduces identically, meaning the directories were never the missing input.
Deferring further, past `nativeInitClientSettings`, is **not** coherent: it
segfaults deterministically (fault address `0x10`, a null-pointer-shaped
dereference, 2/2 plain runs plus a captured backtrace confirming the fault is
inside the native itself, not the calling code) — that native depends on
state only libroblox.so's own constructors set up, so Android's actual
ordering (settings before storage) cannot be reproduced this way from
outside the engine.

## 0004 — an Android-shaped library path, and tracing `dladdr`/`dl_iterate_phdr`

Two additions, both inert unless something calls the new export or sets the
new trace variable. `linker.cpp` gets `mcpelauncher_set_realpath`, which
overwrites a loaded library's `soinfo::realpath_` — pure metadata, no
reopening, no remapping — callable between `mcpelauncher_defer_next_ctors`
and `mcpelauncher_run_deferred_ctors` from patch 0003, so the override is in
place before any constructor-time code that asks the linker "what is my own
path". `libdl.cpp` gets `CORDIAL_TRACE_DLADDR=1`, tracing every `dladdr()`
call with its argument and result, and every `dl_iterate_phdr()` call (call
site only — the per-entry `dlpi_name` goes to the caller's own callback,
which this does not intercept).

Exists to test `docs/analysis/flag-init.md` §31: whether the engine derives
its private data directory by walking up from its own library path the way
an Android app locates `/data/user/0/<pkg>` from `/data/app/<pkg>/lib/<abi>/`.
`crates/cordial-runtime/src/bin/load.rs` wires both together behind
`CORDIAL_ANDROID_LIBPATH=1`: defer `libroblox.so`'s constructors, override its
realpath to an Android-shaped `/data/app/~~.../com.roblox.client-.../lib/
x86_64/libroblox.so`, then run the deferred constructors.

**What it established, §32 of `docs/analysis/flag-init.md` has the full
record: the hypothesis is wrong, and not for lack of trying the right lever.**
`dladdr()` is called **zero** times across two complete 25-second runs to
`app ready: Landing` — the engine never asks the linker this question at all.
`dl_iterate_phdr()` is called, but its first invocation in either run comes
strictly *after* `RbxStorage::init`'s three failing `stat("")` calls, in a
burst of about twenty back-to-back calls consistent with C++ exception
unwinding walking a stack, not with computing a directory beforehand — and
`CORDIAL_TRACE_PATHS=1` shows zero reads of `/proc/self/maps` or
`/proc/self/exe` anywhere in either run, the third route named in the
hypothesis. With the override applied and constructors demonstrably run under
it (no crash, two clean repeats), the failing `stat("")` triple is
byte-for-byte identical to the unmodified baseline. None of the three ways
native code can ask the linker "where am I" are used before, during, or in
place of the failure.

## 0005 — parse `DT_RELR` and `DT_ANDROID_RELR` again

**Unlike the four above, this one changes behaviour, and a checkout without it
cannot load a RELR-packed library.** The port wraps the three RELR cases in
`soinfo::prelink_image` in `#if 0` while keeping `relocate_relr` and its call in
`linker_relocate.cpp`, so a library whose relative relocations are packed that
way loads with every one of them left as a bare file offset. Nothing reports
it; the linker prints `unused DT entry: unknown OS-specific (type 0x6fffe000)`
as a warning and carries on.

No phone build has needed it. The Quest build's `libovrplatformloader.so` does:
its single `.init_array` entry is stored as `0xcec64` and covered by its
`.relr.dyn`, and under qemu-aarch64 the load died with `SIGSEGV` at `si_addr`
`0xcec64` exactly, deterministically across two runs with ASLR off. With the
cases compiled back in, the same load reached `LOADED` and `JNI_OnLoad returned
0x10006` on three runs out of three.

So it is applied by every build: `crates/cordial-linker-sys/build.rs`
compiles the loader from an overlay of the submodule under `OUT_DIR`, built
by `patches/apply.rs`, in which the files 0005 and 0006 touch are patched
copies and everything else is a symlink to the submodule. The submodule
itself is never written, so a build leaves `git status` clean and the window
title free of a `-dirty` the build caused. A patch already in the submodule
(applied by hand) is built as it is; one that neither applies nor is present
stops the build by name. That used to be `tools/vr/build-aarch64.sh`'s job
alone, applying in place, so a plain `cargo build` produced a client that
could not link the Quest build.

## 0006 — link objects built for another instruction set

Behaviour, like 0005, and applied by the same build script after it (it is made
against a tree with 0005 in). It lets the x86-64 client link the Quest build's
arm64 `libroblox.so` for the dynarmic translator (docs/vr/dynarmic-design.md
§2, milestone M2), and is inert until `mcpelauncher_set_guest_machine` is
called with a non-zero machine, which only `cordial-run --guest-arm64` does.

One linker instance serves both kinds of object rather than a second compiled
copy. The loader's state is process-global -- the solist, the namespaces, the
handle table, `dl_iterate_phdr`'s list, the lock -- and a second copy would need
every one of its external symbols renamed and would not know about the first
one's objects. What actually differs by instruction set is small and belongs
to the object, not the loader, so it is a field on `soinfo`:

- `ElfReader` accepts `EM_AARCH64` besides the host's machine, and records it.
- A guest object's four relocation types that write a 64-bit address (ABS64,
  GLOB_DAT, JUMP_SLOT, RELATIVE, plus NONE) are translated to the host's
  generic numbers and applied by the unchanged code. Anything else is refused
  by name: IRELATIVE would have the host call a guest resolver.
- Once a guest machine is set, each object binds only to libraries of its own
  kind. A virtual library is marked guest when its addresses are guest stubs.
  Without this, `dlopen` in the engine's imports resolved against the linker's
  own host `libdl.so`, which is in its `DT_NEEDED`.
- A guest object's constructors are never called by the host; they are left
  for the translator, and `mcpelauncher_guest_ctors` says what they are.
- Guest segments are mapped without `PROT_EXEC`: the translator only reads
  them, and a stray host jump then faults instead of running arm64 bytes.
- Each guest object logs its relocations by type, which is how the load is
  checked against `llvm-readelf -r`.

Verified on the Quest build 2.740.927: the counts logged equal `llvm-readelf -r`
type for type (RELATIVE 572,841, ABS64 22, GLOB_DAT 56, JUMP_SLOT 598), three
runs out of three, and the native x86-64 load of the phone build produced the
same relocation trace, type and symbol, line for line, with and without it.

## 0007 — dynarmic: keep flag-setting logical ops out of constant folding

Not the loader: this one is against `third_party/dynarmic`, which points at a
repository this project cannot push to either, and `crates/cordial-guest/build.rs`
applies it the same way as 0005 and 0006, on x86-64 only, since nothing else
compiles dynarmic. Behaviour, and a translator bug fix
rather than anything about the engine.

The first run of the Quest build's constructors under the translator
(docs/vr/dynarmic-design.md M3) aborted inside dynarmic with
`assertion failed: value.GetInst()->MayGetNZCVFromOp()`, while translating a
block at libroblox+0x1e6214c that ends `ands x12, x12, x13; b.eq`, where x13 is
all ones. `ANDS` emits an `And64` with a `GetNZCVFromOp` pseudo-op attached;
the constant-propagation pass folds `x & ~0` to `x`, which re-points the
pseudo-op at the shift that produced `x`, and the pseudo-op's argument check
fires. `FoldAdd` and `FoldSub` already leave an op with a pseudo-op alone;
`FoldAND`, `FoldEOR`, `FoldOR` and `FoldNOT` did not. The patch gives them the
same guard. With it the block translates and all 3,622 constructors run.

## 0008 — dynarmic: return-stack and fast-dispatch hits at each site

Against `third_party/dynarmic` like 0007, applied by the same call in
`crates/cordial-guest/build.rs`. Performance only; it changes no guest-visible
result.

dynarmic ends every block that finishes in a guest `RET` by jumping to one
shared handler that pops its return-stack buffer, and every `BR`/`BLR` by
jumping to one shared fast-dispatch handler. Each handler then ends in a host
indirect jump, so all guest returns share one host branch and all indirect
calls another, and the host predictor has only global history to tell their
targets apart. In game those two handlers were 6--10 % of the frame threads'
samples (docs/vr/dynarmic-design.md §9.8). The patch emits each handler's hit
path inline at the site and keeps the shared code for the misses, entered with
the same registers, and deepens the ring from 8 entries to 32, since a C++
engine's call chains between returns routinely run deeper than eight. Measured
in §9.9: fewer host branch misses on `RBX Worker C`, and a frame-rate effect
inside the noise of the runs taken.

## 0003, for timing

`CORDIAL_TIME_CTORS=1` on the native path splits `dlopen`'s time into linking and
constructors, which is the qemu half of M3's comparison, and needs 0003's
deferral. It refuses to run without it rather than report a constructor time of
zero. 0003 was applied by hand for that measurement and taken out again.
