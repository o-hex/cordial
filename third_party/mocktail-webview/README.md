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
