# Patches against the vendored loader

`third_party/mcpelauncher-linker` and its own nested `bionic` are submodules
pointing at `github.com/minecraft-linux`, which this project cannot push to. A
change made inside them can be committed locally, but the submodule pointer would
then name a commit nobody else can fetch, and a fresh clone would fail. So
changes to the loader live here as patches until somebody decides to fork.

**These are not applied automatically.** Applying them is a build-system change
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
