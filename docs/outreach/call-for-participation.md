# Issues offered for This Week in Rust's Call for Participation

Drafts, not filed. Each is scoped so somebody who has never seen this repository
can finish it, which is the actual bar for that section — an issue that needs a
conversation before it can be started will be skipped.

House rules that apply to every one of them, and which the issue text should say
rather than assume: **verify by running, and never state a result you did not
observe.** This project has retracted more conclusions than most have made.

---

## 1. `GameActivity.getWaterfallInsets` has the wrong JNI descriptor

**Good first issue. One line, and a tool already proves it.**

libjnivm binds hooks by name *and* descriptor, and a mismatch fails silently in
both directions — the hook registers and is simply never called. This project has
found four separate instances of that bug.

`tools/hook_descriptors.py ~/.cache/cordial-dex/` checks every hook against the
shipping dex and currently prints:

```
checked 108 hooks that the dex declares
  com/google/androidgamesdk/GameActivity.getWaterfallInsets
      descriptor ()Ljava/lang/Object; != dex ()Landroidx/core/graphics/Insets;
1 hook(s) that cannot bind
```

`native/game_activity.cpp:167` returns `std::shared_ptr<Object>`, from which
libjnivm derives `()Ljava/lang/Object;`. It needs a type that derives
`()Landroidx/core/graphics/Insets;`. The sibling `getWindowInsets` immediately
above has the identical shape and may have the identical bug — check it.

**Done when** `hook_descriptors.py` reports `0 hook(s) that cannot bind`, and the
client still reaches the landing page. Extra credit for establishing whether the
engine ever actually calls it, since a hook that binds and is never called is a
different (and cheaper) situation than one that cannot bind.

---

## 2. `ro.soc.manufacturer` is answered with an empty string

**Good first issue.**

Cordial answers Android's `__system_property_get` from a small table in
`crates/cordial-runtime/src/bionic/mod.rs`. Anything not in the table returns
`""`. Run with `CORDIAL_TRACE_PROPS=1` and the engine asks for four properties,
one of which is unanswered:

```
[props] ro.build.version.sdk = 33
[props] ro.product.model = Cordial
[props] ro.hardware = cordial
[props] ro.soc.manufacturer = <empty, not in table>
```

Twice a run. An empty answer is not obviously harmful here and nothing has been
traced to it — **do not fix it by inventing a plausible-looking vendor string.**
AGENTS.md's rule is that a stub which lies is worse than one which fails, and a
fabricated SoC vendor is a lie the engine may act on. Establish what the field is
used for first; the honest answer may be a real value, or may be leaving it empty
with a comment saying why.

---

## 3. Map which `FLog` channels take a number and which take a severity name

**Self-contained research task. No engine internals needed.**

Roblox's settings document configures log channels, and the value shape is not
uniform — most take a bare verbosity number (`FLogNetwork = "7"`), a minority take
a severity name with an optional sub-level (`FLogAudio = "Info"`,
`DFLogWebSocketTraceError = "Warning,6"`). Giving a channel the wrong shape
**silences it**, which is worse than doing nothing:

| `flags.json` | `[FLog::NativeDM]` lines |
|---|---|
| absent | 29 |
| `{"FLogNativeDM": "100"}` | 0 |
| `{"FLogNativeDM": "Verbose"}` | 30 |

This cost this project a wrong conclusion that had to be retracted: a sweep of 135
channels set with bare integers was read as "loud logging is exhausted" when it had
partly turned logging off.

The binary defines **724** channels. The cached settings document
(`~/.cache/cordial/clientsettings.json`) names the shape for every channel Roblox
itself configures. Produce a table, and ideally make `flags.json` warn when a
channel is given a shape that document disagrees with.

**Done when** a contributor can look up any channel and know what to write.

---

## 4. Two bionic/glibc ABI divergences remain: `mallinfo` and `__cxa_thread_atexit_impl`

**Intermediate. There is a worked example to copy.**

Cordial runs Android's `libroblox.so` against the host's glibc, and the two libcs
disagree about struct layouts. Each disagreement is silent and produces a failure
with no obvious relationship to its cause. Already fixed and documented:
`sigset_t` (8 bytes vs 128), `struct sigaction` (32 vs 152), `addrinfo` field
order, and — this week — `struct statvfs`, where glibc inserts an `int __f_unused`
after `f_fsid` that bionic does not have, shifting `f_flag` and `f_namemax` by
eight bytes on LP64. `ST_RDONLY` lives in `f_flag`.

`native/system_paths.cpp`'s `statvfs` wrapper is the pattern: fill the bionic
shape field by field rather than hoping the two agree.

`struct mallinfo` is 80 bytes in bionic and 40 in glibc, and
`__cxa_thread_atexit_impl` is unhandled. Both are listed in
`docs/analysis/undefined-symbols.tsv`.

**Done when** each is translated with a comment naming the divergence, and — this
is the important half — the report says whether anything observably changed.
"Translated; no behaviour change observed" is a perfectly good result and should
be stated rather than dressed up.

---

## 5. Symbol resolution for `libm`/`libz` should not be gated on a checked-in TSV

**Intermediate, and it has already broken a release once.**

`docs/analysis/undefined-symbols.tsv` is a snapshot of what one build of
`libroblox.so` imported, and the stub table is generated from it. When Roblox
shipped 2.734.0.917 the client stopped loading because it had begun importing
`hypotf`, which was not in the file. A maths function that the host libc already
provides perfectly well.

Host libraries like `libm` and `libz` should be resolved by asking the host, not
by consulting a list of names someone remembered to update. The TSV is still
useful as a record of what a build *did* need; it should not be the gate.

**Done when** a symbol the host provides resolves without the TSV mentioning it,
demonstrated by removing an entry and showing the client still loads. Please keep
the loud failure for symbols nothing can provide — silently resolving those to
something harmless is how the next `hypotf` becomes a mystery instead of an error.

---

## 6. RETRACTED - `initStorageManagerNativeV3` does not run before the directory setters

**Good first issue, and it is a stale comment as much as a bug.**

In `crates/cordial-runtime/src/bin/load.rs`, `initStorageManagerNativeV3` is
called *before* the four `NativeSettingsInterface` directory setters, while its own
comment says it runs "after the directories above are set". Confirmed from the
source's control flow and from stdout ordering across two runs.

This is the LocalStorage store — the working one under `appData/LocalStorage`, not
the content store — so nothing may depend on the order. Establish whether anything
does, then either move the call or fix the comment.

**Either outcome is a fix.** A comment that lies costs more than no comment, and
this repository would rather have the correction than the reorder.

**Not filed, because the premise does not hold.** Checked against a live run at
`0.5.2-165-g76ec67e`, stdout ordering, single run, lines 788-795:

    788  nativeSetFilesDirectory ok
    789  nativeSetCacheDirectory ok
    790  nativeSetExternalDirectory ok
    791  nativeSetBaseDataDirectories ok
    795  initStorageManagerNativeV3 ok

All four directory setters run *before* `initStorageManagerNativeV3`, which is
what its comment says happens. The draft above asserts the opposite and cites
"stdout ordering across two runs" for it. One of those two readings is wrong and
this one is the one with the line numbers, so the draft is retracted rather than
filed -- publishing it would have sent a stranger to fix an ordering that is
already correct, which is worse than filing nothing.

Whether the order changed between the draft and now, or the draft was simply
wrong, is not established here and the difference does not matter for the
decision not to file it.
