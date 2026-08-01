# AGENTS.md

Instructions for any coding agent working in this repository. Human contributors
should read [CONTRIBUTING.md](CONTRIBUTING.md), which says the same things at
more length and explains why.

Cordial loads Roblox's official Android x86-64 `libroblox.so` natively on Linux:
a ported AOSP bionic linker, a bionic/glibc shim, libjnivm in place of Android's
ART, and a framework layer that answers the calls the client makes into the
platform.

## The one rule

**Grep `docs/traces/` before disassembling anything.** It holds a logcat capture
of the same APK on real Android. When a question comes up about what the engine
expects, that capture is a lookup, not an investigation.

Over one long session, **nine consecutive conclusions drawn from reading the
stripped binary were wrong**, and every conclusion drawn from running something
held up. This is the single most expensive mistake available here, and agents are
unusually prone to it because reasoning from a binary feels like progress.

## Verify by running. Do not report what you have not observed

A claim about this engine is worth what it was measured with.

- **Never state a result you did not see.** Not "this should now work", not "the
  fix is complete" — run it and paste what it printed. If you cannot run it, say
  that plainly.
- If a claim cannot be tested, label it **`INFERRED`** in the comment and in the
  pull request. That is an acceptable state. Presenting it as established is not.
- Stability and timing claims need **repetition**. One clean run is not a result;
  one bug here reproduced on roughly one launch in three.
- Use a **control**. Show the behaviour changes when the thing you changed is
  turned off, in the same session.

If you find something already written down is wrong — a comment, `docs/NEXT.md`,
an ADR — **say so plainly and correct it**. Several commits exist only to retract
an earlier claim. That is the highest-value contribution here.

## Which issue this is

Route work by shape. The templates in `.github/ISSUE_TEMPLATE/` carry the
diagnostics for each.

| Shape | Template | Recognise it by |
|---|---|---|
| A new Roblox build fails to load, or needs symbols we lack | `roblox_update.md` | `cannot locate symbol` at load, or a called stub on exit |
| An Android expectation is unanswered, so a feature silently does nothing | `broken_feature.md` | `Constructed Unresolved symbol` in the jnivm log; audio is the live example |
| Something Cordial or a plugin should be able to do | `feature.md` | No engine call is involved |
| Cordial misbehaves at something it already does | `bug_report.md` | It used to work, or clearly should |
| You established or disproved something | `finding.md` | The output is knowledge, not code |

**Missing symbols.** `docs/analysis/undefined-symbols.tsv` generates the stub
table. To find what a build needs that it lacks:

```bash
readelf --dyn-syms -W /path/to/libroblox.so \
  | awk '$7=="UND" {print $8}' | sed 's/@.*//' | sort -u > /tmp/new.txt
cut -f2 docs/analysis/undefined-symbols.tsv | sort -u > /tmp/old.txt
comm -23 /tmp/new.txt /tmp/old.txt
```

Data symbols fail the `DT_NEEDED` walk at load time rather than at first use, so
one missing name stops the whole client.

## Never make a stub lie

A stub that returns success is worse than one that returns failure. The engine
proceeds on an answer that is not true and fails somewhere with no relationship
to the cause. `native/opensles.cpp` reports
`SL_RESULT_FEATURE_UNSUPPORTED` rather than handing back a dead engine object;
that is the pattern. Reporting failure keeps the gap where someone can find it.

## Permanently out of scope

**No in-process code execution against the Roblox process.** No hooking, no
memory patching, no injected script environment, and no API by which a plugin
could request one. Not disabled — *absent*, so there is no primitive to extract
or re-enable in a fork. [ADR-001](docs/adr/ADR-001-in-process-hooking.md),
[ADR-003](docs/adr/ADR-003-plugin-isolation.md).

**No Roblox code, ever.** No APK, asset, or decompiled material committed,
vendored, or pasted into an issue. Observing a running binary is fine and is how
nearly everything here was established — call order, load order, argument shapes,
syscalls, timing, and method prototypes declared in the dex. Transcribing a
decompilation of *how it implements* something is not. The line is not the tool,
it is what you take away. Any `decompiled/` directory is off-limits.

**No client-side integrity flags, watermarks, or obfuscation-as-security.**

Asset overlays **are** in scope, non-destructively and off by default
([ADR-010](docs/adr/ADR-010-plugin-asset-overlays.md), superseding ADR-004).

## Plugin capabilities expose effects, not channels

A plugin never receives a socket, a D-Bus connection, or a file descriptor.
Cordial holds the permission and performs the effect; the plugin sends a payload.
`presence.set` takes a presence structure and Cordial owns the Discord socket.

A broker should be a payload type and an effect. If a proposed capability needs a
design document, it is too broad and wants splitting.
[ADR-007](docs/adr/ADR-007-host-resources-are-brokered.md).

## The ADRs are the decision record

`docs/adr/` records what was decided and why, including reversals. Before
proposing something that contradicts one, read it.

**Arguing with an ADR is welcome and expected** — ADR-004 was reversed exactly
that way, by someone pointing out the reasoning did not hold. What is not
acceptable is quietly contradicting one in code. If a change makes an ADR wrong,
update the ADR in the same change, and mark the old one superseded rather than
deleting its reasoning.

## Style

Read the surrounding file before writing. This codebase has a consistent voice
and matching it is not optional.

- **Comments explain *why*, anchored in the failure that motivated the code.**
  Not what the line does. The good ones name the bug that would otherwise recur.
- British-ish prose. No emoji in code or comments. No bullet-list comment blocks.
- **Commit messages say what you measured**, not only what you changed. They are
  long here on purpose.
- Prefer correcting a stale comment over leaving it. A comment that lies costs
  more than no comment.

## Build and test

```bash
cargo build --release      # Clang required; AOSP bionic does not build with GCC
cargo test --workspace
```

Both must pass. Run them; do not assume.

Running the client needs an APK the user supplies — Cordial ships none:

```bash
cargo run --release --bin cordial-load -- \
  --lib-dir /path/to/lib/x86_64 --apk /path/to/base.apk \
  --host-libc --game-activity --run 30
```

Useful switches: `CORDIAL_TRACE_TEXT=1` for text entry, `CORDIAL_TRACE_PATHS=1`
for path-taking libc calls, `--dump-classes <file>` for the Java surface Roblox
asked for. **`CORDIAL_TRACE=1` aborts the engine** — it wraps variadic functions
ABI-unsafely. Do not reach for it.

## Two practical cautions

**Never synthesise input with `XTestFake*`, `ydotool`, `wlr-virtual-keyboard`,
the `RemoteDesktop` portal, or anything else that injects at the compositor.**
It lands on whatever has focus, which is the developer's session. This has
already hijacked a developer's cursor once mid-session.

This rule used to end "window-targeted `XSendEvent` only", which no longer means
anything — [ADR-011](docs/adr/ADR-011-wayland-and-libadwaita.md) is Wayland, and
Wayland has no window-targeted injection. To drive Cordial's own input, call
`input::pass_key_event`/`input::pass_text` directly; Cordial is the client, so
there is nothing to send through. To drive somebody else's window, nest a
headless compositor on its own `WAYLAND_DISPLAY` and inject inside that.

**Do not test with an account anyone cares about**, and keep test accounts on a
separate IP. The risk is collateral rather than causal: enforcement is automated,
runs in waves, and associates accounts sharing an address.
