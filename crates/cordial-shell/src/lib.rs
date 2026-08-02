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

pub mod host_window;
pub mod profile;
