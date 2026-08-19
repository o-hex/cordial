//! What the shell has that is not only the shell's.
//!
//! `cordial-shell` is a binary — see `main.rs` — and everything about the
//! chooser, settings and shell configuration stays inside it. The one part
//! that had to become a library is [`host_window`]: ADR-011 says the shell's
//! window and the engine's host window are the same window, and there is no
//! way to honour that sentence with two crates each building their own.
//!
//! `cordial-runtime` depends on this crate for that module alone.
//!
//! [`profile`] is here for a related but not identical reason. It is the
//! launcher's, not the window's — but the launcher is what takes ADR-012's
//! claim on a profile, and `cordial_runtime::profile` already implements the
//! same contract in a crate this one cannot depend on without a cycle. Putting
//! it in the library half means the runtime can adopt this copy and delete its
//! own without the code moving twice. See that module's header.
//!
//! [`network`] and [`pvpn`] are here for the same shape of reason as
//! `profile`, deliberately placed rather than accidentally landing here: both
//! of Cordial's entry points — this crate's own `launch.rs`, and
//! `cordial-run` invoked directly, which AGENTS.md documents as fully
//! supported — need to refuse the same `vpn-required` profile the same way,
//! and `cordial-runtime` already depends on this crate for `host_window`, so
//! putting the gate here costs no new edge and needs no second copy.

pub mod host_window;
pub mod network;
pub mod profile;
pub mod pvpn;
// Not pulled in by `host_window` or `network` -- registered here on its own
// so `cordial-runtime` can reach it as `cordial_shell::refresh_watch`, which
// `refresh_watch.rs`'s own header names as the one thing left to do before
// its `watch` can be wired to the engine. Wiring `watch` itself onto the
// engine's own window is not done by this line alone -- see
// `crates/cordial-runtime/src/bin/load.rs`'s `wire_refresh_rate` for what
// that still needs and does not yet have.
pub mod refresh_watch;
// `webview_policy` needs nothing beyond `gtk4::glib::Uri`, so it is always
// compiled and always under test -- see its own header on why it is the part
// that has to be right. `webview` needs `webkitgtk6.0-devel`, which is the
// `webview` feature's whole reason for existing (see `Cargo.toml`).
//
// Both used to be `mod`, private to the `cordial-shell` binary, and nothing
// else in that binary ever called `webview::open`. That made the module dead
// code in the strongest sense: not merely unused but unreachable from
// `cordial-runtime`, which is the crate that actually receives Roblox's
// `openWindow` request (`crates/cordial-runtime/src/webview.rs`). Declaring
// both here, the way `host_window` already is, is what makes that crate able
// to call them at all -- `main.rs` no longer declares its own copies, and
// nothing there used to reference them either.
pub mod webview_policy;
#[cfg(feature = "webview")]
pub mod webview;
