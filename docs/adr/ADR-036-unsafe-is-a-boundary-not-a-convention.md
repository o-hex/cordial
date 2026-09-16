# ADR-036: The unsafe/safe boundary is a lint, not a convention

**Status:** Accepted, 2026-09-16

## Context

`unsafe` in this workspace was already concentrated in two crates by
convention: `cordial-runtime` and `cordial-linker-sys` are where Roblox's
`libroblox.so` calls into Cordial with raw pointers and where Cordial calls
back into the bionic linker, and the other three crates were supposed to stay
clean of it. Nothing enforced that. A textual count on `main` before this
change:

    crate                unsafe sites   SAFETY comments
    cordial-runtime          542            276
    cordial-linker-sys       111             97
    cordial-shell             15             10
    cordial-plugins            1              1
    cordial-update              1              0

`cordial-plugins` and `cordial-update` each carried exactly one `unsafe`
block, both raw libc calls that had nothing to do with the ABI edge --
`libc::flock` for an advisory lock and `libc::kill(-pid, SIGKILL)` for a
sandboxed process group. Nobody put them there to touch memory unsafely; they
were reached for because they were the obvious way to make one syscall, in a
crate whose header comment says nothing about a boundary the two lines then
quietly crossed. A convention nobody checks is a convention that erodes one
syscall at a time, and the only thing that would have stopped either line
landing was a compiler that refused to build it.

## Decision

**`unsafe_code` is `deny` at the workspace level, and every one of the five
member crates opts in with `[lints]\nworkspace = true`.** `cordial-runtime`
and `cordial-linker-sys` each carry their own crate-root
`#![allow(unsafe_code)]`, with a comment saying why, at every compilation
unit that needs it -- that means both the library and, for `cordial-runtime`,
the `cordial-run` binary's own crate root in `src/bin/load.rs`, since the
lint attribute has to be present at each crate root a `[lints]` table covers.

`cordial-shell` also carries the allow, but for a narrower reason than the
other two -- see "`cordial-shell` is not a third ABI-edge crate" below.
`cordial-plugins` and `cordial-update` carry `#![forbid(unsafe_code)]`,
stronger than a bare `deny`: a `forbid` cannot be locally overridden by a
`#[allow(unsafe_code)]` on some future function, so reopening either crate to
unsafe code takes deleting a line that names this ADR, which is a change a
reviewer will actually see.

## Why a lint, not a convention

A convention is enforced by whoever remembers to look, and this codebase has
already shown that nobody reliably does -- both stray sites in
`cordial-plugins` and `cordial-update` passed review. A lint is enforced by
the compiler on every build, including one run by someone who has never read
this ADR. That is the entire difference this change makes: nothing about
*where* unsafe code is allowed changes, only who is responsible for noticing
when it appears somewhere it should not.

## Why the Asterinas model does not map here

Asterinas and similar frameworks push `unsafe` down to a thin kernel of
primitives and build everything else, including the interface an untrusted
caller sees, out of safe code on top -- the unsafety is *underneath* the
public surface, hidden from a caller who never has to see it.

That model assumes the boundary between safe and unsafe is one this project
gets to draw. It is not, here. `libroblox.so` is not a caller Cordial exposes
a safe API to -- it is calling *into* Cordial's stub table with raw pointers
and expecting particular ABI-shaped answers back, because it was built for
Android's ART and JNI, not for anything Cordial designed. The unsafety at
that edge is not an implementation detail Cordial chose to hide beneath a
safe wrapper; it **is** the interface, dictated by a binary Cordial did not
write and is contractually barred from modifying (see AGENTS.md's rule
against touching Roblox code). A safe wrapper around `symtab::build`'s stub
table would still need, underneath it, exactly the same raw pointer
dereferences this ADR is not trying to make disappear -- the choice is
between naming that plainly at the crate that does it, or hiding it one layer
further down and calling the outer layer clean.

## `cordial-shell` is not a third ABI-edge crate, but it is not zero either

`cordial-shell`'s 15 unsafe sites are not ABI-edge unsafe in the sense the
two crates above are -- nothing in it exists because Roblox is calling in.
But three real things do belong there and have no safe wrapper available:
`audio_devices.rs` calls the one C ABI `native/pipewire_backend.h` exposes,
so the settings window can enumerate audio sinks the same way the client
enumerates them, without a second implementation of "list the sinks" to
disagree with the first (see that module's own header). `profile.rs` takes
ADR-012's advisory lock on a profile directory with `flock`/`fcntl` and signals
a launched client with `kill`, all on raw fds and a raw pid, because that is
what an advisory process lock is made of. `host_window.rs` sets
`GDK_BACKEND` before GTK reads it, which `glib` marks unsafe because
`setenv` racing a concurrent `getenv` is undefined behaviour on the C side of
that call, not because Rust is doing anything unusual.

None of it is memory-unsafe Rust hiding behind an `unsafe` marker to avoid a
lint; all of it is a syscall or an extern call with no safe wrapper in this
tree. Rewriting fifteen call sites through a hand-rolled safe shim, just so
`cordial-shell` could carry `forbid` like the other two, would be exactly the
churn the next section rejects, at a tenth of the scale. `cordial-shell`
therefore gets the same `#![allow(unsafe_code)]` treatment as the two ABI-edge
crates, with its own comment explaining the narrower reason, rather than the
`forbid` given to `cordial-plugins` and `cordial-update` once their one stray
site each was removed.

## Why the two strays were fixed with `rustix`, not moved to a new crate

One alternative considered for the two stray sites was giving them a home
of their own -- a small crate for "syscalls other crates need," so
`cordial-plugins` and `cordial-update` could `forbid(unsafe_code)` without
touching the calls themselves. That was rejected: it moves two lines of
`unsafe` into a place chosen for the sake of a lint working. Nothing about
`libc::flock` or `libc::kill(-pid, SIGKILL)` was any easier to review, verify
or maintain in a crate whose only reason to exist was one function each. It
would have been a diff that made the "0 sites" table entry technically true
and left the actual work no more scrutinised than before -- churn for a
cosmetic win, and this workspace has enough real unsafe surface without
manufacturing a crate to hold two lines nobody will ever look at again.

`rustix` was already resolved into `Cargo.lock` at version 1.1.4, pulled in
transitively by `async-io`, `async-process`, `async-signal`, `polling`, `tempfile`
and `zbus` -- so making `cordial-plugins` and `cordial-update` direct users of it
adds a dependency edge, not a second copy of one to build or audit.
`rustix::fs::flock` wraps the same `flock(2)` `provider::exclusive` was already
calling, turning the C `-1`-on-failure convention into an `Err` with no `unsafe`
block left at the call site. `rustix::process::kill_process_group` wraps the same
`kill(-pid, sig)` `Plugin::kill` was already calling -- it takes the *positive*
pid and negates it internally, so the exact process-group semantics that
justify the negative pid in the first place (see `host.rs`'s own comment on why
a single `SIGKILL` to the child alone is not enough) are preserved without this
crate spelling out the negation itself.

## Why not `std::fs::File::try_lock`

`std::fs::File::try_lock` does the same `flock(2)` with no extra dependency at
all, and would have been the simpler fix. It stabilised in Rust 1.89. This
workspace pins `rust-version = "1.75"` in the root `Cargo.toml`, deliberately:
`.github/workflows/release.yml` carries comments at the AppImage and Flatpak
build steps explaining that the Flatpak runtime this ships against trails
current stable, and breaking that pin to remove one `unsafe` block is a worse
trade than keeping a small, already-present dependency. If the Flatpak
runtime's baseline ever moves past 1.89, `try_lock` becomes the better choice
and this is the ADR that should be updated to say so.

## What is a lint and what is a ratchet

Three lints, one of them workspace-wide policy and two of them measured
against what `main` actually contained on 2026-09-16 rather than picked in
advance:

**`unsafe_code = "deny"`** at the workspace level, per crate carve-outs as
above. This is the actual boundary and is not a ratchet.

**`clippy::undocumented_unsafe_blocks = "warn"`.** `cargo clippy --workspace
--all-targets -- -W clippy::undocumented_unsafe_blocks` (with
`-A clippy::not_unsafe_ptr_arg_deref`, see below) found **190** unsafe blocks
across the workspace with no `SAFETY:` comment above them -- almost all in
`cordial-runtime` and `cordial-linker-sys`, which is what accumulates when
`unsafe` is merged with no lint checking for a comment. That gap does not
close in one change, so this starts at `warn`: a number to bring
down, not a target already met. `tools/unsafe-audit.py --clippy` reports the
current count on demand; `tools/unsafe-audit.py` alone gives a fast,
build-free approximation for a quick check between clippy runs.

**`unsafe_op_in_unsafe_fn = "deny"`.** This lint requires every raw operation
inside an `unsafe fn` body to sit in its own nested `unsafe { }` block, rather
than treating the whole body as implicitly unsafe (edition 2024 makes that
the default; edition 2021, which this workspace is on, does not). Set to
`"deny"` and built before fixing anything, it found **16** real sites, all in
`cordial-runtime` -- `cordial-linker-sys`, `cordial-shell`, `cordial-plugins`
and `cordial-update` had none. That is a small, entirely mechanical diff, not
the hundreds a workspace with roughly 650 unsafe sites might suggest: most
`unsafe fn` bodies here already wrapped their raw operations in an inner
`unsafe {}` out of habit, and the 16 that had not were fixed in this change
rather than deferred. **This is the opposite of what was expected going in**
-- the task briefing for this change assumed turning the lint on "may
produce a large diff across 542 sites" and asked that the diff be measured
before committing to a level; measured, it was small enough to fix outright,
so `unsafe_op_in_unsafe_fn` is `"deny"`, not a ratchet.

## A pre-existing clippy failure, found and not fixed here

Getting either clippy measurement above required `-A clippy::not_unsafe_ptr_arg_deref`.
Without it, `cargo clippy --workspace` does not complete at all: `cordial-linker-sys`
alone has 50 public functions clippy reports as "might dereference a raw
pointer but is not marked `unsafe`", and `not_unsafe_ptr_arg_deref` is in
clippy's `correctness` group, which is deny-by-default -- so it stops the
whole workspace's clippy run before `cordial-runtime` or `cordial-shell`, both
of which depend on `cordial-linker-sys`, are even reached. This is pre-existing
on `main`, unrelated to unsafe-block documentation, and out of scope for this
change: fixing 50 call sites' public signatures is its own piece of work, not
a side effect of a lints change. It is filed as a follow-up rather than folded
in here -- see the tracking issue below.

## A worked example of the FFI-edge pattern, not a rewrite of it

`cordial-runtime` has roughly 350 `extern "C"` functions, most of them
`libroblox.so`'s call-in surface. Auditing all of them to validate every raw
pointer, convert to `&CStr`/slices at the boundary, and delegate to a safe
inner function is real work worth doing, and far too large to fold into a
lints change. Three call sites that all reimplemented the same "borrow a
request string, write a response back into a caller buffer" shape --
`linking::on_open_url`, `permissions::reply`, and `webview::on_open_window`
-- were converted to share `ffi_util::borrow_request` and
`ffi_util::write_response` instead of each open-coding the same null check,
`CStr::from_ptr` and length-checked `copy_nonoverlapping`. That is the pattern
this ADR asks the rest of the crate to follow incrementally: see
[issue #55](https://github.com/luohoa97/cordial/issues/55) for the rest of it.

## Consequences

A crate outside the two (or, narrowly, three) named here that adds an
`unsafe` block now fails to build, with an error naming exactly which lint
and which line, rather than passing review because nobody happened to look.
`cordial-plugins` and `cordial-update` cannot reopen without a code change
that has to explain itself against this ADR by name. `cordial-runtime` and
`cordial-linker-sys` keep the unsafe surface the ABI edge actually requires,
now with a documented, ratcheting count of how much of it lacks a SAFETY
comment, and a script that reports that count on demand instead of by hand.
