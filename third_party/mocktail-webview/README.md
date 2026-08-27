# mocktail's webview, vendored

Upstream: <https://github.com/komaruworld/mocktail>, Apache-2.0,
Copyright 2026 komaruworld. Licence text in `LICENSE` beside this file.

These files are **unmodified copies** kept as the reference for Cordial's own
web window. They are not compiled. Nothing in Cordial's build reads this
directory; `crates/cordial-linker-sys/build.rs` compiles `native/`, not here.

## Why they are here rather than read from a checkout

Because the alternative was worse. The protocol Cordial has to speak is
Roblox's, and it was read out of the engine directly — see
`crates/cordial-runtime/src/webview.rs`, which reports the whole vocabulary at
startup. What these files supply is the other half: how somebody who already
made this work handled the parts the protocol does not describe. The JavaScript
bridge and its origin check, the window lifecycle, cookie sharing with the
signed-in session, and what a `mutateWindow` actually does to a live view.

Working that out from scratch, when a compatible licence permits reading the
answer, would be a choice to spend a week producing something worse.

## What is taken and what is not

Cordial is Rust with a GTK4/libadwaita shell (ADR-011); mocktail is C++ with
SDL3 under a GTK shell, and its helper is a separate process with its own
`main()`. So this is not a drop-in — the port is a rewrite in a different
language against a different window model, and the parts worth taking are the
decisions rather than the lines.

Two of those decisions Cordial keeps:

* **Every bridge message is origin-checked.** This window is where a user signs
  in and where payment happens. A page that can post arbitrary commands at the
  engine is the entire security boundary, and mocktail rejects untrusted origins
  explicitly rather than trusting the page.
* **The web content is isolated from the engine.** mocktail achieves it with a
  separate helper process.

One it does not: Cordial hosts the view in-process as an `AdwDialog`, because
WebKitGTK already runs page content in its own sandboxed processes. A hand-rolled
helper adds a second IPC layer for isolation WebKit provides anyway, and costs
the attached dialog that `getHideHeaderKey` and `getShowDomainAsTitleKey` show
the protocol expects.

Any Cordial file derived from these carries its origin in its own header, per
Apache-2.0 section 4(b).

## Nothing here is compiled

**No build script references these files and no binary contains them.** They are
the reference for code that was derived from them, which lives in Rust:

    crates/cordial-shell/src/webview.rs          the window
    crates/cordial-shell/src/webview_policy.rs   the security rules, theirs
    crates/cordial-runtime/src/webview.rs        the cookie handling

Each of those states its own origin in its header, which is where Apache-2.0
section 4(b) is actually satisfied -- a NOTICE at the repository root is not
where somebody reading a source file looks.

**The licence does not require this copy to exist.** Apache-2.0 asks for a copy
of the licence, notice that files were changed, retained attribution, and the
NOTICE file. It does not ask you to vendor the original. This is kept as the
provenance trail, deliberately, for two reasons:

It is what makes "derived how, and from what" checkable by somebody who was not
there. This project's whole practice is checking a claim against a reference
rather than against a memory of one.

And one argument in the derived code reasons from what is *absent* here.
`crates/cordial-runtime/src/webview.rs` sets its own bound on a
`.ROBLOSECURITY` cookie precisely because `webview_roblox_cookie.h` is not among
the vendored files, so their equivalent constant cannot be read and only their
reasoning transfers. Delete this directory and that becomes a magic number with
a citation pointing at nothing.

Its presence has been mistaken for a dependency at least once, which is why this
section exists.
