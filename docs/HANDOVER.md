# Handover

Written for whoever picks this up next. It is deliberately not a summary of the
README — it is the things that are true, that cost somebody time to establish,
and that are not obvious from reading the tree.

Start with [AGENTS.md](../AGENTS.md). It is short, it is the contract, and every
rule in it was bought with a wasted afternoon.

## If you forked this

Open a pull request too. Not instead — as well.

Forking is fine and the licence exists to make it fine. What costs everybody is
**divergence**, and divergence is not measured in commits. Ten small fixes
cherry-pick in an afternoon, because the commit messages here say what was
measured and are still legible a year later. One fork that restructured a
subsystem to fix something its own way is not mergeable at all, and at that
point that fork is the real project and this one is a museum.

So the ask is narrow: whatever you are doing in your fork, also put it up as a
PR, and keep the branch rebased on `main`. That turns "somebody has to
reconstruct what you did" into "somebody clicks merge".

The queue will sit for a while. The author has stepped back and intends to come
back to hand the project over rather than to resume it, so PRs opened now are
read when that happens rather than next week. That is a real cost to you and it
is stated plainly rather than dressed up — but the queue is the first thing a
new maintainer inherits, and a PR in it is worth far more than a commit in a
fork nobody knows about.

## Where this stands

Cordial loads Roblox's official Android x86-64 `libroblox.so` natively on Linux
— ported AOSP bionic linker, bionic/glibc shim, libjnivm in place of ART, and a
framework layer answering the platform calls the client makes.

You can sign in, stay signed in across restarts, load a game, move around, turn
the camera and hear sound. That is further than it sounds: for most of this
project's life the client rendered a login form nobody could type into.

**The single most valuable habit here is refusing to state a result you did not
observe.** Several commits exist only to retract an earlier claim, and they are
the good ones. `docs/NEXT.md` is the long-form working record; it is 1700 lines
and it is honest, including about the things that turned out to be wrong.

## What needs an account, and therefore needs you

A large fraction of what is still unverified is unverified for one reason: it
requires a signed-in client inside a running experience. No contributor working
without an account can settle any of it, and no automated agent should try.

- **Does a granted pointer lock actually turn the camera?** The lock is
  requested, the compositor's refusal path is honest, and the release paths
  work. That relative motion reaches the camera is `INFERRED`. Granting requires
  the pointer over the canvas, which requires a real mouse in a real session.
- **Does game audio reach OpenSL ES at all?** Sound demonstrably comes out of
  the bridge, measured off the sink monitor with a zeroed-buffer control. But at
  the Landing screen the engine asks for no audio whatsoever, and
  `--dump-classes` shows Roblox naming `org.fmod.AudioDevice` — FMOD's *Java*
  `AudioTrack` output path. If FMOD picks that inside an experience, the whole
  OpenSL bridge is the wrong door. Unresolved.
- ~~**The 1 fps report.**~~ **Withdrawn, and worth reading as a cautionary
  tale.** A report of 1 fps in an experience looked like a perfect match for the
  documented idle throttle, which drops presents to exactly 1.0/s when nothing
  is happening. A whole theory followed: input not reaching the engine in the
  way its idle heuristic counts, with pointer capture as the likely fix. It was
  the developer's own machine under memory pressure from unrelated applications,
  and the same session on a quiet machine was "buttery smooth" on an Intel iGPU.
  Nothing in Cordial was at fault. Two coincidences did the damage — the number
  matched a real documented behaviour exactly, and the throttle was fresh in
  everyone's mind. **Ask what else was running before believing a performance
  report, including your own.**

**Do not test with an account anyone cares about**, and keep test accounts on a
separate IP. Enforcement is automated, runs in waves, and associates accounts
sharing an address. The risk is collateral rather than causal.

## Per-profile network egress (ADR-016), and what is still missing

A profile's `network.json` can now say `"mode": "vpn-required"`, which refuses
to start the client at all unless `pvpn status` reports traffic actually
passing — checked at both the shell's `launch.rs` and `cordial-run`'s own
`main`, so starting the client directly cannot skip it. Read ADR-016 before
touching any of this; the short version is below, with what to trust and what
not to.

**What this actually guarantees, and does not.** A `vpn-required` profile will
never make even Cordial's own client-settings request on the machine's
ordinary route while believing itself protected. It does **not** isolate two
profiles running at once from each other — `pvpn`'s tunnel is one, global,
machine-wide route, and ADR-012's own two-windows-at-once case means a second
profile running alongside a `vpn-required` one shares whatever route is
active, VPN or not. Read the mechanism's name literally: a launch gate, not a
sandbox.

**Why it stops at a gate rather than a namespace, and this is the load-bearing
fact for whoever picks this up next.** `pvpn` drives Proton's own Linux client,
which brings its tunnel up as a NetworkManager connection — confirmed by
reading `bin/pvpn` in the sibling project, not assumed. NetworkManager is a
system service in the host's own network namespace, so the interface it
creates lands there regardless of which namespace the command that asked for
it was run inside. `ip netns exec cordial-<profile> pvpn up` would not produce
a namespace-scoped tunnel; it would produce the same machine-wide one `pvpn up`
always produces. A real per-profile tunnel needs `pvpn` (or something
alongside it) to hand over the WireGuard parameters an established connection
negotiated, so a second, namespace-local interface can be brought up directly
with `wg-quick`, bypassing NetworkManager entirely. Nothing in `pvpn` exposes
that today. **This is the concrete next step**, not "make the namespace work"
in the abstract — the namespace mechanics themselves (veth, routing, `ip netns
exec`) are ordinary and not the hard part; extracting a usable tunnel
definition out of a NetworkManager-managed Proton connection is.

**`unshare --net -- ip link` fails with `Operation not permitted` in an
ordinary unprivileged shell** — measured directly, on the machine this was
written on, not assumed from `CAP_NET_ADMIN` documentation. Building the
namespace path also means deciding how Cordial's packaging (Flatpak in
particular) gets that capability at all, which ADR-007's existing argument
against broad sandbox permissions bears on directly.

**`INFERRED`, and deliberately not leaned on:** whether Roblox's own curl
usage inside the engine honours `http_proxy`/`HTTPS_PROXY` is unresolved —
the structural path for it to work is real (same process, same `environ`,
`bionic/mod.rs` does not shim `getenv`), but whether Roblox's code explicitly
disables that via `CURLOPT_PROXY` was not observed and cannot be without
tracing a signed-in client. It does not matter for anything built here: the
decision not to ship an `http_proxy`-shaped setting rests on `RtcIoRna` (the
real-time game transport) not being HTTP at all, which holds regardless.

**A measurement trap this work tripped over and is worth adding to the list
above:** `CORDIAL_PROFILE_ROOT` was being guarded by three *different*,
mutually unaware mutexes — one each in `profile.rs`, `profile_switcher.rs`,
and (new, in this change) `launch.rs`. Each looked correct in isolation and
none of them stopped a different file's tests setting the same process-wide
variable at the same moment. It surfaced as one failure in several runs of
`cargo test -p cordial-shell`, reading another test's scratch directory back
mid-assertion — exactly the "passed anyway on the first run" shape
`profile.rs`'s own tests already warn about. Fixed by sharing one mutex
(`crate::PROFILE_ROOT_ENV`, declared in `main.rs`) across every file in that
binary that touches the variable. If a future file needs to point
`CORDIAL_PROFILE_ROOT` at a scratch directory in a test, use that one rather
than adding a fourth private mutex that looks like it works.

## A local Sober issue corpus for triage (ADR-017)

`tools/sober-corpus/` is a Deno tool, not Rust, and it does not touch the
engine — worth saying plainly because everything else in this file does.
It pulls vinegarhq/sober's issue tracker (another Roblox-on-Linux project,
closed source, so its GitHub repo is purely an issue tracker) into a local,
gitignored, PII-redacted corpus. Sober's tracker is prior art: real users
hitting real problems on the same engine Cordial loads, with a
maintainer's actual answer attached. `just sober-corpus-fetch` pulls it;
`just sober-corpus-derive` filters it down to the cases with a substantive
maintainer reply. Read `tools/sober-corpus/README.md` for day-to-day use
and ADR-017 for the reasoning.

**Run and verified, this session, not assumed:** a cold run against the
live tracker completed in 91.9s, 22 pages, ~44 GraphQL points of a
5,000/hour budget, and produced 2,195 issues (2,194 at the time the
original version of this tool was measured elsewhere; the tracker gained
one issue in the interim, which is itself a small proof the fetch is
talking to the real, current API). SIGKILLing it mid-run (confirmed dead:
exit 137) and re-running showed "Resuming an interrupted pass: 3 page(s) /
300 issue(s) already done this pass" and continued from page 4 rather than
restarting — the checkpoint scheme works as designed. A warm re-run with
nothing new to fetch cost one page, 2 points, 4.1s. `grep`ing the full
corpus for `/home/` turned up zero unredacted username paths (1,786
occurrences of the redaction marker, one benign non-match that was a
literal `"me"` string in a user's own malformed `$HOME`, not a real
username). Cross-checked against `gh api repos/vinegarhq/sober/pulls
?state=all` — zero of the repo's 20 real pull request numbers appear among
the 2,195 fetched issue numbers, confirming the GraphQL `issues` connection
this fetcher uses does not leak pull requests the way REST's `/issues`
endpoint would.

**Dropped on purpose:** an LLM-scoring harness (`evaluate.ts`, `model.ts`,
`sampling.ts`, a Cordial-context builder) existed alongside the fetcher in
the project this was ported from. It called an external LLM API to
auto-judge diagnoses against the derived set, and the founder explicitly
cancelled it ("No openrouter"). It is not in this tree and nothing here
depends on it — what ships is the fetcher and the maintainer-reply quality
filter that makes the raw corpus worth searching by hand.

## Open threads, with what is actually known

**Fullscreen offsets the content** — right and down on entering, then bleeding
up and left past the window edge on restore. Issue #7.

**The lead previously recorded here is disproved.** It said the swapchain extent
comes from `wayland::current().geometry()`, that geometry is written only by
`apply_resize`, and that `apply_resize` early-returns when the size is unchanged
— so the thing to measure was whether it fires with the fullscreen size at all.
It does. Measured 2026-08-06 with `CORDIAL_INSTR=1` and temporary logging added
around the early return:

```text
apply_resize(accept) 1370x765  -> 3440x1359      entering fullscreen
surface_caps currentExtent <- geometry() 3440x1359
apply_resize(accept) 3440x1359 -> 1370x765       restoring
surface_caps currentExtent <- geometry() 1370x765   (repeatedly, after)
```

666 `apply_resize` entries over the run, 650 of them early returns and 16 real
changes — but **both** fullscreen transitions are in the 16. The geometry
updates, and the extent handed to `vkGetPhysicalDeviceSurfaceCapabilitiesKHR`
tracks it in both directions. Neither the early return nor a stale extent
explains the offset, and the next person should not spend the afternoon there.

**What is now suspicious instead, and is not yet explained.** The fullscreen
surface was **3440x1359**. `org.gnome.Mutter.DisplayConfig` on the same machine
reports a 1920x1200 mode at scale 1.0, so that surface is both wider than the
display and an odd number of pixels tall. Every height in the run is odd — 721,
765, 1359 — while every width is even. That pattern wants explaining before any
fix is attempted; it is the kind of detail that turns out to be the whole bug or
a complete red herring, and this investigation has produced three of the latter
in one evening.

The measurement to take next is what the *compositor* configured versus what
Cordial applied: log the `xdg_toplevel::configure` width and height alongside
the `apply_resize` call they produce. If those disagree, the fix is in the
configure handling. If they agree, the offset is downstream of the surface size
entirely — canvas placement or the engine's own viewport — and neither of those
is where anyone has been looking.

**Textures and meshes render wrong, and the font is broken until it isn't.**
Unexplained. The leading suspect is Cordial's own MAILBOX-for-FIFO present-mode
substitution, which is what took the frame rate from a variable 35–50 to a flat
60 — if the engine's upload path relied on FIFO's pacing, MAILBOX would let it
sample buffers before their uploads land. `CORDIAL_PRESENT_MODE=fifo` is the
control and it has not been run. Ruled out already: ASTC support (this
developer's Intel iGPU has it) and a fall back to the software rasteriser
(`intel_icd` and `libvulkan_intel.so` are both installed).

**Typing into text fields draws nothing** until the field loses focus. The
per-keystroke sync path is understood; what is missing is an EditText-equivalent
overlay. `CORDIAL_TRACE_TEXT=1`.

**Shift+F5 does not open Roblox's stats menu.** Two candidates, neither
established: the key path sends an evdev keycode with Android `META_*` modifier
bits, which is a mixed vocabulary and mixed vocabulary is what cost four failed
keyboard theories before — or these are desktop-only debug shortcuts that the
Android build never wires at all. Settling it is cheap.

**Web views are unimplemented**, which is why a lot of Roblox's UI does nothing.
Needs `webkitgtk6.0-devel`; it is absent on the developer's host and present in
their distrobox.

**There is no Roblox Android build to download.** ADR-015 permits fetching and
the entire fetcher is built and proven — streaming, SHA-256, zip refusals,
metered detection. It has no URL because Roblox publishes no Android artefact:
`setup.rbxcdn.com/android/DeployHistory.txt` is 403, `client-version/AndroidApp`
is 500, and `roblox.com/download` offers Google Play and the Amazon Appstore and
no file. Sober does not fetch from Roblox either; it routes users through Google
Play. Aptoide is deliberately not wired — a mirror offering only a hash it
supplied itself is verification theatre.

## The application ID, and why it is not org.cordial.Cordial

**It was, until 2026-08-05, and that was a claim on a domain this project does
not own.** `cordial.org` was registered on 1999-04-16, is held through InterNetX
GmbH with `clientTransferProhibited` set, resolves to 185.148.170.48 behind
iwelt.de nameservers and does not expire until 2027-04-16. It is a
twenty-seven-year-old domain in active use. **"Just register cordial.org" is not
on the table**, and anyone who assumes it is will spend an afternoon finding that
out — which is the only reason this section still exists now that the rename is
done.

It mattered because **Flathub requires an application ID over a domain or forge
account you demonstrably control**, and rejects a submission claiming one it does
not. It is not a formality: the ID is what a user's machine trusts for the
lifetime of the install.

The ID is now **`io.github.luohoa97.Cordial`**, which needs no domain, matches
the homepage, and is what `<developer id="io.github.luohoa97">` in the metainfo
had been saying all along.

**It was done on the same day the remote first went live, on purpose.** A rename
after a package has users is a support burden; before, it is a `git mv` and a
sed. If you are reading this because you want to move to a real domain later,
that is fine — buy it first, and expect the cost below rather than the cost
above.

**What a rename touches**, recorded because the next one will need it:

- `app-id` in the manifest, and `APP_ID` in `.github/workflows/flatpak.yml`
- the manifest, desktop, metainfo and **both** icon filenames — all of which must
  match the ID, and one of which is the README banner rather than the app icon
- `<id>` and `<launchable>` in the metainfo
- `const APP_ID` in `crates/cordial-shell/src/main.rs`, which is also what makes
  Cordial single-instance
- two `include_str!` paths that pin the desktop file's contents from tests
- the `Icon=` URL in `packaging/cordial.flatpakrepo`
- the README's install, uninstall and plugin-directory paths
- **user data**, which is the one that bites. A Flatpak's data lives at
  `~/.var/app/<app-id>/`, so a rename orphans every profile, sign-in and
  extracted Roblox build behind it. Nobody had any at the time of this one. Next
  time somebody will, and they are owed either a migration or a release note
  saying plainly what is being left behind — and since [ADR-012](adr/ADR-012-profiles-and-instances.md)
  holds profiles under an `flock`, a migration running while an instance is up is
  its own problem.

**Do not do a blanket `sed` over the tree and call it done.** This rename did,
and it rewrote prose in this very file that was *about* the old identifier,
turning a true sentence into a false one. Grep the diff for the new string in
running text, not just in code.

## Flathub, and why it is not the plan

**Flathub's generative-AI policy does not allow applications containing
AI-generated or AI-assisted code, documentation, or any other content**, and it
extends to the submission itself: the pull request, manifest, metadata, patches,
build scripts and every review comment on it must not be LLM-generated either.
Submissions that violate it can be rejected without further review, and repeat
violations can earn a permanent ban.

**Cordial is squarely inside that.** Large parts of the tree, this document
included, were written with an LLM, and the git history says so in
`Co-Authored-By` trailers on dozens of commits. That was recorded deliberately
and it should stay recorded.

So the honest position is: **the GitHub Pages remote is Cordial's distribution
channel**, not a waiting room. Anything in the tree that reads as "until we get
on Flathub" is wrong and should be corrected where you find it.

The policy does say **exceptions may be granted for mature, well-maintained
projects.** That is the only route, and it is a route that starts with saying
plainly what is in the tree, in a pull request a human wrote. It is not a route
that starts with rewriting history to remove the trailers — that is deceiving the
reviewers the policy exists to serve, and it would cost this project the one
property that makes its claims worth anything.

**If somebody does pursue an exception**, three technical things still stand in
the way and are worth fixing regardless, because they are what a self-hosted
remote wants anyway:

1. **The build needs the network** ([issue #3](https://github.com/luohoa97/cordial/issues/3)).
   Flathub's builders have none. The submodule half is done — `libjnivm` and
   `mcpelauncher-linker` are pinned as `git` sources by commit — and the crate
   half is not. `flatpak-cargo-generator.py` from `flatpak-builder-tools` turns
   `Cargo.lock` into a `cargo-sources.json`, after which `--share=network` comes
   out of `build-options.build-args`. This also makes the local build
   reproducible, which is reason enough on its own.
2. ~~**The application ID.**~~ **Done** — `io.github.luohoa97.Cordial`, see the
   section above.
3. **No screenshots.** The metainfo has none, and Flathub's linter requires at
   least one. Cheap, and worth having for the software-centre listing on the
   existing remote whatever happens with Flathub.

**What would not have stopped you, for the record:** being a third-party Roblox
client is not itself disqualifying — Sober ships on Flathub as
`org.vinegarhq.Sober` — and Cordial's case is the easier one, since it ships no
Roblox code, asset or APK at all and the user supplies the client where Sober
fetches it. That was never the obstacle. The AI policy is.

## Why Roblox reports a client-integrity problem, so far

Open, and the most-asked question about this project. What follows is what has
been **established**, so nobody spends the afternoon twice.

**Play Integrity is not reachable from Cordial, and is not what differs from
Sober.** `docs/traces/` shows the real Android client calling Google Play
Services during startup — `requestIntegrityToken(IntegrityTokenRequest{nonce=…,
cloudProjectNumber=676451317595})` for `com.roblox.client`, binding to
`com.android.vending/…finsky.integrityservice.IntegrityService`, and Finsky
answering `Integrity key attestation record generated successfully`.

That happens in **the APK's Java layer, not in `libroblox.so`**. Cordial runs the
native engine and provides the platform beneath it; it never runs the app's Java
code, so the call is not made and cannot be. Measured rather than reasoned:
`--dump-classes` reports **3249** classes the engine asked Cordial for and **not
one** is Play-Integrity shaped. Sober is in exactly the same position and shows
no integrity error, so whatever differs, it is not this.

**Do not try to produce that token.** It is a device and app attestation from
Google, and the only ways to make one appear are forging it or relaying one from
a real device. Both are circumventing an anti-tamper control rather than making
Cordial an honest client, and both are out of scope here for the same reason
in-process hooking is ([ADR-001](adr/ADR-001-in-process-hooking.md)).

**What was actually different, and is now fixed.** Sober's own log on this
machine, and the capture from real Android, both contain:

```text
rbx.JNIRobloxSettings: Setting default app policy file:
  content/guac/defaultConfigs/GuacDefaultPolicy-GlobalDist.json
```

Cordial never called `nativeSetDefaultAppPolicyFile`, so the engine came up with
no app policy at all. That was issue #5. It is wired now, relative rather than
absolute because both the capture and Sober log it that way, and the engine
emits the identical line and still reaches `APP_READY Landing`.

**Whether that fixes the integrity report is not established.** It needs a
signed-in join to see and that run has not happened. A gap being real is not
evidence it is the gap that broke anything.

### What 304 actually looks like, with a Sober control beside it

Captured 2026-08-06 on the `CordialTest` profile, joining place 17625359962,
against a Sober run from 2026-08-05 that played for seventeen minutes. Both
lines are from the engine's own log, times are seconds since process start.

```text
CORDIAL  248.658  Transport selection: useRbxTransportEnabled=false,
                  selectedTransport=RakNet, rccPort=52398
         248.973  Connection accepted from 128.116.51.33|52398
         253.672  Error RbxTransportDummyClient: Failed to establish
                  connection to 128.116.51.33:41297, reason NoResponse
         309.161  Disconnect reason received: 304

SOBER    258.157  Transport selection: useRbxTransportEnabled=false,
                  selectedTransport=RakNet, rccPort=63935
         258.203  Connection accepted from 128.116.51.33|63935
         260.174  Error RbxTransportDummyClient: Failed to establish
                  connection to 128.116.51.33:43197, reason NoResponse
        1053.358  DisconnectClientInitiated — the player left
```

**`RbxTransportDummyClient … NoResponse` was written up here as benign. That
was wrong, and it is the opposite of benign — it is the strongest lead there
is.** The retraction is left in place rather than tidied away, because the
reasoning that produced it is a trap worth seeing.

The first control used a Sober log in which the *first* join also logged
`NoResponse`, which looked like proof the failure did not matter. Read further
and that join was abandoned 1.2 seconds later for a different place, and the
join that actually stuck had the DummyClient **connected**. One log, one glance,
one wrong conclusion — from a control that was real but not read to the end.

Re-run 2026-08-06 with both clients pointed at the same place by deep link,
same machine, same network, same server IP:

```text
SOBER    54.616  DummyClient will connect to 128.116.51.33:57479
         54.654  Connected to server at 128.116.51.33:57479      (38 ms)
         54.654  Started ping thread (interval 1000 ms, 64 bytes)
         54.654  Started time sync thread (interval 5000 ms)
        306.058  Disconnection Notification. Reason: 285 — the player left
                 no 304, four minutes in the place

CORDIAL   4.121  DummyClient will connect to 128.116.51.33:41297
          4.156  Connection accepted from 128.116.51.33|52398   (RakNet, fine)
          9.122  Error: Failed to establish connection, reason NoResponse
                 — a five second timeout, no connection, no threads
         64.292  Disconnect reason received: 304
```

**Sober connects that socket in 38 milliseconds. Cordial times out after five
seconds and is disconnected 60.1 seconds after its RakNet accept.** RakNet
itself — also UDP — connects fine in both, so this is not "UDP does not work".
Something specific to the `RtcIoRna` path fails only under Cordial.

The ping thread and the time sync thread are the shape to keep in mind: a
liveness channel that never opens, and a kick almost exactly sixty seconds
later. That is a hypothesis and not yet a mechanism — nobody has established
*why* the connect times out.

**What the control does establish.** The two clients are indistinguishable
through connection: same transport selection, same RakNet accept, same
harmless probe failure. The join *succeeds*. Cordial is then disconnected
**60.5 seconds after the connection was accepted** — `connectionTime 248689`
against `timeMS 309172` — while Sober is never disconnected at all.

So 304 is not a handshake or an integrity check at load. **It is something
periodic that Cordial fails roughly a minute into replication**, at which point
the server sends a disconnect whose text talks about missing or corrupted files.
The message is worth reading as a category rather than a diagnosis: nothing in
Cordial's install is missing, and the same binary reaches the same server the
same way that Sober does.

**One behavioural difference worth the next look, and no more than that.** On
Sober's second join the DummyClient *did* connect and started two threads:

```text
RbxTransportDummyClient  Connected to server at 128.116.51.33:54811
RbxTransportDummyClient  Started ping thread (interval: 1000 ms, payload: 64 bytes)
RbxTransportDummyClient  Started time sync thread (interval: 5000 ms, …)
```

Cordial has never reached that on any join observed so far. Whether it matters,
or is simply which server answered on the day, is **unestablished** — it is
recorded because it is the only place the two logs diverge, not because it is
believed to be the cause. A ping thread and a time-sync thread failing to exist
is at least the right shape for something that kills a session sixty seconds in,
and that is exactly the kind of resemblance that has been wrong twice already.

### The transport probe fails on some servers and not others

**A previous version of this section said the transport socket is "written and
never read" and blamed `RtcIoRna`'s event loop. That was wrong, and the very
capture proposed to test it is what disproved it.** The retraction stays visible:
this is the third lead in this investigation to dissolve, and all three dissolved
the same way — a real observation from one run, generalised into a property of
Cordial.

What was observed, across three joins on 2026-08-06:

```text
18:01   server 128.116.51.33   DummyClient NoResponse    304 at 60.1 s
18:18   server 128.116.51.33   DummyClient NoResponse    304 at 64.3 s
18:47   server 128.116.63.33   DummyClient CONNECTED     no 304, ran 96 s
                               ping thread + time sync thread started
```

And with `strace -f -e trace=desc,network` over the third:

```text
fd 140 (DummyClient)   epoll_ctl registrations: 1     recv: yes
```

**The socket is epoll-registered and the event loop works.** The failing runs are
not Cordial failing to poll; two of three joins simply got no answer from one
particular server while a third answered in 432 ms.

**What is established.** Cordial can connect that socket — nothing structural
prevents it. 304 has never been observed on a run where the DummyClient
connected. The failures correlate with the server address, not with the build.
Sober's own logs contain a `NoResponse` too, on a join it abandoned 1.2 seconds
later for a different server.

**What is not established, and must not be written down as though it were:** why
`128.116.51.33` does not answer, whether the unanswered probe *causes* the 304 or
merely accompanies it, and whether Sober's advantage is different behaviour or
simply better luck with which edge it lands on.

**The next capture is cheap and the baseline now exists**, which is what was
missing before: rejoin until the client lands on a server that does not answer,
take the same `-e trace=desc,network` capture, and diff `epoll_ctl` and `recv` on
that fd against the good run above. One failing capture against one known-good
capture answers it. Guessing in between does not.

**Still unchecked, and the obvious next candidates.** Comparing Sober's
`appData` with Cordial's, Sober has `ClientSettings/` and `rbx-storage.db`;
Cordial has neither. And [issue #2](https://github.com/luohoa97/cordial/issues/2)
— User-Agent and base URL not matching what the real client sends — is still
open, with the exact string sitting in the capture:

```text
Mozilla/5.0 (…; 13) AppleWebKit/537.36 (KHTML, like Gecko)  ROBLOX Android App
2.732.1043 Tablet Hybrid()  GooglePlayStore RobloxApp/2.732.1043
(GlobalDist; GooglePlayStore)
```

Matching that is honest self-description — it is what the official Android
client sends, and Cordial *is* running that client. Forging an attestation is
not the same kind of thing, and the line between them is the one to hold.

## Traps that have already caught people

**Do not use present counts as a frame rate.** Every figure recorded before
2026-08-02 is an idle throttle integrated over a window, and several were quoted
as evidence.

**Do not measure timing under `WAYLAND_DEBUG=1`.** Three findings taken that way
vanished on untraced repeats minutes later.

**`CORDIAL_TRACE=1` aborts the engine.** It wraps variadic functions ABI-unsafely.

**`/proc/locks` names the wrong process.** Its PID column is whoever *acquired*
the lock, which `Claim::hand_to` makes the launcher — and the launcher has
usually exited. Scan `/proc/*/fd` instead. Observed: `/proc/locks` naming a PID
that no longer existed while a live process held the descriptor.

**Cumulative counters are not rates.** `pgsteal_kswapd` and friends integrate
since boot; reading one as a rate produced a confident and wrong diagnosis of
system-wide thrashing in this very project's history. Sample twice.

**`mutter --headless` segfaults** within a second of Cordial's window mapping,
on unmodified HEAD, with and without GPU rendering, while `gtk4-demo` in the
same nested compositor runs indefinitely. AGENTS.md offers nesting a headless
compositor as the way to drive Cordial's own window without touching the
developer's session, and it does not currently work. That is why
`crates/cordial-runtime/examples/pointer_capture_probe.rs` exists.

**Never synthesise input at the compositor.** `XTestFake*`, `ydotool`,
`wlr-virtual-keyboard`, the RemoteDesktop portal — all land on whatever has
focus, which is the developer's session. This has hijacked a developer's cursor
once already, mid-session.

## Layout

    crates/cordial-runtime   the loader, bionic shim, Android framework layer
    crates/cordial-shell     the launcher; also owns the shared window definition
    crates/cordial-plugins   registry, dependency resolution, unpacking
    crates/cordial-update    version check, download, verification, metered
    native/                  C++ shims: OpenSL ES, PipeWire, Java classes
    third_party/             the AOSP bionic linker port
    docs/adr/                decisions, including the reversed ones
    docs/traces/             a logcat capture of the same APK on real Android

**`docs/traces/` is the single most under-used thing in this repository.** Grep
it before disassembling anything. Note that the startup log is gzipped, so a
plain `grep -r` silently misses it — use `zgrep`, and do not read a zero hit as
evidence of absence.

## The two profile modules

`cordial_shell::profile` and `cordial_runtime::profile` implement the same
contract twice. That is not a design; `cordial-runtime` depends on
`cordial-shell` for `host_window`, so the dependency cannot be inverted without
a cycle. The shell's copy is the live one — `cordial-run` never calls its own.
Unifying them is a genuinely good first contribution.

## House style

Comments explain *why*, anchored in the failure that motivated the code. Commit
messages say what you measured, and they are long here on purpose. British-ish
prose, no emoji, no bullet-list comment blocks. Read the surrounding file before
writing; the voice is consistent and matching it is not optional.

Arguing with an ADR is welcome — ADR-004 was reversed exactly that way. What is
not acceptable is quietly contradicting one in code.
