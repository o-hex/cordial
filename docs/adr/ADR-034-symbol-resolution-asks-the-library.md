# ADR-034: Symbol resolution asks the library, not a checked-in list

## Status

Accepted, 2026-09-13. Closes [#15](https://github.com/luohoa97/cordial/issues/15).

## Context

`crates/cordial-runtime/build.rs` reads `docs/analysis/undefined-symbols.tsv`
and generates one stub function per line, plus a `SYMBOLS` array pairing each
name with its stub. `symtab::build` then iterated exactly that array: every
symbol Cordial classified, resolved and registered came from the file.

So the file was not a record of what the engine imports. It was the *gate*. A
symbol absent from it was never classified, never registered, and the
`DT_NEEDED` walk failed before any window appeared.

That has now cost two releases. `hypotf` appeared in Roblox 2.734.0.917 and
`getpwuid_r` in 2.738.0.1393; both are ordinary C library functions that every
host has had for decades, and both stopped the client dead until somebody
hand-added a line to a TSV. Nothing about either was a genuine gap in Cordial.
The gap was that resolution was reading the wrong source of truth.

Measured on this host, 2026-09-13, against `libroblox.so` of 2.738.0.1393: the
library imports 573 distinct symbols, 565 strongly and 8 weakly. The TSV
currently lists 656 names across eleven consumer libraries and covers all 573.
So the file is not wrong today -- it is simply always one Roblox update away
from being wrong, with no warning and a total failure as the symptom.

## Decision

**`symtab::build` takes the set of symbols the engine actually imports**, read
out of the ELF in `--lib-dir` by a new `cordial_runtime::elf`, and resolves the
union of that set and `SYMBOLS`.

Three consequences, each chosen deliberately:

**A discovered symbol is resolved from the host on the same rules as any
other.** Generic goes to the host candidates and then to libc when
`--host-libc` is set; Khronos goes to the host's GLES or EGL; Cordial's own
`bionic`/`android` overrides still win over everything.

**A discovered symbol nothing can answer is not given a stub.** There is no
generated stub to give it -- stubs are generated per TSV line and a discovered
symbol has none -- and manufacturing one would mean a shared function returning
zero for a symbol nobody has looked at. That is the failure
[`AGENTS.md`](../../AGENTS.md) names as never making a stub lie, and it is what
the issue asked to preserve: the load fails, the linker names the symbol, and
`load.rs` prints what class of thing it was first. Silently answering it is how
the next `hypotf` becomes a mystery instead of an error.

**Weak imports are distinguished from strong ones.** `libroblox.so` imports
`__gcov_dump`, `__gcov_flush`, `getentropy`, `__cxa_thread_atexit_impl` and
four `ZSTD_trace_*` hooks weakly; those resolve to zero and the load proceeds.
Reporting them as "the load will fail" would be a false alarm on every launch,
which is the fastest way to teach everyone to ignore the one that matters.

**The TSV stays**, with a smaller job: it generates the stubs, and it is what
gives each stub its own address so a hit names the symbol. It is no longer
consulted to decide whether a symbol exists.

## The `libdl` exception

`dlopen`, `dlsym`, `dlclose`, `dlerror`, `dladdr`, `dl_iterate_phdr` and
`dlvsym` are skipped, and this is load-bearing rather than tidiness. glibc
exports all seven, so a host lookup *succeeds* -- and taking it would hand the
guest our loader. The engine's `dlopen("libvulkan.so")` would then ask glibc
for a library only the bionic linker knows about and get nothing, with no error
pointing anywhere near the cause. `build.rs` has always skipped the same names,
so they were never in `SYMBOLS`; discovery has to skip them too or it undoes
that silently. The list is duplicated in `symtab.rs` because a build script
cannot import from the crate it builds.

## Section headers, not `PT_DYNAMIC`

`elf.rs` finds `.dynsym` through the section header table, so the entry count
is `sh_size / sh_entsize` and is exact. Walking `DT_SYMTAB` instead means
recovering the count from `DT_HASH`'s `nchain` or from `DT_GNU_HASH`'s bucket
chains, which is more code and more ways to be quietly wrong. An object with
its section headers stripped is an error naming that, never an empty set: a
silent empty answer is indistinguishable from a build that added nothing, which
would put this straight back where it started.

## The alternative, and why not

mocktail sidesteps the class entirely: `stubs/CMakeLists.txt` builds an empty
`.so` per library and force-links the real host one into it with
`LINKER:--no-as-needed`, so every symbol libm or libz has ever exported is
present whether anything asked for it or not. It is a good idea and it is where
this one came from -- mocktail is Apache-2.0 and the credit is owed.

It was not taken because it answers a different question. Force-linking makes a
*library* wholesale available; Cordial resolves per symbol, and it has to,
because `libc.so` is deliberately curated -- bionic and glibc disagree on
`struct stat`, `pthread_mutex_t`, `DIR`, `FILE`, `sigset_t`, `struct statvfs`
and `struct mallinfo`, and passing those through unchanged overruns the
caller's object. A mechanism that could not distinguish "libm, all of it" from
"libc, these five entry points and no others" would either give up the curation
or need a second mechanism beside it. Reading the imports keeps one rule for
every library and leaves the per-symbol judgement exactly where it already was.

## Evidence

Measured on this host, 2026-09-13, release build, same tree, sequential runs:

* `engine imports: 573 symbols` -- agrees exactly with
  `readelf --dyn-syms -W | awk '$7=="UND"' | sed 's/@.*//' | sort -u | wc -l`.
* **Control.** With the `hypotf` line deleted from the TSV and this change
  stashed: `LOAD FAILED after 26ms: dlopen failed: cannot locate symbol
  "hypotf"`, exit 1. That is issue #15, reproduced.
* **Test.** Same deleted line, this change applied: `note: hypotf is not in
  docs/analysis/undefined-symbols.tsv; resolved from the host under libm.so`,
  then `LOADED in 98ms`, the game loaded and flags loaded, exit 0.
* **The loud failure survives.** With `AMediaCodec_configure` deleted instead --
  an Android API no host has -- the run printed `1 symbol the engine imports
  cannot be answered by Cordial, the host, or a stub: AMediaCodec_configure --
  an Android API belonging to libmediandk.so`, then failed by name, exit 1.
* **Silent when the list is current.** Unmodified tree: 650 stubs, 573 imports,
  `LOADED in 33ms`, and neither a `note:` nor an unprovidable line.

## Consequences

A Roblox update that adds a libc, libm, libz, GLES or EGL import no longer
breaks the client, and the log says which symbol arrived and where it came
from. One that adds an Android API still fails, which is correct -- that is a
real gap and wants
[`roblox_update.yml`](../../.github/ISSUE_TEMPLATE/roblox_update.yml).

Discovered names are `Box::leak`ed, because `Entry::symbol` is `&'static str`
and the table is built once at startup and handed to the linker, which copies
the names into `std::string` keys of its own. The leak is bounded by how many
imports an update adds, which has been one at a time.

`docs/analysis/undefined-symbols.tsv` should still be regenerated when a build
changes, so the stub table keeps naming symbols individually. It is now
housekeeping rather than a release blocker.
