# What Cordial costs at startup and at rest, against two controls

**Status:** measurement only, nothing in the tree modified. Three clients
running the same Roblox 2.734.917 engine on one host, on the landing page, no
experience joined. Sober 1.7.1 and mocktail 1.0.3 (both Flatpak) are the
controls; Cordial is `v0.6.0-32-gd30d7c0-dirty`, where `-dirty` is the
`third_party/mcpelauncher-linker` submodule pointer and nothing else — the
crates were clean at build time (`cargo` reported nothing to rebuild at the
commit).

Host: 13th Gen Intel i7-13620H, **16 logical cores**, 15.7 GB RAM, Intel UHD
(i915), Linux 7.1.8. All runs on the maintainer's own Wayland session at
3440x1359, sequentially, one client at a time.

**Bottom line up front:** Cordial is **not slower to start** than either
control, and it is **not heavier at rest** — except that on **three runs out of
nine it fails to enter the engine's idle throttle and then burns a whole CPU
core indefinitely**, where both controls always settle to 8-9% of one core.
That bimodality, not a uniform slowness, is the thing worth chasing, and it is
the best candidate this measurement offers for "it feels laggy". Two reported
beliefs did **not** survive contact: startup is not getting slower in any way
this data can show, and Cordial does **not** exit faster than the others — by
the one exit measure available it is the slowest of the three.

---

## 1. The table

Every cell states its own method. Cells that cannot be compared say so rather
than carry a number of convenience.

| | **Cordial** | **mocktail** | **Sober** |
|---|---|---|---|
| **Startup — engine milestone** `setStage: (stage:LuaApp)`, from process launch. Wall clock, from the engine's own FastLog ISO timestamps. | **not comparable** — Cordial emits no engine FastLog at all (§4) | **4.45 ± 0.32 s** (n=3) | **4.60 ± 0.42 s** (n=3) |
| **Startup — external proxy.** Time from launch until RSS first reaches 90% of its settled value. Derived only from `/proc` sampling, so identical rule for all three. | **4.47 ± 0.70 s** (n=9) | 5.37 ± 1.06 s (n=3) | 5.89 ± 0.29 s (n=3) |
| **Peak CPU during startup**, max over 0.5 s samples, % of one core. | **342 ± 8 %** (n=3) | 142 ± 4 % (n=3) | 153 ± 5 % (n=3) |
| **Idle CPU**, % of one core, fixed 20–45 s window measured from launch (same wall-clock slice for all three). | **bimodal: 103.5 ± 1.5 % on 3 of 9 runs; 6.3 ± 1.1 % on the other 6** | 8.7 ± 0.1 % (n=3) | 7.8 ± 0.1 % (n=3) |
| **RSS at the landing page**, median over the last 30 s, summed across the client's whole process set. | **802 ± 4 MB** (n=3) | 875 ± 304 MB (n=3) | 1131 ± 80 MB (n=3) |
| **PSS** (same window; avoids double-counting shared pages). | **774 ± 2 MB** (n=3) | 822 ± 303 MB (n=3) | 1023 ± 64 MB (n=3) |
| **Memory growth at rest**, least-squares slope of RSS over the final 60 s. | −2.4 ± 5.7 MB/min | +13.3 ± 12.2 MB/min | −30.7 ± 17.2 MB/min |
| **Exit**, SIGTERM to every process in the client's set → last process reaped. | **0.599 ± 0.216 s** (n=9) | 0.302 ± 0.205 s (n=3) | **0.111 ± 0.001 s** (n=3) |
| **Frame rate** | **not comparable** (§5) | **not comparable** (§5) | **not comparable** (§5) |
| Background load during the measured window (system busy fraction, all 16 cores) | 0.120 ± 0.014 | 0.077 ± 0.013 | 0.064 ± 0.016 |

No growth figure is resolvable: in all three the slope is smaller than its own
standard deviation over a 60 s fit. Treat the row as "nothing detectable at
this window length", not as three measured rates. Sober's and mocktail's large
RSS spreads are real — both *free* several hundred megabytes about a minute in,
so where the window falls matters; Cordial's spread of 4 MB shows it does not.

---

## 2. Why the external startup proxy is trustworthy

"Time until RSS reaches 90% of its settled value" is a proxy, and a proxy is
worth what it was checked against. On the two clients where the true engine
milestone *is* available, the proxy can be calibrated directly: it overshoots
`setStage: (stage:LuaApp)` by **0.92 ± 1.15 s** on mocktail and **1.29 ± 0.18 s**
on Sober. So the proxy consistently lands about a second late, in the same
direction, on both.

Applying that correction to Cordial's 4.47 s proxy puts its landing page at
roughly 3.2–3.6 s — i.e. **Cordial is if anything the fastest of the three to
the landing page**, and is certainly not slower. That is the opposite of the
reported impression, and it is the claim this document is most confident about,
because the proxy is the only measure here computed identically for all three.

Startup timing is also where Cordial spends the most CPU: **342% of one core at
peak against 142% and 153%**. It reaches the landing page at least as fast while
using more than twice the CPU to get there. On a 16-core desktop that is free;
on a laptop on battery it is not.

---

## 3. The finding that matters: Cordial sometimes never idles

Sampling CPU in 10 s buckets over nine Cordial runs splits them cleanly into two
populations with nothing in between:

```
cordial-1   [166, 105, 105, 105,  99]                     pinned   (tail median 102%)
cordial-2   [165, 105, 105, 105, 105, 105, 103, 103, ...] pinned   (tail median 103%)
cordial-5   [163, 106, 105, 105, 105, 106, 104]           pinned   (tail median 105%)
cordial-3   [161, 105,  53,   6,   6,   7,   4, 4, ...]  throttled (tail median   4%)
cordial-4   [ 86,   7,   7,   7,   6,   7,   4]          throttled (tail median 6.5%)
cordial-6   [ 80,   7,   6,   6,   6,   6,   4]          throttled (tail median   6%)
cordial-7   [ 91,   7,   6,   6,   6,   6,   4]          throttled (tail median   6%)
cordial-8   [ 90,   7,   7,   6,   6,   7,   5]          throttled (tail median   6%)
cordial-9   [151,   9,   9,   8,   9,   7,   6]          throttled (tail median 7.5%)
```

**Three of nine pinned at ~104% of a core for as long as the run lasted** (up to
150 s, never recovering). Six of nine dropped to 4–7%. Both controls, every run,
settled to 8–9% and stayed:

```
mocktail-2  [43, 10, 10, 11, 10, 10,  9,  9,  8,  9, ...]  tail 8.7 ± 0.1 %
sober-2     [71,  9, 10,  9,  9,  9, 12,  7,  8,  7, ...]  tail 7.8 ± 0.1 %
```

Two things follow, and they point in opposite directions:

**When Cordial idles, it idles better than either control** — 6.3% against 8.7%
and 7.8%. There is no general inefficiency to find here.

**When it does not, it costs twelve times what the controls do**, permanently,
on the landing page, with nothing happening. One core at 100% is enough to spin
laptop fans, drain a battery, and make everything else on the machine feel
worse — which is the shape of the reported complaint.

`cordial-3` is the informative one: it shows the transition happening *late*
(`161, 105, 53, 6, ...` — pinned for ~20 s, then dropping). So the throttle is
not simply on or off from the start; it is a race that is sometimes lost
outright. The 3-in-9 rate is close to the "roughly one launch in three" figure
AGENTS.md records for another bug, which may or may not be coincidence.

**What I did not establish:** *why*. The obvious candidate is window occlusion
or focus — these ran on the maintainer's live session while they were using it,
so some windows will have been covered and some not, and I had no way to record
which. That confound is real and I cannot exclude it. Equally it may be the
engine's own idle throttle (the one that drops presents to exactly 1.0/s,
recorded in AGENTS.md) failing to engage. **Distinguishing those two is the next
experiment**, and it is cheap: run Cordial with the window deliberately raised
and then deliberately covered, several times each, and see whether the mode
follows visibility. I did not run it because it needs a controlled display the
Flatpaks cannot share (§6).

---

## 4. Cordial emits no engine log, and no timestamps at all

Sober and mocktail both surface the engine's FastLog with absolute ISO
timestamps, which is what makes their startup chains comparable to each other:

```
[I/Roblox] 2026-08-20T09:45:08.136Z,1.136420,... [FLog::SingleSurfaceApp] setStage: (stage:LuaApp)
info: Roblox: 2026-08-20T05:49:29.229Z,19.229647,... [FLog::AndroidGLView] nativeInitClientSettings
```

Cordial prints **zero** `FLog::`/`DFLog::` lines — only its own `[roblox]`,
`[cordial]`, `[android]` prefixes — so the engine-internal phase breakdown
cannot be compared to either control. Its nearest equivalent is
`[roblox] app ready: Startup` / `Home` / `RootSwitchNavigator` from
`NativeHelper::onAppReady` in `native/init_params.cpp`, which is a different
event and is not claimed here to be the same one.

Worse for the specific question asked: **no line of Cordial's output carries a
timestamp**. I checked all 265 `.log` files under `~/.cache/cordial-*`; 20
contain startup markers and **not one is timestamped**. So the claim that
*startup is getting slower as more is implemented* **cannot be tested against
anything already on disk** — there is no clock in the record. It can only be
tested going forward, either by timestamping the output or by timing runs
externally as this document does.

Every timing here is therefore sourced from outside the process (`date +%s.%N`
immediately before `exec`, plus `/proc` sampling), so these numbers stay valid
whether or not internal timestamps are ever added.

---

## 5. Frame rate: not comparable, and not measured

Not reported for any of the three, for two independent reasons, either of which
alone would be sufficient.

**No common frame counter exists.** Cordial has `vkQueuePresentKHR`
instrumentation; Sober and mocktail have none, and neither can be given any —
they are installed Flatpaks and adding a counter would mean modifying somebody
else's shipped binary. The engine itself logs no frame rate in any of the three;
grepping all three logs for `fps|frame rate|framerate|rendering frequency`
returns only `Register/Restoring rendering frequency`, which is a state change
and not a rate.

**Input cannot be driven identically.** AGENTS.md requires input to flow for the
whole measurement, forbids compositor-level injection, and sanctions exactly one
route — a nested headless compositor. That route is unavailable here because
Cordial cannot run in one (§6).

Present counts without input were not used, per AGENTS.md; no proxy was
substituted.

---

## 6. Cordial cannot run under a nested headless compositor

Reported separately because it is a Cordial bug, not merely an obstacle to this
measurement.

Under `mutter 50.4` started as
`mutter --headless --wayland --no-x11 --wayland-display=<name> --virtual-monitor 1280x720@60`,
**the compositor segfaults within about 5–15 s of Cordial connecting**, every
time it was tried (3 observations):

```
mutter[3755670]: Using Wayland display name 'wayland-cordial-bench'   19:56:33
systemd: cordial-bench-mutter.service: Main process exited, code=dumped, status=11/SEGV   19:56:38
```

Cordial then dies of `Gdk-Message: Error flushing display: Broken pipe`. It gets
as far as `app ready: PlatformAccountRouter` and `pumping the looper` first, so
this is not a failure to start.

**mocktail on the identical compositor ran a full 45 s and reported
`display refresh current=60.000 Hz`.** So the crash is specific to Cordial's
Wayland client, not to headless mutter or to this host.

Under `mutter --devkit` (nested rather than headless) Cordial instead panicked
3/3 with `Gtk has to be initialized before using libadwaita` — that one was a
genuine bug at the then-current commit and has since been fixed in `d30d7c0`;
the headless SEGV above was reproduced *after* that fix and is unrelated.

Consequence beyond this document: the sanctioned route for driving input at a
window that is not Cordial's own is a nested compositor, and Cordial cannot
currently live in one. Any future frame-rate or input-latency comparison is
blocked behind this.

---

## 7. Method, and what would invalidate it

Nine runs in the reported matrix (3 per client, interleaved
`cordial, mocktail, sober` × 3 so drift affects all three alike), 150 s each,
plus six extra Cordial runs of 70 s for the idle-mode denominator in §3.
Sampling at 2 Hz throughout.

The process set for each client is resolved differently, and getting this wrong
is the trap worth recording. **Sober runs the engine as a different uid in a
different session** (`/proc/self/exe`, uid 10156, ~1 GB RSS), so walking the
launched process's session or its descendants finds only the launcher and
reports **48 MB** — which reads as an extraordinarily light client rather than a
missed one. The fix is to select by the Flatpak's own systemd scope
(`app-flatpak-org.vinegarhq.Sober-*.scope`, recursively), which reports 1199 MB
across 8 processes. Cordial is selected by setsid session instead, because its
cgroup is the launching shell's scope and would sweep in half the desktop.
Descendant-walking fails for all three: `flatpak run` and `cordial-run` both
hand off to a child and let the parent go.

CPU is `utime+stime` summed across the set, differenced between samples, over
`SC_CLK_TCK`; reported as percent of **one** core, of 16.

Threats to these numbers, in the order I would attack them:

- **Occlusion is uncontrolled.** Runs were on a session in active use. This is
  the live confound for §3 and possibly inflates the variance everywhere else.
  Background load is reported per row so a figure taken during a busy period is
  at least visible as such; it stayed between 0.05 and 0.14 of the machine.
- **Another agent's `cordial-run` was resident throughout**, idling at 2.8% of
  one core. Small against 16 cores, and included in the reported background
  load, but not zero.
- **n=3 per client** for everything except the Cordial idle-mode split (n=9) and
  Cordial exit (n=9). The idle-CPU and exit figures for the controls are tight
  enough (SD ≤ 0.2% and ≤ 0.2 s) that more runs would not move them; the memory
  and growth rows are not.
- **Exit is SIGTERM, not a window close.** A user quits by closing the window;
  SIGTERM was chosen because it is the one stimulus all three accept
  identically. Cordial does real work on the way out (writes
  `cordial-unimplemented.log`, unregisters gamemode), which plausibly explains
  why it is slowest here while still *feeling* fastest. The reported belief that
  Cordial exits faster is **not confirmed by this measure**, and may simply be
  measuring something this measure does not.
