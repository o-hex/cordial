# Four runtime performance techniques, described rather than copied

**Status:** analysis. Nothing here is a plan, nothing here was built, and the one
code change this document actually recommends is a comment correction plus a
one-line caching fix in a diagnostic that is off by default. Three of the four
techniques are already answered in Cordial's tree and the fourth should not be
adopted.

The source material is mocktail pull request #73 by `glook9001`, branch
`glook9001/mocktail:perf-runtime-optimizations`, head `3be9d15` — twenty-one
commits, twenty-five files, +1142/−199 against `f797ec7`. **Note the head:** the
commit this investigation was pointed at, `d4e696d`, is the Vulkan
swapchain change and not the symbol-caching work at all, so anyone re-reading
this from that commit will find a different patch than the one described below.

---

## Why this document contains no code, when copying would have been legal

mocktail is Apache-2.0 and Cordial is GPL-3.0. Apache-2.0 is one-way compatible
with GPL-3.0, so adapting mocktail's implementation with attribution is
*permitted* — that is the arrangement CLAUDE.md already records, and it is why
`third_party/mocktail-webview` exists at all. If the licence chain ended there,
this document would be unnecessary.

It does not end there. **The pull request's own opening sentence says it adopts
architectural patterns from a project called Nuah**, and the commit that carries
the looper and affinity work repeats the claim by name. Nuah carries no licence
at all. CLAUDE.md's rule is that Nuah may be read and never copied from, which is
the ordinary conclusion about code with no grant attached: reading is not
infringement, redistribution is.

The pull request does not launder that. "Nuah" appears zero times in the diff —
no source header, no comment, no NOTICE, no attribution file, and no statement of
what licence Nuah is under. The changed files keep mocktail's Apache-2.0 header,
or AOSP's BSD header for the two vendored bionic files. So the Apache-2.0 grant
covers mocktail's own authorship of these files and says nothing about material
that may have arrived from somewhere with no grant at all. A downstream project
cannot tell which is which, and the safe reading is that the whole branch
inherits the caution owed to the unlicensed upstream.

None of that makes the *techniques* anybody's property. Caching a symbol lookup,
polling with `epoll` and waking with an `eventfd`, pinning a thread with
`sched_setaffinity`, and letting a compiler substitute an instruction for a call
are decades older than all three projects and belong to nobody. Naming an
operating-system interface — `dlsym(RTLD_DEFAULT)`, `epoll_wait`, `eventfd`,
`pthread_setname_np`, `/sys/devices/system/cpu/*/topology/core_id` — is naming an
operating-system fact.

The reason to take care anyway is that the path from "read an unlicensed
implementation" to "write a suspiciously similar implementation" is exactly the
path clean-room procedure exists to keep clean, and the cost of walking it
carelessly is not a technical one that can be measured and fixed later. So this
document describes behaviour, invariants, ordering constraints, failure modes and
measurement. It contains no code from mocktail, none from Nuah, and none of its
own. Somebody implementing from it will write their own.

The precedent is `third_party/libbadcpu`, whose README records the same
distinction: vendored from a clean-room reimplementation written from documented
behaviour, with none of the upstream repository's decompiled material taken.

**One further caution about the source's standing.** The pull request is open,
unmerged, has no reviews and no maintainer participation of any kind — all six
comments on it are the author talking to himself — and its six CI jobs were still
queued when it was read, so **nothing in it is known to compile**. The author
describes his own branch, in his own word, as slop, and says more is coming. That
is not a reason to ignore the ideas; three of the four are ordinary and sound.
It is a reason not to treat any claim in it as established, and a strong reason
not to transcribe an implementation that has never been built or reviewed.

---

## 1. Caching symbol resolution

### The problem it solves

A shim standing between a guest binary and a host library has to find the host's
implementation. Doing that by name at every call means walking the dynamic
loader's hash tables and comparing strings on every call, in a global search when
the handle is the default one. The cost is per-lookup and constant; whether it
matters is entirely a function of how often the call happens.

### The mechanism, as behaviour

Resolve the name once, keep the address, and afterwards spend a load and an
indirect branch. Three invariants make that correct rather than merely fast.

The population point must be ordered after the providing object is in the search
scope, or the first lookup stores nothing and the cache is a permanent record of
a mistake. Related, and the sharper edge: **a negative result must not be cached
as though it were an answer, but neither may it be silently retried forever.**
mocktail's version stores only non-null results, so a name that genuinely does
not exist pays the full lookup on every call for the life of the process while a
hardcoded fallback — 1280 by 720, in the window-size case — is returned instead.
That is the worst of both: the slow path and the wrong answer, with nothing said
about either. A cache that cannot express "looked, and it is not there" should
log the miss the first time and then stop looking.

Once published, an entry must not be mutated while a reader can see it, or a
reader observes a torn pointer. And a cache with no invalidation is honest only
while its targets cannot be unloaded. mocktail's targets are exports of its own
host process, so nothing can unload them and the absence of invalidation is
correct; the moment the same idea is aimed at a symbol in a library that can be
`dlclose`d, it is a use-after-unload waiting for a slow day.

Worth recording, because it changes what the technique *is*: **the symbols
mocktail caches are not the Android and EGL entry points.** They are its own
host-boundary upcall symbols — the native-window accessor, window width and
height, a direct-Vulkan predicate, and the EGL display, config, context, surface,
make-current, release-current, swap, proc-address, width and height accessors —
about eleven names that its guest-facing stubs reach for through
`dlsym(RTLD_DEFAULT)`. So the technique is "cache the upcall pointer across your
own process boundary", not "cache the platform API". That distinction is what
decides whether it transfers.

### Does it apply to Cordial

Almost entirely no — and the reason usually given for saying so is not quite the
right one.

Cordial has no host-boundary upcall to cache. `crates/cordial-runtime/src/symtab.rs`
builds the whole `{soname -> {symbol -> address}}` table once, at load
(`symtab.rs:172`). Each candidate host library is `dlopen`ed once
(`symtab.rs:354`), each symbol `dlsym`ed once, and each result checked with
`dladdr` to confirm the *defining* object is the one that was asked for
(`symtab.rs:370`, `symtab.rs:385`) — a check that exists because `dlsym` on a
handle searches that handle's whole dependency chain, so asking libm for `memcpy`
succeeds and would otherwise attribute four hundred libc symbols to libm. The
table is then handed to the linker, and the guest's relocations bind to
addresses. There is no name in the hot path to look up.

The entry points the technique names are Cordial's own Rust functions.
`ANativeWindow_*` and the backend's EGL overrides are registered by
`wayland::overrides()` or `window::overrides()` at `android/mod.rs:374`, so
nothing resolves them at all — they are the answer, not a forwarding stub in
front of one.

**And here is the correction that matters, because the usual account of this gets
the conclusion right and the reason wrong.** `crates/cordial-runtime/src/android/glcount.rs` does
*not* resolve once. Its module header says the wrappers "cost a counter increment
per call and change nothing else" (`glcount.rs:9`), and the doc comment on the
host-resolution helper says "resolved once" (`glcount.rs:40`). Neither is true.
The forwarding macro calls that helper from inside the wrapper body
(`glcount.rs:59`), and the helper builds a `CString` (`glcount.rs:45`) and calls
`dlsym(RTLD_DEFAULT)` (`glcount.rs:50`). So every wrapped call performs a heap
allocation, a global symbol lookup, and a free. Among the wrapped calls are
`glDrawElements` and `glDrawArrays` (`glcount.rs:107`).

Two things follow, and they point in opposite directions.

The shipped client is unaffected. The counting wrappers are registered only when
`CORDIAL_COUNT_GL` is set (`android/mod.rs:378`), so in normal use there is no
per-call lookup anywhere on the hot path. The familiar arithmetic — ten calls a
frame at 116 ns is about a microsecond, a rounding error at 240 Hz — is right
about the client anybody actually runs.

The instrument is affected, and that is the part this project cannot afford to
shrug at. A scene issuing two thousand draw calls a frame pays two thousand
lookups and two thousand malloc/free pairs per frame, which is a wholly different
number from a microsecond, and it is paid precisely when somebody is trying to
measure the renderer. AGENTS.md's opening rule is about measurements taken with
broken instruments; a counter whose documentation says it costs an increment and
which in fact costs the dynamic loader is that failure inside Cordial's own
diagnostics. The comment that lies costs more than no comment.

The same lookup-per-call pattern appears in the cold-path EGL forwarders —
`wayland.rs:4758`, `wayland.rs:4803`, `wayland.rs:4861`, and their X11
counterparts at `window.rs:1393` and `window.rs:1452`. Those run once per surface,
once per display, and once per interval change. The cost there is unmeasurable
and they are not worth touching for their own sake.

A note on magnitude generally, since the pull request quotes 116.82 ns against
3.18 ns and a 36.6-fold ratio from ten million operations with no harness
committed, no machine named, no warm-up described, no variance, and — the
omission that matters most for this particular measurement — **no statement of
how large the link map was**, which is the entire independent variable for the
cost of a `RTLD_DEFAULT` search. A per-operation figure without a call count is
the same error as quoting present counts as a frame rate: it is a rate detached
from the quantity it would have to be multiplied by to mean anything. There is no
end-to-end measurement anywhere in that pull request — no frame time, no
percentile, no before-and-after in a running client, for this or any of its
techniques.

### What Cordial would build

Nothing new. Fix `glcount.rs` so each wrapper resolves its host function once,
holding the address in a per-wrapper one-time initialisation populated on first
call; use static C string literals so no path allocates; and treat a null result
as "the host has no such entry point", reported once, rather than as a value to
re-derive on every call. Then correct the two comments to say what the code does.

### The measurement, and the control

The claim to test is not that the client got faster, because the flag is off in
normal use and a claim that cannot be true should not be measured. The claim is
that **the counting instrument no longer perturbs what it counts.**

Run `tools/frame-pacing-check.py`, which drives the pointer for the whole
measurement and reads the percentiles out of the ring in
`crates/cordial-runtime/src/android/frame_pacing.rs`. Take a pair: one run with
`CORDIAL_COUNT_GL=1`, one with it unset, same session, same input rate. Then take
the same pair after the change. What would show the fix working is the p95 and p99
gap between the counted and uncounted runs closing, with the uncounted arm
unmoved.

Two constraints on that measurement, both learned here already. Drive input for
the whole run: an idle client presents at exactly 1.0 a second and its p99 sits
near 1000 ms, and every startup-freeze survey that recorded such a tail was
recording the idle throttle rather than a stall (`docs/NEXT.md`, "Frame pacing,
measured properly"). And take it on the GLES backend: the Vulkan path presents
through Cordial's own `vkQueuePresentKHR` (`android/vulkan.rs:533`) rather than a
counting forwarder, so a Vulkan session cannot show the effect at all and a null
result there would mean nothing.

If the gap between the counted and uncounted arms is already inside run-to-run
variation, say so and leave the code alone apart from the comments. A fix
indistinguishable from its control is not a fix.

---

## 2. A real `ALooper` over `epoll` and `eventfd`

### The problem it solves

`GameActivity`'s native side runs a poll loop and dispatches whatever comes back:
input, lifecycle, and whatever descriptors the application registered. It is not
optional and it cannot be faked. A looper that answers without looking turns the
engine's main loop into a busy spin; one that never returns hangs it.

### What Android's contract actually requires

Stated as behaviour, from the NDK's published `android/looper.h` and the AOSP
implementation behind it:

`ALooper_pollOnce` waits up to the given timeout in milliseconds — negative means
block, zero means do not wait — and **invokes the callbacks of every descriptor on
which an event occurred**, returning the callback result once one or more
callbacks have run. Descriptors registered with an identifier rather than a
callback are reported one at a time, by returning that identifier and filling the
out-parameters. AOSP collects the entire ready set from one `epoll_wait` into a
response list and then hands the identifier-bearing entries out one per call from
that remembered list, so three ready identifier registrations produce three
returns from a single syscall. A callback returning zero means "remove me", and
the looper must remove the registration. A callback may add or remove descriptors
while it runs, so no lock over the registration set may be held across the call.
`ALooper_wake` must make a blocked poll return promptly, from any thread. Loopers
are per-thread, and `ALooper_forThread` returns the caller's or null.

*The response-list detail is `INFERRED`: it matches the header's wording and the
implementation as remembered, but no copy of AOSP's `Looper.cpp` was available on
this host to check it against. The rest is the header's documented contract.*

### Does it apply to Cordial

Cordial already has the real thing, so the only live question is the gap.

`crates/cordial-runtime/src/android/looper.rs` creates an `epoll` instance and an
`eventfd` per looper and registers the wake descriptor on the set
(`looper.rs:511`, `looper.rs:547`). `ALooper_addFd` performs a real `epoll_ctl`
and records a registration (`looper.rs:1428`); removal deregisters
(`looper.rs:1489`); `ALooper_wake` writes the eight bytes an `eventfd` requires
(`looper.rs:1501`). The four return constants carry their Android values
(`looper.rs:252`). Loopers are per-thread. This is not the stub the pull request
replaces.

**On mocktail's "before" state, one correction worth having, because it inverts
the failure.** The old stub's `ALooper_pollOnce` returned −2 with a comment
naming it as the timeout value. −2 is the *callback* value; timeout is −3. So the
stub was not telling every caller "nothing happened" — it was telling every
caller "I dispatched a callback for you", on every poll, forever. That is a
stub that lies in the precise sense AGENTS.md means, and the comment beside it
lied about which lie it was telling.

**The gap in Cordial is real.** The dispatch loop
(`looper.rs:1545` onward, the batch walk beginning after the `epoll_wait` return)
iterates the ready set and returns from inside the loop on the first descriptor it
can act on — the wake value for the wake descriptor, the callback value after one
callback, or the identifier. The remainder of the batch is discarded. Android
would have run every ready callback first.

Two things make that narrower than it looks, and both should be said before
anybody writes code.

The descriptors are registered level-triggered — `EPOLLIN` and `EPOLLOUT`, no
edge-trigger flag anywhere in the file — so a discarded ready descriptor is
reported again by the next `epoll_wait`. **No event is lost. The cost is a syscall
per ready descriptor instead of per batch**, and a latency of one loop iteration
for everything after the first. Notably, mocktail's new implementation rests on
exactly the same invariant for exactly the same reason — it drains all callbacks
but short-circuits on the first identifier registration and abandons the rest of
its batch — and the pull request does not state the invariant anywhere. An
implementation that quietly depends on level-triggering and does not say so is one
edge-trigger optimisation away from silently dropping events.

And the caller here is not one that suffers from it. `looper.rs:261` records the
engine calling `ALooper_pollOnce(0)` in a tight loop, so a batch of N drains in N
iterations of a loop that already runs millions of times a second. The change
would buy syscalls, not latency, for this caller.

The two parts of the contract that are easiest to get wrong are already right: a
callback returning zero removes the registration, and the registration borrow is
explicitly released before the callback runs so that a callback may add or remove
descriptors — the comment there says so and names the panic that would otherwise
occur.

**A second correction, on the symbol count.** The engine imports
seven `ALooper_*` symbols and Cordial exports eight. Verified with `readelf` on
this build's `libroblox.so`: the undefined list holds `ALooper_acquire`,
`ALooper_addFd`, `ALooper_forThread`, `ALooper_pollOnce`, `ALooper_prepare`,
`ALooper_release` and `ALooper_removeFd`. `ALooper_pollAll` and
`ALooper_isPolling` are absent, as expected — and so is **`ALooper_wake`**,
which Cordial exports (`looper.rs:1738`) and uses itself to break its own pump out
of a blocking poll. That eighth export is not dead code; it is Cordial's, not the
engine's. A report that says "all seven" should say "the seven it imports, plus
wake, which it does not". Incidentally `ALooper_isPolling` is not an NDK-exported
symbol in the first place — it is a method on the C++ `Looper` in libutils — so
mocktail's version of it is an addition beyond the NDK surface rather than a gap
being filled.

### What Cordial would build if the gap were worth closing

Keep the responses from one `epoll_wait`; dispatch every callback-bearing one
before returning; hand the identifier-bearing ones out one per subsequent
`pollOnce` from the remembered set without a further syscall; and discard the
remembered set whenever a fresh `epoll_wait` happens.

The invariants that would have to hold. A remembered response must be dropped if
its registration was removed before it is handed out, because an earlier callback
in the same batch may have removed it — mocktail handles this by re-looking-up
each descriptor rather than trusting the snapshot, which is the right shape. The
wake descriptor must still short-circuit and must still be drained exactly once.
The remembered set is per-looper and therefore per-thread, so it needs no locking,
which is the same reasoning that already puts the registrations behind a
single-threaded cell rather than a mutex. And there is a hazard neither
implementation guards against and a new one should: **descriptor-number reuse
inside a single batch.** A callback that closes a descriptor and opens another can
alias a later entry in the same remembered batch, and the re-lookup will find the
new registration under the old number and dispatch to it.

Anything touching this loop has to be measured against two behaviours Cordial's
looper carries and mocktail's cannot, because both are in tension with batching
and both are what the shipped client runs. The zero-timeout coalescer
(`looper.rs:257` onward, `looper.rs:332`) answers an idle spin from memory for up
to 250 µs, because the engine's busy poll was costing a whole core — measured on a
live client with one thread at 99.5% and every stack sample inside `epoll_wait`,
while the GPU sat at 45% and would not clock past 967 MHz of its 1.50 GHz boost.
And the bounded "infinite" wait (`looper.rs:54` onward) refuses to sleep forever,
because two clients were caught frozen in `epoll_wait(-1)` with their present
counts nailed down.

### The measurement, and the control

**Read one number before writing anything.** `LooperStats.registered`
(`looper.rs:452`) counts the descriptors on each looper, and the development
control socket exposes it as the `loopers` command (`devctl.rs:175`). If the
engine's looper carries one or two descriptors, a batch can never contain more
than one or two ready ones, batch draining can never save more than a syscall a
poll, and the extra state is not worth having. Nobody in this repository has ever
read that number. It is one command against a running client and it may end the
question.

If it does not, the instrument is `CORDIAL_INSTR=1`, which turns on the per-thread
poll census (`looper.rs:95`). It reports, per polling thread and per second, the
requested-timeout mix and what each poll returned — empty, wake, callback,
identifier, unclaimed — with a sampled `epoll_wait` duration taken on one call in
1024 so the clock reads do not dominate what they measure. The deciding statistic
is callbacks plus identifiers per poll. **If that is essentially always one, the
change is unmeasurable by construction** and should not be made.

If the change is made, the confirming numbers are the `epoll_wait` call rate
falling while callbacks-plus-identifiers per second stays the same. The control is
the same session length and the same input rate with the current code. Drive input
for the whole measurement, because an idle client's poll mix is the idle
throttle's and not the game's. And do not claim a frame-rate win from it without
the frame-rate measurement: this is a syscall-count change, and the honest
prediction is that the frame-pacing percentiles do not move at all.

The census also has a counter built for exactly the failure this change could
introduce. `unclaimed` counts a descriptor that came back ready and that no
registration claims — level-triggered, so ready again immediately, which turns any
zero-timeout caller into a spin. It once read 829 in a sixty-second run, naming
one descriptor, and the cause was benign; the count is there so that a real
black-hole descriptor announces itself. A batching change that mishandles a
remembered response would show up there first.

---

## 3. Pinning threads to physical cores by name

### The problem it claims to solve

On a simultaneous-multithreading part, two threads scheduled on the two logical
CPUs of one physical core share that core's execution resources. If the render
thread and a background worker land on siblings, the render thread runs at a
fraction of a core while another physical core is idle. Pinning by role is an
attempt to stop the scheduler making that pairing.

### The mechanism, as behaviour

Classify each thread by its name into a role; enumerate the machine's CPUs; build
one affinity mask per role; apply it with `sched_setaffinity` or
`pthread_setaffinity_np`. The role classification is a substring match on the
thread name, so it depends on the engine naming its threads and on those names
being what the table expects.

mocktail's foreground set matches `Render`, `Vulkan`, `Present`, `Graphics`,
`Main` and `Display`; its background set matches `Worker`, `Job`, `Http`, `Asset`,
`Audio` and `Physics`. Matching is case-sensitive, plain substring, first branch
wins, and anything matching neither is left alone. The hook is the vendored
bionic `pthread_setname_np`, applied as a side effect of a successful rename, so a
thread the engine never names is never bound and a later rename re-applies. It is
**on by default**, opt-out through an environment variable set to exactly `0` or
`off` — a variable whose name reads like an opt-in and is not, so a typo enables
it.

**The topology discovery does not do what the feature is named after, and this is
the finding that decides the section.** It walks CPU indices 0 to 63, reads
`/sys/devices/system/cpu/cpuN/topology/core_id`, and stops at the first index
whose path will not open. Every CPU whose `core_id` is 0 goes into the foreground
mask; every CPU whose `core_id` is 1 or more goes into the background mask.
Nothing reads `thread_siblings_list`, `core_siblings_list`, or
`physical_package_id`, and nothing anywhere detects whether the machine has SMT at
all.

Since hyperthread siblings share a `core_id`, the foreground mask is core 0
*together with its siblings*. The change is titled "SMT Physical CPU Core Affinity
& Isolation" and its summary claims it "Isolates Render/Vulkan to Core 0"; what it
does is **co-locate** the latency-critical threads onto one physical core's two
logical CPUs and hand every other physical core to the background pool. That is
the opposite of SMT isolation. `core_id` is also unique only within a package, so
on a multi-socket or multi-die machine the foreground mask spans sockets and the
"isolation" is across NUMA nodes.

### Does it apply to Cordial

Two of the three preconditions are already here, which is worth knowing because it
means the reason not to do this is not "it would be hard".

The topology walk exists. `crates/cordial-runtime/src/flags.rs:571`
(`physical_cores`) already reads `physical_package_id` and `core_id` for every
CPU, counts distinct package-and-core pairs, and falls back to the thread count
when sysfs is unreadable. It is the correct walk — the one mocktail's version
skips — and its doc comment (`flags.rs:441` onward) carries a worked correction
that is directly relevant here: the machine this repository is developed on was
described in that comment as fourteen cores and is in fact ten, six hyperthreaded
performance cores plus four unshared efficiency cores giving sixteen threads,
established by walking that same topology and confirmed against `lscpu`. Every
performance-mode flag table sizes its worker pools off that number, and anybody
reasoning from the stale comment was reasoning from a count that was wrong by four
in the direction that mattered. Core counting is easy to get wrong and this
repository has already got it wrong once.

The interception point exists in outline. The engine imports `pthread_setname_np`
— verified in this build's undefined-symbol list — so the engine does name its
threads, and `native/thread_trace.cpp` already wraps `pthread_create` behind
`CORDIAL_TRACE_THREADS`. **But `pthread_create` is the wrong hook for a
name-matching policy, because the name does not exist yet when the thread is
created.** mocktail is right about this: the moment a role becomes knowable is the
rename. That wrapper does not exist in Cordial.

What does not exist is any reason to believe the policy wins. Cordial has never
recorded the engine's thread inventory. The nearest thing to evidence is that the
engine renames its main thread to `Main` — which is how `pgrep -x cordial-run`
came to report nothing for a client that was plainly running, and which
`docs/analysis/crash-trace.md:388` records from the other side. One name is not a
table.

### Why it should not be adopted

The reasons are worth writing down rather than asserted, because "it seemed like a bad idea" does not survive the next person who
reads the pull request.

**There is no measurement, and the place where one should be contains a
restatement of the mechanism.** The pull request's section on affinity contains no
number of any kind. Its summary table has a column headed "Measured Improvement",
and the entry under it for this feature is the sentence "Isolates Render/Vulkan to
Core 0 (No L1/L2 drops)" — a description of what the code intends, presented as a
result, under a heading claiming it was measured. The commit that carries the
change ends its message with the 36.6-fold figure, which belongs to the symbol
cache in the same commit and says nothing about affinity. A follow-up comment adds
that it "IS VERY IMPORTANT ON LOWEND HARDWARE" and names no machine. There is no
cache-miss count despite the L1/L2 claim, no frame time, and no run with the
policy disabled.

**A name table is an unverifiable guess about someone else's internals, and it
fails silently.** A name that does not match binds nothing and reports nothing.
That is the "control that reports success and does not act" shape this codebase
has already found in its own settings — `flags.rs:550` exists because a
performance-mode table sat in the tree with no caller at all, so choosing a mode
was not possible by any route while everything reported fine. If this were ever
built here, it would have to log every thread it saw, its name, and whether it
matched, so a table gone stale on the next Roblox build announces itself instead
of quietly becoming a no-op.

**Six foreground roles do not fit two hyperthreads.** Render, Vulkan, Present,
Graphics, Main and Display on one physical core means that on any machine where
more than two of them are simultaneously runnable, the scheme deterministically
creates the contention it exists to prevent — where the scheduler would only have
done it by accident, and would have moved off it.

**Core 0's identity is a numbering accident.** On the 6P+4E part this repository is
developed on, physical core 0 happens to be a performance core. On a part that
enumerates efficiency cores first, the same rule pins the renderer to the slowest
core in the machine, and nothing in the scheme notices. Telling P from E needs
`cpu_capacity` or the per-CPU maximum frequency, and neither is read.

**A restricted cpuset defeats it silently.** Flatpak, systemd slices, containers
and `taskset` all narrow the affinity mask a process may set, while
`/sys/devices/system/cpu/*` continues to enumerate every host CPU. So the computed
mask can name CPUs the process may not use, `sched_setaffinity` fails with
`EINVAL`, and mocktail explicitly discards the return value — no log, no fallback,
no cross-check against the affinity the process already had. Cordial ships as a
Flatpak. This is the common case here, not the exotic one.

**With no SMT the scheme reduces to something strictly worse than the kernel.**
Every logical CPU is a physical core, one of them gets the renderer, and the rest
get everything else, chosen by nothing.

**And the kernel is not naive.** Linux's scheduler is SMT-aware and prefers an
idle physical core to an idle sibling when placing a task. Beating it requires
knowing something it does not, and "this thread is called Render" is not
obviously that thing.

### What would have to be measured before anyone believed it

The order matters, because the first step usually ends it.

First, inventory. Run a real session and read the thread names — either from
`/proc/<pid>/task/*/comm` directly, or from `CORDIAL_INSTR=1`, whose poll census
already prints each polling thread's name beside its tid, or from
`CORDIAL_TRACE_THREADS=1` for creation. **If no thread is named anything the table
would match, stop.**

Second, establish that the bad pairing actually happens. Sample which CPU each
named thread is running on over a real session — the `processor` field of
`/proc/<pid>/task/<tid>/stat` — and count how often two render-side threads are on
siblings of one physical core. If the kernel already almost never does that, there
is nothing to fix and the policy can only make it worse.

Only then is there a hypothesis. Third, measure the frame-pacing percentiles with
the policy on and off in the same session, with input driven for the whole run, and
report p99 and max rather than the median — the claim is about stutter, and the
median will not move. A client presenting 120 frames a second in even 8.3 ms steps
and one presenting them as sixty pairs 16 ms apart have the same count and feel
completely different; that is why `frame_pacing.rs` exists. Repeat it: one clean
run is not a result in a project with a bug that reproduced on one launch in three.
And run it on at least an SMT-only part, a hybrid part, and a machine under a
restrictive cpuset, because "it helped on my laptop" is exactly the claim that has
to survive contact with somebody else's machine.

**The instrument to distrust here is CPU percentage.** A pinned thread that is
waiting shows less CPU and the same frame time, and a report that quotes
utilisation as though it were performance repeats the error
`docs/analysis/startup-and-idle-cost.md` spent a whole document unpicking —
including its own finding that mocktail's 8% idle is not a target to match but the
cost of an engine that was never told it had focus.

---

## 4. Math builtins in a libm shim

### The problem it claims to solve

A call into glibc's libm goes through a procedure-linkage-table indirection and
lands in an implementation that sets `errno` on domain and range errors.
Compiling the same operations in your own translation unit with `-O3
-fno-math-errno` lets the compiler substitute an inline instruction where one
exists and skip both.

### The mechanism, as behaviour

`-fno-math-errno` is a promise to the compiler that no math function it recognises
needs to set `errno`. That promise licenses it to replace a call with an
instruction where an instruction exists, and to hoist, reassociate around, or
delete calls it can prove unused. It changes what the program observes after a
domain error: the square root of a negative number sets `EDOM` under the C
standard's errno regime, and under this flag produces a NaN and leaves `errno`
alone.

Note what it is not. It is not a fast-math flag; mocktail does not pass
`-ffast-math`, so rounding modes and the reassociation of ordinary floating-point
arithmetic are nominally untouched. And note the limit that decides everything
below: **it affects code the compiler compiles, and it cannot affect how an
already-compiled guest binary calls anything.**

mocktail's version replaces a libm that was an empty translation unit linked
against the host's with about a hundred explicit definitions, built with `-O3
-fno-math-errno`, still linked `--no-as-needed` against the host libm. Its stated
mechanism is a combination of PLT avoidance and builtin inlining — "directly emits
hardware vector/FPU instructions without PLT/glibc overhead". Errno-wrapper
avoidance is implied by the flag and never argued. There are no benchmark numbers
for it at all; the only claim is a qualitative table entry naming three
instructions.

### Does it apply to Cordial

No, and there are three independent reasons, any one of which is sufficient.

**Cordial cannot remove the guest's indirection.** `libroblox.so`'s calls into libm
go through its own PLT and GOT, resolved by Cordial's ported linker. Whoever is on
the far side, the guest still executes an indirect branch. Removing it means
editing the guest's code, which ADR-001 puts permanently out of scope. The PLT half
of the claim is not available here at any price, and it is worth noticing that it
is barely available to mocktail either — a shim exporting these symbols with default
visibility is reached through a PLT by anything that calls it.

**The functions where a builtin substitutes an instruction are not the functions
this engine imports.** Checked against the build in `~/.cache/cordial/lib/x86_64`:
of the sixty-odd libm symbols in `libroblox.so`'s undefined list — the inverse
trigonometric family, the trigonometric family, the hyperbolics, `exp`, `exp2`,
`expm1`, `log`, `log2`, `log10`, `pow`, `powl`, `cbrt`, `hypot`, `fmod`,
`remainder`, `remquo`, `frexp`, `ldexp`, `modf`, `ilogb`, `nextafter`, `round`,
`lround`, `llround`, `sincos`, `nan`, and their float variants — **not one of
`sqrt`, `fabs`, `floor`, `ceil`, `trunc`, `rint`, `nearbyint`, `fma`, `fmin`,
`fmax` or `copysign` appears.** Those are precisely the ones with a
single-instruction x86-64 form, and their absence is the evidence that Roblox's own
compiler already inlined them when it built the shared object. What remains is the
transcendental set, which has no instruction form on x86-64 that is both fast and
accurate. A shim compiled with builtins emits a call to *some* libm for every one
of them. The best available outcome is that Cordial's shim becomes a slower way of
reaching the same glibc code.

**A faster libm is a different libm, and that is a correctness decision.**
Substituting anyone's sine for glibc's changes results in the last bits. Roblox's
physics and replication are not obviously insensitive to that, and a divergence
that surfaces only as a desync in a game nobody is testing is the worst failure
shape available here. The safe form of a math shim is one that forwards to the host
and changes nothing about the value returned — which is what `symtab.rs` already
does with no shim at all (`symtab.rs:181`, `symtab.rs:370`).

**A hazard in mocktail's version that anyone adopting the pattern must check
first, marked `INFERRED` because it was not compiled.** For the transcendentals
there is no instruction, and the corresponding compiler builtin is defined to fall
back to a call to the library function of the same name. A function named `sin`
whose body is the `sin` builtin, exported with default visibility from an object
that resolves through its own PLT ahead of the host libm, is at serious risk of
lowering to a call to itself. That is unbounded recursion on the hottest math paths
rather than a speedup. The pull request's CI had produced no result when this was
read, so nothing in it is known to compile, and a "no benchmark, no test" change is
exactly where a fault of that shape survives. A milder version of the same risk:
replacing a blanket forwarder with an explicit list means any libm symbol the
engine needs and the list omits now depends on the retained link to the host libm
to be found, and the coverage was never checked against an undefined-symbol set.

### What would have to be true for the errno change to be safe

That nothing in the engine reads `errno` after a math call and takes a decision on
it. That is a question about the engine's source, not about its binary, and it is
therefore not answerable here. It is emphatically not answerable by disassembly —
that is the class of question AGENTS.md's one rule is about, the one that produced
nine consecutive wrong conclusions in a single session.

So the honest position is that it is unknown. An unknown correctness change in
exchange for an unmeasured speed change is a bad trade whichever way the unknown
resolves, and the direction of the risk is the wrong one: modern glibc already
avoids the errno path in the common case, so the saving is small where it is real,
while the failure mode is silent. The pull request does not discuss `errno` once —
nor rounding modes, nor floating-point exception flags, nor `long double` width,
nor the difference between glibc's correctly-rounded implementations and whatever
a builtin lowers to.

### The measurement, and the control

This is the one technique here that can be measured without launching the client,
which is also the reason to distrust a number taken that way.

**The measurement that should be taken first, and that nobody has taken, is
whether the engine spends any time in libm at all.** Sample a real session with
`perf record`, input driven for the whole run, and attribute time to
`libm.so.6`. `eu-stack` sampling is the fallback on this host if `perf` is
unavailable — it needs no symbols, though it leaves `libroblox` frames as bare
addresses. If libm does not appear in the profile, there is nothing here to
optimise, the correctness risk buys nothing, and the question is closed.

Only if it does appear is a microbenchmark worth writing, and then the useful
comparison is not "builtins against glibc" but "our forwarding shim against no
shim", because that is the change actually on offer. The control is the same
session with the shim absent, in the same run, and the reported statistic is
frame-pacing p99 rather than a per-operation nanosecond figure — a rate detached
from a call count is the error this whole document keeps running into.

---

## Other things travelling in the same pull request

Not asked about, but a reader deciding what to take from this branch should know
they are in it.

**Android logging is silenced by default.** `__android_log_print` and
`__android_log_write` now return immediately unless an environment variable is set
or an observer is registered, where previously every engine log line reached
stderr. The advertised saving is the cost of work no longer performed at all, not
the same work performed faster. For a project that debugs by reading engine output
this is a diagnostic regression presented as an optimisation, and the flag is
opt-in, so the default posture is blind. `docs/analysis/startup-and-idle-cost.md`
§4 already records that Cordial emits no engine log and treats that as a defect to
fix rather than a saving to bank.

**An asset-resolution cache that memoises failures and never invalidates.**
Resolved paths and failed lookups are both kept for the life of the process with
no expiry and no filesystem watch, so **an asset absent at first probe can never be
found later in that process**. That conflicts directly with ADR-010's asset
overlays if an overlay can be enabled or materialised after startup.

**A `madvise` shim that rewrites `MADV_FREE` to `MADV_DONTNEED`**, with the stated
rationale of avoiding failed `EINVAL` retries. That rationale is wrong on any Linux
from 4.5 onward, where `MADV_FREE` is supported natively and does not return
`EINVAL`. The rewrite is a semantic downgrade — `MADV_FREE` is lazy and lets the
allocator reclaim its own pages cheaply, `MADV_DONTNEED` discards immediately with
the TLB and refault cost that implies — so this is plausibly a pessimisation
shipped inside a performance change. It is also precisely the stub that lies: the
caller asked for one behaviour and silently received another.

**Vulkan swapchain minimum image count forced to four.** Two paths are touched; one
clamps against the driver's reported maximum and the other does not, so a driver
reporting a maximum of two or three is handed an out-of-range request at swapchain
creation. No measurement accompanies it.

**A sweep making environment variables read-once at first use** across the window
layer, tracing predicates and proxy configuration. The behavioural consequence is
uniform and worth stating as an invariant for anyone tempted by the same sweep
here: runtime toggling stops working, and the first read must happen after the
environment is final. Cordial already does this in places and for good reason —
`looper.rs`'s census flag and coalescing window are both read once — but it is a
behaviour change, not a free optimisation.

**No tests were added,** for any of the four techniques, including the looper
rewrite, which is the change most amenable to a unit test and the one where a
return-value or dispatch-ordering error would stay invisible until something in the
engine starved. Cordial's own looper carries unit tests asserting, among other
things, that an expired infinite wait reports a timeout rather than a fabricated
wake.

---

## Recommendation

| Technique | Verdict | Reasoning |
|---|---|---|
| Cache symbol resolution across a host boundary | **Already have** | `symtab.rs` resolves every host symbol once at load and hands the linker addresses, so no name is looked up on the hot path; `ANativeWindow_*` and the EGL overrides are Cordial's own functions, not forwarding stubs. There is no host-boundary upcall here to cache. |
| Resolve once in `glcount.rs` | **Adopt — as an instrument fix, not a speed fix** | It resolves per call and allocates per call, contradicting both its own comments, and it wraps `glDrawElements`. Gated off by `CORDIAL_COUNT_GL`, so the shipped client is unaffected; what is affected is anything timed with the counters on, which is the broken instrument AGENTS.md's opening rule is about. |
| `epoll` + `eventfd` `ALooper` | **Already have** | `android/looper.rs` is a real per-thread epoll looper with a wake eventfd, ident and callback registrations, callback-driven removal, and no borrow held across a callback. It is not the stub the pull request replaces — and that stub was returning the callback value, not the timeout value, so it was claiming dispatches it never made. |
| Batch-draining `pollOnce` | **Do not adopt yet — read `loopers` first** | The gap is real against the NDK contract's "invokes the callbacks of every descriptor on which an event occurred". Level-triggered epoll means nothing is lost, only a syscall per ready descriptor, and the engine polls in a tight loop so it is not a latency cost. If `LooperStats.registered` shows one or two descriptors on the engine's looper, there is nothing to batch and the state is not worth carrying. |
| SMT physical-core affinity | **Do not adopt** | No measurement supports it and the "Measured Improvement" column contains a restatement of the mechanism. The topology walk reads only `core_id`, so it co-locates the render threads on one core's hyperthreads rather than isolating them, and breaks outright on multi-socket. Six foreground roles do not fit two hyperthreads, "core 0" is the wrong core on a hybrid part, a Flatpak cpuset makes the call fail with the return value discarded, and Cordial has never even recorded the engine's thread names. |
| Wrap `pthread_setname_np` | **Adopt only as instrumentation, if at all** | It is the right hook for anything name-based, because the name does not exist at `pthread_create` — but the thing to do with it first is record the inventory Cordial lacks, not bind anything. |
| Math builtins in a libm shim | **Do not adopt** | Cordial cannot remove the guest's PLT without editing the guest, which ADR-001 forbids; the engine imports no math function with a single-instruction form, so there is no builtin to substitute; changing which libm answers is an unbounded correctness change; and the pattern carries a plausible self-recursion fault that has never been compiled. |
| Silencing Android logging, memoising asset misses, `MADV_FREE` rewriting | **Do not adopt** | Two regressions and a stub that lies, travelling in the same branch under the performance heading. |
